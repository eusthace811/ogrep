#!/usr/bin/env python3
"""Linux loopback RPC load evaluation against independent ripgrep file sets."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import random
import socket
import statistics
import subprocess
import time


def rpc(port, method, params=None):
    with socket.create_connection(('127.0.0.1', port), timeout=60) as s:
        s.settimeout(60)
        s.sendall((json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params or {}}) + '\n').encode())
        result = json.loads(s.makefile('rb').readline())
        if 'error' in result:
            raise RuntimeError(result['error'])
        return result['result']


def resources(pid):
    stat = Path(f'/proc/{pid}/stat').read_text().split(') ', 1)[1].split()
    memory = {line.split(':')[0]: int(line.split()[1]) for line in Path(f'/proc/{pid}/status').read_text().splitlines()
              if line.startswith(('VmRSS:', 'VmHWM:'))}
    return {'cpu_seconds': (int(stat[11]) + int(stat[12])) / os.sysconf('SC_CLK_TCK'), **memory}


def ready(server, index):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        assert server.poll() is None, 'server exited'
        try:
            info = json.loads((index / 'serve.json').read_text())
            state = rpc(info['port'], 'status') if info['pid'] == server.pid else {}
            if state.get('hidden_complete') and not state.get('indexing') and not state.get('reconcile_running') and state.get('last_reconcile_at') is not None:
                return info['port'], state
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(.05)
    raise TimeoutError('server did not become ready')


def stop(server):
    server.terminate()
    try:
        server.wait(timeout=10)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary', type=Path, required=True)
    ap.add_argument('--corpus', type=Path, required=True)
    ap.add_argument('--index', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--requests', type=int, default=1024)
    args = ap.parse_args()
    binary, root, index = args.binary.resolve(), args.corpus.resolve(), args.index.resolve()
    patterns = ['fn main', 'NonNull', 'typeck', 'absent_ZGREP_897342938', r'impl.*Debug', r'(alloc|dealloc).*Layout']
    expected = []
    for pattern in patterns:
        p = subprocess.run(['rg', '--no-config', '-l', '-t', 'rust', '--', pattern, '.'], cwd=root, capture_output=True, check=False, timeout=60)
        assert p.returncode in (0, 1), p.stderr
        expected.append(sorted(p.stdout.decode().splitlines()))
    record = {'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'corpus': str(root),
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
              'description': 'Loopback RPC, one TCP connection per request, six mixed regex queries, four server worker threads; independent rg -l -t rust expected file sets. Warm page cache. Raw latency samples retained; not a distributed service test.',
              'patterns': patterns, 'expected_file_counts': [len(x) for x in expected], 'loads': []}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.with_suffix('.server.log').open('w') as log:
        server = subprocess.Popen([str(binary), 'serve', str(root), '--index-path', str(index), '--no-watch', '-j', '4'], stdout=log, stderr=log)
        try:
            port, record['initial_status'] = ready(server, index)
            def request(query):
                started = time.perf_counter_ns()
                result = rpc(port, 'search', {'pattern': patterns[query], 'types': ['rust'], 'files_only': True})
                elapsed = (time.perf_counter_ns() - started) / 1e6
                actual = sorted({'./' + row['file'].removeprefix('./') for row in result['matches'] if row['type'] == 'match'})
                return {'query': query, 'latency_ms': elapsed, 'correct': actual == expected[query],
                        **({'expected': expected[query], 'actual': actual} if actual != expected[query] else {})}
            for query in range(len(patterns)):
                assert request(query)['correct'], f'warmup mismatch for {patterns[query]}'
            for clients in [1, 4, 16]:
                work = [i % len(patterns) for i in range(args.requests)]
                random.Random(941 + clients).shuffle(work)
                before = resources(server.pid)
                start = time.perf_counter()
                with ThreadPoolExecutor(max_workers=clients) as pool:
                    samples = list(pool.map(request, work))
                seconds = time.perf_counter() - start
                after = resources(server.pid)
                latencies = sorted(x['latency_ms'] for x in samples)
                row = {'clients': clients, 'requests': len(samples), 'errors': sum(not x['correct'] for x in samples),
                       'elapsed_seconds': seconds, 'requests_per_second': len(samples)/seconds,
                       'median_ms': statistics.median(latencies), 'p95_ms': latencies[int(.95*(len(latencies)-1))],
                       'p99_ms': latencies[int(.99*(len(latencies)-1))], 'resources_before': before,
                       'resources_after': after, 'server_cpu_seconds': after['cpu_seconds']-before['cpu_seconds'], 'samples': samples}
                record['loads'].append(row)
                args.output.write_text(json.dumps(record, indent=2)+'\n')
                print({k:v for k,v in row.items() if k != 'samples'}, flush=True)
                assert row['errors'] == 0
        finally:
            stop(server)


if __name__ == '__main__':
    main()
