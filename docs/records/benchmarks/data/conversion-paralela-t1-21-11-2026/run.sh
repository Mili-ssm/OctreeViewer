#!/usr/bin/env bash
# Usage: run.sh LABEL INPUT.laz WORKDIR W RANGES   (one conversion, BIN env = binary; writes LABEL.{log.xz,tsv,meta})
set -u
D=$(cd "$(dirname "$0")" && pwd)
label=$1; input=$2; work=$3; w=$4; r=$5
BIN=${BIN:?set BIN}
mkdir -p "$work"
python3 -I "$D/sample.py" "$D/$label.tsv" & sp=$!
{ echo "load_before=$(cut -d' ' -f1-3 /proc/loadavg)"; echo "start=$(date +%s.%N)"; } > "$D/$label.meta"
timeout 1800 "$BIN" convert-profile "$input" "$work/out.oct" "$w" "$r" > "$D/$label.log" 2> "$D/$label.err"
echo "exit=$?" >> "$D/$label.meta"; echo "end=$(date +%s.%N)" >> "$D/$label.meta"
echo "sha=$(sha256sum "$work/out.oct" | cut -d' ' -f1)" >> "$D/$label.meta"
echo "output_bytes=$(stat -c %s "$work/out.oct")" >> "$D/$label.meta"
echo "attrs_sha=$(sha256sum "$work/out.octattrs" | cut -d" " -f1)" >> "$D/$label.meta"
kill $sp; wait $sp 2>/dev/null
xz -9f "$D/$label.log"   # kept compressed; analyze.py reads LABEL.log.xz
rm -rf "$work"
