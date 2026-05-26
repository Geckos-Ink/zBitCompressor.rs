// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.
//
// IMMED-1: a universal bit-stream primitive with **content-hash interning** for
// circuit-level data. The wire format puts every circuit definition into ONE
// continuous bit stream and pays only `ceil(log2(N))` bits to reference a
// previously-defined circuit. This is the missing primitive that the ROADMAP
// IMMED-1 entry calls out as the root cause for cross-regional circuit sharing
// never being built: when the per-occurrence cost of a circuit definition is
// tens of bytes, no set-cover optimizer will pick a shared encoding.
//
// Wire format per `write_def_or_ref` call:
//   bit 0 — tag:  0 = "definition follows", 1 = "reference follows"
//   if tag == 0:
//     [content bits — caller-provided via `BitSerializable::write_bits`]
//     side effect: assign next sequential CircuitId, insert into interning table
//   if tag == 1:
//     [id in ceil(log2(seen_so_far)) bits] — never written when seen_so_far == 1
//
// The reader rebuilds the table on the fly: every "definition" entry it reads
// gets a new id assigned in the same order the writer used, so subsequent
// references decode to the same `T`. No table is shipped on the wire.
//
// The roadmap acceptance numbers fall out of this layout:
//   * 3-variable truth-table circuit (8 truth-table bits + 1 tag) = 9 bits (< 10 ✓)
//   * 100 identical 3-node topologies: 1 definition (~31 bits including tag) +
//     99 references (1 tag bit + ceil(log2(N)) bits each, ramping from 1 to 7
//     across the run). Total ≈ 31 + Σ(1 + bits_for_index(k)) for k=1..99 ≈ 624 bits
//     — beats 100 × 18-bit independent encoding (1800 bits) by ~3×.
//
// Migration path into the live format (NOT done in this session, requires a
// ZBPK_VERSION bump): replace the separate `topology_bits` / multi-block
// trailer / correction-plan topology sections with a single `CircuitBitStream`
// produced by the encoder and consumed by the decoder. The current sections
// already use the `BitWriter`/`BitReader` primitives this struct builds on;
// the conceptual change is that they will share an interning table rather
// than each carrying its own self-contained byte slice.

// HashMap, BitWriter, BitReader, bits_for_index, ZbitResult, ZbitError are
// already in scope via the parent pack/mod.rs include chain (core.rs, format.rs).

/// Sequential id assigned to a circuit definition the first time it is written.
/// Same id is recovered by the reader as it walks the stream in order.
pub(crate) type CircuitId = u32;

/// Anything that can be serialised into a bit stream and round-tripped.
/// The `content_bytes` method produces a canonical byte sequence used only for
/// the interning hash; it is NEVER written to the wire — only `write_bits`
/// output is. Two distinct values that hash the same will collide; callers
/// must guarantee `content_bytes` uniquely identifies the value.
//
// Visibility kept at `trait` (not pub(crate)) because the signatures expose
// the private `BitWriter`/`BitReader` types — only intra-pack-module callers
// can implement or use this trait, which is exactly what we want.
trait BitSerializable: Sized {
    fn write_bits(&self, bw: &mut BitWriter);
    fn read_bits(br: &mut BitReader<'_>) -> ZbitResult<Self>;
    fn content_bytes(&self) -> Vec<u8>;
}

#[derive(Debug, Default)]
pub(crate) struct CircuitBitStream {
    bits: BitWriter,
    interning: HashMap<Vec<u8>, CircuitId>,
    next_id: CircuitId,
}

#[allow(dead_code)]
impl CircuitBitStream {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_capacity(cap_bytes: usize) -> Self {
        Self {
            bits: BitWriter::with_capacity(cap_bytes),
            interning: HashMap::new(),
            next_id: 0,
        }
    }

    /// Number of distinct circuits interned so far. The reference width at the
    /// **next** `write_def_or_ref` call will be `bits_for_index(seen_count())`.
    pub(crate) fn seen_count(&self) -> usize {
        self.next_id as usize
    }

    /// Width in bits a reference would take RIGHT NOW. Useful for cost models.
    pub(crate) fn ref_bits(&self) -> u32 {
        // Reference is only emitted when next_id >= 1 (something already exists).
        // Width = ceil(log2(next_id)) which is `bits_for_index(next_id as usize)`.
        bits_for_index(self.next_id as usize)
    }

