# zBit Circuit-Based Compression Roadmap

_Last updated: 2026-07-02_

## Honest Note on Dictionary Compaction Limits (v3/v4 retrospective)

After landing the v3 format with bit-packed everything — topology
nodes, framed dict, recursive-circuit fixed section, multi-block plan dictionary,
adaptive-transformed-xz dict, monotonic-delta dict — the **total dictionary footprint
on cat is now ~110 bytes out of 2 670 567 byte compressed output**, i.e. ~0.004 %.
Further header-level bit-packing cannot meaningfully change the ratio. The XZ-compressed
payload (and the CABAC-coded correction stream for preflate paths) is >99.99 % of every
file we ship.

The v4 bump is deliberately not another dictionary-compaction round. It spends one reserved
4-bit method slot on `raw-brotli`, a no-dictionary Brotli q11 payload for bounded text-like
inputs. This is the kind of secondary high-performance bitstream codec that can move a
payload when the data profile actually matches it: the paper corpus improves from raw-xz
`20 561` bytes (`0.331549`) to raw-brotli `18 573` bytes (`0.299492`). The same outer-
compression idea was probed on existing `.zbpk` artifacts: zstd/xz/brotli all lost on
paper and primary; zstd saved only 58 bytes on the 83 MB depth artifact after wrapper
overhead would leave a sub-0.0001% gain. Keep outer repacking as a future option only when
it has a real corpus-level win.

Concretely, the per-corpus ceiling for dictionary compaction is:

| Corpus | Compressed file | Dictionary footprint | Hard ceiling for further header work |
|---|---:|---:|---:|
| paper | 18 573 B | ~17 B header, no method dict | < 17 B (4-byte magic, 2-byte version, ...) |
| primary.3b | 562 799 B | ~17 B header + ~12 B monotonic-delta dict | ~10 B saveable, ~0.002 % ratio |
| cat | 2 670 567 B | ~17 B header + ~25 B recursive fixed + ~15 B framed + ~13 B topology bits | ~50 B saveable, ~0.002 % ratio |
| depth_anything | ~83 MB | ~17 B header + ~10 B adaptive-xz dict | ~10 B saveable, ~0 % ratio |

**To actually move ratio, the payload must change.** The three real levers, in order of
implementation feasibility:

1. **Cross-region atlas for repeated circuits** (the long-running ROADMAP target). When
   distant byte ranges share a generation rule (same transform + parameters + small
   residual), store the rule once and reference it from multiple positions. This was the
   user's "classic dictionary compression through repeated circuits" request. The
   multi-block plan dictionary added in v3 is the first concrete step — it deduplicates
   plans across blocks within one pack, paying `ceil(log2(unique_plans))` bits per block
   instead of repeating each plan's `(kind, period, head)` tuple. The next step is a
   pack-wide circuit atlas that works across both single-pack regions and multiple stream
   blocks.

2. **Container-aware paths** for known input shapes. For PNGs we already do this through
   `recursive-circuit-xz` (deflate-aware via preflate). The same shape applied to other
   containers — ZIP-wrapped PyTorch checkpoints, MP4 tracks, ELF sections — would let us
   parse the structure, split metadata from bulk weights/data, and run targeted
   compression (e.g. float-bit-plane separation for tensor weights). Estimated win:
   5-25 % on the matching corpus.

3. **Replace XZ with a context-adaptive entropy coder tuned for the typical
   transformed-payload distribution.** This is a major undertaking (months of work) and
   the expected win vs LZMA2 is modest (a few percent) on general data. Not the right
   first investment.

Dictionary-level micro-compaction continues to be a useful **principle** — the format
should never spend bytes on bits the enumerations don't need — but the next ROADMAP
items that actually move ratio are payload-level, not header-level.

## Recent Landed Items

- **2026-07-02 benchmark-runtime pass** (no format change, all tracked output bytes
  identical). Four search-cost cuts plus measurement fixes:
  1. The raw-xz tuning-matrix skip gate (`core.rs`) now considers all structural candidates
     (recursive-circuit-xz, monotonic-delta, adaptive-transformed-xz), tiered: skip outright
     when the structural candidate is ≤ 5/8 of the cheap preset-3 total; in the borderline
     band, one easy XZ-9 probe bounds the matrix (every entry is a preset-9 variant with a
     few-percent spread vs easy-9) and the matrix is skipped when the structural candidate
     beats the probe by more than 1/16. Primary and cat no longer walk the matrix.
  2. Forced transform-plan variants (tail delta/xor/gather heads, bit-plane-on-tail, row
     predictors) are pre-ranked on the existing 512 KiB sample and only a profile-bounded
     top slice pays the full-data XZ-3 ranking encode (fast 2 / balanced 4 / deep+research
     unbounded). Cat balanced: 35 -> 20 Phase-A encodes.
  3. The winner tuned-XZ refinement after plan search uses extreme presets only on
     deep/research (`enable_xz_extreme_winner_refine`). Measured on cat: the extreme jobs
     doubled the refinement wall time (14.9 s -> 6.4 s without them) and the refinement
     winner was already held by the Phase-B XZ-9 pass.
  4. Prelude candidates (index/huffman/deflate/zstd/brotli/cheap-xz estimate) run
     concurrently via nested `rayon::join`.
  Measurement fixes: the cat benchmark script now runs `--release` like the depth script
  (debug builds roughly doubled its wall time); `raw_xz_ms` no longer absorbs the
  recursive/monotonic/adaptive build time (cat reported 51 s where the true matrix cost
  was ~1.6 s); the winner-refinement time is now included in `recursive transform
  evaluation`; `[profile.dev.package."*"] opt-level = 3` makes `cargo test` run optimized
  codec dependencies. Balanced results (release-to-release, identical bytes): primary
  `4.8 s -> 2.7 s`, cat `51.8 s -> 34.3 s`, depth_anything `628 s -> 359 s` (IMMED-5's
  `< 120 s` target is still open; the remaining depth cost is the adaptive plan search
  itself, not the raw-xz matrix). Raw-brotli additionally probes TEXT vs GENERIC mode in
  parallel (ties on paper; may help other text corpora).
- **RawBrotli** (new top-level pack method, ZBPK v4) — implemented. It is gated by a cheap
  text-likeness check plus an 8 MiB bound so binary corpora do not pay q11 Brotli cost.
  Current paper benchmark: `62015 -> 18573`, ratio `0.299492`, validation PASS. Primary
  remains `monotonic-delta` at `562799`, validation PASS.
- **N1 row-aware predictors** — implemented (see N1 entry below). Available but does not
  win on already-filtered PNG IDAT data.
- **N3 per-block transform plans** — implemented (see N3 entry below). Format extension is
  live; ready to win on heterogeneous inputs.
- **AdaptiveTransformedXz** (new top-level pack method, ROADMAP item *N7*) — implemented.
  Brings the existing transform-plan-search pipeline to inputs that are *not* framed
  deflate. Previously, on a corpus like a PyTorch `.pth` model file (PK-ZIP wrapper around
  pickled float32 tensors), the only winning candidate was plain raw-xz; the periodic /
  delta transforms our recursive path uses were unreachable because no CRC32-framed run
  was detected. The new method runs `choose_adaptive_transform_plan` directly on the raw
  input, picks the best reversible transform plan, encodes the transformed payload with
  the full codec / tuned-XZ selection, and stores a 18-byte dictionary
  `(transform_kind, period, head, codec, plain_len)` so the decoder can invert the
  transform deterministically. The candidate is gated by two heuristics: (a) skip when
  recursive-circuit-xz is already evaluated (same search would be duplicated), and (b)
  skip when raw-xz already achieves ratio ≤ 0.30 (high-compression corpora like
  `primary.3b.bin` get no measurable headroom from the transform plan and the plan-search
  cost is not worth paying). Test coverage:
  `pack::tests::adaptive_pack_can_choose_adaptive_transformed_xz_and_roundtrips`.

## IMMEDIATE: Critical Architectural Debt (Blocks All Circuit-Level Gains)

These are not enhancements. They are structural corrections to decisions that rendered the
Boolean circuit layer cosmetic. Every Phase 0–9 item depends on these being addressed first.
Two-sentence summary: **bit-packed dynamic encoding was applied to the file header but not to
the circuit definitions inside it; the exact minimizer was bounded at 16 inputs but no
hierarchical decomposition was added to cover the rest; together, the Boolean layer does not
participate in actual compression of large files.**

### IMMED-1. Bit-Stream Encoding Must Be the Universal Primitive, Not a Header-Only Feature

**The problem.** ZBPK v3 spends careful engineering ensuring the file header occupies exactly
the bits its enumeration values require — method (4 bits), transform kind (6 bits), period/head
as nibble-varints, topology parent index as `⌈log₂(N_prev)⌉` bits. The result: ~110 bytes of
header overhead. This is correct in principle.

The same principle was never applied to the content these headers describe. Circuit definitions,
implicant lists, cube encodings, DAG node records, and residual payloads are written as
byte-aligned fixed-width structs or as raw byte slices inside the payload area. There is no
continuous bit stream for circuit-level data. The bit-packing was applied to the wrapping
envelope only; the wrapped data is still padded to byte boundaries everywhere.

The consequence: even if a shared circuit were discovered, the cost to serialize it is large
enough (tens of bytes per circuit node) that sharing rarely pays off, which is one structural
reason the circuit atlas was never built. You cannot profitably reference a circuit that costs
50 bytes to describe when a reference itself could cost 5 bits.

