# Fase 1: niveles a demanda (#35) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

Estado: propuesto · 16-11-2026

**Goal:** the cell cut may want any depth of a node's buildable range; a depth the pruned chain does not hold is
requested from the build worker, built from the node's points and added to the stored node, so a cloud whose leaves
only keep fine levels (#35) draws at the level its view asks for.

**Architecture:** the build side keeps building whole nodes (pruned chain) and now also reports each node's
buildable depth range; the render side chooses depths over that range, and a wanted depth that is not stored becomes
a *level request* (render → build, latest set wins) instead of a refused offer; the build side builds requested
levels with a budget of its own and ships them as `nodeLevels`, which the driver merges into the stored node. The
in-process path wires the same calls without workers. Per-octant selection (ADR 0034) is unchanged in this phase.

**Tech Stack:** Rust (crates `viewer_render_wgpu`, `viewer_wasm`), wasm-bindgen, JS module workers
(`examples/web-host`), `node --test`.

**Spec:** [ADR 0048](../adr/0048-el-render-dibuja-el-nivel-pedido.md); master plan
[nivel-pedido-y-lod-por-celda.md](nivel-pedido-y-lod-por-celda.md) (phase 1, decision D1); measurements in
[R3_CELL_LOD_0A_15-11-2026](../records/benchmarks/R3_CELL_LOD_0A_15-11-2026.md).

## Global Constraints

- Code, comments and commits in English; docs in Spanish; dates `dd-mm-aaaa`. No AI attribution in commits.
- `FormatVersion::V<n>` literals only inside `viewer_format` (`scripts/check_version_literals.sh`).
- Checks: `scripts/check.sh` with `FORMAT_PARITY_REAL_INPUT=/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets/belv_2009_digital-aerial_pcd_copc.laz` before the branch is merged. Single tests: `cargo test -p <crate> --profile fast --offline --lib <path>` and `node --test examples/web-host/tests/<file>.test.mjs`.
- Buildable range of a node: leaf `first = representative_depth_for_spacing(grid, spacing)` (or `representative_depth(grid, bounds)` without spacing) to `MAX_CELL_DEPTH` (= 63); internal node `first = representative_depth(grid, bounds)` to `first + 5`. Same values as `PointNodeCellSource::node_cells` today (`node_cells.rs:191-203`).
- R3-Q21 (`docs/requirements.md`): the image only refines; while a requested level is missing, the resident levels keep drawing.
- Residency rule D4 (drawn depths + next finer) is phase 2b, not this phase; this phase only must not request depths the cut does not want.
- Recolouring (R3-Q33) rebuilds nodes: on-demand levels are dropped and requested again, never recoloured in place.
- Byte identity of `.oct` files is unaffected (render-side only); `cell_cut_digest` goldens change and each new value is committed with the reason.

## Review Focus

1. A requested node whose blocks were evicted from the build cache: the request must schedule the block reads and be served later, never starve silently (Task 5 test `an_uncached_request_schedules_its_blocks_and_is_served_later`).
2. A zoom that changes the wanted depths every frame: only the latest request set is kept; stale requests are not built (Task 5 test `a_newer_request_set_replaces_the_pending_one`).
3. A colour-mode change while on-demand levels are stored: the node is recoloured without `same_shape` failures (Task 2 test `recolor_drops_on_demand_levels`).
4. A level that arrives for a node the driver already forgot, or for an older generation: ignored without error (Task 2 test `levels_for_a_forgotten_node_or_old_generation_are_ignored`).
5. Internal nodes: no request deeper than `first + 5` (Task 3 test `internal_nodes_never_want_beyond_their_range`).

---

### Task 1: Build one level and report the buildable range

**Files:**
- Modify: `crates/viewer_render_wgpu/src/node_cells.rs` (`NodeCells`, `PointNodeCellSource::node_cells`, new fn)
- Modify: `crates/viewer_render_wgpu/src/lib.rs` (re-export)
- Test: `crates/viewer_render_wgpu/src/node_cells_tests.rs`

**Interfaces:**
- Produces: `NodeCells { node, levels, buildable: (u8, u8) }` (inclusive min, max depth); `pub fn node_cell_level(grid: &CellGrid, node: NodeId, bounds: SpatialBounds, spacing: Option<f64>, is_leaf: bool, blocks: &[PointBlock], depth: u8) -> Result<NodeCellLevel, NodeCellError>` (same cells and `octant_offsets` as the chain builds at that depth; `Err(AbsoluteDepthExceeded)` outside the buildable range).

- [ ] **Step 1: Write the failing tests** in `node_cells_tests.rs`:
  - `single_level_matches_the_unpruned_chain`: with `PointNodeCellSource { min_growth: 1.0 }` on the existing leaf fixture, for every level `l` of `node_cells(..)`, `node_cell_level(.., l.depth) == l`.
  - `a_level_finer_than_separation_keeps_one_cell_per_position`: depth = deepest chain depth + 2 → `cells.len()` equals the number of distinct positions.
  - `buildable_range_matches_the_chain_bounds`: leaf → `(first, MAX_CELL_DEPTH)`; internal → `(first, first + 5)`; a depth outside → `Err(NodeCellError::AbsoluteDepthExceeded)`.
- [ ] **Step 2:** `cargo test -p viewer_render_wgpu --profile fast --offline --lib node_cells` → FAIL (`node_cell_level` not defined).
- [ ] **Step 3:** implement `node_cell_level` by reusing the private `build_level` and `octant_offsets` (factor the shared preamble of `node_cells` into a private helper); fill `buildable` in `node_cells`.
- [ ] **Step 4:** same command → PASS; `cargo clippy -p viewer_render_wgpu --offline --all-targets -- -D warnings` clean.
- [ ] **Step 5:** commit `feat(render): build one cell level on its own and report the buildable range`.

### Task 2: Carry the range in `PackedNode` and merge levels into a stored node

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_driver.rs` (`PackedNode`, `pack_node`, `store_packed`, new `add_packed_levels`, recolour path)
- Modify: `crates/viewer_render_wgpu/src/cell_codec.rs` (`CODEC_VERSION` 1 → 2, encode/decode `buildable`)
- Modify: `crates/viewer_render_wgpu/src/cell_pool.rs` (`PoolNode.buildable`)
- Test: `crates/viewer_render_wgpu/src/cell_driver_tests.rs`, codec tests in `cell_codec.rs`

**Interfaces:**
- Consumes: `NodeCells.buildable` (Task 1).
- Produces: `PackedNode.buildable: (u8, u8)`; `PoolNode.buildable: (u8, u8)`; `pub fn add_packed_levels(&mut self, generation: u64, levels: PackedNode) -> Result<LevelsAdded, DriverError>` on `CellDriver`, where `levels.levels` holds only new depths; `pub enum LevelsAdded { Added(usize), Ignored }`.

- [ ] **Step 1: Write the failing tests:**
  - codec: `node_levels_round_trip_with_the_buildable_range` (encode → decode equal, `buildable` kept); a version-1 buffer → `Err` (version mismatch).
  - driver: `added_levels_extend_the_stored_chain_in_order` (stored `[10, 14]` + level 12 → stored `[10, 12, 14]`, `set_level_sizes` has 12, `cpu_bytes` grows by the level's bytes); `an_already_stored_depth_is_ignored`; `levels_for_a_forgotten_node_or_old_generation_are_ignored` (→ `Ok(LevelsAdded::Ignored)`); `recolor_drops_on_demand_levels` (store chain `[10, 14]`, add 12, recolour with a node of depths `[10, 14]` → `Ok`, stored back to `[10, 14]`, `cpu_bytes` drops by level 12).
- [ ] **Step 2:** `cargo test -p viewer_render_wgpu --profile fast --offline --lib cell_driver` and `--lib cell_codec` → FAIL.
- [ ] **Step 3:** implement; on recolour, remove depths not present in the incoming node before the `same_shape` check.
- [ ] **Step 4:** both commands → PASS; clippy clean.
- [ ] **Step 5:** commit `feat(render): merge on-demand levels into stored nodes`.

### Task 3: The cut wants any buildable depth

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_cut.rs` (`CellCutNode`, `octant_wanted_depth`, `octant_wanted_depth_near`, promotion at `cell_cut_with` ~:504)
- Modify: `crates/viewer_render_wgpu/src/cell_pool.rs` (`wanted_cut`, `refresh_depths`, `Ctx::depth`, `continuity_depth`, `capped`)
- Test: `crates/viewer_render_wgpu/src/cell_cut_tests.rs`, `cell_pool_tests.rs`

**Interfaces:**
- Consumes: `PoolNode.buildable` (Task 2).
- Produces: `CellCutNode { bounds, stored, resident, buildable: (u8, u8) }`; `octant_wanted_depth(view, grid, coordinate_origin, bounds, octant, buildable: (u8, u8), target_px) -> u8` = the coarsest depth `d` in the range whose cell meets the target, else the range max; `CellCutEntry.wanted` documented as "buildable depth the view asks for".

- [ ] **Step 1: Write the failing tests:**
  - replace `pruned_depth_resolves_to_next_stored_finer` with `a_pruned_depth_is_wanted_as_is`: full chain 3..=10, stored `[3, 10]` → `wanted == w_full` for every octant.
  - `internal_nodes_never_want_beyond_their_range`: eye inside an internal node → `wanted <= first + 5`.
  - `a_leaf_can_want_finer_than_its_chain`: eye inside a leaf at 1 px → `wanted > stored.last()`.
  - `octant_choice_matches_the_brute_force_oracle` runs over the buildable range instead of `stored`.
  - continuity: `the_continuity_cap_uses_the_buildable_range` replaces `the_continuity_cap_snaps_to_a_stored_level_and_falls_back_to_the_coarsest`.
- [ ] **Step 2:** `cargo test -p viewer_render_wgpu --profile fast --offline --lib cell_cut` and `--lib cell_pool` → FAIL.
- [ ] **Step 3:** implement; spanning promotion picks the first buildable depth `> node_depth` (no `expect` on `stored`).
- [ ] **Step 4:** update `GOLDEN` in `cell_cut_digest_golden` to the new digest; note in the commit body why each value changed. Both commands → PASS; clippy clean.
- [ ] **Step 5:** commit `feat(render): let the cut want any buildable depth`.

### Task 4: Missing levels become requests, not refused offers

**Files:**
- Modify: `crates/viewer_render_wgpu/src/cell_pool.rs` (`FramePlan`, request loop ~:2351-2378), `cell_driver.rs` (request loop ~:813-852, `CellMessages`-facing output)
- Test: `cell_pool_tests.rs`, `cell_driver_tests.rs`

**Interfaces:**
- Consumes: Tasks 2–3.
- Produces: `FramePlan.level_requests: Vec<LevelKey>` (wanted, not stored, in request priority order); `CellDriver::take_level_requests(&mut self) -> Option<Vec<(NodeId, u8)>>` returning the current set only when it changed since the last call.

- [ ] **Step 1: Write the failing tests:**
  - pool: `a_wanted_depth_outside_the_chain_is_requested_and_residents_keep_drawing` (stored `[10]`, wanted 12 → `level_requests == [12]`, the drawn depth stays 10, no hole: covered volume unchanged).
  - driver: `a_missing_level_is_requested_without_failing_the_frame` (no `OfferRefused`, `take_level_requests() == Some([(node, 12)])`, a second frame with the same view → `None`); `an_arrived_level_is_offered_on_the_next_frame` (after `add_packed_levels`, the next frame uploads depth 12 and draws it).
  - keep `a_refused_offer_does_not_keep_the_node_from_being_forgotten` passing (a stored level that cannot be offered still refuses).
- [ ] **Step 2:** `cargo test -p viewer_render_wgpu --profile fast --offline --lib cell_pool` and `--lib cell_driver` → FAIL.
- [ ] **Step 3:** implement; a missing depth inside `buildable` goes to `level_requests` and stays `Requested`; outside `buildable` it is a bug (`debug_assert!`).
- [ ] **Step 4:** both commands → PASS; clippy clean.
- [ ] **Step 5:** commit `feat(render): request missing levels instead of refusing the frame`.

### Task 5: The build side serves level requests

**Files:**
- Modify: `crates/viewer_wasm/src/lod_cells.rs` (`CellState`, `CellUpdateConfig`, `plan_cell_update`, new `apply_level_requests`, `build_node_level`)
- Modify: `crates/viewer_wasm/src/cells_glue.rs` (`BuilderCells`, `BuilderOutput.levels`)
- Test: the `lod_cells` tests module (`lod_dataset.rs` ~:1418)

**Interfaces:**
- Consumes: `node_cell_level` (Task 1), `PackedNode.buildable` (Task 2).
- Produces: `LodDataset::apply_level_requests(&mut self, requests: &[(NodeId, u8)])` (replaces the pending set); `CellUpdateConfig.max_level_builds: usize` (default 8, separate from `max_node_builds`); `BuilderOutput.levels: Vec<EncodedNode>` (one `PackedNode` per node with its new levels, encoded with `encode_node_levels`); on `set_cell_paint` the pending set is cleared.

- [ ] **Step 1: Write the failing tests:**
  - `a_requested_level_is_built_and_shipped` (request `(leaf, chain_min - 1)` for a built leaf → next update's `levels` holds that depth only).
  - `a_newer_request_set_replaces_the_pending_one` (request A, then B before an update → only B is built).
  - `an_uncached_request_schedules_its_blocks_and_is_served_later` (evict the node's blocks → the update lists them to read and ships nothing; after `insert_block` the next update ships the level).
  - `level_builds_respect_their_own_budget` (12 requests → 8 built in one update, 4 in the next).
  - `a_colour_change_clears_pending_requests`.
- [ ] **Step 2:** `cargo test -p viewer_wasm --profile fast --offline --lib lod_cells` → FAIL.
- [ ] **Step 3:** implement; an uncached node adds its blocks to the update's read list (the same list `plan_cell_update` fills for candidates).
- [ ] **Step 4:** same command → PASS; `cargo clippy -p viewer_wasm --offline --all-targets -- -D warnings` and the wasm32 clippy of `scripts/check.sh` clean.
- [ ] **Step 5:** commit `feat(wasm): build requested cell levels on demand`.

### Task 6: Wire the protocol (workers and in-process)

**Files:**
- Modify: `crates/viewer_wasm/src/bindings/cells.rs` (`addCellLevels(generation, bytes)`, `applyLevelRequests(array)`, `CellMessages` → `levelRequests`), `crates/viewer_wasm/src/cells_glue.rs` (`ViewerCells::push_levels`, `InProcessCells::update`)
- Modify: `examples/web-host/viewer-worker.js` (`forwardCellMessages`, `nodeLevels` handler), `examples/web-host/build-worker.js` (header comment, `cellInbox`, `postCellUpdate`)
- Test: `examples/web-host/tests/cells-protocol.test.mjs`; an in-process test in the `lod_cells` sim (`lod_cells_glue_sim.rs`)

**Interfaces:**
- Consumes: Tasks 4–5.
- Produces: render → build `levelRequests{gen, cutEpoch, requests: Uint32Array}` (pairs node-id low/high + depth, or the encoding the existing `evict` message uses for node ids); build → render `nodeLevels{gen, generation, buffers}` (transfer list = buffers).

- [ ] **Step 1: Write the failing tests** in `cells-protocol.test.mjs`:
  - `levelRequests are forwarded after evict and ack, latest set wins when coalesced`.
  - `nodeLevels are applied in order after nodeCells of the same batch, with their buffers transferred`.
  - `levelRequests and nodeLevels for another gen are ignored`.
  - Rust sim: `in_process_cells_serve_a_level_request` (an in-process update requests and then draws a pruned depth).
- [ ] **Step 2:** `node --test examples/web-host/tests/cells-protocol.test.mjs` and `cargo test -p viewer_wasm --profile fast --offline --lib lod_cells_glue_sim` → FAIL.
- [ ] **Step 3:** implement; keep `nodeLevels` outside the epoch filter like `nodeCells`.
- [ ] **Step 4:** both commands → PASS; `node --test examples/web-host/tests/*.test.mjs` all pass.
- [ ] **Step 5:** commit `feat(web): carry level requests and on-demand levels between workers`.

### Task 7: Acceptance on #35 and docs

**Files:**
- Test: `crates/viewer_wasm/src/lod_cells_glue_sim.rs` (or the `lod_cells` sim tests)
- Modify: `docs/plans/nivel-pedido-y-lod-por-celda.md` (tick phase 1), `docs/estado.md`, `examples/web-host/PROBES.md` if a probe reads `stored`
- Create: `docs/records/benchmarks/R3_FASE1_NIVELES_A_DEMANDA_<fecha>.md`

- [ ] **Step 1: Write the failing test** `a_close_pair_leaf_draws_at_its_spacing_from_far` (the ADR 0046 scenario: a leaf whose chain keeps only fine levels, viewed from far at 4 px → after the level request round-trip the drawn depth equals the depth the full chain would draw).
- [ ] **Step 2:** `cargo test -p viewer_wasm --profile fast --offline --lib a_close_pair_leaf_draws_at_its_spacing_from_far` → PASS (Tasks 1–6 make it pass; it guards the #35 scenario from now on).
- [ ] **Step 3:** build the demo (`scripts/serve-demo.sh`) and verify with Playwright (desktop and mobile): `fs_e430_n4008_18.las` converted to V7 and opened with the file picker draws a surface, not stray points; the built-in sample still reaches the target; console clean. Record request-to-resident p95 and the request queue length over a 10 s zoom (kill criterion: steady growth, or p95 > 1 s at 4 px).
- [ ] **Step 4:** `scripts/check.sh` with `FORMAT_PARITY_REAL_INPUT` → all steps pass.
- [ ] **Step 5:** write the record and the docs; commit `docs: record phase 1 (on-demand levels)`.
