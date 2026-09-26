# zgrep-code 0.2.0 evaluation and release qualification

The measured improvement is faster local search startup: the path membership table is now sorted only when needed. Exact output, hidden-file type selection and dependency fixes also add value. Server throughput is not claimed to improve generally. This report qualifies trusted local Linux/WSL use; it does not qualify a shared network service or native macOS/Windows execution.

## Search latency

Speedups below are geometric means of per-query median ratios; above 1 means faster. The broader workload comes from 102 upstream queries. The two 12-query workloads were selected to exercise planner strengths and weaknesses, so they are not representative usage distributions.

| Workload | Mode | Queries | vs tgrep | vs zgrep 0.1.0 |
| --- | --- | ---: | ---: | ---: |
| rust-broad | local | 102 | 1.267× | 1.262× |
| rust-broad | server | 102 | 1.010× | 0.998× |
| rust-targeted | local | 12 | 1.728× | 1.362× |
| rust-targeted | server | 12 | 1.330× | 1.004× |
| cpython-targeted | local | 12 | 1.196× | 1.082× |
| cpython-targeted | server | 12 | 1.087× | 0.988× |

All **6,804 timed invocations** reproduced their validated sorted file lists and exit codes. Independent ripgrep scans checked **252 workload/query combinations** across the three engines. [Scan verification](evidence/release-0.2.0/scan-verification.json).

Nine rounds per workload, two untimed warmups, seeded randomized query/engine order, four query threads. Timings include CLI startup and, in server mode, TCP transport. Warm cache; server startup is excluded. No builds, tests or other benchmarks launched by this runner overlap the timed workloads. This workstation is not isolated from unrelated OS activity.

Descriptive per-query 95% intervals use 5,000 paired-round bootstrap resamples, with no multiple-comparison correction. They measure run variability, not uncertainty across machines or query populations. [All medians and intervals](evidence/release-0.2.0/summary.json).

| Broad Rust workload | Queries with interval entirely faster than tgrep | Entirely slower |
| --- | ---: | ---: |
| local | 102/102 | 0/102 |
| server | 6/102 | 1/102 |

Five lowest observed speedups in the broad server workload are retained below; this is not a claim that every query improved.

| Query | tgrep ms | 0.2.0 ms | Speedup [95% interval] |
| --- | ---: | ---: | ---: |
| `HashMap<` | 7.44 | 8.77 | 0.849 [0.779, 1.164] |
| `TODO` | 5.25 | 5.80 | 0.906 [0.818, 1.030] |
| `expect(` | 17.46 | 18.91 | 0.923 [0.881, 0.987] |
| `writeln!(` | 5.62 | 6.02 | 0.934 [0.862, 1.099] |
| `loop {` | 12.09 | 12.95 | 0.934 [0.925, 1.106] |

## Cold index pages and restarts

The cache test uses disposable index copies, `posix_fadvise(DONTNEED)` and `mincore`. Every cold trial required zero resident index pages before running. All 180 cold/warm invocations agreed with independent scans. Source-file pages, executable pages, filesystem metadata and device caches were **not** established cold.

| Cold-index query | tgrep median ms | 0.1.0 median ms | 0.2.0 median ms |
| --- | ---: | ---: | ---: |
| `NonNull` | 30.35 | 29.54 | 22.35 |
| absent literal | 29.75 | 24.09 | 16.86 |

Ten process restarts all completed startup reconciliation and returned the correct first-query files. Median readiness was **858.7 ms**, including up to 50 ms polling delay; median first RPC query was **9.55 ms**. [Restart data](evidence/release-0.2.0/startup.json); [cache data](evidence/release-0.2.0/cache.json).

## Load, memory and indexing

Each concurrency level ran 1,024 loopback RPC requests against the frozen Rust tree, mixing six regexes (including a broad query returning 24,778 files). Each response file set was checked against ripgrep. These are end-to-end local RPC latencies with a four-thread server pool, not distributed-service measurements.

| Clients | Requests/s | Median ms | p95 ms | p99 ms | Server peak RSS MiB | Incorrect results |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 30.8 | 6.58 | 151.67 | 158.23 | 527.1 | 0 |
| 4 | 68.5 | 13.34 | 255.57 | 312.85 | 868.1 | 0 |
| 16 | 79.8 | 97.31 | 734.98 | 875.63 | 1662.0 | 0 |

Higher concurrency increases queueing and memory substantially. The server is suitable for trusted local use; four clients is a more conservative operating point on this measured workstation. The load generator and large result transport can also limit throughput. A worker-thread limit is not request admission control. [Raw load and CPU data](evidence/release-0.2.0/load.json).

