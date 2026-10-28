#!/usr/bin/env bash
# Fitness check: FormatVersion::V<n> literals must not appear in non-test Rust
# source outside crates/viewer_format/ (use the format crate's capability API).
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
while IFS=: read -r file line text; do
  # Hits at or below the first #[cfg(test)] line of the file are test code.
  first_test=$(grep -n '#\[cfg(test)\]' "$file" | head -1 | cut -d: -f1 || true)
  if [ -n "$first_test" ] && [ "$line" -gt "$first_test" ]; then
    continue
  fi
  echo "$file:$line:$text"
  fail=1
done < <(grep -rnE 'FormatVersion::V[0-9]' crates src examples --include='*.rs' 2>/dev/null \
  | grep -v '^crates/viewer_format/' \
  | grep -vE '_tests\.rs:|/tests/|/examples/|/benches/' || true)

if [ "$fail" -ne 0 ]; then
  echo "error: FormatVersion::V<n> literals found in non-test code outside crates/viewer_format/" >&2
  exit 1
fi
echo "version literals: ok"
