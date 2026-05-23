// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

fn apply_unary_delta(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; data.len()];
    out[0] = data[0];
    for i in 1..data.len() {
        out[i] = data[i].wrapping_sub(data[i - 1]);
    }
    out
}

fn invert_unary_delta(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; data.len()];
    out[0] = data[0];
    for i in 1..data.len() {
        out[i] = data[i].wrapping_add(out[i - 1]);
    }
    out
}

fn apply_unary_xor(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; data.len()];
    out[0] = data[0];
    for i in 1..data.len() {
        out[i] = data[i] ^ data[i - 1];
    }
    out
}

fn invert_unary_xor(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; data.len()];
    out[0] = data[0];
    for i in 1..data.len() {
        out[i] = data[i] ^ out[i - 1];
    }
    out
}

fn apply_periodic_head_tail(data: &[u8], period: usize, head: usize) -> Option<Vec<u8>> {
    if period < 2 || head == 0 || head >= period {
        return None;
    }
    let mut heads = Vec::with_capacity(data.len() / period * head + period);
    let mut tails = Vec::with_capacity(data.len().saturating_sub(heads.capacity()));
    let mut cursor = 0usize;
    while cursor < data.len() {
        let end = (cursor + period).min(data.len());
        let chunk = &data[cursor..end];
        let h = head.min(chunk.len());
        heads.extend_from_slice(&chunk[..h]);
        tails.extend_from_slice(&chunk[h..]);
        cursor = end;
    }
    heads.extend_from_slice(&tails);
    Some(heads)
}

fn invert_periodic_head_tail(
    data: &[u8],
    original_len: usize,
    period: usize,
    head: usize,
) -> Option<Vec<u8>> {
    if period < 2 || head == 0 || head >= period || data.len() != original_len {
        return None;
    }
    let mut total_head = 0usize;
    let mut cursor = 0usize;
    while cursor < original_len {
        let end = (cursor + period).min(original_len);
        total_head = total_head.checked_add(head.min(end - cursor))?;
        cursor = end;
    }

    let head_stream = data.get(..total_head)?;
    let tail_stream = data.get(total_head..)?;
    let mut out = Vec::with_capacity(original_len);

    let mut h_cur = 0usize;
    let mut t_cur = 0usize;
    let mut pos = 0usize;
    while pos < original_len {
        let end = (pos + period).min(original_len);
        let chunk_len = end - pos;
        let h = head.min(chunk_len);
        let t = chunk_len - h;

        out.extend_from_slice(head_stream.get(h_cur..h_cur + h)?);
        out.extend_from_slice(tail_stream.get(t_cur..t_cur + t)?);
        h_cur += h;
        t_cur += t;
        pos = end;
    }

    if out.len() == original_len {
        Some(out)
    } else {
        None
    }
}

fn apply_periodic_gather(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 2 || period > data.len() {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    for offset in 0..period {
        let mut cursor = offset;
        while cursor < data.len() {
            out.push(data[cursor]);
            cursor = cursor.saturating_add(period);
        }
    }
    Some(out)
}

fn invert_periodic_gather(data: &[u8], original_len: usize, period: usize) -> Option<Vec<u8>> {
    if period < 2 || period > original_len || data.len() != original_len {
        return None;
    }
    let mut out = vec![0u8; original_len];
    let mut read_cursor = 0usize;
    for offset in 0..period {
        let mut cursor = offset;
        while cursor < original_len {
            out[cursor] = *data.get(read_cursor)?;
            read_cursor += 1;
            cursor = cursor.saturating_add(period);
        }
    }
    if read_cursor != data.len() {
        return None;
    }
    Some(out)
}

fn apply_periodic_delta(data: &[u8], period: usize) -> Option<Vec<u8>> {
    // period == 1 collapses to ordinary unary delta over the input; allowing it lets
    // head-tail-tail-delta and similar composites express "delta-1 on the tail".
    if period < 1 || period > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    for i in period..data.len() {
        out[i] = data[i].wrapping_sub(data[i - period]);
    }
    Some(out)
}

fn invert_periodic_delta(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 1 || period > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    for i in period..data.len() {
        out[i] = data[i].wrapping_add(out[i - period]);
    }
    Some(out)
}

fn apply_periodic_xor(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 1 || period > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    for i in period..data.len() {
        out[i] = data[i] ^ data[i - period];
    }
    Some(out)
}

fn invert_periodic_xor(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 1 || period > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    for i in period..data.len() {
        out[i] = data[i] ^ out[i - period];
    }
    Some(out)
}

fn periodic_head_bytes(original_len: usize, period: usize, head: usize) -> Option<usize> {
    if period < 2 || head == 0 || head >= period {
        return None;
    }
    let mut total_head = 0usize;
    let mut cursor = 0usize;
    while cursor < original_len {
        let end = (cursor + period).min(original_len);
        total_head = total_head.checked_add(head.min(end - cursor))?;
        cursor = end;
    }
    Some(total_head)
}

fn apply_head_tail_tail_gather(
    data: &[u8],
    period: usize,
    tail_gather_period: usize,
) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, 1)?;
    let total_head = periodic_head_bytes(data.len(), period, 1)?;
    let head_stream = head_tail.get(..total_head)?;
    let tail_stream = head_tail.get(total_head..)?;
    let tail_gathered = apply_periodic_gather(tail_stream, tail_gather_period)?;
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(head_stream);
    out.extend_from_slice(&tail_gathered);
    Some(out)
}

