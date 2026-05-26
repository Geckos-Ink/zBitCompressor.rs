// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PackMethod {
    RawCopy,
    IndexedRaw,
    IndexedCircuit,
    IndexedHuffman,
    RawDeflate,
    RawZstd,
    FramedRaw,
    RecursiveCircuitXz,
    MonotonicDelta,
    RawXz,
    // Raw Brotli candidate for text-heavy payloads where LZMA2 is not the best entropy
    // model. It has no dictionary: the ZBPK payload is the Brotli stream.
    RawBrotli,
    // Adaptive reversible transform applied to the raw input followed by a codec encode.
    // Generalises the transform machinery (previously gated behind framed-deflate detection)
    // to inputs that are not framed deflate — e.g. PyTorch model weights, raw float tensors,
    // fixed-stride binary tables. Stores `(transform_kind, period, head, codec, plain_len)`
    // in the dictionary so the decoder can invert the transform and recover the original
    // bytes.
    AdaptiveTransformedXz,
}

impl PackMethod {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::RawCopy => 0,
            Self::IndexedRaw => 1,
            Self::IndexedCircuit => 2,
            Self::IndexedHuffman => 3,
            Self::RawDeflate => 4,
            Self::RawZstd => 5,
            Self::FramedRaw => 6,
            Self::RecursiveCircuitXz => 7,
            Self::MonotonicDelta => 8,
            Self::RawXz => 9,
            Self::RawBrotli => 10,
            Self::AdaptiveTransformedXz => 11,
        }
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::RawCopy),
            1 => Some(Self::IndexedRaw),
            2 => Some(Self::IndexedCircuit),
            3 => Some(Self::IndexedHuffman),
            4 => Some(Self::RawDeflate),
            5 => Some(Self::RawZstd),
            6 => Some(Self::FramedRaw),
            7 => Some(Self::RecursiveCircuitXz),
            8 => Some(Self::MonotonicDelta),
            9 => Some(Self::RawXz),
            10 => Some(Self::RawBrotli),
            11 => Some(Self::AdaptiveTransformedXz),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::RawCopy => "raw-copy",
            Self::IndexedRaw => "indexed-raw",
            Self::IndexedCircuit => "indexed-circuit",
            Self::IndexedHuffman => "indexed-huffman",
            Self::RawDeflate => "raw-deflate",
            Self::RawZstd => "raw-zstd",
            Self::FramedRaw => "framed-raw",
            Self::RecursiveCircuitXz => "recursive-circuit-xz",
            Self::MonotonicDelta => "monotonic-delta",
            Self::RawXz => "raw-xz",
            Self::RawBrotli => "raw-brotli",
            Self::AdaptiveTransformedXz => "adaptive-transformed-xz",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackEvaluation {
    pub original_size: usize,
    pub symbol_bits: usize,
    pub unique_symbols: usize,
    pub payload_bytes: usize,

    pub raw_total_bytes: usize,
    pub indexed_raw_total_bytes: usize,

    pub indexed_circuit_total_bytes: Option<usize>,
    pub indexed_huffman_total_bytes: Option<usize>,
    pub raw_deflate_total_bytes: Option<usize>,
    pub raw_zstd_total_bytes: Option<usize>,
    pub raw_xz_total_bytes: Option<usize>,
    pub raw_brotli_total_bytes: Option<usize>,
    pub framed_raw_total_bytes: Option<usize>,
    pub recursive_circuit_xz_total_bytes: Option<usize>,
    pub monotonic_delta_total_bytes: Option<usize>,
    pub adaptive_transformed_xz_total_bytes: Option<usize>,

    pub chosen_method: PackMethod,
    pub chosen_reason: String,
}

impl PackEvaluation {
    pub fn new() -> Self {
        Self {
            original_size: 0,
            symbol_bits: 0,
            unique_symbols: 0,
            payload_bytes: 0,
            raw_total_bytes: 0,
            indexed_raw_total_bytes: 0,
            indexed_circuit_total_bytes: None,
            indexed_huffman_total_bytes: None,
            raw_deflate_total_bytes: None,
            raw_zstd_total_bytes: None,
            raw_xz_total_bytes: None,
            raw_brotli_total_bytes: None,
            framed_raw_total_bytes: None,
            recursive_circuit_xz_total_bytes: None,
            monotonic_delta_total_bytes: None,
            adaptive_transformed_xz_total_bytes: None,
            chosen_method: PackMethod::RawCopy,
            chosen_reason: String::new(),
        }
    }
}

