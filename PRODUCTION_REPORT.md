# zgrep-code 0.3.0 evaluation and release qualification

The 102-query Rust workload measured **1.334× local speedup** and **1.119× server-backed CLI speedup** versus tgrep. Source-handle reuse reduces local read work; compact file-list replies reduce response work. Bounded admission controls concurrent search/response allocations, and CLI/RPC/agent refresh receipts make completed edits searchable before success. These are workload-specific improvements, not a universal ranking.

This report covers trusted local Linux/WSL use. It does not qualify a shared network service or native macOS/Windows operation. Previous [0.2.0 evidence](evidence/release-0.2.0/PRODUCTION_REPORT.md) and [development experiments](evidence/next-release/README.md) are preserved.

## Search latency

Speedups are geometric means of per-query median ratios; above one means faster. The broad set contains 102 upstream queries. Each targeted set has 12 queries chosen to exercise strengths and weaknesses, not a representative usage distribution.

| Workload | Mode | Queries | vs tgrep | vs 0.2.0 |
| --- | --- | ---: | ---: | ---: |
| rust-broad | local | 102 | 1.334× | 1.052× |
| rust-broad | server | 102 | 1.119× | 1.109× |
| rust-targeted | local | 12 | 1.825× | 1.044× |
| rust-targeted | server | 12 | 1.416× | 1.095× |
| cpython-targeted | local | 12 | 1.222× | 1.039× |
| cpython-targeted | server | 12 | 1.132× | 1.040× |

All **6,804 timed invocations** reproduced their validated file sets and exit codes. Independent ripgrep scans checked **252 workload/query combinations** across the three engines. Nine randomized, interleaved rounds, two untimed warmups, four query threads; ordinary `-l` semantics, including CLI startup and TCP transport in server mode. The server startup itself is excluded. Source pages are warm; the workstation is not isolated from unrelated OS activity. The runner overlaps no builds, tests or other benchmarks with timing.

[Independent scan checks](evidence/release-0.3.0/scan-verification.json); [Exact commands](evidence/release-0.3.0/run.json).

Per-query 95% intervals use 5,000 paired-round bootstrap resamples without multiple-comparison correction. They describe within-run variability, not uncertainty across machines or user query distributions.

| Workload/mode | Baseline | Intervals entirely faster | Entirely slower |
| --- | --- | ---: | ---: |
| rust-broad-local | tgrep | 102/102 | 0/102 |
| rust-broad-server | tgrep | 55/102 | 1/102 |
| rust-targeted-local | tgrep | 12/12 | 0/12 |
| rust-targeted-server | tgrep | 5/12 | 0/12 |
| cpython-targeted-local | tgrep | 12/12 | 0/12 |
| cpython-targeted-server | tgrep | 5/12 | 0/12 |
| rust-broad-local | 0.2.0 | 52/102 | 0/102 |
| rust-broad-server | 0.2.0 | 55/102 | 0/102 |
| rust-targeted-local | 0.2.0 | 5/12 | 0/12 |
| rust-targeted-server | 0.2.0 | 3/12 | 0/12 |
| cpython-targeted-local | 0.2.0 | 7/12 | 0/12 |
| cpython-targeted-server | 0.2.0 | 3/12 | 0/12 |

[All tgrep comparisons](evidence/release-0.3.0/summary.json); [All 0.2.0 comparisons](evidence/release-0.3.0/summary-release.json).

The five lowest broad-server speedups against each baseline are retained below. A sub-one ratio is slower; small differences may be noise. All individual regressions remain in the linked raw summaries.

| Baseline | Query | Baseline ms | 0.3.0 ms | Speedup [95% interval] |
| --- | --- | ---: | ---: | ---: |
| tgrep | `XXX` | 4.63 | 5.17 | 0.896 [0.750, 1.153] |
| tgrep | `BTreeMap<` | 5.23 | 5.59 | 0.935 [0.860, 0.969] |
| tgrep | `TODO` | 5.06 | 5.28 | 0.957 [0.790, 1.212] |
| tgrep | `Rc<RefCell` | 4.69 | 4.85 | 0.968 [0.846, 1.084] |
| tgrep | `Pin<` | 6.29 | 6.27 | 1.002 [0.908, 1.093] |
| 0.2.0 | `MaybeUninit` | 7.05 | 7.65 | 0.921 [0.828, 1.057] |
| 0.2.0 | `impl Display` | 5.19 | 5.45 | 0.952 [0.822, 1.128] |
| 0.2.0 | `Arc<Mutex` | 4.33 | 4.43 | 0.976 [0.885, 1.166] |
| 0.2.0 | `BTreeMap<` | 5.52 | 5.59 | 0.988 [0.904, 1.072] |
| 0.2.0 | `Rc<RefCell` | 4.81 | 4.85 | 0.992 [0.879, 1.124] |

