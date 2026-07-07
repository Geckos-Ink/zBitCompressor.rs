// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u8> {
    let b = *bytes
        .get(*cursor)
        .ok_or_else(|| ZbitError::Parse("unexpected end of pack".to_string()))?;
    *cursor += 1;
    Ok(b)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u16> {
    let slice = bytes
        .get(*cursor..(*cursor + 2))
        .ok_or_else(|| ZbitError::Parse("unexpected end of pack".to_string()))?;
    *cursor += 2;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u32> {
    let slice = bytes
        .get(*cursor..(*cursor + 4))
        .ok_or_else(|| ZbitError::Parse("unexpected end of pack".to_string()))?;
    *cursor += 4;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u64> {
    let slice = bytes
        .get(*cursor..(*cursor + 8))
        .ok_or_else(|| ZbitError::Parse("unexpected end of pack".to_string()))?;
    *cursor += 8;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

fn push_varint_u64(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Predicts the byte length of `push_varint_u64(value)` without allocating; used by the
/// `*_dictionary_size` helpers when we need the exact dictionary byte count for candidate
/// selection without serialising the whole dictionary twice.
fn varint_u64_len(value: u64) -> usize {
    let mut len = 1usize;
    let mut remaining = value >> 7;
    while remaining != 0 {
        len += 1;
        remaining >>= 7;
    }
    len
}

// --- Bit-level writer/reader ---------------------------------------------------------
// MSB-first bit packing. Used by the bit-packed circuit-topology serialiser so every
// field consumes only the bits it actually needs (relation:1, order:2, kind:6, etc.)
// instead of fixed byte-aligned slots. The writer pads the final byte to a byte
// boundary; the reader knows the bit count from the surrounding format header (number
// of nodes + per-field bit widths) and stops before consuming any padding bits.

#[derive(Debug, Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit_pos: usize,
}

#[allow(dead_code)]
impl BitWriter {
    fn with_capacity(cap_bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(cap_bytes),
            bit_pos: 0,
        }
    }

    fn bit_len(&self) -> usize {
        self.bit_pos
    }

    fn write_bits(&mut self, value: u64, num_bits: u32) {
        debug_assert!(num_bits <= 64);
        if num_bits == 0 {
            return;
        }
        for i in (0..num_bits).rev() {
            let bit = ((value >> i) & 1) as u8;
            let byte_idx = self.bit_pos >> 3;
            let bit_in_byte = 7 - (self.bit_pos & 7);
            if byte_idx >= self.bytes.len() {
                self.bytes.push(0);
            }
            self.bytes[byte_idx] |= bit << bit_in_byte;
            self.bit_pos += 1;
        }
    }

    /// Writes `value` as a sequence of 4-bit nibbles: 3 value bits + 1 continuation bit
    /// each. Small values (0..=7) cost a single 4-bit nibble; larger values pay 4 bits
    /// per additional 3 bits of magnitude. Suitable for parameter fields that are
    /// usually small (head=1, distance=4) but occasionally large (period=6401).
    fn write_nibble_varint(&mut self, mut value: u64) {
        loop {
            let chunk = (value & 0x07) as u8;
            value >>= 3;
            let has_more = value != 0;
            self.write_bits(chunk as u64, 3);
            self.write_bits(if has_more { 1 } else { 0 }, 1);
            if !has_more {
                break;
            }
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit_pos: usize,
    bit_limit: usize,
}

#[allow(dead_code)]
impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_pos: 0,
            bit_limit: bytes.len() * 8,
        }
    }

    fn bit_pos(&self) -> usize {
        self.bit_pos
    }

    fn read_bits(&mut self, num_bits: u32) -> ZbitResult<u64> {
        debug_assert!(num_bits <= 64);
        if num_bits == 0 {
            return Ok(0);
        }
        let new_pos = self.bit_pos.checked_add(num_bits as usize).ok_or_else(|| {
            ZbitError::Parse("bit reader position overflow".to_string())
        })?;
        if new_pos > self.bit_limit {
            return Err(ZbitError::Parse(
                "bit reader ran past end of buffer".to_string(),
            ));
        }
        let mut value: u64 = 0;
        for _ in 0..num_bits {
            let byte_idx = self.bit_pos >> 3;
            let bit_in_byte = 7 - (self.bit_pos & 7);
            let bit = (self.bytes[byte_idx] >> bit_in_byte) & 1;
            value = (value << 1) | bit as u64;
            self.bit_pos += 1;
        }
        Ok(value)
    }

    fn read_nibble_varint(&mut self) -> ZbitResult<u64> {
        let mut value: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            let chunk = self.read_bits(3)?;
            let cont = self.read_bits(1)?;
            let shifted = chunk.checked_shl(shift).ok_or_else(|| {
                ZbitError::Parse("nibble varint shift overflow".to_string())
            })?;
            value |= shifted;
            if cont == 0 {
                return Ok(value);
            }
            shift = shift.checked_add(3).ok_or_else(|| {
                ZbitError::Parse("nibble varint shift overflow".to_string())
            })?;
            if shift > 63 {
                return Err(ZbitError::Parse(
                    "nibble varint exceeds 64-bit range".to_string(),
                ));
            }
        }
    }
}

/// Number of bits required to encode an index in `0..count`. 1 bit for any count <= 2,
/// then ceil(log2(count)). Used to compute the parent-index field width for each node.
fn bits_for_index(count: usize) -> u32 {
    if count <= 1 {
        return 0;
    }
    // count >= 2 → ceil(log2(count))
    let mut bits = 0u32;
    let mut value = count - 1;
    while value > 0 {
        bits += 1;
        value >>= 1;
    }
    bits
}

fn read_varint_u64(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u64> {
    let mut shift = 0u32;
    let mut value = 0u64;

    for _ in 0..10 {
        let byte = read_u8(bytes, cursor)?;
        let chunk = (byte & 0x7F) as u64;
        let shifted = chunk.checked_shl(shift).ok_or_else(|| {
            ZbitError::Parse("varint shift overflow while decoding u64".to_string())
        })?;
        value |= shifted;
        if (byte & 0x80) == 0 {
            return Ok(value);
        }
        shift = shift.saturating_add(7);
    }

    Err(ZbitError::Parse(
        "varint exceeds 10-byte u64 representation".to_string(),
    ))
}
