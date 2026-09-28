# Operating ogrep

ogrep is built and tested on Linux x86-64 and Windows x86-64 (MSVC). The CI
definitions also cover macOS. Install it with `make install` on Linux, macOS and
WSL, or with `./x install` from Git Bash on Windows (see the
[README](../README.md)).

## Running the server

```sh
ogrep index /path/to/repository     # optional: serve builds the index if it is missing
ogrep serve /path/to/repository     # foreground; stop with Ctrl-C
ogrep status /path/to/repository    # service and indexing state
```

`serve` watches for changes unless `--no-watch` is passed. Searches discover it
through the index's `serve.json`. When no server is running, searches use the
on-disk index, and when there is no index they scan the files directly. Only one
server can serve an index directory at a time.

## Freshness and scope

An on-disk index reflects its last publication. The watcher is asynchronous, so
a search issued immediately after an edit can run before the update. A status
that says indexing is complete does not prove the index matches the current
contents.

When a search must see completed edits, run
`ogrep refresh /path/to/repository --timeout-secs 30` and wait for it to
succeed. It needs the running server for that exact root and index path. It
rereads eligible content and applies the server's scope settings.

- `--json` prints the receipt, and the deadline accepts 1–300 seconds.
- A timeout, an incomplete read or publication, an invalid receipt, or an
  unavailable or older server returns exit 2, without falling back to a search.
- Server work may continue after a timeout or cancellation.
- Concurrent mutations may or may not appear, and covered edits must remain
  stable until the refresh completes.

For a direct scan of the eligible files, use `--no-index`. Or rebuild a
repository that is not changing with `index`. No operation promises an atomic
snapshot of a tree that is being changed concurrently.

Default eligibility applies the ignore rules, the hidden and binary file
policies, the supported extensions and the size limit. A query with no match
tells you about that scope, not about every byte in the directory. `--files`
shows the eligible paths. `-L`, `--one-file-system` and `--ignore-file` only
affect scanning, so combine them with `--no-index`. See the
[agent guidance](../AGENTS.md) for other fallback flags and for output
differences from ripgrep.

The index is disposable derived data. If it needs repair, stop the server,
remove the repository's `.ogrep` directory and rebuild. Keep `.ogrep/` out of
Git.

## Compatibility limits

By default, LF matching keeps the CR bytes in CRLF source lines, including in
JSON offsets and unterminated content. `--crlf`, which changes matching
semantics in ripgrep, is rejected with exit 2, so use ripgrep when you need
CRLF-aware anchors or line terminators. `--no-crlf` selects the supported
default. Selecting a file type positively admits matching hidden files, but
hidden ancestor directories and ignore rules still apply.

A few ripgrep differences remain:

- A match failure in the PCRE-style engine stops the search, where ripgrep
  carries on.
- Under `--no-messages`, a folder containing an unreadable file exits 0, where
  ripgrep exits 2.
- Nest-limit parity with ripgrep does not hold under `-x` and `-w`.

## Service access and resources

The server is a trusted local tool. It binds to loopback and has no
authentication or multi-user authorization, so other local processes can send
it search, reload and refresh requests. Do not forward its port or expose it as
a shared service. Run it under the account that owns the repository, and
protect the repository and the index with normal filesystem permissions.

Limits:

- `-j 4` bounds the parallel search worker pool.
- `serve --search-concurrency 4` is the default request limit (accepted range
  1–32), held through response writes.
- At most 32 searches wait, each for up to 30 seconds.
- The service accepts at most 64 connections and request frames up to 1 MiB.
- Socket reads and writes have 30-second idle timeouts.

Explicit overload returns `-32002`, and the CLI exits 2 without a local
fallback. Status and refresh bypass the search queue but share the connection
cap. Admission does not bound per-query output or process memory.
`--max-memory` controls flush decisions during the initial build; it is not a
hard limit on process RSS. Use operating-system limits when a deployment needs a
hard budget.

For filesystems that only support polling, use
`serve --watch-mode poll --poll-interval 1` when a one-second reconciliation
cadence is appropriate. The interval starts after the previous pass finishes.
Native mode falls back to polling automatically when watch registration or the
configured budget prevents reliable coverage. `status` shows the active mode.

After a crash, restart the server and let reconciliation finish. An in-place
`ogrep index` rebuild truncates `lookup.bin` and `index.bin` under any running
local search, so stop searches first or build into a separate `--index-path`.
On Windows the rebuild fails with error 1224 while anything has the index open,
and it does not corrupt anything.

A full-tree refresh is meant for batches of edits, not for every keystroke. On
the unchanged 63,000-file Rust tree it takes about 0.5 s and peaks at about
0.8 GiB of memory.
