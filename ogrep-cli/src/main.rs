/// ogrep — trigram-indexed grep with client/server architecture.
///
/// Usage:
///   ogrep index [path]           Build the trigram index
///   ogrep serve [path]           Start the search server
///   ogrep <pattern> [path]       Search (auto-delegates to server)
///   ogrep status [path]          Show index/server status
mod admission;
mod cards;
mod cpu;
mod extras;
mod glob_filter;
mod graph_search;
mod history_search;
mod index;
mod matching;
mod mem;
mod ops_search;
mod output;
mod post;
mod profile;
mod rank;
mod refresh;
mod search;
mod serve;
mod status;
mod suggest;
mod walkcount;

use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use ogrep_core::builder;
use output::ColorMode;

/// Set when anything is reported to stderr, mirroring ripgrep's `messages`
/// module, which drives the exit code.
static ERRORED: AtomicBool = AtomicBool::new(false);

/// A tool flag (see [`Cli::tools`]). Every tool works on one path.
struct Tool {
    flag: &'static str,
    takes: Takes,
    /// The default of `--budget`, when it sets how many results the tool
    /// prints (rather than implying --rank).
    budget: Option<usize>,
}

/// What a tool reads from the pattern argument.
#[derive(Clone, Copy, PartialEq)]
enum Takes {
    /// A search pattern, from the argument, -e, -f or --ident.
    Pattern,
    /// One name, path or service, from the argument only.
    Subject,
    /// Nothing: a positional argument is the path.
    Nothing,
}

