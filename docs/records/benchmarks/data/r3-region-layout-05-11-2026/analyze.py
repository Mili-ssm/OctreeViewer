import sys, statistics as st
def blocks(path):
    out, name, hdr = {}, None, None
    for line in open(path):
        line = line.rstrip('\n')
        if line.startswith('# block'): name = line.split()[2]; hdr = None; out[name] = []; continue
        if line.startswith('#') or not line or name is None: continue
        cols = line.split('\t')
        if hdr is None: hdr = cols; continue
        out[name].append(dict(zip(hdr, cols)))
    return out
for ds in sys.argv[1:]:
    b = blocks(f"{ds}.tsv")
    L = {(r['W'], r['F'], r['model'], r['group']): r for r in b['layout']}
    enc = {}
    for r in b['encode_time']:
        enc.setdefault((r['W'], r['F']), []).append(int(r['encode_ns'])); build = int(r['build_ns'])
    def gpu(w, f, m, g):
        r = L[(w, f, m, g)]; return int(r['pages'])*(8192+32)
    base = {g: gpu('10','page','b_greedy',g) for g in ('proxy_near','proxy_far')}
    base_c = build + st.median(enc[('10','page')])
    print(f"== {ds}: build {build/1e9:.1f} s")
    print("W\tF\tB/cell near(a|b)\tB/cell far(b)\ttail% near b\tpages near b\tencode s med [min-max]\tencode/build\tscore")
    for (w, f) in sorted(enc, key=lambda k:(int(k[0]), k[1])):
        rn = L[(w,f,'b_greedy','proxy_near')]; ra = L[(w,f,'a_aligned','proxy_near')]; rf = L[(w,f,'b_greedy','proxy_far')]
        tail = int(rn['tail_bytes'])/(int(rn['pages'])*8192)*100
        e = enc[(w,f)]; c = build + st.median(e)
        gn = gpu(w,f,'b_greedy','proxy_near')/base['proxy_near']; gf = gpu(w,f,'b_greedy','proxy_far')/base['proxy_far']
        score = ((gn+gf)/2 + c/base_c)/2
        print(f"{w}\t{f}\t{float(ra['bytes_per_cell']):.2f}|{float(rn['bytes_per_cell']):.2f}\t{float(rf['bytes_per_cell']):.2f}\t{tail:.1f}\t{rn['pages']}\t{st.median(e)/1e9:.3f} [{min(e)/1e9:.3f}-{max(e)/1e9:.3f}]\t{st.median(e)/build:.4f}\t{score:.4f} (gpu {(gn+gf)/2:.4f})")
    print("mask4:", ' '.join(f"{r['W']}{r['F'][0]}={float(r['mean_occupied_fraction']):.3f}" for r in b['mask4'] if r['F']=='page'))
    mb = {}
    for r in b['max_relative_bits']: mb.setdefault(r['class'], []).append(int(r['bits']))
    print("max relative bits:", {k: max(v) for k, v in mb.items()})
    for g in ('internal','leaf_finest'):
        print(g, ' '.join(f"{w}{f[0]}:{float(L[(w,f,'b_greedy',g)]['bytes_per_cell']):.2f}" for (w,f) in sorted(enc, key=lambda k:(int(k[0]),k[1]))))
