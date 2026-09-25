# Operating zgrep 0.1.0

This release was built and exercised on Linux x86-64. The repository includes inherited Linux/macOS/Windows CI jobs, but this evaluation did not run remote CI or certify macOS/Windows packages. A local Linux release archive and checksum can be produced with `python3 scripts/package.py` from a clean checkout. It builds the locked dependencies, smoke-tests the packaged binary, and records source and binary identities in `BUILD.json`. No archive is published automatically by that command.

## Install and use

Build with a Rust toolchain supporting edition 2024 and the locked dependencies. `make install` installs `~/.cargo/bin/zgrep-code`; put that directory on PATH. For a packaged build, verify the archive with its adjacent `.sha256` file, unpack, and copy `zgrep-code` to a directory on PATH. Do not overwrite the system `zgrep` gzip utility.

```sh
zgrep-code index /path/to/repository
zgrep-code serve /path/to/repository
# In another terminal:
zgrep-code -F --explain -- 'some_identifier' /path/to/repository
```

`serve` runs in the foreground and watches for changes unless `--no-watch` is passed. Stop it with Ctrl-C. Searches discover it through the index's `serve.json`, or use an on-disk index when no server is available. `zgrep-code status /path/to/repository` reports service and indexing state.

## Freshness and scope

An on-disk index reflects its last publication. A watcher is asynchronous; an immediate post-edit query can precede its update. Completion or hidden-coverage status is not proof of current contents. Use `--no-index` when the search must read current eligible files, or rebuild a quiescent repository with `index`. No operation promises an atomic snapshot of a tree being changed concurrently.

Default eligibility applies ignore rules, hidden/binary policies, supported extensions, and size limits. A query with no match is evidence about that scope, not every byte in the directory. `--files` shows eligible paths. `-L`, `--one-file-system`, and `--ignore-file` only affect scanning: combine them with `--no-index`. See [agent guidance](../AGENTS.md) for additional fallback flags and output differences from ripgrep.

The index is disposable derived data. Stop the server before removing the exact repository's `.zgrep` directory, then rebuild it if repair is needed. Keep `.zgrep/` out of Git. The fork uses a separate directory from upstream `.tgrep/`; initial fork setup builds its own index. These planner changes do not require a new posting format.

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

The local release gates cover the complete Rust workspace, the separate ignored polling measurement, example tests, formatting, Clippy, agent integrations, installer checks, real CLI/server smoke tests, and output parity on frozen corpora. Raw records and performance limits are in [BENCHMARKS.md](../BENCHMARKS.md). This does not establish universal performance, zero defects, or support for arbitrary dependency/toolchain updates. Cold-cache latency, production-scale memory profiles, and watcher behavior on other operating systems remain unmeasured here.

The old upstream signing workflow is archived under `docs/upstream/`; it depended on Microsoft's infrastructure. There is no new hosted download repository or signed release. Download installers require an explicit `ZGREP_REPO`. Local installation and packaging need neither an account nor a publishing step.