fn invert_head_tail_tail_gather(
    data: &[u8],
    original_len: usize,
    period: usize,
    tail_gather_period: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let total_head = periodic_head_bytes(original_len, period, 1)?;
    let head_stream = data.get(..total_head)?;
    let tail_stream = data.get(total_head..)?;
    let tail = invert_periodic_gather(tail_stream, original_len - total_head, tail_gather_period)?;
    let mut merged = Vec::with_capacity(original_len);
    merged.extend_from_slice(head_stream);
    merged.extend_from_slice(&tail);
    invert_periodic_head_tail(&merged, original_len, period, 1)
}

fn apply_head_tail_tail_gather_delta(
    data: &[u8],
    period: usize,
    tail_gather_period: usize,
) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, 1)?;
    let total_head = periodic_head_bytes(data.len(), period, 1)?;
    let head_stream = head_tail.get(..total_head)?;
    let tail_stream = head_tail.get(total_head..)?;
    let tail_gathered = apply_periodic_gather(tail_stream, tail_gather_period)?;
    let tail_delta = apply_unary_delta(&tail_gathered);
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(head_stream);
    out.extend_from_slice(&tail_delta);
    Some(out)
}

fn invert_head_tail_tail_gather_delta(
    data: &[u8],
    original_len: usize,
    period: usize,
    tail_gather_period: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let total_head = periodic_head_bytes(original_len, period, 1)?;
    let head_stream = data.get(..total_head)?;
    let tail_delta = data.get(total_head..)?;
    let tail_gathered = invert_unary_delta(tail_delta);
    let tail = invert_periodic_gather(
        &tail_gathered,
        original_len - total_head,
        tail_gather_period,
    )?;
    let mut merged = Vec::with_capacity(original_len);
    merged.extend_from_slice(head_stream);
    merged.extend_from_slice(&tail);
    invert_periodic_head_tail(&merged, original_len, period, 1)
}

fn apply_head_tail_tail_delta(
    data: &[u8],
    period: usize,
    tail_delta_period: usize,
) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, 1)?;
    let total_head = periodic_head_bytes(data.len(), period, 1)?;
    let head_stream = head_tail.get(..total_head)?;
    let tail_stream = head_tail.get(total_head..)?;
    let tail_delta = apply_periodic_delta(tail_stream, tail_delta_period)?;
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(head_stream);
    out.extend_from_slice(&tail_delta);
    Some(out)
}

fn invert_head_tail_tail_delta(
    data: &[u8],
    original_len: usize,
    period: usize,
    tail_delta_period: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let total_head = periodic_head_bytes(original_len, period, 1)?;
    let head_stream = data.get(..total_head)?;
    let tail_delta = data.get(total_head..)?;
    let tail = invert_periodic_delta(tail_delta, tail_delta_period)?;
    let mut merged = Vec::with_capacity(original_len);
    merged.extend_from_slice(head_stream);
    merged.extend_from_slice(&tail);
    invert_periodic_head_tail(&merged, original_len, period, 1)
}

fn apply_head_tail_tail_xor(data: &[u8], period: usize, tail_xor_period: usize) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, 1)?;
    let total_head = periodic_head_bytes(data.len(), period, 1)?;
    let head_stream = head_tail.get(..total_head)?;
    let tail_stream = head_tail.get(total_head..)?;
    let tail_xor = apply_periodic_xor(tail_stream, tail_xor_period)?;
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(head_stream);
    out.extend_from_slice(&tail_xor);
    Some(out)
}

fn invert_head_tail_tail_xor(
    data: &[u8],
    original_len: usize,
    period: usize,
    tail_xor_period: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let total_head = periodic_head_bytes(original_len, period, 1)?;
    let head_stream = data.get(..total_head)?;
    let tail_xor = data.get(total_head..)?;
    let tail = invert_periodic_xor(tail_xor, tail_xor_period)?;
    let mut merged = Vec::with_capacity(original_len);
    merged.extend_from_slice(head_stream);
    merged.extend_from_slice(&tail);
    invert_periodic_head_tail(&merged, original_len, period, 1)
}

