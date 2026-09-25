# Changes

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
