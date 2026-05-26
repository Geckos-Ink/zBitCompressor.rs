# zBit File Format Technical Description

_Current implementation snapshot: 2026-05-26._

This document describes the binary formats implemented by `zbit-rs` today. It is
intended as an optimization map: it names every field, its bit/byte width, and the
places where future compression work can still pay.

The implementation has three wire formats:

- `.zbpk`: normal adaptive pack file. Current version: **4**.
- `.zbps`: stream pack file built from nested `.zbpk` nodes. Current version: **1**.
- `.zbit`: standalone Boolean circuit model serialization. Current version: **1**.

Unless explicitly stated otherwise:

- Multi-byte integer fields are little-endian.
- `varint u64` means a LEB128-style 7-bit continuation varint:
  `byte[0..6] = value bits`, `byte[7] = 1` when another byte follows.
  It is capped at 10 bytes.
- Bit streams are MSB-first when written by `BitWriter`.
- The symbol-index stream used by `IndexedRaw` and `IndexedCircuit` is the exception:
  symbol ids are packed LSB-first inside each fixed-width index.
- All decoders reject trailing bytes, unsupported flags, out-of-range enum values, and
  output lengths that differ from the declared `original_size`.

## Shared Scalar Encodings

### Fixed integers

| Name | Bytes | Encoding |
|---|---:|---|
| `u8` | 1 | byte |
| `u16` | 2 | little-endian |
| `u32` | 4 | little-endian |
| `u64` | 8 | little-endian |

Magic values are stored as numeric little-endian integers. For example, `.zbpk`
uses `u32` value `0x5A42504B`, not a separately byte-swapped text literal.

### `varint u64`

```text
loop:
  out_byte = value & 0x7f
  value >>= 7
  if value != 0: out_byte |= 0x80
  write out_byte
  stop when value == 0
```

This is used for `.zbpk` header counts/sizes and most method dictionaries.

### Nibble varint

Used inside compact circuit topology bit streams.

Each nibble is `3 value bits + 1 continuation bit`, written MSB-first through the
bit writer. Small values `0..=7` cost 4 bits. Larger values cost 4 bits for each
additional 3 bits of magnitude.

### `bits_for_index(count)`

Number of bits needed to encode an index into `count` alternatives:

```text
if count <= 1: 0 bits
else: ceil(log2(count))
```

This is used for topology parent ids and multi-block plan selectors. A selector
therefore costs zero bits when there is only one possible value.

## `.zbpk` Adaptive Pack, Version 4

### Header

The header is variable length after the fixed prefix.

```text
u32 magic             = 0x5A42504B
u16 version           = 4
u16 flags             = 0
u8  packed_method     bits 0..3 = PackMethod id
                       bits 4..7 = bits_per_symbol
varint unique_count
varint original_size
varint dict_size
varint payload_size
dict[dict_size]
payload[payload_size]
```

The 4-bit method field leaves method ids `12..15` reserved. If more than 16
methods are needed, the format must be bumped again.

### Pack Method Ids

| Id | Method | Dictionary | Payload |
|---:|---|---|---|
| 0 | `raw-copy` | none | original bytes |
| 1 | `indexed-raw` | unique byte table | fixed-width symbol ids |
| 2 | `indexed-circuit` | per-symbol `.zbit` blobs | fixed-width symbol ids |
| 3 | `indexed-huffman` | symbol/code-length pairs | canonical Huffman bit stream |
| 4 | `raw-deflate` | none | zlib/deflate stream |
| 5 | `raw-zstd` | none | zstd stream |
| 6 | `framed-raw` | frame reconstruction dictionary | concatenated frame payload bytes |
| 7 | `recursive-circuit-xz` | framed + preflate + transform dictionary | transformed plain + correction streams |
| 8 | `monotonic-delta` | integer/gap transform dictionary | codec-compressed gap stream |
| 9 | `raw-xz` | none | XZ/LZMA2 stream with `Check::None` |
| 10 | `raw-brotli` | none | Brotli q11 stream |
| 11 | `adaptive-transformed-xz` | transform-plan dictionary | codec-compressed transformed input |

### Method 0: `raw-copy`

Header constraints:

```text
bits_per_symbol = 0
unique_count = 0
dict_size = 0
payload_size = original_size
```

Decoder returns `payload` as-is.

Optimization note: this is the safety baseline. Method selection should never
produce a file larger than the raw-copy candidate unless the caller explicitly
chooses a method outside adaptive selection.

### Method 1: `indexed-raw`

Dictionary:

```text
u8 unique_symbols[unique_count]
```

The unique byte table is sorted by byte value (`0..=255`) because it is built by
scanning the full byte alphabet in order.

Payload:

```text
symbol_id[original_size] packed at bits_per_symbol bits each
```

`bits_per_symbol = bits_needed_for_count(unique_count)`, where the implementation
returns 1 bit for `unique_count <= 1`, then `ceil(log2(unique_count))`.

The index payload packs each symbol id LSB-first:

```text
for bit i in 0..bits_per_symbol:
  payload[(bit_offset + i) / 8] bit ((bit_offset + i) % 8)
```

Optimization note: this method only wins on very low-alphabet data. The byte table
is already minimal at one byte per symbol; the main opportunity would be run/entropy
coding the index stream, which is what `indexed-huffman` partly covers.

### Method 2: `indexed-circuit`

Dictionary:

```text
repeat unique_count times:
  u8  stored_symbol
  u32 blob_len
  u8  blob[blob_len]     # `.zbit` model bytes for that symbol
```

Payload is identical to `indexed-raw`: fixed-width LSB-first symbol ids.

Decoder parses each model blob as `.zbit`, decodes the symbol, and verifies it
matches `stored_symbol`.

Optimization note: today this is mostly architectural scaffolding. For byte-sized
symbols, the raw byte table is usually denser than a circuit blob, so the candidate
is heavily gated.

### Method 3: `indexed-huffman`

Dictionary:

```text
repeat unique_count times:
  u8 symbol
  u8 code_length
```

The decoder reconstructs canonical Huffman codes by sorting entries by
`(code_length, symbol)`. Code length must be in `1..=56`.

Payload:

```text
canonical Huffman codes, MSB-first, padded to final byte
```

The decoder reads codes until exactly `original_size` symbols have been produced.

Optimization note: code lengths are stored byte-aligned. For small alphabets, a
future format could delta-code or RLE the length table, but this only helps when
Huffman is already competitive.

### Methods 4, 5, 9, 10: Raw Codec Payloads

These methods have no dictionary and require:

```text
bits_per_symbol = 0
unique_count = 0
dict_size = 0
```

| Method | Payload codec |
|---|---|
| `raw-deflate` | zlib wrapper through `flate2::ZlibEncoder` |
| `raw-zstd` | zstd stream |
| `raw-xz` | XZ/LZMA2 stream, `Check::None`; selected by tuned matrix |
| `raw-brotli` | Brotli q11, lgwin 22; evaluated only for bounded text-like inputs |

Optimization note: `raw-brotli` is the v4 practical answer to "try compressing the
output format again" for text-like payloads. Directly applying Brotli to the raw
paper corpus wins substantially; wrapping already-dense `.zbpk` artifacts was
measured to lose on paper/primary and save only 58 bytes on the 83 MB depth corpus.

### Method 6: `framed-raw`

This method models a file that contains many repeated framed chunks:

```text
prefix
repeat chunks:
  u32be chunk_len
  u8[4] frame_tag
  u8[chunk_len] chunk_payload
  u32be crc32(frame_tag || chunk_payload)
suffix
```

Dictionary:

```text
varint prefix_len
varint suffix_len
u8[4] frame_tag
varint base_chunk_len
varint full_chunk_count
varint tail_chunk_len
varint total_chunks
u8 prefix[prefix_len]
u8 suffix[suffix_len]
```

Payload:

```text
concatenated chunk_payload bytes, without per-frame length/tag/crc wrappers
```

Reconstruction:

- Emit `prefix`.
- For each full chunk, take `base_chunk_len` bytes from payload.
- If `total_chunks == full_chunk_count + 1`, take one tail chunk of
  `tail_chunk_len` bytes.
- For every chunk, write big-endian length, `frame_tag`, payload bytes, and CRC32.
- Emit `suffix`.

