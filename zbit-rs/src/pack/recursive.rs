// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

fn push_u32_be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn read_u32_be_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

// Upper bound on a single CRC32-framed payload. Anything larger is almost certainly a
// false-positive read of an unrelated 4-byte field interpreted as a frame length, and
// computing CRC32 across megabytes for a false positive is the dominant cost when a non-
// framed input (e.g. a PyTorch model archive) feeds `build_framed_payload_run`.
const FRAMED_PAYLOAD_MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
// Cumulative cap on bytes hashed by CRC32 across all probing in build_framed_payload_run.
// Once exceeded the analyzer bails out and returns whatever best run it has found, or
// None. Sized comfortably for legitimate framed inputs (cat IDAT is ~3 MB) but tight
// enough that pathologically-distributed inputs cannot blow up to minutes of wall time.
const FRAMED_PAYLOAD_HASH_BUDGET_BYTES: usize = 256 * 1024 * 1024;

fn parse_crc32_frame_at(
    input: &[u8],
    start: usize,
    hash_budget_remaining: &mut usize,
) -> Option<(u32, [u8; 4], usize, usize)> {
    let frame_len_u32 = read_u32_be_at(input, start)?;
    let frame_len = frame_len_u32 as usize;
    // Reject implausibly large frame lengths before paying for memory access / CRC32.
    if frame_len > FRAMED_PAYLOAD_MAX_FRAME_BYTES {
        return None;
    }
    let tag_off = start.checked_add(4)?;
    let tag_slice = input.get(tag_off..tag_off + 4)?;
    let tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
    let data_off = tag_off + 4;
    let data_end = data_off.checked_add(frame_len)?;
    let crc_off = data_end;
    let next = crc_off.checked_add(4)?;
    if next > input.len() {
        return None;
    }
    // Each frame check costs CRC32 over `frame_len + 4` bytes. Accumulate against the
    // shared budget so a stream of false positives can't blow past the cap.
    let hash_cost = frame_len.saturating_add(4);
    if hash_cost > *hash_budget_remaining {
        return None;
    }
    *hash_budget_remaining = hash_budget_remaining.saturating_sub(hash_cost);
    let data = input.get(data_off..data_end)?;
    let crc = read_u32_be_at(input, crc_off)?;
    let mut hasher = Crc32Hasher::new();
    hasher.update(&tag);
    hasher.update(data);
    if hasher.finalize() != crc {
        return None;
    }
    Some((frame_len_u32, tag, data_off, next))
}

fn build_framed_payload_run(input: &[u8]) -> Option<FramedPayloadRun> {
    if input.len() < 24 {
        return None;
    }

    let mut best: Option<(usize, FramedPayloadRun)> = None;
    let mut start = 0usize;
    let mut hash_budget = FRAMED_PAYLOAD_HASH_BUDGET_BYTES;

    while start + 12 <= input.len() {
        // Once the CRC32 budget is exhausted we bail out — for inputs without any framed
        // run this caps the analyzer in milliseconds instead of minutes.
        if hash_budget == 0 {
            break;
        }
        let Some((first_len_u32, first_tag, first_data_off, first_next)) =
            parse_crc32_frame_at(input, start, &mut hash_budget)
        else {
            start += 1;
            continue;
        };

        let mut chunk_lengths = vec![first_len_u32];
        let mut payload = Vec::<u8>::new();
        payload
            .extend_from_slice(input.get(first_data_off..first_data_off + first_len_u32 as usize)?);

        let mut cursor = first_next;
        while let Some((len_u32, tag, data_off, next)) =
            parse_crc32_frame_at(input, cursor, &mut hash_budget)
        {
            if tag != first_tag {
                break;
            }
            chunk_lengths.push(len_u32);
            payload.extend_from_slice(input.get(data_off..data_off + len_u32 as usize)?);
            cursor = next;
        }

        if chunk_lengths.len() < 2 {
            start += 1;
            continue;
        }

        let total_chunks = u32::try_from(chunk_lengths.len()).ok()?;
        let base_chunk_len = chunk_lengths[0];
        let mut full_chunk_count = total_chunks;
        let mut tail_chunk_len = 0u32;

        if chunk_lengths.iter().any(|&len| len != base_chunk_len) {
            if chunk_lengths
                .iter()
                .take(chunk_lengths.len().saturating_sub(1))
                .any(|&len| len != base_chunk_len)
            {
                start += 1;
                continue;
            }
            full_chunk_count = total_chunks.saturating_sub(1);
            tail_chunk_len = *chunk_lengths.last().unwrap_or(&0u32);
        }

        let run = FramedPayloadRun {
            prefix: input[..start].to_vec(),
            suffix: input[cursor..].to_vec(),
            frame_tag: first_tag,
            payload,
            base_chunk_len,
            full_chunk_count,
            tail_chunk_len,
            total_chunks,
        };

        let candidate_size = ZBPK_HEADER_BYTES + framed_dictionary_size(&run) + run.payload.len();
        match &best {
            Some((best_size, _)) if *best_size <= candidate_size => {}
            _ => best = Some((candidate_size, run)),
        }

        // After committing to a multi-frame run, skip past it: nothing inside the run can
        // produce a better candidate (subsequent starts would only yield SHORTER runs).
        start = cursor;
    }

    best.map(|(_, run)| run)
}

// v3 framed dictionary: every size field is a varint. Six varints + 4 fixed tag bytes
// replace the prior six fixed u32 slots (24 bytes) and the tag (4 bytes). For typical
// chunk runs (~hundreds of KB chunks, tens of frames) every varint is 1-3 bytes, so the
// fixed section shrinks from 28 bytes to roughly 10-14 bytes.
fn framed_dictionary_size(stream: &FramedPayloadRun) -> usize {
    let mut size = 4 /* frame_tag */ + stream.prefix.len() + stream.suffix.len();
    size += varint_u64_len(stream.prefix.len() as u64);
    size += varint_u64_len(stream.suffix.len() as u64);
    size += varint_u64_len(stream.base_chunk_len as u64);
    size += varint_u64_len(stream.full_chunk_count as u64);
    size += varint_u64_len(stream.tail_chunk_len as u64);
    size += varint_u64_len(stream.total_chunks as u64);
    size
}

fn write_framed_dictionary(out: &mut Vec<u8>, stream: &FramedPayloadRun) {
    push_varint_u64(out, stream.prefix.len() as u64);
    push_varint_u64(out, stream.suffix.len() as u64);
    out.extend_from_slice(&stream.frame_tag);
    push_varint_u64(out, stream.base_chunk_len as u64);
    push_varint_u64(out, stream.full_chunk_count as u64);
    push_varint_u64(out, stream.tail_chunk_len as u64);
    push_varint_u64(out, stream.total_chunks as u64);
    out.extend_from_slice(&stream.prefix);
    out.extend_from_slice(&stream.suffix);
}

