# ogrep reference

Everything about indexing, serving and searching in detail. For a quick start
see the [README](../README.md); for running ogrep inside an AI agent see
[AGENTS.md](../AGENTS.md); `ogrep --help` lists every flag.

## Architecture

```
ogrep <pattern> ---TCP---> ogrep serve (multi-client)
    (client)                   |
                          HybridIndex
                          /         \
                   IndexReader    LiveIndex
                   (mmap disk)   (in-memory overlay)
                        ^              ^
                        |              |
                  Periodic Flush  File Watcher (notify)
                  (50K files /    Background Indexer
                   5 min)         (rayon parallel)
```

- **IndexReader** — mmap'd on-disk index (zero-copy, binary search on sorted
  trigram lookup table)
- **LiveIndex** — in-memory overlay for files modified after server start, or
  being built by the background indexer
- **HybridIndex** — merges both layers; overlay takes precedence
- **Background Indexer** — builds the index in parallel batches of 1,024 files
  (with additional byte-based splitting); resumed partial indexes use batches of
  500 files. Clients scan until complete hidden-file coverage is published
- **Periodic Flush** — every 50K files or 5 minutes, the in-memory index is
  flushed to disk and the reader is swapped, keeping memory bounded
- **Automatic refresh** — native `notify` subscriptions update LiveIndex in
  real time when available; budget or registration failures switch to polling
- **Filename Index** — `--files` unions content-index paths with a compact
  sidecar containing only admitted paths that have no searchable content
- **TCP Server** — JSON-RPC 2.0 over newline-delimited TCP; each connection
  handled in a separate thread; multiple clients can connect simultaneously
- **File Cache** — 50K-entry content cache with RwLock for lock-free reads

## Usage

### Build the index

```bash
ogrep index .                          # index current directory
ogrep index /path/to/repo             # index a specific repo
ogrep index . --index-path /tmp/idx   # custom index location
ogrep index . --exclude vendor --exclude third_party  # skip directories
ogrep index . --no-sparse-grams       # trigrams only: smaller index
ogrep index . --history               # also index commit history (see --history)
ogrep index . --graph                 # also build the file graph (see --near, --map)
```

