# zBitCompressor.rs

**zBit is an experimental bits-to-Karnaugh-map circuits compression algorithm.**

Its central peculiarity is that it does not only look at a file as a linear sequence of bytes. It also tries to treat the file as a Boolean landscape: bits, positions, symbols, chunks, frame payloads, corrections, and transformed ranges can become cells in a Karnaugh-like map. When regions of that map can be grouped, simplified, linked, or represented as circuits with less metadata than the original bytes, zBit can store the circuit/structure instead of storing the data literally.

In a classic compressor, the main question is often: **"which previous byte sequence or symbol distribution can describe this sequence cheaply?"** In zBit, the deeper question is: **"which Boolean/circuit structure explains these bits cheaply, and is the explanation smaller than the raw data plus corrections?"**

This makes zBit closer to an adaptive structural compressor than to a single fixed codec. The current Rust implementation combines exact Boolean minimization, heuristic cover refinement, SAT-assisted local pruning, canonical circuit DAGs, adaptive byte packing, recursive transform topology, and chunked stream grouping. The packer is intentionally conservative: circuit modeling is used only when the model, dictionary, references, and residual corrections beat simpler representations.

## The Peculiarity: Compression by Boolean Structure

The conceptual model is:

```text
file bytes -> bit/position/context map -> Karnaugh-like groups -> simplified circuits -> encoded structure + residual corrections
```

A Karnaugh map groups adjacent equal Boolean outputs so they can be represented by fewer literals. zBit generalizes that idea beyond the small classroom grid:

- **cells** can be individual bits, byte-symbol rows, chunk ranges, transformed positions, framed payload bytes, correction bytes, or stream pieces;
- **ON/OFF/DC sets** describe what must evaluate to `1`, what must remain `0`, and what may be ignored or corrected elsewhere;
- **cubes / implicants** are generalized K-map groups over many dimensions;
- **circuits** are executable descriptions of these groups and relations;
- **compression wins** only when the structural explanation is smaller than the literal bytes.

The algorithm is therefore not simply "find repeated bytes". A useful pattern may be a Boolean relation, a reusable circuit slice, a transformed residual, a monotonic integer stream, a framed-data reconstruction plan, a global stream slice, or a conventional entropy/codec candidate if that is actually smaller.

## Why This Is Different from Classic Compression

| Classic compression tendency | zBit structural/circuit tendency |
| --- | --- |
| Search repeated substrings, dictionaries, or symbol probabilities. | Search Boolean regions, circuit covers, transform topology, reusable slices, and candidate pack structures. |
| Treat data mainly as a byte stream. | Treat data as bytes **and** as bit-level maps over positions, contexts, chunks, and reversible transforms. |
| A match is usually a previous sequence or statistical code. | A match can be a simplified circuit, a cube cover, a frame reconstruction rule, a correction plan, or a stream/global slice. |
| One dominant codec strategy is applied to the whole input or block. | Many reversible representations compete; the smallest validated candidate is selected. |
| Good entropy coding can hide local redundancy. | zBit tries to expose deeper logical structure before choosing how to encode it. |

This is why the project should be read as a **bits-to-Karnaugh-map-to-circuits compressor**: the Boolean/circuit view is the distinctive research direction, while the adaptive packer makes the approach usable on real files without forcing circuits where they are not economical.

## Algorithm Narrative

At a high level, zBit works like a structural search engine for reversible explanations of data:

1. **Map the data into candidate spaces.** The same input may be seen as raw bytes, indexed symbols, small truth tables, frame payloads, transformed ranges, stream chunks, or correction streams.
2. **Find compressible Boolean regions.** For bounded truth-table problems, exact minimization builds implicants like K-map groups. For larger inputs, heuristic and SAT-assisted passes try cheaper local improvements.
3. **Build or reuse circuit-like descriptions.** Canonical nodes, recursive transform nodes, group nodes, and global slices represent structure that can be serialized and later decoded deterministically.
4. **Estimate total cost, not elegance.** A beautiful circuit is rejected if its metadata is larger than raw bytes or a conventional codec candidate.
5. **Validate roundtrip reconstruction.** Every selected representation must decode back to the exact original bytes.

