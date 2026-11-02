# Native `.oct` format, v7.4-pre

Development identity: major **7**, revision **4**, opt-in as `v7.4-pre`. V5 remains the default;
v5 and v7.1–3-pre stay readable with unchanged bytes. This revision changes the coordinate
frame, node identity and CRS metadata for the dense LOD pyramid of [R4](../../plans/render-r4-piramide-densa.md).
Decisions come from [ADR 0029](../../adr/0029-octree-global-por-bits-de-enteros.md) and the owner's
resolution of 02-11-2026 ([record](../../records/benchmarks/R4_CONVERT_TIME_2_02-11-2026.md)).

## Differences from V7.3-pre

Everything not listed here matches [V7.3-pre](FORMAT_V7_PRE3.md) (and through it [V5](FORMAT_V5.md)):
header, footer, file roles, payload directory entries, geometry/attribute codecs, CRCs, limits.

| Area | V7.3-pre | V7.4-pre |
| --- | --- | --- |
| Header revision / index magic / footer magic | `3` / `OCTVI73P` / `OCTVE73P` | `4` / `OCTVI74P` / `OCTVE74P` |
| Coordinates | per-block `f64` origin and scale, `u32` quantization | global `i64` quanta, `u32` offsets, fixed scale 2⁻¹⁵ |
| Node identity | bounds only | bounds **plus** prefix, derived absolute level, `spacing` |
| Frame | `f64` origin of the cloud minimum | `i64` quanta origin inside the root node (the writer uses the cloud minimum) |
| CRS | descriptive string | identity (authority + code) plus raw bytes |
| Source scale/offset | absent | stored for exact export |
| Fixed by the version | — | `spatial_divisions` = 2, leaf limit 64 000 points, `sample_grid` = 256 |

## Global frame

For each axis, a source coordinate `c` in CRS units becomes `q = round(c × 2^15)` as `i64`
(`f64::round`, ties away from zero; the product is exact). With LAS input `c = X as f64 * scale + offset`
evaluated in `f64`. The quantum 2⁻¹⁵ is fixed by the version: it is not a field. Writers reject any
point with `|c| >= 2^48` CRS units (`|q| < 2^63`, the `i64` range); no other format limit applies. The v7.4-pre converter rejects `|c| >= 2^35` CRS units so derived `f64` node bounds stay exact.

The octree is implicit in the bits of `u = q - base`, read as `u64`, with `base` per axis (the **lattice
base**). A cloud with a CRS identity (authority and code, or raw CRS bytes) uses `base = i64::MIN`, which
is `q ^ 0x8000_0000_0000_0000` (the sign bit flipped): the **global** lattice, shared by every cloud of
the CRS. A **local** cloud (no code and no raw CRS bytes; the CRS record says so, no extra field) uses
`base = frame origin quanta`, so its cells start at its own minimum corner and its root is its own first
split. Local clouds never merge, so they share nothing. A node at **absolute level** `a` (0..=64) has a per-axis **prefix** `P` of `a` bits (`u64`, `P < 2^a`) and covers

```text
u in [P << (64 - a), (P + 1) << (64 - a))      per axis; for a = 0, P = 0 covers everything
side = 2^(64 - a) quanta = 2^(49 - a) CRS units
node origin (quanta) = base + (P << (64 - a))     (global: ((P << (64 - a)) ^ 0x8000_0000_0000_0000) as i64)
```

A child at level `a + 1` has `P_child = (P_parent << 1) | bit` per axis (`spatial_divisions` = 2 only).
Shifts by 64 are defined as 0. Two clouds with the same CRS share nodes and cells at every level (RF-23).

**Stored root.** The root is the **smallest node containing all points**; the chain of single-child nodes
above it is omitted. It is the first node with more than one child whenever one exists, and a single-leaf
cloud also has a root. The index states the root's absolute level `a0` (any value 0..=64) and prefix; no
cloud box enlarges the root. A cloud straddling a high bit (e.g. local coordinates centred on 0) gets a
shallow root with chains of single-child nodes below it (tens of near-empty nodes); this is accepted. A
**local** cloud has no such chain: its lattice starts at its minimum corner, so its prefix is 0 and the
root is the first node that holds the cloud.

**Integer geometry.** Node origins and bounds are defined and computed in `i64` quanta from level and
prefix, never in `f64`. Every `f64` slot holding geometry (frame origin, node bounds) is derived and
informative: readers must not use it for exact computations.

