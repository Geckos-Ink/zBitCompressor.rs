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
    fn multi_block_apply_invert_roundtrip() {
        // Focused unit test for the N3 multi-block apply/invert logic without going through
        // the full preflate roundtrip. We synthesize a plain payload with two regions that
        // prefer different plans, apply per-block plans, concatenate, then invert each block
        // and check that we recover the original payload byte-for-byte. This exercises the
        // exact code path the decoder uses for multi-block streams.
        let mut region_a = Vec::with_capacity(8 * 128);
        for row in 0..128u8 {
            region_a.push(row); // "filter byte"
            for col in 0..7u8 {
                region_a.push(col.wrapping_mul(11).wrapping_add(row & 0x07));
            }
        }
        let mut region_b = Vec::with_capacity(8 * 128);
        let mut state: u32 = 0xDEAD_FACE;
        for _ in 0..region_b.capacity() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            region_b.push(state as u8);
        }
        let mut plain = Vec::with_capacity(region_a.len() + region_b.len());
        plain.extend_from_slice(&region_a);
        plain.extend_from_slice(&region_b);
        let plain_len = plain.len();
        assert_eq!(plain_len % 2, 0);
        let block_size = (plain_len / 2) as u32;

        let plan_a = CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTail,
            period: 8,
            head: 1,
        };
        let plan_b = CircuitTransformPlan {
            kind: CircuitTransformKind::Identity,
            period: 0,
            head: 0,
        };
        let transformed_a = apply_transform_plan(&plain[..block_size as usize], &plan_a)
            .expect("apply plan A");
        let transformed_b = apply_transform_plan(&plain[block_size as usize..], &plan_b)
            .expect("apply plan B");
        assert_eq!(transformed_a.len(), block_size as usize);
        assert_eq!(transformed_b.len(), plain_len - block_size as usize);
        let mut concat = Vec::with_capacity(plain.len());
        concat.extend_from_slice(&transformed_a);
        concat.extend_from_slice(&transformed_b);
        assert_eq!(concat.len(), plain_len);

        // Reverse the same logic the decoder applies in the multi-block branch.
        let plans = [plan_a, plan_b];
        let block_count = plans.len();
        let leading_bytes = (block_size as usize) * (block_count - 1);
        let last_block_len = plain_len - leading_bytes;
        let mut recovered = Vec::with_capacity(plain_len);
        for (idx, plan) in plans.iter().enumerate() {
            let block_start = idx * (block_size as usize);
            let block_len = if idx + 1 == block_count {
                last_block_len
            } else {
                block_size as usize
            };
            let block_end = block_start + block_len;
            let block_transformed = &concat[block_start..block_end];
            let block_plain = invert_transform_plan(block_transformed, block_len, plan)
                .expect("invert per block");
            recovered.extend_from_slice(&block_plain);
        }
        assert_eq!(recovered, plain, "multi-block apply/invert roundtrip");

        // Sanity-check that the dictionary serialiser correctly tags this stream as
        // multi-block in the on-disk topology_count field.
        let topology = build_topology_for_plan(&plan_a).expect("topology");
        let placeholder_base = FramedPayloadRun {
            prefix: Vec::new(),
            suffix: Vec::new(),
            frame_tag: *b"FRMB",
            payload: vec![0u8; 16],
            base_chunk_len: 8,
            full_chunk_count: 2,
            tail_chunk_len: 0,
            total_chunks: 2,
        };
        let stream = RecursiveCircuitStream {
            base: placeholder_base,
            transformed_payload: concat.clone(),
            corrections_payload: Vec::new(),
            plain_len,
            transformed_encoded_len: concat.len(),
            correction_plain_len: 0,
            correction_encoded_len: 0,
            transformed_codec: PayloadCodec::Raw,
            correction_codec: PayloadCodec::Raw,
            zlib_header: [0u8; 2],
            zlib_adler32: [0u8; 4],
            transform_plan: plan_a,
            topology,
            multi_block: Some(MultiBlockPlan {
                block_size,
                plans: plans.to_vec(),
            }),
        };
        let mut dict_bytes = Vec::new();
        write_recursive_circuit_dictionary(&mut dict_bytes, &stream);
        // Verify the serialised size matches the size formula exactly and that the
        // multi-block plan section is the expected extra payload over a single-plan
        // dictionary built from the same topology (with the same compact/legacy form).
        assert_eq!(dict_bytes.len(), recursive_circuit_dictionary_size(&stream));
        let mut single_plan_stream = stream.clone();
        single_plan_stream.multi_block = None;
        let single_plan_size = recursive_circuit_dictionary_size(&single_plan_stream);
        let expected_multi_block_bytes = multi_block_section_size(
            stream.multi_block.as_ref().expect("multi_block built above"),
        );
        assert_eq!(
            dict_bytes.len(),
            single_plan_size + expected_multi_block_bytes,
            "multi-block plan section must add exactly {expected_multi_block_bytes} bytes \
             beyond the single-plan dictionary size"
        );
    }

    #[test]
    fn compact_topology_bit_lengths_match_design() {
        // Sanity check that the bit-packed topology actually spends only the bits it
        // claims to. Field widths: relation 1, order 2, kind_index 6, is_root 1,
        // parent_index = ceil(log2(prev_count)) bits (0 for the first node), and
        // params encoded as nibble-varint (3 value bits + 1 continuation bit per
        // nibble). Anything larger here means a regression — exactly the kind of
        // unused-combination waste this layout exists to eliminate.

        // Node #0: root with kind=0 and zero params.
        let root = CircuitTopologyNode {
            id: 1,
            parent_id: u32::MAX,
            relation: 0,
            order: 0,
            kind: 0,
            param_a: 0,
            param_b: 0,
            hash64: 0,
        };
        let root_bits = compact_topology_node_bit_len(&root, 0);
        // 1 + 2 + 6 + 1 + 0 (no parent) + 4 (one nibble for 0) + 4 = 18 bits
        assert_eq!(root_bits, 18, "root node should be exactly 18 bits");

        // Node #1: child of node #0 with a PNG-stride period of 6401 and head=1.
        // Field widths: 1+2+6+1+0(parent_index has 0 width for prev_count==1) +
        // nibble-varint(6401) + nibble-varint(1). 6401 in base-8 is 14401, which
        // needs ceil(log2(6402)/3) = 5 nibbles → 20 bits. 1 needs 1 nibble = 4 bits.
        let stride_child = CircuitTopologyNode {
            id: 2,
            parent_id: 1,
            relation: 1,
            order: 2,
            kind: 4,
            param_a: 6401,
            param_b: 1,
            hash64: 0,
        };
        let child_bits = compact_topology_node_bit_len(&stride_child, 1);
        assert_eq!(child_bits, 1 + 2 + 6 + 1 + 0 + 20 + 4);

        // The two-node topology in bytes: ceil((18 + 34) / 8) = 7 payload bytes,
        // plus the 4-byte node_bit_len header in front => 11 bytes total. The
        // legacy fixed-width form for the same nodes would be 56 bytes (28 each),
        // so the new layout uses ~20% of the original space.
        let total_bytes = compact_topology_total_size(&[root.clone(), stride_child.clone()]);
        let legacy_bytes = 2 * TOPOLOGY_NODE_BYTES;
        assert_eq!(total_bytes, 4 + ((18 + child_bits + 7) / 8));
        assert!(
            total_bytes * 4 < legacy_bytes,
            "bit-packed two-node topology ({total_bytes} B) should be at least 4x \
             smaller than the legacy layout ({legacy_bytes} B)"
        );
    }

    #[test]
    fn adaptive_pack_can_choose_adaptive_transformed_xz_and_roundtrips() {
        // Synthesize a payload that has stride-4 correlation at the float-tensor level but
        // whose absolute byte values are noisy enough that raw-xz cannot trivially crush it
        // (we deliberately keep its ratio above the 0.30 skip-heuristic threshold so the
        // adaptive-transformed-xz candidate is actually evaluated). Each "row" of four
        // bytes nudges each column independently by a small signed delta, so adjacent rows
        // are similar at stride 4 but the per-column byte stream looks pseudo-random.
        let mut input = Vec::with_capacity(256 * 1024);
        let mut state: u32 = 0x1357_BD9F;
        let mut prev: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
        for _ in 0..(256 * 1024 / 4) {
            let mut new_word = [0u8; 4];
            for j in 0..4 {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                // Larger nudge range adds enough byte-level entropy to keep raw-xz from
                // dropping below the 0.30 ratio gate, so the adaptive path is exercised.
                let nudge = ((state >> 4) & 0x3F) as i8 - 32;
                new_word[j] = prev[j].wrapping_add(nudge as u8);
            }
            input.extend_from_slice(&new_word);
            prev = new_word;
        }

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("zbit_pack_adaptive_xz_{stamp}.zbpk"));
        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);
        assert_eq!(output, input, "adaptive-transformed-xz roundtrip");

        // We do not require adaptive to be the WINNER here (raw-xz could still beat it on
        // some seeds), but on stride-structured data above the 16-KiB gate and the 0.30
        // raw-xz heuristic the candidate must at least be evaluated.
        let raw_xz_size = stats.raw_xz_candidate_bytes.unwrap_or(usize::MAX);
        if raw_xz_size.saturating_mul(10) > stats.original_size.saturating_mul(3) {
            assert!(
                stats.adaptive_transformed_xz_candidate_bytes.is_some(),
                "adaptive-transformed-xz must be evaluated when raw-xz ratio > 0.30 \
                 (raw-xz {raw_xz_size} on input {})",
                stats.original_size
            );
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