The long-term development direction is a stronger **Circuit Atlas**: a cacheable dictionary of reusable circuits/slices that can link distant and apparently unrelated parts of a file or stream when they share hidden Boolean structure. The current code already contains foundations for that direction through canonical models, adaptive candidates, recursive topology metadata, stream grouping, and global-slice references.

## Scope

This repository currently contains:

- `zbit-rs/`: the active Rust crate
- `papers/`: theory and implementation-guidance documents

The implementation is intentionally aligned with the paper guidance that exact methods are valuable for bounded local problems, while practical compression needs representation-aware heuristics and strict validation.

## Theory -> Implementation Mapping

### 1. Exact two-level minimization in bounded scope

Paper guidance: exact minimization is strongest on small support functions and should be bounded.

Implemented:

- Quine-McCluskey style prime implicant generation
- exact minimum cover selection with branch-and-bound search
- don't-care support in minimization
- hard exact limit: `ZBIT_MAX_INPUTS_EXACT = 16`
- **hierarchical Shannon-cofactor decomposition** above the 16-input exact bound
  (ROADMAP IMMED-2): functions over more than 16 inputs split on a chosen variable,
  recurse on each cofactor, and combine via mux nodes; probe-based essential-input pruning
  detects variables the function does not actually depend on so e.g. a 32-input stride
  function with 4 active bits collapses to a small leaf instead of a deep Shannon tree

Code:

- `zbit-rs/src/minimizer.rs`
- `zbit-rs/src/hierarchical.rs`
- `zbit-rs/src/model.rs`

### 2. Canonical structural representation + rewrite-ready flow

Paper guidance: representation choice matters.

Implemented:

- canonical node interning (`Pin`, `Not`, `And`, `Or`, `Xor`)
- commutative normalization and simplification rules
- deterministic serialized model format (`.zbit`)
- advanced rewrite flow with:
  - graph-style resubstitution (absorbed-term elimination)
  - AIG-like consensus merges (local rewriting)
  - balancing-aware objective estimation

Code:

- `zbit-rs/src/model.rs`
- `zbit-rs/src/advanced.rs`

### 3. Espresso-style iterative cover heuristics

Paper guidance: large search spaces need iterative heuristic improvements in addition to exact bounded methods.

Implemented:

- iterative expand/select loop inspired by Espresso-style cover refinement
- legal expansion under ON+DC constraints
- greedy objective-aware cube selection and irredundancy cleanup

Code:

- `zbit-rs/src/advanced.rs`

### 4. SAT-assisted local exactness

Paper guidance: SAT is useful as a bounded local oracle inside larger heuristic flows.

Implemented:

- lightweight CNF SAT solver (DPLL with unit propagation)
- SAT-driven local redundancy pruning for cubes in a candidate cover
- bounded SAT window control (`sat_local_exact_inputs`)

Code:

- `zbit-rs/src/sat.rs`
- `zbit-rs/src/advanced.rs`

### 5. Technology-aware mapping objectives

Paper guidance: objective function must match target technology, not just literal count.

Implemented:

- objective-aware scoring for:
  - literal minimization
  - ASIC area proxy
  - ASIC delay proxy
  - FPGA LUT4/LUT6 proxies
- advanced model entrypoints with explicit objective selection

Code:

- `zbit-rs/src/advanced.rs`
- `zbit-rs/src/model.rs`

### 6. Representation-aware adaptive packing

Paper guidance: choose method by objective/cost, avoid one fixed algorithm worldview.

Implemented:

- adaptive selection among:
  - `raw-copy`
  - `indexed-raw`
  - `indexed-circuit`
  - `indexed-huffman`
  - `raw-deflate`
  - `raw-zstd`
  - `raw-xz` / `raw-brotli` / `framed-raw` / `recursive-circuit-xz` / `monotonic-delta` /
    `adaptive-transformed-xz`
- rule-based gating for circuit-dictionary evaluation
- size-based final method choice, never worse than raw baseline by design
- strict `.zbpk` parser validation
- **bit-stream interning primitive** (ROADMAP IMMED-1): `CircuitBitStream` writes the
  first occurrence of any circuit-level definition (transform plan, topology node, etc.)
  inline and re-references it at `ceil(log2(N))` bits cost; in-tree primitive plus
  acceptance tests for cross-region sharing (ROADMAP IMMED-3) of distant byte ranges with
  identical structure
