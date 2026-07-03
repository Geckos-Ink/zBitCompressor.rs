// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::Instant;

use crate::error::{ZbitError, ZbitResult};
use crate::model::ZbitModel;
use crate::pack_rules::{choose_best_method, should_evaluate_circuit, PackEvaluation, PackMethod};
use blake3::Hasher as Blake3Hasher;
use crc32fast::Hasher as Crc32Hasher;
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use preflate_rs::{preflate_whole_deflate_stream, recreate_whole_deflate_stream, PreflateConfig};
use rayon::prelude::*;
use xxhash_rust::xxh3::xxh3_64;
use xz2::read::XzDecoder;
use xz2::stream::{Check, Filters, LzmaOptions, MatchFinder, Mode, Stream};
use xz2::write::XzEncoder as XzWriterEncoder;
use zstd::stream as zstd_stream;

pub const ZBPK_MAGIC: u32 = 0x5A42_504B; // "ZBPK"
pub const ZBPK_VERSION: u16 = 5;
// Upper bound used for candidate-size estimation. The actual header is variable-length:
//   magic u32 (4) + version u16 (2) + flags u16 (2) + packed method-and-bits u8 (1)
//   + varint unique_count + varint original_size + varint dict_size + varint payload_size.
// A varint of a u64 is at most 10 bytes; sizes up to 2^28 (256 MiB) fit in 4 bytes, sizes
// up to 2^35 (32 GiB) fit in 5. We size the estimate to comfortably cover anything that
// will actually be packed and use it only for sorting candidates by total bytes — the
// final write emits the exact varint sequence regardless.
pub const ZBPK_HEADER_BYTES: usize = 9 + 4 * 5;
pub const ZBPS_MAGIC: u32 = 0x5A42_5053; // "ZBPS"
pub const ZBPS_VERSION: u16 = 1;
const ZBPS_HEADER_BYTES: usize = 40;
const ZBPS_BLOCK_HEADER_BYTES: usize = 21;
const ZBPS_NODE_KIND_PIECE: u8 = 0;
const ZBPS_NODE_KIND_GROUP: u8 = 1;
const ZBPS_NODE_KIND_SPLIT: u8 = 2;
const ZBPS_NODE_KIND_GLOBAL_SLICE: u8 = 3;
const ZBPS_HISTORY_NONE: u8 = u8::MAX;
const ZBPS_FLAG_CARRY_GROUPING_HISTORY: u16 = 0x0001;
const ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS: u16 = 0x0002;
const ZBPS_FLAG_SHARED_GROUPING_PAYLOAD: u16 = 0x0004;
const MAX_HUFFMAN_CODE_BITS: u8 = 56;
const ZBPK_MAX_OUTPUT_BYTES: usize = 1usize << 30; // 1 GiB hard safety bound

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum CompressionProfile {
    Fast,
    Balanced,
    Deep,
    Research,
}

impl CompressionProfile {
    fn from_env() -> Self {
        let value = std::env::var("ZBIT_COMPRESSION_PROFILE")
            .or_else(|_| std::env::var("ZBIT_PROFILE"))
            .unwrap_or_else(|_| "balanced".to_string());
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" => Self::Fast,
            "deep" => Self::Deep,
            "research" => Self::Research,
            _ => Self::Balanced,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Deep => "deep",
            Self::Research => "research",
        }
    }

    fn max_transform_plans(self) -> usize {
        match self {
            Self::Fast => 4,
            Self::Balanced => 8,
            Self::Deep => 12,
            Self::Research => 16,
        }
    }

    fn full_eval_transform_plans(self) -> usize {
        // Number of plans that receive the expensive choose_best_codec evaluation.
        // The rest are scored only with a quick XZ-3 pass and dropped.
        // XZ-3 ranks XZ-9 winners reliably for our payloads (same LZMA family); we still
        // keep a head margin to recover from the occasional rank swap.
        match self {
            Self::Fast => 1,
            Self::Balanced => 2,
            Self::Deep => 4,
            Self::Research => 8,
        }
    }

    fn multi_block_split_counts(self) -> &'static [u32] {
        // Block counts to probe for the N3 multi-block recursive-circuit path. Each count
        // is an N where the inflated plain is split into N consecutive blocks, each block
        // picks its own best transform plan, and the concatenated transformed bytes go
        // through a single codec encode. Off entirely for fast/balanced because plan
        // search per block is expensive; deep/research probe a small set and the winner
        // is selected by total compressed size.
        match self {
            Self::Fast | Self::Balanced => &[],
            Self::Deep => &[2, 4],
            Self::Research => &[2, 4, 8],
        }
    }

    fn correction_transform_plan_budget(self) -> usize {
        match self {
            Self::Fast => 4,
            Self::Balanced => 8,
            Self::Deep => 12,
            Self::Research => 16,
        }
    }

    fn monotonic_gap_transform_plan_budget(self) -> usize {
        match self {
            Self::Fast => 0,
            Self::Balanced => 0,
            Self::Deep => 8,
            Self::Research => 12,
        }
    }

    fn enable_xz_extreme_for_raw_xz(self) -> bool {
        !matches!(self, Self::Fast)
    }

    fn enable_xz_extreme_refinement(self) -> bool {
        !matches!(self, Self::Fast)
    }

    fn enable_xz_extreme_winner_refine(self) -> bool {
        // The winner tuned-XZ refinement after a transform-plan search re-encodes one
        // already-chosen payload with the tuning matrix. The extreme-preset entries there
        // double the wall time of the whole refinement (they are the slowest jobs) and
        // have not been observed to beat the plain preset-9 variants on a transformed
        // payload (cat: refine wins nothing; the Phase-B XZ-9 pass already holds the
        // winner). Keep extreme in the refinement only where the user opted into
        // exhaustive search.
        matches!(self, Self::Deep | Self::Research)
    }

    fn should_attempt_recursive_on_realtime_blocks(self) -> bool {
        !matches!(self, Self::Fast)
    }

    fn realtime_deflate_compression(self) -> Compression {
        match self {
            Self::Fast => Compression::fast(),
            Self::Balanced => Compression::new(6),
            Self::Deep | Self::Research => Compression::best(),
        }
    }

    fn realtime_zstd_level(self) -> i32 {
        match self {
            Self::Fast => 1,
            Self::Balanced => 6,
            Self::Deep => 12,
            Self::Research => 19,
        }
    }

    fn stream_split_points(self, start: usize, end: usize) -> Vec<usize> {
        let chunk_span = end.saturating_sub(start);
        if chunk_span <= 1 {
            return Vec::new();
        }

        match self {
            Self::Fast => vec![start + (chunk_span / 2)],
            Self::Balanced => {
                if chunk_span <= 4 {
                    ((start + 1)..end).collect()
                } else {
                    let mut mids = vec![
                        start + (chunk_span / 2),
                        start + (chunk_span / 3),
                        start + ((chunk_span * 2) / 3),
                    ];
                    mids.retain(|mid| *mid > start && *mid < end);
                    mids.sort_unstable();
                    mids.dedup();
                    mids
                }
            }
            Self::Deep | Self::Research => ((start + 1)..end).collect(),
        }
    }

    fn should_attempt_stream_shared_grouping_payload(self) -> bool {
        !matches!(self, Self::Fast)
    }

    fn should_attempt_recursive_on_stream_shared_payload(self) -> bool {
        matches!(self, Self::Deep | Self::Research)
    }

    fn max_parallel_codec_threads(self) -> usize {
        // IMMED-5 work-stealing budget: bound the rayon pool used by codec/transform searches.
        // 0 means "use the global default" (all available cores). balanced is kept slightly
        // below max so I/O-saturated runs don't thrash; deep/research take everything.
        match self {
            Self::Fast => 2,
            Self::Balanced => 0,
            Self::Deep | Self::Research => 0,
        }
    }
}

