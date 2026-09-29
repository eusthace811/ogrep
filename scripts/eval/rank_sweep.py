#!/usr/bin/env python3
"""Grid search over --rank weights (OGREP_EXP_RANK) on the localization sets.

Every configuration is scored on each corpus; configurations are ordered by the
mean MRR across corpora, so a setting that helps one corpus at the other's
expense does not win.

Usage: rank_sweep.py --bin BIN --set CORPUS:DATA:INDEX ... [--grid 'def=1,3,6;path=0,2,5']
"""

import argparse
import concurrent.futures as cf
import itertools
import json
import os
import subprocess
from pathlib import Path


def first_relevant(argv, cwd, env, relevant):
    out = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, errors="replace", env=env).stdout
    for i, line in enumerate(l for l in out.splitlines() if l and not l.startswith("#")):
        if line in relevant:
            return i + 1
    return None


def score(binary, sets, spec, jobs):
    env = {**os.environ, "OGREP_EXP_RANK": spec}
    per = {}
    with cf.ThreadPoolExecutor(jobs) as pool:
        for corpus, data, index in sets:
            items = json.loads(Path(data).read_text())
            argv = lambda q: [binary, "--index-path", index, "-l", "-F", "--rank", "--budget", "100000", "--", q]
            ranks = list(pool.map(lambda it: first_relevant(argv(it["query"]), corpus, env, set(it["relevant"])), items))
            n = len(ranks)
            per[Path(corpus).name] = {
                "mrr": sum(1 / r for r in ranks if r) / n,
                "acc1": sum(1 for r in ranks if r == 1) / n,
                "acc5": sum(1 for r in ranks if r and r <= 5) / n,
            }
    return per


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--set", action="append", required=True)
    ap.add_argument("--grid", default="def=1,3,6;path=0,2,5,10;test=0.3,0.6,1;b=0.3,0.75")
    ap.add_argument("--jobs", type=int, default=16)
    ap.add_argument("--json-out")
    args = ap.parse_args()
    sets = [tuple(str(Path(p).resolve()) for p in s.split(":")) for s in args.set]
    axes = [(k, v.split(",")) for k, v in (a.split("=") for a in args.grid.split(";"))]
    results = []
    for combo in itertools.product(*[vals for _, vals in axes]):
        spec = ",".join(f"{k}={v}" for (k, _), v in zip(axes, combo))
        per = score(str(Path(args.bin).resolve()), sets, spec, args.jobs)
        mean = sum(p["mrr"] for p in per.values()) / len(per)
        results.append({"spec": spec, "mean_mrr": mean, **{f"{c}_{m}": v for c, p in per.items() for m, v in p.items()}})
        print(f"{mean:.4f}  {spec}  " + "  ".join(f"{c}: mrr {p['mrr']:.3f} acc1 {p['acc1']:.3f}" for c, p in per.items()), flush=True)
    results.sort(key=lambda r: -r["mean_mrr"])
    print("\nbest:")
    for r in results[:8]:
        print(f"  {r['mean_mrr']:.4f}  {r['spec']}")
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()
