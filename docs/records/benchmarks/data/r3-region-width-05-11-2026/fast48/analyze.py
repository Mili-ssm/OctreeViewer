import csv, pathlib, statistics
p=pathlib.Path('/tmp/r3-width-lab-fast48-05-11-2026'); raw=[]; audits={}; nodes={}
for line in (p/'raw.txt').read_text().splitlines():
 t=line.split()
 if not t: continue
 v=dict(x.split('=',1) for x in t[1:] if '=' in x)
 if t[0]=='raw': raw.append(v)
 elif t[0]=='region_audit': audits[(v['path'],v['node'],v['width'],v['fast'])]=v
 elif t[0].startswith('path=') and 'node' in v and 'leaf' in v: v['path']=t[0][5:]; nodes[(v['path'],v['node'])]=v
assert len(raw)==2448 and len(audits)==238 and len(nodes)==17
base={(r['path'],r['node'],r['sample']):int(r['total_ns']) for r in raw if r['candidate']=='fast48'}
assert len(base)==153
rows=[]
for (path,nid),node in nodes.items():
 for typ,width,fast in [('legacy80','',''),('fast48','','')]+[('region',str(w),f) for w in [8,10,12,15,16,18,21] for f in ['false','true']]:
  rr=[r for r in raw if r['path']==path and r['node']==nid and r['candidate']==typ and (typ!='region' or (r['width']==width and r['fast']==fast))]
  assert len(rr)==9 and len({r['sample'] for r in rr})==9
  ratios=[int(r['total_ns'])/base[(path,nid,r['sample'])] for r in rr]
  a=audits[(path,nid,width,fast)] if typ=='region' else {}
  rows.append(dict(dataset=path.rsplit('/',1)[-1],node=nid,leaf=node['leaf'],points=node['points'],candidate=typ,width=width,fast=fast,single=a.get('single',''),prep_median_ns=statistics.median(int(r['prep_ns']) for r in rr),sort_median_ns=statistics.median(int(r['sort_ns']) for r in rr),consume_median_ns=statistics.median(int(r['consume_ns']) for r in rr),total_median_ns=statistics.median(int(r['total_ns']) for r in rr),ratio_median=round(statistics.median(ratios),4),ratio_min=round(min(ratios),4),ratio_max=round(max(ratios),4),regions=a.get('regions',''),entry_bytes=a.get('entries_bytes',rr[0].get('entry_bytes','')),directory_live_payload_lower_bound=a.get('directory_live_payload_lower_bound',''),btree_actual_bytes='unknown' if typ=='region' else 'n/a'))
with (p/'per-node.csv').open('x',newline='') as f:
 w=csv.DictWriter(f,fieldnames=list(rows[0]));w.writeheader();w.writerows(rows)
for key in nodes:
 path,nid=key; subset=[r for r in rows if r['dataset']==path.rsplit('/',1)[-1] and r['node']==nid]; base_row=next(r for r in subset if r['candidate']=='fast48'); best=min((r for r in subset if r['candidate']=='region'),key=lambda r:r['ratio_median'])
 print(base_row['dataset'],nid,'leaf='+base_row['leaf'],'points='+base_row['points'],'fast48_ns='+str(base_row['total_median_ns']),'best_width='+best['width'],'fast='+best['fast'],'ratio='+str(best['ratio_median']),'sample_range='+str(best['ratio_min'])+'..'+str(best['ratio_max']),'regions='+best['regions'])
