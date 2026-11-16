# Fase 2a: selección por celda en la GPU (#44) Implementation Plan

Estado: en curso · 16-11-2026 · fase 2a del [plan maestro](nivel-pedido-y-lod-por-celda.md) (pre-mortem aplicado)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** behind an option `cellLod: octant|cell` (default `octant`), the GPU decides per cell which of a node's
active levels draws each point, so level boundaries follow the distance of each cell (bands around the eye) instead of
octant borders; one Rust twin of the decision serves the cut, picking, simulations and tests.

**Architecture:** the CPU keeps choosing *which levels are active* per node (in cell mode: the closest resident to
each depth the octants want, measured with the same metric as the GPU); the slot pass draws a cell at depth `d` iff
it is not masked, it does not refine or no finer level is active, and its ancestors at all coarser active depths
refine (E4). The refine test uses integer cell coordinates relative to the node corner, so a cell and its children
evaluate the ancestor identically, and the E4 rule makes "exactly one drawn cell per point" hold by
construction, with or without a monotone metric. The CPU half (twin, measurement, cut, simulations) lands before any GPU change. Residency (D4) and the
per-node wanted set are phase 2b.

**Tech Stack:** Rust (`viewer_render_wgpu`, `viewer_wasm`), WGSL in Rust string constants, wgpu headless tests,
wasm-bindgen, JS module workers (`examples/web-host`), `node --test`.

**Spec:** [ADR 0050](../adr/0050-nivel-por-celda-segun-distancia.md) (requirement), decisions D2, D3 and D5 of the
[master plan](nivel-pedido-y-lod-por-celda.md), R3-Q21 and RQ-09 in [requirements](../requirements.md), phase 0a
risk in [R3_CELL_LOD_0A_15-11-2026](../records/benchmarks/R3_CELL_LOD_0A_15-11-2026.md).

## Design decisions (agent, 16-11-2026)

| # | Decision | Why |
| --- | --- | --- |
| E1 | **Metric (measured in Task 2, [record](../records/benchmarks/R3_FASE2A_METRICA_16-11-2026.md)):** `edge_px(c)` = the actual projected edge, as `projected_voxel_edge_pixels`: for the 8 eye-relative corners `o`, `d = dot(o, fwd)`; any `d ≤ 0` → `+inf`; else pixel `focal · (dot(o, right) / d, dot(o, up) / d)`; the result is the largest 2D length of the 12 cube edges. `refines(c)` = `side > min_side` ∧ `edge_px(c) > target_px` | Task 2: fewest instances (0.85–0.99× `octant_tight` inside Majadas), ≤ 0.006 % cells above target, 0 outside; the bound `side·focal·dmax/max(zmin, near)²` failed (0.596 %) because clamping to `near` under-refines near the eye. It is not monotone by proof (0 non-monotone pairs measured), so E4 does not rely on monotonicity |
| E2 | **Integer coordinates:** relative to the node corner at each depth, `rel = abs − (node_lo << (d − node_depth))`, a `u32` per axis; ancestor at `d_prev` = `rel >> (d − d_prev)`. Cell mode only activates `node_depth ≤ d ≤ node_depth + 31` (the CPU clamps; a 2 km node stops at ~1 µm) | D2 of the master plan; the anchor depends only on the node bounds, so it never changes after insertion; no cell larger than its node draws over its siblings |
| E3 | **Position for the metric:** `lo = node_anchor + f32(rel) · side`, the anchor being the node corner eye-relative in f32 (rewritten every frame the eye moves, like page origins); the ancestor side is `side · 2^(d − d_prev)` (exact). Drawing positions keep using the page origin | Same integer → same float → same decision for a cell and for its children |
| E4 | **Draw rule** (active depths `A` of the node as a `u64` bitmask): draw `c` at `d` iff ¬`masked_any(c)` ∧ (no active depth above `d` ∨ ¬`refines(c)`) ∧ for **every** active depth `a < d`: `refines'(anc_a(c))`, with `refines'(x)` = `refines(x)` ∨ `masked_any(x)`. Evaluate ancestors from the coarsest, stopping at the first that does not refine | Along the chain of a point the drawn cell is the first active depth that does not refine: exactly one by construction, with or without a monotone metric; cost ≤ `|A|` evaluations per cell |
| E5 | **`masked_any`** (D5): depth `≥ rep` (= `node_depth + 3`): the one 8³ bit; `node_depth ≤ d < rep`: one bit of a precomputed mip (4³ as `u64`, 2³ as 8 bits, 1³ as 1 bit) | Today a coarse cell tests only the bit at its minimum corner and paints over resident children |
| E6 | **Active set in cell mode:** `A` = { closest resident (finer on a tie) to `w` : `w` ∈ octant wanted depths }, clamped to E2's range, with no "spanning level forces one depth" rule. The octant wanted depth keeps `projected_voxel_edge_pixels` at the octant's nearest point (ADR 0034), the same metric as E1, so the CPU and the GPU agree | Closest resident: owner, R3-Q21, 16-11-2026; one metric on both sides. This set can still have gaps (e.g. `{6, 13}`); the one-level rule of [ADR 0053](../adr/0053-transiciones-de-un-nivel.md) closes them in phase 2b |
| E7 | **D5 gate:** a node's mask bits are applied only while it has an active depth `≥ rep`; until then its masking children are held (they do not draw, as `eval_hold` does) and that depth is requested first | Without it a partially masked coarse cell leaves a hole (pre-mortem); octant mode overdrew instead |
| E8 | **GPU layout** (the compute stage already uses the 8 storage buffers of the WebGPU minimum, `cell_render.rs:60-65`): page entry 32 → 48 B (words 8–10 `rel` of the page minimum, word 11 `depth`); mask slot 16 → 32 words, now a *node record* (0–15 mask; 16–18 anchor f32; 19 `node_depth`; 20–21 `A`; 22–23 4³ mip; 24 2³ mip and 1³ `<< 8`; 25–31 reserved); frame uniform 128 → 176 B (+ `forward`, `right`, `up`, `focal`, `target_px`, `min_side`, `lod_mode`, `counters`); per-depth counters appended to the `draw_args` buffer | No new binding |

