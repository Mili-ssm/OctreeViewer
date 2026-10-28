#!/usr/bin/env bash
# Fitness check: FormatVersion::V<n> literals must not appear in non-test Rust
# source outside crates/viewer_format/ (use the format crate's capability API).
#
# Test code is recognised as: files named *_tests.rs, tests/, examples/,
# benches/, and anything below a `#[cfg(test)]` / `#[cfg(all(test, ...))]` /
# `#[cfg(any(test, ...))]` attribute that is directly followed by a `mod` item.
# This assumes test modules are inline `mod ... {` blocks (kept at the end of
# the file) or `#[path]` files.
set -euo pipefail
cd "$(dirname "$0")/.."

for d in crates src examples; do
  [ -d "$d" ] || { echo "error: missing directory $d" >&2; exit 1; }
done

# Line number of the first cfg(test) attribute followed by `mod`, or empty.
first_test_mod() {
  awk '
    pending && /^[[:space:]]*(#\[|\/\/)/ { next }
    pending { if ($0 ~ /^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?mod[[:space:]]/) { print pending; exit } pending = 0 }
    /^[[:space:]]*#\[cfg\((test|all\(test|any\(test)/ { pending = NR }
  ' "$1"
}

hits=$(grep -rnE 'FormatVersion::V[0-9]' crates src examples --include='*.rs' \
  | grep -v '^crates/viewer_format/' \
  | grep -vE '_tests\.rs:|/tests/|/examples/|/benches/' || true)

fail=0
while IFS=: read -r file line text; do
  [ -n "$file" ] || continue
  first_test=$(first_test_mod "$file")
  if [ -n "$first_test" ] && [ "$line" -gt "$first_test" ]; then
    continue
  fi
  echo "$file:$line:$text"
  fail=1
done <<< "$hits"

if [ "$fail" -ne 0 ]; then
  echo "error: FormatVersion::V<n> literals found in non-test code outside crates/viewer_format/" >&2
  exit 1
fi
echo "version literals: ok"
