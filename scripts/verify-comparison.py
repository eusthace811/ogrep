#!/usr/bin/env python3
"""Check completed evaluation samples against fresh ripgrep scans and provenance."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inputs', type=Path, nargs='+')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    rg = Path(shutil.which('rg')).resolve()
    report = {'scan_engine': str(rg), 'scan_sha256': sha(rg.read_bytes()),
              'scan_version': subprocess.check_output([str(rg), '--version'], text=True).strip(),
              'evaluations': {}}
    for path in args.inputs:
        data = json.loads(path.read_text())
        corpus = Path(data['corpus'])
        assert subprocess.check_output(['git', '-C', str(corpus), 'rev-parse', 'HEAD'], text=True).strip() == data['corpus_revision']
        assert not subprocess.check_output(['git', '-C', str(corpus), 'status', '--porcelain'])
        for engine in data['engines'].values():
            assert sha(Path(engine['path']).read_bytes()) == engine['sha256']
        assert len(data['samples']) == len(data['queries']) * data['rounds'] * len(data['engines'])
        expected_pairs = {(r, e, q['name']) for r in range(data['rounds'])
                          for e in data['engines'] for q in data['queries']}
        assert {(s['round'], s['engine'], s['query']) for s in data['samples']} == expected_pairs
        checks = {}
        for query in data['queries']:
            command = [str(rg), '--no-config', '-l', '--color', 'never', '-j', '4',
                       *query.get('flags', []), '--', query['pattern'], '.']
            scan = subprocess.run(command, cwd=corpus, capture_output=True, timeout=120)
            assert scan.returncode in (0, 1), scan.stderr
            rows = sorted(scan.stdout.splitlines())
            digest = sha(b'\n'.join(rows))
            for name, validated in data['validation'][query['name']].items():
                assert (len(rows), digest, scan.returncode) == (validated['rows'], validated['sha256'], validated['exit']), (path, query['name'], name)
                if name != 'rg':
                    assert ('(via server)' in validated['diagnostics']) == (data['mode'] == 'server')
            checks[query['name']] = {'rows': len(rows), 'sha256': digest, 'exit': scan.returncode}
        report['evaluations'][path.name] = {'timed_samples_checked': len(data['samples']),
                                           'queries_checked_against_scan': checks,
                                           'corpus_clean': True, 'binary_identities_match': True}
        print(f'{path.name}: {len(checks)} queries agree with fresh scans', flush=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