## Global Constraints

- Code, comments and commits in English; docs in Spanish; dates `dd-mm-aaaa`. No AI attribution in commits, PRs or issues.
- `FormatVersion::V<n>` literals only inside `viewer_format` (`scripts/check_version_literals.sh`).
- Single tests: `cargo test -p <crate> --profile fast --offline --lib <path>`; GPU tests run through `for_each_adapter` (software fallback included); JS: `node --test examples/web-host/tests/<file>.test.mjs`. Before merge: `scripts/check.sh` with `FORMAT_PARITY_REAL_INPUT=/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets/belv_2009_digital-aerial_pcd_copc.laz`.
- Default mode stays `octant`: every existing test, golden and probe keeps passing unchanged in `octant` mode.
- The compute pass keeps ≤ 8 storage buffers (`slot_pass_stays_within_eight_storage_buffers`).
- One WGSL function `cell_refines` evaluates both the cell and its ancestor; its Rust twin `cell_lod::refines` uses the same f32 operations in the same order (`mul_add` only where the WGSL uses `fma`).
- R3-Q21: a cell whose finer level is not active keeps drawing; RQ-09 measured with the actual edge (`projected_voxel_edge_pixels`): ≤ 0.5 % above target inside the dataset, 0 outside (ADR 0034).
- No new dependencies. Datasets for measurements: `/home/mili/.cache/codex-worktrees/octree-r3-v75/target/render-data/{belvedere-v7,majadas-v7}`.

## Review Focus

1. Eye inside a cell, or a cell with a corner behind the eye: `edge_px = +inf`, it refines (unless `side ≤ min_side`), never NaN (Task 1 `a_cell_around_the_eye_refines`).
2. Children of one ancestor split across two pages agree on the ancestor's decision (Task 7 partition test with a 2-page level).
3. A child node entering under a parent with no resident depth `≥ rep`: no hole and no overlap (Task 3 `children_wait_for_the_parent_d5_level`, Task 4 sims).
4. Switching `cellLod` at runtime with levels resident: the next frame is valid in the new mode (Task 10 `switching_mode_rewrites_the_page_table`).
5. Active depths non-contiguous (`{6, 9, 13}` relative), a gap ≥ 16 and the span top 31; an anchor far from the eye (`‖anchor‖ = 1e4`, `d − node_depth = 29`) (Task 1).

## Kill criteria

- Task 2: with the chosen metric, Majadas grazing > 2.5× the CPU path at 1 px or > 2× at 2–8 px, or above-target by actual edge > 0.5 % inside the dataset → stop phase 2 and re-plan the metric with the owner.
- Task 1 partition fails on a case away from the threshold → E4/E5 are wrong; re-plan.
- Task 7 GPU partition finds a double or a hole away from the threshold on any adapter that ran → master-plan kill: move to the 512-cell grid.
- Task 4 cell-mode sims show holes outside lost zones after E7 → stop and pull the needed part of D4 into this phase.
- A subagent hits its turn limit → split the task before retrying. Phase 2a past ~5 days of work → cut scope.

