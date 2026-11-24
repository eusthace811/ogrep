# Ranking, structure, graph, incident and history flags

All of these are deterministic. They are computed from the repository's files
and its git history, run offline, and print the file and line behind each
answer. `docs/REFERENCE.md` in the ogrep repository has the measurements.

## Prerequisites

| Flags | Needs |
|---|---|
| everything below except the next two rows | nothing beyond the normal index (they also run without one, by scanning) |
| `--near`, `--map`, `--callees`; better `--related-tests` | `ogrep index . --graph` (seconds; rerun after large changes; an older graph is still used, with a note) |
| `--history`, `--introduced` | `ogrep index . --history` (rerun to add new commits; fast) |
| `--suspects`, `--timeline`, `--conflicts`, `--history`, `--at`, `--changed-since`, `--template-to-code` | a git repository (`--template-to-code` otherwise prints templates without code) |

## Ranking

- **`--rank`** orders the files that matched by relevance: lines defining the
  match count most, then other matching lines (diminishing returns), then the
  match in the path. Tests, vendored or generated code and prose weigh less.
  Ties break by path, so the order is reproducible.
  - `--budget N`: at most N files (default 20).
  - `--per-file N`: at most N lines per file (default 3).
  - `--rank-cutoff auto`: also stop where scores drop below a quarter of the
    best, or halve from one file to the next.
  - `--explain-rank`: each file's score and features on a `#` line.
  - `--json` adds a `rank` record before each file.
  - It refuses `-v`, `--files`, `--files-without-match`, `--passthru`,
    `--include-zero`, `--sort` and `-r`.
- **`--rank-fit FILE`** grid-searches the ranking weights for one repository
  from `{"query": ..., "relevant": [paths]}` examples (JSON array or lines), and
  writes `rank-weights.json` beside the index. `--rank` then uses it.
- **`--about TEXT`** takes a description instead of a pattern; every positional
  argument is a path.
  - **Terms:** words become case-insensitive terms. Identifiers are split and
    also kept whole. Numbers, words under three letters and filler words are
    dropped.
  - **Scoring:** files are scored with BM25 plus `--rank`'s priors, and each file
    shows the lines with the most different terms.
  - **Prose:** Markdown, reStructuredText, AsciiDoc and `.txt` files with headings
    are also scored by their best heading section. That section is printed as a
    `path=LINE=Heading` row (a `scope` record whose `chain` lists the enclosing
    headings) and supplies the lines shown. Code files score exactly as before.
    On documentation tasks the right file came first 7–10 points more often, and
    the right section 8–26 points more often.
  - **Refusals:** `-e`, `-f`, `--ident`, `-w`, `-x` and `--at`. Also a text with
    no usable word, which exits 2: today that includes descriptions with no
    ASCII words, such as Chinese.
  - **Speed:** about 0.1-0.5 s on large repositories. `--rank` on a known name
    is faster.
  - **Confidence:** when the first two files score within 5%, it is unsure. Text
    output then adds one stderr line (silenced by `--no-messages` and `-q`).
    With `--json`, the first `rank` or `card` record carries
    `"confidence": {"level": "confident"|"unsure", "margin": ...}`, and the
    margin is 1 when only one file matched.
  - **What the levels mean:** unsure covers about 30% of queries. The first file
    is then right 41-55% of the time, and a right file is in the top five 77-92%
    of the time. When confident, the first file is right 78-79% of the time.
- **`--cards[=TOKENS]`** prints each kept file as a card instead of lines.
  - **Contents:** the path, a summary, the outline (definitions with those around
    the matches first, up to 12, or the heading tree for prose) and the matches
    marked `>`, stopping at about TOKENS (default 2000; the first card always
    prints).
  - **Combinations:** it implies `--rank`, works with `--about`, `--budget`,
    `--per-file` and `--explain-rank`, and refuses `-l`, `-c`,
    `--count-matches`, `-o`, `--vimgrep`, context flags and `--at`.
  - **JSON:** `card` records, each line with `line`, `depth`, `text` and
    `matched`.

## Structure

Built-in parsers cover Rust, Python, Go, JavaScript, TypeScript/TSX, Java, C,
C++, C#, Ruby and PHP. Other files fall back to keyword checks or count as code.
These flags combine with each other and with `--rank`.

- **`--scope`** prints the enclosing definition before each group of matches.
  JSON adds `scope` records with the whole chain of enclosing definitions.
- **`--kind def`** keeps only matches covering the whole name at a definition.
  **`--kind ref`** keeps the rest.
- **`--in code|comments|strings`** keeps matches starting in that kind of text.
- They cannot be combined with `-r`, and treat non-UTF-8 files as unparsed.

## Graph (after `ogrep index . --graph`)

- **`--near PATTERN`**: the files most related to the files PATTERN matches,
  with the names that link them.
- **`--map[=TOKENS]`**: an outline (definition lines only) of the matched files,
  then of related files, within about TOKENS (default 1024).
- **`--callers`**: each definition that uses PATTERN in code (not comments or
  strings), once, with its number of using lines. It doesn't need the graph.
- **`--callees NAME`**: for each definition of NAME, the repository names it
  calls or uses as types. Names too ambiguous to resolve are skipped.

