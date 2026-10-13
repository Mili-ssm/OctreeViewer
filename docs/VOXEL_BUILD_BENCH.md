# Voxel cell reuse benchmark

## Scope and reproduction

The builder keeps one fixed-size hint: the most recently resolved terminal cell and its
visible/culled result. A new point reuses it only if the original floor/clamp coordinate
formula maps the point to exactly the same integer key at that depth. Otherwise the builder
runs the original root-to-terminal traversal. The hint lives only within one builder and
uses the existing stable-projection callback contract. It changes neither cell size nor the
accumulator, attributes, ordering, instance limits or dynamic-memory budgets. It adds one
constant-size field, outside the existing accumulator/projection-cache estimate; that
estimate remains a model rather than RSS. No persistent geometry or camera cache is added.

```bash
cargo run --profile perf --offline -p viewer_render_wgpu --example voxel_build_bench -- data/datasets/sensoforest/majadas-tls.oct 3
```

`voxel_build_bench` selects the fitted-camera final cut at 1280×720 and 4 px, reads coordinates
only (gray), then times accumulation plus finish, also reporting those phases separately. It
prepares the same unique blocks outside the timer and keeps them resident, capped by default
at 256 MiB of `PointBlock::estimated_memory_bytes()`. Optional `MAX_DECODED_MIB` (1–2048) and
`TARGET_PIXELS` (0.5–16) follow `RUNS`. Before reading payloads, it sums selected storage point
counts plus `PointBlock` headers and rejects a cut whose lower bound exceeds the cap; it also
checks actual accounted block memory while decoding.
The cap excludes index, allocator and blocks-vector overhead; there may also be transient
reader allocations. This diagnostic is not an out-of-core or RSS measurement. It requires
1–20 runs (default three), checks identical scenes within each process, and outputs JSON with
individual timings, median/range, callback counts, cell-edge metrics, memory estimates and
an FNV-1a checksum of the instance bytes (including color and scalar fields). The checksum is
a reproducibility check, not a cryptographic integrity guarantee.

## Native paired measurements — 12-10-2026

Before is `6cd4a7c` plus the same diagnostic fixture; after adds only the terminal-cell hint
in `voxel_grid.rs`. Both binaries use the **perf** profile, rustc 1.99.0, Linux
7.2.9-200.fc44.x86_64, AMD Ryzen AI 9 365 CPU. No browser, conversion or compilation runs during
native measurement. For each dataset, three process pairs alternate order (before/after,
after/before, before/after), with three builds per process: nine samples per version.
Inputs and decoded-block order are unchanged. File reads are outside the timer; builds mix
the initial CPU-cache state with subsequent warm builds. These figures isolate final-cut
CPU construction and do not measure all progressive cuts, file loading, upload or FPS.

Median seconds [minimum–maximum] across the nine samples:

| Dataset | Cut source points / blocks | Final voxels | Before (s) | After (s) | Median ratio |
| --- | ---: | ---: | ---: | ---: | ---: |
| Majadas TLS | 7,015,449 / 202 | 6,397 | 1.2965 [1.2784–1.3339] | 0.3474 [0.3428–0.3530] | 3.73× |
| Albuera UAV | 12,673,149 / 568 | 17,085 | 2.4033 [2.2926–2.4406] | 1.2317 [1.1985–1.2827] | 1.95× |
| Outer Banks `fs_e430_n4008_18` | 111,190 / 1 | 2,410 | 0.01832 [0.01725–0.03731] | 0.00826 [0.00801–0.00898] | 2.22× |

Each pair preserves the instance checksum, source/block/voxel counts, edge range, zero
oversized voxels, callback counts and reported builder-memory metrics. Checksums respectively
`563a1f928baa5a75`, `a66ff905faf0cb39`, `ebb46abe0106a53f`. Majadas retains
34,782,624 estimated builder bytes, 255,880 instance-capacity bytes and 11,917 peak projected
cells on native 64-bit; WASM estimates differ with pointer size. The native fixture guard
rejects **Belvedere** before timing at block 309 in both versions because its complete
selected payload exceeds 268,435,456 accounted bytes. No Belvedere speedup is inferred and
the cap was not raised to obtain a result.

The synthetic 4,096-point `sample.oct` provides a limit to the speedup claim. An initial short
series (three process pairs × twenty builds) produced pooled medians 5.36 ms before and
7.62 ms after, with both ranges extending to ~12.1 ms and substantial variation between
processes. A follow-up after an explicit Majadas CPU warm-up, nine alternating pairs × twenty
builds, gave 5.37 ms before and 5.35 ms after (180 samples per version), again with ~12.1 ms
outliers. Its 4,096-voxel scene checksum is unchanged. No meaningful synthetic speedup is
established; the cause of the initial timing shift is not isolated. Both series remain in
the raw reports (`synthetic-pair-*` and `synthetic-warm-pair-*`). Locality benefits are not
assumed for every point distribution.

