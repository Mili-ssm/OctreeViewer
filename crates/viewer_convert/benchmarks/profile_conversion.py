#!/usr/bin/env python3
"""Sample process memory and temporary-file sizes during a conversion benchmark."""

from __future__ import annotations

import argparse
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import time
from typing import Any


DEFAULT_BINARY = Path("target/perf/examples/real_dataset_bench")
KILL_TIMEOUT_SECONDS = 5
MAX_RECORDED_ERRORS = 100


def regular_file_sizes(run_dir: Path, output_path: Path) -> dict[str, int]:
    """Return unique temporary file sizes, excluding final outputs and symlinks."""
    excluded = {output_path, output_path.with_suffix(".octattrs")}
    found: dict[str, int] = {}
    seen_inodes: set[tuple[int, int]] = set()

    def visit(directory: Path, inside_work: bool) -> None:
        with os.scandir(directory) as entries:
            for entry in entries:
                try:
                    path = Path(entry.path)
                    if entry.is_symlink():
                        continue
                    if entry.is_dir(follow_symlinks=False):
                        visit(path, inside_work or entry.name.endswith(".work"))
                        continue
                    if not entry.is_file(follow_symlinks=False):
                        continue
                    if path in excluded or not (inside_work or entry.name.endswith(".tmp")):
                        continue
                    metadata = entry.stat(follow_symlinks=False)
                except FileNotFoundError:
                    # A spool may be removed while an individual scan is in progress.
                    continue
                inode = (metadata.st_dev, metadata.st_ino)
                if inode in seen_inodes:
                    continue
                seen_inodes.add(inode)
                found[str(path)] = metadata.st_size

    visit(run_dir, False)
    return found


def read_proc_memory(pid: int) -> tuple[int | None, int | None]:
    values: dict[str, int] = {}
    with open(f"/proc/{pid}/status", encoding="ascii") as status:
        for line in status:
            if line.startswith(("VmRSS:", "VmHWM:")):
                key, value, unit = line.split()[:3]
                if unit != "kB":
                    raise ValueError(f"unexpected {key} unit: {unit}")
                values[key[:-1]] = int(value) * 1024
    return values.get("VmRSS"), values.get("VmHWM")


def final_size(path: Path) -> int | None:
    try:
        metadata = path.stat(follow_symlinks=False)
    except FileNotFoundError:
        return None
    if not path.is_symlink() and metadata and path.is_file():
        return metadata.st_size
    return None