#[derive(Parser)]
#[command(
    name = "ogrep",
    about = "Trigram-indexed grep — fast regex search for large codebases",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Search pattern (when not using a subcommand).
    #[arg(global = false)]
    pattern: Option<String>,

    /// Root directories or files to search.
    #[arg(global = false, value_name = "PATH", num_args = 0..)]
    paths: Vec<String>,

    // ── Matching ──────────────────────────────────────
    /// Case-insensitive matching.
    #[arg(short = 'i', long = "ignore-case", global = true)]
    ignore_case: bool,

    /// Force case-sensitive matching (overrides --smart-case).
    #[arg(short = 's', long = "case-sensitive", global = true)]
    case_sensitive: bool,

    /// Smart case: case-insensitive when the patterns match some character
    /// literally and no such character is uppercase. Every pattern counts (-e,
    /// -f, --ident, --with and --without included); escapes like \S or \p{Lu}
    /// are not literals. Under -P, or when lookaround or backreferences send the
    /// patterns to the PCRE-style engine, every character counts except one
    /// after a backslash.
    #[arg(short = 'S', long = "smart-case", global = true)]
    smart_case: bool,

    /// Treat pattern as a literal string.
    #[arg(short = 'F', long = "fixed-strings", global = true)]
    fixed_strings: bool,

    /// Match only when neither adjacent character is a word character. The
    /// later of -w and -x wins.
    #[arg(
        short = 'w',
        long = "word-regexp",
        global = true,
        group = "match_boundary",
        overrides_with = "line_regexp"
    )]
    word_regexp: bool,

    /// Invert match: show lines that do NOT match.
    #[arg(short = 'v', long = "invert-match", global = true)]
    invert_match: bool,

    /// Additional patterns (can be specified multiple times).
    #[arg(short = 'e', long = "regexp", global = true)]
    regexp: Vec<String>,

    /// Read patterns from FILE, one per line; `-` reads standard input
    /// (repeatable). A blank line is an empty pattern, which matches every line.
    #[arg(short = 'f', long = "file", value_name = "FILE", global = true)]
    pattern_file: Vec<String>,

    /// Search for an identifier in any naming convention: `--ident getUserId`
    /// also finds get_user_id, GetUserID, get-user-id and GET_USER_ID
    /// (repeatable; combines with -e as alternatives).
    #[arg(long = "ident", value_name = "NAME", global = true)]
    ident: Vec<String>,

    /// Only search files that also contain a line matching PATTERN (repeatable;
    /// every one is required). Uses the index to narrow candidates.
    #[arg(long = "with", value_name = "PATTERN", global = true)]
    with: Vec<String>,

    /// Order files by relevance (definitions, match density, file-name hits;
    /// tests, vendored code and prose count less) and print the best first,
    /// with a few telling lines each. Output order is deterministic.
    #[arg(long = "rank")]
    rank: bool,

    /// Find the files a description is about: a task, a bug report, a commit
    /// message. Its words (identifiers split into their parts) are searched
    /// case-insensitively and files are ranked by how many of the rarer words
    /// they hold, as --rank prints them (--budget, --per-file and
    /// --explain-rank apply). Every positional argument is a path.
    #[arg(long = "about", value_name = "TEXT")]
    about: Option<String>,

    /// Print each ranked file as a card: its path, a one-line summary, its
    /// outline (definitions, or headings for prose) with the matching lines
    /// marked `>`, within about TOKENS tokens in all (default 2000). Implies
    /// --rank; works with --about
    #[arg(
        long = "cards",
        value_name = "TOKENS",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "2000",
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    cards: Option<u64>,

    /// With --rank: print at most this many files (default 20). Implies --rank,
    /// except with --near, --related-tests, --similar and --similar-incidents,
    /// where it is how many results they print (default 20, 15, 15 and 10).
    #[arg(long = "budget", value_name = "FILES", value_parser = clap::value_parser!(u64).range(1..))]
    budget: Option<u64>,

    /// With --rank: print at most this many matching lines per file (default 3).
    /// Implies --rank.
    #[arg(long = "per-file", value_name = "LINES", value_parser = clap::value_parser!(u64).range(1..))]
    per_file: Option<u64>,

    /// With --rank: `auto` also stops where scores fall away; `none` (default)
    /// keeps every file up to --budget. Implies --rank.
    #[arg(long = "rank-cutoff", value_name = "MODE", value_parser = ["auto", "none"])]
    rank_cutoff: Option<String>,

    /// Fit --rank's weights to this repository from FILE: a JSON array (or
    /// JSON lines) of `{"query": ..., "relevant": [paths]}`, the files each
    /// query should rank first. Writes `rank-weights.json` beside the index,
    /// which --rank then uses; delete it to return to the defaults.
    #[arg(long = "rank-fit", value_name = "FILE")]
    rank_fit: Option<PathBuf>,

    /// With --rank: print each file's score and the features behind it, on a
    /// line starting with `#`. Implies --rank.
    #[arg(long = "explain-rank")]
    explain_rank: bool,

    /// Show the definition (function, method, class, ...) enclosing each match,
    /// as `path=line=text` before its matches, like `git grep -p`. Uses the
    /// built-in parsers (Rust, Python, Go, JavaScript, TypeScript, Java, C,
    /// C++, C#, Ruby, PHP).
    #[arg(long = "scope")]
    scope: bool,

    /// Keep only matches that define the matched name (`def`) or only the
    /// others (`ref`). Parsed languages use their grammar; others fall back to
    /// a keyword check (`fn name`, `def name`, `class Name`, ...).
    #[arg(long = "kind", value_name = "def|ref", value_parser = ["def", "ref"])]
    kind: Option<String>,

    /// Keep only matches in code, comments or string literals (parsed
    /// languages; other files count as code).
    #[arg(long = "in", value_name = "code|comments|strings", value_parser = ["code", "comments", "strings"])]
    in_zone: Option<String>,

    /// List the files most related to the files PATTERN matches (they use or
    /// are used by them), best first, with the names that link them. Needs
    /// `ogrep index --graph`. `--budget` sets how many (default 20).
    #[arg(long = "near")]
    near: bool,

    /// Print an outline (definition lines) of the files most related to what
    /// PATTERN matches, within about TOKENS tokens (default 1024). Needs
    /// `ogrep index --graph`.
    // `--map=N` only: a bare value would swallow the pattern (`--map Foo .`).
    #[arg(
        long = "map",
        value_name = "TOKENS",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "1024"
    )]
    map: Option<usize>,

    /// List the definitions (functions, methods, ...) that use PATTERN, once
    /// each, with the number of uses.
    #[arg(long = "callers")]
    callers: bool,

    /// For each definition named PATTERN, list the names it uses that are
    /// defined in the repository, and where. Needs `ogrep index --graph`.
    #[arg(long = "callees")]
    callees: bool,

    /// List services, alerts and other entities found in the repository's
    /// operations files (Compose, Kubernetes, Backstage, Prometheus rules),
    /// with what each depends on. PATTERN filters entity names (`.` for all).
    #[arg(long = "entities")]
    entities: bool,

    /// With --entities: give each entity and dependency the first-parent
    /// commit since which the repository's files have held it (`since` in
    /// --json output). Reads the operations files of every such commit.
    #[arg(long = "dated", requires = "entities")]
    dated: bool,

    /// Show every service that depends on the service PATTERN, directly or not,
    /// by distance, with the file and line each dependency comes from.
    #[arg(long = "blast")]
    blast: bool,

    /// Rank likely causes for failing services (PATTERN: comma-separated
    /// names): services they depend on, weighted by how many failing services
    /// each explains and by commits since --since (default 24 hours) and, with
    /// --until, no later than it.
    #[arg(long = "suspects")]
    suspects: bool,

    /// Commits to the services PATTERN (comma-separated names) and to what
    /// they directly depend on, since --since (default 7 days) and, with
    /// --until, no later than it, newest first.
    #[arg(long = "timeline")]
    timeline: bool,

    /// Read a stack trace from the file PATTERN (`-` for standard input) and
    /// show each frame found in the repository, with its enclosing definition.
    #[arg(long = "stack-trace")]
    stack_trace: bool,

    /// Group the log lines PATTERN matches into templates (variable parts as
    /// `<*>`), most frequent first, with first and last timestamps.
    #[arg(long = "log-templates")]
    log_templates: bool,

    /// With --log-templates: only templates first seen since WHEN.
    #[arg(long = "new-since", value_name = "WHEN")]
    new_since: Option<String>,

    /// With --log-templates: show the code that writes each message.
    #[arg(long = "template-to-code")]
    template_to_code: bool,

    /// Rank incident write-ups (files under postmortem, incident, rca, retro or
    /// outage paths) by similarity to PATTERN: text, or a file with the
    /// description (`-` for standard input).
    #[arg(long = "similar-incidents")]
    similar_incidents: bool,

    /// List the tests most related to the file PATTERN: named after it, using
    /// its definitions (with `ogrep index --graph`), or changed with it.
    #[arg(long = "related-tests")]
    related_tests: bool,

    /// Find code similar to the definition at PATTERN (`path:line`), ranked by
    /// token-shingle overlap: copies, near-duplicates and variants.
    #[arg(long = "similar")]
    similar: bool,

    /// Files changed both here (committed since the fork, or uncommitted) and
    /// on other local branches, and whether the changed lines overlap.
    #[arg(long = "conflicts")]
    conflicts: bool,

    /// Code names quoted in documentation (`like_this`) that no longer appear
    /// in any code file. PATTERN filters names (`.` for all).
    #[arg(long = "doc-drift")]
    doc_drift: bool,

    /// Search the lines commits added or removed instead of the current files,
    /// newest commit first (needs `ogrep index --history`). Like
    /// `git log -G PATTERN`, but answered from an index.
    #[arg(long = "history")]
    history: bool,

    /// Report when PATTERN first appeared in a commit, when it was last
    /// removed, and whether it is present now (needs `ogrep index --history`).
    #[arg(long = "introduced")]
    introduced: bool,

    /// With --history/--introduced/--suspects/--timeline: only commits since
    /// WHEN (a date, `2 weeks ago`, or 2h, 3d, 2w, 6mo, 1y). For --suspects
    /// and --timeline a revision instead keeps commits not reachable from it.
    #[arg(long = "since", value_name = "WHEN")]
    since: Option<String>,

    /// With --history/--introduced/--suspects/--timeline: only commits until
    /// WHEN. For --suspects and --timeline the default --since window then
    /// counts back from WHEN, and a revision instead keeps commits reachable
    /// from it.
    #[arg(long = "until", value_name = "WHEN")]
    until: Option<String>,

    /// With --history/--introduced: only commits whose author (`Name <email>`)
    /// matches this regex, case-insensitively.
    #[arg(long = "author", value_name = "PATTERN")]
    author: Option<String>,

    /// Search only files changed since REV or WHEN (a date, `2 weeks ago`, or
    /// 2h, 3d, 2w, 6mo, 1y), including uncommitted and untracked files.
    #[arg(long = "changed-since", value_name = "REV|WHEN")]
    changed_since: Option<String>,

    /// Search the files as they were at git revision REV (a commit, tag or
    /// branch) instead of the working tree. Reads git's object store; needs no
    /// index.
    #[arg(long = "at", value_name = "REV")]
    at: Option<String>,

    /// Skip files that contain any line matching PATTERN (repeatable).
    #[arg(long = "without", value_name = "PATTERN", global = true)]
    without: Vec<String>,

    /// Enable multiline matching (patterns may span line boundaries).
    #[arg(short = 'U', long = "multiline", global = true)]
    multiline: bool,

    /// Allow `.` to match a newline. Implies --multiline.
    #[arg(long = "multiline-dotall", global = true)]
    multiline_dotall: bool,

    // ── Output mode ──────────────────────────────────
    /// Print only filenames with matches.
    #[arg(short = 'l', long = "files-with-matches", global = true)]
    files_only: bool,

    /// Print files that do NOT match the pattern.
    #[arg(long = "files-without-match", global = true)]
    files_without_match: bool,

    /// Print match count per file.
    #[arg(short = 'c', long = "count", global = true)]
    count: bool,

    /// Print only the matched parts of a line.
    #[arg(short = 'o', long = "only-matching", global = true)]
    only_matching: bool,

    /// Limit matches per file.
    #[arg(short = 'm', long = "max-count", global = true)]
    max_count: Option<usize>,

    /// Stop after NUM results in total: matching lines, or files with -l and
    /// -c. The output is exactly the first NUM results of the full output, and
    /// the search stops reading files once it has them.
    #[arg(long = "max-results", value_name = "NUM", global = true,
          value_parser = clap::value_parser!(u64).range(1..))]
    max_results: Option<u64>,

    /// List files that would be searched (no search performed).
    #[arg(long = "files", global = true)]
    list_files: bool,

    /// Suppress all output; exit code only (0 = match found, 1 = no match).
    #[arg(short = 'q', long = "quiet", global = true)]
    quiet: bool,

    // ── Filtering ────────────────────────────────────
    /// Filter files by glob pattern (can be specified multiple times).
    ///
    /// With an index, globs filter the indexed corpus. Use --no-index to let
    /// positive globs reinclude ignored files.
    #[arg(short = 'g', long = "glob", global = true, action = clap::ArgAction::Append)]
    glob: Vec<String>,

    /// Like --glob, but case-insensitive.
    #[arg(long = "iglob", global = true, action = clap::ArgAction::Append)]
    iglob: Vec<String>,

    /// Treat all --glob patterns as case-insensitive.
    #[arg(long = "glob-case-insensitive", global = true)]
    glob_case_insensitive: bool,

    /// Filter files by type (e.g., rust, py, js). Repeatable. Use --type-list to see all.
    #[arg(short = 't', long = "type", global = true, action = clap::ArgAction::Append)]
    file_type: Vec<String>,

    /// Exclude files matching a type. Repeatable; takes precedence over --type.
    #[arg(short = 'T', long = "type-not", global = true, action = clap::ArgAction::Append)]
    type_not: Vec<String>,

    /// Add or extend a type: `name:glob` or `name:include:type1,type2`.
    #[arg(long = "type-add", global = true, value_name = "SPEC", action = clap::ArgAction::Append)]
    type_add: Vec<String>,

    /// Remove all globs for a type before applying --type-add.
    #[arg(long = "type-clear", global = true, value_name = "NAME", action = clap::ArgAction::Append)]
    type_clear: Vec<String>,

    /// Print all supported file types.
    #[arg(long = "type-list", global = true)]
    type_list: bool,

    /// Ignore files larger than NUM bytes (suffixes K, M, G allowed).
    /// Defaults to 64M; use --no-max-filesize for no limit.
    #[arg(
        long = "max-filesize",
        global = true,
        value_name = "NUM",
        overrides_with = "no_max_filesize"
    )]
    max_filesize: Option<String>,

    /// Apply no file size limit, as ripgrep does.
    #[arg(
        long = "no-max-filesize",
        global = true,
        overrides_with = "max_filesize"
    )]
    no_max_filesize: bool,

    /// Text encoding to use: `auto` (BOM sniffing), `none`, or a label like `utf-16le`.
    #[arg(
        short = 'E',
        long = "encoding",
        global = true,
        value_name = "ENCODING",
        overrides_with = "no_encoding"
    )]
    encoding: Option<String>,

    /// Reset --encoding back to `auto`.
    #[arg(long = "no-encoding", global = true, overrides_with = "encoding")]
    no_encoding: bool,

    /// Search binary files as if they were text.
    #[arg(short = 'a', long = "text", global = true)]
    text: bool,

    // ── Context ──────────────────────────────────────
    /// Lines of context after each match.
    #[arg(short = 'A', long = "after-context", global = true)]
    after_context: Option<usize>,

    /// Lines of context before each match.
    #[arg(short = 'B', long = "before-context", global = true)]
    before_context: Option<usize>,

    /// Lines of context before and after each match.
    #[arg(short = 'C', long = "context", global = true)]
    context: Option<usize>,

    /// Print the file name for each match (default behavior, ripgrep compatibility).
    #[arg(short = 'H', long = "with-filename", global = true)]
    with_filename: bool,

    /// Suppress filenames in output.
    #[arg(short = 'I', long = "no-filename", global = true)]
    no_filename: bool,

    /// Show line numbers (default behavior, ripgrep compatibility).
    #[arg(short = 'n', long = "line-number", global = true)]
    line_number: bool,

    /// Suppress line numbers in output.
    #[arg(short = 'N', long = "no-line-number", global = true)]
    no_line_number: bool,

    // ── Output formatting ────────────────────────────
    /// Group matches by file with heading.
    #[arg(long = "heading", global = true)]
    heading: bool,

    /// Don't group matches; flat output.
    #[arg(long = "no-heading", global = true)]
    no_heading: bool,

    /// JSON output (one object per line).
    #[arg(long = "json", global = true)]
    json: bool,

    /// Output in vim-compatible format (file:line:col:content).
    #[arg(long = "vimgrep", global = true)]
    vimgrep: bool,

    /// Color mode: auto, always, or never.
    #[arg(long = "color", default_value = "auto", global = true)]
    color: String,

    /// Use NUL byte as filename separator (for xargs -0).
    #[arg(short = '0', long = "null", global = true)]
    null: bool,

    /// Trim leading/trailing whitespace from each line.
    #[arg(long = "trim", global = true)]
    trim: bool,

    // ── Index control ────────────────────────────────
    /// Print query plan and timing stats.
    #[arg(long = "stats", visible_alias = "explain", global = true)]
    stats: bool,

    /// Skip the index, grep all files directly.
    #[arg(long = "no-index", global = true)]
    no_index: bool,

    /// Custom index directory.
    #[arg(long = "index-path", global = true)]
    index_path: Option<PathBuf>,

    // ── File discovery ───────────────────────────────
    /// Include hidden files and directories.
    #[arg(short = '.', long = "hidden", global = true)]
    hidden: bool,

    /// Don't respect .gitignore or p4ignore.ini files.
    #[arg(long = "no-ignore", global = true)]
    no_ignore: bool,

    /// Follow symbolic links while searching.
    #[arg(short = 'L', long = "follow", global = true)]
    follow: bool,

    /// Suppress error messages about nonexistent or unreadable files.
    #[arg(long = "no-messages", global = true)]
    no_messages: bool,

    /// Unrestricted search. -u = no-ignore, -uu = +hidden, -uuu = +binary.
    #[arg(short = 'u', long = "unrestricted", action = clap::ArgAction::Count, global = true)]
    unrestricted: u8,

    /// Search binary files, reporting a note instead of printing their lines.
    #[arg(long = "binary", global = true)]
    binary: bool,

    // ── Matching ─────────────────────────────────────
    /// Only match when the whole line matches the pattern. The later of -w and
    /// -x wins.
    #[arg(
        short = 'x',
        long = "line-regexp",
        global = true,
        group = "match_boundary",
        overrides_with = "word_regexp"
    )]
    line_regexp: bool,

    /// Use the PCRE-style engine, enabling lookaround and backreferences.
    #[arg(short = 'P', long = "pcre2", global = true)]
    pcre2: bool,

    /// Regex engine to use: default, pcre2, or auto.
    #[arg(
        long = "engine",
        value_name = "ENGINE",
        default_value = "auto",
        global = true
    )]
    engine: String,

    /// Print the PCRE-style engine version and exit.
    #[arg(long = "pcre2-version", global = true)]
    pcre2_version: bool,

    /// Disable Unicode-aware matching.
    #[arg(long = "no-unicode", global = true)]
    no_unicode: bool,

    /// Upper size limit for the compiled regex (suffixes K, M, G allowed).
    #[arg(long = "regex-size-limit", value_name = "NUM", global = true)]
    regex_size_limit: Option<String>,

    /// Upper size limit for the regex DFA cache (suffixes K, M, G allowed).
    #[arg(long = "dfa-size-limit", value_name = "NUM", global = true)]
    dfa_size_limit: Option<String>,

    /// Replace each match with TEXT. Capture groups are available as $1, ${name}.
    #[arg(short = 'r', long = "replace", value_name = "TEXT", global = true)]
    replace: Option<String>,

    /// Print both matching and non-matching lines.
    #[arg(long = "passthru", global = true)]
    passthru: bool,

    /// Stop searching a file after a line that does not match.
    #[arg(long = "stop-on-nonmatch", global = true)]
    stop_on_nonmatch: bool,

    // ── Output detail ────────────────────────────────
    /// Show the column number of the first match on each line.
    #[arg(long = "column", global = true, overrides_with = "no_column")]
    column: bool,

    /// Don't show column numbers.
    #[arg(long = "no-column", global = true, overrides_with = "column")]
    no_column: bool,

    /// Print the 0-based byte offset of each output line.
    #[arg(short = 'b', long = "byte-offset", global = true)]
    byte_offset: bool,

    /// Don't print lines longer than NUM bytes.
    #[arg(short = 'M', long = "max-columns", value_name = "NUM", global = true)]
    max_columns: Option<usize>,

    /// Print a truncated preview instead of suppressing a long line entirely.
    #[arg(long = "max-columns-preview", global = true)]
    max_columns_preview: bool,

    /// Count individual matches instead of matching lines.
    #[arg(long = "count-matches", global = true)]
    count_matches: bool,

    /// Print a count of zero for files with no match.
    #[arg(long = "include-zero", global = true)]
    include_zero: bool,

    /// Alias for --color always --heading --line-number.
    #[arg(short = 'p', long = "pretty", global = true)]
    pretty: bool,

    /// String printed between non-contiguous context blocks.
    #[arg(long = "context-separator", value_name = "SEP", global = true)]
    context_separator: Option<String>,

    /// Never print a context separator.
    #[arg(long = "no-context-separator", global = true)]
    no_context_separator: bool,

    /// Separator between the path/line/column fields of a matching line.
    #[arg(long = "field-match-separator", value_name = "SEP", global = true)]
    field_match_separator: Option<String>,

    /// Separator between the path/line/column fields of a context line.
    #[arg(long = "field-context-separator", value_name = "SEP", global = true)]
    field_context_separator: Option<String>,

    /// Character to use as the path separator in output.
    #[arg(long = "path-separator", value_name = "SEP", global = true)]
    path_separator: Option<String>,

    /// Sort results. Choices: none, path, modified, accessed, created.
    #[arg(long = "sort", value_name = "SORTBY", global = true)]
    sort: Option<String>,

    /// Sort results in descending order. Same choices as --sort.
    #[arg(long = "sortr", value_name = "SORTBY", global = true)]
    sortr: Option<String>,

    /// Deprecated alias for --sort path.
    #[arg(long = "sort-files", global = true)]
    sort_files: bool,

    // ── File discovery (advanced) ────────────────────
    /// Descend at most NUM directories below each search path.
    #[arg(long = "max-depth", value_name = "NUM", global = true)]
    max_depth: Option<usize>,

    /// Don't cross file system boundaries.
    #[arg(long = "one-file-system", global = true)]
    one_file_system: bool,

    /// Read extra ignore globs from PATH. Repeatable; later files take precedence.
    #[arg(long = "ignore-file", value_name = "PATH", global = true, action = clap::ArgAction::Append)]
    ignore_file: Vec<String>,

    /// Match --ignore-file globs case-insensitively.
    #[arg(long = "ignore-file-case-insensitive", global = true)]
    ignore_file_case_insensitive: bool,

    /// Don't respect .ignore files.
    #[arg(long = "no-ignore-dot", global = true)]
    no_ignore_dot: bool,

    /// Don't respect .git/info/exclude.
    #[arg(long = "no-ignore-exclude", global = true)]
    no_ignore_exclude: bool,

    /// Don't respect --ignore-file arguments.
    #[arg(long = "no-ignore-files", global = true)]
    no_ignore_files: bool,

    /// Don't respect the global gitignore.
    #[arg(long = "no-ignore-global", global = true)]
    no_ignore_global: bool,

    /// Suppress messages about unparseable ignore files.
    #[arg(long = "no-ignore-messages", global = true)]
    no_ignore_messages: bool,

    /// Don't respect ignore files in parent directories.
    #[arg(long = "no-ignore-parent", global = true)]
    no_ignore_parent: bool,

    /// Don't respect .gitignore files.
    #[arg(long = "no-ignore-vcs", global = true)]
    no_ignore_vcs: bool,

    /// Respect .gitignore files even outside a git repository.
    #[arg(long = "no-require-git", global = true)]
    no_require_git: bool,

    // ── Accepted for ripgrep compatibility ───────────
    /// Number of threads to use. ogrep sizes its pool automatically.
    #[arg(short = 'j', long = "threads", value_name = "NUM", global = true)]
    threads: Option<usize>,

    /// Accepted for compatibility; ogrep always reads files directly.
    #[arg(long = "mmap", global = true, overrides_with = "no_mmap")]
    mmap: bool,

    /// Accepted for compatibility; ogrep always reads files directly.
    #[arg(long = "no-mmap", global = true, overrides_with = "mmap")]
    no_mmap: bool,

    /// Flush output on every line.
    #[arg(
        long = "line-buffered",
        global = true,
        overrides_with = "block_buffered"
    )]
    line_buffered: bool,

    /// Buffer output in blocks (the default when not writing to a terminal).
    #[arg(
        long = "block-buffered",
        global = true,
        overrides_with = "line_buffered"
    )]
    block_buffered: bool,

    /// Accepted for compatibility; ogrep reads no configuration file.
    #[arg(long = "no-config", global = true)]
    no_config: bool,

    /// Accepted for compatibility; ogrep's colors are not configurable yet.
    #[arg(long = "colors", value_name = "SPEC", global = true, action = clap::ArgAction::Append)]
    colors: Vec<String>,

    /// Print debug messages to stderr (implies `--stats`).
    #[arg(long = "debug", global = true)]
    debug: bool,

    /// Print verbose trace messages to stderr (implies `--stats`).
    #[arg(long = "trace", global = true)]
    trace: bool,

    /// Not supported: CRLF-aware anchor and line-terminator matching.
    #[arg(long = "crlf", global = true, overrides_with = "no_crlf")]
    crlf: bool,

    /// Use default LF matching, preserving CR bytes in source lines.
    #[arg(long = "no-crlf", global = true, overrides_with = "crlf")]
    no_crlf: bool,

    /// Not supported: ogrep does not decompress archives before searching.
    #[arg(short = 'z', long = "search-zip", global = true)]
    search_zip: bool,
}

