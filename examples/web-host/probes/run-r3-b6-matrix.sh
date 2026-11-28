#!/usr/bin/env bash
# RENDER_DATA_DIR (default target/render-data; use an absolute path from elsewhere) holds belvedere-v8.oct and majadas-v8.oct.
# R3 Task 17 (B6) measurement matrix: CPU path (cells=0) vs cells path (cells=1), both with the build
# worker, on Belvedere and Majadas (V8), local file and RED-REF HTTP Range. Run from the repository
# root with pkg/ built and Chrome with WebGPU. Outputs go to target/r3-b6/ (JSON per run, one log).
#   timing   : R0 harness, 10 s orbit during load and after the target (RQ-01, RQ-05, GTT, GPU time)
#   pixels   : same with orbit screenshots and stats samples (RQ-02 empty pixels; perturbs RQ-01 on purpose)
#   rest     : no orbit during load; time to "reached" at the fitted pose
set -u
OUT=${OUT:-target/r3-b6}
DATA=${RENDER_DATA_DIR:-target/render-data}
mkdir -p "$OUT"
PORT=8767
# Only processes started here are stopped: the probe's browser (Playwright, its own temporary profile)
# and RED-REF server die with the probe, and the static server by its PID. The owner's Chrome is never touched.
STATIC_PID=
PROBE_PID=
cleanup() {
  [ -n "$PROBE_PID" ] && kill "$PROBE_PID" 2>/dev/null
  [ -n "$STATIC_PID" ] && kill "$STATIC_PID" 2>/dev/null
  return 0
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT TERM
if ! curl -sf -o /dev/null "http://127.0.0.1:$PORT/pkg/viewer_wasm_bg.wasm"; then
  python3 -m http.server "$PORT" --bind 127.0.0.1 >"$OUT/static.log" 2>&1 &
  STATIC_PID=$!
  echo "$STATIC_PID" >"$OUT/static.pid"
  sleep 1
fi
run() { # name outer-timeout-s probe-args...
  local name=$1 limit=$2; shift 2
  echo "== $(date +%T) $name"
  # The probe marks the run "functional" when a foreign Chrome is running; it does not kill it.
  timeout "$limit" node examples/web-host/probes/probe-orbit-load.mjs "$@" "$OUT/$name.json" >"$OUT/$name.log" 2>&1 &
  PROBE_PID=$!
  wait "$PROBE_PID"
  PROBE_PID=
  tail -2 "$OUT/$name.log"
}
arm() { [ "$1" = cpu ] && echo 0 || echo 1; }
for kind in ${KINDS:-timing pixels rest}; do
  extra=()
  [ "$kind" = pixels ] && extra=(--pixels)
  [ "$kind" = rest ] && extra=(--rest-first)
  for ds in majadas belvedere; do
    for a in cpu cells; do
      run "$kind-$ds-$a-4px-local" 400 "$DATA/$ds-v8.oct" --px=4 --cells="$(arm $a)" --max-load-s=120 --pixels=final "${extra[@]}"
    done
  done
  if [ "$kind" != pixels ]; then
    for a in cpu cells; do
      run "$kind-majadas-$a-4px-remote" 500 "$DATA/majadas-v8.oct" --px=4 --cells="$(arm $a)" --max-load-s=180 --remote --pixels=final "${extra[@]}"
    done
    run "$kind-majadas-cells-1px-local" 500 "$DATA/majadas-v8.oct" --px=1 --cells=1 --max-load-s=240 --pixels=final "${extra[@]}"
  fi
done
echo "== done $(date +%T)"