fn should_evaluate_prime_sequence() -> bool {
    std::env::var("ZBIT_ENABLE_PRIME_SEQUENCE")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CompressionBudgets {
    /// 0 = rayon global default.
    pub max_parallel_codec_threads: usize,
}

impl CompressionBudgets {
    fn from_profile(profile: CompressionProfile) -> Self {
        Self {
            max_parallel_codec_threads: profile.max_parallel_codec_threads(),
        }
    }
}

fn should_evaluate_raw_brotli(input: &[u8]) -> bool {
    if input.is_empty() || input.len() > 8 * 1024 * 1024 {
        return false;
    }
    if std::str::from_utf8(input).is_ok() {
        return true;
    }

    let ascii_text_bytes = input
        .iter()
        .filter(|&&byte| matches!(byte, b'\n' | b'\r' | b'\t' | 0x20..=0x7e))
        .count();
    let control_bytes = input
        .iter()
        .filter(|&&byte| byte < 0x20 && !matches!(byte, b'\n' | b'\r' | b'\t'))
        .count();

    ascii_text_bytes.saturating_mul(100) >= input.len().saturating_mul(90)
        && control_bytes.saturating_mul(100) <= input.len()
}

#[derive(Debug, Clone, Default)]
pub struct CandidateTimingStats {
    pub index_stream_ms: f64,
    pub huffman_stream_ms: f64,
    pub raw_deflate_ms: f64,
    pub raw_zstd_ms: f64,
    pub raw_brotli_ms: f64,
    pub raw_xz_ms: f64,
    pub framed_extraction_ms: f64,
    pub recursive_preflate_ms: f64,
    pub recursive_transform_sampling_ms: f64,
    pub recursive_transform_eval_ms: f64,
    pub recursive_correction_modeling_ms: f64,
    pub recursive_total_ms: f64,
    pub candidate_validation_ms: f64,
    pub stream_block_planning_ms: f64,
    pub stream_global_payload_ms: f64,
    /// IMMED-5: wall-clock time spent inside the encoder (excludes file I/O).
    pub total_compression_ms: f64,
    /// IMMED-5: derived MiB/s, original_size / total_compression_ms.
    pub compression_throughput_mib_s: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub codec_hits: usize,
    pub codec_misses: usize,
    pub preflate_hits: usize,
    pub preflate_misses: usize,
    pub stream_range_hits: usize,
    pub stream_range_misses: usize,
    pub tuned_xz_hits: usize,
    pub tuned_xz_misses: usize,
}

#[derive(Debug, Clone)]
pub struct PackStats {
    pub original_size: usize,
    pub compressed_size: usize,

    pub unique_symbols: usize,
    pub bits_per_symbol: u8,
    pub payload_bytes: usize,

    pub raw_dictionary_bytes: usize,
    pub circuit_dictionary_bytes: usize,
    pub huffman_dictionary_bytes: usize,

    pub raw_candidate_bytes: usize,
    pub indexed_raw_candidate_bytes: usize,
    pub indexed_circuit_candidate_bytes: Option<usize>,
    pub indexed_huffman_candidate_bytes: Option<usize>,
    pub raw_deflate_candidate_bytes: Option<usize>,
    pub raw_zstd_candidate_bytes: Option<usize>,
    pub raw_brotli_candidate_bytes: Option<usize>,
    pub raw_xz_candidate_bytes: Option<usize>,
    pub framed_raw_candidate_bytes: Option<usize>,
    pub recursive_circuit_xz_candidate_bytes: Option<usize>,
    pub monotonic_delta_candidate_bytes: Option<usize>,
    pub prime_sequence_candidate_bytes: Option<usize>,
    pub adaptive_transformed_xz_candidate_bytes: Option<usize>,

    pub chosen_method: PackMethod,
    pub chosen_reason: String,
    pub circuit_rule_note: String,
    pub active_profile: String,
    pub skipped_candidates: Vec<String>,
    pub timings: CandidateTimingStats,
    pub cache_stats: CacheStats,
}

#[derive(Debug, Clone)]
pub struct StreamPackOptions {
    pub chunk_size: usize,
    pub key_piece_interval: usize,
    pub max_group_depth: u8,
    pub max_group_pieces: usize,
    pub carry_grouping_history: bool,
    pub realtime_mode: bool,
    pub wide_overfitting_circuits: bool,
}

impl Default for StreamPackOptions {
    fn default() -> Self {
        Self {
            chunk_size: 256 * 1024,
            key_piece_interval: 8,
            max_group_depth: 2,
            max_group_pieces: 8,
            carry_grouping_history: true,
            realtime_mode: true,
            wide_overfitting_circuits: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamPackStats {
    pub original_size: usize,
    pub compressed_size: usize,
    pub chunk_size: usize,
    pub total_chunks: usize,
    pub key_piece_interval: usize,
    pub key_piece_count: usize,
    pub block_count: usize,
    pub max_group_depth: u8,
    pub max_group_pieces: usize,
    pub piece_node_count: usize,
    pub grouped_node_count: usize,
    pub split_node_count: usize,
    pub max_depth_used: u8,
    pub grouping_hint_updates: usize,
    pub key_piece_decode_note: String,
    pub effective_wide_overfitting_circuits: bool,
    pub adaptive_wide_promotion_used: bool,
    pub shared_grouping_payload_used: bool,
    pub active_profile: String,
    pub skipped_candidates: Vec<String>,
    pub timings: CandidateTimingStats,
    pub cache_stats: CacheStats,
}

#[derive(Debug, Clone)]
struct IndexStream {
    unique_symbols: Vec<u8>,
    bits_per_symbol: u8,
    payload: Vec<u8>,
    frequencies: [u32; 256],
}

#[derive(Debug, Clone)]
struct HuffmanStream {
    symbols: Vec<u8>,
    code_lengths: Vec<u8>,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
struct HuffmanNode {
    symbol: Option<u8>,
    left: Option<usize>,
    right: Option<usize>,
}

#[derive(Debug, Clone)]
struct FramedPayloadRun {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    frame_tag: [u8; 4],
    payload: Vec<u8>,
    base_chunk_len: u32,
    full_chunk_count: u32,
    tail_chunk_len: u32,
    total_chunks: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum CircuitTransformKind {
    Identity,
    DeltaPrev,
    XorPrev,
    BitPlaneTranspose,
    BitPlaneTransposeDelta,
    BitPlaneTransposeXor,
    PeriodicHeadTail,
    PeriodicGather,
    PeriodicDelta,
    PeriodicXor,
    PeriodicGatherDelta,
    PeriodicGatherXor,
    PeriodicHeadTailTailGather,
    PeriodicHeadTailTailGatherDelta,
    PeriodicHeadTailTailDelta,
    PeriodicHeadTailTailXor,
    PeriodicHeadTailDelta,
    PeriodicHeadTailXor,
    // Row-aware predictors operate on the tail of `periodic-head-tail(period, head=1)`,
    // treating each row of `period-1` bytes independently. They never cross row boundaries
    // and never touch the gathered head/filter-byte run.
    PeriodicHeadTailTailRowDelta, // PNG-style Sub filter inside each row, distance = `head`
    PeriodicHeadTailTailRowXor,   // same, XOR variant for bit-flipped row neighbours
    PeriodicHeadTailTailRowUp,    // PNG-style Up filter: subtract the same byte in previous row
    // Bit-plane operations on the tail of `periodic-head-tail(period, head=1)`. The head
    // (filter-byte run) is left as bytes; only the tail is transposed bit-plane-wise. For
    // filtered scanline residuals the high bit planes are mostly homogeneous (sign of small
    // residual), so the transposed tail compresses much better with XZ.
    PeriodicHeadTailTailBitPlaneTranspose,
    PeriodicHeadTailTailBitPlaneTransposeDelta,
}

impl CircuitTransformKind {
    fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::DeltaPrev => "delta-prev",
            Self::XorPrev => "xor-prev",
            Self::BitPlaneTranspose => "bit-plane-transpose",
            Self::BitPlaneTransposeDelta => "bit-plane-transpose-delta",
            Self::BitPlaneTransposeXor => "bit-plane-transpose-xor",
            Self::PeriodicHeadTail => "periodic-head-tail",
            Self::PeriodicGather => "periodic-gather",
            Self::PeriodicDelta => "periodic-delta",
            Self::PeriodicXor => "periodic-xor",
            Self::PeriodicGatherDelta => "periodic-gather-delta",
            Self::PeriodicGatherXor => "periodic-gather-xor",
            Self::PeriodicHeadTailTailGather => "periodic-head-tail-tail-gather",
            Self::PeriodicHeadTailTailGatherDelta => "periodic-head-tail-tail-gather-delta",
            Self::PeriodicHeadTailTailDelta => "periodic-head-tail-tail-delta",
            Self::PeriodicHeadTailTailXor => "periodic-head-tail-tail-xor",
            Self::PeriodicHeadTailDelta => "periodic-head-tail-delta",
            Self::PeriodicHeadTailXor => "periodic-head-tail-xor",
            Self::PeriodicHeadTailTailRowDelta => "periodic-head-tail-tail-row-delta",
            Self::PeriodicHeadTailTailRowXor => "periodic-head-tail-tail-row-xor",
            Self::PeriodicHeadTailTailRowUp => "periodic-head-tail-tail-row-up",
            Self::PeriodicHeadTailTailBitPlaneTranspose => {
                "periodic-head-tail-tail-bit-plane-transpose"
            }
            Self::PeriodicHeadTailTailBitPlaneTransposeDelta => {
                "periodic-head-tail-tail-bit-plane-transpose-delta"
            }
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Identity => 0,
            Self::DeltaPrev => 1,
            Self::XorPrev => 2,
            Self::BitPlaneTranspose => 3,
            Self::BitPlaneTransposeDelta => 16,
            Self::BitPlaneTransposeXor => 17,
            Self::PeriodicHeadTail => 4,
            Self::PeriodicGather => 5,
            Self::PeriodicDelta => 6,
            Self::PeriodicXor => 7,
            Self::PeriodicGatherDelta => 8,
            Self::PeriodicGatherXor => 9,
            Self::PeriodicHeadTailTailGather => 10,
            Self::PeriodicHeadTailTailGatherDelta => 11,
            Self::PeriodicHeadTailTailDelta => 12,
            Self::PeriodicHeadTailTailXor => 13,
            Self::PeriodicHeadTailDelta => 14,
            Self::PeriodicHeadTailXor => 15,
            Self::PeriodicHeadTailTailRowDelta => 18,
            Self::PeriodicHeadTailTailRowXor => 19,
            Self::PeriodicHeadTailTailRowUp => 20,
            Self::PeriodicHeadTailTailBitPlaneTranspose => 21,
            Self::PeriodicHeadTailTailBitPlaneTransposeDelta => 22,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Identity),
            1 => Some(Self::DeltaPrev),
            2 => Some(Self::XorPrev),
            3 => Some(Self::BitPlaneTranspose),
            16 => Some(Self::BitPlaneTransposeDelta),
            17 => Some(Self::BitPlaneTransposeXor),
            4 => Some(Self::PeriodicHeadTail),
            5 => Some(Self::PeriodicGather),
            6 => Some(Self::PeriodicDelta),
            7 => Some(Self::PeriodicXor),
            8 => Some(Self::PeriodicGatherDelta),
            9 => Some(Self::PeriodicGatherXor),
            18 => Some(Self::PeriodicHeadTailTailRowDelta),
            19 => Some(Self::PeriodicHeadTailTailRowXor),
            20 => Some(Self::PeriodicHeadTailTailRowUp),
            21 => Some(Self::PeriodicHeadTailTailBitPlaneTranspose),
            22 => Some(Self::PeriodicHeadTailTailBitPlaneTransposeDelta),
            10 => Some(Self::PeriodicHeadTailTailGather),
            11 => Some(Self::PeriodicHeadTailTailGatherDelta),
            12 => Some(Self::PeriodicHeadTailTailDelta),
            13 => Some(Self::PeriodicHeadTailTailXor),
            14 => Some(Self::PeriodicHeadTailDelta),
            15 => Some(Self::PeriodicHeadTailXor),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct CircuitTransformPlan {
    kind: CircuitTransformKind,
    period: u32,
    head: u32,
}

#[derive(Debug, Clone)]
struct CircuitTopologyNode {
    id: u32,
    parent_id: u32,
    relation: u8, // 0 = series, 1 = parallel
    order: u16,
    kind: u8,
    param_a: u32,
    param_b: u32,
    hash64: u64,
}

#[derive(Debug, Clone, Copy)]
enum PayloadCodec {
    Raw,
    Xz,
    Zstd,
    XzExtreme,
}

impl PayloadCodec {
    fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Xz => "xz",
            Self::Zstd => "zstd",
            Self::XzExtreme => "xz-extreme",
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Raw => 0,
            Self::Xz => 1,
            Self::Zstd => 2,
            Self::XzExtreme => 3,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Raw),
            1 => Some(Self::Xz),
            2 => Some(Self::Zstd),
            3 => Some(Self::XzExtreme),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct RecursiveCircuitStream {
    base: FramedPayloadRun,
    transformed_payload: Vec<u8>,
    corrections_payload: Vec<u8>,
    plain_len: usize,
    transformed_encoded_len: usize,
    correction_plain_len: usize,
    correction_encoded_len: usize,
    transformed_codec: PayloadCodec,
    correction_codec: PayloadCodec,
    zlib_header: [u8; 2],
    zlib_adler32: [u8; 4],
    transform_plan: CircuitTransformPlan,
    topology: Vec<CircuitTopologyNode>,
    // Multi-block extension (N3 in ROADMAP). `None` keeps the legacy single-plan layout.
    // When `Some`, the inflated plain was split into `plans.len()` consecutive blocks; each
    // block of size `block_size` (last block may be shorter) was transformed with its own
    // plan and the transformed payloads were concatenated before codec encoding.
    multi_block: Option<MultiBlockPlan>,
}

#[derive(Debug, Clone)]
struct MultiBlockPlan {
    // Size of every non-last block. The last block may be shorter when plain_len is not an
    // exact multiple of block_size; its size is `plain_len - block_size * (plans.len() - 1)`.
    block_size: u32,
    plans: Vec<CircuitTransformPlan>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum MonotonicDeltaMode {
    GapVarint,
    GapDeltaVarint,
    GapBytes,
    GapTrailingZeroVarint,
    GapTrailingZeroBytes,
}

impl MonotonicDeltaMode {
    fn as_u8(self) -> u8 {
        match self {
            Self::GapVarint => 0,
            Self::GapDeltaVarint => 1,
            Self::GapBytes => 2,
            Self::GapTrailingZeroVarint => 3,
            Self::GapTrailingZeroBytes => 4,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::GapVarint),
            1 => Some(Self::GapDeltaVarint),
            2 => Some(Self::GapBytes),
            3 => Some(Self::GapTrailingZeroVarint),
            4 => Some(Self::GapTrailingZeroBytes),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct MonotonicDeltaStream {
    width: u8,
    count: u64,
    first_value: u64,
    transformed_plain_len: usize,
    mode: MonotonicDeltaMode,
    trailing_zero_shift: u8,
    transform_plan: Option<CircuitTransformPlan>,
    codec: PayloadCodec,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PrimeSequenceStream {
    width: u8,
    count: u64,
    first_value: u64,
    last_value: u64,
}

#[derive(Debug, Clone)]
struct AdaptiveTransformedXzStream {
    transform_plan: CircuitTransformPlan,
    codec: PayloadCodec,
    plain_len: u64,
    payload: Vec<u8>,
}

#[derive(Debug, Default)]
struct DecodeNode {
    symbol: Option<u8>,
    left: Option<Box<DecodeNode>>,
    right: Option<Box<DecodeNode>>,
}

#[derive(Debug, Clone, Copy)]
struct StreamHeader {
    flags: u16,
    chunk_size: usize,
    key_piece_interval: usize,
    original_size: usize,
    total_chunks: usize,
    block_count: usize,
}

#[derive(Debug, Clone)]
struct PackedRangeCandidate {
    pack_bytes: Vec<u8>,
    method: PackMethod,
    original_size: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct PayloadHash {
    fast64: u64,
    strong128: [u8; 16],
    len: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct CodecProfileKey {
    allow_raw: bool,
    allow_xz_extreme: bool,
    profile: CompressionProfile,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct TunedXzCacheKey {
    input_hash: PayloadHash,
    allow_xz_extreme: bool,
    profile: CompressionProfile,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct StreamRangeCacheKey {
    input_hash: PayloadHash,
    abs_start_chunk: usize,
    abs_end_chunk: usize,
    realtime_mode: bool,
    allow_recursive_candidate: bool,
    profile: CompressionProfile,
}

#[derive(Debug, Default)]
struct CompressionCache {
    codec_outputs: HashMap<(PayloadHash, CodecProfileKey), (PayloadCodec, Vec<u8>)>,
    preflate_outputs: HashMap<(PayloadHash, u32), Option<(Vec<u8>, Vec<u8>)>>,
    stream_range_candidates: HashMap<StreamRangeCacheKey, PackedRangeCandidate>,
    // Cache for `choose_best_tuned_xz_candidate(input, profile, allow_xz_extreme)`. Reused
    // when both the top-level `raw-xz` candidate and the adaptive-transformed-xz winner
    // refinement encode the same byte stream (e.g. when the adaptive plan winner is
    // Identity), avoiding the ~30-minute duplicate XZ matrix walk on multi-MB inputs.
    tuned_xz_outputs: HashMap<TunedXzCacheKey, Option<(PayloadCodec, Vec<u8>)>>,
}

#[derive(Debug)]
struct CompressionContext {
    profile: CompressionProfile,
    #[allow(dead_code)] // IMMED-5: held for inner par_iter sites that want to reconsult the budget
    budgets: CompressionBudgets,
    timings: CandidateTimingStats,
    skipped_candidates: Vec<String>,
    cache_stats: CacheStats,
    cache: CompressionCache,
}

impl CompressionContext {
    fn new(profile: CompressionProfile) -> Self {
        Self {
            profile,
            budgets: CompressionBudgets::from_profile(profile),
            timings: CandidateTimingStats::default(),
            skipped_candidates: Vec::new(),
            cache_stats: CacheStats::default(),
            cache: CompressionCache::default(),
        }
    }

    /// Run a closure inside a rayon thread pool sized by the active budget.
    /// When max_parallel_codec_threads is 0 (or 1), uses the global pool directly.
    #[allow(dead_code)] // IMMED-5: kept for downstream callers that want a scoped pool install
    fn with_codec_pool<R, F: FnOnce() -> R + Send>(&self, f: F) -> R
    where
        R: Send,
    {
        let n = self.budgets.max_parallel_codec_threads;
        if n == 0 {
            return f();
        }
        match rayon::ThreadPoolBuilder::new().num_threads(n).build() {
            Ok(pool) => pool.install(f),
            Err(_) => f(),
        }
    }

    fn push_skipped(&mut self, note: impl Into<String>) {
        self.skipped_candidates.push(note.into());
    }
}

#[derive(Debug, Clone)]
enum StreamNodeKind {
    Piece {
        chunk_len: usize,
        method: PackMethod,
        pack_bytes: Vec<u8>,
    },
    Group {
        chunk_count: usize,
        original_len: usize,
        method: PackMethod,
        pack_bytes: Vec<u8>,
    },
    Split {
        level: u8,
        left: Box<StreamNode>,
        right: Box<StreamNode>,
    },
    GlobalSlice {
        chunk_count: usize,
        original_offset: usize,
        original_len: usize,
    },
}

#[derive(Debug, Clone)]
struct StreamNode {
    kind: StreamNodeKind,
    chunk_count: usize,
    original_len: usize,
    encoded_size: usize,
}

#[derive(Debug, Clone)]
struct StreamDecodedNode {
    bytes: Vec<u8>,
    chunk_count: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct StreamNodeCounts {
    piece_nodes: usize,
    grouped_nodes: usize,
    split_nodes: usize,
    max_depth_used: u8,
}

fn bits_needed_for_count(count: usize) -> u8 {
    if count <= 1 {
        return 1;
    }

    let mut bits = 0u8;
    let mut value = count - 1;
    while value > 0 {
        bits += 1;
        value >>= 1;
    }
    bits
}

fn pack_symbol_index(payload: &mut [u8], bit_offset: usize, value: u32, bits: u8) {
    for i in 0..bits {
        if ((value >> i) & 1) != 0 {
            let pos = bit_offset + i as usize;
            payload[pos >> 3] |= 1u8 << (pos & 7);
        }
    }
}

fn unpack_symbol_index(payload: &[u8], bit_offset: usize, bits: u8) -> u32 {
    let mut value = 0u32;
    for i in 0..bits {
        let pos = bit_offset + i as usize;
        let bit = (payload[pos >> 3] >> (pos & 7)) & 1;
        value |= (bit as u32) << i;
    }
    value
}

fn bit_at_msb_first(payload: &[u8], bit_index: usize) -> Option<u8> {
    let byte = *payload.get(bit_index >> 3)?;
    let shift = 7usize.saturating_sub(bit_index & 7);
    Some((byte >> shift) & 1)
}

fn push_bit_msb_first(out: &mut Vec<u8>, bit_index: &mut usize, bit: bool) {
    if (*bit_index & 7) == 0 {
        out.push(0);
    }

    if bit {
        let byte_idx = *bit_index >> 3;
        let shift = 7usize.saturating_sub(*bit_index & 7);
        out[byte_idx] |= 1u8 << shift;
    }

    *bit_index += 1;
}

fn push_code_msb_first(out: &mut Vec<u8>, bit_index: &mut usize, code: u64, len: u8) {
    for shift in (0..(len as usize)).rev() {
        let bit = ((code >> shift) & 1) != 0;
        push_bit_msb_first(out, bit_index, bit);
    }
}

fn payload_hash(data: &[u8]) -> PayloadHash {
    let fast64 = xxh3_64(data);
    let mut hasher = Blake3Hasher::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut strong128 = [0u8; 16];
    strong128.copy_from_slice(&digest.as_bytes()[..16]);
    PayloadHash {
        fast64,
        strong128,
        len: data.len(),
    }
}

fn build_index_stream(input: &[u8]) -> ZbitResult<IndexStream> {
    let mut present = [false; 256];
    let mut id_map = [u16::MAX; 256];
    let mut frequencies = [0u32; 256];

    for &b in input {
        present[b as usize] = true;
        frequencies[b as usize] = frequencies[b as usize].saturating_add(1);
    }

    let mut unique_symbols = Vec::with_capacity(256);
    for b in 0u16..=255 {
        if present[b as usize] {
            id_map[b as usize] = unique_symbols.len() as u16;
            unique_symbols.push(b as u8);
        }
    }

    let bits = bits_needed_for_count(unique_symbols.len());
    let payload_bits = input.len() * bits as usize;
    let payload_bytes = (payload_bits + 7) / 8;
    let mut payload = vec![0u8; payload_bytes];

    let mut bit_offset = 0usize;
    for &b in input {
        let id = id_map[b as usize];
        if id == u16::MAX {
            return Err(ZbitError::Internal(
                "id_map missing symbol while packing".to_string(),
            ));
        }
        pack_symbol_index(&mut payload, bit_offset, id as u32, bits);
        bit_offset += bits as usize;
    }

    Ok(IndexStream {
        unique_symbols,
        bits_per_symbol: bits,
        payload,
        frequencies,
    })
}

fn compute_huffman_lengths(symbols: &[u8], frequencies: &[u32; 256]) -> ZbitResult<Vec<u8>> {
    if symbols.is_empty() {
        return Ok(Vec::new());
    }

    if symbols.len() == 1 {
        return Ok(vec![1u8]);
    }

    let mut nodes = Vec::with_capacity(symbols.len() * 2);
    let mut heap = BinaryHeap::<Reverse<(u64, usize, usize)>>::new();
    let mut tie = 0usize;

    for &symbol in symbols {
        let freq = frequencies[symbol as usize] as u64;
        nodes.push(HuffmanNode {
            symbol: Some(symbol),
            left: None,
            right: None,
        });
        let idx = nodes.len() - 1;
        heap.push(Reverse((freq.max(1), tie, idx)));
        tie += 1;
    }

    while heap.len() > 1 {
        let Reverse((f_a, _, idx_a)) = heap
            .pop()
            .ok_or_else(|| ZbitError::Internal("huffman heap unexpectedly empty".to_string()))?;
        let Reverse((f_b, _, idx_b)) = heap
            .pop()
            .ok_or_else(|| ZbitError::Internal("huffman heap unexpectedly empty".to_string()))?;

        nodes.push(HuffmanNode {
            symbol: None,
            left: Some(idx_a),
            right: Some(idx_b),
        });

        let parent = nodes.len() - 1;
        heap.push(Reverse((f_a.saturating_add(f_b), tie, parent)));
        tie += 1;
    }

    let Reverse((_, _, root)) = heap
        .pop()
        .ok_or_else(|| ZbitError::Internal("huffman root missing".to_string()))?;

    let mut length_by_symbol = [0u8; 256];
    let mut stack = vec![(root, 0u8)];

    while let Some((node_idx, depth)) = stack.pop() {
        let node = nodes
            .get(node_idx)
            .ok_or_else(|| ZbitError::Internal("huffman node index out of range".to_string()))?;

        if let Some(symbol) = node.symbol {
            let assigned = depth.max(1);
            length_by_symbol[symbol as usize] = assigned;
            continue;
        }

        let next_depth = depth.saturating_add(1);
        if let Some(left) = node.left {
            stack.push((left, next_depth));
        }
        if let Some(right) = node.right {
            stack.push((right, next_depth));
        }
    }

    let lengths = symbols
        .iter()
        .map(|&symbol| length_by_symbol[symbol as usize])
        .collect::<Vec<_>>();

    Ok(lengths)
}

fn build_canonical_codes(symbols: &[u8], code_lengths: &[u8]) -> ZbitResult<Vec<(u8, u64, u8)>> {
    if symbols.len() != code_lengths.len() {
        return Err(ZbitError::Internal(
            "huffman symbols and lengths length mismatch".to_string(),
        ));
    }

    let mut entries = symbols
        .iter()
        .copied()
        .zip(code_lengths.iter().copied())
        .collect::<Vec<_>>();

    if entries.is_empty() {
        return Ok(Vec::new());
    }

    if entries.iter().any(|(_, len)| *len == 0) {
        return Err(ZbitError::Internal(
            "zero-length huffman code in non-empty dictionary".to_string(),
        ));
    }

    if entries.iter().any(|(_, len)| *len > MAX_HUFFMAN_CODE_BITS) {
        return Err(ZbitError::Limit(format!(
            "huffman code length exceeds {MAX_HUFFMAN_CODE_BITS} bits"
        )));
    }

    entries.sort_unstable_by_key(|(symbol, len)| (*len, *symbol));

    let mut out = Vec::with_capacity(entries.len());
    let mut code = 0u64;
    let mut prev_len = entries[0].1;

    out.push((entries[0].0, 0u64, prev_len));

    for (symbol, len) in entries.into_iter().skip(1) {
        if len < prev_len {
            return Err(ZbitError::Internal(
                "non-monotonic canonical length order".to_string(),
            ));
        }

        let shift = (len - prev_len) as u32;
        code = code
            .checked_add(1)
            .ok_or_else(|| ZbitError::Internal("canonical huffman code overflow".to_string()))?;
        code = code
            .checked_shl(shift)
            .ok_or_else(|| ZbitError::Internal("canonical huffman shift overflow".to_string()))?;

        out.push((symbol, code, len));
        prev_len = len;
    }

    Ok(out)
}

fn build_huffman_stream(input: &[u8], stream: &IndexStream) -> ZbitResult<Option<HuffmanStream>> {
    if input.is_empty() {
        return Ok(None);
    }

    let symbols = stream.unique_symbols.clone();
    let code_lengths = compute_huffman_lengths(&symbols, &stream.frequencies)?;

    if code_lengths.iter().any(|&len| len > MAX_HUFFMAN_CODE_BITS) {
        return Ok(None);
    }

    let codes = build_canonical_codes(&symbols, &code_lengths)?;

    let mut code_by_symbol = [0u64; 256];
    let mut len_by_symbol = [0u8; 256];
    for (symbol, code, len) in codes {
        code_by_symbol[symbol as usize] = code;
        len_by_symbol[symbol as usize] = len;
    }

    let mut payload = Vec::new();
    let mut bit_index = 0usize;
    for &byte in input {
        let len = len_by_symbol[byte as usize];
        if len == 0 {
            return Err(ZbitError::Internal(
                "missing huffman code for input symbol".to_string(),
            ));
        }
        let code = code_by_symbol[byte as usize];
        push_code_msb_first(&mut payload, &mut bit_index, code, len);
    }

    Ok(Some(HuffmanStream {
        symbols,
        code_lengths,
        payload,
    }))
}

fn decode_huffman_dictionary(
    dict_bytes: &[u8],
    unique_count: usize,
) -> ZbitResult<(Vec<u8>, Vec<u8>)> {
    if dict_bytes.len() != unique_count.saturating_mul(2) {
        return Err(ZbitError::Parse(
            "indexed-huffman dictionary size must be 2 * unique_count".to_string(),
        ));
    }

    let mut symbols = Vec::with_capacity(unique_count);
    let mut lengths = Vec::with_capacity(unique_count);

    for i in 0..unique_count {
        let symbol = dict_bytes[i * 2];
        let len = dict_bytes[i * 2 + 1];

        if len == 0 || len > MAX_HUFFMAN_CODE_BITS {
            return Err(ZbitError::Parse(
                "indexed-huffman dictionary contains invalid code length".to_string(),
            ));
        }

        symbols.push(symbol);
        lengths.push(len);
    }

    Ok((symbols, lengths))
}

fn insert_decode_code(root: &mut DecodeNode, code: u64, len: u8, symbol: u8) -> ZbitResult<()> {
    let mut cursor = root;

    for shift in (0..(len as usize)).rev() {
        let bit = ((code >> shift) & 1) as u8;

        if bit == 0 {
            let next = cursor
                .left
                .get_or_insert_with(|| Box::<DecodeNode>::default());
            cursor = next.as_mut();
        } else {
            let next = cursor
                .right
                .get_or_insert_with(|| Box::<DecodeNode>::default());
            cursor = next.as_mut();
        }
    }

    if cursor.symbol.replace(symbol).is_some() {
        return Err(ZbitError::Parse(
            "indexed-huffman dictionary assigns duplicate code".to_string(),
        ));
    }

    Ok(())
}

fn decode_huffman_payload(
    payload: &[u8],
    original_size: usize,
    symbols: &[u8],
    code_lengths: &[u8],
) -> ZbitResult<Vec<u8>> {
    let codes = build_canonical_codes(symbols, code_lengths)
        .map_err(|e| ZbitError::Parse(format!("invalid canonical huffman dictionary: {e}")))?;

    let mut root = DecodeNode::default();
    for (symbol, code, len) in codes {
        insert_decode_code(&mut root, code, len, symbol)?;
    }

    let mut out = vec![0u8; original_size];
    let mut bit_index = 0usize;

    for byte in out.iter_mut() {
        let mut node = &root;

        loop {
            if let Some(symbol) = node.symbol {
                *byte = symbol;
                break;
            }

            let bit = bit_at_msb_first(payload, bit_index).ok_or_else(|| {
                ZbitError::Parse("indexed-huffman payload ended before decoding output".to_string())
            })?;
            bit_index += 1;

            node = if bit == 0 {
                node.left
                    .as_deref()
                    .ok_or_else(|| ZbitError::Parse("invalid huffman prefix path".to_string()))?
            } else {
                node.right
                    .as_deref()
                    .ok_or_else(|| ZbitError::Parse("invalid huffman prefix path".to_string()))?
            };
        }
    }

    Ok(out)
}


fn model_blob_for_symbol(symbol: u8) -> ZbitResult<Vec<u8>> {
    let mut outputs = [0u8; 8];
    for i in 0..8u32 {
        outputs[i as usize] = if ((symbol >> i) & 1) != 0 { 1 } else { 0 };
    }

    let mut model = ZbitModel::new(3)?;
    model.compress_from_table(&outputs, None)?;
    Ok(model.to_bytes())
}

fn decode_symbol_from_blob(blob: &[u8]) -> ZbitResult<u8> {
    let model = ZbitModel::from_bytes(blob)?;
    let table = model.decompress_to_table()?;
    if table.len() != 8 {
        return Err(ZbitError::Internal(
            "symbol model did not decode to 8-entry table".to_string(),
        ));
    }

    let mut value = 0u8;
    for i in 0..8u8 {
        if table[i as usize] != 0 {
            value |= 1u8 << i;
        }
    }
    Ok(value)
}

fn build_circuit_blobs(symbols: &[u8]) -> ZbitResult<(Vec<Vec<u8>>, usize)> {
    let mut blobs = Vec::with_capacity(symbols.len());
    let mut total_bytes = 0usize;

    for &symbol in symbols {
        let blob = model_blob_for_symbol(symbol)?;
        total_bytes += 1 + 4 + blob.len();
        blobs.push(blob);
    }

    Ok((blobs, total_bytes))
}


fn write_pack_bytes(
    method: PackMethod,
    input: &[u8],
    stream: &IndexStream,
    circuit_blobs: Option<&[Vec<u8>]>,
    circuit_dict_bytes: usize,
    huffman_stream: Option<&HuffmanStream>,
    raw_deflate_payload: Option<&[u8]>,
    raw_zstd_payload: Option<&[u8]>,
    raw_brotli_payload: Option<&[u8]>,
    raw_xz_payload: Option<&[u8]>,
    framed_run: Option<&FramedPayloadRun>,
    recursive_stream: Option<&RecursiveCircuitStream>,
    monotonic_stream: Option<&MonotonicDeltaStream>,
    prime_sequence_stream: Option<&PrimeSequenceStream>,
    adaptive_xz_stream: Option<&AdaptiveTransformedXzStream>,
) -> ZbitResult<Vec<u8>> {
    if stream.unique_symbols.len() > u16::MAX as usize {
        return Err(ZbitError::Limit(
            "unique symbol count exceeds pack format limit".to_string(),
        ));
    }

    let (bits_per_symbol, unique_count, dict_size, payload_size) = match method {
        PackMethod::RawCopy => (0u8, 0usize, 0usize, input.len()),
        PackMethod::IndexedRaw => (
            stream.bits_per_symbol,
            stream.unique_symbols.len(),
            stream.unique_symbols.len(),
            stream.payload.len(),
        ),
        PackMethod::IndexedCircuit => (
            stream.bits_per_symbol,
            stream.unique_symbols.len(),
            circuit_dict_bytes,
            stream.payload.len(),
        ),
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.ok_or_else(|| {
                ZbitError::Internal("huffman stream missing for indexed-huffman method".to_string())
            })?;
            (
                0u8,
                hs.symbols.len(),
                hs.symbols.len() * 2,
                hs.payload.len(),
            )
        }
        PackMethod::RawDeflate => {
            let payload = raw_deflate_payload.ok_or_else(|| {
                ZbitError::Internal(
                    "raw-deflate payload missing for raw-deflate method".to_string(),
                )
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::RawZstd => {
            let payload = raw_zstd_payload.ok_or_else(|| {
                ZbitError::Internal("raw-zstd payload missing for raw-zstd method".to_string())
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::RawBrotli => {
            let payload = raw_brotli_payload.ok_or_else(|| {
                ZbitError::Internal(
                    "raw-brotli payload missing for raw-brotli method".to_string(),
                )
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::RawXz => {
            let payload = raw_xz_payload.ok_or_else(|| {
                ZbitError::Internal("raw-xz payload missing for raw-xz method".to_string())
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::FramedRaw => {
            let run = framed_run.ok_or_else(|| {
                ZbitError::Internal("framed-run missing for framed-raw method".to_string())
            })?;
            (0u8, 0usize, framed_dictionary_size(run), run.payload.len())
        }
        PackMethod::RecursiveCircuitXz => {
            let recursive = recursive_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "recursive stream missing for recursive-circuit-xz method".to_string(),
                )
            })?;
            (
                0u8,
                0usize,
                recursive_circuit_dictionary_size(recursive),
                recursive.transformed_encoded_len + recursive.correction_encoded_len,
            )
        }
        PackMethod::MonotonicDelta => {
            let monotonic = monotonic_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "monotonic stream missing for monotonic-delta method".to_string(),
                )
            })?;
            (
                0u8,
                0usize,
                monotonic_delta_dictionary_size(monotonic),
                monotonic.payload.len(),
            )
        }
        PackMethod::PrimeSequence => {
            let prime_sequence = prime_sequence_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "prime-sequence stream missing for prime-sequence method".to_string(),
                )
            })?;
            (
                0u8,
                0usize,
                prime_sequence_dictionary_size(prime_sequence),
                0usize,
            )
        }
        PackMethod::AdaptiveTransformedXz => {
            let adaptive = adaptive_xz_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "adaptive-transformed-xz stream missing for adaptive-transformed-xz method"
                        .to_string(),
                )
            })?;
            (
                0u8,
                0usize,
                adaptive_transformed_xz_dictionary_size(adaptive),
                adaptive.payload.len(),
            )
        }
    };

    let mut out = Vec::with_capacity(ZBPK_HEADER_BYTES + dict_size + payload_size);

    // v5 header layout (same packed fields as v4; v5 adds the opt-in prime-sequence
    // method and optional monotonic gap-stream transform metadata):
    //   magic u32        — wire-stable
    //   version u16      — wire-stable
    //   flags u16        — reserved, MUST be 0
    //   packed u8        — bits 0..3 = method index (16 slots), bits 4..7 = bits_per_symbol
    //                       (0..8). Both enums fit cleanly in 4 bits each.
    //   varint unique_count        — typically 0..256 for byte streams
    //   varint original_size       — varint avoids spending 8 bytes on tiny files
    //   varint dict_size           — varint
    //   varint payload_size        — varint
    // We assert at write time that method/bits_per_symbol fit in 4 bits — if a future
    // refactor pushes either above 15 we bump the version again rather than silently
    // truncate.
    push_u32(&mut out, ZBPK_MAGIC);
    push_u16(&mut out, ZBPK_VERSION);
    push_u16(&mut out, 0);
    let method_index = method.as_u8();
    if method_index > 0x0F {
        return Err(ZbitError::Internal(format!(
            "method index {method_index} exceeds 4-bit header field"
        )));
    }
    if bits_per_symbol > 0x0F {
        return Err(ZbitError::Internal(format!(
            "bits_per_symbol {bits_per_symbol} exceeds 4-bit header field"
        )));
    }
    let packed_byte = (method_index & 0x0F) | ((bits_per_symbol & 0x0F) << 4);
    out.push(packed_byte);
    push_varint_u64(&mut out, unique_count as u64);
    push_varint_u64(&mut out, input.len() as u64);
    push_varint_u64(&mut out, dict_size as u64);
    push_varint_u64(&mut out, payload_size as u64);

    match method {
        PackMethod::RawCopy => {}
        PackMethod::IndexedRaw => {
            out.extend_from_slice(&stream.unique_symbols);
        }
        PackMethod::IndexedCircuit => {
            let blobs = circuit_blobs.ok_or_else(|| {
                ZbitError::Internal("circuit blobs missing for indexed-circuit method".to_string())
            })?;
            if blobs.len() != stream.unique_symbols.len() {
                return Err(ZbitError::Internal(
                    "circuit blob count does not match unique symbol count".to_string(),
                ));
            }

            for (symbol, blob) in stream.unique_symbols.iter().zip(blobs.iter()) {
                out.push(*symbol);
                push_u32(&mut out, blob.len() as u32);
                out.extend_from_slice(blob);
            }
        }
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "huffman stream missing for indexed-huffman dictionary".to_string(),
                )
            })?;

            if hs.symbols.len() != hs.code_lengths.len() {
                return Err(ZbitError::Internal(
                    "huffman dictionary symbol/length mismatch".to_string(),
                ));
            }

            for (&symbol, &len) in hs.symbols.iter().zip(hs.code_lengths.iter()) {
                out.push(symbol);
                out.push(len);
            }
        }
        PackMethod::RawDeflate
        | PackMethod::RawZstd
        | PackMethod::RawBrotli
        | PackMethod::RawXz => {}
        PackMethod::PrimeSequence => {
            let prime_sequence = prime_sequence_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "prime-sequence stream missing for prime-sequence dictionary".to_string(),
                )
            })?;
            write_prime_sequence_dictionary(&mut out, prime_sequence);
        }
        PackMethod::FramedRaw => {
            let run = framed_run.ok_or_else(|| {
                ZbitError::Internal("framed-run missing for framed-raw dictionary".to_string())
            })?;
            write_framed_dictionary(&mut out, run);
        }
        PackMethod::RecursiveCircuitXz => {
            let recursive = recursive_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "recursive stream missing for recursive-circuit-xz dictionary".to_string(),
                )
            })?;
            write_recursive_circuit_dictionary(&mut out, recursive);
        }
        PackMethod::MonotonicDelta => {
            let monotonic = monotonic_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "monotonic stream missing for monotonic-delta dictionary".to_string(),
                )
            })?;
            write_monotonic_delta_dictionary(&mut out, monotonic);
        }
        PackMethod::AdaptiveTransformedXz => {
            let adaptive = adaptive_xz_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "adaptive-transformed-xz stream missing for adaptive-transformed-xz dictionary"
                        .to_string(),
                )
            })?;
            write_adaptive_transformed_xz_dictionary(&mut out, adaptive);
        }
    }

    match method {
        PackMethod::RawCopy => out.extend_from_slice(input),
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => {
            out.extend_from_slice(&stream.payload)
        }
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "huffman stream missing for indexed-huffman payload".to_string(),
                )
            })?;
            out.extend_from_slice(&hs.payload);
        }
        PackMethod::RawDeflate => {
            let payload = raw_deflate_payload.ok_or_else(|| {
                ZbitError::Internal(
                    "raw-deflate payload missing for raw-deflate method".to_string(),
                )
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::RawZstd => {
            let payload = raw_zstd_payload.ok_or_else(|| {
                ZbitError::Internal("raw-zstd payload missing for raw-zstd method".to_string())
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::RawBrotli => {
            let payload = raw_brotli_payload.ok_or_else(|| {
                ZbitError::Internal(
                    "raw-brotli payload missing for raw-brotli method".to_string(),
                )
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::RawXz => {
            let payload = raw_xz_payload.ok_or_else(|| {
                ZbitError::Internal("raw-xz payload missing for raw-xz method".to_string())
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::FramedRaw => {
            let run = framed_run.ok_or_else(|| {
                ZbitError::Internal("framed-run missing for framed-raw payload".to_string())
            })?;
            out.extend_from_slice(&run.payload);
        }
        PackMethod::RecursiveCircuitXz => {
            let recursive = recursive_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "recursive stream missing for recursive-circuit-xz payload".to_string(),
                )
            })?;
            out.extend_from_slice(&recursive.transformed_payload);
            out.extend_from_slice(&recursive.corrections_payload);
        }
        PackMethod::MonotonicDelta => {
            let monotonic = monotonic_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "monotonic stream missing for monotonic-delta payload".to_string(),
                )
            })?;
            out.extend_from_slice(&monotonic.payload);
        }
        PackMethod::PrimeSequence => {}
        PackMethod::AdaptiveTransformedXz => {
            let adaptive = adaptive_xz_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "adaptive-transformed-xz stream missing for adaptive-transformed-xz payload"
                        .to_string(),
                )
            })?;
            out.extend_from_slice(&adaptive.payload);
        }
    }

    Ok(out)
}

fn compress_adaptive_to_bytes(input: &[u8]) -> ZbitResult<(Vec<u8>, PackStats)> {
    let profile = CompressionProfile::from_env();
    let budgets = CompressionBudgets::from_profile(profile);
    if budgets.max_parallel_codec_threads != 0 {
        if let Ok(pool) = rayon::ThreadPoolBuilder::new()
            .num_threads(budgets.max_parallel_codec_threads)
            .build()
        {
            return pool.install(move || compress_adaptive_to_bytes_inner(input, profile));
        }
    }
    compress_adaptive_to_bytes_inner(input, profile)
}

fn compress_adaptive_to_bytes_inner(
    input: &[u8],
    profile: CompressionProfile,
) -> ZbitResult<(Vec<u8>, PackStats)> {
    let total_timer = Instant::now();
    let mut context = CompressionContext::new(profile);

    // The prelude candidates are independent of each other (huffman depends only on the
    // index stream), so they evaluate concurrently. Each closure times its own work; the
    // per-stage timings therefore report CPU spans that overlap in wall time. The cheap
    // raw-xz preset-3 estimate joins the same batch: it is needed later as the gate for
    // the adaptive-transformed-xz path and the raw-xz tuning-matrix skip decision.
    let (index_huffman, (deflate_res, (zstd_res, (brotli_res, xz_cheap_res)))) = rayon::join(
        || {
            let index_timer = Instant::now();
            let stream = build_index_stream(input)?;
            let index_ms = index_timer.elapsed().as_secs_f64() * 1000.0;
            let huffman_timer = Instant::now();
            let huffman_stream = build_huffman_stream(input, &stream)?;
            let huffman_ms = huffman_timer.elapsed().as_secs_f64() * 1000.0;
            Ok::<_, ZbitError>((stream, huffman_stream, index_ms, huffman_ms))
        },
        || {
            rayon::join(
                || {
                    let timer = Instant::now();
                    let payload = build_raw_deflate_payload(input)?;
                    Ok::<_, ZbitError>((payload, timer.elapsed().as_secs_f64() * 1000.0))
                },
                || {
                    rayon::join(
                        || {
                            let timer = Instant::now();
                            let payload = build_raw_zstd_payload(input)?;
                            Ok::<_, ZbitError>((payload, timer.elapsed().as_secs_f64() * 1000.0))
                        },
                        || {
                            rayon::join(
                                || {
                                    let timer = Instant::now();
                                    let payload = if should_evaluate_raw_brotli(input) {
                                        Some(build_raw_brotli_payload(input)?)
                                    } else {
                                        None
                                    };
                                    Ok::<_, ZbitError>((
                                        payload,
                                        timer.elapsed().as_secs_f64() * 1000.0,
                                    ))
                                },
                                || {
                                    let timer = Instant::now();
                                    let payload = xz_encode_easy_preset(input, 3)?;
                                    Ok::<_, ZbitError>((
                                        payload,
                                        timer.elapsed().as_secs_f64() * 1000.0,
                                    ))
                                },
                            )
                        },
                    )
                },
            )
        },
    );
    let (stream, huffman_stream, index_ms, huffman_ms) = index_huffman?;
    context.timings.index_stream_ms = index_ms;
    context.timings.huffman_stream_ms = huffman_ms;

    let raw_candidate_bytes = ZBPK_HEADER_BYTES + input.len();
    let indexed_raw_candidate_bytes =
        ZBPK_HEADER_BYTES + stream.unique_symbols.len() + stream.payload.len();
    let indexed_huffman_candidate_bytes = huffman_stream
        .as_ref()
        .map(|hs| ZBPK_HEADER_BYTES + hs.symbols.len() * 2 + hs.payload.len());

    let (raw_deflate_payload, raw_deflate_ms) = deflate_res?;
    context.timings.raw_deflate_ms = raw_deflate_ms;
    let raw_deflate_candidate_bytes = Some(ZBPK_HEADER_BYTES + raw_deflate_payload.len());

    let (raw_zstd_payload, raw_zstd_ms) = zstd_res?;
    context.timings.raw_zstd_ms = raw_zstd_ms;
    let raw_zstd_candidate_bytes = Some(ZBPK_HEADER_BYTES + raw_zstd_payload.len());

    let (raw_brotli_payload, raw_brotli_ms) = brotli_res?;
    context.timings.raw_brotli_ms = raw_brotli_ms;
    if raw_brotli_payload.is_none() {
        context.push_skipped("raw-brotli skipped: bounded text-likeness gate did not pass");
    }
    let raw_brotli_candidate_bytes = raw_brotli_payload
        .as_ref()
        .map(|payload| ZBPK_HEADER_BYTES + payload.len());

    // The cheap raw-xz estimate (preset 3, no tuning matrix) is the gate for the
    // adaptive-transformed-xz path AND, more importantly, the fast way to decide whether
    // running the full raw-xz tuning matrix is worth it. On a 95 MiB PyTorch model the
    // full matrix walks ~5-10 minutes; if the estimate proves a structural candidate wins
    // by a wide margin, the matrix is skipped entirely (see the tiered gate below).
    let (raw_xz_cheap_payload, raw_xz_cheap_ms) = xz_cheap_res?;
    let raw_xz_cheap_size = raw_xz_cheap_payload.len();
    let mut raw_xz_elapsed_ms = raw_xz_cheap_ms;

    if !context.profile.enable_xz_extreme_for_raw_xz() {
        context.push_skipped(format!(
            "raw-xz extreme profiles skipped by '{}' profile",
            context.profile.name()
        ));
    }

    let framed_timer = Instant::now();
    let framed_run = build_framed_payload_run(input);
    context.timings.framed_extraction_ms = framed_timer.elapsed().as_secs_f64() * 1000.0;
    let framed_raw_candidate_bytes = framed_run
        .as_ref()
        .map(|run| ZBPK_HEADER_BYTES + framed_dictionary_size(run) + run.payload.len());
    let recursive_stream = match framed_run.as_ref() {
        Some(run) => build_recursive_circuit_stream(input, run, &mut context)?,
        None => {
            context.push_skipped("recursive-circuit-xz skipped: framed payload analyzer unavailable");
            None
        }
    };
    let recursive_circuit_xz_candidate_bytes = recursive_stream.as_ref().map(|recursive| {
        ZBPK_HEADER_BYTES
            + recursive_circuit_dictionary_size(recursive)
            + recursive.transformed_encoded_len
            + recursive.correction_encoded_len
    });
    let best_prelude_candidate_bytes = [
        Some(raw_candidate_bytes),
        Some(indexed_raw_candidate_bytes),
        indexed_huffman_candidate_bytes,
        raw_deflate_candidate_bytes,
        raw_zstd_candidate_bytes,
        raw_brotli_candidate_bytes,
        Some(ZBPK_HEADER_BYTES + raw_xz_cheap_size),
    ]
    .into_iter()
    .flatten()
    .min();
    let prime_sequence_stream = if should_evaluate_prime_sequence() {
        build_prime_sequence_stream(input)?
    } else {
        context.push_skipped(
            "prime-sequence skipped: disabled by default; set ZBIT_ENABLE_PRIME_SEQUENCE=1 \
             to enable exact prime-table detection",
        );
        None
    };
    let prime_sequence_candidate_bytes = prime_sequence_stream
        .as_ref()
        .map(|stream| ZBPK_HEADER_BYTES + prime_sequence_dictionary_size(stream));
    let monotonic_stream = if prime_sequence_stream.is_some() {
        context.push_skipped(
            "monotonic-delta skipped: exact prime-sequence candidate covers numeric table",
        );
        None
    } else {
        build_monotonic_delta_stream(input, &mut context, best_prelude_candidate_bytes)?
    };
    let monotonic_delta_candidate_bytes = monotonic_stream.as_ref().map(|stream| {
        ZBPK_HEADER_BYTES + monotonic_delta_dictionary_size(stream) + stream.payload.len()
    });

    let adaptive_xz_stream = build_adaptive_transformed_xz_stream(
        input,
        &mut context,
        recursive_stream.is_some(),
        Some(raw_xz_cheap_size),
    )?;

    let adaptive_transformed_xz_candidate_bytes = adaptive_xz_stream.as_ref().map(|stream| {
        ZBPK_HEADER_BYTES + adaptive_transformed_xz_dictionary_size(stream) + stream.payload.len()
    });

    // Now decide whether the full raw-xz tuning matrix is worth the cost. All structural
    // candidates (recursive-circuit-xz, monotonic-delta, adaptive-transformed-xz) are
    // already sized at this point, so the decision tiers are:
    //   1. adaptive clearly wins over the cheap estimate (the original depth_anything gate);
    //   2. the best structural candidate is at most 5/8 of the cheap raw-xz total — the
    //      tuning matrix would need a >37% gain over preset 3 to even tie, beyond anything
    //      the preset-9 parameter matrix has ever produced;
    //   3. the structural candidate beats the cheap estimate by at least 1 KiB but not by
    //      the 5/8 margin: pay a single easy XZ-9 probe. Every matrix entry is a preset-9
    //      variant whose spread vs easy-9 is a few percent, so if the structural candidate
    //      still beats easy-9 by more than 1/16 the matrix cannot win and is skipped.
    // Whenever the matrix is skipped the probe payload stands in as the raw-xz candidate
    // and loses selection to the structural method, leaving the ratio unchanged.
    let raw_xz_cheap_total = ZBPK_HEADER_BYTES + raw_xz_cheap_size;
    let best_structural_total = [
        recursive_circuit_xz_candidate_bytes,
        monotonic_delta_candidate_bytes,
        prime_sequence_candidate_bytes,
        adaptive_transformed_xz_candidate_bytes,
    ]
    .into_iter()
    .flatten()
    .min();

    let raw_xz_matrix_timer = Instant::now();
    let adaptive_clearly_wins = adaptive_xz_stream
        .as_ref()
        .is_some_and(|adaptive| adaptive.payload.len() + 1024 < raw_xz_cheap_size);
    let structural_beats_cheap_widely = best_structural_total
        .is_some_and(|best| best + 1024 <= raw_xz_cheap_total * 5 / 8);
    let structural_beats_cheap = best_structural_total
        .is_some_and(|best| best + 1024 < raw_xz_cheap_total);
    let raw_xz_payload = if adaptive_clearly_wins {
        context.push_skipped(
            "raw-xz full tuning matrix skipped: adaptive-transformed-xz clearly wins"
                .to_string(),
        );
        raw_xz_cheap_payload
    } else if structural_beats_cheap_widely {
        context.push_skipped(
            "raw-xz full tuning matrix skipped: structural candidate beats cheap raw-xz \
             estimate beyond any plausible tuning gain"
                .to_string(),
        );
        raw_xz_cheap_payload
    } else if structural_beats_cheap {
        let easy9_payload = xz_encode_easy_preset(input, 9)?;
        let easy9_total = ZBPK_HEADER_BYTES + easy9_payload.len();
        let easy9_margin_total = easy9_total.saturating_sub(easy9_total / 16);
        if best_structural_total.is_some_and(|best| best + 1024 <= easy9_margin_total) {
            context.push_skipped(
                "raw-xz full tuning matrix skipped: structural candidate beats easy XZ-9 \
                 probe beyond the tuning-matrix margin"
                    .to_string(),
            );
            if easy9_payload.len() < raw_xz_cheap_size {
                easy9_payload
            } else {
                raw_xz_cheap_payload
            }
        } else {
            build_raw_xz_payload(input, &mut context)?
        }
    } else {
        build_raw_xz_payload(input, &mut context)?
    };
    raw_xz_elapsed_ms += raw_xz_matrix_timer.elapsed().as_secs_f64() * 1000.0;
    context.timings.raw_xz_ms = raw_xz_elapsed_ms;
    let raw_xz_candidate_bytes = Some(ZBPK_HEADER_BYTES + raw_xz_payload.len());

    let mut eval = PackEvaluation::new();
    eval.original_size = input.len();
    eval.symbol_bits = 8;
    eval.unique_symbols = stream.unique_symbols.len();
    eval.payload_bytes = stream.payload.len();
    eval.raw_total_bytes = raw_candidate_bytes;
    eval.indexed_raw_total_bytes = indexed_raw_candidate_bytes;
    eval.indexed_huffman_total_bytes = indexed_huffman_candidate_bytes;
    eval.raw_deflate_total_bytes = raw_deflate_candidate_bytes;
    eval.raw_zstd_total_bytes = raw_zstd_candidate_bytes;
    eval.raw_brotli_total_bytes = raw_brotli_candidate_bytes;
    eval.raw_xz_total_bytes = raw_xz_candidate_bytes;
    eval.framed_raw_total_bytes = framed_raw_candidate_bytes;
    eval.recursive_circuit_xz_total_bytes = recursive_circuit_xz_candidate_bytes;
    eval.monotonic_delta_total_bytes = monotonic_delta_candidate_bytes;
    eval.prime_sequence_total_bytes = prime_sequence_candidate_bytes;
    eval.adaptive_transformed_xz_total_bytes = adaptive_transformed_xz_candidate_bytes;

    let (should_eval_circuit, circuit_rule_note) = should_evaluate_circuit(&eval);
    if !should_eval_circuit {
        context.push_skipped(format!("indexed-circuit skipped: {circuit_rule_note}"));
    }

    let (circuit_blobs, circuit_dictionary_bytes, indexed_circuit_candidate_bytes) =
        if should_eval_circuit {
            let (blobs, dict_bytes) = build_circuit_blobs(&stream.unique_symbols)?;
            let candidate = ZBPK_HEADER_BYTES + dict_bytes + stream.payload.len();
            eval.indexed_circuit_total_bytes = Some(candidate);
            (Some(blobs), dict_bytes, Some(candidate))
        } else {
            (None, 0usize, None)
        };

    choose_best_method(&mut eval);

    let pack_bytes = write_pack_bytes(
        eval.chosen_method,
        input,
        &stream,
        circuit_blobs.as_deref(),
        circuit_dictionary_bytes,
        huffman_stream.as_ref(),
        Some(raw_deflate_payload.as_slice()),
        Some(raw_zstd_payload.as_slice()),
        raw_brotli_payload.as_deref(),
        Some(raw_xz_payload.as_slice()),
        framed_run.as_ref(),
        recursive_stream.as_ref(),
        monotonic_stream.as_ref(),
        prime_sequence_stream.as_ref(),
        adaptive_xz_stream.as_ref(),
    )?;

    let (bits_per_symbol, payload_bytes, huffman_dictionary_bytes) = match eval.chosen_method {
        PackMethod::RawCopy => (0u8, input.len(), 0usize),
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => {
            (stream.bits_per_symbol, stream.payload.len(), 0usize)
        }
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal("indexed-huffman selected without huffman stream".to_string())
            })?;
            (0u8, hs.payload.len(), hs.symbols.len() * 2)
        }
        PackMethod::RawDeflate => (0u8, raw_deflate_payload.len(), 0usize),
        PackMethod::RawZstd => (0u8, raw_zstd_payload.len(), 0usize),
        PackMethod::RawBrotli => {
            let payload = raw_brotli_payload.as_ref().ok_or_else(|| {
                ZbitError::Internal("raw-brotli selected without raw-brotli payload".to_string())
            })?;
            (0u8, payload.len(), 0usize)
        }
        PackMethod::RawXz => (0u8, raw_xz_payload.len(), 0usize),
        PackMethod::FramedRaw => {
            let run = framed_run.as_ref().ok_or_else(|| {
                ZbitError::Internal("framed-raw selected without framed run".to_string())
            })?;
            (0u8, run.payload.len(), 0usize)
        }
        PackMethod::RecursiveCircuitXz => {
            let recursive = recursive_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal(
                    "recursive-circuit-xz selected without recursive stream".to_string(),
                )
            })?;
            (
                0u8,
                recursive.transformed_encoded_len + recursive.correction_encoded_len,
                0usize,
            )
        }
        PackMethod::MonotonicDelta => {
            let monotonic = monotonic_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal("monotonic-delta selected without monotonic stream".to_string())
            })?;
            (0u8, monotonic.payload.len(), 0usize)
        }
        PackMethod::PrimeSequence => (0u8, 0usize, 0usize),
        PackMethod::AdaptiveTransformedXz => {
            let adaptive = adaptive_xz_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal(
                    "adaptive-transformed-xz selected without adaptive stream".to_string(),
                )
            })?;
            (0u8, adaptive.payload.len(), 0usize)
        }
    };

    let active_profile = context.profile.name().to_string();
    let skipped_candidates = context.skipped_candidates;
    let mut timings = context.timings;
    let cache_stats = context.cache_stats;
    let total_ms = total_timer.elapsed().as_secs_f64() * 1000.0;
    timings.total_compression_ms = total_ms;
    timings.compression_throughput_mib_s = if total_ms > 0.0 {
        (input.len() as f64) / (total_ms / 1000.0) / (1024.0 * 1024.0)
    } else {
        0.0
    };

    let stats = PackStats {
        original_size: input.len(),
        compressed_size: pack_bytes.len(),
        unique_symbols: stream.unique_symbols.len(),
        bits_per_symbol,
        payload_bytes,
        raw_dictionary_bytes: stream.unique_symbols.len(),
        circuit_dictionary_bytes,
        huffman_dictionary_bytes,
        raw_candidate_bytes,
        indexed_raw_candidate_bytes,
        indexed_circuit_candidate_bytes,
        indexed_huffman_candidate_bytes,
        raw_deflate_candidate_bytes,
        raw_zstd_candidate_bytes,
        raw_brotli_candidate_bytes,
        raw_xz_candidate_bytes,
        framed_raw_candidate_bytes,
        recursive_circuit_xz_candidate_bytes,
        monotonic_delta_candidate_bytes,
        prime_sequence_candidate_bytes,
        adaptive_transformed_xz_candidate_bytes,
        chosen_method: eval.chosen_method,
        chosen_reason: eval.chosen_reason,
        circuit_rule_note,
        active_profile,
        skipped_candidates,
        timings,
        cache_stats,
    };

    Ok((pack_bytes, stats))
}

