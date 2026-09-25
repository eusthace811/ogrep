# Next improvements and evaluation plan

The existing comparison is reproducible evidence for warm, single-query file-list latency and selected correctness properties. It does not establish the performance limit of the design. New measurements identify local startup as a promising next target and begin evaluating edit visibility.

## New evidence

### Opening an index

`profile_open` executes the public reader, metadata and filename-sidecar loaders, then an absent literal plan. Two iterations warm up and 100 are recorded. Initial file-identity reads explicitly warm the OS cache. This is a stage microbenchmark on existing index snapshots, not a complete CLI benchmark. Each snapshot file's length and BLAKE3 digest are retained; the user-updated corpus checkouts are not searched or modified.

| Stage, median milliseconds | CPython snapshot (6,250 files) | Rust snapshot (63,035 files) |
| --- | ---: | ---: |
| Load metadata | 0.0085 | 0.0226 |
| Open reader | 0.7788 | 9.4179 |
| Load visibility sidecar | 0.0176 | 0.0305 |
| Reload metadata for scope | 0.0080 | 0.0177 |
| Execute absent-query plan | 0.0632 | 0.0569 |
| Release loaded structures | 0.1151 | 0.9310 |

The Rust reader open takes about 9.4 ms versus 0.057 ms for the absent-query plan. This justifies investigating initialization before further optimizing intersections. Source inspection shows `IndexReader::open` decodes every file path, validates IDs, allocates the path vector, and eagerly sorts a path-membership index. The profile does **not** isolate how much of reader time belongs to each operation, so it cannot yet predict a particular speedup from lazy sorting. Repeated metadata loading is measurable but small here; it is a lower priority.

[CPython samples](../evidence/next-evaluation/open-cpython.json), [Rust samples](../evidence/next-evaluation/open-rust.json), [profiler](../zgrep-core/examples/profile_open.rs).

### Mutation-to-search visibility

A new disposable fixture has 1,000 filler source files and one changing file. Each engine performs six cycles of edit, rename, atomic replacement, delete and create. The benchmark waits for complete native-watcher coverage and initial reconciliation before mutations. Each observation verifies that the CLI used the server; every final answer is checked against an independent ripgrep scan. No user's files or pulled checkout are changed.

| Operation | tgrep median ms | zgrep median ms | Largest observed tgrep / zgrep ms |
| --- | ---: | ---: | ---: |
| edit | 26.88 | 25.88 | 37.53 / 38.31 |
| rename | 25.91 | 31.65 | 27.59 / 37.81 |
| replace | 26.56 | 26.56 | 37.57 / 37.55 |
| delete | 25.92 | 25.72 | 27.67 / 37.96 |
| create | 26.91 | 31.59 | 37.67 / 37.55 |

**All 60 mutations converged to the expected scan result.** Six observations per operation are insufficient for tail-latency or stable regression claims. Latency includes mutation completion, CLI startup/TCP and up to 10 ms between polling attempts; engines run sequentially. This measures observed search visibility, not a precise watcher-processing timer, durability, or a generation barrier. Deletion or content changes may become visible through exact verification before all index bookkeeping catches up. The experiment does not demonstrate a freshness advantage for zgrep.

[Raw observations and statuses](../evidence/next-evaluation/refresh.json), [script](../scripts/evaluate-refresh.py). Native watcher logs are preserved beside the result. Next tests should include bulk changes, ignore-rule changes, polling fallback, restarts and concurrent queries.

## Prioritized implementation experiments

| Priority | Candidate | Why it could matter | Evidence required before shipping |
| --- | --- | --- | --- |
| 1 | Build path-membership ordering lazily; reduce path-table allocation work | Reader initialization is a large measured fixed cost for local searches | Isolate sort/decode costs; compare current binary against candidate on all 102 upstream queries; retain dense-ID, corruption and atomic-visibility checks; test simultaneous first access and reader publication |
| 2 | Parallelize bounded candidate verification in local mode | The local search path currently verifies candidate files in a serial loop; broad/common queries can spend far more time here than in the planner | Compare 1/4/8 threads on common and broad queries; verify output, errors, ordering, `-q`, counts and JSON; measure peak RSS and small-query overhead |
| 3 | Explicit refresh acknowledgement or generation tokens | Observed watcher delay is asynchronous, and missed events cannot support immediate completeness claims | Specify content-version semantics; test own-edit acknowledgement, missed events, deletes/renames and branch switches; compare settled snapshots against scans |
| 4 | Function-level context / definition navigation | Adds useful functionality independent of small latency gains | Begin with a single language; evaluate correct enclosing spans, malformed-file fallback, and task/tool-call reduction against line context |
| 5 | Compression or better weighted sparse grams | May matter for much larger indexes or constrained memory | Demonstrate a storage/RSS/latency bottleneck first; compare whole index build, update and query cost. The previous sparse prototype's roughly 3× posting expansion remains negative evidence for that particular design |

