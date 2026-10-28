# Native `.oct` format, version 5 (final)

This document specifies the complete version 5 file format. V5 is the default stable format. The project also supports the separately specified experimental [v6.1-pre](../../records/archive/FORMAT_V6_PRE1.md), [v6.2-pre](../../records/archive/FORMAT_V6_PRE2.md), [v6.3-pre](../../records/archive/FORMAT_V6_PRE3.md) and [v6.4-pre](../../records/archive/FORMAT_V6_PRE4.md). Provisional v5 files (magic `OCTVWFRM`) and older `.oct` versions are not read; regenerate them from their LAS/LAZ source or convert the EPT dataset again.

V5 stores one shared dataset frame, hierarchy, schema and metadata index in the primary `.oct` file. Each block has independent XYZ, display RGB and schema attribute payloads. Native and browser converters emit a separate `.octattrs` companion for RGB and columns. The stream API also supports monolithic storage using the same payload layout. A split primary opens without its companion and supplies neutral RGB `(160,160,160)`.

The primary contains structural metadata, including attribute descriptors and finite ranges, but no attribute values in split mode. Only the companion contains those values. Block ID and row index identify the same point across both files.

All integer fields and floating-point bit patterns are little endian. Offsets and lengths are unsigned 64-bit byte counts. Strings are UTF-8 preceded by a `u32` byte length, without a terminator. A zero length represents an absent optional string. All CRCs use reflected IEEE CRC-32 (polynomial `0xEDB88320`, initial value and final XOR `0xFFFFFFFF`); CRC detects accidental corruption, not malicious modification.

The common header probe recognizes exact version identities; the reader also implements v6 pre1/pre2/pre3/pre4,
while final v6 and later revisions remain unsupported.
See [v6 preparation](../../records/benchmarks/STORAGE_V6_PREPARATION.md). The finalized envelope deliberately
rejects provisional v5; regenerate both files together. The incompatible envelope change was
authorized before v6 development. Subsequent incompatible payload changes require a new identity.

## File structure

```text
24-byte header | contiguous block payloads | index | 40-byte footer
```

### Header (24 bytes)

| Offset | Field | Type | Required value |
| ---: | --- | --- | --- |
| 0 | Magic | 8 bytes | ASCII `OCTVFRM1` (common envelope 1) |
| 8 | Version | `u16` | `5` |
| 10 | Header length | `u16` | `24` |
| 12 | Role | `u32` | `0` monolithic primary, `1` split primary, `2` attribute companion |
| 16 | Development revision | `u32` | `0` for final v5 |
| 20 | Header CRC | `u32` | CRC of bytes 0–19 |

Any other version, magic, header length, role, revision or checksum is rejected. Role tags are
encoded by `FileRole::{Monolithic, SplitPrimary, Attributes}`; combinations such as `3` are invalid. A companion cannot be opened as a primary.

### Footer (40 bytes)

The footer is the final 40 bytes of the file.

| Offset | Field | Type | Meaning |
| ---: | --- | --- | --- |
| 0 | Magic | 8 bytes | ASCII `OCTVEND5` |
| 8 | Index offset | `u64` | Absolute index start |
| 16 | Index length | `u64` | Index byte length, excluding footer |
| 24 | File length | `u64` | Exact length including footer |
| 32 | Index CRC | `u32` | CRC of all index bytes |
| 36 | Footer CRC | `u32` | CRC of the preceding 36 footer bytes |

The primary declared file length must equal its actual length, and its index must end exactly at the footer. Its payload section begins at byte 24 and ends exactly at the index. Gaps and overlaps are invalid.

### Attribute companion

```text
24-byte header (flags=2) | RGB and attribute payloads | 40-byte footer
```

