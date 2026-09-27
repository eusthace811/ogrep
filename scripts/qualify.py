#!/usr/bin/env python3
"""Run local Linux release gates and retain command, output and binary provenance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--upstream', type=Path, required=True)
    args = ap.parse_args()
    directory = args.output.parent
    directory.mkdir(parents=True, exist_ok=True)
    commands = [
        ('format', ['cargo', 'fmt', '--all', '--', '--check']),
        ('clippy', ['cargo', 'clippy', '--workspace', '--all-targets', '--all-features', '--locked', '--offline', '--', '-D', 'warnings']),
        ('workspace', ['cargo', 'test', '--workspace', '--all-features', '--locked', '--offline']),
        ('polling', ['cargo', 'test', '-p', 'zgrep-cli', '--locked', '--offline', 'measure_polling_1000_files_and_20_file_change_batch', '--', '--ignored', '--nocapture']),
        ('sparse-example', ['cargo', 'test', '-p', 'zgrep-core', '--example', 'sparse_probe', '--locked', '--offline']),
        ('agent', ['python3', 'scripts/agent/test_agent.py']),
        ('installer', ['bash', 'scripts/test-install.sh']),
        ('output', ['python3', 'scripts/evaluate-output-modes.py', '--binary', 'target/release/zgrep', '--output', str(directory/'output-modes.json')]),
        ('unicode', ['python3', 'scripts/compare-unicode.py', '--upstream', str(args.upstream), '--fork', 'target/release/zgrep', '--output', str(directory/'unicode.json')]),
    ]
    binary = Path('target/release/zgrep')
    record = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'version': subprocess.check_output([str(binary), '--version'], text=True).strip(),
              'toolchain': subprocess.check_output(['rustc', '-Vv'], text=True), 'checks': []}
    for name, command in commands:
        started = time.monotonic()
        log = directory/f'qualification-{name}.log'
        with log.open('w') as stream:
            p = subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT,
                               env={**os.environ, 'ZGREP_AGENT_TEST_BINARY': str(binary.resolve())})
        content = log.read_text()
        row = {'name': name, 'command': command, 'exit': p.returncode,
               'elapsed_seconds': time.monotonic()-started, 'log': str(log)}
        if name in ('workspace', 'polling', 'sparse-example'):
            row['passed'] = sum(map(int, re.findall(r'test result: ok\. (\d+) passed', content)))
            row['ignored'] = sum(map(int, re.findall(r'test result: ok\. \d+ passed; \d+ failed; (\d+) ignored', content)))
        record['checks'].append(row)
        args.output.write_text(json.dumps(record, indent=2)+'\n')
        print(name, 'PASS' if p.returncode == 0 else 'FAIL', flush=True)
        if p.returncode:
            raise SystemExit(p.returncode)
    assert record['binary_sha256'] == hashlib.sha256(binary.read_bytes()).hexdigest()


if __name__ == '__main__':
    main()
