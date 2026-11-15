import json,sys,pathlib
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); log=d['workerMetrics']['lodLog']; print(p.name,'mode',d['mode'],'reached',d['targetReached'],'load_s',round(d['loadElapsedMs']/1000,2),'async',d['buildWorkerMetrics']['asyncBlocks']['blocks'],'counters',d['counters'],'pendingEvictions',d['lodFinal']['pendingTargetEvictions'],'uncoveredMax',max((x['uncoveredNodes'] for x in log),default=None),'oversizedMax',max((x['oversizedVoxels'] for x in log),default=None),'dropped',d['workerMetrics']['lodLogDropped'],'errors',len(d['consoleErrors']))
for phase in d['phases']:
 rows=[x for x in log if phase['startEpoch']<=x['time']<=phase['endEpoch']]; inc=[(a['stagePx'],b['stagePx']) for a,b in zip(rows,rows[1:]) if b['stagePx']>a['stagePx']]; print(phase['label'],'p95/max',phase['summary']['intervalMs']['p95'],phase['summary']['intervalMs']['max'],'increases',inc)
pss=p.with_suffix('.pss.txt')
if pss.exists(): print('PSS KiB peak', max((int(x.split()[2]) for x in pss.read_text().splitlines() if x and not x.startswith('#')),default=0))
