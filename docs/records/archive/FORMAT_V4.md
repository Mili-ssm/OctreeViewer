> Historical specification. The current reader/writer supports **v5 only**; see [FORMAT_V5.md](../../reference/format/FORMAT_V5.md). V4 files must be regenerated.

# Native `.oct` format, version 4

This document specifies the complete version 4 file format. V4 was the only format version accepted or written before the v5 migration. Older `.oct` versions are not read; regenerate them from their LAS/LAZ source or convert the EPT dataset again.

V4 stores one shared dataset frame, hierarchy, schema and metadata index, and independent payloads for each block: one geometry payload and one payload per schema attribute. Geometry and attribute payloads can therefore be read, checked and decompressed independently.

All integer fields and floating-point bit patterns are little endian. Offsets and lengths are unsigned 64-bit byte counts. Strings are UTF-8 preceded by a `u32` byte length, without a terminator. A zero length represents an absent optional string. All CRCs use reflected IEEE CRC-32 (polynomial `0xEDB88320`, initial value and final XOR `0xFFFFFFFF`); CRC detects accidental corruption, not malicious modification.

## File structure

```text
24-byte header | contiguous block payloads | index | 40-byte footer
```

### Header (24 bytes)

| Offset | Field | Type | Required value |
| ---: | --- | --- | --- |
| 0 | Magic | 8 bytes | ASCII `OCTVWFRM` |
| 8 | Version | `u16` | `4` |
| 10 | Header length | `u16` | `24` |
| 12 | Flags | `u32` | `0` |
| 16 | Reserved | `u32` | `0` |
| 20 | Header CRC | `u32` | CRC of bytes 0–19 |

Any other version, magic, header length, flags, reserved value or checksum is rejected.

### Footer (40 bytes)

The footer is the final 40 bytes of the file.

| Offset | Field | Type | Meaning |
| ---: | --- | --- | --- |
| 0 | Magic | 8 bytes | ASCII `OCTVEND4` |
| 8 | Index offset | `u64` | Absolute index start |
| 16 | Index length | `u64` | Index byte length, excluding footer |
| 24 | File length | `u64` | Exact length including footer |
| 32 | Index CRC | `u32` | CRC of all index bytes |
| 36 | Footer CRC | `u32` | CRC of the preceding 36 footer bytes |

The declared file length must equal the actual length, and the index must end exactly at the footer. The block section begins at byte 24 and ends exactly at the index. Gaps and overlaps are invalid.

## Index

The index begins with the 8-byte magic `OCTVIDX4`. Fields follow in this order.

1. Dataset coordinate-frame origin: three `f64` values.
2. Source point count: a `u8` tag (`0` absent, `1` present), followed by a `u64` count only when present. Other tags are invalid.
3. CRS string and sampling-policy string, each encoded as `u32` byte length followed by UTF-8 bytes. Length zero means absent; the values are stored as descriptive text and are not interpreted by the format reader.
4. Hierarchy semantics: `u8`, where `0` means replacement and `1` means additive. Other values are invalid.
5. Visual scale: one positive, finite `f64`. It affects renderer presentation only; stored positions, bounds, errors, CRS and source units remain unchanged.
6. Attribute schema: `u32` attribute count followed by descriptors in schema order. Each descriptor is name length (`u32`), UTF-8 name bytes, type tag (`u8`) and component count (`u32`). Names are case-sensitive; names must be non-empty and unique and component counts non-zero.
7. Node table: `u64` node count followed by records sorted by ascending `NodeId`.
8. Block directory: `u64` block count followed by records sorted by ascending `BlockId`.

The ten stable attribute type tags are `0=U8`, `1=I8`, `2=U16`, `3=I16`, `4=U32`, `5=I32`, `6=U64`, `7=I64`, `8=F32`, and `9=F64`. Primitive values use their fixed-width little-endian representation. Floating-point values preserve their bit patterns, including NaNs and signed zero.

### Node records

Each node record contains:

| Field | Type |
| --- | --- |
| Node ID | `u64` |
| Parent tag (`0` none, `1` present) and parent ID when present | `u8`, optional `u64` |
| Level | `u32` |
| Bounds: minimum XYZ followed by maximum XYZ | 6 × `f64` |
| Own-payload point count | `u64` |
| Geometric error | `f64` |
| Block-reference count and ascending block IDs | `u32`, then count × `u64` |
| Attribute-range count | `u32` |
| Attribute-range records | variable |

Each attribute-range record contains a schema index (`u32`), then minimum and maximum arrays, each with one value per component in the attribute's declared primitive type. Range records are sorted by schema index, have unique in-range indices, and must match the descriptor's type and component count. Every minimum must be less than or equal to its corresponding maximum. Empty nodes have no ranges. A range describes only that node's own payload; floating-point range producers can omit a range when a component has no finite value. Attribute payload values themselves remain unchanged.

The node point count describes only the node's own blocks, not descendant payloads. Each referenced block must exist, each directory block must be referenced, and the sum of its blocks' point counts must equal the node's declared count. The node geometric error must be finite, non-negative and at least the maximum quantization error of its own blocks. Hierarchy parentage, bounds and semantics must satisfy the spatial hierarchy invariants.

### Block directory records

Each block begins with a fixed 93-byte prefix:

| Field | Type | Meaning |
| --- | --- | --- |
| Block ID | `u64` | Unique block identity |
| Payload offset | `u64` | Absolute file offset of the block's payload section |
| Total stored payload length | `u64` | Sum of all stored geometry and attribute payload lengths |
| Point count | `u64` | Rows shared by geometry and every attribute |
| Quantization error | `f64` | Conservative maximum source-to-decoded local position error |
| Local quantization origin | 3 × `f64` | Per-axis transform origin |
| Local quantization scale | 3 × `f64` | Per-axis transform scale |
| Reserved aggregate codec | `u8` | Must be zero |
| Reserved aggregate checksum | `u32` | Must be zero |