---

### Task 1: Rust twin of the per-cell decision

**Files:**
- Create: `crates/viewer_render_wgpu/src/cell_lod.rs`, `crates/viewer_render_wgpu/src/cell_lod_tests.rs`
- Modify: `crates/viewer_render_wgpu/src/lib.rs` (module)

**Interfaces:**
- Produces:
  - `pub struct LodView { pub forward: [f32; 3], pub focal: f32, pub near: f32, pub target_px: f32, pub min_side: f32 }` (scaled render units, eye at the origin) and `LodView::from_camera(camera: &OrbitCamera, viewport: [f32; 2], target_px: f32, min_side_scaled: f32) -> LodView`.
  - `pub struct NodeLod { pub anchor: [f32; 3], pub node_depth: u8, pub active: u64, pub mask: [u64; 8] }` with `pub fn mips(&self) -> (u64, u8, bool)`; bit `d − node_depth` of `active` = depth `d`.
  - `pub fn edge_bound(view: &LodView, lo: [f32; 3], side: f32) -> f32`; `pub fn refines(view: &LodView, lo: [f32; 3], side: f32) -> bool` (E1).
  - `pub fn cell_lo(node: &NodeLod, rel: [u32; 3], side: f32) -> [f32; 3]` (E3).
  - `pub fn masked_any(node: &NodeLod, depth: u8, rel: [u32; 3]) -> bool` (E5).
  - `pub fn draws(view: &LodView, node: &NodeLod, depth: u8, rel: [u32; 3], side: f32) -> bool` (E4).
  - `pub fn drawn_depth_at(view: &LodView, node: &NodeLod, point: [f64; 3], node_lo: [f64; 3], side_at_node_depth: f64) -> Option<u8>` (oracle: the depth whose cell draws a point given in node-local scaled units).
  - `pub const MAX_NODE_SPAN: u8 = 31`.

- [x] **Step 1: Write the failing tests** in `cell_lod_tests.rs`:
  - `the_bound_covers_the_projected_edge`: 10 000 seeded cubes with `zmin ≥ near` under random cameras; `edge_bound ≥ projected_voxel_edge_pixels` of the same cube (same frame via `ViewProjection`) with relative slack 1e-5.
  - `refines_is_monotone_from_child_to_parent`: random chains, including `‖anchor‖ = 1e4` with `d − node_depth = 29`.
  - `a_cell_around_the_eye_refines`: eye inside the cube, and cube crossing `near` → `true`, finite bound, never NaN.
  - `min_side_stops_refinement`: `side ≤ min_side` → `false` at any distance.
  - `exactly_one_cell_draws_each_point`: ≥ 1000 seeded poses × scenes with ≥ 3 active depths, non-contiguous depths, a gap ≥ 16, a partially masked node, the eye inside a cell, and band borders (target within 1e-3 of `edge_bound`); every sample point outside set mask bits is drawn by exactly one active depth, points under a set bit by none.
  - `masked_any_matches_brute_force` for every depth from `node_depth` to `node_depth + 5`.
- [x] **Step 2:** `cargo test -p viewer_render_wgpu --profile fast --offline --lib cell_lod` → FAIL (module missing).
- [x] **Step 3:** implement.
- [x] **Step 4:** same command → PASS; `cargo clippy -p viewer_render_wgpu --offline --all-targets` clean.
- [x] **Step 5:** commit `feat(render): add the per-cell LOD decision and its tests`.

### Task 2: Measure the metric on Majadas and Belvedere (gate)

**Files:**
- Modify: `crates/viewer_render_wgpu/examples/coverage_spike.rs` (bring the `--cell-lod` code of branch `explore/cell-lod-spike`, commits `9e0ec17` and `5f8ba56`, and add `--metric bound|edge|distance`, where `bound` calls `cell_lod::refines`, `edge` refines while `projected_voxel_edge_pixels > target`, `distance` is the phase 0a rule)
- Create: `docs/records/benchmarks/R3_FASE2A_METRICA_<dd-mm-aaaa>.md` and its data folder

