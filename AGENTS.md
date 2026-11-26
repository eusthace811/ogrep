# ogrep for coding agents

A short guide for AI agents (and the humans who wire them up) that want to
use `ogrep` as a fast search tool inside a repository. It complements the
[command reference](docs/REFERENCE.md), which documents every flag; this file covers the few
things an agent has to get right. Install the bundled skill with
`make install-skill` (or `./x install-skill` on Windows) into `~/.agents/skills`;
for Claude Code, link it into `~/.claude/skills` (see the README). It carries
this guidance in a form the agent loads on demand.

## The mental model

ogrep provides indexed text search with many ripgrep-compatible flags and an optional server.
Examples use the installed command name, `ogrep`.

```
ogrep index .        # once: build the index into ./.ogrep
ogrep serve .        # once per session: keep the index warm and watch for changes
ogrep "pattern" .    # every search: finds the server, answers in milliseconds
```

A search resolves in this order:

1. **Server** running for this tree: query it over TCP. Fastest. A file
   watcher keeps the index close to the filesystem, though a silently missed
   notification is repaired only by periodic reconciliation (scheduled hourly
   and deferrable for up to four hours while queried); `--no-watch` disables it.
2. **On-disk index** but no server: read `.ogrep/` directly. Fast, but only as
   fresh as the last successful index publication. Legacy indexes without
   hidden-file coverage metadata and incomplete builds fall back to scanning.
   Rebuild with `ogrep index .` or resume `ogrep serve .` to enable indexed queries.
3. **No index**: scan every file, like grep. Correct but slow on large trees.
   ogrep prints a warning on stderr when this happens.

An agent never has to choose between these; the command is the same. Results
can differ, though. An on-disk index omits changes since its last build. A
server started with no index, a partial index, or legacy hidden-file coverage
lets clients fall back to scanning until it establishes the full corpus.
`ogrep status .` shows `Hidden coverage: complete` once indexed queries are
available, and `Indexing: complete` once the initial build is done. Do not read either as a
freshness signal: a server that starts on an existing index reconciles it
against the filesystem in the background while already reporting complete,
and it never covers watcher events missed later. When a search must reflect current file
contents, pass `--no-index`. It reads every eligible file from disk instead of
consulting the index, still applying the normal ignore, hidden-file, binary
and size rules. It is slow on large trees, so use it deliberately.

## Setup

```bash
make install                          # installs ~/.cargo/bin/ogrep
ogrep --version                  # ensure ~/.cargo/bin is on PATH
```

Then, from the repository root, once:

```bash
ogrep serve . &
```

`serve` builds the index if none exists and answers queries while it builds.
It writes `.ogrep/serve.json` (PID, port and served root) so clients can find
it. Do not commit `.ogrep/`; add it to `.gitignore`.

Check that a server is up:

```bash
ogrep status .
```

If your agent framework cannot keep a background process alive, skip `serve`
and run `ogrep index .` instead. Searches then use the on-disk index. That
index is not updated by searches or edits, so re-run `ogrep index .` after any
change a later search has to see, including your own edits.

## Searching

