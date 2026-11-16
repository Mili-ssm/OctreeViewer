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

## Design decisions (agent, 16-11-2026; pre-mortem applied)

| # | Decision | Why |
| --- | --- | --- |
| F1 | Each node gets a **far** wanted depth besides the per-octant near ones: the same probe (actual projected edge) at the node corner with the largest depth along `forward`; octants entirely behind the eye contribute nothing; `far = min(far, near)`. The near side is bounded by the near plane (D4). Range = `[far, max near]`, clamped to buildable, caps and span, with `lo ≤ hi` | The coarsest depth any cell wants is at the node's farthest point; a Euclidean farthest corner behind the eye would project to +inf and want the tail (the #57 OOM path) |
| F2 | The range lives in a cell-mode-only field of `CellCutEntry` (`range: Option<(u8, u8)>`); `wanted`/`drawn`/`stage` keep their per-octant near-depth meaning (holds, stage counts, digests and oracles unchanged) | Squeezing a range into the 8 octant slots breaks the R3-Q21 hold and the oracles |
| F3 | Built step by step: (1b) activate every **already resident** depth in the range — no builds, no new bytes; (1c) fetch stored depths in the range; (1d) build missing ones on demand, but never below the node's spacing/separation depth (the tail stays as today until Task 4); the fit counts requested depths at their estimated size | The cheapest step may already give the bands; the tail measured ~531 MiB in phase 0a |
| F4 | **Cross-node chain instead of the per-node mask** (Task 2): a child node's cell treats its ancestor node's active depths as its coarser ancestors in the E4 rule, and the ancestor's cells yield only where a finer depth the child holds is wanted; no whole-footprint masking | The owner (16-11-2026) sees straight node borders where a child replaces its parent wholesale; ADR 0050 forbids whole octants or nodes changing at once |
| F5 | Octant mode unchanged | Default of the library |

## Global Constraints

As in phase 2a (English code/commits, Spanish docs, no AI attribution; octant mode byte-for-byte unchanged; checks
`cargo test -p <crate> --profile fast --offline --lib`, clippy, `scripts/check.sh` with the real input before merge).

## Kill criteria

- Uncapped need above 460 MiB at ≥ 2 px (Majadas inside a leaf), or the fit coarsening nodes where phase 2a did not → stop 1d, keep 1b/1c.
- Any node with `far > near`, or a range wider than ~6 for a node that does not contain the eye → fix the probe first.
- After 1b the owner's pose shows no bands → the diagnosis is wrong; re-plan with the owner before 1c/1d.
- Request queue not draining within ~2 s at the owner's pose, build-worker heap over the CPU budget, or frame p95 over twice phase 2a while loading → stop on-demand range builds.
- Task 2 GPU partition test (parent + child pairs) finds a double or a hole away from the threshold → stop and re-plan.

--- | --- | --- |
| F1 | Each node gets a **far** wanted depth besides the per-octant near ones: the same probe (actual projected edge) at the node corner with the largest depth along `forward`; octants entirely behind the eye contribute nothing; `far = min(far, near)`. The near side is bounded by the near plane (D4). Range = `[far, max near]`, clamped to buildable, caps and span, with `lo ≤ hi` | The coarsest depth any cell wants is at the node's farthest point; a Euclidean farthest corner behind the eye would project to +inf and want the tail (the #57 OOM path) |
| F2 | The range lives in a cell-mode-only field of `CellCutEntry` (`range: Option<(u8, u8)>`); `wanted`/`drawn`/`stage` keep their per-octant near-depth meaning (holds, stage counts, digests and oracles unchanged) | Squeezing a range into the 8 octant slots breaks the R3-Q21 hold and the oracles |
| F3 | Built step by step: (1b) activate every **already resident** depth in the range — no builds, no new bytes; (1c) fetch stored depths in the range; (1d) build missing ones on demand, but never below the node's spacing/separation depth (the tail stays as today until Task 4); the fit counts requested depths at their estimated size | The cheapest step may already give the bands; the tail measured ~531 MiB in phase 0a |
| F4 | **Cross-node chain instead of the per-node mask** (Task 2): a child node's cell treats its ancestor node's active depths as its coarser ancestors in the E4 rule, and the ancestor's cells yield only where a finer depth the child holds is wanted; no whole-footprint masking | The owner (16-11-2026) sees straight node borders where a child replaces its parent wholesale; ADR 0050 forbids whole octants or nodes changing at once |
| F5 | Octant mode unchanged | Default of the library |

## Global Constraints

As in phase 2a (English code/commits, Spanish docs, no AI attribution; octant mode byte-for-byte unchanged; checks
`cargo test -p <crate> --profile fast --offline --lib`, clippy, `scripts/check.sh` with the real input before merge).

## Kill criteria

- Resident bytes above 512 MiB at 1 px or 460 MiB at ≥ 2 px in Majadas (grazing, inside a leaf) with F1–F2 → stop
  and decide with the owner (fit raising the px target of the whole cloud, R3-Q11, or limiting the tail below spacing).

---

### Task 0: The px target is a minimum, and 0.1 mm length minimum by default ([ADR 0054](../adr/0054-el-objetivo-de-px-es-el-tamano-minimo.md))

- [ ] Refine rule in both modes: a cell refines only if its children would still project to at least the px target and measure at least the length minimum (cubes between N and 2N px; the octant probe and the twin + WGSL share it); default length minimum 0.1 mm in cloud units (metres until the CRS unit is read, phase 3), applied by the driver unless the API sets another; goldens updated with the reason; GPU partition test and sims green in both modes; browser screenshots at the owner's poses.

### Task 1: The depth range per node (F1–F3)

- [ ] **1a** compute and expose the range (`CellCutEntry.range`), no behaviour change; tests: far ≤ near, a node straddling the eye plane, the eye inside the node, an octant behind the eye.
- [ ] **1b** activate every already-resident depth inside the range (cell mode); browser screenshot of the owner's pose (kill criterion: no bands → stop).
- [ ] **1c** fetch stored depths inside the range.
- [ ] **1d** build missing depths on demand down to the spacing/separation depth only; the fit counts requested depths; measure the uncapped need, `levels.bin` coverage, request queue and build-worker heap in the browser.
- [ ] Test: no drawn cell finer than its parent cell needed (`refines()` of the parent), exempting the buildable minimum, caps and the span floor; the sims' `finer_than_wanted` counter drops.

### Task 2: Cross-node chain instead of the per-node mask (F4) — to detail after Task 1 (node record links to its ancestor's record; GPU partition test over parent + child pairs)

### Task 3: One level per frame (ADR 0053, temporal) — to detail after Task 2

Known case (16-11-2026, task 0a): with the closest-resident rule (R3-Q21) a node drawing depth 10 that wants 8 jumps to 7 when only {7, 10} are resident (seeded dolly simulations at their original px, which task 0a halved to 1.0 / 2.0). With ADR 0053 and 0054 the transition must step from the drawn depth towards the wanted one, one level per frame, using a resident intermediate; restore `SYNTHETIC_CASE_PX` = 2.0 and the glue literals (2.0, 4.0) as this task's regression test.

### Task 4: Residency D4 and coarse levels derived from children

### Task 5: The tail below spacing (ADR 0053, pending review) — measure, then decide with the owner
