# Native `.oct` format, v7.3-pre

Development identity: major **7**, revision **3**, opt-in. The final [v7](FORMAT_V7.md) is the default output
([ADR 0044](../../adr/0044-v7-final-congelada-y-predeterminada.md)); later unimplemented revisions remain rejected. V7.1-pre and V7.2-pre retain their exact bytes.

## Binary contract

Metadata, hierarchy, quantization, file roles, payload directories and decoding match
[V7.2-pre](FORMAT_V7_PRE2.md). The common 24-byte header uses revision `3`; index and footer
markers are `OCTVI73P` and `OCTVE73P`. The identity applies to primary/monolithic files,
companions, prepared blocks and checkpoints.

Geometry selection is unchanged from V7.2-pre: raw and delta remain fallback candidates and
compact 24-bit occupancy remains the only occupancy candidate. Attribute dictionary candidates
are searched only for tuple widths 1, 2 and 4. Other widths use raw/LZ4, constant and byte-plane
candidates where valid.

## Status

This is an experimental opt-in identity, not a promoted default; the default output is the final [v7](FORMAT_V7.md) ([ADR 0044](../../adr/0044-v7-final-congelada-y-predeterminada.md)).


The native/async readers, exact checkpoint recovery and Blob/OPFS WASM paths were exercised with
small fixtures. See [integration evidence](../../records/benchmarks/data/storage-v7-v73-integration-2026-10-25.json)
and the [V7 plan](../../records/archive/format-v7-plan.md). Full-corpus conversion/size/latency comparisons
remain pending; attribute-only sampling does not certify this revision's end-to-end performance.
