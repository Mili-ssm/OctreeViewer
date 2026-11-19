# Fase 2b: rango de niveles por nodo y transiciones de un nivel (#44) Implementation Plan

Estado: cerrado · 19-11-2026 · sustituido: su rango por nodo lo ejecutó el [plan del ADR 0055](adr-0055-rango-por-nodo.md) ([registro de cierre](../benchmarks/R3_CIERRE_RANGO_POR_NODO_19-11-2026.md)); fase 2b del [plan maestro](nivel-pedido-y-lod-por-celda.md); replanteada en [replanteamiento](replanteamiento-lod-por-celda-18-11-2026.md) (propuesta pendiente del propietario)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** in cell mode every node keeps active the whole range of depths its cells want, from the one its farthest
cell wants to the one its nearest cell wants, so the GPU draws bands inside every node and no cell is drawn finer
than the px target needs; then, transitions advance one level per frame (ADR 0053) and residency follows D4.

**Why first:** the owner saw it in the demo (16-11-2026): a node near the eye is drawn whole at one fine level —
no bands inside it, and far cells of that node far below the px target ("va directo a por los puntos"). Cause:
phase 2a activates only the closest resident to each octant's wanted depth, measured at the octant's **nearest**
point, so a node whose octants are all near has one active depth and the per-cell rule has nothing to choose from
(plan 2a, E6).

**Spec:** [ADR 0050](../../adr/0050-nivel-por-celda-segun-distancia.md), [ADR 0053](../../adr/0053-transiciones-de-un-nivel.md)
(RF-31), D4 of the [master plan](nivel-pedido-y-lod-por-celda.md), R3-Q11/Q19/Q21 in [requirements](../../requirements.md),
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
- Resident bytes above 512 MiB at 1 px or 460 MiB at ≥ 2 px in Majadas (grazing, inside a leaf) with F1–F2 → stop
  and decide with the owner (fit raising the px target of the whole cloud, R3-Q11, or limiting the tail below spacing).

## Status at pause (16-11-2026, late) — resume here

> **Update 18-11-2026:** step 1 of "Next" (contiguous active set, P2 test) is done on the local branch
> `explore/2b-contiguous-active` (39a9c33), and the black quadrant at P2 is gone in the browser. A rethink of the
> requirements behind this phase, a second experiment and the questions for the owner are in
> [Replanteamiento del LOD por celda](replanteamiento-lod-por-celda-18-11-2026.md). Read it before continuing here.

Branch `feat/cell-lod-2b` (worktree `~/.cache/codex-worktrees/octree-publicar`), HEAD 6da8f79, tree clean, **not
pushed, no PR**. Evidence and agent reports: `.superpowers/sdd/fase-2b/` (git-ignored; `evidence/` holds the
screenshots, the P4 and P2/P3 diagnoses and the browser script `run.js`).

**Done since Task 2:**
- 1f45186 + 51dee2a — the empty quadrant at P4 (close pose, built-in sample). The geometric octant box of a tall
  leaf cube contained the eye, so the probe got +inf and asked for the finest depth: sub-pixel cubes, black region.
  Three rulings:
  - **Data box.** Cell mode probes the octant box ∩ the frame data extent (`SpatialStats::data_bounds`, once per
    node, independent of the stored levels).
  - **Behind and empty octants.** An octant whose data is wholly behind the eye or the LOD near distance, or that is
    empty, inherits the finest wish of its in-front siblings (coarsest if there are none).
  - **D5 floor.** The fit never caps a node below its D5 depth. Before, a cap below it made `d5_wanted` vanish and
    the D5 gate held the children forever.
  - A per-level data box was rejected: it made the wish depend on the build history, and the colour-switch tests
    caught it.
- 41e6f80 — the web host forwards `dataMin`/`dataMax` to the render worker; `beginRemoteCells` rejects malformed
  extents (51dee2a).
- 6da8f79 — the spatial gate also counts pairs with both samples on screen.

