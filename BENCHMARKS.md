# zgrep 0.1.0 evaluation

Latest: [0.2.0 production evaluation](PRODUCTION_REPORT.md), including broader startup gains, output correctness, cold index pages, load and recovery. The measurements below are retained 0.1.0 evidence.

A [fresh head-to-head evaluation](TGREP_COMPARISON.md) repeats the comparison with another random order, adds the 102-query workload in local mode, and directly demonstrates the Unicode correctness differences. The release-development measurements below remain unchanged.

The retained approach improves the existing planner without replacing the index: preserve mandatory Boolean subplans, consult posting cardinalities before loading, probe sparse candidates directly, and compare existing position masks within literal runs. It also fixes an inherited Unicode fixed-insensitive false-negative bug and exposes actual index work through `--explain`.

The strongest measured benefit is nested alternation. Across the two selected queries, median local latency improved **2.90× on CPython and 13.47× on Rust**; warm-server latency improved **1.94× and 9.97×**. The broader 102-query upstream-derived server workload improved only **1.026×** in geometric mean query speedup (**1.043×** in the ratio of summed median latencies). These results support a targeted planner improvement, not a universal claim that zgrep is faster than tgrep or ripgrep.

## Evidence and methodology

Measured on an Intel Core i9-14900KS, 32 available logical CPUs, Linux 6.6.87.2 under WSL2 x86-64, glibc 2.39. Queries use four search threads. This is a shared workstation, not an isolated benchmark appliance. See each JSON's platform, affinity, engine version, and SHA-256 fields. Baselines are tgrep 1.0.10 at source commit `56f8c62` and installed ripgrep 14.1.0. The fork is 0.1.0; its measured binary SHA-256 is `ea34ade2e6bdaa19226d623f00bb7f3921ab74287bf3c185a82e14240bfd72cc`.

Frozen public corpora:

| Corpus | Git revision | Fork index size before final local run |
| --- | --- | ---: |
| CPython | `b54ae4cdfce4b84b22889b5c5b9ed0ab9bcb3702` | 81,379,386 bytes |
| Rust | `1a8fa555801329bd0e803d7384b5a21191c61f30` | 238,859,925 bytes |

Indices include hidden-file coverage; query eligibility is narrower. The CPython workload uses C/header/Python files and the Rust workload uses Rust files. This aligns the indexed tool's supported-extension scope with ripgrep. Initial exploratory unrestricted comparisons exposed `.dat` eligibility differences; they were not treated as matching workloads. Both frozen Git checkouts were clean after final evaluation. Small index metadata/visibility updates can change measured byte counts across runs; the posting format is unchanged. Initial Rust indexing was 2.212 s upstream versus 2.216 s fork in one exploratory measurement, not evidence of a build-speed improvement.

[`scripts/evaluate.py`](scripts/evaluate.py) executes complete file-list queries (`-l`, color disabled, no user config), verifies sorted output, then performs two excluded warmups plus diagnostic verification of local/server routing. Every timed run must reproduce its validation output and exit code. All engines' validation file-list hashes and exit codes were also checked jointly by the summary script. Each round interleaves all query/engine pairs in seeded random order. Samples are whole CLI invocation latency, including startup, path resolution, verification, output capture and, in server mode, TCP. Server mode uses a warm content cache and `--no-watch`; startup reconciliation completes during warmup. It does not benchmark watcher throughput.

The custom suites have 12 cases and 21 rounds per mode/corpus. The independent workload has 102 queries and 11 rounds on the Rust server. Its strings come from upstream's checked-in benchmark query list, using 91 literal queries with `-F` and 11 regex queries; all use `-t rust`. This corrects the inherited timing script's use of invalid regex strings and ignored error exits. The workload is independent of the new planner cases, but it is not a blind user study.

**All 6,390 timed searches matched their validated outputs and exits.** All three engines agreed on complete file lists and exits across the workload definitions. That is evidence about file-list semantics on these frozen eligible corpora; other output modes are covered by regression tests, not timed here.

No timed runs overlapped our builds or test suites. All are **warm OS-cache** measurements. There are no cold-cache, first-byte, production memory, multi-client throughput, or break-even claims. Medians and every raw sample are retained; sample p95 values exist in the JSON but are too small a sample for tail-latency guarantees.

Speedup below is upstream median / fork median; greater than one favors the fork. Brackets are descriptive 95% percentile intervals from 5,000 paired-round bootstrap resamples (seed 1984), recomputing the ratio of medians. Pairing preserves round membership, not simultaneous execution. There is no correction for multiple comparisons, and intervals do not account for other machines or workloads. [Summary calculation](scripts/summarize-evaluation.py), [summary data](evidence/final-summary.json).

## End-to-end results