pub fn compress_adaptive_to_file(input: &[u8], path: impl AsRef<Path>) -> ZbitResult<PackStats> {
    let (pack_bytes, stats) = compress_adaptive_to_bytes(input)?;
    fs::write(path.as_ref(), &pack_bytes)?;
    Ok(stats)
}

fn decode_circuit_dictionary(dict_bytes: &[u8], unique_count: usize) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;
    let mut symbols = Vec::with_capacity(unique_count);

    for _ in 0..unique_count {
        let stored_symbol = read_u8(dict_bytes, &mut cursor)?;
        let blob_len = read_u32(dict_bytes, &mut cursor)? as usize;

        let blob = dict_bytes
            .get(cursor..cursor + blob_len)
            .ok_or_else(|| ZbitError::Parse("circuit dictionary blob out of bounds".to_string()))?;
        cursor += blob_len;

        let decoded = decode_symbol_from_blob(blob)?;
        if decoded != stored_symbol {
            return Err(ZbitError::Parse(
                "decoded symbol does not match dictionary symbol".to_string(),
            ));
        }

        symbols.push(decoded);
    }

    if cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in circuit dictionary".to_string(),
        ));
    }

    Ok(symbols)
}

fn decompress_pack_bytes(bytes: &[u8]) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;

    let magic = read_u32(&bytes, &mut cursor)?;
    if magic != ZBPK_MAGIC {
        return Err(ZbitError::Parse("invalid ZBPK magic".to_string()));
    }

    let version = read_u16(&bytes, &mut cursor)?;
    if !(4..=ZBPK_VERSION).contains(&version) {
        return Err(ZbitError::Parse(format!(
            "unsupported ZBPK version: {version}"
        )));
    }

    let flags = read_u16(&bytes, &mut cursor)?;
    if flags != 0 {
        return Err(ZbitError::Parse(
            "non-zero flags are unsupported".to_string(),
        ));
    }

    let packed_byte = read_u8(&bytes, &mut cursor)?;
    let method = PackMethod::from_u8(packed_byte & 0x0F)
        .ok_or_else(|| ZbitError::Parse("invalid pack method".to_string()))?;
    let bits_per_symbol = (packed_byte >> 4) & 0x0F;
    let unique_count = read_varint_u64(&bytes, &mut cursor)? as usize;
    let original_size = read_varint_u64(&bytes, &mut cursor)? as usize;
    let dict_size = read_varint_u64(&bytes, &mut cursor)? as usize;
    let payload_size = read_varint_u64(&bytes, &mut cursor)? as usize;

    if original_size > ZBPK_MAX_OUTPUT_BYTES {
        return Err(ZbitError::Parse(format!(
            "original_size exceeds safety bound ({ZBPK_MAX_OUTPUT_BYTES} bytes)"
        )));
    }

    let dict = bytes
        .get(cursor..cursor + dict_size)
        .ok_or_else(|| ZbitError::Parse("dictionary range out of bounds".to_string()))?;
    cursor += dict_size;

    let payload = bytes
        .get(cursor..cursor + payload_size)
        .ok_or_else(|| ZbitError::Parse("payload range out of bounds".to_string()))?;
    cursor += payload_size;

    if cursor != bytes.len() {
        return Err(ZbitError::Parse("trailing bytes in pack".to_string()));
    }

    match method {
        PackMethod::RawCopy => {
            if bits_per_symbol != 0
                || unique_count != 0
                || dict_size != 0
                || payload_size != original_size
            {
                return Err(ZbitError::Parse(
                    "invalid raw-copy header/dictionary/payload sizing".to_string(),
                ));
            }
            return Ok(payload.to_vec());
        }
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => {
            if bits_per_symbol == 0 {
                return Err(ZbitError::Parse(
                    "indexed methods require bits_per_symbol > 0".to_string(),
                ));
            }
        }
        PackMethod::RawDeflate => {
            if bits_per_symbol != 0 || unique_count != 0 || dict_size != 0 {
                return Err(ZbitError::Parse(
                    "raw-deflate requires bits_per_symbol=0, unique_count=0, dict_size=0"
                        .to_string(),
                ));
            }
            return decode_raw_deflate_payload(payload, original_size);
        }
        PackMethod::RawZstd => {
            if bits_per_symbol != 0 || unique_count != 0 || dict_size != 0 {
                return Err(ZbitError::Parse(
                    "raw-zstd requires bits_per_symbol=0, unique_count=0, dict_size=0".to_string(),
                ));
            }
            return decode_raw_zstd_payload(payload, original_size);
        }
        PackMethod::RawBrotli => {
            if bits_per_symbol != 0 || unique_count != 0 || dict_size != 0 {
                return Err(ZbitError::Parse(
                    "raw-brotli requires bits_per_symbol=0, unique_count=0, dict_size=0"
                        .to_string(),
                ));
            }
            return decode_raw_brotli_payload(payload, original_size);
        }
        PackMethod::RawXz => {
            if bits_per_symbol != 0 || unique_count != 0 || dict_size != 0 {
                return Err(ZbitError::Parse(
                    "raw-xz requires bits_per_symbol=0, unique_count=0, dict_size=0".to_string(),
                ));
            }
            return decode_raw_xz_payload(payload, original_size);
        }
        PackMethod::FramedRaw => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "framed-raw requires bits_per_symbol=0 and unique_count=0".to_string(),
                ));
            }
            return decode_framed_payload(dict, payload, original_size);
        }
        PackMethod::RecursiveCircuitXz => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "recursive-circuit-xz requires bits_per_symbol=0 and unique_count=0"
                        .to_string(),
                ));
            }
            return decode_recursive_circuit_payload(dict, payload, original_size);
        }
        PackMethod::MonotonicDelta => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "monotonic-delta requires bits_per_symbol=0 and unique_count=0".to_string(),
                ));
            }
            return decode_monotonic_delta_payload(dict, payload, original_size);
        }
        PackMethod::PrimeSequence => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "prime-sequence requires bits_per_symbol=0 and unique_count=0".to_string(),
                ));
            }
            return decode_prime_sequence_payload(dict, payload, original_size);
        }
        PackMethod::AdaptiveTransformedXz => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "adaptive-transformed-xz requires bits_per_symbol=0 and unique_count=0"
                        .to_string(),
                ));
            }
            return decode_adaptive_transformed_xz_payload(dict, payload, original_size);
        }
        PackMethod::IndexedHuffman => {
            if bits_per_symbol != 0 {
                return Err(ZbitError::Parse(
                    "indexed-huffman requires bits_per_symbol == 0".to_string(),
                ));
            }

            let (symbols, lengths) = decode_huffman_dictionary(dict, unique_count)?;
            return decode_huffman_payload(payload, original_size, &symbols, &lengths);
        }
    }

    let symbol_by_id = match method {
        PackMethod::IndexedRaw => {
            if dict_size != unique_count {
                return Err(ZbitError::Parse(
                    "indexed-raw dictionary size must equal unique_count".to_string(),
                ));
            }
            dict.to_vec()
        }
        PackMethod::IndexedCircuit => decode_circuit_dictionary(dict, unique_count)?,
        PackMethod::RawCopy
        | PackMethod::IndexedHuffman
        | PackMethod::RawDeflate
        | PackMethod::RawZstd
        | PackMethod::RawBrotli
        | PackMethod::RawXz
        | PackMethod::FramedRaw
        | PackMethod::RecursiveCircuitXz
        | PackMethod::MonotonicDelta
        | PackMethod::PrimeSequence
        | PackMethod::AdaptiveTransformedXz => {
            unreachable!()
        }
    };

    let needed_bits = original_size * bits_per_symbol as usize;
    if payload_size * 8 < needed_bits {
        return Err(ZbitError::Parse(
            "payload does not contain enough bits for output size".to_string(),
        ));
    }

    let mut out = vec![0u8; original_size];
    let mut bit_offset = 0usize;

    for byte in out.iter_mut() {
        let idx = unpack_symbol_index(payload, bit_offset, bits_per_symbol) as usize;
        bit_offset += bits_per_symbol as usize;

        if idx >= symbol_by_id.len() {
            return Err(ZbitError::Parse(
                "symbol index out of dictionary range".to_string(),
            ));
        }

        *byte = symbol_by_id[idx];
    }

    Ok(out)
}