**The fix.** Extend the bit-stream writer/reader used for header fields so it becomes the
universal primitive for ALL structured data at every recursive level:

- Every `CircuitTopologyNode` field uses variable-width encoding down to its content. Already
  partially done for topology — complete it for circuit content.
- Every cube/implicant encodes as: `mask_popcount` (number of fixed literals, short varint),
  followed by `mask_popcount` × `(variable_index, polarity)` pairs. A single-literal implicant
  costs ≈3–5 bits. A 4-literal implicant costs ≈12 bits. Compare to any current fixed-width
  struct.
- Every DAG node uses a type tag (3–4 bits) followed by variable-width child IDs using
  `⌈log₂(N_prev_nodes)⌉` bits — already designed for topology nodes, apply consistently to all
  circuit content.
- Residual byte runs: length varint, then bytes. No fixed-size run headers.
- Every recursive level (transform plan → sub-plan → implicant cover → implicant → cube) chains
  into the same bit stream. Not separate byte arrays pointed to by a header.

Define a `CircuitBitStream` struct that owns the interning table:

```rust
pub(crate) struct CircuitBitStream {
    bits: BitVec,
    interning: HashMap<CircuitContentHash, CircuitId>,
    next_id: CircuitId,
}

impl CircuitBitStream {
    /// First occurrence: write full definition. Subsequent: write only the ID.
    pub fn write_circuit_or_ref(&mut self, circuit: &CircuitDef) -> CircuitId;
    /// Cost in bits of a reference at current interning table size.
    pub fn ref_bits(&self) -> u32 { self.next_id.ilog2() + 1 }
}
```

Every encoder that emits circuit content (transform topology, implicant cover, DAG node) must go
through `CircuitBitStream`. The second region to use a circuit pays only `⌈log₂(N)⌉` bits for
it, not a full re-serialization. This makes cross-regional sharing automatic and cheap.

**Acceptance criteria:**
- A truth-table circuit for a 3-variable function serializes in under 10 bits.
- A file with 100 regions using the same 3-node transform topology emits the definition once
  (≈30 bits) and 99 references (≈7 bits each for `log₂(100)`), totaling ≈723 bits. Current
  cost: 100 × 18 bits = 1800 bits. Shared encoding must be measurably smaller.
- The decoder reads circuit-level data from the same bit stream, reconstructing the interning
  table on the fly. No separate byte slices for circuit content.
- This replaces the current separate `topology_bits` section in the recursive dictionary.

### IMMED-2. Boolean Minimization Must Not Be Bounded at 16 Inputs

**The problem.** The exact minimizer is bounded at 16 inputs because it stores a full truth
table (2^n entries). At 16 inputs this is 64 K minterms. Beyond 16 the system falls through to
Espresso-style heuristic refinement. This is not wrong for truth-table-based synthesis — it is
wrong as a ceiling on what circuits the compressor can describe.

16 inputs bounds the compressor to describing functions of 16 bits of context. A function that
generates a 4-byte float32 element from a stride index already exceeds this. A function that
models the bit-plane structure of 8 bytes of tensor weight from a 3D index (channel × row ×
col) requires ~30 inputs. The periodic-stride pattern behind the 15.96% tensor savings IS
expressible as a Boolean function over ~30 inputs. It is NOT enumerable as a truth table but IS
expressible as a small hierarchical circuit. The 16-input limit silently excludes the exact
structures this system was designed to exploit.

The bug is conflating "number of function inputs" with "truth-table size". These are equal only
at leaf level. Top-level circuits over large input spaces must be decomposed.

**The fix.** Add hierarchical decomposition above the existing bounded exact minimizer:

1. **Shannon cofactor decomposition.** For n > 16 inputs, select a splitting variable
   (`f = x·f|x=1 + ¬x·f|x=0`), recursively minimize both cofactors, and combine via a mux
   node. Recursion depth is bounded by the resulting literal count, not by 2^n. This is the
   standard path in every serious synthesis tool.

2. **Structural input encoding.** Compression-context circuit inputs are not arbitrary bit
   positions — they are structured: `(period_index, byte_offset_within_period, bit_plane)`.
   This structure must be explicit. A function over `(period=4, offset=0..3, plane=0..7)` has
   5 relevant input bits, not 30 random bits. Model inputs as a product of small groups; apply
   decomposition along group boundaries before falling back to Shannon.

3. **BDD backend for medium-depth functions.** For 10 < n ≤ 24 inputs, try an ordered BDD as
   the intermediate representation before committing to Shannon splitting. BDD size grows
   linearly with circuit nodes, not exponentially with inputs, for the strided/periodic
   functions that appear in compression contexts. Reject and fall back to heuristic if BDD
   size exceeds a configurable node budget.

   Decision table:
   - n ≤ 10: truth table, exact Q-M.
   - 10 < n ≤ 20: try BDD, fall back to heuristic if node count > budget.
   - n > 20: Shannon decomposition into sub-problems of size ≤ 16, minimize each exactly.

4. **Do not remove the 16-input exact path.** Keep it as the leaf minimizer. The limit is
   correct for the truth-table representation. What must be removed is the assumption that
   16 inputs is the limit of what can be described — that is the architectural bug.

**Acceptance criteria:**
- A function over a 4-byte periodic stride (32 inputs, strongly decomposable) can be
  represented and serialized as a hierarchical circuit.
- A bit-plane separation function (input: 8 bytes, output: reordered bits) can be represented
  as a circuit, not only as a `CircuitTransformKind` enum variant.
- The exact minimizer remains operative at leaf level (n ≤ 16).
- Compression of the tensor corpus does not regress; circuits can explicitly describe the
  periodic-stride pattern, not only encode it implicitly via a transform-kind integer.

### IMMED-3. Circuit Sharing Requires the Bit-Stream Interning From IMMED-1 to Exist First

**The problem.** Cross-regional circuit reuse (`GlobalSlice`, the Atlas) is the stated goal of
this entire roadmap. It has not been implemented, and the structural reason is IMMED-1: without
a cheap bit-level encoding for circuit definitions, sharing a circuit costs more than repeating
the per-region representation it is supposed to replace. No set-cover optimizer will select an
atlas entry that costs more than it saves.

The current `GlobalSlice` mechanism stores one globally-compressed payload and points decoded
output ranges at slices of it. This is output-slice reuse, not semantic circuit reuse. It
requires the entire global output to be reconstructed before any slice can be used, which
breaks streaming restart guarantees and makes the restart cost proportional to global payload
size.

**The fix is not a separate implementation — it is the consequence of IMMED-1 and IMMED-2.**
Once circuit definitions are encoded in the `CircuitBitStream` with interning (IMMED-1) and
circuits can represent the relevant functions without a 16-input ceiling (IMMED-2), circuit
sharing becomes the natural default behavior: the second occurrence of any circuit definition is
automatically encoded as a back-reference, at `⌈log₂(N)⌉` bits cost. No explicit "atlas
candidate" infrastructure is needed to make basic circuit reuse work; it falls out of the
encoding layer.

The Atlas infrastructure (Phases 2–6 below) then handles the harder problems: discovering
non-identical but structurally similar regions, sparse residuals, and globally optimal range
selection. But it can be built incrementally on top of a base layer that already does exact
circuit sharing cheaply.

**Acceptance criteria:**
- Two distant byte ranges with identical transform topology and parameters are automatically
  encoded with one definition + one reference, not two definitions.
- Stream blocks that share a transform plan reference the shared definition without
  reconstructing the global decoded output.
- `GlobalSlice` nodes are no longer required for the common case of repeated exact circuits.

### IMMED-4. Correction Stream: Reduce Tokens Before CABAC, Not After

**The problem.** The preflate correction stream is CABAC-coded; its byte-level entropy is
near-uniform; no post-hoc transform reduces it. This analysis is correct as far as it goes.
The mistake is treating the CABAC output as the invariant and looking for wins downstream of it.

The real opportunity is reducing the number of corrections before CABAC coding, by improving
the predictor within the decomposition pipeline. Typed substreams (one CABAC instance per
`CodecCorrection` family) have lower per-family entropy than the mixed stream; CABAC's context
model adapts faster to a homogeneous token distribution.

**Status (2026-05-26): blocked by vendor API surface — needs an upstream patch.** The 2026-05-26
implementation pass uncovered that the in-tree-only path proposed below is not actually
reachable without modifying `vendor/preflate-rs`:

- `ReconstructionData` is declared `struct` (not `pub struct`) in
  `vendor/preflate-rs/src/stream_processor.rs:32`. Its `read(data) -> Result<Self>` method is
  similarly private. `bitcode::decode::<ReconstructionData>` cannot be called from outside the
  crate.
- Even if `ReconstructionData` were exposed, `PredictionDecoderCabac::decode_correction(action)`
  needs the caller to supply the next correction *family* (the action) at every step. That
  ordering is driven by the `token_predictor` / `tree_predictor` replay, not by the byte stream.
  So "draining tokens from the CABAC stream" requires running the entire recreate pipeline with
  an instrumented decoder — which means depending on private preflate-rs types
  (`TokenPredictor`, `DeflateParser`, `PredictionDecoder`).
- Re-encoding into typed substreams loses the per-token *interleaving order* the recreate
  consumer expects; the consumer would need to be modified to read from typed substreams as the
  predictor demands each next family.

**The fix (revised path):**

1. **Upstream patch (vendor/preflate-rs)**: expose `ReconstructionData` (and its `read` method),
   `PredictionDecoder`, and an instrumented-decoder hook that the recreate pipeline calls
   through. Alternatively, add a public `recreate_with_typed_corrections(plaintext, parameters,
   per_family_substreams)` entry point.