**Frame origin.** The index stores the frame origin as 3 × `i64` quanta, a point **inside the root
node** (the converter writes the cloud's minimum quantized corner, exact in quanta). It is independent of
the root corner so that a shallow root (R4-Q5/Q6) keeps `f32` local positions small. The `f64` origin
triple of the index is that origin in CRS units (`origin_quanta × 2^-15`, rounded to nearest `f64`).
Decoded positions and stored node bounds are frame-local (`world − frame origin`), as in V5. Readers
validate that the quanta lie inside the root node and that the `f64` triple matches them.

## Index layout

The index follows V5's field order with these changes (numbers refer to the V5 index list).

| # | Field | V7.4-pre |
| --- | --- | --- |
| 1 | Frame origin | 3 × `f64`, unchanged slot; value derived from the quanta origin below. **New, immediately after it:** root absolute level `a0` (`u32`), root prefix (3 × `u64`), frame origin (3 × `i64` quanta) |
| 2 | Source point count | unchanged |
| 3 | CRS string | replaced by the CRS record below |
| 3b | Source scale and offset (new, after the CRS record) | `u8` tag (0 absent, 1 present; other tags invalid); if present 3 × `f64` scale then 3 × `f64` offset. Scale finite and > 0, offset finite |
| 3c | Sampling policy string | unchanged slot; descriptive value `sample-grid:256;leaf:64000`, not interpreted |
| 4–6 | Semantics, visual scale, schema | unchanged |
| 7 | Node table | node record changed (below) |
| 8 | Block directory | block record changed (below) |

### CRS record

| Field | Type | Rule |
| --- | --- | --- |
| Authority | `u32` length + ASCII | Empty if no code. Otherwise upper-case ASCII, ≤ 16 bytes (`EPSG`) |
| Code | `u32` | `0` iff authority is empty |
| Raw encoding | `u8` | `0` none, `1` WKT (UTF-8), `2` GeoTIFF keys |
| Raw length and bytes | `u32` + bytes | Empty iff encoding is `0` |

Encoding `1` stores the WKT VLR (`LASF_Projection`, record 2112) payload verbatim. Encoding `2` stores
three length-prefixed (`u32`) byte strings, all inside the raw length: the payloads of records 34735
(GeoKeyDirectory), 34736 (doubles) and 34737 (ASCII); an absent record has length 0. Raw bytes are kept
whole (RF-24) and not interpreted by the reader beyond UTF-8 validity for encoding `1`; the raw length
is capped by `max_string_bytes` (1 MiB by default).

**Identity** (what RF-02 compares): `(authority, code)` when present, taken from the WKT root `AUTHORITY`
(the root node's own, not a nested one) or from GeoKey 3072 (projected, checked first) or 2048
(geographic); GeoKey values 0 and 32767 (undefined, user-defined) are no code; otherwise the raw bytes compared exactly; with
neither the cloud is **local**: it is never merged with or mixed into another cloud, not even another local one.

### Node record

V5 record plus two inserted fields; everything else keeps its order and type.

| Field | Type | Change |
| --- | --- | --- |
| Node ID, parent, **Level** | `u64`, tag+`u64`, `u32` | Level stays the depth below the stored root (root = 0), because the hierarchy validator requires it; absolute level = `a0 + depth` and must be ≤ 64 |
| Prefix (new, right after Level) | 3 × `u64` | Must equal the parent's shifted prefix plus one bit per axis; the root's equals the index root prefix |
| Bounds | 6 × `f64` | Derived, informative: the integer box from level and prefix minus the frame origin, times 2⁻¹⁵, rounded to nearest `f64`; readers check equality with that value |
| Point count, geometric error | `u64`, `f64` | Geometric error unchanged in meaning |
| Spacing (new, right after geometric error) | `f64` | Below |
| Blocks, ranges | unchanged | |

**Spacing** is finite, positive and an exact power of two (zero mantissa bits; checked from the `f64`
bits, no `log2`). An internal node has `side / 256 = 2^(41 - a)` CRS units: it halves per level. A leaf
stores the measured spacing `side / 2^d`, with `d` the first level where the occupied cells, once more
than one, grow by less than ×1.5 (`d` up to 32; a cloud tucked into a corner of its leaf is measured where
it starts to spread) (`0 <= d <= 64 - a`, i.e. between one quantum and `side`). Readers validate the power of two,
the range `2^-15 <= spacing <= side` and, for a node with children, the exact value `2^(41 - a)`.

The index header grows by 24 bytes (frame origin quanta). The minimum encoded node grows by 32 bytes (3 × `u64` + `f64`): `MIN_NODE_RECORD_LEN` in
`index_codec.rs` goes from 85 to 117.

### Block record

The 94-byte V7 prefix (V5's 93 plus the geometry-encoding byte) becomes 70 bytes: the local
quantization origin and scale (6 × `f64`) are replaced by one **block origin** (3 × `i64`, quanta).

| Field | Type |
| --- | --- |
| Block ID, payload offset, primary stored length, point count | 4 × `u64` |
| Geometry encoding | `u8` (as V7.3-pre) |
| Quantization error | `f64` |
| Block origin | 3 × `i64` |
| Reserved aggregate codec and checksum | `u8`, `u32`, both zero |

The scale is always 2⁻¹⁵. For a node whose side is ≤ 2^32 quanta (`a >= 32`) the block origin equals the
node origin. A node with side > 2^32 quanta (`a < 32`) is **split**: one block per occupied cell of side
2^32 quanta, whose origin is the node origin plus a multiple of 2^32 per axis; blocks are listed in
ascending block ID, and their order is otherwise free (open point 2). Payload directory entries follow unchanged.

## Payload and decoding

Geometry is the V7.3-pre triple of `u32` per row with the same codecs (raw, delta-varint, compact
24-bit occupancy); only the meaning of the values changes: `offset = q - block_origin`, `0 <= offset < 2^32`
per axis, and the point lies inside its node. Decoding to the reader's `f32` local position:

```text
local = (((block_origin + offset) - frame_origin_quanta) as f64 * 2^-15) as f32
```

with the difference in checked `i128` (it can exceed `i64` for a shallow root) and its conversion to `f64` rounded to nearest, which is below `f32` resolution.
**Quantization error** keeps V5 semantics: a conservative bound on the Euclidean distance from the
source position (frame-local `f64`, before rounding to the grid, so the grid error ≤ √3·2^-16 is included) to
the decoded `f32`, with the V5 arithmetic margin (`norm3`, no `hypot`); the writer rejects a block above `--max-error`. The node geometric error must be at least the
maximum error of its blocks.

## Exact export

With source scale/offset present, a decoded point maps back to `X = round((q × 2^-15 - offset) / scale)`
(`f64`). This returns the exact LAS integer when `scale >= 1e-4`: the grid error is ≤ 2^-16 (≈ 1.5e-5),
under half the scale. Finer scales lose exactness. Without scale/offset the cloud is not exportable
as LAS integers.

## Validation (readers)

Every "must" above, plus the V5 rules for IDs, ranges, contiguity and CRCs. Cross-field checks: frame
origin quanta inside the root node and `f64` triple = quanta × 2^-15; level ≤ 64; child prefix = parent prefix extended; bounds derived from prefix;
authority empty iff code 0; encoding 0 iff raw empty.

## Writer restrictions

- `spatial_divisions` = 2, leaf limit 64 000 and `sample_grid` = 256 are not options in this version.
- Hierarchy source EPT or Additive with `--format v7.4-pre` is an explicit error.
- Block checkpoint/resume is not offered (`supports_block_checkpoint` false).
- Output must be byte-identical between the web and native converters (RQ-08).

## Not in this revision

- **LAS flags (withheld, synthetic, keypoint, overlap) and Extra Bytes (RF-24)** are deferred to a later
  pre revision: they are orthogonal to the pyramid and the coordinate frame, and a new pre revision is cheap.
- Reprojection (RF-22) is a CLI option that does not change the format (R4-Q4).
- Quanta finer than 2⁻¹⁵ and CRS units where 2⁻¹⁵ is too coarse (degrees): revisit per ADR 0029.

## Measured CRS evidence (02-11-2026)

| Dataset | LAS | CRS in the file | Scale / offset | Result |
| --- | --- | --- | --- | --- |
| Belvedere | 1.4 COPC | WKT VLR 2112 with `AUTHORITY["EPSG","32632"]` | 0.01 / 0 | identity `EPSG:32632`, raw = WKT |
| Majadas | 1.2 | none | 0.001 / 262000, 4424000, 0 | **local**: not mergeable |

## Open points for review

1. **Absolute level is derived, not stored.** The plan says "per node: absolute level"; the Level slot
   keeps the depth because `SpatialHierarchy` requires the root at level 0 and child = parent + 1.
   Absolute level = `a0` + depth carries the same information without redundancy.
2. **Block origin field** replaces the V5 origin/scale to support split nodes (> 2^32 quanta); ADR 0029
   says only "split". Alternative: derive it from the node and forbid points in nodes with `a < 32`.
   The one-block-per-cell split rule and the free block order are proposals.
3. **GeoTIFF raw layout** (three length-prefixed records) and the `sample-grid:256;leaf:64000` policy
   string are proposals; no source of GeoTIFF CRS has been measured yet.
4. **Internal spacing** `side / 256` assumes the pyramid's internal grid is exactly `sample_grid`.
5. `FormatMetadata::crs()` (`Option<&str>`) and `DatasetMetadata` need a richer CRS/frame API (Task 3).
6. **WKT2 and compound CRS.** Identity extraction covers WKT1 `AUTHORITY` and GeoKeys only; WKT2 `ID[...]`
   and the vertical part of a compound CRS are unspecified.
7. **CRS evidence** (table above) was measured on 02-11-2026 without a committed record; Task 4 should
    repeat it with `las::Header::vlrs()` and cite the output.
