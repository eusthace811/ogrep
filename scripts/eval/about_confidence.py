#!/usr/bin/env python3
"""Evaluation for `--about`'s confidence: does `unsure` mark the queries whose first file is wrong?

Tasks: the localization sets (`localize.py`), queried with `--about` on the commit subject. The first
`rank` record of `--json` output carries `confidence` (`level`, `margin`). Reported per level: the share
of queries, Acc@1 and Acc@5 (a changed file first, or in the top five), and the AUROC of the margin for
"the first file is right" (0.5 is chance).

Usage:
  about_confidence.py CORPUS DATA.json --bin BIN [--index IDX]
"""

import argparse
import json
import subprocess
from pathlib import Path

from localize import description


def auroc(scores, labels):
    pos = [s for s, y in zip(scores, labels) if y]
    neg = [s for s, y in zip(scores, labels) if not y]
    if not pos or not neg:
        return float("nan")
    return sum((p > n) + 0.5 * (p == n) for p in pos for n in neg) / (len(pos) * len(neg))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("corpus")
    ap.add_argument("data")
    ap.add_argument("--bin", required=True)
    ap.add_argument("--index")
    args = ap.parse_args()
    root = Path(args.corpus).resolve()
    base = [str(Path(args.bin).resolve())] + (["--index-path", str(Path(args.index).resolve())] if args.index else [])
    items = json.loads(Path(args.data).read_text())
    rows = []
    for it in items:
        out = subprocess.run(base + ["--about", description(it["subject"]), "--json", "--budget", "5", "--per-file",
                                     "1"], cwd=root, capture_output=True, text=True, errors="replace").stdout
        ranks = [json.loads(line)["data"] for line in out.splitlines() if '"type":"rank"' in line]
        if not ranks:
            rows.append(("none", 0.0, False, False))
            continue
        paths = [r["path"]["text"] for r in ranks]
        relevant = set(it["relevant"])
        c = ranks[0]["confidence"]
        rows.append((c["level"], c["margin"], paths[0] in relevant, bool(relevant & set(paths))))
    print(f"{root.name}: {len(rows)} tasks, AUROC of the margin "
          f"{auroc([r[1] for r in rows], [r[2] for r in rows]):.3f}")
    for level in ("confident", "unsure", "none"):
        group = [r for r in rows if r[0] == level]
        if group:
            print(f"  {level:<9} {len(group) / len(rows):5.1%} of tasks, Acc@1 {sum(r[2] for r in group) / len(group):.3f}"
                  f", Acc@5 {sum(r[3] for r in group) / len(group):.3f}")


if __name__ == "__main__":
    main()