/// Every argument that needs parsing and can fail, resolved once up front.
struct ResolvedArgs {
    max_filesize: Option<u64>,
    /// Whether the size limit was asked for rather than inherited from
    /// [`ogrep_core::walker::DEFAULT_MAX_FILE_SIZE`]. A file named directly on
    /// the command line ignores the inherited limit — pointing at a file is an
    /// unambiguous request to search it — but honours one the user set.
    max_filesize_requested: bool,
    encoding: ogrep_core::encoding::EncodingMode,
    engine: matching::RegexEngine,
    regex_size_limit: Option<usize>,
    dfa_size_limit: Option<usize>,
    sort: Option<search::SortMode>,
}

/// Parse a `--sort`/`--sortr` value. `none` means "leave results unordered".
fn parse_sort(key: &str, reverse: bool) -> Result<Option<search::SortMode>> {
    if key == "none" {
        return Ok(None);
    }
    let key = search::SortKey::from_str_opt(key)
        .ok_or_else(|| anyhow::anyhow!("unrecognized sort criteria: {key}"))?;
    Ok(Some(search::SortMode { key, reverse }))
}

/// Parse a byte size, accepting K/M/G suffixes like ripgrep.
///
/// `flag` names the option in the error message so a bad `--regex-size-limit`
/// does not report a problem with `--max-filesize`.
fn parse_byte_size(flag: &str, s: &str) -> Result<u64> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last() {
        Some('K') | Some('k') => (&s[..s.len() - 1], 1024u64),
        Some('M') | Some('m') => (&s[..s.len() - 1], 1024 * 1024),
        Some('G') | Some('g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid {flag} value: {s}"))?;
    n.checked_mul(mult)
        .ok_or_else(|| anyhow::anyhow!("{flag} value is too large: {s}"))
}

/// Parse a size limit that the regex engines take as a `usize`.
///
/// ripgrep stores these limits as `usize` and rejects a value that does not
/// fit ("size is too big") instead of truncating it. Truncating would apply a
/// silently different limit than the one asked for on a 32-bit target, so
/// reject it here too.
fn parse_size_limit(flag: &str, s: &str) -> Result<usize> {
    let bytes = parse_byte_size(flag, s)?;
    usize::try_from(bytes)
        .map_err(|_| anyhow::anyhow!("{flag} value is too large for this platform: {s}"))
}

/// Posting accumulation strategy for `ogrep index`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
enum IndexStrategyArg {
    /// Accumulate all postings in memory and sort once.
    Memory,
    /// Bound peak memory with an external merge sort that spills to disk.
    External,
}

impl From<IndexStrategyArg> for builder::IndexStrategy {
    fn from(value: IndexStrategyArg) -> Self {
        match value {
            IndexStrategyArg::Memory => Self::InMemory,
            IndexStrategyArg::External => Self::External,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Build or rebuild the trigram index.
    Index {
        /// Root directory to index.
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Force a full rebuild.
        #[arg(long)]
        force: bool,

        /// Exclude directories from indexing (can be specified multiple times).
        #[arg(long = "exclude", action = clap::ArgAction::Append)]
        exclude: Vec<String>,

        /// How postings are accumulated during the build.
        ///
        /// `external` (default) bounds peak memory with an external merge
        /// sort, spilling sorted segments to disk when the arena fills. If the
        /// arena never fills it is identical to `memory`, so small repos are
        /// unaffected. `memory` holds every posting in RAM and sorts once;
        /// peak memory then grows with repo size, unbounded.
        #[arg(
            long = "index-strategy",
            value_name = "STRATEGY",
            default_value = "external"
        )]
        strategy: IndexStrategyArg,

        /// Arena size in megabytes before `--index-strategy=external` spills to
        /// disk. Lower values reduce peak memory at the cost of more spill
        /// segments to merge. Ignored by `--index-strategy=memory`.
        #[arg(long = "index-buffer", value_name = "MB", value_parser = clap::value_parser!(u64).range(1..))]
        index_buffer_mb: Option<u64>,

        /// Index trigrams only, without sparse n-grams. The index is about half
        /// the size and long literals are looked up with more, commoner keys.
        /// A server that later rebuilds the index adds the n-grams back.
        #[arg(long = "no-sparse-grams")]
        no_sparse_grams: bool,

        /// Also index the lines each commit added or removed, for `--history`
        /// and `--introduced` searches. An existing history index is brought up
        /// to date with HEAD; `--force` rebuilds it.
        #[arg(long = "history")]
        history: bool,

        /// With --history: index at most this many recent commits (default
        /// 20000; 0 for every commit).
        #[arg(long = "history-depth", value_name = "N", requires = "history")]
        history_depth: Option<usize>,

        /// With --history: index only commits since this date (any form
        /// `git log --since` takes, or 2h, 3d, 2w, 6mo, 1y).
        #[arg(long = "history-since", value_name = "WHEN", requires = "history")]
        history_since: Option<String>,

        /// Also build the file graph (which files use names others define) for
        /// `--near`, `--map` and `--callees`.
        #[arg(long = "graph")]
        graph: bool,
    },

    /// Start the persistent search server.
    Serve {
        /// Root directory to serve.
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Disable all automatic refresh, including watching and polling.
        #[arg(long, conflicts_with_all = ["watch_mode", "poll_interval", "watch_budget"])]
        no_watch: bool,

        /// Use native notifications with automatic polling fallback, or polling only.
        #[arg(long, value_enum, default_value = "auto", value_name = "MODE")]
        watch_mode: serve::WatchMode,

        /// Seconds to wait after a polling reconciliation completes (1-86400).
        #[arg(
            long,
            default_value_t = 120,
            value_name = "SECONDS",
            value_parser = clap::value_parser!(u64).range(1..=86400)
        )]
        poll_interval: u64,

        /// Process-local native watch ceiling, not available shared OS capacity.
        #[arg(
            long,
            default_value_t = 8192,
            value_name = "N",
            value_parser = clap::value_parser!(u32).range(1..)
        )]
        watch_budget: u32,

        /// Maximum memory budget in megabytes for the in-memory index built
        /// during the initial scan. When the indexer's working set exceeds
        /// this, it flushes to disk and continues, keeping peak memory bounded
        /// while still producing a complete index. Defaults to 50% of physical
        /// RAM (clamped between 512 MB and 16 GB).
        #[arg(long = "max-memory", value_name = "MB", value_parser = clap::value_parser!(u64).range(1..))]
        max_memory_mb: Option<u64>,

        /// Maximum CPU budget for the initial index build, as a percentage of
        /// logical cores (1-100). The parallel file-reading/trigram-extraction
        /// work is confined to this fraction of cores so the host stays
        /// responsive. Defaults to 50%.
        #[arg(long = "max-cpu", value_name = "PERCENT")]
        max_cpu_percent: Option<u8>,

        /// Exclude directories from indexing (can be specified multiple times).
        #[arg(long = "exclude", action = clap::ArgAction::Append)]
        exclude: Vec<String>,

        /// Number of accumulated index mutations that triggers a background
        /// save. Higher values reduce save frequency (and the pauses they
        /// cause during heavy churn) at the cost of more unsaved work if the
        /// process is killed. Defaults to 5000.
        #[arg(long = "auto-save-mutations", value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
        auto_save_mutations: Option<u32>,

        /// Maximum number of filesystem events buffered between the OS
        /// watcher and the indexing worker. Raise it if bulk changes (branch
        /// switches, builds) log watcher queue overflows; each overflow costs
        /// a full stale check instead of incremental updates. Defaults to
        /// 16384.
        // Bounded by `usize::MAX` so the value always survives the conversion
        // at the call site. Without it, a value above `usize::MAX` would
        // truncate on a 32-bit target — and truncating to 0 turns the
        // watcher's `sync_channel` into a rendezvous channel, where every
        // `try_send` fails and no event is ever delivered. Plain `//` so the
        // rationale stays out of `--help`.
        #[arg(
            long = "watcher-queue-cap",
            value_name = "N",
            value_parser = clap::value_parser!(u64).range(1..=usize::MAX as u64)
        )]
        watcher_queue_cap: Option<u64>,
        /// Maximum simultaneous search/file-list requests, including response writes.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u64).range(1..=32))]
        search_concurrency: u64,
    },

    /// Search for a pattern.
    Search {
        /// The regex pattern to search for.
        pattern: String,

        /// Root directories or files to search.
        #[arg(value_name = "PATH", num_args = 0..)]
        paths: Vec<String>,
    },

    /// Show index and server status.
    Status {
        /// Root directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Wait for a running server to publish completed edits as searchable.
    Refresh {
        /// Served root directory (not a subdirectory).
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Total client wait in seconds; timed-out server work may continue.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
        timeout_secs: u64,
    },

    /// Count text files in a directory (fast walker, no indexing).
    CountFiles {
        /// Root directory to scan.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

impl Cli {
    fn try_parse_args_from<I, T>(args: I) -> std::result::Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        // ripgrep takes the next argument as a flag's value whatever it
        // starts with (`-e -x`, `--without -debug`, `-r -`), so every flag that
        // requires a value does too. Flags whose value is optional (`--map`)
        // keep clap's rule, or they would swallow the flag after them.
        let mut command = Self::command().mut_args(|arg| {
            let requires_value = !arg.is_positional()
                && arg.get_action().takes_values()
                && arg.get_num_args().is_none_or(|n| n.min_values() > 0);
            if requires_value {
                arg.allow_hyphen_values(true)
            } else {
                arg
            }
        });
        let matches = command.try_get_matches_from_mut(args)?;
        // Clap conflicts are unconditional; this one depends on the mode's
        // value and must not treat the inherited budget as an explicit flag.
        if let Some(("serve", serve)) = matches.subcommand()
            && serve.get_one::<serve::WatchMode>("watch_mode") == Some(&serve::WatchMode::Poll)
            && serve.value_source("watch_budget") == Some(clap::parser::ValueSource::CommandLine)
        {
            return Err(command
                .find_subcommand_mut("serve")
                .expect("serve subcommand exists")
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "--watch-budget cannot be used with --watch-mode poll",
                ));
        }
        let mut cli = Self::from_arg_matches(&matches)?;
        // Global flags reach a subcommand without their mutual overrides
        // applied across the two levels. The group keeps the choice made at
        // each level, so the deepest one that made a choice wins, as the
        // later flag does within one level.
        let mut scope = &matches;
        while let Some((_, subcommand)) = scope.subcommand() {
            if let Some(boundary) = subcommand
                .get_many::<clap::Id>("match_boundary")
                .and_then(|mut choices| choices.next_back())
            {
                cli.word_regexp = boundary == "word_regexp";
                cli.line_regexp = boundary == "line_regexp";
            }
            scope = subcommand;
        }
        Ok(cli)
    }

    /// Resolve `--max-filesize` once, so a malformed value is reported instead
    /// of silently falling back to a different limit than the user asked for.
    ///
    /// Absent both flags this is [`walker::DEFAULT_MAX_FILE_SIZE`], not `None`.
    /// Resolving the default *here* rather than at each use is what keeps the
    /// index and the search agreeing: a walk that capped at 64 MiB while the
    /// search did not would look exactly like "this file contains no match".
    fn max_filesize_bytes(&self) -> Result<Option<u64>> {
        if self.no_max_filesize {
            return Ok(None);
        }
        match self.max_filesize.as_deref() {
            Some(s) => Ok(Some(parse_byte_size("--max-filesize", s)?)),
            None => Ok(ogrep_core::walker::DEFAULT_MAX_FILE_SIZE),
        }
    }

    /// Resolve `-E/--encoding`. `--no-encoding` means `auto`, and clap's
    /// mutual `overrides_with` makes whichever flag came last win, as ripgrep
    /// does.
    fn encoding_mode(&self) -> Result<ogrep_core::encoding::EncodingMode> {
        match self.encoding.as_deref() {
            Some(label) => ogrep_core::encoding::parse_encoding(label),
            None => Ok(ogrep_core::encoding::EncodingMode::Auto),
        }
    }

    /// Parse every argument that can fail, so a bad value exits with a usage
    /// error instead of silently searching with different settings.
    fn resolve(&self) -> Result<ResolvedArgs> {
        let engine = if self.pcre2 {
            matching::RegexEngine::Pcre2
        } else {
            matching::RegexEngine::from_str_opt(&self.engine)
                .ok_or_else(|| anyhow::anyhow!("unrecognized regex engine: {}", self.engine))?
        };

        let sort = match (&self.sort, &self.sortr, self.sort_files) {
            (Some(key), _, _) => parse_sort(key, false)?,
            (None, Some(key), _) => parse_sort(key, true)?,
            (None, None, true) => Some(search::SortMode {
                key: search::SortKey::Path,
                reverse: false,
            }),
            _ => None,
        };

        Ok(ResolvedArgs {
            max_filesize: self.max_filesize_bytes()?,
            max_filesize_requested: self.max_filesize.is_some() || self.no_max_filesize,
            encoding: self.encoding_mode()?,
            engine,
            regex_size_limit: self
                .regex_size_limit
                .as_deref()
                .map(|s| parse_size_limit("--regex-size-limit", s))
                .transpose()?,
            dfa_size_limit: self
                .dfa_size_limit
                .as_deref()
                .map(|s| parse_size_limit("--dfa-size-limit", s))
                .transpose()?,
            sort,
        })
    }

    /// The tool flags given: modes that answer their own question instead
    /// of printing matching lines. More than one is refused.
    fn tools(&self) -> Vec<Tool> {
        use Takes::*;
        let tool = |set: bool, flag, takes, budget| {
            set.then_some(Tool {
                flag,
                takes,
                budget,
            })
        };
        [
            tool(self.history, "--history", Pattern, None),
            tool(self.introduced, "--introduced", Pattern, None),
            tool(self.near, "--near", Pattern, Some(20)),
            tool(self.map.is_some(), "--map", Pattern, None),
            tool(self.callees, "--callees", Subject, None),
            tool(self.related_tests, "--related-tests", Subject, Some(15)),
            tool(self.similar, "--similar", Subject, Some(15)),
            tool(self.conflicts, "--conflicts", Nothing, None),
            tool(self.doc_drift, "--doc-drift", Pattern, None),
            tool(self.entities, "--entities", Pattern, None),
            tool(self.blast, "--blast", Subject, None),
            tool(self.suspects, "--suspects", Subject, None),
            tool(self.timeline, "--timeline", Subject, None),
            tool(self.stack_trace, "--stack-trace", Subject, None),
            tool(self.log_templates, "--log-templates", Pattern, None),
            tool(
                self.similar_incidents,
                "--similar-incidents",
                Subject,
                Some(10),
            ),
            tool(self.rank_fit.is_some(), "--rank-fit", Nothing, None),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// The result count of a tool that takes `--budget` as one.
    fn tool_budget(&self) -> usize {
        let default = self.tools().first().and_then(|t| t.budget).unwrap_or(20);
        self.budget.map_or(default, |b| b as usize)
    }

    /// Whether context lines would be printed (as `SearchOptions::has_context`
    /// decides).
    fn has_context(&self) -> bool {
        search::context_lines(self.before_context, self.context) > 0
            || search::context_lines(self.after_context, self.context) > 0
    }

    /// Whether a flag (`-e`, `-f`, `--ident`) supplies the pattern, so every
    /// positional argument is a path.
    fn pattern_from_flags(&self) -> bool {
        !self.regexp.is_empty()
            || !self.pattern_file.is_empty()
            || !self.ident.is_empty()
            || self.about.is_some()
    }

    /// Split the positional arguments into a pattern and a list of paths.
    ///
    /// ripgrep only treats the first positional as the pattern when no pattern
    /// was supplied another way. Once `-e`, `-f` or `--ident` is present every
    /// positional is a path, so `rg -e foo src` searches `src` rather than also
    /// searching for the literal string `src`.
    fn split_pattern_and_paths(
        &self,
        positional: Option<&String>,
        rest: &[String],
    ) -> (String, Vec<String>) {
        if !self.pattern_from_flags() {
            return (positional.cloned().unwrap_or_default(), rest.to_vec());
        }
        let mut paths = Vec::with_capacity(rest.len() + 1);
        paths.extend(positional.cloned());
        paths.extend(rest.iter().cloned());
        (String::new(), paths)
    }

    /// Everything that reshapes a search's output after it runs.
    fn post_options(&self) -> post::PostOptions {
        post::PostOptions {
            rank: self.rank_options(),
            scope: self.scope,
            kind: match self.kind.as_deref() {
                Some("def") => Some(post::KindFilter::Def),
                Some("ref") => Some(post::KindFilter::Ref),
                _ => None,
            },
            zone: match self.in_zone.as_deref() {
                Some("code") => Some(post::ZoneFilter::Code),
                Some("comments") => Some(post::ZoneFilter::Comments),
                Some("strings") => Some(post::ZoneFilter::Strings),
                _ => None,
            },
            callers: self.callers,
        }
        .with_callers()
    }

    /// The `--rank` settings, when ranking was asked for (directly or through a
    /// flag that implies it).
    fn rank_options(&self) -> Option<rank::RankOptions> {
        // --budget is the result count of the tools that take one; only
        // elsewhere does it imply --rank.
        let budget_is_rank = !self.tools().iter().any(|t| t.budget.is_some());
        let wanted = self.rank
            || self.about.is_some()
            || self.cards.is_some()
            || (self.budget.is_some() && budget_is_rank)
            || self.per_file.is_some()
            || self.rank_cutoff.is_some()
            || self.explain_rank;
        if !wanted {
            return None;
        }
        let defaults = rank::RankOptions::default();
        Some(rank::RankOptions {
            budget: self.budget.map_or(defaults.budget, |b| b as usize),
            per_file: self.per_file.map_or(defaults.per_file, |k| k as usize),
            cutoff: match self.rank_cutoff.as_deref() {
                Some("auto") => rank::Cutoff::Auto,
                _ => rank::Cutoff::None,
            },
            explain: self.explain_rank,
            weights: rank::Weights::default(),
            about: self.about.is_some(),
            corpus_files: 0,
            cards: self.cards.map(|t| t as usize),
        })
    }

    fn build_search_opts(&self, pattern: String, resolved: &ResolvedArgs) -> search::SearchOptions {
        // `-p/--pretty` is ripgrep's alias for --color always --heading -n.
        let heading = if self.heading || self.pretty {
            Some(true)
        } else if self.no_heading {
            Some(false)
        } else {
            None
        };
        let color = if self.pretty {
            ColorMode::Always
        } else {
            ColorMode::from_str_opt(&self.color).unwrap_or(ColorMode::Auto)
        };
        let no_ignore = self.no_ignore || self.unrestricted >= 1;
        let hidden = self.hidden || self.unrestricted >= 2;
        // ripgrep's third `-u` is `--binary`, not `-a/--text`: binary files
        // become visible and are summarised with a note, rather than having
        // their lines dumped as text.
        let binary = self.binary || self.unrestricted >= 3;
        let text = self.text;

        // ripgrep implements `-c`, `--count-matches`, `-l`, `--files-without-match`
        // and `--files` in a printer that predates `--json` and has no JSON form,
        // so those flags win outright and the whole invocation produces no JSON —
        // not even a `summary`. Dropping the flag here rather than only in
        // `make_output_config` keeps every other `json`-conditional decision
        // (binary handling, match detail) consistent with the printer in use.
        let json = self.json
            && !(self.count
                || self.count_matches
                || self.files_only
                || self.files_without_match
                || self.list_files);

        search::SearchOptions {
            pattern,
            extra_patterns: self
                .regexp
                .iter()
                .cloned()
                .chain(
                    self.ident
                        .iter()
                        .map(|name| search::identifier_pattern(name)),
                )
                .chain(self.about.iter().flat_map(|text| rank::about_terms(text)))
                .collect(),
            file_patterns: None,
            with_patterns: self.with.clone(),
            without_patterns: self.without.clone(),
            // --about's terms are words, lower-cased, matched in any case.
            case_insensitive: self.ignore_case || self.about.is_some(),
            case_sensitive: self.case_sensitive && self.about.is_none(),
            smart_case: self.smart_case && self.about.is_none(),
            fixed_string: self.fixed_strings || self.about.is_some(),
            files_only: self.files_only,
            files_without_match: self.files_without_match,
            count: self.count,
            word_boundary: self.word_regexp,
            max_count: self.max_count,
            max_results: self.max_results,
            json,
            vimgrep: self.vimgrep,
            stats: self.stats || self.debug || self.trace,
            no_index: self.no_index,
            glob: self.glob.clone(),
            iglob: self.iglob.clone(),
            glob_case_insensitive: self.glob_case_insensitive,
            all_spans: false,
            reshaping: false,
            offsets: false,
            only_paths: None,
            types: self.file_type.clone(),
            types_not: self.type_not.clone(),
            type_add: self.type_add.clone(),
            type_clear: self.type_clear.clone(),
            invert_match: self.invert_match,
            only_matching: self.only_matching,
            after_context: self.after_context,
            before_context: self.before_context,
            context: self.context,
            heading,
            color,
            null: self.null,
            trim: self.trim,
            // --multiline-dotall implies --multiline, as in ripgrep.
            multiline: self.multiline || self.multiline_dotall,
            multiline_dotall: self.multiline_dotall,
            no_ignore,
            hidden,
            quiet: self.quiet,
            no_filename: self.no_filename,
            // ripgrep only turns line numbers on for a terminal, so a piped
            // stream stays `path:content` for whatever parses it. `--column`,
            // `--vimgrep` and `-p` all ask for the line number explicitly.
            no_line_number: self.no_line_number
                || !(self.line_number
                    || self.column
                    || self.vimgrep
                    || self.pretty
                    || crate::output::atty_check()),
            text,
            binary,
            // Replaced per path argument in `run_search`.
            path_display: crate::output::PathDisplay::Bare,
            max_filesize: resolved.max_filesize,
            max_filesize_requested: resolved.max_filesize_requested,
            encoding: resolved.encoding,
            follow: self.follow,
            no_messages: self.no_messages,
            no_ignore_messages: self.no_ignore_messages,
            line_regexp: self.line_regexp,
            no_unicode: self.no_unicode,
            engine: resolved.engine,
            regex_size_limit: resolved.regex_size_limit,
            dfa_size_limit: resolved.dfa_size_limit,
            replace: self.replace.clone(),
            passthru: self.passthru,
            stop_on_nonmatch: self.stop_on_nonmatch,
            column: self.column || self.vimgrep,
            byte_offset: self.byte_offset,
            max_columns: self.max_columns,
            max_columns_preview: self.max_columns_preview,
            count_matches: self.count_matches,
            include_zero: self.include_zero,
            context_separator: if self.no_context_separator {
                None
            } else {
                Some(
                    self.context_separator
                        .clone()
                        .unwrap_or_else(|| "--".to_string()),
                )
            },
            field_match_separator: self
                .field_match_separator
                .clone()
                .unwrap_or_else(|| ":".to_string()),
            field_context_separator: self
                .field_context_separator
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            path_separator: self.path_separator.clone(),
            sort: resolved.sort,
            at_rev: self.at.clone(),
            max_depth: self.max_depth,
            one_file_system: self.one_file_system,
            ignore_files: if self.no_ignore_files {
                Vec::new()
            } else {
                self.ignore_file.clone()
            },
            ignore_file_case_insensitive: self.ignore_file_case_insensitive,
            no_ignore_dot: self.no_ignore_dot,
            no_ignore_exclude: self.no_ignore_exclude,
            no_ignore_global: self.no_ignore_global,
            no_ignore_parent: self.no_ignore_parent,
            no_ignore_vcs: self.no_ignore_vcs,
            no_require_git: self.no_require_git,
            threads: self.threads,
            line_buffered: self.line_buffered,
        }
    }

    /// File-discovery flags that are declared globally (so every subcommand
    /// accepts them) but that `index` and `serve` do not plumb into their
    /// walks.
    ///
    /// These change *which files exist* as far as the tool is concerned, so
    /// accepting one and ignoring it produces an index that silently disagrees
    /// with what was asked for — and, for `serve`, one that then disagrees with
    /// `ogrep search` over the same tree. That is indistinguishable from a
    /// corpus with no matches, so the only safe answer is to refuse.
    ///
    /// Both builders include hidden files by default, so `--hidden` is accepted
    /// as a redundant flag. `--threads` is excluded deliberately: it is documented as
    /// accepted-for-compatibility and ignored everywhere, not just here.
    fn unsupported_discovery_flags(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.follow {
            out.push("--follow");
        }
        if self.max_depth.is_some() {
            out.push("--max-depth");
        }
        if self.one_file_system {
            out.push("--one-file-system");
        }
        if !self.ignore_file.is_empty() {
            out.push("--ignore-file");
        }
        if self.ignore_file_case_insensitive {
            out.push("--ignore-file-case-insensitive");
        }
        if self.no_ignore_dot {
            out.push("--no-ignore-dot");
        }
        if self.no_ignore_exclude {
            out.push("--no-ignore-exclude");
        }
        if self.no_ignore_files {
            out.push("--no-ignore-files");
        }
        if self.no_ignore_global {
            out.push("--no-ignore-global");
        }
        if self.no_ignore_parent {
            out.push("--no-ignore-parent");
        }
        if self.no_ignore_vcs {
            out.push("--no-ignore-vcs");
        }
        out
    }
}

/// Exit with a clear error rather than building an index under settings the
/// caller did not ask for. See [`Cli::unsupported_discovery_flags`].
fn reject_unsupported_discovery_flags(cli: &Cli, subcommand: &str) {
    let unsupported = cli.unsupported_discovery_flags();
    if unsupported.is_empty() {
        return;
    }
    eprintln!(
        "ogrep: `{subcommand}` does not support {}; \
         it would be ignored and the index would not match what you asked for",
        unsupported.join(", ")
    );
    process::exit(2);
}

fn main() {
    // clap's derive builds every argument in a single generated stack frame,
    // and ogrep declares enough flags that an unoptimised build overflows the
    // default main-thread stack before parsing finishes. Run the real work on a
    // thread with room to spare so debug builds behave like release ones.
    let worker = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(run_cli)
        .expect("failed to spawn worker thread");
    if worker.join().is_err() {
        process::exit(2);
    }
}

fn run_cli() {
    let cli = Cli::try_parse_args_from(std::env::args_os()).unwrap_or_else(|error| error.exit());
    let no_ignore = cli.no_ignore || cli.unrestricted >= 1;

    // Handle --type-list. It reflects --type-add/--type-clear so users can
    // verify a custom definition without running a search.
    if cli.type_list {
        let mut defs = ogrep_core::filetypes::TypeDefs::builtin();
        for name in &cli.type_clear {
            defs.clear(name);
        }
        for spec in &cli.type_add {
            if let Err(e) = defs.add(spec) {
                eprintln!("ogrep: {e}");
                process::exit(2);
            }
        }
        defs.print_list();
        process::exit(0);
    }

    // `-z/--search-zip` would need transparent decompression. Failing loudly is
    // the only safe answer: silently ignoring it makes an archive full of hits
    // look like a repo with none.
    if cli.crlf {
        eprintln!("ogrep: --crlf matching is not supported; use default LF mode or ripgrep --crlf");
        process::exit(2);
    }

    // `--ident` generates a regex; escaping it as a literal would search for
    // the generated pattern text itself.
    if !cli.ident.is_empty() && cli.fixed_strings {
        eprintln!("ogrep: --ident cannot be combined with -F/--fixed-strings");
        process::exit(2);
    }

    // --about's text is the whole query: other pattern sources would be scored
    // as words of it, and -w/-x would change what a word matches.
    if let Some(text) = &cli.about {
        let conflicts = [
            (!cli.regexp.is_empty(), "-e/--regexp"),
            (!cli.pattern_file.is_empty(), "-f/--file"),
            (!cli.ident.is_empty(), "--ident"),
            (cli.word_regexp, "-w/--word-regexp"),
            (cli.line_regexp, "-x/--line-regexp"),
        ];
        if let Some((_, flag)) = conflicts.iter().find(|(set, _)| *set) {
            eprintln!("ogrep: --about cannot be combined with {flag}");
            process::exit(2);
        }
        if rank::about_terms(text).is_empty() {
            eprintln!("ogrep: --about: no words to search for in {text:?}");
            process::exit(2);
        }
    }

    // A card replaces a file's lines; these ask for a different shape.
    if cli.cards.is_some() {
        let conflicts = [
            (cli.files_only, "-l/--files-with-matches"),
            (
                cli.count || cli.count_matches,
                "-c/--count or --count-matches",
            ),
            (cli.only_matching, "-o/--only-matching"),
            (cli.vimgrep, "--vimgrep"),
            (cli.has_context(), "context lines (-A/-B/-C)"),
        ];
        if let Some((_, flag)) = conflicts.iter().find(|(set, _)| *set) {
            eprintln!("ogrep: --cards cannot be combined with {flag}");
            process::exit(2);
        }
    }

    // `--files` lists what would be searched without reading any content, so
    // it cannot apply content conditions; silently ignoring them would list
    // files the user excluded.
    if cli.list_files && (!cli.with.is_empty() || !cli.without.is_empty()) {
        eprintln!("ogrep: --with/--without cannot be combined with --files");
        process::exit(2);
    }

    if cli.search_zip {
        eprintln!("ogrep: -z/--search-zip is not supported; decompress the files first");
        process::exit(2);
    }

    // History searches report commits, not files.
    let history = cli.history || cli.introduced;
    if !history && cli.author.is_some() {
        eprintln!("ogrep: --author needs --history or --introduced");
        process::exit(2);
    }
    if !(history || cli.suspects || cli.timeline) && (cli.since.is_some() || cli.until.is_some()) {
        eprintln!(
            "ogrep: --since and --until need --history, --introduced, --suspects or --timeline"
        );
        process::exit(2);
    }
    if (cli.new_since.is_some() || cli.template_to_code) && !cli.log_templates {
        eprintln!("ogrep: --new-since and --template-to-code need --log-templates");
        process::exit(2);
    }
    if cli.at.is_some() && (history || cli.changed_since.is_some()) {
        eprintln!("ogrep: --at cannot be combined with --history, --introduced or --changed-since");
        process::exit(2);
    }
    // Each tool answers its own question; two at once would silently run
    // one of them.
    let tools = cli.tools();
    // Ranked output and the tools size their answers with --budget.
    if cli.max_results.is_some() {
        let other = tools.first().map(|t| t.flag).or_else(|| {
            cli.rank_options()
                .is_some()
                .then_some("--rank, --about or --cards (use --budget)")
        });
        if let Some(other) = other {
            eprintln!("ogrep: --max-results cannot be combined with {other}");
            process::exit(2);
        }
    }
    if let [first, second, ..] = &tools[..] {
        eprintln!(
            "ogrep: {} cannot be combined with {}",
            first.flag, second.flag
        );
        process::exit(2);
    }
    // History has its own checks below. The other tools print their own
    // results, so the search's reshaping and -q do not apply to them, and
    // --changed-since and --at, which narrow or move the files a search
    // reads, would be ignored: they gather their inputs from the tree.
    if let Some(tool) = tools.first().filter(|_| !history) {
        let refused = [
            (
                cli.post_options().active(),
                "--rank, --scope, --kind, --in or --callers",
            ),
            (cli.quiet, "-q/--quiet"),
        ];
        if let Some((_, flags)) = refused.iter().find(|(set, _)| *set) {
            eprintln!("ogrep: {} cannot be combined with {flags}", tool.flag);
            process::exit(2);
        }
        let narrowed = [
            (cli.changed_since.is_some(), "--changed-since"),
            (cli.at.is_some(), "--at"),
        ];
        if let Some((_, flag)) = narrowed.iter().find(|(set, _)| *set) {
            eprintln!("ogrep: {flag} cannot be combined with {}", tool.flag);
            process::exit(2);
        }
        let wrong_pattern = match tool.takes {
            Takes::Pattern => None,
            Takes::Subject => {
                Some("takes its subject as the pattern argument, not -e, -f or --ident")
            }
            Takes::Nothing => Some("takes no pattern"),
        };
        if let Some(problem) = wrong_pattern.filter(|_| cli.pattern_from_flags()) {
            eprintln!("ogrep: {} {problem}", tool.flag);
            process::exit(2);
        }
    }

    // --at refuses what reads the working-tree file after the search (--scope,
    // --kind, --in and --callers parse it; --files lists the tree).
    if cli.at.is_some() {
        let working_tree = [
            (cli.list_files, "--files"),
            (cli.scope, "--scope"),
            (cli.kind.is_some(), "--kind"),
            (cli.in_zone.is_some(), "--in"),
            (cli.callers, "--callers"),
            // Both read the files themselves: sizes and outlines would come
            // from the working tree, not the revision searched.
            (cli.about.is_some(), "--about"),
            (cli.cards.is_some(), "--cards"),
            (
                [&cli.sort, &cli.sortr]
                    .iter()
                    .any(|key| key.as_deref().is_some_and(|k| k != "path" && k != "none")),
                "--sort by time",
            ),
        ];
        if let Some((_, flag)) = working_tree.iter().find(|(set, _)| *set) {
            eprintln!("ogrep: --at cannot be combined with {flag}");
            process::exit(2);
        }
    }
    if history {
        let conflicts = [
            (cli.changed_since.is_some(), "--changed-since"),
            (
                cli.post_options().active(),
                "--rank, --scope, --kind, --in or --callers",
            ),
            (cli.files_without_match, "--files-without-match"),
            (cli.list_files, "--files"),
            (cli.passthru, "--passthru"),
            (cli.include_zero, "--include-zero"),
            (cli.invert_match, "-v/--invert-match"),
            (
                cli.sort.is_some() || cli.sortr.is_some() || cli.sort_files,
                "--sort",
            ),
            (cli.has_context(), "context lines (-A/-B/-C)"),
            (cli.vimgrep, "--vimgrep"),
            (
                !cli.with.is_empty() || !cli.without.is_empty(),
                "--with/--without",
            ),
            // A commit's changed lines are searched as one text: a multiline
            // match would span unrelated hunks and files.
            (cli.multiline || cli.multiline_dotall, "-U/--multiline"),
        ];
        if let Some((_, flag)) = conflicts.iter().find(|(set, _)| *set) {
            eprintln!("ogrep: --history cannot be combined with {flag}");
            process::exit(2);
        }
    }

    // Ranking and structure filters work on the files that matched; these
    // flags ask for something other than matching files, or for an order of
    // their own.
    if cli.post_options().active() {
        let conflicts = [
            (cli.files_without_match, "--files-without-match"),
            (cli.list_files, "--files"),
            (cli.passthru, "--passthru"),
            (cli.include_zero, "--include-zero"),
            (cli.invert_match, "-v/--invert-match"),
            (
                cli.sort.is_some() || cli.sortr.is_some() || cli.sort_files,
                "--sort",
            ),
        ];
        if let Some((_, flag)) = conflicts.iter().find(|(set, _)| *set) {
            eprintln!(
                "ogrep: --rank, --scope, --kind, --in and --callers cannot be combined with {flag}"
            );
            process::exit(2);
        }
    }
    // --rank prints files in its own order, dropping --scope's headers and
    // replacing --callers' listing; --callers lists definitions, not lines
    // to count or files to name.
    let post = cli.post_options();
    let shapes = [
        (
            post.rank.is_some() && cli.scope,
            "--rank cannot be combined with --scope",
        ),
        (
            post.rank.is_some() && cli.callers,
            "--rank cannot be combined with --callers",
        ),
        (
            cli.callers && (cli.count || cli.count_matches || cli.files_only),
            "--callers cannot be combined with -c, --count-matches or -l",
        ),
        // --callers is the references of a name; --kind would be overridden.
        (
            cli.callers && cli.kind.is_some(),
            "--callers cannot be combined with --kind",
        ),
        // --callers prints definitions, not match rows to shape.
        (
            cli.callers
                && (cli.only_matching
                    || cli.column
                    || cli.byte_offset
                    || cli.vimgrep
                    || cli.has_context()),
            "--callers cannot be combined with -o, --column, -b, --vimgrep or -A/-B/-C",
        ),
        // These read each match's text or place it in the file, and a
        // replaced line's matches are neither.
        (
            cli.replace.is_some() && post.active(),
            "--replace cannot be combined with --rank (or --budget, --per-file, --rank-cutoff, \
             --explain-rank), --scope, --kind, --in or --callers",
        ),
    ];
    if let Some((_, message)) = shapes.iter().find(|(set, _)| *set) {
        eprintln!("ogrep: {message}");
        process::exit(2);
    }

    // `--pcre2-version` reports the backtracking engine and exits, like ripgrep.
    // ogrep links fancy-regex rather than PCRE2 itself, so say so instead of
    // printing a PCRE2 version number that no PCRE2 is behind.
    if cli.pcre2_version {
        println!("fancy-regex (PCRE2-compatible backtracking engine; PCRE2 is not linked)");
        process::exit(0);
    }

    // Validate every parseable argument up front: a bad value must fail loudly
    // rather than silently searching with different settings than were asked
    // for, which is indistinguishable from "no matches".
    let resolved = match cli.resolve() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ogrep: {e}");
            process::exit(2);
        }
    };
    let max_filesize = resolved.max_filesize;

    // Refuse discovery flags the index-building subcommands can't honour. Done
    // before the match so the arms keep consuming `cli.command` by value.
    match &cli.command {
        Some(Command::Index { .. }) => reject_unsupported_discovery_flags(&cli, "index"),
        Some(Command::Serve { .. }) => reject_unsupported_discovery_flags(&cli, "serve"),
        _ => {}
    }

    let result = match cli.command {
        Some(Command::Index {
            path,
            exclude,
            strategy,
            index_buffer_mb,
            no_sparse_grams,
            force,
            history,
            history_depth,
            history_since,
            graph,
        }) => {
            let history_opts = history.then(|| ogrep_core::history::HistoryOptions {
                depth: match history_depth {
                    Some(0) => None,
                    Some(n) => Some(n),
                    None => Some(ogrep_core::history::DEFAULT_DEPTH),
                },
                since: history_since.clone(),
            });
            index::run(index::RunOptions {
                root: &path,
                index_path: cli.index_path.as_deref(),
                include_hidden: true,
                no_ignore,
                no_require_git: cli.no_require_git,
                exclude_dirs: &exclude,
                strategy: strategy.into(),
                index_buffer_mb,
                // Already carries `walker::DEFAULT_MAX_FILE_SIZE` when the user
                // named no limit, so indexing and searching cap identically.
                max_file_size: max_filesize,
                sparse_grams: !no_sparse_grams,
            })
            .and_then(|()| match history_opts {
                Some(opts) => index::run_history(&path, cli.index_path.as_deref(), &opts, force),
                None => Ok(()),
            })
            .and_then(|()| {
                if graph {
                    index::run_graph(&path, cli.index_path.as_deref())
                } else {
                    Ok(())
                }
            })
        }
        Some(Command::Serve {
            path,
            no_watch,
            watch_mode,
            poll_interval,
            watch_budget,
            max_memory_mb,
            max_cpu_percent,
            exclude,
            auto_save_mutations,
            watcher_queue_cap,
            search_concurrency,
        }) => {
            let memory_cap = max_memory_mb
                .map(|mb| mb.saturating_mul(1024 * 1024))
                .unwrap_or_else(mem::default_memory_cap_bytes);
            let index_threads = cpu::index_thread_count(max_cpu_percent.unwrap_or(50));
            serve::run(
                &path,
                cli.index_path.as_deref(),
                serve::ServeOptions {
                    no_watch,
                    watch_mode,
                    poll_interval: Duration::from_secs(poll_interval),
                    watch_budget: watch_budget as usize,
                    exclude_dirs: &exclude,
                    memory_cap_bytes: memory_cap,
                    index_threads,
                    search_concurrency: search_concurrency as usize,
                    no_ignore,
                    no_require_git: cli.no_require_git,
                    // Same resolved value the search path uses, so the index
                    // and the queries against it agree on what exists.
                    max_file_size: max_filesize,
                    auto_save_mutations,
                    // Clap's range bound guarantees this fits; saturating keeps
                    // the conversion total, and errs toward a large cap rather
                    // than a zero-length (rendezvous) queue.
                    watcher_queue_cap: watcher_queue_cap
                        .map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
                },
            )
        }
        Some(Command::Search {
            ref pattern,
            ref paths,
        }) => {
            let (pattern, paths) = cli.split_pattern_and_paths(Some(pattern), paths);
            run_search(&cli, pattern, &paths, &resolved)
        }
        Some(Command::Status { path }) => status::run(&path, cli.index_path.as_deref()),
        Some(Command::Refresh { path, timeout_secs }) => refresh::run(
            &path,
            cli.index_path.as_deref(),
            Duration::from_secs(timeout_secs),
            cli.json,
        ),
        Some(Command::CountFiles { path }) => walkcount::run(&path, cli.hidden, no_ignore),
        None => {
            if let Some(file) = &cli.rank_fit {
                run_rank_fit(&cli, file, &resolved)
            } else if cli.list_files {
                let opts = cli.build_search_opts(String::new(), &resolved);
                let paths = list_files_paths(&cli);
                list_files(&cli, &paths, &opts)
            } else if cli.conflicts {
                // --conflicts takes no pattern: a positional is the path.
                let paths: Vec<String> = cli.pattern.iter().chain(&cli.paths).cloned().collect();
                run_search(&cli, ".".to_string(), &paths, &resolved)
            } else if cli.pattern.is_some() || cli.pattern_from_flags() {
                let (pattern, paths) =
                    cli.split_pattern_and_paths(cli.pattern.as_ref(), &cli.paths);
                run_search(&cli, pattern, &paths, &resolved)
            } else {
                eprintln!("Usage: ogrep <pattern> [PATH ...]");
                eprintln!("       ogrep index [path]");
                eprintln!("       ogrep serve [path]");
                eprintln!("       ogrep status [path]");
                eprintln!("Search defaults to the current directory when no path is provided.");
                eprintln!("Run `ogrep --help` for full usage.");
                process::exit(2);
            }
        }
    };

    if let Err(e) = result {
        if search::is_broken_pipe(&e) {
            process::exit(0);
        }
        eprintln!("ogrep: {e}");
        process::exit(2);
    }
}