## Load, CPU and memory

Each engine ran 1,024 loopback requests at each of 1/4/16 clients, mixing six regexes. Every file set matched ripgrep. All clients request optional compact replies; older servers ignore that parameter and send legacy rows. The current daemon uses the default four active search slots. Status is sampled during load. These are end-to-end local RPC results, including Python client parsing; the generator can constrain throughput.

| Engine | Clients | Requests/s | Median ms | p95 ms | p99 ms | Server CPU s/1024 | Peak RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tgrep | 1 | 29.7 | 6.76 | 157.92 | 166.04 | 66.73 | 538.0 |
| tgrep | 4 | 68.2 | 13.77 | 248.32 | 305.91 | 68.81 | 855.9 |
| tgrep | 16 | 73.4 | 118.95 | 777.84 | 939.14 | 74.33 | 1606.7 |
| 0.2.0 | 1 | 28.8 | 6.67 | 164.18 | 171.69 | 67.49 | 536.3 |
| 0.2.0 | 4 | 65.7 | 14.07 | 262.92 | 322.44 | 69.78 | 844.2 |
| 0.2.0 | 16 | 75.7 | 105.18 | 751.90 | 905.18 | 72.36 | 1630.3 |
| 0.3.0 | 1 | 54.0 | 5.11 | 76.58 | 81.46 | 52.16 | 529.4 |
| 0.3.0 | 4 | 151.7 | 9.72 | 99.46 | 117.55 | 52.69 | 683.3 |
| 0.3.0 | 16 | 193.6 | 64.23 | 175.86 | 205.73 | 56.46 | 740.5 |

Peak RSS is cumulative across concurrency levels in each process. Client CPU/RSS and all latency samples are retained. This comparison combines compact replies and admission; it cannot attribute the total gain to either alone. [Repeated admission-only experiments](docs/SERVICE_ADMISSION.md) found lower memory and tail latency with a modest throughput cost and higher median latency. Do not describe the limiter itself as a general speedup. Per-query output and total RSS are not bounded by the request count.

[Current load](evidence/release-0.3.0/load.json); [0.2.0 load](evidence/release-0.3.0/load-release.json); [tgrep load](evidence/release-0.3.0/load-upstream.json).

Fresh indexing used one build per engine on warm source pages. These are descriptive resource measurements; no indexing-speed improvement is claimed.

| Engine | Wall s | CPU s | Peak RSS MiB | Index MiB |
| --- | ---: | ---: | ---: | ---: |
| upstream | 2.06 | 6.00 | 172.8 | 227.70 |
| release | 2.06 | 5.94 | 173.6 | 227.70 |
| fork | 2.11 | 6.04 | 172.4 | 227.70 |

[Index resource data](evidence/release-0.3.0/index-resources.json).

## Refresh correctness and operating cost

Explicit refresh rereads eligible content and returns success after searchable publication. It covers completed edits whose content and eligibility stay stable through completion; concurrent mutations may or may not appear. It is not an atomic filesystem snapshot. Timeout and cancellation do not acknowledge freshness, and server work may continue. Agent `freshness: current` remains a direct scan.

| Mode | Search clients | Refresh receipts | Median edit refresh ms | Maximum edit refresh ms | Maximum status ms | Checked concurrent searches |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| disabled | 0 | 38 | 16.29 | 18.24 | 0.72 | 0 |
| disabled | 16 | 37 | 26.62 | 36.50 | 25.10 | 3,437 |
| native | 0 | 38 | 17.81 | 20.00 | 1.98 | 0 |
| native | 16 | 37 | 27.02 | 39.23 | 21.00 | 3,752 |
| poll | 0 | 38 | 16.51 | 18.67 | 0.52 | 0 |
| poll | 16 | 37 | 29.71 | 39.10 | 20.81 | 3,839 |

All **225 receipts** passed immediate exact line-output and filename-set comparisons with independent ripgrep scans (**450 comparisons**), without polling retries. All **11,028 concurrent searches** matched the independently checked stable file set. The fixture uses 1,000 stable files plus edited subjects and covers 12 mutation types, ignore changes, restored mtime, binary filenames, hidden files and quiet-mode restarts. tgrep returns method-not-found for this explicit RPC; that is a capability comparison, not a speed ratio. [Raw refresh matrix](evidence/release-0.3.0/refresh-completion.json).

