# Benchmarks

All numbers were measured on an Intel i9-14900KS (32 threads) under WSL2, with
warm file caches, on the source of the Rust compiler (63,186 files, pinned at
commit `2c1a66d7db9`). The versions compared were ogrep 0.1.0, tgrep 1.0.10 and
ripgrep 15.2.0.

## 102-query benchmark

This is the benchmark tgrep publishes: `scripts/benchmark.sh` with
`scripts/benchmark-queries-rust.json`. It builds the index, starts
`ogrep serve`, runs each of the 102 queries once as a separate process, and then
runs the same queries with `rg -n`. Most of the queries are broad (`pub fn`,
`impl Debug`, `Vec<`), which is where an index helps least. Each figure is the
median of three runs:

| Tool | Total, 102 queries | Average per query |
|---|---:|---:|
| ripgrep | 3,719 ms | 36.5 ms |
| tgrep (client and server) | 6,035 ms | 59.2 ms |
| **ogrep (client and server)** | **3,027 ms** | **29.7 ms** |

Both indexes are 203.4 MB. Across the three runs, building one took 2.1–3.5 s
with ogrep and 2.1 s with tgrep.

## A targeted search

Specific searches are where an index pays off most. Here is `-F 'fn parse_header'`
against the local index, with no server, as the median of 20 runs:

| Tool | Files read | Time |
|---|---:|---:|
| ripgrep | every file | 43.0 ms |
| tgrep | 3 | 17.5 ms |
| **ogrep** | **3** | **3.4 ms** |

All three print the same two lines.

## Per-query comparison with tgrep

A stricter comparison timed every query separately, with five seeded,
interleaved rounds. It covered 264 queries in six workloads: 102 broad and 12
targeted Rust queries, plus 12 targeted queries on CPython. Each ran against the
local index and against the server. Every timed output was first checked
against a fresh ripgrep scan, and all 264 agreed.

The table gives the geometric mean of per-query speedups over tgrep:

| Workload | Local index | Server |
|---|---:|---:|
| Rust, broad (102 queries) | 3.70× | 1.93× |
| Rust, targeted (12 queries) | 4.82× | 2.13× |
| CPython, targeted (12 queries) | 2.23× | 1.30× |

Under concurrent load (1,024 server requests at each client count):

| Clients | ogrep req/s | tgrep req/s | ogrep p99 | tgrep p99 | ogrep peak RSS | tgrep peak RSS |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 108 | 30 | 31 ms | 160 ms | 524 MiB | 548 MiB |
| 4 | 195 | 72 | 83 ms | 289 ms | 524 MiB | 845 MiB |
| 16 | 232 | 83 | 222 ms | 837 ms | 580 MiB | 1,617 MiB |

A refresh of the unchanged tree takes about 0.5 s, because files whose content
has not changed are skipped.

The harness and raw results for this section are in the repository history, at
commit `8064145` (`evidence/ogrep-0.1.0/`).

## Where the speed comes from

- **Local searches** check candidate files in parallel, and `-l` stops reading a
  file at its first match. Patterns anchored with `^` or `$` are prescanned over
  the whole file. Opening the index borrows paths from one mapped table instead
  of allocating each one.
- **Server searches** serialize replies directly and filter candidates in
  parallel. Each file's first NUL offset is computed once, when it is cached.
  Every request and reply goes out in a single write with `TCP_NODELAY` set.
- **Query planning** plans each pattern separately. Mandatory alternatives are
  kept inside concatenations, posting lists are read in order of size, and
  position masks reject impossible literal alignments.

## Reproduce

```sh
make release
./scripts/benchmark.sh --queries scripts/benchmark-queries-rust.json --skip-build
./scripts/benchmark.sh --repo-path /path/to/repo --skip-build   # any repository you have
```

`scripts/benchmark.ps1` is the Windows equivalent. There are query sets for
Linux, Chromium, Gecko, Go, Kubernetes and Rust in `scripts/benchmark-queries-*.json`.
Each benchmark has a matching manual GitHub workflow under `.github/workflows/`.
To benchmark another binary, such as tgrep, pass `--ogrep-bin PATH`.

Microbenchmarks for query execution, trigram extraction and index building:

```sh
cargo bench -p ogrep-core
```
