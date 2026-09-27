# Changes

## 0.3.0

- Reuse the opened source handle when mmap verification falls back to an owned read. Ordinary local profiling and broad/targeted comparisons retain both gains and regressions.
- Negotiate compact file-list RPC replies while preserving legacy clients and binary markers.
- Bound service work: four simultaneous search/file requests by default, a 32-request queue and 64 connections; hold search slots through response writes. Explicit overload returns an error without local fallback.
- Add `refresh` CLI/RPC completion: reread eligible content and acknowledge searchable publication. Bound refresh work to one active and one pending pass; preserve deadline, failure and concurrent-edit semantics.
- Expose `refresh_index` to MCP and pi agents. Retain direct scans for `freshness: current`; reject incompatible binaries during agent installation.
- Preserve recreated files and directory discovery when delayed removal notifications arrive.
- Final 0.3.0 qualification is tracked separately from the retained 0.2.0 release evidence.

## 0.2.0

- Defer the reader's sorted path membership table until it is needed. Dense file-ID lookup remains immediate; concurrent first use initializes once. The 102-query Rust local workload improves about 1.26× against both tgrep and 0.1.0.
- Preserve CRLF source bytes, unterminated JSON lines and server JSON byte offsets.
- Admit matching hidden files under positive file type selections while retaining hidden-directory and ignore-rule filtering, in scan/local/server search and file listing.
- Reject unsupported `--crlf` matching with exit 2 instead of silently accepting it. `--no-crlf` selects the supported LF mode.
- Update crossbeam-epoch, anyhow, lru, memmap2 and notify dependencies; retain the advisory audit and rerun watcher/recovery tests.
- Add exact-output differential checks, bulk-edit/crash/polling recovery evaluation, concurrent load/resource measurements and verified cold-index-cache experiments. Package an operating guide and test the extracted binary against ripgrep.

Linux/WSL local CLI/service qualification is recorded in [PRODUCTION_REPORT.md](PRODUCTION_REPORT.md). No shared-service or native macOS/Windows qualification is claimed. Index format and asynchronous freshness contracts are unchanged.

## 0.1.0

Fork of Microsoft tgrep `56f8c62` (1.0.10), retaining its MIT license.

- Preserve mandatory nested alternatives in regex query plans. Queries such as `(foo|bar).*(baz|qux)` can intersect both groups instead of scanning the corpus.
- Consult posting cardinalities before allocating lists, stop on absent required grams, and probe mapped postings when very few candidates survive.
- Use existing modulo-eight position masks to reject impossible alignments within a literal. Keep separate literal runs independent across regex gaps.
- Fix indexed fixed-string Unicode case-insensitive false negatives, including Kelvin sign, long s, accented characters, and dotted I.
- Add `--explain` as a visible alias of `--stats`, including posting-work counters for local and server searches. This executes the search normally.
- Use `.zgrep/` for indexes and service discovery. Install as `zgrep-code` to avoid the Unix gzip search command. Agent installation checks capabilities before accepting a binary from PATH.
- Provide reproducible output-checked benchmarks, generated candidate-soundness tests, and a retained sparse-gram experiment. The sparse prototype is not enabled in production.

The on-disk posting format is unchanged. The fork's directory and process identity are separate from upstream. Index freshness, file eligibility, and exact match verification retain their inherited contracts; see [operations](docs/OPERATIONS.md).
