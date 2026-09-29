#!/usr/bin/env python3
"""Per-query latency comparison of ogrep binaries, each on its own index.

Every query runs `--rounds` times per variant, with variants interleaved in a
seeded random order inside each round, so drift in machine load hits all
variants alike. The figure per query is the median wall time; the summary gives
the geometric mean of per-query speedups over the first variant, total median
time, p50/p95 of per-query medians, and total candidate files read (the summary
`searches` counter, from one extra `--json` run).

Usage:
  bench.py --corpus DIR --queries JSON... --variant NAME=BIN:INDEX ... [--rounds 5]
      [--flags "-n"] [--json-out FILE]
"""

import argparse
import os
import json
import math
import random
import statistics
import subprocess
import time
from pathlib import Path


def load_queries(paths):
    out, seen = [], set()
    for p in paths:
        data = json.loads(Path(p).read_text())
        for q in data["queries"] if isinstance(data, dict) else data:
            if q not in seen:
                seen.add(q)
                out.append(q)
    return out


ENV = {}


def timed(argv, cwd, env=None):
    t0 = time.perf_counter()
    subprocess.run(argv, cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   env={**os.environ, **(env or {})})
    return (time.perf_counter() - t0) * 1000


def searches(argv, cwd, env=None):
    # `--json` must precede the `--` that ends the flags.
    cut = argv.index("--")
    out = subprocess.run(argv[:cut] + ["--json"] + argv[cut:], cwd=cwd, capture_output=True,
                         env={**os.environ, **(env or {})}).stdout
    for line in reversed(out.splitlines()):
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        if rec.get("type") == "summary":
            return rec["data"]["stats"].get("searches", 0)
    return 0


def pct(values, p):
    values = sorted(values)
    k = (len(values) - 1) * p
    lo, hi = math.floor(k), math.ceil(k)
    return values[lo] + (values[hi] - values[lo]) * (k - lo)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--queries", nargs="+", required=True)
    ap.add_argument("--variant", action="append", required=True, help="NAME=BIN:INDEX_DIR")
    ap.add_argument("--rounds", type=int, default=5)
    ap.add_argument("--flags", default="")
    ap.add_argument("--seed", type=int, default=11)
    ap.add_argument("--json-out")
    args = ap.parse_args()

    variants, envs, vflags = [], {}, {}
    for v in args.variant:
        name, rest = v.split("=", 1)
        # Optional extra flags and environment: NAME=BIN:INDEX#--flag --flag@KEY=VAL,KEY=VAL
        rest, _, env = rest.partition("@")
        rest, _, flags = rest.partition("#")
        vflags[name] = flags.split()
        binary, index = rest.rsplit(":", 1)
        variants.append((name, str(Path(binary).resolve()), str(Path(index).resolve())))
        envs[name] = dict(kv.split("=", 1) for kv in env.split(",") if kv)
    corpus = Path(args.corpus).resolve()
    queries = load_queries(args.queries)
    extra = args.flags.split()
    rng = random.Random(args.seed)

    def argv(v, q):
        return [v[1], "--index-path", v[2], *extra, *vflags[v[0]], "--", q]

    times = {v[0]: {q: [] for q in queries} for v in variants}
    # Warm the page cache for every index once.
    for v in variants:
        for q in queries[:10]:
            timed(argv(v, q), corpus, envs[v[0]])
    for _ in range(args.rounds):
        for q in queries:
            order = variants[:]
            rng.shuffle(order)
            for v in order:
                times[v[0]][q].append(timed(argv(v, q), corpus, envs[v[0]]))
    med = {n: {q: statistics.median(t) for q, t in qs.items()} for n, qs in times.items()}
    work = {v[0]: sum(searches(argv(v, q), corpus, envs[v[0]]) for q in queries) for v in variants}

    base = variants[0][0]
    report = {"corpus": str(corpus), "queries": len(queries), "rounds": args.rounds,
              "flags": args.flags, "variants": {}}
    print(f"{corpus.name}: {len(queries)} queries x {args.rounds} rounds, flags {args.flags!r}")
    for name, _, _ in variants:
        m = med[name]
        speedups = [med[base][q] / m[q] for q in queries if m[q] > 0]
        geo = math.exp(sum(math.log(s) for s in speedups) / len(speedups))
        row = {"total_ms": sum(m.values()), "p50_ms": pct(list(m.values()), 0.5),
               "p95_ms": pct(list(m.values()), 0.95), "geomean_speedup_vs_" + base: geo,
               "candidate_files": work[name]}
        report["variants"][name] = row
        print(f"  {name:>10}: total {row['total_ms']:8.1f} ms  p50 {row['p50_ms']:6.2f}  "
              f"p95 {row['p95_ms']:7.2f}  geomean x{geo:5.3f}  candidates {work[name]}")
    # Largest regressions and wins against the first variant, for inspection.
    if len(variants) > 1:
        other = variants[1][0]
        ratio = sorted(((med[other][q] / med[base][q], q) for q in queries), reverse=True)
        print("  slowest vs base:", [(round(r, 2), q) for r, q in ratio[:5]])
        print("  fastest vs base:", [(round(r, 2), q) for r, q in ratio[-5:]])
        report["per_query"] = {q: {n: med[n][q] for n, _, _ in variants} for q in queries}
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