/// A path argument, kept alongside the exact text the user typed.
///
/// ripgrep echoes the argument verbatim into every path it prints, so the
/// original spelling (`.`, `./src`, `src\`, an absolute path) has to survive
/// past the point where the path is canonicalised for the actual walk.
struct SearchTarget {
    path: PathBuf,
    typed: String,
}

impl SearchTarget {
    /// How paths found under this argument should be printed.
    fn display(&self) -> output::PathDisplay {
        if self.typed.is_empty() {
            return output::PathDisplay::Bare;
        }
        if self.path.is_file() {
            return output::PathDisplay::Exact(self.typed.clone());
        }
        // A trailing separator the user typed is kept as typed; otherwise
        // ripgrep joins with the platform separator.
        let mut prefix = self.typed.clone();
        if !prefix.ends_with(['/', '\\']) {
            prefix.push(std::path::MAIN_SEPARATOR);
        }
        output::PathDisplay::Prefix(prefix)
    }
}

fn normalize_search_paths(paths: &[String]) -> Vec<SearchTarget> {
    if paths.is_empty() {
        return vec![SearchTarget {
            path: PathBuf::from("."),
            typed: String::new(),
        }];
    }

    paths
        .iter()
        .map(|path| {
            let mut trimmed = path.trim();
            loop {
                let unquoted = trimmed
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .or_else(|| {
                        trimmed
                            .strip_prefix('\'')
                            .and_then(|s| s.strip_suffix('\''))
                    });
                match unquoted {
                    Some(inner) => trimmed = inner.trim(),
                    None => break,
                }
            }
            SearchTarget {
                path: PathBuf::from(trimmed),
                typed: trimmed.to_string(),
            }
        })
        .collect()
}

