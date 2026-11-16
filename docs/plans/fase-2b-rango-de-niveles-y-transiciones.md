# Fase 2b: rango de niveles por nodo y transiciones de un nivel (#44) Implementation Plan

Estado: en curso · 16-11-2026 · fase 2b del [plan maestro](nivel-pedido-y-lod-por-celda.md)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** in cell mode every node keeps active the whole range of depths its cells want, from the one its farthest
cell wants to the one its nearest cell wants, so the GPU draws bands inside every node and no cell is drawn finer
than the px target needs; then, transitions advance one level per frame (ADR 0053) and residency follows D4.

**Why first:** the owner saw it in the demo (16-11-2026): a node near the eye is drawn whole at one fine level —
no bands inside it, and far cells of that node far below the px target ("va directo a por los puntos"). Cause:
phase 2a activates only the closest resident to each octant's wanted depth, measured at the octant's **nearest**
point, so a node whose octants are all near has one active depth and the per-cell rule has nothing to choose from
(plan 2a, E6).

**Spec:** [ADR 0050](../adr/0050-nivel-por-celda-segun-distancia.md), [ADR 0053](../adr/0053-transiciones-de-un-nivel.md)
(RF-31), D4 of the [master plan](nivel-pedido-y-lod-por-celda.md), R3-Q11/Q19/Q21 in [requirements](../requirements.md),
design E1–E8 of [phase 2a](fase-2a-lod-por-celda-en-gpu.md).

## Design decisions (agent, 16-11-2026)

| # | Decision | Why |
| --- | --- | --- |
| F1 | Each octant gets a **far** wanted depth besides the near one: the same probe (`OctantProbe`, actual projected edge) with the cube placed at the octant's point **farthest** from the eye. The node's wanted range is `[min far, max near]` over its non-masked octants, clamped to its buildable range and the cell span | The coarsest depth any cell of the node can want is the one at its farthest point; the finest, at its nearest |
| F2 | Cell mode wants **every** depth of the range: missing ones are requested (built on demand, phase 1), coarse to fine; the active set is the closest resident (finer on a tie) to each depth of the range | Contiguous active depths: bands inside the node and neighbours ≤ 1 level apart (ADR 0053, spatial) |
| F3 | Octant mode unchanged | Default of the library |

## Global Constraints

As in phase 2a (English code/commits, Spanish docs, no AI attribution; octant mode byte-for-byte unchanged; checks
`cargo test -p <crate> --profile fast --offline --lib`, clippy, `scripts/check.sh` with the real input before merge).

## Kill criteria

- Resident bytes above 512 MiB at 1 px or 460 MiB at ≥ 2 px in Majadas (grazing, inside a leaf) with F1–F2 → stop
  and decide with the owner (fit raising the px target of the whole cloud, R3-Q11, or limiting the tail below spacing).

---

### Task 1: The wanted range per node (F1, F2)

**Files:** `crates/viewer_render_wgpu/src/cell_cut.rs` (far probe, range), `cell_pool.rs` (cached far depths next to
`NodeDepths.depths`, requests and active set over the range in cell mode), tests; simulations in `crates/viewer_wasm`
must keep passing in both modes.

- [ ] **Step 1: failing tests:** `the_far_probe_wants_coarser_than_the_near_one` (an octant spanning near and far
  distances); `cell_mode_wants_the_whole_range_of_a_near_node` (a node whose 8 octants are all near: its active set
  covers `[min far, max near]`, the missing depths are requested coarse to fine); `cell_mode_never_draws_a_far_cell_finer_than_needed`
  (twin oracle: with the range resident, no drawn cell's projected edge is below target/2 unless it is the finest
  buildable depth or a masked/held case).
- [ ] **Step 2:** implement; octant mode untouched.
- [ ] **Step 3:** measure resident bytes and instances in the native bench (`cell_pool_bench --cell-lod`) on Majadas
  and Belvedere at 1/4 px (grazing, inside a leaf) against the kill criteria; browser screenshots of the owner's pose
  (near node) in cell mode before/after.
- [ ] **Step 4:** commit `feat(render): want the whole depth range of a node in cell mode`.

### Task 2: One level per frame (ADR 0053, temporal) — to detail after Task 1

### Task 3: Residency D4 and coarse levels derived from children — to detail after Task 1

### Task 4: The tail below spacing (ADR 0053, pending review) — measure, then decide with the owner
