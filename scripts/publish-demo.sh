#!/usr/bin/env bash
# Builds the experimental web demo as a static site and, with --push, publishes it to the
# gh-pages branch (https://mili-ssm.github.io/OctreeViewer/, ADR 0045).
#   scripts/publish-demo.sh [--no-build] [--push]
# Without --push it only assembles target/demo-site and prints how to test it.
set -euo pipefail
cd "$(dirname "$0")/.."

build=1
push=0
for arg in "$@"; do
  case "$arg" in
    --no-build) build=0 ;;
    --push) push=1 ;;
    -h|--help) sed -n '2,5p' "$0"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

if [ "$push" = "1" ] && [ -n "$(git status --porcelain)" ]; then
  echo "refusing to publish with a dirty working tree; commit or stash first" >&2
  exit 1
fi

if [ "$build" = "1" ]; then
  scripts/build-wasm-bindings.sh
fi
for f in viewer_wasm.js viewer_wasm_bg.wasm; do
  [ -f "pkg/$f" ] || { echo "pkg/$f missing: run without --no-build" >&2; exit 1; }
done

site="target/demo-site"
rm -rf "$site"
mkdir -p "$site/pkg"
host=examples/web-host
cp "$host"/index.html "$host"/sample-v8.oct "$host"/sample-v8.octattrs "$site"/
# The real second cloud (Belvedere crop, CC BY 4.0) is built outside git by
# scripts/make-demo-belvedere.sh; the page opens it over HTTP Range from datasets/.
crop=target/demo-data/belvedere-crop-v8
if [ -f "$crop.oct" ] && [ -f "$crop.octattrs" ]; then
  mkdir -p "$site/datasets"
  cp "$crop.oct" "$crop.octattrs" "$site/datasets/"
elif [ "$push" = "1" ]; then
  echo "$crop.oct missing: run scripts/make-demo-belvedere.sh before publishing" >&2
  exit 1
else
  echo "note: $crop.oct missing; the site is built without the Belvedere crop" >&2
fi
# The page and its workers; probes/ (probes, spike, RED-REF server) and tests/ stay out: the glob is not recursive.
for f in "$host"/*.js "$host"/*.mjs; do
  cp "$f" "$site"/
done
cp pkg/viewer_wasm.js pkg/viewer_wasm_bg.wasm "$site/pkg/"
# Workers import the bindings from ../../pkg in the repository layout; here pkg/ sits next to them.
sed -i 's#"\.\./\.\./pkg/viewer_wasm\.js"#"./pkg/viewer_wasm.js"#' "$site"/*.js
if grep -l '\.\./\.\./pkg' "$site"/*.js "$site"/*.mjs 2>/dev/null; then
  echo "unpatched ../../pkg import left in the site" >&2
  exit 1
fi
# Every relative import of the page and its workers must resolve inside the site.
missing=0
for ref in $(grep -hoE "(from|import\(|new URL\() *['\"]\./[^'\"]*['\"]" "$site"/*.js "$site"/*.mjs | grep -oE "\./[^'\"]*" | sort -u); do
  [ -f "$site/$ref" ] || { echo "missing in site: $ref" >&2; missing=1; }
done
[ "$missing" = "0" ] || exit 1
# Cache busting (plan A10): a visitor with a cached wasm of an older format must not pair it with
# V8 data. The bindings get the commit in their file names and the page, its workers and the
# bindings glue reference those names, so old and new bundles never mix; datasets are versioned by name.
build_id="$(git rev-parse --short HEAD)"
glue="viewer_wasm-$build_id.js"
wasm="viewer_wasm_bg-$build_id.wasm"
mv "$site/pkg/viewer_wasm.js" "$site/pkg/$glue"
mv "$site/pkg/viewer_wasm_bg.wasm" "$site/pkg/$wasm"
sed -i "s#viewer_wasm_bg\\.wasm#$wasm#" "$site/pkg/$glue"
sed -i "s#\"\\./pkg/viewer_wasm\\.js\"#\"./pkg/$glue\"#" "$site"/*.js
sed -i -E "s#\"\\./(viewer-worker|build-worker|converter-worker)\\.js\"#\"./\\1.js?v=$build_id\"#g" "$site"/*.js "$site"/*.mjs
sed -i "s#module.src = \"\\./main\\.js\"#module.src = \"./main.js?v=$build_id\"#" "$site/index.html"
grep -q "main.js?v=$build_id" "$site/index.html" || { echo "index.html cache-busting rewrite failed" >&2; exit 1; }
grep -q "$wasm" "$site/pkg/$glue" || { echo "wasm name rewrite failed in $glue" >&2; exit 1; }
if grep -lE '"\./pkg/viewer_wasm\.js"|viewer_wasm_bg\.wasm' "$site"/*.js "$site/pkg/$glue"; then
  echo "unversioned bindings reference left in the site" >&2
  exit 1
fi
touch "$site/.nojekyll"

site_abs="$(cd "$site" && pwd)"
sha="$(git rev-parse --short HEAD)"

if [ "$push" = "0" ]; then
  echo "Site built at $site_abs"
  echo "Test it: python3 -m http.server -d $site_abs 8001   then open http://127.0.0.1:8001/"
  exit 0
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/gh-pages.XXXXXX")"
cleanup() { git worktree remove --force "$tmp" 2>/dev/null || true; rm -rf "$tmp"; git worktree prune; }
trap cleanup EXIT
git fetch origin gh-pages 2>/dev/null || true
if git show-ref --verify --quiet refs/remotes/origin/gh-pages; then
  git worktree add --detach "$tmp" origin/gh-pages
  git -C "$tmp" checkout -B gh-pages origin/gh-pages
else
  git worktree add --detach "$tmp"
  git -C "$tmp" checkout --orphan gh-pages
fi
git -C "$tmp" rm -rfq --ignore-unmatch . >/dev/null
find "$tmp" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
cp -a "$site_abs"/. "$tmp"/
git -C "$tmp" add -A
if git -C "$tmp" diff --cached --quiet; then
  echo "gh-pages already matches $sha; nothing to publish"
  exit 0
fi
git -C "$tmp" commit -q -m "deploy: publish the experimental demo from $sha"
git -C "$tmp" push origin gh-pages
echo "Published $sha to gh-pages"
