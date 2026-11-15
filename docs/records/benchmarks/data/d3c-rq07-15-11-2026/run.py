"""RQ-07 harness: interleaved V5 / V7 conversions, one JSON line per conversion.
Usage: run.py EXE OUTDIR ROUNDS RUNS_JSONL  (run from the worktree root)."""
import json, os, subprocess, sys, time
exe, outdir, rounds, jsonl = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
L = '/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets/belv_2009_digital-aerial_pcd_copc.laz'
V = {'v5': ['--format-version', 'v5'], 'v7': ['--format-version', 'v7'],
     'v7.5-pre-hybrid': ['--format-version', 'v7.5-pre', '--row-order', 'hybrid']}
def run(variant, rnd):
    out = os.path.join(outdir, 'out.oct')
    for p in (out, out + 'attrs'):
        if os.path.exists(p): os.remove(p)
    tf = os.path.join(outdir, 'time.txt')
    t0 = time.monotonic()
    r = subprocess.run(['/usr/bin/time', '-f', '%M', '-o', tf, exe, L, out] + V[variant],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wall = time.monotonic() - t0
    rss = int(open(tf).read().split()[-1])
    sz = [os.path.getsize(p) if os.path.exists(p) else None for p in (out, out + 'attrs')]
    d = {'variant': variant, 'round': rnd, 'rc': r.returncode, 'wall_s': round(wall, 3),
         'peak_rss_kib': rss, 'oct_bytes': sz[0], 'octattrs_bytes': sz[1],
         'loadavg_at_end': os.getloadavg()}
    open(jsonl, 'a').write(json.dumps(d) + '\n'); print(d, flush=True)
    for p in (out, out + 'attrs'):
        if os.path.exists(p): os.remove(p)
for i in range(1, rounds + 1):
    order = ['v5', 'v7'] if i % 2 else ['v7', 'v5']
    for v in order: run(v, i)
run('v7.5-pre-hybrid', 0)
