// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

fn build_raw_deflate_payload(input: &[u8]) -> ZbitResult<Vec<u8>> {
    build_raw_deflate_payload_with_compression(input, Compression::best())
}

fn build_raw_deflate_payload_with_compression(
    input: &[u8],
    compression: Compression,
) -> ZbitResult<Vec<u8>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), compression);
    encoder
        .write_all(input)
        .map_err(|e| ZbitError::Io(format!("zlib write failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| ZbitError::Io(format!("zlib finish failed: {e}")))
}

fn decode_raw_deflate_payload(payload: &[u8], original_size: usize) -> ZbitResult<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(payload);
    let mut out = Vec::with_capacity(original_size.min(1 << 20));
    decoder
        .read_to_end(&mut out)
        .map_err(|e| ZbitError::Parse(format!("zlib decode failed: {e}")))?;

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "raw-deflate output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

fn build_raw_zstd_payload(input: &[u8]) -> ZbitResult<Vec<u8>> {
    build_raw_zstd_payload_with_level(input, 19)
}

fn build_raw_zstd_payload_with_level(input: &[u8], level: i32) -> ZbitResult<Vec<u8>> {
    zstd_stream::encode_all(input, level)
        .map_err(|e| ZbitError::Io(format!("zstd encode failed: {e}")))
}

fn decode_raw_zstd_payload(payload: &[u8], original_size: usize) -> ZbitResult<Vec<u8>> {
    let out = zstd_stream::decode_all(payload)
        .map_err(|e| ZbitError::Parse(format!("zstd decode failed: {e}")))?;

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "raw-zstd output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

fn brotli_encode_with_mode(
    input: &[u8],
    mode: brotli::enc::backward_references::BrotliEncoderMode,
) -> ZbitResult<Vec<u8>> {
    let mut params = brotli::enc::BrotliEncoderParams::default();
    params.quality = 11;
    // 24 is the largest standard window; it only matters for inputs above the 4 MiB
    // window implied by lgwin 22 and costs nothing on smaller ones.
    params.lgwin = 24;
    params.mode = mode;
    let mut out = Vec::new();
    brotli::BrotliCompress(&mut Cursor::new(input), &mut out, &params)
        .map_err(|e| ZbitError::Io(format!("brotli encode failed: {e}")))?;
    Ok(out)
}

fn build_raw_brotli_payload(input: &[u8]) -> ZbitResult<Vec<u8>> {
    // The candidate is gated by a text-likeness check, so the TEXT context model usually
    // wins; GENERIC is kept as the second probe because mixed markdown/binary payloads
    // occasionally prefer it. Both decode with the same stream format.
    let (text, generic) = rayon::join(
        || {
            brotli_encode_with_mode(
                input,
                brotli::enc::backward_references::BrotliEncoderMode::BROTLI_MODE_TEXT,
            )
        },
        || {
            brotli_encode_with_mode(
                input,
                brotli::enc::backward_references::BrotliEncoderMode::BROTLI_MODE_GENERIC,
            )
        },
    );
    let text = text?;
    let generic = generic?;
    Ok(if text.len() <= generic.len() {
        text
    } else {
        generic
    })
}

fn decode_raw_brotli_payload(payload: &[u8], original_size: usize) -> ZbitResult<Vec<u8>> {
    let mut decoder = brotli::Decompressor::new(Cursor::new(payload), 64 * 1024);
    let mut out = Vec::with_capacity(original_size.min(1 << 20));
    let mut buf = [0u8; 8192];

    loop {
        let read = decoder
            .read(&mut buf)
            .map_err(|e| ZbitError::Parse(format!("brotli decode failed: {e}")))?;
        if read == 0 {
            break;
        }
        if out.len().saturating_add(read) > original_size {
            return Err(ZbitError::Parse(format!(
                "raw-brotli output exceeds expected length {original_size}"
            )));
        }
        out.extend_from_slice(&buf[..read]);
    }

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "raw-brotli output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

fn build_raw_xz_payload(
    input: &[u8],
    context: &mut CompressionContext,
) -> ZbitResult<Vec<u8>> {
    let allow_xz_extreme = context.profile.enable_xz_extreme_for_raw_xz();
    let (_, payload) = choose_best_tuned_xz_candidate_cached(input, allow_xz_extreme, context)?
        .ok_or_else(|| ZbitError::Internal("raw-xz candidate set is empty".to_string()))?;
    Ok(payload)
}

fn decode_raw_xz_payload(payload: &[u8], original_size: usize) -> ZbitResult<Vec<u8>> {
    let out = xz_decode_all(payload)?;
    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "raw-xz output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }
    Ok(out)
}


fn max_value_for_width(width: usize) -> Option<u64> {
    if width == 0 || width > 8 {
        return None;
    }
    if width == 8 {
        return Some(u64::MAX);
    }
    Some((1u64 << (width * 8)) - 1)
}

fn read_le_u64_width(slice: &[u8], width: usize) -> Option<u64> {
    if width == 0 || width > 8 || slice.len() != width {
        return None;
    }
    let mut value = 0u64;
    for (shift, &byte) in slice.iter().enumerate() {
        value |= (byte as u64) << (shift * 8);
    }
    Some(value)
}

fn write_le_u64_width(out: &mut Vec<u8>, value: u64, width: usize) -> ZbitResult<()> {
    let max = max_value_for_width(width).ok_or_else(|| {
        ZbitError::Internal("monotonic-delta width must be between 1 and 8".to_string())
    })?;
    if value > max {
        return Err(ZbitError::Parse(format!(
            "monotonic-delta value {value} does not fit in {width} bytes"
        )));
    }
    for shift in 0..width {
        out.push(((value >> (shift * 8)) & 0xFF) as u8);
    }
    Ok(())
}

fn encode_zigzag_i64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn decode_zigzag_i64(value: u64) -> Option<i64> {
    if value > ((i64::MAX as u64) << 1) + 1 {
        return None;
    }
    Some(((value >> 1) as i64) ^ (-((value & 1) as i64)))
}

fn encode_gap_varints(gaps: &[u64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(gaps.len());
    for &gap in gaps {
        push_varint_u64(&mut out, gap);
    }
    out
}

fn encode_gap_bytes(gaps: &[u64]) -> Option<Vec<u8>> {
    if gaps.iter().any(|&gap| gap > u8::MAX as u64) {
        return None;
    }
    let mut out = Vec::with_capacity(gaps.len());
    for &gap in gaps {
        out.push(gap as u8);
    }
    Some(out)
}

fn encode_gap_delta_varints(gaps: &[u64]) -> Option<Vec<u8>> {
    if gaps.is_empty() {
        return Some(Vec::new());
    }
    if gaps.iter().any(|&gap| gap > i64::MAX as u64) {
        return None;
    }

    let mut out = Vec::with_capacity(gaps.len());
    push_varint_u64(&mut out, gaps[0]);

    let mut prev = gaps[0] as i64;
    for &gap_u64 in gaps.iter().skip(1) {
        let gap = gap_u64 as i64;
        let delta = gap - prev;
        push_varint_u64(&mut out, encode_zigzag_i64(delta));
        prev = gap;
    }

    Some(out)
}

fn common_suffix_trailing_zero_shift(gaps: &[u64]) -> u8 {
    gaps.iter()
        .skip(1)
        .copied()
        .filter(|&gap| gap > 0)
        .map(u64::trailing_zeros)
        .min()
        .unwrap_or(0)
        .min(63) as u8
}

fn encode_trailing_zero_gap_varints(gaps: &[u64], shift: u8) -> Option<Vec<u8>> {
    if gaps.is_empty() || shift == 0 || shift >= 64 {
        return None;
    }

    let mut out = Vec::with_capacity(gaps.len());
    push_varint_u64(&mut out, gaps[0]);
    for &gap in gaps.iter().skip(1) {
        if (gap & ((1u64 << shift) - 1)) != 0 {
            return None;
        }
        push_varint_u64(&mut out, gap >> shift);
    }
    Some(out)
}

fn encode_trailing_zero_gap_bytes(gaps: &[u64], shift: u8) -> Option<Vec<u8>> {
    if gaps.is_empty() || gaps[0] > u8::MAX as u64 || shift == 0 || shift >= 64 {
        return None;
    }

    let mut out = Vec::with_capacity(gaps.len());
    out.push(gaps[0] as u8);
    for &gap in gaps.iter().skip(1) {
        if (gap & ((1u64 << shift) - 1)) != 0 {
            return None;
        }
        let scaled = gap >> shift;
        if scaled == 0 || scaled > u8::MAX as u64 {
            return None;
        }
        out.push(scaled as u8);
    }
    Some(out)
}

// v3 dictionary layout for MonotonicDelta (variable length, typically ~10-15 B):
//   meta u8           bits 0..2 = width (1..=8 maps to 0..=7 via `width - 1`)
//                     bits 3..5 = mode  (5 modes fit in 3 bits)
//                     bits 6..7 = codec (4 values fit in 2 bits)
//   shift u8          trailing_zero_shift (0..63 — could be 6 bits but byte-aligned
//                     for round-trip clarity; the upper 2 bits are reserved and MUST be 0)
//   varint count
//   varint first_value
//   varint transformed_plain_len
// Replaces the prior fixed 28-byte layout. For typical primary.3b-style payloads
// (count ~1 M, gaps small) the new dictionary is ~12 bytes — savings on this corpus
// matter less in ratio because the file is already 0.174, but the principle is the
// same as the other dictionaries: spend only the bits each enumeration value needs.
fn monotonic_delta_dictionary_size(stream: &MonotonicDeltaStream) -> usize {
    1 /* meta */ + 1 /* shift */
        + varint_u64_len(stream.count)
        + varint_u64_len(stream.first_value)
        + varint_u64_len(stream.transformed_plain_len as u64)
}

// v3 dictionary layout for AdaptiveTransformedXz (variable length, typically ~7-10 B):
//   codec_kind u8     bits 0..1 = codec (4 values fit in 2 bits)
//                     bits 2..7 = transform_kind_index (49 values fit in 6 bits,
//                                  encoded via the same `kind_to_compact_index` table
//                                  used by the recursive-circuit and topology paths)
//   varint period
//   varint head
//   varint plain_len
// Reduces from the prior fixed 18-byte layout: a typical adaptive-transformed-xz
// candidate on a 95 MiB PyTorch model goes from 18 bytes to 9-10 bytes (5-byte plain_len
// varint + ~1-2 bytes period/head + 1 byte codec/kind).
fn adaptive_transformed_xz_dictionary_size(stream: &AdaptiveTransformedXzStream) -> usize {
    1 + varint_u64_len(stream.transform_plan.period as u64)
        + varint_u64_len(stream.transform_plan.head as u64)
        + varint_u64_len(stream.plain_len)
}

fn write_adaptive_transformed_xz_dictionary(
    out: &mut Vec<u8>,
    stream: &AdaptiveTransformedXzStream,
) {
    let kind_u8 = stream.transform_plan.kind.as_u8();
    let kind_index = kind_to_compact_index(kind_u8).unwrap_or(kind_u8 & 0x3F);
    let codec_kind = (stream.codec.as_u8() & 0x03) | ((kind_index & 0x3F) << 2);
    out.push(codec_kind);
    push_varint_u64(out, stream.transform_plan.period as u64);
    push_varint_u64(out, stream.transform_plan.head as u64);
    push_varint_u64(out, stream.plain_len);
}

fn decode_adaptive_transformed_xz_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;
    let codec_kind = read_u8(dict_bytes, &mut cursor)?;
    let codec = PayloadCodec::from_u8(codec_kind & 0x03).ok_or_else(|| {
        ZbitError::Parse("adaptive-transformed-xz dictionary has invalid codec".to_string())
    })?;
    let kind_index = (codec_kind >> 2) & 0x3F;
    let kind_u8 = compact_index_to_kind(kind_index).ok_or_else(|| {
        ZbitError::Parse(format!(
            "adaptive-transformed-xz dictionary has invalid kind index {kind_index}"
        ))
    })?;
    let kind = CircuitTransformKind::from_u8(kind_u8).ok_or_else(|| {
        ZbitError::Parse("adaptive-transformed-xz dictionary has invalid transform kind".to_string())
    })?;
    let period = read_varint_u64(dict_bytes, &mut cursor)? as u32;
    let head = read_varint_u64(dict_bytes, &mut cursor)? as u32;
    let plain_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    if cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in adaptive-transformed-xz dictionary".to_string(),
        ));
    }
    if plain_len != original_size {
        return Err(ZbitError::Parse(format!(
            "adaptive-transformed-xz plain_len {plain_len} does not match original_size {original_size}"
        )));
    }
    let transformed = decode_with_codec(payload, codec, plain_len)?;
    let plan = CircuitTransformPlan { kind, period, head };
    let recovered = invert_transform_plan(&transformed, plain_len, &plan).ok_or_else(|| {
        ZbitError::Parse(
            "adaptive-transformed-xz inverse transform produced invalid output".to_string(),
        )
    })?;
    if recovered.len() != plain_len {
        return Err(ZbitError::Parse(format!(
            "adaptive-transformed-xz output length mismatch: expected {plain_len} got {}",
            recovered.len()
        )));
    }
    Ok(recovered)
}

fn write_monotonic_delta_dictionary(out: &mut Vec<u8>, stream: &MonotonicDeltaStream) {
    // width ∈ 1..=8 stored as `width - 1` in 3 bits; mode in 3 bits; codec in 2 bits.
    debug_assert!(stream.width >= 1 && stream.width <= 8);
    let width_field = ((stream.width - 1) & 0x07) as u8;
    let mode_field = (stream.mode.as_u8() & 0x07) << 3;
    let codec_field = (stream.codec.as_u8() & 0x03) << 6;
    out.push(width_field | mode_field | codec_field);
    out.push(stream.trailing_zero_shift & 0x3F);
    push_varint_u64(out, stream.count);
    push_varint_u64(out, stream.first_value);
    push_varint_u64(out, stream.transformed_plain_len as u64);
}

fn build_monotonic_delta_stream(
    input: &[u8],
    context: &mut CompressionContext,
) -> ZbitResult<Option<MonotonicDeltaStream>> {
    let candidate_widths = [3usize, 4, 2, 5, 6, 7, 8, 1];
    let mut best: Option<(MonotonicDeltaStream, usize)> = None;

    for width in candidate_widths {
        if input.is_empty() || input.len() < width * 3 || (input.len() % width) != 0 {
            continue;
        }

        let count = input.len() / width;
        if count < 2 {
            continue;
        }

        let mut values = Vec::with_capacity(count);
        let mut valid = true;
        for chunk in input.chunks_exact(width) {
            let Some(value) = read_le_u64_width(chunk, width) else {
                valid = false;
                break;
            };
            values.push(value);
        }
        if !valid {
            continue;
        }

        let mut gaps = Vec::with_capacity(count.saturating_sub(1));
        let mut monotonic = true;
        for idx in 1..values.len() {
            if values[idx] <= values[idx - 1] {
                monotonic = false;
                break;
            }
            gaps.push(values[idx] - values[idx - 1]);
        }
        if !monotonic {
            continue;
        }

        let mut mode_candidates = Vec::new();
        if let Some(bytes) = encode_gap_bytes(&gaps) {
            mode_candidates.push((MonotonicDeltaMode::GapBytes, bytes));
        }
        mode_candidates.push((MonotonicDeltaMode::GapVarint, encode_gap_varints(&gaps)));
        if let Some(bytes) = encode_gap_delta_varints(&gaps) {
            mode_candidates.push((MonotonicDeltaMode::GapDeltaVarint, bytes));
        }
        let trailing_zero_shift = common_suffix_trailing_zero_shift(&gaps);
        if let Some(bytes) = encode_trailing_zero_gap_bytes(&gaps, trailing_zero_shift) {
            mode_candidates.push((MonotonicDeltaMode::GapTrailingZeroBytes, bytes));
        }
        if let Some(bytes) = encode_trailing_zero_gap_varints(&gaps, trailing_zero_shift) {
            mode_candidates.push((MonotonicDeltaMode::GapTrailingZeroVarint, bytes));
        }

        for (mode, transformed) in mode_candidates {
            let (codec, payload) =
                choose_best_codec_cached(&transformed, true, true, context)?;
            let shift = match mode {
                MonotonicDeltaMode::GapTrailingZeroBytes
                | MonotonicDeltaMode::GapTrailingZeroVarint => trailing_zero_shift,
                MonotonicDeltaMode::GapBytes
                | MonotonicDeltaMode::GapVarint
                | MonotonicDeltaMode::GapDeltaVarint => 0,
            };
            let stream = MonotonicDeltaStream {
                width: width as u8,
                count: count as u64,
                first_value: values[0],
                transformed_plain_len: transformed.len(),
                mode,
                trailing_zero_shift: shift,
                codec,
                payload,
            };
            let total_size = monotonic_delta_dictionary_size(&stream) + stream.payload.len();
            match &best {
                Some((_, best_size)) if *best_size <= total_size => {}
                _ => best = Some((stream, total_size)),
            }
        }
    }

    Ok(best.map(|(stream, _)| stream))
}

fn decode_monotonic_delta_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut dict_cursor = 0usize;
    let meta = read_u8(dict_bytes, &mut dict_cursor)?;
    let width = ((meta & 0x07) as usize) + 1;
    let mode_field = (meta >> 3) & 0x07;
    let mode = MonotonicDeltaMode::from_u8(mode_field).ok_or_else(|| {
        ZbitError::Parse("monotonic-delta dictionary has invalid mode".to_string())
    })?;
    let codec_field = (meta >> 6) & 0x03;
    let codec = PayloadCodec::from_u8(codec_field).ok_or_else(|| {
        ZbitError::Parse("monotonic-delta dictionary has invalid codec".to_string())
    })?;
    let shift_field = read_u8(dict_bytes, &mut dict_cursor)?;
    if shift_field & 0xC0 != 0 {
        return Err(ZbitError::Parse(
            "monotonic-delta dictionary reserves the top 2 bits of trailing_zero_shift"
                .to_string(),
        ));
    }
    let trailing_zero_shift = shift_field & 0x3F;
    let count = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let first_value = read_varint_u64(dict_bytes, &mut dict_cursor)?;
    let transformed_plain_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;

    if dict_cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in monotonic-delta dictionary".to_string(),
        ));
    }
    if width == 0 || width > 8 {
        return Err(ZbitError::Parse(
            "monotonic-delta width must be between 1 and 8".to_string(),
        ));
    }

    let expected_original = count
        .checked_mul(width)
        .ok_or_else(|| ZbitError::Parse("monotonic-delta output length overflow".to_string()))?;
    if expected_original != original_size {
        return Err(ZbitError::Parse(format!(
            "monotonic-delta output length mismatch: expected {original_size} got {expected_original}"
        )));
    }
    if count == 0 {
        if original_size == 0 {
            return Ok(Vec::new());
        }
        return Err(ZbitError::Parse(
            "monotonic-delta count is zero for non-empty payload".to_string(),
        ));
    }

    let transformed = decode_with_codec(payload, codec, transformed_plain_len)?;
    let mut transformed_cursor = 0usize;
    let max_value = max_value_for_width(width).ok_or_else(|| {
        ZbitError::Parse("monotonic-delta width does not have a valid value range".to_string())
    })?;

    let mut out = Vec::with_capacity(original_size);
    write_le_u64_width(&mut out, first_value, width)?;
    let mut current = first_value;

    let mut prev_gap = 0u64;
    for idx in 1..count {
        let gap = match mode {
            MonotonicDeltaMode::GapBytes => read_u8(&transformed, &mut transformed_cursor)? as u64,
            MonotonicDeltaMode::GapVarint => {
                read_varint_u64(&transformed, &mut transformed_cursor)?
            }
            MonotonicDeltaMode::GapTrailingZeroBytes => {
                if idx == 1 {
                    read_u8(&transformed, &mut transformed_cursor)? as u64
                } else {
                    let scaled = read_u8(&transformed, &mut transformed_cursor)? as u64;
                    scaled
                        .checked_shl(trailing_zero_shift as u32)
                        .ok_or_else(|| {
                            ZbitError::Parse(
                                "monotonic-delta trailing-zero byte gap overflow".to_string(),
                            )
                        })?
                }
            }
            MonotonicDeltaMode::GapTrailingZeroVarint => {
                if idx == 1 {
                    read_varint_u64(&transformed, &mut transformed_cursor)?
                } else {
                    let scaled = read_varint_u64(&transformed, &mut transformed_cursor)?;
                    scaled
                        .checked_shl(trailing_zero_shift as u32)
                        .ok_or_else(|| {
                            ZbitError::Parse(
                                "monotonic-delta trailing-zero varint gap overflow".to_string(),
                            )
                        })?
                }
            }
            MonotonicDeltaMode::GapDeltaVarint => {
                if idx == 1 {
                    let first_gap = read_varint_u64(&transformed, &mut transformed_cursor)?;
                    prev_gap = first_gap;
                    first_gap
                } else {
                    let encoded_delta = read_varint_u64(&transformed, &mut transformed_cursor)?;
                    let delta = decode_zigzag_i64(encoded_delta).ok_or_else(|| {
                        ZbitError::Parse(
                            "monotonic-delta gap-delta zigzag value exceeds i64 range".to_string(),
                        )
                    })? as i128;
                    let next_gap = (prev_gap as i128).checked_add(delta).ok_or_else(|| {
                        ZbitError::Parse(
                            "monotonic-delta gap-delta overflow while decoding".to_string(),
                        )
                    })?;
                    if next_gap <= 0 || next_gap > u64::MAX as i128 {
                        return Err(ZbitError::Parse(
                            "monotonic-delta decoded non-positive gap".to_string(),
                        ));
                    }
                    let gap = next_gap as u64;
                    prev_gap = gap;
                    gap
                }
            }
        };

        if gap == 0 {
            return Err(ZbitError::Parse(
                "monotonic-delta gap must be strictly positive".to_string(),
            ));
        }

        current = current.checked_add(gap).ok_or_else(|| {
            ZbitError::Parse("monotonic-delta value overflow while decoding".to_string())
        })?;
        if current > max_value {
            return Err(ZbitError::Parse(
                "monotonic-delta decoded value exceeds width capacity".to_string(),
            ));
        }
        write_le_u64_width(&mut out, current, width)?;
    }

    if transformed_cursor != transformed.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in monotonic-delta transformed payload".to_string(),
        ));
    }

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "monotonic-delta output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}