fn apply_head_tail_delta(data: &[u8], period: usize, head: usize) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, head)?;
    Some(apply_unary_delta(&head_tail))
}

fn invert_head_tail_delta(
    data: &[u8],
    original_len: usize,
    period: usize,
    head: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let recovered = invert_unary_delta(data);
    invert_periodic_head_tail(&recovered, original_len, period, head)
}

fn apply_head_tail_xor(data: &[u8], period: usize, head: usize) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, head)?;
    Some(apply_unary_xor(&head_tail))
}

fn invert_head_tail_xor(
    data: &[u8],
    original_len: usize,
    period: usize,
    head: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let recovered = invert_unary_xor(data);
    invert_periodic_head_tail(&recovered, original_len, period, head)
}

fn apply_bit_plane_transpose(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let n = data.len();
    let mut out = vec![0u8; n];
    for (byte_idx, &byte) in data.iter().enumerate() {
        for bit in 0..8usize {
            if ((byte >> bit) & 1) == 0 {
                continue;
            }
            let dst_bit_index = bit * n + byte_idx;
            let dst_byte = dst_bit_index >> 3;
            let dst_bit = dst_bit_index & 7;
            out[dst_byte] |= 1u8 << dst_bit;
        }
    }
    out
}

fn invert_bit_plane_transpose(data: &[u8], original_len: usize) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    if data.is_empty() {
        return Some(Vec::new());
    }
    let n = original_len;
    let mut out = vec![0u8; n];
    for bit in 0..8usize {
        for byte_idx in 0..n {
            let src_bit_index = bit.checked_mul(n).and_then(|v| v.checked_add(byte_idx))?;
            let src_byte = *data.get(src_bit_index >> 3)?;
            let src_bit = (src_byte >> (src_bit_index & 7)) & 1;
            if src_bit != 0 {
                out[byte_idx] |= 1u8 << bit;
            }
        }
    }
    Some(out)
}

