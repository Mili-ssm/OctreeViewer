#!/usr/bin/env python3
"""Summarize exported web viewer benchmark JSON files as a Markdown or CSV table."""

from __future__ import annotations

import argparse
import csv
import json
import re
import statistics
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any


def comparable(run: dict[str, Any]) -> bool:
    stable = run.get("steadyState", {})
    scene = run.get("scene") or {}
    refinement = run.get("refinement")
    if "refinement" in run and (
        not refinement
        or refinement.get("targetLodMs") is None
        or refinement.get("colorMode") != run.get("configuration", {}).get("colorMode")
    ):
        return False
    return (
        not stable.get("cameraChanged", True)
        and not stable.get("viewportChanged", True)
        and not stable.get("settingsChanged", True)
        and scene.get("coverageComplete") is True
        and scene.get("pendingBlocks") == 0
        and stable.get("measurementWindowMs", 0) >= 10_000
        and stable.get("meanFps") is not None
    )


def median(values: list[float | int | None]) -> str:
    numbers = [float(value) for value in values if isinstance(value, (float, int))]
    if not numbers:
        return "—"
    result = f"{statistics.median(numbers):.2f}"
    if len(numbers) > 1:
        result += f" [{min(numbers):.2f}–{max(numbers):.2f}]"
    return result


def run_group_key(run: dict[str, Any]) -> tuple[Any, ...]:
    dataset = run.get("dataset", {})
    configuration = run.get("configuration", {})
    environment = run.get("environment", {})
    adapter = environment.get("gpuStats", {}).get("adapter", {}) or {}
    attributes = dataset.get("attributes")
    attribute_key = (
        ("sidecar", attributes.get("name"), attributes.get("fileBytes"))
        if attributes is not None
        else ("none" if "attributes" in dataset else "unknown", None, None)
    )
    return (
        environment.get("userAgent", "unknown browser"),
        adapter.get("name", "unknown GPU"),
        dataset.get("name", "unknown dataset"),
        dataset.get("sourcePointCount"),
        dataset.get("fileBytes"),
        tuple(configuration.get("viewportFramebuffer", [])),
        configuration.get("targetVoxelPixels"),
        configuration.get("colorMode"),
        configuration.get("depthShading"),
        attribute_key,
        (run.get("refinement") or {}).get("trigger", "dataset"),
    )


def collect(paths: list[Path]) -> list[list[str]]:
    groups: dict[tuple[Any, ...], list[dict[str, Any]]] = defaultdict(list)
    for path in paths:
        run = json.loads(path.read_text(encoding="utf-8"))
        if run.get("schemaVersion") != 1:
            raise ValueError(f"{path}: unsupported benchmark schema")
        groups[run_group_key(run)].append(run)

    rows: list[list[str]] = []
    for key, runs in sorted(groups.items(), key=lambda entry: str(entry[0])):
        valid = [run for run in runs if comparable(run)]
        attribute_kind, attribute_name, attribute_bytes = key[9]
        if attribute_kind == "sidecar":
            attribute_label = f"{attribute_name} ({attribute_bytes} B)"
        elif attribute_kind == "none":
            attribute_label = "none"
        else:
            attribute_label = "unknown (legacy)"
        configuration_labels = [attribute_label, str(key[7] or "—"), str(key[8]), str(key[10])]
        if not valid:
            rows.append([
                str(key[0]), str(key[1]), str(key[2]), str(key[3] or "—"),
                str(key[4] or "—"), "×".join(map(str, key[5])) or "—",
                str(key[6] or "—"), f"0/{len(runs)}", *(["—"] * 8),
                *configuration_labels,
            ])
            continue
        loads = [run.get("load", {}) for run in valid]
        timings = [
            run.get("refinement") or {
                "firstFrameMs": run.get("load", {}).get("firstCoarseFrameMs"),
                "targetLodMs": run.get("load", {}).get("targetLodMs"),
            }
            for run in valid
        ]
        steady = [run.get("steadyState", {}) for run in valid]
        scenes = [run.get("scene", {}) or {} for run in valid]
        environments = [run.get("environment", {}) for run in valid]
        rows.append([
            str(key[0]),
            str(key[1]),
            str(key[2]),
            str(key[3] or "—"),
            str(key[4] or "—"),
            "×".join(map(str, key[5])) or "—",
            str(key[6] or "—"),
            f"{len(valid)}/{len(runs)}",
            median([load.get("metadataOpenMs") for load in loads]),
            median([timing.get("firstFrameMs") for timing in timings]),
            median([timing.get("targetLodMs") for timing in timings]),
            median([steady.get("meanFps") for steady in steady]),
            median([steady.get("frameReplyIntervalP95Ms") for steady in steady]),
            median([steady.get("workerRenderCallP95Ms") for steady in steady]),
            median([scene.get("renderedVoxels") for scene in scenes]),
            median([environment.get("jsHeapUsedBytes") for environment in environments]),
            *configuration_labels,
        ])
    return rows


NUMERIC_CELL = re.compile(r"^-?\d+(\.\d+)?( \[-?\d+(\.\d+)?–-?\d+(\.\d+)?\])?$")


def csv_cell(value: Any) -> Any:
    """Neutralize spreadsheet formulas in text cells; numbers stay untouched."""
    if not isinstance(value, str) or NUMERIC_CELL.match(value):
        return value
    return "'" + value if value[:1] in ("=", "+", "-", "@", "\t", "\r") else value


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("runs", nargs="+", type=Path, help="exported web-bench-*.json files")
    parser.add_argument("--format", choices=("markdown", "csv"), default="markdown")
    args = parser.parse_args()
    headers = [
        "Browser", "GPU adapter", "Dataset", "Points", "File bytes", "Viewport", "Target px",
        "Valid runs", "Metadata ms", "First phase frame ms", "Target LOD ms", "FPS", "Reply p95 ms",
        "CPU frame p95 ms", "Voxels", "JS heap bytes", "Attributes", "Color", "Depth shading", "Timing phase",
    ]
    rows = collect(args.runs)
    if args.format == "csv":
        writer = csv.writer(sys.stdout)
        writer.writerow(headers)
        writer.writerows([csv_cell(cell) for cell in row] for row in rows)
        return
    print("| " + " | ".join(headers) + " |")
    print("| " + " | ".join("---" for _ in headers) + " |")
    for row in rows:
        print("| " + " | ".join(value.replace("|", "\\|") for value in row) + " |")


if __name__ == "__main__":
    main()
