# From the repository root on feat/r3-v75 (see identity.txt).
cargo build -p viewer_render_wgpu --example coverage_spike --profile perf --offline
for d in belvedere majadas; do target/perf/examples/coverage_spike target/render-data/$d-v75.oct 0 --export target/r3-t5-cases; done
node examples/web-host/probe-cube-pool.mjs target/r3-t5-gpu/main.json target/r3-t5-cases/{belvedere,majadas}-{orbit-0,grazing}-{1,4}px target/r3-t5-cases/belvedere-inside-leaf-{1,4}px target/r3-t5-cases/majadas-inside-leaf-4px --rounds=3
node examples/web-host/probe-cube-pool.mjs target/r3-t5-gpu/load.json target/r3-t5-cases/majadas-inside-leaf-1px target/r3-t5-cases/belvedere-inside-leaf-4px --rounds=3
node examples/web-host/probe-cube-pool.mjs target/r3-t5-gpu/diag.json target/r3-t5-cases/* --diag
python3 -I docs/records/benchmarks/data/r3-gpu-spike-05-11-2026/analyze.py main.json.gz load.json.gz
