"""Summarise probe-cube-pool JSON (gzip or plain): median GPU ms over all frames, per-round medians, A/A spread."""
import gzip, json, statistics as st, sys

def load(path):
    opener = gzip.open if path.endswith('.gz') else open
    with opener(path, 'rt') as f:
        return json.load(f)

for path in sys.argv[1:]:
    d = load(path)
    print(f"# {path}: failures {d['failures']}, console errors {len(d['consoleErrors'])}")
    for case, c in d['cases'].items():
        by = {}
        for r in c['rounds']:
            for v in r['variants']:
                by.setdefault(v['name'], []).append(v)
        for name, vs in by.items():
            meds = [st.median(v['series']['gpuMs']) for v in vs]
            frames = [x for v in vs for x in v['series']['gpuMs']]
            enc = st.median([x for v in vs for x in v['series']['encodeMs']])
            aa = (max(meds) - min(meds)) / st.median(meds) * 100 if st.median(meds) else 0.0
            print(f"{case:28} {name:18} gpu_med_ms {st.median(frames):8.3f} "
                  f"rounds {','.join(f'{m:.3f}' for m in meds)} aa_pct {aa:5.1f} "
                  f"encode_ms {enc:.3f} draws {vs[0]['draws']} px {vs[0]['nonBlackPixels']}")
