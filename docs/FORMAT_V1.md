# Octree Viewer Block Format, version 1

This document defines the first version of the project's native point-cloud container. It is a
little-endian, seekable single-file format with an in-memory index and independently verified
point blocks. Version 1 supports raw blocks (`codec = 0`) and independent LZ4 blocks (`codec = 1`).
Version 2 adds an explicit additive hierarchy mode; see [FORMAT_V2.md](FORMAT_V2.md).

The file stores coordinates as per-block `u32` values relative to a local `f64` origin and scale.
Writers must receive source positions in `f64`, before narrowing to render coordinates. Readers
decode only the requested block and convert its local coordinates to `f32` for `viewer_core`.

## Primitive encoding

- All integers and floating-point bit patterns are little-endian.
- `u16`, `u32`, and `u64` are unsigned fixed-width integers. `f64` is IEEE 754 binary64.
- Strings are UTF-8 preceded by their byte length. A string is never NUL-terminated.
- An absent optional value is encoded with a separate tag; integer sentinel values are not used.
- Offsets and lengths are unsigned 64-bit byte counts from the beginning of the file.
- Readers reject non-finite floating-point metadata, invalid ranges, arithmetic overflow, invalid
  UTF-8, unknown mandatory tags, and any offset or length outside the file.

## File layout

```text
24-byte header | block payloads | index | 40-byte footer
```

Payloads are contiguous in writer order. The index starts immediately after the final payload; the
footer is the final 40 bytes. Gaps and overlapping payload ranges are invalid. Padding is not used.

### Header (24 bytes)

| Field | Type | Value |
| --- | --- | --- |
| magic | 8 bytes | ASCII `OCTVWFRM` |
| version | `u16` | `1` |
| header length | `u16` | `24` |
| flags | `u32` | `0` |
| reserved | `u32` | `0` |
| CRC | `u32` | CRC32 of the preceding 20 bytes |

Unknown flags, non-zero reserved bits, or any other version are rejected.

### Block payload

The index supplies the block's ID, point count, byte length, quantization transform and CRC. The
payload has no redundant header and contains, in order:

1. For each point, three interleaved `u32` coordinates (`x`, `y`, `z`).
2. For each point, three interleaved RGB `u8` channels.
3. For each schema attribute in schema order, all values for this block in point order. Components
   of one value are interleaved. Integers and floating-point values use their declared primitive
   representation and little-endian encoding. Floating-point bit patterns are preserved.

Attribute type tags are stable: `0=U8`, `1=I8`, `2=U16`, `3=I16`, `4=U32`, `5=I32`, `6=U64`,
`7=I64`, `8=F32`, `9=F64`. Attribute names are case-sensitive. A schema descriptor contains a
non-empty UTF-8 name, one type tag and a non-zero `u32` component count. Duplicate names are invalid.
All declared attributes occur in every block, including zero-point blocks. Version 1 has no optional
per-block columns; unsupported type tags fail explicitly instead of dropping data.

The uncompressed payload length is exactly:

```text
point_count * (12 coordinate bytes + 3 RGB bytes)
  + sum(point_count * component_count * primitive_width for each attribute)
```

Every multiplication and sum must be checked before allocation or I/O. The block CRC covers the
stored payload bytes. For codec `0`, the stored payload is the uncompressed sequence above. For
codec `1`, the whole sequence is encoded as one independent LZ4 block, with no shared dictionary or
size prefix. The expected decoded length is calculated from the point count and schema before
allocation. The writer uses codec `1` only when it produces fewer bytes than codec `0`; otherwise it
stores the raw sequence. Implementations use the safe block API from `lz4_flex`.

### Index

The index starts with the eight bytes `OCTVIDX1`, followed by these fields in order:

1. Dataset coordinate-frame origin: three `f64` values.
2. Source-point-count tag (`u8`, `0=absent`, `1=present`), followed by a `u64` only when present.
3. CRS string: `u32` byte length (`0` means absent), then UTF-8 bytes.
4. Sampling-policy string, encoded the same way; an empty string means unspecified. Version 1 stores
   this description opaquely and does not interpret it.
5. Attribute count (`u32`) and the attribute descriptors in schema order. Each descriptor contains
   name byte length (`u32`), name bytes, type tag (`u8`) and component count (`u32`).
6. Node count (`u64`) and node records in ascending `NodeId` order. Each node contains `NodeId` (`u64`),
   parent tag (`u8`, `0=none`, `1=present`) and parent ID (`u64` only when present), level (`u32`),
   bounds (`min[x,y,z]`, then `max[x,y,z]`, all `f64`), representative count (`u64`), conservative
   geometric error (`f64`), block count (`u32`) and that many `BlockId` values (`u64`, ascending),
   attribute-range count (`u32`) and that many attribute-range records.
7. Block count (`u64`) and directory records in ascending `BlockId` order. Each record contains ID
   (`u64`), offset (`u64`), payload length (`u64`), point count (`u64`), quantization error (`f64`),
   local-coordinate origin (three `f64` values), per-axis scale (three `f64` values), codec (`u8`,
   `0=none`, `1=LZ4 block`), and payload CRC (`u32`).

Every indexed block must be referenced by at least one node; node references and directory IDs must
be unique where required and must resolve. The stored point count of a node describes that node's
own payload, as in `SpatialNode`, not the sum of its descendants. A node's geometric error includes
the producer's source-to-representative error and the maximum quantization/render-coordinate error
of its own referenced blocks.