2. **In-tree (zbit-rs)**: once the upstream API exists, drive the recreate pipeline with an
   instrumented decoder that records every `(family, value)` pair; split into per-family
   vectors; re-encode each with its own CABAC context; emit via `CircuitBitStream` (the
   IMMED-1 primitive already supports per-family typed sub-streams via separate
   `CircuitBitStream` instances or `BitSerializable`-typed entries).
3. **Decode**: the patched preflate-rs `recreate_with_typed_corrections` consumes the typed
   substreams in the order the predictor demands.
4. **Cost gate**: keep the existing `choose_correction_transform_plan` fallback so the typed
   path is selected only when it beats the current mixed-stream baseline.

**Acceptance criteria (unchanged):**
- Correction stream bytes for cat corpus drop measurably below the current 284 KB baseline.
- Roundtrip validation passes (corrections still reconstruct the original DEFLATE stream).
- Per-family CABAC model shows lower cross-entropy on held-out data than the mixed model.
- If typed encoding does not win on a given input, fallback to single-stream path is automatic.

**Why no work landed this session**: the upstream patch is a real change to a third-party
crate (Microsoft preflate-rs vendored under Apache-2.0). Doing it correctly requires
PR-quality changes with their own tests and an interface stable enough that future preflate-rs
updates won't break the typed path. That's a focused session of its own, not a "while I'm here"
addition to a different roadmap item. The existing N2 section above already documents this
upstream-coordination requirement; IMMED-4 here is now consistent with that finding.

### IMMED-5. Compression Throughput: Complete the Parallelism, Don't Gate Behind Profiles

**The problem.** Deep profile runs at ~0.16 MiB/s compression throughput (628 s for 99 MB).
`rayon` is already a dependency and is used in some paths, but the bottleneck operations are
not parallelized where it counts:

- `choose_best_tuned_xz_candidate` iterates the parameter matrix sequentially. XZ encode calls
  are independent and embarrassingly parallel.
- `choose_adaptive_transform_plan` runs the XZ-3 ranking pass and subsequent full evaluations
  sequentially per transform candidate.
- `build_recursive_circuit_stream` preflate chain-length candidates are partially parallelized
  but correction transform/codec search is sequential.

**The fix:**

1. **XZ tuning matrix: fully parallel.** Replace the inner loop in
   `choose_best_tuned_xz_candidate` with a `par_iter()` over the full parameter matrix.
   Each `xz_encode(data, params)` call is independent. Collect results, take the minimum.

2. **Transform plan search: two-level parallel.** In `choose_adaptive_transform_plan`:
   - Quick-rank pass (XZ-3 on the sample subset): `par_iter()` over all transform candidates.
   - Top-k full evaluations: `par_iter()` over surviving candidates, each running the full
     codec/tuning selection internally.

3. **Work-stealing budget.** Add `max_parallel_codec_threads: usize` to `CompressionBudgets`
   so `balanced` limits parallelism (avoiding I/O saturation) while `deep`/`research` use all
   available cores.

4. **Pipeline the three-stage search.** The current pattern is: apply-transform → XZ-3 score →
   prune → XZ-9 full, executed serially. These stages should pipeline: all XZ-3 calls for all
   candidates launch simultaneously, pruning runs as results arrive, all surviving XZ-9 calls
   launch simultaneously. This maximizes CPU utilization without changing the selection result.

**Target:** deep profile compression throughput ≥ 2 MiB/s on the tensor corpus (from 0.16
MiB/s now). The XZ tuning matrix alone accounts for >50% of current wall time and is trivially
parallelizable with `par_iter`.

**Acceptance criteria:**
- `depth_anything` compression time < 120 s on a multicore machine (was 628 s), without
  ratio regression.
- `fast` and `balanced` profiles are not slowed by the parallel infrastructure (thread pool
  is bounded by profile budget).
- Throughput is reported in MiB/s in all benchmark output alongside existing timing fields.

---

## Ratio-Improvement Evaluation Protocol (Guarding the 2026-07-02 Time Budget)

**The problem.** Ratio improvements have historically landed by adding candidates
unconditionally: every new transform family, tuning entry, and codec probe paid a
full-data encode on every input whether or not it could win. That is exactly how the
suite drifted to 65 s on cat and 628 s on depth — the search paid for everything and
proved nothing. The 2026-07-02 pass recovered ~2x wall time by adding upper-bound probes
and sample pre-ranking; those wall times are now the budget. **A ratio candidate that
cannot state its worst-case time cost and its skip condition is not landable.**

### Budgets (balanced profile, release build)

| Corpus | Compressed bytes (must not increase) | Wall budget |
|---|---:|---:|
| `papers/zbit-algorithmsResearch.md` | `18 573` | `0.15 s` |
| `assets/primary.3b.bin` | `562 799` | `3.0 s` |
| cat challenge | `2 670 567` | `38 s` |
| `depth_anything_v2_vits.pth` | `83 380 762` | `400 s` |

Wall budgets are the current measurement plus ~10 % headroom. A ratio win may raise a
budget only when the same commit re-baselines the table with the measured numbers and
says why the extra time buys bytes.

### Acceptance protocol for any ratio-motivated change

1. Run all four non-stream benchmarks with the release binary. Compressed bytes must be
   **strictly smaller on at least one corpus and byte-identical on the rest** — a change
   that shuffles candidates without changing bytes must reproduce every tracked byte
   count exactly (this is how the 2026-07-02 pass was verified).
2. Wall time within budget on every corpus, single cold run.
3. `Output validation: PASS` everywhere; `cargo test -q` green.
4. A new candidate must carry at least one of:
   - a **prove-can't-win gate**: a cheap encode that upper-bounds the expensive search
     (pattern: preset-3 estimate + 5/8 rule; easy XZ-9 probe + 1/16 rule in
     `core.rs`);
   - **sample pre-rank admission**: scored on the 512 KiB zstd-1 sample before any
     full-data encode (pattern: `aux_forced_budget` in `transforms.rs`);
   - **deep/research-only gating**, promoted to balanced later with a measured
     bytes-per-second case (pattern: row predictors, `enable_xz_extreme_winner_refine`).
5. When trimming or reordering search, capture `ZBIT_TRACE_RECURSIVE=1` before/after and
   verify the winner plan/codec line is unchanged, not just the final byte count.

### Queued ratio candidates, each with its time story

1. **Monotonic gap-stream secondary transform** (primary stretch `< 0.170`). Needs a
   ZBPK v5 monotonic dictionary slot carrying an optional `(kind, period, head)` plan
   applied to the gap stream before codec selection. Time cost: plan search over the
   ~1 MiB gap payload is hundreds of ms and runs **only when monotonic-delta already
   beat every raw candidate**, so paper/cat/depth pay nothing. Accept: primary bytes
   `< 562 799`, wall `≤ 3.0 s`.
2. **Container-aware depth path** (depth stretch `< 0.80`). Parse the PK-ZIP wrapper,
   split metadata from tensor bulk, per-entry plan search bounded by sample pre-rank.
   Expected to *reduce* wall time too: many small bounded searches replace one 95 MiB
   search. Accept: depth bytes `< 83 380 762`, wall `≤ 400 s`.
3. **N4 LZMA delta filter** (lzma-sys raw filter chain; `delta_dist` field already
   inert in `XzTuningParams`). Admission rule: add delta entries to the matrix **only
   when the existing autocorrelation scan reports a dominant period ∈ {2,3,4,8}**,
   never unconditionally. Accept: bytes down somewhere, cat wall `≤ 38 s`.
4. **Brotli as inner payload codec** for transformed/gap payloads. Blocked on the
   saturated 2-bit `PayloadCodec` field — fold the field widening into the same v5 bump
   as (1); probe gated by the same text-likeness + size checks as top-level raw-brotli.
5. **IMMED-4 typed CABAC substreams** — still blocked on the vendor patch (see IMMED-4);
   no in-tree path.

### Harness item (small, do first)

Add `zbit-rs/scripts/check_benchmark_budgets.sh`: runs the four benchmarks with the
release binary, extracts `(compressed bytes, compression ms)` from each report, fails if
any byte count exceeds the table above or any wall time exceeds its budget. This turns
the protocol into one command instead of four manual diffs.

---

## Near-Term Ratio Improvements (Concrete, Pre-Atlas)

The Circuit Atlas described in the rest of this document is the long-term direction. Before
that lands, several concrete, lower-cost ratio improvements remain on the table for the
existing `recursive-circuit-xz` path. They are listed here in priority order, with the
expected impact, what to implement, and the rough cost.

### N1. Row-aware predictor transforms for image-like data (implemented; helps unfiltered raster, not PNG)

The current `periodic-head-tail(period=stride, head=1)` winner on PNG IDAT plain separates
filter bytes from row data but then encodes the row data as-is. The row data is filtered
RGBA scanlines: bytes are channel-interleaved (RGBA RGBA RGBA …) and adjacent rows are
correlated. Vanilla XZ-9 captures some of this but cannot articulate "same channel of
previous pixel" as a coding primitive.

Implemented in `zbit-rs/src/pack/transforms.rs`:

- `PeriodicHeadTailTailRowDelta(period, pixel_stride)` — for each row of `period-1` bytes
  in the tail, apply `data[i] - data[i - pixel_stride]` byte-wise (PNG `Sub` filter).
- `PeriodicHeadTailTailRowXor(period, pixel_stride)` — same shape, XOR instead of delta.
- `PeriodicHeadTailTailRowUp(period)` — `data[i] - data[i - row_len]` (PNG `Up` filter,
  cross-row delta within the tail, never touching the filter-byte block).
