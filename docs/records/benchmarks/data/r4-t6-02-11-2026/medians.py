import json,sys,statistics as st,pathlib,glob
base=pathlib.Path(sys.argv[1])
def rng(v): return f"{st.median(v):.4g} [{min(v):.4g}-{max(v):.4g}]"
for sc in ('majadas-v74-4','belvedere-v74-8','belvedere-v74-4'):
    rows=[]
    for s in (1,2,3):
        p=base/f'{sc}-s{s}.json'; d=json.loads(p.read_text()); ph={x['label']:x['summary']['intervalMs'] for x in d['phases']}
        pss=max(int(x.split()[2]) for x in p.with_suffix('.pss.txt').read_text().splitlines() if x and not x.startswith('#'))/1024
        a=d['workerMetrics']['sceneAgeMs']; log=d['workerMetrics']['lodLog']
        rows.append(dict(load=d['loadElapsedMs']/1000,reached=d['targetReached'],p95=max(v['p95'] for v in ph.values()),mx=max(v['max'] for v in ph.values()),
          builds=d['counters']['sceneBuilds'],vsp=int(d['counters']['voxelSourcePoints'])/1e6,age95=a['p95'],agemax=a['max'],pss=pss,
          unc=d['counters']['uncoveredNodes'],uncmax=max(x['uncoveredNodes'] for x in log),ovs=d['counters']['oversizedVoxels'],ev=d['lodFinal']['pendingTargetEvictions'],evp=d['counters']['evictedPendingBlocks']))
    print(sc,'reached',[r['reached'] for r in rows])
    for k in rows[0]:
        if k!='reached': print('  ',k,rng([r[k] for r in rows]))
for p in sorted(base.glob('*-pixels.json')):
    d=json.loads(p.read_text())
    for ph in d['phases']:
        f=[c['emptyFraction'] for c in ph['emptyPixelCaptures'] or []]
        print(p.name,ph['label'],'emptyFraction hull',[round(x,3) for x in f])