## Tests, duplicates, branches, docs

- **`--related-tests FILE`**: test files ranked by name (`test_x.py`,
  `x_test.go`), use of FILE's definitions (with the graph) and commits shared
  with FILE. `--budget` sets how many (default 15).
- **`--similar PATH:LINE`**: definitions whose 5-token shingles overlap the
  definition at that line (Jaccard). It exits 1 when nothing is close enough.
- **`--conflicts`**: for each other local branch, the files changed on both
  sides since the common ancestor (committed or not) and whether the lines
  overlap.
- **`--doc-drift`**: names quoted as code in docs (`` `like_this` ``) that no
  code file contains. It skips changelogs, release notes and the docs' own
  examples.

## Incidents

- **`--entities`**: services, workloads, alerts and their dependencies.
  - **Sources:** read from Docker Compose, Kubernetes manifests, Backstage
    `catalog-info.yaml`, Prometheus rules and `.env` files.
  - **Edges:** each is `declared` or `inferred` (for example
    `CART_ADDR=cart:7070`), with its file and line. Telemetry and scraping
    edges are kept apart as `telemetry`/`observes`, so they don't inflate blast
    radius.
- **`--blast SERVICE`**: everything that depends on SERVICE, directly or not,
  grouped by distance, with evidence.
- **`--suspects A,B,...`**: candidate root causes for failing services A, B.
  - **Scoring:** by the share of failing services each explains, tripled if it
    changed since `--since` (default 24h), and discounted by distance.
  - **Evidence:** in a test on 30 injected incidents, the cause was first in 67%
    of cases and in the top three in all of them. Treat it as a ranked list of
    hypotheses to check, not a verdict.
- **`--timeline SERVICE`**: commits to SERVICE and to its direct dependencies,
  newest first (default: the last 7 days; `--since` changes it).
- **`--until WHEN`** (with `--suspects` or `--timeline`): ignore commits made
  after WHEN, to look at a past incident. Without `--since`, the default window
  counts back from WHEN; an explicit `--since` is taken as given.
- **`--stack-trace FILE`** (or `-` for stdin): frames from Python, Java/Kotlin,
  Go, Rust, JavaScript/TypeScript (or any `path:line`).
  - **Resolution:** each frame maps to the tracked file with the longest
    matching path suffix, so frames from containers or CI paths still resolve.
  - **Output:** the line and its enclosing definition.
- **`--log-templates PATTERN PATH...`** groups matching log lines (`.` for all)
  into templates.
  - **Templates:** numbers, ids, hex, IPs and durations become `<*>`. Each
    template shows its count, and first and last seen when lines start with ISO
    8601 timestamps.
  - **`--new-since WHEN`:** keeps only templates first seen after WHEN.
  - **`--template-to-code`:** finds the code that writes each template, from its
    constant two-word pieces looked up through the index (89-97% right file in
    tests). A message assembled in several calls may resolve to a helper.
  - **Input:** log files are read directly.
- **`--similar-incidents TEXT`** (or a file, or `-`): write-ups under paths
  containing `postmortem`, `incident`, `rca`, `retro` or `outage`, ranked by
  BM25, with shared service names counting double. `--budget` sets how many
  (default 10).

## History (after `ogrep index . --history`)

- **`--history PATTERN`**: commits that added or removed a matching line,
  newest first, with `+`/`-` lines.
  - **Accuracy:** it finds exactly the commits `git log -G` finds.
  - **Output options:** `-l` prints one line per commit, `-c` counts per commit,
    `-m N` limits lines per commit, and globs, types and paths restrict files.
  - **Filters:** `--since`, `--until` and `--author` filter commits (`--until` and `--author` only
    work with `--history` and `--introduced`).
  - **JSON:** `commit` records.
- **`--introduced PATTERN`**: the oldest commit that added a match, the newest
  that removed one, and how many files match now.
- **`--changed-since REV|WHEN`**: an ordinary search limited to files changed
  since REV (or since WHEN, such as `2h`), including uncommitted and untracked
  files. It also applies to `--files`, `--rank`, `--scope`, `--kind`, `--in` and
  `--callers`.
- **`--at REV`**: searches the tree at a commit, tag or branch from git's
  objects, without an index or a checkout.
  - **Refusals:** anything that reads the working tree after the search (see
    the skill).
  - **Paths:** they must exist in the working tree.

## JSON record types

Besides ripgrep's `begin`, `match`, `context`, `end` and `summary`, each flag
prints its own record types:

| Flag | Record type |
|---|---|
| `--rank`, `--about` | `rank` |
| `--cards` | `card` |
| `--scope`; `--about` on documentation (the best section) | `scope` |
| `--near` | `near` |
| `--map` | `map` |
| `--callees` | `callee` |
| `--entities` | `entity` |
| `--blast` | `blast` |
| `--suspects` | `suspect` |
| `--timeline` | `event` |
| `--stack-trace` | `frame` |
| `--log-templates` | `template` |
| `--similar-incidents` | `incident` |
| `--related-tests` | `test` |
| `--similar` | `similar` |
| `--conflicts` | `conflict` |
| `--doc-drift` | `drift` |
| `--history` | `commit` |
| `--introduced` | `introduced` |

Skip types you don't recognise.
