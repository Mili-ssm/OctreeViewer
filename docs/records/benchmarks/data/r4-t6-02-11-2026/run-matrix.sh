#!/bin/bash
# Run from the repository root. Server serves target/web-root on 18767.
O=docs/records/benchmarks/data/r4-t6-02-11-2026
python3 -m http.server 18767 --bind 127.0.0.1 --directory target/web-root > $O/server.log 2>&1 &
SRV=$!
sleep 2
run() { python3 $O/run-sampled.py "$@"; echo "$3 exit $?" >> $O/matrix-progress.txt; }
for s in 1 2 3; do run majadas-v74 4 180 majadas-v74-4-s$s; done
run majadas-v74 4 180 majadas-v74-4-pixels
for s in 1 2 3; do run belvedere-v74 8 300 belvedere-v74-8-s$s; done
run belvedere-v74 8 300 belvedere-v74-8-pixels
for s in 1 2 3; do run belvedere-v74 4 300 belvedere-v74-4-s$s; done
kill $SRV
echo finished >> $O/matrix-progress.txt
