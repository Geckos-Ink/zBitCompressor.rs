// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

#[cfg(test)]
fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn append_crc_frame(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        push_u32_be(out, data.len() as u32);
        out.extend_from_slice(chunk_type);
        out.extend_from_slice(data);
        let mut hasher = Crc32Hasher::new();
        hasher.update(chunk_type);
        hasher.update(data);
        push_u32_be(out, hasher.finalize());
    }

    fn build_framed_container_with_many_frames() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"ZBIT-FRAMED-PREFIX");

        let full_chunk_len = 128usize;
        let full_chunks = 96usize;
        let tail_chunk_len = 73usize;
        let total_len = full_chunk_len * full_chunks + tail_chunk_len;

        let mut payload = vec![0u8; total_len];
        let mut state = 0xA5A5_1337u32;
        for byte in &mut payload {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = (state >> 24) as u8;
        }

        let mut cursor = 0usize;
        for _ in 0..full_chunks {
            let slice = &payload[cursor..cursor + full_chunk_len];
            append_crc_frame(&mut out, b"DATA", slice);
            cursor += full_chunk_len;
        }

        append_crc_frame(&mut out, b"DATA", &payload[cursor..]);
        out.extend_from_slice(b"ZBIT-FRAMED-SUFFIX");
        out
    }

    fn build_valid_framed_container_with_split_deflate() -> (Vec<u8>, Vec<u8>) {
        let width = 64u32;
        let height = 64u32;

        let row_bytes = (width as usize) * 4;
        let mut filtered = Vec::with_capacity((row_bytes + 1) * (height as usize));
        let mut prev_raw = vec![0u8; row_bytes];

        for y in 0..height as usize {
            let filter = (y % 5) as u8;
            filtered.push(filter);

            let mut raw_row = vec![0u8; row_bytes];
            for x in 0..width as usize {
                let idx = x * 4;
                raw_row[idx] = ((x * 3 + y * 5) & 0xFF) as u8;
                raw_row[idx + 1] = ((x * 7 + y * 11) & 0xFF) as u8;
                raw_row[idx + 2] = ((x * 13 + y * 17) & 0xFF) as u8;
                raw_row[idx + 3] = 255u8;
            }

            for i in 0..row_bytes {
                let encoded = match filter {
                    0 => raw_row[i],
                    1 => {
                        let left = if i >= 4 { raw_row[i - 4] } else { 0 };
                        raw_row[i].wrapping_sub(left)
                    }
                    2 => raw_row[i].wrapping_sub(prev_raw[i]),
                    3 => {
                        let left = if i >= 4 { raw_row[i - 4] } else { 0 };
                        let up = prev_raw[i];
                        raw_row[i].wrapping_sub(((left as u16 + up as u16) / 2) as u8)
                    }
                    4 => {
                        let left = if i >= 4 { raw_row[i - 4] } else { 0 };
                        let up = prev_raw[i];
                        let up_left = if i >= 4 { prev_raw[i - 4] } else { 0 };
                        raw_row[i].wrapping_sub(paeth_predictor(left, up, up_left))
                    }
                    _ => unreachable!(),
                };
                filtered.push(encoded);
            }

            prev_raw.copy_from_slice(&raw_row);
        }

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&filtered).expect("zlib write");
        let framed_payload = encoder.finish().expect("zlib finish");

        let mut container = Vec::new();
        container.extend_from_slice(b"ZBIT-DEFLATE-PREFIX");

        let chunk = 1024usize;
        let mut cursor = 0usize;
        while cursor < framed_payload.len() {
            let end = (cursor + chunk).min(framed_payload.len());
            append_crc_frame(&mut container, b"DATA", &framed_payload[cursor..end]);
            cursor = end;
        }

        container.extend_from_slice(b"ZBIT-DEFLATE-SUFFIX");
        (container, filtered)
    }

    #[test]
    fn adaptive_pack_roundtrip() {
        let input = b"abcabcabcabc\nxyzxyzxyz\n";

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_test_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
    }

    #[test]
    fn adaptive_pack_can_choose_huffman_and_roundtrip() {
        let input = b"the quick brown fox jumps over the lazy dog\n".repeat(2000);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_huffman_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
        assert!(
            stats.indexed_huffman_candidate_bytes.is_some(),
            "huffman candidate should be evaluated for repetitive text"
        );
    }

    #[test]
    fn adaptive_pack_can_choose_raw_deflate_and_roundtrip() {
        let input = b"lorem ipsum dolor sit amet, consectetur adipiscing elit\\n".repeat(4000);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_deflate_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
        assert!(
            matches!(
                stats.chosen_method,
                PackMethod::RawDeflate | PackMethod::RawZstd | PackMethod::RawXz
            ),
            "expected a strong raw compressor, got {:?}",
            stats.chosen_method
        );
    }

    #[test]
    fn adaptive_pack_can_choose_raw_zstd_and_roundtrip() {
        let input =
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\nbbbbbbbbbbbbbbbbbbbbbbbb\\n".repeat(10_000);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_zstd_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
        assert!(stats.raw_zstd_candidate_bytes.is_some());
    }

    #[test]
    fn adaptive_pack_can_choose_monotonic_delta_and_roundtrip() {
        let mut input = Vec::new();
        let mut value = 10_000u64;
        let mut state = 0xC0FF_EE11u32;

        for _ in 0..90_000usize {
            write_le_u64_width(&mut input, value, 3).expect("write u24 value");
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let gap = ((state >> 27) as u64) + 1;
            value = value.saturating_add(gap);
        }

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_monotonic_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(
            matches!(stats.chosen_method, PackMethod::MonotonicDelta),
            "expected monotonic-delta to be chosen, got {:?}",
            stats.chosen_method
        );
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
        assert!(stats.monotonic_delta_candidate_bytes.is_some());
    }

    #[test]
    fn adaptive_pack_evaluates_framed_raw_and_roundtrips() {
        let input = build_framed_container_with_many_frames();

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_framed_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        let framed_candidate = stats
            .framed_raw_candidate_bytes
            .expect("framed-raw candidate should be available");
        assert!(
            framed_candidate < stats.raw_candidate_bytes,
            "framed-raw should beat raw-copy on multi-frame input"
        );
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
    }

    #[test]
    fn recursive_transform_roundtrip() {
        let (_container, filtered_plain) = build_valid_framed_container_with_split_deflate();
        let plan = CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTail,
            period: 257,
            head: 1,
        };
        let transformed = apply_transform_plan(&filtered_plain, &plan).expect("build transform");
        let decoded = invert_transform_plan(&transformed, filtered_plain.len(), &plan)
            .expect("decode transform");
        assert_eq!(decoded, filtered_plain);
    }

    #[test]
    fn row_aware_transforms_roundtrip() {
        // Builds a tail with multiple rows so the row-bounded predictors are exercised across
        // row boundaries. The first row is identity (no predictor reference), subsequent rows
        // exercise the row-internal predictor (Sub/Xor) and the cross-row Up predictor.
        let row_len = 17usize; // arbitrary, > all pixel strides we test
        let period = row_len + 1; // +1 for the head byte
        let row_count = 12usize;
        let mut input = Vec::with_capacity(period * row_count);
        let mut state: u32 = 0xC0FFEE17;
        for row in 0..row_count {
            input.push(row as u8); // head byte = row index (filter-byte analogue)
            for _ in 0..row_len {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                input.push((state >> 24) as u8);
            }
        }

        let kinds_with_stride = [
            CircuitTransformKind::PeriodicHeadTailTailRowDelta,
            CircuitTransformKind::PeriodicHeadTailTailRowXor,
        ];
        for kind in kinds_with_stride {
            for pixel_stride in [1u32, 2, 3, 4] {
                let plan = CircuitTransformPlan {
                    kind,
                    period: period as u32,
                    head: pixel_stride,
                };
                let transformed = apply_transform_plan(&input, &plan)
                    .unwrap_or_else(|| panic!("apply {:?} stride={pixel_stride}", kind));
                let decoded = invert_transform_plan(&transformed, input.len(), &plan)
                    .unwrap_or_else(|| panic!("invert {:?} stride={pixel_stride}", kind));
                assert_eq!(decoded, input, "{:?} stride={pixel_stride} roundtrip", kind);
            }
        }

        let up_plan = CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTailTailRowUp,
            period: period as u32,
            head: 0,
        };
        let transformed = apply_transform_plan(&input, &up_plan).expect("apply row-up");
        let decoded =
            invert_transform_plan(&transformed, input.len(), &up_plan).expect("invert row-up");
        assert_eq!(decoded, input, "row-up roundtrip");
    }

    #[test]
    fn bit_plane_tail_transforms_roundtrip() {
        // Mixed-entropy input: a head byte plus a tail with both repetitive low-bit content
        // and noisier high-bit content, so the bit-plane transpose moves real bits and the
        // delta has something to extract on top.
        let row_len = 31usize;
        let period = row_len + 1;
        let row_count = 9usize;
        let mut input = Vec::with_capacity(period * row_count);
        let mut state: u32 = 0xDEADBEEF;
        for row in 0..row_count {
            input.push(row as u8);
            for col in 0..row_len {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                // Combine pseudo-random and structured patterns so bit planes are non-trivial.
                let mixed = (state >> 24) as u8 ^ (col as u8) ^ (row as u8).wrapping_mul(11);
                input.push(mixed);
            }
        }

        for kind in [
            CircuitTransformKind::PeriodicHeadTailTailBitPlaneTranspose,
            CircuitTransformKind::PeriodicHeadTailTailBitPlaneTransposeDelta,
        ] {
            let plan = CircuitTransformPlan {
                kind,
                period: period as u32,
                head: 0,
            };
            let transformed = apply_transform_plan(&input, &plan)
                .unwrap_or_else(|| panic!("apply {:?}", kind));
            assert_eq!(
                transformed.len(),
                input.len(),
                "{:?} preserves length",
                kind
            );
            let decoded = invert_transform_plan(&transformed, input.len(), &plan)
                .unwrap_or_else(|| panic!("invert {:?}", kind));
            assert_eq!(decoded, input, "{:?} roundtrip", kind);
        }
    }

    #[test]
    fn adaptive_pack_evaluates_recursive_circuit_xz_candidate_and_roundtrips() {
        let (input, _filtered_plain) = build_valid_framed_container_with_split_deflate();

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_recursive_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(
            stats.recursive_circuit_xz_candidate_bytes.is_some(),
            "recursive-circuit-xz candidate should be available for valid framed deflate container"
        );
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
    }
}
