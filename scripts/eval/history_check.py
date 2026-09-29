#!/usr/bin/env python3
"""History gate: `ogrep --history` must find exactly the commits `git log -G` finds.

Patterns are identifiers sampled from a targeted query set (no regex
metacharacters, so git's and ogrep's regex dialects agree). For each, the full
commit ids from `ogrep --history --json` are compared with
`git log --no-merges -G PATTERN`, restricted to the commits the history index
covers. Timings of both are reported.

Usage: history_check.py --bin BIN --index IDX --corpus DIR --queries JSON [--n 40]
"""

import argparse
import concurrent.futures as cf
import json
import re
import statistics
import subprocess
import time
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--index", required=True)
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--queries", required=True)
    ap.add_argument("--n", type=int, default=40)
    ap.add_argument("--jobs", type=int, default=8)
    args = ap.parse_args()
    root = str(Path(args.corpus).resolve())
    binary = str(Path(args.bin).resolve())
    index = str(Path(args.index).resolve())
    queries = [q for q in json.loads(Path(args.queries).read_text())["queries"]
               if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{5,}", q)][: args.n]
    meta = json.loads((Path(index) / "history" / "history.json").read_text())
    listing = subprocess.run(
        ["git", "log", "--no-merges", "--format=%H %ct", *([f"--max-count={meta['depth']}"] if meta["depth"] else []), meta["head"]],
        cwd=root, capture_output=True, text=True, check=True).stdout.split("\n")
    rows = [l.split() for l in listing if l.strip()]
    # A shallow clone's boundary commits are not indexed (they diff the whole
    # tree against nothing); leave them out of git's answer too.
    shallow = Path(root) / ".git" / "shallow"
    boundary = set(shallow.read_text().split()) if shallow.exists() else set()
    covered = {sha for sha, _ in rows if sha not in boundary}
    # Bound git's walk to the covered period (plus a day of slack); results are
    # still restricted to exactly the covered commits.
    since = [f"--since={min(int(t) for _, t in rows) - 86400}"] if meta["depth"] else []

    def check(q):
        t0 = time.perf_counter()
        out = subprocess.run([binary, "--index-path", index, "--history", "--json", "--", q],
                             cwd=root, capture_output=True, text=True).stdout
        t1 = time.perf_counter()
        ours = {json.loads(l)["data"]["sha"] for l in out.splitlines() if l.strip()}
        git = subprocess.run(["git", "log", "--no-merges", *since, "-G", q, "--format=%H", meta["head"]],
                             cwd=root, capture_output=True, text=True).stdout.split()
        t2 = time.perf_counter()
        theirs = {s for s in git if s in covered}
        return q, ours, theirs, (t1 - t0) * 1000, (t2 - t1) * 1000

    bad, ogrep_ms, git_ms = [], [], []
    with cf.ThreadPoolExecutor(args.jobs) as pool:
        for q, ours, theirs, to, tg in pool.map(check, queries):
            ogrep_ms.append(to)
            git_ms.append(tg)
            if ours != theirs:
                bad.append((q, sorted(theirs - ours)[:3], sorted(ours - theirs)[:3]))
    print(f"{len(queries)} patterns, {len(bad)} mismatches; median ogrep {statistics.median(ogrep_ms):.0f} ms, "
          f"median git log -G {statistics.median(git_ms):.0f} ms (git under {args.jobs}x parallel load)")
    for q, missing, extra in bad:
        print(f"  {q!r}: missing {missing} extra {extra}")
    raise SystemExit(1 if bad else 0)


if __name__ == "__main__":
    main()
