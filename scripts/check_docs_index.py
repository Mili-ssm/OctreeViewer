#!/usr/bin/env python3
"""Check the documentation map that agents navigate by (stdlib only).

- Every tracked Markdown file under docs/ is linked from its index:
  ADRs from docs/adr/README.md, records from docs/records/README.md (which
  docs/README.md links), the rest from docs/README.md.
- Every ADR and every plan has its `Estado:` line on line 3, so
  `grep -n '^Estado:'` returns the state of all of them.

Placement rules: docs/README.md#dónde-va-cada-cosa.
"""
import os
import re
import subprocess
import sys

STATUS_LINE = 3
RECORDS_INDEX = "docs/records/README.md"
SKIP_PREFIXES = ("docs/records/benchmarks/data/",)
LINK_RE = re.compile(r"\]\(([^)\s#]+)")


def linked(index):
    base = os.path.dirname(index)
    with open(index, encoding="utf-8") as fh:
        return {os.path.normpath(os.path.join(base, t)) for t in LINK_RE.findall(fh.read())}


def main():
    root = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
    os.chdir(root)
    files = subprocess.check_output(["git", "ls-files", "docs/*.md"], text=True).split()
    files = [f for f in files if os.path.exists(f) and not f.startswith(SKIP_PREFIXES)]
    indexes = {i: linked(i) for i in ("docs/README.md", "docs/adr/README.md", RECORDS_INDEX)}
    errors = []
    for f in files:
        if f == RECORDS_INDEX and f not in indexes["docs/README.md"]:
            errors.append(f"{f}: not linked from docs/README.md")
        if f in indexes:
            continue
        is_adr = re.match(r"docs/adr/\d{4}-", f) is not None
        index = "docs/adr/README.md" if is_adr else RECORDS_INDEX if f.startswith("docs/records/") else "docs/README.md"
        if f not in indexes[index]:
            errors.append(f"{f}: not linked from {index}")
        if is_adr or f.startswith("docs/plans/"):
            with open(f, encoding="utf-8") as fh:
                lines = fh.read().splitlines()
            if len(lines) < STATUS_LINE or not lines[STATUS_LINE - 1].startswith("Estado:"):
                errors.append(f"{f}:{STATUS_LINE}: missing `Estado:` line")
    for e in errors:
        print(e)
    print(f"docs index: {'ok' if not errors else f'{len(errors)} error(s)'} ({len(files)} files)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