Optimization note: this removes repeated frame wrappers, but it does not alter the
chunk payload itself. The recursive method is the payload-level extension for
deflate-backed framed data.

### Method 7: `recursive-circuit-xz`

This method is used for framed deflate data. It extracts the framed payload,
preflates the inner deflate stream to recover:

- inflated plain bytes
- preflate correction bytes needed to recreate the original deflate stream

It then transforms and codec-compresses the plain and correction streams separately.

Dictionary layout:

```text
# embedded framed-raw dictionary, exactly as method 6:
varint prefix_len
varint suffix_len
u8[4] frame_tag
varint base_chunk_len
varint full_chunk_count
varint tail_chunk_len
varint total_chunks
u8 prefix[prefix_len]
u8 suffix[suffix_len]

# recursive fixed section:
u8[2]  zlib_header
u8[4]  zlib_adler32
varint plain_len
varint correction_plain_len
varint correction_encoded_len
u16 codec_kind
varint transform_period
varint transform_head
u16 topology_field

# topology section:
if topology_field has COMPACT flag:
  u32 topology_bit_len
  u8 topology_bits[ceil(topology_bit_len / 8)]
else:
  legacy topology nodes, 28 bytes each

# optional multi-block section:
if topology_field has MULTI_BLOCK flag:
  bit-packed multi-block plan section
```

`codec_kind` layout:

```text
bits 0..1   transformed_codec
bits 2..3   correction_codec
bits 4..9   transform_kind_index
bits 10..15 reserved = 0
```

`PayloadCodec` ids:

| Id | Codec |
|---:|---|
| 0 | raw |
| 1 | xz |
| 2 | zstd |
| 3 | xz-extreme |

`topology_field` layout:

```text
bit 15      MULTI_BLOCK flag (0x8000)
bit 14      COMPACT flag     (0x4000)
bits 0..13  topology_count
```

Payload layout:

```text
u8 transformed_payload[transformed_encoded_len]
u8 corrections_payload[correction_encoded_len]
```

`transformed_encoded_len` is not serialized. It is recovered as:

```text
payload_size - correction_encoded_len
```

Reconstruction:

1. Decode `transformed_payload` with `transformed_codec` to `plain_len`.
2. Invert the selected transform plan to recover inflated plain bytes.
3. Decode `corrections_payload` with `correction_codec` to
   `correction_plain_len`.
4. Invert the embedded correction transform plan, if any.
5. Use preflate recreation with `zlib_header`, corrections, and plain bytes to
   reconstruct the original deflate stream.
6. Add `zlib_adler32`, then rewrap through the framed dictionary.

#### Compact Topology Section

Compact topology is used when nodes satisfy:

- `order <= 3`
- `kind` maps to the dense 6-bit kind table
- `id == emitted_index + 1`

Each node is written into one continuous MSB-first bit stream:

```text
relation       1 bit       # 0 = series, 1 = parallel
order          2 bits
kind_index     6 bits
is_root        1 bit
if !is_root:
  parent_index bits_for_index(previous_node_count) bits
param_a        nibble varint
param_b        nibble varint
```

Ids are implicit. Hashes are omitted in compact form.

Legacy topology node, used as fallback:

```text
u32 id
u32 parent_id          # u32::MAX means root
u8  relation
u16 order
u8  kind
u32 param_a
u32 param_b
u64 hash64
```

#### Kind Index Mapping

Compact topology and recursive transform headers share this mapping:

```text
kind 0..26                 -> index 0..26
kind 200..222              -> index 27..49
```

The `200..222` range embeds correction transform-plan markers in topology nodes.

Current transform kind ids:

| Id | Transform |
|---:|---|
| 0 | identity |
| 1 | delta-prev |
| 2 | xor-prev |
| 3 | bit-plane-transpose |
| 4 | periodic-head-tail |
| 5 | periodic-gather |
| 6 | periodic-delta |
| 7 | periodic-xor |
| 8 | periodic-gather-delta |
| 9 | periodic-gather-xor |
| 10 | periodic-head-tail-tail-gather |
| 11 | periodic-head-tail-tail-gather-delta |
| 12 | periodic-head-tail-tail-delta |
| 13 | periodic-head-tail-tail-xor |
| 14 | periodic-head-tail-delta |
| 15 | periodic-head-tail-xor |
| 16 | bit-plane-transpose-delta |
| 17 | bit-plane-transpose-xor |
| 18 | periodic-head-tail-tail-row-delta |
| 19 | periodic-head-tail-tail-row-xor |
| 20 | periodic-head-tail-tail-row-up |
| 21 | periodic-head-tail-tail-bit-plane-transpose |
| 22 | periodic-head-tail-tail-bit-plane-transpose-delta |

#### Multi-Block Plan Section

When present, the inflated plain was split into `block_count` consecutive blocks.
Each block has a transform plan; the last block may be shorter.

The entire section is a single MSB-first bit stream:

```text
block_count           4 bits        # max 15
block_size_width      5 bits
block_size            block_size_width bits
plan_table_len_m1     bits_for_index(block_count) bits

repeat plan_table_len:
  kind_index          6 bits

period_width          5 bits
repeat plan_table_len:
  period              period_width bits

head_width            5 bits
repeat plan_table_len:
  head                head_width bits

repeat block_count:
  plan_idx            bits_for_index(plan_table_len) bits

padding to byte boundary
```

The plan table deduplicates repeated plans. If every block uses the same plan,
each per-block selector costs 0 bits.

Optimization note: the next structural step is replacing the independent topology
and multi-block bit streams with one `CircuitBitStream` interning table, so repeated
plans/topologies across sections cost a definition once plus cheap references.

### Method 8: `monotonic-delta`

This method handles fixed-width little-endian strictly increasing integer streams.

Dictionary:

```text
u8 meta
u8 trailing_zero_shift
varint count
varint first_value
varint transformed_plain_len
```

`meta` layout:

```text
bits 0..2   width_minus_1       # width = 1..=8 bytes
bits 3..5   mode
bits 6..7   codec
```

`trailing_zero_shift` uses only bits `0..5`; top two bits must be zero.

Modes:

| Id | Mode | Transformed stream |
|---:|---|---|
| 0 | `GapVarint` | varint gap for every value after first |
| 1 | `GapDeltaVarint` | first gap, then zigzag varint deltas of gaps |
| 2 | `GapBytes` | one byte per gap |
| 3 | `GapTrailingZeroVarint` | first gap, then shifted varint gaps |
| 4 | `GapTrailingZeroBytes` | first gap byte, then shifted one-byte gaps |

Payload is `transformed_plain_len` bytes after decoding with `codec`.

Reconstruction:

1. Decode payload with `codec`.
2. Emit `first_value` in `width` little-endian bytes.
3. Decode each gap, add it to the previous value, and emit the new value.
4. Reject zero/non-positive gaps and values that exceed the selected width.

Optimization note: the transformed gap stream has clear symbol skew. A specialized
entropy code for gap values may beat the general XZ/zstd wrapper for `primary.3b`.

### Method 11: `adaptive-transformed-xz`

This method applies one reversible transform plan directly to the whole input and
then compresses the transformed bytes.

Dictionary:

```text
u8 codec_kind
varint period
varint head
varint plain_len
```

`codec_kind` layout:

```text
bits 0..1   PayloadCodec id
bits 2..7   transform_kind_index
```

Payload:

```text
transformed input bytes encoded with selected PayloadCodec
```

Decoder checks `plain_len == original_size`, decodes the transformed payload, then
inverts the transform plan.

Optimization note: this is the current non-container structural path and is what
wins on the Depth Anything `.pth` corpus. Container-aware tensor parsing would sit
above this and produce more targeted substreams.

## `.zbps` Stream Pack, Version 1

`.zbps` stores a large input as restartable key-piece blocks. Each block contains a
nested stream-node tree. Leaves or grouped nodes are `.zbpk` packs.

### Header

Fixed size: 40 bytes.

```text
u32 magic                 = 0x5A425053
u16 version               = 1
u16 flags
u32 chunk_size
u32 key_piece_interval
u8  max_group_depth
u8[3] reserved            = 0
u32 max_group_pieces
u64 original_size
u32 total_chunks
u32 block_count
```

