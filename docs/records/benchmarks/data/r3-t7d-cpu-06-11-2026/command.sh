# From the repository root on feat/r3-v75 (see identity.txt). Pools: see R3_T7C_NATIVE_06-11-2026 command.sh.
cargo build -p viewer_render_wgpu --example cell_pool_bench --profile perf --offline
target/perf/examples/cell_pool_bench --out target/r3-t7d/bench.json target/r3-t7c-pool > target/r3-t7d/bench.txt
