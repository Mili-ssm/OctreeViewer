#!/bin/sh
set -C
R3_PHASED_TIMING=1 /home/mili/.cache/codex-worktrees/octree-r3-adaptive/target/r3-layout-lab/target/perf/r3-layout-lab /home/mili/.cache/codex-worktrees/octree-r3-v75/target/render-data/majadas-v75.oct /home/mili/.cache/codex-worktrees/octree-r3-v75/target/render-data/belvedere-v75.oct > /tmp/r3-width-lab-fast48-05-11-2026/raw.txt 2> /tmp/r3-width-lab-fast48-05-11-2026/stderr.txt
status=$?
printf '%s\n' "$status" > /tmp/r3-width-lab-fast48-05-11-2026/exit.txt
exit "$status"
