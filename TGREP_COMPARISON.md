# zgrep versus tgrep — fresh evaluation, 25 September 2026

This report compares **zgrep 0.1.0** against **tgrep 1.0.10**, upstream source `56f8c62a2255d794bcaafca5f6a05dcc2d793ad0`. It uses new timings with a different randomized execution order, not just the earlier development results. Both executable hashes still match the binaries used in the release evaluation; no runtime code was changed for this comparison.

**Verdict:** zgrep has a repeatable, large advantage for the tested nested-alternation queries and corrects four demonstrated Unicode candidate-filtering failures. For the broader upstream-derived workload, local performance is effectively tied (**1.002×**) and server performance changes modestly (**1.019×** geometric-mean speedup). The evidence supports targeted value, not a general order-of-magnitude improvement.

## Workload-level performance

Speedup is tgrep latency divided by zgrep latency; values above 1 favor zgrep. The geometric mean gives each query equal weight. The ratio of summed median latencies approximates issuing every workload query once; it weights expensive queries more heavily. These are separate summaries, not throughput measurements under concurrent load.

| Workload / mode | Queries × rounds | Geometric mean speedup | Ratio of summed medians | Geomean excluding nested-alternation case |
| --- | ---: | ---: | ---: | ---: |
| CPython custom / local | 12 × 15 | 1.107× | 1.123× | 1.015× |
| CPython custom / server | 12 × 15 | 1.088× | 1.085× | 1.026× |
| Rust custom / local | 12 × 15 | 1.264× | 1.324× | 1.021× |
| Rust custom / server | 12 × 15 | 1.293× | 1.188× | 1.079× |
| 102 upstream queries / local | 102 × 11 | 1.002× | 0.999× | n/a |
| 102 upstream queries / server | 102 × 15 | 1.019× | 1.018× | n/a |

The custom suites were designed to cover planner behavior; their aggregates are strongly influenced by the nested-alternation case. The 102-query workload comes from upstream's existing benchmark strings (91 fixed literals, 11 regexes) and is the better check against highlighting only favorable examples. This evaluation newly includes that workload in local mode.

## Largest repeatable gains

CPython pattern: `(PyObject|PyType).*(GetAttr|SetAttr)`.
Rust pattern: `(struct|enum).*(TypeError|RegionError)`.

| Case | tgrep median ms | zgrep median ms | Speedup | Descriptive 95% interval |
| --- | ---: | ---: | ---: | ---: |
| CPython custom / local | 43.21 | 14.92 | 2.90× | 2.85–2.94× |
| CPython custom / server | 10.18 | 4.93 | 2.07× | 1.99–2.21× |
| Rust custom / local | 255.41 | 19.36 | 13.19× | 13.11–13.44× |
| Rust custom / server | 56.49 | 5.96 | 9.48× | 7.76–11.03× |

The previous evaluation measured 2.90× / 1.94× for CPython local/server and 13.47× / 9.97× for Rust. The fresh runs support the same qualitative result. zgrep preserves mandatory alternative subplans that upstream drops when building a concatenation plan. This narrows candidates before exact matching, without an index-format change. These gains apply to the tested patterns; many regexes do not have this planner weakness.

## Slower cases and uncertainty

The tables below retain observed regressions. A slower median is not automatically proof of a stable regression, and an interval containing 1 is not proof of equivalence. Per-query intervals use 5,000 paired-round percentile bootstrap resamples of median ratios, seed 1984, with no multiple-comparison correction. They do not capture uncertainty across machines or workloads.

| Broader workload | Observed median speedup range | Queries with interval wholly above 1 | Wholly below 1 |
| --- | ---: | ---: | ---: |
| 102 upstream queries / local | 0.982–1.049× | 5 / 102 | 4 / 102 |
| 102 upstream queries / server | 0.887–1.212× | 9 / 102 | 0 / 102 |

Five slowest observed query ratios in each broader workload:

| Mode | Pattern | tgrep ms | zgrep ms | Speedup [95% interval] |
| --- | --- | ---: | ---: | ---: |
| local | `XXX` | 17.20 | 17.50 | 0.982 [0.961, 1.021] |
| local | `trait\s+\w+` | 105.63 | 107.37 | 0.984 [0.972, 1.003] |
| local | `Result<` | 49.78 | 50.55 | 0.985 [0.968, 0.991] |
| local | `impl<'a` | 39.77 | 40.38 | 0.985 [0.971, 1.002] |
| local | `derive(Debug` | 39.70 | 40.23 | 0.987 [0.973, 1.010] |
| server | `todo!()` | 7.54 | 8.50 | 0.887 [0.825, 1.021] |
| server | `.iter()` | 17.92 | 19.43 | 0.922 [0.855, 1.092] |
| server | `Rc<RefCell` | 4.44 | 4.78 | 0.931 [0.843, 1.067] |
| server | `impl Display` | 5.04 | 5.40 | 0.932 [0.873, 1.068] |
| server | `#[must_use]` | 6.65 | 7.04 | 0.944 [0.862, 1.123] |