- `PeriodicHeadTailTailBitPlaneTranspose(period)` / `…TransposeDelta(period)` — bit-plane
  transpose only on the tail; targets payloads where high bit planes are mostly homogeneous
  (signed-small residuals).

Pixel strides probed: 1, 2, 3, 4 (covers gray, RG, RGB, RGBA). Per-row scope ensures the
predictor never crosses a row boundary and never crosses the head/tail boundary. Forced
candidates for the discovered top period; XZ-3 ranking drops them quickly when they lose.

**Empirical outcome on the existing corpus:** these candidates *lose* the XZ-3 ranking on
the cat PNG IDAT plain (predictor scores ~2.88–3.6 MB vs the winning `periodic-head-tail`
at 2.69 MB at XZ-3, encoding to 2.39 MB vs 2.41 MB at XZ-9). The reason is that PNG IDAT
already carries the optimal per-row filter (Sub/Up/Average/Paeth chosen at encode time), so
applying another predictor on top is a *second* filter pass that adds noise instead of
extracting it.

They remain in the candidate pool because they should still win cleanly on **unfiltered
raster payloads** (raw RGB/RGBA bitmaps, scientific image dumps, fixed-stride binary tables
where each row is uncorrelated unfiltered bytes), and the cost when they lose is bounded by
the XZ-3 quick-rank pass.

Future variant to consider: **filter-aware per-row predictor selection** — choose the
predictor per row based on the row's PNG filter byte, so rows with `filter=0` (no filter)
get the predictor while rows with `filter=1..4` are left alone. This would require either a
new format slot for per-row predictor IDs, or a deterministic mapping from filter byte to
predictor.

### N2. Typed substreams for preflate corrections (blocked on upstream — no in-tree path)

**Investigated and currently blocked.** The corrections payload returned by
`preflate_whole_deflate_stream` is not a raw record stream we can structurally split — it
is `bitcode::encode(&ReconstructionData { parameters, corrections })`, where the inner
`corrections: Vec<u8>` is already a single **CABAC-encoded arithmetic-coded bitstream**
produced by `PredictionEncoderCabac` (`preflate-rs/src/cabac_codec.rs` + the `cabac` crate).
CABAC is context-adaptive arithmetic coding: the per-record correction values are coded
into one continuous bit stream, byte-aligned at the end. There is no record boundary in
the byte stream to split on.

Concrete consequences for our pipeline:

- The byte-level entropy of the 284 093-byte cat corrections is essentially uniform;
  applying XZ, XZ-extreme, or zstd to it returns *raw* (no compression). That is the
  arithmetic-coding floor and is observed empirically. The existing
  `choose_correction_transform_plan` already runs the full transform + codec sweep on the
  corrections and consistently picks `Raw + Identity` for cat-like inputs.
- The `bitcode` outer wrapper around `ReconstructionData` adds only ~25–35 bytes of
  parameter metadata; even if we split the wrapper from the CABAC body and compressed each
  separately, the savings would be in the dozens of bytes — far below the implementation
  cost.

Paths that *would* improve correction encoding, all requiring upstream changes to
`preflate-rs` (out of scope here):

1. **Emit one CABAC stream per `CodecCorrection` family** (literal-correction stream,
   length-correction stream, distance-correction stream, tree-correction stream, etc.).
   Each per-family stream would have its own context tables, so the model would adapt to
   its own statistics and the total bits should drop. Implementation cost is real: the
   encoder/decoder pair needs to keep N parallel `PredictionEncoderCabac` instances,
   demultiplex per call, and merge / split on the wire. Estimated win: 10–25 % on the
   corrections payload — i.e. ~28–71 KB off cat.
2. **Switch the entropy coder** from CABAC (VP8-style) to a modern arithmetic coder with
   better context modelling for the specific token-correction distribution (rANS, ANS,
   adaptive Range Coder). Same effort, similar expected savings.
3. **Reduce the number of corrections** by improving preflate's predictor (better hash
   algorithm detection, better add-policy estimator). This shrinks the input to CABAC
   instead of compressing its output. Win is highly file-dependent.

Recommendation: see **IMMED-4** (above) for the in-tree path that bypasses this blocker by
decoding the CABAC token stream back to typed `CodecCorrection` tokens and re-encoding each
type with its own model — without splitting the opaque byte stream. The upstream-coordinated
approach described below remains valid as a higher-ceiling alternative once IMMED-4 is
implemented and measured. From this codebase, the entropy floor of the mixed CABAC byte
stream is the binding constraint for post-hoc transforms; IMMED-4 attacks the model before
CABAC encoding, not after.

Mitigation we *do* apply already: `choose_correction_transform_plan` keeps testing
transforms + codecs on every preflate run, so when preflate-rs starts emitting more
compressible corrections (different chain length, future encoder change), we will pick
them up automatically without a format bump.

### N3. Per-block (block-local) transform plans inside a single pack (implemented; helps heterogeneous, neutral on uniform inputs)

Implemented in `zbit-rs/src/pack/transforms.rs::build_multi_block_transform` and wired
through `zbit-rs/src/pack/recursive.rs::build_recursive_circuit_stream`. The on-disk format
extension uses the top bit of the existing topology-count `u16` as a `MULTI_BLOCK_FLAG`;
when set, the recursive dictionary trailer carries `block_count u32`, `block_size u32`,
then `block_count` × `(kind u8, period u32, head u32)` plans. Legacy single-plan
dictionaries decode unchanged (the flag bit is never set there).

How it runs:

1. After the existing single-plan template is built, for each block-count `N` allowed by
   the profile (deep: 2/4, research: 2/4/8) we split the inflated plain into `N`
   equal-ish consecutive blocks (last block absorbs the remainder).
2. Per block we call `choose_adaptive_transform_plan(block, profile, 1)` to find the
   block-local best plan. The full-eval budget is intentionally tight (1) so the per-block
   plan-finding does not multiply the worst-case eval time.
3. The block-local plans are applied to their blocks, the transformed bytes are
   concatenated, and the **single** concatenated payload goes through
   `choose_best_codec` + `choose_best_tuned_xz_candidate`. This keeps XZ matches able to
   cross block boundaries.
4. We compute `multi_block_total = payload + 8 + N × 9` (block metadata cost) and compare
   to the single-plan template size; the smaller wins. The single-plan path is never
   regressed because multi-block is only selected on strict improvement.

**Empirical outcome on the existing corpus:**

- Cat (deep profile): single-plan template = `2 386 140 B`; multi-block split=2 =
  `2 491 916 B` (plans = `gather, head-tail`); split=4 = `2 495 460 B`. Single-plan wins
  by ~100 KB because cat is uniformly-structured PNG IDAT and re-arranging different
  halves with different plans destroys XZ's cross-region matches.
- Paper / primary.3b: not framed-deflate so N3 is not invoked.

Why this is still worth keeping: the format extension is now ready to *win* whenever the
input has a real per-region split — e.g. a deflate stream that bundles markup + binary
attachments, or a multi-image archive whose IDAT streams differ in stride. On those
inputs the per-block plan-finding will produce different winners and the multi-block path
will beat the single-plan path on size.

Cost profile on cat (deep): adds ~30–60 s to a full deep encode (per-block plan finding
+ a couple extra full-data XZ-9 encodes). Off entirely for fast/balanced via the
`CompressionProfile::multi_block_split_counts()` gate.

### N4. LZMA delta filter via xz2 / direct lzma-sys (medium impact)

We already added an inert `delta_dist` field to `XzTuningParams`. The xz2 crate does not
expose `Filters::delta()`, but the LZMA delta filter is in the xz Utils binary and the
lzma-sys crate. Two options:

1. Vendor a minimal patch to xz2 exposing `delta(dist)`.
2. Drop to the lzma-sys raw API and build the filter chain manually.

The delta filter combined with LZMA2 historically wins 1–5 % on PNG-decoded plain because
it gives LZMA2 access to channel-aware residual coding.

Expected impact: another 1–3 % on top of N1 if both are kept; partial overlap.

### N5. Wider tuned XZ matrix and BCJ filters (low/uncertain impact)

The current `tuned_xz_param_matrix()` covers `lc/lp/pb`, dict size, nice_len, match_finder.
Add a few candidates with `Mode::Fast` (some payloads compress better with greedy than
optimal parser) and BCJ filters (`x86`, `arm`) for binary-rich payloads. Most non-binary
payloads ignore BCJ, so probe only when the input has executable signatures.

Expected impact: <1 % on cat; potentially larger on disassembled binaries that aren't in
our corpus today.

### N6. Cross-correlation between transformed payload and corrections (low impact, research)

When preflate corrections derive from the same deflate stream we already decoded, parts of
the correction stream sometimes overlap byte-for-byte with the transformed payload (e.g.
literal corrections that quote bytes already present elsewhere). A small offset table that
references back into the transformed payload could replace those bytes. Significant
engineering; modest gain unless preflate is producing redundant references.

### Ratio Targets Before the Full Atlas

