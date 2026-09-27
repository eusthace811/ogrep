#!/usr/bin/env python3
"""Profile ordinary file-list RPC bytes and client costs without enabling stats."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import platform
import random
import re
import socket
import statistics
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
    ap.add_argument('--expected-revision', required=True)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--rounds', type=int, default=9)
    ap.add_argument('--compare-formats', action='store_true',
                    help='Interleave legacy and opt-in compact requests to the same binary')
    args = ap.parse_args()
    assert args.rounds > 0
    binary, root, index = args.binary.resolve(), args.corpus.resolve(), args.index.resolve()
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    assert revision == args.expected_revision
    assert not subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=no'], cwd=root)
    provenance = json.loads((index.parent / 'benchmark-input.json').read_text())
    # evaluate.py records the corpus root and revision beside its index.
    assert provenance['corpus'] == str(root) and provenance['revision'] == revision, provenance
    assert Path(json.loads((index / 'meta.json').read_text())['root_path']).resolve() == root
    patterns = ['fn main', 'NonNull', 'typeck', 'absent_OGREP_897342938', r'impl.*Debug', r'(alloc|dealloc).*Layout']
    expected = []
    for pattern in patterns:
        result = subprocess.run(['rg', '--no-config', '-l', '-t', 'rust', '--', pattern, '.'],
                                cwd=root, capture_output=True, timeout=60)
        assert result.returncode in (0, 1), result.stderr
        expected.append(sorted(result.stdout.decode().splitlines()))
    record = {
        'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'revision': revision, 'corpus': str(root), 'index_provenance': provenance,
        'platform': platform.platform(), 'rounds': args.rounds, 'compare_formats': args.compare_formats,
        'description': 'Sequential loopback ordinary files_only RPC, four server workers, randomized warm rounds after one warmup per query. Receive time includes server work and transport; client JSON parsing timed separately. Server elapsed excludes final response construction/serialization. Compact projection is an offline byte estimate, NOT an implemented performance result. Independent rg expected sets on every request.',
        'patterns': patterns, 'expected_file_counts': list(map(len, expected)), 'samples': [],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    log_path = args.output.with_suffix('.server.log')
    with log_path.open('w') as log:
        server = subprocess.Popen([str(binary), 'serve', str(root), '--index-path', str(index),
                                   '--no-watch', '-j', '4'], stdout=log, stderr=log)
        try:
            port, record['initial_status'] = load.ready(server, index)

            formats = ['legacy', 'compact'] if args.compare_formats else ['legacy']

            def request(query, round_id, response_format):
                payload = (json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': 'search',
                                      'params': {'pattern': patterns[query], 'types': ['rust'],
                                                 'files_only': True,
                                                 'compact_files': response_format == 'compact'}}) + '\n').encode()
                start = time.perf_counter_ns()
                with socket.create_connection(('127.0.0.1', port), timeout=60) as sock:
                    connected = time.perf_counter_ns()
                    sock.sendall(payload)
                    with sock.makefile('rb') as reader:
                        raw = reader.readline()
                    received = time.perf_counter_ns()
                decode_start = time.perf_counter_ns()
                response = json.loads(raw)
                decoded = time.perf_counter_ns()
                assert 'error' not in response, response
                result = response['result']
                actual = sorted({'./' + row['file'].removeprefix('./') for row in result['matches']
                                 if row['type'] == 'match'})
                assert actual == expected[query], f'output mismatch for {patterns[query]}'
                # Retain other fields and binary rows: this estimates only removing
                # match text/positions, not changing counts or the wire envelope.
                result['matches'] = [({'type': 'match', 'file': row['file']}
                                      if row['type'] == 'match' else row) for row in result['matches']]
                compact_bytes = len(json.dumps(response, ensure_ascii=False, separators=(',', ':')).encode()) + 1
                return {'query': query, 'round': round_id, 'format': response_format, 'correct': True,
                        'connect_ms': (connected-start)/1e6, 'receive_ms': (received-connected)/1e6,
                        'decode_ms': (decoded-decode_start)/1e6, 'end_to_end_ms': (decoded-start)/1e6,
                        'server_elapsed_ms': result['elapsed_ms'], 'bytes': len(raw),
                        'offline_compact_bytes': compact_bytes}

            record['warmups'] = [request(query, -1, response_format)
                                 for query in range(len(patterns)) for response_format in formats]
            record['resources_before'] = load.resources(server.pid)
            rng = random.Random(86723)
            for round_id in range(args.rounds):
                queries = [(query, response_format) for query in range(len(patterns)) for response_format in formats]
                rng.shuffle(queries)
                for query, response_format in queries:
                    record['samples'].append(request(query, round_id, response_format))
            record['resources_after'] = load.resources(server.pid)
        finally:
            load.stop(server)
    # Trace order is request order because this experiment has one client.
    phases = re.findall(r'elapsed=([\d.]+)ms \(index=([\d.]+)ms resolve=([\d.]+)ms search=([\d.]+)ms\)',
                        log_path.read_text())
    samples = record['warmups'] + record['samples']
    assert len(phases) == len(samples), (len(phases), len(samples))
    for sample, phase in zip(samples, phases):
        sample['trace_ms'] = dict(zip(['elapsed', 'index', 'resolve', 'search'], map(float, phase)))
    record['summary'] = []
    for query, response_format in [(q, f) for q in range(len(patterns)) for f in formats]:
        selected = [s for s in record['samples'] if s['query'] == query and s['format'] == response_format]
        fields = ['connect_ms', 'receive_ms', 'decode_ms', 'end_to_end_ms', 'server_elapsed_ms',
                  'bytes', 'offline_compact_bytes']
        row = {'pattern': patterns[query], 'format': response_format, 'files': len(expected[query]),
               **{f'median_{key}': statistics.median(s[key] for s in selected) for key in fields},
               'median_trace_ms': {key: statistics.median(s['trace_ms'][key] for s in selected)
                                   for key in ['index', 'resolve', 'search']}}
        record['summary'].append(row)
        print(json.dumps(row), flush=True)
    assert hashlib.sha256(binary.read_bytes()).hexdigest() == record['binary_sha256'], 'binary changed'
    assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == revision
    assert not subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=no'], cwd=root)
    args.output.write_text(json.dumps(record, indent=2) + '\n')


if __name__ == '__main__':
    main()
