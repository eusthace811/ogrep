# lab/results

Raw output of `scripts/evaluate.py` (zgrep's harness, unmodified). Each
file holds samples, validation and per-query summaries; `lab/summarize.py
<dir>` prints the ratios. Built binaries (`lab/bin`) and indexes
(`lab/work`) are not kept.

## Six-workload runs (tgrep 1.0.10, zgrep 0.3.0, ogrep; 5 rounds)

| Run | ogrep build | Notes |
|---|---|---|
| r1 | early candidate | before keeping small server candidate sets sequential |
| r2 | `ogrep-r4` (b25afa8) | first release candidate |
| r3 | after the review rounds (9a76728) | server path 12-19% slower than r2: the regressions below |
| r4 | d3af425 | UTF-8 re-validation fixed; random ~40 ms server stalls remain |
| r5 | 1f01128 | single-write RPC lines with TCP_NODELAY: no stalls |
| r6 | 0e35817 | final: the qualified build (copied to `evidence/ogrep-0.1.0/evaluation`) |

## Diagnostics

| File | Question it answered |
|---|---|
| `e2-broad-local.json` | an early experiment on the broad local workload |
| `diag-rust-targeted-server.json` | reproduce r3's server slowdown: r4 build vs final vs final without `check_filters` |
| `bisect1.json`, `bisect2.json` | bisect that slowdown over eight builds: the jump is at ddd04cd |
| `diag2.json`, `diag3.json` | after the files.bin fix (1.032x vs r2) and after scoping `check_filters` (0.978x) |
| `diag-broad.json` | whether true LRU (00d16c1) caused r4's outlier on `upstream-064`: no |
| `diag-3eng-1.json`, `diag-3eng-2.json` | the outlier only with three engines' servers running: random ~40 ms stalls |
| `diag-nd-1.json`, `diag-nd-2.json` | same runs with single-write lines and TCP_NODELAY: no stalls |
| `diag-moved.json` | moving `plan_alternatives` to ogrep-cli left search speed unchanged (1.010x) |
