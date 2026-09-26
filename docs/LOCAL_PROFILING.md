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
