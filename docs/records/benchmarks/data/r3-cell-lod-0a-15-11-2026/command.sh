# From the repository root on explore/cell-lod-spike (commit and source hash in identity.txt).
cargo build --offline --profile perf -p viewer_render_wgpu --example coverage_spike
for d in belvedere majadas; do
  /usr/bin/time -v target/perf/examples/coverage_spike target/render-data/$d-v7.oct --cell-lod > $d.tsv 2> $d.err
done
# equivalent: cargo run --offline --profile perf -p viewer_render_wgpu --example coverage_spike -- FILE --cell-lod