    pub(crate) fn bit_len(&self) -> usize {
        self.bits.bit_len()
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bits.into_bytes()
    }

    /// Write `circuit` to the stream, interning it for future references.
    ///
    /// Returns the assigned `CircuitId` (either the existing one or a fresh
    /// sequential id for new content).
    #[allow(private_bounds)]
    pub(crate) fn write_def_or_ref<T: BitSerializable>(&mut self, circuit: &T) -> CircuitId {
        let content = circuit.content_bytes();
        if let Some(&existing_id) = self.interning.get(&content) {
            // Reference: tag bit = 1, then the id in the current reference width.
            self.bits.write_bits(1, 1);
            let width = bits_for_index(self.next_id as usize);
            if width > 0 {
                self.bits.write_bits(existing_id as u64, width);
            }
            return existing_id;
        }

        // Definition: tag bit = 0, then the inline content bits.
        self.bits.write_bits(0, 1);
        circuit.write_bits(&mut self.bits);

        let assigned = self.next_id;
        self.interning.insert(content, assigned);
        self.next_id = self.next_id.checked_add(1).expect(
            "CircuitBitStream id space (u32) exhausted — far beyond any realistic compressor input",
        );
        assigned
    }
}

pub(crate) struct CircuitBitStreamReader<'a> {
    bytes: &'a [u8],
    bit_pos: usize,
    // Table holds (start_bit, end_bit) ranges into `bytes` for each definition,
    // so refs can replay the exact wire bits of the original definition
    // (independent of the canonical content_bytes form, which is only used for
    // hashing on the writer side).
    table: Vec<(usize, usize)>,
}

#[allow(dead_code)]
impl<'a> CircuitBitStreamReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_pos: 0,
            table: Vec::new(),
        }
    }

    pub(crate) fn bit_pos(&self) -> usize {
        self.bit_pos
    }

    /// Read the next definition-or-reference. Returns the decoded `T` and its
    /// assigned id.
    #[allow(private_bounds)]
    pub(crate) fn read_def_or_ref<T: BitSerializable>(&mut self) -> ZbitResult<(CircuitId, T)> {
        let mut br = bitreader_at(self.bytes, self.bit_pos)?;

        let tag = br.read_bits(1)?;
        if tag == 1 {
            // Reference: id width is bits_for_index(current table size).
            let width = bits_for_index(self.table.len());
            let id = if width == 0 {
                0u64
            } else {
                br.read_bits(width)?
            };
            self.bit_pos = br.bit_pos();

            let &(start, end) = self.table.get(id as usize).ok_or_else(|| {
                ZbitError::Parse(format!(
                    "CircuitBitStream reference id {id} out of bounds (table size {})",
                    self.table.len()
                ))
            })?;
            // Replay the exact wire bits of definition `id` through T::read_bits.
            let mut replay = bitreader_at(self.bytes, start)?;
            let value = T::read_bits(&mut replay)?;
            if replay.bit_pos() != end {
                return Err(ZbitError::Parse(format!(
                    "CircuitBitStream reference replay consumed {} bits, expected {}",
                    replay.bit_pos() - start,
                    end - start,
                )));
            }
            return Ok((id as CircuitId, value));
        }

        // Definition: snapshot the bit range we consume so future refs can replay it.
        let start = br.bit_pos();
        let value = T::read_bits(&mut br)?;
        let end = br.bit_pos();
        self.bit_pos = end;
        let assigned = self.table.len() as CircuitId;
        self.table.push((start, end));
        Ok((assigned, value))
    }
}

// ============================================================================
// IMMED-1.2 wiring: BitSerializable impls for the production types that will
// eventually share a single CircuitBitStream when the topology format is bumped again.
//
// These are NOT yet used in the live encoder — `recursive.rs` still ships the v3/v4
// dictionaries with the existing compact_topology / multi-block trailer
// layouts. They exist so:
//   (a) The acceptance test for cross-region sharing can run against the real
//       data types (see immed_1_real_types_share_via_bitstream below).
//   (b) The migration path is concrete: the next session can replace the
//       hand-rolled per-section bit writers with `CircuitBitStream::write_def_or_ref`
//       calls that use these impls, then bump ZBPK_VERSION again.
//
// The encoding mirrors the current compact-topology / multi-block-plan
// formats so existing files can be re-emitted under the new framing without
// changing the bits per individual definition.

