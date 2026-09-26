#!/usr/bin/env python3
"""Measure ordinary local indexed search phases and instrumentation overhead."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import statistics
import subprocess
import time


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ['normal', 'profile', 'corpus', 'index', 'queries', 'output']:
        ap.add_argument('--' + name, type=Path, required=True)
    ap.add_argument('--rounds', type=int, default=9)
    ap.add_argument('--expected-revision', required=True)
    args = ap.parse_args()
    assert args.rounds > 0
    root, index = args.corpus.resolve(), args.index.resolve()
    binaries = {'normal': args.normal.resolve(), 'profile': args.profile.resolve()}
    queries = json.loads(args.queries.read_text())
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    assert revision == args.expected_revision
    assert not subprocess.check_output(['git', 'status', '--porcelain'], cwd=root)
    assert not (index / 'serve.json').exists(), 'Use an index without a server descriptor'
    assert json.loads((index.parent / 'benchmark-input.json').read_text()) == {'corpus': str(root), 'revision': revision}
    assert Path(json.loads((index / 'meta.json').read_text())['root_path']).resolve() == root
    expected = {}
    for q in queries:
        p = subprocess.run(['rg', '--no-config', '-l', '--color', 'never', '-j', '4',
                            *q.get('flags', []), '--', q['pattern'], '.'], cwd=root, capture_output=True, timeout=120)
        assert p.returncode in (0, 1), p.stderr
        expected[q['name']] = (p.returncode, sorted(p.stdout.splitlines()))
    result = {'corpus': str(root), 'revision': revision, 'index': str(index),
              'platform': platform.platform(), 'rounds': args.rounds, 'queries': queries,
              'queries_sha256': sha(args.queries),
              'binaries': {name: {'path': str(path), 'sha256': sha(path)} for name, path in binaries.items()},
              'description': 'Warm OS cache, ordinary -l local indexed requests, randomized profile/normal pairs. Every output and exit checked against independent ripgrep. No --stats. Read includes mmap attempt and mapped UTF-8 validation; decode covers owned-buffer decoding/NUL detection. Verify/render includes matching: subtract match to estimate remaining rendering work. Subprocess latency also includes startup/teardown outside the phase timer. Diagnostic times are instrumented; compare paired normal timings for overhead.',
              'warmups': [], 'samples': []}
    def request(q, engine, round_id):
        cmd = [str(binaries[engine]), '-l', '--color', 'never', '--no-config', '-j', '4',
               '--index-path', str(index), *q.get('flags', []), '--', q['pattern'], '.']
        start = time.perf_counter_ns()
        env = dict(os.environ)
        env.pop('ZGREP_PROFILE_LOCAL', None)
        if engine == 'profile':
            env['ZGREP_PROFILE_LOCAL'] = '1'
        p = subprocess.run(cmd, cwd=root, capture_output=True, timeout=120, env=env)
        elapsed = (time.perf_counter_ns() - start) / 1e6
        assert (p.returncode, sorted(p.stdout.splitlines())) == expected[q['name']], (q, engine, p.stderr)
        records = [json.loads(line.removeprefix('[profile-local] ')) for line in p.stderr.decode().splitlines()
                   if line.startswith('[profile-local] ')]
        assert len(records) == (1 if engine == 'profile' else 0), p.stderr
        assert b'warning:' not in p.stderr and b'via server' not in p.stderr, p.stderr
        return {'query': q['name'], 'engine': engine, 'round': round_id, 'ms': elapsed,
                'correct': True, 'files': len(expected[q['name']][1]),
                **({'profile': records[0]} if records else {})}
    for q in queries:
        for engine in binaries:
            result['warmups'].append(request(q, engine, -1))
    rng = random.Random(41892)
    for round_id in range(args.rounds):
        work = [(q, engine) for q in queries for engine in binaries]
        rng.shuffle(work)
        for q, engine in work:
            result['samples'].append(request(q, engine, round_id))
        print(f'completed {round_id + 1}/{args.rounds}', flush=True)
    result['summary'] = {}
    for q in queries:
        rows = [s for s in result['samples'] if s['query'] == q['name']]
        profiled = [s['profile'] for s in rows if s['engine'] == 'profile']
        result['summary'][q['name']] = {
            'files': len(expected[q['name']][1]),
            **{engine + '_ms': statistics.median(s['ms'] for s in rows if s['engine'] == engine) for engine in binaries},
            'phases_ms': {key: statistics.median(s['phases_ms'][key] for s in profiled) for key in profiled[0]['phases_ms']},
        }
    for engine, path in binaries.items():
        assert sha(path) == result['binaries'][engine]['sha256']
    assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == revision
    assert not subprocess.check_output(['git', 'status', '--porcelain'], cwd=root)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