fn decode_framed_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut dict_cursor = 0usize;
    let prefix_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let suffix_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let tag_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 4)
        .ok_or_else(|| ZbitError::Parse("framed-raw missing frame tag".to_string()))?;
    dict_cursor += 4;
    let frame_tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
    let base_chunk_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let full_chunk_count = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let tail_chunk_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let total_chunks = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;

    let prefix = dict_bytes
        .get(dict_cursor..dict_cursor + prefix_len)
        .ok_or_else(|| ZbitError::Parse("framed-raw prefix range out of bounds".to_string()))?;
    dict_cursor += prefix_len;

    let suffix = dict_bytes
        .get(dict_cursor..dict_cursor + suffix_len)
        .ok_or_else(|| ZbitError::Parse("framed-raw suffix range out of bounds".to_string()))?;
    dict_cursor += suffix_len;

    if dict_cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in framed-raw dictionary".to_string(),
        ));
    }

    let tail_present = if total_chunks == full_chunk_count {
        false
    } else if total_chunks == full_chunk_count + 1 {
        true
    } else {
        return Err(ZbitError::Parse(
            "framed-raw dictionary has inconsistent chunk counters".to_string(),
        ));
    };

    let expected_payload = full_chunk_count
        .checked_mul(base_chunk_len)
        .and_then(|v| {
            if tail_present {
                v.checked_add(tail_chunk_len)
            } else {
                Some(v)
            }
        })
        .ok_or_else(|| ZbitError::Parse("framed-raw payload length overflow".to_string()))?;

    if payload.len() != expected_payload {
        return Err(ZbitError::Parse(format!(
            "framed-raw payload length mismatch: expected {expected_payload} got {}",
            payload.len()
        )));
    }

    let chunk_overhead = total_chunks
        .checked_mul(12)
        .ok_or_else(|| ZbitError::Parse("framed-raw chunk overhead overflow".to_string()))?;
    let mut out = Vec::with_capacity(
        prefix
            .len()
            .checked_add(payload.len())
            .and_then(|v| v.checked_add(suffix.len()))
            .and_then(|v| v.checked_add(chunk_overhead))
            .ok_or_else(|| ZbitError::Parse("framed-raw output length overflow".to_string()))?,
    );

    out.extend_from_slice(prefix);

    let mut payload_cursor = 0usize;
    for idx in 0..total_chunks {
        let chunk_len = if idx < full_chunk_count {
            base_chunk_len
        } else {
            tail_chunk_len
        };
        let chunk_data = payload
            .get(payload_cursor..payload_cursor + chunk_len)
            .ok_or_else(|| {
                ZbitError::Parse("framed-raw payload chunk range out of bounds".to_string())
            })?;
        payload_cursor += chunk_len;

        push_u32_be(&mut out, chunk_len as u32);
        out.extend_from_slice(&frame_tag);
        out.extend_from_slice(chunk_data);

        let mut hasher = Crc32Hasher::new();
        hasher.update(&frame_tag);
        hasher.update(chunk_data);
        push_u32_be(&mut out, hasher.finalize());
    }

    out.extend_from_slice(suffix);

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "framed-raw output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}


fn preflate_chain_candidates(profile: CompressionProfile) -> Vec<u32> {
    let default = match profile {
        CompressionProfile::Fast => vec![4096u32],
        CompressionProfile::Balanced => vec![4096u32, 8192, 16384],
        CompressionProfile::Deep => vec![4096u32, 8192, 16384, 24576],
        CompressionProfile::Research => vec![2048u32, 4096, 8192, 16384, 24576, 32768],
    };
    let Some(raw) = std::env::var_os("ZBIT_PREFLATE_CHAIN_CANDIDATES") else {
        return default;
    };

    let mut out = Vec::new();
    for token in raw.to_string_lossy().split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = trimmed.parse::<u32>() {
            if value >= 256 {
                out.push(value);
            }
        }
    }

    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        default
    } else {
        out
    }
}

fn build_recursive_circuit_stream(
    _input: &[u8],
    base: &FramedPayloadRun,
    context: &mut CompressionContext,
) -> ZbitResult<Option<RecursiveCircuitStream>> {
    if base.payload.len() < 6 {
        context.push_skipped("recursive-circuit-xz skipped: framed payload smaller than 6 bytes");
        return Ok(None);
    }
    let recursive_total_timer = Instant::now();

    let mut zlib_header = [0u8; 2];
    zlib_header.copy_from_slice(&base.payload[..2]);
    let mut zlib_adler32 = [0u8; 4];
    zlib_adler32.copy_from_slice(&base.payload[base.payload.len() - 4..]);
    let deflate_stream = &base.payload[2..base.payload.len() - 4];
    let deflate_stream_hash = payload_hash(deflate_stream);

    let preflate_timer = Instant::now();
    let mut preflate_results = Vec::new();
    let mut missing_chain_lengths = Vec::new();
    for max_chain_length in preflate_chain_candidates(context.profile) {
        let cache_key = (deflate_stream_hash, max_chain_length);
        if let Some(cached) = context.cache.preflate_outputs.get(&cache_key) {
            context.cache_stats.preflate_hits = context.cache_stats.preflate_hits.saturating_add(1);
            if let Some((corrections, plain)) = cached {
                preflate_results.push((max_chain_length, corrections.clone(), plain.clone()));
            }
            continue;
        }

        context.cache_stats.preflate_misses = context.cache_stats.preflate_misses.saturating_add(1);
        missing_chain_lengths.push(max_chain_length);
    }

    let evaluated_missing = missing_chain_lengths
        .into_par_iter()
        .map(|max_chain_length| {
            let mut config = PreflateConfig::default();
            config.verify_compression = true;
            config.plain_text_limit = ZBPK_MAX_OUTPUT_BYTES;
            config.max_chain_length = max_chain_length;
            let evaluated = match preflate_whole_deflate_stream(deflate_stream, &config) {
                Ok((chunk, plain)) => Some((chunk.corrections, plain.text().to_vec())),
                Err(_) => None,
            };
            (max_chain_length, evaluated)
        })
        .collect::<Vec<_>>();
    for (max_chain_length, evaluated) in evaluated_missing {
        let cache_key = (deflate_stream_hash, max_chain_length);
        context
            .cache
            .preflate_outputs
            .insert(cache_key, evaluated.clone());
        if let Some((corrections, plain)) = evaluated {
            preflate_results.push((max_chain_length, corrections, plain));
        }
    }
    preflate_results.sort_unstable_by_key(|(max_chain_length, _, _)| *max_chain_length);
    context.timings.recursive_preflate_ms += preflate_timer.elapsed().as_secs_f64() * 1000.0;
    if preflate_results.is_empty() {
        context.push_skipped(
            "recursive-circuit-xz unavailable: preflate reconstruction failed for all chain candidates"
        );
        context.timings.recursive_total_ms += recursive_total_timer.elapsed().as_secs_f64() * 1000.0;
        return Ok(None);
    }

    let plain_len = preflate_results[0].2.len();
    let transformed_template = choose_adaptive_transform_plan(
        &preflate_results[0].2,
        context.profile,
        context.profile.max_transform_plans(),
    )?;
    context.timings.recursive_transform_sampling_ms += transformed_template.sampling_ms;
    context.timings.recursive_transform_eval_ms += transformed_template.eval_ms;

    // N3 multi-block path: probe a small set of block-count candidates allowed by the active
    // profile. Each candidate splits the inflated plain into N equal-ish blocks, picks the
    // best transform plan per block, concatenates the transformed bytes, and re-encodes with
    // the full codec selection. We keep the candidate with the smallest *codec payload +
    // multi-block metadata*; if no candidate beats the single-plan template, we discard them
    // all and stay on the legacy single-plan path.
    let multi_block_candidate: Option<MultiBlockTransformResult> = {
        let splits = context.profile.multi_block_split_counts();
        if splits.is_empty() {
            None
        } else {
            let trace_recursive = std::env::var_os("ZBIT_TRACE_RECURSIVE").is_some();
            let plain_slice: &[u8] = &preflate_results[0].2;
            let profile = context.profile;
            let mut results: Vec<MultiBlockTransformResult> = splits
                .iter()
                .copied()
                .filter_map(|n| build_multi_block_transform(plain_slice, n, profile).ok().flatten())
                .collect();
            if trace_recursive {
                for result in &results {
                    eprintln!(
                        "zbit-trace recursive multi-block split={} block_size={} payload={} plans={:?}",
                        result.plans.len(),
                        result.block_size,
                        result.payload.len(),
                        result
                            .plans
                            .iter()
                            .map(|p| (p.kind.name(), p.period, p.head))
                            .collect::<Vec<_>>(),
                    );
                }
            }
            // Sort by total candidate cost: codec payload bytes plus the *actual* deduplicated
            // multi-block dictionary section bytes (one entry per unique plan in the plan table,
            // plus a bit-packed per-block index stream). The previous estimate used
            // `4 + 4 + N * 9` — the worst case where every block carries a distinct
            // (kind, period, head) tuple. The deduplicated form is what we actually write to
            // disk, so it must be what we compare against.
            results.sort_by_key(|result| {
                let plan = MultiBlockPlan {
                    block_size: result.block_size,
                    plans: result.plans.clone(),
                };
                result.payload.len() + multi_block_section_size(&plan)
            });
            results.into_iter().next()
        }
    };
    if let Some(ref mb) = multi_block_candidate {
        context.timings.recursive_transform_eval_ms += mb.eval_ms;
    }

    // Pick the best primary template (single-plan vs multi-block) by total payload + metadata
    // cost. Both forms share the same per-chain correction modelling stage below.
    let single_template_total =
        transformed_template.payload.len();
    let multi_block_winner = multi_block_candidate.as_ref().and_then(|mb| {
        let plan = MultiBlockPlan {
            block_size: mb.block_size,
            plans: mb.plans.clone(),
        };
        let mb_total = mb.payload.len() + multi_block_section_size(&plan);
        if mb_total < single_template_total {
            Some(mb)
        } else {
            None
        }
    });
    let trace_recursive = std::env::var_os("ZBIT_TRACE_RECURSIVE").is_some();
    if trace_recursive {
        eprintln!(
            "zbit-trace recursive template-pick single={} multi-block={:?} winner={}",
            single_template_total,
            multi_block_candidate.as_ref().map(|mb| {
                let plan = MultiBlockPlan {
                    block_size: mb.block_size,
                    plans: mb.plans.clone(),
                };
                mb.payload.len() + multi_block_section_size(&plan)
            }),
            if multi_block_winner.is_some() {
                "multi-block"
            } else {
                "single-plan"
            }
        );
    }
    let template_payload = if let Some(mb) = multi_block_winner {
        mb.payload.clone()
    } else {
        transformed_template.payload.clone()
    };
    let template_codec = if let Some(mb) = multi_block_winner {
        mb.codec
    } else {
        transformed_template.codec
    };
    let template_multi_block = multi_block_winner.map(|mb| MultiBlockPlan {
        block_size: mb.block_size,
        plans: mb.plans.clone(),
    });

    let correction_timer = Instant::now();
    let profile = context.profile;
    let evaluated = preflate_results
        .into_par_iter()
        .map(|(max_chain_length, corrections, _)| {
            let (
                correction_plan,
                correction_codec,
                corrections_payload,
                _correction_sample_ms,
                _correction_eval_ms,
            ) = choose_correction_transform_plan(&corrections, profile)?;

            let mut topology = transformed_template.topology.clone();
            let _ = embed_correction_plan_in_topology(&mut topology, &correction_plan)?;

            let stream = RecursiveCircuitStream {
                base: base.clone(),
                transformed_payload: template_payload.clone(),
                corrections_payload,
                plain_len,
                transformed_encoded_len: template_payload.len(),
                correction_plain_len: corrections.len(),
                correction_encoded_len: 0,
                transformed_codec: template_codec,
                correction_codec,
                zlib_header,
                zlib_adler32,
                transform_plan: transformed_template.plan,
                topology,
                multi_block: template_multi_block.clone(),
            };
            let correction_encoded_len = stream.corrections_payload.len();
            let mut stream = stream;
            stream.correction_encoded_len = correction_encoded_len;

            let candidate_total = ZBPK_HEADER_BYTES
                + recursive_circuit_dictionary_size(&stream)
                + stream.transformed_encoded_len
                + stream.correction_encoded_len;
            if trace_recursive {
                eprintln!(
                    "zbit-trace recursive chain={} plan={} period={} head={} transformed={}({}) corrections={}({}) corr-plain={} total={}",
                    max_chain_length,
                    stream.transform_plan.kind.name(),
                    stream.transform_plan.period,
                    stream.transform_plan.head,
                    stream.transformed_encoded_len,
                    stream.transformed_codec.name(),
                    stream.correction_encoded_len,
                    stream.correction_codec.name(),
                    stream.correction_plain_len,
                    candidate_total,
                );
                eprintln!(
                    "zbit-trace recursive correction-plan={} period={} head={}",
                    correction_plan.kind.name(),
                    correction_plan.period,
                    correction_plan.head
                );
            }
            Ok::<_, ZbitError>((max_chain_length, stream, candidate_total))
        })
        .collect::<ZbitResult<Vec<_>>>()?;
    context.timings.recursive_correction_modeling_ms +=
        correction_timer.elapsed().as_secs_f64() * 1000.0;

    let Some((_max_chain_length, stream, _)) = evaluated
        .into_iter()
        .min_by_key(|(max_chain_length, _, candidate_total)| (*candidate_total, *max_chain_length))
    else {
        context.push_skipped("recursive-circuit-xz unavailable: no valid correction model candidate");
        context.timings.recursive_total_ms += recursive_total_timer.elapsed().as_secs_f64() * 1000.0;
        return Ok(None);
    };

    let trace_recursive = std::env::var_os("ZBIT_TRACE_RECURSIVE").is_some();

    if trace_recursive {
        eprintln!(
            "zbit-trace recursive selected chain plan={} period={} head={} transformed={}({}) corrections={}({}) plain={}",
            stream.transform_plan.kind.name(),
            stream.transform_plan.period,
            stream.transform_plan.head,
            stream.transformed_encoded_len,
            stream.transformed_codec.name(),
            stream.correction_encoded_len,
            stream.correction_codec.name(),
            plain_len,
        );
    }
    context.timings.recursive_total_ms += recursive_total_timer.elapsed().as_secs_f64() * 1000.0;
    Ok(Some(stream))
}