- **profile-bounded parallelism** (ROADMAP IMMED-5): `rayon::ThreadPoolBuilder::install`
  wraps the encoder entry points, sized by `CompressionBudgets::max_parallel_codec_threads`
  (2 for `fast`; unbounded for `balanced`/`deep`/`research`)

Code:

- `zbit-rs/src/pack/`
- `zbit-rs/src/pack/bitstream.rs`
- `zbit-rs/src/pack_rules.rs`

### 7. Streaming compression with multi-level grouping

Implemented:

- `.zbps` chunk-stream container with key-piece intervals for restartable decode
- per-chunk/per-group adaptive selection with configurable multi-level grouping depth
- deterministic block boundaries so receivers can start decode from key pieces without replaying full history
- optional grouping-history hints in block headers for sharing generalized grouping strategy over time
- optional shared-grouping payload layer in non-wide realtime mode, so blocks can reference global generalized circuits/slices when local piece compression is weaker

Code:

- `zbit-rs/src/pack/`
- `zbit-rs/src/bin/benchmark_stream_real_file.rs`

### 8. Validation and benchmark as first-class workflow

Paper guidance: implementation quality requires verification + measurement loops.

Implemented:

- unit + integration tests for:
  - exact minimization
  - Espresso-style heuristic optimization
  - SAT local pruning
  - objective-aware advanced compression
  - model and pack roundtrip validation
- benchmark binary with method rationale, candidate sizes, timings, throughput, ratio, and output validation

Code:

- `zbit-rs/tests/`
- `zbit-rs/src/bin/benchmark_real_file.rs`

## Repository Layout

- `README.md`: this file
- `LICENSE`: PolyForm Noncommercial License 1.0.0
- `papers/zbit-algorithmsResearch.md`: theory and architecture recommendations
- `zbit-rs/`: Rust crate

Inside `zbit-rs/`:

- `src/lib.rs`: public API
- `src/model.rs`: exact Boolean model + `.zbit` serialization
- `src/minimizer.rs`: exact minimization engine
- `src/hierarchical.rs`: Shannon-cofactor decomposition above the 16-input exact bound
- `src/advanced.rs`: heuristic/rewrite/SAT/objective optimization flow
- `src/sat.rs`: internal SAT solver used by local exactness pruning
- `src/pack/`: adaptive `.zbpk` + streaming `.zbps` compression/decompression
- `src/pack/bitstream.rs`: `CircuitBitStream` bit-level interning primitive (ROADMAP IMMED-1)
- `src/pack_rules.rs`: method-selection rules
- `src/bin/benchmark_real_file.rs`: real-file benchmark binary
- `src/bin/benchmark_stream_real_file.rs`: real-file stream benchmark binary
- `tests/`: integration tests

## Build and Run

From repository root:

```bash
cargo test --manifest-path zbit-rs/Cargo.toml
```

Run the model validation demo:

```bash
cargo run --manifest-path zbit-rs/Cargo.toml --bin zbit-rs
```

Run the real-file benchmark (defaults already target `papers/zbit-algorithmsResearch.md`):

```bash
cargo run --release --manifest-path zbit-rs/Cargo.toml --bin zbit-benchmark -- \
  papers/zbit-algorithmsResearch.md \
  zbit-rs/benchmark_algorithmsResearch.zbpk \
  zbit-rs/benchmark_latest.txt
```

Run the cat challenge benchmark with auto-download (if missing in `assets/`):

```bash
bash zbit-rs/scripts/benchmark_cat_challenge.sh
```

Run the streaming benchmark (chunked/key-piece mode):

```bash
cargo run --manifest-path zbit-rs/Cargo.toml --bin zbit-benchmark-stream -- \
  assets/cat_challenge.png \
  zbit-rs/benchmark_cat_challenge_stream.zbps \
  zbit-rs/benchmark_cat_challenge_stream_latest.txt \
  262144 8 2 8
```

Optional trailing flags: `realtime_mode`, `wide_overfitting_circuits`, `carry_grouping_history`
as boolean values (`true`/`false` or `1`/`0`).

Compression profile control is available for both real-file and stream paths via
`ZBIT_COMPRESSION_PROFILE` (`fast`, `balanced`, `deep`, `research`), defaulting to `balanced`.

Run the cat challenge streaming benchmark script (auto-download if missing):

```bash
bash zbit-rs/scripts/benchmark_cat_challenge_stream.sh
```

