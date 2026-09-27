# 0.3.0 release evidence

The serial 20-stage `run.json` records the combined evaluation of runtime/agent source `37731c433c8593caeede2f2308d1ef361bc83116`. All release comparisons use binary SHA-256 `2fc6f9d5350821848d08c96ee278445fed15f198f093dcde2edb8f381e2b013a`. The [report](../../PRODUCTION_REPORT.md) retains per-query regressions, statistical limits, client parsing costs and refresh resource observations.

`refresh-stability.json` is a separate follow-up prompted by rising memory near the end of the original 15-pass `refresh-cost.json`. It preserves that run, uses the same binary, corpus and queries, and adds 60 passes plus warmup with 732 independent query comparisons. No builds, tests or other benchmarks ran alongside either experiment. Reproduce the follow-up with a new private work directory:

```sh
python3 scripts/evaluate-refresh-cost.py \
  --binary target/release/zgrep \
  --corpus ../build/corpora-pinned/rust \
  --expected-revision 1a8fa555801329bd0e803d7384b5a21191c61f30 \
  --index-source ../build/release-0.3.0-eval/rust/fork/index \
  --work ../build/NEW-STABILITY-WORK \
  --queries evidence/queries-rust.json --rounds 60 \
  --output evidence/NEW-STABILITY.json
```

`release-build.log` records the normal release build. Qualification explicitly points agent tests at that executable. Packaging receipts and the completion audit identify the exact extracted executable and archive separately from report-only commits. Earlier development and 0.2.0 evidence remain intact.
