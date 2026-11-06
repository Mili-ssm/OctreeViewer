# From the repository root on feat/r3-v75 (see identity.txt).
cargo build -p viewer_render_wgpu --example coverage_spike --example cell_pool_bench --profile perf --offline
for d in belvedere majadas; do target/perf/examples/coverage_spike target/render-data/$d-v75.oct 0 --export-pool target/r3-t7c-pool; done
target/perf/examples/cell_pool_bench --out target/r3-t7c/bench.json target/r3-t7c-pool > target/r3-t7c/bench.txt