| Corpus | Current | N1 measured | N2/N3 measured | AdaptiveTransformedXz measured | Pre-atlas stretch |
| --- | ---: | ---: | ---: | ---: | ---: |
| `papers/zbit-algorithmsResearch.md` | `0.331855` | unchanged | unchanged | skipped (size gate) | `< 0.325` (BWT preproc) |
| `assets/primary.3b.bin` | `0.174058` | unchanged | unchanged | skipped (raw-xz gate) | `< 0.170` (gap-stream secondary transform) |
| cat normal | `0.899412` | unchanged | unchanged (single-plan wins) | skipped (recursive gate) | `< 0.870` |
| cat stream wide-overfit | `0.899455` | unchanged | unchanged | n/a (stream path) | `< 0.872` |
| `depth_anything_v2_vits.pth` | raw-xz `0.911471` | n/a | n/a | **`0.840376`** (selected) | `< 0.80` (container-aware) |

N1 did not move the cat ratio because PNG IDAT data carries an already-optimal per-row
predictor. The path forward for ratio on cat is N2 (typed correction substreams) and N3
(per-block transform plans), or — for already-filtered payloads where local transforms
are exhausted — the full Circuit Atlas (cross-region nonlocal references and shared
dictionaries).

These pre-atlas targets are intentionally conservative: they capture what is reachable
without changing the dictionary format beyond a minor version bump. Anything beyond
`< 0.85` on cat realistically requires the full Circuit Atlas.

### Sequencing Recommendation

1. Land **N1** behind the existing budgets (cheap, format-compatible, isolated risk).
2. Use the trace flag (`ZBIT_TRACE_RECURSIVE=1`) to verify the new row predictors win on
   the cat plan ranking; if not, prune the variants before merging.
3. Land **N2** behind a `.zbpk` minor version bump; reuse existing `choose_best_codec_cached`
   plumbing per substream.
4. Evaluate **N3** vs **N4**: pick whichever delivers more measured ratio on the corpus.
5. Re-baseline ratio targets; revisit Circuit Atlas only after N1–N4 are settled.

The rest of this document describes the long-term Circuit Atlas direction.

---

## Purpose

This roadmap is focused on the next large leap for `zBitCompressor.rs`: drastically improving compression ratio in both normal `.zbpk` mode and streaming `.zbps` mode by turning the current recursive transform metadata into a real, reusable, content-linked circuit system.

The central goal is no longer just “try more codecs” or “add more local transforms”. The next goal is to let the compressor discover a circuit once, cache it, simplify it, and reference it quickly from many distant regions of the same file, including regions that are not adjacent and do not look identical until the right reversible transform, predictor, bit-plane view, or residual model is applied.

The decoder must remain byte-exact and self-contained. Compression-time caches may be aggressive and persistent, but the compressed artifact must contain every dictionary, circuit, residual, schedule, and dependency needed for deterministic decompression.

## Current Baseline From the Code

The implementation is already beyond the first version of the old roadmap:

- `zbit-rs/Cargo.toml` already includes `rayon`.
- `choose_best_codec` already evaluates multiple XZ candidates in parallel and adds a zstd candidate.
- `choose_adaptive_transform_plan` already samples many reversible transform plans, evaluates selected plans in parallel, and does winner-only XZ extreme refinement.
- `build_recursive_circuit_stream` already searches preflate chain candidates in parallel and models correction streams.
- `PackStats`, `StreamPackStats`, benchmark reports, active profile reporting, skipped-candidate notes, and timing breakdowns already exist.
- Stream mode already has key-piece blocks, split/group nodes, `wide_overfitting_circuits`, shared grouping payload support, and key-piece resume validation.

Current tracked benchmark snapshot:

| Corpus / mode | Selected method | Original bytes | Compressed bytes | Ratio | Main bottleneck |
| --- | ---: | ---: | ---: | ---: | --- |
| `papers/zbit-algorithmsResearch.md` | `raw-xz` | `62,015` | `20,632` | `0.332694` | generic text codec selection |
| `assets/primary.3b.bin` | `monotonic-delta` | `3,233,613` | `562,836` | `0.174058` | framed scan overhead, not ratio |
| cat challenge normal | `recursive-circuit-xz` | `2,969,404` | `2,670,718` | `0.899412` | preflate + transformed payload coding |
| cat challenge stream wide/shared | global/slice recursive pack | `2,969,404` | `2,670,846` | `0.899455` | global payload construction and validation |

The important limitation is architectural:

- `CircuitTopologyNode` currently serializes transform metadata and hashes, not a true reusable circuit graph.
- `recursive-circuit-xz` transforms one inflated payload and one correction stream; it does not yet build a global circuit atlas with reusable subgraphs.
- `indexed-circuit` is intentionally skipped for byte streams because `symbol_bits <= 8`; the existing symbol-level circuit dictionary is not useful for large byte-level file structure.
- Stream `GlobalSlice` gives good ratio by storing one whole-file compressed payload and slicing decoded output, but this is not the same as restart-safe shared circuit reuse. It still requires the global output to be reconstructed before the slice can be used.
- `build_best_stream_node` uses per-block memoization, but there is no cross-block cache of equivalent circuits, transforms, residual models, or encoded candidates.
- The compressor has no content-addressed index that can connect “this region here is produced by the same circuit as that distant region there, with a small patch”.

This roadmap therefore treats the next version as a **Circuit Atlas compressor**.

## Target Architecture: Circuit Atlas

A **Circuit Atlas** is a global dictionary of reusable reversible circuits, predictors, transforms, and residual encoders discovered over the whole input. It must be usable in normal mode and stream mode.

A circuit atlas entry should answer:

1. What transformation or predictor graph is shared?
2. Which file ranges use it?
3. Which range-specific parameters are needed?
4. Which residual bytes or correction records remain after applying it?
5. What is the exact inverse/decode schedule?
6. How much dictionary cost is amortized by all references?

### Design Principles

- **Self-contained compressed files:** never require an external cache to decode.
- **Compression cache is allowed; decode cache is optional:** compression may use persistent and in-memory caches to find candidates quickly, but the final artifact embeds the selected atlas.
- **References must be explicit:** a distant region may reference a shared circuit ID, but the decoder must know how to reconstruct that region without guessing.
- **Circuit reuse is selected by cost, not by excitement:** a shared circuit is emitted only if dictionary cost plus reference/residual cost beats independent encoding.
- **Cross-file learning is allowed only as a compressor hint:** persistent learned atlases can speed up discovery, but if a learned circuit is used, its serialized form must be embedded in the output.
- **Stream restart constraints are first-class:** a stream key-piece must be decodable from its key boundary using only global atlas dictionaries and local/key-piece residual payloads, not by reconstructing the whole original file first.

## Phase 0: Stabilize the Current Pipeline Before Adding Atlas Logic

Priority: immediate.

### 0.1 Rename and Clarify Current “Circuit” Paths

Current names make the implementation look more circuit-based than it really is. Rename or document internally:

- `recursive-circuit-xz` means: framed payload extraction + preflate reconstruction + reversible transform metadata + encoded transformed payload + encoded correction payload.
- `CircuitTopologyNode` means: transform topology metadata, not a minimized AIG/XAG circuit graph.
- `wide_overfitting_circuits` means: whole-file global pack + output slice nodes, not stream-safe shared circuit dictionaries.

Implementation guidance:

- Keep public method names stable until format migration is ready.
- Add comments near `RecursiveCircuitStream`, `CircuitTopologyNode`, `StreamNodeKind::GlobalSlice`, and `ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS` explaining the current semantics.
- In benchmark reports, add a note differentiating:
  - `global-output-slice` reuse,
  - `shared-grouping-payload`,
  - future `shared-circuit-atlas` reuse.

Acceptance criteria:

- No behavior change.
- Reports no longer imply that stream mode is already doing true shared circuit-map reuse.

### 0.2 Split `pack.rs` Into Implementation Modules

`zbit-rs/src/pack.rs` is now carrying too many responsibilities. Before adding atlas logic, split code into modules while preserving public APIs.

Suggested module split:

```text
zbit-rs/src/pack/
  mod.rs                    # public pack/stream API and format dispatch
  format.rs                 # ZBPK/ZBPS headers, read/write helpers, versioning
  codecs.rs                 # raw/deflate/zstd/xz encode/decode and candidate selection
  transforms.rs             # CircuitTransformKind, plan apply/invert, scoring
  framed.rs                 # CRC32-framed run detector/rebuilder
  recursive.rs              # RecursiveCircuitStream + preflate correction path
  monotonic.rs              # monotonic-delta candidate
  stream.rs                 # ZBPS block/node planner/decode
  stats.rs                  # PackStats, StreamPackStats, timings
  cache.rs                  # compression-time memoization hooks
  atlas.rs                  # new circuit atlas candidate path
```

Acceptance criteria:

- `cargo test --manifest-path zbit-rs/Cargo.toml` passes.
- Benchmarks produce byte-identical compressed outputs for deterministic profiles, except for allowed header/report-only differences.
- Future atlas code can be added without growing a single 200k+ byte file further.

### 0.3 Add a Real Compression Context Object

Current functions pass mutable timings and skipped-candidate vectors through many call stacks. Replace this with a context object that also owns caches and budgets.

Suggested structure:

```rust
pub(crate) struct CompressionContext {
    pub profile: CompressionProfile,
    pub timings: CandidateTimingStats,
    pub skipped_candidates: Vec<String>,
    pub cache: CompressionCache,
    pub budgets: CompressionBudgets,
    pub trace: TraceFlags,
}
```

Use it in:

- `compress_adaptive_to_bytes`
- `build_recursive_circuit_stream`
- `choose_adaptive_transform_plan`
- `choose_best_codec`
- `compress_stream_to_bytes`
- `build_best_stream_node`
- future atlas builder functions

Acceptance criteria:

- Existing stats are still reported.
- Cache hit/miss counters can be added without changing every function signature again.

## Phase 1: Content-Addressed Compression Cache

