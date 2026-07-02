# AGENTS.md

## Working Rules
- Update this document after every repo edit to help rapid understanding and navigation of the project.
- Put the short license and copyright comment at the top of every `.rs` file.

## Project Navigation
- Rust crate root: `zbit-rs/`
- Main sources: `zbit-rs/src/`
- Integration tests: `zbit-rs/tests/`
- Research/input papers: `papers/`
- Forward implementation roadmap: `ROADMAP.md`
- License file: `LICENSE`

## Recent Updates
- 2026-07-02: Benchmark-suite runtime pass (non-stream). **No format change; all tracked
  compressed outputs byte-identical** (paper `18573`, primary `562799`, cat `2670567`).

  Files modified:
  - **`zbit-rs/src/pack/core.rs`** — (a) prelude candidates (index+huffman chain, deflate,
    zstd, brotli, cheap raw-xz preset-3 estimate) now run concurrently via nested
    `rayon::join`; (b) the raw-xz tuning-matrix skip gate considers all structural
    candidates, tiered: skip when structural ≤ 5/8 of the cheap preset-3 total, else one
    easy XZ-9 probe and skip when structural beats it by > 1/16 (matrix entries are all
    preset-9 variants with a few-percent spread); (c) new
    `CompressionProfile::enable_xz_extreme_winner_refine` (deep/research only);
    (d) `raw_xz_ms` now reports only the cheap estimate + probe/matrix work instead of
    absorbing the recursive/monotonic/adaptive build time.
  - **`zbit-rs/src/pack/transforms.rs`** — forced transform-plan variants are collected
    into an auxiliary list, pre-ranked with the existing cheap 512 KiB sample scorer, and
    only a profile-bounded top slice (fast 2 / balanced 4 / deep+research unbounded) enters
    the full-data XZ-3 Phase-A ranking; the winner tuned-XZ refinement uses the new
    deep/research-only extreme gate; refinement time is now included in the reported
    `recursive transform evaluation` and traced via `plan-eval-phases` under
    `ZBIT_TRACE_RECURSIVE=1`.
  - **`zbit-rs/src/pack/codecs.rs`** — `build_raw_brotli_payload` probes
    `BROTLI_MODE_TEXT` and `BROTLI_MODE_GENERIC` in parallel (lgwin 24) and keeps the
    smaller payload; same stream format, decode unchanged.
  - **`zbit-rs/Cargo.toml`** — `[profile.dev] opt-level = 1` and
    `[profile.dev.package."*"] opt-level = 3` so tests/dev runs use optimized codec
    dependencies; **`zbit-rs/scripts/benchmark_cat_challenge.sh`** now runs `--release`
    (it was the only benchmark script still running debug builds).
  - **`README.md`, `ROADMAP.md`** — refreshed benchmark tables and documented the pass.
    ROADMAP also gained a **"Ratio-Improvement Evaluation Protocol"** section: per-corpus
    byte/wall budgets (paper 0.15 s, primary 3 s, cat 38 s, depth 400 s at balanced,
    release), the acceptance checklist for ratio-motivated changes (strict byte win on
    ≥ 1 corpus, byte-identity elsewhere, budgets respected, every new candidate carries a
    prove-can't-win gate / sample pre-rank / deep-only gating), the queued ratio
    candidates with their time story, and a `check_benchmark_budgets.sh` harness item.

  Benchmark status (balanced, release, same machine, identical bytes):
  - paper `96.7 ms` (was `820 ms` debug), ratio `0.299492`, PASS.
  - primary.3b `2 716 ms` (was `4 784 ms` release-to-release; `9 803 ms` in the old debug
    report), ratio `0.174046`, PASS — the raw-xz matrix is skipped outright.
  - cat `34 343 ms` (was `51 833 ms` release-to-release; `58 945-94 527 ms` debug),
    ratio `0.899361`, PASS — easy-9 probe tier fires; Phase-A plans 35 -> 20; extreme
    refinement dropped at balanced (measured contributing nothing to the winner).
  - depth_anything `359 490 ms` (was `628 278 ms`, both release via the script), ratio
    `0.840376`, output bytes identical (`83 380 762`), PASS — the trimmed winner
    refinement and aux-plan bound did not change the adaptive selection.

  Validation: `cargo test -q` PASS (38 lib + integration, non-ignored). Deep/research
  budgets intentionally unchanged (aux trimming and extreme-refine gate do not apply).
- 2026-05-26: Added **`papers/format_description.md`**, a code-grounded technical
  description of the current binary formats for optimization work. It documents `.zbpk`
  v4, `.zbps` v1, and `.zbit` v1: byte order, varints, bit-stream conventions,
  complete `.zbpk` method ids and dictionaries/payloads, recursive-circuit topology and
  multi-block bit layouts, stream node records, validation invariants, and high/low-value
  optimization targets. This should be the first reference when changing the output format.
- 2026-05-26: Added ZBPK v4 `raw-brotli` as a bounded text-codec payload method after
  probing the user's "compress the output format again" idea. External wrapper probe results:
  zstd/xz/brotli all lost on the paper and primary `.zbpk` artifacts; zstd saved only
  58 bytes on the 83 MB depth artifact before any wrapper metadata, so live outer repacking
  was not worth the format complexity. The useful ratio win was a direct no-dictionary
  Brotli q11 candidate for text-like inputs.

  Files added or modified:
  - **`zbit-rs/Cargo.toml` / `Cargo.lock`** — added `brotli = "8"` dependency.
  - **`zbit-rs/src/pack_rules.rs`** — `PackMethod::RawBrotli` added at method index 10;
    `AdaptiveTransformedXz` moved to 11; method selection now ranks raw-brotli by exact
    total packed bytes.
  - **`zbit-rs/src/pack/codecs.rs`** — `build_raw_brotli_payload` and bounded
    `decode_raw_brotli_payload` implemented.
  - **`zbit-rs/src/pack/core.rs`** — `ZBPK_VERSION` bumped to 4; `raw_brotli_ms` and
    `raw_brotli_candidate_bytes` added to stats; candidate gated by an 8 MiB text-likeness
    check so binary corpora skip q11 Brotli cost.
  - **`zbit-rs/src/pack/stream.rs`** — updated `write_pack_bytes` callsites for the new
    raw-brotli payload slot (stream mode does not evaluate raw-brotli).
  - **`zbit-rs/src/pack/bitstream.rs` / `recursive.rs`** — updated future
    `CircuitBitStream` migration comments now that v4 is consumed by raw-brotli.
  - **`zbit-rs/src/bin/benchmark_real_file.rs`** — report now prints raw-brotli candidate
    size and timing.
  - **`zbit-rs/src/pack/tests.rs`** — added `adaptive_pack_can_choose_raw_brotli_and_roundtrip`
    over the paper markdown corpus.
  - **`README.md`, `zbit-rs/README.md`, `ROADMAP.md`, `AGENTS.md`** — documented v4,
    raw-brotli, benchmark results, and the outer-wrapper probe.

  Benchmark status:
  - Paper corpus improved from `raw-xz` `20 561 B` (`0.331549`) to `raw-brotli`
    `18 573 B` (`0.299492`), validation PASS.
  - `primary.3b` remains `monotonic-delta` `562 799 B` (`0.174046`), validation PASS;
    raw-brotli skipped by text-likeness gate.

  Validation: `cargo check --manifest-path zbit-rs/Cargo.toml` PASS;
  `cargo test -q --manifest-path zbit-rs/Cargo.toml` PASS (38 lib tests + non-ignored
  integration tests). `cargo fmt` was attempted but blocked because `rustfmt` is not
  installed for the active stable Apple toolchain.
- 2026-05-26: Implemented IMMED-5 (parallelism) + IMMED-1 (`CircuitBitStream` primitive) +
  IMMED-2 (hierarchical Boolean decomposition above 16 inputs) + IMMED-3 acceptance test
  layer; documented IMMED-4 vendor-API blocker. All 42 lib tests PASS; smoke roundtrip on
  paper corpus PASS (`raw-xz` selected, ratio `0.331549`, validation PASS).

  Files added or modified:
  - **`zbit-rs/src/pack/bitstream.rs` (new)** — `CircuitBitStream` + `CircuitBitStreamReader`
    + `BitSerializable` trait. Definition tag = 0 + inline content; reference tag = 1 +
    `ceil(log2(N))` bits. Reader snapshots bit-ranges into the source buffer so refs replay
    exact wire bits (T does not need `Clone`). Includes `BitSerializable` impls for the live
    `CircuitTransformPlan` and `CircuitTopologyNode` types so future format migration is
    drop-in. 8 tests covering all ROADMAP IMMED-1/3 acceptance criteria.
  - **`zbit-rs/src/pack/mod.rs`** — `include!("bitstream.rs")` added.
  - **`zbit-rs/src/pack/core.rs`** — `CompressionBudgets` struct with `max_parallel_codec_threads`
    field; threaded into `compress_adaptive_to_bytes`/`compress_stream_to_bytes` via
    `rayon::ThreadPoolBuilder::install` at the top of each entry point. `total_compression_ms`
    and `compression_throughput_mib_s` added to `CandidateTimingStats` and reported on every
    `PackStats`/`StreamPackStats`.
  - **`zbit-rs/src/pack/stream.rs`** — same wrapper pattern for the stream entry point;
    `stream_finalize_timings` helper centralises the wall-clock-to-MiB/s conversion.
  - **`zbit-rs/src/pack/recursive.rs`** — migration pointer comment above the multi-block
    trailer documenting the v4 bump shape (replace hand-rolled per-section bit writers with
    `CircuitBitStream::write_def_or_ref::<CircuitTransformPlan>` calls).
  - **`zbit-rs/src/hierarchical.rs` (new)** — Shannon cofactor decomposition above the
    `LEAF_BUDGET = 16` exact-minimizer bound. `FunctionDescription` trait (callable, so the
    function can describe arbitrary `n ≤ 64` inputs without paying `2^n` upfront).
    `decompose(&f, splitting_order)` returns a `HierarchicalCircuit` tree of `Leaf` / `Mux` /
    `Const` nodes. Probe-based essential-input pruning (`probe_essential_inputs`) detects
    variables the function does not depend on so a 32-input function with 4 active inputs
    decomposes to a few small leaves instead of 2^16 redundant ones. 5 tests including the
    ROADMAP-2 32-input periodic-stride acceptance.
  - **`zbit-rs/src/lib.rs`** — `pub mod hierarchical;` added.
  - **`ROADMAP.md`** — IMMED-4 section rewritten with the concrete vendor-API blocker
    (lines 237-285): `ReconstructionData` is private in preflate-rs, `PredictionDecoder`
    ordering is driven by token_predictor replay not by the byte stream, typed-substream
    re-encoding loses the interleaving order the recreate consumer expects. Revised fix
    plan requires an upstream patch first.

  **Status of IMMED items (`ROADMAP.md` lines 75-312):**
  - IMMED-5 (parallelism) — **landed**. Most par_iter sites were already in place;
    `CompressionBudgets` and `compression_throughput_mib_s` are new.
  - IMMED-1 (`CircuitBitStream`) — **landed at the primitive + integration-test layer**.
    Live format migration (replacing the v3 compact-topology section with `CircuitBitStream`)
    documented as a v4 bump in `recursive.rs` and left for a focused next session.
  - IMMED-2 (hierarchical decomposition) — **landed**. BDD intermediate (`10 < n ≤ 24`)
    deferred per the ROADMAP decision-table comment in source; the Shannon path covers the
    32-input ROADMAP-2 acceptance.
  - IMMED-3 (cross-region sharing) — **acceptance test layer landed** in
    `immed_3_distant_regions_share_topology`. The ROADMAP says IMMED-3 "is the consequence of
    IMMED-1 and IMMED-2"; the end-to-end win awaits the Phase-2 cross-region encoder that
    actually emits multi-region transforms.
  - IMMED-4 (typed CABAC correction substreams) — **blocked on vendor patch**, see ROADMAP
    update above.

  Tracked benchmark ratios are unchanged (no format changes shipped this session). Memory
  index updated in `~/.claude/projects/.../memory/MEMORY.md` with `project_immed_status.md`.
- 2026-05-23: Two real structural changes on top of the v3 dictionary compaction.
  1. **Multi-block plan dictionary** (`zbit-rs/src/pack/recursive.rs`). The first
     concrete "classic dictionary compression through repeated circuits" step. The
     multi-block trailer now writes:
     - `varint block_count`, `varint block_size`
     - `varint plan_table_len` plus the **deduplicated** unique `(kind_index u8, varint
       period, varint head)` triples
     - a bit stream of per-block indices, each using exactly `ceil(log2(plan_table_len))`
       bits.
     When every block shares one plan the per-block cost collapses to **0 bits**; when
     there are 2 distinct plans across N blocks the cost is N bits + one extra plan in
     the table. Replaces the prior "one full plan triple per block" layout.
  2. **`transformed_encoded_len` removed from the recursive-circuit dictionary**. It is
     redundant: `transformed_encoded_len = payload_size − correction_encoded_len`, and
     both other values are already on the wire (ZBPK header and the recursive dict
     respectively). Saves a varint per recursive-circuit-xz file.
  3. **Runtime raw-xz skip when adaptive clearly wins**
     (`zbit-rs/src/pack/core.rs::compress_adaptive_to_bytes`). The flow now:
     - computes a cheap raw-xz estimate via a single `xz_encode_easy_preset(input, 3)`
       call (1 XZ-3 encode, no tuning matrix)
     - runs the framed/recursive/adaptive paths
     - **skips** the full raw-xz tuning matrix when adaptive's encoded size is at least
       1 KiB smaller than the cheap raw-xz estimate (in that case raw-xz cannot win, so
       the matrix walk would be pure waste). The cheap payload is used as the raw-xz
       candidate and loses selection to adaptive.
     For depth_anything this drops compression time from `1 405 690 ms → 628 278 ms`
     (~2.2x faster vs prior, ~8.6x faster vs the original baseline). For
     paper/primary/cat the path is unchanged (adaptive skipped by size/ratio/recursive
     gates respectively, so the full raw-xz tuning still runs).

  ROADMAP.md gained an "Honest Note on Dictionary Compaction Limits" section: with all
  five dictionary sections bit-packed, the dictionary footprint on cat is now ~110 B
  out of 2 670 567 B compressed output (~0.004 %). Further header-level compaction
  cannot meaningfully change ratio; the payload (>99.99 % of every file) is what
  matters. The next real ratio levers are documented there: the cross-region Circuit
  Atlas, container-aware paths (PyTorch tensor-aware compression in particular), and
  — much further out — replacing XZ with a context-adaptive entropy coder.

  Tracked ratios held: paper `0.331549`, primary.3b `0.174046`, cat `0.899361` (151 B
  saved cumulatively since the original v2 baseline), depth_anything `0.840376`. All
  PASS validation. All 22 lib tests + integration suite PASS.
- 2026-05-23: Bumped `ZBPK_VERSION` to **3** and applied the same bit-wise / enumeration
  cut-out philosophy to every dictionary section of the on-disk format. The previous
  rounds had touched only the topology nodes; this round closes the gap so no field of
  any pack method spends bytes on combinations the enumeration never uses.

  Format changes (all v3):
  - **ZBPK header** (`zbit-rs/src/pack/core.rs`). Was: `magic u32 + version u16 + flags
    u16 + method u8 + bits_per_symbol u8 + unique_count u16 + 3 × u64 sizes` = 36 fixed
    bytes. Now: `magic + version + flags` kept fixed; `method` and `bits_per_symbol`
    packed into one byte (4 bits each, covering 16 method slots and 0..=15 bits-per-
    symbol); `unique_count`, `original_size`, `dict_size`, `payload_size` written as
    `push_varint_u64`. Header on a 62 KB paper file shrinks from 36 B to 17 B.
  - **Framed-payload dictionary** (`zbit-rs/src/pack/recursive.rs`). The six u32 size
    fields (prefix_len, suffix_len, base_chunk_len, full_chunk_count, tail_chunk_len,
    total_chunks) are now varints. Frame tag stays a fixed 4 bytes. Typical fixed
    section drops from 28 B to ~12 B.
  - **Recursive-circuit fixed section** (51 B → ~25 B). The four `u64` size fields
    (plain_len, transformed_encoded_len, correction_plain_len, correction_encoded_len)
    are varints. Codecs and transform kind are packed into a single `u16`: bits 0..1 =
    transformed_codec, bits 2..3 = correction_codec, bits 4..9 = transform_kind index
    (49 values mapped to 0..48 via `kind_to_compact_index`), bits 10..15 reserved.
    `period` and `head` are varints.
  - **Multi-block plan trailer**. `block_count` and `block_size` are varints; each
    per-plan entry is `kind_index u8 (6 bits used) + varint period + varint head`,
    replacing the prior fixed `u8 + u32 + u32` = 9 B per entry with typically 3-5 B.
  - **Adaptive-transformed-xz dictionary** (18 B → ~9 B). One packed byte
    `(codec:2 | kind_index:6)` replaces the prior `kind u8 + codec u8`; `period`,
    `head`, and `plain_len` are varints.
  - **Monotonic-delta dictionary** (28 B → ~12 B). One packed byte
    `(width-1:3 | mode:3 | codec:2)` replaces the prior `width u8 + mode u8 + codec u8`;
    `trailing_zero_shift` reserves the top 2 bits of its byte; `count`, `first_value`,
    `transformed_plain_len` are varints.

  All five share the same kind-index dense table from the topology bit-packing, so the
  49 distinct `CircuitTransformKind`-derived values fit in 6 bits everywhere. Legacy
  v2 files no longer decode (intentional — the format was experimental and v2 artefacts
  were ephemeral benchmark output already cleaned up by the scripts).

  Measured on the existing corpus (all PASS validation):
  - paper `0.331855 → 0.331549`, **−19 B** (`20580 → 20561`), time 64 → 71 ms.
  - primary.3b `0.174058 → 0.174046`, **−37 B** (`562836 → 562799`), time 4 538 → 5 307 ms.
  - cat `0.899380 → 0.899363`, **−53 B** (`2670624 → 2670571`), time 56 814 → 58 945 ms.
    Cumulative savings since the original v2 baseline: 147 bytes on the 5-node cat
    topology + the new dictionary compaction.
  - depth_anything: pending re-measure with the v3 format; the adaptive-transformed-xz
    dictionary drops from 18 B to ~9 B and the header from 36 B to ~22 B → expected
    ~23 B saving in the dictionary footprint on the 99 MiB corpus.

  Tests: existing 22 lib tests + integration suite all PASS. The
  `multi_block_apply_invert_roundtrip` test was updated to compute the expected extra
  bytes via `multi_block_section_size(...)` instead of the prior constant-width
  assumption.
- 2026-05-23: Three combined improvements to compression time + circuit serialisation
  efficiency.
  1. **Bounded framed-payload analyzer** (`zbit-rs/src/pack/recursive.rs`). The
     `build_framed_payload_run` scanner now caps each individual frame at 64 MiB and
     caps cumulative CRC32 work at 256 MiB across the whole scan; after committing to a
     valid multi-frame run, `start` advances past the run instead of `+= 1`. On a 95 MiB
     PyTorch model that previously burned 64.6 minutes in framed_extraction (false-
     positive CRC32 checks over megabytes per offset on non-framed input), the analyzer
     now returns in milliseconds. On cat (real framed-deflate input), the same change
     drops framed_extraction from 215 ms to 2.8 ms because we no longer re-probe every
     offset after the run is found.
  2. **Tuned-XZ candidate cache** (`zbit-rs/src/pack/codecs.rs` +
     `zbit-rs/src/pack/core.rs`). New `tuned_xz_outputs` namespace in the compression
     cache keyed by `(payload_hash, allow_xz_extreme, profile)`. Avoids the duplicate
     full XZ matrix walk that previously fired both inside `build_raw_xz_payload` and
     inside any other caller hashing the same byte stream. Reported as
     `tuned-xz hits/misses` in the benchmark report.
  3. **Bit-packed topology serialisation** — the headline circuit-representation win.
     Legacy topology nodes used a fixed 28-byte (224-bit) per-node on-disk layout
     (id/parent u32 each + relation u8 + order u16 + kind u8 + param_a/b u32 each +
     8-byte hash) — almost none of which actually used all its bits in practice. The new
     form writes the whole topology as a single MSB-first bit stream where every field
     consumes only the bits it needs:
     - **relation**: 1 bit
     - **order**: 2 bits (builders never emit > 3)
     - **kind_index**: 6 bits (49 distinct kind values mapped to a dense 0..48 range;
       0..26 for normal topology kinds, 27..49 for embedded correction-plan kinds)
     - **is_root**: 1 bit
     - **parent_index**: `ceil(log2(prev_count))` bits when not root — 0 bits when there
       is only one possible parent, 1 bit for nodes #2..#3, 2 bits for #4..#5, …
     - **param_a / param_b**: nibble varints (3 value bits + 1 continuation bit per
       nibble; head=1 costs 4 bits, period=6401 costs 20 bits)
     - **id**: implicit (= position + 1), 0 bits on the wire
     - **hash64**: removed; the overall decode pipeline already validates the topology
       end-to-end through inverse transforms.

     A trivial root node spends 18 bits (was 224 bits); a child node with a PNG-stride
     6401 period spends 34 bits (was 224). The whole 5-node cat topology shrinks from
     140 bytes to 18 bytes — an ~8x compaction on the topology itself. Signalled by
     `0x4000 RECURSIVE_TOPOLOGY_COMPACT_FLAG` on the topology-count `u16` (composes
     with the existing `0x8000 MULTI_BLOCK_FLAG`); legacy dictionaries without the flag
     continue to decode with the fixed-width path and full hash verification.

  Measured impact on the existing corpus:
  - paper `0.331855` ratio unchanged, time `101 → 64 ms` (~1.6 x faster).
  - primary.3b `0.174058` ratio unchanged, time `4 780 → 4 538 ms`.
  - cat ratio **improved** `0.899412 → 0.899380` (94 fewer bytes thanks to bit-packed
    topology — the 5-node cat topology shrinks from 140 B to ~18 B on the wire), time
    `60 335 → 56 814 ms`.
  - depth_anything: `5 429 743 → 1 405 690 ms` (~3.9 x faster); framed_extraction alone
    `3 877 752 → 255 ms`. Ratio unchanged at `0.840376`. The remaining cost is
    legitimate XZ work: 946 s in raw-xz tuning + 289 s in adaptive transform evaluation.
  Tests: new `compact_topology_node_size_beats_legacy_layout` exercises the per-node
  sizing math; the existing `multi_block_apply_invert_roundtrip` was updated to
  recompute the expected dictionary footprint via the size formula instead of relying
  on the legacy 28-byte-per-node assumption. All 22 lib tests + integration suite PASS.
- 2026-05-23: Added a new tracked benchmark corpus (PyTorch model file) and landed a new
  pack method `AdaptiveTransformedXz` that delivers a concrete ratio win on it.
  - New script `zbit-rs/scripts/benchmark_depth_anything.sh` (downloads
    `depth_anything_v2_vits.pth`, runs the standard `zbit-benchmark` binary, writes
    `zbit-rs/benchmark_depth_anything_latest.txt`). Mirrored ignored integration test
    `zbit-rs/tests/depth_anything_benchmark.rs`. `.gitignore` updated to skip the
    downloaded asset.
  - New `PackMethod::AdaptiveTransformedXz` (u8=10) extends the transform-plan search
    machinery to inputs that are *not* framed deflate (e.g. PK-ZIP-wrapped PyTorch tensor
    archives). On the cat-challenge PNG and other framed-deflate inputs this method is
    skipped because recursive-circuit-xz already exercises the same search.
    Implementation: `build_adaptive_transformed_xz_stream` in
    `zbit-rs/src/pack/transforms.rs` calls `choose_adaptive_transform_plan` directly on
    the raw input, encodes the best transformed payload with the full codec / tuned-XZ
    selection, and stores `(transform_kind, period, head, codec, plain_len)` (18 bytes)
    in the dictionary. Two cost gates: (a) skip when recursive-circuit-xz already
    evaluates the same transforms; (b) skip when raw-xz ratio ≤ 0.30 (already strong); a
    128 KiB size threshold also keeps small text inputs out of the slow plan search.
  - New depth_anything benchmark snapshot (PASS validation): raw-xz candidate `90 414 940`
    bytes, adaptive-transformed-xz candidate `83 380 790` bytes — adaptive wins by
    ~7 MB (~7.8 % of compressed size). Final ratio `0.840376` on 99 218 434 input bytes.
  - Tracked ratios on the other corpora unchanged: paper `0.331855` (96 ms, adaptive
    skipped by size gate), primary.3b `0.174058` (6 229 ms, adaptive skipped by raw-xz
    ratio gate), cat `0.899412` (recursive-circuit-xz still selected, adaptive skipped
    by recursive gate).
  - Tests: new `pack::tests::adaptive_pack_can_choose_adaptive_transformed_xz_and_roundtrips`
    exercises the new method end-to-end; all 21 lib tests + integration suite PASS.
- 2026-05-23: Landed **N3 per-block transform plans** (ROADMAP item) and investigated **N2 typed correction substreams** end-to-end. N3 splits the inflated plain payload of `recursive-circuit-xz` into N consecutive blocks (deep: 2/4 candidates; research: 2/4/8), runs `choose_adaptive_transform_plan` per block (budget=1 to bound per-block cost), applies each block-local plan, concatenates the transformed bytes, and re-encodes them with a single codec pass (so XZ matches can still cross block boundaries). The format extension is a backward-compatible bit on the on-disk topology-count `u16`: when the top bit is set, a `block_count u32 + block_size u32 + N × (kind u8, period u32, head u32)` trailer follows the topology nodes. Legacy single-plan dictionaries decode unchanged. Implementation in `zbit-rs/src/pack/transforms.rs` (`build_multi_block_transform`, `MultiBlockTransformResult`), `zbit-rs/src/pack/recursive.rs` (template-pick, write/decode multi-block branch), `zbit-rs/src/pack/core.rs` (`MultiBlockPlan` struct, `RECURSIVE_TOPOLOGY_MULTI_BLOCK_FLAG`, `RecursiveCircuitStream.multi_block`). Empirical: cat-challenge multi-block tries `split=2: 2491916 bytes` and `split=4: 2495460 bytes` vs single-plan `2386140 bytes`; single-plan wins by ~100 KB because cat IDAT plain is uniformly-structured (per-block rearrangement destroys XZ cross-block matches). N3 is therefore neutral on cat but format-ready to *win* on heterogeneous inputs where blocks have substantially different best plans. N2 investigation (in `ROADMAP.md`): the preflate corrections payload is CABAC-encoded by `preflate-rs::cabac_codec`, so the bytes are already arithmetic-coded — they cannot be split structurally without modifying `preflate-rs` to emit one CABAC stream per `CodecCorrection` family (documented in ROADMAP). New unit test `multi_block_apply_invert_roundtrip` exercises the new format. All 20 lib tests + integration tests PASS. Tracked benchmarks unchanged: paper `0.331855` / 101 ms, primary.3b `0.174058` / 4 780 ms, cat `0.899412` / 58 026 ms.
- 2026-05-23: Added a near-term ratio-improvement roadmap section to `ROADMAP.md` and landed two new tail-only transform families plus deep-search XZ tunings. New transforms in `zbit-rs/src/pack/core.rs` (`CircuitTransformKind::PeriodicHeadTailTailRowDelta` / `RowXor` / `RowUp` for PNG-style Sub/XOR-stride/Up predictors over the tail of `periodic-head-tail(period, head=1)`, and `PeriodicHeadTailTailBitPlaneTranspose` / `…TransposeDelta` for bit-plane transposing only the row-data tail). Implementations + inverses + topology nodes added in `zbit-rs/src/pack/transforms.rs`; new variants force-included in the candidate pool around the discovered top period, gated by the existing `full_eval_transform_plans()` budget so balanced does not pay extra full XZ encodes for losing candidates. `zbit-rs/src/pack/codecs.rs` now also exposes a `deep_search_xz_budget(profile)` and two tuning entries with `depth=2000`/`depth=4000` for deep/research-profile winner refinement (off for fast/balanced to keep the eval bounded). On the existing corpus the bit-plane and row predictors lost the XZ-3 ranking on the already-PNG-filtered cat plain (the simpler `periodic-head-tail` wins by ~23 KB on XZ-9), but they remain available for unfiltered raster payloads where they should win cleanly. Tracked benchmark reports refreshed; all ratios unchanged (paper `0.331855`, primary `0.174058`, cat `0.899412`, cat stream `0.899455`) and validation/resume all PASS.
- 2026-05-23: Sped up the adaptive transform/codec selection without changing any tracked ratio. In `zbit-rs/src/pack/transforms.rs` the recursive transform evaluator now scores every selected plan once with a cheap XZ-3 encode and only runs the expensive `choose_best_codec` on the top-`full_eval_transform_plans()` plans (`fast=1, balanced=2, deep=4, research=8`). `score_periodic_candidates` deduplicates near-equal periods (drops candidates within 8 of an already-kept period) to stop wasting work on six adjacent periods around an image stride. `tuned_xz_budget` in `zbit-rs/src/pack/codecs.rs` was trimmed (`balanced (10,6) -> (4,3)`, `deep (14,10) -> (10,6)`) to cut the winner-refinement XZ-9 batch. `apply_periodic_delta` / `apply_periodic_xor` now accept `period == 1` so head-tail-tail-delta/xor can express plain unary delta/xor on the tail, and the tail candidates include `tail_delta_period=1`. Forced tail candidates now include `tail_gather_period=1` plus `PeriodicHeadTailTailGatherDelta`. A `delta_dist` field was added to `XzTuningParams` (currently inert because xz2 does not expose the LZMA delta filter; tunings ship the field for forward compatibility). Refreshed reports show same-ratio runs with substantial speedups vs the 2026-05-07 baseline: paper `313.161 ms -> 122.009 ms` (~2.6x), primary `16634.566 ms -> 4715.248 ms` (~3.5x), cat `112499.682 ms -> 60335.199 ms` (~1.9x), multilevel `realtime-deep 280198.816 ms -> 193723.466 ms`. Validation/resume all PASS. Tracked outputs refreshed: `zbit-rs/benchmark_latest.txt`, `zbit-rs/benchmark_primary.3b_latest.txt`, `zbit-rs/benchmark_cat_challenge_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt`; root `README.md` benchmark tables updated to match.
- 2026-05-14: Reworked stream-mode performance tiering for sustainable benchmark tradeoffs by adding profile-aware realtime codec/planner policies in `zbit-rs/src/pack/core.rs`, `zbit-rs/src/pack/codecs.rs`, and `zbit-rs/src/pack/stream.rs`: fast/balanced/deep now use distinct realtime deflate/zstd settings, stream split-search budgets, and shared/global recursive gating (`shared grouping payload` disabled for fast; recursive on shared payload limited to deep/research). Updated multilevel benchmark runner `zbit-rs/scripts/benchmark_cat_challenge_stream_multilevel.sh` to bind each profile row to explicit `ZBIT_COMPRESSION_PROFILE` values and refreshed `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt` plus root `README.md` profile table. New cat multilevel snapshot (PASS output + PASS resume): `realtime-fast` `2969404 -> 2966468` (`0.999011`, `1444.139 ms`), `realtime-balanced` `2969404 -> 2965252` (`0.998602`, `3807.462 ms`), `realtime-deep` `2969404 -> 2670846` (`0.899455`, `280198.816 ms`), `wide-overfit` `2969404 -> 2670846` (`0.899455`, `257437.597 ms`).
- 2026-05-08: Strengthened the root `README.md` opening narrative around zBit as an experimental bits-to-Karnaugh-map circuits compression algorithm, added sections explaining its difference from classic compression, the Boolean/Karnaugh/circuit mental model, and the high-level algorithm narrative/Circuit Atlas direction; refreshed the `.zbpk` method list and added a crate-local `zbit-rs/README.md` intro with the same positioning.
- 2026-05-08: Added `zbit_algorithm_structure.md` as a human/agent architecture guide connecting the theory in `papers/` with the current `zbit-rs/` implementation: file-as-Boolean-map/Karnaugh intuition, exact/heuristic/SAT minimization, canonical circuit DAGs, adaptive pack candidates, recursive transform topology, streaming piece/group/global-slice planning, implementation reference tables, limitations, and future Circuit Atlas direction.
- 2026-05-07: Applied a roadmap-aligned generic codec-search enhancement in `zbit-rs/src/pack/codecs.rs` by adding winner-selection over a broader tuned LZMA2/XZ matrix (custom `lc/lp/pb`, dictionary sizes, nice lengths, and match-finder variants for both normal and extreme presets), and wired recursive transformed-payload refinement in `zbit-rs/src/pack/transforms.rs` to reuse this tuned XZ chooser while keeping candidate-selection structure stable. Validated with `cargo check`, targeted pack tests, and benchmark reruns: paper benchmark improved from `62015 -> 20632` (`0.332694`) to `62015 -> 20580` (`0.331855`, `PASS`) in `zbit-rs/benchmark_latest.txt`; tracked cat normal (`2670718`, `0.899412`) and cat stream (`2670846`, `0.899455`) remain unchanged in ratio with PASS output/resume validation during verification runs.
- 2026-05-07: Completed second-phase `pack/` decomposition to make editing and roadmap work tractable: split `zbit-rs/src/pack/core.rs` into focused files `zbit-rs/src/pack/format.rs` (binary read/write helpers), `zbit-rs/src/pack/codecs.rs` (raw codec + monotonic + codec selection/cache), `zbit-rs/src/pack/transforms.rs` (reversible transforms + adaptive/topology planning), and `zbit-rs/src/pack/recursive.rs` (framed run + preflate recursive-circuit path), with `zbit-rs/src/pack/core.rs` reduced to shared types/index-huffman and high-level pack orchestration. Updated `zbit-rs/src/pack/mod.rs` include order accordingly and preserved license headers in all Rust files. Validation: `cargo check` and full `cargo test` PASS (all non-ignored tests).
- 2026-05-07: Refactored `zbit-rs/src/pack.rs` into a `zbit-rs/src/pack/` module layout to improve maintainability and editing flow without behavior changes: moved implementation into `zbit-rs/src/pack/core.rs`, `zbit-rs/src/pack/stream.rs`, and `zbit-rs/src/pack/tests.rs`, added `zbit-rs/src/pack/mod.rs` as module entry (`include!` composition), and removed the former monolithic file `zbit-rs/src/pack.rs`. Preserved license headers on all new Rust files and validated with `cargo check` plus full `cargo test` (all non-ignored tests PASS).
- 2026-05-07: Applied the first roadmap Circuit Atlas groundwork in `zbit-rs/src/pack.rs` by adding a shared `CompressionContext` (`profile`, timings, skipped-candidate notes), stable payload fingerprinting (`xxh3` + `blake3`), and compression caches for codec outputs, preflate chain results, and stream range candidates with hit/miss telemetry (`CacheStats`) surfaced through `PackStats`/`StreamPackStats` and benchmark reports (`zbit-rs/src/bin/benchmark_real_file.rs`, `zbit-rs/src/bin/benchmark_stream_real_file.rs`). Stream planning now uses a stream-level absolute-range cache across key-piece blocks instead of per-block ephemeral range caches. Added dependencies in `zbit-rs/Cargo.toml` (`blake3`, `xxhash-rust`), updated lockfile, and refreshed tracked cat reports (`zbit-rs/benchmark_cat_challenge_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_latest.txt`): ratios remain at current best baselines (`0.899412` normal, `0.899455` stream wide-overfit) with PASS validation/resume, and reports now include cache hit/miss telemetry per namespace.
- 2026-05-07: Replaced `ROADMAP.md` with a code-grounded Circuit Atlas implementation roadmap focused on drastic ratio improvements through content-addressed caches, nonlocal cross-region circuit linking, real canonical circuit graphs, atlas candidate selection, normal-mode `CircuitAtlas` packing, stream shared-circuit dictionaries, restart-safe atlas references, container/image-plane modeling, preflate correction substreams, and expanded benchmark/regression targets.
- 2026-05-07: Tried an additional low-overhead ratio optimization pass in `zbit-rs/src/pack.rs` by extending adaptive transform search with reversible bit-plane transforms (`bit-plane-transpose`, `bit-plane-transpose-delta`, `bit-plane-transpose-xor`) and serializing matching topology nodes for recursive/correction modeling. Validated with targeted pack tests and refreshed tracked benchmark reports (`zbit-rs/benchmark_latest.txt`, `zbit-rs/benchmark_primary.3b_latest.txt`, `zbit-rs/benchmark_cat_challenge_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt`) plus root `README.md` benchmark tables. Current measurements show no compressed-size/ratio change on tracked corpora (paper `20632`, primary `562836`, cat `2670718`, stream `2670846`), while runtime remains in the same optimized band (cat real-file ~`112.5s`, stream wide-overfit ~`115.1s`, all validation/resume checks PASS).
- 2026-05-07: Implemented roadmap-aligned compression optimization controls in `zbit-rs/src/pack.rs` with profile-driven candidate budgets (`ZBIT_COMPRESSION_PROFILE`: `fast`/`balanced`/`deep`/`research`), deterministic parallel candidate evaluation (`rayon`) for XZ profile probes, transform-plan scoring/evaluation, and preflate chain exploration, plus detailed candidate/stage timing capture and skipped-candidate reporting (`CandidateTimingStats`) surfaced through `PackStats`/`StreamPackStats` and benchmark reporters (`zbit-rs/src/bin/benchmark_real_file.rs`, `zbit-rs/src/bin/benchmark_stream_real_file.rs`). Stream planning now disables expensive local recursive root candidates when shared global grouping payload is active, reducing duplicate heavy work while preserving validation guarantees. Added `rayon` dependency in `zbit-rs/Cargo.toml` and refreshed reports (`zbit-rs/benchmark_latest.txt`, `zbit-rs/benchmark_primary.3b_latest.txt`, `zbit-rs/benchmark_cat_challenge_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt`) plus root `README.md` benchmark tables/docs. New benchmark snapshot keeps ratios while significantly reducing compression time on large cat workloads: real-file cat `617251.656 ms -> 100856.977 ms` at `0.899412`, wide-overfit stream `621159.756 ms -> 108493.158 ms` at `0.899455`, multilevel `realtime-balanced` `1079593.691 ms -> 124218.269 ms` at `0.899455`, and `realtime-deep` `1054402.704 ms -> 127240.705 ms` at `0.899455` (all output and resume validations PASS).
- 2026-05-07: Added `ROADMAP.md` with a prioritized plan for improving cat challenge ratio and compression speed: per-candidate timing/profiles, deterministic parallel candidate evaluation, transform/preflate/codec memoization, container/image-plane modeling, preflate correction substream modeling, canonical ID-based circuit-map generation, AIG/XAG cut rewriting, compression-aware graph cost extraction, and streaming block parallel DP/shared dictionaries. The roadmap also captures current cat and stream benchmark baselines plus general references for logic synthesis and compression/container modeling.
- 2026-05-07: Reworked non-wide streaming compression in `zbit-rs/src/pack.rs` from forced adaptive-wide promotion to a hybrid shared-grouping payload flag (`ZBPS_FLAG_SHARED_GROUPING_PAYLOAD`) that keeps `wide_overfitting_circuits=false` while allowing per-block selection between local piece/group packing and global-slice references; updated stream decode flag parsing accordingly, expanded `StreamPackStats` with `shared_grouping_payload_used`, and surfaced this in `zbit-rs/src/bin/benchmark_stream_real_file.rs` output/reporting. Refreshed reports (`zbit-rs/benchmark_cat_challenge_stream_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt`) now improve `realtime-fast` from `0.999011` (2969404 -> 2966468) to `0.899597` (2969404 -> 2671266) while `realtime-balanced`/`realtime-deep` remain `0.899455` with non-wide effective mode and PASS validation/resume checks. Updated root `README.md` benchmark tables and streaming feature notes to reflect these results and resource/timing metrics.
- 2026-05-07: Improved non-wide stream benchmark modes by adding adaptive wide-promotion logic in `zbit-rs/src/pack.rs` for real-time/deep streaming configurations (`max_group_depth >= 2`, large multi-block payloads), with explicit reporting fields in `StreamPackStats` (`effective_wide_overfitting_circuits`, `adaptive_wide_promotion_used`) and stream benchmark report output updates in `zbit-rs/src/bin/benchmark_stream_real_file.rs`; also added safe recursive candidate admission via roundtrip validation before selection to avoid invalid partial-block recursive candidates. Refreshed multilevel stream report `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt` now shows improved ratios for additional profiles: `realtime-balanced` and `realtime-deep` both reach `0.899455` (from prior `0.998624`) with validation PASS and resume PASS.
- 2026-05-07: Implemented multilevel stream benchmark tracking by extending `zbit-rs/src/bin/benchmark_stream_real_file.rs` with configurable stream-mode flags (`realtime_mode`, `wide_overfitting_circuits`, `carry_grouping_history`) and resource metrics (RSS before/after compression/decompression, RSS deltas, peak RSS), added `zbit-rs/scripts/benchmark_cat_challenge_stream_multilevel.sh` to run profile matrix benchmarks (`realtime-fast`, `realtime-balanced`, `realtime-deep`, `wide-overfit`) and generate consolidated report `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt`, and added ignored integration coverage `zbit-rs/tests/cat_challenge_stream_multilevel_benchmark.rs`. Also added matching resource metrics to standard real-file benchmark reports in `zbit-rs/src/bin/benchmark_real_file.rs`, refreshed tracked benchmark reports (`zbit-rs/benchmark_latest.txt`, `zbit-rs/benchmark_primary.3b_latest.txt`, `zbit-rs/benchmark_cat_challenge_latest.txt`, `zbit-rs/benchmark_cat_challenge_stream_latest.txt`), updated stream script args (`zbit-rs/scripts/benchmark_cat_challenge_stream.sh`), and updated root `README.md` benchmark tables to include compression/decompression time and resource usage plus the multilevel stream table section.
- 2026-05-07: Added a `wide_overfitting_circuits` streaming mode in `zbit-rs/src/pack.rs` (new `.zbps` global-overfit payload flag + global-slice node) so stream blocks can decode from key-piece boundaries while sharing one whole-file overfit pack; stream benchmark options now enable this mode by default (`zbit-rs/src/bin/benchmark_stream_real_file.rs`, `zbit-rs/tests/stream_real_file_benchmark.rs`). Refreshed `zbit-rs/benchmark_cat_challenge_stream_latest.txt` at 256 KiB chunks now reaches ratio `0.899455` (`2969404 -> 2670846`, savings `10.05%`, output PASS, key-piece resume PASS), exceeding prior stream ratio `0.998624`.
- 2026-05-07: Patched streaming real-time candidate selection in `zbit-rs/src/pack.rs` to evaluate packed-size winners across `raw-copy`, `raw-deflate`, `raw-zstd`, and structural `framed-raw` (instead of only generic raw codecs), enabling real stream compression on cat challenge blocks while keeping key-piece resume validation. Refreshed `zbit-rs/benchmark_cat_challenge_stream_latest.txt` at 256 KiB chunks now reports ratio `0.998624` (`2969404 -> 2965318`, savings `0.14%`, output PASS, key-piece resume PASS), improving over prior stream ratio `1.000064`.
- 2026-05-07: Implemented real-time streaming compression/decompression in `zbit-rs/src/pack.rs` with a new `.zbps` container (`ZBPS` v1), chunk/key-piece settings (`StreamPackOptions`), multi-level piece/group topology selection, key-piece resume decode support, and optional grouping-history hints; exposed new APIs in `zbit-rs/src/lib.rs` (`compress_adaptive_stream_to_file`, `decompress_stream_file`, `decompress_stream_file_from_key_piece`) and added stream benchmark tooling (`zbit-rs/src/bin/benchmark_stream_real_file.rs`, `zbit-rs/scripts/benchmark_cat_challenge_stream.sh`, `zbit-rs/tests/stream_real_file_benchmark.rs`, `zbit-rs/tests/cat_challenge_stream_benchmark.rs`) plus `.zbps` ignore rule and README usage/docs updates. Generated `zbit-rs/benchmark_cat_challenge_stream_latest.txt` using 256 KiB chunks with key-piece interval 8: `2969404 -> 2969594` (ratio `1.000064`, output validation PASS, key-piece resume PASS).
- 2026-05-06: Expanded root `README.md` benchmark-results section with a concise performance table for the three tracked tests (paper, `primary.3b.bin`, and cat challenge), including selected method, original/compressed bytes, ratio, savings, and validation status, while keeping the report-file paths for latest outputs.
- 2026-05-06: Added a brief benchmark-results location section to root `README.md`, documenting where to write the latest tracked outputs for the three benchmark tests: `zbit-rs/benchmark_latest.txt`, `zbit-rs/benchmark_primary.3b_latest.txt`, and `zbit-rs/benchmark_cat_challenge_latest.txt`.
- 2026-05-06: Added a generic `raw-xz` adaptive pack candidate (`zbit-rs/src/pack.rs`, `zbit-rs/src/pack_rules.rs`) using an appended method ID, added benchmark reporting (`zbit-rs/src/bin/benchmark_real_file.rs`), switched internal XZ candidate streams to `Check::None`, and extended monotonic-delta with trailing-zero gap scaling modes for aligned integer-stream gaps while preserving decode validation. Refreshed reports improved all three tracked ratios: paper `0.338176` -> `0.332694` (62015 -> 20632, raw-xz, PASS), `assets/primary.3b.bin` `0.174132` -> `0.174058` (3233613 -> 562836, monotonic-delta, PASS), and cat challenge `0.899415` -> `0.899412` (2969404 -> 2670718, recursive-circuit-xz, PASS).
- 2026-05-06: Ran additional multi-cycle compression tuning focused on chaotic data (`assets/cat_challenge.png`) by extending recursive preflate evaluation in `zbit-rs/src/pack.rs` (multi-chain `max_chain_length` candidate search: `4096/8192/16384`) and by broadening XZ payload codec probing to include both legacy easy presets and tuned LZMA2 profiles (explicit `lc/lp/pb`, match finder, and nice length variants) while preserving best-size fallback for non-chaotic files. Refreshed reports: cat challenge improved from ratio `0.901576` (2969404 -> 2677142) to `0.899415` (2969404 -> 2670726, validation PASS); `assets/primary.3b.bin` remained at `0.174132` (3233613 -> 563076, PASS); `papers/zbit-algorithmsResearch.md` remained at `0.338176` (62015 -> 20972, PASS).
- 2026-05-06: Added `assets/primary.3b.bin` to real-file benchmark test coverage (`zbit-rs/tests/real_file_benchmark.rs`), introduced adaptive `monotonic-delta` packing (`zbit-rs/src/pack.rs`, `zbit-rs/src/pack_rules.rs`) for strictly increasing fixed-width integer streams (gap-byte / gap-varint / gap-delta-varint modes with codec selection and full decode validation), and extended benchmark reporting with monotonic candidate fields (`zbit-rs/src/bin/benchmark_real_file.rs`); `primary.3b.bin` benchmark improved from `raw-zstd` ratio `0.801232` (3233613 -> 2590875) to `monotonic-delta` ratio `0.174132` (3233613 -> 563076, validation PASS) while paper benchmark remains `raw-zstd` ratio `0.338176` (62015 -> 20972, validation PASS). Added tracked report `zbit-rs/benchmark_primary.3b_latest.txt` and refreshed `zbit-rs/benchmark_latest.txt`.
- 2026-04-30: Added local vendored `preflate-rs` (`vendor/preflate-rs`) and switched `zbit-rs/Cargo.toml` to the path dependency, then implemented multi-candidate predictor selection in preflate first-chunk analysis (evaluate all viable parameter/hash candidates and pick the smallest serialized reconstruction corrections blob); recursive cat benchmark improved from ratio `0.901591` (2969404 -> 2677187) to `0.901576` (2969404 -> 2677142), validation PASS.
- 2026-04-30: Followed up vendored preflate integration by cleaning dead-code warnings for legacy single-candidate estimator entrypoints (`#[allow(dead_code)]` on compatibility functions) so `cargo check`/benchmark runs stay warning-clean while multi-candidate predictor scoring remains active.
- 2026-04-30: Expanded recursive circuit transform search in `zbit-rs/src/pack.rs` with new `periodic-head-tail-delta` / `periodic-head-tail-xor` families and dynamic tail-period probes (including `period-1` / half-tail), plus embedded correction-plan metadata in recursive topology nodes for deterministic decode-time correction inversion; cat benchmark remains at ratio `0.901591` (2969404 -> 2677187 bytes, validation PASS) because preflate corrections stayed incompressible at 284138 bytes across tested reversible transforms.
- 2026-04-30: Removed format-specific naming/references from adaptive packed-stream internals by renaming method/report labels to `recursive-circuit-xz` and by replacing PNG-bound frame extraction with a generic CRC32-framed run detector (stored 4-byte frame tag, deterministic run layout metadata, and exact frame CRC rebuild on decode); benchmark helper script now reports neutral header metadata instead of format-specific fields.
- 2026-04-30: Extended recursive circuit packing in `zbit-rs/src/pack.rs` with richer transform families (periodic gather/delta/xor plus recursive tail transforms), payload codec selection (`raw`/`xz`/`zstd` plus final winner-only `xz-extreme` refinement), and deterministic topology hashing (`hash64` per node) verified during decode; refreshed cat benchmark now reaches ratio `0.901591` (2969404 -> 2677187 bytes, `9.84%` savings, validation PASS).
- 2026-04-30: Reworked `png-preflate-xz` internals in `zbit-rs/src/pack.rs` from PNG-specific row assumptions to a generic adaptive circuit-transform search (identity/delta/xor/periodic-head-tail) driven by stream self-correlation, with explicit recursive circuit topology metadata (unique node IDs, series/parallel relations, and order indices) serialized in the pack dictionary for deterministic pattern matching and reconstruction.
- 2026-04-30: Added adaptive `png-preflate-xz` in `zbit-rs/src/pack.rs` + `zbit-rs/src/pack_rules.rs` (exact `IDAT` deflate reconstruction using preflate corrections plus reversible high-dimensional scanline transform with XZ coding), integrated into pack selection, decode validation, and benchmark reporting (`zbit-rs/src/bin/benchmark_real_file.rs`); refreshed cat benchmark now selects `png-preflate-xz` with ratio `0.901633` (2969404 -> 2677315 bytes, validation PASS), improving over prior `png-idat-raw` ratio `0.998557`.
- 2026-04-30: Added a new adaptive `png-idat-raw` method in `zbit-rs/src/pack.rs` + `zbit-rs/src/pack_rules.rs` that repacks contiguous PNG `IDAT` chunks by storing only concatenated IDAT payload and deterministic chunk layout metadata (CRC recomputed on decode), with full roundtrip validation and benchmark reporting (`zbit-rs/src/bin/benchmark_real_file.rs`); refreshed cat benchmark now selects `png-idat-raw` and improves from no gain (`1.000012`) to positive savings ratio `0.998557` (2969404 -> 2965120 bytes, validation PASS).
- 2026-04-30: Enhanced `zbit-rs/scripts/benchmark_cat_challenge.sh` with PNG sanity reporting (size, resolution, bit depth, color type) and warnings when the downloaded asset differs from the expected 40MB/16-bit HDR profile; current download is ~2.83 MiB, 8-bit RGBA PNG.
- 2026-04-30: Added `raw-zstd` as a new adaptive packing candidate/method with roundtrip decode support and benchmark-candidate reporting; refreshed paper benchmark now selects `raw-zstd` with ratio `0.338176` (62015 -> 20972 bytes, validation PASS), improving over prior `raw-deflate` ratio `0.355849`.
- 2026-04-30: Added cat challenge automation under `zbit-rs/scripts/benchmark_cat_challenge.sh` and ignored test hook `zbit-rs/tests/cat_challenge_benchmark.rs`; script downloads `assets/cat_challenge.png` only if missing and regenerates tracked report `zbit-rs/benchmark_cat_challenge_latest.txt`.
- 2026-04-30: Updated `.gitignore` to ignore `assets/cat_challenge.png` and generated `zbit-rs/*.zbpk` artifacts.
- 2026-04-29: Added `raw-deflate` adaptive pack method (zlib/deflate) with selection-rule integration and benchmark reporting; latest benchmark on `papers/zbit-algorithmsResearch.md` now selects `raw-deflate` with ratio `0.355849` (62015 -> 22068 bytes, validation PASS), improving over prior `indexed-huffman` ratio `0.605595`.
- 2026-04-29: Added large-file decode safety bound in pack parser (`original_size` hard cap at 1 GiB) to prevent unbounded expansion risk during decompression.
- 2026-04-29: Improved adaptive packing by adding `indexed-huffman` (canonical Huffman dictionary + variable-length payload) with decode support and candidate selection logic updates; refreshed benchmark now selects `indexed-huffman` and improves `papers/zbit-algorithmsResearch.md` compression from ratio `0.877433` to `0.605595` (62015 -> 37556 bytes, validation PASS).
- 2026-04-29: Refreshed `zbit-rs/benchmark_latest.txt` from a new benchmark run on `papers/zbit-algorithmsResearch.md` (selected `indexed-raw`, 62015 -> 54414 bytes, ratio `0.877433`, savings `12.26%`, compression `8.764 ms`, decompression `9.791 ms`, output validation PASS).
- 2026-04-29: Implemented advanced library optimization flow in `zbit-rs` with Espresso-style iterative cover heuristics, AIG-style rewrite/resubstitution passes, SAT-assisted local redundancy pruning, and technology-aware objectives (ASIC area/delay, FPGA LUT4/LUT6), plus model entrypoints and new validation tests (`zbit-rs/src/advanced.rs`, `zbit-rs/src/sat.rs`, `zbit-rs/tests/advanced_validation.rs`).
- 2026-04-29: Added `OPENCLAW.md` with a practical handoff guide for continuing this repository with a simpler local AI agent (task scoping, prompt template, validation gates, and escalation criteria).
- 2026-04-23: Replaced root `README.md` with a theory-to-implementation guide aligned to `papers/zbit-algorithmsResearch.md` and current `zbit-rs` capabilities (exact bounded minimization, adaptive packing, validation workflow, and documented non-implemented roadmap items).
- 2026-04-23: Updated moved sample paper path references from `../studies/algorithmsResearch.md` to `../papers/zbit-algorithmsResearch.md` in tests, benchmark binary defaults, and crate README.
- 2026-04-23: Added short license/copyright headers to all Rust source/test files and markdown files under `papers/`.
- 2026-04-23: Updated copyright headers to include year and contact: `Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.`
