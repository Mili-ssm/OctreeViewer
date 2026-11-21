# Native `.oct` format, v7.2-pre

> Histórico: formato retirado con la V8 (D4, 21-11-2026); la referencia vigente es [FORMAT_V8](FORMAT_V8.md).

Development identity: major **7**, revision **2**, selected as `v7.2-pre`. Final V7 is the default ([ADR 0044](../../adr/0044-v7-final-congelada-y-predeterminada.md)); other
unimplemented revisions remain rejected. This revision tests a faster writer candidate search;
V7.1-pre remains readable and its writer behavior is unchanged.

## Binary contract

Metadata, hierarchy, quantization, file roles, payload directories and decoding are shared with
[V7.1-pre](FORMAT_V7_PRE1.md). The exact identity uses the common 24-byte header plus these
revision-specific markers:

| Field | V7.2-pre |
| --- | --- |
| Header magic | `OCTVFRM1` |
| Major / revision | `7` / `2` |
| Index magic | `OCTVI72P` |
| Footer magic | `OCTVE72P` |

The writer keeps the existing raw/delta fallback and evaluates only 24-bit compact occupancy.
It does not search 28-bit or legacy occupancy candidates. Compact occupancy stores the same
quantized coordinates and source row order; the candidate is used only when its compressed
payload is smaller than the fallback and fits the block limit. This changes encoding selection,
not decoded values or quantization semantics. Readers retain the V7.1-pre decoder set.

V7.2-pre files, prepared blocks, companions and checkpoints have their own exact identity. A
checkpoint cannot resume under V7.1-pre or vice versa. EPT block recovery is supported for both
revisions and keeps the chosen version in its manifest.

## Status

This is a performance experiment, not a promoted default. Majadas and Albuera profiles, logical
checksums, determinism, and recovery checks are recorded in the [V7 plan](../../records/archive/format-v7-plan.md).
Conversion time is still being optimized before any promotion decision.