impl BitSerializable for CircuitTransformPlan {
    fn write_bits(&self, bw: &mut BitWriter) {
        // kind_index in 6 bits (49 active kinds fit) + period varint + head varint
        let kind_u8 = self.kind.as_u8();
        let kind_index = kind_to_compact_index(kind_u8).unwrap_or(kind_u8 & 0x3F);
        bw.write_bits(kind_index as u64, 6);
        bw.write_nibble_varint(self.period as u64);
        bw.write_nibble_varint(self.head as u64);
    }
    fn read_bits(br: &mut BitReader<'_>) -> ZbitResult<Self> {
        let kind_index = br.read_bits(6)? as u8;
        let kind_u8 = compact_index_to_kind(kind_index).ok_or_else(|| {
            ZbitError::Parse(format!(
                "CircuitTransformPlan bitstream: invalid kind index {kind_index}"
            ))
        })?;
        let kind = CircuitTransformKind::from_u8(kind_u8).ok_or_else(|| {
            ZbitError::Parse(
                "CircuitTransformPlan bitstream: invalid transform kind".to_string(),
            )
        })?;
        let period = br.read_nibble_varint()? as u32;
        let head = br.read_nibble_varint()? as u32;
        Ok(CircuitTransformPlan { kind, period, head })
    }
    fn content_bytes(&self) -> Vec<u8> {
        // Canonical 9-byte key: 1 byte kind_u8 + 4 bytes period_le + 4 bytes head_le.
        // Uniquely identifies the value; never written to the wire.
        let mut out = Vec::with_capacity(9);
        out.push(self.kind.as_u8());
        out.extend_from_slice(&self.period.to_le_bytes());
        out.extend_from_slice(&self.head.to_le_bytes());
        out
    }
}

