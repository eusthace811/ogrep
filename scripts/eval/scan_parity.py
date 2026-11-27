#!/usr/bin/env python3
"""Indexed results against a full scan, for two builds side by side.

Part A: queries on CORPUS, through the local index and through a server. The
truth for each is the same query with `--no-index`, run by the new build.
Part B: small trees in layouts that test index and server lookup: moved,
copied beside or inside the original, searched from a subdirectory, and with a
server whose tree moved or was copied.

Each row prints ok or DIFF/WRONG for both builds, so a row wrong for both is
a known gap and a row wrong only for the new build is a regression. Exit 1 if
the new build is wrong anywhere.

CORPUS must hold an index built by a compatible binary (`ogrep index .`);
the script removes a stale `.ogrep/serve.json` there before starting servers.

Usage:
  scan_parity.py CORPUS --base BIN --new BIN [--work DIR]
"""
import argparse, os, shutil, subprocess, sys, tempfile, time

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument("corpus")
parser.add_argument("--base", required=True, help="baseline ogrep binary")
parser.add_argument("--new", required=True, help="ogrep binary under test")
parser.add_argument("--work", help="directory for logs and layout trees (default: a new temporary one)")
cli = parser.parse_args()
corpus = os.path.abspath(cli.corpus)
S = os.path.abspath(cli.work) if cli.work else tempfile.mkdtemp(prefix="scan-parity-")
os.makedirs(S, exist_ok=True)
BIN = {"base": os.path.abspath(cli.base), "new": os.path.abspath(cli.new)}


def run(binary, args, cwd, timeout=300):
    r = subprocess.run([binary, *args], cwd=cwd, capture_output=True, timeout=timeout)
    return r.returncode, r.stdout.decode("utf-8", "replace"), r.stderr.decode("utf-8", "replace")


def norm(out):
    return sorted(l for l in out.splitlines() if l.strip())


QUERIES = [
    ["-F", "impl"], ["-c", "-F", "impl"], ["-l", "-F", "unsafe"], ["-n", "-F", "fn main"],
    ["-i", "-c", "-F", "todo"], ["-w", "-c", "-F", "new"], ["-c", "-e", r"fn \w+_test"],
    ["-l", "-t", "rust", "-F", "async fn"], ["-l", "-g", "*.toml", "-F", "version"],
    ["-c", "-g", "!**/tests/**", "-F", "assert_eq"], ["-n", "-C2", "-F", "unreachable!(\"internal"],
    ["-n", "-m", "1", "-F", "use std::sync"], ["-l", "--hidden", "-F", "workflow"],
    ["-c", "-P", "(?<=fn )main"], ["-l", "--max-filesize", "10K", "-F", "impl"],
    ["-c", "-s", "-F", "Impl"], ["-l", "-F", "zzz_no_such_token_qq"], ["--files"],
    ["--files", "-t", "rust"], ["-c", "-F", "impl", "aho-corasick-1.1.4"],
    ["-l", "-F", "unsafe", "anes-0.1.6"],
]


def with_path(q):
    return q if len(q) and os.path.isdir(os.path.join(corpus, q[-1])) and q[-1] != "." else q + ["."]


