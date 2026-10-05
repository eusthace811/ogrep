# ogrep

**Fast code search for big repositories.** ogrep is a grep that indexes your
repository once. After that it answers each search by reading only the few files
that can possibly match. The command line, output and exit codes are ripgrep's,
so if you know `rg`, you already know ogrep.

```sh
ogrep index .                  # once: build the index (a few seconds for 63,000 files)
ogrep -n 'fn parse_header' .   # search, like rg
```

## Why ogrep

**ripgrep reads every file on every search.** That is fast on a small project
and slow on a big one. ogrep's index records which files contain which
three-character sequences (*trigrams*). Before searching, it uses the index to
cut the repository down to the files that could match, then checks those files
with a real regex engine. You get exactly the lines ripgrep would print, because
matching is still done on the file contents.

Searching the Rust compiler's source (63,000 files) for `fn parse_header`:

| Tool | Files read | Time |
|---|---:|---:|
| ripgrep | 63,000 | 43 ms |
| tgrep | 3 | 17.5 ms |
| **ogrep** | **3** | **3.4 ms** |

The bigger the repository and the more specific the search, the bigger the gap.
[BENCHMARKS.md](BENCHMARKS.md) has the full numbers, including broad searches,
where the gap is smaller.

It also does things plain grep cannot:

- **Stays current.** `ogrep serve` keeps the index in memory and updates it as
  you edit. `ogrep refresh` waits until your latest edits are searchable.
- **Searches by file conditions.** `--with PATTERN` and `--without PATTERN`
  search only files that also contain, or never contain, another pattern.
- **Finds identifiers in any naming style.** `--ident getUserId` also finds
  `get_user_id`, `GetUserID`, `get-user-id` and `GET_USER_ID`.
- **Explains itself.** `--explain` shows how many files the index ruled out and
  how long it took.
- **Finds where things live, not just lines.** `--rank` puts the defining files
  first, `--about "bug report text"` finds the files a description is about and
  says when it is unsure, and `--cards` shows each file's outline within a token
  budget.
- **Knows code structure, history and operations.** Definitions and uses,
  callers and callees, the tests to run, the commits that added a line, blast
  radius and likely causes during an incident, stack traces and log messages
  mapped back to code. Everything runs offline and deterministically.

## Install

You need a Rust toolchain (edition 2024, Rust 1.85 or newer).

**Linux, macOS and WSL:**

```sh
make install          # builds and installs ~/.cargo/bin/ogrep and ogrep-lab (put it on PATH)
```

**Windows (Git Bash):**

```sh
./x install           # builds ogrep.exe and installs it into ~/bin
hash -r
ogrep --version
```

`./x` also runs from WSL, where it drives the Windows toolchain. Windows will not
replace a running `ogrep.exe`, so stop any `ogrep serve` before reinstalling.
`./x help` lists every command.

**On a corporate network (Artifactory):** cargo downloads crates from whatever
the machine's `~/.cargo/config.toml` substitutes for crates.io, usually an
Artifactory Cargo remote. Nothing registry-specific is committed here.

- `./x doctor` shows which registry cargo will use, checks that every locked
  crate resolves, and prints the config lines to add if one does not.
- `./x fetch` downloads every crate up front.
- To use a registry for a single run, set
  `OGREP_CARGO_REGISTRY=sparse+https://<host>/artifactory/api/cargo/<repo>/index/`.
  Add `CARGO_REGISTRIES_OGREP_MIRROR_TOKEN` if the registry needs a token.
- `./x vendor` copies every crate into `./vendor` for fully offline builds.

The Windows build links the C runtime statically, so it does not need the
Visual C++ redistributable.

## How to use it

### The three commands you need

```sh
cd /path/to/your/repo

ogrep index .            # 1. Build the index into ./.ogrep. Do this once.
ogrep serve .            # 2. Optional: keep it warm and follow your edits.
                         #    Leave it running in its own terminal.
ogrep 'TODO' .           # 3. Search, from any terminal.
```

Add `.ogrep/` to the repository's `.gitignore`.

If you skip step 2, searches read the index from disk. That is still fast, but
the index only knows the files as they were when you ran `ogrep index`. Run
`ogrep index .` again after big changes, or just keep `ogrep serve` running.

With no index at all, ogrep scans every file like ripgrep does, and warns you.

### Everyday searches

| I want to... | Command |
|---|---|
| find a regex | `ogrep 'fn \w+_config' .` |
| find exact text (no regex) | `ogrep -F 'Vec<Option<T>>' .` |
| ignore case | `ogrep -i 'timeout' .` |
| match whole words only | `ogrep -w 'id' .` |
| see line numbers | `ogrep -n 'panic!' .` |
| list matching files only | `ogrep -l 'unsafe' .` |
| count matches per file | `ogrep -c 'unwrap()' .` |
| see lines around each match | `ogrep -C 3 'deadlock' .` |
| search only some file types | `ogrep -t rust -t toml 'serde' .` |
| search only some paths | `ogrep -g 'src/**' -g '!*_test.rs' 'retry' .` |
| search several patterns at once | `ogrep -e 'TODO' -e 'FIXME' .` |
| files using Mutex that are async | `ogrep -l --with async -- Mutex .` |
| ...but not test modules | `ogrep -l --with async --without '#\[cfg\(test' -- Mutex .` |
| an identifier in every naming style | `ogrep -n --ident parseHeader .` |
| lookaround or backreferences | `ogrep -P '\bfoo(?!bar)' .` |
| JSON output for scripts | `ogrep --json 'pattern' .` |
| see how the index helped | `ogrep --explain 'pattern' .` |

