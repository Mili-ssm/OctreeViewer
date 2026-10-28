# Native `.oct` format, v6.2-pre

Exact identity: major **6**, revision **2**. V5 remains the default. Pre2 reads/writes
point data with the same quantization, row identity and typed attributes as v5/pre1.
Its purpose is smaller physical geometry payloads and less I/O, accepting CPU reconstruction.
Native Rust structure size is not the storage layout or the acceptance criterion.

## Scope and envelope

The common header, index metadata, outer spatial hierarchy, block directory, independent CRC/LZ4
payloads and `.octattrs` contract follow [v6.1-pre](FORMAT_V6_PRE1.md). Both files must have the
same exact identity. New index/footer markers are `OCTVI62P` and `OCTVE62P`.

Geometry tags permitted in pre2: `0` raw XYZ u32, `1` delta-varint, **`2` occupied-level arrays**.
Pre1 still permits only tags 0/1; it must reject tag 2 even with valid checksums. Unsupported
revision pre5 and final v6 remain rejected; [pre3](FORMAT_V6_PRE3.md) adds attribute codecs
and [pre4](FORMAT_V6_PRE4.md) adds compact occupancy and byte planes. Prepared blocks/checkpoints retain exact identity.

The level arrays are **per independently addressable geometry block**, not one contiguous array
covering the whole cloud. This preserves selected-block reads through the existing traits and
keeps RGB/attribute columns independent. The outer hierarchy/index is retained for source LOD;
this revision does not compact its separate node records or define a shared physical voxel grid.

## Occupied geometry stream (tag 2)

All integers are little endian. N is the point count in the existing block directory; occupied
mode requires `0 < N <= u32::MAX`. No source-order point coordinates are stored separately.
The entire following stream is one ordinary independently compressed/checksummed payload.

```text
configuration:u8
level_counts[D+1]:u32[]
level_0_nodes[...]:6-byte[]
...
level_D_nodes[...]:6-byte[]
leaf_row_groups:canonical-u32-varint[]
packed_residuals:byte[]
```

Configuration: bits 0–5 store residual width `b` (0–32), bit 6 enables explicit own-octant
metadata, bit 7 must be zero. Depth is `D = 32-b`. Each level is nonempty, contains no more
than N nodes, and the root level has exactly one node. The counts precede all node arrays.

### Voxel/node record: exactly 6 bytes, without native padding

| Byte offset | Field | Width | Meaning |
| ---: | --- | --- | --- |
| 0 | Child mask | `u8` | Bit k marks occupied octant k. |
| 1, bits 0–3 | Child count | 4 bits | 0–8; must equal `popcount(mask)`. |
| 1, bits 4–6 | Own octant | 3 bits | Position within the parent, only when configuration bit 6 is set. |
| 1, bit 7 | Reserved | 1 bit | Must be zero. |
| 2 | First child | `u32` | Index in the next level's array, never a byte offset or memory address. |

The octant convention is `(x_bit<<2)|(y_bit<<1)|z_bit`, matching the existing core subdivision.
Parents are in path order; each parent's children are contiguous in increasing occupied-octant
order. A child's index is `first_child + popcount(mask & ((1<<octant)-1))`.

`first_child` must equal the running prefix of prior parents' child counts, and the final prefix
must fill the next level exactly. There are no gaps, shared children or orphan nodes. Internal
nodes have at least one child. Leaves have mask/count/index zero; their own-octant bits may be
nonzero in the explicit variant. The root's own octant is always zero. In the inferred variant
all own-octant bits are zero. In the explicit variant they must match the parent's mask/order;
the field does not allow arbitrary coordinates or parentage.

### Rows and exact coordinate reconstruction

For each leaf in final-level/path order:

1. Nonzero row count, canonical unsigned u32 LEB128.
2. First original row ID, absolute canonical u32 LEB128.
3. Remaining original row IDs as strictly positive differences, canonical u32 LEB128.

