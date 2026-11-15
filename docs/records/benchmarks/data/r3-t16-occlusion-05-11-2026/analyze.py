"""Summarise the R3 Task 16a runs: python3 -I analyze.py final.json.gz check.json.gz

Static variants: median over rounds of the per-round GPU median, with A/A = (max - min) / median of
those round medians in parentheses. Moving variants: median over rounds of the warm median (frames
1..119) with its A/A, the worst p95 of any round and the worst cold frame (first frame after a
variant switch). The last column is the worst static A/A of the case.
"""
import gzip
import json
import math
import statistics
import sys
from collections import defaultdict
from functools import reduce

STATIC = ["w16-ranges", "k", "kp", "kpc"]
MOVING = ["w16-ranges-moving", "k-moving", "kp-moving", "kpc-moving"]


def load(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as handle:
        return json.load(handle)


def spread(values):
    median = statistics.median(values)
    return (max(values) - min(values)) / median if median else 0.0


def timing(path):
    data = load(path)
    gcds = []
    print(f"# {path}: {data['browserVersion']}, rounds {data['args']['rounds']}")
    print("case | " + " | ".join(STATIC) + " | " + " | ".join(m + " (med/p95/cold)" for m in MOVING) + " | worst A/A")
    for name, case in data["cases"].items():
        static, moving = defaultdict(list), defaultdict(list)
        for round_ in case["rounds"]:
            for v in round_["variants"]:
                if v.get("tickGcd"):
                    gcds.append(v["tickGcd"])
                if v["moving"]:
                    moving[v["name"]].append((v["warm"]["median"], v["warm"]["p95"], v["cold"]["gpuMs"]))
                else:
                    static[v["name"]].append(v["summary"]["gpuMs"]["median"])
        cells, worst_aa = [], 0.0
        for variant in STATIC:
            values = static[variant]
            median = statistics.median(values)
            worst_aa = max(worst_aa, (max(values) - min(values)) / median if median else 0.0)
            cells.append(f"{median:.2f} ({spread(values) * 100:.1f} %)")
        for variant in MOVING:
            values = moving[variant]
            cells.append(
                f"{statistics.median(v[0] for v in values):.2f} ({spread([v[0] for v in values]) * 100:.1f} %)"
                f"/{max(v[1] for v in values):.2f}/{max(v[2] for v in values):.2f}"
            )
        print(f"{name} | " + " | ".join(cells) + f" | {worst_aa * 100:.1f} %")
    if gcds:
        print(f"# tick GCD over all variants: {reduce(math.gcd, gcds)}")
    print(f"# failures: {len(data['failures'])} ({sorted({f.split(' ')[0] for f in data['failures']})})")


def check(path):
    data = load(path)
    print(f"# {path}")
    print("case | k-nofrustum = drawn_cells | holes | shallower | min phase-1 cells kp/kpc | empty reference frames | max colour diff")
    for name, case in data["cases"].items():
        t = next(v for v in case.values() if isinstance(v, dict) and "count" in v)
        moving = t["moving"]
        holes = sum(f["holes"] for k in t["static"] for f in t["static"][k]["frames"]) + sum(m["holes"] for m in moving.values())
        shallower = sum(f["shallower"] for k in t["static"] for f in t["static"][k]["frames"]) + sum(
            m["shallower"] for m in moving.values()
        )
        colour = max(m["colorDiffFractionMax"] for m in moving.values())
        print(
            f"{name} | {t['count']['ok']} | {holes} | {shallower} | "
            f"{moving['kp'].get('minPhase1Cells')}/{moving['kpc'].get('minPhase1Cells')} | "
            f"{moving['k'].get('emptyRefFrames')} | {colour * 100:.3f} %"
        )
    print(f"# failures: {len(data['failures'])} ({sorted({f.split(' ')[0] for f in data['failures']})})")


if __name__ == "__main__":
    timing(sys.argv[1])
    check(sys.argv[2])
