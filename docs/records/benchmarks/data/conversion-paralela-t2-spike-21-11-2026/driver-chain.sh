#!/usr/bin/env bash
# Usage: driver-chain.sh   (chained-segment variant fc19b56, OCTREE_SPIKE_CHAIN=1; writes driver.log)
# Round 1: Majadas W=8, W=4. Round 2: Belvedere W=8 x3, W=1 x1, Majadas W=8 replicate.
set -u
D=$(cd "$(dirname "$0")" && pwd)
ROOT=$(git -C "$D" rev-parse --show-toplevel)
M=/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz
S=$ROOT/target/t2scratch
export OCTREE_SPIKE_STATS=1 OCTREE_SPIKE_CHAIN=1
run() { sleep 60; echo "$(date +%T) $1 load=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$D/driver.log"; BIN=$S/bin_chain "$D/run.sh" "$@"; }
B=/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets/belv_2009_digital-aerial_pcd_copc.laz
if [ "${ROUND:-1}" = 1 ]; then
  run ch-maj-w8-r1 $M $S/w 8 4
  run ch-maj-w4-r1 $M $S/w 4 4
else
  for r in 1 2 3; do run ch-belv-w8-r$r $B $S/w 8 4; done
  run ch-belv-w1-r1 $B $S/w 1 4
  run ch-maj-w8-r2 $M $S/w 8 4
fi
echo finished > "$D/driver-chain.done"
