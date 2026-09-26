# Production readiness audit — 0.2.0

Goal: proceed until the release is production ready. The 0.1.0 artifacts and their evidence are retained as a baseline; they do not automatically satisfy this new release audit.

The existing product scope is a local exact-search CLI and agent service, initially validated on Linux/WSL. The user asked to proceed without choosing a broader platform scope, so validation continues for Linux/WSL local use. No claim of native macOS/Windows qualification or shared multi-user service readiness is made by Linux tests.

## Release gates

The measured executable is `45777bbc84a9043cf3de690b781cf623fa1806368ae377ae5de4386f5450351d`; all final evaluation records below refer to that hash.

| Gate | Evidence | Status |
| --- | --- | --- |
| Scope and operating contract | [Operations](OPERATIONS.md): Linux/WSL, eligibility, errors, freshness, local service access, resource controls and recovery | Passed for existing local-tool scope |
| Measured improvement | [Final report](../PRODUCTION_REPORT.md), 6,804 checked timings, frozen worktrees, retained baselines and regressions, 252 independent scan comparisons | Passed: 1.267× broad local speedup vs tgrep, 1.262× vs 0.1.0 |
| Core correctness | [Qualification](../evidence/release-0.2.0/qualification.json): 951 workspace tests, generated planner checks, concurrent lazy access/close, corruption and publication tests | Passed |
| Broader CLI semantics | [Output matrix](../evidence/release-0.2.0/output-modes-020.json): 96 exact-output/exit checks across scan/local/server | Passed within documented compatibility scope |
| Update and recovery | [Recovery](../evidence/release-0.2.0/recovery.json): 15 checkpoints, bulk churn, native/poll/fallback, SIGKILL/offline changes/restart, 4,296 concurrent queries | Passed |
| Load and resources | [Load](../evidence/release-0.2.0/load.json): 3,072 checked requests at 1/4/16 clients; [indexing](../evidence/release-0.2.0/index-resources.json): elapsed, CPU, RSS, storage | Passed; saturation and memory limits reported |
| Cache/startup | [Cache](../evidence/release-0.2.0/cache.json): 180 checks with mincore-verified cold index pages; [startup](../evidence/release-0.2.0/startup.json): 10 reconciled restarts | Passed for index-file cache; whole-machine/source/device-cold behavior unclaimed |
| Dependencies and tooling | [Audit](../evidence/release-0.2.0/audit.log): zero vulnerabilities/warnings; format, Clippy, polling/property, 34 agent and 49 installer tests | Passed |
| Release delivery | Version 0.2.0, local archive, checksum, exact source/binary provenance, extracted-binary checks and operating guide | Packaging pending |
| Final audit | Current artifact/source/evidence identities, all adopted gates, limitations and clean committed tree | Pending packaging |

The cold-cache boundary is explicit: only disposable index file pages were established cold. OS executable/metadata/device caches and the source corpus were not. This does not establish performance on every machine or filesystem. The local server is trusted and unauthenticated; shared multi-user hosting remains outside this product's release contract.

Research features in `NEXT_EVALUATION.md` remain hypotheses, not release requirements. Sparse-gram indexing was evaluated and rejected. Concurrency control, smaller file-list replies and refresh completion are supported next investigations, not claims about implemented behavior.

## Experiment history

The following records describe intermediate candidates; the table above identifies the final evidence.

## Output parity findings

The new independent 81-check scan/local/server comparison retained its initial 45 failures in `evidence/production/output-modes-before-crlf-fix.json`. Matching previously stripped CR bytes from CRLF input; JSON also invented a final newline for unterminated input, and server JSON omitted source positions. The fixes retain CR as source content, reconstruct only existing terminators, and request position details for server JSON. Targeted CLI tests pass. The old maximum-column test normalized CRLF in its expected output; its expectation now preserves the source CR, with its column-limit assertions retained.

`output-modes-crlf-fixed.json` retains the remaining three failures: positive type selection must admit matching hidden files while respecting hidden ancestor directories and ignore rules. This was a separate scope fix, completed in the subsequent records. Passing targeted tests does not complete the release gates.

The hidden-file/type correction now passes 96 independent checks across scanning, local indexing and the service (`output-modes-type-fixed.json`). The earlier candidate-count regression expected exclusion of `.hidden.rs` under `-t rust`; expected counts now include that real matching file while preserving the raw-candidate, size-limit and backend parity assertions. The service concurrency test now fails on indexing timeout instead of continuing against a partial index.

Dependency audit found patched upstream issues in [crossbeam-epoch](https://rustsec.org/advisories/RUSTSEC-2026-0204.html), [anyhow](https://rustsec.org/advisories/RUSTSEC-2026-0190.html), [lru](https://rustsec.org/advisories/RUSTSEC-2026-0253.html), and [memmap2](https://rustsec.org/advisories/RUSTSEC-2026-0186.html). These are updated to patched versions; the full before/after audit records are retained. notify 8.2 replaces notify 7 to remove its dependency on [unmaintained instant](https://rustsec.org/advisories/RUSTSEC-2024-0384.html); the final full watcher and recovery suite passes with that upgrade.

Candidate 0.2.0 passes the updated dependency audit with zero vulnerabilities and zero warnings (`dependency-audit-final.json`). All 15 independent recovery checkpoints passed across native watching, explicit polling and budget-triggered polling fallback; 4,501 concurrent queries completed during bulk edits/ignore churn. These include SIGKILL, 50 edits across the crash boundary and restart (`recovery-candidate.json`). This is a controlled fixture, not a filesystem-wide freshness guarantee.

The first 0.2.0 candidate (`8405e472...`) passed all qualification commands, 6,804 timed output checks and 252 independent scan comparisons. Broad local Rust speedup is 1.262× versus tgrep and 1.257× versus 0.1.0; broad server speed is effectively unchanged. The 12-query CPython server workload is 0.949× versus 0.1.0. A subsequent experiment removed redundant traversal-filter compilation from indexed requests; that small workload regression is retained in the evidence rather than hidden.

The 25-round paired experiment supports deferring traversal-filter compilation: it improves the CPython server workload by 1.063× versus the first 0.2.0 candidate, with equal results. Against 0.1.0 it is 0.987× in that run (near neutral, not a claimed server speedup). The change preserves upfront type-name and glob validation; only filesystem walks compile the second ignore matcher. The final evaluation uses this exact executable throughout, in `evidence/release-0.2.0/`.
