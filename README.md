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

Code:

- `zbit-rs/src/minimizer.rs`
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
- rule-based gating for circuit-dictionary evaluation
- size-based final method choice, never worse than raw baseline by design
- strict `.zbpk` parser validation

Code:

- `zbit-rs/src/pack/`
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
- `src/advanced.rs`: heuristic/rewrite/SAT/objective optimization flow
- `src/sat.rs`: internal SAT solver used by local exactness pruning
- `src/pack/`: adaptive `.zbpk` + streaming `.zbps` compression/decompression
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
cargo run --manifest-path zbit-rs/Cargo.toml --bin zbit-benchmark -- \
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

Current snapshot (reports generated on 2026-05-23):

### Latest Single-Run Benchmarks

| Test | Input | Selected method/profile | Original -> Compressed (bytes) | Ratio | Savings | Compression ms | Decompression ms | Validation |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Paper benchmark | `papers/zbit-algorithmsResearch.md` | `raw-xz` | `62015 -> 20580` | `0.331855` | `66.81%` | `63.937` | `0.849` | `PASS` |
| Primary binary benchmark | `assets/primary.3b.bin` | `monotonic-delta` | `3233613 -> 562836` | `0.174058` | `82.59%` | `3695.210` | `18.812` | `PASS` |
| Cat challenge benchmark | `assets/cat_challenge.png` | `recursive-circuit-xz` | `2969404 -> 2670632` | `0.899383` | `10.07%` | `49632.414` | `483.917` | `PASS` |
| Cat challenge stream benchmark | `assets/cat_challenge.png` | `wide-overfit stream` | `2969404 -> 2670846` | `0.899455` | `10.05%` | `112964.407` | `8186.741` | `PASS` |
| Depth Anything model benchmark | `assets/depth_anything_v2_vits.pth` | `adaptive-transformed-xz` | `99218434 -> 83380790` | `0.840376` | `15.96%` | `1405690.364` | `236.317` | `PASS` |

Compression times are substantially lower than the 2026-05-07 snapshot: paper `313 ms -> 64 ms` (~4.9x faster), primary `16 635 ms -> 3 695 ms` (~4.5x faster), cat `112 500 ms -> 49 632 ms` (~2.3x faster), and the new depth_anything model corpus `5 429 743 ms -> 1 405 690 ms` (~3.9x faster — almost all of it from bounding the false-positive CRC32 frame scan). Cat ratio even **improved** slightly (`0.899412 -> 0.899383`, 86 bytes saved) thanks to the new compact topology serialisation (described below); all other corpus ratios held byte-identical. The improvement comes from (a) a cheap XZ-3 ranking pass that picks the top-K transform plans before the expensive `choose_best_codec` evaluation, (b) a leaner per-plan winner-refinement tuning matrix, (c) a bounded framed-payload analyzer that no longer burns minutes hashing megabytes per offset on inputs without a valid CRC32 run, and (d) a tuned-XZ cache that avoids re-running the full XZ matrix on identical byte streams.

Several new tail-only reversible transforms have been added without changing tracked ratios: `periodic-head-tail-tail-row-delta` / `row-xor` / `row-up` (PNG-style Sub / XOR / Up predictors applied only to the row-data tail), `periodic-head-tail-tail-bit-plane-transpose` (with and without follow-up unary delta), plus deep-search XZ tunings (`depth=2000`/`4000`) gated to the deep / research profiles. They lose the XZ-3 ranking on the already-filtered cat PNG IDAT plain (where the simple `periodic-head-tail` still wins by ~23 KB on XZ-9) but stay available for unfiltered raster payloads where they should win cleanly.

The N3 multi-block recursive-circuit path is also landed (deep/research only): the inflated plain is optionally split into 2, 4, or 8 consecutive blocks, each block picks its own best transform plan, and the concatenated transformed bytes go through a single codec pass. The on-disk format extension uses a backward-compatible top-bit flag on the topology count so legacy single-plan dictionaries decode unchanged. For cat (uniformly-structured PNG IDAT) the multi-block path's best candidate is ~106 KB larger than the single-plan one — the rearrangement breaks XZ cross-block matches — so single-plan still wins and is selected. Multi-block is ready to win cleanly on heterogeneous inputs where per-region best plans differ substantially.

A new top-level pack method `adaptive-transformed-xz` brings the same reversible-transform search to inputs that are *not* framed deflate (e.g. PyTorch `.pth` model files, raw float-tensor dumps). It runs `choose_adaptive_transform_plan` on the raw input, encodes the best transformed payload with the full codec/tuned-XZ selection, and stores a small 18-byte dictionary `(transform_kind, period, head, codec, plain_len)` so the decoder can invert the transform deterministically. Two cost gates keep it bounded: skip when recursive-circuit-xz already covers the same search; skip when raw-xz already compresses to ≤ 0.30 of the input (already-strong corpora like `primary.3b.bin`). A 128 KiB size threshold keeps small text files out of the plan search. On the new `depth_anything_v2_vits.pth` corpus it wins by **~7 MB (~7.8 %)** vs raw-xz alone: raw-xz `90 414 940 → 83 380 790` adaptive-transformed-xz, final ratio `0.840376` on 99 218 434 input bytes.

### Compact (bit-packed) topology serialisation

Circuit-topology nodes used to be serialised in a fixed 28-byte-per-node layout: `id` u32 + `parent_id` u32 + `relation` u8 + `order` u16 + `kind` u8 + `param_a` u32 + `param_b` u32 + a per-node 8-byte FNV-style hash. With ~5 distinct relation values, 2-bit-wide `order`, and small node ids and parameters in practice, that layout left several bits per field unused for every node written. The new **compact** form, signalled by the `0x4000 COMPACT_TOPOLOGY_FLAG` bit on the on-disk topology-count `u16`, writes each node as:

- 1 flag byte: `(relation:1 | order:7)`
- 1 raw `kind` byte
- varint `id`
- varint `parent_id + 1` (0 = root sentinel; avoids a 5-byte varint per root)
- varint `param_a`
- varint `param_b`
- **no per-node hash** — overall decode correctness already validates the topology end-to-end through the inverse-transform pipeline

A trivial root node serialises in 6 bytes (was 28); a node carrying a PNG-stride 6401 period serialises in 7 bytes (was 28). The compact flag is independent of the existing `0x8000 MULTI_BLOCK_FLAG` so both extensions compose. Legacy single-plan dictionaries (neither flag set) continue to decode with the fixed-width path and full per-node hash verification, so older `.zbpk` files keep working unchanged. The measurable ratio win on cat (`0.899412 -> 0.899383`, 86 bytes saved across a 4-node topology) is small in absolute terms but principled: the format no longer wastes bits per node on unused enumeration combinations.

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
- version: `2`
- 36-byte fixed header + dictionary + payload
- adaptive methods include `raw-copy`, `indexed-raw`, `indexed-circuit`, `indexed-huffman`, `raw-deflate`, `raw-zstd`, `raw-xz`, `framed-raw`, `recursive-circuit-xz`, and `monotonic-delta`
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
