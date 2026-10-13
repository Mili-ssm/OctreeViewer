#!/usr/bin/env python3
"""Download a pinned, partial three-node USGS EPT sample for Additive diagnostics."""

from __future__ import annotations

import argparse
import hashlib
import json
import tempfile
import urllib.request
from pathlib import Path

SOURCE = (
    "https://s3-us-west-2.amazonaws.com/usgs-lidar-public/NC_HurricaneFlorence_1_2020/"
)
# Original payloads, not synthesized points. Keep every selected node's parent.
NODES = {
    "0-0-0-0": (
        6518,
        "a7d60b640ca060c98e5b579d491f6c17781e13791d3dbc99ea998cf3ca84072c",
    ),
    "1-0-0-0": (
        1822,
        "201729aa4d47b3fd3680dcb2560423635498f0e8ce3b187f46f6d75ffd7fb4e2",
    ),
    "2-0-1-1": (
        544,
        "b31c5602258ca106a3108f76e228238645efe51f914875528d7464a615714f7f",
    ),
}
MAX_FILE_BYTES = 1024 * 1024


def download(relative: str) -> bytes:
    with urllib.request.urlopen(SOURCE + relative, timeout=30) as response:
        data = response.read(MAX_FILE_BYTES + 1)
    if len(data) > MAX_FILE_BYTES:
        raise ValueError(f"source file exceeds {MAX_FILE_BYTES} bytes: {relative}")
    return data


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def prepare(output: Path) -> dict:
    if output.exists():
        raise FileExistsError(f"output already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    # Publish only after every payload and count has been verified.
    with tempfile.TemporaryDirectory(
        prefix="additive-ept-", dir=output.parent
    ) as temporary:
        root = Path(temporary) / "fixture"
        (root / "ept-data").mkdir(parents=True)
        (root / "ept-hierarchy").mkdir()
        source_manifest = download("ept.json")
        source_hierarchy = download("ept-hierarchy/0-0-0-0.json")
        manifest = json.loads(source_manifest)
        hierarchy = json.loads(source_hierarchy)
        if manifest["dataType"] != "laszip" or manifest["hierarchyType"] != "json":
            raise ValueError("source no longer uses laszip and JSON hierarchy")
        files = {}
        for key, (count, expected_sha256) in NODES.items():
            if hierarchy.get(key) != count:
                raise ValueError(f"source count changed for {key}")
            data = download(f"ept-data/{key}.laz")
            digest = hashlib.sha256(data).hexdigest()
            if digest != expected_sha256:
                raise ValueError(f"source payload changed for {key}")
            (root / "ept-data" / f"{key}.laz").write_bytes(data)
            files[key] = {"points": count, "bytes": len(data), "sha256": digest}
        source_points = manifest["points"]
        manifest["points"] = sum(count for count, _ in NODES.values())
        write_json(root / "ept.json", manifest)
        write_json(
            root / "ept-hierarchy" / "0-0-0-0.json",
            {key: count for key, (count, _) in NODES.items()},
        )
        (root / "source-ept.json").write_bytes(source_manifest)
        (root / "source-root-hierarchy.json").write_bytes(source_hierarchy)
        provenance = {
            "source": SOURCE,
            "partial_subset": True,
            "scope": "Three ancestor-connected nodes; not full source coverage. Bounds/schema unchanged; points and hierarchy filtered.",
            "source_points": source_points,
            "selected_points": manifest["points"],
            "source_manifest_sha256": hashlib.sha256(source_manifest).hexdigest(),
            "source_hierarchy_sha256": hashlib.sha256(source_hierarchy).hexdigest(),
            "nodes": files,
        }
        write_json(root / "provenance.json", provenance)
        if output.exists():
            raise FileExistsError(f"output already exists: {output}")
        root.rename(output)
    return provenance


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(json.dumps(prepare(args.output), indent=2))


if __name__ == "__main__":
    main()
