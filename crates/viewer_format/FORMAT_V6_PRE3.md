# Native `.oct` format, v6.3-pre

Exact identity: major **6**, revision **3**, opt-in. V5 remains the default. This revision keeps
[pre2 geometry](FORMAT_V6_PRE2.md), quantization, source-row order, hierarchy and schema. It adds
independent attribute codecs to reduce physical bytes while accepting CPU reconstruction.

The common 24-byte header and 40-byte footer follow v5. Index/footer markers are `OCTVI63P`
and `OCTVE63P`. Primary and companion must have the same exact revision; [pre4](FORMAT_V6_PRE4.md) adds its own codecs; pre5 and final v6
remain unsupported. Both split and monolithic streams, prepared blocks and checkpoints carry
the exact identity.

## Payload directory and version gates

The existing 29-byte directory record is unchanged:

```text
offset:u64, stored_length:u64, raw_length:u64, codec:u8, stored_crc32:u32
```

No new per-column index metadata is added. `raw_length` is still the full logical column size,
not the dictionary size. Geometry still allows raw XYZ, delta-varint and occupied streams, each
with payload codec 0/1 only. RGB and typed attributes allow:

| Codec | Meaning | Revisions |
| ---: | --- | --- |
| 0 | Raw bytes | v5, pre1, pre2, pre3 |
| 1 | LZ4 block | v5, pre1, pre2, pre3 |
| 2 | One complete constant tuple | pre3/pre4 attributes only |
| 3 | Local tuple dictionary + packed row IDs | pre3/pre4 attributes only |

Readers reject tags 2/3 in geometry or in older revisions even with repaired index checksums.
For new tags, row count must be at least two and `stored_length < raw_length`. The CRC covers
**all stored bytes**, checked before decoding. Existing raw/LZ4 rules and lazy column reads
remain in force. A geometry-only read does not require `.octattrs`.

## Tuple definition

RGB is one three-byte tuple per row. A typed tuple is all components of one attribute in
schema order, each encoded little-endian with its primitive width. Width is therefore known
from the schema, and row count from the block directory. There are no casts through floating
point and no numeric equality comparisons: NaN payloads, signed zeros and large integers retain
all bits. Dictionaries never combine separate columns or reorder source rows.

## Constant (tag 2)

The payload is exactly **one tuple**, with no header, compression or IDs. Repeat its bytes for
all rows. The directory validates the stored length against the schema width. Empty and
single-row columns use raw/LZ4. A constant transform is selected only if the complete tuple
occupies strictly fewer bytes than the raw/LZ4 candidate; large constant tuples can still use
LZ4 or a one-entry compressed dictionary when smaller.

## Dictionary (tag 3)

```text
entry_count:u32
table_codec:u8
ids_codec:u8
table_stored_length:u32
table_stored_bytes[table_stored_length]
ids_stored_bytes[remaining payload bytes]
```

The header is exactly **10 bytes**. Inner codecs must be raw (0) or LZ4 (1); nesting transforms
is rejected. Tables contain complete tuples. Writers assign IDs in first-occurrence order and
compare byte keys with collision-resolving equality. The decoder does not require unique table
entries or first-occurrence order; aliases are harmless and reconstruct identical bytes.

Let N be directory row count, W tuple width, K entry count. Require `1 <= K <= N`.
Table raw size is `K*W`. Bits per ID are `ceil(log2(K))`, zero for K=1. IDs have raw size
`ceil(N*bits/8)` and are packed consecutively LSB-first, in original row order. Unused high bits
of the final byte must be zero. Every decoded ID must be less than K. There are no additional
lengths or padding. Both inner decompressions must produce their exact expected lengths.

The writer compares raw/LZ4 against the complete dictionary header + independently selected
raw/LZ4 table + independently selected raw/LZ4 IDs. Ties retain the previous candidate, so a
new attribute payload never exceeds its pre2 baseline size. V5/pre1/pre2 output remains
unchanged. The primary index remains the same size as pre2 for equivalent inputs, although its
bytes/CRC change when companion offsets/codecs change.

## Limits and resource contract

Checked arithmetic determines tuple widths and packed sizes, including on 32-bit WASM.
Before allocating, the reader enforces existing logical block limits, validates lengths and
bounds each decoded intermediate by the logical column size. The table and packed IDs are
materialized before expansion; the expanded byte column is then parsed into typed values.
These per-buffer limits **do not bound total peak RSS**. Encoding temporarily holds the raw
column, baseline, hash map, table, IDs and candidates. A new memory/performance optimization
must preserve exactness and be measured separately.

## Validation and remaining work

Unit and integration tests cover constant/dictionary/raw-LZ4 fallback, RGB components and all
ten primitive types, large integers, NaNs/signed zero, malformed lengths/tags/IDs/padding,
allocation limits, CRC failures, repaired-CRC malformed streams and old-version relabeling.
Counting readers verify exactly the stored bytes of the requested column are read; other
columns/geometry are untouched. Conversion, EPT and checkpoint version matrices include pre3.

See [measured delivery](../../docs/STORAGE_V6_PRE3_BENCH.md) for valid file sizes, CPU, checksums,
WASM/browser coverage and limits. Bitpacking integer ranges, shared RGB palettes, implicit
occupied leaves and reconstructed topology prefixes remain separate pending improvements.
