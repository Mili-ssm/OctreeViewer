# From the repository root on feat/r3-v75 (see identity.txt).
D=docs/records/benchmarks/data/r3-t6-accept-05-11-2026
CELL_PAGES_REQUIRE_GPU=1 cargo test -p viewer_render_wgpu --offline cell_pages -- --nocapture   # cell-pages-tests.txt (lines starting with #)
cargo build -p viewer_render_wgpu --example coverage_spike --profile perf --offline
for d in belvedere majadas; do target/perf/examples/coverage_spike target/render-data/$d-v75.oct 0 --export target/r3-t6-accept --check-cell-pages; done   # check-cell-pages.txt (lines starting with "# check-cell-pages")
for c in $(ls target/r3-t16-cases); do diff <(cd target/r3-t16-cases/$c && sha256sum *) <(cd target/r3-t6-accept/$c && sha256sum *) > /dev/null && echo "same $c" || echo "DIFF $c"; done   # export-recheck.txt
mkdir -p /tmp/fmaprobe/src && cp $D/fma-probe/main.rs /tmp/fmaprobe/src/ && cp $D/fma-probe/Cargo.toml.txt /tmp/fmaprobe/Cargo.toml && (cd /tmp/fmaprobe && cargo run --release)   # output.txt
