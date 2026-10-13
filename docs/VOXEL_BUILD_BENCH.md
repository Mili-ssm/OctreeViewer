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

### Albuera repetition with the same instrumented WASM

Three fresh Firefox profiles repeated the same coordinate-only protocol and budgets on Albuera.
All six phases reached `target_complete`, zero pending blocks and zero console errors. Raw
exports: ignored `target/prefix-opportunity-albuera/run-{1,2,3}/`. The WASM hash matches the
Majadas measurement above. The orchestration wrapper initially failed its final hash-recording
step because it used an incorrect package path; all probes and summary files had already
finished successfully. Hash recording was corrected to the served `pkg/viewer_wasm_bg.wasm`.

| Initial phase | Round 1 | Round 2 | Round 3 |
| --- | ---: | ---: | ---: |
| Full builds / reused updates | 233 / 513 | 233 / 513 | 233 / 513 |
| Strict-prefix builds | 3 | 3 | 3 |
| Retained / added source points | 140,949 / 228,274 | 140,949 / 228,274 | 140,949 / 228,274 |
| Candidate full-build time (ms) | 33 | 32 | 24 |
| All full-build time (ms) | 75,371 | 75,280 | 78,387 |
| First terminal observation (s, sampled every 10 s) | 100.135 | 100.139 | 130.185 |

All initial phases loaded 746 blocks, processed 503,187,945 source points across full rebuilds
and finished with 17,085 voxels. Each camera phase added 12 loads, six full builds and eight
reused updates, retained 17,085 voxels, and added no strict-prefix opportunity. Initial candidate
work remains below 0.05% of full-build time. The slower third terminal observation is retained;
these are structural opportunity measurements, not paired speedup evidence. Neither measured
cloud supports introducing a persistent append-only accumulator yet. Belvedere and Additive
cases, combined peak memory and Chrome visual validation remain pending.

### Belvedere at the default cache budget

Three fresh Firefox 157.0.1 profiles repeat the coordinate-only probe at 1280×720,
4 px and a 256 MiB cache, using the same instrumented WASM hash above. Raw exports:
ignored `target/prefix-opportunity-belvedere/run-{1,2,3}/`; provenance accompanies them.
OS caches were not flushed. Native fixture compilation/checks overlapped parts of the first
two rounds; these observations measure structural opportunities, not comparable speedups.
All six phases paused under cache pressure with zero browser console errors. Initial/camera
exports retain coverage of the visible cut, with 782/780 requested blocks still pending;
this does not complete the requested 4 px target.

| Initial phase | Round 1 | Round 2 | Round 3 |
| --- | ---: | ---: | ---: |
| Full builds / reused updates | 156 / 350 | 156 / 350 | 156 / 350 |
| Strict-prefix builds | 4 | 4 | 4 |
| Retained / added source points | 5,650 / 134,576 | 5,650 / 134,576 | 5,650 / 134,576 |
| Candidate full-build time (ms) | 21 | 19 | 18 |
| All full-build time (ms) | 100,140 | 97,786 | 95,049 |

Every initial observation loads 507 blocks and processes 548,864,502 source points across
full rebuilds. Camera phases add 179 loads, 74 full builds and 104 reused updates, but zero
prefix opportunities; final visible voxels change from 20,977 to 20,707. Prefix candidates
account for less than 0.03% of initial full-build time. A persistent append-only accumulator
is not justified by this bounded trajectory either. This result does not cover a fully
resident final cut or the unfinished 1 GiB target; cache pressure and the full rebuild cost
remain separate problems. Combined peak memory and Additive workloads still need evidence.

### Deterministic Additive corpus

Generate a local EPT source with 73 nodes (complete depths 0–2) and 512 points per node,
37,376 points total. Each payload is LASzip format 0; grids stay inside their node cells and
use a different offset at each depth. This synthetic corpus exercises retained ancestor
payloads and progressive descendant loading; its timings do not represent real LiDAR.
Existing output directories are rejected. Two generated trees were byte-identical (75 files).

```bash
cargo run --offline -p viewer_convert --example generate_additive_fixture -- target/additive-prefix-ept
cargo run --release --offline -p viewer_convert -- target/additive-prefix-ept target/additive-prefix-v5.oct
node examples/web-host/probe-cache-camera.mjs target/additive-prefix-v5.oct target/additive-prefix-run
```

