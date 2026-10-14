#!/usr/bin/env python3
"""Run the experimental dictionary example; keep provenance and all per-stream results."""
import argparse
import hashlib
import json
import platform
import subprocess
from datetime import datetime, timezone
from pathlib import Path


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--sample', action='append', required=True, help='NAME=INPUT.oct')
    parser.add_argument('--max-blocks', type=int, default=2**32 - 1)
    parser.add_argument('--repetitions', type=int, default=3)
    args = parser.parse_args()
    if args.max_blocks < 1 or args.repetitions < 1:
        parser.error('limits must be positive')
    samples = []
    for sample in args.sample:
        name, path = sample.split('=', 1)
        if not name or Path(name).name != name or name in {'.', '..'}:
            parser.error('sample names must be single directory names')
        samples.append((name, Path(path).resolve()))
    if len({name for name, _ in samples}) != len(samples):
        parser.error('sample names must be unique')
    args.output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve()
    report = {
        'created_utc': datetime.now(timezone.utc).isoformat(),
        'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
        'binary_sha256': sha256(binary),
        'example_sha256': sha256(Path(__file__).parents[1] / 'examples/attribute_dictionary_bench.rs'),
        'runner_sha256': sha256(Path(__file__)),
        'platform': platform.platform(),
        'cpu': next((line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), 'unknown'),
        'cache': 'uncontrolled OS cache; persisted streams reread immediately; CPU timing excludes file I/O',
        'timing': 'per-block mean of repeated CPU operations, then sum; fixed mode order; not end-to-end conversion/read time',
        'samples': [],
    }
    for name, path in samples:
        destination = args.output / name
        command = [str(binary), str(path), str(destination), str(args.max_blocks), str(args.repetitions)]
        print(f'Running {name}', flush=True)
        inputs = {str(p): {'bytes': p.stat().st_size, 'sha256': sha256(p)} for p in (path, path.with_suffix('.octattrs'))}
        result = subprocess.run(command, text=True, capture_output=True, check=False)
        (args.output / f'{name}.stdout.jsonl').write_text(result.stdout)
        (args.output / f'{name}.stderr.txt').write_text(result.stderr)
        if result.returncode:
            raise RuntimeError(f'{name}: exit {result.returncode}: {result.stderr}')
        records = [json.loads(line) for line in result.stdout.splitlines()]
        summaries = [record for record in records if record.get('summary')]
        artifacts = {str(p): {'bytes': p.stat().st_size, 'sha256': sha256(p)} for p in sorted(destination.glob('*.bin'))}
        report['samples'].append({'name': name, 'command': command, 'inputs': inputs, 'artifacts': artifacts, 'records': records})
        (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        for row in summaries:
            print(json.dumps(row), flush=True)


if __name__ == '__main__':
    main()