def part_a():
    print("== Part A: corpus", flush=True)
    truth = {}
    for i, q in enumerate(QUERIES):
        truth[i] = run(BIN["new"], ["--no-index", *with_path(q)], corpus)
    results = {}
    for name, b in BIN.items():
        for i, q in enumerate(QUERIES):
            results[(name, "local", i)] = run(b, with_path(q), corpus)
        info = os.path.join(corpus, ".ogrep/serve.json")
        if os.path.exists(info):
            os.remove(info)
        log = open(f"{S}/eval-serve.log", "w")
        p = subprocess.Popen([b, "serve", ".", "--no-watch"], cwd=corpus, stderr=log, stdout=subprocess.DEVNULL)
        try:
            t0 = time.time()
            while True:
                text = open(f"{S}/eval-serve.log").read()
                if "stale check: index is up-to-date" in text or "stale check: streamed" in text:
                    if "Hidden coverage: complete" in run(b, ["status", "."], corpus)[1]:
                        break
                if time.time() - t0 > 300:
                    raise SystemExit("server did not get ready")
                time.sleep(0.05)
            for i, q in enumerate(QUERIES):
                results[(name, "server", i)] = run(b, ["--stats", *with_path(q)], corpus)
        finally:
            p.kill(); p.wait()
    bad = 0
    for i, q in enumerate(QUERIES):
        tcode, tout, _ = truth[i]
        row = []
        for name in BIN:
            for mode in ("local", "server"):
                code, out, err = results[(name, mode, i)]
                ok = code == tcode and norm(out) == norm(tout)
                if mode == "server":
                    out_lines = [l for l in out.splitlines() if not l.startswith(("Search completed", "Filename search"))]
                    ok = code == tcode and norm("\n".join(out_lines)) == norm(tout)
                    via = "via server" in err
                    row.append(f"{name}/{mode}={'ok' if ok else 'DIFF'}{'' if via else '(no-server)'}")
                else:
                    row.append(f"{name}/{mode}={'ok' if ok else 'DIFF'}")
                if name == "new" and not ok:
                    bad += 1
        print(f"  {' '.join(q):45} truth: exit {tcode} {len(norm(tout)):6} lines | " + " ".join(row), flush=True)
    return bad


def tree(root):
    os.makedirs(os.path.join(root, "src"))
    open(os.path.join(root, "src/a.rs"), "w").write("fn foo() { bar() }\nfn bar() {}\n// needle one\n")
    open(os.path.join(root, "src/b.rs"), "w").write("fn baz() { foo() }\n// needle two\n")
    open(os.path.join(root, "top.txt"), "w").write("needle top\n")


def server(b, root):
    log = open(os.path.join(os.path.dirname(root), "serve.log"), "w")
    p = subprocess.Popen([b, "serve", ".", "--no-watch"], cwd=root, stderr=log, stdout=subprocess.DEVNULL)
    t0 = time.time()
    while "Hidden coverage: complete" not in run(b, ["status", "."], root)[1]:
        if time.time() - t0 > 60:
            raise SystemExit("layout server did not get ready")
        time.sleep(0.05)
    return p


