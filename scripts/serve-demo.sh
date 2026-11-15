#!/usr/bin/env bash
# Builds the wasm bindings (unless --no-build) and serves the repository root for the web-host demo.
# Note: python's http.server does not implement HTTP Range, so `?url=` remote files need
# examples/web-host/red-ref-server.mjs instead (see examples/web-host/README.md).
set -euo pipefail
cd "$(dirname "$0")/.."

build=1
for arg in "$@"; do
  case "$arg" in
    --no-build) build=0 ;;
    -h|--help) echo "usage: scripts/serve-demo.sh [--no-build]   (PORT=8000 by default)"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

if [ "$build" = "1" ]; then
  scripts/build-wasm-bindings.sh
fi
[ -f pkg/viewer_wasm.js ] || { echo "pkg/viewer_wasm.js missing: run without --no-build" >&2; exit 1; }

port="${PORT:-8000}"
echo "Demo: http://127.0.0.1:${port}/examples/web-host/"
exec python3 -m http.server "$port" --bind 127.0.0.1