Run the cat challenge multilevel streaming benchmark matrix (multiple profiles):

```bash
bash zbit-rs/scripts/benchmark_cat_challenge_stream_multilevel.sh
```

## Latest Benchmark Result Files

Current snapshot (primary refreshed on 2026-07-03 after disabling prime-sequence by default; other non-stream reports refreshed on 2026-07-02 with **release builds** — see the timing note below; stream reports kept from 2026-05-23):

### Latest Single-Run Benchmarks

| Test | Input | Selected method/profile | Original -> Compressed (bytes) | Ratio | Savings | Compression ms | Decompression ms | Validation |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Paper benchmark | `papers/zbit-algorithmsResearch.md` | `raw-brotli` | `62015 -> 18573` | `0.299492` | `70.05%` | `96.723` | `1.825` | `PASS` |
| Primary binary benchmark | `assets/primary.3b.bin` | `monotonic-delta` | `3233613 -> 562799` | `0.174046` | `82.60%` | `3271.924` | `33.379` | `PASS` |
| Cat challenge benchmark | `assets/cat_challenge.png` | `recursive-circuit-xz` | `2969404 -> 2670567` | `0.899361` | `10.06%` | `34343.559` | `506.423` | `PASS` |
| Cat challenge stream benchmark | `assets/cat_challenge.png` | `wide-overfit stream` | `2969404 -> 2670846` | `0.899455` | `10.05%` | `112964.407` | `8186.741` | `PASS` |
| Depth Anything model benchmark | `assets/depth_anything_v2_vits.pth` | `adaptive-transformed-xz` | `99218434 -> 83380762` | `0.840376` | `15.96%` | `359490.041` | `220.875` | `PASS` |

`raw-brotli` is now available for bounded text-like inputs and moves the paper benchmark from `20561` bytes (`raw-xz`, ratio `0.331549`) to `18573` bytes (ratio `0.299492`). The candidate is gated by a cheap text-likeness check so binary corpora such as `primary.3b`, cat, and depth_anything do not pay the q11 Brotli search cost. Outer recompression of `.zbpk` artifacts was also probed: it lost on paper/primary and saved only 58 bytes on the 83 MB depth artifact with zstd, so it is documented but not promoted into the live format yet.

`prime-sequence` is a ZBPK v5 structural method for fixed-width little-endian tables that are exactly consecutive primes. It stores only `(width, count, first, last)` and regenerates the table with a bounded sieve during decode. The detector only accepts a byte-for-byte consecutive-prime table, but it is disabled by default because it makes cross-file ratio comparisons misleading; set `ZBIT_ENABLE_PRIME_SEQUENCE=1` to evaluate it. In opt-in runs, `assets/primary.3b.bin` moves from the default `562799` bytes to `25` bytes with validation PASS.

Compression times remain substantially lower than the 2026-05-07 snapshot on the structural paths: primary `16 635 ms -> 3 272 ms`, cat `112 500 ms -> 60 747 ms`, and depth_anything `5 429 743 ms -> 628 278 ms`. Cat ratio improved slightly (`0.899412 -> 0.899361`, 151 bytes saved cumulatively across the v3 dictionary compaction). The structural-speed improvements come from (a) a cheap XZ-3 ranking pass before the expensive `choose_best_codec` evaluation, (b) a leaner per-plan winner-refinement tuning matrix, (c) a bounded framed-payload analyzer, (d) a tuned-XZ cache, and (e) a runtime gate that skips the full raw-xz tuning matrix when a cheap XZ-3 estimate plus adaptive-transformed-xz prove raw-xz cannot win.