// Top bit of the on-disk topology count signals the N3 multi-block extension. When set,
// an additional `block_count u32`, `block_size u32`, then `block_count` plans of the form
// (kind u8, period u32, head u32) follow the topology nodes. Legacy single-plan
// dictionaries set neither this bit nor the trailing section, and decode unchanged.
const RECURSIVE_TOPOLOGY_MULTI_BLOCK_FLAG: u16 = 0x8000;
// Second-highest bit signals the *bit-packed* topology serialisation. With this flag set
// the topology is written as a single bit stream (MSB-first) where every field takes
// exactly the bits it needs:
//   * relation       — 1 bit
//   * order          — 2 bits (builders only emit 0..3; eligibility checks the bound)
//   * kind_index     — 6 bits (49 in-use kind values mapped to a dense 0..48 range)
//   * is_root        — 1 bit
//   * parent_index   — ceil(log2(prev_node_count)) bits when not root (0 bits for the
//                      very first node, 1 bit for node #2, 2 bits for node #3..#4, …)
//   * param_a / param_b — nibble varints (3 value bits + 1 continuation per nibble)
// `id` is implicit (= 1-based node position) so the wire carries no id bytes at all.
// `hash64` is NOT serialised — overall decode correctness already validates the topology
// end-to-end through the inverse-transform pipeline. The bit stream is followed by a
// `node_bit_len` u32 in the surrounding fixed header (written just before the bit bytes)
// so the reader can verify it stopped at the right offset and skip the byte-alignment
// padding cleanly. Legacy fixed-width dictionaries (flag clear) decode unchanged.
const RECURSIVE_TOPOLOGY_COMPACT_FLAG: u16 = 0x4000;
const RECURSIVE_TOPOLOGY_COUNT_MASK: u16 = 0x3FFF;

const COMPACT_TOPOLOGY_KIND_INDEX_BITS: u32 = 6;
const COMPACT_TOPOLOGY_ORDER_BITS: u32 = 2;

/// Dense 0..N mapping of the actual u8 `kind` values the topology builders emit. The
/// builder uses kinds 0..26 for normal topology nodes and 200..222 (= correction-plan
/// base + transform kind) for embedded correction-plan markers; together 50 distinct
/// values fit comfortably in 6 bits.
fn kind_to_compact_index(kind: u8) -> Option<u8> {
    if kind <= 26 {
        Some(kind)
    } else if kind >= TOPOLOGY_CORRECTION_PLAN_KIND_BASE
        && kind <= TOPOLOGY_CORRECTION_PLAN_KIND_BASE.saturating_add(22)
    {
        let offset = kind - TOPOLOGY_CORRECTION_PLAN_KIND_BASE;
        Some(27 + offset)
    } else {
        None
    }
}

