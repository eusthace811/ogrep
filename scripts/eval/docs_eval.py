#!/usr/bin/env python3
"""Documentation localization (round 9): find the docs file and section a change was about, from its description.

Dataset (mined offline from git history, deterministic):
  Non-merge first-parent commits that changed 1-2 documentation files (under DOCS, with the given extensions,
  outside the excluded paths). For each changed file, the first added line of 25+ characters that occurs exactly
  once in the file at HEAD locates the answer: the deepest heading above that line. The query is the commit subject
  without its ticket prefix, kept when it has 4+ words.

Run: `ogrep --about QUERY --json --budget 40 [-g !EXCLUDE ...] DOCS`; the files are ranked records, and a file's
section is its `scope` record (printed for prose files since round 9; older binaries print none, and the section of
the first shown line is used instead).

Metrics: file Acc@1, Acc@5 and MRR; section Acc@1 and Acc@3 (a hit is a printed section whose heading line is the
answer's), over the files in rank order.

Usage:
  docs_eval.py build CORPUS --docs docs --ext .txt --exclude docs/releases/ --out DATA.json [--n 300] [--since 2022-01-01]
  docs_eval.py run CORPUS DATA.json --bin BIN --index IDX --docs docs [--exclude releases/] [--json-out FILE]
"""

import argparse
import json
import re
import subprocess
import time
from pathlib import Path

from localize import description

ADORN = "=-~^*+#\"'`"


def headings(lines, path=""):
    """Heading line numbers (1-based), as ogrep's cards find them, outside ``` fences (and ~~~ in Markdown):
    Markdown `#` headings (not in .txt files, where `#` starts a comment), AsciiDoc `=` headings, and titles
    underlined by one repeated punctuation character, at least as long and 3+ characters."""
    out, fence = [], None
    prefix = None if path.lower().endswith(".txt") else "=" if path.lower().endswith(".adoc") else "#"
    # ~~~ also opens a Markdown fence; elsewhere it is a title underline.
    fences = ("```", "~~~") if path.lower().endswith((".md", ".markdown")) else ("```",)
    for i, line in enumerate(lines):
        t = line.rstrip()
        marker = next((m for m in fences if t.lstrip().startswith(m)), None)
        if marker:
            fence = marker if fence is None else (None if fence == marker else fence)
            continue
        if fence:
            continue
        level = len(t) - len(t.lstrip(prefix)) if prefix else 0
        if 1 <= level <= 6 and t[level:].startswith(" "):
            out.append(i + 1)
            continue
        if i + 1 < len(lines):
            n = lines[i + 1].rstrip()
            if (t.strip() and not t.startswith(" ") and n and n[0] in ADORN and len(n) >= len(t) and len(n) >= 3
                    and set(n) == {n[0]}):
                out.append(i + 1)
    return out


def build(args):
    root = Path(args.corpus)
    exts = tuple(args.ext.split(","))
    excludes = tuple(e for e in args.exclude.split(",") if e)
    log = subprocess.run(["git", "log", "--no-merges", "--first-parent", f"--since={args.since}",
                          "--format=\x01%H\x02%s", "--name-only", "--", args.docs], cwd=root, capture_output=True,
                         text=True, errors="replace", check=True).stdout
    items = []
    for block in log.split("\x01")[1:]:
        header, _, names = block.partition("\n")
        sha, _, subject = header.partition("\x02")
        files = [f for f in names.split("\n") if f.strip() and f.endswith(exts) and not f.startswith(excludes)]
        if not 1 <= len(files) <= 2:
            continue
        answers = []
        for f in files:
            p = root / f
            if not p.exists():
                continue
            lines = p.read_text(errors="replace").split("\n")
            diff = subprocess.run(["git", "show", "--format=", "-U0", sha, "--", f], cwd=root, capture_output=True,
                                  text=True, errors="replace").stdout
            heads = headings(lines, f)
            for a in (l[1:] for l in diff.split("\n") if l.startswith("+") and not l.startswith("+++")):
                if len(a.strip()) < 25 or lines.count(a) != 1:
                    continue
                at = lines.index(a) + 1
                above = [h for h in heads if h <= at]
                if above:
                    answers.append({"file": f, "line": at, "section": above[-1]})
                break
        q = description(subject)
        if answers and len(q.split()) >= 4:
            items.append({"commit": sha, "subject": subject, "query": q, "answers": answers})
        if len(items) >= args.n:
            break
    Path(args.out).write_text(json.dumps(items, indent=1))
    print(f"{len(items)} items -> {args.out}")