Priority: immediate for speed and necessary for deep atlas search.

The current code recomputes many expensive candidates for equivalent payloads, adjacent merged stream ranges, and repeated transform outputs. A cache layer is required before adding much deeper candidate discovery.

### 1.1 Add Stable Fingerprints

Add a lightweight fingerprint type for compression-time lookup.

Suggested dependencies:

```toml
blake3 = "1"
xxhash-rust = { version = "0.8", features = ["xxh3"] }
smallvec = "1"
```

Use two levels:

- `xxh3_64` or equivalent for fast table lookup.
- `blake3` or full byte equality check for collision safety when reusing payloads.

Suggested key:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct PayloadHash {
    pub fast64: u64,
    pub strong128: [u8; 16],
    pub len: usize,
}
```

Never trust a fast hash alone for byte-perfect reconstruction. Use it to find candidates, then verify full bytes or strong hash before reuse.

### 1.2 Cache Expensive Candidate Outputs

Add `CompressionCache` with separate namespaces:

```rust
pub(crate) struct CompressionCache {
    pub codec_outputs: HashMap<(PayloadHash, CodecProfileKey), EncodedPayload>,
    pub transform_outputs: HashMap<(PayloadHash, CircuitTransformPlan), Vec<u8>>,
    pub transform_scores: HashMap<(PayloadHash, CircuitTransformPlan, SampleKey), usize>,
    pub preflate_outputs: HashMap<(PayloadHash, PreflateKey), PreflateResultSummary>,
    pub range_candidates: HashMap<RangeCandidateKey, PackedRangeCandidate>,
    pub atlas_candidates: HashMap<AtlasCandidateKey, AtlasCandidateSummary>,
}
```

Cache these immediately:

- `apply_transform_plan(data, plan)` outputs.
- quick sample zstd scores in `choose_adaptive_transform_plan`.
- final `choose_best_codec` outputs.
- preflate results by `(deflate_stream_hash, max_chain_length)`.
- correction transform/coding candidates.
- `stream_pack_range_candidate` across all stream blocks, not only inside one block.

### 1.3 Cross-Block Stream Cache

Currently each block creates a new `pack_cache` in `compress_stream_to_bytes`. Promote it to the stream-level context.

Change:

```rust
for block_index in 0..block_count {
    let mut pack_cache = HashMap::new();
    ...
}
```

to:

```rust
let mut stream_range_cache = HashMap::new();
for block_index in 0..block_count {
    ... use &mut stream_range_cache ...
}
```

Then make range keys absolute:

```rust
struct RangeCandidateKey {
    input_hash: PayloadHash,
    abs_start_chunk: usize,
    abs_end_chunk: usize,
    realtime_mode: bool,
    allow_recursive: bool,
    profile: CompressionProfile,
}
```

Acceptance criteria:

- Stream reports include cache hits/misses for range candidates, codecs, transforms, and preflate.
- Repeated stream benchmark warm runs are materially faster.
- No cache hit is accepted without collision-safe validation.

### 1.4 Persistent Compressor-Side Atlas Cache

Add an optional cache directory controlled by environment variables:

```text
ZBIT_CACHE_DIR=.zbit-cache
ZBIT_ENABLE_PERSISTENT_CACHE=1
ZBIT_CACHE_MAX_BYTES=...
```

Persist only compression hints:

- fingerprints of successful transform plans,
- preflate parameter winners,
- codec profile winners,
- reusable circuit signatures,
- nonlocal range-match indexes.

Do not require this cache for decode. If a persistent circuit is selected, serialize the circuit into the `.zbpk` or `.zbps` output.

Acceptance criteria:

- Cold runs work exactly as before.
- Warm runs can skip expensive failed candidates.
- Cache entries are versioned by format version, transform version, codec key, and profile.

## Phase 2: Global Nonlocal Match and Circuit Discovery

Priority: highest for drastic ratio improvement.

The current compressor mostly models one contiguous transformed payload at a time. The next improvement is to find relationships between distant regions.

### 2.1 Multi-Scale Content-Defined Segmentation

Before building circuits, segment input at multiple scales:

- fixed windows: 256 B, 1 KiB, 4 KiB, 16 KiB, 64 KiB, 256 KiB, 1 MiB;
- content-defined chunks using Gear/Rabin-style rolling hashes;
- format-derived boundaries from framed/container analyzers;
- stream chunks and key-piece blocks;
- row/tile boundaries for image-like inflated payloads;
- correction-record boundaries for preflate corrections.

Create:

```rust
pub(crate) struct Segment {
    pub id: SegmentId,
    pub offset: usize,
    pub len: usize,
    pub origin: SegmentOrigin,
    pub fingerprints: SegmentFingerprints,
}
```

Compute fingerprints over several normalized views:

- raw bytes,
- delta-prev,
- xor-prev,
- bit-plane transpose,
- periodic gather candidates,
- low-byte/high-byte planes,
- row predictor residual views when geometry is known,
- correction-record typed streams.

### 2.2 Build a Nonlocal Occurrence Index

Add an index that can answer:

- Where else did this exact segment occur?
- Where else did this transformed view occur?
- Which far-away segments are similar enough to patch cheaply?
- Which circuit signature appears many times?

Suggested structure:

```rust
pub(crate) struct OccurrenceIndex {
    exact: HashMap<PayloadHash, SmallVec<[SegmentId; 4]>>,
    normalized: HashMap<NormalizedHash, SmallVec<[SegmentViewId; 4]>>,
    simhash_buckets: HashMap<SimBucket, SmallVec<[SegmentViewId; 8]>>,
    ngram_buckets: HashMap<NGramHash, SmallVec<[SegmentViewId; 8]>>,
}
```

Use bounded candidate lists to avoid explosion:

- cap per bucket by profile,
- keep far-distance and diverse-context candidates,
- prefer matches with repeated occurrences,
- discard candidates that cannot amortize dictionary bytes.

### 2.3 Transformed Reference Candidates

For every promising pair `(source, target)`, try reversible links:

```text
target ≈ transform(source, params) + residual
```

Candidate transforms:

- identity copy,
- xor with previous byte,
- modular delta,
- add/subtract constant,
- byte rotation / bit rotation,
- bit-plane transpose,
- low/high nibble split,
- channel/plane permutation,
- periodic gather/scatter,
- row/column predictor residual,
- sparse patch over exact/near-exact reference,
- small affine GF(2) mapping over bit windows.

Represent a link as:

```rust
pub(crate) struct CircuitLinkCandidate {
    pub source: SegmentId,
    pub target: SegmentId,
    pub circuit: CircuitId,
    pub params: LinkParams,
    pub residual_model: ResidualModel,
    pub estimated_cost: BitCost,
    pub verified: bool,
}
```

The target range is encoded as a reference to the source circuit plus residual payload, not as an independent codec payload.

Acceptance criteria:

- Distant repeated or near-repeated regions are detected even when separated by megabytes.
- The compressor can emit a nonlocal reference candidate for normal `.zbpk` mode.
- The same discovery engine can be reused by stream mode with stricter dependency rules.

### 2.4 Sparse Patch and Residual Encoding

For transformed links, residuals are the key. Add specialized residual streams:

- exact-copy: no residual;
- sparse byte patches: varint gap positions + changed byte;
- sparse bit patches: bitset or RLE positions + xor mask;
- dense residual: delta/xor residual bytes passed to `choose_best_codec`;
- small alphabet residual: canonical Huffman/rANS candidate;
- repeated residual: dictionary over residual motifs;
- row/tile residual: per-row sparse patch or predictor residual.

Suggested residual decision:

```text
if mismatch_count == 0:
    ExactReference
elif mismatch_count / len < sparse_threshold:
    SparsePatch
elif entropy(residual) < entropy(target):
    DenseResidualCodec
else:
    reject link
```

Acceptance criteria:

- Link reports show source offset, target offset, transform, residual type, residual bytes, and saved bytes.
- Exact and sparse links pass roundtrip tests over random and adversarial data.

## Phase 3: Real Circuit Graph Core

Priority: highest after nonlocal candidate discovery.

The current `src/circuit.rs` uses hash-cached `Gate` references and `BitsMap`, but it is not the right core for large compression graphs. Keep it for legacy small truth-table demos, and add a new canonical graph core.

### 3.1 Add `circuit_graph.rs`

Suggested node model:

```rust
pub(crate) type NodeId = u32;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum GraphOp {
    InputByte,
    InputBit,
    Const(u64),
    Not,
    And,
    Or,
    Xor,
    AddMod8,
    SubMod8,
    RotateLeft8(u8),
    Gather { period: u32, lane: u32 },
    Scatter { period: u32, lane: u32 },
    Delta { distance: u32 },
    XorPrev { distance: u32 },
    Predictor(PredictorKind),
    ResidualApply(ResidualKind),
}

