# Refresh completion evaluation

Development evidence, 2026-09-27. Runtime source `0220002`, integrated agent source `4fc088d`; ordinary release-build binary SHA-256 `1e975f67d925cec6b5f2cdd3c9fd6b8c7cc8dbc6f322d1234ed516768c302138`. The binary still identifies as development version 0.2.0. This does not replace the previously qualified release or complete next-release qualification.

## Immediate correctness under load

[Evaluator](../scripts/evaluate-refresh-completion.py), [raw results](../evidence/next-release/refresh-completion.json), [run log](../evidence/next-release/refresh-completion-run.log). A disposable fixture contains 1,000 stable source files plus edited subjects. Native, polling and no-watch daemons are tested with zero and 16 concurrent search clients, four server search slots and four worker threads. Each condition includes three cycles of edit, create, rename, atomic replacement, delete, ignore/unignore, equal-size writes with restored mtime, subtree creation/recreation, binary filename discovery and hidden-file edits. Quiet conditions also include an offline edit and server restart.

All **225 successful receipts** were followed immediately by exact line-output and filename-set agreement with fresh independent ripgrep scans: **450 comparisons**, with no visibility polling retries. Content checks assert actual server routing. All **10,840 concurrent searches** matched an independently established, unchanged file set. All **599 status probes** succeeded; each condition observed an active refresh and search admission stayed within four slots.

| Watch mode | Quiet median refresh | 16-client median refresh | 16-client maximum refresh | Worst status probe, loaded |
| --- | ---: | ---: | ---: | ---: |
| Disabled | 16.55 ms | 24.49 ms | 36.78 ms | 17.88 ms |
| Native | 17.27 ms | 26.27 ms | 34.87 ms | 25.27 ms |
| Poll | 16.37 ms | 28.06 ms | 40.44 ms | 21.07 ms |

Timing includes the refresh CLI's startup, transport and receipt validation. Medians exclude initial index construction and restart. These are small-fixture observations on one Linux host, not confidence intervals or a large-repository performance claim. The load driver and CLI checks also consume CPU. Per-refresh server CPU deltas in loaded conditions include concurrent searches, so they cannot isolate refresh CPU cost. Peak RSS in the raw data is cumulative per daemon.

The preserved tgrep 1.0.10 binary returned `-32601` for the same `refresh` RPC method. That establishes that this explicit method is an extension; it is not a speed comparison or a claim that tgrep lacks other rebuild/scan operations. The fork's no-watch agent test additionally demonstrates the useful difference: a newly created marker is absent from indexed search before the receipt and present afterwards.

## Cost on the pinned Rust corpus

[Evaluator](../scripts/evaluate-refresh-cost.py), [raw samples](../evidence/next-release/refresh-cost-rust.json), [run log](../evidence/next-release/refresh-cost-rust-run.log). The corpus is the clean pinned revision `1a8fa555801329bd0e803d7384b5a21191c61f30`. The evaluator copies an independently guarded index into a private directory and does not edit the corpus. A no-watch daemon runs with four worker threads, one full refresh warmup and three timed unchanged-tree refreshes. Twelve targeted queries after every pass agree with independent ripgrep file sets and confirm server routing.

The tree contains 63,009 content-indexed files and 63,026 eligible content candidates including files classified as binary. Median refresh latency was **3,069.87 ms**, range **3,041.37–3,090.81 ms**. Median server CPU was **5.95 seconds** per timed refresh. Cumulative peak server RSS reached **2,121.20 MiB**; the worst status probe was **32.48 ms**. All 48 post-refresh query checks (warmup plus three timed passes) passed.

The implementation rereads and rebuilds postings for every eligible file on each explicit pass. Logs attribute roughly 50–60 ms to the metadata walk and 2.4–2.7 seconds to the streamed delta/build/merge phase. These broad log phases are diagnostic, not an isolated CPU profile. The explicit barrier is useful after batches of edits and before repeated indexed queries; it has a substantial cost compared with an ordinary selective search.

RSS rose across the short-run samples, so a fresh-daemon follow-up used one warmup and **15 timed passes**. [Raw stability samples](../evidence/next-release/refresh-stability-rust.json) and [log](../evidence/next-release/refresh-stability-rust-run.log) retain every pass. Median latency was **2,989.92 ms**, range **2,918.40–3,141.81 ms**, with **5.84 CPU seconds** median and **1,909.80 MiB** cumulative peak RSS. The peak stopped increasing for the last six timed passes; RSS after those passes ranged from about 1,403 to 1,455 MiB. All 192 query checks (warmup plus 15 passes) agreed with independent scans, and the worst status probe was **30.02 ms**.

This follow-up is observed stability over 16 passes, not a formal RSS bound or evidence of indefinite operation. The higher 2,121.20 MiB peak from the first process remains part of the result. Reusing content identities to avoid regenerating unchanged postings is a possible later optimization, provided every eligible file is still read and all publication/failure semantics remain intact. It is deferred from this release: the current barrier has a measured, explicit batch-operation cost.

## Remaining qualification

The CLI, RPC and agent paths have focused, live-daemon and load evidence. Final combined release gates, broader post-change comparisons, packaging and the completion audit remain required. The source report and raw unfavorable observations are retained; none is treated as an achieved release-wide speedup.