fn list_files_paths(cli: &Cli) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(pattern) = cli.pattern.clone() {
        paths.push(pattern);
    }
    paths.extend(cli.paths.iter().cloned());
    paths
}

fn list_files(cli: &Cli, paths: &[String], opts: &search::SearchOptions) -> anyhow::Result<()> {
    // A bad glob or type is an argument error, reported before any path is
    // looked at, even a missing one, as rg does.
    if let Err(e) = opts.check_filters(opts.effective_case_insensitive()) {
        eprintln!("ogrep: {e}");
        process::exit(2);
    }
    let mut opts = opts.clone();
    let mut changed = history_search::ChangedSets::new(cli.changed_since.as_deref());
    for target in normalize_search_paths(paths) {
        if !target.path.exists() {
            report_missing_path(&target.path, opts.no_messages);
            continue;
        }
        opts.path_display = target.display();
        let restricted = restrict_to_changed(&mut changed, &opts, &target.path);
        search::list_files(
            &target.path,
            cli.index_path.as_deref(),
            restricted.as_ref().unwrap_or(&opts),
        )?;
    }
    Ok(())
}

/// ripgrep reports unreadable paths on stderr and finishes the remaining
/// paths, then exits 2. Silently skipping them hides typos and broken globs.
fn report_missing_path(path: &std::path::Path, no_messages: bool) {
    ERRORED.store(true, std::sync::atomic::Ordering::SeqCst);
    if !no_messages {
        // Ask the OS for the message instead of hard-coding one: the text and
        // the errno differ per platform, and on Windows even between a missing
        // file (error 2) and a missing directory component (error 3).
        let reason = match std::fs::metadata(path) {
            Err(err) => err.to_string(),
            // The path appeared between the existence check and this call.
            Ok(_) => std::io::Error::from(std::io::ErrorKind::NotFound).to_string(),
        };
        let path = path.display();
        eprintln!("ogrep: {path}: IO error for operation on {path}: {reason}");
    }
}