On the pinned Rust tree, **63,009 content files**, one warmup plus 15 timed unchanged-tree refreshes gave median **2.981 s**, range **2.858–3.086 s**, and median **5.97 server CPU seconds**. Cumulative peak RSS was **2539.9 MiB**; worst status latency **31.25 ms**. All 192 follow-up query checks agreed with independent scans.

Refresh regenerates postings for every eligible file, so it is a batch operation with substantial CPU and memory cost. Repeated-pass observations do not establish an indefinite memory bound. The [development cost and stability report](docs/REFRESH_EVALUATION.md) retains both earlier favorable and unfavorable resource measurements. Avoid invoking it for every keystroke; a direct scan can be preferable for one query. [All release refresh-cost samples](evidence/release-0.3.0/refresh-cost.json).

A separate **60-pass** follow-up (plus warmup) retained the same binary and 12 independently checked queries after each receipt: **732 comparisons**. Median refresh was **2.951 s**, cumulative peak **3037.1 MiB**, and worst status latency **29.05 ms**. Across the final 20 passes, post-query RSS ranged **2096.3–2565.0 MiB**, and cumulative peak ranged **3037.1–3037.1 MiB**. The original 15-pass run remains above; the follow-up does not replace it or prove indefinite stability. [Extended observation](evidence/release-0.3.0/refresh-stability.json).

## Cache, recovery and correctness

- **979 Rust workspace tests**, all features, plus the separate ignored polling measurement and sparse-gram example/property tests; format and all-target/all-feature Clippy pass.
- **43 agent integration/installer tests**, including real pi→MCP→CLI→server refresh, cancellation/shutdown, and explicit overload without scan fallback; **49 shell installer scenarios**. Agent tests explicitly use the measured release binary.
- **96 exact output/exit comparisons** with ripgrep across scan/local/server modes; source line bytes, JSON offsets, counts, context, Unicode, multiline, scopes and error exits. JSON normalization removes performance counters only. This remains a documented subset of ripgrep compatibility.
- **15 recovery checkpoints** across native, poll and forced polling fallback, with **4,301 concurrent queries** during bulk changes; SIGKILL and 50 edits across restart are included.
- **180 cold/warm index checks** require `mincore`-verified zero resident index pages after eviction. Source, executable, metadata and device caches were not established cold.
- **10 restarts**, median reconciled readiness **908.8 ms**, including up to 50 ms polling delay; median first query **8.73 ms**.
- Dependency audit has zero reported vulnerabilities and warnings against its recorded RustSec database; packaging repeats the audit. Earlier failures and fixes remain in historical evidence.

[Qualification commands](evidence/release-0.3.0/qualification.json); [Output cases](evidence/release-0.3.0/output-modes.json); [Recovery](evidence/release-0.3.0/recovery.json); [Cache](evidence/release-0.3.0/cache.json); [Restarts](evidence/release-0.3.0/startup.json); [Known-positive Unicode comparison](evidence/release-0.3.0/unicode.json); [Dependency audit](evidence/release-0.3.0/audit.log).

## Provenance, delivery and limits

- Measured source: `37731c433c8593caeede2f2308d1ef361bc83116`; executable SHA-256: `2fc6f9d5350821848d08c96ee278445fed15f198f093dcde2edb8f381e2b013a`.
- Baselines: tgrep `56f8c62`, version 1.0.10; retained zgrep 0.2.0. Frozen Rust `1a8fa555801329bd0e803d7384b5a21191c61f30` and CPython `b54ae4cdfce4b84b22889b5c5b9ed0ab9bcb3702`; clean states, index provenance, binary identities and routes are verified.
- Workstation: Intel Core i9-14900KS, Linux/WSL2 x86-64. Full platform, affinity and toolchain records are in the raw evaluations and qualification manifest.
- Reproduce from a clean, built checkout with `python3 scripts/evaluate-release.py --output-directory evidence/NEW-RUN --work-root ../build/NEW-RUN`. Output must be empty; the refresh-cost work directory must be new.

Packaging and extracted-executable validation remain required. This report is not yet a completion claim.

The loopback service has no multi-user authorization. Admission bounds active/pending work, not output size or total RSS. Large refreshes can be expensive. No remote publication/signing, native macOS/Windows qualification, whole-machine cold-cache test or universal performance proof is claimed. [Operations](docs/OPERATIONS.md) documents freshness, scope, compatibility and recovery.

The next optional experiments are boolean-only verification, matcher scratch reuse, avoiding JSON tree copies and content-identity reuse during refresh. They require new profiles and differential tests; none is an additional speed promise. [Research](docs/RESEARCH_NEXT.md), [local profiling](docs/LOCAL_PROFILING.md) and [admission ablation](docs/SERVICE_ADMISSION.md) record why the shipped changes were chosen.
