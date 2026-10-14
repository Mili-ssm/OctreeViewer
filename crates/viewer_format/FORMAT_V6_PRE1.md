# Native `.oct` format, v6.1-pre

First implemented experimental v6 revision. Its exact identity is major **6**, revision **1**;
[v6.2-pre](FORMAT_V6_PRE2.md) is implemented separately; final v6 and later revisions remain
unsupported. Default output remains final v5.
This revision changes geometry encoding without adding loss beyond the existing v5 quantization.
It is not yet a voxel-occupancy or mesh format.

## Envelope and shared fields

All integer fields are little endian. The common 24-byte `OCTVFRM1` envelope, `FileRole`, header
CRC and exact identity rules are specified in [v5](FORMAT_V5.md). Both primary and attribute
companion carry major 6 / revision 1. The primary can be opened without its companion and returns
neutral display gray, exactly like v5.

The layout otherwise follows v5 metadata, coordinate frame, schema, node hierarchy, ranges,
payload directory, independent payload codecs and split/monolithic storage, with these differences:

| Field | v5 | v6.1-pre |
| --- | --- | --- |
| Header major / revision | 5 / 0 | 6 / 1 |
| Index magic | `OCTVIDX5` | `OCTVI61P` |
| Footer magic, both files | `OCTVEND5` | `OCTVE61P` |
| Block directory after `point_count` | Next field is quantization error | One `u8` geometry encoding, then quantization error |
| Geometry encoding | Interleaved `u32` XYZ | `0` direct `u32` or `1` signed-delta varints |

The five existing reserved bytes after the block transform remain zero. No metadata or payload
is silently interpreted under a different revision. Sidecars must have the exact same version as
the primary, plus matching index identity and extents. Prepared blocks record their layout and
are rejected by an incompatible writer before touching either sink.

## Geometry modes

Quantization is unchanged: the same source-local `f64` positions produce the same affine
origin/scale, quantized `u32` triples and measured error bound as v5. Rows are never reordered;
RGB and every attribute column retain their exact row association and bit patterns.

### Mode 0: direct

The geometry stream is `x:u32, y:u32, z:u32` for each point, exactly `12*N` bytes before LZ4.

### Mode 1: delta

Initialize previous XYZ to `[0, 0, 0]` at the start of each block. Visit rows in source-block order,
then axes X/Y/Z. For each coordinate:

1. Subtract the previous coordinate in signed `i64` and update that axis's predictor.
2. ZigZag encode the delta: `((delta << 1) ^ (delta >> 63)) as u64`.
3. Write canonical unsigned LEB128, least significant seven bits first; bit 7 marks continuation.

A delta spans `[-4294967295, 4294967295]`. ZigZag therefore uses **33 bits**, at most five bytes;
the fifth byte can have value up to `0x1f` and must not continue. Decoding rejects non-minimal
varints, truncation, overlong encodings and predictor results outside `u32`. Exactly `3*N` varints
must be consumed, with no trailing bytes. An empty geometry stream has length zero.

The mode describes the geometry stream before its ordinary `BlockCodec::{None,Lz4}` layer.
Payload CRC continues to cover **stored bytes**, as in v5; strict delta validation is still required
after CRC/decompression. Invalid coordinates are rejected even if all file CRCs are valid.

## Selection and limits

The writer compares both geometry candidates after `compress_block` has selected raw or LZ4
for each one. It chooses the smaller stored geometry payload; ties choose mode 0. Mode 1 is
ineligible if its transformed block size exceeds the configured limit. The extra index byte per
block remains overhead, so even fallback cannot guarantee a smaller total file than v5.

Both writer and reader bound two sizes, using checked arithmetic:

- **Logical bytes:** `12*N` geometry + `3*N` display RGB + the canonical sizes of all columns.
- **Raw payload bytes:** the sum of each actual stream length before LZ4. Delta geometry must
  satisfy `3*N <= raw_length <= 15*N`; direct geometry must have `raw_length == 12*N`.

Logical expansion is checked before point allocation. Raw payload limits are checked before
decompression. The decoder visits coordinates directly without allocating another `u32` point
array; it reserves `N` Point elements, not a byte count interpreted as an element count.

