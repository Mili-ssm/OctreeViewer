import datetime, os, pathlib, subprocess, sys, time
out = pathlib.Path('docs/records/benchmarks/data/r4-t6-02-11-2026')
scenario, px, limit, label = sys.argv[1:]
base = out / label
names = ('cargo', 'rustc', 'chrome', 'chromium', 'firefox')
checks = [(name, subprocess.run(['pgrep', '-ax', name], capture_output=True, text=True).stdout.strip()) for name in names]
base.with_suffix('.pre.txt').write_text(datetime.datetime.now().astimezone().isoformat()+'\n'+pathlib.Path('/proc/loadavg').read_text()+''.join(f'{n}: {v or "absent"}\n' for n,v in checks))
if any(v for _,v in checks): raise SystemExit('foreign process present; no run started')
cmd = ['node','examples/web-host/probe-orbit-load.mjs',f'target/render-data/{scenario}.oct',str(base)+'.json','--url=http://127.0.0.1:18767/examples/web-host/index.html',f'--px={px}',f'--max-load-s={limit}','--orbit-s=10','--browser=chrome']
if label.endswith('-pixels'): cmd.append('--pixels')
with open(str(base)+'.log','w') as log, open(str(base)+'.pss.txt','w') as pss:
 log.write('command: '+subprocess.list2cmdline(cmd)+'\n'); log.flush()
 proc=subprocess.Popen(cmd,stdout=log,stderr=subprocess.STDOUT)
 pss.write('# epoch_s chrome_processes pss_kib pids; descendants of probe node, 2s interval, excludes VRAM\n')
 while proc.poll() is None:
  tree={}
  for stat in pathlib.Path('/proc').glob('[0-9]*/stat'):
   try:
    s=stat.read_text(); tail=s[s.rfind(')')+2:].split(); tree[int(stat.parent.name)]=(int(tail[1]),s[s.find('(')+1:s.rfind(')')])
   except (OSError,ValueError,IndexError): pass
  descendants={proc.pid}; old=-1
  while old!=len(descendants):
   old=len(descendants); descendants.update(pid for pid,(ppid,_) in tree.items() if ppid in descendants)
  chrome=[pid for pid in descendants if tree.get(pid,(0,''))[1] in ('chrome','chromium')]; total=0; sampled=[]
  for pid in chrome:
   try:
    for line in pathlib.Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines():
     if line.startswith('Pss:'): total+=int(line.split()[1]); sampled.append(pid); break
   except OSError: pass
  pss.write(f'{time.time():.3f} {len(sampled)} {total} {",".join(map(str,sorted(sampled)))}\n'); pss.flush(); time.sleep(2)
 sys.exit(proc.returncode)
