# From the worktree root (branch experiment/r3-region-widths, base 3eb78f7 + base-codex-crates.patch + spike-source.patch).
cargo build -p viewer_render_wgpu --example region_width_spike --profile perf --offline
for d in belvedere majadas; do
  /usr/bin/time -v target/perf/examples/region_width_spike target/render-data/$d-v75.oct > $d.tsv 2> $d.time
done
python3 -I analyze.py belvedere majadas
