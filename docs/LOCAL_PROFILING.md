# Local verification profile

The optional `profile-search` Cargo feature adds local indexed search timers. Set `ZGREP_PROFILE_LOCAL=1` in that diagnostic build to emit a `[profile-local]` JSON record on stderr. Normal builds compile out clocks and thread-local records. This does not set `--stats`, change result limits or request additional match totals.

Build the diagnostic and ordinary binaries separately, preserving each before the next build:

```bash
cargo build --release -p zgrep-cli --features profile-search
cp target/release/zgrep /tmp/zgrep-profile
cargo build --release -p zgrep-cli
cp target/release/zgrep /tmp/zgrep-normal
```

Run `scripts/profile-local.py --help` for the corpus/index/query inputs. It requires a clean pinned corpus, matching index provenance and an index without a server descriptor. It compares full output and exit codes with independent ripgrep scans on every run, interleaves diagnostic and ordinary binaries, and retains hashes and raw timings. It does not time full-statistics searches as a substitute for ordinary `-l` behavior.

The phases are index opening/coverage/scope resolution, matcher and filter compilation, posting selection, path filtering/sorting, per-file path/size checks, reads, owned-buffer decoding, matching, verification/rendering, and the final output flush. Read time includes mmap attempts and validation of successfully mapped UTF-8 files. `verify_render_ms` includes `match_ms`; subtract matching to estimate the remaining output/bookkeeping work. Do not sum those two inclusive fields. Process latency also includes startup and teardown outside the local-search timer. Failed reads and early exits still close their active timing spans.

## Initial measurement

Source follows `68e6920` with only optional instrumentation. [Raw results](../evidence/next-release/local-phases-baseline.json): 12 targeted Rust queries, nine randomized rounds, 216 timed requests plus 24 warmups, all matching ripgrep. Full all-feature workspace tests and all-target Clippy pass in the sibling logs.

Representative median milliseconds:

| Query | Ordinary process | Index open | Path/size checks | Read | Decode | Matching | Verification/render, inclusive |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `fn` | 311.96 | 6.84 | 45.18 | 143.63 | 5.49 | 67.79 | 81.03 |
| `impl` | 151.35 | 6.84 | 19.24 | 61.83 | 3.28 | 42.27 | 47.89 |
| `Result` | 55.88 | 6.99 | 4.55 | 16.50 | 1.60 | 19.74 | 21.06 |
| absent literal | 9.83 | 6.92 | 0 | 0 | 0 | 0 | 0 |

Phase columns come from diagnostic runs; the first column is the separate ordinary binary. Instrumented/ordinary median ratios across the 12 queries ranged from 0.991 to 1.045; small negative apparent overhead is run variability, not an optimization. These are warm-cache diagnostic measurements on the recorded WSL workstation, not a speedup claim.

The next justified experiments are reducing file-open/read overhead and unnecessary match-location construction. Source inspection found that files below the 1 MiB mapping threshold are opened/stat'ed for a rejected mmap attempt, closed, then reopened by `std::fs::read`. Reusing that open handle is a smaller first experiment than adding threads. Matching is also substantial for broad successful queries, supporting a later existence-only verifier. Decode and postings are not the leading costs in these examples, so SIMD decoding or a new postings format is lower priority here. Selective and absent searches still spend much of their time opening the index.

## Handle reuse experiment

Implemented at `e9f4b57`: open once, attempt mapping using that handle, and read from the same handle if mapping is declined. Mapping does not advance its read position. Decoding, lossy source-offset repair and output semantics remain on their existing paths. This does not introduce parallel reads or retain multiple files' contents.

The [second diagnostic](../evidence/next-release/local-phases-single-open.json) measured the `fn` read phase at 102.86 ms versus 143.63 ms initially, and `impl` at 44.21 versus 61.83 ms. Those separate diagnostic runs are explanatory evidence; the following ordinary-binary comparisons are the performance evidence.

| Local workload | Queries | Candidate / tgrep speedup | Candidate / 0.2.0 speedup | Candidate / immediately preceding source speedup |
| --- | ---: | ---: | ---: | ---: |
| Rust broad, upstream-derived | 102 | 1.337x | 1.051x | 1.050x |
| Rust targeted | 12 | 1.787x | 1.037x | 1.026x |
| CPython targeted | 12 | 1.231x | 1.011x | Not included in main run |

Ratios are geometric means of per-query median latency ratios, above 1 meaning faster. Nine randomized interleaved rounds yielded **4,428 timed runs**. Every timed output and exit matched its expected result; [independent verification](../evidence/next-release/single-open-scan-verification.json) checked all 126 workload queries against fresh ripgrep scans, binary hashes, clean pinned corpora, complete sample matrices and indexed execution routes. The source immediately before handle reuse was `a10e72f`, built without profiling; it isolates this change from the earlier RPC and instrumentation additions.

The broad workload had 44 descriptive per-query 95% intervals wholly above 1 versus 0.2.0 and none wholly below; versus tgrep all 102 were wholly above 1. Intervals use 5,000 paired-round bootstrap resamples, with no multiple-comparison correction. These characterize variability on this host, not other machines or a population of all queries. [Versus tgrep](../evidence/next-release/single-open-vs-tgrep.json), [versus 0.2.0](../evidence/next-release/single-open-vs-020.json), [versus preceding source](../evidence/next-release/single-open-vs-before.json).

### Unfavorable results and follow-up

Three initial comparisons had intervals below 1: Rust `#[deny(` and `(struct|enum).*(TypeError|RegionError)` versus the immediately preceding source, and CPython `PyObject_GetOptionalAttrString` versus 0.2.0. The median penalties were about 0.91, 0.41 and 0.16 ms respectively. These results remain in the original records.

A separate **41-round follow-up** selected those queries plus common/absent controls before running with a different random seed. It retained **861 additional timed runs**, all independently checked. None of the three slowdowns had an interval wholly below 1 in the follow-up. For the CPython query the 0.2.0/candidate ratio was still 0.992 (interval 0.959–1.010); this is not evidence of a gain on that query. The common-query controls improved while absent-query controls were approximately unchanged. The follow-up does not replace the original aggregate score or prove an absence of small regressions. [Versus 0.2.0](../evidence/next-release/single-open-followup-vs-020.json), [versus preceding source](../evidence/next-release/single-open-followup-vs-before.json), [scan verification](../evidence/next-release/single-open-followup-scan.json).

The default workspace suite passed 953 tests, with one existing polling test ignored; all-target/all-feature Clippy and formatting passed. The output matrix passed all 96 scan/local/server checks. Logs and raw data use the `single-open-` prefix under `evidence/next-release/`.

Measured candidate binary SHA-256: `e7ddcadf9e9bf875776a663f137cd7aba937e6777dc5bdb59402f5aec6f9b5ce`; preserved at `../build/next-local-profile/zgrep-single-open`. The diagnostic and preceding binaries are preserved alongside it. The ordinary CLI measurements use warm OS caches on one WSL workstation. Full next-release qualification, service admission control and refresh completion remain outstanding.