def terminate_child(process: subprocess.Popen[bytes]) -> int:
    if process.poll() is not None:
        return process.returncode
    process.terminate()
    try:
        return process.wait(timeout=KILL_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        process.kill()
        return process.wait()


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def phase_peaks(stdout_path: Path, samples_path: Path) -> dict[str, Any]:
    """Attribute only observations wholly inside one progress interval; stream samples."""
    fields: dict[int, dict[str, Any]] = {}
    with stdout_path.open(encoding="utf-8") as stdout:
        for line in stdout:
            match = re.fullmatch(r"phase_(\d+)_(name|start_unix_ns|end_unix_ns)=(.+)\n?", line)
            if match is None:
                continue
            ordinal, key, value = match.groups()
            phase = fields.setdefault(int(ordinal), {})
            if key in phase:
                raise ValueError("duplicate phase boundary field")
            phase[key] = value if key == "name" else int(value)
    if not fields or sorted(fields) != list(range(len(fields))):
        raise ValueError("missing or non-contiguous phase metadata")
    phases = [fields[index] for index in range(len(fields))]
    for index, phase in enumerate(phases):
        if set(phase) != {"name", "start_unix_ns", "end_unix_ns"}:
            raise ValueError("incomplete phase metadata")
        if phase["start_unix_ns"] < 0 or phase["end_unix_ns"] < phase["start_unix_ns"]:
            raise ValueError("invalid phase boundaries (possible clock change)")
        if index and phases[index - 1]["end_unix_ns"] != phase["start_unix_ns"]:
            raise ValueError("non-contiguous phase boundaries")
        phase.update(ordinal=index, sample_count=0, rss_sampled_peak_bytes=None,
                     temporary_sampled_peak_bytes=None)
    unassigned = 0
    cursor = 0
    previous_end = 0
    with samples_path.open(encoding="utf-8") as samples:
        for line in samples:
            sample = json.loads(line)
            start, end = sample["start_unix_ns"], sample["end_unix_ns"]
            if start < previous_end or end < start:
                raise ValueError("non-monotonic sample timestamps (possible clock change)")
            previous_end = end
            while cursor < len(phases) and start >= phases[cursor]["end_unix_ns"]:
                cursor += 1
            if (cursor == len(phases) or start < phases[cursor]["start_unix_ns"]
                    or end > phases[cursor]["end_unix_ns"]):
                unassigned += 1
                continue
            phase = phases[cursor]
            phase["sample_count"] += 1
            for source, target in (("rss_bytes", "rss_sampled_peak_bytes"),
                                   ("temporary_bytes", "temporary_sampled_peak_bytes")):
                if sample[source] is not None:
                    phase[target] = max(phase[target] or 0, sample[source])
    return {"phases": phases, "phase_unassigned_sample_count": unassigned}


def profile(input_path: Path, run_dir: Path, binary: Path, workers: int, divisions: int,
            interval_ms: int) -> int:
    if not input_path.is_file():
        raise ValueError(f"input is not a regular file: {input_path}")
    if run_dir.exists():
        raise FileExistsError(f"run directory already exists: {run_dir}")
    run_dir.mkdir(parents=True)
    output_path = run_dir / "output.oct"
    command = [str(binary.resolve()), "convert-profile", str(input_path), str(output_path),
               str(workers), str(divisions)]
    started = time.monotonic()
    report: dict[str, Any] = {
        "command": command,
        "exit_code": None,
        "status": "incomplete",
        "elapsed_seconds": None,
        "interval_ms": interval_ms,
        "sample_count": 0,
        "rss_sampled_peak_bytes": None,
        "vmhwm_observed_peak_bytes": None,
        "temporary_sampled_peak_bytes": None,
        "temporary_observed_growth_bytes": 0,
        "output_oct_bytes": None,
        "output_octattrs_bytes": None,
        "observation_errors": [],
        "phases": [],
        "phase_unassigned_sample_count": None,
    }
    errors: list[str] = report["observation_errors"]
    previous_sizes: dict[str, int] = {}
    rss_peak: int | None = None
    hwm_peak: int | None = None
    temporary_peak: int | None = None
    samples_path = run_dir / "samples.jsonl"
    process: subprocess.Popen[bytes] | None = None
    try:
        with (run_dir / "stdout.log").open("wb") as stdout, (run_dir / "stderr.log").open("wb") as stderr:
            process = subprocess.Popen(command, stdout=stdout, stderr=stderr)
            with samples_path.open("w", encoding="utf-8") as samples:
                while process.poll() is None:
                    sample_time = time.monotonic()
                    sample: dict[str, Any] = {"elapsed_seconds": sample_time - started,
                                              "start_unix_ns": time.time_ns()}
                    try:
                        rss, hwm = read_proc_memory(process.pid)
                        sample["rss_bytes"] = rss
                        sample["vmhwm_bytes"] = hwm
                        if rss is not None:
                            rss_peak = max(rss_peak or 0, rss)
                        if hwm is not None:
                            hwm_peak = max(hwm_peak or 0, hwm)
                    except FileNotFoundError:
                        # The process can exit between poll() and reading /proc.
                        if process.poll() is None:
                            _record_error(errors, "process disappeared from /proc while running")
                        sample["rss_bytes"] = None
                        sample["vmhwm_bytes"] = None
                    except (OSError, ValueError) as error:
                        _record_error(errors, f"/proc/{process.pid}/status: {error}")
                        sample["rss_bytes"] = None
                        sample["vmhwm_bytes"] = None
                    try:
                        sizes = regular_file_sizes(run_dir, output_path)
                        temporary_bytes = sum(sizes.values())
                        report["temporary_observed_growth_bytes"] += sum(
                            max(size - previous_sizes.get(path, 0), 0)
                            for path, size in sizes.items()
                        )
                        previous_sizes = sizes
                        temporary_peak = max(temporary_peak or 0, temporary_bytes)
                        sample["temporary_bytes"] = temporary_bytes
                    except OSError as error:
                        _record_error(errors, f"temporary-file scan: {error}")
                        sample["temporary_bytes"] = None
                    sample["end_unix_ns"] = time.time_ns()
                    samples.write(json.dumps(sample, sort_keys=True) + "\n")
                    samples.flush()
                    report["sample_count"] += 1
                    remaining = interval_ms / 1000 - (time.monotonic() - sample_time)
                    if remaining > 0:
                        time.sleep(remaining)
            report["exit_code"] = process.wait()
        report["status"] = "complete" if report["exit_code"] == 0 and not errors else "incomplete"
    except KeyboardInterrupt:
        if process is not None:
            report["exit_code"] = terminate_child(process)
        _record_error(errors, "interrupted by KeyboardInterrupt")
    except Exception as error:
        if process is not None:
            report["exit_code"] = terminate_child(process)
        _record_error(errors, f"sampler: {type(error).__name__}: {error}")
    finally:
        report["elapsed_seconds"] = time.monotonic() - started
        report["rss_sampled_peak_bytes"] = rss_peak
        report["vmhwm_observed_peak_bytes"] = hwm_peak
        report["temporary_sampled_peak_bytes"] = temporary_peak
        report["output_oct_bytes"] = final_size(output_path)
        report["output_octattrs_bytes"] = final_size(output_path.with_suffix(".octattrs"))
        try:
            report.update(phase_peaks(run_dir / "stdout.log", samples_path))
        except (OSError, ValueError, KeyError, TypeError) as error:
            _record_error(errors, f"phase attribution: {error}")
        if report["exit_code"] == 0 and any(
            report[key] in (None, 0) for key in ("output_oct_bytes", "output_octattrs_bytes")
        ):
            _record_error(errors, "converter exited successfully without a nonempty output pair")
        if report["exit_code"] != 0 or errors:
            report["status"] = "incomplete"
        report["temporary_peak_note"] = "sampled logical sizes; may miss short peaks"
        report["temporary_growth_note"] = "sum of positive per-path size deltas between samples; not exact I/O"
        report["phase_peak_note"] = (
            "only samples wholly within progress intervals; empty phases have null peaks; "
            "VmHWM is process lifetime and is not attributed; Unix wall clock must remain stable"
        )
        write_json(run_dir / "report.json", report)
    return 0 if report["status"] == "complete" else 1


def _record_error(errors: list[str], message: str) -> None:
    if len(errors) < MAX_RECORDED_ERRORS:
        errors.append(message)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("run_dir", type=Path)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--divisions", type=int, default=2)
    parser.add_argument("--interval-ms", type=int, default=100)
    args = parser.parse_args(argv)
    if args.workers < 1 or args.divisions not in (2, 4, 8) or args.interval_ms < 1:
        parser.error("workers and interval-ms must be positive; divisions must be 2, 4, or 8")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        return profile(args.input, args.run_dir, args.binary, args.workers,
                       args.divisions, args.interval_ms)
    except (ValueError, FileExistsError, OSError) as error:
        print(error, file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
