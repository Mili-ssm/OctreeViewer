"""Behavioral checks for exact-output guards and evidence retention."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUNNER = Path(__file__).with_name('compare_io_binaries.py')


@unittest.skipUnless(Path('/usr/bin/time').exists(), 'native Linux time required')
class BinaryComparisonTests(unittest.TestCase):
    def compare(self, root, mismatch, skip_reads=False):
        source = root / 'source.las'
        source.write_bytes(b'fixture')
        paths = []
        for build in ['before', 'after']:
            convert = root / f'{build}-convert'
            value = b'different' if mismatch and build == 'after' else b'identical'
            convert.write_text('#!/usr/bin/env python3\nimport pathlib,sys\n'
                               'output=pathlib.Path(sys.argv[2])\n'
                               f'output.write_bytes({value!r})\n'
                               'output.with_suffix(".octattrs").write_bytes(b"attributes")\n')
            read = root / f'{build}-read'
            read.write_text('#!/usr/bin/env python3\n'
                            'print("checksum=abcd points=7 blocks=1")\n')
            for path in [convert, read]:
                path.chmod(0o755)
            if skip_reads:
                read.write_text('#!/usr/bin/env python3\nraise SystemExit(99)\n')
            paths.extend([f'--{build}-convert', str(convert), f'--{build}-read', str(read)])
        report = root / 'report.json'
        process = subprocess.run([sys.executable, str(RUNNER), str(source), str(root / 'outputs'),
                                  str(report), '--rounds', '1', *(['--skip-reads'] if skip_reads else []), *paths],
                                 capture_output=True, text=True, timeout=20)
        return process, json.loads(report.read_text())

    def test_matching_outputs_complete_and_retain_common_read_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            process, report = self.compare(root, mismatch=False)
            self.assertEqual(process.returncode, 0, process.stderr)
            self.assertEqual(report['status'], 'complete')
            self.assertEqual(len(report['runs']), 6)
            self.assertTrue((root / 'outputs/before-1.oct').exists())
            self.assertFalse((root / 'outputs/after-1.oct').exists())
            self.assertEqual(report['runs'][-1]['metrics']['checksum'], 'abcd')

    def test_encoder_only_mode_skips_readers_and_keeps_hash_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            process, report = self.compare(Path(directory), mismatch=False, skip_reads=True)
            self.assertEqual(process.returncode, 0, process.stderr)
            self.assertEqual(report['status'], 'complete')
            self.assertEqual(len(report['runs']), 2)
            self.assertEqual(set(report['summaries']), {'convert'})

    def test_different_outputs_fail_and_preserve_both_pairs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            process, report = self.compare(root, mismatch=True)
            self.assertNotEqual(process.returncode, 0)
            self.assertEqual(report['status'], 'logical_mismatch')
            self.assertEqual(len(report['runs']), 2)
            for build in ['before', 'after']:
                self.assertTrue((root / f'outputs/{build}-1.oct').exists())
                self.assertTrue((root / f'outputs/{build}-1.octattrs').exists())


if __name__ == '__main__':
    unittest.main()
