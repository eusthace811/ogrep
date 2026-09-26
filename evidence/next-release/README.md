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

Reproduce the format comparison:

```bash
python3 scripts/profile-rpc.py --binary target/release/zgrep \
  --corpus ../build/corpora-pinned/rust \
  --index ../build/production-eval-rust/fork/index \
  --expected-revision 1a8fa555801329bd0e803d7384b5a21191c61f30 \
  --compare-formats --rounds 9 --output /tmp/rpc-compact.json
```