Fresh indexing, one build per engine on warm source pages (descriptive measurements, not a statistical speed claim):

| Engine | Wall seconds | CPU seconds | Peak RSS MiB | Index MiB |
| --- | ---: | ---: | ---: | ---: |
| upstream | 2.16 | 6.23 | 175.5 | 227.70 |
| release | 2.06 | 6.00 | 172.3 | 227.70 |
| fork | 2.06 | 5.94 | 172.1 | 227.70 |

The posting format and practical index size are unchanged; tiny byte differences include metadata. No indexing-resource reduction is claimed. [Resource records](evidence/release-0.2.0/index-resources.json).

## Correctness and recovery

- **951 Rust workspace tests**, plus the separate polling measurement and sparse-gram property test; generated planner checks cover 161,098 pattern/document comparisons.
- **34 agent integration tests** and **49 installer scenarios**; format and all-target Clippy checks pass.
- **96 exact-output comparisons** against ripgrep across scan/local/server modes: lines, counts, JSON, context, quiet, file listing, types/globs, Unicode, multiline and error exits. JSON normalization removes performance counters only; error wording is not compared.
- **15 exact-output recovery checkpoints** across native notifications, explicit polling and forced polling fallback; **4,296 concurrent queries** during bulk edits and ignore changes. Each mode covers 400 bulk mutations, ignore exclusion/restoration, SIGKILL, 50 edits around the crash and restart.
- Dependency audit: **zero vulnerabilities and zero warnings** against the recorded RustSec database. Prior failing audits and fixes are retained under `evidence/production/`.
- The known-positive Unicode fixture still shows four indexed false negatives in tgrep, all corrected by the fork; all scanners and the ASCII control agree.

[Commands and test provenance](evidence/release-0.2.0/qualification.json); [output cases](evidence/release-0.2.0/output-modes-020.json); [recovery records](evidence/release-0.2.0/recovery.json); [audit](evidence/release-0.2.0/audit.log); [Unicode comparison](evidence/release-0.2.0/unicode-020.json).

The new differential tests exposed inherited CRLF stripping, invented newlines in JSON, missing server JSON byte offsets and hidden-file type filtering. Initial failures are preserved, alongside focused regression tests. `--crlf` was silently ignored upstream; 0.2.0 rejects that unsupported matching mode with exit 2. Default LF matching preserves CR source bytes. This release is not full ripgrep compatibility.

## Provenance and delivery

- Measured runtime source commit: `43de89f7489b69b70d11ca8dbc402e409d275ab1`.
- Release executable SHA-256: `45777bbc84a9043cf3de690b781cf623fa1806368ae377ae5de4386f5450351d`.
- Upstream tgrep: commit `56f8c62`, version 1.0.10; retained 0.1.0 release is the second baseline.
- Rust corpus: `1a8fa555801329bd0e803d7384b5a21191c61f30`; CPython: `b54ae4cdfce4b84b22889b5c5b9ed0ab9bcb3702`. Separate detached worktrees protect benchmarks from changes to the user’s normal clones. Index provenance, clean corpus states, binary hashes and execution routes are verified.
- Intel Core i9-14900KS, 32 available logical CPUs, WSL2 Linux 6.6.87.2 x86-64, Rust 1.98.1. No macOS/Windows runtime qualification or remote publication.
- Run `python3 scripts/evaluate-release.py --output-directory evidence/NEW-RUN` after preparing the documented frozen corpora and baselines. [Exact command manifest](evidence/release-0.2.0/run.json).

Packaging and extracted-executable validation are the remaining release gate.

## Operating limits and further improvements

Indexes and watchers are asynchronous; these checks do not create an atomic current-filesystem snapshot. For current eligible-file reads use `--no-index`. The trusted loopback service has no multi-user authorization. Search scope, unsupported options, memory controls and recovery are documented in [OPERATIONS.md](docs/OPERATIONS.md).

There is room to improve further: bound concurrent work and reduce file-list response overhead at the measured concurrency knee; profile parallel local verification for broad queries; add an explicit refresh-completion contract for agents. These are hypotheses to evaluate, not shipped capabilities. Sparse-gram indexing remains rejected by the retained experiment because its posting growth did not justify its selectivity. See [research](docs/RESEARCH.md) and [next evaluations](docs/NEXT_EVALUATION.md).

This evidence establishes useful improvements for the measured workloads and a qualified local release. It is not a formal proof for all regexes, filesystems, input encodings or operating systems.