fn compact_index_to_kind(index: u8) -> Option<u8> {
    if index <= 26 {
        Some(index)
    } else if index >= 27 && index <= 49 {
        let offset = index - 27;
        Some(TOPOLOGY_CORRECTION_PLAN_KIND_BASE + offset)
    } else {
        None
    }
}

fn nibble_varint_bit_len(mut value: u64) -> usize {
    let mut bits = 4usize;
    let mut remaining = value >> 3;
    value >>= 3;
    while remaining != 0 {
        bits += 4;
        remaining >>= 3;
    }
    let _ = value;
    bits
}

fn compact_topology_node_bit_len(node: &CircuitTopologyNode, prev_count: usize) -> usize {
    // 1 (relation) + 2 (order) + 6 (kind) + 1 (is_root)
    let mut bits = 1 + COMPACT_TOPOLOGY_ORDER_BITS as usize + COMPACT_TOPOLOGY_KIND_INDEX_BITS as usize + 1;
    if node.parent_id != u32::MAX {
        bits += bits_for_index(prev_count) as usize;
    }
    bits += nibble_varint_bit_len(node.param_a as u64);
    bits += nibble_varint_bit_len(node.param_b as u64);
    bits
}

fn compact_topology_total_bit_len(nodes: &[CircuitTopologyNode]) -> usize {
    let mut bits = 0usize;
    for (idx, node) in nodes.iter().enumerate() {
        bits += compact_topology_node_bit_len(node, idx);
    }
    bits
}

fn compact_topology_total_size(nodes: &[CircuitTopologyNode]) -> usize {
    // 4-byte node_bit_len header preceding the bit stream, then the padded bit stream.
    let bit_len = compact_topology_total_bit_len(nodes);
    let payload_bytes = (bit_len + 7) / 8;
    4 + payload_bytes
}

// Compact form is used when every node's metadata fits the bit-packed encoding:
//   * order is in 0..3 (2 bits),
//   * kind has a defined index in `kind_to_compact_index`,
//   * the id is sequential (1-based node position) — true for all builders today.
// If any node fails these checks, fall back to the legacy fixed-width layout.
fn compact_topology_eligible(nodes: &[CircuitTopologyNode]) -> bool {
    for (idx, node) in nodes.iter().enumerate() {
        if node.order > 3 {
            return false;
        }
        if kind_to_compact_index(node.kind).is_none() {
            return false;
        }
        if node.id != (idx as u32) + 1 {
            return false;
        }
    }
    true
}

// v3 recursive-circuit fixed section layout (everything but the topology + multi-block):
//   zlib_header [2 fixed]
//   zlib_adler32 [4 fixed]
//   varint plain_len
//   varint transformed_encoded_len
//   varint correction_plain_len
//   varint correction_encoded_len
//   codec_kind u16 little-endian, bit layout:
//     bits 0..1   transformed_codec      (4 values fit in 2 bits)
//     bits 2..3   correction_codec       (4 values fit in 2 bits)
//     bits 4..9   transform_kind_index   (49 values mapped to 0..48 via kind_to_compact_index)
//     bits 10..15 reserved (must be 0)
//   varint period
//   varint head
//   topology_field u16 (existing layout: COMPACT_FLAG | MULTI_BLOCK_FLAG | count)
// Multi-block trailing section, when present:
//   varint block_count
//   varint block_size
//   per plan: { kind_index_u8 (6 bits used), varint period, varint head }
// The legacy fixed-width form (used internally for size estimation when we want a fast
// upper bound) was 51 bytes; the actual v3 dictionary is variable but typically 25-35 B.
fn recursive_circuit_fixed_section_size(stream: &RecursiveCircuitStream) -> usize {
    // `transformed_encoded_len` is intentionally NOT serialised: it is reconstructed at
    // decode time as `payload_size - correction_encoded_len` (both available from the
    // ZBPK header and this dictionary respectively). Storing it would be redundant.
    2 /* zlib_header */ + 4 /* zlib_adler32 */
        + varint_u64_len(stream.plain_len as u64)
        + varint_u64_len(stream.correction_plain_len as u64)
        + varint_u64_len(stream.correction_encoded_len as u64)
        + 2 /* codec_kind u16 */
        + varint_u64_len(stream.transform_plan.period as u64)
        + varint_u64_len(stream.transform_plan.head as u64)
        + 2 /* topology_field u16 */
}

// IMMED-1.2 migration target (ZBPK v4): the section below currently rolls its own
// width-prefix bit encoding for the multi-block plan table. The replacement is a single
// `CircuitBitStream::write_def_or_ref::<CircuitTransformPlan>` per plan, which:
//   - drops the explicit `multi_block_unique_plans` dedup pass (the bit-stream interning
//     table handles it automatically),
//   - lets the same stream span main topology + correction topology + multi-block plans,
//     so a plan that appears in two of those sections pays one definition + one
//     `log2(N)`-bit reference instead of two independent encodings,
//   - matches the existing per-plan bit budget (6-bit kind + nibble-varint period/head)
//     because the `BitSerializable for CircuitTransformPlan` impl in bitstream.rs is
//     bit-identical to today's per-plan layout.
// Blocked by: ZBPK_VERSION bump (v3 → v4), updating decode_recursive_circuit_payload to
// thread one shared `CircuitBitStreamReader` through every embedded section, and
// updating roundtrip tests. See `pack::bitstream_integration_tests::immed_1_*` for the
// expected savings shape on N occurrences of the same plan.
//
// Multi-block plan dictionary — the whole trailer is **one continuous bit stream** with
// every field sized to exactly the bits its value or context requires. No byte alignment
// between fields, no varint continuation overhead. A `block_size` of 1024 takes 11 bits
// (10 value bits plus a 1-bit width-prefix tag) plus its 5-bit width header for the
// width-prefixed fields; a 6-bit kind index takes 6 bits; the per-block plan selector
// takes `ceil(log2(plan_table_len))` bits each, which is **zero** when every block
// picks the same plan. The decoder peels the stream by sequential `read_bits` calls.
//
// Wire layout (single bit stream, MSB-first within each field):
//
//   block_count           in 4 bits                              (capped at 15)
//   block_size_width      in 5 bits                              (0..31, fits any u32 ≤ 2^31)
//   block_size            in block_size_width bits
//   plan_table_len_m1     in `bits_for_index(block_count)` bits  (table_len = m1 + 1)
//
//   for each plan in the table:
//     kind_index          in 6 bits                              (49 active kinds map into 0..63)
//
//   period_width          in 5 bits                              (max width across all periods)
//   for each plan: period in period_width bits
//
//   head_width            in 5 bits                              (max width across all heads)
//   for each plan: head   in head_width bits
//
//   for each block:
//     plan_idx            in `bits_for_index(plan_table_len)` bits   (0 bits when table_len=1)
//
//   padding to the next byte boundary
//
// The "width" headers are the bit-shifting analogue of varint: spend 5 bits once to
// declare a per-group max, then pack every value in that group at exactly that width.
// On the cat split=2 trailer this brings the trailer from 15 B (varint flat) down to
// 12 B; on cat split=4 from 19 B down to 18 B. The savings on shipped packs are zero —
// cat single-plan always wins — but the format is now genuinely "only the bits needed".
const MULTI_BLOCK_COUNT_BITS: u32 = 4;
const MULTI_BLOCK_WIDTH_HEADER_BITS: u32 = 5;
const MULTI_BLOCK_KIND_INDEX_BITS: u32 = 6;
const MULTI_BLOCK_MAX_BLOCKS: usize = (1 << MULTI_BLOCK_COUNT_BITS) - 1;
const MULTI_BLOCK_MAX_FIELD_WIDTH: u32 = (1 << MULTI_BLOCK_WIDTH_HEADER_BITS) - 1;

fn multi_block_unique_plans(plan: &MultiBlockPlan) -> Vec<CircuitTransformPlan> {
    let mut unique: Vec<CircuitTransformPlan> = Vec::with_capacity(plan.plans.len());
    for entry in &plan.plans {
        if !unique.iter().any(|existing| existing == entry) {
            unique.push(*entry);
        }
    }
    unique
}

