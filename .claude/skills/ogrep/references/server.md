# Index, server and freshness

## Three ways a search runs

A search picks the first of these that is available, in this order:

1. **Server.** `ogrep serve` is running for this index, and `.ogrep/serve.json`
   says where to find it. This is the fastest way, and it stays current as files
   change.
2. **Local index.** `.ogrep/` exists but no server is running. This is fast, but
   it only knows the files as they were at the last `ogrep index`.
3. **Scan.** There is no index, or a flag needs the filesystem. It reads every
   eligible file, like ripgrep, and warns on stderr.

`ogrep --stats` (alias `--explain`) says which one ran. `(via server)` means the
server answered. `Brute-force search` means it scanned.

## Commands

```bash
ogrep index .                       # build or rebuild ./.ogrep
ogrep index . --exclude vendor      # skip directories (repeatable)
ogrep index . --graph               # also the file graph for --near, --map, --callees (seconds)
ogrep index . --history             # also the git history index for --history, --introduced
ogrep index . --history --history-depth 0   # every commit, not just the latest 20,000
ogrep serve .                       # foreground server; builds the index if missing
ogrep serve . --no-watch            # serve without following edits
ogrep serve . --watch-mode poll     # for network or virtual filesystems without native events
ogrep refresh .                     # wait (default 30 s) until completed edits are searchable
ogrep refresh . --timeout-secs 120  # larger trees
ogrep refresh . --json              # receipt as JSON
ogrep status .                      # index size, server PID/port, watcher and indexing state
ogrep --files .                     # the searchable file list
ogrep count-files .                 # count text files without indexing
```

**Keep the flags on `index` and `serve` the same.** Four flags decide which files
belong in the index: `--no-require-git`, `--no-ignore`, `--max-filesize` and
`--exclude`. If they differ between the two commands, the server treats the
files it cannot see as deleted.

## Starting and stopping the server from an agent

```bash
nohup ogrep serve . > /dev/null 2>&1 &            # survives the calling shell
ogrep status . | grep -E 'PID|Indexing'           # confirm it is up
```

A server that has just started reconciles the index with the disk. That catches
up on edits made while no server was running. Until the reconciliation
finishes, searches fall back to the local index or a scan, and they stay
correct. Servers keep running after the session ends. To stop one:

```bash
pkill -f 'ogrep serve'                  # Linux, macOS, WSL, Git Bash
taskkill //IM ogrep.exe //F             # Windows from Git Bash (stops every ogrep.exe)
```

Only one server can serve an index at a time. A second `ogrep serve` for the
same index refuses to start.

## Refresh semantics

- `refresh` rereads every eligible file under the served root and returns once
  the result is searchable. Files whose content did not change are skipped
  cheaply.
- It needs a running server for that exact root and index path. Without one it
  exits 2 with "refresh requires a running server".
- A timeout exits 2, but the server keeps working. Retry before you rely on the
  results.
- Edits must be complete while it runs. Files being written during the refresh
  may or may not be included.
- It is meant for batches of edits, not for every keystroke. On a 63,000-file
  tree it takes about 0.5 s.

## Subdirectories and index location

The index is looked up at `<first path>/.ogrep`. To search part of a repository
and still use the root index:

```bash
ogrep -g 'src/parser/**' 'pattern' .
ogrep --index-path .ogrep 'pattern' src/parser        # from the root
ogrep --index-path ../../.ogrep 'pattern' .           # from inside src/parser
```

`--index-path DIR` puts the index anywhere, for example outside a read-only
checkout. Pass the same `--index-path` to `index`, `serve`, `refresh`, `status`
and every search.

## Windows

- The binary is `ogrep.exe`. In Git Bash it is on PATH after `./x install`.
  Commands are identical.
- Windows will not replace a running `ogrep.exe`. Stop every `ogrep serve`
  before reinstalling.
- An in-place `ogrep index .` fails with error 1224 while a server or search has
  the index open. Stop the server first. Nothing is corrupted.
- Paths print with `\` after the part you typed. Pass `--path-separator /` for
  forward slashes.

## Resource notes

- Building an index uses a bounded external sort: about 150–200 MiB of memory
  regardless of repository size. It takes about 2 s for 63,000 files.
- The index is about half the size of the source it covers: 203 MB for the
  Rust compiler's 408 MB tree.
- The server admits 4 concurrent searches and queues 32 more. When the queue is
  full, a search exits 2 with an overload error; retry it.
