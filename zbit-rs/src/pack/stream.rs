// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

fn compress_stream_realtime_pack_bytes(
    input: &[u8],
    allow_recursive_candidate: bool,
    context: &mut CompressionContext,
) -> ZbitResult<(Vec<u8>, PackMethod)> {
    let raw_deflate_payload = build_raw_deflate_payload_with_compression(
        input,
        context.profile.realtime_deflate_compression(),
    )?;
    let raw_zstd_payload =
        build_raw_zstd_payload_with_level(input, context.profile.realtime_zstd_level())?;

    let empty_stream = IndexStream {
        unique_symbols: Vec::new(),
        bits_per_symbol: 0,
        payload: Vec::new(),
        frequencies: [0u32; 256],
    };

    let raw_copy_bytes = write_pack_bytes(
        PackMethod::RawCopy,
        input,
        &empty_stream,
        None,
        0,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    let raw_deflate_bytes = write_pack_bytes(
        PackMethod::RawDeflate,
        input,
        &empty_stream,
        None,
        0,
        None,
        Some(raw_deflate_payload.as_slice()),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    let raw_zstd_bytes = write_pack_bytes(
        PackMethod::RawZstd,
        input,
        &empty_stream,
        None,
        0,
        None,
        None,
        Some(raw_zstd_payload.as_slice()),
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    let mut candidates = vec![
        (PackMethod::RawCopy, raw_copy_bytes),
        (PackMethod::RawDeflate, raw_deflate_bytes),
        (PackMethod::RawZstd, raw_zstd_bytes),
    ];

    if let Some(framed_run) = build_framed_payload_run(input) {
        let framed_bytes = write_pack_bytes(
            PackMethod::FramedRaw,
            input,
            &empty_stream,
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            Some(&framed_run),
            None,
            None,
            None,
        )?;
        candidates.push((PackMethod::FramedRaw, framed_bytes));

        if allow_recursive_candidate
            && context.profile.should_attempt_recursive_on_realtime_blocks()
            && input.len() >= (512 * 1024)
        {
            if let Some(recursive_stream) =
                build_recursive_circuit_stream(input, &framed_run, context)?
            {
                let recursive_bytes = write_pack_bytes(
                    PackMethod::RecursiveCircuitXz,
                    input,
                    &empty_stream,
                    None,
                    0,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(&recursive_stream),
                    None,
                    None,
                )?;
                let validation_timer = Instant::now();
                if let Ok(decoded) = decompress_pack_bytes(&recursive_bytes) {
                    if decoded == input {
                        candidates.push((PackMethod::RecursiveCircuitXz, recursive_bytes));
                    }
                }
                context.timings.candidate_validation_ms +=
                    validation_timer.elapsed().as_secs_f64() * 1000.0;
            }
        } else if allow_recursive_candidate
            && !context.profile.should_attempt_recursive_on_realtime_blocks()
        {
            context.push_skipped(format!(
                "stream recursive realtime candidate skipped by '{}' profile",
                context.profile.name()
            ));
        }
    }

    candidates.sort_by_key(|(_, bytes)| bytes.len());
    let (method, bytes) = candidates
        .into_iter()
        .next()
        .ok_or_else(|| ZbitError::Internal("missing stream realtime pack candidate".to_string()))?;
    Ok((bytes, method))
}

fn compress_stream_wide_overfit_pack_bytes(
    input: &[u8],
    allow_recursive_candidate: bool,
    context: &mut CompressionContext,
) -> ZbitResult<(Vec<u8>, PackMethod)> {
    let raw_deflate_payload = build_raw_deflate_payload_with_compression(
        input,
        context.profile.realtime_deflate_compression(),
    )?;
    let raw_zstd_payload =
        build_raw_zstd_payload_with_level(input, context.profile.realtime_zstd_level())?;

    let empty_stream = IndexStream {
        unique_symbols: Vec::new(),
        bits_per_symbol: 0,
        payload: Vec::new(),
        frequencies: [0u32; 256],
    };

    let raw_copy_bytes = write_pack_bytes(
        PackMethod::RawCopy,
        input,
        &empty_stream,
        None,
        0,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;
    let raw_deflate_bytes = write_pack_bytes(
        PackMethod::RawDeflate,
        input,
        &empty_stream,
        None,
        0,
        None,
        Some(raw_deflate_payload.as_slice()),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;
    let raw_zstd_bytes = write_pack_bytes(
        PackMethod::RawZstd,
        input,
        &empty_stream,
        None,
        0,
        None,
        None,
        Some(raw_zstd_payload.as_slice()),
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    let mut candidates = vec![
        (PackMethod::RawCopy, raw_copy_bytes),
        (PackMethod::RawDeflate, raw_deflate_bytes),
        (PackMethod::RawZstd, raw_zstd_bytes),
    ];

    if let Some(framed_run) = build_framed_payload_run(input) {
        let framed_bytes = write_pack_bytes(
            PackMethod::FramedRaw,
            input,
            &empty_stream,
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            Some(&framed_run),
            None,
            None,
            None,
        )?;
        candidates.push((PackMethod::FramedRaw, framed_bytes));

        if allow_recursive_candidate {
            if let Some(recursive_stream) =
                build_recursive_circuit_stream(input, &framed_run, context)?
            {
                let recursive_bytes = write_pack_bytes(
                    PackMethod::RecursiveCircuitXz,
                    input,
                    &empty_stream,
                    None,
                    0,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(&recursive_stream),
                    None,
                    None,
                )?;
                let validation_timer = Instant::now();
                if let Ok(decoded) = decompress_pack_bytes(&recursive_bytes) {
                    if decoded == input {
                        candidates.push((PackMethod::RecursiveCircuitXz, recursive_bytes));
                    }
                }
                context.timings.candidate_validation_ms +=
                    validation_timer.elapsed().as_secs_f64() * 1000.0;
            }
        } else {
            context.push_skipped(format!(
                "stream wide/global recursive candidate skipped by '{}' profile",
                context.profile.name()
            ));
        }
    }

    candidates.sort_by_key(|(_, bytes)| bytes.len());
    let (method, bytes) = candidates.into_iter().next().ok_or_else(|| {
        ZbitError::Internal("missing stream wide-overfit pack candidate".to_string())
    })?;
    Ok((bytes, method))
}

fn compress_stream_range_pack_bytes(
    input: &[u8],
    realtime_mode: bool,
    allow_recursive_candidate: bool,
    context: &mut CompressionContext,
) -> ZbitResult<(Vec<u8>, PackMethod)> {
    if realtime_mode {
        compress_stream_realtime_pack_bytes(input, allow_recursive_candidate, context)
    } else {
        let (pack_bytes, stats) = compress_adaptive_to_bytes(input)?;
        Ok((pack_bytes, stats.chosen_method))
    }
}

fn stream_pack_range_candidate(
    input_hash: PayloadHash,
    chunks: &[Vec<u8>],
    absolute_chunk_base: usize,
    start: usize,
    end: usize,
    realtime_mode: bool,
    enable_realtime_root_recursive_candidate: bool,
    context: &mut CompressionContext,
) -> ZbitResult<PackedRangeCandidate> {
    let abs_start_chunk = absolute_chunk_base
        .checked_add(start)
        .ok_or_else(|| ZbitError::Limit("stream range absolute start overflow".to_string()))?;
    let abs_end_chunk = absolute_chunk_base
        .checked_add(end)
        .ok_or_else(|| ZbitError::Limit("stream range absolute end overflow".to_string()))?;
    let original_size = chunks[start..end]
        .iter()
        .map(|chunk| chunk.len())
        .sum::<usize>();
    let is_block_root = start == 0 && end == chunks.len();
    let allow_recursive_candidate = enable_realtime_root_recursive_candidate
        && is_block_root
        && original_size >= (512 * 1024);
    let cache_key = StreamRangeCacheKey {
        input_hash,
        abs_start_chunk,
        abs_end_chunk,
        realtime_mode,
        allow_recursive_candidate,
        profile: context.profile,
    };
    if let Some(cached) = context.cache.stream_range_candidates.get(&cache_key) {
        context.cache_stats.stream_range_hits =
            context.cache_stats.stream_range_hits.saturating_add(1);
        return Ok(cached.clone());
    }
    context.cache_stats.stream_range_misses = context.cache_stats.stream_range_misses.saturating_add(1);

    let mut merged = Vec::with_capacity(original_size);
    for chunk in &chunks[start..end] {
        merged.extend_from_slice(chunk);
    }

    let (pack_bytes, method) = compress_stream_range_pack_bytes(
        &merged,
        realtime_mode,
        allow_recursive_candidate,
        context,
    )?;
    let candidate = PackedRangeCandidate {
        pack_bytes,
        method,
        original_size,
    };
    context
        .cache
        .stream_range_candidates
        .insert(cache_key, candidate.clone());
    Ok(candidate)
}

fn stream_piece_node(
    chunk_len: usize,
    method: PackMethod,
    pack_bytes: Vec<u8>,
) -> ZbitResult<StreamNode> {
    let encoded_size = 1usize
        .checked_add(4)
        .and_then(|v| v.checked_add(1))
        .and_then(|v| v.checked_add(4))
        .and_then(|v| v.checked_add(pack_bytes.len()))
        .ok_or_else(|| ZbitError::Limit("stream piece node size overflow".to_string()))?;

    Ok(StreamNode {
        kind: StreamNodeKind::Piece {
            chunk_len,
            method,
            pack_bytes,
        },
        chunk_count: 1,
        original_len: chunk_len,
        encoded_size,
    })
}

fn stream_group_node(
    chunk_count: usize,
    original_len: usize,
    method: PackMethod,
    pack_bytes: Vec<u8>,
) -> ZbitResult<StreamNode> {
    let encoded_size = 1usize
        .checked_add(4)
        .and_then(|v| v.checked_add(8))
        .and_then(|v| v.checked_add(1))
        .and_then(|v| v.checked_add(4))
        .and_then(|v| v.checked_add(pack_bytes.len()))
        .ok_or_else(|| ZbitError::Limit("stream group node size overflow".to_string()))?;

    Ok(StreamNode {
        kind: StreamNodeKind::Group {
            chunk_count,
            original_len,
            method,
            pack_bytes,
        },
        chunk_count,
        original_len,
        encoded_size,
    })
}

fn stream_global_slice_node(
    chunk_count: usize,
    original_offset: usize,
    original_len: usize,
) -> ZbitResult<StreamNode> {
    let encoded_size = 1usize
        .checked_add(4)
        .and_then(|v| v.checked_add(8))
        .and_then(|v| v.checked_add(8))
        .ok_or_else(|| ZbitError::Limit("stream global-slice node size overflow".to_string()))?;

    Ok(StreamNode {
        kind: StreamNodeKind::GlobalSlice {
            chunk_count,
            original_offset,
            original_len,
        },
        chunk_count,
        original_len,
        encoded_size,
    })
}

fn stream_split_node(level: u8, left: StreamNode, right: StreamNode) -> ZbitResult<StreamNode> {
    let chunk_count = left
        .chunk_count
        .checked_add(right.chunk_count)
        .ok_or_else(|| ZbitError::Limit("stream split chunk count overflow".to_string()))?;
    let original_len = left
        .original_len
        .checked_add(right.original_len)
        .ok_or_else(|| ZbitError::Limit("stream split length overflow".to_string()))?;
    let encoded_size = 1usize
        .checked_add(1)
        .and_then(|v| v.checked_add(left.encoded_size))
        .and_then(|v| v.checked_add(right.encoded_size))
        .ok_or_else(|| ZbitError::Limit("stream split node size overflow".to_string()))?;

    Ok(StreamNode {
        kind: StreamNodeKind::Split {
            level,
            left: Box::new(left),
            right: Box::new(right),
        },
        chunk_count,
        original_len,
        encoded_size,
    })
}

fn build_best_stream_node(
    chunks: &[Vec<u8>],
    start: usize,
    end: usize,
    absolute_chunk_base: usize,
    depth_remaining: u8,
    max_group_pieces: usize,
    realtime_mode: bool,
    enable_realtime_root_recursive_candidate: bool,
    input_hash: PayloadHash,
    context: &mut CompressionContext,
    memo: &mut HashMap<(usize, usize, u8), StreamNode>,
) -> ZbitResult<StreamNode> {
    if let Some(cached) = memo.get(&(start, end, depth_remaining)) {
        return Ok(cached.clone());
    }

    let chunk_span = end.saturating_sub(start);
    if chunk_span == 0 {
        return Err(ZbitError::Internal(
            "stream node builder called with empty range".to_string(),
        ));
    }

    if chunk_span == 1 {
        let candidate = stream_pack_range_candidate(
            input_hash,
            chunks,
            absolute_chunk_base,
            start,
            end,
            realtime_mode,
            enable_realtime_root_recursive_candidate,
            context,
        )?;
        let node = stream_piece_node(
            chunks[start].len(),
            candidate.method,
            candidate.pack_bytes.clone(),
        )?;
        memo.insert((start, end, depth_remaining), node.clone());
        return Ok(node);
    }

    let child_depth = if depth_remaining > 0 {
        depth_remaining - 1
    } else {
        0
    };

    let mut best_split: Option<StreamNode> = None;
    for mid in context.profile.stream_split_points(start, end) {
        let left = build_best_stream_node(
            chunks,
            start,
            mid,
            absolute_chunk_base,
            child_depth,
            max_group_pieces,
            realtime_mode,
            enable_realtime_root_recursive_candidate,
            input_hash,
            context,
            memo,
        )?;
        let right = build_best_stream_node(
            chunks,
            mid,
            end,
            absolute_chunk_base,
            child_depth,
            max_group_pieces,
            realtime_mode,
            enable_realtime_root_recursive_candidate,
            input_hash,
            context,
            memo,
        )?;
        let split = stream_split_node(depth_remaining, left, right)?;
        match &best_split {
            Some(best) if split.encoded_size >= best.encoded_size => {}
            _ => best_split = Some(split),
        }
    }

    let mut best = best_split
        .ok_or_else(|| ZbitError::Internal("failed to build stream split candidate".to_string()))?;

    let is_block_root = start == 0 && end == chunks.len();
    let allow_group_candidate = depth_remaining > 0
        && chunk_span <= max_group_pieces
        && (chunk_span <= 2 || is_block_root || chunk_span == max_group_pieces);
    if allow_group_candidate {
        let grouped_candidate = stream_pack_range_candidate(
            input_hash,
            chunks,
            absolute_chunk_base,
            start,
            end,
            realtime_mode,
            enable_realtime_root_recursive_candidate,
            context,
        )?;
        let grouped = stream_group_node(
            chunk_span,
            grouped_candidate.original_size,
            grouped_candidate.method,
            grouped_candidate.pack_bytes.clone(),
        )?;
        if grouped.encoded_size <= best.encoded_size {
            best = grouped;
        }
    }

    memo.insert((start, end, depth_remaining), best.clone());
    Ok(best)
}

fn collect_stream_node_counts(node: &StreamNode, depth: u8, counts: &mut StreamNodeCounts) {
    counts.max_depth_used = counts.max_depth_used.max(depth);
    match &node.kind {
        StreamNodeKind::Piece { .. } => counts.piece_nodes += 1,
        StreamNodeKind::Group { .. } => counts.grouped_nodes += 1,
        StreamNodeKind::GlobalSlice { .. } => counts.grouped_nodes += 1,
        StreamNodeKind::Split { left, right, .. } => {
            counts.split_nodes += 1;
            collect_stream_node_counts(left, depth.saturating_add(1), counts);
            collect_stream_node_counts(right, depth.saturating_add(1), counts);
        }
    }
}

fn stream_node_history_method(node: &StreamNode) -> PackMethod {
    match &node.kind {
        StreamNodeKind::Piece { method, .. } | StreamNodeKind::Group { method, .. } => *method,
        StreamNodeKind::GlobalSlice { .. } => PackMethod::RecursiveCircuitXz,
        StreamNodeKind::Split { left, right, .. } => {
            if left.original_len >= right.original_len {
                stream_node_history_method(left)
            } else {
                stream_node_history_method(right)
            }
        }
    }
}

fn write_stream_node(out: &mut Vec<u8>, node: &StreamNode) -> ZbitResult<()> {
    match &node.kind {
        StreamNodeKind::Piece {
            chunk_len,
            method,
            pack_bytes,
        } => {
            out.push(ZBPS_NODE_KIND_PIECE);
            push_u32(out, usize_to_u32(*chunk_len, "stream chunk_len")?);
            out.push(method.as_u8());
            push_u32(
                out,
                usize_to_u32(pack_bytes.len(), "stream piece pack bytes")?,
            );
            out.extend_from_slice(pack_bytes);
        }
        StreamNodeKind::Group {
            chunk_count,
            original_len,
            method,
            pack_bytes,
        } => {
            out.push(ZBPS_NODE_KIND_GROUP);
            push_u32(
                out,
                usize_to_u32(*chunk_count, "stream grouped chunk_count")?,
            );
            push_u64(
                out,
                usize_to_u64(*original_len, "stream grouped original_len")?,
            );
            out.push(method.as_u8());
            push_u32(
                out,
                usize_to_u32(pack_bytes.len(), "stream group pack bytes")?,
            );
            out.extend_from_slice(pack_bytes);
        }
        StreamNodeKind::Split { level, left, right } => {
            out.push(ZBPS_NODE_KIND_SPLIT);
            out.push(*level);
            write_stream_node(out, left)?;
            write_stream_node(out, right)?;
        }
        StreamNodeKind::GlobalSlice {
            chunk_count,
            original_offset,
            original_len,
        } => {
            out.push(ZBPS_NODE_KIND_GLOBAL_SLICE);
            push_u32(
                out,
                usize_to_u32(*chunk_count, "stream global-slice chunk_count")?,
            );
            push_u64(
                out,
                usize_to_u64(*original_offset, "stream global-slice original_offset")?,
            );
            push_u64(
                out,
                usize_to_u64(*original_len, "stream global-slice original_len")?,
            );
        }
    }
    Ok(())
}

fn decode_stream_node(
    node_bytes: &[u8],
    cursor: &mut usize,
    global_output: Option<&[u8]>,
) -> ZbitResult<StreamDecodedNode> {
    let kind = read_u8(node_bytes, cursor)?;
    match kind {
        ZBPS_NODE_KIND_PIECE => {
            let chunk_len = read_u32(node_bytes, cursor)? as usize;
            let expected_method = PackMethod::from_u8(read_u8(node_bytes, cursor)?)
                .ok_or_else(|| ZbitError::Parse("invalid stream piece method id".to_string()))?;
            let pack_len = read_u32(node_bytes, cursor)? as usize;
            let pack_bytes = node_bytes.get(*cursor..*cursor + pack_len).ok_or_else(|| {
                ZbitError::Parse("stream piece pack range out of bounds".to_string())
            })?;
            *cursor += pack_len;

            let output = decompress_pack_bytes(pack_bytes)?;
            if output.len() != chunk_len {
                return Err(ZbitError::Parse(format!(
                    "stream piece output length mismatch: expected {chunk_len}, got {}",
                    output.len()
                )));
            }

            let actual_method = PackMethod::from_u8(*pack_bytes.get(8).ok_or_else(|| {
                ZbitError::Parse("stream piece packed header too short".to_string())
            })?)
            .ok_or_else(|| ZbitError::Parse("stream piece packed method id invalid".to_string()))?;
            if actual_method != expected_method {
                return Err(ZbitError::Parse(format!(
                    "stream piece method mismatch: expected {}, got {}",
                    expected_method.name(),
                    actual_method.name()
                )));
            }

            Ok(StreamDecodedNode {
                bytes: output,
                chunk_count: 1,
            })
        }
        ZBPS_NODE_KIND_GROUP => {
            let chunk_count = read_u32(node_bytes, cursor)? as usize;
            let original_len =
                u64_to_usize(read_u64(node_bytes, cursor)?, "stream grouped original_len")?;
            let expected_method = PackMethod::from_u8(read_u8(node_bytes, cursor)?)
                .ok_or_else(|| ZbitError::Parse("invalid stream group method id".to_string()))?;
            let pack_len = read_u32(node_bytes, cursor)? as usize;
            let pack_bytes = node_bytes.get(*cursor..*cursor + pack_len).ok_or_else(|| {
                ZbitError::Parse("stream group pack range out of bounds".to_string())
            })?;
            *cursor += pack_len;

            let output = decompress_pack_bytes(pack_bytes)?;
            if output.len() != original_len {
                return Err(ZbitError::Parse(format!(
                    "stream group output length mismatch: expected {original_len}, got {}",
                    output.len()
                )));
            }

            let actual_method = PackMethod::from_u8(*pack_bytes.get(8).ok_or_else(|| {
                ZbitError::Parse("stream group packed header too short".to_string())
            })?)
            .ok_or_else(|| ZbitError::Parse("stream group packed method id invalid".to_string()))?;
            if actual_method != expected_method {
                return Err(ZbitError::Parse(format!(
                    "stream group method mismatch: expected {}, got {}",
                    expected_method.name(),
                    actual_method.name()
                )));
            }

            Ok(StreamDecodedNode {
                bytes: output,
                chunk_count,
            })
        }
        ZBPS_NODE_KIND_SPLIT => {
            let _level = read_u8(node_bytes, cursor)?;
            let left = decode_stream_node(node_bytes, cursor, global_output)?;
            let right = decode_stream_node(node_bytes, cursor, global_output)?;

            let mut bytes = Vec::with_capacity(
                left.bytes
                    .len()
                    .checked_add(right.bytes.len())
                    .ok_or_else(|| ZbitError::Limit("stream split output overflow".to_string()))?,
            );
            bytes.extend_from_slice(&left.bytes);
            bytes.extend_from_slice(&right.bytes);

            let chunk_count = left
                .chunk_count
                .checked_add(right.chunk_count)
                .ok_or_else(|| ZbitError::Limit("stream split chunk count overflow".to_string()))?;

            Ok(StreamDecodedNode { bytes, chunk_count })
        }
        ZBPS_NODE_KIND_GLOBAL_SLICE => {
            let chunk_count = read_u32(node_bytes, cursor)? as usize;
            let original_offset = u64_to_usize(
                read_u64(node_bytes, cursor)?,
                "stream global-slice original_offset",
            )?;
            let original_len = u64_to_usize(
                read_u64(node_bytes, cursor)?,
                "stream global-slice original_len",
            )?;

            let global = global_output.ok_or_else(|| {
                ZbitError::Parse(
                    "stream global-slice node missing global overfit payload".to_string(),
                )
            })?;

            let range_end = original_offset.checked_add(original_len).ok_or_else(|| {
                ZbitError::Parse("stream global-slice range overflow".to_string())
            })?;
            let bytes = global
                .get(original_offset..range_end)
                .ok_or_else(|| {
                    ZbitError::Parse("stream global-slice range out of bounds".to_string())
                })?
                .to_vec();

            Ok(StreamDecodedNode { bytes, chunk_count })
        }
        _ => Err(ZbitError::Parse(format!(
            "invalid stream node kind: {kind}"
        ))),
    }
}

fn parse_stream_header(bytes: &[u8], cursor: &mut usize) -> ZbitResult<StreamHeader> {
    let magic = read_u32(bytes, cursor)?;
    if magic != ZBPS_MAGIC {
        return Err(ZbitError::Parse("invalid ZBPS magic".to_string()));
    }

    let version = read_u16(bytes, cursor)?;
    if version != ZBPS_VERSION {
        return Err(ZbitError::Parse(format!(
            "unsupported ZBPS version: {version}"
        )));
    }

    let flags = read_u16(bytes, cursor)?;
    if flags
        & !(ZBPS_FLAG_CARRY_GROUPING_HISTORY
            | ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS
            | ZBPS_FLAG_SHARED_GROUPING_PAYLOAD)
        != 0
    {
        return Err(ZbitError::Parse(format!(
            "unsupported ZBPS flags: 0x{flags:04x}"
        )));
    }

    let chunk_size = read_u32(bytes, cursor)? as usize;
    let key_piece_interval = read_u32(bytes, cursor)? as usize;
    let _max_group_depth = read_u8(bytes, cursor)?;
    let _reserved0 = read_u8(bytes, cursor)?;
    let _reserved1 = read_u8(bytes, cursor)?;
    let _reserved2 = read_u8(bytes, cursor)?;
    let max_group_pieces = read_u32(bytes, cursor)? as usize;
    let original_size = u64_to_usize(read_u64(bytes, cursor)?, "stream original_size")?;
    let total_chunks = read_u32(bytes, cursor)? as usize;
    let block_count = read_u32(bytes, cursor)? as usize;

    if chunk_size == 0 {
        return Err(ZbitError::Parse(
            "stream chunk_size must be > 0".to_string(),
        ));
    }
    if key_piece_interval == 0 {
        return Err(ZbitError::Parse(
            "stream key_piece_interval must be > 0".to_string(),
        ));
    }
    if max_group_pieces == 0 {
        return Err(ZbitError::Parse(
            "stream max_group_pieces must be > 0".to_string(),
        ));
    }
    if original_size > ZBPK_MAX_OUTPUT_BYTES {
        return Err(ZbitError::Parse(format!(
            "stream original_size exceeds safety bound ({ZBPK_MAX_OUTPUT_BYTES} bytes)"
        )));
    }

    let expected_chunks = expected_chunk_count(original_size, chunk_size);
    if expected_chunks != total_chunks {
        return Err(ZbitError::Parse(format!(
            "stream total_chunks mismatch: header={total_chunks}, expected={expected_chunks}"
        )));
    }
    let expected_blocks = if total_chunks == 0 {
        0
    } else {
        (total_chunks + key_piece_interval - 1) / key_piece_interval
    };
    if expected_blocks != block_count {
        return Err(ZbitError::Parse(format!(
            "stream block_count mismatch: header={block_count}, expected={expected_blocks}"
        )));
    }

    Ok(StreamHeader {
        flags,
        chunk_size,
        key_piece_interval,
        original_size,
        total_chunks,
        block_count,
    })
}

fn compress_stream_to_bytes(
    input: &[u8],
    options: &StreamPackOptions,
) -> ZbitResult<(Vec<u8>, StreamPackStats)> {
    let profile = CompressionProfile::from_env();
    let budgets = CompressionBudgets::from_profile(profile);
    if budgets.max_parallel_codec_threads != 0 {
        if let Ok(pool) = rayon::ThreadPoolBuilder::new()
            .num_threads(budgets.max_parallel_codec_threads)
            .build()
        {
            return pool.install(move || compress_stream_to_bytes_inner(input, options, profile));
        }
    }
    compress_stream_to_bytes_inner(input, options, profile)
}

fn compress_stream_to_bytes_inner(
    input: &[u8],
    options: &StreamPackOptions,
    profile: CompressionProfile,
) -> ZbitResult<(Vec<u8>, StreamPackStats)> {
    validate_stream_options(options)?;
    let total_timer = Instant::now();
    let mut context = CompressionContext::new(profile);

    let chunks = split_stream_chunks(input, options.chunk_size);
    let total_chunks = chunks.len();
    let block_count = if total_chunks == 0 {
        0
    } else {
        (total_chunks + options.key_piece_interval - 1) / options.key_piece_interval
    };

    let mut flags = 0u16;
    if options.carry_grouping_history {
        flags |= ZBPS_FLAG_CARRY_GROUPING_HISTORY;
    }
    if options.wide_overfitting_circuits {
        flags |= ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS;
    }
    let input_hash = payload_hash(input);

    let shared_grouping_payload_bytes = if !options.wide_overfitting_circuits
        && options.realtime_mode
        && block_count > 1
        && input.len() >= (1024 * 1024)
        && context.profile.should_attempt_stream_shared_grouping_payload()
    {
        let shared_timer = Instant::now();
        let (pack_bytes, _) = compress_stream_wide_overfit_pack_bytes(
            input,
            context
                .profile
                .should_attempt_recursive_on_stream_shared_payload(),
            &mut context,
        )?;
        match decompress_pack_bytes(&pack_bytes) {
            Ok(decoded) if decoded == input => {
                flags |= ZBPS_FLAG_SHARED_GROUPING_PAYLOAD;
                context.timings.stream_global_payload_ms +=
                    shared_timer.elapsed().as_secs_f64() * 1000.0;
                Some(pack_bytes)
            }
            _ => {
                context.push_skipped(
                    "shared grouping payload rejected: roundtrip validation failed",
                );
                context.timings.stream_global_payload_ms +=
                    shared_timer.elapsed().as_secs_f64() * 1000.0;
                None
            }
        }
    } else {
        if !options.wide_overfitting_circuits
            && options.realtime_mode
            && block_count > 1
            && input.len() >= (1024 * 1024)
            && !context.profile.should_attempt_stream_shared_grouping_payload()
        {
            context.push_skipped(format!(
                "shared grouping payload skipped by '{}' profile",
                context.profile.name()
            ));
        }
        None
    };

    if options.wide_overfitting_circuits {
        let global_timer = Instant::now();
        let (global_pack_bytes, global_method) = compress_stream_wide_overfit_pack_bytes(
            input,
            true,
            &mut context,
        )?;
        context.timings.stream_global_payload_ms += global_timer.elapsed().as_secs_f64() * 1000.0;
        let mut out = Vec::with_capacity(
            ZBPS_HEADER_BYTES
                .checked_add(4)
                .and_then(|v| v.checked_add(global_pack_bytes.len()))
                .and_then(|v| {
                    v.checked_add(
                        block_count
                            .checked_mul(
                                ZBPS_BLOCK_HEADER_BYTES
                                    .checked_add(1 + 4 + 8 + 8)
                                    .unwrap_or(0),
                            )
                            .unwrap_or(0),
                    )
                })
                .ok_or_else(|| {
                    ZbitError::Limit("stream wide-overfit output size overflow".to_string())
                })?,
        );

        push_u32(&mut out, ZBPS_MAGIC);
        push_u16(&mut out, ZBPS_VERSION);
        push_u16(&mut out, flags);
        push_u32(
            &mut out,
            usize_to_u32(options.chunk_size, "stream chunk_size")?,
        );
        push_u32(
            &mut out,
            usize_to_u32(options.key_piece_interval, "stream key_piece_interval")?,
        );
        out.push(options.max_group_depth);
        out.extend_from_slice(&[0u8; 3]);
        push_u32(
            &mut out,
            usize_to_u32(options.max_group_pieces, "stream max_group_pieces")?,
        );
        push_u64(
            &mut out,
            usize_to_u64(input.len(), "stream original input size")?,
        );
        push_u32(&mut out, usize_to_u32(total_chunks, "stream total_chunks")?);
        push_u32(&mut out, usize_to_u32(block_count, "stream block_count")?);

        push_u32(
            &mut out,
            usize_to_u32(
                global_pack_bytes.len(),
                "stream wide-overfit global pack bytes length",
            )?,
        );
        out.extend_from_slice(&global_pack_bytes);

        let mut counts = StreamNodeCounts::default();
        let mut grouping_hint_updates = 0usize;
        let mut history_method: Option<PackMethod> = None;

        for block_index in 0..block_count {
            let block_start = block_index * options.key_piece_interval;
            let block_end = (block_start + options.key_piece_interval).min(total_chunks);
            let block_chunk_count = block_end - block_start;
            let block_original_len = expected_stream_block_len(
                input.len(),
                options.chunk_size,
                total_chunks,
                block_start,
                block_chunk_count,
            )?;
            let block_offset = block_start.checked_mul(options.chunk_size).ok_or_else(|| {
                ZbitError::Limit("stream wide-overfit block offset overflow".to_string())
            })?;

            let root =
                stream_global_slice_node(block_chunk_count, block_offset, block_original_len)?;
            collect_stream_node_counts(&root, 0, &mut counts);

            let mut node_bytes = Vec::with_capacity(root.encoded_size);
            write_stream_node(&mut node_bytes, &root)?;
            if node_bytes.len() != root.encoded_size {
                return Err(ZbitError::Internal(format!(
                    "stream wide-overfit node encoded size mismatch: expected {}, got {}",
                    root.encoded_size,
                    node_bytes.len()
                )));
            }

            push_u32(
                &mut out,
                usize_to_u32(block_start, "stream block first_chunk_index")?,
            );
            push_u32(
                &mut out,
                usize_to_u32(block_chunk_count, "stream block chunk_count")?,
            );
            push_u64(
                &mut out,
                usize_to_u64(block_original_len, "stream block original_len")?,
            );
            let history_field = if options.carry_grouping_history {
                history_method
                    .map(|method| method.as_u8())
                    .unwrap_or(ZBPS_HISTORY_NONE)
            } else {
                ZBPS_HISTORY_NONE
            };
            out.push(history_field);
            if history_field != ZBPS_HISTORY_NONE {
                grouping_hint_updates = grouping_hint_updates.saturating_add(1);
            }
            push_u32(
                &mut out,
                usize_to_u32(node_bytes.len(), "stream block node bytes")?,
            );
            out.extend_from_slice(&node_bytes);

            history_method = Some(global_method);
        }

        let stats = StreamPackStats {
            original_size: input.len(),
            compressed_size: out.len(),
            chunk_size: options.chunk_size,
            total_chunks,
            key_piece_interval: options.key_piece_interval,
            key_piece_count: block_count,
            block_count,
            max_group_depth: options.max_group_depth,
            max_group_pieces: options.max_group_pieces,
            piece_node_count: counts.piece_nodes,
            grouped_node_count: counts.grouped_nodes,
            split_node_count: counts.split_nodes,
            max_depth_used: counts.max_depth_used,
            grouping_hint_updates,
            key_piece_decode_note: format!(
                "receiver can start from chunk indices that are multiples of {} (block/key boundaries)",
                options.key_piece_interval
            ),
            effective_wide_overfitting_circuits: true,
            adaptive_wide_promotion_used: false,
            shared_grouping_payload_used: false,
            active_profile: context.profile.name().to_string(),
            skipped_candidates: context.skipped_candidates,
            timings: stream_finalize_timings(context.timings, total_timer, input.len()),
            cache_stats: context.cache_stats,
        };

        return Ok((out, stats));
    }

    let shared_payload_overhead = shared_grouping_payload_bytes
        .as_ref()
        .map(|payload| {
            4usize
                .checked_add(payload.len())
                .ok_or_else(|| ZbitError::Limit("stream shared payload size overflow".to_string()))
        })
        .transpose()?
        .unwrap_or(0);
    let mut out = Vec::with_capacity(
        ZBPS_HEADER_BYTES
            .checked_add(shared_payload_overhead)
            .and_then(|v| v.checked_add(block_count.checked_mul(ZBPS_BLOCK_HEADER_BYTES)?))
            .ok_or_else(|| ZbitError::Limit("stream output header size overflow".to_string()))?,
    );

    push_u32(&mut out, ZBPS_MAGIC);
    push_u16(&mut out, ZBPS_VERSION);
    push_u16(&mut out, flags);
    push_u32(
        &mut out,
        usize_to_u32(options.chunk_size, "stream chunk_size")?,
    );
    push_u32(
        &mut out,
        usize_to_u32(options.key_piece_interval, "stream key_piece_interval")?,
    );
    out.push(options.max_group_depth);
    out.extend_from_slice(&[0u8; 3]);
    push_u32(
        &mut out,
        usize_to_u32(options.max_group_pieces, "stream max_group_pieces")?,
    );
    push_u64(
        &mut out,
        usize_to_u64(input.len(), "stream original input size")?,
    );
    push_u32(&mut out, usize_to_u32(total_chunks, "stream total_chunks")?);
    push_u32(&mut out, usize_to_u32(block_count, "stream block_count")?);
    if let Some(payload) = shared_grouping_payload_bytes.as_ref() {
        push_u32(
            &mut out,
            usize_to_u32(payload.len(), "stream shared grouping payload length")?,
        );
        out.extend_from_slice(payload);
    }

    let mut counts = StreamNodeCounts::default();
    let mut grouping_hint_updates = 0usize;
    let mut history_method: Option<PackMethod> = None;
    let enable_realtime_root_recursive_candidate = options.realtime_mode
        && options.max_group_depth >= 2
        && shared_grouping_payload_bytes.is_none();
    if options.realtime_mode
        && options.max_group_depth >= 2
        && shared_grouping_payload_bytes.is_some()
    {
        context.push_skipped(
            "local realtime recursive root candidates skipped: shared grouping payload is active"
        );
    }

    for block_index in 0..block_count {
        let block_start = block_index * options.key_piece_interval;
        let block_end = (block_start + options.key_piece_interval).min(total_chunks);
        let block_chunk_count = block_end - block_start;
        let block_original_len = chunks[block_start..block_end]
            .iter()
            .map(|chunk| chunk.len())
            .sum::<usize>();

        let planning_timer = Instant::now();
        let mut memo = HashMap::new();
        let local_root = build_best_stream_node(
            &chunks[block_start..block_end],
            0,
            block_chunk_count,
            block_start,
            options.max_group_depth,
            options.max_group_pieces,
            options.realtime_mode,
            enable_realtime_root_recursive_candidate,
            input_hash,
            &mut context,
            &mut memo,
        )?;
        context.timings.stream_block_planning_ms += planning_timer.elapsed().as_secs_f64() * 1000.0;
        let block_offset = block_start
            .checked_mul(options.chunk_size)
            .ok_or_else(|| ZbitError::Limit("stream block offset overflow".to_string()))?;
        let root = if shared_grouping_payload_bytes.is_some() {
            let global_slice =
                stream_global_slice_node(block_chunk_count, block_offset, block_original_len)?;
            if global_slice.encoded_size <= local_root.encoded_size {
                global_slice
            } else {
                local_root
            }
        } else {
            local_root
        };
        collect_stream_node_counts(&root, 0, &mut counts);

        let mut node_bytes = Vec::with_capacity(root.encoded_size);
        write_stream_node(&mut node_bytes, &root)?;
        if node_bytes.len() != root.encoded_size {
            return Err(ZbitError::Internal(format!(
                "stream node encoded size mismatch: expected {}, got {}",
                root.encoded_size,
                node_bytes.len()
            )));
        }

        push_u32(
            &mut out,
            usize_to_u32(block_start, "stream block first_chunk_index")?,
        );
        push_u32(
            &mut out,
            usize_to_u32(block_chunk_count, "stream block chunk_count")?,
        );
        push_u64(
            &mut out,
            usize_to_u64(block_original_len, "stream block original_len")?,
        );
        let history_field = if options.carry_grouping_history {
            history_method
                .map(|method| method.as_u8())
                .unwrap_or(ZBPS_HISTORY_NONE)
        } else {
            ZBPS_HISTORY_NONE
        };
        out.push(history_field);
        if history_field != ZBPS_HISTORY_NONE {
            grouping_hint_updates = grouping_hint_updates.saturating_add(1);
        }
        push_u32(
            &mut out,
            usize_to_u32(node_bytes.len(), "stream block node bytes")?,
        );
        out.extend_from_slice(&node_bytes);

        history_method = Some(stream_node_history_method(&root));
    }

    let stats = StreamPackStats {
        original_size: input.len(),
        compressed_size: out.len(),
        chunk_size: options.chunk_size,
        total_chunks,
        key_piece_interval: options.key_piece_interval,
        key_piece_count: block_count,
        block_count,
        max_group_depth: options.max_group_depth,
        max_group_pieces: options.max_group_pieces,
        piece_node_count: counts.piece_nodes,
        grouped_node_count: counts.grouped_nodes,
        split_node_count: counts.split_nodes,
        max_depth_used: counts.max_depth_used,
        grouping_hint_updates,
        key_piece_decode_note: format!(
            "receiver can start from chunk indices that are multiples of {} (block/key boundaries)",
            options.key_piece_interval
        ),
        effective_wide_overfitting_circuits: false,
        adaptive_wide_promotion_used: false,
        shared_grouping_payload_used: shared_grouping_payload_bytes.is_some(),
        active_profile: context.profile.name().to_string(),
        skipped_candidates: context.skipped_candidates,
        timings: stream_finalize_timings(context.timings, total_timer, input.len()),
        cache_stats: context.cache_stats,
    };

    Ok((out, stats))
}

fn stream_finalize_timings(
    mut timings: CandidateTimingStats,
    total_timer: Instant,
    original_size: usize,
) -> CandidateTimingStats {
    let total_ms = total_timer.elapsed().as_secs_f64() * 1000.0;
    timings.total_compression_ms = total_ms;
    timings.compression_throughput_mib_s = if total_ms > 0.0 {
        (original_size as f64) / (total_ms / 1000.0) / (1024.0 * 1024.0)
    } else {
        0.0
    };
    timings
}

pub fn compress_adaptive_stream_to_file(
    input: &[u8],
    path: impl AsRef<Path>,
    options: &StreamPackOptions,
) -> ZbitResult<StreamPackStats> {
    let (stream_bytes, stats) = compress_stream_to_bytes(input, options)?;
    fs::write(path.as_ref(), &stream_bytes)?;
    Ok(stats)
}

fn decompress_stream_from_bytes(
    bytes: &[u8],
    start_key_piece: Option<usize>,
) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;
    let header = parse_stream_header(bytes, &mut cursor)?;

    if cursor != ZBPS_HEADER_BYTES {
        return Err(ZbitError::Parse(format!(
            "stream header size mismatch: expected {ZBPS_HEADER_BYTES}, got {cursor}"
        )));
    }

    let has_global_grouping_payload = (header.flags & ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS) != 0
        || (header.flags & ZBPS_FLAG_SHARED_GROUPING_PAYLOAD) != 0;
    let global_overfit_output = if has_global_grouping_payload {
        let global_pack_len = read_u32(bytes, &mut cursor)? as usize;
        let global_pack_bytes = bytes.get(cursor..cursor + global_pack_len).ok_or_else(|| {
            ZbitError::Parse("stream global overfit payload range out of bounds".to_string())
        })?;
        cursor += global_pack_len;
        let decoded = decompress_pack_bytes(global_pack_bytes)?;
        if decoded.len() != header.original_size {
            return Err(ZbitError::Parse(format!(
                "stream global overfit output length mismatch: expected {} got {}",
                header.original_size,
                decoded.len()
            )));
        }
        Some(decoded)
    } else {
        None
    };

    if let Some(key_piece_index) = start_key_piece {
        if key_piece_index >= header.total_chunks {
            return Err(ZbitError::InvalidArg(
                "start_key_piece is outside stream chunk range",
            ));
        }
        if key_piece_index % header.key_piece_interval != 0 {
            return Err(ZbitError::InvalidArg(
                "start_key_piece must align with key_piece_interval boundary",
            ));
        }
    }

    let mut out = Vec::new();
    let mut total_decoded_chunks = 0usize;
    let mut total_decoded_bytes = 0usize;
    let mut started = start_key_piece.is_none();
    let start_key_piece_value = start_key_piece.unwrap_or(0);

    for expected_block_index in 0..header.block_count {
        let first_chunk_index = read_u32(bytes, &mut cursor)? as usize;
        let block_chunk_count = read_u32(bytes, &mut cursor)? as usize;
        let block_original_len = u64_to_usize(read_u64(bytes, &mut cursor)?, "stream block len")?;
        let _history_method_id = read_u8(bytes, &mut cursor)?;
        let node_bytes_len = read_u32(bytes, &mut cursor)? as usize;
        let node_bytes = bytes
            .get(cursor..cursor + node_bytes_len)
            .ok_or_else(|| ZbitError::Parse("stream block node range out of bounds".to_string()))?;
        cursor += node_bytes_len;

        let expected_first = expected_block_index
            .checked_mul(header.key_piece_interval)
            .ok_or_else(|| ZbitError::Parse("stream block index overflow".to_string()))?;
        if first_chunk_index != expected_first {
            return Err(ZbitError::Parse(format!(
                "stream block order mismatch: expected first chunk {expected_first}, got {first_chunk_index}"
            )));
        }

        if block_chunk_count == 0 {
            return Err(ZbitError::Parse(
                "stream block chunk_count must be > 0".to_string(),
            ));
        }
        let expected_block_len = expected_stream_block_len(
            header.original_size,
            header.chunk_size,
            header.total_chunks,
            first_chunk_index,
            block_chunk_count,
        )?;
        if expected_block_len != block_original_len {
            return Err(ZbitError::Parse(format!(
                "stream block length mismatch: header={} expected={expected_block_len}",
                block_original_len
            )));
        }

        if !started {
            if first_chunk_index == start_key_piece_value {
                started = true;
            } else {
                continue;
            }
        }

        let mut node_cursor = 0usize;
        let decoded = decode_stream_node(
            node_bytes,
            &mut node_cursor,
            global_overfit_output.as_deref(),
        )?;
        if node_cursor != node_bytes.len() {
            return Err(ZbitError::Parse(
                "trailing bytes in stream block node".to_string(),
            ));
        }
        if decoded.chunk_count != block_chunk_count {
            return Err(ZbitError::Parse(format!(
                "stream block chunk count mismatch: header={} decoded={}",
                block_chunk_count, decoded.chunk_count
            )));
        }
        if decoded.bytes.len() != block_original_len {
            return Err(ZbitError::Parse(format!(
                "stream block byte length mismatch: header={} decoded={}",
                block_original_len,
                decoded.bytes.len()
            )));
        }

        total_decoded_chunks = total_decoded_chunks
            .checked_add(decoded.chunk_count)
            .ok_or_else(|| ZbitError::Limit("stream decoded chunk count overflow".to_string()))?;
        total_decoded_bytes = total_decoded_bytes
            .checked_add(decoded.bytes.len())
            .ok_or_else(|| ZbitError::Limit("stream decoded size overflow".to_string()))?;
        out.extend_from_slice(&decoded.bytes);
    }

    if cursor != bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes after stream blocks".to_string(),
        ));
    }

    if start_key_piece.is_none() {
        if total_decoded_chunks != header.total_chunks {
            return Err(ZbitError::Parse(format!(
                "decoded stream chunks mismatch: expected {} got {}",
                header.total_chunks, total_decoded_chunks
            )));
        }
        if total_decoded_bytes != header.original_size {
            return Err(ZbitError::Parse(format!(
                "decoded stream length mismatch: expected {} got {}",
                header.original_size, total_decoded_bytes
            )));
        }
    } else if !started {
        return Err(ZbitError::Parse(
            "requested start_key_piece boundary not found in stream blocks".to_string(),
        ));
    }

    Ok(out)
}

pub fn decompress_stream_file(path: impl AsRef<Path>) -> ZbitResult<Vec<u8>> {
    let bytes = fs::read(path)?;
    decompress_stream_from_bytes(&bytes, None)
}

pub fn decompress_stream_file_from_key_piece(
    path: impl AsRef<Path>,
    key_piece_index: usize,
) -> ZbitResult<Vec<u8>> {
    let bytes = fs::read(path)?;
    decompress_stream_from_bytes(&bytes, Some(key_piece_index))
}
