#!/usr/bin/env python3
"""Ranking evaluation (E7): find the file a change was about, from one identifier.

Dataset (mined offline from git history, deterministic):
  For each recent non-merge commit that changed 1-4 non-test source files, take
  the most specific identifier in its subject line (snake_case, CamelCase or
  dotted, at least 6 characters). The files the commit changed that still
  contain that identifier at HEAD are the relevant answers. This is the
  localization task a coding agent faces: a name from a bug report or task, and
  the question of where to look.

Variants: each prints the matching files (`-l -F`) in some order; the metric is
where the first relevant file lands.
  plain   ogrep's own order (file-ID order)
  path    --sort=path
  count   most matches first (from -c), ties by path
  rank    --rank --budget 100000
  about   --about SUBJECT --budget 100000: the commit subject itself, minus a
          ticket prefix ("Fixed #123 --", "gh-123:"), as a description

Metrics: Acc@1, Acc@5, Acc@10, MRR, and the median number of files printed.

Usage:
  localize.py build CORPUS --out DATA.json [--n 300] [--since 2023-01-01]
  localize.py run CORPUS DATA.json --bin BIN --index IDX [--variants plain,path,count,rank]
      [--extra "--rank-cutoff=auto"] [--json-out FILE]
"""

import argparse
import json
import re
import statistics
import subprocess
import time
from pathlib import Path

CODE_EXT = {".py", ".rs", ".c", ".h", ".cc", ".cpp", ".go", ".js", ".ts", ".java", ".rb", ".php"}
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*")
STOP = {"Refs", "Fixed", "Fixes", "Added", "Removed", "Changed", "Updated", "Improved",
        "Corrected", "Reverted", "Merge", "Revert", "Cleanup", "Rollup"}


def is_test(path):
    p = path.lower()
    name = p.rsplit("/", 1)[-1]
    return (p.startswith(("test/", "tests/")) or "/test/" in p or "/tests/" in p
            or name.startswith("test_") or "_test." in name or name.endswith("tests.py")
            or "/testing/" in p or p.startswith("lib/test/"))


def specific_identifiers(subject):
    out = []
    for tok in IDENT.findall(subject):
        last = tok.rsplit(".", 1)[-1]
        if last in STOP or len(last) < 6:
            continue
        snake = "_" in last.strip("_")
        camel = re.search(r"[a-z][A-Z]", last) is not None
        if snake or camel or "." in tok:
            out.append(last)
    # Most specific first: longest, then lexicographic for determinism.
    return sorted(set(out), key=lambda t: (-len(t), t))


def build(args):
    root = Path(args.corpus)
    log = subprocess.run(
        ["git", "log", "--no-merges", *([] if args.all_parents else ["--first-parent"]),
         f"--since={args.since}",
         "--format=\x01%H\x02%s", "--name-only"],
        cwd=root, capture_output=True, text=True, errors="replace", check=True).stdout
    head_files = set(subprocess.run(["git", "ls-files"], cwd=root, capture_output=True,
                                    text=True, check=True).stdout.splitlines())
    items = []
    for block in log.split("\x01")[1:]:
        header, _, names = block.partition("\n")
        sha, _, subject = header.partition("\x02")
        files = [f for f in names.split("\n") if f.strip()]
        src = [f for f in files if Path(f).suffix in CODE_EXT and not is_test(f) and f in head_files]
        if not 1 <= len(src) <= 4:
            continue
        for ident in specific_identifiers(subject):
            relevant = []
            for f in src:
                try:
                    if ident.encode() in (root / f).read_bytes():
                        relevant.append(f)
                except OSError:
                    pass
            if relevant:
                items.append({"commit": sha, "subject": subject, "query": ident,
                              "relevant": sorted(relevant)})
                break
        if len(items) >= args.n:
            break
    Path(args.out).write_text(json.dumps(items, indent=1))
    print(f"{len(items)} items -> {args.out}")


TICKET = re.compile(r"^(?:(?:(?:Fixed|Refs|Fixes|Closes)\s+#?\d+(?:\s*,\s*#?\d+)*\s*--|gh-\d+:|bpo-\d+:)\s*)+", re.I)


def description(subject):
    """A commit subject as a task description: without its ticket prefix."""
    return TICKET.sub("", subject).strip()


def files_of(argv, cwd):
    out = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, errors="replace").stdout
    return [line for line in out.splitlines() if line and not line.startswith("#")]


def run(args):
    root = Path(args.corpus).resolve()
    items = json.loads(Path(args.data).read_text())
    base = [str(Path(args.bin).resolve()), "--index-path", str(Path(args.index).resolve())]
    extra = args.extra.split() if args.extra else []

    def order(variant, q, item):
        if variant == "plain":
            return files_of(base + ["-l", "-F", "--", q], root)
        if variant == "path":
            return files_of(base + ["-l", "-F", "--sort=path", "--", q], root)
        if variant == "count":
            rows = files_of(base + ["-c", "-F", "--", q], root)
            counted = []
            for r in rows:
                path, _, n = r.rpartition(":")
                counted.append((-int(n), path))
            return [p for _, p in sorted(counted)]
        if variant == "rank":
            return files_of(base + ["-l", "-F", "--rank", "--budget", "100000", *extra, "--", q], root)
        if variant == "about":
            return files_of(base + ["-l", "--about", description(item["subject"]), "--budget", "100000",
                                    *extra], root)
        raise SystemExit(f"unknown variant {variant}")

    report = {}
    for variant in args.variants.split(","):
        ranks, printed, times = [], [], []
        for item in items:
            t0 = time.perf_counter()
            files = order(variant, item["query"], item)
            times.append(time.perf_counter() - t0)
            printed.append(len(files))
            relevant = set(item["relevant"])
            pos = next((i + 1 for i, f in enumerate(files) if f in relevant), None)
            ranks.append(pos)
        n = len(ranks)
        acc = lambda k: sum(1 for r in ranks if r and r <= k) / n
        mrr = sum(1 / r for r in ranks if r) / n
        report[variant] = {"n": n, "acc@1": acc(1), "acc@5": acc(5), "acc@10": acc(10),
                           "mrr": mrr, "found": sum(1 for r in ranks if r) / n,
                           "median_files": statistics.median(printed),
                           "p50_ms": round(statistics.median(times) * 1000, 1),
                           "p95_ms": round(sorted(times)[int(0.95 * (n - 1))] * 1000, 1)}
        r = report[variant]
        print(f"{variant:>6}: n={n} acc@1 {r['acc@1']:.3f} acc@5 {r['acc@5']:.3f} "
              f"acc@10 {r['acc@10']:.3f} MRR {r['mrr']:.3f} found {r['found']:.3f} "
              f"median files {r['median_files']} p50 {r['p50_ms']} ms p95 {r['p95_ms']} ms")
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(report, indent=1))


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("corpus")
    b.add_argument("--out", required=True)
    b.add_argument("--n", type=int, default=300)
    b.add_argument("--since", default="2023-01-01")
    b.add_argument("--all-parents", action="store_true",
                   help="follow merged branches too (repos whose first-parent history is only merges)")
    r = sub.add_parser("run")
    r.add_argument("corpus")
    r.add_argument("data")
    r.add_argument("--bin", required=True)
    r.add_argument("--index", required=True)
    r.add_argument("--variants", default="plain,path,count,rank")
    r.add_argument("--extra", default="")
    r.add_argument("--json-out")
    args = ap.parse_args()
    build(args) if args.cmd == "build" else run(args)


if __name__ == "__main__":
    main()
