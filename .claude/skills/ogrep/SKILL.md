---
name: ogrep
description: Search code with `ogrep`, an indexed, ripgrep-compatible grep that reads only the files that can match. Use it to search file contents in a repository with a `.ogrep/` directory, in any large repository (monorepos, compilers, kernels) when `ogrep` is installed, or when the user mentions ogrep, indexed grep, `ogrep serve` or `ogrep refresh`. Also use it for what plain grep cannot do, such as files with X and also Y (`--with`) or never Y (`--without`); an identifier in every naming style (`--ident`); the files a task or bug report is about (`--about`); where a name is defined or mainly used (`--rank`, `--kind def`); file outlines within a token budget (`--cards`, `--map`); callers, callees and the tests to run; near-duplicate code, stale doc names, branch conflicts; the commits that added a line (`--history`); and incident triage (blast radius, likely causes, stack traces, new log messages and the code writing them, similar postmortems). Prefer it to repeated `rg`/`grep` or Grep-tool calls in big trees.
allowed-tools: Bash(ogrep:*)
---

# Searching code with ogrep

`ogrep` is grep with an index. `ogrep index .` records which files contain which
three-character sequences. Each search then uses that record to narrow the
repository down to a handful of candidate files, and runs a real regex engine
over only those. The flags, output format and exit codes are ripgrep's. Anything
you would type after `rg`, you can type after `ogrep`.

On a 63,000-file repository, a specific search takes about 3 ms with ogrep and
about 40 ms with ripgrep. The gap grows with the size of the repository.

## The contract

- **stdout is results, in ripgrep's format.** `path:line:text` with `-n`, one
  path per line with `-l`, ripgrep's JSON event stream with `--json`.
- **stderr is diagnostics.** It carries warnings such as "no index ... scanning
  every file", and the `--stats`/`--explain` report.
- **Exit codes:**

| Code | Meaning |
|---|---|
| 0 | at least one match |
| 1 | no match. This is not an error; do not retry with other tools. If you searched for one identifier, stderr may list similar names that do exist (`similar names: enable_feature, ...`): check whether you misspelt it |
| 2 | error: bad regex, unreadable path, unknown flag, or `refresh` without a server |

An unsupported ripgrep flag is rejected with exit 2, never silently ignored.

## Before the first search

```bash
command -v ogrep || echo "ogrep is not installed"   # fall back to rg / the Grep tool
ogrep status .                                     # index? server running? indexing complete?
```

Run everything from the **repository root**. The index lives in `./.ogrep`.

- **No index yet:** build one. It takes a few seconds even for large trees.
  Hide the index folder from git without touching tracked files:

  ```bash
  ogrep index .
  grep -qxF '.ogrep/' .git/info/exclude 2>/dev/null || echo '.ogrep/' >> .git/info/exclude
  ```

- **You will edit files and search again in this session:** start the server once,
  in the background. It follows edits and makes `ogrep refresh` available:

  ```bash
  ogrep serve . > /dev/null 2>&1 &     # or run it with run_in_background
  ```

  Searches find the server automatically. It keeps running after the session ends.
  Stop it with `pkill -f 'ogrep serve'` (on Windows: `taskkill //IM ogrep.exe //F`).

## Freshness: the one thing to get right

The index is a snapshot, and **a stale index misses new content silently**. A
file added or edited after `ogrep index` is not found: exit 1, with no warning.
After you or anything else edits files, choose one of these:

| Situation | Do this |
|---|---|
| server running | `ogrep refresh .`, wait for exit 0, then search |
| no server, just need current results once | `ogrep --no-index 'pattern' .` (scans like rg) |
| no server, many searches coming | `ogrep index .` again, or start `ogrep serve .` |

`ogrep refresh .` returns once completed edits are searchable. It takes
milliseconds on small trees and about half a second on 63,000 files. It exits 2
if no server is running; do not treat that as "no results".

## Searching

```bash
ogrep -n 'fn \w+_config' .                 # regex (the default)
ogrep -n -F 'Vec<Option<T>>' .             # literal text: use -F for anything with regex metacharacters
ogrep -n -i 'timeout' .                    # ignore case (-S: smart case)
ogrep -n -w 'id' .                         # whole word
ogrep -l 'unsafe' .                        # file names only
ogrep -c 'unwrap()' .                      # count per file
ogrep -n -C 3 'deadlock' .                 # context lines
ogrep -n -t rust -t toml 'serde' .         # file types (ogrep --type-list)
ogrep -n -g 'src/**' -g '!**/tests/**' 'retry' .   # path globs
ogrep -n -e 'TODO' -e 'FIXME' .            # several patterns (OR)
ogrep -n -P '\bfoo(?!bar)' .               # lookaround / backreferences
ogrep --json 'pattern' .                   # machine-readable events
ogrep -n --max-results 50 'Vec<' .         # only the first 50 matching lines; stops searching there
```

