# Service admission evaluation

Runtime source: `c7a5a28`. Binary SHA-256: `d1d6a27fbaeeb7123a0f24ca7c2a005de78859fb83e03f4e98360c2e4d593335`. Baseline: the preceding handle-reuse binary, which already supports compact replies but has unbounded connection/search admission. This isolates admission from the earlier response-format improvement.

## Behavior

The default admits four simultaneous search/file-list requests and queues at most 32 more in FIFO order. A permit covers matching, response construction, transmission and response disposal. Status/control methods bypass the search queue. Connections are capped at 64, request frames at 1 MiB, and queued searches have a 30-second admission deadline. See [RPC limits and error behavior](RPC.md#service-admission-and-overload).

`--search-concurrency 1..32` controls admission independently of `-j`, which controls matching workers. We keep four as the default based on the memory and tail-latency tradeoff below. Eight is available when a caller prefers less queueing for small requests and accepts greater memory use.

## Evaluation

Each condition uses the frozen Rust corpus, four matching workers, six mixed file-list patterns, compact replies, one loopback connection per search, warm caches and exact file-set validation against independent ripgrep scans. Status is probed every 50 ms in a separate client thread; its observed latency includes client scheduling and parsing. Raw samples, binary hashes, corpus revisions, CPU and RSS are retained under `evidence/next-release/admission-*`.

The initial sweep ran 1,024 requests at each of 1/4/16 clients for unbounded, limit-four and limit-eight conditions: **9,216 requests**, all correct. Its fixed condition order is a limitation, so the 16-client condition was repeated three times per configuration with condition order shuffled within each trial: **9,216 more requests**, all correct. The repeat [command manifest](../evidence/next-release/admission-repeat-run.json) and [summary](../evidence/next-release/admission-repeat-summary.json) retain all outcomes.

The table gives medians across those three independent 16-client runs. These are descriptive measurements from one workstation, not confidence intervals over other workloads or hardware.

| Condition | Requests/s | Median latency ms | p95 ms | p99 ms | Peak server MiB | Status p95 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Unbounded baseline | 215.5 | 46.4 | 224.6 | 273.2 | 1034.8 | 74.4 |
| Four active | 208.7 | 62.0 | 161.0 | 192.9 | 723.7 | 29.4 |
| Eight active | 215.9 | 47.1 | 220.2 | 270.4 | 1009.6 | 71.6 |

Four active requests reduced median peak RSS by about **30%**, p95 latency by **28%**, and p99 by **29%**, with approximately **3% lower throughput**. Median latency increased by about **34%**: small requests now queue behind earlier work rather than bypassing concurrent broad searches. Server CPU per 1,024 requests was 55.44 seconds versus 54.63 for unbounded admission, so CPU savings are not claimed for the limiter. Eight recovered the throughput/median tradeoff but retained most of the memory and tail cost.

The initial 1/4-client results show no substantial throughput penalty at or below the limit. In all observations, active requests and queue lengths stayed within their advertised limits. Status requests all succeeded; the worst status latency across the three limit-four repeat runs was 48.96 ms. A status request still needs a connection slot: this is not a guarantee of control access through an exhausted connection cap.

Peak RSS is a process high-water mark. In the initial sweeps it is cumulative through the 1/4/16-client sequence; each repeat starts a fresh process and measures only the 16-client load after warmup. The Python generator approaches one CPU core at this load, so requests/s should not be interpreted as isolated server capacity. The caps bound concurrent and pending work, not the memory of one arbitrary result set.

## Correctness and resource tests

The all-feature workspace suite passes **962 tests**, with one existing polling test ignored by the default suite. All-target/all-feature Clippy and formatting pass; all **96 output/exit comparisons** against ripgrep pass.

New tests cover FIFO queue order, queue exhaustion, timeout removal, permit release, status access while search admission is occupied, response ownership until disposal, bounded unterminated frames, a client that stops reading and then disconnects, the actual 64-connection cap and slot reuse, invalid CLI concurrency values, and explicit overload propagation through both content-search and file-list CLI paths. The CLI does not convert an explicit overload response into local fallback work. Logs are retained as `admission-tests.log`, `admission-clippy.log` and `admission-output.json`.

This completes the admission experiment, not qualification of the next release. Refresh completion, agent integration and the final combined release checks remain outstanding.

The 0.3.0 release review also checks the adapter’s preliminary status request: an explicit `-32002` there raises a retryable tool error before any service launch or local scan. The regression test covers null-id connection rejection and ordinary request ids across search, file lookup and refresh.