def scenario(name, b):
    """Return list of (label, indexed (code,out,err), truth (code,out)) for one binary."""
    tmp = tempfile.mkdtemp(dir=S, prefix=f"ev-{name}-")
    t = os.path.join(tmp, "t")
    tree(t)
    rows = []
    procs = []

    def check(label, args, cwd, truth_args=None):
        got = run(b, args, cwd)
        truth = run(BIN["new"], ["--no-index", *(truth_args or args)], cwd)
        rows.append((label, got, truth))

    try:
        if name == "plain":
            run(b, ["index", ".", "--graph"], t)
            check("root", ["-n", "needle", "."], t)
            check("subdir", ["--index-path", ".ogrep", "-n", "needle", "src"], t)
            check("files", ["--files", "."], t)
        elif name == "moved":
            run(b, ["index", ".", "--graph"], t)
            # The truth for --callees is the same query before the move.
            before = run(BIN["new"], ["--callees", "-F", "foo", "."], t)
            before_parent = run(BIN["new"], ["--index-path", "t/.ogrep", "--callees", "-F", "foo", "."], tmp)
            assert before[0] == 0 and "bar -> src/a.rs:2" in before[1], before
            m = os.path.join(tmp, "m"); os.rename(t, m)
            open(os.path.join(m, "src/b.rs"), "a").write("// needle added\n")
            check("search", ["-n", "needle", "."], m)
            check("subdir", ["--index-path", ".ogrep", "-n", "needle", "src"], m)
            g = run(b, ["--callees", "-F", "foo", "."], m)
            rows.append(("callees", g, before))
            g = run(b, ["--index-path", "m/.ogrep", "--callees", "-F", "foo", "."], tmp)
            rows.append(("callees-from-parent", g, (before_parent[0], before_parent[1].replace("t/src/", "m/src/"), "")))
        elif name == "copy-beside":
            run(b, ["index", ".", "--graph"], t)
            c = os.path.join(tmp, "c"); shutil.copytree(t, c)
            open(os.path.join(c, "src/a.rs"), "w").write("fn foo() { qux() }\nfn qux() {}\n// needle copied\n")
            check("search", ["-n", "needle", "."], c)
            check("files", ["--files", "."], c)
        elif name == "copy-inside":
            run(b, ["index", ".", "--graph"], t)
            c = os.path.join(t, "backup"); shutil.copytree(t, c, ignore=shutil.ignore_patterns("backup"))
            open(os.path.join(c, "src/a.rs"), "w").write("// needle copied inside\n")
            check("search", ["-n", "needle", "."], c)
            check("files", ["--files", "."], c)
        elif name == "server-moved":
            run(b, ["index", "."], t)
            procs.append(server(b, t))
            m = os.path.join(tmp, "m"); os.rename(t, m)
            check("search", ["-n", "needle", "."], m)
            check("files", ["--files", "."], m)
            check("absent", ["-n", "zz_absent_qq", "."], m)
        elif name == "server-copy":
            run(b, ["index", "."], t)
            procs.append(server(b, t))
            c = os.path.join(tmp, "c"); shutil.copytree(t, c)
            open(os.path.join(c, "src/a.rs"), "w").write("// needle copied\n")
            check("search", ["-n", "needle", "."], c)
            run(b, ["index", "."], c)
            check("search-after-reindex", ["-n", "needle", "."], c)
            check("original", ["-n", "needle", "."], t)
        elif name == "server-plain":
            run(b, ["index", "."], t)
            procs.append(server(b, t))
            check("search", ["-n", "needle", "."], t)
            check("files", ["--files", "."], t)
            code, out, err = run(b, ["-n", "--", "needle(", "."], t)
            rows.append(("bad-regex", (code, out, err), (2, "", "")))
    finally:
        for p in procs:
            p.kill(); p.wait()
    shutil.rmtree(tmp, ignore_errors=True)
    return rows


def part_b():
    print("== Part B: layouts", flush=True)
    bad = 0
    for name in ["plain", "moved", "copy-beside", "copy-inside", "server-moved", "server-copy", "server-plain"]:
        res = {b: scenario(name, path) for b, path in BIN.items()}
        for (label, bgot, truth), (_, fgot, _) in zip(res["base"], res["new"]):
            def ok(got):
                return got[0] == truth[0] and norm(got[1]) == norm(truth[1])
            bo, fo = ok(bgot), ok(fgot)
            if label == "bad-regex":
                fallback = lambda g: "falling back" in g[2]
                note = f"base: exit {bgot[0]} fallback-line={fallback(bgot)}; new: exit {fgot[0]} fallback-line={fallback(fgot)}"
                bo, fo = bgot[0] == 2, fgot[0] == 2 and not fallback(fgot)
            else:
                note = ""
            warn = lambda g: ("warn" if g[2].strip() else "")
            print(f"  {name:13} {label:22} base={'ok' if bo else 'WRONG'}{'/'+warn(bgot) if warn(bgot) else ''} "
                  f"new={'ok' if fo else 'WRONG'}{'/'+warn(fgot) if warn(fgot) else ''} {note}", flush=True)
            if not fo:
                bad += 1
                print(f"      new   got exit {fgot[0]}: {fgot[1]!r} err {fgot[2][:200]!r}; truth {truth[0]}: {truth[1]!r}")
            if not bo:
                print(f"      base  got exit {bgot[0]}: {bgot[1][:200]!r} err {bgot[2][:160]!r}")
    return bad


bad = part_a() + part_b()
print(f"new build wrong: {bad}")
sys.exit(1 if bad else 0)
