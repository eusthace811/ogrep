# Operating ogrep 0.1.0

ogrep 0.1.0 is built and tested on Linux x86-64 and on Windows x86-64 (MSVC). Its performance was measured on Linux. The repository keeps zgrep's inherited Linux/macOS/Windows CI definitions, but they have not been run remotely for ogrep. `python3 scripts/package.py` produces a local Linux release archive and checksum from a clean checkout. It builds the locked dependencies, smoke-tests the packaged binary, and records source and binary identities in `BUILD.json`. That command does not publish anything.

## Install and use

Build with a Rust toolchain supporting edition 2024 and the locked dependencies. On Linux, `make install` installs `~/.cargo/bin/ogrep`; put that directory on PATH. On Windows, run `./x install` from Git Bash, which installs `ogrep.exe` into `~/bin`; see the README for WSL, corporate registries and offline builds. For a packaged build, verify the archive with its adjacent `.sha256` file, unpack, and copy `ogrep` to a directory on PATH.

```sh
ogrep index /path/to/repository
ogrep serve /path/to/repository
# In another terminal:
ogrep -F --explain -- 'some_identifier' /path/to/repository
```

`serve` runs in the foreground and watches for changes unless `--no-watch` is passed. Stop it with Ctrl-C. Searches discover it through the index's `serve.json`, or use an on-disk index when no server is available. `ogrep status /path/to/repository` reports service and indexing state.

## Freshness and scope

An on-disk index reflects its last publication. A watcher is asynchronous; an immediate post-edit query can precede its update. Completion or hidden-coverage status is not proof of current contents. Run `ogrep refresh /path/to/repository --timeout-secs 30` and wait for success before indexed searches that must see completed edits. It requires the running server for the exact root and index path, rereads eligible content and applies the server’s scope settings. `--json` prints the receipt; deadlines accept 1–300 seconds. Timeout, incomplete reads/publication, invalid receipts and unavailable/old servers return exit 2 without fallback. Server work may continue after timeout or cancellation. Concurrent mutations may or may not appear; covered edits must remain stable through completion. Use `--no-index` for a direct eligible-file scan, or rebuild a quiescent repository with `index`. No operation promises an atomic snapshot of a tree being changed concurrently.

Default eligibility applies ignore rules, hidden/binary policies, supported extensions, and size limits. A query with no match is evidence about that scope, not every byte in the directory. `--files` shows eligible paths. `-L`, `--one-file-system`, and `--ignore-file` only affect scanning: combine them with `--no-index`. See [agent guidance](../AGENTS.md) for additional fallback flags and output differences from ripgrep.

The index is disposable derived data. Stop the server before removing the exact repository's `.ogrep` directory, then rebuild it if repair is needed. Keep `.ogrep/` out of Git. The fork uses a separate directory from upstream `.tgrep/`; initial fork setup builds its own index. These planner changes do not require a new posting format.

## Explain output

`--explain` performs the real search and writes diagnostics to stderr without changing matching stdout. It includes candidate/scan information and `Index work: {...}`:

| Counter | Meaning |
| --- | --- |
| `lists_loaded` | Full posting lists requested |
| `lists_probed` | Posting lists probed for surviving candidate IDs |
| `postings_copied` | Entries returned from list loads and probes |
| `candidate_probes` | IDs requested from probes |
| `position_rejections` | Candidates rejected by incompatible position residues |

These are work counts, not bytes read or allocations. Probes inspect mapped entries not counted in `postings_copied`. Broad queries and short literals can still require extensive scanning. `--stats` remains supported; server diagnostics identify `(via server)`.

## Validation and release limits

The local release gates cover the complete Rust workspace, the separate ignored polling measurement, example tests, formatting, Clippy, agent integrations, installer checks, real CLI/server smoke tests, and output parity on frozen corpora. Raw records and performance limits are in [BENCHMARKS.md](../BENCHMARKS.md). This does not establish universal performance, zero defects, or support for arbitrary dependency/toolchain updates. The combined release evaluation includes broad/targeted comparisons against tgrep and 0.2.0, cold-index pages, concurrency/RSS, indexing resources, restarts, immediate refresh parity and repeated refresh cost. See [the release report](../PRODUCTION_REPORT.md) for the current qualification status. Whole-machine cold caches and watcher behavior on other operating systems remain unmeasured.

The old upstream signing workflow is archived under `docs/upstream/`; it depended on Microsoft's infrastructure. There is no new hosted download repository or signed release. Download installers require an explicit `OGREP_REPO`. Local installation and packaging need neither an account nor a publishing step.

## Compatibility limits

Default LF matching preserves CR bytes in CRLF source lines, including JSON offsets and unterminated content. `--crlf` previously did nothing despite changing matching semantics in ripgrep. The CLI rejects it with exit 2; use ripgrep when CRLF-aware anchors/terminators are required. `--no-crlf` selects the supported default. This is an explicit compatibility limit, not full ripgrep parity. Positive file type selections admit matching hidden files, but hidden ancestor directories and ignore rules still apply.

## Service access and resources

The server is a trusted local tool. It binds to loopback and does not provide authentication or multi-user authorization. Other local processes can send search, reload and refresh requests; do not forward its port or expose it as a shared service. Run it under the account that owns the repository and protect the repository and index with normal filesystem permissions.

Use `-j 4` to bound the parallel search worker pool. `serve --search-concurrency 4` is the default request limit (accepted range 1–32), held through response writes. At most 32 searches wait for up to 30 seconds; the service accepts at most 64 connections and request frames up to 1 MiB. Socket reads/writes have 30-second idle timeouts. Explicit overload returns `-32002`; updated CLI clients exit 2 without local fallback. Status and refresh bypass the search queue but share the global connection cap. Refresh has one active and one pending pass, with later callers never joining a pass already started. Per-query output and process RSS remain separately unbounded by admission. More clients can increase queueing and memory use. `--max-memory` controls initial index-building flush decisions, not a hard process RSS limit. Broad results, mapped indexes and cached contents consume additional memory. Operating-system service limits can enforce deployment-specific budgets.

For polling-only filesystems, use `serve --watch-mode poll --poll-interval 1` when one-second reconciliation cadence is appropriate; the interval begins after the previous pass finishes. Native mode automatically falls back when watch registration or the configured budget prevents reliable coverage. Check `status` for the active mode. Stop the process before manual index repair; after a crash, restart it and allow reconciliation to finish. Use an independent `--no-index` scan when verifying current files after recovery.

Full-tree refresh is intended for edit batches, not every keystroke. The release evaluation on the 63,009-file Rust corpus measured a 2.981-second median refresh and 2.48 GiB cumulative peak RSS across 15 passes with intervening searches. A separate 60-pass follow-up reached 2.97 GiB peak, holding that peak for its final 20 passes; final-20 post-query RSS ranged 2.05–2.50 GiB. These finite observations do not establish an indefinite memory bound. These are observations, not fixed deployment bounds; the final report records the measured release binary. `freshness: current` in agent tools still performs a direct scan. The agent `refresh_index` tool follows the explicit receipt contract and cancellation closes its waiting CLI, not the shared service.
