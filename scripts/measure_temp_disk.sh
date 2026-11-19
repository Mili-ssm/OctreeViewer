#!/usr/bin/env bash
# Measures the converter's peak temp-disk use (RQ-06).
# Usage: scripts/measure_temp_disk.sh CONVERTER INPUT OUTPUT_DIR [-- EXTRA_ARGS...]
# Prints: peak_work_bytes=<n> peak_fs_used_delta_bytes=<n> wall_s=<s> max_rss_kb=<n> sha_oct=<hex> sha_octattrs=<hex>
# work_bytes is the APPARENT size (du -sb); on a compressed filesystem (btrfs compress=zstd) it overstates real
# use. fs_used_delta_bytes (df used now minus df used before the run) is a noisy proxy of real use: other
# writers on the same filesystem affect it.
# Samples (elapsed_s<TAB>work_bytes<TAB>fs_used_delta_bytes) go to OUTPUT_DIR/samples.tsv. The converter keeps its temps in
# `<output>.<pid>.<n>.work` next to the output, so OUTPUT_DIR/*.work is sampled once per second.
# The sampling loop ends with the converter process. Deletes .oct/.octattrs after hashing them.
# MIN_FREE_KB (default 60 GB) aborts the run if free space on OUTPUT_DIR drops below it.
set -euo pipefail

if [ "$#" -lt 3 ]; then
    echo "usage: $0 CONVERTER INPUT OUTPUT_DIR [-- EXTRA_ARGS...]" >&2
    exit 2
fi
converter=$1 input=$2 outdir=$3
shift 3
if [ "${1:-}" = "--" ]; then shift; fi
extra=("$@")
min_free_kb=${MIN_FREE_KB:-62914560}

mkdir -p "$outdir"
rm -f "$outdir"/out.oct "$outdir"/out.octattrs "$outdir"/samples.tsv
rm -rf "$outdir"/*.work
time_log="$outdir/time.log"
samples="$outdir/samples.tsv"
: >"$samples"

fs_used0=$(df -B1 --output=used "$outdir" | tail -1 | tr -d ' ')
start=$(date +%s.%N)
/usr/bin/time -v -o "$time_log" "$converter" "$input" "$outdir/out.oct" "${extra[@]}" &
pid=$!

peak=0
peak_delta=0
aborted=0
while kill -0 "$pid" 2>/dev/null; do
    bytes=0
    if compgen -G "$outdir/*.work" >/dev/null; then
        # Spool files vanish while du walks them: a sampler failure must never abort the script.
        bytes=$( { du -sb "$outdir"/*.work 2>/dev/null || true; } | awk '{s+=$1} END {print s+0}')
    fi
    elapsed=$(awk -v a="$start" -v b="$(date +%s.%N)" 'BEGIN {printf "%.1f", b-a}')
    used=$(df -B1 --output=used "$outdir" 2>/dev/null | tail -1 | tr -d ' ' || true)
    delta=$(( ${used:-$fs_used0} - fs_used0 ))
    printf '%s\t%s\t%s\n' "$elapsed" "$bytes" "$delta" >>"$samples"
    if [ "$bytes" -gt "$peak" ]; then peak=$bytes; fi
    if [ "$delta" -gt "$peak_delta" ]; then peak_delta=$delta; fi
    free_kb=$(df --output=avail -k "$outdir" | tail -1 | tr -d ' ')
    if [ "$free_kb" -lt "$min_free_kb" ]; then
        echo "free space ${free_kb} KB below ${min_free_kb} KB: aborting" >&2
        kill "$pid" 2>/dev/null || true
        aborted=1
        break
    fi
    sleep 1
done

status=0
wait "$pid" || status=$?
end=$(date +%s.%N)
if [ "$aborted" -ne 0 ] || [ "$status" -ne 0 ]; then
    echo "converter failed (status $status, aborted=$aborted)" >&2
    exit 1
fi

wall=$(awk -v a="$start" -v b="$end" 'BEGIN {printf "%.1f", b-a}')
rss=$(awk -F': ' '/Maximum resident set size/ {print $2}' "$time_log")
sha_oct=$(sha256sum "$outdir/out.oct" | cut -d' ' -f1)
sha_attrs=none
if [ -f "$outdir/out.octattrs" ]; then
    sha_attrs=$(sha256sum "$outdir/out.octattrs" | cut -d' ' -f1)
fi
rm -f "$outdir"/out.oct "$outdir"/out.octattrs
echo "peak_work_bytes=$peak peak_fs_used_delta_bytes=$peak_delta wall_s=$wall max_rss_kb=$rss sha_oct=$sha_oct sha_octattrs=$sha_attrs"
