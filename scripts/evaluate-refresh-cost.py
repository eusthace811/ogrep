#!/usr/bin/env python3
"""Measure full-tree refresh cost on a clean pinned corpus without editing it."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import statistics
import subprocess
import threading
import time

spec = importlib.util.spec_from_file_location('load_eval', Path(__file__).with_name('evaluate-load.py'))
load = importlib.util.module_from_spec(spec)
spec.loader.exec_module(load)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary', type=Path, required=True)
    ap.add_argument('--corpus', type=Path, required=True)
    ap.add_argument('--expected-revision', required=True)
    ap.add_argument('--index-source', type=Path, required=True)
    ap.add_argument('--work', type=Path, required=True, help='New private work directory; must not exist')
    ap.add_argument('--queries', type=Path, required=True)
    ap.add_argument('--rounds', type=int, default=3)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    if args.rounds < 1:
        ap.error('rounds must be positive')
    binary, root = args.binary.resolve(), args.corpus.resolve()
    def validate_corpus():
        assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == args.expected_revision
        assert not subprocess.check_output(['git', 'status', '--porcelain'], cwd=root), 'corpus must be clean'
    validate_corpus()
    provenance = json.loads((args.index_source.parent / 'benchmark-input.json').read_text())
    assert provenance == {'corpus': str(root), 'revision': args.expected_revision}, provenance
    args.work.mkdir(parents=True, exist_ok=False)
    index = args.work.resolve() / 'index'
    shutil.copytree(args.index_source, index)
    queries = json.loads(args.queries.read_text())
    # Ordinary -l semantics, no --stats while measuring refresh. Each query's
    # actual route and exact file set are checked separately after every pass.
    expected = {}
    for query in queries:
        result = subprocess.run(['rg', '--no-config', '-l', '--color', 'never', '-j', '4', *query.get('flags', []), '--', query['pattern'], '.'], cwd=root, capture_output=True, timeout=120)
        assert result.returncode in (0, 1), result.stderr
        expected[query['name']] = (result.returncode, sorted(result.stdout.splitlines()))
    report = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'binary_sha256': sha(binary), 'binary': str(binary), 'corpus': str(root), 'revision': args.expected_revision,
              'queries': queries, 'query_file_sha256': sha(args.queries), 'rounds': [],
              'description': 'Private copied index, --no-watch -j4, one untimed full refresh warmup then repeated unchanged-tree refreshes; status sampled every 50ms. Each receipt is followed by independent rg file-set agreement and proof of the CLI server route. CPU delta covers only refresh including any server bookkeeping. Peak RSS is cumulative per daemon. No source edits, cache dropping, load clients or cross-host claim.'}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    stop = threading.Event()
    observations = []
    def save():
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    with args.output.with_suffix('.server.log').open('w') as log:
        server = subprocess.Popen([str(binary), 'serve', str(root), '--index-path', str(index), '--no-watch', '-j', '4'], stdout=log, stderr=log)
        observer = None
        try:
            port, report['initial_status'] = load.ready(server, index)
            def observe():
                while not stop.wait(.05):
                    began = time.perf_counter_ns()
                    try:
                        state = load.rpc(port, 'status')
                        observations.append({'latency_ms': (time.perf_counter_ns() - began) / 1e6, 'manual_refresh': state['manual_refresh']})
                    except Exception as error:
                        observations.append({'error': str(error)})
            observer = threading.Thread(target=observe)
            observer.start()
            for round_number in range(-1, args.rounds):
                print(f'{root.name}: refresh {round_number} (-1 is warmup)', flush=True)
                before = load.resources(server.pid)
                began = time.perf_counter_ns()
                result = subprocess.run([str(binary), 'refresh', str(root), '--index-path', str(index), '--json', '--timeout-secs', '300'], capture_output=True, timeout=305)
                elapsed = (time.perf_counter_ns() - began) / 1e6
                after = load.resources(server.pid)
                assert result.returncode == 0, result.stderr
                receipt = json.loads(result.stdout)
                assert receipt['status'] == 'refreshed' and receipt['root'] == str(root), receipt
                checks = {}
                for query in queries:
                    found = subprocess.run([str(binary), '--no-config', '-l', '--stats', '--color', 'never', '-j', '4', '--index-path', str(index), *query.get('flags', []), '--', query['pattern'], '.'], cwd=root, capture_output=True, timeout=120)
                    assert b'(via server)' in found.stderr, found.stderr
                    assert (found.returncode, sorted(found.stdout.splitlines())) == expected[query['name']], query
                    checks[query['name']] = {'rows': len(expected[query['name']][1]), 'stdout_sha256': hashlib.sha256(b'\n'.join(expected[query['name']][1])).hexdigest(), 'correct': True}
                sample = {'round': round_number, 'latency_ms': elapsed, 'receipt': receipt,
                          'cpu_seconds': after['cpu_seconds'] - before['cpu_seconds'],
                          'rss_kib': after['VmRSS'], 'peak_rss_kib': after['VmHWM'], 'checks': checks}
                if round_number == -1:
                    report['warmup'] = sample
                else:
                    report['rounds'].append(sample)
                save()
                print(f'{root.name}: {elapsed:.1f} ms, {sample["cpu_seconds"]:.2f} CPU seconds, {sample["peak_rss_kib"] / 1024:.1f} MiB peak', flush=True)
        finally:
            stop.set()
            if observer:
                observer.join(timeout=65)
            load.stop(server)
    assert observer is not None and not observer.is_alive()
    assert observations and all('error' not in sample for sample in observations)
    assert any(sample['manual_refresh']['running'] for sample in observations), 'did not observe an active pass'
    report['status_samples'] = observations
    report['summary'] = {'refresh_median_ms': statistics.median(r['latency_ms'] for r in report['rounds']),
                         'refresh_min_ms': min(r['latency_ms'] for r in report['rounds']),
                         'refresh_max_ms': max(r['latency_ms'] for r in report['rounds']),
                         'cpu_median_seconds': statistics.median(r['cpu_seconds'] for r in report['rounds']),
                         'peak_rss_mib': max(r['peak_rss_kib'] for r in report['rounds']) / 1024,
                         'status_max_ms': max(r['latency_ms'] for r in observations)}
    validate_corpus()
    assert sha(binary) == report['binary_sha256']
    save()


if __name__ == '__main__':
    main()