/// Width in bits required to represent a value of magnitude `value` (0 → 0 bits, 1 → 1
/// bit, 1024 → 11 bits, etc.). This is exactly the `bits_for_index(value + 1)` formula
/// applied to a single field rather than to a range size, which is what we need when a
/// field carries an unbounded magnitude (block_size, period, head) rather than an index
/// into a known set.
fn bits_for_value(value: u64) -> u32 {
    if value == 0 {
        return 0;
    }
    let mut bits = 0u32;
    let mut v = value;
    while v > 0 {
        bits += 1;
        v >>= 1;
    }
    bits
}

/// Width in bits needed to encode each entry of a fixed-width group whose maximum value
/// is `max_value`. Same as `bits_for_value` but named for the per-group case.
fn group_field_width(max_value: u64) -> u32 {
    bits_for_value(max_value)
}

/// Pre-computes the per-group widths used by both the size formula and the writer/reader.
fn multi_block_layout(plan: &MultiBlockPlan) -> MultiBlockLayout {
    let unique = multi_block_unique_plans(plan);
    let block_size_width = bits_for_value(plan.block_size as u64);
    let max_period = unique.iter().map(|p| p.period as u64).max().unwrap_or(0);
    let max_head = unique.iter().map(|p| p.head as u64).max().unwrap_or(0);
    let period_width = group_field_width(max_period);
    let head_width = group_field_width(max_head);
    MultiBlockLayout {
        unique,
        block_size_width,
        period_width,
        head_width,
    }
}

struct MultiBlockLayout {
    unique: Vec<CircuitTransformPlan>,
    block_size_width: u32,
    period_width: u32,
    head_width: u32,
}

fn multi_block_trailer_bit_len(plan: &MultiBlockPlan, layout: &MultiBlockLayout) -> usize {
    let block_count = plan.plans.len();
    let table_len = layout.unique.len();
    let mut bits = MULTI_BLOCK_COUNT_BITS as usize;
    bits += MULTI_BLOCK_WIDTH_HEADER_BITS as usize + layout.block_size_width as usize;
    // plan_table_len_m1 in bits_for_index(block_count) bits — block_count=1 needs 0 bits,
    // block_count=2 needs 1 bit, etc.
    bits += bits_for_index(block_count) as usize;
    bits += MULTI_BLOCK_KIND_INDEX_BITS as usize * table_len;
    bits += MULTI_BLOCK_WIDTH_HEADER_BITS as usize + layout.period_width as usize * table_len;
    bits += MULTI_BLOCK_WIDTH_HEADER_BITS as usize + layout.head_width as usize * table_len;
    let per_block_bits = bits_for_index(table_len) as usize;
    bits += per_block_bits * block_count;
    bits
}

fn multi_block_section_size(plan: &MultiBlockPlan) -> usize {
    let layout = multi_block_layout(plan);
    let bits = multi_block_trailer_bit_len(plan, &layout);
    (bits + 7) / 8
}

fn write_multi_block_section(out: &mut Vec<u8>, mb: &MultiBlockPlan) {
    let layout = multi_block_layout(mb);
    let block_count = mb.plans.len();
    let table_len = layout.unique.len();

    debug_assert!(
        block_count <= MULTI_BLOCK_MAX_BLOCKS,
        "multi-block trailer assumes block_count <= {MULTI_BLOCK_MAX_BLOCKS} \
         (the profile multi_block_split_counts() values stay well under this)",
    );
    debug_assert!(
        layout.block_size_width <= MULTI_BLOCK_MAX_FIELD_WIDTH
            && layout.period_width <= MULTI_BLOCK_MAX_FIELD_WIDTH
            && layout.head_width <= MULTI_BLOCK_MAX_FIELD_WIDTH,
        "multi-block trailer width header is 5 bits; no field may need more than 31 bits",
    );

    let total_bits = multi_block_trailer_bit_len(mb, &layout);
    let mut bw = BitWriter::with_capacity((total_bits + 7) / 8);

    bw.write_bits(block_count as u64, MULTI_BLOCK_COUNT_BITS);
    bw.write_bits(layout.block_size_width as u64, MULTI_BLOCK_WIDTH_HEADER_BITS);
    if layout.block_size_width > 0 {
        bw.write_bits(mb.block_size as u64, layout.block_size_width);
    }

    let table_len_bits = bits_for_index(block_count);
    if table_len_bits > 0 {
        // We encode (table_len - 1) so the value 0 is reachable when block_count == 1.
        bw.write_bits((table_len - 1) as u64, table_len_bits);
    }

    for plan in &layout.unique {
        let kind_u8 = plan.kind.as_u8();
        let kind_index = kind_to_compact_index(kind_u8).unwrap_or(kind_u8 & 0x3F);
        bw.write_bits(kind_index as u64, MULTI_BLOCK_KIND_INDEX_BITS);
    }

    bw.write_bits(layout.period_width as u64, MULTI_BLOCK_WIDTH_HEADER_BITS);
    if layout.period_width > 0 {
        for plan in &layout.unique {
            bw.write_bits(plan.period as u64, layout.period_width);
        }
    }

    bw.write_bits(layout.head_width as u64, MULTI_BLOCK_WIDTH_HEADER_BITS);
    if layout.head_width > 0 {
        for plan in &layout.unique {
            bw.write_bits(plan.head as u64, layout.head_width);
        }
    }

    let per_block_bits = bits_for_index(table_len);
    if per_block_bits > 0 {
        for plan in &mb.plans {
            let idx = layout
                .unique
                .iter()
                .position(|existing| existing == plan)
                .expect("multi_block_unique_plans was built from these same plans");
            bw.write_bits(idx as u64, per_block_bits);
        }
    }

    debug_assert_eq!(
        bw.bit_len(),
        total_bits,
        "writer must consume exactly the bits the size formula predicted",
    );
    out.extend_from_slice(&bw.into_bytes());
}

