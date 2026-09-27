#!/usr/bin/env python3
"""Measure observed edit-to-search visibility on a disposable native-watcher fixture."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import socket
import subprocess
import tempfile
import time


def status(port):
    with socket.create_connection(('127.0.0.1', port), timeout=2) as stream:
        stream.sendall(b'{"jsonrpc":"2.0","id":1,"method":"status"}\n')
        return json.loads(stream.makefile('rb').readline())['result']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True, help='NAME=BINARY')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = {'description': '1000 filler files; 6 cycles of edit, rename, atomic replace, delete, create. '
              'Mutation-to-first-correct CLI completion, including file write, CLI/TCP and up to 10 ms polling delay. '
              'Native watchers only; not a generation barrier or guarantee for external edits.',
              'platform': platform.platform(), 'engines': {}}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for engine_arg in args.engine:
        name, executable = engine_arg.split('=', 1)
        binary = Path(executable).resolve()
        record = {'binary': str(binary), 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                  'version': subprocess.check_output([str(binary), '--version'], text=True).strip(),
                  'samples': []}
        result['engines'][name] = record
        with tempfile.TemporaryDirectory(prefix='ogrep-refresh-eval-') as directory:
            temporary = Path(directory)
            root = temporary / 'corpus'
            root.mkdir()
            (root / '.git').mkdir()
            for i in range(1000):
                (root / f'filler_{i:04}.rs').write_text(f'fn filler_{i}() {{ /* ordinary source */ }}\n' * 10)
            subject = root / 'subject.rs'
            subject.write_text('// initial contents\n')
            index = temporary / 'index'
            subprocess.run([str(binary), 'index', str(root), '--index-path', str(index)],
                           capture_output=True, check=True, timeout=60)
            log_path = args.output.parent / f'refresh-{name}-server.log'
            with log_path.open('w') as log:
                server = subprocess.Popen([str(binary), 'serve', str(root), '--index-path', str(index)],
                                          stdout=log, stderr=log)
                try:
                    deadline = time.monotonic() + 30
                    while True:
                        assert server.poll() is None, 'server exited'
                        assert time.monotonic() < deadline, 'server not ready'
                        try:
                            discovery = json.loads((index / 'serve.json').read_text())
                            current = status(discovery['port']) if discovery['pid'] == server.pid else {}
                        except (FileNotFoundError, json.JSONDecodeError, OSError):
                            current = {}
                        if (current.get('watch_mode_active') == 'native' and
                                current.get('hidden_complete') and not current.get('indexing') and
                                current.get('last_reconcile_at') is not None and
                                current.get('reconcile_running') is False):
                            break
                        time.sleep(.05)
                    record['initial_status'] = current
                    search = [str(binary), '--no-config', '-F', '-n', '--no-heading', '--color', 'never',
                              '--stats', '--index-path', str(index), '--', 'OGREP_REFRESH_', '.']
                    # Warm CLI/server search before applying mutations.
                    p = subprocess.run(search, cwd=root, capture_output=True, text=True, timeout=10)
                    assert p.returncode == 1 and not p.stdout and '(via server)' in p.stderr
                    for cycle in range(6):
                        moved = root / 'moved.rs'
                        content = ''
                        for operation in ('edit', 'rename', 'replace', 'delete', 'create'):
                            next_content = f'const MARKER: &str = "OGREP_REFRESH_{cycle}_{operation}";'
                            staging = temporary / 'replacement.rs'
                            if operation == 'replace':
                                staging.write_text(next_content + '\n')
                            started = time.perf_counter_ns()
                            if operation == 'edit':
                                subject.write_text(next_content + '\n')
                                content, filename = next_content, 'subject.rs'
                            elif operation == 'rename':
                                subject.rename(moved)
                                filename = 'moved.rs'
                            elif operation == 'replace':
                                staging.replace(moved)
                                content, filename = next_content, 'moved.rs'
                            elif operation == 'delete':
                                moved.unlink()
                                filename = None
                            else:
                                subject.write_text(next_content + '\n')
                                content, filename = next_content, 'subject.rs'
                            expected = [f'./{filename}:1:{content}'] if filename else []
                            expected_exit = 0 if expected else 1
                            observations = []
                            while True:
                                p = subprocess.run(search, cwd=root, capture_output=True, text=True, timeout=10)
                                elapsed_ms = (time.perf_counter_ns() - started) / 1e6
                                assert p.returncode in (0, 1), p.stderr
                                assert '(via server)' in p.stderr, p.stderr
                                correct = sorted(p.stdout.splitlines()) == expected and p.returncode == expected_exit
                                observations.append({'elapsed_ms': elapsed_ms, 'correct': correct})
                                if correct or elapsed_ms >= 10000:
                                    break
                                time.sleep(.01)
                            scan = subprocess.run([shutil.which('rg'), '--no-config', '-F', '-n', '--no-heading',
                                                   '--color', 'never', '--', 'OGREP_REFRESH_', '.'],
                                                  cwd=root, capture_output=True, text=True, timeout=10)
                            assert sorted(scan.stdout.splitlines()) == expected and scan.returncode == expected_exit
                            record['samples'].append({'cycle': cycle, 'operation': operation,
                                                      'visible_ms': elapsed_ms, 'correct': correct,
                                                      'expected': expected, 'observations': observations})
                            args.output.write_text(json.dumps(result, indent=2) + '\n')
                            assert correct, f'{name}/{operation}: did not converge within 10 seconds'
                    record['final_status'] = status(discovery['port'])
                finally:
                    server.terminate()
                    try:
                        server.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait()
        args.output.write_text(json.dumps(result, indent=2) + '\n')
        print(f'{name}: {len(record["samples"])} changes converged and matched independent scans', flush=True)


if __name__ == '__main__':
    main()