Flags:

| Bit | Value | Meaning |
|---:|---:|---|
| 0 | `0x0001` | carry grouping history hint |
| 1 | `0x0002` | wide overfitting/global payload mode |
| 2 | `0x0004` | shared grouping payload mode |

If either global/shared payload flag is set, the header is followed by:

```text
u32 global_pack_len
u8  global_pack_bytes[global_pack_len]    # nested `.zbpk`, decodes to original_size bytes
```

### Block Records

Each key-piece block stores:

```text
u32 first_chunk_index
u32 chunk_count
u64 original_len
u8  history_method        # PackMethod id or 0xff
u32 node_bytes_len
u8  node_bytes[node_bytes_len]
```

`first_chunk_index` must align with `key_piece_interval`. This is what allows
restart from a key-piece boundary.

### Stream Nodes

Node ids:

| Id | Node |
|---:|---|
| 0 | piece |
| 1 | group |
| 2 | split |
| 3 | global-slice |

Piece node:

```text
u8  kind = 0
u32 chunk_len
u8  expected_method
u32 pack_len
u8  pack_bytes[pack_len]      # nested `.zbpk`
```

Group node:

```text
u8  kind = 1
u32 chunk_count
u64 original_len
u8  expected_method
u32 pack_len
u8  pack_bytes[pack_len]      # nested `.zbpk`
```

Split node:

```text
u8 kind = 2
u8 level
node left
node right
```

Global-slice node:

```text
u8  kind = 3
u32 chunk_count
u64 original_offset
u64 original_len
```

Global-slice nodes reference byte ranges inside the decoded global `.zbpk` payload.

Optimization note: `.zbps` has considerable fixed per-block/node metadata. It buys
restartability, but it is not yet as bit-packed as `.zbpk`. For stream ratio work,
the best targets are block header compaction, node tag/length varints, and replacing
global-slice output reuse with restart-safe circuit references.

## `.zbit` Circuit Model, Version 1

`.zbit` serializes a canonical Boolean circuit DAG.

Header:

```text
u32 magic       = 0x5A424954
u16 version     = 1
u16 reserved    = 0
u32 num_inputs
u32 root_id
u32 node_count
```

Nodes:

```text
repeat node_count:
  u8  node_type
  u32 value
  u32 input_count
  u32 inputs[input_count]
```

Node type ids:

| Id | Node |
|---:|---|
| 0 | pin |
| 1 | not |
| 2 | and |
| 3 | or |
| 4 | xor |

Rules:

- `num_inputs <= 31`.
- `root_id < node_count`.
- Node arguments must reference earlier nodes only.
- `pin` nodes must have zero inputs.
- `not` nodes must have one input.
- Commutative node inputs are sorted during decode.
- Duplicate canonical nodes are rejected.

Optimization note: `.zbit` is not currently optimized for space. The pack format only
uses it in the `indexed-circuit` dictionary, which is usually gated off for byte
symbols. If circuit dictionaries become live payload data, this should be migrated to
the same `CircuitBitStream` interning primitive described in `zbit-rs/src/pack/bitstream.rs`.

## Current Format Optimization Map

High-value areas:

1. Payload-level modeling, not header shaving. Current cat dictionary overhead is tiny
   relative to the transformed/compressed payload and preflate correction stream.
2. Container-aware transforms. PNG-like framed deflate is modeled; ZIP/PyTorch tensor
   structure is still mostly opaque.
3. Monotonic gap entropy coding. `primary.3b` has a highly skewed gap distribution that
   still goes through a generic codec.
4. CircuitBitStream migration for recursive topology and multi-block plans. This is the
   path to cheap cross-region circuit references.
5. Stream metadata compaction. `.zbps` is much less bit-packed than `.zbpk` because it
   prioritizes restartability and nested pack reuse.

Low-value areas:

1. Further `.zbpk` header bit packing. The v4 header is already around tens of bytes.
2. Generic outer recompression of `.zbpk`. It was measured and did not produce a
   meaningful win on current corpora.
3. More byte-level dictionary compaction inside method dictionaries unless paired with
   a payload/model change.

