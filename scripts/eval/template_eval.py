#!/usr/bin/env python3
"""Evaluation for `--log-templates --template-to-code`: from a production log line to the code that wrote it.

Dataset (mined from the repository itself, deterministic): string literals passed to logging calls, errors
and exceptions in tracked non-test code. Each becomes a log line the way a service would print it: an ISO
timestamp, a level, a logger name (`svc`) and the message with its placeholders (`%s`, `{}`, `${x}`, ...)
filled with realistic values, numeric and not (`42`, `orders-7f9c2`, `/var/lib/app/data.db`, `timeout`).
So the exact literal is never in the line, and the method cannot know which words were filled in. The
answer is any file holding the same literal.

Metric: the share of lines whose reported code location is in a right file, and the median time.

Usage:
  template_eval.py CORPUS --bin BIN [--n 150] [--seed 7]
"""

import argparse
import json
import random
import re
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

EXT = {".py", ".go", ".java", ".js", ".ts", ".rs", ".cs", ".kt", ".rb", ".php", ".ex", ".exs", ".cpp", ".cc", ".h",
       ".scala", ".swift", ".c"}
CALL = re.compile(
    r"""(?:\b(?:log|logger|LOG|LOGGER|_logger|logging|console|slog|zap|self\.logger|self\.log|Log)\w*\s*\.\s*"""
    r"""(?:debug|info|warn|warning|error|exception|critical|fatal|Debug|Info|Warn|Error|Fatal|Printf|Println|"""
    r"""LogInformation|LogWarning|LogError|LogCritical|LogDebug|Errorf|Infof|Warnf)\w*"""
    r"""|\b(?:fmt\.Errorf|errors\.New|panic|raise\s+\w+|throw\s+new\s+\w+|new\s+\w*(?:Error|Exception)|"""
    r"""(?:info|warn|error|debug)!|log\.Printf|log\.Fatalf|LOG_(?:ERR|WRN|INF|DBG)|GGML_LOG_\w+|LLAMA_LOG_\w+|"""
    r"""fprintf\s*\(\s*stderr\s*,))\s*\(?\s*(?:_\(\s*)?[f$]?(["'`])([^"'`\n]{18,160})\1""")
PLACEHOLDER = re.compile(r"%\(\w+\)[sdrfi]|%[-+0-9.]*(?:l{0,2}|z)[sdrfivqwxTu]|\{[^{}]{0,40}\}|\$\{[^}]*\}|#\{[^}]*\}")
VALUES = ["42", "orders-7f9c2", "/var/lib/app/data.db", "1.5", "user@example.com", "203.0.113.7", "SKU-0042",
          "timeout", "3", "2026-10-03"]


def tracked(root):
    names = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, text=True, check=True).stdout
    out = []
    for f in names.split("\n"):
        p = f.lower()
        if not f or Path(f).suffix not in EXT:
            continue
        if "/test" in p or p.startswith("test") or "_test." in p or "/vendor/" in p or "node_modules" in p:
            continue
        out.append(f)
    return out


def mine(root, files, n, seed):
    literals = []
    for f in files:
        try:
            text = (Path(root) / f).read_text(errors="replace")
        except OSError:
            continue
        for m in CALL.finditer(text):
            msg = m.group(2).replace("\\n", "").strip()
            if len(re.sub(r"\W", "", PLACEHOLDER.sub("", msg))) >= 12:
                literals.append((f, msg))
    seen, unique = set(), []
    for f, msg in literals:
        if msg not in seen:
            seen.add(msg)
            unique.append((f, msg))
    random.Random(seed).shuffle(unique)
    items = []
    for i, (f, msg) in enumerate(unique[:n]):
        k = iter(range(100))
        filled = PLACEHOLDER.sub(lambda _: VALUES[(i + next(k)) % len(VALUES)], msg)
        level = ["ERROR", "WARN", "INFO"][i % 3]
        items.append({"line": f"2026-10-03T10:{i // 60 % 60:02d}:{i % 60:02d}Z {level} svc {filled}",
                      "relevant": sorted({g for g, m in literals if m == msg})})
    return items


def to_code(binary, root, line):
    with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as fh:
        fh.write(line + "\n")
    out = subprocess.run([binary, "--log-templates", "--template-to-code", "--json", ".", fh.name], cwd=root,
                         capture_output=True, text=True, errors="replace").stdout
    Path(fh.name).unlink()
    for row in out.splitlines():
        d = json.loads(row)
        if d.get("type") == "template" and d["data"].get("code"):
            return d["data"]["code"][0]
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("corpus")
    ap.add_argument("--bin", required=True)
    ap.add_argument("--n", type=int, default=150)
    ap.add_argument("--seed", type=int, default=7)
    args = ap.parse_args()
    root = Path(args.corpus).resolve()
    binary = str(Path(args.bin).resolve())
    items = mine(root, tracked(root), args.n, args.seed)
    hits, times = 0, []
    for it in items:
        t = time.perf_counter()
        found = to_code(binary, root, it["line"])
        times.append(time.perf_counter() - t)
        hits += found in it["relevant"]
    print(f"{root.name}: {len(items)} log lines, right file {hits / max(len(items), 1):.3f}, "
          f"p50 {statistics.median(times) * 1000:.0f} ms")


if __name__ == "__main__":
    main()
