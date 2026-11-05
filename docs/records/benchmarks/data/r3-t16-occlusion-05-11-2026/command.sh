# From the repository root on feat/r3-v75 (see identity.txt). Datasets as in R3_GPU_SPIKE_05-11-2026.
D=docs/records/benchmarks/data/r3-t16-occlusion-05-11-2026
cargo build -p viewer_render_wgpu --example coverage_spike --profile perf --offline
for d in belvedere majadas; do target/perf/examples/coverage_spike target/render-data/$d-v75.oct 0 --export target/r3-t16-cases; done
export CHROME_BIN=/usr/bin/google-chrome
node examples/web-host/probe-cube-pool.mjs target/r3-t16-gpu/check3.json target/r3-t16-cases/* --t16-check
node examples/web-host/probe-cube-pool.mjs target/r3-t16-gpu/final.json target/r3-t16-cases/* --t16 --rounds=3
gzip -9 -c target/r3-t16-gpu/final.json > $D/final.json.gz
gzip -9 -c target/r3-t16-gpu/check3.json > $D/check.json.gz
python3 -I $D/analyze.py $D/final.json.gz $D/check.json.gz > $D/summary.txt
# Reproduction check: re-export with the final binary and compare every exported file (export-recheck.txt).
for d in belvedere majadas; do target/perf/examples/coverage_spike target/render-data/$d-v75.oct 0 --export target/r3-t16-recheck; done
for c in $(ls target/r3-t16-cases); do diff <(cd target/r3-t16-cases/$c && sha256sum *) <(cd target/r3-t16-recheck/$c && sha256sum *) > /dev/null && echo "same $c" || echo "DIFF $c"; done