For a broad pattern you only want to sample, use `--max-results N` rather than
`| head -N`: the output is the same, but ogrep stops reading files once it has N.

**Put options before `--`.** `--` ends option parsing, so use
`ogrep -n -F -- '-v' .`. In `ogrep -- '-v' . -n`, the `-n` is taken as a path.

### Which flag for which question

| You have / want | Use |
|---|---|
| exact text or a regex, every occurrence | plain search (above) |
| a name; where it is defined or mainly used | `--rank -F NAME`, `--kind def` |
| a task, bug report or commit message, not a name | `--about "TEXT"` |
| to read many candidate files cheaply | `--cards` (with `--rank` or `--about`), `--map` |
| who calls / what it calls | `--callers`, `--callees` (after `ogrep index . --graph`) |
| the tests to run after changing a file | `--related-tests FILE` |
| when a line was added or removed | `--history`, `--introduced` (after `ogrep index . --history`) |
| an outage: what depends on what, what changed | `--entities`, `--blast`, `--suspects`, `--timeline` |
| a stack trace, a log file | `--stack-trace FILE`, `--log-templates ... --template-to-code` |

Each answer is computed from the repository's files and its git history, with
file and line evidence; none of them uses a model or the network.

### Searching part of the repository

The index is found in the **first path you pass**, so `ogrep 'x' src/` looks for
`src/.ogrep`. It does not find one, and it scans every file under `src/`
instead. To stay on the index, pick one of these:

```bash
ogrep -n -g 'src/**' 'pattern' .                  # search from the root, filter by glob
ogrep -n --index-path .ogrep 'pattern' src/lib    # or name the root's index explicitly
```

### What only ogrep can do

```bash
ogrep -l --with 'async fn' -- Mutex .                        # files with Mutex that also have async fn
ogrep -l --with 'async fn' --without '#\[cfg\(test' -- Mutex .   # ...excluding test modules
ogrep -n --ident getUserId .    # getUserId, get_user_id, GetUserID, get-user-id, GET_USER_ID
ogrep --explain -F 'parse_header' . > /dev/null   # how much the index narrowed the search (stderr)
```

`--with` and `--without` are file-level conditions. They are checked anywhere in
the file, not on the matching line. `--with` also narrows the index lookup.

### Finding where something lives: `--rank`

When you need the place a name is defined or mainly used, not every mention,
rank the results. The best files come first, each with a few telling lines
(definitions first); tests, vendored code and docs weigh less.

```bash
ogrep -n --rank -F bulk_update .                  # best 20 files, 3 lines each
ogrep -n --rank --budget 5 --per-file 1 -F bulk_update .
ogrep -l --rank -F bulk_update .                  # ranked file names only
ogrep -n --explain-rank --budget 5 -F bulk_update .   # with each file's score
```

Ranking only reorders and trims real matches, and its order is deterministic.
Drop `--rank` when you need every occurrence. `--rank-cutoff auto` also stops
where the scores fall off sharply. `--rank-fit examples.json .` tunes the
weights for one repository from `{"query", "relevant": [paths]}` examples.

### Starting from a task or bug report: `--about`

When you have a description instead of a name, pass the text itself. Its words
are searched together and the files are ranked the same way:

```bash
ogrep -l --about "Handled out-of-range year in admin date_hierarchy" .
ogrep -n --about "$(cat issue.txt)" --budget 10 .
```

If ogrep prints `--about: unsure` on stderr (or `"confidence":{"level":"unsure"}`
with `--json`), the first two files scored almost the same. The first file is
then right less than half the time, but a right one is usually in the top
five: read those, or rerun with a name from the code added to the text. When it
is silent, the first file is right about four times in five.

In documentation (Markdown, reStructuredText, AsciiDoc), `--about` scores each
heading section on its own and prints the best one as a `path=LINE=Heading`
row (a `scope` record with `--json`) above the file's lines. To find where
something is documented, search the docs directory and read that section:

```bash
ogrep -n --about "how to configure the cache timeout" -g 'docs/**'
```

### Reading many files at once: `--cards`

Instead of matching lines, `--cards` prints each ranked file as a card: its
path, a one-line summary from its own docstring or heading, its outline
(definitions, or the heading tree for Markdown and reStructuredText) and its
matching lines marked `>`, within a token budget (default 2000):

```bash
ogrep --about "pool exhausted under load" --cards .
ogrep --rank --cards=800 -F bulk_update .
```

At the same budget, cards showed the function a change touched 1.6-2 times as
often as matching lines did. Use them to pick which files to open; open the file
before editing it. `--cards` cannot be combined with `-l`, `-c`, `-o`,
`--vimgrep` or context flags.

### Structure-aware filters

```bash
ogrep -n --scope -F bulk_update .        # each hit under its enclosing function/class
ogrep -n --kind def -F bulk_update .     # the definition itself
ogrep -n --kind ref -F bulk_update .     # the uses
ogrep -n --in comments TODO .            # comments only (or: code, strings)
```