Regression tests compare hinted versus full traversal on mixed-depth and culled cells,
clamped root boundaries and adjacent faces, deterministic random points, disabled/evicting
projection caches and the depth-24 nonfinite-projection fallback. Scene tests preserve RGB,
intensity/classification and both voxel/memory budget errors. The full renderer/WASM test
run passes **81 tests, with two native GPU diagnostics ignored**; native and WASM Clippy
with warnings denied, formatting and a release WASM rebuild also pass.

Raw pair reports, binary SHA-256/environment, frozen executables and summary are under
ignored `target/voxel-hint-bench/`. Baseline binary SHA-256:
`384ee776531faaf93adcd69bd52fb2161fee3d64eed94eee84488a37851fcfe3`;
after: `98d8e1d3b4f7681e1fbe77e3002cb0844be8bf589dfa8ea0af1ad721dce7bbe0`.

## Firefox validation after the change

Release WASM rebuilt offline, Firefox 157.0.1 headless, new temporary profile, WebGPU enabled,
HTTP memory/disk cache disabled, 1280×720, 4 px, fitted camera, gray XYZ, depth shading and
256 MiB decoded cache. OS cache was not flushed; adapter identity and browser RSS remain
unavailable. No conversion or compilation ran concurrently. Page/worker recreated for each
run in one browser session; full 5 s warm-up plus 10 s reply window after target completion.

Majadas produces **3/3 valid exports**. Median [minimum–maximum]: first coarse **46 [45–76] ms**,
target LOD **59,696 [58,370–65,937] ms**, cumulative voxel build **53,422 [52,723–58,964] ms**,
and **60.00 [60.00–60.00] reply FPS**. All retain 239 loaded blocks/builds, 522,040,584
accumulated cut source points, 7,015,449 final selected source points, 202 visible blocks,
6,397 voxels, complete 4 px coverage, zero pending blocks and the same cache/byte-read counts.
The first screenshot is byte-identical to both earlier Majadas screenshots, SHA-256
`dd4d9b7d87e251f9a16c0d00514142df209c87e13caac34b625484184a65b3d9`.

Earlier unpaired sessions took 152,983–162,900 ms to reach target; they are retained as
historical context, not a paired browser speedup ratio. The native ratios above isolate the
change; a progressive browser before/after pairing is still needed for a comparable web
ratio. The earlier third-attempt BiDi timeout did not recur in these three runs, and its
cause remains unknown. Albuera/Belvedere, RGB/attribute pairs and Chrome remain open.
Raw exports, screenshots, session/adapter details and summary: ignored
`target/voxel-hint-bench/web-after/` and `web-summary.json`.

## Belvedere phase and capacity diagnostic — 13-10-2026

The diagnostic now exports `samples_accumulate_seconds`, `samples_finish_seconds`,
`selected_source_points_from_storage` and `selected_decoded_bytes_floor`. The lower bound is
point count × native `size_of::<Point>()` plus one `PointBlock` header per unique block. It is
neither RSS nor a browser cache measurement. Raising its optional cap allows a bounded native
fixture; it does not raise the product's browser cache limit.

```bash
# Preflight rejection before reading point payloads:
cargo run --profile perf --offline -p viewer_render_wgpu --example voxel_build_bench -- data/datasets/belv_2009_digital-aerial_pcd_copc.oct 1 1024 4
# Retain the complete native cut for phase timing:
cargo run --profile perf --offline -p viewer_render_wgpu --example voxel_build_bench -- data/datasets/belv_2009_digital-aerial_pcd_copc.oct 3 2048 4
# Coarser cut within the default fixture cap:
cargo run --profile perf --offline -p viewer_render_wgpu --example voxel_build_bench -- data/datasets/belv_2009_digital-aerial_pcd_copc.oct 3 256 8
```

At 4 px, preflight reports **1,004 unique blocks**, **74,188,113 source points** and a minimum
**1,187,066,032 B (1,132.1 MiB)** decoded payload. That cut cannot fit in 1 GiB even before
allocator/index overhead. The explicit 2 GiB diagnostic fixture retains exactly that accounted
payload. The diagnostic now applies the metadata visual scale to camera fit, frustum, node errors and
voxel projection, matching the renderer formulas. Belvedere stores scale **0.01649201621495024**;
earlier diagnostic measurements used normalized scale 1.0. Historical paired speed ratios remain
limited to that shared native fixture and are not browser speed ratios.
This is capacity evidence for this fitted native target, not proof of the exact live browser
cache working set: progressive refinement also retains fallback blocks and pending selections.
It does not complete the previous camera-change browser observation.

