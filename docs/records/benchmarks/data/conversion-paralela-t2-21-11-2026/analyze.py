#!/usr/bin/env python3
"""Summarise one run (LABEL.log.xz/.tsv/.meta) as a JSON line, with the node-event model.
Usage: analyze.py LABEL... (prints JSON lines). Model: list scheduling of the depth>=1 node events
(durations = own elapsed_ns, children ready when the parent ends, FIFO in preorder) on P threads."""
import heapq, json, lzma, re, sys, os
from collections import deque

def load(label):
    d = os.path.dirname(os.path.abspath(__file__))
    kv, events = {}, []
    for line in lzma.open(f"{d}/{label}.log.xz", "rt"):
        line = line.rstrip("\n")
        if line.startswith("internal_spatial_node_event="):
            p = line.split("=", 1)[1].split(",")
            events.append(dict(id=int(p[0]), parent=None if p[1] == "-" else int(p[1]), depth=int(p[2]),
                               inp=int(p[3]), lod=int(p[4]), leaf=p[5] == "1", ns=int(p[7]),
                               child_bytes=[int(x) for x in p[10].split(";")] if p[10] else []))
        elif "=" in line:
            k, v = line.split("=", 1)
            kv[k] = v
    meta = dict(l.strip().split("=", 1) for l in open(f"{d}/{label}.meta") if "=" in l)
    rows = [list(map(float, l.split("\t"))) for l in open(f"{d}/{label}.tsv") if l.count("\t") == 7]
    return kv, events, meta, rows

def simulate(events, threads):
    nodes = {e["id"]: e for e in events if e["depth"] >= 1}
    kids = {}
    for e in nodes.values():
        kids.setdefault(e["parent"], []).append(e["id"])
    ready = sorted(i for i, e in nodes.items() if e["depth"] == 1)
    busy, t, segments = [], 0, []  # busy: heap of (end, id)
    ready_q = deque(ready)
    while ready_q or busy:
        while ready_q and len(busy) < threads:
            i = ready_q.popleft()
            heapq.heappush(busy, (t + nodes[i]["ns"], i))
        end, i = heapq.heappop(busy)
        n_busy = len(busy) + 1
        segments.append((end - t, n_busy))
        t = end
        for c in sorted(kids.get(i, [])):
            ready_q.append(c)
        # finished jobs with the same end are handled by the next iterations (zero-length segments)
    return t, segments

def critical_path(events):
    # Children come from parent links: the event's last field is child spool bytes, not ids.
    nodes = {e["id"]: e for e in events if e["depth"] >= 1}
    kids = {}
    for e in nodes.values():
        kids.setdefault(e["parent"], []).append(e["id"])
    best = {}
    for i in sorted(nodes, reverse=True):  # preorder ids: children have larger ids
        best[i] = nodes[i]["ns"] + max((best[c] for c in kids.get(i, [])), default=0)
    return max(best.values()) if best else 0

def summarise(label, model=True):
    kv, events, meta, rows = load(label)
    f = lambda k: float(kv[k]) if k in kv else None
    out = dict(label=label, total_ms=f("conversion_elapsed_ms"), sha=meta.get("sha"), exit=meta.get("exit"),
               load_before=meta.get("load_before"))
    stages = {k[6:-3]: float(v) for k, v in kv.items() if re.fullmatch(r"stage_\w+_ms", k)}
    out["stages_ms"] = stages
    out["metadata_ms"] = sum(stages.get(k, 0) for k in ("BeforeFirstProgress", "InspectingInput", "PlanningMetadata"))
    if rows:
        a, b = rows[0], rows[-1]
        dur = b[0] - a[0]
        out["sample_s"] = round(dur, 1)
        out["system_cores"] = round((b[2] - a[2]) / 100 / dur, 2)
        out["iowait_cores"] = round((b[3] - a[3]) / 100 / dur, 2)
        out["kthread_cpu_s"] = round((b[4] - a[4]) / 100, 2)
        out["psi_io_some_pct"] = round((b[5] - a[5]) / 1e4 / dur, 2)
        out["psi_io_full_pct"] = round((b[6] - a[6]) / 1e4 / dur, 2)
        out["load_end"] = b[7]
    io = {}
    for k, v in kv.items():
        m = re.fullmatch(r"phase_(\d+)_io_(read|write)_bytes", k)
        if m:
            io[m.group(2)] = io.get(m.group(2), 0) + int(v)
    out["proc_io_total_read_bytes"], out["proc_io_total_write_bytes"] = io.get("read"), io.get("write")
    if model and events:
        d1 = [e for e in events if e["depth"] == 1]
        tot = sum(e["inp"] for e in d1) or 1
        out["root_children"] = len(d1)
        out["root_child_share_bytes"] = sorted((round(e["inp"] / tot, 3) for e in d1), reverse=True)
        out["root_own_s"] = round(next(e["ns"] for e in events if e["depth"] == 0) / 1e9, 2)
        out["nodes"] = len(events)
        work = sum(e["ns"] for e in events if e["depth"] >= 1)
        out["hier_work_s"] = round(work / 1e9, 2)
        out["hier_critical_path_s"] = round(critical_path(events) / 1e9, 2)
        sim = {}
        for p in (1, 2, 4, 8, 16):
            t, seg = simulate(events, min(p, len(d1)) if p > 1 else 1)
            sim[p] = dict(makespan_s=round(t / 1e9, 2),
                          below_P_pct=round(100 * sum(s for s, n in seg if n < min(p, len(d1))) / t, 1) if t else 0)
        out["sim_threads"] = sim
        free = {}
        for p in (4, 8, 16):
            t, seg = simulate(events, p)
            free[p] = dict(makespan_s=round(t / 1e9, 2),
                           below_P_pct=round(100 * sum(s for s, n in seg if n < p) / t, 1) if t else 0)
        out["sim_free_threads"] = free
    return out

if __name__ == "__main__":
    for l in sys.argv[1:]:
        print(json.dumps(summarise(l)))