The command line follows ripgrep. The common `rg` flags are supported with the
same names; an unsupported flag is rejected with an error rather than ignored.
The full list is in the [command reference](docs/REFERENCE.md#cli-flags). Two accepted flags
only take effect on a full scan: `--one-file-system` and `--ignore-file`. An
indexed search ignores them silently, so pair them with `--no-index`.
`-L`/`--follow` always scans, since the index never follows links.

```bash
ogrep -- "fn parse_config" .                 # regex, default
ogrep -F -- "Vec<Option<T>>" .               # literal string
ogrep -w -t rust -- handle .                 # whole word, Rust files only
ogrep -g "src/**" -C 2 -- "TODO|FIXME" .     # glob scope, 2 lines of context
ogrep -l -- "impl .* for Server" .           # file names only
ogrep -c -- deprecated .                     # count per file
ogrep --files -t py .                        # list searchable Python files
```

Rules of thumb for agents:

- **Put `--` before the pattern** and pass the search root explicitly. Shell
  quotes do not stop the parser from reading a bare `index`, `serve`,
  `search`, `status`, `refresh`, `count-files` or `help` as a subcommand;
  `ogrep -- serve .` searches for the word. Everything after `--` is read as
  the pattern and paths, so all flags must come before it.
- **Prefer `-F`** when the query is a symbol or a string the user typed. It
  avoids regex-escaping mistakes.
- **Narrow with `-t` or `-g`** before adding `-m`. Both keep compatible indexes
  in use; globs filter the indexed corpus rather than reinclude ignored files.
  `-m` only trims output.
- **Use `-l` first** on a broad query, then search the specific files. This
  keeps output small.
- **Use `--max-results N`** rather than `| head -N` to sample a broad query:
  the output is the same first N results (lines, or files with `-l`/`-c`), and
  ogrep stops reading files once it has them.
- **Use `-C 2` or `-C 3`** when you need to read the surrounding code.
- **Use `-q`** when you only need a yes/no answer; read the exit code.

## Beyond text search

Searching for text is only part of what ogrep does. These flags answer the
questions an agent usually asks next, offline and deterministically, citing the
file and line behind each answer. [docs/REFERENCE.md](docs/REFERENCE.md) has
the details and measurements.

```bash
ogrep -n --rank -F bulk_update .            # files that define or mainly use a name, best first
ogrep -l --about "$(cat issue.txt)" .       # files a task or bug report is about
ogrep --about "pool exhausted" --cards .    # each ranked file as summary + outline + matches
ogrep -n --kind def -F bulk_update .        # only the definition (--kind ref: only uses)
ogrep -n --scope -F bulk_update .           # matches under their enclosing function
ogrep --callers -w -F bulk_update .         # definitions that use it
ogrep --related-tests src/pool.rs .         # tests to run after changing a file
ogrep --history 'pool exhausted' .          # commits that added or removed a matching line
ogrep --blast cart .                        # what depends on a service, during an incident
ogrep --stack-trace crash.txt .             # a stack trace's frames, in this repository
```

- **Start from a description with `--about`, and read its confidence.** When
  the first two files score within 5%, ogrep prints `--about: unsure` on stderr,
  and with `--json` the first `rank` record carries
  `"confidence": {"level": "unsure", "margin": ...}`. Then the first file is
  right less than half the time, but a right file is in the top five 77-92% of
  the time: read the top five, or add a name from the code to the text. A
  confident first file is right about four times in five.
- **In documentation, `--about` names the section.** Markdown,
  reStructuredText and AsciiDoc files are scored by their best heading section,
  printed as a `path=LINE=Heading` row (a `scope` record with `--json`) above
  the lines it shows. Read that section rather than the whole page.
- **Use `--cards` to choose files and `--map` to see the code around a name,
  within a token budget.** Then open the file itself before editing.
- **Some flags need an extra index.** `--near`, `--map` and `--callees` need
  `ogrep index . --graph`, and `--history` and `--introduced` need
  `ogrep index . --history`. Both take seconds and add to `.ogrep/`.
- **`--at REV` searches a past revision.** Flags that read the working tree
  after the search refuse it.

## Machine-readable output

`--json` emits one JSON object per line, in ripgrep's format. Record types
are `begin`, `match`, `context`, `end`, and `summary`.

Real output, run from a checkout of this repository:

```bash
ogrep --json -F -- "fn main" ogrep-cli/build.rs
```

```json
{"data":{"path":{"text":"ogrep-cli/build.rs"}},"type":"begin"}
{"data":{"absolute_offset":400,"line_number":11,"lines":{"text":"fn main() {\n"},"path":{"text":"ogrep-cli/build.rs"},"submatches":[{"end":7,"match":{"text":"fn main"},"start":0}]},"type":"match"}
{"data":{"binary_offset":null,"path":{"text":"ogrep-cli/build.rs"},"stats":{"bytes_printed":260,"bytes_searched":1775,"elapsed":{"human":"0.000010s","nanos":9709,"secs":0},"matched_lines":1,"matches":1,"searches":1,"searches_with_match":1}},"type":"end"}
{"data":{"elapsed_total":{"human":"0.000470s","nanos":469542,"secs":0},"stats":{"bytes_printed":515,"bytes_searched":1775,"elapsed":{"human":"0.000470s","nanos":469542,"secs":0},"matched_lines":1,"matches":1,"searches":1,"searches_with_match":1}},"type":"summary"}
```

Any parser written for `rg --json` works as is, with one exception: on a line
that is not valid UTF-8, ripgrep emits base64 `lines.bytes`, while ogrep
always emits `lines.text` with each bad byte replaced by U+FFFD. A consumer
that depends on the raw bytes of such lines will see repaired text instead.

`--vimgrep` gives `file:line:col:text`, one row per match, which is the
easiest format to feed into a "jump to location" step.

The analysis flags add record types of their own:
- `rank` and `card` (`--rank`, `--about`, `--cards`);
- `scope`;
- `near`, `map` and `callee`;
- `entity`, `blast`, `suspect`, `event` (`--timeline`), `frame`, `template` and
  `incident`;
- `test`, `similar`, `conflict` and `drift`;
- `commit` and `introduced` (`--history`, `--introduced`).

A parser should skip record types it does not know.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | At least one match |
| `1` | No match |
| `2` | Error (unreadable path, bad regex, ...) |

A match plus an error yields `2`, unless `-q` is set, which yields `0`. Same
as ripgrep. When a search for one identifier finds nothing, stderr may name
similar identifiers that do exist (`ogrep: no matches for "enabel_feature";
similar names: enable_feature`); the exit code stays `1`.

## When the index is not used

These fall back to a full scan even with a server running:

- `--no-ignore` and variants, `-u`/`-uu`/`-uuu`
- `-a`/`--text`, `--binary`, non-`auto` `-E`/`--encoding`
- `-L`/`--follow`
- `--no-index` (explicit)
- naming a single file instead of a directory

Avoid these on large repositories unless you need them.

`index` and `serve` include hidden, non-ignored files by default; passing
`--hidden` to either is redundant. Queries without `--hidden` still apply normal
hidden-file filtering, including Windows hidden attributes. `--hidden` searches
and `--files --hidden` use compatible local/server indexes without disabling
ignore rules. Positive and negative `--glob`/`--iglob` patterns also stay
indexed, for both content searches and `--files`. They filter the indexed
corpus only, so a positive glob does not reinclude ignored files in indexed
mode. Use `--no-index` when those matches are needed; filesystem scans retain
ripgrep-style glob overrides. Negative globs such as `--glob '!.git'` exclude
entire matching directory subtrees.
The configured index directory and all its staging/retired generations are
excluded from indexing, watcher processing, and query filesystem walks, even
with a custom path.

Legacy or incomplete indexes, and older servers that cannot confirm hidden-file
coverage, fall back to scanning. A current server upgrades legacy coverage by
reconciling the tree; `ogrep index .` can rebuild it explicitly.

New indexes have a format boundary that older local readers reject instead of
exposing hidden paths without filtering. Downgrading requires rebuilding with
the older binary, preferably using a separate `--index-path`. New readers retain
legacy-format support for migration.

## Freshness

- With a **server**, results reflect the last watcher event the indexing
  worker has processed. Events are queued and applied asynchronously, so a
  search issued right after an edit can run before the index has caught up.
  To make completed edits searchable before a sequence of indexed queries, run
  `ogrep refresh . --timeout-secs 30` and wait for success. The command
  requires a running server for this root, rereads eligible content and can be
  expensive on large trees. `--no-index` performs a direct scan for one search.
- With only an **on-disk index**, results reflect the last `ogrep index`.
  Files created since then are not found. Run `ogrep index .` again, or
  start `ogrep serve .`.
- `ogrep --files` reads from the index too. Add `--no-index` to list what is
  on disk right now.

`refresh` covers edits completed before the request whose contents and
eligibility remain stable through completion. Concurrent mutations may or may
not appear; it is not an atomic filesystem snapshot. It applies the server's
ignore, exclusion and size settings to the entire served root, including
filename-only assets. Use the same root and `--index-path` as `serve`.
`--timeout-secs` accepts 1–300; `--json` prints a receipt after success. Errors,
including timeout and an unavailable/older server, exit 2 without a fallback.
Timed-out server work may still complete; retry before relying on indexed
freshness. The installed agent adapters expose this as `refresh_index` while
retaining `freshness: "current"` for direct scans.

## Keep `index`, `serve` and search flags aligned

Some flags describe the index, so `index`, `serve` and search have to agree on
them. If they differ, the client either cannot find the server or silently
searches a different set of files.

- `--exclude <DIR>`: `index` and `serve` only. Use the same value on both.
- `--no-ignore`: `index` and `serve` must agree. A server started without it
  on an index built with it treats the ignored files as deleted and drops
  them. Passing it to a search is different: that forces a full scan.
- `--index-path`, `--max-filesize`, `--no-max-filesize`, `--no-require-git`:
  `index`, `serve` and every search. An index built with
  `--no-max-filesize` but searched with the default cap hides every file
  above 64 MiB.

```bash
ogrep index . --index-path /tmp/idx --exclude vendor
ogrep serve . --index-path /tmp/idx --exclude vendor
ogrep "pattern" . --index-path /tmp/idx
```

## Repositories without `.git`

ogrep indexes plain directories normally, but like ripgrep it ignores
`.gitignore` files outside a Git repository. The index is then larger than
expected, and ogrep prints a warning saying so. Pass `--no-require-git` to
`index`, `serve` and search to apply the ignore rules anyway.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `warning: no index at ... - scanning every file` | No index at the path the search looked in | If a server or index uses `--index-path`, pass the same value to the search; otherwise run `ogrep index .` or `ogrep serve .` |
| The same warning when searching a subdirectory | The index is looked up in the first path, so `ogrep x src/` looks for `src/.ogrep` | Search from the root with `-g 'src/**'`, or pass `--index-path .ogrep` |
| `Server unavailable or index coverage not ready, falling back to local index` | Server died, `serve.json` is stale, or the server is still building | Restart `ogrep serve .` |
| The same message ending in `: cannot open the served root: ...` | The tree was moved, removed or made unreadable under a running server | Start `ogrep serve .` in the tree's current location; fix its permissions |
| The same message ending in `: cannot read an indexed file: ...` or `: server .git flush wait timed out` | The server ran out of file descriptors or memory, or is still applying a large `.git` backlog | Results come from the local index, which omits changes since its last publication; retry later, or raise the server's descriptor limit |
| A new file is not found, no server | On-disk index predates the file | Run `ogrep index .` |
| A new file is not found, server running | First build still in progress, or the watcher event is still queued | Run `refresh` and wait for success, or pass `--no-index` for this search; re-running `ogrep index .` does not update a running server |
| Search is slow despite a server | Flag bypasses the index (see above) | Drop the flag or scope with `-g`/`-t` |

## Tool definition sketch

If you expose ogrep to a model as a tool, a minimal schema is:

```json
{
  "name": "ogrep",
  "description": "Fast regex search over the repository. ripgrep-compatible flags. Use -F for literal strings, -t/-g to scope, -l for file names only, -C N for context.",
  "parameters": {
    "type": "object",
    "properties": {
      "pattern": {"type": "string", "description": "Regex, or literal string with -F"},
      "path": {"type": "string", "default": "."},
      "flags": {"type": "array", "items": {"type": "string"}}
    },
    "required": ["pattern"]
  }
}
```

Before invoking ogrep, canonicalize `path` beneath the configured repository root
and reject paths that resolve outside it. Allowlist search-only flags rather than
forwarding arbitrary tokens. Then run `ogrep <flags...> -- <pattern> <path>` and
return stdout, stderr and the exit code together. Treat `1` as "no results", not
as a failure. Treat `2` as an error; stderr carries the cause, such as a bad regex.
Always return stderr: the "no index" warning can arrive with code `0` or `1`.
