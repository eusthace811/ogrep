#!/usr/bin/env python3
"""Differential freeze check: a candidate ogrep must answer exactly like a baseline.

Every query in the matrix runs through four combinations of binary and index:

  base/base   baseline binary on the baseline's index (the reference)
  cand/cand   candidate binary on its own index (new builds must not change answers)
  cand/base   candidate on an index the baseline built (upgrading keeps old indexes working)
  base/cand   baseline on an index the candidate built (downgrading stays safe)

Output order follows file IDs, which come from a parallel walk, so results are compared
per file: each file's block of lines must match exactly, but blocks may appear in any
order. Runs with `--sort=path` are compared byte for byte. Exit codes and stderr are
compared too, after removing timing fields.

Usage:
  scripts/freeze/freeze.py --base BIN --cand BIN --corpus DIR [--queries JSON ...]
      [--work DIR] [--jobs N] [--flags default,json,...] [--limit N]
"""

import argparse
import concurrent.futures as cf
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Each entry: name -> extra argv. Chosen to exercise every output path that exists today.
FLAG_MATRIX = {
    "default": [],
    "n": ["-n"],
    "json": ["--json"],
    "l": ["-l"],
    "c": ["-c"],
    "count_matches": ["--count-matches"],
    "context": ["-n", "-A2", "-B1"],
    "w": ["-w"],
    "i": ["-i"],
    "F": ["-F"],
    "sort_path": ["--sort=path", "-n"],
    "vimgrep": ["--vimgrep"],
    "only": ["-o", "-n"],
    "files_without": ["--files-without-match"],
    "max_count": ["-m", "2", "-n"],
    "with": ["--with", "fn"],
    "without": ["--without", "test"],
}

# bytes_printed counts the printed timing strings, whose length varies run to run.
WORK_KEYS = ("searches", "bytes_searched")
TIMING_KEYS = ("elapsed", "elapsed_total", "secs", "nanos", "human", "bytes_printed")


