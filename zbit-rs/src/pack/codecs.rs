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

// v5 dictionary layout for MonotonicDelta (variable length, typically ~10-18 B):
//   meta u8           bits 0..2 = width (1..=8 maps to 0..=7 via `width - 1`)
//                     bits 3..5 = mode  (5 modes fit in 3 bits)
//                     bits 6..7 = codec (4 values fit in 2 bits)
//   shift u8          trailing_zero_shift (0..63 — could be 6 bits but byte-aligned
//                     for round-trip clarity; the upper 2 bits are reserved and MUST be 0)
//   varint count
//   varint first_value
//   varint transformed_plain_len
//   optional transform plan:
//     kind_index u8   6 low bits used, via the compact transform-kind table
//     varint period
//     varint head
// Replaces the prior fixed 28-byte layout. For typical primary.3b-style payloads
// (count ~1 M, gaps small) the new dictionary is ~12 bytes — savings on this corpus
// matter less in ratio because the file is already 0.174, but the principle is the
// same as the other dictionaries: spend only the bits each enumeration value needs.
fn monotonic_delta_dictionary_size(stream: &MonotonicDeltaStream) -> usize {
    let transform_plan_size = stream.transform_plan.map_or(0, |plan| {
        1 + varint_u64_len(plan.period as u64) + varint_u64_len(plan.head as u64)
    });
    1 /* meta */ + 1 /* shift */
        + varint_u64_len(stream.count)
        + varint_u64_len(stream.first_value)
        + varint_u64_len(stream.transformed_plain_len as u64)
        + transform_plan_size
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

fn encode_atlas_indices(indices: &[usize], unique_count: usize) -> Vec<u8> {
    let width = bits_for_index(unique_count);
    if width == 0 {
        return Vec::new();
    }
    let mut writer = BitWriter::with_capacity((indices.len() * width as usize + 7) / 8);
    for &idx in indices {
        writer.write_bits(idx as u64, width);
    }
    writer.into_bytes()
}

fn exact_block_atlas_dictionary_size(stream: &ExactBlockAtlasStream) -> usize {
    1 + varint_u64_len(stream.chunk_size as u64)
        + varint_u64_len(stream.block_count as u64)
        + varint_u64_len(stream.tail_len as u64)
        + varint_u64_len(stream.unique_count as u64)
        + varint_u64_len(stream.unique_payload.len() as u64)
        + varint_u64_len(stream.index_plain_len as u64)
        + varint_u64_len(stream.index_payload.len() as u64)
        + varint_u64_len(stream.tail_payload.len() as u64)
}

fn write_exact_block_atlas_dictionary(out: &mut Vec<u8>, stream: &ExactBlockAtlasStream) {
    let codecs = (stream.unique_codec.as_u8() & 0x03)
        | ((stream.index_codec.as_u8() & 0x03) << 2)
        | ((stream.tail_codec.as_u8() & 0x03) << 4);
    out.push(codecs);
    push_varint_u64(out, stream.chunk_size as u64);
    push_varint_u64(out, stream.block_count as u64);
    push_varint_u64(out, stream.tail_len as u64);
    push_varint_u64(out, stream.unique_count as u64);
    push_varint_u64(out, stream.unique_payload.len() as u64);
    push_varint_u64(out, stream.index_plain_len as u64);
    push_varint_u64(out, stream.index_payload.len() as u64);
    push_varint_u64(out, stream.tail_payload.len() as u64);
}

fn build_exact_block_atlas_for_chunk_size(
    input: &[u8],
    chunk_size: usize,
    context: &mut CompressionContext,
    best_prelude_candidate_bytes: Option<usize>,
) -> ZbitResult<Option<(ExactBlockAtlasStream, usize)>> {
    if input.len() < chunk_size * 4 {
        return Ok(None);
    }

    let block_count = input.len() / chunk_size;
    let tail_len = input.len() - block_count * chunk_size;
    if block_count < 4 {
        return Ok(None);
    }

    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut unique_ranges: Vec<(usize, usize)> = Vec::new();
    let mut indices = Vec::with_capacity(block_count);
    for block_idx in 0..block_count {
        let start = block_idx * chunk_size;
        let end = start + chunk_size;
        let slice = &input[start..end];
        let hash = xxh3_64(slice);
        let existing = buckets
            .get(&hash)
            .and_then(|ids| {
                ids.iter().copied().find(|&id| {
                    let (unique_start, unique_end) = unique_ranges[id];
                    input[unique_start..unique_end] == slice[..]
                })
            });
        let id = match existing {
            Some(id) => id,
            None => {
                let id = unique_ranges.len();
                unique_ranges.push((start, end));
                buckets.entry(hash).or_default().push(id);
                id
            }
        };
        indices.push(id);
    }

    let unique_count = unique_ranges.len();
    if unique_count >= block_count {
        return Ok(None);
    }
    let repeated_blocks = block_count - unique_count;
    if repeated_blocks < 2 {
        return Ok(None);
    }

    let index_plain = encode_atlas_indices(&indices, unique_count);
    let unique_plain_len = unique_count * chunk_size;
    let raw_model_size = unique_plain_len + index_plain.len() + tail_len;
    if let Some(best_prelude_total) = best_prelude_candidate_bytes {
        let estimated_total = ZBPK_HEADER_BYTES
            + 16
            + varint_u64_len(chunk_size as u64)
            + varint_u64_len(block_count as u64)
            + varint_u64_len(unique_count as u64)
            + raw_model_size;
        if estimated_total + 1024 >= best_prelude_total {
            return Ok(None);
        }
    }

    let mut unique_plain = Vec::with_capacity(unique_plain_len);
    for (start, end) in unique_ranges {
        unique_plain.extend_from_slice(&input[start..end]);
    }
    let tail_plain = if tail_len == 0 {
        Vec::new()
    } else {
        input[input.len() - tail_len..].to_vec()
    };

    let (unique_codec, unique_payload) =
        choose_best_codec_cached(&unique_plain, false, true, context)?;
    let (index_codec, index_payload) = if index_plain.is_empty() {
        (PayloadCodec::Raw, Vec::new())
    } else {
        choose_best_codec_cached(&index_plain, true, false, context)?
    };
    let (tail_codec, tail_payload) = if tail_plain.is_empty() {
        (PayloadCodec::Raw, Vec::new())
    } else {
        choose_best_codec_cached(&tail_plain, true, true, context)?
    };

    let stream = ExactBlockAtlasStream {
        chunk_size,
        block_count,
        tail_len,
        unique_count,
        index_plain_len: index_plain.len(),
        unique_codec,
        index_codec,
        tail_codec,
        unique_payload,
        index_payload,
        tail_payload,
    };
    let total_size = ZBPK_HEADER_BYTES
        + exact_block_atlas_dictionary_size(&stream)
        + stream.unique_payload.len()
        + stream.index_payload.len()
        + stream.tail_payload.len();
    Ok(Some((stream, total_size)))
}

fn build_exact_block_atlas_stream(
    input: &[u8],
    context: &mut CompressionContext,
    best_prelude_candidate_bytes: Option<usize>,
) -> ZbitResult<Option<ExactBlockAtlasStream>> {
    let mut best: Option<(ExactBlockAtlasStream, usize)> = None;
    for chunk_size in [1024usize, 4096, 16 * 1024, 64 * 1024] {
        if let Some(candidate) = build_exact_block_atlas_for_chunk_size(
            input,
            chunk_size,
            context,
            best_prelude_candidate_bytes,
        )? {
            match &best {
                Some((_, best_size)) if *best_size <= candidate.1 => {}
                _ => best = Some(candidate),
            }
        }
    }

    let Some((stream, total_size)) = best else {
        context.push_skipped(
            "exact-block-atlas skipped: no repeated fixed-size block layout passed the cost gate",
        );
        return Ok(None);
    };
    if let Some(best_prelude_total) = best_prelude_candidate_bytes {
        if total_size >= best_prelude_total {
            context.push_skipped(format!(
                "exact-block-atlas rejected: total {total_size} does not beat best prelude total {best_prelude_total}"
            ));
            return Ok(None);
        }
    }
    Ok(Some(stream))
}

fn decode_exact_block_atlas_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;
    let codecs = read_u8(dict_bytes, &mut cursor)?;
    if codecs & 0xC0 != 0 {
        return Err(ZbitError::Parse(
            "exact-block-atlas codec byte reserves top 2 bits".to_string(),
        ));
    }
    let unique_codec = PayloadCodec::from_u8(codecs & 0x03).ok_or_else(|| {
        ZbitError::Parse("exact-block-atlas has invalid unique codec".to_string())
    })?;
    let index_codec = PayloadCodec::from_u8((codecs >> 2) & 0x03).ok_or_else(|| {
        ZbitError::Parse("exact-block-atlas has invalid index codec".to_string())
    })?;
    let tail_codec = PayloadCodec::from_u8((codecs >> 4) & 0x03).ok_or_else(|| {
        ZbitError::Parse("exact-block-atlas has invalid tail codec".to_string())
    })?;
    let chunk_size = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let block_count = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let tail_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let unique_count = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let unique_payload_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let index_plain_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let index_payload_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let tail_payload_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    if cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in exact-block-atlas dictionary".to_string(),
        ));
    }
    if chunk_size == 0 || block_count == 0 || unique_count == 0 || unique_count > block_count {
        return Err(ZbitError::Parse(
            "exact-block-atlas has invalid chunk/block counters".to_string(),
        ));
    }
    let full_len = block_count.checked_mul(chunk_size).ok_or_else(|| {
        ZbitError::Parse("exact-block-atlas full-block length overflow".to_string())
    })?;
    if full_len.checked_add(tail_len) != Some(original_size) {
        return Err(ZbitError::Parse(format!(
            "exact-block-atlas output length mismatch: expected {original_size}"
        )));
    }
    let unique_plain_len = unique_count.checked_mul(chunk_size).ok_or_else(|| {
        ZbitError::Parse("exact-block-atlas unique length overflow".to_string())
    })?;
    let expected_index_plain_len =
        (block_count * bits_for_index(unique_count) as usize + 7) / 8;
    if index_plain_len != expected_index_plain_len {
        return Err(ZbitError::Parse(
            "exact-block-atlas index length does not match block/index width".to_string(),
        ));
    }

    let unique_end = unique_payload_len;
    let index_end = unique_end.checked_add(index_payload_len).ok_or_else(|| {
        ZbitError::Parse("exact-block-atlas payload length overflow".to_string())
    })?;
    let tail_end = index_end.checked_add(tail_payload_len).ok_or_else(|| {
        ZbitError::Parse("exact-block-atlas payload length overflow".to_string())
    })?;
    if tail_end != payload.len() {
        return Err(ZbitError::Parse(
            "exact-block-atlas payload section lengths do not match payload".to_string(),
        ));
    }
    let unique_plain = decode_with_codec(&payload[..unique_end], unique_codec, unique_plain_len)?;
    let index_plain =
        decode_with_codec(&payload[unique_end..index_end], index_codec, index_plain_len)?;
    let tail_plain = decode_with_codec(&payload[index_end..tail_end], tail_codec, tail_len)?;

    let index_width = bits_for_index(unique_count);
    let mut reader = BitReader::new(&index_plain);
    let mut out = Vec::with_capacity(original_size);
    for _ in 0..block_count {
        let id = if index_width == 0 {
            0usize
        } else {
            reader.read_bits(index_width)? as usize
        };
        if id >= unique_count {
            return Err(ZbitError::Parse(
                "exact-block-atlas index references unknown chunk".to_string(),
            ));
        }
        let start = id * chunk_size;
        let end = start + chunk_size;
        out.extend_from_slice(&unique_plain[start..end]);
    }
    out.extend_from_slice(&tail_plain);
    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "exact-block-atlas output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }
    Ok(out)
}

