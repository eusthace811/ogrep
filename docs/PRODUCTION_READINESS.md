# Production readiness audit — 0.3.0

Scope: trusted local Linux/WSL CLI and agent service. The prior release and all its evidence remain archived under `evidence/release-0.2.0`. Native macOS/Windows and shared multi-user hosting are outside this qualification.

The measured executable is `2fc6f9d5350821848d08c96ee278445fed15f198f093dcde2edb8f381e2b013a`, from runtime/agent source `37731c433c8593caeede2f2308d1ef361bc83116`.

| Gate | Evidence | Status |
| --- | --- | --- |
| Operating contract | [Operations](OPERATIONS.md), freshness, eligibility, failures, admission, access and recovery | Documented |
| Broad-query verification | [Local profiling](LOCAL_PROFILING.md), read-handle reuse, [final report](../PRODUCTION_REPORT.md) | Passed: broad local 1.334× vs tgrep, 1.052× vs 0.2.0 |
| Service work and response costs | Compact replies, bounded admission, repeated [ablation](SERVICE_ADMISSION.md), [release load](../evidence/release-0.3.0/load.json) | Passed: 9,216 checked requests across all three engines; control probes and concurrency limits checked |
| Refresh completion | CLI/RPC/agent contract, focused failure/race tests, [immediate parity](../evidence/release-0.3.0/refresh-completion.json) | Passed: 225 receipts, 450 immediate comparisons and 11,028 concurrent searches in native/poll/no-watch modes |
| Refresh operating cost | [Pinned Rust cost](../evidence/release-0.3.0/refresh-cost.json), 15 timed passes and 192 follow-up comparisons | Passed for documented batch use: separate 60-pass follow-up adds 732 checked searches; roughly 3 GiB observed peak, no hard bound claimed |
| Search correctness and comparison | [Six-workload report](../PRODUCTION_REPORT.md), randomized checked timings, frozen corpora and baselines | Passed: 6,804 timings and 252 independent scan combinations; regressions retained |
| Workspace and tooling | [Qualification](../evidence/release-0.3.0/qualification.json), all features and targets | Passed: 979 workspace tests, separate polling/example checks, format and Clippy |
| Agent and installation | Qualification agent/installer logs; measured release binary used explicitly | Passed: 43 agent tests including real pi refresh and overload, 49 installer scenarios |
| Output and compatibility | [Output matrix](../evidence/release-0.3.0/output-modes.json), [Unicode](../evidence/release-0.3.0/unicode.json) | Passed: 96 exact output/exit cases, within documented compatibility limits |
| Mutation and recovery | [Recovery](../evidence/release-0.3.0/recovery.json), native/poll/fallback, churn, SIGKILL/offline edits/restart | Passed: 15 checkpoints and 4,301 concurrent queries |
| Cache, startup and indexing | [Cache](../evidence/release-0.3.0/cache.json), [startup](../evidence/release-0.3.0/startup.json), [resources](../evidence/release-0.3.0/index-resources.json) | Passed: 180 checks, 10 restarts; CPU/RSS/storage retained |
| Dependencies | [Recorded RustSec audit](../evidence/release-0.3.0/audit.log) | Passed: zero reported vulnerabilities/warnings; packaging repeat pending |
| Delivery and exact provenance | Clean-source archive, checksums, extracted-binary tests and identity audit | Pending |

The 20-command combined evaluation completed successfully. Packaging and the final source/artifact audit remain required before declaring release completion.

Only index-file pages were established cold with `mincore`; source/executable/metadata/device caches were not. The current server is loopback-only and unauthenticated. Admission bounds simultaneous work, not per-query output or total RSS. Refresh rereads content and covers completed edits stable through completion; it does not establish an atomic filesystem snapshot. Timeout and cancellation do not acknowledge freshness. Large-tree refresh cost is material and is reported rather than hidden.

The historical [0.2.0 audit](../evidence/release-0.2.0/PRODUCTION_READINESS.md) preserves earlier output fixes, dependencies and failures. Research ideas in [RESEARCH_NEXT.md](RESEARCH_NEXT.md) remain optional hypotheses unless adopted into a future goal; completing this release does not establish a speed ceiling.