[Every query, median, and interval](evidence/comparison-2026-09-25/query-results.md) is included, along with [raw summary data](evidence/comparison-2026-09-25/summary.json). These results do not establish that every query improves.

## Correctness comparison

All **6,744 timed invocations** reproduced their validated full sorted file lists and exit codes. A separate post-timing ripgrep scan checked all **252 workload/query combinations** against both engines, including local results recorded before server startup reconciliation. All agreed. Corpus Git revisions and clean states, execution-mode diagnostics, sample completeness, and executable hashes were also rechecked. [Independent scan verification](evidence/comparison-2026-09-25/scan-verification.json).

A separate known-positive fixture directly compares indexed `-Fi` searches with scanning. Each query should find exactly its corresponding file:

| File text | Query | tgrep indexed | zgrep indexed | tgrep scan / zgrep scan / rg |
| --- | --- | --- | --- | --- |
| `KELVIN` | `kelvin` | MISSED match | Correct match | All correct |
| `ſHELL` | `shell` | MISSED match | Correct match | All correct |
| `CAFÉ` | `café` | MISSED match | Correct match | All correct |
| `İSTANBUL` | `İstanbul` | MISSED match | Correct match | All correct |
| `ORDINARY_IDENTIFIER` | `ordinary_identifier` | Correct match | Correct match | All correct |

This isolates an inherited index-prefilter bug, rather than a difference in matching semantics: tgrep's scanner finds the matches that its indexed path misses. zgrep's conservative Unicode analysis fixes those cases. The ASCII control checks that the fixture's index path works normally. [Exact results and diagnostics](evidence/comparison-2026-09-25/unicode.json), [reproduction script](scripts/compare-unicode.py).

The existing release validation remains relevant but was not rerun wholesale for this report: 946 Rust workspace tests, the separate polling test and sparse-gram property, 34 agent tests, and 49 installer scenarios passed. Generated candidate checks covered 161,098 pattern/document comparisons. These tests and the corpus checks are evidence, not a formal proof for all regexes or mutable filesystems.

## Resources, functions, and limitations

- The on-disk posting format is unchanged; this comparison does not claim a storage improvement. No new indexing-time or peak-memory benchmark was run.
- `--explain` adds actual posting-load/probe/copy and positional-rejection counters. It executes the search and leaves matching stdout unchanged.
- Lazy posting access and positional masks reduce work in some queries. The CLI gains remain small when startup, file verification, or broad output dominates.
- Both tools retain the inherited asynchronous watcher/freshness contract. This work does not make an index an atomic current-filesystem snapshot; use `--no-index` for current eligible-file reads.
- Measurements cover warm caches, file-list (`-l`) completion latency, one query at a time, and a Linux/WSL2 workstation. Cold-cache behavior, peak RAM, multi-client load, macOS/Windows execution, watcher lag, and other output modes were not benchmarked here.

## Reproducibility

Hardware is an Intel Core i9-14900KS with 32 available logical CPUs; each query uses four search threads. OS: Linux 6.6.87.2 WSL2 x86-64, glibc 2.39. Both binaries are optimized release builds from the same recorded Rust 1.98.1 environment. Benchmarks run sequentially without overlapping our builds, tests, or scan verification. The workstation is not isolated from unrelated system activity.

Corpora remain pinned to CPython `b54ae4cdfce4b84b22889b5c5b9ed0ab9bcb3702` and Rust `1a8fa555801329bd0e803d7384b5a21191c61f30`. Queries use shared eligible language scopes: C/header/Python in CPython, Rust in Rust. Complete file lists are validated before timing; each round randomizes query/engine order with seed 990251. Two warmups and route diagnostics are excluded. Server mode uses `--no-watch`; startup reconciliation is excluded by the workload warmup before timing. The additional scans verify outputs after the timed runs.

Run began `2026-09-25T09:03:55.224613+00:00` and ended `2026-09-25T09:08:56.377366+00:00`. [Exact command manifest](evidence/comparison-2026-09-25/run.json). Each raw result includes binary hashes, versions, corpus revision, all samples, validation hashes, and diagnostics.

Reproduce an individual run with its command from the manifest. Recompute summaries and independently verify the six raw result JSON files:

```sh
python3 scripts/summarize-evaluation.py RESULT1.json RESULT2.json ... --output summary.json
python3 scripts/verify-comparison.py RESULT1.json RESULT2.json ... --output scans.json
python3 scripts/compare-unicode.py \
  --upstream ../build/baseline/release/tgrep \
  --fork target/release/zgrep --output unicode.json
```

The evidence supports keeping the Boolean-plan and Unicode fixes. A broad speed claim would require more representative workloads and operating conditions; the new local 102-query result especially shows why the selected large gains should not be advertised as general search speedups.