fn read_le_u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_le_u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn parse_stored_zip_tensor_ranges(input: &[u8]) -> Vec<(usize, usize)> {
    const LOCAL_FILE_HEADER: u32 = 0x0403_4B50;
    const CENTRAL_DIRECTORY_HEADER: u32 = 0x0201_4B50;
    const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4B50;
    const DATA_DESCRIPTOR_FLAG: u16 = 1 << 3;
    const STORED_METHOD: u16 = 0;

    let mut cursor = 0usize;
    let mut ranges = Vec::new();
    while cursor + 30 <= input.len() {
        let Some(signature) = read_le_u32_at(input, cursor) else {
            break;
        };
        if signature == CENTRAL_DIRECTORY_HEADER || signature == END_OF_CENTRAL_DIRECTORY {
            break;
        }
        if signature != LOCAL_FILE_HEADER {
            return Vec::new();
        }
        let flags = read_le_u16_at(input, cursor + 6).unwrap_or(0);
        let method = read_le_u16_at(input, cursor + 8).unwrap_or(u16::MAX);
        let compressed_size = read_le_u32_at(input, cursor + 18).unwrap_or(u32::MAX) as usize;
        let uncompressed_size = read_le_u32_at(input, cursor + 22).unwrap_or(u32::MAX) as usize;
        let name_len = read_le_u16_at(input, cursor + 26).unwrap_or(0) as usize;
        let extra_len = read_le_u16_at(input, cursor + 28).unwrap_or(0) as usize;
        let name_start = cursor + 30;
        let data_start = match name_start
            .checked_add(name_len)
            .and_then(|value| value.checked_add(extra_len))
        {
            Some(value) => value,
            None => return Vec::new(),
        };
        let data_end = match data_start.checked_add(compressed_size) {
            Some(value) if value <= input.len() => value,
            _ => return Vec::new(),
        };
        let name = std::str::from_utf8(input.get(name_start..name_start + name_len).unwrap_or(&[]))
            .unwrap_or("");
        let is_tensor_storage = name.contains("/data/") || name.starts_with("data/");
        if flags & DATA_DESCRIPTOR_FLAG == 0
            && method == STORED_METHOD
            && compressed_size == uncompressed_size
            && compressed_size >= 4096
            && is_tensor_storage
        {
            ranges.push((data_start, compressed_size));
        }
        cursor = data_end;
    }
    ranges
}

