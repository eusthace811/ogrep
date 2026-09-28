# ogrep

Indexed exact text and regex search for large codebases. ogrep is based on zgrep 0.3.0, which forked [Microsoft tgrep](https://github.com/microsoft/tgrep) at `56f8c62`. It keeps zgrep's index format, command line, server protocol and exact match verification, and removes the costs that dominated zgrep's measured profile. It also adds file-level conditions (`--with`, `--without`) and naming-convention-insensitive identifier search (`--ident`).

On zgrep's own evaluation harness (102-query Rust workload, targeted Rust and CPython sets, local and server-backed), ogrep 0.1.0 is faster than both on every workload. All 264 queries return output identical to fresh ripgrep scans:

| Workload | vs tgrep 1.0.10 | vs zgrep 0.3.0 |
|---|---:|---:|
| Rust broad, local | 3.70× | 2.77× |
| Rust broad, server | 1.93× | 1.72× |
| Rust targeted, local | 4.82× | 2.69× |
| Rust targeted, server | 2.13× | 1.52× |
| CPython targeted, local | 2.23× | 1.84× |
| CPython targeted, server | 1.30× | 1.17× |

Under 16 concurrent clients the server handles 3.5× tgrep's request rate at a quarter of its p99 latency and a third of its memory. Those numbers were measured on Linux. What changed is in [OGREP.md](OGREP.md). The production qualification against tgrep covers gates, recovery, refresh, load, startup, cache and dependencies, and is in [evidence/ogrep-0.1.0/PRODUCTION_READINESS.md](evidence/ogrep-0.1.0/PRODUCTION_READINESS.md).

## Install

### Linux

```sh
make release          # target/release/ogrep
make install          # ~/.cargo/bin/ogrep (put ~/.cargo/bin on PATH)
make test             # whole test suite
```

### Windows

Use `./x` from Git Bash. It builds `ogrep.exe` with the Windows MSVC toolchain:

```sh
./x fetch             # download every locked crate (shows the registry serves them all)
./x install           # release build, then ogrep.exe into ~/bin, already on Git Bash's PATH
hash -r
ogrep --version
```

`./x test` runs the test suite, and `./x help` lists the other commands. The same script runs from WSL, where it drives the Windows `cargo.exe` and keeps the build in `%USERPROFILE%\.ogrep-target`, because the WSL share cannot hold a Windows `target/`. Builds and installs from WSL work. Run the tests from a checkout on a Windows disk, though. Some tests create their fixtures inside the repository, and on the WSL share eight hidden-file tests fail: it has no file locks and no Windows hidden attribute. `./x install` refuses to run while `ogrep.exe` is running, since Windows will not replace a running executable. Stop `ogrep serve` first.

**On a corporate network**, cargo takes crates from whatever the machine's `~/.cargo/config.toml` substitutes for crates.io, usually an Artifactory Cargo remote. Nothing registry-specific is committed here. `./x doctor` shows which registry cargo will use and checks that every locked crate resolves. If it fails, it prints the config lines to add. To use a registry for one run only, set `OGREP_CARGO_REGISTRY=sparse+https://<host>/artifactory/api/cargo/<repo>/index/`, plus `CARGO_REGISTRIES_OGREP_MIRROR_TOKEN` if it needs a token. `./x vendor` copies every crate into `./vendor` for fully offline builds.

The Windows build uses a static C runtime, so `ogrep.exe` does not need the Visual C++ redistributable.

## Use

```sh
ogrep index /path/to/repository                   # build the index into ./.ogrep
ogrep serve /path/to/repository                   # optional: keep it warm and watch for changes
ogrep -F --explain -- 'some_identifier' /path/to/repository
ogrep -l --with async --without '#\[cfg\(test' -- Mutex .   # files using Mutex that are async and not test modules
ogrep -n --ident parseHeader .                    # parse_header, ParseHeader, PARSE_HEADER, parse-header, ...
```

Use `ogrep --help` for flags, [agent guidance](AGENTS.md) for integration semantics, and [operations](docs/OPERATIONS.md) for freshness, recovery and platform limits.

## Search contracts

Indexes and service discovery use `.ogrep/`. Ignore rules, supported file types, size limits, and binary/hidden policies determine the searchable corpus. The index is a prefilter; exact matching still verifies candidates.

An on-disk index reflects its last publication. The server's watcher is asynchronous. Use `refresh` and wait for success before indexed queries that must see completed edits, or `--no-index` for a direct scan of current eligible files. Refresh requires the running server and does not provide an atomic filesystem snapshot. Indexing-complete status is not a freshness guarantee. Add `.ogrep/` to the searched repository's `.gitignore`.

## Inherited from zgrep 0.3.0

zgrep's own changes over tgrep, all kept in ogrep:

- **Better regex plans:** retain mandatory alternatives inside concatenations instead of discarding useful constraints. zgrep measured the tested nested-alternation queries **2.9× faster on CPython and 13.5× on Rust** with local indexes.
- **Less posting work:** order lookups by cardinality, probe common lists for sparse candidates, and use existing position masks to reject impossible literal alignments.
- **Correct Unicode fixed searches:** indexed `-Fi` retains matches such as `kelvin` against `KELVIN`, which tgrep's planner could incorrectly exclude.
- **Explain actual work:** `--explain` performs the search and reports posting loads, probes, copies and positional rejections on stderr.
- **Refresh completion:** `ogrep refresh /path/to/repository` and the agent `refresh_index` tool wait for searchable publication of completed edits. See the [contract and evaluation](docs/REFRESH_EVALUATION.md).
- **Smaller replies and bounded work:** compact file listings, and four active searches with a bounded queue. See the [admission measurements](docs/SERVICE_ADMISSION.md).
- **More accurate output:** preserve CRLF bytes and unterminated lines, correct server JSON offsets, and honor positive type selection for hidden files.

zgrep's measurements and history are kept as they were written, under the zgrep name: [benchmark report](BENCHMARKS.md), [0.3.0 evaluation](PRODUCTION_REPORT.md), [tgrep comparison](TGREP_COMPARISON.md), [changes](CHANGELOG.md), [development decisions](DEVELOPMENT.md), [research](docs/RESEARCH.md) and `evidence/`. The [inherited command reference](docs/upstream/README.md) is tgrep's.

Original copyright and MIT license are retained in [LICENSE](LICENSE). Upstream performance and adoption claims in archived documents are not claims about this fork.