Nested patterns are `(PyObject|PyType).*(GetAttr|SetAttr)` for CPython and `(struct|enum).*(TypeError|RegionError)` for Rust. Short and broad cases are deliberately retained because index advantages can disappear there.

### CPYTHON, local

[Raw samples and validation](evidence/final-cpython-local.json) · [Queries](evidence/queries-cpython.json)

| Query | tgrep ms | zgrep ms | rg ms | Speedup [95% interval] |
| --- | ---: | ---: | ---: | ---: |
| literal-rare | 4.24 | 4.10 | 13.71 | 1.03 [1.02, 1.05] |
| literal-common | 27.05 | 27.07 | 16.67 | 1.00 [0.99, 1.01] |
| literal-absent | 3.64 | 3.41 | 13.66 | 1.07 [1.04, 1.09] |
| literal-long | 4.97 | 4.66 | 13.75 | 1.06 [1.05, 1.09] |
| short | 72.32 | 71.99 | 29.34 | 1.00 [0.99, 1.02] |
| broad | 57.62 | 58.10 | 27.24 | 0.99 [0.98, 1.00] |
| alternation | 10.15 | 9.43 | 15.87 | 1.08 [1.06, 1.09] |
| nested-alternation | 45.47 | 15.66 | 16.19 | 2.90 [2.86, 2.96] |
| regex-selective | 9.15 | 9.16 | 14.17 | 1.00 [0.99, 1.02] |
| regex-common | 20.19 | 20.08 | 15.37 | 1.01 [0.99, 1.01] |
| case-insensitive | 8.32 | 8.28 | 15.01 | 1.00 [0.99, 1.01] |
| scoped | 5.86 | 5.91 | 8.23 | 0.99 [0.97, 1.01] |

Geometric mean query speedup: 1.114×. Ratio of summed medians: 1.131×. The large selected Boolean gain influences these aggregates.

### CPYTHON, server

[Raw samples and validation](evidence/final-cpython-server.json) · [Queries](evidence/queries-cpython.json)

| Query | tgrep ms | zgrep ms | rg ms | Speedup [95% interval] |
| --- | ---: | ---: | ---: | ---: |
| literal-rare | 2.85 | 2.86 | 13.35 | 1.00 [0.96, 1.06] |
| literal-common | 8.27 | 7.69 | 16.72 | 1.08 [0.99, 1.14] |
| literal-absent | 2.37 | 2.25 | 13.46 | 1.06 [1.01, 1.10] |
| literal-long | 3.89 | 3.48 | 13.36 | 1.12 [1.05, 1.20] |
| short | 17.02 | 16.94 | 31.04 | 1.00 [0.96, 1.09] |
| broad | 15.03 | 15.01 | 28.42 | 1.00 [0.93, 1.07] |
| alternation | 4.39 | 4.72 | 15.38 | 0.93 [0.89, 1.02] |
| nested-alternation | 10.31 | 5.31 | 15.74 | 1.94 [1.83, 2.13] |
| regex-selective | 4.84 | 4.59 | 13.91 | 1.05 [0.95, 1.14] |
| regex-common | 7.11 | 6.50 | 15.09 | 1.09 [0.97, 1.14] |
| case-insensitive | 4.54 | 4.62 | 14.91 | 0.98 [0.89, 1.13] |
| scoped | 3.51 | 3.70 | 8.03 | 0.95 [0.89, 1.00] |

Geometric mean query speedup: 1.079×. Ratio of summed medians: 1.084×. The large selected Boolean gain influences these aggregates.

### Rust, local

[Raw samples and validation](evidence/final-rust-local.json) · [Queries](evidence/queries-rust.json)

| Query | tgrep ms | zgrep ms | rg ms | Speedup [95% interval] |
| --- | ---: | ---: | ---: | ---: |
| literal-rare | 22.51 | 22.34 | 78.01 | 1.01 [0.99, 1.03] |
| literal-common | 64.28 | 64.28 | 89.40 | 1.00 [0.99, 1.01] |
| literal-absent | 17.54 | 16.87 | 77.38 | 1.04 [1.02, 1.06] |
| literal-long | 21.12 | 20.24 | 77.94 | 1.04 [1.03, 1.08] |
| short | 327.21 | 327.82 | 298.21 | 1.00 [0.99, 1.00] |
| broad | 162.88 | 161.94 | 147.87 | 1.01 [0.99, 1.01] |
| alternation | 36.19 | 30.92 | 81.10 | 1.17 [1.15, 1.19] |
| nested-alternation | 265.45 | 19.71 | 80.98 | 13.47 [13.29, 13.63] |
| regex-selective | 22.22 | 22.18 | 80.20 | 1.00 [0.98, 1.02] |
| regex-common | 21.89 | 21.46 | 77.88 | 1.02 [1.01, 1.04] |
| case-insensitive | 17.92 | 17.79 | 79.93 | 1.01 [0.99, 1.03] |
| scoped | 28.60 | 28.59 | 32.90 | 1.00 [0.98, 1.02] |

