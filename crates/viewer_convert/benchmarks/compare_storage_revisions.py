#!/usr/bin/env python3
"""Repeat a recorded corpus/options across exact revisions, with alternating order and RSS."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import statistics
import subprocess
import sys
import time

import storage_suite


def fingerprint(path: Path) -> dict:
    return {"path": str(path.resolve()), "bytes": path.stat().st_size,
            "sha256": storage_suite.sha256_file(path)}


OPTION_FLAGS = ("--workers", "--divisions", "--points-per-block", "--max-decoded-mib")


def _require(container: dict, key: str, where: str):
    if not isinstance(container, dict) or key not in container:
        raise ValueError(f"reference lacks required field {where}.{key}" if where else
                         f"reference lacks required field {key}")
    return container[key]


def _resolve(path, root: Path | None) -> Path:
    path = Path(path)
    return path if root is None or path.is_absolute() else root / path


def parse_reference(reference: dict, root: Path | None = None) -> list[dict]:
    """Return [{label, source, options: {flag: value}}] from either supported reference layout.

    Command layout: ``datasets`` is a list of ``{label, runs: [{command: [...]}]}`` (a storage_suite
    command line). Recorded layout: ``datasets`` is a mapping ``label -> {input: {path},
    logical_read: {budget_mib}}`` with top-level ``conversion_options``
    ``{workers, divisions, points_per_block}``.

    Relative source paths are resolved against ``root`` (the repository root) in both layouts, never
    the cwd or the reference file's directory: the recorded paths (e.g. ``data/x.laz``) are
    repository-relative. With ``root=None`` they are returned unchanged.
    """
    datasets = _require(reference, "datasets", "")
    parsed = []
    if isinstance(datasets, list):
        for index, dataset in enumerate(datasets):
            where = f"datasets[{index}]"
            label = _require(dataset, "label", where)
            runs = _require(dataset, "runs", where)
            if not runs:
                raise ValueError(f"reference lacks required field {where}.runs[0]")
            template = _require(runs[0], "command", f"{where}.runs[0]")
            options = {}
            for flag in OPTION_FLAGS:
                if flag not in template:
                    raise ValueError(f"reference {where}.runs[0].command lacks option {flag}")
                position = template.index(flag) + 1
                if position >= len(template):
                    raise ValueError(f"reference {where}.runs[0].command has option {flag} without a value")
                options[flag] = str(template[position])
            parsed.append({"label": label, "source": _resolve(template[2], root), "options": options})
    elif isinstance(datasets, dict):
        conversion = _require(reference, "conversion_options", "")
        for label, dataset in datasets.items():
            where = f"datasets.{label}"
            source = _require(_require(dataset, "input", where), "path", f"{where}.input")
            budget = _require(_require(dataset, "logical_read", where), "budget_mib", f"{where}.logical_read")
            options = {"--workers": _require(conversion, "workers", "conversion_options"),
                       "--divisions": _require(conversion, "divisions", "conversion_options"),
                       "--points-per-block": _require(conversion, "points_per_block", "conversion_options"),
                       "--max-decoded-mib": budget}
            parsed.append({"label": label, "source": _resolve(source, root),
                           "options": {k: str(v) for k, v in options.items()}})
    else:
        raise ValueError("reference field datasets must be a list or a mapping")
    return parsed


def parse_args(argv=None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path, help="recorded comparison: datasets list with runs[0].command, or a storage-suite "
                        "report with datasets{label: input.path, logical_read.budget_mib} and conversion_options")
    parser.add_argument("output", type=Path, help="new directory for full raw evidence")
    parser.add_argument("report", type=Path, help="compact aggregate JSON")
    parser.add_argument("--convert-binary", type=Path, required=True)
    parser.add_argument("--read-binary", type=Path, required=True)
    parser.add_argument("--runner", type=Path, default=Path(storage_suite.__file__))
    parser.add_argument("--versions", nargs=2, choices=storage_suite.FORMAT_VERSIONS,
                        default=["v5", "v7.2-pre"])
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--modes", nargs="+", choices=storage_suite.MODES, default=list(storage_suite.MODES),
                        help="read modes to run (default: all; 'reconstruct' decodes the whole dataset into RAM)")
    args = parser.parse_args(argv)
    args.modes = [mode for mode in storage_suite.MODES if mode in set(args.modes)]
    if args.rounds <= 0 or args.versions[0] == args.versions[1]:
        parser.error("positive rounds and two distinct revisions required")
    return args


def main() -> None:
    args = parse_args()
    root = args.runner.resolve().parents[3]
    datasets = parse_reference(json.loads(args.reference.read_text()), root)
    missing = [str(d["source"]) for d in datasets if not d["source"].is_file()]
    if missing:  # validate before creating any output so a failure leaves nothing behind
        raise FileNotFoundError(f"missing source files: {', '.join(missing)}")
    args.output.mkdir(parents=True, exist_ok=False)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    sources = [p for folder in ("viewer_format/src", "viewer_convert/src")
               for p in sorted((root / "crates" / folder).glob("*.rs"))]
    sources += [root / "Cargo.toml", root / "Cargo.lock", args.runner, Path(__file__)]
    report = {"status": "incomplete", "reference": fingerprint(args.reference),
              "environment": storage_suite.repository_metadata(root),
              "profile": "perf", "modes": args.modes, "protocol": "alternating revisions; serial conversion then hierarchy/resident/BFS; Linux time peak RSS",
              "cache_state": "no purge; reads after conversion/validation/hashes on local NVMe",
              "binaries": [fingerprint(args.convert_binary), fingerprint(args.read_binary)],
              "sources": [fingerprint(p) for p in sources], "datasets": []}
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    for dataset in datasets:
        source = dataset["source"]
        label = "".join(c if c.isalnum() else "-" for c in dataset["label"])
        result = {"label": dataset["label"], "source": fingerprint(source), "runs": [], "summaries": {}}
        report["datasets"].append(result)
        collected = {v: [] for v in args.versions}
        for repetition in range(1, args.rounds + 1):
            order = args.versions if repetition % 2 else args.versions[::-1]
            for version in order:
                run_dir = args.output.resolve() / label / f"{repetition}-{version}"
                command = [sys.executable, str(args.runner.resolve()), str(source), "output.oct",
                           str(run_dir), "--convert-binary", str(args.convert_binary.resolve()),
                           "--read-binary", str(args.read_binary.resolve()), "--format-version", version,
                           "--repetitions", "1", "--profile", "perf", "--cache-state", report["cache_state"],
                           "--measure-rss",
                           "--modes", *args.modes]
                for option, value in dataset["options"].items():
                    command += [option, value]
                start = time.monotonic()
                subprocess.run(command, check=True, timeout=1800)
                raw_path = run_dir / "report.json"
                raw = json.loads(raw_path.read_text())
                if raw["status"] != "complete":
                    raise RuntimeError(f"incomplete {raw_path}")
                collected[version].append(raw)
                result["runs"].append({"round": repetition, "version": version, "command": command,
                                       "wall_seconds": time.monotonic() - start, "raw_report": fingerprint(raw_path)})
                for v, runs in collected.items():
                    if not runs:
                        continue
                    conversions = [r["conversions"][0] for r in runs]
                    for artifact in ("oct", "octattrs"):
                        if len({c["artifacts"][artifact]["sha256"] for c in conversions}) != 1:
                            raise RuntimeError(f"unstable hashes for {v} {artifact}")
                    stats = {"conversion": storage_suite.numeric_summary(conversions),
                             "artifacts": conversions[0]["artifacts"], "rss": {}}
                    for operation in ("conversion", *args.modes):
                        records = conversions if operation == "conversion" else [r["reads"][operation][0] for r in runs]
                        stats["rss"][operation] = {"median": statistics.median(r["peak_rss_bytes"] for r in records),
                                                  "samples": len(records)}
                        if operation != "conversion":
                            stats[operation] = storage_suite.numeric_summary(records)
                    result["summaries"][v] = stats
                for mode in (m for m in ("reconstruct", "progressive") if m in args.modes):
                    records = [r["reads"][mode][0]["metrics"][0] for runs in collected.values() for r in runs]
                    signatures = {(r["checksum"], r["points"], r["blocks"]) for r in records}
                    if len(signatures) != 1:
                        raise RuntimeError(f"logical mismatch {dataset['label']} {mode}")
                    result.setdefault("checksums", {})[mode] = records[0]["checksum"]
                args.report.write_text(json.dumps(report, indent=2) + "\n")
                print(f"{dataset['label']} round {repetition} {version} complete", flush=True)
    report["status"] = "complete"
    args.report.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