/// Exit as grep does for a tool's result: 0 when it found something, 1 when
/// not, 2 on an error (reported first, with its causes).
fn exit_with(result: anyhow::Result<bool>) -> ! {
    match result {
        Ok(found) => process::exit(if found { 0 } else { 1 }),
        Err(e) if search::is_broken_pipe(&e) => process::exit(0),
        Err(e) => {
            eprintln!("ogrep: {e:#}");
            process::exit(2);
        }
    }
}

/// `--changed-since`: the options for one path argument, limited to the files
/// changed since the given revision or time (`None` when the flag is absent).
/// Exits on a git error, like any other argument error.
fn restrict_to_changed(
    changed: &mut history_search::ChangedSets,
    opts: &search::SearchOptions,
    target: &Path,
) -> Option<search::SearchOptions> {
    if !changed.is_active() {
        return None;
    }
    // A named file is searched whole or not at all: ask about its directory.
    let (dir, file) = match target.file_name() {
        Some(name) if target.is_file() => (
            target.parent().filter(|p| !p.as_os_str().is_empty()),
            Some(name.to_string_lossy()),
        ),
        _ => (Some(target), None),
    };
    match changed
        .under(dir.unwrap_or(Path::new(".")))
        .map_err(|e| e.context("--changed-since"))
    {
        Ok(paths) => {
            let only = match (paths, file) {
                (Some(paths), Some(file)) if !paths.contains(file.as_ref()) => {
                    Some(Default::default())
                }
                (paths, None) => paths,
                _ => None,
            };
            Some(search::SearchOptions {
                only_paths: only.map(std::sync::Arc::new),
                ..opts.clone()
            })
        }
        Err(e) => exit_with(Err(e)),
    }
}

