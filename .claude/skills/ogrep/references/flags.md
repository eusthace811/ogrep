# Search flags by task

Flags follow ripgrep. `ogrep --help` has the full list.

## What to match

| Flag | Effect |
|---|---|
| *(default)* | Rust regex syntax (like rg) |
| `-F` | literal text, with no regex metacharacters |
| `-i` / `-s` / `-S` | ignore case / case-sensitive / smart case. Smart case ignores case unless a literal character in any pattern is uppercase |
| `-w` / `-x` | whole word / whole line |
| `-v` | lines that do NOT match |
| `-e PAT` (repeatable) | several patterns, OR-ed together. Once `-e` or `-f` is used, every positional argument is a path |
| `-f FILE` | patterns from a file, one per line. `-` reads stdin, and a blank line matches everything |
| `-P` | PCRE-style engine: lookaround and backreferences. Without `-P`, patterns that need it switch to it automatically |
| `-U`, `--multiline-dotall` | matches that span lines. With `--multiline-dotall`, `.` also matches a newline |
| `--ident NAME` | an identifier in every naming style: camelCase, snake_case, PascalCase, kebab-case, SCREAMING_CASE |
| `--with PAT` (repeatable) | only files that also contain a line matching PAT. Every `--with` is required |
| `--without PAT` (repeatable) | skip files containing a line matching PAT |

## Which files

| Flag | Effect |
|---|---|
| `-t TYPE` / `-T TYPE` | include / exclude a file type. `--type-list` shows the types |
| `--type-add 'web:*.{html,css}'` | define a type |
| `-g GLOB` / `-g '!GLOB'` | include / exclude paths. Uses the index. Positive globs do not re-include ignored files unless you pass `--no-index` |
| `--iglob GLOB` | case-insensitive glob |
| `--hidden` | include hidden files and directories |
| `--no-ignore`, `-u`, `-uu`, `-uuu` | ignore `.gitignore`; then also hidden files; then also binary files. These scan instead of using the index |
| `--max-filesize 8M`, `--no-max-filesize` | change or lift the 64 MiB limit |
| `--max-depth N` | limit recursion |
| `--files` | list the searchable files; do not search |

## Output

| Flag | Effect |
|---|---|
| `-n` / `-N` | line numbers on / off. On by default only when writing to a terminal |
| `-l`, `--files-without-match` | file names with / without a match |
| `-c`, `--count-matches` | count matching lines / individual matches per file |
| `-o` | only the matched text |
| `-A N`, `-B N`, `-C N` | context lines after / before / around each match |
| `-m N` | at most N matching lines per file |
| `--column`, `-b` | column / byte offset |
| `--vimgrep` | `file:line:col:text`, one row per match |
| `--json` | ripgrep's JSON event stream: begin, match, context, end, summary |
| `-r TEXT` | show each match replaced by TEXT (`$1`, `${name}`); files are not changed |
| `-H` / `-I` | file names on / off |
| `--heading` / `--no-heading` | group by file / flat |
| `-0` | NUL after file names, for `xargs -0` |
| `--path-separator /` | forward slashes on Windows |
| `--sort path` | deterministic order |
| `-q` | no output, exit code only |
| `--color never` | no ANSI colors |

## Index and diagnostics

| Flag | Effect |
|---|---|
| `--stats`, `--explain` | stderr: query plan, candidate files, index work, time, and whether the server answered |
| `--no-index` | scan the files as they are on disk now |
| `--index-path DIR` | use an index somewhere other than `<path>/.ogrep` |
| `-j N` | number of search threads |

## Accepted but different from ripgrep

- These are accepted and ignored: `--mmap`, `--no-mmap`, `--no-config`, `--colors`.
- `--crlf` is rejected (exit 2), and so is `-z`/`--search-zip`.