fn read_multi_block_section(
    dict_bytes: &[u8],
    cursor: &mut usize,
) -> ZbitResult<(u32, Vec<CircuitTransformPlan>)> {
    // Hand the BitReader the rest of the dictionary; we track exactly how many bits we
    // consume and advance `cursor` by the matching byte count (rounded up to clear the
    // trailing padding bits cleanly).
    let bit_buf = dict_bytes
        .get(*cursor..)
        .ok_or_else(|| ZbitError::Parse("multi-block trailer range out of bounds".to_string()))?;
    let mut br = BitReader::new(bit_buf);

    let block_count = br.read_bits(MULTI_BLOCK_COUNT_BITS)? as usize;
    if block_count == 0 {
        return Err(ZbitError::Parse(
            "multi-block trailer claims zero blocks".to_string(),
        ));
    }

    let block_size_width = br.read_bits(MULTI_BLOCK_WIDTH_HEADER_BITS)? as u32;
    if block_size_width > MULTI_BLOCK_MAX_FIELD_WIDTH {
        return Err(ZbitError::Parse(format!(
            "multi-block trailer block_size_width {block_size_width} out of range",
        )));
    }
    let block_size = if block_size_width == 0 {
        0u32
    } else {
        let raw = br.read_bits(block_size_width)?;
        if raw > u32::MAX as u64 {
            return Err(ZbitError::Parse(
                "multi-block trailer block_size exceeds u32".to_string(),
            ));
        }
        raw as u32
    };
    if block_size == 0 {
        return Err(ZbitError::Parse(
            "multi-block trailer block_size is zero".to_string(),
        ));
    }

    let table_len_bits = bits_for_index(block_count);
    let table_len_m1 = if table_len_bits == 0 {
        0u64
    } else {
        br.read_bits(table_len_bits)?
    };
    let table_len = (table_len_m1 + 1) as usize;
    if table_len == 0 || table_len > block_count {
        return Err(ZbitError::Parse(format!(
            "multi-block trailer plan table size {table_len} is invalid (must be 1..={block_count})",
        )));
    }

    let mut kinds: Vec<CircuitTransformKind> = Vec::with_capacity(table_len);
    for _ in 0..table_len {
        let kind_index = br.read_bits(MULTI_BLOCK_KIND_INDEX_BITS)? as u8;
        let kind_u8 = compact_index_to_kind(kind_index).ok_or_else(|| {
            ZbitError::Parse(format!(
                "multi-block trailer invalid kind index {kind_index}",
            ))
        })?;
        let kind = CircuitTransformKind::from_u8(kind_u8).ok_or_else(|| {
            ZbitError::Parse("multi-block trailer invalid transform kind".to_string())
        })?;
        kinds.push(kind);
    }

    let period_width = br.read_bits(MULTI_BLOCK_WIDTH_HEADER_BITS)? as u32;
    if period_width > MULTI_BLOCK_MAX_FIELD_WIDTH {
        return Err(ZbitError::Parse(format!(
            "multi-block trailer period_width {period_width} out of range",
        )));
    }
    let mut periods: Vec<u32> = Vec::with_capacity(table_len);
    for _ in 0..table_len {
        let period = if period_width == 0 {
            0u64
        } else {
            br.read_bits(period_width)?
        };
        if period > u32::MAX as u64 {
            return Err(ZbitError::Parse(
                "multi-block trailer period exceeds u32".to_string(),
            ));
        }
        periods.push(period as u32);
    }

    let head_width = br.read_bits(MULTI_BLOCK_WIDTH_HEADER_BITS)? as u32;
    if head_width > MULTI_BLOCK_MAX_FIELD_WIDTH {
        return Err(ZbitError::Parse(format!(
            "multi-block trailer head_width {head_width} out of range",
        )));
    }
    let mut heads: Vec<u32> = Vec::with_capacity(table_len);
    for _ in 0..table_len {
        let head = if head_width == 0 {
            0u64
        } else {
            br.read_bits(head_width)?
        };
        if head > u32::MAX as u64 {
            return Err(ZbitError::Parse(
                "multi-block trailer head exceeds u32".to_string(),
            ));
        }
        heads.push(head as u32);
    }

    let plan_table: Vec<CircuitTransformPlan> = kinds
        .into_iter()
        .zip(periods.into_iter())
        .zip(heads.into_iter())
        .map(|((kind, period), head)| CircuitTransformPlan { kind, period, head })
        .collect();

    let per_block_bits = bits_for_index(table_len);
    let mut block_plans: Vec<CircuitTransformPlan> = Vec::with_capacity(block_count);
    if per_block_bits == 0 {
        for _ in 0..block_count {
            block_plans.push(plan_table[0]);
        }
    } else {
        for _ in 0..block_count {
            let idx = br.read_bits(per_block_bits)? as usize;
            if idx >= table_len {
                return Err(ZbitError::Parse(format!(
                    "multi-block trailer plan index {idx} exceeds table {table_len}",
                )));
            }
            block_plans.push(plan_table[idx]);
        }
    }

    let consumed_bits = br.bit_pos();
    *cursor += (consumed_bits + 7) / 8;
    Ok((block_size, block_plans))
}

fn recursive_circuit_dictionary_size(stream: &RecursiveCircuitStream) -> usize {
    let use_compact = compact_topology_eligible(&stream.topology);
    let topology_bytes = if use_compact {
        compact_topology_total_size(&stream.topology)
    } else {
        stream.topology.len() * TOPOLOGY_NODE_BYTES
    };
    let mut size = framed_dictionary_size(&stream.base)
        + recursive_circuit_fixed_section_size(stream)
        + topology_bytes;
    if let Some(mb) = &stream.multi_block {
        size += multi_block_section_size(mb);
    }
    size
}

fn write_recursive_circuit_dictionary(out: &mut Vec<u8>, stream: &RecursiveCircuitStream) {
    write_framed_dictionary(out, &stream.base);
    out.extend_from_slice(&stream.zlib_header);
    out.extend_from_slice(&stream.zlib_adler32);
    push_varint_u64(out, stream.plain_len as u64);
    // transformed_encoded_len is intentionally omitted — see comment on
    // recursive_circuit_fixed_section_size.
    push_varint_u64(out, stream.correction_plain_len as u64);
    push_varint_u64(out, stream.correction_encoded_len as u64);
    // codec_kind u16: see the comment on recursive_circuit_fixed_section_size for the
    // bit layout. The dictionary uses the same kind-index mapping as the compact topology
    // so we re-use kind_to_compact_index here; if the active transform_kind is outside
    // that mapping (shouldn't happen with current builders) we conservatively fall back
    // to the u8 value masked to 6 bits — the decoder will reject it and we get a clear
    // error instead of a silent corruption.
    let transform_kind_u8 = stream.transform_plan.kind.as_u8();
    let kind_index = kind_to_compact_index(transform_kind_u8).unwrap_or(transform_kind_u8 & 0x3F);
    let codec_kind = ((stream.transformed_codec.as_u8() as u16) & 0x03)
        | (((stream.correction_codec.as_u8() as u16) & 0x03) << 2)
        | (((kind_index as u16) & 0x3F) << 4);
    push_u16(out, codec_kind);
    push_varint_u64(out, stream.transform_plan.period as u64);
    push_varint_u64(out, stream.transform_plan.head as u64);
    let use_compact = compact_topology_eligible(&stream.topology);
    let mut topology_field =
        (stream.topology.len() as u16) & RECURSIVE_TOPOLOGY_COUNT_MASK;
    if stream.multi_block.is_some() {
        topology_field |= RECURSIVE_TOPOLOGY_MULTI_BLOCK_FLAG;
    }
    if use_compact {
        topology_field |= RECURSIVE_TOPOLOGY_COMPACT_FLAG;
    }
    push_u16(out, topology_field);
    if use_compact {
        let mut bw = BitWriter::with_capacity(
            (compact_topology_total_bit_len(&stream.topology) + 7) / 8,
        );
        for (idx, node) in stream.topology.iter().enumerate() {
            bw.write_bits(node.relation as u64, 1);
            bw.write_bits(node.order as u64, COMPACT_TOPOLOGY_ORDER_BITS);
            let kind_index = kind_to_compact_index(node.kind)
                .expect("eligibility check guarantees a defined compact kind index");
            bw.write_bits(kind_index as u64, COMPACT_TOPOLOGY_KIND_INDEX_BITS);
            if node.parent_id == u32::MAX {
                bw.write_bits(1, 1); // is_root
            } else {
                bw.write_bits(0, 1);
                // Parent is one of the previously emitted nodes; the topology builder
                // assigns ids sequentially as `(emitted_index + 1)`, so we can map
                // parent_id directly to the emitted index (parent_id - 1).
                let parent_index = (node.parent_id as usize).saturating_sub(1);
                let width = bits_for_index(idx);
                if width > 0 {
                    bw.write_bits(parent_index as u64, width);
                }
            }
            bw.write_nibble_varint(node.param_a as u64);
            bw.write_nibble_varint(node.param_b as u64);
        }
        let node_bit_len = bw.bit_len() as u32;
        push_u32(out, node_bit_len);
        out.extend_from_slice(&bw.into_bytes());
    } else {
        for node in &stream.topology {
            push_u32(out, node.id);
            push_u32(out, node.parent_id);
            out.push(node.relation);
            push_u16(out, node.order);
            out.push(node.kind);
            push_u32(out, node.param_a);
            push_u32(out, node.param_b);
            push_u64(out, node.hash64);
        }
    }
    if let Some(mb) = &stream.multi_block {
        write_multi_block_section(out, mb);
    }
}

