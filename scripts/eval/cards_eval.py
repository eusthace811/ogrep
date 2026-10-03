#!/usr/bin/env python3
"""Evaluation for `--cards`: at the same token budget, does the output show the function a change was in?

Tasks: the localization sets (`localize.py`), queried with `--about` on the commit subject. Ground truth:
the names of the functions, methods and classes the commit changed, read from its diff's hunk headers
with git's language-aware function patterns (`diff=python`, `diff=rust`, `diff=cpp`, ...); tasks whose
hunks name no function are skipped.

A task succeeds when one of those names appears in the part of the output that belongs to a file the
commit changed, within the first TOKENS tokens of the output (4 characters a token), so every variant
gets the same budget:

  lines3   --about TEXT              (3 matching lines per file)
  lines8   --about TEXT --per-file 8 (more matching lines per file)
  cards    --about TEXT --cards=TOKENS

Usage:
  cards_eval.py CORPUS DATA.json --bin BIN [--index IDX] [--tokens 1000] [--n 300]
"""

import argparse
import json
import re
import subprocess
import tempfile
from pathlib import Path

from localize import description

ATTRS = "*.py diff=python\n*.rs diff=rust\n*.c diff=cpp\n*.cc diff=cpp\n*.cpp diff=cpp\n*.h diff=cpp\n" \
        "*.hpp diff=cpp\n*.go diff=golang\n*.java diff=java\n*.js diff=javascript\n*.ts diff=javascript\n" \
        "*.rb diff=ruby\n*.php diff=php\n*.cs diff=csharp\n"
NAME = re.compile(r"\b(?:def|class|fn|struct|enum|trait|impl|func|function|interface|module|mod)\s+(?:<[^>]*>\s*)?([A-Za-z_]\w*)")
CNAME = re.compile(r"([A-Za-z_]\w*)\s*\(")


def changed_names(root, sha, files, attrs):
    out = subprocess.run(["git", "-c", f"core.attributesFile={attrs}", "show", "-U0", "--format=", sha, "--", *files],
                         cwd=root, capture_output=True, text=True, errors="replace").stdout
    names = set()
    for line in out.splitlines():
        if not line.startswith("@@"):
            continue
        header = line.split("@@", 2)[-1].strip()
        if not header:
            continue
        m = NAME.search(header)
        if m:
            names.add(m.group(1))
        else:
            c = CNAME.findall(header)
            if c:
                names.add(c[-1] if c[-1] not in ("if", "for", "while", "switch", "return") else c[0])
    return {n for n in names if len(n) >= 3}


def blocks(text, cards):
    """Output text -> [(path, chunk text)] in order, each chunk a contiguous piece of one file's output."""
    out = []
    for line in text.splitlines(keepends=True):
        if cards:
            if line and not line[0].isspace() and not line.startswith("#"):
                out.append([line.split(" — ")[0].strip(), line])
            elif out:
                out[-1][1] += line
        else:
            path, sep, _ = line.partition(":")
            if not sep:
                continue
            if out and out[-1][0] == path:
                out[-1][1] += line
            else:
                out.append([path, line])
    return out


def success(text, cards, relevant, names, budget_chars):
    used = 0
    for path, chunk in blocks(text, cards):
        chunk = chunk[:max(0, budget_chars - used)]
        used += len(chunk)
        if path in relevant and any(re.search(rf"\b{re.escape(n)}\b", chunk) for n in names):
            return True
        if used >= budget_chars:
            break
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("corpus")
    ap.add_argument("data")
    ap.add_argument("--bin", required=True)
    ap.add_argument("--index")
    ap.add_argument("--tokens", type=int, default=1000)
    ap.add_argument("--n", type=int, default=300)
    args = ap.parse_args()
    root = Path(args.corpus).resolve()
    base = [str(Path(args.bin).resolve())] + (["--index-path", str(Path(args.index).resolve())] if args.index else [])
    items = json.loads(Path(args.data).read_text())[:args.n]
    with tempfile.NamedTemporaryFile("w", suffix=".attrs", delete=False) as fh:
        fh.write(ATTRS)
    variants = {
        "lines3": ([], False),
        "lines8": (["--per-file", "8"], False),
        "cards": ([f"--cards={args.tokens}"], True),
    }
    wins = {v: 0 for v in variants}
    used = 0
    for it in items:
        names = changed_names(root, it["commit"], it["relevant"], fh.name)
        if not names:
            continue
        used += 1
        for v, (extra, cards) in variants.items():
            text = subprocess.run(base + ["--about", description(it["subject"]), "--budget", "40", *extra], cwd=root,
                                  capture_output=True, text=True, errors="replace").stdout
            wins[v] += success(text, cards, set(it["relevant"]), names, args.tokens * 4)
    Path(fh.name).unlink()
    print(f"{root.name}: {used} tasks with named functions, budget {args.tokens} tokens")
    for v, w in wins.items():
        print(f"  {v:<7} changed function shown for a changed file: {w / max(used, 1):.3f}")


if __name__ == "__main__":
    main()