fn apply_transform_plan(data: &[u8], plan: &CircuitTransformPlan) -> Option<Vec<u8>> {
    match plan.kind {
        CircuitTransformKind::Identity => Some(data.to_vec()),
        CircuitTransformKind::DeltaPrev => Some(apply_unary_delta(data)),
        CircuitTransformKind::XorPrev => Some(apply_unary_xor(data)),
        CircuitTransformKind::BitPlaneTranspose => Some(apply_bit_plane_transpose(data)),
        CircuitTransformKind::BitPlaneTransposeDelta => {
            let transposed = apply_bit_plane_transpose(data);
            Some(apply_unary_delta(&transposed))
        }
        CircuitTransformKind::BitPlaneTransposeXor => {
            let transposed = apply_bit_plane_transpose(data);
            Some(apply_unary_xor(&transposed))
        }
        CircuitTransformKind::PeriodicHeadTail => {
            apply_periodic_head_tail(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicGather => apply_periodic_gather(data, plan.period as usize),
        CircuitTransformKind::PeriodicDelta => apply_periodic_delta(data, plan.period as usize),
        CircuitTransformKind::PeriodicXor => apply_periodic_xor(data, plan.period as usize),
        CircuitTransformKind::PeriodicGatherDelta => {
            let gathered = apply_periodic_gather(data, plan.period as usize)?;
            Some(apply_unary_delta(&gathered))
        }
        CircuitTransformKind::PeriodicGatherXor => {
            let gathered = apply_periodic_gather(data, plan.period as usize)?;
            Some(apply_unary_xor(&gathered))
        }
        CircuitTransformKind::PeriodicHeadTailTailGather => {
            apply_head_tail_tail_gather(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailTailGatherDelta => {
            apply_head_tail_tail_gather_delta(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailTailDelta => {
            apply_head_tail_tail_delta(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailTailXor => {
            apply_head_tail_tail_xor(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailDelta => {
            apply_head_tail_delta(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailXor => {
            apply_head_tail_xor(data, plan.period as usize, plan.head as usize)
        }
    }
}

fn invert_transform_plan(
    data: &[u8],
    original_len: usize,
    plan: &CircuitTransformPlan,
) -> Option<Vec<u8>> {
    match plan.kind {
        CircuitTransformKind::Identity => {
            if data.len() == original_len {
                Some(data.to_vec())
            } else {
                None
            }
        }
        CircuitTransformKind::DeltaPrev => {
            if data.len() == original_len {
                Some(invert_unary_delta(data))
            } else {
                None
            }
        }
        CircuitTransformKind::XorPrev => {
            if data.len() == original_len {
                Some(invert_unary_xor(data))
            } else {
                None
            }
        }
        CircuitTransformKind::BitPlaneTranspose => invert_bit_plane_transpose(data, original_len),
        CircuitTransformKind::BitPlaneTransposeDelta => {
            if data.len() != original_len {
                return None;
            }
            let recovered = invert_unary_delta(data);
            invert_bit_plane_transpose(&recovered, original_len)
        }
        CircuitTransformKind::BitPlaneTransposeXor => {
            if data.len() != original_len {
                return None;
            }
            let recovered = invert_unary_xor(data);
            invert_bit_plane_transpose(&recovered, original_len)
        }
        CircuitTransformKind::PeriodicHeadTail => {
            invert_periodic_head_tail(data, original_len, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicGather => {
            invert_periodic_gather(data, original_len, plan.period as usize)
        }
        CircuitTransformKind::PeriodicDelta => {
            if data.len() == original_len {
                invert_periodic_delta(data, plan.period as usize)
            } else {
                None
            }
        }
        CircuitTransformKind::PeriodicXor => {
            if data.len() == original_len {
                invert_periodic_xor(data, plan.period as usize)
            } else {
                None
            }
        }
        CircuitTransformKind::PeriodicGatherDelta => {
            if data.len() != original_len {
                return None;
            }
            let recovered = invert_unary_delta(data);
            invert_periodic_gather(&recovered, original_len, plan.period as usize)
        }
        CircuitTransformKind::PeriodicGatherXor => {
            if data.len() != original_len {
                return None;
            }
            let recovered = invert_unary_xor(data);
            invert_periodic_gather(&recovered, original_len, plan.period as usize)
        }
        CircuitTransformKind::PeriodicHeadTailTailGather => invert_head_tail_tail_gather(
            data,
            original_len,
            plan.period as usize,
            plan.head as usize,
        ),
        CircuitTransformKind::PeriodicHeadTailTailGatherDelta => {
            invert_head_tail_tail_gather_delta(
                data,
                original_len,
                plan.period as usize,
                plan.head as usize,
            )
        }
        CircuitTransformKind::PeriodicHeadTailTailDelta => invert_head_tail_tail_delta(
            data,
            original_len,
            plan.period as usize,
            plan.head as usize,
        ),
        CircuitTransformKind::PeriodicHeadTailTailXor => {
            invert_head_tail_tail_xor(data, original_len, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailDelta => {
            invert_head_tail_delta(data, original_len, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailXor => {
            invert_head_tail_xor(data, original_len, plan.period as usize, plan.head as usize)
        }
    }
}


fn score_periodic_candidates(data: &[u8], max_period: usize, top_k: usize) -> Vec<usize> {
    if data.len() < 2048 || max_period < 2 || top_k == 0 {
        return Vec::new();
    }

    let n = data.len();
    let max_p = max_period.min(n.saturating_sub(1));
    let target_samples = 20_000usize.min(n.saturating_sub(2));
    if target_samples == 0 {
        return Vec::new();
    }

    let mut scored = Vec::<(usize, f64)>::new();
    for period in 2..=max_p {
        let range = n - period;
        let step = (range / target_samples).max(1);
        let mut matches = 0usize;
        let mut total = 0usize;
        let mut idx = period;
        while idx < n {
            if data[idx] == data[idx - period] {
                matches += 1;
            }
            total += 1;
            idx = idx.saturating_add(step);
        }
        if total == 0 {
            continue;
        }
        let score = matches as f64 / total as f64;
        scored.push((period, score));
    }

    scored.sort_by(|a, b| b.1.total_cmp(&a.1));

    // Dedup nearby periods: consecutive sampling tends to pick the true period plus several
    // neighbours (e.g. 6397, 6401, 6405 for a stride near 6401). The neighbours produce nearly
    // identical transformed payloads, so they waste expensive plan evaluation. Keep only periods
    // that differ from every already-kept period by at least 8.
    let mut kept = Vec::<usize>::with_capacity(top_k);
    for (period, _) in scored {
        if kept.len() >= top_k {
            break;
        }
        if kept
            .iter()
            .any(|&existing| existing.abs_diff(period) < 8)
        {
            continue;
        }
        kept.push(period);
    }
    kept
}

const TOPOLOGY_HASH_OFFSET: u64 = 0xcbf29ce484222325;
const TOPOLOGY_HASH_PRIME: u64 = 0x100000001b3;
const TOPOLOGY_NODE_BYTES: usize = 28;
const TOPOLOGY_CORRECTION_PLAN_KIND_BASE: u8 = 200;

fn topology_hash_mix(mut state: u64, value: u64) -> u64 {
    state ^= value;
    state.wrapping_mul(TOPOLOGY_HASH_PRIME)
}

fn finalize_topology_hashes(nodes: &mut [CircuitTopologyNode]) -> ZbitResult<()> {
    let mut hash_by_id = HashMap::<u32, u64>::new();
    for node in nodes.iter_mut() {
        let parent_hash = if node.parent_id == u32::MAX {
            TOPOLOGY_HASH_OFFSET
        } else {
            *hash_by_id.get(&node.parent_id).ok_or_else(|| {
                ZbitError::Internal("topology contains child before parent".to_string())
            })?
        };
        let mut hash = TOPOLOGY_HASH_OFFSET;
        hash = topology_hash_mix(hash, parent_hash);
        hash = topology_hash_mix(hash, node.id as u64);
        hash = topology_hash_mix(hash, node.parent_id as u64);
        hash = topology_hash_mix(hash, node.relation as u64);
        hash = topology_hash_mix(hash, node.order as u64);
        hash = topology_hash_mix(hash, node.kind as u64);
        hash = topology_hash_mix(hash, node.param_a as u64);
        hash = topology_hash_mix(hash, node.param_b as u64);
        node.hash64 = hash;
        hash_by_id.insert(node.id, hash);
    }
    Ok(())
}

fn push_topology_node(
    nodes: &mut Vec<CircuitTopologyNode>,
    parent_id: u32,
    relation: u8,
    order: u16,
    kind: u8,
    param_a: u32,
    param_b: u32,
) -> u32 {
    let id = (nodes.len() as u32).saturating_add(1);
    nodes.push(CircuitTopologyNode {
        id,
        parent_id,
        relation,
        order,
        kind,
        param_a,
        param_b,
        hash64: 0,
    });
    id
}

fn embed_correction_plan_in_topology(
    nodes: &mut Vec<CircuitTopologyNode>,
    correction_plan: &CircuitTransformPlan,
) -> ZbitResult<bool> {
    if correction_plan.kind == CircuitTransformKind::Identity
        && correction_plan.period == 0
        && correction_plan.head == 0
    {
        return Ok(false);
    }

    let root_id = nodes
        .iter()
        .find(|node| node.parent_id == u32::MAX)
        .map(|node| node.id)
        .ok_or_else(|| ZbitError::Internal("topology root node missing".to_string()))?;
    let order = nodes
        .iter()
        .filter(|node| node.parent_id == root_id && node.relation == 0)
        .map(|node| node.order)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let kind = TOPOLOGY_CORRECTION_PLAN_KIND_BASE.saturating_add(correction_plan.kind.as_u8());
    let _ = push_topology_node(
        nodes,
        root_id,
        0,
        order,
        kind,
        correction_plan.period,
        correction_plan.head,
    );
    finalize_topology_hashes(nodes)?;
    Ok(true)
}

fn decode_embedded_correction_plan(
    kind: u8,
    period: u32,
    head: u32,
) -> Option<CircuitTransformPlan> {
    let kind_u8 = kind.checked_sub(TOPOLOGY_CORRECTION_PLAN_KIND_BASE)?;
    let transform_kind = CircuitTransformKind::from_u8(kind_u8)?;
    Some(CircuitTransformPlan {
        kind: transform_kind,
        period,
        head,
    })
}

fn build_topology_for_plan(plan: &CircuitTransformPlan) -> ZbitResult<Vec<CircuitTopologyNode>> {
    let mut nodes = Vec::new();
    let root_id = push_topology_node(&mut nodes, u32::MAX, 0, 0, 0, 0, 0);

    match plan.kind {
        CircuitTransformKind::Identity => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 1, 0, 0);
        }
        CircuitTransformKind::DeltaPrev => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 2, 1, 0);
        }
        CircuitTransformKind::XorPrev => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 3, 1, 0);
        }
        CircuitTransformKind::BitPlaneTranspose => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 19, 8, 0);
        }
        CircuitTransformKind::BitPlaneTransposeDelta => {
            let bitplane_id = push_topology_node(&mut nodes, root_id, 0, 0, 19, 8, 0);
            let _ = push_topology_node(&mut nodes, bitplane_id, 0, 0, 20, 1, 0);
        }
        CircuitTransformKind::BitPlaneTransposeXor => {
            let bitplane_id = push_topology_node(&mut nodes, root_id, 0, 0, 19, 8, 0);
            let _ = push_topology_node(&mut nodes, bitplane_id, 0, 0, 21, 1, 0);
        }
        CircuitTransformKind::PeriodicHeadTail => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, plan.head);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, plan.head, 0);
            let _ = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(plan.head),
                0,
            );
        }
        CircuitTransformKind::PeriodicGather => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 5, plan.period, 0);
        }
        CircuitTransformKind::PeriodicDelta => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 6, plan.period, 0);
        }
        CircuitTransformKind::PeriodicXor => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 7, plan.period, 0);
        }
        CircuitTransformKind::PeriodicGatherDelta => {
            let gather_id = push_topology_node(&mut nodes, root_id, 0, 0, 5, plan.period, 0);
            let _ = push_topology_node(&mut nodes, gather_id, 0, 0, 12, 1, 0);
        }
        CircuitTransformKind::PeriodicGatherXor => {
            let gather_id = push_topology_node(&mut nodes, root_id, 0, 0, 5, plan.period, 0);
            let _ = push_topology_node(&mut nodes, gather_id, 0, 0, 13, 1, 0);
        }
        CircuitTransformKind::PeriodicHeadTailTailGather => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, 1);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, 1, 0);
            let tail_id = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(1),
                0,
            );
            let _ = push_topology_node(&mut nodes, tail_id, 0, 0, 14, plan.head, 0);
        }
        CircuitTransformKind::PeriodicHeadTailTailGatherDelta => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, 1);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, 1, 0);
            let tail_id = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(1),
                0,
            );
            let gather_id = push_topology_node(&mut nodes, tail_id, 0, 0, 14, plan.head, 0);
            let _ = push_topology_node(&mut nodes, gather_id, 0, 0, 12, 1, 0);
        }
        CircuitTransformKind::PeriodicHeadTailTailDelta => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, 1);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, 1, 0);
            let tail_id = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(1),
                0,
            );
            let _ = push_topology_node(&mut nodes, tail_id, 0, 0, 15, plan.head, 0);
        }
        CircuitTransformKind::PeriodicHeadTailTailXor => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, 1);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, 1, 0);
            let tail_id = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(1),
                0,
            );
            let _ = push_topology_node(&mut nodes, tail_id, 0, 0, 16, plan.head, 0);
        }
        CircuitTransformKind::PeriodicHeadTailDelta => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, plan.head);
            let _ = push_topology_node(&mut nodes, split_id, 0, 0, 17, 1, 0);
        }
        CircuitTransformKind::PeriodicHeadTailXor => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, plan.head);
            let _ = push_topology_node(&mut nodes, split_id, 0, 0, 18, 1, 0);
        }
    }

    finalize_topology_hashes(&mut nodes)?;
    Ok(nodes)
}

