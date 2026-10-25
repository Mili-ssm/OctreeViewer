#!/usr/bin/env python3
"""Run repeated conversion and storage-read benchmarks into a new report directory."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
from typing import Any, Sequence


FORMAT_VERSIONS = ("v5", "v6.1-pre", "v6.2-pre", "v6.3-pre", "v6.4-pre", "v7.1-pre", "v7.2-pre")
MODES = ("hierarchy", "reconstruct", "progressive")
KEY_VALUE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)=([^\s]+)")


def metric_value(raw: str) -> Any:
    try:
        if raw.lower().startswith(("0x", "-0x")):
            return int(raw, 16)
        return int(raw)
    except ValueError:
        try:
            return float(raw)
        except ValueError:
            return raw


def parse_conversion_metrics(stdout: str) -> dict[str, Any]:
    """Parse the real_dataset_bench one key=value field per line output."""
    metrics: dict[str, Any] = {}
    for line in stdout.splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key):
            metrics[key] = metric_value(value.strip())
    return metrics


def parse_reader_metrics(stdout: str) -> list[dict[str, Any]]:
    """Parse storage_bench records, which contain multiple fields per line."""
    records = []
    for line in stdout.splitlines():
        fields = {
            key: value if key == "checksum" else metric_value(value)
            for key, value in KEY_VALUE.findall(line)
        }
        if fields:
            records.append(fields)
    return records


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def command_version(command: Sequence[str], cwd: Path) -> str | None:
    try:
        result = subprocess.run(command, cwd=cwd, check=False, capture_output=True, text=True)
    except OSError:
        return None
    return (result.stdout or result.stderr).strip() or None


def repository_metadata(root: Path) -> dict[str, Any]:
    head = command_version(("git", "rev-parse", "HEAD"), root)
    dirty_result = subprocess.run(
        ("git", "status", "--porcelain"), cwd=root, check=False, capture_output=True, text=True
    )
    return {
        "git_head": head,
        "git_dirty": bool(dirty_result.stdout.strip()) if dirty_result.returncode == 0 else None,
        "platform": platform.platform(),
        "cpu": platform.processor() or platform.machine(),
        "python": sys.version.split()[0],
        "rustc": command_version(("rustc", "--version"), root),
    }


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="LAS/LAZ source")
    parser.add_argument("output", type=Path, help="output basename/template; files are created in run_dir")
    parser.add_argument("run_dir", type=Path, help="new directory for outputs, logs, and report")
    parser.add_argument("--convert-binary", type=Path, required=True)
    parser.add_argument("--read-binary", type=Path, required=True)
    parser.add_argument("--format-version", choices=FORMAT_VERSIONS, default="v5")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--divisions", type=int, default=2)
    parser.add_argument("--points-per-block", type=int, default=262144)
    parser.add_argument("--max-decoded-mib", type=int, default=512)
    parser.add_argument("--cache-state", required=True, help="free-form cache-state description")
    parser.add_argument("--profile", default="perf", help="profile label recorded in the report (default: perf)")
    parser.add_argument("--measure-rss", action="store_true", help="Linux process peak RSS via /usr/bin/time")
    args = parser.parse_args(argv)
    if args.repetitions <= 0:
        parser.error("--repetitions must be positive")
    if args.workers <= 0 or args.divisions <= 0 or args.points_per_block <= 0 or args.max_decoded_mib <= 0:
        parser.error("--workers, --divisions, --points-per-block, and --max-decoded-mib must be positive")
    if args.points_per_block > 1_000_000:
        parser.error("--points-per-block must not exceed 1000000")
    return args


def numeric_summary(runs: list[dict[str, Any]]) -> dict[str, dict[str, float | int]]:
    samples: dict[str, list[float | int]] = {}
    for run in runs:
        if run.get("exit_code") != 0 or run.get("artifact_error"):
            continue
        metrics = run.get("metrics", {})
        records = metrics if isinstance(metrics, list) else [metrics]
        for record in records:
            if "level" in record:
                continue
            for key, value in record.items():
                if isinstance(value, (int, float)) and not isinstance(value, bool) and (
                    "unix_ns" not in key
                    and ("time" in key or key.endswith(("_ms", "_ns", "_bytes")) or "bytes" in key)
                ):
                    samples.setdefault(key, []).append(value)
    summary = {}
    for key, values in samples.items():
        ordered = sorted(values)
        middle = len(ordered) // 2
        median: float | int = (
            ordered[middle]
            if len(ordered) % 2
            else (ordered[middle - 1] + ordered[middle]) / 2
        )
        summary[key] = {"median": median, "min": ordered[0], "max": ordered[-1], "samples": len(values)}
    return summary


def write_report(path: Path, report: dict[str, Any]) -> None:
    temporary = path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def run_operation(
    command: list[str], run_dir: Path, log_stem: str, parser: Any, measure_rss: bool = False
) -> dict[str, Any]:
    rss_path = run_dir / f"{log_stem}.peak-rss-kib.log"
    execution = (["/usr/bin/time", "--format=%M", f"--output={rss_path}", *command]
                 if measure_rss else command)
    result = subprocess.run(execution, cwd=run_dir, check=False, capture_output=True, text=True)
    (run_dir / f"{log_stem}.stdout.log").write_text(result.stdout, encoding="utf-8")
    (run_dir / f"{log_stem}.stderr.log").write_text(result.stderr, encoding="utf-8")
    record = {
        "command": command,
        "execution_command": execution,
        "exit_code": result.returncode,
        "stdout_log": f"{log_stem}.stdout.log",
        "stderr_log": f"{log_stem}.stderr.log",
        "metrics": parser(result.stdout),
    }
    if measure_rss and result.returncode == 0:
        record["peak_rss_bytes"] = int(rss_path.read_text().strip()) * 1024
    return record


def run_suite(args: argparse.Namespace) -> int:
    input_path = args.input.resolve()
    convert_binary = args.convert_binary.resolve()
    read_binary = args.read_binary.resolve()
    run_dir = args.run_dir.resolve()
    for path, label in ((input_path, "input"), (convert_binary, "convert binary"), (read_binary, "read binary")):
        if not path.is_file():
            raise ValueError(f"{label} does not exist or is not a file: {path}")
    for path, label in ((convert_binary, "convert binary"), (read_binary, "read binary")):
        if not os.access(path, os.X_OK):
            raise ValueError(f"{label} is not executable: {path}")
    if args.repetitions <= 0:
        raise ValueError("repetitions must be positive")
    if args.workers <= 0 or args.divisions <= 0 or args.points_per_block <= 0 or args.max_decoded_mib <= 0:
        raise ValueError("workers, divisions, points per block, and decoded budget must be positive")
    if args.points_per_block > 1_000_000:
        raise ValueError("points per block must not exceed 1000000")
    if args.measure_rss and not Path("/usr/bin/time").is_file():
        raise ValueError("--measure-rss requires /usr/bin/time")
    if run_dir.exists():
        raise ValueError(f"run directory already exists: {run_dir}")

    input_metadata = {
        "path": str(input_path),
        "bytes": input_path.stat().st_size,
        "sha256": sha256_file(input_path),
    }
    binary_metadata = {
        "convert": {"path": str(convert_binary), "sha256": sha256_file(convert_binary)},
        "read": {"path": str(read_binary), "sha256": sha256_file(read_binary)},
    }
    run_dir.mkdir(parents=True)
    report_path = run_dir / "report.json"
    root = Path(__file__).resolve().parents[3]
    report: dict[str, Any] = {
        "status": "incomplete",
        "input": input_metadata,
        "binaries": binary_metadata,
        "environment": repository_metadata(root),
        "options": {
            "format_version": args.format_version,
            "repetitions": args.repetitions,
            "workers": args.workers,
            "divisions": args.divisions,
            "points_per_block": args.points_per_block,
            "max_decoded_mib": args.max_decoded_mib,
            "cache_state": args.cache_state,
            "profile": args.profile,
            "measure_rss": args.measure_rss,
        },
        "conversions": [],
        "reads": {mode: [] for mode in MODES},
        "summaries": {},
        "error": None,
    }
    write_report(report_path, report)
    failed = False
    try:
        base_name = args.output.name
        stem = Path(base_name).stem
        suffix = Path(base_name).suffix or ".oct"
        for repetition in range(1, args.repetitions + 1):
            output_path = run_dir / f"{stem}-run-{repetition:03d}{suffix}"
            conversion = run_operation(
                [
                    str(convert_binary), "convert-profile", str(input_path), str(output_path),
                    str(args.workers), str(args.divisions), str(args.points_per_block), args.format_version,
                ],
                run_dir,
                f"convert-{repetition:03d}",
                parse_conversion_metrics,
                args.measure_rss,
            )
            conversion.update(repetition=repetition, output=str(output_path))
            if conversion["exit_code"] == 0:
                attributes_path = output_path.with_suffix(".octattrs")
                try:
                    output_size = output_path.stat().st_size
                    attributes_size = attributes_path.stat().st_size
                    if output_size <= 0 or attributes_size <= 0:
                        raise ValueError("output pair must contain nonempty files")
                    conversion["artifacts"] = {
                        "oct": {
                            "path": str(output_path),
                            "bytes": output_size,
                            "sha256": sha256_file(output_path),
                        },
                        "octattrs": {
                            "path": str(attributes_path),
                            "bytes": attributes_size,
                            "sha256": sha256_file(attributes_path),
                        },
                    }
                except (OSError, ValueError) as error:
                    conversion["artifact_error"] = str(error)
            report["conversions"].append(conversion)
            report["summaries"]["conversion"] = numeric_summary(report["conversions"])
            write_report(report_path, report)
            if conversion["exit_code"] != 0 or conversion.get("artifact_error"):
                failed = True
                raise RuntimeError(
                    conversion.get("artifact_error")
                    or f"conversion repetition {repetition} exited {conversion['exit_code']}"
                )
            for mode in MODES:
                read = run_operation(
                    [
                        str(read_binary), str(output_path), mode, "1", str(args.max_decoded_mib),
                    ],
                    run_dir,
                    f"read-{repetition:03d}-{mode}",
                    parse_reader_metrics,
                    args.measure_rss,
                )
                read.update(repetition=repetition, output=str(output_path), mode=mode)
                report["reads"][mode].append(read)
                report["summaries"][mode] = numeric_summary(report["reads"][mode])
                write_report(report_path, report)
                if read["exit_code"] != 0:
                    failed = True
                    raise RuntimeError(f"{mode} read for conversion {repetition} exited {read['exit_code']}")
        report["status"] = "complete"
    except Exception as error:  # retain all completed work and logs
        report["status"] = "failed" if failed else "incomplete"
        report["error"] = str(error)
    finally:
        report["summaries"] = {
            "conversion": numeric_summary(report["conversions"]),
            **{
                mode: numeric_summary(report["reads"][mode])
                for mode in MODES
            },
        }
        write_report(report_path, report)
    return 0 if report["status"] == "complete" else 1


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        return run_suite(args)
    except (OSError, ValueError) as error:
        print(f"storage_suite: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
