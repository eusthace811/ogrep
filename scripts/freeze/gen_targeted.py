#!/usr/bin/env python3
"""Sample a deterministic set of targeted queries from a corpus.

Broad queries (`pub fn`) are where an index helps least; targeted ones (long
identifiers, phrases from comments and strings, rare literals) are where sparse
n-grams should help most. This samples both kinds from the files git tracks,
using a fixed seed, so every run and every machine gets the same set.

Usage: gen_targeted.py CORPUS [--n 150] [--seed 7] > queries.json
"""

import argparse
import json
import random
import re
import subprocess
from pathlib import Path

IDENT = re.compile(rb"\b[A-Za-z_][A-Za-z0-9_]{9,40}\b")
PHRASE = re.compile(rb"[A-Za-z][A-Za-z ,'-]{18,60}[A-Za-z]")
META = re.compile(r"[.^$*+?()\[\]{}|\\]")
EXTS = {".rs", ".py", ".c", ".h", ".go", ".js", ".ts", ".java", ".md", ".txt", ".toml", ".rst", ".html"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("corpus")
    ap.add_argument("--n", type=int, default=150)
    ap.add_argument("--seed", type=int, default=7)
    args = ap.parse_args()
    root = Path(args.corpus)
    files = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, check=True).stdout.decode().splitlines()
    files = sorted(f for f in files if Path(f).suffix in EXTS)
    rng = random.Random(args.seed)
    rng.shuffle(files)
    idents, phrases = [], []
    for f in files:
        if len(idents) >= args.n and len(phrases) >= args.n // 2:
            break
        try:
            data = (root / f).read_bytes()
        except OSError:
            continue
        if b"\0" in data[:8192] or len(data) > 2_000_000:
            continue
        ids = IDENT.findall(data)
        if ids and len(idents) < args.n:
            idents.append(rng.choice(ids).decode())
        ph = [p.strip() for p in PHRASE.findall(data) if p.count(b" ") >= 2]
        if ph and len(phrases) < args.n // 2:
            phrases.append(rng.choice(ph).decode("utf-8", "replace"))
    queries = []
    seen = set()
    for q in idents + [META.sub(r"\\\g<0>", p) for p in phrases]:
        if q not in seen:
            seen.add(q)
            queries.append(q)
    print(json.dumps({"corpus": str(root), "seed": args.seed, "queries": queries}, indent=1))


if __name__ == "__main__":
    main()
