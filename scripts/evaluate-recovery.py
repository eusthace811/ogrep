#!/usr/bin/env python3
"""Disposable Linux bulk mutation, ignore refresh, polling and crash recovery checks."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import time

spec = importlib.util.spec_from_file_location('load_eval', Path(__file__).with_name('evaluate-load.py'))
load = importlib.util.module_from_spec(spec)
spec.loader.exec_module(load)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    binary = args.binary.resolve()
    report = {'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'description': '500 source files; bulk edit/rename/delete/create, ignore changes, reads during churn, SIGKILL, offline edits and restart. Final exact line output compared to rg. Transient queries may see asynchronous mixed generations; eventual convergence deadline 30s.', 'modes': []}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for name, flags in [('native', []), ('poll', ['--watch-mode', 'poll', '--poll-interval', '1']),
                        ('fallback', ['--watch-budget', '1', '--poll-interval', '1'])]:
        row = {'mode': name, 'checks': []}
        report['modes'].append(row)
        with tempfile.TemporaryDirectory(prefix='zgrep-recovery-') as directory:
            root, index = Path(directory)/'corpus', Path(directory)/'index'
            root.mkdir()
            (root/'.git').mkdir()
            (root/'nested').mkdir()
            (root/'nested'/'deep').mkdir()
            for i in range(500):
                (root/'nested'/f'f{i:04}.rs').write_text(f'ZGREP_STRESS_original_{i}\n')
            subprocess.run([str(binary), 'index', str(root), '--index-path', str(index)], capture_output=True, check=True, timeout=60)
            logs = []
            def start():
                log = (args.output.parent/f'recovery-{name}-{len(logs)}.log').open('w')
                logs.append(log)
                child = subprocess.Popen([str(binary), 'serve', str(root), '--index-path', str(index), '-j', '4', *flags], stdout=log, stderr=log)
                try:
                    port, status = load.ready(child, index)
                    return child, port, status
                except BaseException:
                    load.stop(child)
                    raise
            server, port, row['initial_status'] = start()
            expected_mode = 'native' if name == 'native' else 'poll'
            assert row['initial_status']['watch_mode_active'] == expected_mode, row['initial_status']
            def converge(label):
                expected = subprocess.run(['rg', '--no-config', '--sort', 'path', '-n', '--no-heading', '--color', 'never', 'ZGREP_STRESS_', '.'], cwd=root, capture_output=True, check=True).stdout
                start_time = time.monotonic()
                tries = 0
                while True:
                    p = subprocess.run([str(binary), '--no-config', '--sort', 'path', '-n', '--no-heading', '--color', 'never', '--stats', '--index-path', str(index), 'ZGREP_STRESS_', '.'], cwd=root, capture_output=True, timeout=20)
                    tries += 1
                    assert p.returncode == 0 and b'(via server)' in p.stderr, p.stderr
                    if p.stdout == expected:
                        break
                    assert time.monotonic()-start_time < 30, f'{name}/{label}: did not converge'
                    time.sleep(.05)
                row['checks'].append({'label': label, 'seconds': time.monotonic()-start_time, 'attempts': tries,
                                      'matched_lines': len(expected.splitlines()), 'stdout_sha256': hashlib.sha256(expected).hexdigest(), 'correct': True})
                args.output.write_text(json.dumps(report, indent=2)+'\n')
            try:
                converge('initial')
                done = threading.Event()
                def read_during_churn():
                    count = 0
                    while not done.is_set():
                        answer = load.rpc(port, 'search', {'pattern': 'ZGREP_STRESS_', 'files_only': True})
                        assert isinstance(answer['matches'], list)
                        count += 1
                    return count
                with ThreadPoolExecutor(max_workers=4) as pool:
                    readers = [pool.submit(read_during_churn) for _ in range(4)]
                    try:
                        for i in range(400):
                            old = root/'nested'/f'f{i:04}.rs'
                            if i < 100:
                                old.write_text(f'ZGREP_STRESS_changed_{i}\n')
                            elif i < 200:
                                old.rename(root/'nested'/'deep'/f'moved{i:04}.rs')
                            elif i < 300:
                                old.unlink()
                            else:
                                (root/f'new{i:04}.rs').write_text(f'ZGREP_STRESS_created_{i}\n')
                        converge('400-bulk-mutations')
                        (root/'.gitignore').write_text('new*.rs\nnested/deep/\n')
                        converge('ignore-excludes-200')
                        (root/'.gitignore').write_text('')
                        converge('ignore-restores-200')
                    finally:
                        done.set()
                    row['concurrent_queries'] = sum(f.result() for f in readers)
                # A hard stop with outstanding new edits exercises restart's
                # stale-index repair; no graceful save is requested.
                for i in range(25):
                    (root/'nested'/f'f{i:04}.rs').write_text(f'ZGREP_STRESS_before_crash_{i}\n')
                server.kill()
                server.wait(timeout=10)
                for i in range(25, 50):
                    (root/'nested'/f'f{i:04}.rs').write_text(f'ZGREP_STRESS_offline_{i}\n')
                server, port, row['restart_status'] = start()
                converge('crash-and-50-edits-restart')
                row['final_resources'] = load.resources(server.pid)
            finally:
                load.stop(server)
                for log in logs:
                    log.close()
        args.output.write_text(json.dumps(report, indent=2)+'\n')
        print(f'{name}: {len(row["checks"])} exact-output checkpoints, {row["concurrent_queries"]} concurrent queries', flush=True)


if __name__ == '__main__':
    main()