**Measured (cell-mode real_sim, Belvedere V7; values at 4e427e2 → at 1f45186):**

| Sim | Node-change jumps | Within-node jumps | Border > 1, total | Border > 1, visible |
| --- | --- | --- | --- | --- |
| orbit s1 | 2993 → 2166 | 0 → 0 | 924/19174 → 2627/17741 | 0/42 → 0/49 |
| orbit s7 | 1115 → 343 | 4911 → 0 | 900/19322 → 1857/12267 | 0/22 → 0/27 |

- 8295 of the 8335 failing border pairs involve an octant behind the eye. Those cells are not drawn, and at 4e427e2
  they "passed" only because they asked for the finest depth.
- **The visible count is weak.** The sim camera's clip range (near 2.3, far 4.6) leaves only ~50 pairs on screen,
  so the browser is the real check.

**Browser at HEAD (built-in sample, cell mode; `evidence/fix-P*.png`):**

| Pose | Result |
| --- | --- |
| P1 | Good: concentric bands |
| P4 | The hole is fixed. A straight horizontal node border remains where large cubes switch to small ones, probably more than one level apart (unmeasured) |
| P2 | The lower-left quadrant is black |
| P3 | 1–2 px dots appear |

**Open defect, root cause proven (`evidence/p23-report.md`):** nodes 14 and 17 contain the eye.
- Their straddling octants want 17–18.
- `range.hi` counts only octants wholly in front (`cell_cut.rs` ~:1056), so `cell_active_depths` (`cell_pool.rs`
  ~:1078) activates `{11, 18}`. Depths 12–17 are resident but inactive.
- The GPU rule jumps 11 → 18, and depth-18 cells measure 0.16–0.28 px.
- In a browser trial, `hi = max(hi, finest drawn)` made the left half as lit as the right.
- The fix is not new design. F1 and ADR 0053 already require a contiguous active set from the far wish up to the
  finest near wish.
- An unfinished implementation is saved as `.superpowers/sdd/fase-2b/contiguous-wip.diff` (`cell_cut.rs` + a
  P2-pose test). It was interrupted, so check it before applying.
- In P3 the gaps between 4–6 px cubes are the sample's own spacing (ground points about 0.13 units apart), which is
  by design (ADR 0054).

