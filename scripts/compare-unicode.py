#!/usr/bin/env python3
"""Compare indexed and scanned Unicode fixed-insensitive searches to known files."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--upstream', type=Path, required=True)
    parser.add_argument('--fork', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    engines = {'upstream': args.upstream.resolve(), 'fork': args.fork.resolve(),
               'rg': Path(shutil.which('rg')).resolve()}
    fixtures = [('kelvin.txt', 'KELVIN', 'kelvin'),
                ('shell.txt', 'ſHELL', 'shell'),
                ('cafe.txt', 'CAFÉ', 'café'),
                ('istanbul.txt', 'İSTANBUL', 'İstanbul'),
                ('ascii.txt', 'ORDINARY_IDENTIFIER', 'ordinary_identifier')]
    result = {'description': 'Known-positive fixed-insensitive cases; separate indexed and scan paths.',
              'engines': {}, 'fixtures': fixtures, 'queries': []}
    for name, binary in engines.items():
        result['engines'][name] = {
            'path': str(binary), 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
            'version': subprocess.check_output([str(binary), '--version'], text=True).strip()}
    with tempfile.TemporaryDirectory(prefix='zgrep-compare-unicode-') as directory:
        temporary = Path(directory)
        corpus = temporary / 'corpus'
        corpus.mkdir()
        (corpus / '.git').mkdir()
        for filename, content, _ in fixtures:
            (corpus / filename).write_text(content + '\n')
        for name in ('upstream', 'fork'):
            subprocess.run([str(engines[name]), 'index', str(corpus), '--index-path', str(temporary / name)],
                           check=True, capture_output=True, timeout=60)
        for filename, _, pattern in fixtures:
            query = {'pattern': pattern, 'expected_files': [filename], 'results': {}}
            for name, binary in engines.items():
                for mode in (('scan',) if name == 'rg' else ('indexed', 'scan')):
                    command = [str(binary), '--no-config', '-Fi', '-l', '--color', 'never']
                    if name != 'rg':
                        command += ['--index-path', str(temporary / name)]
                        if mode == 'scan':
                            command += ['--no-index']
                        else:
                            command += ['--stats']
                    command += ['--', pattern, '.']
                    p = subprocess.run(command, cwd=corpus, capture_output=True, text=True, timeout=30)
                    assert p.returncode in (0, 1), (command, p.stderr)
                    files = sorted(str(Path(row)) for row in p.stdout.splitlines())
                    query['results'][name + '-' + mode] = {
                        'files': files, 'exit': p.returncode, 'diagnostics': p.stderr,
                        'correct': files == [filename] and p.returncode == 0}
                    # Every scanner and both fork paths must match the known positive.
                    if name != 'upstream' or mode == 'scan':
                        assert files == [filename] and p.returncode == 0, query
            result['queries'].append(query)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__':
    main()
