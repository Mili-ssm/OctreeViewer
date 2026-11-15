#!/usr/bin/env python3
"""Summary table of the R3 B6 matrix (target/r3-b6/*.json, written by probe-orbit-load.mjs)."""
import glob, json, os, sys

d = sys.argv[1] if len(sys.argv) > 1 else "target/r3-b6"
f = lambda v, n=1: "-" if v is None else f"{v:.{n}f}"
rows = []
for path in sorted(glob.glob(os.path.join(d, "*.json"))):
    r = json.load(open(path))
    if "phases" not in r and "error" not in r:
        continue
    name = os.path.basename(path)[:-5]
    ph = {p["label"]: p for p in r.get("phases", [])}
    iv = lambda l: ((ph[l].get("summary") or {}).get("intervalMs") or {} if l in ph else {})
    ll = lambda l, k: (ph[l].get("lodLog") or {}).get(k) if l in ph else None
    gpu = lambda l: (ph[l].get("gpuFrameMs") or {}).get("p95") if l in ph else None
    px = r.get("finalEmptyPixels") or {}
    pss = r.get("pss") or {}
    g = r.get("gtt") or {}
    rows.append([name, "Y" if r.get("targetReached") else "N", f(r.get("timeToReachedMs"), 0) if "timeToReachedMs" in r else f((r.get("remote") or {}).get("timeToReachedFromFirstGetMs"), 0),
        f(r.get("pendingDrainedMs"), 0),
        f"{f(iv('during-load').get('p95'))}/{f(iv('during-load').get('max'))}", f"{f(iv('after-target').get('p95'))}/{f(iv('after-target').get('max'))}",
        f"{ll('during-load','stageRises')}/{ll('after-target','stageRises')}", f"{ll('during-load','uncoveredMax')}/{ll('after-target','uncoveredMax')}",
        f(px.get("emptyFraction", px.get("fraction")), 3), pss.get("treePeakMiB"), (pss.get("peakByTypeMiB") or {}).get("renderer"),
        f"{f(gpu('during-load'), 2)}/{f(gpu('after-target'), 2)}", g.get("peakDeltaMiB"), g.get("idleChromeDeltaMiB"), "ERR" if r.get("error") else ""])
hdr = ["run", "reached", "t_reach ms", "drained ms", "RQ01 load p95/max", "RQ01 orbit p95/max", "stage rises load/orbit", "uncovered max load/orbit", "empty frac", "PSS tree MiB", "PSS renderer MiB", "GPU p95 load/orbit", "GTT peak MiB", "GTT idle-chrome MiB", "err"]
print("| " + " | ".join(hdr) + " |\n|" + "---|" * len(hdr))
for row in rows:
    print("| " + " | ".join(str(c) for c in row) + " |")
