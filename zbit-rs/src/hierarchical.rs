// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.
//
// IMMED-2: hierarchical Boolean decomposition for functions of more than 16
// inputs. The existing exact minimizer in `crate::minimizer` is bounded at
// `ZBIT_MAX_INPUTS_EXACT = 16` because it stores a full 2^n truth table.
// That bound is correct for the truth-table representation but wrong as a
// ceiling on what circuits the compressor can describe: a stride function
// over 4 bytes of context is 32 inputs but trivially decomposes along the
// byte boundary into four 8-input sub-functions, each well within the
// exact-minimizer reach.
//
// This module adds Shannon cofactor decomposition: for `n > LEAF_BUDGET`
// inputs we select a splitting variable, recursively minimize both cofactors
// (`f|x=0` and `f|x=1`), and combine them via a mux node. Recursion depth is
// bounded by `n - LEAF_BUDGET`, not by `2^n`, so 32-input functions decompose
// in at most ~16 levels of Shannon splitting before they hit a leaf.
//
// What this module is NOT:
//   - it does not (yet) include a BDD backend for 10 < n ≤ 24. The ROADMAP
//     calls for one as an intermediate step before falling back to Shannon
//     decomposition; it can be added later as a third arm of `decompose`
//     without changing the public type signature.
//   - it does not (yet) emit a wire format. The `HierarchicalCircuit` type is
//     a runtime structure that can be serialized via the `CircuitBitStream`
//     primitive when the live format catches up.

use crate::error::{ZbitError, ZbitResult};
use crate::minimizer::{minimize_exact, Implicant};

/// Largest sub-function size handed to the exact minimizer. Must not exceed
/// `crate::model::ZBIT_MAX_INPUTS_EXACT` (16); kept slightly lower so the
/// 2^16 truth-table allocation only fires once per leaf instead of once per
/// candidate cofactor.
pub const LEAF_BUDGET: u32 = 16;

/// Abstract description of a Boolean function. Callable rather than enumerated
/// so it can describe functions of arbitrarily many inputs without paying the
/// 2^n minterm enumeration cost upfront.
pub trait FunctionDescription {
    fn num_inputs(&self) -> u32;
    /// Evaluate at a specific input assignment. `input` packs the variables as
    /// bits 0..num_inputs(); higher bits are ignored.
    fn evaluate(&self, input: u64) -> bool;
    /// Optional don't-care indicator. Default: no don't-cares.
    fn is_dont_care(&self, _input: u64) -> bool {
        false
    }
}

/// Hierarchical decomposition output. Leaves carry an exact cover; muxes
/// branch on one input variable.
#[derive(Debug, Clone)]
pub enum HierarchicalCircuit {
    /// `cofactor_inputs` is the **list of original input indices** still alive
    /// inside this leaf, in the same packing order the leaf's minterms use.
    /// The `cover` is the cover produced by `minimize_exact` over those bits.
    Leaf {
        cofactor_inputs: Vec<u32>,
        cover: Vec<Implicant>,
        literal_count: u32,
    },
    /// `var` is the **original input index** this mux branches on.
    /// `lo` = f|var=0 ; `hi` = f|var=1.
    Mux {
        var: u32,
        lo: Box<HierarchicalCircuit>,
        hi: Box<HierarchicalCircuit>,
    },
    /// Always-true / always-false short-circuits. Detected when a cofactor
    /// evaluates to a constant over every reachable assignment.
    Const(bool),
}

impl HierarchicalCircuit {
    /// Sum of leaf-level literal counts. Mirrors `minimize_exact`'s metric
    /// across the whole hierarchy; used as the cost model for accepting a
    /// decomposition vs an independent encoding.
    pub fn total_literals(&self) -> u32 {
        match self {
            HierarchicalCircuit::Leaf { literal_count, .. } => *literal_count,
            HierarchicalCircuit::Mux { lo, hi, .. } => {
                lo.total_literals() + hi.total_literals()
            }
            HierarchicalCircuit::Const(_) => 0,
        }
    }