def run(args):
    root = Path(args.corpus).resolve()
    items = json.loads(Path(args.data).read_text())
    base = [str(Path(args.bin).resolve()), "--index-path", str(Path(args.index).resolve())]
    globs = [x for e in args.exclude.split(",") if e for x in ("-g", f"!{e}**")]
    franks, sranks, times = [], [], []
    for it in items:
        t0 = time.perf_counter()
        out = subprocess.run(base + ["--about", it["query"], "--json", "--budget", "40", *args.extra.split(), *globs, *([] if args.docs == "." else [args.docs])], cwd=root,
                             capture_output=True, text=True, errors="replace").stdout
        times.append(time.perf_counter() - t0)
        files, sections = [], []
        current, scoped = None, False
        for line in out.splitlines():
            r = json.loads(line)
            if r["type"] == "rank":
                current, scoped = r["data"]["path"]["text"], False
                files.append(current)
            elif r["type"] == "scope" and current:
                sections.append((current, r["data"]["line_number"]))
                scoped = True
            elif r["type"] == "match" and current and not scoped:
                # No scope record (an older binary): the section of the first shown line.
                p = root / current
                try:
                    lines = p.read_text(errors="replace").split("\n")
                except OSError:
                    lines = []
                above = [h for h in headings(lines, current) if h <= r["data"]["line_number"]]
                if above:
                    sections.append((current, above[-1]))
                scoped = True
        gold_files = {a["file"] for a in it["answers"]}
        gold_secs = {(a["file"], a["section"]) for a in it["answers"]}
        franks.append(next((i + 1 for i, f in enumerate(files) if f in gold_files), None))
        uniq = list(dict.fromkeys(sections))
        sranks.append(next((i + 1 for i, s in enumerate(uniq) if s in gold_secs), None))
    n = len(items)
    acc = lambda ranks, k: sum(1 for r in ranks if r and r <= k) / n
    report = {"n": n, "file_acc@1": acc(franks, 1), "file_acc@5": acc(franks, 5),
              "file_mrr": sum(1 / r for r in franks if r) / n, "section_acc@1": acc(sranks, 1),
              "section_acc@3": acc(sranks, 3), "p50_ms": round(sorted(times)[n // 2] * 1000, 1),
              "p95_ms": round(sorted(times)[int(0.95 * (n - 1))] * 1000, 1)}
    print(" ".join(f"{k} {v:.3f}" if isinstance(v, float) else f"{k} {v}" for k, v in report.items()))
    if args.json_out:
        Path(args.json_out).write_text(json.dumps({"report": report, "file_ranks": franks, "section_ranks": sranks},
                                                  indent=1))


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("corpus")
    b.add_argument("--docs", required=True)
    b.add_argument("--ext", default=".txt,.rst,.md")
    b.add_argument("--exclude", default="")
    b.add_argument("--out", required=True)
    b.add_argument("--n", type=int, default=300)
    b.add_argument("--since", default="2022-01-01")
    r = sub.add_parser("run")
    r.add_argument("corpus")
    r.add_argument("data")
    r.add_argument("--bin", required=True)
    r.add_argument("--index", required=True)
    r.add_argument("--docs", required=True)
    r.add_argument("--exclude", default="", help="comma-separated paths relative to --docs, skipped with -g")
    r.add_argument("--extra", default="", help="more ogrep arguments, space-separated (e.g. -g *.txt)")
    r.add_argument("--json-out")
    args = ap.parse_args()
    build(args) if args.cmd == "build" else run(args)


if __name__ == "__main__":
    main()