- [x] **Step 1:** run `--cell-lod` for each metric on both datasets, poses grazing, orbit-0 and inside-leaf, 1/2/4/8 px, residency `r2`; report instances vs CPU path and vs `octant_tight`, above-target cells by actual edge, and for `edge` the number of child-refines-but-parent-does-not pairs (non-monotone cases).
- [x] **Step 2:** write the record (facts, inferences) and a verdict: the metric that passes the kill criteria with the fewest instances; `edge` qualifies only with 0 non-monotone pairs. If none passes → stop (kill criteria).
- [x] **Step 3:** if the verdict is not `bound`, amend E1 and Task 1's twin in a follow-up commit before Task 3.
- [x] **Step 4:** commit `docs: measure the per-cell LOD metric (phase 2a gate)`.
- [x] **Amendment (verdict `edge`, 16-11-2026):** the twin of Task 1 switches to E1 `edge_px` (`LodView` gets `right` and `up`, loses `near`; `edge_bound` becomes `edge_px`) and to the E4 all-ancestors rule; `the_bound_covers_the_projected_edge` becomes `edge_px_matches_the_camera` (relative 1e-4 against `projected_voxel_edge_pixels` in f64) and the partition test runs unchanged. Commit `feat(render): use the actual projected edge in the per-cell LOD twin`.

### Task 3: The cut and the pool in cell mode (CPU only)

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_cut.rs` (octant wanted with the twin metric in cell mode; `drawn_levels` without single-depth forcing in cell mode), `cell_pool.rs` (`LodMode`, per-node active set, E7 gate and hold, `MaskUpdate` gains `active: u64` and `node_depth`)
- Test: `cell_cut_tests.rs`, `cell_pool_tests.rs`

**Interfaces:**
- Consumes: Task 1 twin (and Task 2's verdict).
- Produces: `pub enum LodMode { Octant, Cell }` (default `Octant`); `CellPool::set_lod_mode(&mut self, mode: LodMode)`; in cell mode `FramePlan.active` holds one `ActiveLevel` per (node, active depth) with `octant_bits = 0xff`, and `MaskUpdate { slot, mask, active, node_depth }` describes every applied node; `CellCutEntry.drawn` keeps meaning "per-octant closest resident" for stats; `cell_cut_digest` unchanged in octant mode.

- [x] **Step 1: Write the failing tests:** `cell_mode_activates_the_closest_resident_of_each_wanted_depth`; `cell_mode_clamps_active_depths_to_the_node_span`; `cell_mode_never_forces_one_depth_for_a_spanning_level`; `children_wait_for_the_parent_d5_level` (E7: mask empty and children held until a depth `≥ rep` is resident, and that depth is requested first); `cell_mode_wants_with_the_twin_metric`.
- [x] **Step 2:** `--lib cell_pool` and `--lib cell_cut` → FAIL.
- [x] **Step 3:** implement; octant mode untouched (goldens unchanged).
- [x] **Step 4:** all `viewer_render_wgpu` lib tests → PASS; clippy clean.
- [x] **Step 5:** commit `feat(render): plan cell mode in the cut and the pool`.

### Task 4: Streaming simulations in cell mode (CPU only)

**Files:**
- Modify: `crates/viewer_wasm/src/lod_cells_stream_tests.rs` (`Harness` mode, cut-level sample oracle) — subtask 4a; `lod_cells_sim.rs`, `lod_cells_glue_sim.rs` (mode in the digest at rest) — subtask 4b
- Test: same files

**Interfaces:**
- Consumes: `cell_lod::drawn_depth_at` (Task 1), `LodMode` (Task 3).
- Produces: in cell mode the oracle samples a 7³ lattice over the *whole cut* (all nodes, so overlap between nodes is visible) and gets each sample's drawn depth from the twin over the plan's active sets and masks. Gates: no sample drawn twice, no hole outside lost zones, R3-Q21 per sample with `wanted` = the twin's per-cell wanted depth (first depth of the node span whose cell at the sample does not refine, capped by the buildable range) (`now ≥ min(before, wanted)`, existing excuses for counted stages), `staged_coarsenings == 0` unexcused. Depths the bound wants but the cut does not activate are a reported counter, not a failure. Octant mode keeps its oracle.

- [x] **Step 1: Write the failing tests:** the existing streaming scenarios and the 18 seeded sims parametrised by mode; `cell_mode_bands_follow_the_distance` (straight zoom over a flat node: the drawn depth does not increase with sample distance within a node).
- [x] **Step 2:** `cargo test -p viewer_wasm --profile fast --offline --lib lod_cells` → FAIL.
- [x] **Step 3:** implement; a real violation is a defect of Tasks 1–3: stop and report it, never excuse it in the oracle.
- [x] **Step 4:** same → PASS in both modes.
- [x] **Step 5:** commit `test(wasm): run the streaming simulations in cell mode`.

### Task 5: Page entry and frame uniform layout (no behaviour change)

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_pages.rs` (`PageEntry`, `PageInfo::entry`, `CELL_PAGES_WGSL`), `cell_render.rs` (`FrameUniform`/`CellFrame`, `PAGE_BYTES`, `encode_cells`), `cell_driver.rs` (passes `rel_origin`)
- Test: `cell_codec_tests.rs`, `cell_render_tests.rs`