Use a fresh dedicated Firefox profile per probe, with the root HTTP server and WebGPU settings
from the host README. Conversion validates all 37,376 output points. Coordinate-only browser
measurement uses the same default 4 px target, 256 MiB cache and 1280×720 framebuffer.

Three fresh system Firefox 157.0.1 profiles completed all six phases with zero pending blocks,
full coverage, 37,376 final voxels and zero console errors. The served WASM hash matches the
preceding prefix runs. Raw exports: ignored `target/prefix-opportunity-additive-v5/run-{1,2,3}/`.

| Initial phase | Round 1 | Round 2 | Round 3 |
| --- | ---: | ---: | ---: |
| Full builds / reused updates | 75 / 0 | 75 / 0 | 75 / 0 |
| Strict-prefix builds | 2 | 2 | 2 |
| Retained / added source points | 5,120 / 1,024 | 5,120 / 1,024 | 5,120 / 1,024 |
| Candidate full-build time (ms) | 35 | 35 | 33 |
| All full-build time (ms) | 5,524 | 6,247 | 4,594 |

Initial phases load all 73 blocks and process 1,457,664 source points across rebuilds. Camera
adds three full builds and 112,128 processed points, with no new prefix candidates or loads.
Candidate work is below 0.8% of initial rebuild time. Additive semantics alone do not imply a
strict append-only traversal. This corpus does not justify persistent accumulation; broader
Additive sources and combined peak memory remain open. These are structural opportunity
measurements, not paired performance evidence; Chrome checks overlapped parts of the runs.

An initial failed attempt used a stale prebuilt converter and produced v3, rejected by the v5
browser. The probe then failed while reading the absent scene. Rebuilding the release converter
and writing a distinct v5 output resolved the input problem; the failed attempt is retained
under `target/prefix-opportunity-additive/run-1/` and is excluded from the three valid rounds.
Always invoke `cargo run` as above when reproducing with the current checkout.

Playwright MCP also exercised Chrome 155/SwiftShader file opening, camera input and desktop/mobile
viewports (1688×768 and 390×844), without application console errors. The status reports target
completion, but the visible statistics remain at an earlier 16 px cut and the screenshots have
a white canvas, matching the unresolved Chrome presentation limitation. A DOM-completion wait
therefore timed out; this is partial functional evidence, not a visual pass or camera/resize
completion. A separate bundled Playwright Firefox attempt also timed out and is not counted
among the successful system-Firefox probes. Screenshots are under ignored
`target/additive-playwright-{desktop,mobile}.png`. Chrome presentation and current panel
statistics need a dedicated reproduction before attributing the cause to this dataset or HZB.

## Joint native construction memory diagnostic — 13-10-2026

The existing fixture retains its first scene for equality checks while constructing later
scenes. Each sample now reports that retained instance capacity and the checked sum of
`decoded_bytes_accounted + previous_scene_capacity_bytes + estimated_peak_bytes`.
The first sample has no previous scene; subsequent samples retain exactly the first one.
This combines accounted payload with the builder's conservative peak model, including its
projection-cache reservation. It is not an observed simultaneous allocation peak. It excludes
index/reader allocations, allocator overhead, vector headers, GPU and browser state.
No persistent accumulator or product memory budget was introduced.

Linux `/proc/self/status` snapshots before opening, after decoding, and immediately before
and after each timed build report RSS and process-lifetime HWM in bytes. Missing/unsupported
counters are JSON null. HWM includes loading/decompression and earlier builds; boundary RSS
can miss transient peaks. There is no sampler thread, and snapshots are outside build timing.
This native fixture cannot establish the progressive WASM or GPU peak.

Native **perf** profile, same Ryzen AI 9 365/Linux host, fitted 1280×720 camera, coordinates
only, OS cache not flushed. Three fresh processes per case, three builds per process; no
compilation, tests or browser probe overlap the series. Byte counts below are decimal bytes.

| Case | Decoded accounted | Builder peak model | Retained instances | Combined model, builds 2–3 | Process HWM range |
| --- | ---: | ---: | ---: | ---: | ---: |
| Outer Banks 4 px | 1,779,096 | 34,017,120 | 96,400 | 35,892,616 | 6,098,944–6,311,936 |
| Majadas 4 px | 112,258,496 | 34,782,624 | 255,880 | 147,297,000 | 134,864,896–135,237,632 |
| Belvedere 8 px | 211,835,216 | 34,541,280 | 205,600 | 246,582,096 | 225,112,064–225,366,016 |