**2026-07-02 runtime pass (no ratio change, all bytes identical):** the non-stream benchmark timings above are measured with release builds — the benchmark scripts previously ran the default debug profile, which roughly doubled cat wall time (release alone: cat `94.5 s -> 51.8 s` on the same machine, identical output bytes). On top of that, four search-cost fixes landed: (a) the raw-xz tuning-matrix skip gate now also considers the structural candidates (recursive-circuit-xz, monotonic-delta), tiered by a cheap preset-3 bound and, in the borderline band, a single easy XZ-9 probe — primary and cat no longer walk the matrix at all; (b) the ~27 unconditionally forced transform-plan variants are pre-ranked on the existing cheap sample and only a profile-bounded top slice (balanced: 4) pays the full-data XZ-3 ranking encode — deep/research keep the full set; (c) the winner tuned-XZ refinement drops the extreme-preset entries on fast/balanced (they doubled refinement wall time and never beat the plain preset-9 variants on a transformed payload); (d) the prelude candidates (index/huffman/deflate/zstd/brotli/cheap-xz) run concurrently. Net effect at balanced: primary `4.8 s -> 2.7 s`, cat `51.8 s -> 34.3 s` release-to-release, depth_anything `628 s -> 359 s` (same script, both release; identical output bytes `83380762`). The `raw-xz encode` timing field previously absorbed the recursive/monotonic/adaptive build time (it reported 51 s on cat where the real matrix cost was ~1.6 s); it now reports only the cheap estimate plus the probe/matrix work. `cargo test` also got faster: dependencies (xz2, zstd, brotli, preflate-rs) are now compiled optimized in dev/test builds via a `[profile.dev.package."*"]` override.

Several new tail-only reversible transforms have been added without changing tracked ratios: `periodic-head-tail-tail-row-delta` / `row-xor` / `row-up` (PNG-style Sub / XOR / Up predictors applied only to the row-data tail), `periodic-head-tail-tail-bit-plane-transpose` (with and without follow-up unary delta), plus deep-search XZ tunings (`depth=2000`/`4000`) gated to the deep / research profiles. They lose the XZ-3 ranking on the already-filtered cat PNG IDAT plain (where the simple `periodic-head-tail` still wins by ~23 KB on XZ-9) but stay available for unfiltered raster payloads where they should win cleanly.

The N3 multi-block recursive-circuit path is also landed (deep/research only): the inflated plain is optionally split into 2, 4, or 8 consecutive blocks, each block picks its own best transform plan, and the concatenated transformed bytes go through a single codec pass. The on-disk format extension uses a backward-compatible top-bit flag on the topology count so legacy single-plan dictionaries decode unchanged. For cat (uniformly-structured PNG IDAT) the multi-block path's best candidate is ~106 KB larger than the single-plan one — the rearrangement breaks XZ cross-block matches — so single-plan still wins and is selected. Multi-block is ready to win cleanly on heterogeneous inputs where per-region best plans differ substantially.

A new top-level pack method `adaptive-transformed-xz` brings the same reversible-transform search to inputs that are *not* framed deflate (e.g. PyTorch `.pth` model files, raw float-tensor dumps). It runs `choose_adaptive_transform_plan` on the raw input, encodes the best transformed payload with the full codec/tuned-XZ selection, and stores a small 18-byte dictionary `(transform_kind, period, head, codec, plain_len)` so the decoder can invert the transform deterministically. Two cost gates keep it bounded: skip when recursive-circuit-xz already covers the same search; skip when raw-xz already compresses to ≤ 0.30 of the input (already-strong corpora like `primary.3b.bin`). A 128 KiB size threshold keeps small text files out of the plan search. On the new `depth_anything_v2_vits.pth` corpus it wins by **~7 MB (~7.8 %)** vs raw-xz alone: raw-xz `90 414 940 → 83 380 790` adaptive-transformed-xz, final ratio `0.840376` on 99 218 434 input bytes.

### ZBPK v4/v5: bit-wise dictionaries plus structural method slots

The on-disk format is now **ZBPK v4**. v3 rebuilt every dictionary section under the same rule the topology bit-packing introduced: spend only the bits each enumeration value or magnitude actually needs, never a whole byte for a single bit of state. v4 keeps that layout and uses another reserved 4-bit method slot for `raw-brotli`, a no-dictionary payload method for text-heavy inputs:

- **ZBPK header.** `method` (≤ 16 values) and `bits_per_symbol` (0..=15) are packed into one byte (4 bits each). `unique_count`, `original_size`, `dict_size`, and `payload_size` are varints instead of fixed `u16`/`u64`. The header on a 62 KB text file shrinks from 36 B to **17 B**.
- **Framed-payload dictionary.** The six u32 size fields (prefix_len, suffix_len, base_chunk_len, full_chunk_count, tail_chunk_len, total_chunks) are varints; the fixed section drops from 28 B to ~12 B.
- **Recursive-circuit fixed section.** The four u64 sizes are varints. A single `u16` carries `(transformed_codec:2 | correction_codec:2 | transform_kind_index:6 | reserved:6)` instead of three separate `u8`s. `period` and `head` are varints. Together this drops the fixed section from 51 B to ~25 B.
- **Multi-block plan trailer.** `block_count`, `block_size`, and each plan's `period`/`head` are varints; the plan's kind index is one byte (6 bits used). Per-plan slot goes from 9 B to 3-5 B.
- **Adaptive-transformed-xz dictionary.** One packed byte `(codec:2 | kind_index:6)` plus varint `period`/`head`/`plain_len`. Drops from 18 B to ~9 B.
- **Monotonic-delta dictionary.** One packed byte `(width-1:3 | mode:3 | codec:2)`, then `trailing_zero_shift` (6 bits used out of its byte), then varint `count`/`first_value`/`transformed_plain_len`. Drops from 28 B to ~12 B.
- **Raw-brotli payload.** No dictionary; the payload is a Brotli q11 stream. It is evaluated only for bounded text-like inputs and selected only when it beats the other total packed sizes.
- **Prime-sequence dictionary (v5, opt-in encoder).** A no-payload structural method for exact consecutive-prime tables: one byte width plus varint `count`, `first_value`, and `last_value`. Decoding stays enabled for v5 files; encoding requires `ZBIT_ENABLE_PRIME_SEQUENCE=1`.

All five share the **same** dense `kind_to_compact_index` table the topology bit-packing introduced, so the 49 distinct `CircuitTransformKind`-derived values fit in exactly 6 bits everywhere they appear. Measured wins on the existing corpus (all `PASS`):

| Corpus | Before v3 | After v3 | Δ bytes |
|---|---:|---:|---:|
| paper | 20 580 | **20 561** | −19 B |
| primary.3b | 562 836 | **562 799** | −37 B (v5 prime-sequence can improve to 25 B when explicitly enabled) |
| cat | 2 670 624 | **2 670 571** | −53 B (cumulative −147 B vs the original v2) |

The absolute byte counts are small because the payload dominates, but the format no longer carries any bits the enumerations themselves don't need — every bit on the wire is principled.

### Bit-packed topology serialisation

Circuit-topology nodes used to be serialised in a fixed 28-byte-per-node layout: `id` u32 + `parent_id` u32 + `relation` u8 + `order` u16 + `kind` u8 + `param_a` u32 + `param_b` u32 + a per-node 8-byte FNV-style hash. With only 2 valid `relation` values, an `order` that builders never push above 3, and ~49 distinct `kind` values in use, that layout left several bits per field unused for every node written. The new **bit-packed** form, signalled by the `0x4000 RECURSIVE_TOPOLOGY_COMPACT_FLAG` bit on the on-disk topology-count `u16`, writes the entire topology as a single MSB-first bit stream where every field consumes only the bits it actually needs:

- **relation** — 1 bit
- **order** — 2 bits (0..3 covers every builder)
- **kind_index** — 6 bits (49 in-use kinds mapped to a dense 0..48 range; 0..26 = normal topology kinds, 27..49 = embedded correction-plan kinds)
- **is_root** — 1 bit
- **parent_index** — `ceil(log2(prev_node_count))` bits when not root: 0 bits for the very first child (only one possible parent), 1 bit for nodes #2..#3, 2 bits for nodes #4..#5, …
- **param_a / param_b** — nibble varints (3 value bits + 1 continuation bit per nibble; small values cost 4 bits each, PNG-stride 6401 costs 20 bits)
- **id** — implicit (= `position + 1`), 0 bits on the wire
- **hash64** — removed; overall decode correctness already validates the topology end-to-end through the inverse-transform pipeline

A trivial root node spends **18 bits** (was 224 bits of fixed slot); a child node carrying a PNG-stride 6401 period spends **34 bits** (was 224). The whole 5-node topology for the cat-challenge PNG goes from 140 bytes to 18 bytes — roughly an **8× compaction** on the topology itself. The compact flag is independent of the existing `0x8000 MULTI_BLOCK_FLAG` so both extensions compose. Legacy single-plan dictionaries (neither flag set) continue to decode with the fixed-width path and full per-node hash verification, so older `.zbpk` files keep working unchanged. Cat ratio measured win: `0.899412 -> 0.899380` (94 bytes saved across the 5-node topology). The number is small in absolute terms because the topology was tiny vs the payload, but it is principled: the wire no longer carries any bits for combinations the enumerations never use.

