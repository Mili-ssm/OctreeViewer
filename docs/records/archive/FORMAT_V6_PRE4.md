# Native `.oct` format, v6.4-pre

Exact identity: major **6**, revision **4**, opt-in. V5 remains the default; final v6 and pre5
are unsupported. Pre4 preserves source rows, quantization/error, schema, hierarchy, and the
split companion contract. It adds two independent physical candidates to [pre3](FORMAT_V6_PRE3.md),
without serializing the separate experimental node summaries or joint attribute dictionaries.

The common 24-byte header and 40-byte footer follow v5. Markers are `OCTVI64P` and `OCTVE64P`.
Primary, companion, prepared blocks and checkpoints require the same exact revision. Legacy
tags keep their bytes/meaning; older layouts reject new tags even with repaired checksums.

## Independent byte-plane attributes (payload codec 4)

Geometry payload codecs remain raw/LZ4 (0/1). RGB and typed attributes additionally allow
constant (2), dictionary (3), and **byte planes + LZ4 (4)**. Codec 4 has no extra payload header.
For N rows and tuple width W derived from the existing schema/directory:

```text
planes[byte*N + row] = original[row*W + byte]
stored = LZ4_block(planes)
```

All components of one attribute form its tuple, as in pre3. The transform separates byte
positions, not numeric values: it preserves NaN payloads, signed zeros and integer bits.
Each column remains independently compressed/checksummed and independently readable. No
shared row IDs or cross-column dependencies are introduced.

The directory retains its 29-byte record. `raw_length` is the original logical column size.
Require N>=2, W>1, `0 < stored_length < raw_length`, and pre4 identity. The CRC covers all
stored bytes and is checked before decompression. Decode exactly `raw_length` bytes of LZ4,
then invert the transpose. Generic raw/LZ4 decoding never interprets codec 4 itself.

The writer first evaluates the exact pre3 candidates; it tries the shuffle only for W>1,
N>=2 and when the winner is not Constant. Replace only if the actual LZ4 bytes are strictly
smaller. No per-column metadata is added, so selected attribute payload sizes never exceed
pre3 for identical input. This is a byte-size guarantee, not a CPU/latency guarantee.

## Compact occupancy geometry (geometry tag 3)

Tags 0/1/2 retain raw XYZ, delta-varint and legacy six-byte occupancy nodes. Tag 3 stores:

```text
residual_bits:u8                 // b=0..32; bits 6/7 reserved zero
level_counts[D+1]:u32[]          // D=32-b, root first
internal_level_0_masks:u8[]
...
internal_level_D_minus_1_masks:u8[]
leaf_row_groups:canonical-u32-varint[]
packed_residuals:byte[]
```

The internal levels contain exactly one nonzero occupancy mask per node. **Leaves have no
node records.** Child counts are popcounts and first-child indices are prefixes of prior
masks' popcounts. No count, own-octant or first-child field is serialized. The octant convention,
path ordering, leaf groups, original row IDs and residual bits are unchanged from
[pre2](FORMAT_V6_PRE2.md). At D=0 the root is an implicit leaf and there are no masks.

Every level count is nonzero and <=N; root count is one. Every internal mask is nonzero; its
level's popcount sum equals the next level count. Validation precedes traversal. Ascending-octant
DFS visits every depth in path order, allowing first-child prefixes to be reconstructed with
33 running child counters and a depth-bounded stack. No prefix array per node or native tree
is materialized. Final counters must fill every next level exactly.

Leaf groups must cover each source row once, ascending within each leaf. The existing
canonical varint, row uniqueness/count, residual extent/padding and trailing-byte checks
apply. The reconstruction returns exact original u32 coordinates in original row order.

For N>0 a conservative raw bound is `N+6 <= raw_length <= 54*N+133`, checked before allocation.
The upper bound allows 32*N internal masks, 5*N leaf-count bytes, 5*N row-ID bytes, 12*N
residual bytes and 133 header/count bytes. Empty geometry uses raw fallback. Arithmetic is
checked on native and 32-bit WASM.

The writer evaluates the existing b=24/28 legacy occupancy candidates and their own-octant
variants, plus the compact variant for those same b values. It reuses each built
`QuantizedOccupancy`, compresses candidates sequentially, and replaces the current winner only
when actual stored bytes decrease. Compact candidates must fit the configured aggregate raw
block budget. Raw/delta/legacy occupancy remain available; pre4 geometry payloads cannot
exceed the pre3 winner. Index size for equivalent source/schema is unchanged, though tags,
offsets and checksums differ. No precision, physical-grid or CRS contract is changed.

## Resources and verification

Byte-plane decoding can retain stored bytes, the full decompressed plane buffer and the full
reconstructed column simultaneously. Occupancy still allocates `12*N` output bytes and an
N-bit row-uniqueness bitset. Existing per-buffer/logical-block limits are not a combined RSS
limit. Candidate construction and compression add CPU and transient memory even when they lose.

Tests cover all 33 residual widths, u32 extremes, duplicates, D=0, implicit leaves, bad masks/
counts/varints/rows/padding, truncation, exact attribute bits/components, selective byte counts,
new-tag rejection by older revisions, wrong companions and unknown/final versions. Conversion,
EPT and checkpoint version matrices include pre4. See the [delivery measurements](../benchmarks/STORAGE_V6_PRE4_BENCH.md)
for valid file sizes, CPU/RSS, checksums, dependency provenance and browser evidence limits.
