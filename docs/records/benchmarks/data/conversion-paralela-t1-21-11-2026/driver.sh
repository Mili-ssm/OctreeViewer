#!/usr/bin/env bash
# Usage: driver.sh   (task-1b matrix, sequential; writes driver.log; driver.done marker is not kept)
set -u
D=$(cd "$(dirname "$0")" && pwd)
ROOT=$(git -C "$D" rev-parse --show-toplevel)
DS=/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets
B=$DS/belv_2009_digital-aerial_pcd_copc.laz
M=$DS/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz
S=$ROOT/target/t1scratch
run() { bin=$1; shift; echo "$(date +%T) $1 load=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$D/driver.log"; BIN=$S/$bin "$D/run.sh" "$@"; }
for r in 1 2 3 4 5; do
  if [ $((r % 2)) -eq 1 ]; then o="A B"; else o="B A"; fi
  for v in $o; do run bin_$v belv-w1-$v-r$r $B $S/w 1 4; done
done
for r in 1 2 3; do
  for w in 4 8 16; do run bin_B belv-w$w-B-r$r $B $S/w $w 4; done
done
for w in 1 4 8 16; do run bin_B maj-w$w-B-r1 $M $S/w $w 4; done
echo finished > "$D/driver.done"