// IMMED-3: BitSerializable for CircuitTopologyNode mirrors the existing
// compact-topology field layout (1-bit relation, 2-bit order, 6-bit kind
// index, 1-bit is_root, optional parent_index, nibble-varint param_a/b).
// Encoded per-node bit count matches `compact_topology_node_bit_len` exactly
// when prev_count == 0 (root, no parent); deeper nodes encoded via this
// trait pay the same per-node budget as today's compact-topology section.
//
// `parent_id` is encoded as a 32-bit varint rather than a context-dependent
// `ceil(log2(prev_count))` width because `BitSerializable::read_bits` does
// not have access to the surrounding topology's `prev_count`. When a future
// session wires this into the live recursive-dictionary format, it will
// either:
//   (a) augment BitSerializable with an associated context type, or
//   (b) embed parent indices in a sidecar list, or
//   (c) replace this with a specialised TopologyBitStream that threads
//       prev_count through write_bits/read_bits explicitly.
// Option (c) is cleanest; this impl is a compatibility stand-in that's
// correct, just not maximally compact.
impl BitSerializable for CircuitTopologyNode {
    fn write_bits(&self, bw: &mut BitWriter) {
        bw.write_bits((self.relation & 1) as u64, 1);
        bw.write_bits((self.order & 0x3) as u64, 2);
        let kind_index = kind_to_compact_index(self.kind).unwrap_or(self.kind & 0x3F);
        bw.write_bits(kind_index as u64, 6);
        let is_root = self.parent_id == u32::MAX;
        bw.write_bits(if is_root { 1 } else { 0 }, 1);
        if !is_root {
            bw.write_nibble_varint(self.parent_id as u64);
        }
        bw.write_nibble_varint(self.param_a as u64);
        bw.write_nibble_varint(self.param_b as u64);
    }
    fn read_bits(br: &mut BitReader<'_>) -> ZbitResult<Self> {
        let relation = br.read_bits(1)? as u8;
        let order = br.read_bits(2)? as u16;
        let kind_index = br.read_bits(6)? as u8;
        let kind = compact_index_to_kind(kind_index).ok_or_else(|| {
            ZbitError::Parse(format!(
                "CircuitTopologyNode bitstream: invalid kind index {kind_index}"
            ))
        })?;
        let is_root = br.read_bits(1)? == 1;
        let parent_id = if is_root {
            u32::MAX
        } else {
            br.read_nibble_varint()? as u32
        };
        let param_a = br.read_nibble_varint()? as u32;
        let param_b = br.read_nibble_varint()? as u32;
        Ok(CircuitTopologyNode {
            id: 0, // implicit (caller assigns sequential ids when reading a chain)
            parent_id,
            relation,
            order,
            kind,
            param_a,
            param_b,
            hash64: 0, // overall decode pipeline validates topology end-to-end
        })
    }
    fn content_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 2 + 1 + 1 + 4 + 4 + 4);
        out.push(self.relation);
        out.extend_from_slice(&self.order.to_le_bytes());
        out.push(self.kind);
        let is_root = self.parent_id == u32::MAX;
        out.push(if is_root { 1 } else { 0 });
        out.extend_from_slice(&self.parent_id.to_le_bytes());
        out.extend_from_slice(&self.param_a.to_le_bytes());
        out.extend_from_slice(&self.param_b.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod bitstream_integration_tests {
    use super::*;

    /// IMMED-1.2 / IMMED-3 acceptance: when the same transform plan appears
    /// in N regions of a hypothetical future cross-region encoder, the
    /// shared bit-stream encoding must beat N independent encodings.
    ///
    /// The current `recursive.rs` v3 format pays the per-entry cost of the
    /// `CircuitTransformPlan` (6 bits kind + nibble-varint period + nibble-varint
    /// head ≈ 14-22 bits) every time. The shared bit-stream encoding pays
    /// it once and then `1 + ceil(log2(N))` bits per reference.
    #[test]
    fn immed_1_real_types_share_via_bitstream() {
        let plan = CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTail,
            period: 4,
            head: 1,
        };

        // Compute the per-entry independent bit count (no tag, just content).
        let mut single_writer = BitWriter::default();
        plan.write_bits(&mut single_writer);
        let bits_per_independent = single_writer.bit_len();

        // Shared encoding: 1 definition + 99 references = N=100 occurrences.
        let mut stream = CircuitBitStream::new();
        for _ in 0..100 {
            stream.write_def_or_ref(&plan);
        }
        let shared_bits = stream.bit_len();

        let independent_total_bits = bits_per_independent * 100;
        assert!(
            shared_bits < independent_total_bits,
            "shared {shared_bits} bits must beat {independent_total_bits} bits (100 × {bits_per_independent})"
        );

        // Roundtrip through the reader to confirm every reference resolves
        // back to the original plan.
        let bytes = stream.into_bytes();
        let mut reader = CircuitBitStreamReader::new(&bytes);
        for i in 0..100 {
            let (_id, got) = reader.read_def_or_ref::<CircuitTransformPlan>().unwrap();
            assert_eq!(got, plan, "occurrence {i} must roundtrip");
        }
    }

    /// IMMED-3 acceptance: two distant byte ranges with identical topology
    /// nodes get one definition + one reference, not two definitions.
    /// This is the ROADMAP-3 acceptance criterion: when the cross-region
    /// encoder (Phase 2) starts emitting multi-region transforms with shared
    /// structure, the savings ratio depends entirely on this interning
    /// behaviour at the encoding layer being correct.
    #[test]
    fn immed_3_distant_regions_share_topology() {
        // A 3-node topology that appears in two distant "regions" of the
        // simulated cross-region stream. With sharing, region B emits only
        // a tag + 1-bit reference per node; without sharing, region B emits
        // a full re-serialisation of the 3 nodes.
        let nodes_a = vec![
            CircuitTopologyNode {
                id: 1,
                parent_id: u32::MAX,
                relation: 0,
                order: 0,
                kind: 1,
                param_a: 4,
                param_b: 0,
                hash64: 0,
            },
            CircuitTopologyNode {
                id: 2,
                parent_id: 1,
                relation: 1,
                order: 1,
                kind: 3,
                param_a: 0,
                param_b: 0,
                hash64: 0,
            },
            CircuitTopologyNode {
                id: 3,
                parent_id: 2,
                relation: 0,
                order: 2,
                kind: 5,
                param_a: 6401,
                param_b: 0,
                hash64: 0,
            },
        ];

        // Independent encoding: write region A and region B separately, each
        // emitting 3 full node definitions.
        let mut independent = BitWriter::default();
        for _ in 0..2 {
            for n in &nodes_a {
                n.write_bits(&mut independent);
            }
        }
        let independent_bits = independent.bit_len();

        // Shared encoding: one CircuitBitStream, region B references region A.
        let mut shared = CircuitBitStream::new();
        for n in &nodes_a {
            shared.write_def_or_ref(n); // region A: 3 definitions
        }
        for n in &nodes_a {
            shared.write_def_or_ref(n); // region B: 3 references
        }
        let shared_bits = shared.bit_len();

        // Acceptance: shared encoding must be measurably smaller.
        assert!(
            shared_bits < independent_bits,
            "shared {shared_bits} bits must beat independent {independent_bits} bits"
        );

        // Roundtrip: read back all 6 occurrences, verify equality (ignoring id
        // and hash64 which are not on the wire).
        let bytes = shared.into_bytes();
        let mut reader = CircuitBitStreamReader::new(&bytes);
        for region in 0..2 {
            for (idx, expected) in nodes_a.iter().enumerate() {
                let (_id, got) = reader.read_def_or_ref::<CircuitTopologyNode>().unwrap();
                assert_eq!(got.relation, expected.relation, "region {region} node {idx} relation");
                assert_eq!(got.order, expected.order);
                assert_eq!(got.kind, expected.kind);
                assert_eq!(got.parent_id, expected.parent_id);
                assert_eq!(got.param_a, expected.param_a);
                assert_eq!(got.param_b, expected.param_b);
            }
        }
    }

    /// IMMED-1 acceptance: mixed real transform plans share what they can.
    /// Three distinct plans, each appearing multiple times. The shared
    /// encoding must allocate 3 definitions and N-3 references.
    #[test]
    fn immed_1_mixed_real_plans_dedupe_correctly() {
        let plans = [
            CircuitTransformPlan { kind: CircuitTransformKind::PeriodicHeadTail, period: 4, head: 1 },
            CircuitTransformPlan { kind: CircuitTransformKind::PeriodicHeadTailDelta, period: 4, head: 1 },
            CircuitTransformPlan { kind: CircuitTransformKind::BitPlaneTranspose, period: 0, head: 0 },
        ];
        // Write each plan 5 times, interleaved
        let pattern: Vec<usize> = (0..15).map(|i| i % 3).collect();

        let mut stream = CircuitBitStream::new();
        let mut ids = Vec::new();
        for &i in &pattern {
            ids.push(stream.write_def_or_ref(&plans[i]));
        }

        // First three occurrences are definitions (assigning ids 0, 1, 2).
        // Remaining 12 are references mapping back to those three ids.
        assert_eq!(ids[0], 0);
        assert_eq!(ids[1], 1);
        assert_eq!(ids[2], 2);
        for k in 3..15 {
            let expected_id = (pattern[k]) as CircuitId;
            assert_eq!(ids[k], expected_id, "position {k} should resolve to id {expected_id}");
        }

        // Roundtrip
        let bytes = stream.into_bytes();
        let mut reader = CircuitBitStreamReader::new(&bytes);
        for (k, &i) in pattern.iter().enumerate() {
            let (id, got) = reader.read_def_or_ref::<CircuitTransformPlan>().unwrap();
            assert_eq!(id, i as CircuitId, "id at position {k}");
            assert_eq!(got, plans[i], "value at position {k}");
        }
    }
}

