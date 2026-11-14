#!/usr/bin/env bash
# R3 Task 17 (B6) measurement matrix: CPU path (cells=0) vs cells path (cells=1), both with the build
# worker, on Belvedere and Majadas v7.5, local file and RED-REF HTTP Range. Run from the repository
# root with pkg/ built and Chrome with WebGPU. Outputs go to target/r3-b6/ (JSON per run, one log).
#   timing   : R0 harness, 10 s orbit during load and after the target (RQ-01, RQ-05, GTT, GPU time)
#   pixels   : same with orbit screenshots and stats samples (RQ-02 empty pixels; perturbs RQ-01 on purpose)
#   rest     : no orbit during load; time to "reached" at the fitted pose
set -u
OUT=${OUT:-target/r3-b6}
DATA=target/render-data
mkdir -p "$OUT"
PORT=8767
if ! curl -sf -o /dev/null "http://127.0.0.1:$PORT/pkg/viewer_wasm_bg.wasm"; then
  python3 -m http.server "$PORT" --bind 127.0.0.1 >"$OUT/static.log" 2>&1 &
  echo $! >"$OUT/static.pid"
  sleep 1
fi
run() { # name outer-timeout-s probe-args...
  local name=$1 limit=$2; shift 2
  echo "== $(date +%T) $name"
  for _ in $(seq 15); do pgrep -x chrome >/dev/null || break; sleep 2; done
  pkill -x chrome 2>/dev/null || true
  timeout "$limit" node examples/web-host/probe-orbit-load.mjs "$@" "$OUT/$name.json" 2>&1 | tail -2
  pkill -x chrome 2>/dev/null || true
}
arm() { [ "$1" = cpu ] && echo 0 || echo 1; }
for kind in ${KINDS:-timing pixels rest}; do
  extra=()
  [ "$kind" = pixels ] && extra=(--pixels)
  [ "$kind" = rest ] && extra=(--rest-first)
  for ds in majadas belvedere; do
    for a in cpu cells; do
      run "$kind-$ds-$a-4px-local" 400 "$DATA/$ds-v75.oct" --px=4 --cells="$(arm $a)" --max-load-s=120 --pixels=final "${extra[@]}"
    done
  done
  if [ "$kind" != pixels ]; then
    for a in cpu cells; do
      run "$kind-majadas-$a-4px-remote" 500 "$DATA/majadas-v75.oct" --px=4 --cells="$(arm $a)" --max-load-s=180 --remote --pixels=final "${extra[@]}"
    done
    run "$kind-majadas-cells-1px-local" 500 "$DATA/majadas-v75.oct" --px=1 --cells=1 --max-load-s=240 --pixels=final "${extra[@]}"
  fi
done
echo "== done $(date +%T)"
