#!/usr/bin/env bash
# Fitness check: FormatVersion::V<n> literals must not appear in non-test Rust
# source outside crates/viewer_format/ (use the format crate's capability API).
#
# Test code is recognised as: files named *_tests.rs, tests/, examples/,
# benches/, and anything below a `#[cfg(test)]` / `#[cfg(all(test, ...))]` /
# `#[cfg(any(test, ...))]` attribute that is directly followed by an inline
# `mod ... { }` block (only lines inside its braces are skipped) or a `mod x;`
# declaration (its `#[path]` file is excluded by name).
set -euo pipefail
cd "$(dirname "$0")/.."

for d in crates examples; do
  [ -d "$d" ] || { echo "error: missing directory $d" >&2; exit 1; }
done

# "start end" line ranges of inline cfg(test) mod blocks; `mod x;` declarations
# (e.g. with #[path]) yield none because their files are excluded by name.
# Braces are counted per line, so braces inside string literals on the mod's
# own lines could skew the end; test mods here do not do that.
test_mod_ranges() {
  awk '
    function count(line,   t) { t = line; opens += gsub(/\{/, "{", t); t = line; closes += gsub(/\}/, "}", t) }
    inside { count($0); if (opens == closes) { print start, NR; inside = 0 } next }
    pending && /^[[:space:]]*(#\[|\/\/)/ { next }
    pending {
      if ($0 ~ /^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?mod[[:space:]]/ && $0 !~ /;[[:space:]]*(\/\/.*)?$/) {
        start = pending; opens = 0; closes = 0; count($0)
        if (opens > 0 && opens == closes) print start, NR; else inside = 1
      }
      pending = 0
    }
    /^[[:space:]]*#\[cfg\((test|all\(test|any\(test)/ { pending = NR }
  ' "$1"
}

hits=$(grep -rnE 'FormatVersion::V[0-9]' crates examples --include='*.rs' \
  | grep -v '^crates/viewer_format/' \
  | grep -vE '_tests\.rs:|/tests/|/examples/|/benches/' || true)

fail=0
while IFS=: read -r file line text; do
  [ -n "$file" ] || continue
  in_test=0
  while read -r start end; do
    [ -n "$start" ] || continue
    if [ "$line" -ge "$start" ] && [ "$line" -le "$end" ]; then in_test=1; fi
  done < <(test_mod_ranges "$file")
  [ "$in_test" -eq 0 ] || continue
  echo "$file:$line:$text"
  fail=1
done <<< "$hits"

if [ "$fail" -ne 0 ]; then
  echo "error: FormatVersion::V<n> literals found in non-test code outside crates/viewer_format/" >&2
  exit 1
fi
echo "version literals: ok"