### Latest Cat Stream Multilevel Profiles

| Profile | Ratio | Savings | Original -> Compressed (bytes) | Compression ms | Decompression ms | Compression MiB/s | Decompression MiB/s | Validation | Resume |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `realtime-fast` | `0.999011` | `0.10%` | `2969404 -> 2966468` | `1021.660` | `6.007` | `2.772` | `471.450` | `PASS` | `PASS` |
| `realtime-balanced` | `0.998602` | `0.14%` | `2969404 -> 2965252` | `3501.634` | `6.831` | `0.809` | `414.583` | `PASS` | `PASS` |
| `realtime-deep` | `0.899455` | `10.05%` | `2969404 -> 2670846` | `193723.466` | `8251.937` | `0.015` | `0.343` | `PASS` | `PASS` |
| `wide-overfit` | `0.899455` | `10.05%` | `2969404 -> 2670846` | `302112.453` | `10152.587` | `0.009` | `0.279` | `PASS` | `PASS` |

Latest outputs for the tracked tests are written to:

- `zbit-rs/benchmark_latest.txt`: paper benchmark (`papers/zbit-algorithmsResearch.md`)
- `zbit-rs/benchmark_primary.3b_latest.txt`: primary binary benchmark (`assets/primary.3b.bin`)
- `zbit-rs/benchmark_cat_challenge_latest.txt`: cat challenge benchmark (`assets/cat_challenge.png`)
- `zbit-rs/benchmark_cat_challenge_stream_latest.txt`: cat challenge stream benchmark (`assets/cat_challenge.png`, 256 KiB chunks)
- `zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt`: cat challenge multilevel stream profile matrix
- `zbit-rs/benchmark_depth_anything_latest.txt`: PyTorch model benchmark (`assets/depth_anything_v2_vits.pth`)

## Programmatic Usage (Library)

```rust
use zbit_rs::{
    ZbitModel, StreamPackOptions, compress_adaptive_stream_to_file, compress_adaptive_to_file,
    decompress_file, decompress_stream_file,
};

// 2-input XOR truth table
let outputs = [0u8, 1, 1, 0];
let mut model = ZbitModel::new(2)?;
model.compress_from_table(&outputs, None)?;
model.validate_against_table(&outputs)?;

// Advanced flow with technology-aware objective
let report = model.compress_from_table_with_objective(
    &outputs,
    None,
    zbit_rs::MappingObjective::FpgaLut6,
)?;
assert!(report.selected.estimated_luts > 0);

// Pack/unpack bytes
let input = b"abcabcabc";
let _stats = compress_adaptive_to_file(input, "example.zbpk")?;
let output = decompress_file("example.zbpk")?;
assert_eq!(output, input);

let stream_options = StreamPackOptions::default();
let _stream_stats = compress_adaptive_stream_to_file(input, "example.zbps", &stream_options)?;
let stream_output = decompress_stream_file("example.zbps")?;
assert_eq!(stream_output, input);
# Ok::<(), zbit_rs::ZbitError>(())
```

## File Formats (Current)

### `.zbit` model

- magic: `ZBIT` (`0x5A42_4954`)
- version: `1`
- stores canonical node DAG and root id

### `.zbpk` pack

- magic: `ZBPK` (`0x5A42_504B`)
- version: `4`
- variable header + dictionary + payload
- adaptive methods include `raw-copy`, `indexed-raw`, `indexed-circuit`, `indexed-huffman`, `raw-deflate`, `raw-zstd`, `raw-xz`, `raw-brotli`, `framed-raw`, `recursive-circuit-xz`, `monotonic-delta`, and `adaptive-transformed-xz`
- method selection is cost-based: circuit/structural candidates are accepted only when they beat safer raw or codec-backed candidates

### `.zbps` stream pack

- magic: `ZBPS` (`0x5A42_5053`)
- version: `1`
- fixed stream header + independent key-piece blocks
- each block stores a multi-level piece/group topology and embedded `.zbpk` payloads
- key-piece interval enables restartable decode from block boundaries

## References

- Main theory and recommendations: `papers/zbit-algorithmsResearch.md`
- Crate internals and API: `zbit-rs/src/`

## License

PolyForm Noncommercial License 1.0.0. See `LICENSE`.