fn zip_tensor_split_dictionary_size(stream: &ZipTensorSplitStream) -> usize {
    let ranges = stream.data_lengths.len();
    1 + varint_u64_len(ranges as u64)
        + varint_u64_len(stream.metadata_plain_len as u64)
        + varint_u64_len(stream.tensor_plain_len as u64)
        + varint_u64_len(stream.metadata_payload.len() as u64)
        + varint_u64_len(stream.tensor_payload.len() as u64)
        + stream
            .gap_lengths
            .iter()
            .map(|&len| varint_u64_len(len as u64))
            .sum::<usize>()
        + stream
            .data_lengths
            .iter()
            .map(|&len| varint_u64_len(len as u64))
            .sum::<usize>()
}

fn write_zip_tensor_split_dictionary(out: &mut Vec<u8>, stream: &ZipTensorSplitStream) {
    let codecs = (stream.metadata_codec.as_u8() & 0x03) | ((stream.tensor_codec.as_u8() & 0x03) << 2);
    out.push(codecs);
    push_varint_u64(out, stream.data_lengths.len() as u64);
    push_varint_u64(out, stream.metadata_plain_len as u64);
    push_varint_u64(out, stream.tensor_plain_len as u64);
    push_varint_u64(out, stream.metadata_payload.len() as u64);
    push_varint_u64(out, stream.tensor_payload.len() as u64);
    for idx in 0..stream.data_lengths.len() {
        push_varint_u64(out, stream.gap_lengths[idx] as u64);
        push_varint_u64(out, stream.data_lengths[idx] as u64);
    }
    push_varint_u64(out, *stream.gap_lengths.last().unwrap_or(&0) as u64);
}

