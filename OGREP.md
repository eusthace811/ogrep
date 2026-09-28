# ogrep: what changed from zgrep 0.3.0

ogrep started as a clone of [zgrep](../zgrep) 0.3.0, itself a fork of Microsoft tgrep. It keeps zgrep's index format, CLI, RPC protocol and correctness contracts, and removes the costs that dominated zgrep's measured profile. Since 0.1.0 everything is named ogrep: the binary (`target/release/ogrep`, `ogrep.exe`), the crates (`ogrep-core`, `ogrep-cli`) and the per-repository index folder (`.ogrep/`). zgrep's history and measurement records keep the name they were written under. zgrep itself was not modified.

Measurement notes, raw results and helper scripts live in `lab/` (`lab/results/`, `lab/run-eval.sh`, `lab/summarize.py`). The research behind the changes is in `../research_notes/Beating zgrep search algorithms/`.

## What changed

**Local indexed search**
- Candidates are verified on a rayon pool sized by `-j`, in batches rendered in candidate order, so output is identical to the sequential loop. zgrep ignored `-j` here.
- `-l` stops searching a file at its first matching line.
- Patterns anchored with `^`/`$` get the whole-buffer line prescan too. They are recompiled in multi-line mode with CRLF mode off, so `\r$` still matches CRLF lines.
- Opening the index is 6× cheaper. Paths are borrowed from one mmap'd `files.bin` instead of 63K `String` allocations, UTF-8 is validated in the same pass, and the table identity is hashed on a second thread.

**Server**
- Replies are serialized directly. zgrep deep-copied every row twice through `json!`.
- Plain `-l` requests an opt-in `file_list` reply: a string array that the client parses without building a `Value` tree. Older servers are detected and handled.
- Candidate filtering and cache resolution run in parallel. Each file's first-NUL offset is computed once when it is cached, instead of rescanned byte by byte on every query. Small candidate sets stay on the request thread.

**Refresh**
- `refresh` still reads every eligible file, per zgrep's contract. It skips rebuilding postings, and publishing when nothing changed, when a fresh read under unchanged precise metadata shows the content identity already indexed. On the unchanged Rust tree: 2.98 s → 0.57 s, and about half the peak RSS.

**New options**
- `--with PATTERN` / `--without PATTERN`: file-level conditions. Only search files that also contain, or never contain, a matching line. `--with` patterns also narrow the trigram candidate set. Works in scan, local and server modes.
- `--ident NAME`: one identifier in every naming convention. `--ident getUserId` matches `getUserId`, `get_user_id`, `GetUserID`, `get-user-id` and `GET_USER_ID`, and still narrows by the index.

```sh
ogrep -l --with async --without '#\[cfg\(test' -- Mutex .   # async files using Mutex, excluding test modules
ogrep -n --ident parseHeader .                              # parse_header, ParseHeader, PARSE_HEADER, ...
```

## Results

On zgrep's own harness (102-query Rust workload plus targeted Rust and CPython sets, local and server), ogrep 0.1.0 is faster than both tgrep 1.0.10 and zgrep 0.3.0 on every workload, and all 264 queries agree with fresh ripgrep scans:

| Workload | vs tgrep | vs zgrep |
|---|---:|---:|
| Rust broad, local / server | 3.70× / 1.93× | 2.77× / 1.72× |
| Rust targeted, local / server | 4.82× / 2.13× | 2.69× / 1.52× |
| CPython targeted, local / server | 2.23× / 1.30× | 1.84× / 1.17× |

The full qualification against tgrep covers gates, correctness, recovery, refresh, load, startup, cache, index resources and dependencies. It lives in [evidence/ogrep-0.1.0/PRODUCTION_READINESS.md](evidence/ogrep-0.1.0/PRODUCTION_READINESS.md), with the raw results beside it.

## Verification

- 997 workspace tests pass on Linux (zgrep's tests plus the new ones), 1 ignored as in zgrep; clippy and fmt are clean.
- 966 pass on Windows (MSVC, run from Git Bash with `./x test` in a checkout on a Windows disk), 1 ignored. The gap is platform-specific tests: symlinks, FIFOs and Unix change times run only on Linux, while the Windows-only tests cover mapped index files that must not be replaced or removed while mapped, and Windows hidden attributes.
- One inherited test, `concurrent_high_volume_searches`, failed on Windows for zgrep 0.3.0 and ogrep alike. It sends 50 simultaneous searches to a server that admits 4 active plus 32 queued. It now retries the documented "queue is full" answer instead of treating it as a failure.
- The anchored prescan is checked exhaustively against the per-line matcher.
- New options are checked for agreement across scan, local and server modes.
- Every timed benchmark invocation reproduced its validated output.

## Build and install

Linux: `make release` / `make install` (`~/.cargo/bin/ogrep`). Windows: `./x install` from Git Bash (`ogrep.exe` into `~/bin`). The README covers WSL, corporate registries (Artifactory) and offline builds.

## Reproduce

```sh
cargo build --release -p ogrep-cli && cp target/release/ogrep lab/bin/ogrep-new
lab/run-eval.sh NEWRUN 5 ogrep-new     # tgrep vs zgrep vs ogrep, 6 workloads
python3 lab/summarize.py lab/results/NEWRUN
```
