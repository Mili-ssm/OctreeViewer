#!/usr/bin/env bash
# Usage: driver.sh   (spike matrix, sequential; writes driver.log). Step 0 runs the spike binary with
# splitting disabled (OCTREE_SPIKE_SPLIT_POINTS huge), so it measures the unchanged root-range behaviour.
set -u
D=$(cd "$(dirname "$0")" && pwd)
ROOT=$(git -C "$D" rev-parse --show-toplevel)
DS=/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets
B=$DS/belv_2009_digital-aerial_pcd_copc.laz
M=$DS/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz
S=$ROOT/target/t2scratch
mkdir -p "$S"
export OCTREE_SPIKE_STATS=1
run() { echo "$(date +%T) $1 load=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$D/driver.log"; BIN=$S/bin_spike "$D/run.sh" "$@"; sleep 20; }
off() { OCTREE_SPIKE_SPLIT_POINTS=1000000000000 run "$@"; }
for r in 1 2 3; do
  off s0-belv-w1-r4-r$r $B $S/w 1 4
  off s0-belv-w1-r8-r$r $B $S/w 1 8
done
off s0-maj-w8-r8-r1 $M $S/w 8 8
run sp-maj-w8-r1 $M $S/w 8 4
run sp-maj-w4-r1 $M $S/w 4 4
run sp-maj-w16-r1 $M $S/w 16 4
for r in 1 2 3; do run sp-belv-w1-r$r $B $S/w 1 4; done
for r in 1 2 3; do run sp-belv-w8-r$r $B $S/w 8 4; done
echo finished > "$D/driver.done"
