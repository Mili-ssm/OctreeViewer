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
only (gray), then times accumulation plus finish. It prepares the same unique blocks outside
the timer and keeps them resident, capped at 256 MiB of `PointBlock::estimated_memory_bytes()`.
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
