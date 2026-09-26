# Experiments after 0.2.0

These are development experiments, not qualification of a new release. The production baseline and its evidence remain in `../release-0.2.0/`.

## Compact file-list RPC

Source rationale and primary references: [research report](../../docs/RESEARCH_NEXT.md). Protocol: [RPC extensions](../../docs/RPC.md).

- `rpc-baseline.json`: preserved 0.2.0, 6 warmups + 54 timed requests, ordinary file listings, independent ripgrep file sets on every request.
- `rpc-compact.json`: experimental binary, 12 warmups + 108 timed requests, random interleaving of legacy and compact response formats within nine rounds. Both use the same server/index/matcher. Every request checks its complete file set against ripgrep. The sibling `.server.log` files retain phase traces.
- `compact-workspace.log`: full workspace tests, including new legacy/compact protocol tests; 953 passed, one existing polling test ignored by the default suite.
- `compact-output.json`: 96 independent output/exit checks against ripgrep in scan/local/server modes, zero mismatches.
- `compact-clippy.log`: all-target workspace Clippy with warnings denied. Formatting was also checked.

For `fn main`, compact replies cut median bytes from 4,858,576 to 1,891,775 (61%) and median receive-plus-decode latency from 152.14 to 71.86 ms (2.12x). JSON decode fell from 43.97 to 7.62 ms. These are **one-query direct-RPC diagnostics**, not the 102-query CLI score and not an improvement to local indexed search. Existing CLI clients already request less detail than legacy default RPC, so their gain may be smaller.

Other nonempty-query median ratios were 1.09x–1.49x. The absent query's median worsened from 0.60 to 0.81 ms, despite identical 110-byte replies; retain this unfavorable observation rather than discarding it. Nine rounds on one host are insufficient to characterize small differences in scheduler/connection overhead. The full CLI comparison and load/resource qualification are separate required evaluations before a release claim.

### Broader CLI confirmation

The subsequent `compact-broad-server.json` compares tgrep 1.0.10, zgrep 0.2.0 and the candidate on all 102 upstream-derived Rust queries. Nine randomized interleaved rounds produced **2,754 timed process-per-query runs**, with complete sorted output and exit checks on every run. `compact-broad-scan-verification.json` independently verifies all 102 query outputs against fresh ripgrep scans, binary identities, the clean corpus revision, sample completeness and server routing.

| Baseline | Geometric mean of per-query median speedups | Faster medians | Per-query 95% intervals entirely above/below 1 |
| --- | ---: | ---: | ---: |
| tgrep 1.0.10 | 1.116x | 96/102 | 58 / 0 |
| zgrep 0.2.0 | 1.111x | 92/102 | 59 / 0 |

The intervals use 5,000 paired-round bootstrap resamples without multiple-comparison correction; they are descriptive run-variability estimates. Ten query medians were worse than 0.2.0, although none had an interval wholly below 1. All per-query results are retained in `compact-broad-vs-tgrep.json` and `compact-broad-vs-020.json`. The CLI gain is substantially smaller than the 2.12x single-query direct-RPC example, as expected from the CLI's existing request for reduced match detail.

Measured runtime source: `4d8bb58`. Candidate binary SHA-256: `026a02498855ef9cc897d8ddcca79c93a92fd2d93a5c3719b3a4c8cb2a4bf373`. The existing 1.267x local-search result describes 0.2.0 versus tgrep; this patch changes the server path and does not establish a new local-search score. Service load/resource measurements, further local verification work and refresh completion remain required before qualification of the next release.

### Concurrent RPC follow-up

`compact-load-020.json` and `compact-load-candidate.json` retain fresh runs of 1,024 mixed requests at each of 1/4/16 clients, with independent ripgrep file-set checks for all **6,144 requests** and zero errors. Both clients request the same optional compaction flag; 0.2.0 ignores it. Four matching workers, one connection per request and warm caches match the earlier load setup.

| Clients | Requests/s, 0.2.0 → candidate | p95 ms, 0.2.0 → candidate | Server CPU seconds per 1,024 requests, 0.2.0 → candidate | Server peak MiB, 0.2.0 → candidate |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 32.0 → 59.6 | 144.6 → 66.9 | 64.75 → 51.91 | 530.0 → 529.4 |
| 4 | 73.3 → 153.0 | 238.3 → 99.7 | 67.19 → 51.39 | 871.7 → 704.2 |
| 16 | 84.0 → 207.7 | 680.4 → 239.5 | 68.90 → 53.83 | 1640.3 → 1121.8 |

This is one sequential load sweep per binary, with 0.2.0 run first; it is not a repeated interleaved throughput experiment. Peak RSS is the process high-water mark, cumulative through the 1/4/16-client sequence. Both are material limits on interpretation.

At 16 clients the Python generator consumed approximately 1.12/1.05 CPU cores respectively. Its peak RSS fell from 2,569,808 to 138,412 KiB because compact responses require far fewer Python objects. Thus the 2.47x end-to-end throughput ratio includes a major client benefit and **does not establish a 2.47x increase in server capacity**. The independent server CPU and RSS reductions support real server savings as well. More than 1 GiB server peak RSS remains at 16 clients: compaction does not replace the planned admission control or prove a bounded-resource service.

Reproduce the broader comparison and independent validation:

```bash
python3 scripts/evaluate.py \
  --engine upstream=../build/baseline/release/tgrep \
  --engine release=../build/baseline/zgrep-0.2.0 \
  --engine fork=target/release/zgrep \
  --corpus ../build/corpora-pinned/rust --queries evidence/queries-upstream-rust.json \
  --work ../build/next-eval-rust --mode server --rounds 9 \
  --expected-revision 1a8fa555801329bd0e803d7384b5a21191c61f30 \
  --output /tmp/compact-broad-server.json
python3 scripts/verify-comparison.py /tmp/compact-broad-server.json --output /tmp/scan-check.json
python3 scripts/summarize-evaluation.py /tmp/compact-broad-server.json --output /tmp/vs-tgrep.json
python3 scripts/summarize-evaluation.py /tmp/compact-broad-server.json --baseline release --output /tmp/vs-020.json
```

Reproduce the format comparison:

```bash
python3 scripts/profile-rpc.py --binary target/release/zgrep \
  --corpus ../build/corpora-pinned/rust \
  --index ../build/production-eval-rust/fork/index \
  --expected-revision 1a8fa555801329bd0e803d7384b5a21191c61f30 \
  --compare-formats --rounds 9 --output /tmp/rpc-compact.json
```
