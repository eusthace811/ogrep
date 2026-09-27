# zgrep

Indexed exact text and regex search, forked from [Microsoft tgrep](https://github.com/microsoft/tgrep) at `56f8c62`. Version 0.3.0 adds source-read reuse, compact file-list replies, bounded server admission and explicit refresh completion for CLI and agents. It retains the improved query planner, existing index format and exact match verification.

The installed command is **`zgrep-code`**. Unix systems commonly already have an unrelated gzip utility called `zgrep`; the Cargo build output remains `target/release/zgrep`.

```sh
make install                            # ~/.cargo/bin/zgrep-code
zgrep-code index /path/to/repository
zgrep-code -F --explain -- 'some_identifier' /path/to/repository
zgrep-code serve /path/to/repository     # optional foreground server + watcher
```

Put `~/.cargo/bin` on PATH, or use `./target/release/zgrep` directly after `cargo build --release --locked`. `python3 scripts/package.py` creates a checksummed local Linux archive from a clean checkout and verifies the unpacked executable.

## What changed

- **Less local read work:** reuse the opened file when mmap falls back to a buffered read. [Phase measurements and comparisons](docs/LOCAL_PROFILING.md) retain favorable and unfavorable results.
- **Smaller replies and bounded work:** opt-in compact file listings preserve old callers; four active searches and a bounded queue control concurrent response memory. [Admission measurements](docs/SERVICE_ADMISSION.md) include throughput and latency tradeoffs.
- **Refresh completion:** `zgrep-code refresh /path/to/repository` and the agent `refresh_index` tool wait for searchable publication of completed edits. [Contract and evaluation](docs/REFRESH_EVALUATION.md) include failure behavior and the cost of rereading a large tree.
- **Retained local startup improvement:** lazy path ordering improved the previous release’s 102-query Rust workload by 1.27× versus tgrep. The combined [0.3.0 evaluation](PRODUCTION_REPORT.md) measures **1.334× local and 1.119× server-backed CLI speedup** against tgrep across the 102-query Rust workload, with 6,804 checked timings overall. The [0.2.0 report](evidence/release-0.2.0/PRODUCTION_REPORT.md) is retained separately.
- **More accurate output:** preserve CRLF bytes and unterminated lines, correct server JSON offsets, and honor positive type selection for hidden files. Unsupported `--crlf` matching now fails explicitly.

- **Better regex plans:** retain mandatory alternatives inside concatenations instead of discarding useful constraints. The tested nested-alternation queries ran **2.9× faster on CPython and 13.5× on Rust** with local indexes; the server gains were 1.9× and 10.0×.
- **Less posting work:** order lookups by cardinality, probe common lists for sparse candidates, and use existing position masks to reject impossible literal alignments. No index format migration is needed.
- **Correct Unicode fixed searches:** indexed `-Fi` now retains matches such as `kelvin` against `KELVIN`, which the inherited planner could incorrectly exclude.
- **Explain actual work:** `--explain` performs the search and reports posting loads, probes, copies, and positional rejections on stderr. Normal matching output is unchanged.

These are measured improvements on specific workloads, not a universal speedup. Ordinary queries are often similar to upstream; some regress. Broad local searches can be slower than ripgrep. See [the complete benchmark report](BENCHMARKS.md), including raw samples, output checks, independent upstream queries, and the sparse-gram prototype we rejected after evaluation.

The historical [0.1.0 tgrep comparison](TGREP_COMPARISON.md) adds 6,744 timed invocations and the 102-query workload in local mode: general local performance is effectively tied, while the targeted regex and Unicode improvements hold up.

[Next improvements and evaluation](docs/NEXT_EVALUATION.md) adds startup-stage measurements, a mutation-to-search visibility benchmark, and priorities for broader validation.

## Search contracts

Indexes and service discovery use `.zgrep/`, separate from upstream's `.tgrep/`. Ignore rules, supported file types, size limits, and binary/hidden policies determine the searchable corpus. The index is a prefilter; exact matching still verifies candidates.

An on-disk index reflects its last publication. The server's watcher is asynchronous. Use `refresh` and wait for success before indexed queries that must see completed edits, or `--no-index` for a direct scan of current eligible files. Refresh requires the running server and does not provide an atomic filesystem snapshot. Indexing-complete status is not a freshness guarantee. Add `.zgrep/` to the searched repository's `.gitignore`.

Use `zgrep-code --help` for flags, [agent guidance](AGENTS.md) for integration semantics, and [operations](docs/OPERATIONS.md) for installation, freshness, recovery, and platform limits. The local release gates were run on Linux x86-64; macOS/Windows CI definitions are retained but were not executed in this evaluation. No release has been remotely published or signed.

[Changes](CHANGELOG.md) · [Development decisions](DEVELOPMENT.md) · [Research and future directions](docs/RESEARCH.md) · [Inherited command reference](docs/upstream/README.md)

Original copyright and MIT license are retained in [LICENSE](LICENSE). Upstream performance and adoption claims in archived documents are not claims about this fork.