fn decode_recursive_circuit_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut dict_cursor = 0usize;
    let prefix_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let suffix_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let tag_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 4)
        .ok_or_else(|| ZbitError::Parse("recursive-circuit-xz missing frame tag".to_string()))?;
    dict_cursor += 4;
    let frame_tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
    let base_chunk_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let full_chunk_count = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let tail_chunk_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let total_chunks = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;

    let prefix = dict_bytes
        .get(dict_cursor..dict_cursor + prefix_len)
        .ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz prefix range out of bounds".to_string())
        })?;
    dict_cursor += prefix_len;

    let suffix = dict_bytes
        .get(dict_cursor..dict_cursor + suffix_len)
        .ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz suffix range out of bounds".to_string())
        })?;
    dict_cursor += suffix_len;

    let zlib_header_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 2)
        .ok_or_else(|| ZbitError::Parse("recursive-circuit-xz missing zlib header".to_string()))?;
    dict_cursor += 2;
    let mut zlib_header = [0u8; 2];
    zlib_header.copy_from_slice(zlib_header_slice);

    let zlib_adler_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 4)
        .ok_or_else(|| ZbitError::Parse("recursive-circuit-xz missing zlib adler32".to_string()))?;
    dict_cursor += 4;
    let mut zlib_adler32 = [0u8; 4];
    zlib_adler32.copy_from_slice(zlib_adler_slice);

    let plain_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let correction_plain_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    let correction_encoded_len = read_varint_u64(dict_bytes, &mut dict_cursor)? as usize;
    // transformed_encoded_len is not stored on the wire — it is exactly
    // `payload.len() - correction_encoded_len`. We validate the relationship below.
    let transformed_encoded_len =
        payload
            .len()
            .checked_sub(correction_encoded_len)
            .ok_or_else(|| {
                ZbitError::Parse(
                    "recursive-circuit-xz correction_encoded_len exceeds payload length"
                        .to_string(),
                )
            })?;
    let codec_kind = read_u16(dict_bytes, &mut dict_cursor)?;
    let transformed_codec =
        PayloadCodec::from_u8(((codec_kind & 0x03) as u8) & 0x07).ok_or_else(|| {
            ZbitError::Parse(
                "recursive-circuit-xz dictionary has invalid transformed codec".to_string(),
            )
        })?;
    let correction_codec =
        PayloadCodec::from_u8((((codec_kind >> 2) & 0x03) as u8) & 0x07).ok_or_else(|| {
            ZbitError::Parse(
                "recursive-circuit-xz dictionary has invalid correction codec".to_string(),
            )
        })?;
    let kind_index = ((codec_kind >> 4) & 0x3F) as u8;
    let transform_kind_u8 = compact_index_to_kind(kind_index).ok_or_else(|| {
        ZbitError::Parse(format!(
            "recursive-circuit-xz dictionary has invalid kind index {kind_index}"
        ))
    })?;
    let transform_kind = CircuitTransformKind::from_u8(transform_kind_u8).ok_or_else(|| {
        ZbitError::Parse("recursive-circuit-xz dictionary has invalid transform kind".to_string())
    })?;
    let transform_period = read_varint_u64(dict_bytes, &mut dict_cursor)? as u32;
    let transform_head = read_varint_u64(dict_bytes, &mut dict_cursor)? as u32;
    let topology_raw = read_u16(dict_bytes, &mut dict_cursor)?;
    let multi_block_present = (topology_raw & RECURSIVE_TOPOLOGY_MULTI_BLOCK_FLAG) != 0;
    let compact_topology = (topology_raw & RECURSIVE_TOPOLOGY_COMPACT_FLAG) != 0;
    let topology_count = (topology_raw & RECURSIVE_TOPOLOGY_COUNT_MASK) as usize;
    let mut correction_plan = CircuitTransformPlan {
        kind: CircuitTransformKind::Identity,
        period: 0,
        head: 0,
    };

    let mut seen_root = false;
    let mut last_id = 0u32;
    // hash_by_id is only consulted for legacy (non-compact) topology integrity validation.
    let mut hash_by_id = HashMap::<u32, u64>::new();
    if compact_topology {
        // Bit-packed topology stream: first a u32 node_bit_len telling the reader how
        // many bits the per-node block holds (so we can detect a malformed file early
        // and skip the byte-alignment padding cleanly), then the bits themselves.
        let node_bit_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
        let node_bytes_len = (node_bit_len + 7) / 8;
        let bit_buf = dict_bytes
            .get(dict_cursor..dict_cursor + node_bytes_len)
            .ok_or_else(|| {
                ZbitError::Parse(
                    "compact recursive-circuit-xz topology bit stream range out of bounds"
                        .to_string(),
                )
            })?;
        let mut br = BitReader::new(bit_buf);
        for idx in 0..topology_count {
            let relation = br.read_bits(1)? as u8;
            let order = br.read_bits(COMPACT_TOPOLOGY_ORDER_BITS)? as u16;
            let kind_index = br.read_bits(COMPACT_TOPOLOGY_KIND_INDEX_BITS)? as u8;
            let kind = compact_index_to_kind(kind_index).ok_or_else(|| {
                ZbitError::Parse(format!(
                    "compact recursive-circuit-xz topology has invalid kind index {kind_index}"
                ))
            })?;
            let is_root = br.read_bits(1)? != 0;
            let parent_id = if is_root {
                u32::MAX
            } else {
                // For idx == 0 there is no previous node, so a non-root first node is
                // invalid. For idx >= 1 with `bits_for_index(idx) == 0` (i.e. exactly
                // one possible parent), the parent index is implicit and we encode 0
                // bits — parent must be node #0 (id == 1).
                if idx == 0 {
                    return Err(ZbitError::Parse(
                        "compact recursive-circuit-xz topology has non-root first node"
                            .to_string(),
                    ));
                }
                let width = bits_for_index(idx);
                let parent_idx = if width == 0 {
                    0usize
                } else {
                    br.read_bits(width)? as usize
                };
                if parent_idx >= idx {
                    return Err(ZbitError::Parse(
                        "compact recursive-circuit-xz parent index past current node"
                            .to_string(),
                    ));
                }
                (parent_idx as u32).saturating_add(1)
            };
            let param_a_u64 = br.read_nibble_varint()?;
            let param_b_u64 = br.read_nibble_varint()?;
            if param_a_u64 > u32::MAX as u64 || param_b_u64 > u32::MAX as u64 {
                return Err(ZbitError::Parse(
                    "compact recursive-circuit-xz topology param exceeds u32".to_string(),
                ));
            }
            let id = (idx as u32) + 1;
            let param_a = param_a_u64 as u32;
            let param_b = param_b_u64 as u32;
            if relation > 1 {
                return Err(ZbitError::Parse(
                    "compact recursive-circuit-xz topology relation must be 0 or 1"
                        .to_string(),
                ));
            }
            if idx > 0 && id <= last_id {
                return Err(ZbitError::Parse(
                    "compact recursive-circuit-xz topology ids must be strictly increasing"
                        .to_string(),
                ));
            }
            if parent_id == u32::MAX {
                seen_root = true;
            }
            if let Some(plan) = decode_embedded_correction_plan(kind, param_a, param_b) {
                correction_plan = plan;
            }
            last_id = id;
            // unused for compact form: order, hash table
            let _ = order;
        }
        if br.bit_pos() != node_bit_len {
            return Err(ZbitError::Parse(format!(
                "compact recursive-circuit-xz topology consumed {} bits but header claimed {}",
                br.bit_pos(),
                node_bit_len
            )));
        }
        dict_cursor += node_bytes_len;
    } else {
        for idx in 0..topology_count {
            let id = read_u32(dict_bytes, &mut dict_cursor)?;
            let parent_id = read_u32(dict_bytes, &mut dict_cursor)?;
            let relation = read_u8(dict_bytes, &mut dict_cursor)?;
            let order = read_u16(dict_bytes, &mut dict_cursor)?;
            let kind = read_u8(dict_bytes, &mut dict_cursor)?;
            let param_a = read_u32(dict_bytes, &mut dict_cursor)?;
            let param_b = read_u32(dict_bytes, &mut dict_cursor)?;
            let stored_hash = read_u64(dict_bytes, &mut dict_cursor)?;
            // Legacy form carries an explicit per-node FNV-style hash for tamper detection;
            // verify it inline so the legacy format keeps its current integrity guarantee.
            let parent_hash = if parent_id == u32::MAX {
                TOPOLOGY_HASH_OFFSET
            } else {
                *hash_by_id.get(&parent_id).ok_or_else(|| {
                    ZbitError::Parse(
                        "recursive-circuit-xz topology references unknown parent".to_string(),
                    )
                })?
            };
            let mut expected_hash = TOPOLOGY_HASH_OFFSET;
            expected_hash = topology_hash_mix(expected_hash, parent_hash);
            expected_hash = topology_hash_mix(expected_hash, id as u64);
            expected_hash = topology_hash_mix(expected_hash, parent_id as u64);
            expected_hash = topology_hash_mix(expected_hash, relation as u64);
            expected_hash = topology_hash_mix(expected_hash, order as u64);
            expected_hash = topology_hash_mix(expected_hash, kind as u64);
            expected_hash = topology_hash_mix(expected_hash, param_a as u64);
            expected_hash = topology_hash_mix(expected_hash, param_b as u64);
            if stored_hash != expected_hash {
                return Err(ZbitError::Parse(
                    "recursive-circuit-xz topology hash mismatch".to_string(),
                ));
            }
            hash_by_id.insert(id, expected_hash);
            if relation > 1 {
                return Err(ZbitError::Parse(
                    "recursive-circuit-xz topology relation must be 0 or 1".to_string(),
                ));
            }
            if idx > 0 && id <= last_id {
                return Err(ZbitError::Parse(
                    "recursive-circuit-xz topology node ids must be strictly increasing"
                        .to_string(),
                ));
            }
            if parent_id == u32::MAX {
                seen_root = true;
            }
            if let Some(plan) = decode_embedded_correction_plan(kind, param_a, param_b) {
                correction_plan = plan;
            }
            last_id = id;
        }
    }

    let mut block_plans: Vec<CircuitTransformPlan> = Vec::new();
    let mut block_size_field = 0u32;
    if multi_block_present {
        let (block_size, plans) = read_multi_block_section(dict_bytes, &mut dict_cursor)?;
        block_size_field = block_size;
        block_plans = plans;
    }

    if dict_cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in recursive-circuit-xz dictionary".to_string(),
        ));
    }
    if topology_count == 0 || !seen_root {
        return Err(ZbitError::Parse(
            "recursive-circuit-xz topology metadata missing valid root".to_string(),
        ));
    }

    let expected_payload = transformed_encoded_len
        .checked_add(correction_encoded_len)
        .ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz payload length overflow".to_string())
        })?;
    if payload.len() != expected_payload {
        return Err(ZbitError::Parse(format!(
            "recursive-circuit-xz payload length mismatch: expected {expected_payload} got {}",
            payload.len()
        )));
    }

    let transformed_payload = &payload[..transformed_encoded_len];
    let corrections_payload = &payload[transformed_encoded_len..];
    let transformed = decode_with_codec(transformed_payload, transformed_codec, plain_len)?;
    let correction_transformed =
        decode_with_codec(corrections_payload, correction_codec, correction_plain_len)?;
    let corrections = invert_transform_plan(
        &correction_transformed,
        correction_plain_len,
        &correction_plan,
    )
    .ok_or_else(|| {
        ZbitError::Parse("recursive-circuit-xz correction stream is invalid".to_string())
    })?;
    let plan = CircuitTransformPlan {
        kind: transform_kind,
        period: transform_period,
        head: transform_head,
    };
    let filtered_plain = if block_plans.is_empty() {
        invert_transform_plan(&transformed, plain_len, &plan).ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz transformed stream is invalid".to_string())
        })?
    } else {
        // Multi-block extension: split transformed bytes into block_count consecutive blocks
        // (every non-last block is exactly block_size_field bytes; the last block carries the
        // remainder). Invert each block with its own plan and concatenate.
        let block_size = block_size_field as usize;
        let block_count = block_plans.len();
        let leading_bytes = block_size
            .checked_mul(block_count.saturating_sub(1))
            .ok_or_else(|| {
                ZbitError::Parse(
                    "recursive-circuit-xz multi-block size overflow".to_string(),
                )
            })?;
        if leading_bytes > plain_len {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz multi-block leading bytes exceed plain_len".to_string(),
            ));
        }
        let last_block_len = plain_len - leading_bytes;
        if transformed.len() != plain_len {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz multi-block transformed length mismatch".to_string(),
            ));
        }
        let mut out = Vec::with_capacity(plain_len);
        for (idx, plan) in block_plans.iter().enumerate() {
            let block_start = idx * block_size;
            let block_len = if idx + 1 == block_count {
                last_block_len
            } else {
                block_size
            };
            let block_end = block_start + block_len;
            let block_transformed = transformed
                .get(block_start..block_end)
                .ok_or_else(|| {
                    ZbitError::Parse(
                        "recursive-circuit-xz multi-block range out of bounds".to_string(),
                    )
                })?;
            let block_plain = invert_transform_plan(block_transformed, block_len, plan)
                .ok_or_else(|| {
                    ZbitError::Parse(
                        "recursive-circuit-xz multi-block stream is invalid".to_string(),
                    )
                })?;
            out.extend_from_slice(&block_plain);
        }
        // The single-plan transform_plan field is still embedded but unused in multi-block mode
        // (we keep it filled with the first block's plan for telemetry). Silence the unused
        // binding below.
        let _ = plan;
        out
    };

    let deflate_stream = recreate_whole_deflate_stream(&filtered_plain, &corrections)
        .map_err(|e| ZbitError::Parse(format!("preflate recreate failed: {e}")))?;

    let mut framed_payload = Vec::with_capacity(
        2usize
            .checked_add(deflate_stream.len())
            .and_then(|v| v.checked_add(4))
            .ok_or_else(|| {
                ZbitError::Parse("recursive-circuit-xz framed payload overflow".to_string())
            })?,
    );
    framed_payload.extend_from_slice(&zlib_header);
    framed_payload.extend_from_slice(&deflate_stream);
    framed_payload.extend_from_slice(&zlib_adler32);

    let tail_present = if total_chunks == full_chunk_count {
        false
    } else if total_chunks == full_chunk_count + 1 {
        true
    } else {
        return Err(ZbitError::Parse(
            "recursive-circuit-xz dictionary has inconsistent chunk counters".to_string(),
        ));
    };

    let expected_framed_payload = full_chunk_count
        .checked_mul(base_chunk_len)
        .and_then(|v| {
            if tail_present {
                v.checked_add(tail_chunk_len)
            } else {
                Some(v)
            }
        })
        .ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz framed length overflow".to_string())
        })?;
    if framed_payload.len() != expected_framed_payload {
        return Err(ZbitError::Parse(format!(
            "recursive-circuit-xz framed length mismatch: expected {expected_framed_payload} got {}",
            framed_payload.len()
        )));
    }

    let chunk_overhead = total_chunks.checked_mul(12).ok_or_else(|| {
        ZbitError::Parse("recursive-circuit-xz chunk overhead overflow".to_string())
    })?;
    let mut out = Vec::with_capacity(
        prefix
            .len()
            .checked_add(framed_payload.len())
            .and_then(|v| v.checked_add(suffix.len()))
            .and_then(|v| v.checked_add(chunk_overhead))
            .ok_or_else(|| {
                ZbitError::Parse("recursive-circuit-xz output length overflow".to_string())
            })?,
    );
    out.extend_from_slice(prefix);

    let mut payload_cursor = 0usize;
    for idx in 0..total_chunks {
        let chunk_len = if idx < full_chunk_count {
            base_chunk_len
        } else {
            tail_chunk_len
        };
        let chunk_data = framed_payload
            .get(payload_cursor..payload_cursor + chunk_len)
            .ok_or_else(|| {
                ZbitError::Parse("recursive-circuit-xz frame range out of bounds".to_string())
            })?;
        payload_cursor += chunk_len;

        push_u32_be(&mut out, chunk_len as u32);
        out.extend_from_slice(&frame_tag);
        out.extend_from_slice(chunk_data);

        let mut hasher = Crc32Hasher::new();
        hasher.update(&frame_tag);
        hasher.update(chunk_data);
        push_u32_be(&mut out, hasher.finalize());
    }

    out.extend_from_slice(suffix);

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "recursive-circuit-xz output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

