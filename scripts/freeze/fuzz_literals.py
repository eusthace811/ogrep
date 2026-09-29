#!/usr/bin/env python3
"""Soundness fuzz: random literals sampled from the corpus must find the same files.

The index only ever narrows candidates, so a bug in it shows up as a file that
a reference search finds and the candidate misses. This samples substrings of
random length (3 to 48 bytes) at random offsets of random tracked text files,
optionally flips their case or searches case-insensitively, and compares the
`-l -F` file sets of a baseline binary on its index with a candidate binary on
its index. Seeded, so a failure reproduces.

Usage: fuzz_literals.py --base BIN:INDEX --cand BIN:INDEX --corpus DIR [--n 2000] [--seed 1]
"""

import argparse
import concurrent.futures as cf
import random
import subprocess
from pathlib import Path


def sample(root, n, seed):
    rng = random.Random(seed)
    names = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, text=True,
                           check=True).stdout.splitlines()
    names.sort()
    out = []
    while len(out) < n:
        name = rng.choice(names)
        try:
            data = (Path(root) / name).read_bytes()
        except OSError:
            continue
        if not data or b"\0" in data[:8192] or len(data) > 2_000_000:
            continue
        length = rng.choice([3, 4, 5, 6, 7, 8, 9, 10, 12, 16, 20, 24, 32, 48])
        if len(data) < length:
            continue
        start = rng.randrange(len(data) - length + 1)
        chunk = data[start:start + length]
        if b"\n" in chunk or b"\r" in chunk:
            continue
        try:
            text = chunk.decode("utf-8")
        except UnicodeDecodeError:
            continue
        if text.strip() != text or not text.strip():
            continue
        mode = rng.choice(["exact", "exact", "upper", "ci"])
        if mode == "upper":
            text = text.upper()
        out.append((text, mode))
    return out


def files(spec, root, text, mode):
    binary, index = spec
    argv = [binary, "--index-path", index, "-l", "-F"]
    if mode == "ci":
        argv.append("-i")
    p = subprocess.run(argv + ["--", text], cwd=root, capture_output=True)
    return p.returncode, sorted(p.stdout.split(b"\n"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--cand", required=True)
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--n", type=int, default=2000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--jobs", type=int, default=16)
    args = ap.parse_args()
    base = tuple(str(Path(p).resolve()) for p in args.base.rsplit(":", 1))
    cand = tuple(str(Path(p).resolve()) for p in args.cand.rsplit(":", 1))
    root = str(Path(args.corpus).resolve())
    cases = sample(root, args.n, args.seed)

    def check(case):
        text, mode = case
        a = files(base, root, text, mode)
        b = files(cand, root, text, mode)
        return case, a, b

    bad = []
    with cf.ThreadPoolExecutor(args.jobs) as pool:
        for case, a, b in pool.map(check, cases):
            if a != b:
                missing = sorted(set(a[1]) - set(b[1]))[:3]
                extra = sorted(set(b[1]) - set(a[1]))[:3]
                bad.append((case, a[0], b[0], missing, extra))
    print(f"{len(cases)} literals, {len(bad)} mismatches on {root}")
    for case, ea, eb, missing, extra in bad[:20]:
        print(f"  {case!r}: exit {ea} vs {eb}; missing {missing} extra {extra}")
    raise SystemExit(1 if bad else 0)


if __name__ == "__main__":
    main()