Three builds per process, median [minimum–maximum]:

| Target | Source points / blocks | Accumulation (s) | Finish (ms) | Final voxels |
| --- | ---: | ---: | ---: | ---: |
| 4 px | 74,188,113 / 1004 | 12.7368 [12.5165–12.7766] | 5.518 [5.374–5.538] | 20,977 |
| 8 px | 13,238,581 / 320 | 1.4350 [1.4334–1.4386] | 1.298 [1.277–1.298] | 5,140 |

Accumulation accounts for >99.9% of timed construction. Checksums at 4/8 px are respectively
`a22b8f76486544d3` / `c057ef2774b0ca75`; both match the normalized-scale fixture.
The corrected scale preserves these cuts and scene bytes for Belvedere; equivalence is not
assumed for every dataset.

All runs use the native `perf` profile on the same Ryzen AI 9 365/Linux host, fitted camera,
1280×720, coordinate-only input and depth-independent CPU construction. Reads are outside the
timer; no compilation or browser probe runs concurrently with the reported series. A preliminary
series overlapped static checks and is excluded. The checksum remains stable across builds.
Final-cut CPU times must not be divided into the 837 s of accumulated Firefox voxel work to
claim a native/WASM speed ratio: that browser run builds many intermediate cuts.

Raw JSON and the frozen diagnostic executable are under ignored `target/voxel-phase-bench/`.
Next: measure a localized reduction of redundant accumulator-map lookups with alternating
before/after builds, preserving scene bytes and budget errors; then revisit progressive work
and the browser sample after camera input.

## Single accumulator lookup — 13-10-2026

`accumulate_block` now uses one `BTreeMap::entry` lookup per retained point. Occupied
entries keep accumulating directly; vacant entries check renderability, voxel count and
estimated memory before insertion. Integer keys, traversal, attribute aggregation and sorted
scene emission are unchanged. This localized change was already present in the working tree
when this measurement started; the baseline is `ef82604` and the after binary includes only
that `voxel_grid.rs` diff.

Same native perf profile, Ryzen AI 9 365/Linux/rustc 1.99.0, fitted camera, 1280×720,
coordinate-only blocks. Three process pairs alternate before/after, after/before,
before/after, each process building three times (nine samples per version/case).
Reads are outside the timer; OS cache is not flushed. No compilation, tests or browser
runs overlap the valid series. Belvedere 4 px uses the explicit 2 GiB diagnostic cap;
8 px and Majadas use 256 MiB. These are resident final-cut CPU measurements.

Accumulation seconds, median [minimum–maximum]:

| Case | Before | After | Median reduction |
| --- | ---: | ---: | ---: |
| Belvedere 4 px | 12.73875 [12.60519–12.85715] | 11.45416 [11.32916–11.58113] | 10.1% |
| Belvedere 8 px | 1.43873 [1.41818–1.47185] | 1.23878 [1.23075–1.25659] | 13.9% |
| Majadas 4 px | 0.34209 [0.33808–0.36128] | 0.28681 [0.28437–0.31772] | 16.2% |

All 18 scene builds per case preserve scene checksum, voxel/block/source counts,
projected edge range, zero oversized voxels, callback counts and builder-memory metrics.
Checksums are `a22b8f76486544d3`, `c057ef2774b0ca75`, `563a1f928baa5a75` respectively.
Finish medians before/after are 5.415/5.412 ms, 1.281/1.285 ms and 1.476/1.461 ms.
No progressive WASM/browser improvement or RSS reduction is established by this series.

Frozen executables, environment, raw JSON and summary: ignored `target/voxel-entry-verified/`.
Binary SHA-256 before: `322d36894ecb93f11282f9c253fd937fa5ade7093b2946c6ff69c4c49e1029b3`;
after: `e9678be313682e1b31e4daf44cf3b01eae8491c9b413ce9b8d55495be45d4815`.
An earlier attempted series in `target/voxel-entry-bench/` reused the baseline executable
for both labels and is excluded; binary hashes were checked before the valid rerun.

Renderer/WASM tests pass (85 tests, two GPU diagnostics and one microbenchmark ignored).
Formatting and native/WASM Clippy with warnings denied also pass.
Existing regressions exercise repeated occupied cells under an exact one-voxel budget,
new-cell memory/instance failures, RGB and typed attributes, and nonoverlapping integer cells.
Next: rebuild release WASM and repeat the changed-camera observation with fresh exports
and a longer no-progress window. The 1 GiB cache still cannot hold the complete fitted 4 px
native cut; faster lookup does not resolve that capacity constraint.

