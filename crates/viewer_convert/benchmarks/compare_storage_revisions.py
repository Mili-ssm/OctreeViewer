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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path, help="recorded comparison with datasets/runs/command")
    parser.add_argument("output", type=Path, help="new directory for full raw evidence")
    parser.add_argument("report", type=Path, help="compact aggregate JSON")
    parser.add_argument("--convert-binary", type=Path, required=True)
    parser.add_argument("--read-binary", type=Path, required=True)
    parser.add_argument("--runner", type=Path, default=Path(storage_suite.__file__))
    parser.add_argument("--versions", nargs=2, default=["v6.3-pre", "v6.4-pre"])
    parser.add_argument("--rounds", type=int, default=3)
    args = parser.parse_args()
    if args.rounds <= 0 or args.versions[0] == args.versions[1]:
        parser.error("positive rounds and two distinct revisions required")
    args.output.mkdir(parents=True, exist_ok=False)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    reference = json.loads(args.reference.read_text())
    root = args.runner.resolve().parents[3]
    sources = [p for folder in ("viewer_format/src", "viewer_convert/src")
               for p in sorted((root / "crates" / folder).glob("*.rs"))]
    sources += [root / "Cargo.toml", root / "Cargo.lock", args.runner, Path(__file__)]
    report = {"status": "incomplete", "reference": fingerprint(args.reference),
              "environment": storage_suite.repository_metadata(root),
              "profile": "perf", "protocol": "alternating revisions; serial conversion then hierarchy/resident/BFS; Linux time peak RSS",
              "cache_state": "no purge; reads after conversion/validation/hashes on local NVMe",
              "binaries": [fingerprint(args.convert_binary), fingerprint(args.read_binary)],
              "sources": [fingerprint(p) for p in sources], "datasets": []}
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    for dataset in reference["datasets"]:
        template = dataset["runs"][0]["command"]
        source = Path(template[2])
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
                           "--measure-rss"]
                for option in ("--workers", "--divisions", "--points-per-block", "--max-decoded-mib"):
                    command += [option, template[template.index(option) + 1]]
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
                    for operation in ("conversion", *storage_suite.MODES):
                        records = conversions if operation == "conversion" else [r["reads"][operation][0] for r in runs]
                        stats["rss"][operation] = {"median": statistics.median(r["peak_rss_bytes"] for r in records),
                                                  "samples": len(records)}
                        if operation != "conversion":
                            stats[operation] = storage_suite.numeric_summary(records)
                    result["summaries"][v] = stats
                for mode in ("reconstruct", "progressive"):
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