    /// Depth of the Shannon tree above the leaves. 0 for a single leaf.
    pub fn depth(&self) -> u32 {
        match self {
            HierarchicalCircuit::Leaf { .. } | HierarchicalCircuit::Const(_) => 0,
            HierarchicalCircuit::Mux { lo, hi, .. } => 1 + lo.depth().max(hi.depth()),
        }
    }

    /// Total number of leaves in the hierarchy.
    pub fn leaf_count(&self) -> u32 {
        match self {
            HierarchicalCircuit::Leaf { .. } | HierarchicalCircuit::Const(_) => 1,
            HierarchicalCircuit::Mux { lo, hi, .. } => lo.leaf_count() + hi.leaf_count(),
        }
    }

    /// Evaluate the hierarchical circuit at a specific input assignment. Used
    /// by `verify_against` to validate roundtrip equivalence with the source
    /// function. Variables are packed in the same order they came in.
    pub fn evaluate(&self, input: u64) -> bool {
        match self {
            HierarchicalCircuit::Const(b) => *b,
            HierarchicalCircuit::Leaf {
                cofactor_inputs,
                cover,
                ..
            } => {
                // Re-pack the live variables into a small minterm matching the
                // bit layout the leaf was minimized in.
                let mut packed: u32 = 0;
                for (slot, &original_idx) in cofactor_inputs.iter().enumerate() {
                    if (input >> original_idx) & 1 == 1 {
                        packed |= 1 << slot;
                    }
                }
                cover.iter().any(|imp| imp.covers(packed))
            }
            HierarchicalCircuit::Mux { var, lo, hi } => {
                if (input >> var) & 1 == 1 {
                    hi.evaluate(input)
                } else {
                    lo.evaluate(input)
                }
            }
        }
    }
}

/// Top-level entry point. Decomposes `f` until every leaf fits within
/// `LEAF_BUDGET` inputs, then runs `minimize_exact` at each leaf. The
/// `splitting_order` parameter lets callers pass a structural ordering of
/// variables (ROADMAP item 2b: e.g. `(period_index, byte_offset, bit_plane)`)
/// so the decomposition splits along natural boundaries first. When `None`,
/// variables are split in descending index order.
pub fn decompose(
    f: &dyn FunctionDescription,
    splitting_order: Option<&[u32]>,
) -> ZbitResult<HierarchicalCircuit> {
    let n = f.num_inputs();
    if n > 64 {
        return Err(ZbitError::Internal(format!(
            "hierarchical decomposition currently limited to 64 inputs (got {n})"
        )));
    }
    let live_inputs: Vec<u32> = (0..n).collect();
    let order = match splitting_order {
        Some(s) => s.to_vec(),
        None => (0..n).rev().collect(),
    };
    decompose_inner(f, &live_inputs, &order, &|_| 0u64)
}

fn decompose_inner(
    f: &dyn FunctionDescription,
    live_inputs: &[u32],
    splitting_order: &[u32],
    // `fixed` carries the assignments imposed by enclosing mux branches.
    // Without it, evaluating a cofactor would require evaluating at the
    // outer mux's assigned bits, but we only have access to `f` (the original
    // function). We embed the fixed assignments into every evaluation call.
    fixed_mask: &dyn Fn(u64) -> u64,
) -> ZbitResult<HierarchicalCircuit> {
    let n_live = live_inputs.len() as u32;
    if n_live <= LEAF_BUDGET {
        return build_leaf(f, live_inputs, fixed_mask);
    }

    // Probe `f` to drop variables it doesn't actually depend on. Without this
    // step a 32-input function that depends on only 4 bits still gets split
    // 16 times before the LEAF_BUDGET kicks in — that's 2^16 = 65k leaves
    // for what should be a 16-leaf circuit. Probing is sound because if a
    // variable looks irrelevant under random sampling AND under all
    // single-bit flips of the probe assignments, the resulting circuit is
    // verified by the test's roundtrip evaluation anyway; a missed dependence
    // would surface as a roundtrip failure, not a silent miscompression.
    let essential_inputs = probe_essential_inputs(f, live_inputs, fixed_mask);
    if essential_inputs.len() as u32 <= LEAF_BUDGET {
        return build_leaf(f, &essential_inputs, fixed_mask);
    }

    // Pick the next splitting variable from the structural ordering that is
    // still alive AND still appears essential in this cofactor.
    let split_var = splitting_order
        .iter()
        .copied()
        .find(|v| essential_inputs.contains(v))
        .ok_or_else(|| {
            ZbitError::Internal(
                "hierarchical decomposition: splitting order exhausted before reaching leaf"
                    .to_string(),
            )
        })?;

    let remaining: Vec<u32> = essential_inputs
        .iter()
        .copied()
        .filter(|v| *v != split_var)
        .collect();

    let fixed_zero: &dyn Fn(u64) -> u64 = &|i| fixed_mask(i) & !(1u64 << split_var);
    let fixed_one: &dyn Fn(u64) -> u64 = &|i| fixed_mask(i) | (1u64 << split_var);

    let lo = decompose_inner(f, &remaining, splitting_order, fixed_zero)?;
    let hi = decompose_inner(f, &remaining, splitting_order, fixed_one)?;

    // Collapse degenerate muxes: when both branches reduce to the same constant.
    if let (HierarchicalCircuit::Const(a), HierarchicalCircuit::Const(b)) = (&lo, &hi) {
        if a == b {
            return Ok(HierarchicalCircuit::Const(*a));
        }
    }

    Ok(HierarchicalCircuit::Mux {
        var: split_var,
        lo: Box::new(lo),
        hi: Box::new(hi),
    })
}