def run(argv, cwd, timeout=120):
    try:
        p = subprocess.run(argv, cwd=cwd, capture_output=True, timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -999, b"", b"TIMEOUT"


def scrub_json(value):
    if isinstance(value, dict):
        return {k: (0 if k in TIMING_KEYS else scrub_json(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [scrub_json(v) for v in value]
    return value


def normalize_json(stdout):
    """Group rg-style JSON records into per-file blocks, sorted by path."""
    blocks, current, tail = {}, None, []
    for raw in stdout.splitlines():
        if not raw.strip():
            continue
        try:
            rec = scrub_json(json.loads(raw))
        except json.JSONDecodeError:
            tail.append(("raw", raw.decode("utf-8", "replace")))
            continue
        kind = rec.get("type")
        if kind == "begin":
            current = json.dumps(rec.get("data", {}).get("path"), sort_keys=True)
            blocks.setdefault(current, []).append(json.dumps(rec, sort_keys=True))
        elif kind in ("match", "context", "end") and current is not None:
            blocks[current].append(json.dumps(rec, sort_keys=True))
            if kind == "end":
                current = None
        else:
            if kind == "summary":
                # Work counters: how many candidate files were read. A better
                # index legitimately reads fewer; matches must not change.
                stats = rec.get("data", {}).get("stats", {})
                for key in WORK_KEYS:
                    stats.pop(key, None)
            tail.append(json.dumps(rec, sort_keys=True))
    return [blocks[k] for k in sorted(blocks)] + [tail]


def normalize_text(stdout):
    """Sorted multiset of output lines. Paths can contain ':' and '-12-', so lines are
    not grouped by file; order within a file is covered by the byte-exact
    `--sort=path` runs instead."""
    return sorted(line for line in stdout.split(b"\n") if line)


def normalize(stdout, flags):
    if "--sort=path" in flags:
        return stdout
    if "--json" in flags:
        return normalize_json(stdout)
    return normalize_text(stdout)


def scrub_stderr(stderr):
    return re.sub(rb"\d+(\.\d+)?\s*(ms|s|us|\xc2\xb5s)\b", b"<t>", stderr)


def build_index(binary, corpus, index_dir):
    if index_dir.exists():
        shutil.rmtree(index_dir)
    t0 = time.perf_counter()
    code, out, err = run([binary, "index", ".", "--index-path", str(index_dir)], corpus, timeout=3600)
    if code != 0:
        sys.exit(f"index build failed for {binary}: {err.decode(errors='replace')}")
    return time.perf_counter() - t0


def load_queries(paths, limit):
    queries = []
    for p in paths:
        data = json.loads(Path(p).read_text())
        items = data["queries"] if isinstance(data, dict) else data
        queries.extend(items)
    seen, out = set(), []
    for q in queries:
        if q not in seen:
            seen.add(q)
            out.append(q)
    return out[:limit] if limit else out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--cand", required=True)
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--queries", nargs="+", required=True)
    ap.add_argument("--work", default=None)
    ap.add_argument("--jobs", type=int, default=os.cpu_count() // 2 or 4)
    ap.add_argument("--flags", default=",".join(FLAG_MATRIX))
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--combos", default="cand/cand,cand/base,base/cand")
    ap.add_argument("--reuse-index", action="store_true")
    args = ap.parse_args()

    corpus = Path(args.corpus).resolve()
    work = Path(args.work or f"/tmp/ogrep-freeze/{corpus.name}").resolve()
    work.mkdir(parents=True, exist_ok=True)
    idx = {"base": work / "idx-base", "cand": work / "idx-cand"}
    bins = {"base": str(Path(args.base).resolve()), "cand": str(Path(args.cand).resolve())}

    for who in ("base", "cand"):
        if args.reuse_index and (idx[who] / "meta.json").exists():
            continue
        secs = build_index(bins[who], corpus, idx[who])
        print(f"index {who}: {secs:.1f}s", file=sys.stderr)

    queries = load_queries(args.queries, args.limit)
    flag_names = [f for f in args.flags.split(",") if f]
    combos = [tuple(c.split("/")) for c in args.combos.split(",") if c]
    cases = [(q, f) for q in queries for f in flag_names]
    print(f"{len(cases)} cases x {1 + len(combos)} runs on {corpus}", file=sys.stderr)

    def check(case):
        query, fname = case
        flags = FLAG_MATRIX[fname]
        # `--` keeps queries that start with '-' from being read as flags.
        def go(b, i):
            return run([bins[b], "--index-path", str(idx[i]), *flags, "--", query], corpus)
        ref = go("base", "base")
        ref_n = (ref[0], normalize(ref[1], flags), scrub_stderr(ref[2]))
        diffs = []
        for b, i in combos:
            got = go(b, i)
            got_n = (got[0], normalize(got[1], flags), scrub_stderr(got[2]))
            if got_n != ref_n:
                what = [n for n, a, c in zip(("exit", "stdout", "stderr"), ref_n, got_n) if a != c]
                diffs.append({"combo": f"{b}/{i}", "differs": what,
                              "ref_exit": ref[0], "got_exit": got[0],
                              "ref_bytes": len(ref[1]), "got_bytes": len(got[1]),
                              "got_stderr": got[2][:300].decode(errors="replace")})
        return query, fname, diffs

    failures = []
    t0 = time.perf_counter()
    with cf.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for query, fname, diffs in pool.map(check, cases):
            if diffs:
                failures.append({"query": query, "flags": fname, "diffs": diffs})
    secs = time.perf_counter() - t0

    report = work / "freeze-report.json"
    report.write_text(json.dumps({"corpus": str(corpus), "cases": len(cases),
                                  "failures": failures}, indent=1))
    print(f"{len(cases)} cases, {len(failures)} differing, {secs:.1f}s; report: {report}")
    for f in failures[:20]:
        print(f"  DIFF {f['flags']:>14} {f['query']!r}: {f['diffs']}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
