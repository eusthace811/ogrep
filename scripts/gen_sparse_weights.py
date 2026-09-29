#!/usr/bin/env python3
"""Generate ogrep-core/src/sparse_weights.bin, the byte-pair weights for sparse n-grams.

A sparse n-gram is a substring whose two edge byte pairs both outweigh every
byte pair inside it. Weighting rare pairs highest (Cursor's frequency weights,
rather than GitHub's hash weights) makes grams end at rare pairs and span common
ones, which gives shorter posting lists for the same number of grams.

Pairs are counted over ASCII-lowercased text, because ogrep extracts sparse
n-grams from lowercased content only. The weight of a pair is its rank by
ascending frequency: the most frequent pair gets 0 and the rarest (or unseen)
pair gets 65535. Ties break by pair value, so every pair has a distinct weight
and the table is deterministic. It is written as 65536 little-endian u16 values,
indexed by `(first << 8) | second`.

The table is a build input checked into the repository: changing it changes
which grams an index contains, so it must be paired with a bump of
`trigram::SPARSE_GRAMS_VERSION`.

Usage: gen_sparse_weights.py OUT CORPUS [CORPUS ...]
"""

import subprocess
import sys
from pathlib import Path

import numpy as np

MAX_FILE = 4 << 20


def tracked_text_files(root):
    names = subprocess.run(["git", "ls-files", "-z"], cwd=root, capture_output=True, check=True).stdout
    for name in names.split(b"\0"):
        if name:
            yield Path(root) / name.decode("utf-8", "surrogateescape")


def main():
    out, corpora = sys.argv[1], sys.argv[2:]
    counts = np.zeros(65536, dtype=np.int64)
    files = total = 0
    for root in corpora:
        for path in tracked_text_files(root):
            try:
                data = path.read_bytes()
            except OSError:
                continue
            if len(data) < 2 or len(data) > MAX_FILE or b"\0" in data[:8192]:
                continue
            a = np.frombuffer(data.lower(), dtype=np.uint8).astype(np.int64)
            counts += np.bincount((a[:-1] << 8) | a[1:], minlength=65536)
            files += 1
            total += len(data)
    # Ascending frequency, ties by pair value; rank 0 = most frequent.
    order = np.lexsort((np.arange(65536), -counts))
    weights = np.empty(65536, dtype="<u2")
    weights[order] = np.arange(65536, dtype=np.uint32)
    Path(out).write_bytes(weights.tobytes())
    print(f"{files} files, {total >> 20} MB from {len(corpora)} corpora -> {out}", file=sys.stderr)


if __name__ == "__main__":
    main()