**Next, in order:**
1. Finish the contiguous active set and its native P2 test, which must fail first.
2. Re-run the sims and the browser at P1–P5 (`run.js`, after `scripts/build-wasm-bindings.sh`).
3. Measure the P4 straight border; if it is > 1 level, the cross-node chain (F4) comes back.
4. Run the review and `check.sh`.
5. Tick Tasks 1–3 below with their evidence, then open the PR. Merging and publishing need the owner's OK.
6. After the merge, opt `viewer_render_wgpu` and `viewer_wasm` into `[workspace.lints]` (PR #58).

**Not covered here:**
- Octant mode keeps both holes (P4 and P2) and is tracked in [#59](https://github.com/Mili-ssm/OctreeViewer/issues/59).
- With no extent known (`data_bounds == root_bounds`), the cell-mode geometric fallback still asks for the finest
  depth when the eye is inside a node (review finding, deferred).

---

### Task 0: The px target is a minimum, and 0.1 mm length minimum by default ([ADR 0054](../../adr/0054-el-objetivo-de-px-es-el-tamano-minimo.md))

- [x] (7daa1ac, d447b50, 4d1a391, 544bf5c) Refine rule in both modes: a cell refines only if its children would still project to at least the px target and measure at least the length minimum (cubes between N and 2N px; the octant probe and the twin + WGSL share it); default length minimum 0.1 mm in cloud units (metres until the CRS unit is read, phase 3), applied by the driver unless the API sets another; goldens updated with the reason; GPU partition test and sims green in both modes; browser screenshots at the owner's poses.

### Task 1: The depth range per node (F1–F3)

- [x] (4083d8c) **1a** compute and expose the range (`CellCutEntry.range`), no behaviour change; tests: far ≤ near, a node straddling the eye plane, the eye inside the node, an octant behind the eye.
- [x] (4083d8c; browser: graded cubes in nodes with residents, a near node without them still drew its finest level) **1b** activate every already-resident depth inside the range (cell mode); browser screenshot of the owner's pose (kill criterion: no bands → stop).
- [x] (462196a) **1c** fetch stored depths inside the range.
- [x] (b17e61d; contiguous fill also below spacing ae5a254; browser measurements of queue and heap still pending) **1d** build missing depths on demand down to the spacing/separation depth only; the fit counts requested depths; measure the uncapped need, `levels.bin` coverage, request queue and build-worker heap in the browser.
- [ ] (known case 18-11-2026: at P4 node 14 draws depth 11 at 3.2–4.0 px (target 4 px) because its range starts at 11 while its visible part needs 10 (`node_far_depth` probes an off-screen corner where planar perspective stretches the cells), [record](../benchmarks/R3_FASE2B_BORDE_P4_18-11-2026.md); belongs to the per-node range step of ADR 0055) Test: no drawn cell finer than its parent cell needed (`refines()` of the parent), exempting the buildable minimum, caps and the span floor; the sims' `finer_than_wanted` counter drops.

### Task 2: Node hand-offs within one level (re-planned 16-11-2026 from measurements)

Measured (cell-mode sims on Belvedere V7, per-frame gate of Task 3a): 96 % of jumps > 1 level happen at node hand-offs. Of those, 78.8 % because a leaf child cannot draw coarser than its spacing depth (its buildable range starts there, `nd + 9..12`, while the internal parent stops at `nd + 8`); 17.5 % because the parent is at its finest depth when the child arrives; 0 % from the parent's sample being sparse. The cross-node chain (former F4) is not needed for these and is kept only if the border measurement below still fails.

- [x] (dd37e9b) **2a (B)** leaves build cell levels from `rep = node_depth + 3` like internal nodes (their coarse levels aggregate many points; the spacing depth stays the start of the single-point tail), in both modes; goldens updated with the reason.
- [x] (4e427e2) **2b (D)** a node entering the cut seeds its active window at its drawing ancestor's finest active depth + 1 (and a node leaving hands back one level per frame), then widens one level per frame.
- [x] (gate 7556ddc, visible count 6da8f79; numbers in "Status at pause"; P4 straight border measured 18-11-2026: 0 visible pairs over one level, so no cross-node chain — [record](../benchmarks/R3_FASE2B_BORDE_P4_18-11-2026.md)) **2c** measure: the temporal gate's node-change jumps (baseline: dolly 13 066, orbit 17 300–19 500) and a new spatial gate — neighbouring lattice samples across a node border differ by ≤ 1 level; if the spatial gate still fails, plan the cross-node chain.

### Task 3: One level per frame (ADR 0053, temporal) — 3a (stepping rule) moved ahead of Task 2 because the closest-resident jump blocks 1d

- [x] (65e93e1 … 0e8ecd7; temporal gate asserted 0 in octant mode) **3a** per (node, octant) the drawn depth steps one level toward the wanted one through resident depths, never past it; the adjacent depth is requested first; in cell mode the active span moves at most one level per frame at each end (per-point stepping approximated per node).

Known case (16-11-2026, task 0a): with the closest-resident rule (R3-Q21) a node drawing depth 10 that wants 8 jumps to 7 when only {7, 10} are resident (seeded dolly simulations at their original px, which task 0a halved to 1.0 / 2.0). With ADR 0053 and 0054 the transition must step from the drawn depth towards the wanted one, one level per frame, using a resident intermediate; restore `SYNTHETIC_CASE_PX` = 2.0 and the glue literals (2.0, 4.0) as this task's regression test.

### Task 4: Residency D4 and coarse levels derived from children

### Task 5: The tail below spacing (ADR 0053, pending review) — measure, then decide with the owner
