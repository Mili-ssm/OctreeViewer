#!/usr/bin/env bash
# usage: run.sh LABEL DIR
cd $REPO
export CARGO_TARGET_DIR=$REPO/target RENDER_DATA_DIR=$2 R3_REQUIRE_REAL_DATA=1
T=$(ls -t target/perf/build/viewer_wasm/*/out/viewer_wasm-* | grep -v '\.d$' | head -1)
OUT=$REPO/.scratch-logs/test-$1; mkdir -p $OUT
for t in cells_tests::multi_owner_blocks_in_the_v8_datasets cells_stream_tests::bench::majadas_at_1px cells_stream_tests::bench::cells_bench "cells_stream_tests::real_belvedere" "cells_stream_tests::sim::" cells_stream_tests::data_bounded cells_stream_tests::eye_node cells_stream_tests::p4_border cells_stream_tests::synthetic_zoom cells_stream_tests::the_node_hi cells_stream_tests::the_node_lo cells_stream_tests::twin_wanted; do
  n=$(echo "$t" | tr -c 'a-zA-Z0-9\n' _)
  timeout 1800 $T --ignored --nocapture --test-threads=1 "$t" > $OUT/$n.log 2>&1; echo "$t exit $?" >> $OUT/summary.txt
done
echo ALLDONE >> $OUT/summary.txt
