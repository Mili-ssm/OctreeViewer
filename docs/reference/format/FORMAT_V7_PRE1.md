# Native `.oct` format, v7.1-pre

Development identity: major **7**, revision **1**, explicitly selected as `v7.1-pre`.
Final V7 (revision zero) and unimplemented revisions are rejected. This is a working migration
baseline, not the final V7 storage design. V5 remains the operational default until the
[V7 promotion criteria](../../plans/format-v7-plan.md) are met; V5/V6 are scheduled for retirement.

## Binary contract

The payload, metadata, hierarchy, quantization, index records, file roles and validation rules
are inherited from [V6 pre4](../../records/archive/FORMAT_V6_PRE4.md), including its V5/pre1/pre2/pre3 references.
The differences are exact identity in the common 24-byte header and the eight-byte markers:

| Field | V7.1-pre |
| --- | --- |
| Header magic | `OCTVFRM1` |
| Major / revision | `7` / `1` |
| Index magic | `OCTVI71P` |
| Footer magic | `OCTVE71P` |

Header, index and footer checksums cover the same fields as pre4. Both monolithic and split
writers use this identity. Primary and attribute companion must agree exactly, as must
prepared blocks and their writer. Native checkpoints retain output major and revision;
resume never converts an old checkpoint into a new revision.

Geometry supports raw u32, delta-varint, legacy occupancy and compact occupancy; its payload
codecs are raw/LZ4. RGB and typed columns support raw/LZ4, constant, dictionary and byte planes.
Each payload has its own stored-byte CRC, checked before decoding. Original rows, duplicates,
typed bits and quantization/error semantics are unchanged. XYZ does not require a sidecar;
RGB and individual columns remain independently readable. These transforms do not establish
a shared physical grid or CRS between datasets.

For identical blocks/options, codec choices and payload bytes match pre4. Header/index/footer
identities and checksums differ. Identity is not a compression improvement, and existing file
pairs must not be relabelled as a substitute for conversion and validation.

## Execution and resources

The existing native LAS/LAZ preparation pipeline can produce V7.1-pre with ordered output.
EPT local conversion remains serial. `fork_reader` shares a validated index across independent
cursors; this does not connect concurrent reads to the viewer. WASM reading remains synchronous,
and web conversion still defaults to V5; `createForVersion` selects implemented revisions.
V7 web conversion has not been exercised in a browser in this increment. Runtime concurrency is independent of physical version.

The common attribute encoder now packs byte-column dictionary IDs directly from input after
palette construction, avoiding N intermediate u32 IDs. It releases wider-column lookup and
raw dictionary buffers before later phases. Codec candidates and output bytes are preserved.
This removes a specific buffer; it does not establish a measured RSS reduction or an aggregate
memory guarantee. Stored bytes, scratch, output, index and cache still need joint accounting.

`FormatReader::plan_read` now exposes selective payload ranges and a conservative estimate
of logical payload-buffer peaks without reading payloads. `read_selected` checks a per-read
budget before I/O. Both use the existing layout, so no new physical revision is introduced.
The estimate includes codec buffers and typed output; it excludes index, schema/control
metadata, transport caches and allocator/runtime overhead. It is not an aggregate concurrent
quota or an RSS guarantee. See phase 2 of the plan for formulas, evidence and remaining work.

## Use

```sh
cargo run --profile perf -p viewer_convert -- INPUT.laz OUTPUT.oct \
  --format-version v7.1-pre --max-memory-mib 512 --spatial-workers 4 --pipeline-writes
# For LAS/LAZ only, --checkpoint then --resume retain completed compatible phases.
```

V7 changes that alter physical interpretation will require another exact development revision;
the final identity must not be enabled by accepting every major-7 header. See the
[plan](../../plans/format-v7-plan.md) for async integration, format candidates and retirement gates.