pub fn should_evaluate_circuit(eval: &PackEvaluation) -> (bool, String) {
    if eval.unique_symbols == 0 {
        return (
            false,
            "empty input: no circuit dictionary needed".to_string(),
        );
    }

    if eval.symbol_bits <= 8 {
        return (
            false,
            "symbol width <= 8 bits: raw dictionary is denser than circuit descriptors".to_string(),
        );
    }

    if eval.unique_symbols > 64 {
        return (
            false,
            "too many unique symbols: circuit dictionary overhead likely dominates".to_string(),
        );
    }

    if eval.payload_bytes < 4096 {
        return (
            false,
            "payload too small: circuit dictionary overhead cannot be amortized".to_string(),
        );
    }

    (
        true,
        "circuit candidate allowed by heuristic rules".to_string(),
    )
}

pub fn choose_best_method(eval: &mut PackEvaluation) {
    let mut best_method = PackMethod::RawCopy;
    let mut best_size = eval.raw_total_bytes;
    let mut best_reason = format!(
        "raw-copy baseline selected ({} bytes)",
        eval.raw_total_bytes
    );

    if eval.indexed_raw_total_bytes < best_size {
        best_method = PackMethod::IndexedRaw;
        best_size = eval.indexed_raw_total_bytes;
        best_reason = format!(
            "indexed-raw improves size: {} -> {} bytes",
            eval.raw_total_bytes, eval.indexed_raw_total_bytes
        );
    }

    if let Some(circuit_size) = eval.indexed_circuit_total_bytes {
        let threshold = best_size.saturating_sub(best_size / 100);
        if circuit_size < threshold {
            best_method = PackMethod::IndexedCircuit;
            best_size = circuit_size;
            best_reason = format!("indexed-circuit chosen after rules: {circuit_size} bytes");
        }
    }

    if let Some(huffman_size) = eval.indexed_huffman_total_bytes {
        if huffman_size < best_size {
            best_method = PackMethod::IndexedHuffman;
            best_size = huffman_size;
            best_reason = format!(
                "indexed-huffman improves size: {} -> {} bytes",
                eval.raw_total_bytes, huffman_size
            );
        }
    }

    if let Some(deflate_size) = eval.raw_deflate_total_bytes {
        if deflate_size < best_size {
            best_method = PackMethod::RawDeflate;
            best_size = deflate_size;
            best_reason = format!(
                "raw-deflate improves size: {} -> {} bytes",
                eval.raw_total_bytes, deflate_size
            );
        }
    }

    if let Some(zstd_size) = eval.raw_zstd_total_bytes {
        if zstd_size < best_size {
            best_method = PackMethod::RawZstd;
            best_size = zstd_size;
            best_reason = format!(
                "raw-zstd improves size: {} -> {} bytes",
                eval.raw_total_bytes, zstd_size
            );
        }
    }

    if let Some(xz_size) = eval.raw_xz_total_bytes {
        if xz_size < best_size {
            best_method = PackMethod::RawXz;
            best_size = xz_size;
            best_reason = format!(
                "raw-xz improves size: {} -> {} bytes",
                eval.raw_total_bytes, xz_size
            );
        }
    }

    if let Some(brotli_size) = eval.raw_brotli_total_bytes {
        if brotli_size < best_size {
            best_method = PackMethod::RawBrotli;
            best_size = brotli_size;
            best_reason = format!(
                "raw-brotli improves size: {} -> {} bytes",
                eval.raw_total_bytes, brotli_size
            );
        }
    }

    if let Some(framed_size) = eval.framed_raw_total_bytes {
        if framed_size < best_size {
            best_method = PackMethod::FramedRaw;
            best_size = framed_size;
            best_reason = format!(
                "framed-raw improves size: {} -> {} bytes",
                eval.raw_total_bytes, framed_size
            );
        }
    }

    if let Some(recursive_size) = eval.recursive_circuit_xz_total_bytes {
        if recursive_size < best_size {
            best_method = PackMethod::RecursiveCircuitXz;
            best_size = recursive_size;
            best_reason = format!(
                "recursive-circuit-xz improves size: {} -> {} bytes",
                eval.raw_total_bytes, recursive_size
            );
        }
    }

    if let Some(monotonic_delta_size) = eval.monotonic_delta_total_bytes {
        if monotonic_delta_size < best_size {
            best_method = PackMethod::MonotonicDelta;
            best_size = monotonic_delta_size;
            best_reason = format!(
                "monotonic-delta improves size: {} -> {} bytes",
                eval.raw_total_bytes, monotonic_delta_size
            );
        }
    }

    if let Some(adaptive_size) = eval.adaptive_transformed_xz_total_bytes {
        if adaptive_size < best_size {
            best_method = PackMethod::AdaptiveTransformedXz;
            best_size = adaptive_size;
            best_reason = format!(
                "adaptive-transformed-xz improves size: {} -> {} bytes",
                eval.raw_total_bytes, adaptive_size
            );
        }
    }

    eval.chosen_method = best_method;
    eval.chosen_reason = best_reason;
    let _ = best_size;
}
