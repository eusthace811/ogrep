# Production readiness work in progress

Goal: proceed until the release is production ready. The 0.1.0 artifacts and their evidence are retained as a baseline; they do not automatically satisfy this new release audit.

The existing product scope is a local exact-search CLI and agent service, initially validated on Linux/WSL. The user asked to proceed without choosing a broader platform scope, so validation continues for Linux/WSL local use. No claim of native macOS/Windows qualification or shared multi-user service readiness is made by Linux tests.

## Release gates

| Gate | Required evidence | Status |
| --- | --- | --- |
| Scope and operating contract | Supported platforms, file eligibility, output/error semantics, freshness, resource and service-access boundaries documented | Existing contracts; new scope preference pending |
| Measured improvement | Preserve 0.1.0 and tgrep binaries; freeze corpus identities; evaluate startup optimization against both with output checks and regressions retained | In progress: lazy path ordering prototype |
| Core correctness | Complete workspace tests; generated planner checks; new concurrent lazy initialization/close checks; corruption and atomic-publication behavior preserved | Targeted reader checks passed; full candidate suite pending |
| Broader CLI semantics | Independent exact-output checks for lines, counts, JSON, quiet, context, filters, multiple patterns and Unicode | Pending candidate evaluation |
| Update and recovery | Existing recovery suite plus real-server edits, bulk mutations, polling fallback, restart/crash and queries during churn | Small native-watcher baseline exists; candidate stress pending |
| Load and resources | 1/4/16 concurrent clients with retained latency samples and errors; indexing and process memory/CPU measurements | Pending |
| Cache/startup | Warm local/server and restarted-server measurements; genuine cold-cache measurement where an isolated environment can establish it, or explicitly unresolved cold-cache gate | Pending |
| Dependencies and tooling | Locked builds, dependency advisory review, format/Clippy, agent and installer tests | Pending candidate checks |
| Release delivery | Versioned artifact, checksum, exact source/binary provenance, extracted-binary smoke tests and usable operating/recovery guide | 0.1.0 retained; candidate package pending |
| Final audit | Every adopted gate matched to current evidence; limitations and unresolved items reported; clean committed tree | Not complete |

Passing a startup microbenchmark is not enough to accept the optimization. The next end-to-end comparison must show a useful improvement on the independently derived local workload without losing matching results. First access to the deferred structure, live overlays, and publication must remain correct under concurrent use.

Benchmark snapshots are separate detached worktrees so the user's normal corpus checkouts can advance without changing the benchmark inputs. Index metadata must identify the same snapshot root and revision. No runtime test should use a stale index simply because its files exist.

Research features in `NEXT_EVALUATION.md` (structural navigation, ranked discovery, alternative index formats) remain hypotheses, not automatic release requirements. They should enter the release only when they address a measured product need and have their own correctness and cost evidence. The production goal does require completing the release gates above rather than relabeling experimental work as ready.

## Output parity findings

The new independent 81-check scan/local/server comparison retained its initial 45 failures in `evidence/production/output-modes-before-crlf-fix.json`. Matching previously stripped CR bytes from CRLF input; JSON also invented a final newline for unterminated input, and server JSON omitted source positions. The fixes retain CR as source content, reconstruct only existing terminators, and request position details for server JSON. Targeted CLI tests pass. The old maximum-column test normalized CRLF in its expected output; its expectation now preserves the source CR, with its column-limit assertions retained.

`output-modes-crlf-fixed.json` retains the remaining three failures: positive type selection must admit matching hidden files while respecting hidden ancestor directories and ignore rules. This is a separate scope fix still in progress. Passing targeted tests does not complete the release gates.

The hidden-file/type correction now passes 96 independent checks across scanning, local indexing and the service (`output-modes-type-fixed.json`). The earlier candidate-count regression expected exclusion of `.hidden.rs` under `-t rust`; expected counts now include that real matching file while preserving the raw-candidate, size-limit and backend parity assertions. The service concurrency test now fails on indexing timeout instead of continuing against a partial index.

Dependency audit found patched upstream issues in [crossbeam-epoch](https://rustsec.org/advisories/RUSTSEC-2026-0204.html), [anyhow](https://rustsec.org/advisories/RUSTSEC-2026-0190.html), [lru](https://rustsec.org/advisories/RUSTSEC-2026-0253.html), and [memmap2](https://rustsec.org/advisories/RUSTSEC-2026-0186.html). These are updated to patched versions; the full before/after audit records are retained. notify 8.2 replaces notify 7 to remove its dependency on [unmaintained instant](https://rustsec.org/advisories/RUSTSEC-2024-0384.html); the full watcher and recovery suite must pass again for that upgrade.

Candidate 0.2.0 passes the updated dependency audit with zero vulnerabilities and zero warnings (`dependency-audit-final.json`). All 15 independent recovery checkpoints passed across native watching, explicit polling and budget-triggered polling fallback; 4,501 concurrent queries completed during bulk edits/ignore churn. These include SIGKILL, 50 edits across the crash boundary and restart (`recovery-candidate.json`). This is a controlled fixture, not a filesystem-wide freshness guarantee.

The first 0.2.0 candidate (`8405e472...`) passed all qualification commands, 6,804 timed output checks and 252 independent scan comparisons. Broad local Rust speedup is 1.262× versus tgrep and 1.257× versus 0.1.0; broad server speed is effectively unchanged. The 12-query CPython server workload is 0.949× versus 0.1.0. Before freezing the artifact, an experiment will remove redundant traversal-filter compilation from indexed requests; that small workload regression is retained in the evidence rather than hidden.
