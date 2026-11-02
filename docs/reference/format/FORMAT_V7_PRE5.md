# Native `.oct` format, v7.5-pre

Development identity: major **7**, revision **5**, opt-in as `v7.5-pre`. V5 remains the default;
v5 and v7.1–4-pre stay readable with unchanged bytes. This revision pages the node table so that
opening a cloud no longer costs bytes proportional to its node count
([plan R5](../../plans/render-r5-jerarquia-paginada.md), task P2). Decisions come from
[ADR 0030](../../adr/0030-jerarquia-paginada-por-anclas.md) and the owner's resolutions of
02-11-2026; the size model comes from the [P0 spike](../../records/benchmarks/R5_P0_PAGES_02-11-2026.md).

## Differences from V7.4-pre

Everything not listed here matches [V7.4-pre](FORMAT_V7_PRE4.md): header, global frame, CRS record,
geometry/attribute codecs, payload CRCs, block origin and split rule, spacing rules, limits.

| Area | V7.4-pre | V7.5-pre |
| --- | --- | --- |
| Header revision / index magic / footer magic | `4` / `OCTVI74P` / `OCTVE74P` | `5` / `OCTVI75P` / `OCTVE75P` |
| Index | one buffer: schema, every node, every block | **metadata block + root page** next to the footer; the rest in child pages |
| Node identity | `NodeId` and parent stored per node | derived from `(level, prefix)`; keyed in a page by `(depth, path)` |
| Block directory | one table | inline in each node's page record, slimmed (offsets and raw lengths derived) |
| Bounds, spacing of internal nodes | derived, but bounds stored | derived, not stored |
| Split files (`.octattrs`) | supported | **not supported**: monolithic only |
| Index validation at open | whole index | metadata block + root page; each child page when it is read |
| Global checks (no gaps between blocks, totals) | at open | `viewer_convert verify` (R5-Q3); `FormatReader` still runs them because it reads every page |

## File layout

```text
header (24 B, revision 5)
block payloads                      [24, pages_start)         written in node order, as in V7.4
child pages, breadth-first          [pages_start, index_offset)
metadata block                      } index region, [index_offset, footer)
root page                           }   (index_length = metadata block + root page)
footer (40 B, magic OCTVE75P)
```

- Footer fields are V5's: index offset, index length, file length, index CRC32 (over the whole index
  region) and footer CRC32. `index_offset + index_length` is the footer offset.
- The root page is the **last** part of the index region, so a tail read gets footer, metadata block
  and root page together. Readers fetch the last `TAIL_READ_BYTES` (1 MiB) of the file, or the whole
  file if smaller; the footer says whether the index fits in that read, and only if it does not is a
  second request needed. They identify the revision from the **footer magic** and need not read the
  24-byte header, so opening costs one request.
- The metadata block is self-delimiting; the root page is the remainder of the index region.
- Child pages are written breadth-first: the pages hanging from one page are contiguous, so one range
  request can bring several sibling pages. The writer plans all pages, then writes them.

## Metadata block

The V7.4-pre index fields up to and including the schema, unchanged and in the same order
(dataset origin, root level `a0`, root prefix, frame origin quanta, data maximum, source point count,
CRS record, source scale/offset, sampling policy, semantics, visual scale, schema), then:

| Field | Type | Rule |
| --- | --- | --- |
| Index magic | 8 bytes | `OCTVI75P`, first in the block |
| Total nodes | `u64` | 1 ..= `max_nodes`. Informative until every page has been read |
| Total blocks | `u64` | ≤ `max_blocks`. Informative until every page has been read |
| Pages start | `u64` | End of the block payloads and start of the child pages; ≥ 24 and ≤ `index_offset` |
| Extension length | `u32` | Must be 0 in this part. **Hook for [plan task P2a](../../plans/render-r5-jerarquia-paginada.md#tarea-p2a-flags-las-y-extra-bytes-rf-24)**: LAS flags and Extra Bytes descriptors will go here (or as schema attributes); nothing else about them is defined yet |

The schema may hold at most 65 535 attributes in this revision (`u16` indexes in pages).
Hierarchy source EPT, Additive semantics and checkpoints are rejected by the writer (R5 constraint).

## Node identity and order

A node is **`(level, prefix)`**, as in V7.4-pre. Inside the tree below the stored root:

- **Depth** `d = level − a0`. The writer and readers reject `d > 21` (the **depth limit**): the id
  needs `1 + 3d` bits and must fit a `u64`.
- **Octant** of a child: `x | y << 1 | z << 2`, where each bit is the last bit of the child's prefix on
  that axis. Children are ordered by ascending octant.
- **`NodeId`** (derived, never stored): `1` (a sentinel bit), then for each level below the root the
  3-bit octant of the node's ancestor-or-self at that level, most significant first. The root is `1`; the
  parent of `id` is `id >> 3`. The id is unique per `(level, prefix)` under one root.
  `viewer_format::derive_node_id` implements it.
- **Path** (inside a page) of a node at relative depth `r` below the page anchor: the same octants, `3r`
  bits, ancestor first; `path < 8^r` and fits `u16` because `r ≤ 5`. The pair `(r, path)` is the node's
  key in the page.
- **Order in a page**: ascending `(r, path)`. This is breadth-first and fully determined by the tree.

## Pages

A page covers **`k` levels** (1 ≤ `k` ≤ 5) below its **anchor**: a node of the parent page (or the root).
The anchor's own record lives in the parent page, so the anchor can be selected and drawn before its
page is read; the root page additionally holds the root node.

```text
page := header, node record × node_count, crc32
header := anchor level u8, anchor prefix 3 × u64, k u8, node_count u32, first_block_id u64     (38 bytes)
crc32  := over every preceding byte of the page                                                 (4 bytes)
```

The page length is known from the pointer (child pages) or from the index region (root page).

### Node record

| Field | Type | Rule |
| --- | --- | --- |
| Depth | `u8` | Relative to the anchor. Child pages: 1 ..= `k`. Root page: the first record has depth 0 (the root), all others 1 ..= `k` |
| Path | `u16` | `< 8^depth` |
| Flags | `u8` | Bit 0: has children. Other bits must be 0 (reserved; room for P2a) |
| Point count | `u64` | Equals the sum of the point counts of its blocks |
| Geometric error | `f64` | Finite, ≥ 0 and ≥ the quantization error of each of its blocks |
| Spacing exponent | `i8` | **Only if the node has no children.** Leaf spacing is `2^e` CRS units, `−15 ≤ e ≤ 49 − level`. A node with children has the derived spacing `2^(41 − level)` (requires `level ≤ 56`) |
| Block count | `u32` | |
| Blocks | block record × count | Below |
| Range count | `u16` | ≤ schema size; 0 if the point count is 0 |
| Ranges | repeated | `u16` schema index (strictly ascending), min values, max values, as in V7.4-pre |
| Child page pointer | `u64` offset + `u32` length | **Only if depth = `k` and the node has children**: the page anchored at this node |

Not stored (derived): node id, parent id, level, prefix, bounds (from level and prefix, as in V7.4-pre),
internal spacing, block ids, block origin of nodes with `level ≥ 32`, payload offsets inside a block
and the raw lengths of every column except geometry.

### Block record (inline)

| Field | Type | Rule |
| --- | --- | --- |
| Payload offset, payload length, point count | 3 × `u64` | Payload range inside `[24, pages_start)` |
| Geometry encoding | `u8` | As V7.3-pre |
| Quantization error | `f64` | Finite, ≥ 0 |
| Cell | 3 × `u32` | **Only if the node's level is < 32** (split node): the block origin is the node origin plus `cell << 32` quanta per axis; valid iff it satisfies the V7.4-pre split rule. Otherwise the origin is the node origin |
| Payload entries | one per column: geometry, display colours, then the schema | Codec `u8`, CRC32 `u32`, stored length `u64`; the geometry entry also has its raw length `u64` |

Column offsets inside the block are the running sum of the stored lengths, starting at 0, and must fill
the payload length exactly. The raw length of the colour column is `3 × points` and of an attribute
`points × components × width`. The codec rules (constant, dictionary, byte planes, none/LZ4 lengths)
are V7.4-pre's.

### Block ids

`BlockId` is not stored per block: the blocks of a page are numbered `first_block_id`,
`first_block_id + 1`, … in node order, then block order. The writer numbers pages in plan order
(root page first, then breadth-first), so ids are unique in a valid file; a reader that holds several
pages rejects a page whose id range intersects another resident page's.

### Choosing `k`

Per page, the writer takes the **largest `k` ≤ 5** (also ≤ `21 − d_anchor` and ≤ `64 − level`) whose
encoded page is ≤ 512 KB (`TARGET_PAGE_BYTES`); when even `k = 1` is larger it uses `k = 1`
(R5 stop criterion: a `k = 1` page above 1 MiB stops the plan). The page size is computed from the
records, so the choice is deterministic and does not depend on pointer values (fixed width).

## Page validation

A reader validates **each page** when it reads it, and rejects the page on any failure:

1. Length ≥ minimum page and ≤ `max_page_bytes`; CRC32 matches.
2. Header: anchor level ≤ 64, prefix fits the level and lies under the root prefix; `k` in 1 ..= 5,
   `level + k ≤ 64`, anchor depth `+ k ≤ 21`; node count in 1 ..= 37 448 (+1 for the root page) and ≤ `max_nodes`.
3. **Anchor**: the root page's anchor is the stored root; a child page's anchor must equal the level and
   prefix of the node whose pointer was followed.
4. Node keys: depth and path in range, strictly ascending; flags reserved bits 0; the first record of the
   root page is the root (depth 0) and no other record has depth 0.
5. Structure: every node at depth ≥ 2 (≥ 1 in the root page) has its parent in the page and that parent
   has children; every node with children and depth < `k` has at least one child in the page.
6. Block ranges are inside the data section `[24, pages_start)` and **do not overlap** within the page;
   payload directories, split cells, spacing, point counts and geometric errors follow the rules above.
7. A child pointer lies in `[pages_start, index_offset)` and its length is ≥ the minimum page and ≤ `max_page_bytes`.
8. **No `BlockId` repeats** across the pages a reader holds (checked when a page is added; a released page
   frees its ids).

The metadata block and the root page are validated at open (index CRC32 over both, magic, extension
length 0, frame rules of V7.4-pre). Checks that need every page — contiguous payload section without gaps,
declared totals, one owner per block, parent/child prefix consistency across pages — are the job of
`viewer_convert verify`. `FormatReader` (which reads every page eagerly) also runs them and exposes the
assembled hierarchy with derived `NodeId`s; consumers must not assume they equal V7.4-pre's ids.

## Limits

`FormatReaderLimits` gains two limits (`with_page_limits`, accessors `max_page_bytes()` and
`max_resident_pages()`):

| Limit | Default | Meaning |
| --- | --- | --- |
| `max_page_bytes` | 1 MiB | Largest page a reader accepts, from a pointer or the index region |
| `max_resident_pages` | 4096 | Pages a lazy reader may hold; the caller checks it with `PagedReader::check_resident_pages` before adding a page |

`max_nodes`, `max_blocks`, `max_attribute_ranges` and the string limits keep their meaning; the first
two bound the declared totals and, for the eager `FormatReader`, the pages actually read.

## Writer restrictions

- Everything in V7.4-pre's "Writer restrictions" except that split files are not offered and the
  index is paged.
- Output must be byte-identical between the web and native converters (RQ-08); the page plan depends only
  on the tree and the block records, never on timing or thread order.
- `FormatWriterOptions::with_page_target_bytes` lowers the page target for tests and tools; the default is
  512 KB.

## Not in this revision (yet)

- **LAS flags and Extra Bytes (RF-24)** belong to `v7.5-pre` (R5-Q4) but are plan task **P2a**; only the
  extension-length hook above and the reserved node flag bits are defined here.
- Lazy loading in the viewer (P3) and HTTP Range (P4): the sync `PagedReader` is the format-level API
  those build on.
- Converter output, the `verify` subcommand and the 6 M-node generator (second half of P2).

## Open points for review

1. **`BlockId` as `first_block_id` + ordinal** instead of derived from the node: a split node can own many
   blocks, and a node id already uses up to 64 bits, so no id function of the node alone exists.
2. **Total node and block counts** are informative until all pages are read; the lazy path cannot verify them.
3. **Tail read size** (1 MiB) is a hint tuned for a 512 KB root page; `k = 1` root pages up to 1 MiB plus
   the metadata block can need a second request.
4. **Depth limit 21** equals ADR 0030's "revisit if more than 21" threshold (OQ-07), now enforced by the id
   encoding.