fn run_search(
    cli: &Cli,
    pattern: String,
    paths: &[String],
    resolved: &ResolvedArgs,
) -> anyhow::Result<()> {
    let mut opts = cli.build_search_opts(pattern, resolved);
    if !cli.pattern_file.is_empty() {
        let mut patterns = Vec::new();
        let mut read_stdin = false;
        for path in &cli.pattern_file {
            // In order, like ripgrep, so an earlier bad file is reported first.
            if path == "-" && std::mem::replace(&mut read_stdin, true) {
                eprintln!("ogrep: -f/--file: standard input can be read only once");
                process::exit(2);
            }
            match search::read_pattern_file(path) {
                Ok(read) => patterns.extend(read),
                Err(e) => {
                    eprintln!("ogrep: {path}: {e}");
                    process::exit(2);
                }
            }
        }
        opts.file_patterns = Some(patterns);
    }
    // Decided once from every pattern, not again per path argument.
    let ci = opts.effective_case_insensitive();
    // Every pattern source was empty (an empty -f file). Like ripgrep: no
    // match is possible, so without -v there is nothing to search or print.
    // With -v every line is selected, which the empty matcher provides. A bad
    // glob, type or --with/--without is still an argument error. (A search
    // reports those itself when it builds its filters, --no-messages or not,
    // so ordinary searches need no second build here.)
    if !opts.has_patterns() && !opts.invert_match {
        if let Err(e) = opts.check_filters(ci) {
            eprintln!("ogrep: {e}");
            process::exit(2);
        }
        process::exit(1);
    }
    let mut had_matches = false;

    // ripgrep decides these once, from the whole argument list: file names are
    // shown unless exactly one file was named, and line numbers only when
    // writing to a terminal.
    let targets = normalize_search_paths(paths);
    opts.no_filename = cli.no_filename
        || (!cli.with_filename && !cli.vimgrep && targets.len() == 1 && targets[0].path.is_file());

    // Tools that work on one path would drop the others without a word.
    if let Some(tool) = cli.tools().first()
        && targets.len() > 1
    {
        eprintln!("ogrep: {} takes one path at a time", tool.flag);
        process::exit(2);
    }

    if cli.history || cli.introduced {
        run_history_search(cli, &opts, ci, &targets);
    }
    let extras_mode = if cli.related_tests {
        Some(extras::Mode::RelatedTests {
            budget: cli.tool_budget(),
        })
    } else if cli.similar {
        Some(extras::Mode::Similar {
            budget: cli.tool_budget(),
        })
    } else if cli.conflicts {
        Some(extras::Mode::Conflicts)
    } else if cli.doc_drift {
        Some(extras::Mode::DocDrift)
    } else {
        None
    };
    if let Some(mode) = extras_mode {
        let root = &targets[0].path;
        exit_with(extras::run(
            root,
            cli.index_path.as_deref(),
            &opts,
            ci,
            mode,
        ));
    }
    let ops_mode = if cli.entities {
        Some(ops_search::Mode::Entities { dated: cli.dated })
    } else if cli.blast {
        Some(ops_search::Mode::Blast)
    } else if cli.suspects {
        Some(ops_search::Mode::Suspects)
    } else if cli.timeline {
        Some(ops_search::Mode::Timeline)
    } else if cli.stack_trace {
        Some(ops_search::Mode::Trace)
    } else if cli.log_templates {
        Some(ops_search::Mode::LogTemplates {
            new_since: cli.new_since.clone(),
            to_code: cli.template_to_code,
        })
    } else if cli.similar_incidents {
        Some(ops_search::Mode::SimilarIncidents {
            budget: cli.tool_budget(),
        })
    } else {
        None
    };
    if let Some(mode) = ops_mode {
        // --log-templates searches its path; the others work on the
        // repository around it.
        let root = &targets[0].path;
        exit_with(ops_search::run(
            root,
            cli.index_path.as_deref(),
            &opts,
            ci,
            &mode,
            cli.since.as_deref(),
            cli.until.as_deref(),
        ));
    }
    if cli.near || cli.map.is_some() || cli.callees {
        let target = &targets[0];
        let mode = if cli.callees {
            graph_search::Mode::Callees
        } else if let Some(tokens) = cli.map {
            graph_search::Mode::Map { tokens }
        } else {
            graph_search::Mode::Near {
                budget: cli.tool_budget(),
            }
        };
        exit_with(graph_search::run(
            &target.path,
            cli.index_path.as_deref(),
            &opts,
            ci,
            mode,
        ));
    }
    let post_opts = cli.post_options();
    if post_opts.active() {
        run_postprocessed_search(cli, &opts, ci, targets, &post_opts);
    }

    // One writer for the whole invocation: ripgrep emits a single JSON
    // `summary` covering every path it was given, so per-path writers would
    // report one summary each and a consumer reading "the" summary would see
    // only the first path's numbers.
    let mut writer = search::new_writer(&opts);

    let mut changed = history_search::ChangedSets::new(cli.changed_since.as_deref());
    for target in &targets {
        // --max-results spans every path argument.
        if writer.is_full() {
            break;
        }
        if !target.path.exists() {
            report_missing_path(&target.path, opts.no_messages);
            continue;
        }
        writer.set_path_display(target.display());
        let restricted = restrict_to_changed(&mut changed, &opts, &target.path);
        let result = search::run(
            &target.path,
            cli.index_path.as_deref(),
            restricted.as_ref().unwrap_or(&opts),
            ci,
            &mut writer,
        );
        match result {
            Ok(true) => {
                had_matches = true;
                if opts.quiet {
                    // `process::exit` skips destructors, so the summary and any
                    // buffered output have to be committed explicitly.
                    let _ = writer.finish();
                    process::exit(0);
                }
            }
            Ok(false) => {}
            Err(e) if search::is_broken_pipe(&e) => process::exit(0),
            Err(e) => {
                // --no-messages hides per-file search errors, as in ripgrep: a
                // file that cannot be read, or a regex failing on one. Anything
                // else (a bad pattern, a broken index, a busy server) is always
                // reported.
                let file_error = e.is::<std::io::Error>() || e.is::<matching::MatchError>();
                if !(opts.no_messages && file_error) {
                    eprintln!("ogrep: {e}");
                }
                let _ = writer.finish();
                process::exit(2);
            }
        }
    }

    writer.finish()?;

    // ripgrep's exit code: 0 on a match with no errors, 2 if anything was
    // reported to stderr, otherwise 1 for "no matches".
    let errored = ERRORED.load(std::sync::atomic::Ordering::SeqCst);
    // An identifier that matched nothing is often misspelt: name the closest
    // ones on stderr. Output and exit status stay those of the search.
    if !had_matches && !errored && cli.changed_since.is_none() && suggest::applies(&opts) {
        let roots: Vec<&std::path::Path> = targets.iter().map(|t| t.path.as_path()).collect();
        let names = suggest::similar_names(&roots, cli.index_path.as_deref(), &opts);
        if !names.is_empty() {
            eprintln!(
                "ogrep: no matches for {:?}; similar names: {}",
                opts.pattern,
                names.join(", ")
            );
        }
    }
    if had_matches && (opts.quiet || !errored) {
        process::exit(0);
    }
    if errored {
        process::exit(2);
    }
    process::exit(1);
}