Geometric mean query speedup: 1.271×. Ratio of summed medians: 1.336×. The large selected Boolean gain influences these aggregates.

### Rust, server

[Raw samples and validation](evidence/final-rust-server.json) · [Queries](evidence/queries-rust.json)

| Query | tgrep ms | zgrep ms | rg ms | Speedup [95% interval] |
| --- | ---: | ---: | ---: | ---: |
| literal-rare | 6.80 | 6.47 | 78.28 | 1.05 [0.94, 1.17] |
| literal-common | 24.68 | 24.04 | 90.07 | 1.03 [0.98, 1.14] |
| literal-absent | 3.48 | 2.67 | 77.74 | 1.30 [1.24, 1.52] |
| literal-long | 6.67 | 6.34 | 77.65 | 1.05 [0.82, 1.30] |
| short | 158.69 | 160.32 | 251.94 | 0.99 [0.97, 1.01] |
| broad | 79.86 | 77.31 | 149.47 | 1.03 [1.00, 1.06] |
| alternation | 11.30 | 10.57 | 82.07 | 1.07 [0.98, 1.23] |
| nested-alternation | 61.70 | 6.19 | 81.68 | 9.97 [9.53, 10.85] |
| regex-selective | 7.14 | 6.95 | 79.67 | 1.03 [0.82, 1.28] |
| regex-common | 7.52 | 6.79 | 78.83 | 1.11 [0.84, 1.21] |
| case-insensitive | 4.54 | 3.88 | 79.44 | 1.17 [1.10, 1.63] |
| scoped | 10.04 | 9.56 | 33.41 | 1.05 [0.98, 1.21] |

Geometric mean query speedup: 1.297×. Ratio of summed medians: 1.191×. The large selected Boolean gain influences these aggregates.

### Independent upstream workload

The 102-query server workload's geometric mean speedup is 1.026× and its summed-median speedup is 1.043×. Twelve per-query intervals are wholly above 1; none are wholly below 1. This is descriptive evidence of modest gains, not proof of an absence of regressions. The worst observed median ratio is 0.908× (5.58 → 6.15 ms), and the best is 1.177× (185.51 → 157.59 ms). [Every query and interval](evidence/upstream-query-results.md), [raw samples](evidence/final-upstream-rust-server.json), [query definitions](evidence/queries-upstream-rust.json).

Selected-suite median regressions also remain visible: CPython server alternation is 4.39 → 4.72 ms (0.93×), scoped search 3.51 → 3.70 ms (0.95×); Rust server short search is 158.69 → 160.32 ms (0.99×). Their intervals include no change. Broad local queries often favor ripgrep: CPython `return` is 58.10 ms fork versus 27.24 ms rg. Do not replace scanning blindly for every workload.

## Why retain each change?

- **Boolean plans:** upstream discarded useful alternative subplans inside concatenations. Keeping conjunctions of subplans avoids full scans for the selected nested expressions, without exponential DNF expansion. End-to-end results above are the key evidence.
- **Lazy loading and probing:** an absent required gram now requires zero posting copies. In the warm CPython planner profile, rare-literal copies fell from 35,479 to 932 and execution from about 65.8 µs to 5.5 µs; long-literal copies fell from 51,535 to 2,392. That microprofile runs both executors on the *same new plan*, excluding parsing, file checks, and output. It isolates execution work and does not reproduce old parsing. Broad/common planner cases sometimes regress slightly. [Profile and raw samples](evidence/position-profile-cpython.json), [example](zgrep-core/examples/profile_plan.rs).
- **Position masks:** intersect rotated modulo-eight occurrence masks only inside a fixed literal run. Scattered `abcdXbcde` can be rejected for `abcde` while a real match survives; gaps such as `abcd.*bcde` are independent. CPython rare-literal candidates fell 14 → 13 and long-literal candidates 46 → 36 in the profile. Existing six-byte posting entries remain unchanged.
- **Unicode correctness:** the index ASCII-folds bytes; Unicode lowercasing a fixed query is not a sound prefilter for Unicode regex matching. Escaped-literal HIR analysis now conservatively breaks runs at unsafe Unicode classes. Real CLI tests cover Kelvin sign, long s, accented text, and dotted I, both indexed and scanning. This improvement does not depend on a timing claim.

## Sparse n-grams: tested and rejected for this release

