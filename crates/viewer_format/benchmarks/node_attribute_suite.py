#!/usr/bin/env python3
"""Provenance and measured aggregates for standalone node-attribute experiments."""
import argparse
import hashlib
import json
import pathlib
import platform
import subprocess
import time


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return {"path": str(path.resolve()), "bytes": path.stat().st_size,
            "sha256": digest.hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--report", type=pathlib.Path, required=True)
    parser.add_argument("--sample", nargs=3, action="append", required=True,
                        metavar=("LABEL", "INPUT_OR_SYNTHETIC", "RULES"))
    parser.add_argument("--max-blocks", type=int, default=2**32 - 1)
    parser.add_argument("--repetitions", type=int, default=3)
    args = parser.parse_args()
    if args.max_blocks <= 0 or args.repetitions <= 0:
        parser.error("positive limits required")
    args.output.mkdir(parents=True, exist_ok=False)
    root = pathlib.Path(__file__).resolve().parents[2]
    sources = [root / name for name in (
        "viewer_core/src/node_attributes.rs", "viewer_core/src/categorical_attribute.rs",
        "viewer_format/src/experimental_tuple_dictionary.rs",
        "viewer_format/examples/node_attribute_bench.rs",
        "viewer_format/benchmarks/node_attribute_suite.py")]
    report = {"experimental_only": True, "profile": "perf", "platform": platform.platform(),
              "binary": fingerprint(args.binary), "sources": [fingerprint(p) for p in sources],
              "repetitions": args.repetitions, "samples": [],
              "limits": ["warm uncontrolled OS cache", "sum of per-block CPU medians",
                         "no RSS/GPU/WASM benchmark", "normalized depth-4 grid, not persisted geometry",
                         "independent raw/LZ4 control; pre3 payload bytes reported separately",
                         "category-domain artifacts are common overhead outside group bytes"]}
    for label, source, rules in args.sample:
        if not label or pathlib.Path(label).name != label or label in (".", ".."):
            parser.error("sample labels must be simple directory names")
        inputs = []
        if source != "synthetic":
            path = pathlib.Path(source)
            inputs = [fingerprint(path), fingerprint(path.with_suffix(".octattrs"))]
        command = [str(args.binary.resolve()), "--synthetic" if source == "synthetic" else source,
                   str(args.output / label), str(args.max_blocks), str(args.repetitions), rules]
        stdout_path = args.output / f"{label}.jsonl"
        stderr_path = args.output / f"{label}.stderr.log"
        start = time.monotonic()
        with stdout_path.open("w") as stdout, stderr_path.open("w") as stderr:
            subprocess.run(command, stdout=stdout, stderr=stderr, check=True, timeout=1800)
        elapsed = time.monotonic() - start
        records = [json.loads(line) for line in stdout_path.read_text().splitlines()]
        summary = [record for record in records if record.get("summary")]
        artifacts = [fingerprint(p) for p in sorted((args.output / label).glob("*.bin"))]
        manifest = args.output / f"{label}.artifacts.json"
        manifest.write_text(json.dumps(artifacts, indent=2) + "\n")
        sample = {"label": label, "input": inputs, "command": command, "wall_seconds": elapsed,
                  "summary": summary, "input_source_payload_bytes": sum(
                      r.get("input_source_payload_bytes", 0) for r in records),
                  "category_domain_bytes": sum(r.get("category_domain_bytes", 0) for r in records),
                  "categorize_ms": sum(r.get("categorize_ms", 0) for r in records),
                  "artifact_count": len(artifacts), "artifact_bytes": sum(a["bytes"] for a in artifacts),
                  "artifact_manifest": fingerprint(manifest), "stdout": fingerprint(stdout_path),
                  "stderr": fingerprint(stderr_path)}
        report["samples"].append(sample)
        # Retain completed evidence if a later sample fails.
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(f"{label}: {len(artifacts)} artifacts, {elapsed:.3f}s", flush=True)


if __name__ == "__main__":
    main()