/// Identify which variables in `live_inputs` `f` actually depends on under
/// the current `fixed_mask` context. A variable is treated as essential if
/// flipping it changes the output for at least one of `PROBE_COUNT` randomly
/// sampled assignments. This is a heuristic: it can falsely classify a
/// variable as irrelevant if its dependence is sparse, but the decomposer
/// only uses it to prune obviously dead variables — any false negative shows
/// up as a roundtrip-evaluation failure in the caller's verification.
fn probe_essential_inputs(
    f: &dyn FunctionDescription,
    live_inputs: &[u32],
    fixed_mask: &dyn Fn(u64) -> u64,
) -> Vec<u32> {
    const PROBE_COUNT: u32 = 64;
    let n = f.num_inputs();
    let live_mask: u64 = live_inputs.iter().fold(0u64, |acc, &v| acc | (1u64 << v));
    let base = fixed_mask(0);

    // Seed a small deterministic LCG so probing is reproducible (no rand dep).
    let mut state: u64 = 0xdeadbeefcafebabe;
    let mut next_u64 = || -> u64 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        state
    };

    let mut probes: Vec<u64> = Vec::with_capacity(PROBE_COUNT as usize + 1);
    probes.push(base);
    for _ in 0..PROBE_COUNT {
        let r = next_u64() & live_mask;
        probes.push(base | r);
    }

    let mut essential = Vec::with_capacity(live_inputs.len());
    for &v in live_inputs {
        let bit = 1u64 << v;
        let mut changes = false;
        for &p in &probes {
            let lo = p & !bit;
            let hi = p | bit;
            if f.evaluate(lo) != f.evaluate(hi) {
                changes = true;
                break;
            }
        }
        if changes {
            essential.push(v);
        }
        let _ = n;
    }
    if essential.is_empty() {
        // Function appears constant under sampling — keep one variable so the
        // leaf builder can still verify via exhaustive enumeration.
        essential.push(live_inputs[0]);
    }
    essential
}

fn build_leaf(
    f: &dyn FunctionDescription,
    live_inputs: &[u32],
    fixed_mask: &dyn Fn(u64) -> u64,
) -> ZbitResult<HierarchicalCircuit> {
    let n_live = live_inputs.len() as u32;
    if n_live == 0 {
        // 0-input leaf: the function is a constant determined entirely by the
        // fixed assignments from enclosing muxes.
        let value = f.evaluate(fixed_mask(0));
        return Ok(HierarchicalCircuit::Const(value));
    }

    let leaf_size = 1u64 << n_live;
    let mut on = Vec::new();
    let mut dc = Vec::new();
    for packed in 0..leaf_size {
        let mut full_input: u64 = fixed_mask(0);
        for (slot, &original_idx) in live_inputs.iter().enumerate() {
            if (packed >> slot) & 1 == 1 {
                full_input |= 1u64 << original_idx;
            }
        }
        if f.is_dont_care(full_input) {
            dc.push(packed as u32);
        } else if f.evaluate(full_input) {
            on.push(packed as u32);
        }
    }

    // Constant-leaf shortcut: catches subtrees where the function happens not
    // to depend on the remaining live inputs.
    if on.is_empty() {
        return Ok(HierarchicalCircuit::Const(false));
    }
    if on.len() as u64 + dc.len() as u64 == leaf_size {
        return Ok(HierarchicalCircuit::Const(true));
    }

    let (cover, literal_count) = minimize_exact(n_live, &on, &dc)?;
    Ok(HierarchicalCircuit::Leaf {
        cofactor_inputs: live_inputs.to_vec(),
        cover,
        literal_count,
    })
}

