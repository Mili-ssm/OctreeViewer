#!/usr/bin/env python3
"""Check the source layout by component (stdlib only).

- `#[path = "T"]` is allowed only for a sibling test file (`T` has no `/` and
  ends in `_tests.rs`) or a (file, T) pair in ALLOWED_PATHS; stale entries fail.
  Anything else hides the module tree from `x.rs` + `x/` navigation.
- No `cell_*.rs` directly in viewer_render_wgpu/src and no `lod_cells*.rs`
  directly in viewer_wasm/src: a repeated prefix belongs in a subfolder.
- The CPU-policy files of the render crate's `cells/` package must not mention
  `wgpu::`, `crate::gpu`, `cells::render` or `super::render`, so the policy
  stays testable without a GPU.
- Every dependency edge between workspace crates is in ALLOWED_EDGES, so a new
  edge is a deliberate change; stale entries fail.
- `crates/viewer_core/Cargo.toml` declares no `laz` dependency: the LAS preflight
  lives in viewer_convert_core (ADR 0062), so viewer_core stays free of the decoder.
- No `.rs` file of `crates/viewer_convert_core/src/pipeline.rs` or `pipeline/` names the
  `spatial` module in a non-comment line (`crate::spatial`, `super::super::spatial`, or `spatial::`
  inside a multi-line `use crate::{ ... }` group): `pipeline` holds the seams with the environment
  and `spatial` (the octree planner) depends on it, never the other way round.

Rules: CONTRIBUTING.md#convenciones. Plan: docs/records/archive/reorganizacion-por-componentes.md.
"""
import fnmatch
import json
import os
import re
import subprocess
import sys

MAX_ALLOWED_PATHS = 15
PATH_RE = re.compile(r'#\[path\s*=\s*"([^"]+)"\s*\]')

# (file glob, target, reason)
ALLOWED_PATHS = [
    ("crates/viewer_convert/examples/real_dataset_bench.rs",
     "support/conversion_metrics.rs", "example helper shared outside the example root"),
    ("crates/viewer_wasm/src/bindings.rs",
     "../tests/support/reference_spacing_probe.rs", "integration-test support reused by unit tests"),
    ("crates/viewer_format/tests/*.rs",
     "common/v8_fixture.rs", "integration tests share fixtures (no tests/common/mod.rs)"),
    ("crates/viewer_format/tests/*.rs",
     "common/v8_tree.rs", "integration tests share fixtures (no tests/common/mod.rs)"),
]

CELLS = "crates/viewer_render_wgpu/src/cells/"
POLICY_STEMS = ("cut", "lod", "driver", "pages", "codec", "range", "node_cells", "pool")
FORBIDDEN = ("wgpu::", "crate::gpu", "cells::render", "super::render")

# (from, to, kind) between workspace members, from `cargo metadata`.
ALLOWED_EDGES = [
    ("convert_parity", "viewer_convert", "normal"),
    ("convert_parity", "viewer_convert_core", "normal"),
    ("convert_parity", "viewer_format", "normal"),
    ("format_parity", "viewer_convert", "normal"),
    ("format_parity", "viewer_core", "normal"),
    ("format_parity", "viewer_format", "normal"),
    ("viewer_convert", "viewer_convert_core", "normal"),
    ("viewer_convert", "viewer_core", "normal"),
    ("viewer_convert", "viewer_format", "normal"),
    ("viewer_convert", "viewer_render_wgpu", "dev"),
    ("viewer_convert_core", "laz_chunk_probe", "dev"),
    ("viewer_convert_core", "viewer_core", "normal"),
    ("viewer_convert_core", "viewer_format", "normal"),
    ("viewer_format", "viewer_core", "normal"),
    ("viewer_render_wgpu", "viewer_core", "normal"),
    ("viewer_render_wgpu", "viewer_format", "dev"),
    ("viewer_wasm", "viewer_convert_core", "normal"),
    ("viewer_wasm", "viewer_core", "normal"),
    ("viewer_wasm", "viewer_format", "dev"),
    ("viewer_wasm", "viewer_format", "normal"),
    ("viewer_wasm", "viewer_render_wgpu", "normal"),
]


def code_lines(path):
    """Yield (line number, text) of non-comment lines."""
    with open(path, encoding="utf-8") as fh:
        for n, line in enumerate(fh, 1):
            if not line.lstrip().startswith("//"):
                yield n, line