fn build_zip_tensor_split_stream(
    input: &[u8],
    context: &mut CompressionContext,
    best_prelude_candidate_bytes: Option<usize>,
) -> ZbitResult<Option<ZipTensorSplitStream>> {
    if input.len() < 128 * 1024 {
        context.push_skipped("zip-tensor-split skipped: input below container-model threshold");
        return Ok(None);
    }
    let ranges = parse_stored_zip_tensor_ranges(input);
    if ranges.len() < 2 {
        context.push_skipped(
            "zip-tensor-split skipped: no multi-entry stored tensor ZIP layout detected",
        );
        return Ok(None);
    }

    let tensor_plain_len = ranges.iter().map(|(_, len)| *len).sum::<usize>();
    if tensor_plain_len < 128 * 1024 {
        context.push_skipped("zip-tensor-split skipped: tensor payload below threshold");
        return Ok(None);
    }

    let mut metadata = Vec::with_capacity(input.len() - tensor_plain_len);
    let mut tensor = Vec::with_capacity(tensor_plain_len);
    let mut gap_lengths = Vec::with_capacity(ranges.len() + 1);
    let mut data_lengths = Vec::with_capacity(ranges.len());
    let mut cursor = 0usize;
    for (start, len) in ranges {
        let end = start + len;
        if start < cursor || end > input.len() {
            return Ok(None);
        }
        gap_lengths.push(start - cursor);
        metadata.extend_from_slice(&input[cursor..start]);
        tensor.extend_from_slice(&input[start..end]);
        data_lengths.push(len);
        cursor = end;
    }
    gap_lengths.push(input.len() - cursor);
    metadata.extend_from_slice(&input[cursor..]);

    let raw_model_size = metadata.len() + tensor.len();
    if let Some(best_prelude_total) = best_prelude_candidate_bytes {
        if ZBPK_HEADER_BYTES + raw_model_size + 32 >= best_prelude_total {
            context.push_skipped(
                "zip-tensor-split skipped: raw metadata/tensor split cannot beat prelude bound",
            );
            return Ok(None);
        }
    }

    let (metadata_codec, metadata_payload) =
        choose_best_codec_cached(&metadata, true, true, context)?;
    let (tensor_codec, tensor_payload) = choose_best_codec_cached(&tensor, false, true, context)?;
    let stream = ZipTensorSplitStream {
        gap_lengths,
        data_lengths,
        metadata_plain_len: metadata.len(),
        tensor_plain_len: tensor.len(),
        metadata_codec,
        tensor_codec,
        metadata_payload,
        tensor_payload,
    };
    let total_size = ZBPK_HEADER_BYTES
        + zip_tensor_split_dictionary_size(&stream)
        + stream.metadata_payload.len()
        + stream.tensor_payload.len();
    if let Some(best_prelude_total) = best_prelude_candidate_bytes {
        if total_size >= best_prelude_total {
            context.push_skipped(format!(
                "zip-tensor-split rejected: total {total_size} does not beat best prelude total {best_prelude_total}"
            ));
            return Ok(None);
        }
    }
    Ok(Some(stream))
}