These are proposed experiments, not implemented runtime features. The production search binary remains unchanged by this profiling work. A promising microbenchmark must be followed by end-to-end validation before claiming another speed improvement.

## A fuller evaluation matrix

1. **Immutable inputs.** Keep detached worktrees or exported snapshots at recorded commits, a file manifest/content hash, explicit eligibility rules, and index-to-snapshot provenance. Refuse to reuse an index after an input mismatch. This prevents an accidental `git pull` from silently changing an evaluation.
2. **Broader corpora and queries.** Retain CPython/Rust, add a large Linux tree and a few-large-files corpus, include a third language ecosystem, and keep an independent query set separate from optimization development. Exercise absent/common/short literals, nested regexes, Unicode, multi-pattern searches, globs and ignored/hidden files.
3. **More output modes.** Measure `-q`, `-l`, complete matching lines, counts, JSON and context separately. Compare equivalent complete outputs before timing. Faster truncated or differently scoped results are not equivalent searches.
4. **Cache and startup conditions.** Separate warm CLI, warm server, restarted server with warm OS cache, and genuinely cold OS-cache runs in an isolated VM. A new process is not evidence of a cold cache. Do not clear the shared host's cache to manufacture a result.
5. **Total resource cost.** Record indexing and incremental update time, process CPU, peak RSS, index size, startup time and query latency. Compute workload-specific amortization against scanning only when indexed queries are actually cheaper. Include maintenance cost rather than reporting query latency alone.
6. **Concurrency and tails.** Test 1, 4 and 16 clients with enough observations for p95/p99 estimation (at least thousands, with uncertainty), bounded throughput tests, and queries during update bursts. Preserve raw samples and timeouts; do not discard overloaded periods.
7. **Dynamic correctness.** Extend the new mutation fixture to bulk edits, rename storms, ignore changes, missing events, watcher fallback, restart/crash recovery and branch switches. Assert documented consistency rules during changes and exact parity after quiescence; do not assume an atomic mutable-tree snapshot.
8. **Cross-platform release checks.** Run existing CI and watcher/recovery scenarios on native Linux, macOS and Windows. Current WSL2 evidence alone cannot certify their watcher or mmap behavior.

For a candidate optimization, predeclare its target metric and a meaningful improvement threshold, use held-out queries, repeat promising and regressing cases independently, and publish both. Keep correctness a hard gate. Avoid treating many unadjusted per-query confidence intervals as an overall win.

## External benchmark references

- [ripgrep benchsuite](https://github.com/BurntSushi/ripgrep/blob/master/benchsuite/benchsuite) explicitly distinguishes many-small-file code search from few-large-file search and includes fair-scope as well as deliberately default-behavior comparisons. Adapt its corpus/query coverage; align file eligibility rather than copying default timings.
- [Rebar](https://github.com/BurntSushi/rebar) measures regex-engine behavior. It is useful for matcher regressions, but does not measure index loading, file walking, watchers or end-to-end search. Since this fork largely retains the inherited matcher, it is not the first optimization target.
- [Zoekt design](https://github.com/sourcegraph/zoekt/blob/main/doc/design.md) is a useful indexed-search comparison and architectural reference. A fair experiment must align snapshot contents, query semantics, limits and returned results; ranked/limited defaults must not be compared to exhaustive grep as equivalent.
- [Cursor's regex-index research](https://cursor.com/blog/fast-regex-search) discusses sparse grams and probabilistic masks. It motivates alternative designs, not a promise of gains here. Our retained negative sparse-prototype results should guide whether another variant is worth its build/storage cost.

Natural-language code-retrieval benchmarks answer a different product question. They become relevant if ranked discovery is implemented; exact grep should continue to be evaluated for exhaustive matching and documented scope.

## Reproduce the new measurements

```sh
cargo build --release --locked --example profile_open
target/release/examples/profile_open /path/to/existing/index > open.json
python3 scripts/evaluate-refresh.py \
  --engine upstream=/path/to/tgrep --engine fork=target/release/zgrep \
  --output /tmp/refresh-results/refresh.json
```

The profiler deliberately warms mapped files and never searches the source tree. The update harness creates and removes only its own temporary fixture and terminates its owned server in `finally`. It requires native-watcher operation and stops with an error rather than silently benchmarking polling or fallback scanning. Neither experiment changes the existing release binary or benchmark corpus checkouts.