An attribute-range record contains the schema index (`u32`), then a minimum and maximum for each
component, in the attribute's declared primitive type. Both arrays are component-interleaved. Range
records are ordered by schema index; duplicate or out-of-range indices are invalid. A node range
describes only the node's own representative payload, never a sum or union of samples from its
descendants. Empty nodes have no ranges. Writers may omit a range when any component has no finite
numeric value; ranges for integer attributes are exact, and ranges for floating-point attributes
are computed from finite values only. The underlying attribute values are still preserved bit for
bit, including NaNs and infinities.

The quantization transform is expressed in dataset-local coordinates, calculated by subtracting the
dataset origin from source world coordinates in `f64`. For each axis, the writer chooses a finite
origin no greater than the block's minimum and a finite non-negative scale. A constant axis uses
scale `0` and coordinate `0`. Otherwise:

```text
q = round((local_coordinate - block_origin) / scale)
decoded_local = block_origin + q * scale
```

The writer rejects values that are non-finite, outside `u32`, or whose decoded local `f32` position
exceeds the caller's maximum permitted error. It never clamps or silently drops precision. The
recorded quantization error is a conservative maximum Euclidean distance from input `f64` positions
to the exact `f32` positions produced by the reader's decoding operation. It is added to the
producer's geometric-error bound. Scale/origin choices must cover all points in that block.

### Coordinate storage cost

Three `u32` coordinates use 12 bytes per point, the same as three `f32` coordinates and half the
24 bytes of three `f64` coordinates. Version 1 therefore reduces coordinate storage relative to
`f64`, but does not claim a size reduction relative to the current `f32` render representation. Each
block adds 56 bytes for its three `f64` origins, three `f64` scales and `f64` error bound, before its
directory entry. This transform overhead is amortized after five points per block when comparing
only coordinate values against `f64`. RGB adds 3 bytes per point; each attribute adds its declared
primitive width times its component count. Smaller integer attribute types are preserved as
declared rather than widened.

### Initial compression measurement

The checked-in `codec_bench` example measures five encode/decode iterations in release mode for one
4,456,448-byte block (262,144 points, 12 coordinate bytes, RGB and one `u16` attribute per point).
It compares a structured grid with mostly repeated RGB values against deterministic high-entropy
bytes. The result table records the mean elapsed time per iteration; timings are a local
microbenchmark, not a performance guarantee or a replacement for real LAS/LAZ fixtures.

| Payload | Stored bytes | Stored/raw ratio | Encode ms | Decode ms | Selected codec |
| --- | ---: | ---: | ---: | ---: | --- |
| Structured grid | 2,617,998 | 0.587 | 6.56 | 2.17 | LZ4 block |
| High entropy | 4,456,448 | 1.000 | 1.64 | 0.89 | Raw fallback |

Measured with `lz4_flex 0.14.0`, Rust `1.99.0`, release profile, on an AMD Ryzen AI 9 365 system.
The fallback preserves the original size on incompressible data; the encode time still includes an
LZ4 trial. Repeat `cargo run -p viewer_format --release --example codec_bench` on target systems
before making throughput claims.

### Footer (40 bytes)

| Field | Type | Meaning |
| --- | --- | --- |
| magic | 8 bytes | ASCII `OCTVEND1` |
| index offset | `u64` | Absolute start of the index |
| index length | `u64` | Index bytes, excluding footer |
| file length | `u64` | Exact file length including footer |
| index CRC | `u32` | CRC32 over all index bytes |
| footer CRC | `u32` | CRC32 over the preceding 36 footer bytes |

The reader requires the index to end exactly where the footer begins and `file length` to match the
actual file length. The index and footer must fit between the header and end of file. The reader must
validate lengths and configured resource limits before reserving memory.

## Integrity and compatibility

CRC32 is the reflected IEEE CRC-32 algorithm (polynomial `0xEDB88320`, initial value `0xFFFFFFFF`,
final XOR `0xFFFFFFFF`). Header, index, footer and each block are checked independently. CRC detects
accidental corruption; it is not an authenticity mechanism. Opening validates the header, footer,
index checksum, directory ranges and hierarchy. A block checksum is verified when that block is read;
unrequested payload bytes need not be read during open.

Version 1 writers emit version `1`; the current reader accepts versions 1 and 2, interpreting v1 as
replacement semantics. A v1-only reader accepts only version `1`, the exact header and footer
lengths above, zero header flags, and codecs `0` or `1`. Version 1 has no extension sections: adding
fields, changing field meanings or changing required encodings requires a new format version. Unknown
tags, versions, flags and codecs produce explicit errors. Readers preserve attribute names and values
when their primitive type is supported. The format crate exposes configurable limits for index size,
schema strings, node/block counts and decoded block size, and rejects over-limit files before large
reservations.

## Determinism and incomplete output

Writers order node and block records by ID and preserve schema order. They emit blocks in caller
order, then a canonical index and footer. The footer is written only after all payloads and the index
are complete. Files without a valid footer are incomplete and must not be opened as datasets. A
path-based writer publishes through a sibling temporary file and renames it only after successful
finalization.

## Explicit limits of version 1

The index is loaded into memory. Compression is per block and does not share history between blocks.
CRC does not protect against malicious tampering. CRS and sampling-policy text are preserved but not
interpreted. The format does not define an external LAS attribute mapping or a converter's sampling
algorithm.