fn decode_zip_tensor_split_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;
    let codecs = read_u8(dict_bytes, &mut cursor)?;
    if codecs & 0xF0 != 0 {
        return Err(ZbitError::Parse(
            "zip-tensor-split codec byte reserves top 4 bits".to_string(),
        ));
    }
    let metadata_codec = PayloadCodec::from_u8(codecs & 0x03).ok_or_else(|| {
        ZbitError::Parse("zip-tensor-split has invalid metadata codec".to_string())
    })?;
    let tensor_codec = PayloadCodec::from_u8((codecs >> 2) & 0x03).ok_or_else(|| {
        ZbitError::Parse("zip-tensor-split has invalid tensor codec".to_string())
    })?;
    let range_count = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let metadata_plain_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let tensor_plain_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let metadata_payload_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let tensor_payload_len = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    if range_count == 0 {
        return Err(ZbitError::Parse(
            "zip-tensor-split requires at least one tensor range".to_string(),
        ));
    }
    let mut gap_lengths = Vec::with_capacity(range_count + 1);
    let mut data_lengths = Vec::with_capacity(range_count);
    for _ in 0..range_count {
        gap_lengths.push(read_varint_u64(dict_bytes, &mut cursor)? as usize);
        data_lengths.push(read_varint_u64(dict_bytes, &mut cursor)? as usize);
    }
    gap_lengths.push(read_varint_u64(dict_bytes, &mut cursor)? as usize);
    if cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in zip-tensor-split dictionary".to_string(),
        ));
    }
    if metadata_payload_len.checked_add(tensor_payload_len) != Some(payload.len()) {
        return Err(ZbitError::Parse(
            "zip-tensor-split payload lengths do not match payload".to_string(),
        ));
    }

    let metadata = decode_with_codec(
        &payload[..metadata_payload_len],
        metadata_codec,
        metadata_plain_len,
    )?;
    let tensor = decode_with_codec(
        &payload[metadata_payload_len..],
        tensor_codec,
        tensor_plain_len,
    )?;
    if gap_lengths.iter().sum::<usize>() != metadata_plain_len
        || data_lengths.iter().sum::<usize>() != tensor_plain_len
        || metadata_plain_len.checked_add(tensor_plain_len) != Some(original_size)
    {
        return Err(ZbitError::Parse(
            "zip-tensor-split dictionary lengths do not match original size".to_string(),
        ));
    }

    let mut out = Vec::with_capacity(original_size);
    let mut metadata_cursor = 0usize;
    let mut tensor_cursor = 0usize;
    for idx in 0..range_count {
        let gap_len = gap_lengths[idx];
        out.extend_from_slice(&metadata[metadata_cursor..metadata_cursor + gap_len]);
        metadata_cursor += gap_len;
        let data_len = data_lengths[idx];
        out.extend_from_slice(&tensor[tensor_cursor..tensor_cursor + data_len]);
        tensor_cursor += data_len;
    }
    let final_gap = *gap_lengths.last().unwrap_or(&0);
    out.extend_from_slice(&metadata[metadata_cursor..metadata_cursor + final_gap]);
    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "zip-tensor-split output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }
    Ok(out)
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
    if let Some(plan) = stream.transform_plan {
        let kind_u8 = plan.kind.as_u8();
        let kind_index = kind_to_compact_index(kind_u8).unwrap_or(kind_u8 & 0x3F);
        out.push(kind_index & 0x3F);
        push_varint_u64(out, plan.period as u64);
        push_varint_u64(out, plan.head as u64);
    }
}