pub(crate) struct CircuitGraph {
    nodes: Vec<GraphNode>,
    structural_index: HashMap<StructuralKey, NodeId>,
    levels: Vec<u32>,
    refs: Vec<u32>,
}
```

Required properties:

- stable node IDs;
- structural hashing with equality checks;
- commutative input normalization for `And`, `Or`, `Xor`;
- complemented edges for cheap inversions;
- fanout/refcount tracking;
- topological serialization;
- graph versioning.

### 3.2 Circuit Signatures

Each graph or subgraph needs reusable signatures:

```rust
pub(crate) struct CircuitSignature {
    pub op_histogram: SmallVec<[(GraphOpClass, u16); 16]>,
    pub input_arity: u16,
    pub output_arity: u16,
    pub normalized_hash: [u8; 16],
    pub npn_hash: Option<[u8; 16]>,
    pub affine_hash: Option<[u8; 16]>,
}
```

Use signatures for:

- exact subgraph reuse,
- near-equivalent subgraph search,
- NPN-canonical cut replacement,
- persistent atlas lookup,
- cross-region linking.

### 3.3 Cut Enumeration and Exact Local Replacement

Add bounded cut enumeration to simplify graphs and identify reusable components.

Implementation steps:

1. Enumerate cuts up to 6 inputs in fast/balanced profiles.
2. Allow 8 to 12 inputs in deep/research profiles.
3. Build truth tables for cuts.
4. Canonicalize truth tables under input permutation/negation where budget allows.
5. Lookup best known implementation from a small library.
6. Validate replacement by exhaustive table or SAT.
7. Accept only if compression cost improves, not only node count.

Cost must include:

- graph dictionary bytes,
- parameter bytes,
- residual bytes,
- reference count amortization,
- decoder work,
- stream restart overhead.

Acceptance criteria:

- Reports include cuts tried, exact replacements accepted, SAT validations, dictionary bytes saved, and residual bytes changed.
- Synthetic XOR/affine corpora improve relative to AIG-only representation.

### 3.4 XOR/Affine and Modular-Byte Detection

Many compression-relevant relationships are not AND/OR-heavy. Add:

- GF(2) Gaussian elimination over bit windows;
- affine relation extraction for byte/bit planes;
- XOR divisor extraction;
- parity/bitmask predictors;
- modular add/subtract predictors for byte deltas;
- XAG serialization for XOR-rich circuits.

Acceptance criteria:

- XOR-heavy synthetic tests show clear gains.
- Bit-plane and delta-heavy references are represented as compact XAG/affine nodes rather than verbose generic gates.

## Phase 4: Atlas Candidate Selection as Weighted Set Cover

Priority: highest for real compression wins.

After discovery, there may be thousands of candidate circuits and links. Selection must optimize global compressed size.

### 4.1 Cost Model

Define a single cost model:

```text
candidate_gain = independent_cost(target_ranges)
               - atlas_dictionary_cost(circuit)
               - reference_headers_cost
               - params_cost
               - residual_cost
               - dependency_overhead
               - validation_checksum_cost
