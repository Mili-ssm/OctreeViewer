# Octree Viewer Block Format, version 2

Version 2 adds an explicit hierarchy payload-combination mode. All byte encodings not listed here
remain as specified by [version 1](FORMAT_V1.md). This is an additive change: readers must continue
to accept v1 files, interpreting them as replacement hierarchies.

## Version markers

The 24-byte header has the same layout as v1, with `version = 2`. The index starts with
`OCTVIDX2`; the 40-byte footer uses `OCTVEND2`. Header, index and footer checksums and all offsets
retain the v1 definitions.

## Index change

Immediately after the sampling-policy string and before the attribute count, v2 stores one hierarchy
semantics tag:

| Tag | Meaning |
| --- | --- |
| `0` | Replacement: child payloads replace their parent's payload when refining. |
| `1` | Additive: child payloads contain distinct points and are added while ancestor payloads remain selected. |

No other tag is valid. The node point count still describes only the points in that node's own
payload. In additive files, the source-point count is the sum of all node payload counts; writers and
readers validate that relationship when the source count is present. Version 1 has no semantics tag
and always means replacement.

## LOD interpretation

For replacement files, a selected cut contains one payload per visible branch. Refining a node
atomically removes its payload and selects its children. For additive files, the root payload is the
base cut; refinement retains every selected ancestor and adds the child payloads. The point budget
counts all selected payload points in both modes. Producers define the geometric-error estimate for
their hierarchy; the EPT converter currently stores the diagonal of each node's spatial cell as its
resolution estimate.

## Compatibility

Version 2 readers interpret v1 files as replacement semantics and must reject v3 because they do not
understand its scale field. The current reader accepts versions 1, 2 and 3. Future changes to the v2
index layout or tag meanings require a new version. Version 3 adds renderer-only visual scaling; see
[FORMAT_V3.md](FORMAT_V3.md).
