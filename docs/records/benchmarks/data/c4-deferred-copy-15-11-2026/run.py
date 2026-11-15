import subprocess,sys,time,os,re,hashlib
exe,out=sys.argv[1],sys.argv[2]
fmt=sys.argv[3] if len(sys.argv)>3 else 'v7.5-pre'
L='/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets/belv_2009_digital-aerial_pcd_copc.laz'
for s in ('','attrs'):
    try: os.remove(out+('' if not s else 'attrs'))
    except FileNotFoundError: pass
t0=time.monotonic()
p=subprocess.Popen(['/usr/bin/time','-f','RSS %M',exe,L,out,'--format-version',fmt],stderr=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
marks=[];rss=None
import threading
def rd(f):
    global rss
    for line in f:
        line=line.strip()
        if line.startswith('RSS'): rss=int(line.split()[1])
        elif line and not re.match(r'^\d+/\d+ points',line) and not line.startswith(('Converted','Not pres')): marks.append((line,time.monotonic()-t0))
ths=[threading.Thread(target=rd,args=(p.stderr,)),threading.Thread(target=rd,args=(p.stdout,))]
[t.start() for t in ths];p.wait();[t.join() for t in ths]
tot=time.monotonic()-t0
d={}
for i,(n,t) in enumerate(marks[:-0 or None]):
    nxt=marks[i+1][1] if i+1<len(marks) else tot
    d[n]=nxt-t
print(' '.join(f'{k}={v:.2f}' for k,v in d.items()),f'total={tot:.2f} rss={rss}')