The companion has no index or coordinate payload. Its footer uses the same magic and fields: index offset, index length and index CRC refer to the primary index; file length refers to the companion's exact length. Its own footer CRC covers its preceding 36 bytes. Attachment validates all these fields against the primary and the companion extent computed from the primary directory. Identity is based on the index checksum (which includes every payload checksum) and lengths, not a filename. This protects against accidental mismatch, not adversarial CRC collisions.

Writers flush the companion footer before writing the primary footer. A missing companion is valid geometry-only use; a supplied malformed companion is an error. Two path publications cannot be atomic together: native conversion validates both candidates, publishes the companion first, syncs the directory, then publishes the primary without replacement and syncs again. Checkpoints retain fingerprints for both candidates and can resume companion-only publication without overwriting foreign files.

## Index

The index begins with the 8-byte magic `OCTVIDX5`. Fields follow in this order.

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
| Primary stored payload length | `u64` | XYZ stored length in split mode; sum of all payload lengths in monolithic mode |
| Point count | `u64` | Rows shared by geometry and every attribute |
| Quantization error | `f64` | Conservative maximum source-to-decoded local position error |
| Local quantization origin | 3 × `f64` | Per-axis transform origin |
| Local quantization scale | 3 × `f64` | Per-axis transform scale |
| Reserved aggregate codec | `u8` | Must be zero |
| Reserved aggregate checksum | `u32` | Must be zero |

The final five bytes are reserved and must remain zero. V5 stores codecs and checksums only in each payload entry; there is no block-level aggregate codec or checksum.

Immediately after each prefix are exactly `2 + attribute_count` payload directory entries. Entry zero is XYZ; entry one is display RGB; subsequent entries follow schema order. Each entry is 29 bytes:

| Field | Type | Meaning |
| --- | --- | --- |
| Offset | `u64` | Relative to primary block start in monolithic mode; split XYZ offset is zero and split RGB/attribute offsets are absolute companion offsets |
| Stored length | `u64` | Compressed or raw byte length |
| Raw length | `u64` | Exact decoded byte length |
| Codec | `u8` | `0` raw, `1` independent LZ4 block |
| CRC | `u32` | CRC of this payload's stored bytes |

In monolithic mode entries are contiguous from relative offset zero and their stored lengths sum exactly to the primary payload length. In split mode XYZ has offset zero and its stored length equals the primary payload length; RGB/attribute entries for a block are contiguous absolute companion ranges. All companion block ranges, sorted by their physical offset, cover bytes 24 through the companion footer without gaps or overlap. Block IDs need not reflect physical write order. Empty payloads may share a boundary offset. Raw codec requires stored length equal raw length. LZ4 requires stored length less than raw length. Other codec values are invalid. Writers use LZ4 only when it is smaller than raw data; otherwise they store raw bytes. Empty payloads are raw, have zero lengths and CRC zero.

## Payload encoding

### Geometry

Geometry is one interleaved triple of quantized `u32` XYZ per row. Its raw size is exactly `12 * point_count` bytes. Display color is a separate payload of interleaved RGB `u8` triples (`3 * point_count` bytes). It is preserved independently of any schema `rgb` column, which can hold original RGB16 or different representative values.

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

## Public storage interfaces

`FormatReader<R>` is a facade that probes the common identity and dispatches to the private indexed
backend with the selected layout profile. The validated header is passed to that backend without another header read. The facade
implements `DatasetSource` and `OctreeReader`, and retains companion attachment, cursor forks,
metadata and directory statistics. Selective consumers need only `OctreeReader`.

`FormatWriter::{new_for_version,new_split_for_version}` select an output version explicitly;
unsupported identities fail before either sink is touched. The default constructors select final
v5. `BlockEncoder` prepares blocks independently of sinks; `FormatBlockEncoder` selects
v5, v6.1-pre, v6.2-pre, v6.3-pre or v6.4-pre. The converter streams `E::PreparedBlock` to a compatible `PreparedOctreeWriter`; it does
not call a v5 encoding function or impose `Sync` on the writer. The shared layout dispatch now supports v6.1-pre without changing the v5 byte layout; later
revisions must extend that dispatch explicitly.

