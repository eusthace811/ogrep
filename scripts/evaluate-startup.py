#!/usr/bin/env python3
"""Measure restarted server readiness, reconciliation and first validated query."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import time

spec = importlib.util.spec_from_file_location('load_eval', Path(__file__).with_name('evaluate-load.py'))
load = importlib.util.module_from_spec(spec)
spec.loader.exec_module(load)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary', type=Path, required=True)
    ap.add_argument('--corpus', type=Path, required=True)
    ap.add_argument('--index', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    binary, root, index = args.binary.resolve(), args.corpus.resolve(), args.index.resolve()
    expected = subprocess.check_output(['rg', '--no-config', '-l', '-t', 'rust', '--', 'NonNull', '.'], cwd=root).splitlines()
    expected = sorted(x.decode().removeprefix('./') for x in expected)
    record = {'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'corpus': str(root),
              'description': 'Ten sequential warm-page-cache process restarts using the same frozen Rust index. Readiness includes completed startup reconciliation and up to 50ms status polling delay. First RPC query file set independently checked against rg.', 'samples': []}
    with args.output.with_suffix('.server.log').open('w') as log:
        for round_id in range(10):
            started = time.perf_counter()
            server = subprocess.Popen([str(binary), 'serve', str(root), '--index-path', str(index), '--no-watch', '-j', '4'], stdout=log, stderr=log)
            try:
                port, status = load.ready(server, index)
                ready_ms = (time.perf_counter()-started)*1000
                before = time.perf_counter()
                result = load.rpc(port, 'search', {'pattern': 'NonNull', 'types': ['rust'], 'files_only': True})
                query_ms = (time.perf_counter()-before)*1000
                actual = sorted({r['file'].removeprefix('./') for r in result['matches'] if r['type'] == 'match'})
                assert actual == expected
                record['samples'].append({'round': round_id, 'ready_ms': ready_ms, 'first_query_ms': query_ms,
                                          'correct': True, 'resources': load.resources(server.pid)})
                args.output.write_text(json.dumps(record, indent=2)+'\n')
            finally:
                load.stop(server)
            print(f'restart {round_id+1}/10', flush=True)


if __name__ == '__main__':
    main()