The final five bytes are reserved and must remain zero. V4 stores codecs and checksums only in each payload entry; there is no block-level aggregate codec or checksum.

Immediately after each prefix are exactly `1 + attribute_count` payload directory entries. Entry zero is geometry; subsequent entries follow schema order. Each entry is 29 bytes:

| Field | Type | Meaning |
| --- | --- | --- |
| Relative offset | `u64` | Byte offset from this block's payload start |
| Stored length | `u64` | Compressed or raw byte length |
| Raw length | `u64` | Exact decoded byte length |
| Codec | `u8` | `0` raw, `1` independent LZ4 block |
| CRC | `u32` | CRC of this payload's stored bytes |

Directory entries are contiguous from relative offset zero and their stored lengths must sum exactly to the block's total payload length. Raw codec requires stored length equal raw length. LZ4 requires stored length less than raw length. Other codec values are invalid. Writers use LZ4 only when it is smaller than raw data; otherwise they store raw bytes. Empty payloads are raw, have zero lengths and CRC zero.

## Payload encoding

### Geometry

Geometry consists of all quantized XYZ coordinates followed by all display RGB values:

1. For each point, three interleaved `u32` coordinates (`x`, `y`, `z`).
2. For each point, three interleaved RGB `u8` channels.

Its raw size is exactly `15 * point_count` bytes. RGB is display color; dataset attributes, including any color-like attribute, are stored separately according to the schema.

### Attribute columns

Each attribute payload stores only that attribute's values in row order. A row's components are adjacent (interleaved). Its raw length is:

```text
point_count * component_count * primitive_width
```

where primitive widths are 1 byte for `U8/I8`, 2 for `U16/I16`, 4 for `U32/I32/F32`, and 8 for `U64/I64/F64`. All arithmetic must be checked.

Each payload is compressed and checksummed independently. A reader verifies a payload's CRC before decoding it. Index, header and footer CRCs protect the addressing and metadata. An unread attribute's payload need not be read to access geometry or another attribute.

## Coordinates and quantization

Source positions are supplied to the writer as dataset-local `f64` coordinates: the dataset world origin has already been subtracted in `f64`. The per-block transform maps each axis to `u32`:

```text
q = round((local_position - block_origin) / block_scale)
decoded_local = block_origin + q * block_scale
```

The block origin is the finite per-axis minimum. A constant axis has scale zero and quantized value zero; otherwise scale is the finite span divided by `u32::MAX`. Minimum and maximum positions map to `0` and `u32::MAX` respectively. Coordinates are never clamped. Non-finite inputs, unrepresentable spans or decoded coordinates, and values outside `u32` are rejected.

The recorded block quantization error bounds the maximum Euclidean distance from each input local `f64` position to the decoded local `f32` position returned by the reader, including an arithmetic margin. The writer rejects a block if this bound exceeds its configured maximum. The value contributes to the node's geometric-error validation. Decoded render positions are local `f32`; world coordinates are reconstructed by adding the dataset frame origin in `f64` for bounds checks.

## Reading, validation and resource limits

On open, a reader checks the file/header/footer sizes, exact version and magic, all CRCs for metadata, declared lengths and configured limits before allocating the index. It then validates the complete index: UTF-8 strings, schema types and component counts, sorted unique node/block/reference/range IDs, valid hierarchy and spatial metadata, finite transforms and errors, zero reserved block fields, expected per-payload raw lengths, supported codecs, contiguous payload ranges, block/node point-count agreement and node error bounds. Blocks must be contiguous from the end of the header to the index, with no gaps or overlap. Trailing index bytes are rejected.

`FormatReaderLimits` caps index bytes, total stored and expected decoded block bytes, node count, block count, attribute count, components per attribute and string bytes. These caps are checked before relevant reservations or decompression; callers can provide stricter limits. Oversize lengths and arithmetic overflow are errors.

`read_geometry(block_id)` reads and decodes only that block's geometry payload. `read_attribute(block_id, schema_index)` reads only the selected attribute payload. `read_attribute_row(block_id, row_id, schema_index)` returns the selected row's components but decompresses the whole attribute payload, since the compression unit is one column per block. The pair `(block_id, row_id)` preserves row identity across geometry and columns. `DatasetSource::read_block` composes geometry and every schema attribute.

Payload CRC and decompression checks occur when that payload is requested. Corruption in an unrequested attribute therefore does not prevent a geometry-only read. `block_storage_stats()` reports total stored and raw bytes per block, without a codec because the block has multiple independent codecs. `block_payload_storage_stats()` reports each geometry/attribute payload's codec and stored/raw byte counts. Both statistics APIs read the validated index only and exclude header, index and footer bytes.

## Support and evidence

Only version 4 is supported. Files marked version 1, 2, 3, or any other version are rejected; there is no legacy combined-payload fallback. Regenerate such files from their source LAS/LAZ data or reconvert their EPT source.

The crate's current tests exercise V4 writing and reading, all ten attribute types and floating-point bits, additive and replacement semantics, visual scale, selective payload reads, independent checksums, malformed payload directories, configured limits and empty blocks. These format tests describe local codec behavior; they make no browser rendering or large-dataset performance claim.

Writers preserve schema and row order, emit node/block directory records sorted by ID, and append
the index and footer only after every payload succeeds. A file without a valid footer is incomplete.
Path-based conversion publishes through a temporary sibling and renames only after final validation.
