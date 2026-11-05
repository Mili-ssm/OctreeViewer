# From the repository root on feat/r3-v75 (base in identity.txt) with source.patch applied.
cargo build -p viewer_render_wgpu --example node_cells_bench --profile perf --offline
for d in belvedere majadas; do /usr/bin/time -v target/perf/examples/node_cells_bench target/render-data/$d-v75.oct 20 > $d.txt 2> $d.time; done
