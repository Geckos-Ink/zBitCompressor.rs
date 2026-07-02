# zbit-rs

Rust crate for the experimental **bits-to-Karnaugh-map circuits compression** model used by `zBit`.

The crate is not meant to be just another byte-stream codec. It contains the implementation pieces that let zBit reinterpret data as Boolean maps, minimize bounded truth tables into circuits, search structural/recursive representations for real files, and then select the smallest reversible encoding candidate.

Rust migration of the `zBit` compression model with:
- exact small-support minimization (Quine-McCluskey + exact cover)
- Espresso-style iterative cover heuristics
- AIG-style rewriting/resubstitution + balancing-aware scoring
- SAT-assisted local redundancy pruning
- technology-aware objective mapping (ASIC area/delay and FPGA LUT4/LUT6 proxies)
- canonical DAG interning for gates
- model serialization/deserialization (`.zbit`)
- adaptive pack strategy (`raw-copy`, `indexed-raw`, `indexed-circuit`, `indexed-huffman`, `raw-deflate`, `raw-zstd`, `raw-xz`, `raw-brotli`, `framed-raw`, `recursive-circuit-xz`, `monotonic-delta`, `adaptive-transformed-xz`)
- real-file benchmark/report generation

## Build and Test

```bash
cargo test
```

## Demo Validation Binary

```bash
cargo run --bin zbit-rs
```

## Advanced API (Library)

`ZbitModel` now exposes:
- `compress_from_table_advanced(outputs, dont_cares, &AdvancedOptions)`
- `compress_from_table_with_objective(outputs, dont_cares, MappingObjective)`

## Real File Benchmark

```bash
cargo run --release --bin zbit-benchmark -- \
  ../papers/zbit-algorithmsResearch.md \
  benchmark_algorithmsResearch.zbpk \
  benchmark_latest.txt
```

The benchmark writes `benchmark_latest.txt` with:
- selected method and rule rationale
- candidate size comparison
- compression/decompression timings and throughput
- compression ratio and output validation result

## Cat Challenge Benchmark

Run the reproducible cat challenge flow (auto-downloads asset if missing):

```bash
bash scripts/benchmark_cat_challenge.sh
```

This updates:
- `benchmark_cat_challenge_latest.txt` (tracked benchmark reference)