**Interfaces:**
- Produces: `PageEntry(pub [u32; 12])`; `PageInfo::entry(&self, frame, mask_slot, octant_bits, rel_origin: [u32; 3]) -> PageEntry` (`rel_origin` = page minimum relative to the node corner at the page depth, E2); `PAGE_BYTES = 48`; `FrameUniform` gains `forward`, `right`, `up: [f32; 3]`, `focal`, `target_px`, `min_side`, `lod_mode: u32` (0 octant, 1 cell), `counters: u32` (0 off), 176 B.

- [x] **Step 1: Write the failing tests:** `page_entry_round_trips_the_relative_origin_and_depth`; `frame_uniform_is_176_bytes_and_matches_the_wgsl_layout`.
- [x] **Step 2:** `--lib cell_codec`, `--lib cell_render` → FAIL.
- [x] **Step 3:** implement; `slot_state` does not use the new fields yet.
- [x] **Step 4:** all lib tests of both crates → PASS unchanged; the storage-buffer test passes.
- [x] **Step 5:** commit `refactor(render): widen page entries and the frame uniform for per-cell LOD`.

### Task 6: Node records (no behaviour change)

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_render.rs` (`MASK_WORDS` → `NODE_WORDS = 32`, buffer size, `mask_mirror`), `cell_driver.rs` (`GpuTarget::set_node_mask` → `set_node_record`, anchors rewritten when the eye moves), `cell_lod.rs` (`node_record`), `cell_pick.rs` (mask read from the record)
- Test: `cell_codec_tests.rs`, `cell_driver_tests.rs`, `cell_pick_tests.rs`

**Interfaces:**
- Consumes: `MaskUpdate { slot, mask, active, node_depth }` (Task 3).
- Produces: `pub fn node_record(node: &NodeLod) -> [u32; NODE_WORDS]` (E8 word order); `GpuTarget::set_node_record(&mut self, slot: u32, record: &[u32; NODE_WORDS])`.

- [x] **Step 1: Write the failing tests:** `node_record_round_trips` (all E8 words, mips equal `NodeLod::mips`); `anchors_follow_the_eye` (driver test: a camera move rewrites the anchor words).
- [x] **Step 2:** `--lib cell_codec`, `--lib cell_driver` → FAIL.
- [x] **Step 3:** implement.
- [x] **Step 4:** all lib tests → PASS unchanged.
- [x] **Step 5:** commit `refactor(render): store a per-node record next to the mask`.

### Task 7: Per-cell decision in the slot pass and GPU partition test

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_render.rs` (`cell_refines`, `cell_draws` in WGSL; `slot_state` uses `cell_draws` when `lod_mode == 1`, before the phase split; `visible` gets `COPY_SRC`)
- Test: `crates/viewer_render_wgpu/src/cell_render_tests.rs` (scene builder with per-level `rel` origins, node records, a level split into 2 pages)

- [x] **Step 1: Write the failing tests:**
  - `cell_mode_draws_one_cell_per_point_on_every_adapter`: read back `visible`; 200 seeded poses by default and 1000 with `CELL_LOD_POSES=1000`, over the Task 1 scene kinds plus a 2-page level, the eye inside a cell, and 1-ULP threshold poses (bisect `target_px`); occlusion off → each sample point inside the frustum (2 % viewport margin) is covered by exactly one drawn cell; occlusion on → no point covered twice.
  - `cell_mode_matches_the_twin_away_from_the_threshold`: GPU decision equals `cell_lod::draws` whenever the cell's and its ancestors' `edge_px/target` differ from 1 by more than 1e-4.
  - `octant_mode_is_unchanged`: `compaction_count_matches_a_cpu_count_on_every_adapter` passes.
- [x] **Step 2:** `cargo test -p viewer_render_wgpu --profile fast --offline --lib cell_render` → FAIL.
- [x] **Step 3:** implement.
- [x] **Step 4:** same command → PASS (record the adapters that ran); clippy clean.
- [x] **Step 5:** commit `feat(render): decide the drawn level per cell in the slot pass`.

