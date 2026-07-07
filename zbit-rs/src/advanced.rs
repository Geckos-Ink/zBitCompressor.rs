// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::collections::HashSet;

use crate::error::{ZbitError, ZbitResult};
use crate::minimizer::{minimize_exact, Implicant};
use crate::sat::{is_satisfiable, Cnf};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MappingObjective {
    LiteralCount,
    AsicArea,
    AsicDelay,
    FpgaLut4,
    FpgaLut6,
}

#[derive(Debug, Clone)]
pub struct AdvancedOptions {
    pub exact_seed_max_inputs: u32,
    pub espresso_rounds: usize,
    pub max_expand_free_bits: u32,

    pub enable_aig_rewrite: bool,
    pub enable_balancing: bool,
    pub enable_resubstitution: bool,

    pub sat_local_exact_inputs: u32,
    pub objective: MappingObjective,
}

impl Default for AdvancedOptions {
    fn default() -> Self {
        Self {
            exact_seed_max_inputs: 16,
            espresso_rounds: 4,
            max_expand_free_bits: 14,
            enable_aig_rewrite: true,
            enable_balancing: true,
            enable_resubstitution: true,
            sat_local_exact_inputs: 12,
            objective: MappingObjective::LiteralCount,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ObjectiveEstimate {
    pub implicant_count: u32,
    pub literal_count: u32,

    pub estimated_and2: u32,
    pub estimated_not: u32,
    pub estimated_levels: u32,
    pub estimated_luts: u32,

    pub weighted_cost: f64,
}

#[derive(Debug, Clone)]
pub struct AdvancedReport {
    pub used_exact_seed: bool,
    pub used_espresso: bool,
    pub espresso_rounds_run: usize,

    pub aig_merge_count: u32,
    pub resubstitution_removed: u32,
    pub sat_pruned_terms: u32,

    pub objective: MappingObjective,
    pub selected: ObjectiveEstimate,

    pub exact_seed_score: Option<f64>,
    pub heuristic_score: f64,
}

impl Default for AdvancedReport {
    fn default() -> Self {
        Self {
            used_exact_seed: false,
            used_espresso: false,
            espresso_rounds_run: 0,
            aig_merge_count: 0,
            resubstitution_removed: 0,
            sat_pruned_terms: 0,
            objective: MappingObjective::LiteralCount,
            selected: ObjectiveEstimate::default(),
            exact_seed_score: None,
            heuristic_score: f64::INFINITY,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdvancedMinimization {
    pub implicants: Vec<Implicant>,
    pub literal_count: u32,
    pub report: AdvancedReport,
}

#[derive(Debug, Clone, Copy, Default)]
struct FlowStats {
    aig_merge_count: u32,
    resubstitution_removed: u32,
    sat_pruned_terms: u32,
}

fn dedup_implicants(values: &mut Vec<Implicant>) {
    values.sort_unstable_by_key(|i| (i.mask, i.value));
    values.dedup();
}

fn canonicalize_sets(on_set: &[u32], dc_set: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut on = on_set.to_vec();
    on.sort_unstable();
    on.dedup();

    let mut dc = dc_set.to_vec();
    dc.sort_unstable();
    dc.dedup();

    let on_bits = on.iter().copied().collect::<HashSet<_>>();
    dc.retain(|m| !on_bits.contains(m));

    (on, dc)
}

fn cube_covers_cube(a: Implicant, b: Implicant) -> bool {
    ((a.mask & b.mask) == a.mask) && ((b.value & a.mask) == a.value)
}

fn ceil_log2(mut value: u32) -> u32 {
    if value <= 1 {
        return 0;
    }

    value -= 1;
    let mut n = 0;
    while value > 0 {
        value >>= 1;
        n += 1;
    }
    n
}

fn ceil_div(lhs: u32, rhs: u32) -> u32 {
    (lhs + rhs - 1) / rhs
}

fn cube_weight(cube: Implicant, objective: MappingObjective) -> f64 {
    let literals = cube.literal_count().max(1) as f64;
    match objective {
        MappingObjective::LiteralCount => literals,
        MappingObjective::AsicArea => 0.8 + literals,
        MappingObjective::AsicDelay => (ceil_log2(cube.literal_count().max(1)) + 1) as f64,
        MappingObjective::FpgaLut4 => ceil_div(cube.literal_count().max(1), 4) as f64,
        MappingObjective::FpgaLut6 => ceil_div(cube.literal_count().max(1), 6) as f64,
    }
}

pub fn evaluate_cover_objective(
    cover: &[Implicant],
    objective: MappingObjective,
    balanced: bool,
) -> ObjectiveEstimate {
    let implicant_count = cover.len() as u32;
    let literal_count = cover.iter().map(|c| c.literal_count()).sum::<u32>();

    let mut neg_inputs = HashSet::new();
    let mut and2 = 0u32;
    let mut max_term_levels = 0u32;

    for cube in cover {
        let lits = cube.literal_count();
        if lits > 1 {
            and2 += lits - 1;
        }

        let term_levels = if balanced {
            ceil_log2(lits.max(1)) + 1
        } else {
            lits.max(1)
        };
        max_term_levels = max_term_levels.max(term_levels);

        let mut mask = cube.mask;
        while mask != 0 {
            let bit = mask.trailing_zeros();
            let flag = 1u32 << bit;
            if (cube.value & flag) == 0 {
                neg_inputs.insert(bit);
            }
            mask &= !flag;
        }
    }

    if implicant_count > 1 {
        and2 += implicant_count - 1;
    }

    let output_levels = if implicant_count <= 1 {
        max_term_levels
    } else if balanced {
        max_term_levels + ceil_log2(implicant_count)
    } else {
        max_term_levels + implicant_count - 1
    };

    let lut4_terms = cover
        .iter()
        .map(|c| ceil_div(c.literal_count().max(1), 4))
        .sum::<u32>();
    let lut6_terms = cover
        .iter()
        .map(|c| ceil_div(c.literal_count().max(1), 6))
        .sum::<u32>();

    let lut4 = if implicant_count <= 1 {
        lut4_terms
    } else {
        lut4_terms + ceil_div(implicant_count, 4)
    };
    let lut6 = if implicant_count <= 1 {
        lut6_terms
    } else {
        lut6_terms + ceil_div(implicant_count, 6)
    };

    let weighted_cost = match objective {
        MappingObjective::LiteralCount => literal_count as f64,
        MappingObjective::AsicArea => and2 as f64 + (neg_inputs.len() as f64) * 0.2,
        MappingObjective::AsicDelay => output_levels as f64 + (and2 as f64) * 0.01,
        MappingObjective::FpgaLut4 => lut4 as f64 + (output_levels as f64) * 0.05,
        MappingObjective::FpgaLut6 => lut6 as f64 + (output_levels as f64) * 0.05,
    };

    ObjectiveEstimate {
        implicant_count,
        literal_count,
        estimated_and2: and2,
        estimated_not: neg_inputs.len() as u32,
        estimated_levels: output_levels,
        estimated_luts: match objective {
            MappingObjective::FpgaLut4 => lut4,
            MappingObjective::FpgaLut6 => lut6,
            _ => lut6,
        },
        weighted_cost,
    }
}

fn estimate_better(lhs: &ObjectiveEstimate, rhs: &ObjectiveEstimate) -> bool {
    const EPS: f64 = 1e-9;

    if lhs.weighted_cost + EPS < rhs.weighted_cost {
        return true;
    }
    if rhs.weighted_cost + EPS < lhs.weighted_cost {
        return false;
    }

    if lhs.literal_count != rhs.literal_count {
        return lhs.literal_count < rhs.literal_count;
    }

    lhs.implicant_count < rhs.implicant_count
}

fn cube_only_hits_allowed(
    cube: Implicant,
    num_inputs: u32,
    allowed_set: &HashSet<u32>,
    max_free_bits: u32,
) -> bool {
    let free_bits = num_inputs.saturating_sub(cube.mask.count_ones());
    if free_bits > max_free_bits {
        return false;
    }

    let mut open_bits = Vec::new();
    for bit in 0..num_inputs {
        let flag = 1u32 << bit;
        if (cube.mask & flag) == 0 {
            open_bits.push(bit);
        }
    }

    let total = 1u64 << open_bits.len();
    let base = cube.value & cube.mask;

    for state in 0..total {
        let mut minterm = base;
        for (idx, bit) in open_bits.iter().enumerate() {
            if ((state >> idx) & 1) != 0 {
                minterm |= 1u32 << bit;
            }
        }

        if !allowed_set.contains(&minterm) {
            return false;
        }
    }

    true
}

fn greedy_cover(
    pool: &[Implicant],
    on_set: &[u32],
    objective: MappingObjective,
) -> ZbitResult<Vec<Implicant>> {
    if on_set.is_empty() {
        return Ok(Vec::new());
    }
    if pool.is_empty() {
        return Err(ZbitError::Internal(
            "heuristic pool is empty for non-empty ON-set".to_string(),
        ));
    }

    let words = (on_set.len() + 63) / 64;
    let mut rows = Vec::with_capacity(pool.len());

    for cube in pool {
        let mut row = vec![0u64; words];
        for (idx, &m) in on_set.iter().enumerate() {
            if cube.covers(m) {
                row[idx >> 6] |= 1u64 << (idx & 63);
            }
        }
        rows.push(row);
    }

    let mut covered = vec![0u64; words];
    let mut selected = vec![false; pool.len()];
    let mut output = Vec::new();

    loop {
        let mut all_covered = true;
        for (idx, _) in on_set.iter().enumerate() {
            let bit = (covered[idx >> 6] >> (idx & 63)) & 1;
            if bit == 0 {
                all_covered = false;
                break;
            }
        }

        if all_covered {
            break;
        }

        let mut best_idx = None;
        let mut best_merit = 0.0f64;
        let mut best_gain = 0u32;

        for (idx, row) in rows.iter().enumerate() {
            if selected[idx] {
                continue;
            }

            let gain = covered
                .iter()
                .zip(row.iter())
                .map(|(c, r)| (r & !c).count_ones())
                .sum::<u32>();
            if gain == 0 {
                continue;
            }

            let merit = gain as f64 / cube_weight(pool[idx], objective).max(0.001);
            if merit > best_merit || (merit == best_merit && gain > best_gain) {
                best_merit = merit;
                best_gain = gain;
                best_idx = Some(idx);
            }
        }

        let Some(chosen) = best_idx else {
            return Err(ZbitError::Internal(
                "failed to cover ON-set in heuristic selection".to_string(),
            ));
        };

        selected[chosen] = true;
        output.push(pool[chosen]);

        for (dst, src) in covered.iter_mut().zip(rows[chosen].iter()) {
            *dst |= *src;
        }
    }

    let mut i = 0usize;
    while i < output.len() {
        let mut trial = output.clone();
        trial.remove(i);

        let still_covers = on_set
            .iter()
            .all(|&m| trial.iter().any(|cube| cube.covers(m)));

        if still_covers {
            output = trial;
        } else {
            i += 1;
        }
    }

    dedup_implicants(&mut output);
    Ok(output)
}

fn remove_absorbed_terms(cover: &mut Vec<Implicant>) -> u32 {
    dedup_implicants(cover);
    let mut keep = vec![true; cover.len()];

    for i in 0..cover.len() {
        if !keep[i] {
            continue;
        }

        for j in 0..cover.len() {
            if i == j || !keep[j] {
                continue;
            }

            if cube_covers_cube(cover[i], cover[j]) {
                keep[j] = false;
            }
        }
    }

    let before = cover.len();
    let kept = cover
        .iter()
        .zip(keep.iter())
        .filter_map(|(cube, keep_flag)| if *keep_flag { Some(*cube) } else { None })
        .collect::<Vec<_>>();

    *cover = kept;
    (before - cover.len()) as u32
}

fn generate_consensus_merges(
    cover: &[Implicant],
    num_inputs: u32,
    allowed_set: &HashSet<u32>,
    max_free_bits: u32,
) -> Vec<Implicant> {
    let mut out = Vec::new();

    for i in 0..cover.len() {
        for j in (i + 1)..cover.len() {
            let a = cover[i];
            let b = cover[j];

            if a.mask != b.mask {
                continue;
            }

            let diff = (a.value ^ b.value) & a.mask;
            if diff == 0 || !diff.is_power_of_two() {
                continue;
            }

            let merged = Implicant {
                value: a.value & !diff,
                mask: a.mask & !diff,
            };

            if cube_only_hits_allowed(merged, num_inputs, allowed_set, max_free_bits) {
                out.push(merged);
            }
        }
    }

    dedup_implicants(&mut out);
    out
}

fn run_espresso_heuristic(
    num_inputs: u32,
    on_set: &[u32],
    allowed_set: &HashSet<u32>,
    options: &AdvancedOptions,
) -> ZbitResult<(Vec<Implicant>, usize)> {
    let full_mask = if num_inputs == 0 {
        0
    } else {
        (1u32 << num_inputs) - 1
    };

    let mut cover = on_set
        .iter()
        .map(|&m| Implicant {
            value: m & full_mask,
            mask: full_mask,
        })
        .collect::<Vec<_>>();

    dedup_implicants(&mut cover);

    let mut rounds_run = 0usize;
    let mut best = evaluate_cover_objective(&cover, options.objective, options.enable_balancing);

    for _ in 0..options.espresso_rounds {
        rounds_run += 1;

        let mut expanded = Vec::with_capacity(cover.len());
        for cube in &cover {
            let mut current = *cube;

            loop {
                let mut improved = None;

                let mut mask_scan = current.mask;
                while mask_scan != 0 {
                    let bit = mask_scan.trailing_zeros();
                    let flag = 1u32 << bit;
                    mask_scan &= !flag;

                    let candidate = Implicant {
                        value: current.value & !flag,
                        mask: current.mask & !flag,
                    };

                    if !cube_only_hits_allowed(
                        candidate,
                        num_inputs,
                        allowed_set,
                        options.max_expand_free_bits,
                    ) {
                        continue;
                    }

                    let candidate_lits = candidate.literal_count();
                    if candidate_lits < current.literal_count() {
                        improved = Some(candidate);
                        break;
                    }
                }

                match improved {
                    Some(next) => current = next,
                    None => break,
                }
            }

            expanded.push(current);
        }

        let mut pool = cover.clone();
        pool.extend(expanded);

        if options.enable_aig_rewrite {
            let merges = generate_consensus_merges(
                &pool,
                num_inputs,
                allowed_set,
                options.max_expand_free_bits,
            );
            pool.extend(merges);
        }

        dedup_implicants(&mut pool);
        if options.enable_resubstitution {
            let _ = remove_absorbed_terms(&mut pool);
        }

        let next = greedy_cover(&pool, on_set, options.objective)?;
        let next_score =
            evaluate_cover_objective(&next, options.objective, options.enable_balancing);

        if estimate_better(&next_score, &best) {
            cover = next;
            best = next_score;
        }
    }

    Ok((cover, rounds_run))
}

fn cube_false_clause(cube: Implicant, num_inputs: u32) -> Vec<i32> {
    if cube.mask == 0 {
        return Vec::new();
    }

    let mut clause = Vec::new();
    for bit in 0..num_inputs {
        let flag = 1u32 << bit;
        if (cube.mask & flag) == 0 {
            continue;
        }

        let var = bit as i32 + 1;
        if (cube.value & flag) != 0 {
            clause.push(-var);
        } else {
            clause.push(var);
        }
    }

    clause
}

fn forbid_assignment_clause(minterm: u32, num_inputs: u32) -> Vec<i32> {
    let mut clause = Vec::with_capacity(num_inputs as usize);
    for bit in 0..num_inputs {
        let var = bit as i32 + 1;
        if ((minterm >> bit) & 1) != 0 {
            clause.push(-var);
        } else {
            clause.push(var);
        }
    }
    clause
}

fn sat_term_is_redundant(idx: usize, cover: &[Implicant], num_inputs: u32, dc_set: &[u32]) -> bool {
    let mut cnf = Cnf::new(num_inputs as usize);

    let target = cover[idx];
    for bit in 0..num_inputs {
        let flag = 1u32 << bit;
        if (target.mask & flag) == 0 {
            continue;
        }

        let var = bit as i32 + 1;
        if (target.value & flag) != 0 {
            cnf.push_clause(vec![var]);
        } else {
            cnf.push_clause(vec![-var]);
        }
    }

    for (other_idx, cube) in cover.iter().enumerate() {
        if other_idx == idx {
            continue;
        }

        cnf.push_clause(cube_false_clause(*cube, num_inputs));
    }

    for &dc in dc_set {
        cnf.push_clause(forbid_assignment_clause(dc, num_inputs));
    }

    !is_satisfiable(&cnf)
}

fn sat_prune_redundant_terms(
    cover: &mut Vec<Implicant>,
    num_inputs: u32,
    dc_set: &[u32],
    sat_local_exact_inputs: u32,
) -> u32 {
    if num_inputs == 0 || num_inputs > sat_local_exact_inputs {
        return 0;
    }

    let mut removed = 0u32;
    let mut idx = 0usize;

    while idx < cover.len() {
        if sat_term_is_redundant(idx, cover, num_inputs, dc_set) {
            cover.remove(idx);
            removed += 1;
        } else {
            idx += 1;
        }
    }

    removed
}

fn run_rewrite_flows(
    mut cover: Vec<Implicant>,
    num_inputs: u32,
    on_set: &[u32],
    dc_set: &[u32],
    allowed_set: &HashSet<u32>,
    options: &AdvancedOptions,
) -> ZbitResult<(Vec<Implicant>, FlowStats)> {
    dedup_implicants(&mut cover);

    let mut stats = FlowStats::default();

    if options.enable_resubstitution {
        stats.resubstitution_removed += remove_absorbed_terms(&mut cover);
    }

    if options.enable_aig_rewrite {
        let merges = generate_consensus_merges(
            &cover,
            num_inputs,
            allowed_set,
            options.max_expand_free_bits,
        );

        if !merges.is_empty() {
            let mut pool = cover.clone();
            pool.extend(merges.iter().copied());
            dedup_implicants(&mut pool);

            if options.enable_resubstitution {
                stats.resubstitution_removed += remove_absorbed_terms(&mut pool);
            }

            let candidate = greedy_cover(&pool, on_set, options.objective)?;
            let cand_score =
                evaluate_cover_objective(&candidate, options.objective, options.enable_balancing);
            let old_score =
                evaluate_cover_objective(&cover, options.objective, options.enable_balancing);

            if estimate_better(&cand_score, &old_score) {
                cover = candidate;
                stats.aig_merge_count += merges.len() as u32;
            }
        }
    }

    stats.sat_pruned_terms += sat_prune_redundant_terms(
        &mut cover,
        num_inputs,
        dc_set,
        options.sat_local_exact_inputs,
    );

    if options.enable_balancing {
        cover.sort_unstable_by_key(|i| std::cmp::Reverse(i.literal_count()));
    }

    dedup_implicants(&mut cover);
    Ok((cover, stats))
}

pub fn minimize_advanced(
    num_inputs: u32,
    on_set: &[u32],
    dc_set: &[u32],
    options: &AdvancedOptions,
) -> ZbitResult<AdvancedMinimization> {
    let (on, dc) = canonicalize_sets(on_set, dc_set);

    if on.is_empty() {
        return Ok(AdvancedMinimization {
            implicants: Vec::new(),
            literal_count: 0,
            report: AdvancedReport {
                objective: options.objective,
                used_espresso: true,
                ..AdvancedReport::default()
            },
        });
    }

    let allowed_set = on
        .iter()
        .copied()
        .chain(dc.iter().copied())
        .collect::<HashSet<_>>();

    let (heuristic_cover, rounds_run) =
        run_espresso_heuristic(num_inputs, &on, &allowed_set, options)?;
    let (heuristic_cover, heuristic_stats) =
        run_rewrite_flows(heuristic_cover, num_inputs, &on, &dc, &allowed_set, options)?;

    let heuristic_est = evaluate_cover_objective(
        &heuristic_cover,
        options.objective,
        options.enable_balancing,
    );

    let mut report = AdvancedReport {
        used_exact_seed: false,
        used_espresso: true,
        espresso_rounds_run: rounds_run,
        aig_merge_count: heuristic_stats.aig_merge_count,
        resubstitution_removed: heuristic_stats.resubstitution_removed,
        sat_pruned_terms: heuristic_stats.sat_pruned_terms,
        objective: options.objective,
        selected: heuristic_est.clone(),
        exact_seed_score: None,
        heuristic_score: heuristic_est.weighted_cost,
    };

    let mut chosen_cover = heuristic_cover;
    let mut chosen_est = heuristic_est;

    if num_inputs <= options.exact_seed_max_inputs {
        let (exact_cover, _) = minimize_exact(num_inputs, &on, &dc)?;
        let (exact_cover, exact_stats) =
            run_rewrite_flows(exact_cover, num_inputs, &on, &dc, &allowed_set, options)?;

        let exact_est =
            evaluate_cover_objective(&exact_cover, options.objective, options.enable_balancing);
        report.used_exact_seed = true;
        report.exact_seed_score = Some(exact_est.weighted_cost);

        if estimate_better(&exact_est, &chosen_est) {
            chosen_cover = exact_cover;
            chosen_est = exact_est;
            report.aig_merge_count = exact_stats.aig_merge_count;
            report.resubstitution_removed = exact_stats.resubstitution_removed;
            report.sat_pruned_terms = exact_stats.sat_pruned_terms;
        }
    }

    if !on
        .iter()
        .all(|&m| chosen_cover.iter().any(|cube| cube.covers(m)))
    {
        return Err(ZbitError::Internal(
            "advanced minimizer produced invalid ON-set cover".to_string(),
        ));
    }

    report.selected = chosen_est.clone();

    Ok(AdvancedMinimization {
        implicants: chosen_cover,
        literal_count: chosen_est.literal_count,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imp(value: u32, mask: u32) -> Implicant {
        Implicant { value, mask }
    }

    #[test]
    fn resubstitution_absorbs_redundant_terms() {
        let mut cover = vec![imp(0b10, 0b10), imp(0b10, 0b11)];
        let removed = remove_absorbed_terms(&mut cover);

        assert_eq!(removed, 1);
        assert_eq!(cover, vec![imp(0b10, 0b10)]);
    }

    #[test]
    fn sat_prunes_redundant_terms() {
        let mut cover = vec![imp(0b10, 0b10), imp(0b11, 0b11)];
        let removed = sat_prune_redundant_terms(&mut cover, 2, &[], 4);

        assert_eq!(removed, 1);
        assert_eq!(cover, vec![imp(0b10, 0b10)]);
    }

    #[test]
    fn espresso_style_heuristic_generalizes_with_dont_cares() {
        let on = [1u32, 3u32];
        let dc = [0u32, 2u32];

        let options = AdvancedOptions {
            exact_seed_max_inputs: 0,
            objective: MappingObjective::LiteralCount,
            ..AdvancedOptions::default()
        };

        let result = minimize_advanced(2, &on, &dc, &options).expect("advanced minimize");

        assert_eq!(result.implicants.len(), 1);
        assert_eq!(result.literal_count, 0);
        assert_eq!(result.implicants[0], imp(0b00, 0b00));
    }

    #[test]
    fn fpga_objective_reports_lut_metrics() {
        let cover = vec![
            imp(0b0001, 0b1111),
            imp(0b0010, 0b1111),
            imp(0b0100, 0b1111),
        ];
        let estimate = evaluate_cover_objective(&cover, MappingObjective::FpgaLut4, true);

        assert!(estimate.estimated_luts > 0);
        assert!(estimate.weighted_cost > 0.0);
    }
}
