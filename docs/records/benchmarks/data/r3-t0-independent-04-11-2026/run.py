import datetime, json, os, pathlib, subprocess, time
p=pathlib.Path('target/r3-t0-independent-04-11-2026')
log=(p/'http.log').open('x')
server=subprocess.Popen(['python3','-m','http.server','18767','--bind','127.0.0.1'],stdout=log,stderr=subprocess.STDOUT)
def processes():
    rows=subprocess.check_output(['ps','-eo','pid=,ppid=,comm='],text=True).splitlines()
    return {int(a):(int(b),c) for a,b,c in (row.split(None,2) for row in rows)}
def foreign(root):
    procs=processes(); bad=[]
    for pid,(ppid,comm) in procs.items():
        if comm not in ('chrome','chromium','firefox','cargo','rustc'): continue
        ancestor=pid; seen=set()
        while ancestor in procs and ancestor not in seen and ancestor != root:
            seen.add(ancestor); ancestor=procs[ancestor][0]
        if ancestor != root: bad.append((pid,comm))
    return bad
try:
    time.sleep(1)
    if server.poll() is not None: raise RuntimeError('HTTP server exited')
    for round in (1,2,3):
        for dataset,px in [('majadas',4),('belvedere',4),('belvedere',8)]:
            name=f'{dataset}-{px}-r{round}'
            bad=foreign(-1)
            if bad: raise RuntimeError(f'foreign processes before {name}: {bad}')
            args=['node','examples/web-host/probe-orbit-load.mjs',f'target/render-data/{dataset}-v75.oct',str(p/f'{name}.json'),'--url=http://127.0.0.1:18767/examples/web-host/index.html',f'--px={px}','--max-load-s=300','--orbit-s=10','--browser=chrome']
            (p/f'{name}.command').write_text(' '.join(args)+'\n')
            print('START',name,datetime.datetime.now().isoformat(),flush=True)
            with (p/f'{name}.log').open('x') as out:
                proc=subprocess.Popen(args,stdout=out,stderr=subprocess.STDOUT,start_new_session=True)
                while proc.poll() is None:
                    bad=foreign(proc.pid)
                    if bad:
                        os.killpg(proc.pid,15); proc.wait(); raise RuntimeError(f'foreign processes during {name}: {bad}')
                    if server.poll() is not None:
                        os.killpg(proc.pid,15); proc.wait(); raise RuntimeError('HTTP server exited')
                    time.sleep(.5)
            if proc.returncode: raise RuntimeError(f'{name} exit {proc.returncode}')
            j=json.loads((p/f'{name}.json').read_text())
            if j.get('mode')!='timing' or j.get('functionalReasons') or not j.get('targetReached') or j.get('error') or j['environment']['wasmSha256']!='ad1620503a08db0bfa64031ab1036b15ff0991f9e29ec7210a1c03f9b01afc6b': raise RuntimeError(f'{name} failed acceptance')
            print('OK',name,'consoleErrors=',json.dumps(j.get('consoleErrors')),flush=True)
finally:
    server.terminate(); server.wait(); log.close()
