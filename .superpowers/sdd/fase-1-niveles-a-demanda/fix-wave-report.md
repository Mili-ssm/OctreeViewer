# Fix wave report (stopped at turn limit; nothing committed)

Tests: viewer_render_wgpu lib 319 passed; viewer_wasm lib 150 passed; node tests 240 passed.
Not run: clippy (native, wasm32), cargo fmt --check (cargo fmt was run), scripts/build-wasm-bindings.sh.

| Fix | State | Files / tests |
| --- | --- | --- |
| F1 refused depths | partial: code + JS test done, Rust wasm-level test missing | `CellDriver::refuse_levels` (cell_driver.rs); `CellUpdate::refused_levels`, `report_refused` (lod_cells.rs); `BuilderOutput.refused`, `ViewerCells::refuse_levels` (cells_glue.rs); `levelsRefused` result field + `refuseCellLevels` binding (bindings/cells.rs); build-worker.js (message `levelsRefused{gen,nodes,depths}`, header updated); viewer-worker.js; in-process path wired in draw_planned_cells; sims wired (lod_cells_sim.rs, lod_cells_glue_sim.rs). Tests: driver `a_refused_depth_stops_being_wanted_and_the_view_settles` (cell_driver_tests.rs); 2 JS tests in cells-protocol.test.mjs. |
| F2 cap panic | done | `capped` never empty and `CellPool::reconcile_cap` (cell_pool.rs), called from `recolor_node` (cell_driver.rs). Test `a_cap_on_a_dropped_depth_does_not_panic_after_a_recolour` (cell_pool_tests.rs, pool level, not through the driver). |
| F3 stale requests | code done, test missing | `driver_replaced` clears `level_requests` (lod_cells.rs). |
| F4 resident_depth | done | closest resident, tie goes finer, stage only if coarser (cell_cut.rs); tests updated in cell_cut_tests.rs (`resident_depth(5,[3,7,4])` = (4,true); spanning-level test now expects depth 0 whole, all stage). |
| F5 `_node` param | done | node_cells.rs, node_cells_tests.rs, lod_cells.rs callers. |

F4 gate: 0 unexcused in all 18 synthetic sims (stage-excused 0, zone coarsenings 0); synthetic stream harnesses staged 0. Real-data tests (`belvedere-v7.oct`) not run: file absent in target/render-data.

Left:
1. Add wasm-level test `a_refused_depth_stops_being_wanted_and_the_view_settles` (needs a Pair in lod_cells_glue_sim.rs; a real depth-63 want is not reachable from a view, so inject the refusal: a test seam for `unservable_levels` or a leaf whose `node_cell_level` fails). Assert requests no longer list it, `cut_digest_at_rest` Some, no node held.
2. Add `driver_replaced_clears_pending_level_requests` in lod_cells_tests.rs (apply_level_requests, driver_replaced, assert `!has_pending_level_requests(true)`).
3. Run clippy native + wasm32, `cargo fmt --check`, `scripts/build-wasm-bindings.sh`; then commit.
Risks: recolor resets `buildable` so a refusal is re-reported via the unservable skip path (intended); docs/ changes in the tree are the controller's.

## Finish

Added `driver_replaced_clears_pending_level_requests` (lod_cells_tests.rs) and glue-level `a_refused_depth_stops_being_wanted_and_the_view_settles` (lod_cells_glue_sim.rs; injects refusals of the deeper wanted depths, the coarsest buildable depth is served for real). Driver-level test renamed `a_refused_depth_lowers_what_the_node_can_want_and_the_view_settles`.
Checks: render lib 319 passed; wasm lib 152 passed; node 240 passed; clippy native + wasm32 clean; fmt clean; build-wasm-bindings OK. Committed code only (docs untouched).

## Last fixes

- Producer-side refusal test `an_unservable_request_is_reported_as_refused_on_the_next_update` (glue sim, via `BuilderOutput.refused`); reported once, not repeated by later updates. A request asked again is answered again (re-reported, no rebuild): kept as is. No encode/decode step exists outside wasm bindings, so no round-trip test.
- `reconcile_cap` keeps the cap and clamps it to the coarsest stored depth; test updated. Sims/strict gate tests stay green.
- Nits: glue-test comment, `build-worker.js` header, `None => continue` in `serve_level_requests` commented (stale request: node evicted or being rebuilt, so dropped without refusal).
- Plan signature of `node_cell_level` fixed; docs link/index checks ok.
- `docs/requirements.md` had an uncommitted change not made by this wave (R3-Q21 owner confirmation); left out of the commits.
