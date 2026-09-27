#!/usr/bin/env python3
"""Measure fresh index elapsed time, CPU, peak RSS and index bytes on a frozen tree."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--engine', action='append', required=True)
    ap.add_argument('--corpus', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    root = args.corpus.resolve()
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    assert not subprocess.check_output(['git', 'status', '--porcelain'], cwd=root)
    result = {'corpus': str(root), 'revision': revision,
              'description': 'One fresh index build per engine, four threads, warm source cache; GNU time CPU and peak RSS. Descriptive resource measurements, not a statistical indexing-speed claim.', 'engines': {}}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='ogrep-index-resources-') as directory:
        for entry in args.engine:
            name, binary = entry.split('=', 1)
            binary = Path(binary).resolve()
            index = Path(directory)/name
            stat = Path(directory)/f'{name}.time'
            cmd = ['/usr/bin/time', '-f', '%e %U %S %M', '-o', str(stat), str(binary), 'index', str(root), '--index-path', str(index), '-j', '4']
            with (args.output.parent/f'index-resource-{name}.log').open('w') as log:
                subprocess.run(cmd, stdout=log, stderr=log, check=True, timeout=600)
            elapsed, user, system, rss = map(float, stat.read_text().split())
            result['engines'][name] = {'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                                       'command': cmd, 'elapsed_seconds': elapsed, 'user_seconds': user,
                                       'system_seconds': system, 'peak_rss_kib': int(rss),
                                       'index_bytes': sum(p.stat().st_size for p in index.rglob('*') if p.is_file())}
            args.output.write_text(json.dumps(result, indent=2)+'\n')
            print(name, result['engines'][name], flush=True)
    assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == revision
    assert not subprocess.check_output(['git', 'status', '--porcelain'], cwd=root)


if __name__ == '__main__':
    main()