### Maps and call relationships

```bash
ogrep index . --graph                    # once (seconds); rerun after large changes
ogrep --map=1500 -F bulk_update .        # compact outline of the code around a name
ogrep --near -F bulk_update .            # related files, with the names linking them
ogrep --callers -w -F bulk_update .      # functions/methods that use it
ogrep --callees bulk_update .            # what its definition uses, and where that lives
```

### Incidents: blast radius, suspects, traces, logs

```bash
ogrep --entities .                                 # services, alerts and their dependencies, from compose/k8s/catalog files
ogrep --blast cart .                               # who depends on cart (with evidence lines)
ogrep --suspects checkout,frontend --since 2h .    # likely root causes, recent changes weighted
ogrep --timeline checkout --since 3d .             # commits to checkout and to what it depends on
ogrep --suspects checkout --until 2024-03-01T18:00Z .   # a past incident: nothing after WHEN counts
ogrep --stack-trace trace.txt .                    # where each frame of a trace is in this repo (- for stdin)
ogrep --log-templates . logs/app.log               # a log reduced to its message templates, with counts
ogrep --log-templates --new-since 1h . logs/       # log messages that only started recently
ogrep --log-templates --template-to-code ERROR logs/   # and the code that writes them
ogrep --similar-incidents "checkout timeouts, redis pool" .   # related postmortems
```

`--template-to-code` found the writing file for 89-97% of log lines in tests on
three repositories, with values filled in. Dependencies are marked declared or
inferred, with the file and line each comes from: check the evidence before
acting on it.

### Before and after a change

```bash
ogrep --related-tests django/db/models/query.py .   # tests to run, best first
ogrep --similar src/pool.rs:120 .                   # code like the definition at that line
ogrep --conflicts .                                 # lines also changed on other local branches
ogrep --doc-drift .                                 # names quoted in docs that no code has
```

### What changed, and when: history

```bash
ogrep index . --history                  # once; rerun to add new commits (fast)
ogrep --history -n 'pool exhausted' .    # commits that added/removed a matching line
ogrep --introduced 'pool exhausted' .    # first added, last removed, present now?
ogrep --changed-since 2h -n 'panic' .    # only files changed recently (incl. uncommitted)
ogrep --at v1.4.0 -n parse_header .      # the tree as it was at a revision
ogrep --history --since 2w --author jane 'timeout' .   # filter commits
```

`--history` finds exactly the commits `git log -G` finds, in milliseconds
instead of seconds to minutes. `--at` reads git's objects, not the working tree,
so flags that read the working tree after the search (`--about`, `--cards`,
`--scope`, `--kind`, `--in`, `--callers`, the graph and incident tools) refuse
it.

### JSON from the analysis flags

With `--json`, each analysis flag prints its own record type alongside
ripgrep's `begin`/`match`/`end`: `rank` and `card` (with a `confidence` object on
the first one for `--about`), `scope` (`--scope`, and the best section of a
documentation file for `--about`), `near`, `map`, `callee`, `entity`,
`blast`, `suspect`, `event` (`--timeline`), `frame`, `template`, `incident`,
`test`, `similar`, `conflict`, `drift`, `commit` and `introduced`. Parsers should
skip types they do not know.

## What is and is not searched

It skips the same things ripgrep does: `.gitignore`d files, hidden files and
binary files. **It also skips files over 64 MiB**, which ripgrep does not.

- `--hidden` includes hidden files.
- `--no-ignore` includes ignored files.
- `--no-max-filesize` lifts the size limit.
- `ogrep --files .` lists exactly what is searchable.

Some flags cannot use the index and fall back to a full scan, which is still
correct, just slower:

- `--no-ignore`, `-u`
- `-a`/`--text`, `--binary`
- `-E`/`--encoding`
- `-L`/`--follow`
- naming a single file

Two flags only take effect on a scan, and an indexed search ignores them:
`--one-file-system` and `--ignore-file`. Pair them with `--no-index`.

## When a result looks wrong

1. **Missing a match you just wrote?** The index is stale. Run `ogrep refresh .`
   if a server is running, or check with `ogrep --no-index`.
2. **"no index at ... scanning every file" on stderr?** You searched a
   subdirectory or a repository without an index. See the sections above.
3. **Found nothing in a large generated file?** It is probably over 64 MiB. Add
   `--no-max-filesize`.
4. **Output differs from rg?** See [references/troubleshooting.md](references/troubleshooting.md).

## More

* **Index, server, refresh, status and Windows notes:** [references/server.md](references/server.md)
* **Every search flag, grouped by task:** [references/flags.md](references/flags.md)
* **Ranking, structure, graph, incident and history flags in detail:** [references/analysis.md](references/analysis.md)
* **Differences from ripgrep, errors and recovery:** [references/troubleshooting.md](references/troubleshooting.md)
