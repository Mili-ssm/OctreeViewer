import csv, pathlib, statistics
p = pathlib.Path('/tmp/r3-width-lab-05-11-2026')
raw, audits, nodes = [], {}, {}
for line in (p/'raw.txt').read_text().splitlines():
    parts = line.split()
    if not parts: continue
    values = dict(item.split('=',1) for item in parts[1:] if '=' in item)
    if parts[0] == 'raw': raw.append(values)
    elif parts[0] == 'region_audit': audits[(values['path'],values['node'],values['width'],values['fast'])]=values
    elif parts[0].startswith('path=') and 'node' in values and 'leaf' in values:
        values['path']=parts[0][5:]; nodes[(values['path'],values['node'])]=values
assert len(raw)==2295 and len(audits)==238 and len(nodes)==17
base={(r['path'],r['node'],r['sample']):int(r['total_ns']) for r in raw if r['candidate']=='tuple'}
assert len(base)==17*9
out=[]
for key,node in nodes.items():
    path,nid=key
    nodebase=[base[(path,nid,str(i))] for i in range(9)]
    out.append(dict(dataset=path.rsplit('/',1)[-1],node=nid,leaf=node['leaf'],points=node['points'],candidate='tuple',width='',fast='',single='',median_ns=statistics.median(nodebase),paired_ratio=1,regions='',entry_bytes='',directory_payload_lower_bound='',btree_actual_bytes='unknown'))
    for width in ['8','10','12','15','16','18','21']:
        for fast in ['false','true']:
            rr=[r for r in raw if r['path']==path and r['node']==nid and r.get('width')==width and r.get('fast')==fast]
            assert len(rr)==9 and len({r['sample'] for r in rr})==9
            vals=[int(r['total_ns']) for r in rr]
            ratios=[int(r['total_ns']) / base[(path,nid,r['sample'])] for r in rr]
            a=audits[(path,nid,width,fast)]
            out.append(dict(dataset=path.rsplit('/',1)[-1],node=nid,leaf=node['leaf'],points=node['points'],candidate='region',width=width,fast=fast,single=a['single'],median_ns=statistics.median(vals),paired_ratio=round(statistics.median(ratios),4),regions=a['regions'],entry_bytes=a['entries_bytes'],directory_payload_lower_bound=a['directory_live_payload_lower_bound'],btree_actual_bytes='unknown'))
with (p/'per-node.csv').open('x',newline='') as f:
    writer=csv.DictWriter(f,fieldnames=list(out[0]));writer.writeheader();writer.writerows(out)
for dataset in sorted({x['dataset'] for x in out}):
    for fast in ['false','true']:
        print(dataset,'fast='+fast)
        for width in ['8','10','12','15','16','18','21']:
            rows=[x for x in out if x['dataset']==dataset and x['width']==width and x['fast']==fast]
            print(width,'median_of_node_paired_ratios',round(statistics.median(x['paired_ratio'] for x in rows),3),'range',round(min(x['paired_ratio'] for x in rows),3),round(max(x['paired_ratio'] for x in rows),3))
