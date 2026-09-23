zgrep development record

The imported baseline is Microsoft tgrep commit `56f8c62`, workspace version 1.0.10. The original checkout is retained outside this repository for comparison. The first Git commit here records its source before fork changes.

Acceptance gates:

- Exact search must preserve results on the same frozen corpus and eligibility rules; optimizations may admit extra candidates but must never drop a real match.
- Keep raw benchmark samples, corpus revisions, query definitions, binary hashes, machine information, and reproduction commands. Compare matched outputs before timing; randomize execution order and include broad, short, absent, literal, and regex queries.
- Separate warm server, process-per-query, planning, indexing, and end-to-end costs. Report regressions and limitations alongside improvements.
- Run workspace tests, formatting, Clippy, integration/installer tests, and release smoke checks. Record unsupported or untested platforms explicitly.
- Commit each coherent experiment and validation milestone. Do not turn one promising microbenchmark into a universal performance claim.

Initial experiment sequence: instrument and benchmark the baseline; lazy posting lookup and sparse probes; preserve Boolean constraints; positional mask filtering if evidence warrants it; then evaluate whether a new index format is justified.

Research leads and reasoning are recorded in the sibling `../TGREP_RESEARCH.md`. Implementation experiments will cite their sources in the evidence report. Existing index semantics and freshness limitations remain part of the contract until explicitly changed and tested.
