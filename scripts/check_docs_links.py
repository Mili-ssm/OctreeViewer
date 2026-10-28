#!/usr/bin/env python3
"""Check relative links and anchors in git-tracked Markdown files (stdlib only).

Allowlist lines are `file|target`; an allowlisted entry that no longer occurs
is itself an error, so stale entries cannot linger.

Out of scope: reference-style links, `<a id>` anchors, setext headings and
`<url>` link targets.
"""
import os
import re
import subprocess
import sys
import unicodedata
import urllib.parse

LINK_RE = re.compile(r"(?<!\!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
IMG_LINK_RE = re.compile(r"\!\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
HEADING_RE = re.compile(r"^ {0,3}(#{1,6})\s+(.*?)\s*$")
FENCE_RE = re.compile(r"^ {0,3}(```+|~~~+)")


def slugify(text):
    text = re.sub(r"\s#+$", "", text.strip())
    text = re.sub(r"<[^>]*>", "", text)
    text = text.replace("`", "").replace("*", "")
    # Emphasis underscores sit at word boundaries; in-word ones stay.
    text = re.sub(r"(?<![0-9A-Za-z])_+|_+(?![0-9A-Za-z])", "", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = text.strip().lower()
    out = []
    for ch in text:
        if ch in "-_":
            out.append(ch)
        elif ch == " ":
            out.append("-")
        elif unicodedata.category(ch)[0] in "LN" or unicodedata.category(ch) == "Mn":
            out.append(ch)
    return "".join(out)


def iter_lines(path):
    """Yield (lineno, line, in_fence)."""
    fence = None
    with open(path, encoding="utf-8") as fh:
        for no, line in enumerate(fh, 1):
            m = FENCE_RE.match(line)
            if m:
                marker = m.group(1)[0]
                if fence is None:
                    fence = marker
                elif fence == marker:
                    fence = None
                yield no, line, True
                continue
            yield no, line, fence is not None


def anchors(path):
    seen = {}
    result = set()
    for _, line, in_fence in iter_lines(path):
        if in_fence:
            continue
        m = HEADING_RE.match(line)
        if not m:
            continue
        slug = slugify(m.group(2))
        n = seen.get(slug, 0)
        seen[slug] = n + 1
        result.add(slug if n == 0 else f"{slug}-{n}")
    return result


def tracked_paths(root, listed):
    """Absolute paths of tracked files plus each of their parent directories."""
    paths = set()
    for rel in listed:
        full = os.path.join(root, rel)
        while full != root and full not in paths:
            paths.add(full)
            full = os.path.dirname(full)
    paths.add(root)
    return paths


def check_file(root, rel, tracked=None):
    """Return list of (lineno, target) broken links.

    With `tracked` (see `tracked_paths`) targets must be tracked, so a link
    into a git-ignored local path fails here exactly as it does in CI.
    """
    path = os.path.join(root, rel)
    broken = []
    cache = {}
    for no, line, in_fence in iter_lines(path):
        if in_fence:
            continue
        line = re.sub(r"`[^`]*`", "", line)
        for target in LINK_RE.findall(line) + IMG_LINK_RE.findall(line):
            if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith("//"):
                continue
            file_part, _, anchor = target.partition("#")
            if file_part:
                dest = os.path.normpath(os.path.join(os.path.dirname(path), file_part))
                if not (dest in tracked if tracked is not None else os.path.exists(dest)):
                    broken.append((no, target))
                    continue
            else:
                dest = path
            if anchor and os.path.isfile(dest) and dest.endswith(".md"):
                if dest not in cache:
                    cache[dest] = anchors(dest)
                if urllib.parse.unquote(anchor).lower() not in cache[dest]:
                    broken.append((no, target))
    return broken


def load_allowlist(path):
    entries = set()
    if path and os.path.exists(path):
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if line and not line.startswith("#"):
                    entries.add(line)
    return entries


def run(root, files, allowlist_path=None, tracked=None):
    allow = load_allowlist(allowlist_path)
    used = set()
    errors = []
    for rel in files:
        for no, target in check_file(root, rel, tracked):
            key = f"{rel}|{target}"
            if key in allow:
                used.add(key)
            else:
                errors.append(f"{rel}:{no}: {target}")
    for key in sorted(allow - used):
        errors.append(f"stale allowlist entry: {key}")
    return errors


def main():
    root = subprocess.check_output(
        ["git", "rev-parse", "--show-toplevel"], text=True
    ).strip()
    files = subprocess.check_output(
        ["git", "ls-files", "*.md"], cwd=root, text=True
    ).split("\n")
    listed = subprocess.check_output(["git", "ls-files"], cwd=root, text=True).split("\n")
    tracked = tracked_paths(root, [f for f in listed if f])
    files = [f for f in files if f and os.path.exists(os.path.join(root, f))]
    allow = os.path.join(root, "scripts", "docs-links-allowlist.txt")
    errors = run(root, files, allow, tracked)
    for e in errors:
        print(e)
    if errors:
        return 1
    print(f"docs links: ok ({len(files)} files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