/// `--rank-fit`: learn --rank weights from examples and store them.
fn run_rank_fit(cli: &Cli, file: &std::path::Path, resolved: &ResolvedArgs) -> anyhow::Result<()> {
    // The positional argument is the repository; there is no pattern.
    if !cli.paths.is_empty() {
        anyhow::bail!("--rank-fit takes one path at a time");
    }
    let root = PathBuf::from(cli.pattern.as_deref().unwrap_or("."));
    if !root.is_dir() {
        anyhow::bail!("--rank-fit: {} is not a directory", root.display());
    }
    #[derive(serde::Deserialize)]
    struct Item {
        query: String,
        relevant: Vec<String>,
    }
    let text = std::fs::read_to_string(file)?;
    let items: Vec<Item> = serde_json::from_str(&text).or_else(|_| {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<Vec<Item>, _>>()
    })?;
    let index_dir = search::index_dir_for(&root, cli.index_path.as_deref());
    let base = cli.build_search_opts(String::new(), resolved).for_capture();
    let mut examples = Vec::new();
    for item in items {
        let mut opts = base.clone();
        opts.pattern = item.query.clone();
        opts.fixed_string = true;
        opts.extra_patterns.clear();
        let mut capture = search::new_capturing_writer(&opts);
        capture.set_path_display(output::PathDisplay::Bare);
        search::run(&root, cli.index_path.as_deref(), &opts, false, &mut capture)?;
        examples.push(rank::Example {
            files: rank::group(vec![(output::PathDisplay::Bare, capture.take_captured())]),
            patterns: vec![item.query],
            fixed: true,
            ci: false,
            relevant: item.relevant.into_iter().collect(),
        });
    }
    let (weights, before, after) = rank::fit(&examples);
    std::fs::write(
        index_dir.join(rank::WEIGHTS_FILE),
        serde_json::to_vec_pretty(&weights)?,
    )?;
    eprintln!(
        "Fitted on {} examples: MRR {before:.3} with the defaults, {after:.3} with {}",
        examples.len(),
        serde_json::to_string(&weights)?
    );
    eprintln!("Wrote {}", index_dir.join(rank::WEIGHTS_FILE).display());
    Ok(())
}

/// `--history`/`--introduced`: search the history index. Never returns.
fn run_history_search(
    cli: &Cli,
    opts: &search::SearchOptions,
    ci: bool,
    targets: &[SearchTarget],
) -> ! {
    let target = &targets[0];
    let fail = |e: anyhow::Error| -> ! { exit_with(Err(e)) };
    let mut filters = history_search::Filters::default();
    if let Some(when) = &cli.since {
        filters.since = Some(
            history_search::resolve_when(&target.path, when, false)
                .unwrap_or_else(|e| fail(e.context("--since"))),
        );
    }
    if let Some(when) = &cli.until {
        filters.until = Some(
            history_search::resolve_when(&target.path, when, true)
                .unwrap_or_else(|e| fail(e.context("--until"))),
        );
    }
    if let Some(author) = &cli.author {
        filters.author = Some(
            regex::RegexBuilder::new(author)
                .case_insensitive(true)
                .build()
                .unwrap_or_else(|e| fail(anyhow::anyhow!("--author: {e}"))),
        );
    }
    let mode = if cli.introduced {
        history_search::Mode::Introduced
    } else {
        history_search::Mode::Log
    };
    exit_with(history_search::run(
        &target.path,
        cli.index_path.as_deref(),
        opts,
        ci,
        mode,
        &filters,
    ));
}

/// `--rank`, `--scope`, `--kind`, `--in`: run the ordinary search into a
/// capturing writer, then reshape and print (see `post`). Never returns.
fn run_postprocessed_search(
    cli: &Cli,
    opts: &search::SearchOptions,
    ci: bool,
    targets: Vec<SearchTarget>,
    post_opts: &post::PostOptions,
) -> ! {
    // The search must produce matching lines whatever the output format, so
    // file lists and counts are derived afterwards instead of during it.
    let inner = search::SearchOptions {
        sort: None,
        // The structure is looked up where each match is, and JSON prints
        // where it is.
        offsets: post_opts.parses() || opts.json,
        // Every match on a line when --kind/--in judge each one or the output
        // shows them; otherwise each line's first is enough (ranking and
        // scopes go by it), and a line with thousands costs no more, locally
        // or in a server's reply.
        all_spans: post_opts.filters_matches() || opts.wants_match_detail(),
        ..opts.for_capture()
    };
    let mut capture = search::new_capturing_writer(&inner);
    let mut captured = Vec::new();
    let mut changed = history_search::ChangedSets::new(cli.changed_since.as_deref());
    for target in targets {
        if !target.path.exists() {
            report_missing_path(&target.path, opts.no_messages);
            continue;
        }
        let display = target.display();
        capture.set_path_display(display.clone());
        let restricted = restrict_to_changed(&mut changed, &inner, &target.path);
        if let Err(e) = search::run(
            &target.path,
            cli.index_path.as_deref(),
            restricted.as_ref().unwrap_or(&inner),
            ci,
            &mut capture,
        ) {
            let file_error = e.is::<std::io::Error>() || e.is::<matching::MatchError>();
            if !(opts.no_messages && file_error) {
                eprintln!("ogrep: {e}");
            }
            process::exit(2);
        }
        // The fact cache lives beside an existing index; none is created.
        let index_dir = cli
            .index_path
            .clone()
            .or_else(|| {
                std::fs::canonicalize(&target.path)
                    .ok()
                    .filter(|p| p.is_dir())
                    .map(|p| builder::default_index_dir(&p))
            })
            .filter(|dir| dir.join("meta.json").is_file());
        captured.push(post::Captured {
            display,
            root: target.path.clone(),
            index_dir,
            events: capture.take_captured(),
        });
    }
    let mut writer = search::new_writer(opts);
    writer.absorb_search_totals(&capture);
    let printed = match post::emit(captured, opts, ci, post_opts, &mut writer) {
        Ok(printed) => printed,
        Err(e) if search::is_broken_pipe(&e) => process::exit(0),
        Err(e) => {
            eprintln!("ogrep: {e}");
            process::exit(2);
        }
    };
    if writer.finish().is_err() {
        process::exit(2);
    }
    let errored = ERRORED.load(std::sync::atomic::Ordering::SeqCst);
    if printed && (opts.quiet || !errored) {
        process::exit(0);
    }
    process::exit(if errored { 2 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> std::result::Result<Cli, clap::Error> {
        let args: Vec<String> = std::iter::once("ogrep")
            .chain(args.iter().copied())
            .map(String::from)
            .collect();
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || Cli::try_parse_args_from(args))
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn refresh_defaults_and_explicit_modes() {
        for (args, expected_mode, expected_no_watch) in [
            (vec!["serve"], serve::WatchMode::Auto, false),
            (
                vec!["serve", "--watch-mode", "auto"],
                serve::WatchMode::Auto,
                false,
            ),
            (
                vec!["serve", "--watch-mode", "poll"],
                serve::WatchMode::Poll,
                false,
            ),
            (vec!["serve", "--no-watch"], serve::WatchMode::Auto, true),
        ] {
            let Some(Command::Serve {
                no_watch,
                watch_mode,
                poll_interval,
                watch_budget,
                ..
            }) = parse(&args).unwrap().command
            else {
                panic!("expected serve");
            };
            assert_eq!(no_watch, expected_no_watch);
            assert_eq!(watch_mode, expected_mode);
            assert_eq!(poll_interval, 120);
            assert_eq!(watch_budget, 8192);
        }
    }

    #[test]
    fn refresh_explicit_values_and_boundaries() {
        for (interval, budget) in [("1", "1"), ("120", "8192"), ("86400", "4294967295")] {
            let Some(Command::Serve {
                poll_interval,
                watch_budget,
                ..
            }) = parse(&[
                "serve",
                "--poll-interval",
                interval,
                "--watch-budget",
                budget,
            ])
            .unwrap()
            .command
            else {
                panic!("expected serve");
            };
            assert_eq!(poll_interval, interval.parse::<u64>().unwrap());
            assert_eq!(watch_budget, budget.parse::<u32>().unwrap());
        }
        assert!(parse(&["serve", "--watch-mode", "auto", "--watch-budget", "8192"]).is_ok());
        assert!(parse(&["serve", "--watch-mode", "poll", "--poll-interval", "1"]).is_ok());
        assert!(parse(&["serve", "--no-watch", "--watcher-queue-cap", "1"]).is_ok());
    }

    #[test]
    fn refresh_explicit_conflicts_include_default_values() {
        for args in [
            vec!["--watch-mode", "auto"],
            vec!["--watch-mode", "poll"],
            vec!["--poll-interval", "120"],
            vec!["--poll-interval", "1"],
            vec!["--watch-budget", "8192"],
            vec!["--watch-budget", "1"],
        ] {
            for no_watch_first in [true, false] {
                let mut command = vec!["serve"];
                if no_watch_first {
                    command.push("--no-watch");
                }
                command.extend(&args);
                if !no_watch_first {
                    command.push("--no-watch");
                }
                let error = parse(&command).err().expect("explicit flags conflict");
                assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
            }
        }
        for args in [
            ["serve", "--watch-mode", "poll", "--watch-budget", "8192"],
            ["serve", "--watch-budget", "1", "--watch-mode", "poll"],
        ] {
            let error = parse(&args)
                .err()
                .expect("poll does not use native watches");
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
            assert!(error.to_string().contains("--watch-budget"));
        }
    }

    #[test]
    fn refresh_rejects_invalid_values() {
        for (flag, values) in [
            ("--watch-mode", vec!["native", "disabled", "AUTO", ""]),
            ("--poll-interval", vec!["0", "86401", "-1", "1.5", "bad"]),
            (
                "--watch-budget",
                vec!["0", "4294967296", "-1", "1.5", "bad"],
            ),
        ] {
            for value in values {
                assert!(
                    parse(&["serve", &format!("{flag}={value}")]).is_err(),
                    "{flag}={value} should be rejected"
                );
            }
            assert!(parse(&["serve", flag]).is_err());
        }
    }

    /// ripgrep lets the later of -w and -x win, including across the
    /// top-level and `search` command lines.
    #[test]
    fn word_and_line_regexp_override_each_other_in_argument_order() {
        for (args, word) in [
            (vec!["-x", "-w", "foo"], true),
            (vec!["-w", "-x", "foo"], false),
            (vec!["-x", "search", "-w", "foo"], true),
            (vec!["-w", "search", "-x", "foo"], false),
            (vec!["search", "-xw", "foo"], true),
            (vec!["search", "-wx", "foo"], false),
            (vec!["-w", "search", "-xw", "foo"], true),
            (vec!["-x", "search", "-wx", "foo"], false),
            (vec!["-w", "search", "foo"], true),
            (vec!["-x", "search", "foo"], false),
        ] {
            let cli = parse(&args).unwrap();
            assert_eq!(cli.word_regexp, word, "{args:?}");
            assert_eq!(cli.line_regexp, !word, "{args:?}");
        }
    }
}
