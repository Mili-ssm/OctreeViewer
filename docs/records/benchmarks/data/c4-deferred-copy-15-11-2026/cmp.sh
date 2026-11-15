#!/bin/bash
D=/tmp/claude-1000/-home-mili-Documentos-Mili-ssm-github-io-Private-Rust-octree-viewer-v3/7943fd68-3be0-4343-954f-0d8e3d2503e0/scratchpad/c4
n=$1; shift
for i in $(seq $n); do for v in "$@"; do echo -n "$(basename $v) "; python3 -I $D/run.py $v $D/o_$(basename $v).oct; if [ -n "$KEEP" ]; then sha256sum $D/o_$(basename $v).oct* | cut -c1-16; fi; rm -rf $D/o_*; done; done
