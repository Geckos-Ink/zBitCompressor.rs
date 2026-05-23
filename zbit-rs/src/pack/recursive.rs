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

fn framed_dictionary_size(stream: &FramedPayloadRun) -> usize {
    28usize + stream.prefix.len() + stream.suffix.len()
}

fn write_framed_dictionary(out: &mut Vec<u8>, stream: &FramedPayloadRun) {
    push_u32(out, stream.prefix.len() as u32);
    push_u32(out, stream.suffix.len() as u32);
    out.extend_from_slice(&stream.frame_tag);
    push_u32(out, stream.base_chunk_len);
    push_u32(out, stream.full_chunk_count);
    push_u32(out, stream.tail_chunk_len);
    push_u32(out, stream.total_chunks);
    out.extend_from_slice(&stream.prefix);
    out.extend_from_slice(&stream.suffix);
}

fn decode_framed_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut dict_cursor = 0usize;
    let prefix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let suffix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tag_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 4)
        .ok_or_else(|| ZbitError::Parse("framed-raw missing frame tag".to_string()))?;
    dict_cursor += 4;
    let frame_tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
    let base_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let full_chunk_count = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tail_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let total_chunks = read_u32(dict_bytes, &mut dict_cursor)? as usize;

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
            // Sort by total candidate cost (payload bytes + per-plan metadata) and keep best.
            results.sort_by_key(|result| {
                result.payload.len()
                    + 4
                    + 4
                    + result.plans.len() * RECURSIVE_BLOCK_PLAN_BYTES
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
        let mb_total =
            mb.payload.len() + 4 + 4 + mb.plans.len() * RECURSIVE_BLOCK_PLAN_BYTES;
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
            multi_block_candidate.as_ref().map(|mb| mb.payload.len()
                + 4
                + 4
                + mb.plans.len() * RECURSIVE_BLOCK_PLAN_BYTES),
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
// Second-highest bit signals the compact (bit-packed) topology serialisation. When set,
// each topology node is written as:
//   flag_byte u8 :  bit 7 = relation, bits 0..6 = order (0..127, typical 0..3)
//   kind     u8 :  the raw u8 from CircuitTopologyNode.kind (0..255 supported)
//   id           :  varint, the node's u32 id
//   parent_plus1 :  varint, `parent_id + 1` for normal parents; 0 = root sentinel
//                   (encodes `u32::MAX` as 0 to avoid a 5-byte varint per root node)
//   param_a      :  varint of the u32 parameter
//   param_b      :  varint of the u32 parameter
// hash64 is NOT serialised in the compact form — overall decode correctness already
// validates the topology end-to-end through the inverse transform pipeline. Legacy
// dictionaries (flag clear) keep the fixed 28-byte-per-node layout including hash64.
const RECURSIVE_TOPOLOGY_COMPACT_FLAG: u16 = 0x4000;
const RECURSIVE_TOPOLOGY_COUNT_MASK: u16 = 0x3FFF;
const RECURSIVE_BLOCK_PLAN_BYTES: usize = 1 + 4 + 4;

fn varint_u64_len(value: u64) -> usize {
    let mut len = 1usize;
    let mut remaining = value >> 7;
    while remaining != 0 {
        len += 1;
        remaining >>= 7;
    }
    len
}

fn compact_topology_node_size(node: &CircuitTopologyNode) -> usize {
    let parent_plus_one = if node.parent_id == u32::MAX {
        0u64
    } else {
        (node.parent_id as u64) + 1
    };
    // 1 flag byte + 1 kind byte + varints
    2 + varint_u64_len(node.id as u64)
        + varint_u64_len(parent_plus_one)
        + varint_u64_len(node.param_a as u64)
        + varint_u64_len(node.param_b as u64)
}

fn compact_topology_total_size(nodes: &[CircuitTopologyNode]) -> usize {
    nodes.iter().map(compact_topology_node_size).sum()
}

// Compact form is used when every node's `order` fits in 7 bits (the relation bit takes
// the high bit). Current topology builders only emit `order` values in 0..3, so this is
// always true; the fall-back keeps the door open for future builders that emit larger
// orders without breaking older readers.
fn compact_topology_eligible(nodes: &[CircuitTopologyNode]) -> bool {
    nodes.iter().all(|node| node.order <= 0x7F)
}

fn recursive_circuit_dictionary_size(stream: &RecursiveCircuitStream) -> usize {
    let use_compact = compact_topology_eligible(&stream.topology);
    let topology_bytes = if use_compact {
        compact_topology_total_size(&stream.topology)
    } else {
        stream.topology.len() * TOPOLOGY_NODE_BYTES
    };
    let mut size = framed_dictionary_size(&stream.base) + 51 + topology_bytes;
    if let Some(mb) = &stream.multi_block {
        // block_count u32 + block_size u32 + per-plan (kind u8 + period u32 + head u32)
        size += 4 + 4 + mb.plans.len() * RECURSIVE_BLOCK_PLAN_BYTES;
    }
    size
}

fn write_recursive_circuit_dictionary(out: &mut Vec<u8>, stream: &RecursiveCircuitStream) {
    write_framed_dictionary(out, &stream.base);
    out.extend_from_slice(&stream.zlib_header);
    out.extend_from_slice(&stream.zlib_adler32);
    push_u64(out, stream.plain_len as u64);
    push_u64(out, stream.transformed_encoded_len as u64);
    push_u64(out, stream.correction_plain_len as u64);
    push_u64(out, stream.correction_encoded_len as u64);
    out.push(stream.transformed_codec.as_u8());
    out.push(stream.correction_codec.as_u8());
    out.push(stream.transform_plan.kind.as_u8());
    push_u32(out, stream.transform_plan.period);
    push_u32(out, stream.transform_plan.head);
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
        for node in &stream.topology {
            let order_low = (node.order & 0x7F) as u8;
            let flag_byte = (node.relation << 7) | order_low;
            out.push(flag_byte);
            out.push(node.kind);
            push_varint_u64(out, node.id as u64);
            let parent_plus_one = if node.parent_id == u32::MAX {
                0u64
            } else {
                (node.parent_id as u64) + 1
            };
            push_varint_u64(out, parent_plus_one);
            push_varint_u64(out, node.param_a as u64);
            push_varint_u64(out, node.param_b as u64);
        }
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
        push_u32(out, mb.plans.len() as u32);
        push_u32(out, mb.block_size);
        for plan in &mb.plans {
            out.push(plan.kind.as_u8());
            push_u32(out, plan.period);
            push_u32(out, plan.head);
        }
    }
}

fn decode_recursive_circuit_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut dict_cursor = 0usize;
    let prefix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let suffix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tag_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 4)
        .ok_or_else(|| ZbitError::Parse("recursive-circuit-xz missing frame tag".to_string()))?;
    dict_cursor += 4;
    let frame_tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
    let base_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let full_chunk_count = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tail_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let total_chunks = read_u32(dict_bytes, &mut dict_cursor)? as usize;

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

    let plain_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let transformed_encoded_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let correction_plain_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let correction_encoded_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let transformed_codec = PayloadCodec::from_u8(read_u8(dict_bytes, &mut dict_cursor)?)
        .ok_or_else(|| {
            ZbitError::Parse(
                "recursive-circuit-xz dictionary has invalid transformed codec".to_string(),
            )
        })?;
    let correction_codec = PayloadCodec::from_u8(read_u8(dict_bytes, &mut dict_cursor)?)
        .ok_or_else(|| {
            ZbitError::Parse(
                "recursive-circuit-xz dictionary has invalid correction codec".to_string(),
            )
        })?;
    let transform_kind_u8 = read_u8(dict_bytes, &mut dict_cursor)?;
    let transform_kind = CircuitTransformKind::from_u8(transform_kind_u8).ok_or_else(|| {
        ZbitError::Parse("recursive-circuit-xz dictionary has invalid transform kind".to_string())
    })?;
    let transform_period = read_u32(dict_bytes, &mut dict_cursor)?;
    let transform_head = read_u32(dict_bytes, &mut dict_cursor)?;
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
    for idx in 0..topology_count {
        let (id, parent_id, relation, _order, kind, param_a, param_b) = if compact_topology {
            let flag_byte = read_u8(dict_bytes, &mut dict_cursor)?;
            let relation = (flag_byte >> 7) & 0x01;
            let order_u16 = (flag_byte & 0x7F) as u16;
            let kind_byte = read_u8(dict_bytes, &mut dict_cursor)?;
            let id_u64 = read_varint_u64(dict_bytes, &mut dict_cursor)?;
            let parent_plus_one = read_varint_u64(dict_bytes, &mut dict_cursor)?;
            let param_a_u64 = read_varint_u64(dict_bytes, &mut dict_cursor)?;
            let param_b_u64 = read_varint_u64(dict_bytes, &mut dict_cursor)?;
            if id_u64 > u32::MAX as u64 {
                return Err(ZbitError::Parse(
                    "compact recursive-circuit-xz topology id exceeds u32".to_string(),
                ));
            }
            let parent_id_value = if parent_plus_one == 0 {
                u32::MAX
            } else {
                let p = parent_plus_one - 1;
                if p > u32::MAX as u64 {
                    return Err(ZbitError::Parse(
                        "compact recursive-circuit-xz topology parent id exceeds u32"
                            .to_string(),
                    ));
                }
                p as u32
            };
            if param_a_u64 > u32::MAX as u64 || param_b_u64 > u32::MAX as u64 {
                return Err(ZbitError::Parse(
                    "compact recursive-circuit-xz topology param exceeds u32".to_string(),
                ));
            }
            (
                id_u64 as u32,
                parent_id_value,
                relation,
                order_u16,
                kind_byte,
                param_a_u64 as u32,
                param_b_u64 as u32,
            )
        } else {
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
            (id, parent_id, relation, order, kind, param_a, param_b)
        };

        if relation > 1 {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz topology relation must be 0 or 1".to_string(),
            ));
        }
        if idx > 0 && id <= last_id {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz topology node ids must be strictly increasing".to_string(),
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

    let mut block_plans: Vec<CircuitTransformPlan> = Vec::new();
    let mut block_size_field = 0u32;
    if multi_block_present {
        let block_count = read_u32(dict_bytes, &mut dict_cursor)? as usize;
        block_size_field = read_u32(dict_bytes, &mut dict_cursor)?;
        if block_count == 0 {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz multi-block extension claims zero blocks".to_string(),
            ));
        }
        if block_size_field == 0 {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz multi-block extension has zero block size".to_string(),
            ));
        }
        block_plans.reserve(block_count);
        for _ in 0..block_count {
            let kind_u8 = read_u8(dict_bytes, &mut dict_cursor)?;
            let kind = CircuitTransformKind::from_u8(kind_u8).ok_or_else(|| {
                ZbitError::Parse(
                    "recursive-circuit-xz multi-block plan has invalid transform kind".to_string(),
                )
            })?;
            let period = read_u32(dict_bytes, &mut dict_cursor)?;
            let head = read_u32(dict_bytes, &mut dict_cursor)?;
            block_plans.push(CircuitTransformPlan { kind, period, head });
        }
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

