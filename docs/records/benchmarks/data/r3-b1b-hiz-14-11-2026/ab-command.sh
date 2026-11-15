#!/usr/bin/env bash
# A/B of the Hi-Z depth margin: old fixed margin (3bb1d1b reverted) vs derived margin (HEAD).
set -u
S="$1"; cd "$(git rev-parse --show-toplevel)"
build() { cargo build --release --offline -p viewer_wasm --target wasm32-unknown-unknown >/dev/null 2>&1 && wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm; }
build && rm -rf "$S/pkg-new" && cp -r pkg "$S/pkg-new" || exit 1
git revert --no-commit 3bb1d1b || exit 1
build && rm -rf "$S/pkg-old" && cp -r pkg "$S/pkg-old"; ok=$?
git revert --abort; git status --short | head -3
[ $ok -eq 0 ] || exit 1
python3 -m http.server 8767 --bind 127.0.0.1 >/dev/null 2>&1 & srv=$!
for r in 1 2 3; do
  if [ $((r % 2)) -eq 1 ]; then order="old new"; else order="new old"; fi
  for v in $order; do
    rm -rf pkg && cp -r "$S/pkg-$v" pkg
    timeout 600 node examples/web-host/probe-cells-smoke.mjs target/render-data/majadas-v75.oct "$S/$v-$r.json" --px=4 --poses=fitted,leaf --timeout-s=120 --rest-s=10 > "$S/$v-$r.txt" 2>&1
    echo "$v round $r exit=$?"
  done
done
kill $srv; rm -rf pkg && cp -r "$S/pkg-new" pkg
grep -H 'leaf' "$S"/*.txt | cut -c1-400
