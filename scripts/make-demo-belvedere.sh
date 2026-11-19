#!/usr/bin/env bash
# Builds the second web-demo dataset: an XY crop of the Belvedere Glacier LAZ (CC BY 4.0,
# Ioli et al., https://doi.org/10.5281/zenodo.19265915; see data/datasets/README.md), converted
# with the converter defaults (final v7, as sample.oct). Output (not committed):
#   target/demo-data/belvedere-crop.{laz,oct,octattrs}
# The window (EPSG:32632 metres) covers relief, moraine edges and vegetation borders, ~1.3 M points.
#   scripts/make-demo-belvedere.sh [INPUT.laz]
set -euo pipefail
cd "$(dirname "$0")/.."

src="${1:-data/datasets/belv_2009_digital-aerial_pcd_copc.laz}"
out=target/demo-data
xmin=417100 xmax=417700 ymin=5087300 ymax=5087900
[ -f "$src" ] || { echo "input missing: $src" >&2; exit 1; }
mkdir -p "$out"

cargo build -p viewer_convert --release --offline
rm -f "$out"/belvedere-crop.{laz,oct,octattrs}

echo "window: x [$xmin, $xmax] y [$ymin, $ymax]"
uv run --with 'laspy[lazrs]' python -I - "$src" "$out/belvedere-crop.laz" $xmin $xmax $ymin $ymax <<'PY'
import sys, laspy, numpy as np
src, dst = sys.argv[1], sys.argv[2]
x0, x1, y0, y1 = map(float, sys.argv[3:7])
kept = 0
with laspy.open(src) as r:
    # laspy cannot write COPC: keep the point format and scales, drop the COPC VLRs.
    hdr = laspy.LasHeader(version=r.header.version, point_format=r.header.point_format)
    hdr.scales, hdr.offsets = r.header.scales, r.header.offsets
    w = laspy.open(dst, mode="w", header=hdr)
    for p in r.chunk_iterator(4_000_000):
        m = (p.x >= x0) & (p.x < x1) & (p.y >= y0) & (p.y < y1)
        if m.any():
            w.write_points(p[m])
            kept += int(m.sum())
    w.close()
if kept == 0:
    sys.exit("empty crop")
print(f"crop points: {kept}")
PY

target/release/viewer_convert "$out/belvedere-crop.laz" "$out/belvedere-crop.oct"
target/release/viewer_convert verify "$out/belvedere-crop.oct"

total=$(( $(stat -c %s "$out/belvedere-crop.oct") + $(stat -c %s "$out/belvedere-crop.octattrs") ))
ls -l "$out"
echo "oct+octattrs: $total bytes"
[ "$total" -le $((40 * 1024 * 1024)) ] || { echo "over the 40 MB target" >&2; exit 1; }