## Progressive accumulator reuse: local contract audit — 13-10-2026

The current whole-scene reuse is safe for an identical cut and camera/viewport/voxel-stage
context (`viewer_wasm/src/lod_dataset.rs`, `reusable_build_match`). Successful replacement of
a visible payload clears `last_successful_build` in `cache_block`; an unsuccessful replacement
keeps it. Attribute changes clear the loaded blocks and scene context. There is no missing
payload-invalidation bug established by this audit.

An append-only accumulator experiment must compare the **actual traversal sequence** used by
`build_voxel_scene_with_limits` (node order then each node's block order), not only sorted
`plan.block_ids()`. The new sequence must extend the old one without changing previous point
payloads, attributes or projection results. First-point classification is order-sensitive;
floating intensity sums can also depend on order. Sorting or subtracting block aggregates
would need a separate equivalence argument. Replacement transitions can remove fallback
ancestors or replace active descendants, so progressive loading is not generally append-only.
Camera, viewport, stage and payload/attribute changes must invalidate retained accumulation.

`VoxelSceneBuilder::try_finish(self)` currently consumes the map. Retaining a builder would
extend the lifetime of its accumulators/projection cache alongside output instances and the
previous visible scene. Cloning that map for publication would add another peak. Existing
estimated budgets cannot be assumed to cover these combined lifetimes, and poisoned builders
must not replace the last successfully published scene on an insertion/finish error.

Pending acceptance steps before introducing that state:

- Extend the strict-prefix measurements below beyond Majadas to Albuera/Belvedere and
  Additive datasets, separately from replacements and scene reuse.
- Bound retained accumulator + projection cache + old/new instances, and define discard/rebuild
  behavior on context change or exhausted limits; preserve the existing payload invalidation.
- Compare complete scene bytes against full rebuild for Replacement/Additive, classification,
  nonfinite attributes, camera changes, budget failures and shared block references.
- Only then prototype a single retained accumulator if eligible measured work warrants it;
  compare native/WASM and progressive browser work with equivalent inputs. No persistent
  incremental accumulator is implemented by this audit.

## Strict-prefix opportunity measurement — 13-10-2026

The WASM work export now records `prefixRebuilds`, `prefixRetainedSourcePoints`,
`prefixAddedSourcePoints` and `prefixVoxelBuildMs`. Eligibility requires an unchanged successful
build context and a strict extension of the actual ordered node/block traversal. Empty previous
scenes and identical sequences are excluded. Shared block references count each traversal;
source-point totals are decimal strings to preserve integer precision. Payload/attribute
invalidation uses the existing successful-build context. These counters describe full rebuild
attempts, including failed attempts, and do not implement persistent accumulation.

The allocation-free block scan runs before the build timer; its overhead has not been measured
against a baseline. `prefixVoxelBuildMs` is the entire candidate rebuild time, **not time saved**.
Regression coverage checks strict extension, shared references, replacement/removal/reordering,
empty previous scenes and context invalidation. Native renderer/WASM tests: 86 passed, three
ignored; formatting and scoped native/WASM Clippy with warnings denied pass.

Three fresh Firefox profiles ran `probe-cache-camera.mjs` on Majadas coordinates only, at
1280×720, 4 px and the default 256 MiB cache. Each phase reached `target_complete` with zero
pending blocks and no console errors. Raw exports are under ignored
`target/prefix-opportunity-bench/run-{1,2,3}/`; release WASM SHA-256:
`b40f391e76b401dab40e0baca3f95e27823e1bdc2808d879f5a8db9b6018a764`.

| Initial phase | Round 1 | Round 2 | Round 3 |
| --- | ---: | ---: | ---: |
| Full builds / reused updates | 98 / 141 | 98 / 141 | 98 / 141 |
| Strict-prefix builds | 3 | 3 | 3 |
| Retained / added source points | 1,870 / 125 | 1,870 / 125 | 1,870 / 125 |
| Candidate full-build time (ms) | 3 | 4 | 4 |
| All full-build time (ms) | 12,071 | 12,155 | 11,128 |

All rounds loaded 239 blocks, processed 166,721,822 source points across full rebuilds and
finished with 6,397 voxels. The camera phase added three full builds and 11,102,845 source
points, produced 6,396 voxels, and added **zero** strict-prefix opportunities in every round.
Candidate work is less than 0.04% of initial full-build time, before accounting for unavoidable
new-point work and publication. These observations do not justify a retained append-only
accumulator for this case. They do not rule out other datasets or a more complex removal/addition
scheme. Such a scheme still needs equivalence and combined peak-memory evidence before design.