## Reading, validation and resource limits

On open, a reader checks the file/header/footer sizes, exact version and magic, all CRCs for metadata, declared lengths and configured limits before allocating the index. It then validates the complete index: UTF-8 strings, schema types and component counts, sorted unique node/block/reference/range IDs, valid hierarchy and spatial metadata, finite transforms and errors, zero reserved block fields, expected per-payload raw lengths, supported codecs, contiguous payload ranges, block/node point-count agreement and node error bounds. Blocks must be contiguous from the end of the header to the index, with no gaps or overlap. Trailing index bytes are rejected.

`FormatReaderLimits` caps index bytes, total stored and expected decoded block bytes, node count, block count, attribute count, components per attribute and string bytes. These caps are checked before relevant reservations or decompression. The default reader accepts at most 256 attributes; the writer and file layout permit more, so callers must use `FormatReader::with_limits` with an appropriate attribute limit to open a wider schema. Oversize lengths and arithmetic overflow are errors.

`read_positions(block_id)` reads only XYZ and returns neutral gray points. `read_display_colors(block_id)` reads only RGB. `read_geometry(block_id)` composes XYZ and display RGB when attributes are available, otherwise returns gray. `read_attribute(block_id, schema_index)` reads only the selected attribute payload. `read_attribute_row(block_id, row_id, schema_index)` returns the selected row's components but decompresses the whole attribute payload, since the compression unit is one column per block. The pair `(block_id, row_id)` preserves row identity across geometry and columns. `DatasetSource::read_block` composes XYZ, display RGB and every available schema attribute; without a companion it returns gray geometry and no columns. `read_attribute` reports `AttributesUnavailable` if a required companion is not attached. `fork_reader` shares the immutable index but starts without a companion; attach an independent cursor before full reads.

Payload CRC and decompression checks occur when that payload is requested. Corruption in an unrequested attribute therefore does not prevent a geometry-only read. `block_storage_stats()` reports total stored and raw bytes per block, without a codec because the block has multiple independent codecs. `block_payload_storage_stats()` reports each geometry/attribute payload's codec and stored/raw byte counts. Both statistics APIs read the validated index only and exclude header, index and footer bytes.

## Support and evidence

This specification describes v5; the reader also supports the separately specified v6 pre1/pre2/pre3/pre4.
Files marked version 1, 2, 3, 4, or other unimplemented identities are rejected; there is no legacy combined-payload fallback. Regenerate such files from their source LAS/LAZ data or reconvert their EPT source.

The crate's current tests exercise V5 writing and reading, all ten attribute types and floating-point bits, additive and replacement semantics, visual scale, selective payload reads, independent checksums, malformed payload directories, configured limits and empty blocks. These format tests describe local codec behavior; they make no browser rendering or large-dataset performance claim.

Writers preserve schema and row order, emit node/block directory records sorted by ID, and append
the index and footer only after every payload succeeds. A file without a valid footer is incomplete.
Path-based conversion publishes from temporary siblings using non-replacing hard links after final validation of both streams.

## Ranges and display

Writers derive typed finite min/max arrays from the actual block columns and merge them for each node's own blocks. Supplied ranges are replaced with these computed values. Empty columns and columns with a component that has no finite value omit their range. Exact attribute payload bits remain unchanged. `attribute_range` aggregates available node ranges for one component without reading payloads; the viewer uses this range consistently across LOD blocks. Missing/constant ranges and nonfinite values display gray. Integer-to-f64 projection is for color only, not exact-value queries.

Block raw-byte limits count XYZ, display RGB and all schema columns together; the resident index and counts retain the existing configurable limits. Split storage reduces geometry-only transfer, not the total uncompressed per-row information. Per-payload statistics identify `Geometry`, `DisplayColor`, and `Attribute`; block statistics sum both files' payloads and exclude their headers, index and footers.
