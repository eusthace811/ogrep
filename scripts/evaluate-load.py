#!/usr/bin/env python3
"""Linux loopback RPC load evaluation against independent ripgrep file sets."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import random
import resource
import socket
import statistics
import subprocess
import threading
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
    ap.add_argument('--compact-files', action='store_true', help='Request optional compact file-list replies')
    ap.add_argument('--search-concurrency', type=int, help='Pass the new server admission limit (omit for old binaries)')
    ap.add_argument('--observe-status', action='store_true', help='Probe status every 50 ms during each load')
    ap.add_argument('--clients', nargs='+', type=int, default=[1, 4, 16])
    args = ap.parse_args()
    if args.requests < 1 or any(n < 1 for n in args.clients):
        ap.error('requests and client counts must be positive')
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
              'compact_files': args.compact_files,
              'search_concurrency': args.search_concurrency, 'observe_status': args.observe_status,
              'patterns': patterns, 'expected_file_counts': [len(x) for x in expected], 'loads': []}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.with_suffix('.server.log').open('w') as log:
        command = [str(binary), 'serve', str(root), '--index-path', str(index), '--no-watch', '-j', '4']
        if args.search_concurrency is not None:
            command += ['--search-concurrency', str(args.search_concurrency)]
        server = subprocess.Popen(command, stdout=log, stderr=log)
        try:
            port, record['initial_status'] = ready(server, index)
            def request(query):
                started = time.perf_counter_ns()
                result = rpc(port, 'search', {'pattern': patterns[query], 'types': ['rust'],
                                             'files_only': True, 'compact_files': args.compact_files})
                elapsed = (time.perf_counter_ns() - started) / 1e6
                actual = sorted({'./' + row['file'].removeprefix('./') for row in result['matches'] if row['type'] == 'match'})
                return {'query': query, 'latency_ms': elapsed, 'correct': actual == expected[query],
                        **({'expected': expected[query], 'actual': actual} if actual != expected[query] else {})}
            for query in range(len(patterns)):
                assert request(query)['correct'], f'warmup mismatch for {patterns[query]}'
            for clients in args.clients:
                work = [i % len(patterns) for i in range(args.requests)]
                random.Random(941 + clients).shuffle(work)
                observations = []
                observation_stop = threading.Event()
                def observe():
                    while not observation_stop.wait(.05):
                        began = time.perf_counter_ns()
                        try:
                            status = rpc(port, 'status')
                            observations.append({'latency_ms': (time.perf_counter_ns()-began)/1e6,
                                                 'search_admission': status.get('search_admission'),
                                                 'connection_admission': status.get('connection_admission')})
                        except Exception as error:
                            observations.append({'error': str(error)})
                observer = threading.Thread(target=observe, daemon=True) if args.observe_status else None
                if observer:
                    observer.start()
                before = resources(server.pid)
                client_cpu_before = time.process_time()
                start = time.perf_counter()
                try:
                    with ThreadPoolExecutor(max_workers=clients) as pool:
                        samples = list(pool.map(request, work))
                    seconds = time.perf_counter() - start
                finally:
                    observation_stop.set()
                    if observer:
                        observer.join(timeout=5)
                        assert not observer.is_alive(), 'status observer stalled'
                after = resources(server.pid)
                client_cpu_seconds = time.process_time() - client_cpu_before
                latencies = sorted(x['latency_ms'] for x in samples)
                row = {'clients': clients, 'requests': len(samples), 'errors': sum(not x['correct'] for x in samples),
                       'elapsed_seconds': seconds, 'requests_per_second': len(samples)/seconds,
                       'median_ms': statistics.median(latencies), 'p95_ms': latencies[int(.95*(len(latencies)-1))],
                       'p99_ms': latencies[int(.99*(len(latencies)-1))], 'resources_before': before,
                       'resources_after': after, 'server_cpu_seconds': after['cpu_seconds']-before['cpu_seconds'],
                       'client_cpu_seconds': client_cpu_seconds,
                       'client_cpu_cores': client_cpu_seconds / seconds,
                       'client_peak_rss_kib': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                       'status_observations': observations, 'samples': samples}
                record['loads'].append(row)
                args.output.write_text(json.dumps(record, indent=2)+'\n')
                print({k:v for k,v in row.items() if k not in ('samples', 'status_observations')}, flush=True)
                assert row['errors'] == 0
                assert all('error' not in item for item in observations), observations
                for item in observations:
                    for key in ['search_admission', 'connection_admission']:
                        if item[key] is not None:
                            assert item[key]['active'] <= item[key]['capacity']
                            assert item[key]['queued'] <= item[key]['queue_capacity']
        finally:
            stop(server)


if __name__ == '__main__':
    main()
