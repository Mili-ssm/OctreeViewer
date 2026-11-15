#!/usr/bin/env bash
# Runs the 9 cells_bench cases of the R3 Task 9 record; outputs are overwritten in place.
set -u
cd "$(git rev-parse --show-toplevel)"
out=docs/records/benchmarks/data/r3-t9-cells-06-11-2026
for c in "majadas py 4" "majadas px 4" "majadas pz 4" "majadas orbit 4" "majadas px 1" \
         "majadas orbit 1" "belvedere py 4" "belvedere px 1" "belvedere orbit 4"; do
  set -- $c
  name="$1-$2-${3}px"
  BENCH_DATASET=$1 BENCH_POSE=$2 BENCH_PX=$3 BENCH_TIMEOUT_S=1500 timeout 1700 \
    cargo test --profile perf -p viewer_wasm --offline --lib cells_bench -- --ignored --nocapture \
    2>&1 | grep -vE '^\s*(Compiling|Finished|Running)' | sed -e :a -e "/^\n*$/{\$d;N;ba" -e "}" > "$out/$name.txt"
  cp "target/r3-t9c2b-$name.json" "$out/$name.json" || echo "missing json $name"
done
echo DONE