All nine builds per case preserve the existing scene checksums respectively
`ebb46abe0106a53f`, `563a1f928baa5a75`, `c057ef2774b0ca75`; within-process equality
also checks scene bytes, edges, oversized cells and builder memory stats. The large difference
for the small fixture illustrates why charged cache reservations cannot be labeled RSS.
Raw snapshots and summary: ignored `target/voxel-joint-memory/`.

Reproduce each case with the existing command, three times in fresh processes:

```sh
cargo build --profile perf --offline -p viewer_render_wgpu --example voxel_build_bench
target/perf/examples/voxel_build_bench data/samples/fs_e430_n4008_18.oct 3 256 4
target/perf/examples/voxel_build_bench data/datasets/sensoforest/majadas-tls.oct 3 256 4
target/perf/examples/voxel_build_bench data/datasets/belv_2009_digital-aerial_pcd_copc.oct 3 256 8
```

Validation: renderer tests **58 passed / 2 ignored**, renderer all-target Clippy with
warnings denied and formatting. Remaining: progressive browser CPU/GPU peak and Additive
real corpus, before deciding whether retained incremental state is worthwhile.

## Optional native RSS sampling — 13-10-2026

Assessment: the existing combined estimate adds decoded block accounting, the retained first
scene and the conservative builder model. Boundary RSS snapshots can miss allocations during
accumulation/finish. An optional sampler now observes process RSS across construction without
changing production rendering or retaining an accumulator. No sampling thread runs by default.

```sh
cargo build --profile perf --offline -p viewer_render_wgpu --example voxel_build_bench
target/perf/examples/voxel_build_bench data/samples/fs_e430_n4008_18.oct 3 256 4 0
target/perf/examples/voxel_build_bench data/samples/fs_e430_n4008_18.oct 3 256 4 1
target/perf/examples/voxel_build_bench data/datasets/sensoforest/majadas-tls.oct 3 256 4 1
```

The fifth argument is `RSS_SAMPLE_MS`: 0 disables it; integers 1–1000 request a sampling interval.
`process_memory.samples[].sampled_rss` is null when disabled; otherwise it contains
`peak_observed_rss_bytes` and `successful_samples`. Linux `/proc/self/status` supplies RSS;
unavailable RSS produces null peak and zero successful samples. Initial/final observations
count as samples, even if the build completes before the first periodic observation. The
thread stops via a channel and is joined after success or a returned build error, before the
error is propagated. There is no wait for the full interval on shutdown.

The worker observes process-wide memory while decoded blocks, builder state, and (after the
first run) the retained first scene coexist. These objects change during construction: the
sampler does not attribute RSS to individual components or prove their individual maxima
coincide. It includes reader/index/allocator and sampling overhead. Short peaks may be missed;
scheduling and `/proc` granularity make the requested interval approximate. The existing HWM
remains process-lifetime data including decode. Neither value measures WASM/JS heap or GPU.
`timings_include_sampler_overhead` marks perturbed runs; use disabled sampling for CPU timing
comparisons. This bounded resident-input fixture does not simulate browser cache eviction.

Validation in this checkout:

- Small fixture: intervals 0, 1 and 1000 ms, three runs each, all produce 2,410 voxels and
  checksum `ebb46abe0106a53f`; disabled output is null; enabled Linux output has at least the
  two boundary samples. 1001, -1 and nonnumeric intervals fail.
- Majadas at 1 ms: 202 blocks, 7,015,449 points, 6,397 voxels, checksum `563a1f928baa5a75`.
  Three observed RSS maxima: 135,254,016 / 135,454,720 / 135,454,720 bytes;
  267 / 276 / 272 successful samples. Decoded accounting: 112,258,496 bytes;
  builder peak model: 34,782,624 bytes; previous scene capacity in runs 2–3: 255,880 bytes.
  Combined model: 147,041,120 bytes initially, 147,297,000 thereafter. The model and RSS have
  different scopes; their difference is not an allocation saving or model accuracy bound.
- Optimized offline build, example Clippy with warnings denied, formatting and diff checks pass.
  Renderer library tests: 60 pass, two optional tests ignored.

Follow-up remains: browser JS/WASM attribution, real Additive corpus and a controlled lifetime
comparison before considering a persistent accumulator. This diagnostic establishes an observed
native process peak, not evidence that persistent reuse is beneficial.
