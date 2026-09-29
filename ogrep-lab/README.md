# ogrep-lab

A local workbench for an ogrep index. It serves a page on your own machine where
you can:

- see what the index holds;
- see how a query is planned and executed;
- search with every lens ogrep has;
- move through files and the file graph;
- read a pattern's history;
- explore services and incidents;
- run evaluations and compare them over time.

```sh
ogrep index . --history --graph    # the lab uses whatever has been built
ogrep-lab .                        # prints a URL and opens it
```

It is a separate binary so that `ogrep` pays nothing for it: no larger
executable and no extra start-up work on each search. Both are built and
installed together by `make install` (or `./x install` on Windows).

## Offline by design

- No network access. The page is compiled into the binary, and it uses no CDN,
  web fonts or third-party scripts; the charts and the graph are drawn by hand.
- No dependencies beyond those ogrep already has. The HTTP server is a small
  loopback-only server written against the standard library.
- It listens on `127.0.0.1` only. Access needs the random token in the printed
  URL, which the server trades for a `SameSite=Strict`, `HttpOnly` cookie.
  Requests whose `Host` header is not the loopback address are refused, which
  blocks DNS rebinding.
- It only reads the index and the files the index lists. The one thing it
  writes is saved evaluation runs, under `<index>/lab/runs/`.

## Views

| View | What it shows | Needs |
|---|---|---|
| Index | Counts, bytes per part of the index, what is built and whether it is current, file types and directories | the index |
| Search | Matches, `--rank` scores with the features behind them, `--scope` chains; the same pattern as related files, map, callers or callees; the **query plan** next to it | `ogrep` |
| Files | The indexed tree, a file with its outline, the files it uses and is used by, with the names that link them | the index; the graph for outline and links |
| Graph | The neighbourhood of a file, the most central files, a definition finder | `ogrep index --graph` |
| History | When a pattern was first added and last removed, matching lines per month, every commit with its matching lines and diff | `ogrep index --history` |
| Ops | Services and dependencies from Compose, Kubernetes, Backstage and Prometheus files, blast radius, incident suspects, recent changes | operations files in the repository |
| Evals | Runs the suites below, follows progress, charts runs over time, compares two | `ogrep`; git |

`Ctrl K` opens a palette to go to a file, or to a definition with `@name`. `/`
focuses the current view's input. `1` to `7` switch views.

### The query plan

The plan panel runs ogrep-core's own planner against the index on disk. A
recording posting source logs every list the planner loads and every probe it
makes, so the trace shows the real execution order rather than a model of it.

The funnel runs from indexed files to candidates to verified matches. Each key
is shown with its posting count. Keys the planner skipped are faded, and sparse
grams that were pruned as too common are marked.

A running `ogrep serve` also searches unsaved changes from its overlay; this
panel covers the on-disk index only.

## Evaluations

The suites run against any repository, with queries drawn from that repository,
so the numbers describe your codebase rather than a benchmark's.

- **Ranking: localization.** Commits are mined from git history. For each commit
  that changed one to four source files and names a specific identifier in its
  subject, the question is where the first changed file appears when searching
  for that identifier. The measures are Acc@1, Acc@5, Acc@10 and MRR, for
  ogrep's plain order, `--sort=path`, most matches first, and `--rank`. This is
  `scripts/eval/localize.py`, in-process.
- **Latency.** The wall time of `ogrep` invocations on sampled identifiers,
  process start included. Each query runs several rounds, with the variants
  shuffled by a seeded generator inside each round, as in
  `scripts/freeze/bench.py`. The result is p50, p95, mean and max of the
  per-query medians.
- **History parity.** `ogrep --history` must find exactly the commits
  `git log -G` finds within the commits the history index covers. The suite
  also times both. This is `scripts/eval/history_check.py`.

Each run is saved with the ogrep binary and version, the repository HEAD and the
index it ran against. To compare two builds of ogrep, run a suite once with each
(under "Another ogrep binary"), then tick both runs and compare them.

## Options

```
ogrep-lab [PATH] [--index-path DIR] [--port N] [--no-browser] [--ogrep BIN]
```

- `PATH` is the repository; its index is the nearest `.ogrep` at or above it.
  `--index-path` names the index directory instead.
- `--port 0`, the default, picks a free port.
- `--ogrep` chooses the binary for searches and evaluations. The default is the
  `ogrep` next to `ogrep-lab`, else the first one on `PATH`.

## Working on it

The web client is plain HTML, CSS and JavaScript modules in `web/`, with no
build step. With `OGREP_LAB_WEB_DIR=ogrep-lab/web` set, the server reads those
files from disk on each request, so a browser reload shows an edit without a
rebuild. Only the listed asset paths are ever served.
