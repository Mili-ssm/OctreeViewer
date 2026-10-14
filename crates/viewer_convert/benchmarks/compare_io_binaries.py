#!/usr/bin/env python3
"""Compare two native storage builds, alternating order and verifying exact outputs."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import statistics
import subprocess
import time


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(8 * 1024 * 1024), b''):
            digest.update(chunk)
    return {'path': str(path.resolve()), 'bytes': path.stat().st_size,
            'sha256': digest.hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('report', type=Path)
    parser.add_argument('--before-convert', type=Path, required=True)
    parser.add_argument('--after-convert', type=Path, required=True)
    parser.add_argument('--before-read', type=Path, required=True)
    parser.add_argument('--after-read', type=Path, required=True)
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--skip-reads', action='store_true',
                        help='encoder-only comparison; conversion still validates all output blocks')
    parser.add_argument('--workers', type=int, default=4)
    args = parser.parse_args()
    if args.rounds <= 0 or not 1 <= args.workers <= 8:
        parser.error('positive rounds and 1..8 workers required')
    args.output.mkdir(parents=True, exist_ok=False)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    builds = {'before': [args.before_convert.resolve(), args.before_read.resolve()],
              'after': [args.after_convert.resolve(), args.after_read.resolve()]}
    report = {'status': 'running', 'platform': platform.platform(), 'profile': 'perf',
              'cache': 'no purge; alternating builds; operations sequential',
              'source': fingerprint(args.source), 'runner': fingerprint(Path(__file__)),
              'binaries': {k: [fingerprint(p) for p in paths] for k, paths in builds.items()},
              'runs': []}

    def save():
        args.report.write_text(json.dumps(report, indent=2) + '\n')

    def execute(kind, build, repetition, command):
        label = f'{kind}-{build}-{repetition}'
        stats_path = args.output / f'{label}.time'
        started = time.monotonic()
        process = subprocess.run(['/usr/bin/time', '-f', '%e %M %U %S', '-o',
                                  str(stats_path), *map(str, command)],
                                 capture_output=True, text=True, timeout=600)
        run = {'kind': kind, 'build': build, 'round': repetition,
               'command': list(map(str, command)), 'returncode': process.returncode,
               'wall_seconds': time.monotonic() - started,
               'stdout': process.stdout, 'stderr': process.stderr}
        report['runs'].append(run)
        if process.returncode:
            report['status'] = 'failed'
            save()
            raise RuntimeError(process.stderr)
        elapsed, rss, user, system = stats_path.read_text().split()
        run.update(time_elapsed_seconds=float(elapsed), peak_rss_kib=int(rss),
                   user_seconds=float(user), system_seconds=float(system))
        save()
        print(label, round(run['wall_seconds'], 3), run['peak_rss_kib'], flush=True)
        return run

    save()
    signatures = set()
    for repetition in range(1, args.rounds + 1):
        for build in (['before', 'after'] if repetition % 2 else ['after', 'before']):
            output = (args.output / f'{build}-{repetition}.oct').resolve()
            run = execute('convert', build, repetition, [builds[build][0], args.source.resolve(),
                          output, '--format-version', 'v6.4-pre', '--spatial-workers',
                          args.workers, '--max-memory-mib', 512, '--pipeline-writes'])
            pair = [output, output.with_suffix('.octattrs')]
            run['outputs'] = [fingerprint(p) for p in pair]
            signatures.add(tuple(p['sha256'] for p in run['outputs']))
            if len(signatures) != 1:
                report['status'] = 'logical_mismatch'
                save()
                raise RuntimeError('conversion outputs differ')
            save()
            if repetition > 1 or build == 'after':
                for path in pair:
                    path.unlink()
    source = (args.output / 'before-1.oct').resolve()
    signatures = set()
    for workers in ([] if args.skip_reads else [1, 4]):
        for repetition in range(1, args.rounds + 1):
            for build in (['before', 'after'] if repetition % 2 else ['after', 'before']):
                run = execute(f'read-{workers}', build, repetition,
                              [builds[build][1], source, 'progressive', 1, 256, '--workers', workers])
                # All read modes in this runner use the same complete BFS checksum.
                metrics = dict(field.split('=', 1) for field in
                               run['stdout'].splitlines()[0].split() if '=' in field)
                run['metrics'] = metrics
                signatures.add(tuple(metrics[k] for k in ['checksum', 'points', 'blocks']))
                if len(signatures) != 1:
                    report['status'] = 'logical_mismatch'
                    save()
                    raise RuntimeError('read checksums differ')
    report['summaries'] = {}
    for kind in (['convert'] if args.skip_reads else ['convert', 'read-1', 'read-4']):
        report['summaries'][kind] = {}
        for build in builds:
            runs = [r for r in report['runs'] if r['kind'] == kind and r['build'] == build]
            report['summaries'][kind][build] = {
                key: {'median': statistics.median(r[key] for r in runs),
                      'min': min(r[key] for r in runs), 'max': max(r[key] for r in runs)}
                for key in ['wall_seconds', 'peak_rss_kib', 'user_seconds', 'system_seconds']}
    report['status'] = 'complete'
    save()


if __name__ == '__main__':
    main()
