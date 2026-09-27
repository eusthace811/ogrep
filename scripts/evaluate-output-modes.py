#!/usr/bin/env python3
"""Compare complete CLI output semantics against ripgrep in scan/local/server modes."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def normalized(stdout, flags):
    if '--json' not in flags:
        return stdout.decode()
    records = []
    for line in stdout.splitlines():
        record = json.loads(line)
        kind, data = record['type'], record['data']
        # Search-work and timing counters differ intentionally with an index;
        # retain paths, exact bytes/text, line/byte offsets and match counts.
        if kind in ('end', 'summary'):
            data = {k: v for k, v in data.items() if k not in ('stats', 'elapsed_total')}
            stats = record['data']['stats']
            data['matches'] = stats['matches']
            data['matched_lines'] = stats['matched_lines']
        records.append({'type': kind, 'data': data})
    return records


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    binary = args.binary.resolve()
    rg = Path(shutil.which('rg')).resolve()
    cases = [
        ('lines', [], ['needle', '.']),
        ('literal', ['-F'], ['needle', '.']),
        ('regex', [], ['(alpha|beta).*needle', '.']),
        ('count-lines', ['-c'], ['needle', '.']),
        ('count-matches', ['--count-matches'], ['needle', '.']),
        ('files', ['-l'], ['needle', '.']),
        ('without-match', ['--files-without-match'], ['needle', '.']),
        ('quiet-match', ['-q'], ['needle', '.']),
        ('quiet-absent', ['-q'], ['absent_9471832', '.']),
        ('context', ['-C', '1'], ['needle', '.']),
        ('json', ['--json'], ['needle', '.']),
        ('json-context', ['--json', '-C', '1'], ['needle', '.']),
        ('only-matching', ['-o'], ['needle', '.']),
        ('inverted', ['-v'], ['needle', '.']),
        ('max-count', ['-m', '1'], ['needle', '.']),
        ('word', ['-w'], ['needle', '.']),
        ('glob', ['-g', 'src/**'], ['needle', '.']),
        ('rust-type', ['-t', 'rust'], ['needle', '.']),
        ('rust-type-negative', ['-T', 'txt'], ['needle', '.']),
        ('custom-type', ['--type-add', 'custom:*.rs', '-t', 'custom'], ['needle', '.']),
        ('excluded-type', ['-t', 'rust', '-T', 'rust'], ['needle', '.']),
        ('list-rust-type', ['--files', '-t', 'rust'], ['.']),
        ('list-negative-type', ['--files', '-T', 'txt'], ['.']),
        ('hidden', ['--hidden'], ['needle', '.']),
        ('explicit-hidden', [], ['needle', '.hidden.rs']),
        ('multiple', ['-e', 'needle', '-e', 'other'], ['.']),
        ('unicode-kelvin', ['-Fi'], ['kelvin', '.']),
        ('unicode-accent', ['-Fi'], ['café', '.']),
        ('multiline', ['-U'], ['alpha needle\nnext', '.']),
        ('pcre-lookbehind', ['-P'], ['(?<=alpha )needle', '.']),
        ('invalid-regex', [], ['[', '.']),
        ('missing-root', [], ['needle', 'missing.rs']),
    ]
    result = {'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'rg_version': subprocess.check_output([str(rg), '--version'], text=True).strip(),
              'normalization': 'Plain output exact bytes decoded as UTF-8; JSON omits performance counters only, preserving semantic events and match counts. Error wording not compared; exit status is.',
              'checks': []}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='ogrep-output-eval-') as directory:
        temporary = Path(directory)
        root, index = temporary / 'corpus', temporary / 'index'
        root.mkdir()
        (root / '.git').mkdir()
        (root / 'src').mkdir()
        (root / '.hidden-dir').mkdir()
        fixtures = {
            'a.rs': 'before\nalpha needle\nnext\nneedle needle\nneedlework\nother\n',
            'src/b.rs': 'beta needle\r\nKELVIN CAFÉ\r\nend\r\n',
            'empty.rs': '', 'none.rs': 'unrelated content\n',
            'note.txt': 'needle\n', '.hidden.rs': 'needle\n',
            'unterminated.rs': 'needle without a final newline',
            'ignored.rs': 'needle\n', '.gitignore': 'ignored.rs\n.hidden-ignored.rs\n',
            '.hidden-dir/nested.rs': 'needle\n', '.hidden-ignored.rs': 'needle\n',
        }
        for path, content in fixtures.items():
            (root / path).write_bytes(content.encode())
        result['fixtures'] = fixtures
        subprocess.run([str(binary), 'index', str(root), '--index-path', str(index)],
                       capture_output=True, check=True, timeout=60)
        reference = {}
        common = ['--no-config', '--sort', 'path', '--no-heading', '--color', 'never', '-n']
        for name, flags, operands in cases:
            p = subprocess.run([str(rg), *common, *flags, '--', *operands], cwd=root, capture_output=True, timeout=10)
            reference[name] = (p.returncode, normalized(p.stdout, flags))

        def check(mode):
            for name, flags, operands in cases:
                command = [str(binary), *common, '--index-path', str(index), *flags]
                if mode == 'scan':
                    command += ['--no-index']
                p = subprocess.run([*command, '--', *operands], cwd=root, capture_output=True, timeout=10)
                actual = (p.returncode, normalized(p.stdout, flags))
                correct = actual == reference[name]
                result['checks'].append({'mode': mode, 'case': name, 'flags': flags, 'operands': operands,
                                         'correct': correct, 'expected': reference[name], 'actual': actual,
                                         'stderr': p.stderr.decode()})
                args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
                if not correct:
                    print(f'MISMATCH: {mode}/{name}', flush=True)

        check('scan')
        check('local')
        with args.output.with_suffix('.server.log').open('w') as log:
            server = subprocess.Popen([str(binary), 'serve', str(root), '--index-path', str(index), '--no-watch'],
                                      stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 30
                while True:
                    assert server.poll() is None and time.monotonic() < deadline, 'server failed to start'
                    try:
                        ready = json.loads((index / 'serve.json').read_text()).get('pid') == server.pid
                    except (FileNotFoundError, json.JSONDecodeError):
                        ready = False
                    if ready:
                        probe = subprocess.run([str(binary), '--stats', '--index-path', str(index), '-l',
                                                '--', 'needle', '.'], cwd=root, capture_output=True, text=True, timeout=10)
                        if '(via server)' in probe.stderr:
                            break
                    time.sleep(.05)
                check('server')
            finally:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()
    failures = sum(not c['correct'] for c in result['checks'])
    print(f'{len(result["checks"])} checks, {failures} mismatches')
    raise SystemExit(1 if failures else 0)


if __name__ == '__main__':
    main()