```

Do not emit a circuit unless gain is positive with safety margin.

Track separately:

- global dictionary bytes,
- local dictionary bytes,
- graph topology bytes,
- transform parameter bytes,
- references bytes,
- residual bytes,
- correction bytes,
- entropy-coded payload bytes,
- stream restart metadata bytes.

### 4.2 Non-Overlapping Range Selection

A target byte range must be encoded by exactly one selected representation. Use weighted interval scheduling / DP for ranges, with atlas candidates as alternatives.

Normal mode:

- allow global candidates over the whole file;
- references can point anywhere if the decode schedule is acyclic or if source bytes are already materialized;
- a global dictionary can be decoded before payload ranges.

Stream mode:

- allow global dictionary circuits, but not global output dependency;
- a block can reference:
  - global circuit dictionary entries,
  - local block dictionary entries,
  - previous bytes inside the same block,
  - explicitly carried key-piece history if configured;
- a block must not require reconstructing unrelated future or previous blocks to resume from a key-piece.

### 4.3 Greedy + Repair Strategy

Use a practical two-stage selector:

1. Greedy select high-gain circuits by normalized gain:

```text
score = gain / (dictionary_bytes + reference_count_penalty)
```

2. Repair overlaps with DP:

- remove low-gain overlapping links,
- re-pack uncovered gaps with current best local packers,
- re-evaluate dictionary amortization after references are removed.

Acceptance criteria:

- Atlas selection never regresses against the current best adaptive method.
- Reports explain why selected atlas candidates won.
- Rejected high-interest candidates are visible with rejection reason: overlap, negative gain, dependency violation, restart violation, or validation failure.

## Phase 5: New Normal-Mode Pack Method

Priority: high.

Add a new method before trying to replace existing methods:

```rust
PackMethod::CircuitAtlas
```

or, if keeping codec-specific naming:

```rust
PackMethod::CircuitAtlasXz
```

### 5.1 `.zbpk` Format Extension

Bump `.zbpk` version only when the new method is serialized.

Suggested sections:

```text
ZBPK v3
header
method = circuit-atlas
original_size
atlas_dictionary_len
atlas_dictionary
range_program_len
range_program
residual_payload_len
residual_payload
fallback_payload_len
fallback_payload
validation checksum(s)
```

Atlas dictionary contains:

- graph entries,
- transform entries,
- predictor entries,
- residual model entries,
- codec dictionary entries if any.

Range program contains ordered operations:

```rust
pub(crate) enum AtlasRangeOp {
    FallbackPack { offset, len, pack_bytes_ref },
    ExactRef { source_offset, target_offset, len },
    CircuitRef { circuit_id, target_offset, len, params_ref, residual_ref },
    MaterializeTemp { temp_id, circuit_id, params_ref },
}
```

The program must materialize output ranges deterministically and validate final length/checksum.

### 5.2 Normal-Mode Encoder Flow

Change `compress_adaptive_to_bytes` to include atlas as a candidate:

1. Build existing candidates exactly as today.
2. Build `CircuitAtlasCandidate` with a strict budget.
3. Validate atlas candidate by decoding it and comparing to input.
4. Add candidate total to `PackEvaluation`.
5. Choose best by size.

Do not make atlas mandatory. It must beat current methods or be skipped.

### 5.3 Normal-Mode Decoder Flow

Add `decode_circuit_atlas_payload`:

- parse dictionary;
- verify graph signatures/hashes;
- execute range program;
- decode residuals;
- apply inverse transforms;
- fill output ranges;
- reject overlaps, gaps, invalid offsets, cycles, and checksum mismatches.

Acceptance criteria:

- Current `.zbpk` files still decode.
- New atlas candidate can be enabled by profile or environment variable.
- `fast` profile can skip it; `balanced` can use bounded atlas; `deep/research` can use broader atlas.

## Phase 6: Stream-Mode Shared Circuit Atlas

Priority: high and directly requested.

The current stream mode gets good ratio mostly by storing a global pack and using `GlobalSlice`. Replace this with real shared circuit references so blocks can remain restartable without reconstructing unrelated output.

### 6.1 Add Stream Format Concepts

Add new ZBPS flag and node kinds:

```rust
const ZBPS_FLAG_SHARED_CIRCUIT_ATLAS: u16 = 0x0008;
const ZBPS_NODE_KIND_ATLAS_REF: u8 = 4;
const ZBPS_NODE_KIND_LOCAL_ATLAS_GROUP: u8 = 5;
```

Shared section layout:

```text
ZBPS header
optional shared circuit atlas dictionary
block 0 node program
block 1 node program
...
```

A block node may reference shared circuit IDs, but residual payloads and range schedules remain block-local unless explicitly declared global and restart-safe.

### 6.2 Key-Piece Restart Rules

For a key-piece block starting at chunk `K`, decoding from `K` may use:

- shared atlas dictionary stored before blocks;
- local dictionary inside block `K`;
- bytes reconstructed earlier inside the same block;
- optional bounded history explicitly stored in the block header;
- no dependency on decoded output from block `< K`, unless it is stored as independent carry state;
- no dependency on future blocks.

Add validation:

```rust
validate_stream_dependencies(start_key_piece, node_program, atlas_dictionary)
```

Acceptance criteria:

- Key-piece resume validation passes without decoding whole-file global output.
- Non-wide stream mode approaches current wide-overfit ratio on cat challenge without `GlobalSlice` output dependency.
- Reports distinguish:
  - `global_output_slice_bytes`,
  - `shared_atlas_dictionary_bytes`,
  - `block_residual_bytes`,
  - `fallback_local_pack_bytes`.

### 6.3 Cross-Block Circuit Linking

Build one shared atlas over all blocks, then per-block programs reference it.

Algorithm:

1. Segment the whole input and each block.
2. Discover candidate circuits globally.
3. Select shared circuits with weighted set cover under restart constraints.
4. For each block, run local DP:
   - local piece pack,
   - local group pack,
   - shared atlas reference,
   - local atlas reference,
   - fallback raw/deflate/zstd/framed/recursive pack.
5. Emit block-local residuals.

Important: shared circuits must be generic enough to reconstruct a block range from block-local inputs/params/residuals. They must not be “copy bytes from distant decoded offset” unless that source is stored as an explicit dictionary payload or allowed carry state.

### 6.4 Stream Planner Replacement

Replace recursive split search with bottom-up DP over range candidates.

Current `build_best_stream_node` recursively tries splits and occasionally group candidates. New planner:

```rust
for span in 1..=max_group_pieces {
    for start in 0..block_chunk_count - span {
        evaluate piece/group/fallback/atlas candidates
        dp[start][end] = min(candidate, split combinations)
    }
}
```

Benefits:

- predictable candidate count;
- easy parallel range evaluation;
- global cache reuse;
- easier integration of atlas references;
- explicit lower bounds for pruning.

Acceptance criteria:

- Same or better stream size than current recursive splitter.
- Planning time is lower and reported.
- DP can show selected node reason for each range.

## Phase 7: Container and Preflate Correction Modeling

Priority: high for cat challenge ratio.

The cat challenge is a compressed image-like container. The current generic CRC32 frame detector is useful, but deeper gains require modeling the structure inside the framed deflate payload and correction stream.

### 7.1 Generic Container Analyzer Trait

Add:

```rust
pub(crate) trait ContainerAnalyzer {
    fn name(&self) -> &'static str;
    fn detect(input: &[u8]) -> Option<Self> where Self: Sized;
    fn extract(&self, input: &[u8]) -> ZbitResult<ContainerModel>;
    fn rebuild(&self, model: &ContainerModel, payloads: &[&[u8]]) -> ZbitResult<Vec<u8>>;
}
```

Implement first:

- generic CRC32 frame run analyzer using current `build_framed_payload_run` logic;
- PNG-like analyzer when magic/header/chunks validate;
- zlib/deflate wrapper analyzer;
- raw framed stream analyzer.

### 7.2 PNG/Image Plane Modeling

For PNG-like inputs:

- parse chunks;
- split deterministic chunk metadata from payload;
- concatenate `IDAT` payloads;
- preflate/inflate to filtered scanlines;
- extract image geometry: width, height, bit depth, color type, bytes per pixel;
- split filter bytes from row data;
- model filter-byte stream separately;
- reconstruct exact chunks and CRCs.

Reversible transforms to test:

- row predictor residuals: Sub, Up, Average, Paeth;
- choose best predictor per row/tile;
- split RGBA channels;
- byte-plane and bit-plane transpose;
- reversible color transform: green-subtract, integer YCoCg-style transform;
- alpha special case;
- tile-local transforms;
- repeated row/tile circuit links through the atlas.

### 7.3 Preflate Correction Substreams

Current correction payload is treated too opaquely. Split corrections into typed substreams before coding:

- record kind stream;
- literal correction stream;
- length correction stream;
- distance correction stream;
- Huffman tree correction stream;
- block boundary correction stream;
- raw fallback bytes;
- sparse position deltas;
- repeated correction motifs.

Each substream gets its own transform and codec. Then the atlas can find repeated correction circuits across distant blocks or chunks.

Acceptance criteria:

- Reports break down transformed payload bytes vs correction bytes by substream.
- Cat challenge deep profile improves beyond current `0.899412` baseline.
- Preflate roundtrip validation remains byte-perfect.

## Phase 8: Deep Candidate Generation for “Best Compression Rate” Profiles

Priority: medium-high; only after cache and atlas selection exist.

The `research` profile should search broadly but not blindly. Use caches and lower bounds.

### 8.1 Profile Budgets

Extend `CompressionProfile` with explicit budgets:

```rust
pub(crate) struct CompressionBudgets {
    pub max_segments: usize,
    pub max_occurrences_per_bucket: usize,
    pub max_pair_candidates: usize,
    pub max_circuit_candidates: usize,
    pub max_cut_inputs: u8,
    pub max_atlas_dictionary_bytes: usize,
    pub max_time_ms: Option<u64>,
    pub max_memory_bytes: Option<usize>,
}
```

Suggested defaults:

| Profile | Purpose | Atlas search | Cut inputs | Persistent cache |
| --- | --- | --- | ---: | --- |
| `fast` | practical quick encode | off or tiny | 0-4 | read-only hints |
| `balanced` | default useful mode | bounded exact + sparse links | 6 | on if enabled |
| `deep` | ratio-first | broad transformed links | 8 | on |
| `research` | exhaustive experiments | maximal with pruning | 10-12 | on |

### 8.2 Lower Bounds and Early Rejection

Before fully encoding candidates, estimate:

- minimum possible residual bytes;
- dictionary amortization lower bound;
- stream restart overhead;
- codec lower bound from entropy estimate;
- transform parameter cost;
- validation overhead.

Reject candidates that cannot beat current best.

Acceptance criteria:

- Deep/research modes expose candidate counts and rejection reasons.
- Expensive candidates are not evaluated after their lower bound loses.

## Phase 9: Benchmark and Regression Policy

Priority: ongoing.

### 9.1 Add New Corpora

Current corpora are too few. Add:

- repeated distant chunks with small patches;
- repeated transformed chunks;
- XOR/affine-heavy synthetic data;
- PNG-like filtered scanlines;
- random incompressible data;
- mixed structured/unstructured data;
- stream-specific multi-block repeated structure;
- adversarial collision-like chunks for hash validation.

### 9.2 New Report Fields

Add to normal and stream reports:

```text
Atlas:
- atlas enabled: true/false
- discovered segments
- exact links considered/selected
- transformed links considered/selected
- shared circuit count
- local circuit count
- graph dictionary bytes
- reference bytes
- residual bytes
- fallback bytes
- cache hits/misses by namespace
- dependency violations rejected
- validation failures rejected
```

### 9.3 Ratio Targets

Initial realistic targets:

| Benchmark | Current | First target | Deep target |
| --- | ---: | ---: | ---: |
| cat challenge normal | `0.899412` | `< 0.890` | `< 0.875` |
| cat challenge stream non-wide atlas | `0.899455` with global/slice | `< 0.905` without global output slice | `< 0.890` without global output slice |
| `primary.3b.bin` | `0.174058` | no regression | `< 0.170` |
| paper markdown | `0.332694` | no regression | `< 0.320` |
| distant-repeat synthetic | new | beat zstd/xz by clear margin | near reference+patch theoretical cost |

Aggressive “best compression rate than ever” targets should live in `research` profile until they are fast and robust enough for `balanced`.

## Suggested Implementation Order

1. Add `CompressionContext` and promote stream caches to stream/global scope.
2. Split `pack.rs` into modules without behavior changes.
3. Add stable payload fingerprints and memoized codec/transform/preflate caches.
4. Add multi-scale segmentation and occurrence indexing.
5. Add exact and transformed nonlocal reference candidates with sparse residuals.
6. Add `circuit_graph.rs` canonical graph core.
7. Add `atlas.rs` with dictionary, link, residual, and selector structures.
8. Add normal-mode `PackMethod::CircuitAtlas` behind a profile/env gate.
9. Validate atlas decode with strict byte comparison and checksum.
10. Add stream shared-circuit atlas dictionary and atlas-ref nodes.
11. Replace stream recursive split planner with bottom-up DP.
12. Add PNG/image-plane analyzer and typed preflate correction substreams.
13. Add cut rewriting, XOR/affine detection, and compression-aware graph optimization.
14. Expand benchmark corpus and add atlas-specific regression gates.

## Concrete File-Level Changes

### `zbit-rs/src/pack_rules.rs`

- Add `PackMethod::CircuitAtlas`.
- Add `circuit_atlas_total_bytes: Option<usize>` to `PackEvaluation`.
- Update `choose_best_method` to compare atlas candidate after validation.
- Add reason strings that separate atlas dictionary/reference/residual costs.

### `zbit-rs/src/pack.rs` or new `pack/mod.rs`

- Add `CompressionContext`.
- Pass context through pack, recursive, codec, transform, and stream functions.
- Add atlas candidate creation before final method choice.
- Add new decode branch for atlas method.
- Add stats fields for atlas and cache metrics.

### `zbit-rs/src/pack/cache.rs`

- Add `PayloadHash`.
- Add `CompressionCache`.
- Add memory budgeting and optional persistent cache hooks.
- Add collision-safe verification policy.

### `zbit-rs/src/pack/atlas.rs`

- Add `Segment`, `OccurrenceIndex`, `CircuitLinkCandidate`, `ResidualModel`, `AtlasDictionary`, `AtlasRangeOp`, and selector.
- Implement exact-copy, transformed-reference, sparse-patch, and fallback candidates first.
- Keep graph simplification optional at first; make the format capable of storing graph IDs now.

### `zbit-rs/src/circuit_graph.rs`

- Add canonical ID-based graph core.
- Add structural hashing, fanout counts, topological serialization, graph signatures, and basic rewrites.

### `zbit-rs/src/pack/stream.rs`

- Add shared-circuit atlas flag and node kinds.
- Replace per-block `pack_cache` with context/shared cache.
- Implement bottom-up range DP.
- Add key-piece dependency validation.

### `zbit-rs/src/bin/benchmark_real_file.rs`

- Print atlas stats and cache stats.
- Print candidate lower-bound rejection counts.
- Print whether persistent cache was used.

### `zbit-rs/src/bin/benchmark_stream_real_file.rs`

- Print global-output-slice vs shared-circuit-atlas separately.
- Print key-piece dependency validation results.
- Print per-block atlas/fallback/residual byte breakdown.

## Safety and Correctness Rules

- Every selected candidate must roundtrip before becoming eligible for final selection.
- All cross-region references must be range-checked.
- Range programs must reject overlaps and gaps unless explicitly allowed and initialized.
- Hashes are lookup accelerators only; full validation or strong hashes are required for reuse.
- Persistent cache never becomes a decode dependency.
- Stream key-piece resume must be tested for every mode that claims restart support.
- Format version bumps must preserve old decoder behavior.
- Random incompressible data must never grow beyond the current raw-copy safety fallback except for explicitly allowed small metadata overhead in experimental reports.

## Long-Term Vision

The full potential of this project is unlocked when the compressor behaves like a circuit/link optimizer over the entire file, not like a sequence of isolated codec trials.

The desired final behavior is:

- detect that distant byte ranges share a hidden generation rule;
- convert those ranges into a shared circuit plus small residuals;
- simplify the shared circuit graph;
- cache the discovery so repeated runs get faster;
- serialize only the circuits that actually reduce total size;
- let stream blocks reference shared circuits without requiring full-file decode;
- preserve exact byte-for-byte output validation.

At that point, `zBit` becomes a real circuit-based compressor: not merely compressing bytes, but compressing the reusable logic that generates bytes.