The research prototype adapts deterministic pair-weight sparse grams from [danlark1/sparse_ngrams](https://github.com/danlark1/sparse_ngrams/tree/430f3befac3bce593c2aa77d2fd0a727d2089d41), with unsigned byte weights, maximum indexed length 32, and query covering target 16. Credit and license are retained in [experiments/LICENSE-sparse-ngrams](experiments/LICENSE-sparse-ngrams). It uses 2,048 deterministically selected source files per corpus, each at most 64 KiB. Hash collisions can only broaden candidates. A substring embedding property test checks query/index agreement.

| Sample | Trigram postings | Sparse postings | Increase |
| --- | ---: | ---: | ---: |
| CPython, 22,814,230 bytes | 3,542,034 | 11,885,706 | 3.36× |
| Rust, 4,942,016 bytes | 1,216,813 | 3,489,557 | 2.87× |

Most tested queries had equal or worse candidate counts. The Rust long literal improved 9 → 8 candidates (8 actual hits), while the CPython long literal worsened 7 → 9 (4 actual hits). Construction was slower in this prototype, but the implementations are not comparably optimized and no serialized sparse format was built. The defensible conclusion is that **this prototype does not justify a production index migration**, not that sparse grams are generally ineffective. [CPython results](evidence/sparse-cpython.json), [Rust results](evidence/sparse-rust.json), [prototype](zgrep-core/examples/sparse_probe.rs).

## Correctness and release gates

- 946 Rust workspace tests passed; the one separately ignored polling measurement also passed.
- The sparse-gram example property test passed. Planner-generated exact-match checks cover 311 patterns × 259 documents × 2 case modes = 161,098 pattern/document checks, plus disk/hybrid overlay, mutation, deletion, sparse-probe, Unicode, and positional fixtures.
- 34 Python agent tests passed, including the real Node/MCP roundtrip, as did 49 shell installer scenarios.
- Formatting and Clippy for all targets passed. Release compilation succeeded. The local package script additionally validates the extracted binary's local/server Unicode matches, positional rejection, and diagnostics, then emits an artifact checksum and build receipt.

Tests are evidence, not a formal proof over all regexes. Optimized candidates are permitted to be a strict subset of the old conservative candidate set; the independent exact matcher must never have a real match omitted. Inherited freshness and file-eligibility limitations are documented in [operations](docs/OPERATIONS.md).

[Final gate logs and exploratory record](evidence/README.md). The original restored-mtime recovery fixture needed to wait until filesystem ctime advanced; the recovery assertions were preserved. No failing production test was waived. Early sandbox and driver-invocation mistakes are labeled historical, not counted as final passes.

## Reproduce

Use a clean frozen checkout of each corpus and build both baseline and fork with their lockfiles. Index directories must be outside the corpus. From the fork:

```sh
cargo build --release --locked
python3 scripts/evaluate.py \
  --engine upstream=/absolute/path/to/baseline/tgrep \
  --engine fork=target/release/zgrep --engine rg=rg \
  --corpus /absolute/path/to/cpython \
  --queries evidence/queries-cpython.json \
  --work /absolute/path/to/eval-cpython --mode local --rounds 21 \
  --output /tmp/reproduced-cpython-local.json
```

Repeat with `--mode server`, then the Rust corpus and `queries-rust.json`. For the broader server suite use `queries-upstream-rust.json --rounds 11`. Stop other services using these index paths before running. Commands validate routing; stale descriptors are not accepted as ready unless they belong to the launched process. All owned servers are terminated on success or failure.

```sh
python3 scripts/summarize-evaluation.py /tmp/reproduced-*.json --output /tmp/summary.json
cargo test --workspace --locked
cargo test --workspace --examples --locked
cargo test --workspace --locked -- --ignored
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 -B -m unittest discover -s scripts/agent -p 'test_*.py' -v
bash scripts/test-install.sh
```

Build experiments with `cargo build --release --examples --locked`, then run `target/release/examples/profile_plan INDEX QUERIES_JSON` or `target/release/examples/sparse_probe ROOT INDEX QUERIES_JSON`. The sample identity hashes in sparse results permit detecting a changed sample. Packaging requires clean committed source: `python3 scripts/package.py`.

## Research basis and next experiments

The [historical research memo](docs/RESEARCH.md) covers additional directions and primary references. Relevant sources for this release include [GitHub's code-search architecture](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/), [Cursor's 2026 regex-index discussion](https://cursor.com/blog/fast-regex-search), [SIMD/galloping intersection research](https://arxiv.org/abs/1401.6399), [the 2025 n-gram comparison](https://arxiv.org/abs/2504.12251), and [Zoekt's index design](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md). The shipped seek path uses ordinary monotone binary searches; it does not claim to implement SIMD galloping.

Frequency-trained sparse grams, block-compressed postings, explicit refresh acknowledgements, function context, and ranked code discovery remain candidates for separate experiments. The present evidence supports delivering the existing-format planner improvements first. No syntax engine, embedding model, or freshness barrier was shipped under an unrelated performance claim.