Put `--` before a pattern that starts with `-`: `ogrep -- '-v' .`

### Beyond lines: ranking, structure, history, incidents

| I want to... | Command |
|---|---|
| the files that define or mainly use a name | `ogrep --rank -F bulk_update .` |
| the files a bug report or task is about | `ogrep --about "out-of-range year in admin date hierarchy" .` |
| where something is documented, down to the section | `ogrep -n --about "configure the cache timeout" -g 'docs/**'` |
| an outline of each ranked file, within ~1000 tokens | `ogrep --about "pool exhausted" --cards=1000 .` |
| only the definition / only the uses | `ogrep --kind def -F bulk_update .` / `--kind ref` |
| each match under its enclosing function | `ogrep --scope -F bulk_update .` |
| matches in comments only | `ogrep --in comments TODO .` |
| who calls it / what it calls | `ogrep --callers -w -F bulk_update .` / `ogrep --callees bulk_update .` |
| a map of the related code | `ogrep index . --graph`, then `ogrep --map=1500 -F bulk_update .` |
| the tests to run after a change | `ogrep --related-tests django/db/models/query.py .` |
| code that looks like a definition | `ogrep --similar src/pool.rs:120 .` |
| the commits that added or removed a line | `ogrep index . --history`, then `ogrep --history 'pool exhausted' .` |
| only files changed in the last two hours | `ogrep --changed-since 2h 'panic' .` |
| the tree as it was at a tag | `ogrep --at v1.4.0 parse_header .` |
| what depends on a service | `ogrep --blast cart .` |
| likely causes of an outage | `ogrep --suspects checkout,frontend --since 2h .` |
| where a stack trace's frames are | `ogrep --stack-trace crash.txt .` |
| new log messages and the code that writes them | `ogrep --log-templates --new-since 1h --template-to-code . logs/` |
| related postmortems | `ogrep --similar-incidents "checkout timeouts, redis pool" .` |
| names in docs that the code no longer has | `ogrep --doc-drift .` |

[docs/REFERENCE.md](docs/REFERENCE.md) describes each one, with how it was
measured.

### Keeping results current

`ogrep serve` watches the repository and updates the index as files change, but
it does so a moment after each change. If a search must see edits you have just
made (a script that edits and then searches, or an AI agent), wait for them
first:

```sh
ogrep refresh .          # returns once completed edits are searchable
ogrep 'new_function' .
```

To check the files exactly as they are on disk right now, skip the index:
`ogrep --no-index 'pattern' .`

### Other commands

```sh
ogrep status .           # is a server running? is indexing complete?
ogrep --files .          # which files are searchable
ogrep count-files .      # how many text files a repository has
```

### Looking inside the index

```sh
ogrep-lab .              # a local page: the index, query plans, files, graph, history, evaluations
```

`ogrep-lab` is a separate binary that serves a page on 127.0.0.1 only; nothing
is fetched from the network. See [ogrep-lab/README.md](ogrep-lab/README.md).

### What ogrep searches

Like ripgrep, ogrep skips files listed in `.gitignore`, hidden files and binary
files. Unlike ripgrep, it also skips files over 64 MiB, which are nearly always
generated.

- `--hidden` includes hidden files.
- `--no-ignore` includes ignored files.
- `-u`, `-uu` and `-uuu` relax the rules step by step.
- `--no-max-filesize` lifts the size limit.

Exit codes are ripgrep's: 0 found a match, 1 found nothing, 2 hit an error.

## Using ogrep from AI agents

**Claude Code:** the repository ships a skill that teaches Claude when and how to
search with ogrep. Install it for every project:

```sh
make install-skill       # Linux, macOS, WSL: ~/.agents/skills/ogrep
./x install-skill        # Windows: %USERPROFILE%\.agents\skills\ogrep
```

Both install into the shared `~/.agents/skills` directory (`make install-skill
SKILLS_DIR=...` picks another). Claude Code reads `~/.claude/skills`, so link it
there once: `ln -s ../../.agents/skills/ogrep ~/.claude/skills/ogrep`.

**Codex and pi:** `./install-agent.sh` installs MCP search tools that start and
share an ogrep server automatically. Results come back as compact text, each
path once with its lines under it, which costs a model about a third to a half
of the tokens of one JSON record per line. To update an existing installation,
run `./install-agent.sh repair --agent codex,pi --root /path/to/repository`. See
[scripts/agent/README.md](scripts/agent/README.md).

[AGENTS.md](AGENTS.md) covers what an agent has to get right: freshness,
fallbacks and output differences from ripgrep.

## Documentation

- [docs/REFERENCE.md](docs/REFERENCE.md): every command and flag, the index,
  the server, file-size limits, binary files and the on-disk format.
- [docs/OPERATIONS.md](docs/OPERATIONS.md): running the server, freshness,
  resource limits and recovery.
- [docs/RPC.md](docs/RPC.md): the server protocol.
- [BENCHMARKS.md](BENCHMARKS.md): speed, and how to reproduce it.

## Development

```sh
make test        # the whole test suite (./x test on Windows)
make check       # formatting and Clippy
make benchmark   # scripts/benchmark.sh: ogrep against ripgrep on a real repository
```

The code is three crates. `ogrep-core` holds the trigram index, on-disk format,
walker and query planner. `ogrep-cli` holds the command line, output, server and
file watcher. `ogrep-lab` is the local workbench, a separate binary. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Credits and license

ogrep uses Microsoft's [tgrep](https://github.com/microsoft/tgrep) as its
reference design and builds on its code. The trigram index, the server and the
ripgrep-compatible command line come from tgrep. MIT licensed; see
[LICENSE](LICENSE).