Rows within a leaf are ascending. Each row ID is less than N, appears exactly once across all
leaves, and the sum of leaf row counts is N. Varints use at most five bytes; byte five may not
exceed `0x0f`. Overlong/noncanonical encodings, zero subsequent differences and overflow fail.
No bytes may remain between the row groups and residuals.

Residuals occupy exactly `ceil(3*N*b/8)` bytes, in **original row/XYZ order**, least significant
bits first. Final unused bits must be zero. For each original quantized component:

```text
coarse = q >> b
residual = q & ((1<<b)-1)
q = (coarse << b) | residual
```

Shifts/arithmetic use u64, including b=32. Walking octants from an unsigned zero root derives
`coarse` with no stored coordinate per node. This is equivalent to the core candidate's biased
`i32::MIN + coarse` cell space. The affine transform/error remain in the block directory.
They may be anisotropic or constant-axis; these code-space cells are not a new physical cube
or a CRS contract. Exact q and original row order retain RGB/attribute association.

## Writer selection

V5/pre1 layouts remain unchanged. Pre2 evaluates raw and delta as before, then occupied
candidates b=24 and b=28, each with inferred and explicit own-octants. Candidates are built
sequentially. Compare **actual stored bytes after raw/LZ4 selection**, not native capacities or
an estimated record count. Only strictly smaller payloads replace the current winner; ties
preserve the earlier candidate (raw before delta; inferred before explicit within one width).

The octant metadata costs no additional uncompressed byte, but can change compressed length
and CPU work. Both variants are supported so selection can follow measured physical bytes,
without assuming that stored octants or inference always win. CPU measurements are reported
separately; this policy prioritizes bytes and does not guarantee faster conversion/read.

Each candidate must fit the configured aggregate raw block limit, its level counts and child
indices must fit u32, and all size arithmetic is checked. Otherwise use fallback. Empty blocks
use raw encoding. The geometry directory tag already exists in pre1, so pre2 adds no further
per-block index bytes. With equal source/options, its selected geometry cannot exceed pre1's
stored geometry; comparisons against v5 must still include the pre1 index-byte overhead.

## Reader validation and resources

Existing stored/raw/logical payload limits are checked before decompression or point allocation.
For occupied mode a conservative raw-length bound is `N+12 <= raw_length <= 220*N+133`, checked
for overflow. The structural parser validates level counts/ranges before node access, mask/count
consistency, canonical child prefixes, own-octants, leaf fields, residual extent/padding, row
varints/counts/IDs and uniqueness. Truncation and trailing data fail even with correct CRCs.

Node arrays are borrowed slices; the reader does not recreate the core topology/index. It uses
a depth-bounded traversal stack, a row-uniqueness bitset and a temporary `12*N` u32 output array,
then the existing point projection. Limits bound payload/logical sizes, **not peak RSS**. This
CPU reconstruction and temporary allocation are deliberate; memory can be optimized separately
without changing the six-byte disk representation or the public reader traits.

## Verification and evidence

Tests cover all 33 residual widths and both octant variants, exact integer roundtrips, metadata
corruption/truncation, row/padding errors, physical occupied selection in mono/split files,
pre1 rejection with repaired CRCs, typed attributes and source row order, selective I/O/forks,
serial/parallel native conversion, EPT and checkpoint resume in pre1/pre2.

Playwright MCP checked Blob attachment/rejection and real OPFS pre2 conversion on a generated
clustered LAS. Eight browser-written blocks used occupancy; native reconstruction matched the
browser v5 pair's logical checksum. [Browser report](../benchmarks/data/storage-v6-pre2-browser-2026-10-14.json).
No WebGPU adapter was available; this is storage/interoperability evidence, not presentation.

Physical byte/CPU comparisons and real-file measurements are recorded in
[storage benchmarks](../benchmarks/STORAGE_BENCH.md). V6 final, a single global per-depth topology,
physical shared-grid semantics and further codecs remain separate pending work.
