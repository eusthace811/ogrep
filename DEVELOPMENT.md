zgrep development record

The imported baseline is Microsoft tgrep commit `56f8c62`, workspace version 1.0.10. The original checkout is retained outside this repository for comparison. The first Git commit here records its source before fork changes.

Acceptance gates:

- Exact search must preserve results on the same frozen corpus and eligibility rules; optimizations may admit extra candidates but must never drop a real match.
- Keep raw benchmark samples, corpus revisions, query definitions, binary hashes, machine information, and reproduction commands. Compare matched outputs before timing; randomize execution order and include broad, short, absent, literal, and regex queries.
- Separate warm server, process-per-query, planning, indexing, and end-to-end costs. Report regressions and limitations alongside improvements.
- Run workspace tests, formatting, Clippy, integration/installer tests, and release smoke checks. Record unsupported or untested platforms explicitly.
- Commit each coherent experiment and validation milestone. Do not turn one promising microbenchmark into a universal performance claim.

Initial experiment sequence: instrument and benchmark the baseline; lazy posting lookup and sparse probes; preserve Boolean constraints; positional mask filtering if evidence warrants it; then evaluate whether a new index format is justified.

Research leads and reasoning are retained in [docs/RESEARCH.md](docs/RESEARCH.md). Measured results, sources, and reproduction commands are in [BENCHMARKS.md](BENCHMARKS.md). Existing index semantics and freshness limitations remain part of the contract until explicitly changed and tested.


Completed decisions for 0.1.0:

- Retain lazy cardinality planning and sparse posting probes: deterministic work reductions, especially absent and rare literals. CLI startup and file verification often hide the microbenchmark benefit.
- Retain Boolean conjunctions of alternative subplans: large verified improvements on the nested-alternation cases in two corpora, with no DNF expansion or storage change.
- Retain positional masks within individual literal runs: fewer false-positive candidates using metadata already stored. Never join runs across regex gaps. Generated exact-match checks cover both case modes.
- Fix inherited Unicode fixed-insensitive false negatives. This correctness improvement is useful independently of speed.
- Reject the tested sparse-gram index for production: roughly three times as many postings with little or no candidate reduction on the sampled workloads. Keep its source, upstream attribution, property test, and raw results for future experiments.
- Expose real work through `--explain`; use the collision-safe installation name `zgrep-code`; verify agent installers reject the system gzip utility.
- Retain existing freshness semantics explicitly. Refresh acknowledgements, syntax-aware context, SCIP, ranked discovery, compression, and worktree sharing remain research directions. They need separate workload evidence and are not implemented by this release.

Validation uses the complete inherited regression suite plus new planner and CLI checks. A restored-mtime test required retrying writes until filesystem ctime actually advanced; its original stale-version assertions remain intact. Early restricted-environment failures and exploratory timings are preserved, with explanations in `evidence/README.md`. Final gate logs supersede those exploratory runs.

The local production scope is Linux exact-search CLI/server use with the documented eligibility and freshness contracts. A clean-source release package includes build identity and a smoke test of the extracted binary. Remote publication, signed distribution, cross-platform certification, cold-cache performance, and broad claims of beating ripgrep are outside the evidence established here.