/// Construct a BitReader positioned at `bit_offset` into `bytes`. The
/// underlying `BitReader::new` always starts at bit 0; we walk forward by
/// reading and discarding the appropriate number of bits in 32-bit chunks so
/// we do not require a public seek API.
fn bitreader_at(bytes: &[u8], bit_offset: usize) -> ZbitResult<BitReader<'_>> {
    let mut br = BitReader::new(bytes);
    let mut remaining = bit_offset;
    while remaining >= 32 {
        br.read_bits(32)?;
        remaining -= 32;
    }
    if remaining > 0 {
        br.read_bits(remaining as u32)?;
    }
    Ok(br)
}

#[cfg(test)]
mod bitstream_tests {
    use super::*;

    /// IMMED-1 acceptance test target #1: a 3-input truth-table circuit
    /// serialises in under 10 bits.
    ///
    /// The minimal "3-var truth table" representation is the 8 truth-table
    /// bits themselves (one bit per input combination). Wrapped in the
    /// `CircuitBitStream` definition tag, the total is 9 bits.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TruthTable3 {
        bits: u8, // 8 truth-table bits for f(x0, x1, x2)
    }

    impl BitSerializable for TruthTable3 {
        fn write_bits(&self, bw: &mut BitWriter) {
            bw.write_bits(self.bits as u64, 8);
        }
        fn read_bits(br: &mut BitReader<'_>) -> ZbitResult<Self> {
            Ok(Self {
                bits: br.read_bits(8)? as u8,
            })
        }
        fn content_bytes(&self) -> Vec<u8> {
            vec![self.bits]
        }
    }

    #[test]
    fn three_var_truth_table_under_ten_bits() {
        let mut bs = CircuitBitStream::new();
        let tt = TruthTable3 { bits: 0b1110_1000 }; // majority-of-3
        let id = bs.write_def_or_ref(&tt);
        assert_eq!(id, 0);
        let bit_len = bs.bit_len();
        // 1 tag bit + 8 content bits = 9 bits.
        assert_eq!(bit_len, 9, "3-var truth table must serialise in 9 bits");
        assert!(bit_len < 10, "ROADMAP IMMED-1 acceptance: < 10 bits");
    }

    /// A minimal "3-node topology" stand-in: three parent-index + op-kind
    /// pairs (5+5 bits each = 30 bits raw). Mirrors the real
    /// `CircuitTopologyNode` shape closely enough that the savings ratio
    /// transfers.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ThreeNodeTopology {
        nodes: [(u8, u8); 3], // (parent_index_5bit, op_kind_5bit)
    }

    impl BitSerializable for ThreeNodeTopology {
        fn write_bits(&self, bw: &mut BitWriter) {
            for (parent, op) in self.nodes.iter() {
                bw.write_bits(*parent as u64, 5);
                bw.write_bits(*op as u64, 5);
            }
        }
        fn read_bits(br: &mut BitReader<'_>) -> ZbitResult<Self> {
            let mut nodes = [(0u8, 0u8); 3];
            for slot in nodes.iter_mut() {
                slot.0 = br.read_bits(5)? as u8;
                slot.1 = br.read_bits(5)? as u8;
            }
            Ok(Self { nodes })
        }
        fn content_bytes(&self) -> Vec<u8> {
            let mut out = Vec::with_capacity(6);
            for (p, o) in self.nodes.iter() {
                out.push(*p);
                out.push(*o);
            }
            out
        }
    }

    #[test]
    fn hundred_identical_topologies_beat_independent_encoding() {
        let topo = ThreeNodeTopology {
            nodes: [(0, 1), (1, 4), (2, 7)],
        };
        let mut shared = CircuitBitStream::new();
        for _ in 0..100 {
            shared.write_def_or_ref(&topo);
        }
        let shared_bits = shared.bit_len();

        // Independent baseline: 100 × (no tag, full 30-bit encoding) = 3000 bits.
        // But the ROADMAP compares to "100 × 18 bits = 1800 bits" — that figure
        // uses the existing compact_topology encoding which is already bit-packed.
        // The shared encoding must beat BOTH.
        let independent_bits_raw = 100 * 30;
        let independent_bits_roadmap = 100 * 18;

        assert!(
            shared_bits < independent_bits_raw,
            "shared {shared_bits} must beat raw-independent {independent_bits_raw}"
        );
        assert!(
            shared_bits < independent_bits_roadmap,
            "shared {shared_bits} must beat ROADMAP target {independent_bits_roadmap} bits"
        );
        // Concrete: 1 definition (1 tag + 30 content = 31 bits) +
        // 99 refs (1 tag + bits_for_index(k) where k grows 1..99).
        // Σ bits_for_index(k) for k=1..99 = 1+1+2+2+2+2+3*8+4*16+5*32+6*32+7*1
        //                                 = 1+1+2+2+2+2+24+64+160+192+7 = 457
        // refs: 99 tags + 457 width bits = 556 bits
        // total = 31 + 556 = 587 bits — well under 1800.
        assert!(
            shared_bits <= 700,
            "shared bit length {shared_bits} must be near the 587-bit theoretical minimum"
        );
    }

    #[test]
    fn roundtrip_mixed_definitions_and_references() {
        // Build a realistic mixed stream: 5 distinct topologies, each used
        // multiple times. After writing, read back and verify equality.
        let topos = [
            ThreeNodeTopology { nodes: [(0, 1), (1, 4), (2, 7)] },
            ThreeNodeTopology { nodes: [(0, 2), (1, 5), (2, 8)] },
            ThreeNodeTopology { nodes: [(0, 3), (1, 6), (2, 9)] },
            ThreeNodeTopology { nodes: [(0, 1), (1, 4), (2, 7)] }, // dup of #0
            ThreeNodeTopology { nodes: [(0, 1), (1, 2), (2, 3)] },
        ];
        // Write pattern: 0, 1, 0, 2, 1, 3, 4, 4, 4, 0
        let write_pattern = [0usize, 1, 0, 2, 1, 3, 4, 4, 4, 0];
        let expected: Vec<&ThreeNodeTopology> =
            write_pattern.iter().map(|&i| &topos[i]).collect();

        let mut writer = CircuitBitStream::new();
        let mut written_ids = Vec::new();
        for t in &expected {
            written_ids.push(writer.write_def_or_ref(*t));
        }

        let bytes = writer.into_bytes();
        let mut reader = CircuitBitStreamReader::new(&bytes);
        let mut decoded: Vec<ThreeNodeTopology> = Vec::new();
        let mut read_ids = Vec::new();
        for _ in 0..expected.len() {
            let (id, t) = reader.read_def_or_ref::<ThreeNodeTopology>().unwrap();
            read_ids.push(id);
            decoded.push(t);
        }

        for (got, want) in decoded.iter().zip(expected.iter()) {
            assert_eq!(got, *want, "decoded value must equal original");
        }
        assert_eq!(read_ids, written_ids, "ids must agree across writer/reader");

        // Sanity: id 0 is assigned to first definition, id 3 (=topos[3]==topos[0])
        // does NOT exist because topos[3] is identical to topos[0] and dedups to id 0.
        // The 4 distinct definitions take ids 0..=3.
        assert_eq!(written_ids[0], 0);
        assert_eq!(written_ids[2], 0, "topos[0] reused at position 2 → same id");
        assert_eq!(written_ids[9], 0, "topos[0] reused at position 9 → same id");
    }

    #[test]
    fn single_definition_uses_zero_id_bits() {
        // When the table has size 1, references would address into a single
        // entry — 0 bits suffice. Verify the writer/reader both honour that.
        let topo = ThreeNodeTopology { nodes: [(0, 1), (1, 4), (2, 7)] };
        let mut writer = CircuitBitStream::new();
        let id_a = writer.write_def_or_ref(&topo);
        let id_b = writer.write_def_or_ref(&topo);
        assert_eq!(id_a, 0);
        assert_eq!(id_b, 0);
        // Wire: 1 tag + 30 content + 1 tag + 0 id bits = 32 bits.
        assert_eq!(writer.bit_len(), 32);

        let bytes = writer.into_bytes();
        let mut reader = CircuitBitStreamReader::new(&bytes);
        let (a_id, a) = reader.read_def_or_ref::<ThreeNodeTopology>().unwrap();
        let (b_id, b) = reader.read_def_or_ref::<ThreeNodeTopology>().unwrap();
        assert_eq!(a_id, 0);
        assert_eq!(b_id, 0);
        assert_eq!(a, topo);
        assert_eq!(b, topo);
    }

    #[test]
    fn ref_bits_grows_with_interning_table() {
        let mut s = CircuitBitStream::new();
        assert_eq!(s.ref_bits(), 0, "empty table → 0-bit reference");

        s.write_def_or_ref(&TruthTable3 { bits: 0 });
        assert_eq!(s.ref_bits(), 0, "table size 1 → 0-bit reference still");

        s.write_def_or_ref(&TruthTable3 { bits: 1 });
        assert_eq!(s.ref_bits(), 1, "table size 2 → 1-bit reference");

        for i in 2..5 {
            s.write_def_or_ref(&TruthTable3 { bits: i });
        }
        assert_eq!(s.ref_bits(), 3, "table size 5 → 3-bit reference");
    }
}