### Task 8: Per-depth counters

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_render.rs` (counters after the draw args, `read_lod_counters`)
- Test: `cell_render_tests.rs`

**Interfaces:**
- Produces: `LodCounters { per_depth: [u32; 64] }` (drawn cells by absolute depth), filled only when `FrameUniform.counters != 0`; `CellRenderer::read_lod_counters(&self, device, queue) -> LodCounters`.

- [ ] **Step 1:** test `lod_counters_count_drawn_cells_per_depth` (equals the twin tally) → FAIL.
- [ ] **Step 2:** implement → PASS; counters off costs no atomics (branch on the uniform).
- [ ] **Step 3:** commit `feat(render): count drawn cells per depth on the GPU`.

### Task 9: Driver and picking in cell mode

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_driver.rs` (`set_lod_mode`, uniform values from the camera, node records from `MaskUpdate`), `cell_pick.rs` (drawn test through `cell_lod::draws` in cell mode)
- Test: `cell_driver_tests.rs`, `cell_pick_tests.rs`

**Interfaces:**
- Consumes: Tasks 3, 5–7.
- Produces: `CellDriver::set_lod_mode` already exists (forwards to the pool, e13e54b); this task makes a mode switch rewrite every node record, and derives each record's `active` bits from the `ActiveLevel`s emitted that frame (retained frames rebuild the active list but send no `MaskUpdate`, so the two can drift).

- [ ] **Step 1: Write the failing tests:** `driver_in_cell_mode_writes_records_and_uniform`; `pick_in_cell_mode_hits_the_drawn_cell` (GPU, against the twin).
- [ ] **Step 2:** `--lib cell_driver`, `--lib cell_pick` → FAIL.
- [ ] **Step 3:** implement → PASS; all lib tests pass.
- [ ] **Step 4:** commit `feat(render): drive cell mode on the GPU`.

### Task 10: The `cellLod` option end to end

**Files:**
- Modify: `crates/viewer_wasm/src/cells_glue.rs`, `bindings/cells.rs` (`setCellLod(mode: &str)`), `lod_cells.rs` (in-process path), `examples/web-host/viewer-api.mjs`, `viewer-worker.js`, `main.js` (`?cellLod=cell`), `examples/web-host/README.md` (API line)
- Test: `crates/viewer_wasm/src/lod_cells_tests.rs`, `examples/web-host/tests/cells-protocol.test.mjs`

**Interfaces:**
- Consumes: `CellDriver::set_lod_mode` (Task 9).
- Produces: `viewer.setCellLod('octant' | 'cell')` and the worker message `{type: 'setCellLod', mode}`; an unknown string is rejected with an error and the mode is kept; `?cellLod=cell` sets it at start (no UI control in this phase).

- [ ] **Step 1: Write the failing tests:** `switching_mode_rewrites_the_page_table`; `set_cell_lod_rejects_unknown_modes`; the JS round trip in `cells-protocol`.
- [ ] **Step 2:** `cargo test -p viewer_wasm --profile fast --offline --lib lod_cells` and `node --test examples/web-host/tests/cells-protocol.test.mjs` → FAIL.
- [ ] **Step 3:** implement → both PASS.
- [ ] **Step 4:** commit `feat(wasm): expose the cellLod option`.

### Task 11: Probes, docs and record

**Files:**
- Modify: `examples/web-host/probe-cells-parity.mjs` (both modes), `examples/web-host/PROBES.md`, `docs/explanation/REGIONAL_VOXEL_LOD.md` (per-cell rule E1–E7), master plan (tick 2a; D3 records the metric that replaced the distance rule), this plan's `Estado`
- Create: `docs/records/benchmarks/R3_FASE2A_LOD_POR_CELDA_<dd-mm-aaaa>.md`

- [ ] **Step 1:** `probe-cells-parity` in both modes on the demo sample and on `fs_e430` (V7); same-pose screenshots in octant and cell mode during a zoom; nodes with ≥ 2 drawn depths; slot-pass time with timestamp queries where the adapter has them; a run in Chrome WebGPU; `scripts/check.sh` with the real input.
- [ ] **Step 2:** record facts (per-depth counts, above-target by actual edge from the twin, adapters that ran the GPU tests) and inferences separately.
- [ ] **Step 3:** commit `docs: record phase 2a (per-cell LOD in the GPU)`.