`BlockStorageStats` exposes `logical_bytes`, `raw_bytes`, `stored_bytes` and `geometry_encoding`.
Compression ratios based on raw bytes describe the transformed stream; comparisons of total files
must include both files and all metadata. Encoder temporaries include source/quantized positions,
raw/delta candidates and compression outputs; retained-payload counters alone are not peak RSS.

## API, implementation and validation

`StorageLayout::{V5,V6Pre1}` selects exact markers and geometry rules in the shared indexed
reader/writer pipeline. `FormatReader` automatically selects by header; `OctreeReader`,
`OctreeWriter`, `PreparedOctreeWriter` and `BlockEncoder` remain the consumption boundaries.
There is no duplicate v6 parser for unchanged hierarchy/schema data.

```sh
cargo run --profile perf --offline -p viewer_convert -- INPUT.laz OUTPUT.oct \
  --format-version v6.1-pre
```

LAS/LAZ and local EPT use the selected format; checkpoints persist the exact identity and reject
resuming with v5 or another revision. WASM reads either format through Blob. Browser output can
select `WasmLasOctConverter.createForVersion(file, primaryHandle, attributesHandle, "v6.1-pre")`;
the constructor and demo's ordinary conversion remain v5. The converter worker also accepts
`formatVersion: "v6.1-pre"` in its conversion message. No experimental UI control is added.

Tests cover exact v5/v6 coordinates and all ten attribute types, NaN/signed-zero bits, selective
I/O, both geometry modes, canonical varint errors, corruption with repaired CRCs, logical limits,
prepared-block/sidecar mismatch, serial/parallel conversion, EPT and checkpoint resume. See
[comparison evidence](../../docs/STORAGE_BENCH.md) for repeated real-data sizes/times/RSS and
WASM OPFS-to-native interoperability. These are small warmed samples, not scale or FPS claims.

## Later revisions

### Keep the version axes separate

The header `FormatVersion` identifies the serialized `.oct`/`.octattrs` layout and codec. It does
not version the logical meaning of every future octree/voxel representation, and it does not version
the renderer. A renderer change to shaders, exposed-face extraction, grouped drawing or LOD-cut
transitions does not require a new storage identity while it consumes the same stored data. A future
file that persists renderer-derived meshes would need an explicit storage contract for those meshes.

`v6.1-pre` implements only direct-u32 versus delta-varint encoding of quantized point coordinates.
It does not implement occupancy masks or infer child voxel coordinates. Pre2 implements that
encoding within geometry blocks; a global physical-voxel representation remains pending. The current proposal gives each parent an occupancy-position byte, a
child-count/flags byte, and an index/offset to its first child in the next level's array. The
`count` and `first_child` define a contiguous slice; the mask plus a deterministic child order (for
example Morton) maps each occupied slot to its entry in that slice. Given the parent's coordinate
and the grid, the reader derives each child's coordinate from its local ordinal instead of storing
it again. The count can be checked against the mask's population count; whether it is kept for direct
slice access and what flags mean remain open. One byte represents 0–8 children for 2x2x2, but not
512 for 8x8x8. The mask needs 8, 64 or 512 bits for 2x2x2, 4x4x4 or 8x8x8, unless a sparse
alternative is specified. The root grid/anchor, offset units/width, parent order and relationship
to point payloads must also be defined. See the [v6 preparation note](../../docs/STORAGE_V6_PREPARATION.md)
for the proposed slice/rank mapping and open decisions.

Possible count-byte flags could mark that all child records repeat, repeat in pairs, or follow another
compact pattern. Their meaning must specify what is equal (values, occupancy, or equivalent child
subtrees) and how to expand it. For 2x2x2, a 0–8 count needs four bits, leaving at most four flag
bits in the byte; larger branching factors change that trade-off. If flags collapse repeated logical
children into fewer physical records, distinguish the logical child count (`popcount(mask)`) from the
physical slice length. Otherwise `first_child + logical_count` cannot delimit the slice; an explicit
physical count or expansion table/rule is required.

Attribute/value deduplication is a separate storage-compression design. Exposed-face extraction,
grouped geometry and transitions between LOD cuts belong to the renderer unless derived geometry is
chosen for persistence. Each incompatible serialized change gets a new exact prerelease identity;
`v6` final is only enabled after the agreed corpus, compatibility, memory and browser criteria are
satisfied.
