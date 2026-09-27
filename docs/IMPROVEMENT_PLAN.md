# Work after 0.2.0

Baseline: release 0.2.0, binary SHA-256 `45777bbc84a9043cf3de690b781cf623fa1806368ae377ae5de4386f5450351d`, preserved outside the worktree at `../build/baseline/zgrep-0.2.0`. Keep the previous release and all its evidence intact.

The active goal has three required tracks:

1. Profile broad local query verification. Separate index/filter work, reading/decoding, matching and output costs under ordinary query semantics. Experiment on the measured dominant costs, retaining complete result and error behavior, sorted output and resource bounds. Compare against 0.2.0 and tgrep on the frozen broad workload, including selective-query regressions.
2. Profile service response size, serialization/transport and concurrent work. Reduce demonstrated waste and bound expensive concurrent work without making status or refresh control unusable. Keep protocol compatibility explicit. Evaluate 1/4/16 clients with independently checked results, CPU, RSS, throughput and latency; preserve the baseline results.
3. Add refresh completion for CLI/RPC and agents. Define which completed edits are covered, timeout/failure behavior, ignored/removed/renamed paths, publication and concurrent mutations. A success response must follow actual searchable publication, not watcher receipt or an indexing-complete flag. Test no-watch/poll/native modes, concurrent refreshes and edits, restart and failures. Agent integration must use the new contract without weakening existing current-scan semantics.

Each accepted change needs a commit with tests and evidence. Finish with full workspace/agent/installer checks, independent output parity, broad/targeted comparisons and a report of improvements, regressions and remaining limits. Packaging, documentation and a completion audit must describe the resulting release. Research experiments do not substitute for any of the three tracks.

Progress after the initial experiments:

- Track 1: [ordinary local phase profiling and handle reuse](LOCAL_PROFILING.md) are implemented and measured against both baselines, with focused regression follow-up. Further matching work is a possible later experiment; final release qualification is still required.
- Track 2: [compact replies](../evidence/next-release/README.md) and [bounded service admission](SERVICE_ADMISSION.md) are implemented and evaluated. The four-request default trades about 3% throughput for about 30% less peak RSS and lower tail latency in repeated 16-client runs; control probes succeeded. Final combined release checks remain required.
- Track 3: the [refresh contract and delayed-event prerequisite](REFRESH_COMPLETION.md) are documented and the prerequisite is tested. The content-refresh RPC, bounded queue and CLI are implemented with focused and real-daemon tests. Agent integration is implemented with 42 passing adapter/installer tests, including the real pi bridge. Workload evaluation and final qualification remain pending. A metadata-only reconcile or existing `reload` response cannot be relabeled as the promised barrier.