fn xz_encode_easy_preset(data: &[u8], preset: u32) -> ZbitResult<Vec<u8>> {
    let stream = Stream::new_easy_encoder(preset, Check::None)
        .map_err(|e| ZbitError::Io(format!("xz easy stream init failed: {e}")))?;
    let mut encoder = XzWriterEncoder::new_stream(Vec::new(), stream);
    encoder
        .write_all(data)
        .map_err(|e| ZbitError::Io(format!("xz easy encode write failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| ZbitError::Io(format!("xz easy encode finish failed: {e}")))
}

#[derive(Clone, Copy)]
struct XzTuningParams {
    literal_context_bits: u32,
    literal_position_bits: u32,
    position_bits: u32,
    dict_size: u32,
    nice_len: u32,
    match_finder: MatchFinder,
    mode: Mode,
    depth: u32,
    // 0 disables the BCJ-style delta pre-filter; 1..=256 prepends a delta filter with that
    // distance ahead of LZMA2. Useful for byte-aligned structured payloads (channel-interleaved
    // images, fixed-stride binary tables) where same-stride bytes are highly correlated.
    delta_dist: u32,
}

#[derive(Clone, Copy)]
enum XzCandidateKind {
    Easy,
    Tuned(XzTuningParams),
}

#[derive(Clone, Copy)]
struct XzCodecCandidate {
    rank: usize,
    codec: PayloadCodec,
    preset: u32,
    kind: XzCandidateKind,
}

fn xz_encode_with_tuning(
    data: &[u8],
    preset: u32,
    tuning: XzTuningParams,
) -> ZbitResult<Vec<u8>> {
    let mut options = LzmaOptions::new_preset(preset)
        .map_err(|e| ZbitError::Io(format!("xz options preset init failed: {e}")))?;
    options.dict_size(tuning.dict_size);
    options.literal_context_bits(tuning.literal_context_bits);
    options.literal_position_bits(tuning.literal_position_bits);
    options.position_bits(tuning.position_bits);
    options.mode(tuning.mode);
    options.nice_len(tuning.nice_len);
    options.match_finder(tuning.match_finder);
    options.depth(tuning.depth);

    let mut filters = Filters::new();
    // The xz2 binding does not currently expose the LZMA delta filter; tunings that request a
    // delta distance fall back to plain LZMA2. When the upstream binding adds .delta(dist) we
    // can flip this back on without further changes elsewhere in the matrix.
    let _ = tuning.delta_dist;
    filters.lzma2(&options);

    let stream = Stream::new_stream_encoder(&filters, Check::None)
        .map_err(|e| ZbitError::Io(format!("xz stream init failed: {e}")))?;

    let mut encoder = XzWriterEncoder::new_stream(Vec::new(), stream);
    encoder
        .write_all(data)
        .map_err(|e| ZbitError::Io(format!("xz encode write failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| ZbitError::Io(format!("xz encode finish failed: {e}")))
}

fn xz_encode_with_profile(
    data: &[u8],
    preset: u32,
    literal_context_bits: u32,
) -> ZbitResult<Vec<u8>> {
    xz_encode_with_tuning(
        data,
        preset,
        XzTuningParams {
            literal_context_bits,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
    )
}

fn tuned_xz_param_matrix() -> Vec<XzTuningParams> {
    vec![
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 1,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 3,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 4,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 4,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 2,
            literal_position_bits: 2,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 1,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 1,
            position_bits: 1,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 32 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 16 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 8 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 64 * 1024 * 1024,
            nice_len: 192,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 64 * 1024 * 1024,
            nice_len: 128,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 32 * 1024 * 1024,
            nice_len: 192,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 2,
            literal_position_bits: 2,
            position_bits: 0,
            dict_size: 32 * 1024 * 1024,
            nice_len: 192,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 32 * 1024 * 1024,
            nice_len: 192,
            match_finder: MatchFinder::HashChain4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 32 * 1024 * 1024,
            nice_len: 192,
            match_finder: MatchFinder::BinaryTree3,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        // Deeper-search variants: raise the LZMA hash-chain search depth above the preset 9
        // default (~1000). Slower per-encode but can find longer back-references in payloads
        // with global motifs (image scanlines). Cost-gated by the deep/research budgets so
        // balanced does not pay this slowdown.
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 2000,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 4000,
            delta_dist: 0,
        },
        // Large-dictionary variants for inputs > 64 MiB. LZMA2's preset 9 default
        // dict_size is 64 MiB; on 95 MiB+ payloads (e.g. the depth_anything tensor file)
        // this means matches across the file midpoint are unreachable from the matcher.
        // A 128 MiB dict captures cross-half repeats; a 256 MiB dict covers files up to
        // ~256 MiB end-to-end. Memory cost is ~10× dict_size during encode, so gate to
        // deep/research via deep_search_xz_budget (these entries are placed AFTER
        // depth-only entries so the existing budget governs them).
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 128 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 2,
            dict_size: 128 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 0,
        },
        // Delta pre-filter variants: useful for fixed-stride structured payloads such as
        // channel-interleaved image bytes (RGB=3, RGBA=4) or 16/32-bit aligned binary tables.
        // The delta filter computes `out[i] = data[i] - data[i - dist]` byte-wise, then LZMA2
        // compresses the residual. When the payload has that stride structure it can save
        // several percent vs vanilla LZMA2; on structureless data it loses a small constant
        // overhead and gets dropped by the codec winner-selection.
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 4,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 3,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 2,
        },
        XzTuningParams {
            literal_context_bits: 3,
            literal_position_bits: 0,
            position_bits: 0,
            dict_size: 64 * 1024 * 1024,
            nice_len: 273,
            match_finder: MatchFinder::BinaryTree4,
            mode: Mode::Normal,
            depth: 0,
            delta_dist: 1,
        },
    ]
}

fn tuned_xz_budget(profile: CompressionProfile) -> (usize, usize) {
    // (normal_budget, extreme_budget): controls how many entries of tuned_xz_param_matrix() are
    // probed at preset 9 and at preset 9 + extreme flag. The matrix is sorted from
    // most-frequently-winning (no delta filter) at the top to delta-filter variants at the bottom.
    // Keeping balanced lean trims the worst part of the recursive transform evaluation runtime
    // while still covering the high-value tunings; delta-filter variants are sampled by the
    // dedicated delta_xz_budget() so we can reach them without paying the full matrix cost.
    match profile {
        CompressionProfile::Fast => (3, 0),
        CompressionProfile::Balanced => (4, 3),
        CompressionProfile::Deep => (10, 6),
        CompressionProfile::Research => (18, 14),
    }
}

fn delta_xz_budget(_profile: CompressionProfile) -> (usize, usize) {
    // Disabled until the xz2 binding exposes the LZMA delta filter; without it, the delta-marked
    // tuning entries would duplicate plain LZMA2 work and waste eval budget.
    (0, 0)
}

fn deep_search_xz_budget(profile: CompressionProfile) -> (usize, usize) {
    // (normal_deep_budget, extreme_deep_budget): how many of the trailing deep-search tunings
    // (depth > preset default OR dict_size > 64 MiB) to probe. Off for fast/balanced because
    // each candidate roughly doubles per-encode time and 128 MiB-dict variants need ~1.5 GiB
    // memory; deep/research can afford a few for the chance of finding a longer back-reference
    // in image-like payloads with global motifs or cross-half matches in >64 MiB inputs.
    //
    // Matrix layout (between deep_first_idx and delta_first_idx):
    //   [0] depth=2000, [1] depth=4000, [2] dict=128MiB pb=0, [3] dict=128MiB pb=2
    // Deep budget covers items 0..2 (one each of depth+ and dict-128); research adds [3].
    match profile {
        CompressionProfile::Fast | CompressionProfile::Balanced => (0, 0),
        CompressionProfile::Deep => (3, 2),
        CompressionProfile::Research => (4, 3),
    }
}

fn build_tuned_xz_candidates(
    profile: CompressionProfile,
    allow_xz_extreme: bool,
) -> Vec<XzCodecCandidate> {
    let (normal_budget, extreme_budget) = tuned_xz_budget(profile);
    let (normal_delta_budget, extreme_delta_budget) = delta_xz_budget(profile);
    let (normal_deep_budget, extreme_deep_budget) = deep_search_xz_budget(profile);
    let tuning = tuned_xz_param_matrix();
    // Standard tunings end where the first deep-search (depth > 0) entry begins. Delta
    // entries follow the deep-search entries.
    let deep_first_idx = tuning
        .iter()
        .position(|entry| entry.depth > 0)
        .unwrap_or(tuning.len());
    let delta_first_idx = tuning
        .iter()
        .position(|entry| entry.delta_dist >= 1)
        .unwrap_or(tuning.len());

    let mut out = Vec::new();
    let mut rank = 0usize;

    out.push(XzCodecCandidate {
        rank,
        codec: PayloadCodec::Xz,
        preset: 9u32,
        kind: XzCandidateKind::Easy,
    });
    rank += 1;

    for params in tuning.iter().copied().take(deep_first_idx).take(normal_budget) {
        out.push(XzCodecCandidate {
            rank,
            codec: PayloadCodec::Xz,
            preset: 9u32,
            kind: XzCandidateKind::Tuned(params),
        });
        rank += 1;
    }

    for params in tuning
        .iter()
        .copied()
        .skip(deep_first_idx)
        .take(delta_first_idx.saturating_sub(deep_first_idx))
        .take(normal_deep_budget)
    {
        out.push(XzCodecCandidate {
            rank,
            codec: PayloadCodec::Xz,
            preset: 9u32,
            kind: XzCandidateKind::Tuned(params),
        });
        rank += 1;
    }

    for params in tuning.iter().copied().skip(delta_first_idx).take(normal_delta_budget) {
        out.push(XzCodecCandidate {
            rank,
            codec: PayloadCodec::Xz,
            preset: 9u32,
            kind: XzCandidateKind::Tuned(params),
        });
        rank += 1;
    }

    if allow_xz_extreme {
        let extreme = (1u32 << 31) | 9u32;
        out.push(XzCodecCandidate {
            rank,
            codec: PayloadCodec::XzExtreme,
            preset: extreme,
            kind: XzCandidateKind::Easy,
        });
        rank += 1;

        for params in tuning.iter().copied().take(deep_first_idx).take(extreme_budget) {
            out.push(XzCodecCandidate {
                rank,
                codec: PayloadCodec::XzExtreme,
                preset: extreme,
                kind: XzCandidateKind::Tuned(params),
            });
            rank += 1;
        }

        for params in tuning
            .iter()
            .copied()
            .skip(deep_first_idx)
            .take(delta_first_idx.saturating_sub(deep_first_idx))
            .take(extreme_deep_budget)
        {
            out.push(XzCodecCandidate {
                rank,
                codec: PayloadCodec::XzExtreme,
                preset: extreme,
                kind: XzCandidateKind::Tuned(params),
            });
            rank += 1;
        }

        for params in tuning
            .iter()
            .copied()
            .skip(delta_first_idx)
            .take(extreme_delta_budget)
        {
            out.push(XzCodecCandidate {
                rank,
                codec: PayloadCodec::XzExtreme,
                preset: extreme,
                kind: XzCandidateKind::Tuned(params),
            });
            rank += 1;
        }
    }

    out
}

fn choose_best_tuned_xz_candidate(
    data: &[u8],
    profile: CompressionProfile,
    allow_xz_extreme: bool,
) -> ZbitResult<Option<(PayloadCodec, Vec<u8>)>> {
    let candidates = build_tuned_xz_candidates(profile, allow_xz_extreme);
    if candidates.is_empty() {
        return Ok(None);
    }

    let best = candidates
        .into_par_iter()
        .map(|candidate| {
            let payload = match candidate.kind {
                XzCandidateKind::Easy => xz_encode_easy_preset(data, candidate.preset)?,
                XzCandidateKind::Tuned(params) => {
                    xz_encode_with_tuning(data, candidate.preset, params)?
                }
            };
            Ok::<_, ZbitError>((candidate.rank, candidate.codec, payload))
        })
        .collect::<ZbitResult<Vec<_>>>()?
        .into_iter()
        .min_by_key(|(rank, _, payload)| (payload.len(), *rank))
        .map(|(_, codec, payload)| (codec, payload));

    Ok(best)
}

fn xz_decode_all(data: &[u8]) -> ZbitResult<Vec<u8>> {
    let mut decoder = XzDecoder::new(Cursor::new(data));
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| ZbitError::Parse(format!("xz decode failed: {e}")))?;
    Ok(out)
}

fn zstd_encode_with_level(data: &[u8], level: i32) -> ZbitResult<Vec<u8>> {
    zstd_stream::encode_all(data, level)
        .map_err(|e| ZbitError::Io(format!("zstd encode failed: {e}")))
}

fn zstd_decode_exact(data: &[u8], expected_len: usize) -> ZbitResult<Vec<u8>> {
    let out = zstd_stream::decode_all(data)
        .map_err(|e| ZbitError::Parse(format!("zstd decode failed: {e}")))?;
    if out.len() != expected_len {
        return Err(ZbitError::Parse(format!(
            "zstd output length mismatch: expected {expected_len} got {}",
            out.len()
        )));
    }
    Ok(out)
}

fn decode_with_codec(data: &[u8], codec: PayloadCodec, expected_len: usize) -> ZbitResult<Vec<u8>> {
    match codec {
        PayloadCodec::Raw => {
            if data.len() != expected_len {
                return Err(ZbitError::Parse(format!(
                    "raw codec length mismatch: expected {expected_len} got {}",
                    data.len()
                )));
            }
            Ok(data.to_vec())
        }
        PayloadCodec::Xz | PayloadCodec::XzExtreme => {
            let out = xz_decode_all(data)?;
            if out.len() != expected_len {
                return Err(ZbitError::Parse(format!(
                    "xz output length mismatch: expected {expected_len} got {}",
                    out.len()
                )));
            }
            Ok(out)
        }
        PayloadCodec::Zstd => zstd_decode_exact(data, expected_len),
    }
}

fn choose_best_codec(
    data: &[u8],
    allow_raw: bool,
    allow_xz_extreme: bool,
    profile: CompressionProfile,
) -> ZbitResult<(PayloadCodec, Vec<u8>)> {
    // Fast profile uses only the default XZ-9 path; the other profiles probe two extra
    // position_bits variants because they occasionally save several KB on aligned payloads.
    let mut codec_jobs = vec![(0usize, PayloadCodec::Xz, 9u32, 0u32)];
    if !matches!(profile, CompressionProfile::Fast) {
        codec_jobs.push((1usize, PayloadCodec::Xz, 9u32, 3u32));
        codec_jobs.push((2usize, PayloadCodec::Xz, 9u32, 4u32));
    }
    if allow_xz_extreme && profile.enable_xz_extreme_refinement() {
        let extreme = (1u32 << 31) | 9;
        codec_jobs.push((3usize, PayloadCodec::XzExtreme, extreme, 0u32));
        if matches!(profile, CompressionProfile::Deep | CompressionProfile::Research) {
            codec_jobs.push((4usize, PayloadCodec::XzExtreme, extreme, 3u32));
            codec_jobs.push((5usize, PayloadCodec::XzExtreme, extreme, 4u32));
        }
    }

    let mut candidates = codec_jobs
        .into_par_iter()
        .map(|(rank, codec, preset, profile_pb)| {
            let payload = if profile_pb == 0 {
                xz_encode_easy_preset(data, preset)?
            } else {
                xz_encode_with_profile(data, preset, profile_pb)?
            };
            Ok::<_, ZbitError>((rank, codec, payload))
        })
        .collect::<ZbitResult<Vec<_>>>()?;

    let zstd_level = match profile {
        CompressionProfile::Fast => 10,
        CompressionProfile::Balanced => 17,
        CompressionProfile::Deep | CompressionProfile::Research => 19,
    };
    let zstd = zstd_encode_with_level(data, zstd_level)?;
    candidates.push((10usize, PayloadCodec::Zstd, zstd));

    if allow_raw {
        candidates.push((11usize, PayloadCodec::Raw, data.to_vec()));
    }

    let (_, codec, payload) = candidates
        .into_iter()
        .min_by_key(|(rank, _, payload)| (payload.len(), *rank))
        .ok_or_else(|| ZbitError::Internal("codec candidate set is empty".to_string()))?;
    Ok((codec, payload))
}

fn choose_best_tuned_xz_candidate_cached(
    data: &[u8],
    allow_xz_extreme: bool,
    context: &mut CompressionContext,
) -> ZbitResult<Option<(PayloadCodec, Vec<u8>)>> {
    // The tuned-XZ matrix is the dominant per-encode cost on multi-MB payloads (~30 min on
    // a 95 MB PyTorch model). Both the top-level raw-xz candidate and the adaptive-
    // transformed-xz winner refinement frequently invoke it on the same byte stream — in
    // particular when the adaptive plan winner is Identity the two calls are literally
    // identical. Caching by `(input_hash, allow_xz_extreme, profile)` collapses that
    // duplication to a single matrix walk.
    let key = TunedXzCacheKey {
        input_hash: payload_hash(data),
        allow_xz_extreme,
        profile: context.profile,
    };
    if let Some(cached) = context.cache.tuned_xz_outputs.get(&key) {
        context.cache_stats.tuned_xz_hits =
            context.cache_stats.tuned_xz_hits.saturating_add(1);
        return Ok(cached.clone());
    }
    context.cache_stats.tuned_xz_misses =
        context.cache_stats.tuned_xz_misses.saturating_add(1);
    let result = choose_best_tuned_xz_candidate(data, context.profile, allow_xz_extreme)?;
    context.cache.tuned_xz_outputs.insert(key, result.clone());
    Ok(result)
}

fn choose_best_codec_cached(
    data: &[u8],
    allow_raw: bool,
    allow_xz_extreme: bool,
    context: &mut CompressionContext,
) -> ZbitResult<(PayloadCodec, Vec<u8>)> {
    let key = (
        payload_hash(data),
        CodecProfileKey {
            allow_raw,
            allow_xz_extreme,
            profile: context.profile,
        },
    );
    if let Some((codec, payload)) = context.cache.codec_outputs.get(&key) {
        context.cache_stats.codec_hits = context.cache_stats.codec_hits.saturating_add(1);
        return Ok((*codec, payload.clone()));
    }

    context.cache_stats.codec_misses = context.cache_stats.codec_misses.saturating_add(1);
    let (codec, payload) = choose_best_codec(data, allow_raw, allow_xz_extreme, context.profile)?;
    context
        .cache
        .codec_outputs
        .insert(key, (codec, payload.clone()));
    Ok((codec, payload))
}