#[derive(Debug, Clone)]
struct AdaptiveTransformResult {
    plan: CircuitTransformPlan,
    topology: Vec<CircuitTopologyNode>,
    codec: PayloadCodec,
    payload: Vec<u8>,
    sampling_ms: f64,
    eval_ms: f64,
}

fn choose_adaptive_transform_plan(
    data: &[u8],
    profile: CompressionProfile,
    selected_budget: usize,
) -> ZbitResult<AdaptiveTransformResult> {
    let mut plans = vec![
        CircuitTransformPlan {
            kind: CircuitTransformKind::Identity,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::DeltaPrev,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::XorPrev,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTranspose,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTransposeDelta,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTransposeXor,
            period: 0,
            head: 0,
        },
    ];

    let mut add_plan = |plan: CircuitTransformPlan| {
        if !plans.iter().any(|p| *p == plan) {
            plans.push(plan);
        }
    };

    let high_corr_periods = score_periodic_candidates(data, 8192, 8);
    let mut period_candidates = high_corr_periods.clone();
    period_candidates.extend([2usize, 3, 4, 5, 8, 16, 32, 64, 128, 257, 512, 1024, 2048]);
    period_candidates.sort_unstable();
    period_candidates.dedup();

    for period in period_candidates {
        if period < 2 || period > data.len() {
            continue;
        }
        let period_u32 = period as u32;
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicGather,
            period: period_u32,
            head: 0,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicDelta,
            period: period_u32,
            head: 0,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicXor,
            period: period_u32,
            head: 0,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicGatherDelta,
            period: period_u32,
            head: 0,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicGatherXor,
            period: period_u32,
            head: 0,
        });

        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTail,
            period: period_u32,
            head: 1,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTailDelta,
            period: period_u32,
            head: 1,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTailXor,
            period: period_u32,
            head: 1,
        });
        for tail_gather_period in [2u32, 4, 8] {
            if (period_u32.saturating_sub(1)) >= tail_gather_period && tail_gather_period >= 2 {
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailTailGather,
                    period: period_u32,
                    head: tail_gather_period,
                });
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailTailGatherDelta,
                    period: period_u32,
                    head: tail_gather_period,
                });
            }
        }
        let mut tail_delta_periods = vec![1u32, 2, 4, 8, 16];
        let full_tail_period = period_u32.saturating_sub(1);
        if full_tail_period >= 2 {
            tail_delta_periods.push(full_tail_period);
        }
        let half_tail_period = full_tail_period / 2;
        if half_tail_period >= 2 {
            tail_delta_periods.push(half_tail_period);
        }
        tail_delta_periods.sort_unstable();
        tail_delta_periods.dedup();

        for tail_delta_period in tail_delta_periods {
            // tail_delta_period == 1 means a plain unary delta/xor over the tail (no period).
            if (period_u32.saturating_sub(1)) >= tail_delta_period && tail_delta_period >= 1 {
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailTailDelta,
                    period: period_u32,
                    head: tail_delta_period,
                });
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailTailXor,
                    period: period_u32,
                    head: tail_delta_period,
                });
            }
        }
        if period > 4 {
            add_plan(CircuitTransformPlan {
                kind: CircuitTransformKind::PeriodicHeadTail,
                period: period_u32,
                head: 2,
            });
            add_plan(CircuitTransformPlan {
                kind: CircuitTransformKind::PeriodicHeadTailDelta,
                period: period_u32,
                head: 2,
            });
            add_plan(CircuitTransformPlan {
                kind: CircuitTransformKind::PeriodicHeadTailXor,
                period: period_u32,
                head: 2,
            });
        }
        if period > 8 {
            let half = (period / 2) as u32;
            if half > 0 && half < period_u32 {
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTail,
                    period: period_u32,
                    head: half,
                });
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailDelta,
                    period: period_u32,
                    head: half,
                });
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailXor,
                    period: period_u32,
                    head: half,
                });
            }
        }
    }

    let sample_len = data.len().min(512 * 1024);
    let sample = &data[..sample_len];

    let sample_timer = Instant::now();
    let quick_zstd_level = match profile {
        CompressionProfile::Fast | CompressionProfile::Balanced => 1,
        CompressionProfile::Deep => 3,
        CompressionProfile::Research => 5,
    };
    let scored = plans
        .par_iter()
        .map(|plan| {
            let Some(transformed_sample) = apply_transform_plan(sample, plan) else {
                return Ok::<_, ZbitError>(None);
            };
            let quick = zstd_encode_with_level(&transformed_sample, quick_zstd_level)?.len();
            Ok(Some((*plan, quick)))
        })
        .collect::<ZbitResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let sampling_ms = sample_timer.elapsed().as_secs_f64() * 1000.0;

    let mut scored = scored;
    scored.sort_unstable_by_key(|entry| entry.1);
    let mut selected = Vec::<CircuitTransformPlan>::new();
    let mut selected_set = HashSet::<CircuitTransformPlan>::new();
    let bounded_budget = selected_budget
        .max(1)
        .min(profile.max_transform_plans().max(1));
    for (plan, _) in scored.iter().take(bounded_budget) {
        if selected_set.insert(*plan) {
            selected.push(*plan);
        }
    }

    for (idx, period) in high_corr_periods.into_iter().take(2).enumerate() {
        let plan = CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTail,
            period: period as u32,
            head: 1,
        };
        if selected_set.insert(plan) {
            selected.push(plan);
        }
        if idx == 0 {
            let period_u32 = period as u32;
            let mut forced_tail_heads = vec![1u32, 4u32];
            let full_tail = period_u32.saturating_sub(1);
            if full_tail >= 2 {
                forced_tail_heads.push(full_tail);
            }
            let half_tail = full_tail / 2;
            if half_tail >= 2 {
                forced_tail_heads.push(half_tail);
            }
            forced_tail_heads.sort_unstable();
            forced_tail_heads.dedup();

            for forced_kind in [
                CircuitTransformKind::PeriodicHeadTailDelta,
                CircuitTransformKind::PeriodicHeadTailXor,
            ] {
                let forced = CircuitTransformPlan {
                    kind: forced_kind,
                    period: period_u32,
                    head: 4,
                };
                if selected_set.insert(forced) {
                    selected.push(forced);
                }
            }

            for head in forced_tail_heads {
                for forced_kind in [
                    CircuitTransformKind::PeriodicHeadTailTailDelta,
                    CircuitTransformKind::PeriodicHeadTailTailXor,
                    CircuitTransformKind::PeriodicHeadTailTailGather,
                    CircuitTransformKind::PeriodicHeadTailTailGatherDelta,
                ] {
                    let forced = CircuitTransformPlan {
                        kind: forced_kind,
                        period: period_u32,
                        head,
                    };
                    if selected_set.insert(forced) {
                        selected.push(forced);
                    }
                }
            }
        }
    }

    for baseline in [
        CircuitTransformPlan {
            kind: CircuitTransformKind::Identity,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::DeltaPrev,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::XorPrev,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTranspose,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTransposeDelta,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTransposeXor,
            period: 0,
            head: 0,
        },
    ] {
        if selected_set.insert(baseline) {
            selected.push(baseline);
        }
    }

    let trace_recursive = std::env::var_os("ZBIT_TRACE_RECURSIVE").is_some();
    let eval_timer = Instant::now();

    // Phase A: apply each selected plan to the full data and score with a quick XZ encode.
    // A low-preset XZ ranks similarly to the eventual XZ-extreme winner because they share
    // the same LZMA2 / arithmetic coding tail, so this preserves ratio while being much cheaper
    // than running choose_best_codec (~6 XZ-9 encodes) on every plan.
    let quick_full_scored = selected
        .iter()
        .copied()
        .enumerate()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|(rank, plan)| {
            let Some(transformed) = apply_transform_plan(data, &plan) else {
                return Ok::<_, ZbitError>(None);
            };
            let quick_len = xz_encode_easy_preset(&transformed, 3)?.len();
            Ok::<_, ZbitError>(Some((rank, plan, transformed, quick_len)))
        })
        .collect::<ZbitResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    if trace_recursive {
        for (rank, plan, _, quick_len) in &quick_full_scored {
            eprintln!(
                "zbit-trace plan-quick-score rank={} kind={} period={} head={} quick-xz3={}",
                rank,
                plan.kind.name(),
                plan.period,
                plan.head,
                quick_len,
            );
        }
    }

    // Phase B: only run the expensive choose_best_codec on the top-K plans by quick score.
    // For balanced profile this cuts ~8 plans down to ~5, saving a large constant factor while
    // keeping enough headroom that the eventual winner is rarely dropped by the quick scorer.
    let full_eval_budget = profile.full_eval_transform_plans().min(quick_full_scored.len()).max(1);
    let mut quick_sorted = quick_full_scored;
    quick_sorted.sort_unstable_by_key(|(rank, _, _, quick_len)| (*quick_len, *rank));
    let mut top_plans = quick_sorted.into_iter().take(full_eval_budget).collect::<Vec<_>>();
    // Always keep the original best-by-quick-score plan first so its rank ordering is preserved
    top_plans.sort_unstable_by_key(|(rank, _, _, _)| *rank);

    let eval_results = top_plans
        .into_par_iter()
        .map(|(rank, plan, transformed, _quick_len)| {
            let (codec, final_payload) = choose_best_codec(&transformed, true, false, profile)?;
            if trace_recursive {
                eprintln!(
                    "zbit-trace plan-candidate kind={} period={} head={} encoded={} codec={}",
                    plan.kind.name(),
                    plan.period,
                    plan.head,
                    final_payload.len(),
                    codec.name()
                );
            }
            Ok::<_, ZbitError>(Some((rank, plan, transformed, codec, final_payload)))
        })
        .collect::<ZbitResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let eval_ms = eval_timer.elapsed().as_secs_f64() * 1000.0;

    let (_, plan, transformed, mut codec, mut encoded) = eval_results
        .into_iter()
        .min_by_key(|(rank, _, _, _, final_payload)| (final_payload.len(), *rank))
        .ok_or_else(|| {
            ZbitError::Internal("failed to evaluate adaptive transform plans".to_string())
        })?;
    let topology = build_topology_for_plan(&plan)?;

    if let Some((candidate_codec, candidate_payload)) = choose_best_tuned_xz_candidate(
        &transformed,
        profile,
        profile.enable_xz_extreme_refinement(),
    )? {
        if candidate_payload.len() < encoded.len() {
            codec = candidate_codec;
            encoded = candidate_payload;
        }
    }

    Ok(AdaptiveTransformResult {
        plan,
        topology,
        codec,
        payload: encoded,
        sampling_ms,
        eval_ms,
    })
}

fn choose_correction_transform_plan(
    corrections: &[u8],
    profile: CompressionProfile,
) -> ZbitResult<(CircuitTransformPlan, PayloadCodec, Vec<u8>, f64, f64)> {
    let identity_plan = CircuitTransformPlan {
        kind: CircuitTransformKind::Identity,
        period: 0,
        head: 0,
    };
    let (identity_codec, identity_payload) = choose_best_codec(corrections, true, true, profile)?;
    let candidate = choose_adaptive_transform_plan(
        corrections,
        profile,
        profile.correction_transform_plan_budget(),
    )?;
    let candidate_plan = candidate.plan;
    let candidate_codec = candidate.codec;
    let candidate_payload = candidate.payload;

    let candidate_total = candidate_payload.len()
        + if candidate_plan == identity_plan {
            0
        } else {
            TOPOLOGY_NODE_BYTES
        };

    if candidate_total < identity_payload.len() {
        Ok((
            candidate_plan,
            candidate_codec,
            candidate_payload,
            candidate.sampling_ms,
            candidate.eval_ms,
        ))
    } else {
        Ok((identity_plan, identity_codec, identity_payload, 0.0, 0.0))
    }
}