Besides trigrams, the index holds sparse n-grams: longer keys that let a long
literal be answered from a few short posting lists (see
[How It Works](#how-it-works)). They make the index about two to three times
larger. `--no-sparse-grams` leaves them out; searches then use trigrams alone
and return the same results. A server that later rebuilds the index adds the
n-grams back.

Each build reports its elapsed time and peak memory when it finishes:

```
Index built successfully at /tmp/idx
Indexed in 22.6s using external strategy (peak memory 160.1 MiB)
```

The peak is the memory the process itself holds — private/committed bytes, not
resident set. The two differ once large files are memory-mapped: mapped pages are
file-backed and reclaimable, so counting them would report the size of the files
being indexed rather than ogrep's own use. Indexing a single 2 GiB file holds
77.8 MiB while its working set reaches 1.99 GiB. When the working set is
substantially larger it is named alongside, so nothing is hidden:

```
Indexed in 46.1s using external strategy (peak memory 77.8 MiB private, 1.99 GiB working set incl. memory-mapped files)
```

#### Repositories without a `.git` directory

`.gitignore` files only take effect inside a git repository. This matches
ripgrep, which gates them the same way, but it surprises people indexing a
Perforce, Source Depot, or plain-directory enlistment: the root `.gitignore` is
read by nothing, and the only symptom is an index far larger than expected.

ogrep says so rather than leaving you to guess:

```
Walking /src/enlistment...
warning: /src/enlistment has a .gitignore but is not a git repository, so it is
not applied (this matches ripgrep). Pass --no-require-git to apply it.
Found 290018 text files (2893 binary skipped, 0 too large, 0 errors)
```

`--no-require-git` applies the rules anyway, and works on `index`, `serve`, and
search alike, so the index and your queries agree on which files exist:

```bash
ogrep index . --no-require-git
ogrep serve --no-require-git
```

#### Case-insensitive repositories

When git clones onto a filesystem that does not distinguish case — which on
Windows is every clone — it sets `core.ignorecase` and stops distinguishing case
when it matches ignore rules. A rule spelled `QLogs` then hides a directory
named `qlogs`.

Most tools, ripgrep included, always match ignore rules case-sensitively, so
that directory is walked, read and indexed even though `git status` never
mentions it. On one Windows enlistment that was a single 13.4 GiB build
artifact, 71% of the corpus, adding about 16 seconds to *every* query.

ogrep reads `core.ignorecase` and matches the way the repository itself does.
Files git **tracks** are exempt, which is git's own rule — ignore rules only
decide the fate of files git does not already know about. Without that
exemption the same change would have hidden 273 tracked `.JPG`, `.PNG` and
`.RLL` files caught by rules written in lower case.

On that enlistment the walk went from listing one file more than
`git ls-files --cached --others --exclude-standard` to matching it exactly, at a
cost of roughly 0.4 s on a 293k-file walk. `--no-ignore` turns it off along with
every other ignore source, and repositories that distinguish case are unaffected
and pay nothing.

Only the repository's own root `.gitignore` and `.git/info/exclude` are matched
this way. Rules in nested `.gitignore` files are not, because the walk does not
know they exist until it reaches their directory. Missing one only leaves a file
visible that git would hide, which is what every other tool does anyway.

#### Keep `index` and `serve` flags in step

Flags that decide *which files belong in the index* — `--no-require-git`,
`--no-ignore`, `--max-filesize`, `--exclude` — must match between the `ogrep
index` that built an index and the `ogrep serve` that serves it.

The server compares the index against the filesystem at startup and treats an
indexed file it cannot see as deleted. So serving an index built without a cap
under a `--max-filesize 8M` server drops every file above 8 MiB from that index,
permanently. Both sides default to 64 MiB, so this only bites when one side
names a limit; pass the same flags to both:

```bash
ogrep index . --max-filesize 8M
ogrep serve   --max-filesize 8M
```

#### Memory use on very large repos

Builds default to `--index-strategy=external`, which bounds peak memory with an
external merge sort: postings accumulate in a fixed-size arena that spills
sorted, compact segments to disk when full, and the segments are k-way merged
straight into the index. Peak memory is roughly flat in repo size rather than
linear.

If the arena never fills, nothing is spilled and the build takes exactly the
in-memory path, so small and mid-size repos pay nothing for this default.

```bash
ogrep index .                                 # external, 64 MB arena
ogrep index . --index-buffer 16               # smaller arena, lower peak
ogrep index . --index-strategy=memory         # opt out: sort entirely in RAM
```

On the Linux kernel (94,634 files, 990 MiB index), measured under the 1 MiB
indexing cap that was the default at the time, the default strategy is a **~17x
reduction in peak memory, and no slower**:

| Strategy | Spill segments | Peak working set | Build |
| --- | ---: | ---: | ---: |
| `external` (default, 64 MiB arena) | 31 | **160.1 MiB** | 22.6 s |
| `external --index-buffer 16` | 122 | 109.6 MiB | ~23 s |
| `memory` | - | 2.20 - 3.76 GiB | 23 - 32 s |

`--index-buffer` trades peak memory against merge fan-in. Bounded memory is also
*predictable* memory — the `memory` row varied by over a gigabyte across
identical runs because `Vec` growth doubles and both buffers are briefly
resident during the final reallocation, while `external` varied by 8 MiB.

The 1 MiB indexing cap those rows were measured under is now 64 MiB, and files
past 1 MiB are memory-mapped rather than read onto the heap. On the same repo the
`external` build now settles at roughly **152 MiB of private memory in 27 s**,
against 197-200 MiB and 41-42 s when every admitted file was read onto the heap.
The arena bound is unchanged; what changed is that a handful of 20 MB generated
headers no longer cost their full size in heap in every worker that touches one.
The 152 MiB figure was taken with no cap at all, and the 64 MiB default excludes
only files *above* 64 MiB, so it leaves these numbers alone here: the largest file
in the kernel tree is a 22.9 MiB generated AMD register header, and nothing in its
95,862 files reaches the cap.

Two caveats on reading those numbers. The peak ogrep prints is *private bytes*,
which excludes mapped file pages, so it reports what the process actually holds
rather than the size of the files it is reading — the same build is 152 MiB
private against ~192 MiB resident, and the working set is named alongside only
when it is substantially larger, as in [Build the index](#build-the-index).
macOS is the exception: `libc` does not surface the Mach counter that separates
the two, so it still reports resident set there. And a very large file that is
neither valid UTF-8 nor detectably binary still costs about its own size, because
the index has to hold the same repaired bytes a search will match against; a
135 MB Latin-1 file indexes at roughly 205 MiB.

Pass `--max-filesize` if a build has to fit a tighter budget, or
`--no-max-filesize` to lift the 64 MiB default entirely.

`--index-strategy=memory` remains available as an escape hatch for environments
where spilling is undesirable or impossible, such as a read-only or full index
volume. Both strategies produce byte-identical indexes from the same walk —
note that file IDs follow walk order, which the parallel walker does not fix
between runs, so two builds of the same tree need not be byte-identical to each
other.

`ogrep serve` uses the same bounded builder when it has to create an index from
scratch, so starting a server on an unindexed repo costs the same memory as
`ogrep index` (**148.6 MiB** rather than 1.6 GiB on the Linux kernel, and 2.6x
faster). While that first build runs, clients fall back to filesystem scans
rather than returning partial results; incremental updates after it completes
are unaffected.

### Start the server

```bash
ogrep serve .                          # start server (auto-builds index if missing)
ogrep serve . --index-path /tmp/idx    # custom index location
ogrep serve . --watch-mode poll        # poll without any native subscriptions
ogrep serve . --poll-interval 60       # polling cadence after fallback
ogrep serve . --watch-budget 4096      # lower this process's native watch ceiling
ogrep serve . --no-watch              # disable all automatic refresh
ogrep serve . --exclude node_modules   # exclude directories from indexing
```

The server builds the index in the background if none exists and resumes
incomplete builds. Clients scan the filesystem until the full corpus is ready;
`ogrep status` reports indexing progress and hidden-file coverage. Legacy
indexes are upgraded by startup reconciliation. Multiple clients can connect
simultaneously.

On Windows, replaced index generations remain under the index directory's
`.retired` folder while readers still have them memory-mapped. Cleanup uses
non-POSIX deletion so mapped files are not unlinked into NTFS's `$Deleted`
namespace. The server retries cleanup after publication, every minute even
when idle, and on startup. Only generations with a committed marker are
automatically removed; uncommitted backups are preserved for recovery.
This prevents new orphaned generations; it does not reclaim storage already
stranded in `$Deleted` by an older version.

Resource use during that initial build can be tuned. These apply to both
`ogrep serve` and `ogrep index`:

| Flag | Default | Effect |
|------|---------|--------|
| `--max-memory <MB>` | 50% of RAM (512 MB–16 GB) | Flush to disk once the in-memory index exceeds this, bounding peak memory |
| `--max-cpu <PERCENT>` | `50` | Confine parallel reading and trigram extraction to this share of logical cores |
| `--auto-save-mutations <N>` | `5000` | Accumulated index changes that trigger a background save; higher means fewer pauses but more to redo if killed |
| `--watcher-queue-cap <N>` | `16384` | Filesystem events buffered between the OS watcher and the indexing worker; raise it if bulk changes log watcher queue overflows, since each overflow forces a full stale check |

#### Staying in step with the filesystem

Automatic refresh has two modes, configured on `ogrep serve`:

| Flag | Default | Effect |
|------|---------|--------|
| `--watch-mode <auto\|poll>` | `auto` | Prefer native notifications with polling fallback, or use polling only |
| `--poll-interval <SECONDS>` | `120` | Wait after each polling reconciliation completes; range 1-86400 |
| `--watch-budget <N>` | `8192` | Conservative process-local native watch ceiling; range 1-4294967295 |
| `--no-watch` | off | Disable all automatic refresh: native watching, polling, and periodic reconciliation |

In `auto` mode, exceeding the watch budget, exhausting OS watch capacity, or
another error preventing complete native coverage switches the **whole
process** to polling. Status retains the specific fallback reason. This fallback
is sticky until the server restarts: it releases only this process's native watches and
does not keep retrying native registration. `poll` mode starts with **zero
native subscriptions**, including on Linux. It uses ogrep's metadata
reconciliation, not a second native watcher or `notify::PollWatcher`.

The default budget of 8192 is a conservative ceiling, **not an estimate of
free per-user capacity**. On Linux the inotify quota is shared with other
processes running as the same user; they may consume capacity before this
server reaches its budget. Raising the budget does not raise that shared quota.
ogrep does not probe the configured Linux quota: it uses this fixed ceiling
and authoritative OS registration errors. The budget counts one subscription
per admitted directory on Linux/Android; recursive backends count their single
root subscription as one.

The event buffer controlled by `--watcher-queue-cap` is separate: queue overflow
triggers recovery reconciliation, not watch-budget fallback. Only create,
modify, and remove events enter this buffer; read-only access notifications
(including Linux directory-open events from ogrep's own walks) are discarded
before queueing. Native rescan notifications still trigger recovery.

Polling walks the admitted tree and checks metadata for additions, changes,
and deletions. Its default cadence is completion-based: the next poll waits
120 seconds after the previous reconciliation finishes. Actual freshness also
includes scan/update time (and any in-progress indexing or save); it is not a
120-second hard freshness guarantee. Searches do not defer polling, and slow
scans do not cause overlapping polls or catch-up storms.
Fallback does not hide failures: a failed polling reconciliation or unreadable
input remains unhealthy and is reported in status.

Native notifications can also go missing, for example on a network or
virtualised filesystem. Native mode therefore retains its safety
reconciliation: about once an hour it walks the tree and compares it against
the index, waiting for a two-minute gap in queries and deferring no longer than
four hours. `--poll-interval` sets the polling cadence, not this native safety
cadence.

No-change metadata scans leave the index untouched; they do not rewrite it.
A changed delta merge may still rewrite the whole on-disk index. On Linux,
nanosecond mtime and ctime plus file identity detect ordinary writes even when
the modification timestamp is restored. Other OS/filesystem metadata
limitations remain: changes that preserve all available evidence may be
missed. A scan and its updates do not provide an atomic filesystem snapshot.

`--no-watch` still permits the initial build/startup reconciliation, but turns
off all subsequent automatic refresh. It cannot be combined explicitly with
`--watch-mode`, `--poll-interval`, or `--watch-budget`. Explicit
`--watch-mode poll` also rejects `--watch-budget`, since polling uses no native
watches. Inherited default values do not cause conflicts.

On Linux and Android, ogrep registers only the non-ignored directories with
inotify, avoiding watch-descriptor growth beneath ignored trees. This guarantee
is backend-specific: the implementation intentionally keeps one recursive
`ReadDirectoryChangesW` root subscription on Windows and one root FSEvents
stream on macOS, where ignored events are filtered after delivery and ignored
descendants remain watched. kqueue and `PollWatcher` are not covered.

#### Wait for your edits: `ogrep refresh`

The watcher is asynchronous, so a search issued right after an edit can run
before the server has seen it. When a search must see edits you just finished,
ask the server to catch up first:

```bash
ogrep refresh .                    # waits (default 30 s) until completed edits are searchable
ogrep refresh . --timeout-secs 120
ogrep refresh . --json             # prints the receipt
```

`refresh` rereads every eligible file under the served root and returns once the
result is published. Files whose content has not changed are skipped cheaply, so
a refresh of the unchanged 63K-file Rust tree takes about half a second. It
needs a running server for that exact root and index path. A timeout, an
unavailable or older server, or an incomplete refresh exits 2 and never falls
back to a search. Edits must be complete and stable while the refresh runs;
concurrent writes may or may not be included, since this is not an atomic
filesystem snapshot. For a single search of the current files, `--no-index`
scans directly instead.

### Search

```bash
ogrep "pattern" .                 # basic regex search
ogrep "pattern" file1.rs file2.rs # search multiple files/paths
ogrep "TODO|FIXME" .              # alternations
ogrep '\w+(?!_test)' .            # PCRE-style lookahead fallback
ogrep "error" . -i                # case-insensitive
ogrep "error" . -S                # smart-case (auto if all lowercase)
ogrep -F "Vec<T>" .               # literal string
ogrep "MyStruct" . -l             # filenames only
ogrep "pattern" . -c              # count per file
ogrep "pattern" . -o              # only matching text
ogrep "pattern" . -w              # whole word
ogrep "pattern" . -v              # invert match
ogrep "pattern" . -m 5            # max 5 matches per file
ogrep "pattern" . -g "*.rs"       # glob filter
ogrep "pattern" . -g "*.rs" -g "*.toml"  # multiple globs (OR)
ogrep "pattern" . -t rust         # type filter
ogrep "pattern" . -e "also_this"  # multiple patterns
ogrep "pattern" . -A 3            # 3 lines after match
ogrep "pattern" . -B 2            # 2 lines before match
ogrep "pattern" . -C 3            # 3 lines before & after
ogrep "pattern" . --json          # ripgrep-compatible JSON stream
ogrep "pattern" . --vimgrep       # vim-compatible output
ogrep "pattern" . --stats         # show query plan & timing
ogrep "pattern" . --no-index      # brute-force (skip index)
ogrep "pattern" . -U              # multiline matching
ogrep "pattern" . -q              # quiet: exit code only
ogrep "pattern" . --files-without-match  # files that DON'T match
ogrep "pattern" . --no-filename   # suppress filenames
ogrep "pattern" . -N              # suppress line numbers
ogrep --files .                   # list searchable files
ogrep --files src/main.rs         # list a single file if searchable
ogrep --files -t rust .           # list Rust files only
ogrep --type-list                 # show all file types
ogrep "pattern" . --rank          # best files first, 3 lines each, 20 files
ogrep "pattern" . --rank --budget 5 --per-file 1
ogrep "pattern" . --explain-rank  # ranked, with each file's score
```

#### Ranked results: `--rank`

`--rank` orders the files that matched by relevance and prints the best first,
with a few telling lines of each. It suits an agent or a person looking for
*where* something lives rather than every place it is mentioned. The search
itself is unchanged: every printed line is a real match, and ranking only
reorders and trims. Without `--rank` output is exactly as before.

A file scores higher for lines that define the match (`fn name`, `def name`,
`class Name`, `struct Name`, and the like), for other matching lines (with
diminishing returns), and for the match appearing in its path. Tests,
vendored or generated code, and prose weigh less. With several patterns (`-e`),
a rarer pattern counts for more. Within a file, definition lines are shown
first, then the earliest matches, skipping near-duplicate lines.

| Flag | Default | Effect |
|------|---------|--------|
| `--rank` | off | Rank files by relevance |
| `--budget <FILES>` | 20 | Print at most this many files; implies `--rank` |
| `--per-file <LINES>` | 3 | Print at most this many matching lines per file; implies `--rank` |
| `--rank-cutoff auto\|none` | `none` | `auto` also stops where scores fall below a quarter of the best, or halve from one file to the next (the top three are always kept); implies `--rank` |
| `--explain-rank` | off | Print each file's score and features on a line starting with `#`; implies `--rank` |

The order is deterministic: scores are compared at a fixed precision and ties
break by path, so two builds of the same tree rank identically. `-l` prints
ranked file names, `-c` ranked counts, `--json` the usual `begin`/`match`/`end`
messages in ranked order, each file preceded by a
`{"type":"rank","data":{"path","rank","score","features"}}` message. Context
flags keep only the context around the lines shown. `--rank` cannot be combined
with `-v`, `--files`, `--files-without-match`, `--passthru`, `--include-zero`
or `--sort`.

On 600 localization tasks mined from django and rust history (an identifier
from a commit message; the file the commit changed is the answer; see
`scripts/eval/localize.py`), the right file came first 58% of the time with
`--rank`, against 27% (django) and 41% (rust) in ogrep's default order.

With the default traversal rules, `--files` reads the live server or the local
index instead of walking the repository. The local result is an index snapshot;
use `--no-index` to inspect the filesystem as it exists now. Flags that change
traversal membership or the file-size policy also fall back to a walk.

### Structure: scopes, definitions, comments

```bash
ogrep -n --scope bulk_update .            # each match under its enclosing definition
ogrep -n --kind def bulk_update .         # where the name is defined
ogrep -n --kind ref bulk_update .         # every other use
ogrep -n --in comments TODO .             # only in comments (also: code, strings)
ogrep -n --scope --kind def -t rust Mutex # combine freely, with --rank too
```

ogrep parses the files that matched with built-in tree-sitter grammars for
Rust, Python, Go, JavaScript, TypeScript/TSX, Java, C, C++, C#, Ruby and PHP.

- **`--scope`** prints the definition enclosing each group of matches before
  them, the way `git grep --show-function` does: `path=line=text`, or
  `line=text` under a heading. JSON output adds a
  `{"type":"scope","data":{"path","line_number","lines","chain"}}` message
  whose `chain` lists every enclosing definition (kind, name, first and last
  line), outermost first.
- **`--kind def`** keeps matches that cover the whole name at a definition
  (`bulk_update` in `def bulk_update`, not in `def test_bulk_update`);
  **`--kind ref`** keeps the rest. Other languages fall back to a keyword check
  (`fn`, `def`, `class`, ... before the match).
- **`--in code|comments|strings`** keeps matches whose first byte is in that
  kind of text. In languages without a grammar every match counts as code.

Parsed structure is cached beside the index under `facts/`, keyed by a hash of
each file's content, so an edited file is simply parsed again and a cache entry
can never be stale. On the rust targeted queries the median `--scope` search
took 4.3 ms warm against 4.0 ms for a plain search (10.9 ms before the cache
held the files). Build with `--no-default-features` to leave the grammars out:
the binary is then about 9.5 MB instead of 28 MB, and these flags treat every
file as unparsed.

### Related files, maps, callers and callees

```bash
ogrep index . --graph                     # build the file graph (seconds)
ogrep --near -F 'class FilteredRelation' .   # files most related to what matched
ogrep --map=1500 -F bulk_update .         # outline of the related code, ~1500 tokens
ogrep --callers -w -F bulk_update .       # definitions that use bulk_update
ogrep --callees bulk_update .             # what bulk_update's definitions use
```

`ogrep index --graph` records which files use names that other files define.
Definitions come from the parsed structure; uses are the identifiers each file
mentions outside comments and strings. A name defined in more than 16 files, or
a protocol name like `__init__`, is too generic to link files; each link is
weighted by how rare its name is. On the rust compiler (63K files) the graph
has 2.1M links and 304K definitions and takes 1.7 s to build (0.6 s once the
structure cache is warm). It is stored under `graph/`; a search against a newer
index still uses it and says so.

- **`--near PATTERN`** lists the files most related to the files PATTERN
  matches: the weight of links to and from them, divided by the log of each
  file's total link weight so hubs do not win by volume, with the names that
  link them (`--budget` files, default 20). Measured on files changed in the
  same commit, this scoring found 26% of a file's co-changed files in its top
  10, against 16% for personalized PageRank.
- **`--map[=TOKENS]`** prints an outline, definition lines only, of the matched
  files and then the files related to them, within about TOKENS tokens (default
  1024): a repository map for an agent's context. Definitions around the
  matches, and names the matched files mention, come first.
- **`--callers`** lists each definition that uses PATTERN once, with its number
  of uses (parsed languages).
- **`--callees NAME`** lists, for each definition named NAME, the names it calls
  or uses as types that are defined in the repository, preferring definitions
  in the same file or in files it links to, and skipping names too ambiguous to
  resolve.

On the rust compiler `--near` and `--map` take about 120 ms and `--callees`
about 160 ms, mostly loading the graph; `--callers` takes about 10 ms. The graph
is not used for `--rank`: adding file centrality lowered ranking accuracy at
every weight tried.

### Tests, near-duplicates, conflicts, stale docs

```bash
ogrep --related-tests django/db/models/query.py .   # tests to run after changing a file
ogrep --similar django/db/backends/mysql/operations.py:60 .   # code like this definition
ogrep --conflicts .                                 # overlap with other local branches
ogrep --doc-drift .                                 # names in docs missing from the code
ogrep --rank-fit feedback.json .                    # tune --rank for this repository
```

- **`--related-tests FILE`** ranks test files by three signals: named after the
  file (`test_query.py`, `query_test.go`), using its definitions (with
  `ogrep index --graph`), and changed in the same commits (the last 400 commits
  touching the file).
- **`--similar PATH:LINE`** takes the definition around that line (or 30 lines)
  and ranks other definitions by the overlap of their 5-token shingles
  (Jaccard). Candidates are files that contain two of the region's longest
  identifiers, found through the index.
- **`--conflicts`** compares, for each other local branch, the lines changed
  on it since the common ancestor with the lines changed here (committed or
  not), and reports files changed on both sides and whether the lines overlap.
- **`--doc-drift`** lists names quoted as code in documentation (`` `like_this` ``
  or ``` ``LikeThis`` ```) that appear in no code file, skipping release notes and
  changelogs, which name removed things on purpose, and names the docs define in
  their own examples.
- **`--rank-fit FILE`** reads `{"query": ..., "relevant": [paths]}` examples (a
  JSON array or JSON lines), grid-searches the ranking weights for the best mean
  reciprocal rank, and writes them to `rank-weights.json` beside the index;
  `--rank` then uses them. The output stays deterministic: it depends only on
  that file. Trained on half of the django localization set, fitted weights
  raised the other half's MRR from 0.676 to 0.681 and Acc@5 from 0.833 to 0.873.

### Operations and incidents

```bash
ogrep --entities .                         # services, alerts, ... and their dependencies
ogrep --blast cart .                       # who depends on cart, by distance, with evidence
ogrep --suspects checkout,frontend --since 2h .   # likely causes, newest changes weighted
ogrep --timeline checkout --since 3d .     # commits to checkout and what it depends on
ogrep --stack-trace crash.txt .            # frames of a stack trace, in this repository
ogrep --log-templates . logs/app.log       # a log reduced to its message templates
ogrep --log-templates --new-since 2h . logs/   # messages that only started recently
ogrep --log-templates --template-to-code ERROR logs/   # and the code writing each one
ogrep --similar-incidents "checkout timeouts, redis pool" .   # related postmortems
```

**Entities and dependencies** come from the operations files a repository
already has: Docker Compose files (services, `depends_on`, `environment`),
Kubernetes manifests (workloads and container environment), Backstage
`catalog-info.yaml` (`dependsOn`, `consumesApis`), Prometheus alert rules (the
services an alert's labels name) and `.env` files, whose `${VAR}` references
are expanded. Every edge is marked **declared** (the file states it) or
**inferred** (a service's configuration points at another, such as
`CART_ADDR=cart:7070`), with the file and line it comes from. Where a service
sends telemetry (`OTEL_*` variables) and what a telemetry collector scrapes are
kept apart as `telemetry` and `observes` edges: they are not dependencies, and
counting them would put every service in every blast radius.

**`--blast SERVICE`** walks dependency edges backwards: everything that depends
on SERVICE, directly or not, grouped by distance. **`--suspects A,B,...`** ranks
the services the failing ones depend on (and the failing ones themselves) by the
share of failing services each explains, tripled when it changed since `--since`
(default 24 hours), and discounted by distance: a recently changed shared
dependency comes first, and a hub everything uses does not win without evidence.
Commits are attributed to a service through its code directory (`src/<name>`,
or the Compose build context). On 30 incidents injected into the OpenTelemetry
demo (a change to the root cause plus two noise commits; 1-3 failing dependents;
`scripts/eval/suspects_eval.py`) the root cause ranked first in 67% of cases and
in the top three in all of them, against 27% and 57% for ranking by shared
dependency alone.

**`--timeline SERVICE`** lists commits to SERVICE and to the services it depends
on directly, newest first (default: the last 7 days).

**`--stack-trace FILE`** (`-` for standard input) reads Python, Java/Kotlin, Go,
Rust and JavaScript/TypeScript frames, or any `path:line`, resolves each path to
the tracked file with the longest matching path suffix (so frames from
`site-packages`, containers or CI machines still land), and shows the line and
its enclosing definition.

**`--log-templates PATTERN PATH...`** groups the log lines PATTERN matches
(`.` for all) into templates, masking numbers, ids, hex, IPs and durations as
`<*>` and merging lines that agree in at least half of their positions (Drain).
Each template shows its count and, when lines start with an ISO 8601 timestamp,
when it was first and last seen. `--new-since WHEN` keeps templates first seen
after WHEN, the classic signal that something new started happening.
`--template-to-code` finds the code that writes each message (its constant text,
without the level word, in a tracked code file). Log files are read directly,
not through the index.

**`--similar-incidents TEXT`** (or a file, or `-`) ranks write-ups under paths
containing `postmortem`, `incident`, `rca`, `retro` or `outage` by BM25 over the
description, doubling the weight of shared service names.

A decision journal needs no flag: record hypotheses, evidence and verdicts as
JSON lines in a committed file (for example `ogrep-journal.jsonl`, one object
per line with `time`, `author`, `hypothesis`, `evidence` as `path:line` or the
query run, `verdict`, and `valid_to` when it stopped being true). Git keeps its
history, merges it line by line, and ogrep searches it like any other file.

### Search history and past versions

```bash
ogrep index . --history                     # also index what each commit changed
ogrep --history 'def bulk_update' .         # commits that added or removed a matching line
ogrep --history -l 'pool exhausted' .       # one line per commit
ogrep --history --since 2w --author jane 'timeout' .
ogrep --introduced 'pool exhausted' .       # first added, last removed, present now?
ogrep --changed-since HEAD~20 'TODO' .      # search only files changed since a commit
ogrep --changed-since 2h 'panic!' .         # ... or within a time window (with uncommitted changes)
ogrep --at v2.1.0 'parse_header' .          # search the tree as it was at a tag or commit
```

**`--history`** answers "which commits changed a line matching this?", the
question `git log -G PATTERN` answers, from an index instead of by diffing every
commit. `ogrep index --history` indexes the added and removed lines of each
non-merge commit reachable from HEAD (the most recent 20,000 by default;
`--history-depth N`, `0` for all; `--history-since WHEN`). Running it again adds
only the commits since the last run, and rebuilds if the branch was rewritten.

A search asks the history index which commits can match, has git re-diff just
those, and matches their changed lines with the same regex engine as a normal
search, so the commits found are exactly those `git log -G` finds over the same
commits. Output groups matching lines by commit, newest first:

```
commit 9cbdb44014c8 2018-09-18 Tom Forbes <tom@tomforb.es>
    Fixed #23646 -- Added QuerySet.bulk_update() to efficiently update many models.
django/db/models/query.py:476:+    def bulk_update(self, objs, fields, batch_size=None):
```

`+` marks an added line (numbered in the new file), `-` a removed one (numbered
in the old file). `-l` prints one line per commit, `-c` counts per commit, `-m N`
limits lines per commit, and `--json` prints one `{"type":"commit",...}` message
per commit. Globs, `-t` types and a path argument restrict which files' changes
count. `--since`, `--until` and `--author` filter commits.

**`--introduced`** reports the oldest commit that added a matching line, the
newest that removed one, and how many files match now.

| History | django (34K commits) | rust (20K most recent) |
|---|---:|---:|
| Build, from scratch | 16 s | 23 s |
| Size on disk | 260 MB | 156 MB |
| Add 100 new commits | 0.2 s | |
| `--history` median, 30-40 identifiers | 179 ms | 84 ms |
| `git log -G` median, same identifiers | 12.5 s | 17.1 s |

Commits whose changes exceed 1 MB (mass reformats, vendoring) are indexed in
chunks of whole files, so a search re-diffs only the files that can match. The
changes of a single commit beyond 64 MB are not indexed. A shallow clone's
boundary commits, which git diffs against an empty tree, are skipped.

**`--changed-since REV|WHEN`** restricts an ordinary search to files that differ
between REV (or the last commit before WHEN) and the working tree, including
uncommitted edits, plus untracked files that are not ignored. Unchanged files
are never read. It also applies to `--files`, `--rank`, `--scope`, `--kind`,
`--in` and `--callers`; the graph, repository and operations tools refuse it.

**`--at REV`** searches the files as they were at a commit, tag or branch,
reading git's object store directly: no index and no checkout. Globs, types,
`--max-filesize`, `--hidden`, `--rank` and the output formats work as usual;
as in a working-tree search, hidden files are skipped unless `--hidden` is
given, though `.ignore` files are not applied. Paths must exist in the working
tree. Whatever reads the working tree after the search refuses `--at`:
`--files`, `--scope`, `--kind`, `--in`, `--callers`, and the graph,
repository and operations tools.

### Check status

```bash
ogrep status .
```

```
Server status for /src/my-monorepo
  PID:        37980
  Port:       51043
  Files:      152
  Trigrams:   12265
  Cache:      2/50000
  Watcher:    active
  Watch mode: native (requested: auto)
  Watch budget: 8192
  Poll interval: 120s (after completion)
  Reconcile:  idle
  Last successful reconcile: 2m ago
  Reconcile pending: no
  Reconcile overdue: no
  Last reconcile duration: 42ms
  Indexing:   complete
```

Status retains the native `Watcher` indicator and reports the requested mode
(`auto`, `poll`, or `disabled`) and active mode (`native`, `poll`, `disabled`,
or `starting`). A polling server normally shows an inactive native watcher.
Fallback reasons, the last successful reconciliation, the latest attempt's
duration/error, and whether reconciliation is running, pending, or overdue help
distinguish a healthy polling server from one that has stopped refreshing.
Pending means catch-up work is requested; overdue means that work is pending
or the polling interval/native safety deadline has elapsed since completion.
Older servers without these fields still display their existing status.

### Count files

```bash
ogrep count-files .              # count text files (no server needed)
ogrep count-files /path/to/repo  # scan a specific repo
```

Prints the count to stdout (scriptable) and details to stderr:

```
284957
284957 text files (47516 binary skipped, 0 errors) in 1200ms
```

## CLI Flags

| Flag | Description |
|------|-------------|
| `-i, --ignore-case` | Case-insensitive matching |
| `-s, --case-sensitive` | Force case-sensitive matching (overrides `-S`) |
| `-S, --smart-case` | Case-insensitive unless a literal character in any pattern is uppercase (escapes like `\S` do not count) |
| `-F, --fixed-strings` | Treat pattern as a literal string |
| `-w, --word-regexp` | Match whole words only |
| `-v, --invert-match` | Show lines that do NOT match |
| `-o, --only-matching` | Print only the matched parts |
| `-e, --regexp <PAT>` | Additional pattern (repeatable for OR) |
| `-f, --file <FILE>` | Read patterns from file, one per line; `-` reads stdin (repeatable) |
| `--ident <NAME>` | Find an identifier in every naming convention: `getUserId`, `get_user_id`, `GetUserID`, `get-user-id`, `GET_USER_ID` (repeatable) |
| `--with <PAT>` | Only search files that also contain a line matching `PAT` (repeatable; all required; narrows via the index) |
| `--without <PAT>` | Skip files that contain a line matching `PAT` (repeatable) |
| `--rank` | Order files by relevance and print the best first (see [Ranked results](#ranked-results---rank)) |
| `--budget <FILES>` | With `--rank`: at most this many files (default 20) |
| `--per-file <LINES>` | With `--rank`: at most this many lines per file (default 3) |
| `--rank-cutoff <auto\|none>` | With `--rank`: also stop where scores fall away (`auto`) |
| `--explain-rank` | With `--rank`: print each file's score and features |
| `--scope` | Show the definition enclosing each match (`path=line=text`); see [Structure](#structure-scopes-definitions-comments) |
| `--kind <def\|ref>` | Keep matches that define the matched name, or the others |
| `--in <code\|comments\|strings>` | Keep matches in code, comments or string literals |
| `--near` | Files most related to what PATTERN matches (needs `ogrep index --graph`) |
| `--map[=TOKENS]` | Outline of related code within a token budget (needs the graph) |
| `--callers` | Definitions that use PATTERN, once each, with use counts |
| `--callees` | Names used by the definitions of PATTERN, and where they are defined (needs the graph) |
| `--entities` | Services, alerts and dependencies from operations files; PATTERN filters names |
| `--blast` | Everything that depends on the service PATTERN, by distance |
| `--suspects` | Likely causes for failing services PATTERN (comma-separated), with `--since` |
| `--timeline` | Commits to the service PATTERN and its dependencies, with `--since` |
| `--stack-trace` | Frames of the stack trace in file PATTERN (`-`: stdin), resolved to this repository |
| `--log-templates` | Group matching log lines into templates; `--new-since WHEN`, `--template-to-code` |
| `--similar-incidents` | Postmortems most similar to the description PATTERN |
| `--related-tests` | Tests most related to the file PATTERN |
| `--similar` | Code similar to the definition at PATTERN (`path:line`) |
| `--conflicts` | Files and lines changed both here and on other local branches |
| `--doc-drift` | Code names in docs that are missing from the code |
| `--rank-fit <FILE>` | Fit `--rank` weights to this repository from examples |
| `--history` | Search the lines commits added or removed (needs `ogrep index --history`); see [Search history](#search-history-and-past-versions) |
| `--introduced` | When a pattern first appeared, was last removed, and whether it is present now |
| `--since <WHEN>` / `--until <WHEN>` | With `--history`/`--introduced`: commit time window (`2 weeks ago`, `2024-05-01`, `2h`, `3d`) |
| `--author <PAT>` | With `--history`/`--introduced`: author regex (case-insensitive) |
| `--changed-since <REV\|WHEN>` | Search only files changed since a commit or time, with uncommitted changes |
| `--at <REV>` | Search the tree as it was at a git revision |
| `-U, --multiline` | Enable multiline matching (`.` still excludes `\n`) |
| `--multiline-dotall` | Make `.` match `\n`; implies `-U` |
| `-n, --line-number` | Show line numbers (default: on when stdout is a terminal) |
| `-N, --no-line-number` | Suppress line numbers |
| `-c, --count` | Print match count per file |
| `-l, --files-with-matches` | Print only filenames |
| `--files-without-match` | Print files that do NOT match |
| `-q, --quiet` | Suppress output; exit code only |
| `-m, --max-count <N>` | Limit matches per file |
| `-g, --glob <GLOB>` | Filter files by glob pattern, case-sensitive (repeatable) |
| `--iglob <GLOB>` | Case-insensitive glob filter (repeatable) |
| `--glob-case-insensitive` | Treat all `-g` globs as case-insensitive |
| `-t, --type <TYPE>` | Filter by file type (`rust`, `py`, `js`, …; repeatable) |
| `-T, --type-not <TYPE>` | Exclude a file type (repeatable) |
| `--type-add <SPEC>` | Add/extend a type, e.g. `--type-add 'web:*.html'` |
| `--type-clear <TYPE>` | Remove a type's definitions |
| `--type-list` | Print all supported file types (reflects `--type-add`/`--type-clear`) |
| `--files` | List files that would be searched |
| `-A, --after-context <N>` | Lines of context after match |
| `-B, --before-context <N>` | Lines of context before match |
| `-C, --context <N>` | Lines of context before and after |
| `--heading / --no-heading` | Grouped vs flat output |
| `-H, --with-filename` | Show filenames (default: on unless a single file was named) |
| `-I, --no-filename` | Suppress filenames in output |
| `--json` | ripgrep-compatible JSON stream (one object per line) |
| `--vimgrep` | Vim-compatible `file:line:col:content`, one row per match |
| `--color auto/always/never` | Color mode control |
| `-0, --null` | NUL byte filename separator (for xargs) |
| `--trim` | Trim leading/trailing whitespace |
| `-., --hidden` | Include hidden files and directories |
| `--no-ignore` | Don't respect `.gitignore` or `p4ignore.ini` files |
| `-a, --text` | Search binary files as if they were text |
| `--binary` | Search binary files, reporting a note instead of their contents |
| `-u, --unrestricted` | Unrestricted: `-u` = no-ignore, `-uu` = +hidden, `-uuu` = +binary |
| `--max-filesize <SIZE>` | Skip files larger than `SIZE` (`K`/`M`/`G` suffixes); default 64M |
| `--no-max-filesize` | Apply no size limit, as ripgrep does |
| `-L, --follow` | Follow symbolic links |
| `--no-messages` | Suppress error messages about unreadable/missing paths |
| `--no-index` | Skip index, grep all files |
| `--exclude <DIR>` | Exclude directory from indexing (repeatable); `index` and `serve` only, not accepted by a search |
| `--stats`, `--explain` | Run the search and print the query plan, candidate counts and index work on stderr |
| `--index-path <DIR>` | Custom index directory |

**Pattern matching**

| Flag | Description |
|------|-------------|
| `-x, --line-regexp` | The pattern must match a whole line (beats `-w`) |
| `-P, --pcre2` | Use the backtracking engine (lookaround, backreferences) |
| `--engine <auto\|default\|pcre2>` | Pick the regex engine explicitly; `auto` falls back to `pcre2` |
| `--pcre2-version` | Print the backtracking engine in use and exit |
| `--no-unicode` | Disable Unicode-aware character classes |
| `--regex-size-limit <SIZE>` | Cap the compiled regex size (`K`/`M`/`G` suffixes) |
| `--dfa-size-limit <SIZE>` | Cap the regex DFA cache size |
| `-r, --replace <TEXT>` | Replace each match; `$1`/`${name}` expand capture groups |
| `--passthru` | Print every line, matching or not |
| `--stop-on-nonmatch` | Stop searching a file at its first non-matching line |

**Output formatting**

| Flag | Description |
|------|-------------|
| `--column` / `--no-column` | Show the 1-based column of the first match |
| `-b, --byte-offset` | Show the byte offset of the line (or match, with `-o`) |
| `-M, --max-columns <N>` | Omit lines longer than `N` bytes |
| `--max-columns-preview` | Show a truncated preview instead of omitting |
| `--count-matches` | Count matches rather than matching lines |
| `--include-zero` | With `-c`, also print files with a count of `0` |
| `-p, --pretty` | Alias for `--color always --heading -n` |
| `--context-separator <SEP>` | Separator between context groups (default `--`) |
| `--no-context-separator` | Print no separator between context groups |
| `--field-match-separator <SEP>` | Separator between match fields (default `:`) |
| `--field-context-separator <SEP>` | Separator between context fields (default `-`) |
| `--path-separator <SEP>` | Rewrite the separator in printed paths |
| `--sort <KEY>` / `--sortr <KEY>` | Sort by `path`/`modified`/`accessed`/`created`/`none` |
| `--sort-files` | Shorthand for `--sort path` |
| `--line-buffered` / `--block-buffered` | Force line- or block-buffered stdout |

**Encoding**

| Flag | Description |
|------|-------------|
| `-E, --encoding <LABEL>` | Decode files as `LABEL` (e.g. `utf-16le`, `latin1`, `sjis`), or `none` for raw bytes |
| `--no-encoding` | Restore BOM-sniffing auto-detection |

By default ogrep sniffs a UTF-8/UTF-16LE/UTF-16BE BOM and decodes accordingly,
so BOM-marked UTF-16 files are searched as text rather than reported as binary.
A BOM always wins over `-E`, matching ripgrep. Because the index is built with
auto-detection, an explicit `-E` bypasses the index and server and searches
files directly, so results stay correct.

**File walking**

| Flag | Description |
|------|-------------|
| `--max-depth <N>` | Limit directory recursion depth |
| `--one-file-system` | Don't cross file-system boundaries |
| `--ignore-file <FILE>` | Read extra ignore rules from `FILE` (repeatable) |
| `--ignore-file-case-insensitive` | Match ignore rules case-insensitively |
| `--no-ignore-dot` | Ignore `.ignore` files |
| `--no-ignore-exclude` | Ignore `.git/info/exclude` |
| `--no-ignore-files` | Ignore any `--ignore-file` arguments |
| `--no-ignore-global` | Ignore the global gitignore |
| `--no-ignore-parent` | Ignore rules from parent directories |
| `--no-ignore-vcs` | Ignore `.gitignore` files |
| `--no-ignore-messages` | Suppress errors about malformed ignore files |
| `--no-require-git` | Apply git ignore rules outside a git repository |
| `-j, --threads <N>` | Number of search threads |

**Accepted for compatibility**

`--mmap`/`--no-mmap` (ogrep always reads files directly), `--no-crlf` (the
default: CR bytes are kept in output), `--no-config` (ogrep reads no config
file), and `--colors <SPEC>` (colors are not yet configurable) are accepted and
ignored so ripgrep command lines keep working. `--debug`/`--trace` imply
`--stats`. `--crlf` is rejected with exit 2, since ogrep does not implement
CRLF-aware anchors; use ripgrep for that.

For indexed content searches, `--stats` reports the query plan and candidate
counts with or without a server. **Raw candidates** are the files selected by
the trigram index before path, visibility, glob, and type filtering.
**Candidates** counts the files remaining after those filters, but before
`--max-filesize` checks, file reads, or matching, on both local and server searches.
If the raw set covers the entire nonempty index, the summary says **`no index narrowing`**,
even when a usable trigram plan exists. **`(via server)`** describes transport,
not whether the index reduced the search. Older servers that do not return
candidate statistics retain the transport-only summary.

Local searches also print `Index work: {...}`, which counts what the planner did:

| Counter | Meaning |
| --- | --- |
| `lists_loaded` | Full posting lists requested |
| `lists_probed` | Posting lists probed for surviving candidate IDs |
| `postings_copied` | Entries returned from list loads and probes |
| `candidate_probes` | IDs requested from probes |
| `position_rejections` | Candidates rejected by incompatible position residues |

These are work counts, not bytes read or allocations. Broad queries and short
literals can still require extensive scanning.

Inline case flags such as `(?i)needle` and `(?i:needle)` participate in trigram
planning just like `-i`, including with `-P`. Scoped `(?-i:...)` still controls
matching. Only classes with a single ASCII-folded byte are combined into
literal runs; Unicode case-fold alternatives and other classes conservatively
break those runs so indexed searches do not lose matches.

`-z/--search-zip` is **not** supported and exits with code `2` rather than
silently reporting no matches in compressed files.

> **Note:** `-L` means `--follow` (as in ripgrep). Use the long
> `--files-without-match` for the non-matching-files listing.

### Patterns and paths

Without `-e`/`-f`, the first positional argument is the pattern and the rest are
paths. As soon as `-e` or `-f` supplies a pattern, **every** positional becomes
a path, matching ripgrep:

```bash
ogrep -e needle .            # searches for "needle" under .
ogrep -e needle -e other .   # both patterns, still just one path
```

### Output defaults

ogrep matches ripgrep's context-dependent defaults rather than fixed ones:

- **Line numbers** are on only when stdout is a terminal. Piping to another
  command drops them, so `ogrep needle . | cut -d: -f1` behaves the same as it
  does with ripgrep. `--column`, `--vimgrep` and `-p` turn them back on;
  `-b` and `-A/-B/-C` do not.
- **Filenames** are shown unless you named exactly one *file*. A directory
  argument always shows them. `-H`/`-I` override either way.
- **Paths** are printed by appending onto the argument you typed: the argument
  survives verbatim and only the appended part uses the platform separator. So
  `ogrep needle src/` prints `src/main.rs` while `ogrep needle src` prints
  `src\main.rs` on Windows. `--path-separator` rewrites every separator.

### Match limits

`-m/--max-count` limits matching *lines*, as ripgrep does, not individual
matches. A line holding several matches spends one unit of the budget and all
of its matches are still reported, so `ogrep -m1 --vimgrep foo` prints one row
per match on the first matching line. Under `-U/--multiline` the unit is the
contiguous block of lines a match covers, which keeps a match that straddles a
line boundary whole instead of truncating it mid-pattern.

One divergence: ripgrep stops reading a file once the limit is reached, so its
`--stats` `bytes_searched` is lower than ogrep's, which searches from a
whole-file buffer. The match counts themselves agree.

### Multiline matches

`-U/--multiline` lets a match cross line boundaries, and every line a match
covers is printed. `--vimgrep` is the exception: it reports one row per match
so editors get one jump target each, so a match spanning several lines is
reported only on the line it starts on.

Two divergences, both cases where ripgrep names a column that doesn't exist on
the line it prints — `rg -U --column` reports the same column for every line of
a match, and `rg -U --vimgrep -o` can report column 19 of a 7-character line.
ogrep reports the real match position instead.

### Binary files

A file is binary if it contains a NUL byte. Following ripgrep:

- Binary files found by walking a directory are **skipped silently** — they
  appear in neither the output, `-l`, `-c`, nor `--files-without-match`.
- A binary file **named explicitly** on the command line reports a note:
  `bin.dat: binary file matches (found "\0" byte around offset 7)`.
- `--binary` promotes traversal to the explicit behaviour, so binary files are
  searched and summarised with that note.
- `-a`/`--text` disables binary detection entirely and prints matches as text.
- `--json` has no note. As in ripgrep, the matching lines are emitted as
  ordinary `match` events and the file's `end` message carries
  `binary_offset` — the offset of the first NUL — so a consumer can still tell
  a binary hit apart from a text one. `stats.bytes_searched` stops at that
  offset rather than counting the whole file.

ogrep additionally rejects ~65 binary file *extensions* during the walk to keep
indexing cheap, which ripgrep does not do. `--binary` and `-a` also lift that
restriction, and `--files` never applies it.

### Flags that bypass the index

`index` and `serve` include hidden, **non-ignored** files by default.
`--hidden` is accepted on either command but is redundant. Ordinary queries
still hide hidden files and directories; `-./--hidden` includes them using the
index or server, for content searches and `--files`. Visibility is relative to
the requested search root and includes Windows hidden attributes. Ignore rules
inside hidden directories remain active. The configured index directory,
including its staging and retired generations, is always excluded from indexing
and query filesystem walks.

Flags that widen or re-interpret the indexed corpus still walk the tree:
non-`auto` `-E/--encoding`, `-a/--text`, `--binary`, and ignore-disabling flags
such as `--no-ignore`. Naming a single file also skips the index, since reading
one file directly is cheaper than loading one.

Both positive and negative `--glob`/`--iglob` patterns filter the indexed corpus
when a compatible index or server is available, for content searches and
`--files`. Positive globs such as `--glob '*.ts'` do not reinclude ignored files
in indexed mode. Use `--no-index` when those matches are needed: filesystem
scans retain ripgrep-style glob overrides, which can reinclude ignored files.
Negative globs, such as `--glob '!.git'`, exclude entire matching directory
subtrees. `--hidden` never disables ignore rules.

Legacy indexes without proven hidden-file coverage and incomplete builds fall
back to a filesystem scan, rather than returning partial results. Run
`ogrep index` to rebuild, or start a current `ogrep serve` to reconcile and
upgrade an existing index automatically. Queries keep scanning until coverage
is established. A current client also falls back when an older server cannot
confirm that capability. As before, a completed on-disk index is a snapshot,
not a guarantee that later filesystem changes have been indexed.

New indexes use a versioned file table and filename sidecar so older clients
cannot silently expose hidden files by ignoring visibility metadata. Older
binaries reject the new local content index; filename listing can fall back to
walking. New binaries still read older formats to migrate them. To downgrade,
rebuild with the older binary, preferably at a separate `--index-path`.
Visibility is bound to the path table, and filename-only membership is
published together with its visibility; mismatched generations fall back to
scanning rather than guessing.

### Invalid UTF-8

ripgrep searches raw bytes. ogrep decodes first, repairing any undecodable byte
into a `U+FFFD`, which is what makes the trigram index possible. Reported
positions are mapped back, so `--column`, `-b`, `--vimgrep` and `-r` all report
the byte offsets on disk exactly as ripgrep does. Two differences remain on
lines that are not valid UTF-8:

- A pattern can match the substituted `U+FFFD`; ripgrep, seeing raw bytes, never
  matches there. So `ogrep '.' ` finds one more match per repaired byte.
- `--json` always reports `lines.text` with the substitutions in place, and
  submatch offsets that index it. ripgrep instead emits `lines.bytes` as base64
  and reports source offsets. `absolute_offset` is a real file offset either
  way.

### File size limits

Searching and indexing both skip files larger than **64 MiB** by default. This
is a deliberate divergence from ripgrep, which has no default limit.

The divergence is affordable because ogrep is not a one-shot scanner. A file a
walk picks up is also a file the index carries and re-reads on every query whose
trigrams make it a candidate, so an outlier's cost is paid repeatedly rather
than once. On a 292,911-file enlistment where one 13.41 GiB generated build
artifact was 71% of all searchable bytes, the cap cut a cold index build from
214.5 s to 64.2 s and a warm query from 21.30 s to 0.55 s — a 39x difference —
at the cost of one match in one generated file, and it excluded 2 files out of
292,911.

The cost is real: an oversized file is counted but its path is never recorded,
so a match inside one is reported as no match. Two rules keep that from being
silent:

- `--no-max-filesize` restores the uncapped, ripgrep-identical behaviour, and
  `--max-filesize` sets a different bound.
- A file named directly on the command line is never dropped by the *inherited*
  default, only by a limit you passed. `ogrep pattern ./huge.log` searches
  `huge.log`.

The limit is resolved once, before the walk and the search diverge, so an index
and the queries against it always agree on which files exist. A walk that capped
where the search did not would be indistinguishable from "this file contains no
match".

The cap is not what bounds memory. Files past 1 MiB are memory-mapped during
both indexing and searching, so their pages are file-backed and reclaimable, and
mapped files are batched by what they actually put on the heap rather than by
their length — so a build over large files fills its worker pool instead of
running two files at a time. Combined with a faster trigram extractor, that made
indexing a tree of 32 MiB source files about 14x faster. Nor is size a good
proxy for *index* cost: oversized files are overwhelmingly generated and
therefore repetitive, contributing far fewer distinct trigrams per byte than
ordinary source. What the cap buys is bounded *scan* time.

### Exit codes

Same as ripgrep:

| Code | Meaning |
|------|---------|
| `0` | At least one match was found |
| `1` | No matches |
| `2` | An error occurred (e.g. a path could not be read) |

A match plus an error yields `2`, unless `-q` is set, which yields `0`.

An ignore file that fails to parse is *not* one of these errors. Like ripgrep,
ogrep reports it on stderr, skips the offending rule and carries on, leaving the
exit code determined by the search alone. Suppress the message with
`--no-ignore-messages`, or with `--no-messages`, which covers it as well.

## How It Works

1. **Indexing** — walks the repo (respecting `.gitignore` and root-level
   `p4ignore.ini`), skips binary files
   by extension (50+ formats) and content check (first 8KB), extracts all
   overlapping 3-byte trigrams from each text file in parallel (rayon), and
   writes a compact binary inverted index. Full builds stream sorted posting
   groups directly to disk to keep peak memory bounded.

   Alongside the trigrams it indexes **sparse n-grams**, the scheme GitHub
   code search and Cursor use: substrings of 4-8 bytes whose first and last
   byte pairs both outweigh every pair inside them, with pairs weighted by how
   rare they are in source code. The rule looks only at the substring's own
   bytes, so every n-gram of a search literal is present in any text that
   contains it. They are taken from lowercased text, so they serve
   case-sensitive and case-insensitive searches alike. N-grams that occur in
   more than 1% of the files (and at least 32) are left out, since they narrow
   little; their keys are listed in the index so no search mistakes their
   absence for a miss.

2. **Querying** — the regex is parsed with `regex-syntax`, decomposed into
   literal fragments, converted to trigram hashes, and looked up via binary
   search in the mmap'd index. Posting lists are intersected (AND) or
   unioned (OR) to find candidate files, reusing sorted posting-list order when
   possible. Only those candidates are verified with the full regex engine in
   parallel (rayon).

   Against an index with sparse n-grams, each literal is covered by as few
   keys as possible, preferring the longest n-grams; its remaining trigrams are
   consulted only while more than 32 candidates remain. An index without
   n-grams (older, or built with `--no-sparse-grams`) is searched with
   trigrams alone.

3. **Serving** — `ogrep serve` wraps the index in a HybridIndex, watches for
   filesystem changes, and serves queries over TCP. If no index exists, it
   builds one in the background (batches of 1,024 files, split sooner by byte
   budgets); clients scan until complete coverage is published, including when
   resuming a partial index;
   after the initial build, pending changes are auto-saved when 5,000 content mutations accumulate by default, or on the first periodic check at least 10 minutes after startup or the last successful save. Multiple clients connect simultaneously;


   searches use read locks for zero contention.

## On-Disk Format

| File | Description |
|------|-------------|
| `lookup.bin` | Sorted 16-byte entries: `key(u32) + offset(u64) + length(u32)`. Trigram keys are below 2^24; sparse n-gram keys (hashes) lie above; the last key, `0xFFFFFFFF`, lists the n-grams left out as too common |
| `index.bin` | Concatenated posting lists: `file_id(u32)` per entry |
| `files.bin` | Version 3 header, then `file_id(u32) + path_len(u16) + path_bytes`; legacy headerless tables remain readable |
| `files-extra.bin` | Version 2 filename-only paths, visibility, and file-table identity |
| `facts/structure-v2/` | Cache of parsed file structure, keyed by grammar and content: `index.bin` (sorted key, offset, length) and `data.bin` (payloads). A store with another version (such as `structure-v1`) belongs to another ogrep release and is never read by this one; it is left in place in case that release still shares the index, and can be deleted when none is running |
| `rank-weights.json` | Weights fitted by `--rank-fit`, used by `--rank` |
| `graph/graph.bin` | File graph (`ogrep index --graph`): files, links, PageRank, definitions |
| `history/` | History index (`ogrep index --history`): an index of commit diffs, `history.json` and `chunks.json` |
| `meta.json` | Version, file/trigram counts, timestamps, coverage, visibility, file-table identity, and the sparse n-gram version and key count (`sparse_grams`, `sparse_gram_keys`; absent when the index has none) |
| `serve.json` | Server PID and TCP port (for client discovery) |
