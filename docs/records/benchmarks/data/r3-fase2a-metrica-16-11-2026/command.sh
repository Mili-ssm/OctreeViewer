# From the repository root on feat/cell-lod (commit and source hash in identity.txt).
# The spike exits non-zero at the end ("octant_mismatches": its own level_ok check against the
# library cut, unrelated to this measurement; see the record), so the exit status is ignored.
cargo build --offline --profile perf -p viewer_render_wgpu --example coverage_spike
DATA=/home/mili/.cache/codex-worktrees/octree-r3-v75/target/render-data
for d in belvedere majadas; do
  for m in bound edge distance; do
    /usr/bin/time -v -o $d-$m.time target/perf/examples/coverage_spike $DATA/$d-v7.oct \
      --cell-lod --residency r2 --px-only --metric $m --poses orbit-0,grazing,inside-leaf \
      > $d-$m.tsv 2> $d-$m.err || true
  done
done
# $d-$m.time keeps only the "Elapsed" and "Maximum resident" lines of /usr/bin/time -v; the
# .err files (progress on stderr) are not kept.
