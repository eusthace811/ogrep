# Work after 0.2.0

Baseline: release 0.2.0, binary SHA-256 `45777bbc84a9043cf3de690b781cf623fa1806368ae377ae5de4386f5450351d`, preserved outside the worktree at `../build/baseline/zgrep-0.2.0`. Keep the previous release and all its evidence intact.

The goal has three required tracks:

1. Profile broad local query verification. Separate index/filter work, reading/decoding, matching and output costs under ordinary query semantics. Experiment on the measured dominant costs, retaining complete result and error behavior, sorted output and resource bounds. Compare against 0.2.0 and tgrep on the frozen broad workload, including selective-query regressions.
2. Profile service response size, serialization/transport and concurrent work. Reduce demonstrated waste and bound expensive concurrent work without making status or refresh control unusable. Keep protocol compatibility explicit. Evaluate 1/4/16 clients with independently checked results, CPU, RSS, throughput and latency; preserve the baseline results.
3. Add refresh completion for CLI/RPC and agents. Define which completed edits are covered, timeout/failure behavior, ignored/removed/renamed paths, publication and concurrent mutations. A success response must follow actual searchable publication, not watcher receipt or an indexing-complete flag. Test no-watch/poll/native modes, concurrent refreshes and edits, restart and failures. Agent integration must use the new contract without weakening existing current-scan semantics.

Each accepted change needs a commit with tests and evidence. Finish with full workspace/agent/installer checks, independent output parity, broad/targeted comparisons and a report of improvements, regressions and remaining limits. Packaging, documentation and a completion audit must describe the resulting release. Research experiments do not substitute for any of the three tracks.

Implementation and combined evaluation:

- Track 1: [ordinary local profiling and handle reuse](LOCAL_PROFILING.md) isolate read costs and retain focused regression follow-up. Final broad local search is 1.334× faster than tgrep and 1.052× faster than 0.2.0 on this workload.
- Track 2: [compact replies](../evidence/next-release/README.md) and [bounded admission](SERVICE_ADMISSION.md) have compatibility, overload, slow-reader and load evidence. Final broad server-backed CLI search improves 1.119× versus tgrep. At 16 RPC clients, the combined changes deliver 193.6 requests/s versus 73.4, with 740.5 versus 1,606.7 MiB cumulative peak RSS. Admission-only experiments retain the throughput/median-latency tradeoff.
- Track 3: the [refresh contract](REFRESH_COMPLETION.md), RPC, CLI and agent integration are implemented. Final qualification includes 43 agent tests and 225 immediate refresh receipts with 11,028 checked concurrent searches. Full-corpus cost and extended memory observations are in the [release report](../PRODUCTION_REPORT.md).

All 20 combined evaluation stages passed against the same 0.3.0 executable. Packaging and the final [completion audit](PRODUCTION_READINESS.md) passed; all three goal tracks are complete within the documented Linux/WSL scope. The [research list](RESEARCH_NEXT.md) retains optional follow-up hypotheses; no experiment promises a further speedup.