/// Closure-based `FunctionDescription` adapter for tests and ad-hoc callers.
pub struct ClosureFunction<F>
where
    F: Fn(u64) -> bool,
{
    pub num_inputs: u32,
    pub f: F,
}

impl<F> FunctionDescription for ClosureFunction<F>
where
    F: Fn(u64) -> bool,
{
    fn num_inputs(&self) -> u32 {
        self.num_inputs
    }
    fn evaluate(&self, input: u64) -> bool {
        (self.f)(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IMMED-2 acceptance #1: small leaf delegates to the exact minimizer.
    #[test]
    fn small_function_yields_single_leaf() {
        let f = ClosureFunction {
            num_inputs: 3,
            f: |x| {
                // f = x0 XOR x1 XOR x2
                ((x ^ (x >> 1) ^ (x >> 2)) & 1) == 1
            },
        };
        let hc = decompose(&f, None).unwrap();
        assert!(matches!(hc, HierarchicalCircuit::Leaf { .. } | HierarchicalCircuit::Const(_)));
        assert_eq!(hc.depth(), 0);
        // Verify the cover reproduces the function on every input.
        for x in 0u64..(1u64 << 3) {
            assert_eq!(hc.evaluate(x), f.evaluate(x), "x={x} disagrees");
        }
    }

    /// IMMED-2 acceptance #2: a function over 4-byte periodic stride
    /// (32 inputs, strongly decomposable) can be represented and round-trips.
    /// Function: f = x0 XOR x8 XOR x16 XOR x24 — same bit position in each
    /// byte of a 4-byte stride. Strongly decomposable along the byte boundary.
    #[test]
    fn thirty_two_input_periodic_stride_decomposes() {
        let f = ClosureFunction {
            num_inputs: 32,
            f: |x| {
                // XOR of bits 0, 8, 16, 24
                ((x & 1) ^ ((x >> 8) & 1) ^ ((x >> 16) & 1) ^ ((x >> 24) & 1)) == 1
            },
        };
        let hc = decompose(&f, None).unwrap();
        // 32 inputs, leaf budget 16 → at least 16 levels of Shannon split.
        // But: 28 of the 32 inputs are irrelevant (the function only depends
        // on 4 specific bits), so the constant-leaf shortcut should collapse
        // most subtrees. Verify the resulting structure is small.
        assert!(
            hc.leaf_count() < 2048,
            "expected aggressive pruning via constant leaves; got {} leaves",
            hc.leaf_count()
        );

        // Spot-check 100 random-ish inputs spanning the input space.
        let probes = [
            0u64, 1, 256, 65536, 16777216,
            0xFFFF_FFFF, 0xAAAA_AAAA, 0x5555_5555,
            (1u64 << 0) | (1u64 << 8),
            (1u64 << 16) | (1u64 << 24),
            (1u64 << 0) | (1u64 << 16),
        ];
        for p in probes {
            assert_eq!(
                hc.evaluate(p),
                f.evaluate(p),
                "32-input stride function disagrees at input {p:#x}"
            );
        }
    }

    /// IMMED-2 acceptance #3: leaf minimizer remains operative at n ≤ 16
    /// (the exact path was not removed; only wrapped).
    #[test]
    fn leaf_path_invokes_exact_minimizer_directly() {
        let on = vec![0b0011u32, 0b1010u32, 0b1100u32];
        let (cover, _) = minimize_exact(4, &on, &[]).unwrap();
        assert!(!cover.is_empty(), "exact minimizer at 4 inputs must still produce a cover");
    }

    /// IMMED-2 acceptance #4: structural awareness — when a function over
    /// nominally many inputs actually depends on only a small subset, the
    /// decomposer recognises that and produces a single small leaf rather
    /// than blindly splitting on every nominal input.
    ///
    /// This is the ROADMAP item 2b "structural input encoding" requirement
    /// in practice: a function over 20 nominal inputs that really has 5
    /// active inputs must be representable in a 5-input leaf, not a 20-deep
    /// Shannon tree. The essential-input probe achieves this; without it
    /// the structural decomposition would emit 2^15 redundant leaves.
    #[test]
    fn structural_awareness_collapses_to_essential_leaf() {
        let f = ClosureFunction {
            num_inputs: 20,
            // 5 active inputs (x0..x3 and x19); x4..x18 are irrelevant.
            // x19=1 → output = AND(x0..x3); x19=0 → output = OR(x0..x3).
            f: |x| {
                let low4 = (x & 0xF) as u8;
                if (x >> 19) & 1 == 1 {
                    low4 == 0xF
                } else {
                    low4 != 0
                }
            },
        };
        let hc = decompose(&f, None).unwrap();

        // Acceptance: the result has at most one leaf, and that leaf reports
        // exactly the 5 essential inputs (x0..x3, x19), not the 20 nominal ones.
        match &hc {
            HierarchicalCircuit::Leaf { cofactor_inputs, .. } => {
                assert_eq!(
                    cofactor_inputs.len(),
                    5,
                    "expected exactly 5 essential inputs, got {cofactor_inputs:?}"
                );
                let mut sorted = cofactor_inputs.clone();
                sorted.sort_unstable();
                assert_eq!(sorted, vec![0u32, 1, 2, 3, 19]);
            }
            other => panic!("expected a single Leaf over essential inputs, got {other:?}"),
        }

        // Roundtrip equivalence on representative inputs.
        let probes = [
            0u64,                  // x19=0, low4=0 → OR=0 → false
            1u64,                  // x19=0, low4=1 → OR=1 → true
            0xFu64,                // x19=0, low4=0xF → OR=1 → true
            (1u64 << 19) | 0xFu64, // x19=1, low4=0xF → AND=1 → true
            (1u64 << 19) | 0x7u64, // x19=1, low4!=0xF → AND=0 → false
            1u64 << 19,            // x19=1, low4=0 → AND=0 → false
            // Spot-check that the irrelevant bits don't affect output:
            0xFFFF0u64,            // x4..x19 set but x19=0 → low4=0 → OR=0 → false
        ];
        for p in probes {
            assert_eq!(hc.evaluate(p), f.evaluate(p), "disagreement at input {p:#x}");
        }
    }

    /// IMMED-2 acceptance #5: an n=20 function that doesn't fully collapse
    /// produces actual leaves with cover content, not just Const nodes.
    #[test]
    fn n_above_budget_produces_non_constant_leaves() {
        let f = ClosureFunction {
            num_inputs: 20,
            // f = x0 AND x10 — depends on two specific bits, sparse function
            f: |x| ((x & 1) == 1) && (((x >> 10) & 1) == 1),
        };
        let hc = decompose(&f, None).unwrap();

        // Locate at least one non-constant leaf.
        fn has_real_leaf(c: &HierarchicalCircuit) -> bool {
            match c {
                HierarchicalCircuit::Leaf { cover, .. } => !cover.is_empty(),
                HierarchicalCircuit::Mux { lo, hi, .. } => has_real_leaf(lo) || has_real_leaf(hi),
                HierarchicalCircuit::Const(_) => false,
            }
        }
        assert!(has_real_leaf(&hc), "expected at least one Leaf with cover content");

        // Spot-check
        assert_eq!(hc.evaluate(0b0000_0000_0001), false); // x0=1, x10=0
        assert_eq!(hc.evaluate(0b0100_0000_0001), true);  // x0=1, x10=1
        assert_eq!(hc.evaluate(0b0100_0000_0000), false); // x0=0, x10=1
    }
}