pub fn decompress_file(path: impl AsRef<Path>) -> ZbitResult<Vec<u8>> {
    let bytes = fs::read(path)?;
    decompress_pack_bytes(&bytes)
}

fn usize_to_u32(value: usize, what: &str) -> ZbitResult<u32> {
    u32::try_from(value)
        .map_err(|_| ZbitError::Limit(format!("{what} exceeds u32::MAX in stream format")))
}

fn usize_to_u64(value: usize, what: &str) -> ZbitResult<u64> {
    u64::try_from(value)
        .map_err(|_| ZbitError::Limit(format!("{what} exceeds u64::MAX in stream format")))
}

fn u64_to_usize(value: u64, what: &str) -> ZbitResult<usize> {
    usize::try_from(value)
        .map_err(|_| ZbitError::Parse(format!("{what} exceeds platform usize in stream format")))
}

fn validate_stream_options(options: &StreamPackOptions) -> ZbitResult<()> {
    if options.chunk_size == 0 {
        return Err(ZbitError::InvalidArg(
            "stream chunk_size must be greater than 0",
        ));
    }
    if options.key_piece_interval == 0 {
        return Err(ZbitError::InvalidArg(
            "stream key_piece_interval must be greater than 0",
        ));
    }
    if options.max_group_pieces == 0 {
        return Err(ZbitError::InvalidArg(
            "stream max_group_pieces must be greater than 0",
        ));
    }
    if options.max_group_depth > 32 {
        return Err(ZbitError::InvalidArg(
            "stream max_group_depth must be <= 32",
        ));
    }
    let _ = usize_to_u32(options.chunk_size, "stream chunk_size")?;
    let _ = usize_to_u32(options.key_piece_interval, "stream key_piece_interval")?;
    let _ = usize_to_u32(options.max_group_pieces, "stream max_group_pieces")?;
    Ok(())
}

