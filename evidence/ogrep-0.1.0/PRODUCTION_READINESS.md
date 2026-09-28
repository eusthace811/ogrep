# ogrep 0.1.0: production readiness and comparison with tgrep

Scope: a trusted, local CLI and agent search service, on Linux/WSL and on Windows (MSVC), measured on an Intel i9-14900KS under WSL2 with warm page caches. It is qualified against tgrep 1.0.10 (the upstream) and zgrep 0.3.0 (the fork ogrep started from).

Every number below comes from source commit `0e35817`, release binary `85c2647e…` (`lab/bin/ogrep-final`), using zgrep's own qualification and evaluation scripts, which were not modified. The raw results are in this directory.

## Verdict

| Gate | Evidence | Result |
|---|---|---|
| Format, Clippy, workspace tests | [qualification.json](qualification.json) | Passed: 997 tests, 1 ignored (as in zgrep), plus the polling and sparse-example checks |
| Windows (MSVC) | `./x test` from Git Bash, repository on a Windows disk | Passed: 966 tests. The difference from Linux is platform-specific tests only |
| Agent integration, installer | qualification logs | Passed: 43 agent tests, 49 installer scenarios |
| Output compatibility with ripgrep | [output-modes.json](output-modes.json) | 96/96 exact output and exit-code cases |
| Unicode correctness | [unicode.json](unicode.json) | ogrep indexed search 5/5 correct; **tgrep indexed search 1/5** (misses `kelvin` against `K`, the Kelvin sign) |
| Search correctness at scale | [comparison-verification.json](comparison-verification.json) | All 264 queries across 6 workloads agree with fresh ripgrep 15.2 scans |
| Speed vs tgrep and zgrep | [evaluation/](evaluation/) | Faster than both on every workload (table below) |
| Mutation and crash recovery | [recovery.json](recovery.json) | 15/15 checkpoints correct (native, poll and fallback watchers; SIGKILL; offline edits; restart); 5,065 concurrent queries |
| Refresh completion | [refresh-completion.json](refresh-completion.json) | 6 cases; 12,625 concurrent searches during refreshes; median refresh 15–26 ms. tgrep has no refresh at all |
| Concurrent load | [load.json](load.json), [load-upstream.json](load-upstream.json) | 0 errors in every case; 3.5× tgrep's throughput, a quarter of its p99 latency, a third of its memory at 16 clients |
| Startup | [startup.json](startup.json) | 10/10 restarts correct; ready in a median of 859 ms |
| Cold and warm index cache | [index-cache.json](index-cache.json) | 180/180 correct; cold 12.6 ms (tgrep 29.4); warm 4.7 ms (tgrep 21.4) |
| Index build | [index-resources.json](index-resources.json) | Same time, memory and index size as tgrep (2.0 s, about 175 MiB, 227 MiB) |
| Refresh cost | [refresh-cost.json](refresh-cost.json) | About 475 ms per explicit refresh of the unchanged Rust tree (zgrep 0.3.0: 2.98 s) |
| Dependencies | [audit.json](audit.json) | RustSec: 171 crates, 0 vulnerabilities, 0 warnings |

## Speed

zgrep's harness runs one process per query (`-l --color never -j 4`). Five seeded, interleaved rounds give the geometric mean of per-query medians; above 1 means ogrep is faster. Every timed output was validated first.

| Workload | Queries | ogrep vs tgrep | ogrep vs zgrep | zgrep vs tgrep |
|---|---:|---:|---:|---:|
| Rust broad, local | 102 | **3.70×** | 2.77× | 1.33× |
| Rust broad, server | 102 | **1.93×** | 1.72× | 1.13× |
| Rust targeted, local | 12 | **4.82×** | 2.69× | 1.80× |
| Rust targeted, server | 12 | **2.13×** | 1.52× | 1.40× |
| CPython targeted, local | 12 | **2.23×** | 1.84× | 1.21× |
| CPython targeted, server | 12 | **1.30×** | 1.17× | 1.11× |

The zgrep-vs-tgrep column reproduces zgrep's own published figures (1.334× local, 1.119× server), which confirms the harness.

In this run, 2 of the 264 queries have a median slower than zgrep's, by 0.1–0.4 ms on 2.5–3 ms queries (`literal-rare` on CPython, `literal-absent` on Rust). Measured directly over 300 runs, `literal-rare` is not slower: ogrep 2.72–2.96 ms, zgrep 2.74–3.76 ms. No ogrep query had a latency spike; tgrep had two.

