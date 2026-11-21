#!/usr/bin/env bash
# Regenerates the retired-format fixtures: the 24-byte header (`<version>.head`) and the 40-byte
# footer (`<version>.foot`) of the `.oct` a converter that still writes that version produces from a
# small LAS. Needs a converter that still has the writers (before Task 5 of the V8 freeze, or an
# old revision); see README.md. Usage: generate.sh [path/to/viewer_convert]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../../../.." && pwd)"
las="$root/data/samples/fs_e430_n4008_18.las"
convert="${1:-}"
if [ -z "$convert" ]; then
    cargo build --release --offline -p viewer_convert --manifest-path "$root/Cargo.toml"
    convert="${CARGO_TARGET_DIR:-$root/target}/release/viewer_convert"
fi
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
for version in v5 v6.1-pre v6.2-pre v6.3-pre v6.4-pre v7.1-pre v7.2-pre v7.3-pre v7.4-pre v7.5-pre v7 v8.1-pre v8.2-pre; do
    "$convert" "$las" "$scratch/$version.oct" --format-version "$version" >/dev/null 2>&1
    head -c 24 "$scratch/$version.oct" >"$here/$version.head"
    tail -c 40 "$scratch/$version.oct" >"$here/$version.foot"
done
