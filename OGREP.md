# ogrep: experimental fork of zgrep 0.3.0

ogrep is a research fork of [zgrep](../zgrep) (itself a fork of Microsoft tgrep). It keeps zgrep's index format, CLI, RPC protocol and correctness contracts, and removes the costs that dominated zgrep's measured profile. The binary is still built as `target/release/zgrep`, so zgrep's scripts, tests and harness run unchanged. zgrep itself was not modified.

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
zgrep -l --with async --without '#\[cfg\(test' -- Mutex .   # async files using Mutex, excluding test modules
zgrep -n --ident parseHeader .                              # parse_header, ParseHeader, PARSE_HEADER, ...
```

## Results

On zgrep's own harness (102-query Rust workload plus targeted Rust and CPython sets, local and server), ogrep is faster than zgrep 0.3.0 on every workload. See `lab/results/` and the table in `../research_notes/Beating zgrep search algorithms/experiments_ogrep.md`.

## Verification

- 983 workspace tests pass (zgrep's 979 plus 4 new), 1 ignored as in zgrep; clippy and fmt are clean.
- The anchored prescan is checked exhaustively against the per-line matcher.
- New options are checked for agreement across scan, local and server modes.
- Every timed benchmark invocation reproduced its validated output.

## Reproduce

```sh
cargo build --release -p zgrep-cli && cp target/release/zgrep lab/bin/ogrep-new
lab/run-eval.sh NEWRUN 5 ogrep-new     # tgrep vs zgrep vs ogrep, 6 workloads
python3 lab/summarize.py lab/results/NEWRUN
```
