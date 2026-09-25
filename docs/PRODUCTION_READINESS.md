# Production readiness work in progress

Goal: proceed until the release is production ready. The 0.1.0 artifacts and their evidence are retained as a baseline; they do not automatically satisfy this new release audit.

The existing product scope is a local exact-search CLI and agent service, initially validated on Linux/WSL. A platform/scope preference has been requested; work common to all platforms continues while that is unresolved. No claim of native macOS/Windows qualification or shared multi-user service readiness is made by Linux tests.

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
