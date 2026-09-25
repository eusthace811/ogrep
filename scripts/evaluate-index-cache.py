#!/usr/bin/env python3
"""Linux warm/cold index-file cache experiment with mincore-verified eviction.

Only disposable copies of benchmark indexes are evicted. Corpus pages, executable
pages, filesystem metadata and device caches are not claimed cold.
"""
import argparse
import ctypes
import hashlib
import json
import mmap
import os
from pathlib import Path
import random
import shutil
import subprocess
import tempfile
import time

libc = ctypes.CDLL(None, use_errno=True)
libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
libc.mincore.restype = ctypes.c_int


def residency(path, evict=False):
    with path.open('rb') as f:
        length = os.fstat(f.fileno()).st_size
        if not length:
            return 0, 0
        if evict:
            os.posix_fadvise(f.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
        pages = (length + mmap.PAGESIZE-1)//mmap.PAGESIZE
        with mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_COPY) as mapping:
            anchor = ctypes.c_char.from_buffer(mapping)
            vector = (ctypes.c_ubyte * pages)()
            answer = libc.mincore(ctypes.addressof(anchor), length, vector)
            del anchor
            if answer:
                raise OSError(ctypes.get_errno(), 'mincore')
            return pages, sum(bool(x & 1) for x in vector)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--engine', action='append', required=True, help='NAME=BINARY=INDEX')
    ap.add_argument('--corpus', type=Path, required=True)
    ap.add_argument('--rounds', type=int, default=15)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    root = args.corpus.resolve()
    patterns = ['ZGREP_INDEX_CACHE_ABSENT_834789123', 'NonNull']
    expected = {}
    for pattern in patterns:
        p = subprocess.run(['rg', '--no-config', '-l', '-F', '--', pattern, '.'], cwd=root, capture_output=True, timeout=60)
        assert p.returncode in (0,1)
        expected[pattern] = (p.returncode, sorted(p.stdout.splitlines()))
    result = {'description': __doc__, 'corpus': str(root), 'rounds': args.rounds, 'engines': {}, 'samples': []}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='zgrep-cache-eval-') as directory:
        engines = {}
        for entry in args.engine:
            name, binary, original = entry.split('=', 2)
            binary = Path(binary).resolve()
            index = Path(directory)/name
            shutil.copytree(Path(original).resolve(), index)
            for f in index.rglob('*'):
                if f.is_file():
                    with f.open('rb') as stream:
                        os.fsync(stream.fileno())
            files = [f for f in index.rglob('*') if f.is_file()]
            engines[name] = (binary, index, files)
            result['engines'][name] = {'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                                       'index_bytes': sum(f.stat().st_size for f in files)}
        rng = random.Random(97113)
        for round_id in range(args.rounds):
            order = [(name, pattern) for name in engines for pattern in patterns]
            rng.shuffle(order)
            for name, pattern in order:
                binary, index, files = engines[name]
                counts = [residency(f, evict=True) for f in files]
                pages, resident = map(sum, zip(*counts))
                assert resident == 0, f'could not establish cold index for {name}: {resident}/{pages} resident pages'
                command = [str(binary), '--no-config', '-l', '-F', '-j', '4', '--index-path', str(index), '--', pattern, '.']
                for cache in ['cold-index', 'warm-index']:
                    before = time.perf_counter_ns()
                    p = subprocess.run(command, cwd=root, capture_output=True, timeout=60)
                    elapsed = (time.perf_counter_ns()-before)/1e6
                    actual = (p.returncode, sorted(p.stdout.splitlines()))
                    correct = actual == expected[pattern]
                    result['samples'].append({'round': round_id, 'engine': name, 'pattern': pattern, 'cache': cache,
                                              'latency_ms': elapsed, 'correct': correct,
                                              'evicted_index_pages': pages, 'resident_after_eviction': resident})
                    args.output.write_text(json.dumps(result, indent=2)+'\n')
                    assert correct, (name, pattern, p.stderr)
            print(f'round {round_id+1}/{args.rounds}', flush=True)


if __name__ == '__main__':
    main()
