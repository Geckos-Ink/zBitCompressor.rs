# zBit Circuit-Based Compression Roadmap

_Last updated: 2026-05-23_

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

### N2. Typed substreams for preflate corrections (medium impact)

The corrections payload currently encodes as `raw` at 284 093 bytes — neither XZ nor zstd
can compress it. That is because the underlying preflate corrections are a packed stream
of (record-tag, payload) tuples interleaved together. Splitting them into typed substreams
(record-tag stream, literal-correction stream, length-correction stream, distance-correction
stream, tree-correction stream, fallback bytes) and coding each with the best codec per
stream should expose redundancy.

Implementation outline:

1. Parse the corrections blob with `preflate-rs` helpers (or reproduce the splitter logic).
2. For each substream, run the existing `choose_best_codec_cached`.
3. Serialize substream count + per-stream `(kind, plain_len, encoded_len, codec)` into the
   recursive dictionary, then concatenate encoded payloads.
4. Decoder reverses the split and feeds the recovered correction blob to preflate-rs.

Format impact: new dictionary section (length-prefixed list of typed substreams). Bump
`.zbpk` minor version if the format is otherwise rigid; otherwise gate on a new flag.

Risk: requires understanding preflate-rs internal correction record layout. If preflate-rs
exposes only an opaque blob, fall back to a structural splitter (scan for record-type bytes
and split on them).

Expected impact on cat: 20–80 KB depending on substream entropy. Generalizes to any
recursive-circuit-xz input with non-trivial corrections.

### N3. Per-block (block-local) transform plans inside a single pack (medium impact)

Today `recursive-circuit-xz` selects one transform plan for the whole inflated payload.
Image data often has scanline batches with different best transforms (smooth regions vs
high-frequency regions). Splitting the tail into N equal blocks, picking the best transform
per block, and storing per-block plan IDs would let each block use the locally best plan.

Implementation outline:

1. Split tail into N blocks (start with N = 4 or 8).
2. Run `choose_adaptive_transform_plan` on each block independently (with the cheap XZ-3
   ranking already in place).
3. Concatenate transformed blocks; encode as one XZ stream (so dictionary matches still
   cross block boundaries).
4. Dictionary stores `block_count`, then per-block `(transform_kind, period, head)`.
5. Decoder iterates blocks, inverting per-block transforms.

Format impact: new dictionary section. Bump `.zbpk` minor version.

Risk: per-block transforms can hurt XZ matches across blocks; the per-block transform must
either be very local (no shift) or all blocks must agree. Start with same-period transforms
across blocks (only vary kind), then loosen.

Expected impact on cat: 30–80 KB; larger on heterogeneous images. Cost: ~Nx the transform
eval time, but the cheap XZ-3 ranking keeps total under control.

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

| Corpus | Current | N1 measured | N2/N3 target | Pre-atlas stretch |
| --- | ---: | ---: | ---: | ---: |
| `papers/zbit-algorithmsResearch.md` | `0.331855` | `0.331855` (no change) | no regression | `< 0.325` (BWT preproc) |
| `assets/primary.3b.bin` | `0.174058` | `0.174058` (no change) | no regression | `< 0.170` (gap-stream secondary transform) |
| cat normal | `0.899412` | `0.899412` (no change – PNG already filters) | `< 0.880` | `< 0.870` |
| cat stream wide-overfit | `0.899455` | `0.899455` (no change) | `< 0.882` | `< 0.872` |

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
