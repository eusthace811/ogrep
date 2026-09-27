#!/usr/bin/env python3
"""Linux refresh receipts checked immediately against independent ripgrep scans."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import socket
import statistics
import subprocess
import tempfile
import threading
import time

spec = importlib.util.spec_from_file_location('load_eval', Path(__file__).with_name('evaluate-load.py'))
load = importlib.util.module_from_spec(spec)
spec.loader.exec_module(load)
PATTERN = 'OGREP_BARRIER_'
STABLE = 'OGREP_STABLE_777'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def raw_rpc(port, method, params=None, timeout=15):
    with socket.create_connection(('127.0.0.1', port), timeout=timeout) as stream:
        stream.sendall((json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params or {}}) + '\n').encode())
        return json.loads(stream.makefile('rb').readline())


def discover(server, index):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        assert server.poll() is None, 'server exited'
        try:
            info = json.loads((index / 'serve.json').read_text())
            if info['pid'] == server.pid and 'result' in raw_rpc(info['port'], 'status'):
                return info['port']
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(.02)
    raise TimeoutError('no server discovery')


def scan(root, files=False, pattern=PATTERN):
    command = ['rg', '--no-config', '--hidden', '--sort', 'path']
    command += ['--files', '--null', '.'] if files else ['-n', '--no-heading', '--color', 'never', '--', pattern, '.']
    result = subprocess.run(command, cwd=root, capture_output=True, timeout=15)
    assert result.returncode in (0, 1), result.stderr
    return result


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary', required=True, type=Path)
    ap.add_argument('--upstream', type=Path)
    ap.add_argument('--output', required=True, type=Path)
    ap.add_argument('--cycles', type=int, default=3)
    ap.add_argument('--files', type=int, default=1000)
    ap.add_argument('--clients', nargs='+', type=int, default=[0, 16])
    args = ap.parse_args()
    if args.cycles < 1 or args.files < 1 or any(n < 0 for n in args.clients):
        ap.error('positive cycles/files and nonnegative clients required')
    binary = args.binary.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'binary': str(binary), 'binary_sha256': digest(binary.read_bytes()),
              'version': subprocess.check_output([str(binary), '--version'], text=True).strip(),
              'platform': platform.platform(), 'files': args.files, 'cycles': args.cycles,
              'description': 'Disposable 1000-file default fixture, 12 edit types per cycle, all modes, quiet and 16-client default load. Each successful CLI receipt is followed immediately by exact line output and filename-set comparisons with independent rg. No visibility polling retries. Load queries use an unchanged independently checked file set. Refresh CPU deltas include concurrent searches when enabled; peak RSS is cumulative per daemon. Not a cross-platform or full-corpus performance claim.',
              'cases': []}

    def save():
        args.output.write_text(json.dumps(report, indent=2) + '\n')

    for mode, flags in [('disabled', ['--no-watch']), ('native', []), ('poll', ['--watch-mode', 'poll', '--poll-interval', '86400'])]:
        for clients in args.clients:
            print(f'{mode}/{clients}: starting', flush=True)
            row = {'mode': mode, 'clients': clients, 'checks': [], 'status_samples': []}
            report['cases'].append(row)
            with tempfile.TemporaryDirectory(prefix='ogrep-barrier-') as directory:
                temporary = Path(directory)
                root, index = temporary / 'root', temporary / 'index'
                root.mkdir()
                (root / '.git').mkdir()
                (root / 'fixed').mkdir()
                for i in range(args.files):
                    (root / 'fixed' / f'f{i:05}.rs').write_text(f'fn file_{i}() {{ /* {STABLE} */ }}\n' * 10)
                subject = root / 'subject.rs'
                subject.write_text(f'// {PATTERN}initial\n')
                logs = []

                def start():
                    log_path = args.output.with_name(f'{args.output.stem}-{mode}-{clients}-{len(logs)}.log')
                    log = log_path.open('w')
                    logs.append(log)
                    server = subprocess.Popen([str(binary), 'serve', str(root), '--index-path', str(index), '-j', '4', *flags], stdout=log, stderr=log)
                    try:
                        return server, discover(server, index)
                    except BaseException:
                        load.stop(server)
                        raise

                server, port = start()

                def check(label):
                    expected = scan(root)
                    filenames = scan(root, files=True)
                    before = load.resources(server.pid)
                    began = time.perf_counter_ns()
                    completed = subprocess.run([str(binary), 'refresh', str(root), '--index-path', str(index), '--json', '--timeout-secs', '30'], capture_output=True, timeout=35)
                    elapsed = (time.perf_counter_ns() - began) / 1e6
                    assert completed.returncode == 0, completed.stderr
                    receipt = json.loads(completed.stdout)
                    assert receipt['status'] == 'refreshed' and receipt['root'] == str(root), receipt
                    after = load.resources(server.pid)
                    result = subprocess.run([str(binary), '--no-config', '--hidden', '--sort', 'path', '-n', '--no-heading', '--color', 'never', '--stats', '--index-path', str(index), '--', PATTERN, '.'], cwd=root, capture_output=True, timeout=15)
                    assert b'(via server)' in result.stderr, result.stderr
                    assert result.returncode == expected.returncode and result.stdout == expected.stdout, (label, result, expected)
                    listed = raw_rpc(port, 'files', {'hidden': True})
                    expected_files = sorted(p.decode().removeprefix('./') for p in filenames.stdout.split(b'\0') if p)
                    assert sorted(listed['result']['files']) == expected_files, (label, listed)
                    row['checks'].append({'label': label, 'latency_ms': elapsed, 'receipt': receipt,
                                          'cpu_seconds': after['cpu_seconds'] - before['cpu_seconds'],
                                          'rss_kib': after['VmRSS'], 'peak_rss_kib': after['VmHWM'],
                                          'content_exit': result.returncode, 'content_sha256': digest(result.stdout),
                                          'filename_count': len(expected_files), 'filenames_sha256': digest('\0'.join(expected_files).encode()), 'correct': True})
                    save()

                stop = threading.Event()
                observer = None
                try:
                    check('initial-build')
                    row['initial_status'] = raw_rpc(port, 'status')['result']
                    assert row['initial_status']['watch_mode_active'] == mode, row['initial_status']
                    independent = subprocess.run(['rg', '--no-config', '--hidden', '-l', '--', STABLE, '.'], cwd=root, capture_output=True, check=True)
                    expected_fixed = sorted(p.removeprefix('./') for p in independent.stdout.decode().splitlines())

                    def queries():
                        samples = []
                        while not stop.is_set():
                            began = time.perf_counter_ns()
                            answer = raw_rpc(port, 'search', {'pattern': STABLE, 'files_only': True, 'compact_files': True, 'hidden': True})
                            assert sorted(p['file'] for p in answer['result']['matches']) == expected_fixed, answer
                            samples.append((time.perf_counter_ns() - began) / 1e6)
                        return samples

                    def observe():
                        while not stop.wait(.02):
                            began = time.perf_counter_ns()
                            try:
                                status = raw_rpc(port, 'status')['result']
                                row['status_samples'].append({'latency_ms': (time.perf_counter_ns() - began) / 1e6,
                                                              'manual_refresh': status['manual_refresh'], 'search_admission': status['search_admission']})
                            except Exception as error:
                                row['status_samples'].append({'error': str(error)})

                    observer = threading.Thread(target=observe)
                    observer.start()
                    with ThreadPoolExecutor(max_workers=max(1, clients)) as pool:
                        workers = [pool.submit(queries) for _ in range(clients)]
                        try:
                            for cycle in range(args.cycles):
                                def content(label):
                                    return f'// {PATTERN}{cycle:03}_{label:18}\n'
                                subject.write_text(content('edit'))
                                check(f'{cycle}/edit')
                                created, moved = root / 'created.rs', root / 'moved.rs'
                                created.write_text(content('create'))
                                check(f'{cycle}/create')
                                created.rename(moved)
                                check(f'{cycle}/rename')
                                staged = temporary / 'replacement.rs'
                                staged.write_text(content('atomic_replace'))
                                staged.replace(subject)
                                check(f'{cycle}/atomic_replace')
                                moved.unlink()
                                check(f'{cycle}/delete')
                                (root / '.ignore').write_text('subject.rs\n')
                                check(f'{cycle}/ignore')
                                (root / '.ignore').write_text('')
                                check(f'{cycle}/unignore')
                                before = subject.stat()
                                subject.write_text(content('preserved_mtime'))
                                os.utime(subject, ns=(before.st_atime_ns, before.st_mtime_ns))
                                assert subject.stat().st_size == before.st_size and subject.stat().st_mtime_ns == before.st_mtime_ns
                                check(f'{cycle}/preserved_mtime')
                                subtree = root / 'subtree'
                                if subtree.exists():
                                    shutil.rmtree(subtree)
                                (subtree / 'deep').mkdir(parents=True)
                                (subtree / 'deep' / 'old.rs').write_text(content('subtree'))
                                check(f'{cycle}/subtree_create')
                                shutil.rmtree(subtree)
                                (subtree / 'deep').mkdir(parents=True)
                                (subtree / 'deep' / 'new.rs').write_text(content('subtree_recreate'))
                                check(f'{cycle}/subtree_recreate')
                                (root / 'asset.bin').write_bytes(b'\0binary\xff')
                                check(f'{cycle}/binary_filename')
                                (root / '.hidden.rs').write_text(content('hidden'))
                                check(f'{cycle}/hidden')
                        finally:
                            stop.set()
                        row['concurrent_search_ms'] = [sample for worker in workers for sample in worker.result()]
                    observer.join(timeout=20)
                    assert not observer.is_alive()
                    assert row['status_samples'] and all('error' not in r for r in row['status_samples'])
                    assert all(r['search_admission']['active'] <= 4 for r in row['status_samples'])
                    row['resources'] = load.resources(server.pid)
                    if clients == 0:
                        server.kill()
                        server.wait(timeout=10)
                        (root / 'offline.rs').write_text(f'{PATTERN}offline_restart\n')
                        server, port = start()
                        check('offline-edit-and-restart')
                finally:
                    stop.set()
                    if observer:
                        observer.join(timeout=20)
                    load.stop(server)
                    for log in logs:
                        log.close()
            latencies = [r['latency_ms'] for r in row['checks'] if '/' in r['label']]
            row['summary'] = {'refresh_median_ms': statistics.median(latencies), 'refresh_max_ms': max(latencies),
                              'status_max_ms': max(r['latency_ms'] for r in row['status_samples']),
                              'search_count': len(row['concurrent_search_ms'])}
            save()
            print(f'{mode}/{clients}: {len(row["checks"])} immediate exact comparisons, {row["summary"]}', flush=True)

    if args.upstream:
        upstream = args.upstream.resolve()
        with tempfile.TemporaryDirectory(prefix='tgrep-refresh-capability-') as directory:
            root, index = Path(directory) / 'root', Path(directory) / 'index'
            root.mkdir()
            (root / 'source.rs').write_text('capability_marker\n')
            with args.output.with_suffix('.upstream.log').open('w') as log:
                server = subprocess.Popen([str(upstream), 'serve', str(root), '--index-path', str(index), '--no-watch'], stdout=log, stderr=log)
                try:
                    port = discover(server, index)
                    reply = raw_rpc(port, 'refresh', {'timeout_ms': 30000})
                    assert reply['error']['code'] == -32601, reply
                    report['upstream_capability'] = {'binary_sha256': digest(upstream.read_bytes()), 'version': subprocess.check_output([str(upstream), '--version'], text=True).strip(), 'reply': reply,
                                                     'interpretation': 'This baseline does not expose the refresh method. This is a capability comparison, not a speed ratio.'}
                finally:
                    load.stop(server)
        save()


if __name__ == '__main__':
    main()