fn expected_chunk_count(original_size: usize, chunk_size: usize) -> usize {
    if original_size == 0 {
        0
    } else {
        (original_size + chunk_size - 1) / chunk_size
    }
}

fn expected_stream_block_len(
    original_size: usize,
    chunk_size: usize,
    total_chunks: usize,
    first_chunk_index: usize,
    block_chunk_count: usize,
) -> ZbitResult<usize> {
    if block_chunk_count == 0 {
        return Ok(0);
    }

    let block_end = first_chunk_index
        .checked_add(block_chunk_count)
        .ok_or_else(|| ZbitError::Parse("stream block end overflow".to_string()))?;
    if block_end > total_chunks {
        return Err(ZbitError::Parse(
            "stream block exceeds total chunk count".to_string(),
        ));
    }

    if total_chunks == 0 {
        return Ok(0);
    }

    let last_chunk_len = original_size
        .checked_sub(
            chunk_size
                .checked_mul(total_chunks.saturating_sub(1))
                .ok_or_else(|| ZbitError::Parse("stream chunk geometry overflow".to_string()))?,
        )
        .ok_or_else(|| ZbitError::Parse("stream chunk geometry underflow".to_string()))?;

    if block_end < total_chunks {
        return chunk_size
            .checked_mul(block_chunk_count)
            .ok_or_else(|| ZbitError::Parse("stream block size overflow".to_string()));
    }

    if block_chunk_count == 1 {
        return Ok(last_chunk_len);
    }

    let full_chunks_in_block = block_chunk_count - 1;
    let full_len = chunk_size
        .checked_mul(full_chunks_in_block)
        .ok_or_else(|| ZbitError::Parse("stream final block size overflow".to_string()))?;
    full_len
        .checked_add(last_chunk_len)
        .ok_or_else(|| ZbitError::Parse("stream final block size overflow".to_string()))
}

fn split_stream_chunks(input: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    if input.is_empty() {
        return Vec::new();
    }
    input
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}