Concurrent server load (1,024 requests at each client count):

| Clients | ogrep req/s | tgrep req/s | ogrep p99 | tgrep p99 | ogrep peak RSS | tgrep peak RSS |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 108 | 30 | 31 ms | 160 ms | 524 MiB | 548 MiB |
| 4 | 195 | 72 | 83 ms | 289 ms | 524 MiB | 845 MiB |
| 16 | 232 | 83 | 222 ms | 837 ms | 580 MiB | 1,617 MiB |

## ogrep compared with tgrep

- **Interface:** a strict superset. ogrep accepts all 104 of tgrep's flags and adds `--ident`, `--with` and `--without`. It has all of tgrep's subcommands, plus `refresh`, which came from zgrep.
- **Index format:** ogrep searches an index built by tgrep and returns identical results (checked on 3 queries against tgrep's own Rust index). ogrep keeps its own index in `.ogrep/`, so the two never share one by accident.
- **Correctness differences:**
  - **Unicode case folding:** fixed in zgrep, so ogrep's indexed search finds what tgrep misses.
  - **CRLF and JSON output:** fixed in zgrep.
  - **Smart case:** ogrep counts every pattern and only literal characters, as ripgrep does; tgrep looks only at the positional pattern.
  - **Inline flags:** an inline flag in one `-e` pattern no longer leaks into the next.
  - **`-f` handling:** matches ripgrep (read once, repeatable, standard input, blank lines, empty file).
  - **`--no-messages`:** hides only per-file errors.
- **Where the speed comes from:**
  - **Local:** parallel verification, `-l` stopping at the first match, the anchored-pattern prescan, and a cheaper index open.
  - **Server:** direct reply serialization, parallel candidate filtering and cache resolution, cached NUL offsets.
  - **Refresh:** unchanged content is skipped.
  - **Planning:** each pattern is planned separately.

## Problems this qualification found and fixed

Measuring against tgrep and zgrep, rather than trusting earlier numbers, turned up three regressions introduced during code review. All three are fixed, and each fix is measured:

1. **UTF-8 re-validation on the server (commit `d3af425`):** broad server queries lost about 3 ms (Rust targeted server 1.18× slower than the r2 candidate). `PathTable::get` re-checked every path because the mapped `files.bin` could be rewritten in place. The root cause is fixed instead: `files.bin` is now replaced by rename, never rewritten, and a test proves an open reader keeps its paths across an in-place rebuild.
2. **40 ms Nagle stalls (commit `1f01128`):** random server queries stalled for about 40 ms (4–6 per 102-query run), because each RPC line was sent as two writes. Every request and reply is now one write, with `TCP_NODELAY` on both ends. Spikes went to 0.
3. **Index-build layout regression (commit `0e35817`):** index builds ran 5% slower because of a code-layout effect of a function that indexing never calls. The function moved to the crate that uses it.

## Known limits (inherited, not changed here)

- **Loopback-only, unauthenticated server:** admission bounds concurrent work, not per-query output or total memory.
- **In-place `ogrep index` rebuild:** it still truncates `lookup.bin`/`index.bin` under a running local search. `files.bin` is now safe, but those two are inherited as is. On Windows the rebuild instead fails with error 1224 while anything has the index open; it does not corrupt anything.
- **Refresh:** it rereads eligible files and covers edits that are stable through completion. It does not take an atomic snapshot of a tree that is changing concurrently. Explicit refresh of a large tree costs about 0.5 s and 0.8 GiB peak.
- **ripgrep differences:**
  - A PCRE-style match failure stops the search, where ripgrep carries on.
  - Under `--no-messages`, a folder containing an unreadable file exits 0, where ripgrep exits 2.
  - Nest-limit parity with ripgrep does not hold under `-x`/`-w`.
- **Unsigned build:** it has not been remotely published.

## Reproduce

```sh
cargo build --release --locked && cp target/release/ogrep lab/bin/ogrep-final
python3 scripts/qualify.py --output evidence/ogrep-0.1.0/qualification.json --upstream lab/bin/tgrep-1.0.10
lab/run-eval.sh r6 5 ogrep-final && python3 lab/summarize.py lab/results/r6
python3 scripts/verify-comparison.py --output evidence/ogrep-0.1.0/comparison-verification.json lab/results/r6/*.json
# robustness: see scripts/evaluate-{recovery,refresh-completion,load,startup,index-cache,index-resources,refresh-cost}.py
```
