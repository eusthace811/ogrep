#!/usr/bin/env python3
"""Reproducible process-per-query evaluation. No third-party Python dependencies.

Each engine receives the same query flags/corpus. Validate complete sorted output
before timing, interleave engines in seeded random order, retain every sample.
The corpus must remain unchanged. This measures warm OS-cache CLI latency; it
does NOT claim cold-cache measurements. Server mode includes CLI and TCP costs.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess
import time


def run(command, **kwargs):
    p = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       timeout=120, **kwargs)
    if p.returncode not in (0, 1):
        raise RuntimeError(f"{command!r}: exit {p.returncode}: {p.stderr.decode(errors='replace')}")
    return p


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--engine', action='append', required=True, help='NAME=EXECUTABLE; rg is a scan baseline')
    ap.add_argument('--corpus', type=Path, required=True)
    ap.add_argument('--queries', type=Path, required=True)
    ap.add_argument('--work', type=Path, required=True, help='Indexes/logs outside the corpus')
    ap.add_argument('--mode', choices=['local', 'server'], default='local')
    ap.add_argument('--rounds', type=int, default=15)
    ap.add_argument('--seed', type=int, default=77291)
    ap.add_argument('--expected-revision', help='Require this exact clean Git revision and guarded index provenance')
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    corpus = args.corpus.resolve()
    work = args.work.resolve()
    if work.is_relative_to(corpus):
        ap.error('--work must be outside the corpus')
    work.mkdir(parents=True, exist_ok=True)
    queries = json.loads(args.queries.read_text())
    engines = {}
    for entry in args.engine:
        name, executable = entry.split('=', 1)
        engines[name] = Path(shutil.which(executable) or executable).resolve()
    if args.rounds < 1:
        ap.error('--rounds must be positive')
    result = {'schema': 2, 'mode': args.mode, 'cache': 'warm OS cache',
              'rounds': args.rounds, 'seed': args.seed, 'corpus': str(corpus),
              'queries_sha256': digest(args.queries.read_bytes()), 'queries': queries,
              'platform': platform.platform(), 'cpu_count': os.cpu_count(),
              'affinity': sorted(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') else None,
              'engines': {}, 'samples': [], 'validation': {}, 'summary': {}}
    rev = subprocess.run(['git', '-C', str(corpus), 'rev-parse', 'HEAD'], capture_output=True)
    result['corpus_revision'] = rev.stdout.decode().strip() if rev.returncode == 0 else None
    snapshot = {'corpus': str(corpus), 'revision': result['corpus_revision']}
    if args.expected_revision:
        if result['corpus_revision'] != args.expected_revision:
            ap.error(f'corpus revision differs: {result["corpus_revision"]} != {args.expected_revision}')
        clean = subprocess.run(['git', '-C', str(corpus), 'status', '--porcelain'], capture_output=True, check=True)
        if clean.stdout:
            ap.error('corpus must have a clean tracked and untracked working tree')
    result['expected_revision'] = args.expected_revision
    cpuinfo = Path('/proc/cpuinfo')
    result['cpu'] = next((l.partition(':')[2].strip() for l in cpuinfo.read_text().splitlines()
                          if l.startswith('model name')), None) if cpuinfo.exists() else None
    servers = []
    logs = []
    indexes = {}
    def command(name, q):
        cmd = [str(engines[name]), '-l', '--color', 'never', '--no-config', '-j', '4', *q.get('flags', [])]
        if name != 'rg':
            cmd += ['--index-path', str(indexes[name])]
        return cmd + ['--', q['pattern'], '.']

    try:
        for name, binary in engines.items():
            result['engines'][name] = {'path': str(binary), 'sha256': digest(binary.read_bytes()),
                                      'version': run([str(binary), '--version']).stdout.decode().strip()}
            if name == 'rg':
                continue
            index = work / name / 'index'
            indexes[name] = index
            provenance = work / name / 'benchmark-input.json'
            if args.expected_revision and (index / 'meta.json').exists():
                try:
                    previous = json.loads(provenance.read_text())
                except (FileNotFoundError, json.JSONDecodeError):
                    previous = None
                if previous != snapshot:
                    raise RuntimeError(f'{index}: index snapshot is unproven or changed; use a new --work directory')
            if not (index / 'meta.json').exists():
                start = time.perf_counter()
                p = run([str(binary), 'index', str(corpus), '--index-path', str(index), '-j', '4'])
                result['engines'][name]['index_seconds'] = time.perf_counter() - start
                result['engines'][name]['index_output'] = (p.stdout + p.stderr).decode(errors='replace')
                if args.expected_revision:
                    provenance.write_text(json.dumps(snapshot, indent=2) + '\n')
            if args.expected_revision:
                metadata = json.loads((index / 'meta.json').read_text())
                if Path(metadata['root_path']).resolve() != corpus:
                    raise RuntimeError(f'{index}: indexed root differs from corpus')
            result['engines'][name]['index_bytes'] = sum(p.stat().st_size for p in index.rglob('*') if p.is_file())
            if args.mode == 'server':
                log = (work / f'{name}-server.log').open('wb')
                logs.append(log)
                server = subprocess.Popen([str(binary), 'serve', str(corpus), '--index-path', str(index),
                                           '--no-watch', '-j', '4'], stdout=log, stderr=log)
                servers.append(server)
                deadline = time.monotonic() + 60
                while True:
                    if server.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError(f'{name} server failed; see {log.name}')
                    try:
                        descriptor = json.loads((index / 'serve.json').read_text())
                    except (FileNotFoundError, json.JSONDecodeError):
                        descriptor = {}
                    if descriptor.get('pid') == server.pid:
                        break
                    time.sleep(.05)

        for q in queries:
            expected = None
            expected_exit = None
            for name in engines:
                p = run(command(name, q), cwd=corpus)
                rows = sorted(p.stdout.splitlines())
                if expected is None:
                    expected = rows
                    expected_exit = p.returncode
                if p.returncode != expected_exit:
                    raise AssertionError(f"Exit mismatch: {name}, {q['name']}: {p.returncode} != {expected_exit}")
                if rows != expected:
                    raise AssertionError(f"Output mismatch: {name}, {q['name']}; "
                                         f"missing {set(expected)-set(rows)}, extra {set(rows)-set(expected)}")
                result['validation'].setdefault(q['name'], {})[name] = {
                    'rows': len(rows), 'sha256': digest(b'\n'.join(rows)), 'exit': p.returncode}
                run(command(name, q), cwd=corpus)  # Second warmup, excluded from timing.
                if name != 'rg':
                    diagnostic = command(name, q)
                    diagnostic.insert(1, '--stats')
                    stats = run(diagnostic, cwd=corpus).stderr.decode(errors='replace')
                    used_server = '(via server)' in stats
                    if used_server != (args.mode == 'server'):
                        raise AssertionError(f'{name}/{q["name"]}: execution mode differs: {stats}')
                    result['validation'][q['name']][name]['diagnostics'] = stats

        rng = random.Random(args.seed)
        for round_id in range(args.rounds):
            tasks = [(name, q) for q in queries for name in engines]
            rng.shuffle(tasks)
            for name, q in tasks:
                start = time.perf_counter_ns()
                p = run(command(name, q), cwd=corpus)
                elapsed = (time.perf_counter_ns() - start) / 1e6
                expected = result['validation'][q['name']][name]
                if digest(b'\n'.join(sorted(p.stdout.splitlines()))) != expected['sha256'] or p.returncode != expected['exit']:
                    raise AssertionError(f"Output changed during timing: {name}/{q['name']}")
                result['samples'].append({'round': round_id, 'engine': name, 'query': q['name'], 'ms': elapsed})
            print(f'completed round {round_id + 1}/{args.rounds}', flush=True)
        for q in queries:
            result['summary'][q['name']] = {}
            for name in engines:
                values = sorted(s['ms'] for s in result['samples'] if s['engine'] == name and s['query'] == q['name'])
                result['summary'][q['name']][name] = {'median_ms': statistics.median(values),
                    'p95_ms': values[min(len(values)-1, int(.95*len(values)))], 'min_ms': min(values), 'max_ms': max(values)}
        for name, binary in engines.items():
            if digest(binary.read_bytes()) != result['engines'][name]['sha256']:
                raise AssertionError(f'{name}: binary changed during evaluation')
        if args.expected_revision:
            final_revision = subprocess.check_output(['git', '-C', str(corpus), 'rev-parse', 'HEAD'], text=True).strip()
            dirty = subprocess.check_output(['git', '-C', str(corpus), 'status', '--porcelain'])
            if final_revision != args.expected_revision or dirty:
                raise AssertionError('corpus changed during evaluation')
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + '\n')
    finally:
        for server in servers:
            server.terminate()
        for server in servers:
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
