#!/usr/bin/env bash
# Usage: driver.sh   (runs the whole task-0 CLI matrix sequentially; writes driver.log; the driver.done end marker is not kept)
set -u
D=$(cd "$(dirname "$0")" && pwd)
ROOT=$(git -C "$D" rev-parse --show-toplevel)
DS=/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets
B=$DS/belv_2009_digital-aerial_pcd_copc.laz
M=$DS/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz
BT=$ROOT/target/t0scratch
run() { echo "$(date +%T) $1 load=$(cut -d' ' -f1-3 /proc/loadavg)" >> "$D/driver.log"; "$D/run.sh" "$@"; }
for r in 1 2 3; do
  run belv-w1r4-btrfs-r$r $B $BT/btrfs 1 4
  run belv-w1r0-btrfs-r$r $B $BT/btrfs 1 0
  run belv-w4r4-btrfs-r$r $B $BT/btrfs 4 4
  run belv-w8r4-btrfs-r$r $B $BT/btrfs 8 4
  run belv-w4r4-shm-r$r   $B /dev/shm/t0work 4 4
  run belv-w4r4-nocomp-r$r $B $BT/nocomp/w 4 4
done
run maj-w4r4-btrfs-r1 $M $BT/btrfs 4 4
run maj-w8r4-btrfs-r1 $M $BT/btrfs 8 4
run maj-w4r4-nocomp-r1 $M $BT/nocomp/w 4 4
run maj-w1r4-btrfs-r1 $M $BT/btrfs 1 4
run maj-w1r0-btrfs-r1 $M $BT/btrfs 1 0
avail=$(awk '/MemAvailable/{print int($2/1048576)}' /proc/meminfo)
if [ "$avail" -ge 16 ]; then run maj-w4r4-shm-r1 $M /dev/shm/t0work 4 4; else echo "skipped maj shm: MemAvailable=${avail}GiB" >> "$D/driver.log"; fi
echo finished > "$D/driver.done"