fn build_monotonic_delta_stream(
    input: &[u8],
    context: &mut CompressionContext,
    best_prelude_candidate_bytes: Option<usize>,
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
                transform_plan: None,
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

    let Some((mut stream, base_size)) = best else {
        return Ok(None);
    };

    let Some(best_prelude_total) = best_prelude_candidate_bytes else {
        context.push_skipped(
            "monotonic gap transform skipped: no raw/prelude candidate total available",
        );
        return Ok(Some(stream));
    };
    if ZBPK_HEADER_BYTES + base_size >= best_prelude_total {
        context.push_skipped(format!(
            "monotonic gap transform skipped: base monotonic total {} does not beat best prelude total {}",
            ZBPK_HEADER_BYTES + base_size,
            best_prelude_total
        ));
        return Ok(Some(stream));
    }

    let plan_budget = context.profile.monotonic_gap_transform_plan_budget();
    if plan_budget == 0 {
        context.push_skipped(format!(
            "monotonic gap transform skipped by '{}' profile",
            context.profile.name()
        ));
        return Ok(Some(stream));
    }

    let decoded_gap_stream =
        decode_with_codec(&stream.payload, stream.codec, stream.transformed_plain_len)?;
    let candidate =
        choose_adaptive_transform_plan(&decoded_gap_stream, context.profile, plan_budget)?;
    let identity_plan = CircuitTransformPlan {
        kind: CircuitTransformKind::Identity,
        period: 0,
        head: 0,
    };
    let candidate_transform_plan = if candidate.plan == identity_plan {
        None
    } else {
        Some(candidate.plan)
    };
    let candidate_stream = MonotonicDeltaStream {
        width: stream.width,
        count: stream.count,
        first_value: stream.first_value,
        transformed_plain_len: stream.transformed_plain_len,
        mode: stream.mode,
        trailing_zero_shift: stream.trailing_zero_shift,
        transform_plan: candidate_transform_plan,
        codec: candidate.codec,
        payload: candidate.payload,
    };
    let candidate_size =
        monotonic_delta_dictionary_size(&candidate_stream) + candidate_stream.payload.len();
    if candidate_size < base_size {
        stream = candidate_stream;
    } else {
        context.push_skipped(format!(
            "monotonic gap transform rejected: transformed total {} does not beat base {}",
            ZBPK_HEADER_BYTES + candidate_size,
            ZBPK_HEADER_BYTES + base_size
        ));
    }

    Ok(Some(stream))
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
    let transform_plan = if dict_cursor == dict_bytes.len() {
        None
    } else {
        let kind_index = read_u8(dict_bytes, &mut dict_cursor)?;
        if kind_index & 0xC0 != 0 {
            return Err(ZbitError::Parse(
                "monotonic-delta transform kind reserves the top 2 bits".to_string(),
            ));
        }
        let kind_u8 = compact_index_to_kind(kind_index).ok_or_else(|| {
            ZbitError::Parse(format!(
                "monotonic-delta transform has invalid kind index {kind_index}"
            ))
        })?;
        let kind = CircuitTransformKind::from_u8(kind_u8).ok_or_else(|| {
            ZbitError::Parse("monotonic-delta transform has invalid kind".to_string())
        })?;
        let period = read_varint_u64(dict_bytes, &mut dict_cursor)? as u32;
        let head = read_varint_u64(dict_bytes, &mut dict_cursor)? as u32;
        if dict_cursor != dict_bytes.len() {
            return Err(ZbitError::Parse(
                "trailing bytes in monotonic-delta dictionary".to_string(),
            ));
        }
        Some(CircuitTransformPlan { kind, period, head })
    };
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

    let mut transformed = decode_with_codec(payload, codec, transformed_plain_len)?;
    if let Some(plan) = transform_plan {
        transformed = invert_transform_plan(&transformed, transformed_plain_len, &plan).ok_or_else(
            || {
                ZbitError::Parse(
                    "monotonic-delta inverse gap transform produced invalid output".to_string(),
                )
            },
        )?;
        if transformed.len() != transformed_plain_len {
            return Err(ZbitError::Parse(format!(
                "monotonic-delta inverse gap transform length mismatch: expected {transformed_plain_len} got {}",
                transformed.len()
            )));
        }
    }
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

const PRIME_SEQUENCE_MAX_SIEVE_VALUE: u64 = 64 * 1024 * 1024;

fn prime_sequence_dictionary_size(stream: &PrimeSequenceStream) -> usize {
    1 + varint_u64_len(stream.count)
        + varint_u64_len(stream.first_value)
        + varint_u64_len(stream.last_value)
}

fn write_prime_sequence_dictionary(out: &mut Vec<u8>, stream: &PrimeSequenceStream) {
    out.push(stream.width);
    push_varint_u64(out, stream.count);
    push_varint_u64(out, stream.first_value);
    push_varint_u64(out, stream.last_value);
}

fn sieve_prime_flags(limit: usize) -> Vec<bool> {
    let mut prime = vec![true; limit.saturating_add(1)];
    if !prime.is_empty() {
        prime[0] = false;
    }
    if prime.len() > 1 {
        prime[1] = false;
    }

    let mut p = 2usize;
    while p.saturating_mul(p) <= limit {
        if prime[p] {
            let mut multiple = p * p;
            while multiple <= limit {
                prime[multiple] = false;
                multiple += p;
            }
        }
        p += 1;
    }
    prime
}

fn build_prime_sequence_stream(input: &[u8]) -> ZbitResult<Option<PrimeSequenceStream>> {
    let candidate_widths = [3usize, 4, 2, 5, 6, 7, 8, 1];
    for width in candidate_widths {
        if input.is_empty() || input.len() < width || (input.len() % width) != 0 {
            continue;
        }
        let count = input.len() / width;
        let first = read_le_u64_width(&input[..width], width).ok_or_else(|| {
            ZbitError::Internal("prime-sequence first value width is invalid".to_string())
        })?;
        let last_start = input.len() - width;
        let last = read_le_u64_width(&input[last_start..], width).ok_or_else(|| {
            ZbitError::Internal("prime-sequence last value width is invalid".to_string())
        })?;
        if first < 2 || last < first || last > PRIME_SEQUENCE_MAX_SIEVE_VALUE {
            continue;
        }

        let prime = sieve_prime_flags(last as usize);
        if !prime.get(first as usize).copied().unwrap_or(false)
            || !prime.get(last as usize).copied().unwrap_or(false)
        {
            continue;
        }

        let mut cursor = 0usize;
        let mut matched = 0usize;
        let mut valid = true;
        for value in first as usize..=last as usize {
            if !prime[value] {
                continue;
            }
            if cursor + width > input.len() {
                valid = false;
                break;
            }
            let encoded = read_le_u64_width(&input[cursor..cursor + width], width).ok_or_else(
                || ZbitError::Internal("prime-sequence value width is invalid".to_string()),
            )?;
            if encoded != value as u64 {
                valid = false;
                break;
            }
            cursor += width;
            matched += 1;
        }

        if valid && cursor == input.len() && matched == count {
            return Ok(Some(PrimeSequenceStream {
                width: width as u8,
                count: count as u64,
                first_value: first,
                last_value: last,
            }));
        }
    }

    Ok(None)
}

fn decode_prime_sequence_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    if !payload.is_empty() {
        return Err(ZbitError::Parse(
            "prime-sequence payload must be empty".to_string(),
        ));
    }

    let mut cursor = 0usize;
    let width = read_u8(dict_bytes, &mut cursor)? as usize;
    let count = read_varint_u64(dict_bytes, &mut cursor)? as usize;
    let first = read_varint_u64(dict_bytes, &mut cursor)?;
    let last = read_varint_u64(dict_bytes, &mut cursor)?;
    if cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in prime-sequence dictionary".to_string(),
        ));
    }
    if width == 0 || width > 8 {
        return Err(ZbitError::Parse(
            "prime-sequence width must be between 1 and 8".to_string(),
        ));
    }
    if count.checked_mul(width) != Some(original_size) {
        return Err(ZbitError::Parse(
            "prime-sequence count/width does not match original size".to_string(),
        ));
    }
    if first < 2 || last < first || last > PRIME_SEQUENCE_MAX_SIEVE_VALUE {
        return Err(ZbitError::Parse(
            "prime-sequence dictionary values exceed supported bounds".to_string(),
        ));
    }
    let max_value = max_value_for_width(width).ok_or_else(|| {
        ZbitError::Parse("prime-sequence width does not have a valid value range".to_string())
    })?;
    if last > max_value {
        return Err(ZbitError::Parse(
            "prime-sequence last value exceeds width capacity".to_string(),
        ));
    }

    let prime = sieve_prime_flags(last as usize);
    if !prime.get(first as usize).copied().unwrap_or(false)
        || !prime.get(last as usize).copied().unwrap_or(false)
    {
        return Err(ZbitError::Parse(
            "prime-sequence endpoints must be prime".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(original_size);
    let mut emitted = 0usize;
    for value in first as usize..=last as usize {
        if prime[value] {
            write_le_u64_width(&mut out, value as u64, width)?;
            emitted += 1;
        }
    }
    if emitted != count || out.len() != original_size {
        return Err(ZbitError::Parse(
            "prime-sequence dictionary does not describe the requested prime count".to_string(),
        ));
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

const LZMA_FILTER_DELTA: lzma_sys::lzma_vli = 0x03;
const LZMA_DELTA_TYPE_BYTE: lzma_sys::__enum_ty = 0;

#[repr(C)]
struct LzmaOptionsDelta {
    delta_type: lzma_sys::__enum_ty,
    dist: u32,
    reserved_int1: u32,
    reserved_int2: u32,
    reserved_int3: u32,
    reserved_int4: u32,
    reserved_ptr1: *mut std::ffi::c_void,
    reserved_ptr2: *mut std::ffi::c_void,
}

fn lzma_ret_label(code: lzma_sys::lzma_ret) -> &'static str {
    match code {
        lzma_sys::LZMA_OK => "LZMA_OK",
        lzma_sys::LZMA_STREAM_END => "LZMA_STREAM_END",
        lzma_sys::LZMA_MEM_ERROR => "LZMA_MEM_ERROR",
        lzma_sys::LZMA_MEMLIMIT_ERROR => "LZMA_MEMLIMIT_ERROR",
        lzma_sys::LZMA_OPTIONS_ERROR => "LZMA_OPTIONS_ERROR",
        lzma_sys::LZMA_PROG_ERROR => "LZMA_PROG_ERROR",
        _ => "LZMA_ERROR",
    }
}

fn lzma_mode_from_xz2(mode: Mode) -> lzma_sys::lzma_mode {
    match mode {
        Mode::Fast => lzma_sys::LZMA_MODE_FAST,
        Mode::Normal => lzma_sys::LZMA_MODE_NORMAL,
    }
}

fn lzma_match_finder_from_xz2(match_finder: MatchFinder) -> lzma_sys::lzma_match_finder {
    match match_finder {
        MatchFinder::HashChain3 => lzma_sys::LZMA_MF_HC3,
        MatchFinder::HashChain4 => lzma_sys::LZMA_MF_HC4,
        MatchFinder::BinaryTree2 => lzma_sys::LZMA_MF_BT2,
        MatchFinder::BinaryTree3 => lzma_sys::LZMA_MF_BT3,
        MatchFinder::BinaryTree4 => lzma_sys::LZMA_MF_BT4,
    }
}

fn xz_encode_with_delta_tuning(
    data: &[u8],
    preset: u32,
    tuning: XzTuningParams,
) -> ZbitResult<Vec<u8>> {
    if !(1..=256).contains(&tuning.delta_dist) {
        return Err(ZbitError::Internal(format!(
            "LZMA delta distance {} is outside 1..=256",
            tuning.delta_dist
        )));
    }

    // SAFETY: liblzma initializes the option structs and writes only within the output
    // buffer bound returned by lzma_stream_buffer_bound. The filter option structs live
    // until lzma_stream_buffer_encode returns, and the filter array is terminated with
    // LZMA_VLI_UNKNOWN as required by the C API.
    unsafe {
        let mut lzma_options: lzma_sys::lzma_options_lzma = std::mem::zeroed();
        let preset_failed = lzma_sys::lzma_lzma_preset(&mut lzma_options, preset);
        if preset_failed != 0 {
            return Err(ZbitError::Io(format!(
                "lzma-sys preset init failed for preset {preset}"
            )));
        }
        lzma_options.dict_size = tuning.dict_size;
        lzma_options.lc = tuning.literal_context_bits;
        lzma_options.lp = tuning.literal_position_bits;
        lzma_options.pb = tuning.position_bits;
        lzma_options.mode = lzma_mode_from_xz2(tuning.mode);
        lzma_options.nice_len = tuning.nice_len;
        lzma_options.mf = lzma_match_finder_from_xz2(tuning.match_finder);
        lzma_options.depth = tuning.depth;

        let mut delta_options = LzmaOptionsDelta {
            delta_type: LZMA_DELTA_TYPE_BYTE,
            dist: tuning.delta_dist,
            reserved_int1: 0,
            reserved_int2: 0,
            reserved_int3: 0,
            reserved_int4: 0,
            reserved_ptr1: std::ptr::null_mut(),
            reserved_ptr2: std::ptr::null_mut(),
        };
        let mut filters = [
            lzma_sys::lzma_filter {
                id: LZMA_FILTER_DELTA,
                options: (&mut delta_options as *mut LzmaOptionsDelta).cast(),
            },
            lzma_sys::lzma_filter {
                id: lzma_sys::LZMA_FILTER_LZMA2,
                options: (&mut lzma_options as *mut lzma_sys::lzma_options_lzma).cast(),
            },
            lzma_sys::lzma_filter {
                id: lzma_sys::LZMA_VLI_UNKNOWN,
                options: std::ptr::null_mut(),
            },
        ];

        let bound = lzma_sys::lzma_stream_buffer_bound(data.len());
        let mut out = vec![0u8; bound];
        let mut out_pos = 0usize;
        let ret = lzma_sys::lzma_stream_buffer_encode(
            filters.as_mut_ptr(),
            lzma_sys::LZMA_CHECK_NONE,
            std::ptr::null(),
            data.as_ptr(),
            data.len(),
            out.as_mut_ptr(),
            &mut out_pos,
            out.len(),
        );
        if ret != lzma_sys::LZMA_OK {
            return Err(ZbitError::Io(format!(
                "lzma-sys delta+xz encode failed: {} ({ret})",
                lzma_ret_label(ret)
            )));
        }
        out.truncate(out_pos);
        Ok(out)
    }
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
    if tuning.delta_dist != 0 {
        return xz_encode_with_delta_tuning(data, preset, tuning);
    }

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
            delta_dist: 8,
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

fn average_abs_byte_delta(data: &[u8], distance: usize, stride: usize) -> Option<f64> {
    if distance == 0 || data.len() <= distance {
        return None;
    }
    let stride = stride.max(1);
    let mut sum = 0u64;
    let mut count = 0u64;
    let mut idx = distance;
    while idx < data.len() {
        let a = data[idx] as i16;
        let b = data[idx - distance] as i16;
        sum += (a - b).unsigned_abs() as u64;
        count += 1;
        idx = idx.saturating_add(stride);
    }
    if count == 0 {
        None
    } else {
        Some(sum as f64 / count as f64)
    }
}

fn eligible_delta_distances(data: &[u8], profile: CompressionProfile) -> Vec<u32> {
    if matches!(profile, CompressionProfile::Fast) || data.len() < 64 * 1024 {
        return Vec::new();
    }

    let sample_stride = (data.len() / (512 * 1024)).max(1);
    let baseline = average_abs_byte_delta(data, 251, sample_stride)
        .or_else(|| average_abs_byte_delta(data, 127, sample_stride))
        .unwrap_or(64.0);
    let mut scored = [1usize, 2, 3, 4, 8]
        .into_iter()
        .filter_map(|distance| {
            let avg = average_abs_byte_delta(data, distance, sample_stride)?;
            // Admit only when the same-distance residual is clearly structured. The absolute
            // threshold catches smooth byte streams; the relative threshold catches noisier
            // structured payloads whose far-lag residual is much worse.
            if avg <= 18.0 || avg * 4.0 <= baseline * 3.0 {
                Some((distance as u32, avg))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(dist_a, avg_a), (dist_b, avg_b)| {
        avg_a
            .partial_cmp(avg_b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| dist_a.cmp(dist_b))
    });
    scored.into_iter().map(|(distance, _)| distance).collect()
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
    match _profile {
        CompressionProfile::Fast => (0, 0),
        CompressionProfile::Balanced => (2, 0),
        CompressionProfile::Deep => (4, 2),
        CompressionProfile::Research => (5, 3),
    }
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
    delta_distances: &[u32],
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

    let mut delta_params = tuning
        .iter()
        .copied()
        .skip(delta_first_idx)
        .filter(|entry| delta_distances.contains(&entry.delta_dist))
        .collect::<Vec<_>>();
    delta_params.sort_by_key(|entry| {
        delta_distances
            .iter()
            .position(|&distance| distance == entry.delta_dist)
            .unwrap_or(delta_distances.len())
    });

    for params in delta_params.iter().copied().take(normal_delta_budget) {
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

        for params in delta_params.iter().copied().take(extreme_delta_budget) {
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
    let delta_distances = eligible_delta_distances(data, profile);
    let candidates = build_tuned_xz_candidates(profile, allow_xz_extreme, &delta_distances);
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
