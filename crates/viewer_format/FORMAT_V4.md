# Native format v4

V4 stores one shared spatial hierarchy and schema, then an independent geometry
payload and one independent payload per attribute column in each block. All data
stays in one seekable `.oct` file; attributes occupy separate byte ranges, without
copying the hierarchy or XYZ into each attribute.

## Writing and compatibility

V4 is the default for options used by both `FormatWriter` and
`prepare_point_block`:

```rust
let options = FormatWriterOptions::new(0.001, 256 * 1024 * 1024)?;
```

Default writer options activate v4, including in converters. Explicitly calling
`.with_columnar_payloads(false)` retains legacy writing: v1 for replacement
hierarchies with visual scale 1, v2 for additive hierarchies with scale 1, and v3
for another visual scale. The reader accepts v1-v4. Compatibility shares the
metadata/topology parser and retains the legacy combined-payload decoder; v4 uses
the same geometry/attribute decoders with separate payload reads.
Appending a prepared block with a different columnar encoding mode is rejected.

## Layout

All integers and float bit patterns are little endian. Header and footer retain
the existing 24-byte and 40-byte structures and CRC32 checks. The header declares
version 4; index and footer magic are `OCTVIDX4` and `OCTVEND4`.

The index prefix through the node table is identical to v3: dataset frame, source
count, CRS, sampling policy, hierarchy semantics, visual scale, attribute schema,
node topology, block references and node attribute ranges. It appears once.

Each sorted block-directory entry has its existing 93-byte common prefix:

| Field | Type |
| --- | --- |
| block ID | u64 |
| absolute block payload offset | u64 |
| total stored payload length | u64 |
| point count | u64 |
| quantization error | f64 |
| local quantization origin | 3 × f64 |
| local quantization scale | 3 × f64 |
| aggregate codec, reserved zero | u8 |
| aggregate checksum, reserved zero | u32 |

Immediately after this prefix come exactly `1 + schema_attribute_count` payload
entries. The first is geometry; the others follow schema order. Their cardinality
and identity are implicit in the shared schema; each payload entry is 29 bytes:

| Field | Type |
| --- | --- |
| offset relative to the block payload start | u64 |
| stored length | u64 |
| decoded length | u64 |
| codec: 0 = raw, 1 = LZ4 | u8 |
| CRC32 of stored bytes | u32 |

Geometry consists of `point_count` XYZ triplets of quantized u32 coordinates,
followed by `point_count` RGB triplets of u8 display colors. Its decoded length is
`15 * point_count`. Coordinates use the common block quantization transform.

Attribute `i` consists only of its typed primitive values, in row order with each
row's components adjacent. Its decoded length is
`point_count * schema[i].component_count * primitive_width`. All ten existing
primitive types retain their exact bits, including floating point NaNs.
Compression is chosen independently for each payload and is used only if smaller
than raw bytes. Empty payloads use raw encoding, zero length and CRC32 zero.

Payloads are contiguous in directory order and exactly fill their owning block.
Blocks are contiguous in physical order from the end of the header to the index.
There is no aggregate payload checksum in v4: integrity is checked per requested
payload, while index/footer/header checksums protect all addressing metadata.

## Selective reading and point identity

- `read_geometry(block_id)` returns `PointBatch` containing XYZ and display RGB.
- `read_attribute(block_id, schema_index)` returns a complete typed column.
- `read_attribute_row(block_id, row_id, schema_index)` returns that row's typed
  components. The row ID is its zero-based position in the block; the immutable
  pair `(block_id, row_id)` identifies the same row across geometry and columns.
- `DatasetSource::read_block(block_id)` reconstructs geometry and all attributes,
  preserving the existing complete-block API.

In v4, geometry reads consume no attribute payload bytes. A column read consumes
no geometry, other attribute or other block payload bytes. A row lookup decodes
the entire requested column because compression granularity is one column per
block. There is no row-level range decompression or cache. V1-v3 selective APIs
must read their combined legacy block payload.

Opening validates the entire index, checksums, counts, schema, transforms,
contiguity, decoded lengths, codec constraints and bounds before payload reads.
Stored and decoded aggregate block sizes must fit `max_block_bytes`; this remains
conservative even for a geometry-only request. Allocation uses checked arithmetic
and fallible reservation. Index/count/string/component limits remain applicable.
Payload checksums and decompression are checked only when that payload is read;
corruption in an unrequested attribute does not prevent geometry access.

`block_payload_storage_stats()` returns `BlockPayloadStorageStats` entries with
block ID, `BlockPayloadKind`, actual codec and stored/raw byte counts. In v4, the
kind is `Geometry` or `Attribute { schema_index, name }`; there is one entry per
independent payload. Benchmarks can sum geometry and attribute entries separately
without reading their bytes. Entries follow block ID, then geometry/schema order.
V1-v3 report one `CombinedLegacy` entry per block, since their jointly compressed
payload cannot provide independent stored geometry and attribute byte counts.

`block_storage_stats()` remains available as an aggregate. Its stored/raw sizes
sum geometry and attributes; its codec is `None` for v4 because the aggregate has
no single codec. Neither statistics API includes header/index/footer bytes or
performs payload I/O.

## Evidence

The crate's tests use a seekable `ReadSpy` that records every read offset and byte
count. After opening a two-block dataset with two attribute columns, they assert
that geometry reads exactly the geometry payload's stored length and range, and
that a requested column/row reads exactly its own payload's length and range.
Other tests corrupt each payload independently, forge column offsets/lengths/codecs
with repaired index checksums, enforce block limits, preserve all primitive bits,
and exercise the selective API against v1-v3 files. These are local format tests;
they do not establish browser, renderer or large-dataset performance.
