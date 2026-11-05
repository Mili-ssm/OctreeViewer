# From the repository root on feat/r3-v75 (commit and source hash in identity.txt).
cargo build -p viewer_render_wgpu --example coverage_spike --profile perf --offline
for d in belvedere majadas; do /usr/bin/time -v target/perf/examples/coverage_spike target/render-data/$d-v75.oct > $d.tsv 2> $d.time; done
