#!/bin/bash
# Run from repo root. s server on 18767 (target/web-root), o3 on 18768 (target/web-root-o3).
O=docs/records/benchmarks/data/explore-opt-level-wasm-02-11-2026
python3 -m http.server 18767 --bind 127.0.0.1 --directory target/web-root > $O/server-s.log 2>&1 &
S1=$!
python3 -m http.server 18768 --bind 127.0.0.1 --directory target/web-root-o3 > $O/server-o3.log 2>&1 &
S2=$!
sleep 2
for r in 1 2 3; do
  for b in s:18767 o3:18768; do
    n=${b%%:*}; p=${b#*:}
    uptime > $O/belvedere-v74-8-$n-r$r.uptime.txt
    PORT=$p python3 $O/run-sampled.py belvedere-v74 8 300 belvedere-v74-8-$n-r$r
    echo "$n r$r exit $?" >> $O/matrix-progress.txt
  done
done
kill $S1 $S2
echo finished >> $O/matrix-progress.txt
