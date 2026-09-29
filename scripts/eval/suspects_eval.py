#!/usr/bin/env python3
"""Injected-incident evaluation for `ogrep --suspects`.

Each scenario, in a scratch clone of an operations repository (the
OpenTelemetry demo):
  1. picks a root-cause service that other services depend on and whose code
     lives in its own directory;
  2. commits a change to it (the culprit), plus two noise commits to other
     services, some of them failing ones;
  3. names 1-3 services that depend on the root cause as failing (and the root
     cause itself half the time, as when its own health check also fails);
  4. asks `ogrep --suspects FAILING --since 1h` for the ranking and records
     where the root cause lands.

Baselines: "coverage" ignores changes (the most-shared dependency wins) and
"latest change" picks the most recently changed candidate.

Usage: suspects_eval.py --bin BIN --repo DIR [--n 30] [--seed 3]
"""

import argparse
import json
import random
import shutil
import subprocess
import tempfile
import time
from pathlib import Path


def git(repo, *args, env=None):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True, env=env, check=True).stdout


def commit(repo, path, message, when):
    p = Path(repo) / path
    p.parent.mkdir(parents=True, exist_ok=True)
    with open(p, "a") as f:
        f.write(f"\n// injected {message} {when}\n")
    env = {"GIT_AUTHOR_DATE": f"@{when}", "GIT_COMMITTER_DATE": f"@{when}", "PATH": "/usr/bin:/bin:/usr/local/bin",
           "GIT_AUTHOR_NAME": "eval", "GIT_AUTHOR_EMAIL": "e@x", "GIT_COMMITTER_NAME": "eval", "GIT_COMMITTER_EMAIL": "e@x",
           "HOME": "/tmp"}
    git(repo, "add", path)
    git(repo, "commit", "-q", "-m", message, env=env)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--repo", required=True)
    ap.add_argument("--n", type=int, default=30)
    ap.add_argument("--seed", type=int, default=3)
    args = ap.parse_args()
    binary = str(Path(args.bin).resolve())
    rng = random.Random(args.seed)
    scratch = tempfile.mkdtemp(prefix="suspects-eval-")
    repo = str(Path(scratch) / "repo")
    subprocess.run(["git", "clone", "-q", "--local", str(Path(args.repo).resolve()), repo], check=True)
    base = git(repo, "rev-parse", "HEAD").strip()

    def run(*extra):
        out = subprocess.run([binary, *extra], cwd=repo, capture_output=True, text=True)
        return [json.loads(l) for l in out.stdout.splitlines() if l.strip()]

    entities = {e["data"]["entity"]["id"]: e["data"] for e in run("--entities", ".", "--json")}
    services = sorted(entities)
    blast = {s: [b["data"] for b in run("--blast", s, "--json")] for s in services}
    # Code directory per service: src/<name> when it exists.
    code = {s: f"src/{s}" for s in services if (Path(repo) / "src" / s).is_dir()}
    roots = [s for s in services if s in code and blast[s]]
    results = {"ogrep": [], "coverage": [], "latest": []}
    now = int(time.time())
    for i in range(args.n):
        git(repo, "reset", "-q", "--hard", base)
        root = rng.choice(roots)
        dependents = [b["service"] for b in blast[root] if b["depth"] <= 2]
        failing = rng.sample(dependents, k=min(len(dependents), rng.randint(1, 3)))
        if rng.random() < 0.5:
            failing.append(root)
        others = [s for s in code if s != root]
        noise = rng.sample(others, k=2)
        t0 = now - rng.randint(600, 3000)
        commit(repo, f"{code[root]}/INJECTED_CHANGE.txt", f"change {root}", t0)
        for j, n in enumerate(noise):
            commit(repo, f"{code[n]}/NOISE_{j}.txt", f"tweak {n}", t0 + 60 * (j + 1))
        ranked = [s["data"]["suspect"] for s in run("--suspects", ",".join(failing), "--since", "2 hours ago", "--json")]
        order = [s["service"] for s in ranked]
        pos = order.index(root) + 1 if root in order else None
        results["ogrep"].append(pos)
        cov = sorted(ranked, key=lambda s: (-len(s["explains"]), s["distance"], s["service"]))
        results["coverage"].append([s["service"] for s in cov].index(root) + 1 if root in order else None)
        latest = sorted(ranked, key=lambda s: (-(max((c["time"] for c in s["changes"]), default=0)), s["service"]))
        results["latest"].append([s["service"] for s in latest].index(root) + 1 if root in order else None)
        print(f"{i:2} root={root:<18} failing={','.join(failing):<40} rank={pos} top={order[:3]}")
    for name, ranks in results.items():
        n = len(ranks)
        acc = lambda k: sum(1 for r in ranks if r and r <= k) / n
        mrr = sum(1 / r for r in ranks if r) / n
        print(f"{name:>9}: acc@1 {acc(1):.3f} acc@3 {acc(3):.3f} MRR {mrr:.3f}")
    shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    main()