def check_paths(files, errors):
    if len(ALLOWED_PATHS) > MAX_ALLOWED_PATHS:
        errors.append(f"scripts/check_layout.py: ALLOWED_PATHS has {len(ALLOWED_PATHS)} entries (max {MAX_ALLOWED_PATHS}); rethink the rule")
    used = set()
    for f in files:
        for n, line in code_lines(f):
            m = PATH_RE.search(line)
            if not m:
                continue
            target = m.group(1)
            if "/" not in target and target.endswith("_tests.rs"):
                continue
            hit = [i for i, (g, t, _) in enumerate(ALLOWED_PATHS) if t == target and fnmatch.fnmatchcase(f, g)]
            if hit:
                used.update(hit)
            else:
                errors.append(f"{f}:{n}: #[path = \"{target}\"] must be a sibling *_tests.rs or be added to ALLOWED_PATHS")
    for i, (g, t, _) in enumerate(ALLOWED_PATHS):
        if i not in used:
            errors.append(f"scripts/check_layout.py: stale ALLOWED_PATHS entry ({g}, {t})")


def check_prefixes(files, errors):
    for f in files:
        d, name = os.path.split(f)
        if d == "crates/viewer_render_wgpu/src" and name.startswith("cell_"):
            errors.append(f"{f}:1: `cell_*.rs` in the crate root; move it into cells/")
        if d == "crates/viewer_wasm/src" and name.startswith("lod_cells"):
            errors.append(f"{f}:1: `lod_cells*.rs` in the crate root; move it into a lod_cells/ folder")


def is_policy_file(f):
    if not f.startswith(CELLS) or not f.endswith(".rs"):
        return False
    rel = f[len(CELLS):]
    if rel.endswith("_tests.rs") or rel == "pool/tests.rs" or rel.startswith("pool/tests/"):
        return False
    return rel[:-3] in POLICY_STEMS or rel.startswith("pool/")


def check_policy(files, errors):
    for f in files:
        if not is_policy_file(f):
            continue
        for n, line in code_lines(f):
            for word in FORBIDDEN:
                if word in line:
                    errors.append(f"{f}:{n}: CPU-policy file mentions `{word}`")


PIPELINE_ROOT = "crates/viewer_convert_core/src/pipeline"
SPATIAL_WORD = re.compile(r"\bspatial\b")


def check_pipeline_independent(files, errors):
    for f in files:
        if f != PIPELINE_ROOT + ".rs" and not f.startswith(PIPELINE_ROOT + "/"):
            continue
        for n, line in code_lines(f):
            if SPATIAL_WORD.search(line):
                errors.append(f"{f}:{n}: pipeline must not depend on `spatial` (the planner builds on the seams, not the reverse)")


def check_edges(errors):
    try:
        out = subprocess.check_output(
            ["cargo", "metadata", "--offline", "--format-version", "1", "--no-deps"], text=True)
    except (OSError, subprocess.CalledProcessError) as exc:
        errors.append(f"Cargo.toml: crate edges not checked: `cargo metadata` failed ({exc})")
        return
    packages = json.loads(out)["packages"]
    names = {p["name"] for p in packages}
    edges = {
        (p["name"], d["name"], d["kind"] or "normal")
        for p in packages for d in p["dependencies"] if d["name"] in names
    }
    allowed = set(ALLOWED_EDGES)
    for e in sorted(edges - allowed):
        errors.append(f"Cargo.toml: {e[0]} -> {e[1]} ({e[2]}): crate edge not in ALLOWED_EDGES of scripts/check_layout.py")
    for e in sorted(allowed - edges):
        errors.append(f"scripts/check_layout.py: stale ALLOWED_EDGES entry {e}")


def check_viewer_core_has_no_laz(errors):
    path = "crates/viewer_core/Cargo.toml"
    with open(path, encoding="utf-8") as fh:
        for n, line in enumerate(fh, 1):
            if re.match(r'\s*laz\b', line):
                errors.append(f"{path}:{n}: viewer_core must not depend on `laz` (the LAS preflight lives in viewer_convert_core, ADR 0062)")


def main():
    root = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
    os.chdir(root)
    # Untracked files count too, so a stray new file fails before it is staged.
    listed = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "*.rs"], text=True).split()
    files = sorted({f for f in listed if os.path.exists(f)})
    errors = []
    check_paths(files, errors)
    check_prefixes(files, errors)
    check_policy(files, errors)
    check_pipeline_independent(files, errors)
    check_edges(errors)
    check_viewer_core_has_no_laz(errors)
    for e in errors:
        print(e)
    print(f"layout: {'ok' if not errors else f'{len(errors)} error(s)'} ({len(files)} files)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
