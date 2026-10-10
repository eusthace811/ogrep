/// `ogrep search` — search using the trigram index, with server delegation.
///
/// If a running server is detected (via serve.json), the search is delegated
/// over TCP. Otherwise, the on-disk index is loaded directly.
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use ogrep_core::builder;
use ogrep_core::filetypes;
use ogrep_core::meta::IndexMeta;
use ogrep_core::query::{self, QueryPlan};
use ogrep_core::reader::IndexReader;
use ogrep_core::walker;

use crate::matching::SearchMatcher;
use crate::output::{ColorMode, ContextLine, Match, OutputConfig, OutputFormat, OutputWriter};
use crate::serve::ServerInfo;

#[derive(Debug)]
struct ServerOverloaded(String);

impl std::fmt::Display for ServerOverloaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server busy: {}; retry later", self.0)
    }
}
impl std::error::Error for ServerOverloaded {}

/// A reply the server cut at `--max-results` that turned out short once the
/// client dropped rows it never prints (binary files, depth): ask again whole.
#[derive(Debug)]
struct LimitShort;

impl std::fmt::Display for LimitShort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("limited reply fell short")
    }
}
impl std::error::Error for LimitShort {}

fn server_error(error: &serde_json::Value) -> anyhow::Error {
    let message = error
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown error");
    if error.get("code").and_then(|v| v.as_i64()) == Some(crate::serve::OVERLOADED) {
        ServerOverloaded(message.to_owned()).into()
    } else {
        anyhow::anyhow!("server error: {message}")
    }
}

#[derive(Clone)]
#[cfg_attr(test, derive(Default))]
pub struct SearchOptions {
    pub pattern: String,
    pub extra_patterns: Vec<String>,
    /// `--with`: every file searched must also contain a line matching each.
    pub with_patterns: Vec<String>,
    /// `--without`: no file searched may contain a line matching any.
    pub without_patterns: Vec<String>,
    /// The patterns `-f` named, read once when the search starts (see
    /// [`read_pattern_file`]): the case decision, matcher, index plan, server
    /// request and every path argument all use them, and `-f` may name a pipe
    /// that can be read only once.
    pub file_patterns: Option<Vec<String>>,
    pub case_insensitive: bool,
    pub case_sensitive: bool,
    pub smart_case: bool,
    pub fixed_string: bool,
    pub files_only: bool,
    pub files_without_match: bool,
    pub count: bool,
    pub word_boundary: bool,
    pub max_count: Option<usize>,
    /// `--max-results`: stop after this many results in total (matching
    /// lines, or files with `-l`/`-c`), in output order.
    pub max_results: Option<u64>,
    pub json: bool,
    pub vimgrep: bool,
    pub stats: bool,
    pub no_index: bool,
    pub glob: Vec<String>,
    pub iglob: Vec<String>,
    pub glob_case_insensitive: bool,
    /// Report every match span of a line even when the output would not
    /// need them: a captured search is reshaped afterwards (--kind and --in
    /// judge each match, --count-matches counts them), not printed.
    pub all_spans: bool,
    /// A capture for reshaped output (`for_capture`): it keeps binary
    /// files' matching lines, which reshaping counts, filters or prints as a
    /// plain search would (tools that want text lines keep only the note),
    /// and each line's first match even when it needs no others, since
    /// reshaping places and ranks lines by it.
    pub reshaping: bool,
    /// Whether a capture needs each match's offset in its file.
    pub offsets: bool,
    /// `--changed-since`: search only these paths, relative to the search
    /// root. Set per path argument.
    pub only_paths: Option<std::sync::Arc<std::collections::HashSet<String>>>,
    pub types: Vec<String>,
    pub types_not: Vec<String>,
    pub type_add: Vec<String>,
    pub type_clear: Vec<String>,
    pub invert_match: bool,
    pub only_matching: bool,
    pub after_context: Option<usize>,
    pub before_context: Option<usize>,
    pub context: Option<usize>,
    pub heading: Option<bool>,
    pub color: ColorMode,
    pub null: bool,
    pub trim: bool,
    pub multiline: bool,
    pub multiline_dotall: bool,
    pub no_ignore: bool,
    pub hidden: bool,
    pub quiet: bool,
    pub no_filename: bool,
    pub no_line_number: bool,
    pub text: bool,
    /// `--binary`: walk into files an extension check would reject, and report
    /// a "binary file matches" note for them instead of printing their lines.
    pub binary: bool,
    /// How to rebuild printed paths from the search-root-relative ones. Set per
    /// path argument, since each argument is echoed back exactly as typed.
    pub path_display: crate::output::PathDisplay,
    pub max_filesize: Option<u64>,
    /// Whether `--max-filesize` was passed, as opposed to inheriting
    /// [`ogrep_core::walker::DEFAULT_MAX_FILE_SIZE`]. See
    /// [`exceeds_max_filesize`].
    pub max_filesize_requested: bool,
    pub encoding: ogrep_core::encoding::EncodingMode,
    pub follow: bool,
    pub no_messages: bool,
    /// `--no-ignore-messages`: suppress errors from unparseable ignore files.
    /// Independent of `--no-messages`, which suppresses them as well.
    pub no_ignore_messages: bool,
    // ── Matching ──
    pub line_regexp: bool,
    pub no_unicode: bool,
    pub engine: crate::matching::RegexEngine,
    pub regex_size_limit: Option<usize>,
    pub dfa_size_limit: Option<usize>,
    pub replace: Option<String>,
    pub passthru: bool,
    pub stop_on_nonmatch: bool,
    // ── Output detail ──
    pub column: bool,
    pub byte_offset: bool,
    pub max_columns: Option<usize>,
    pub max_columns_preview: bool,
    pub count_matches: bool,
    pub include_zero: bool,
    pub context_separator: Option<String>,
    pub field_match_separator: String,
    pub field_context_separator: String,
    pub path_separator: Option<String>,
    pub sort: Option<SortMode>,
    /// `--at`: search the tree of this revision instead of the working tree.
    pub at_rev: Option<String>,
    // ── File discovery ──
    pub max_depth: Option<usize>,
    pub one_file_system: bool,
    pub ignore_files: Vec<String>,
    pub ignore_file_case_insensitive: bool,
    pub no_ignore_dot: bool,
    pub no_ignore_exclude: bool,
    pub no_ignore_global: bool,
    pub no_ignore_parent: bool,
    pub no_ignore_vcs: bool,
    pub no_require_git: bool,
    pub threads: Option<usize>,
    pub line_buffered: bool,
}

/// What `--sort`/`--sortr` orders results by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortMode {
    pub key: SortKey,
    pub reverse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Path,
    Modified,
    Accessed,
    Created,
}

/// Whether `error` is a write to a closed pipe (`ogrep ... | head`). ripgrep
/// stops quietly with status 0 then; so does ogrep, and a server search that
/// hit it must not be retried against the local index.
pub fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        e.downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == ErrorKind::BrokenPipe)
    })
}

fn flush_before_stats(writer: &mut OutputWriter) -> Result<()> {
    match writer.flush() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

impl SortKey {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "path" => Some(Self::Path),
            "modified" => Some(Self::Modified),
            "accessed" => Some(Self::Accessed),
            "created" => Some(Self::Created),
            _ => None,
        }
    }
}

impl SortMode {
    /// Order paths for `--sort`/`--sortr`.
    ///
    /// Metadata reads can fail (races, permissions); those paths sort first
    /// ascending so a transient error can't drop a file from the results.
    pub fn apply(&self, paths: &mut [PathBuf]) {
        match self.key {
            SortKey::Path => paths.sort(),
            _ => {
                let key = self.key;
                // Cached: `sort_by_key` re-reads the key on every comparison,
                // which would be O(n log n) metadata syscalls over a large repo.
                paths.sort_by_cached_key(|p| (time_key(p, key), p.clone()));
            }
        }
        if self.reverse {
            paths.reverse();
        }
    }

    /// Sort index-backed entries by search-relative paths, looking up metadata
    /// at their actual locations. `PathBuf` keeps ordering component-wise, as
    /// in the filesystem walk, rather than comparing raw path strings.
    fn apply_indexed<T>(
        &self,
        entries: &mut [T],
        index_root: &Path,
        scope: &IndexScope,
        relative_path: impl Fn(&T) -> &str,
    ) {
        entries.sort_by_cached_key(|entry| {
            let rel = relative_path(entry);
            let time = if self.key == SortKey::Path {
                None
            } else {
                time_key(&scope.full_path(index_root, rel), self.key)
            };
            (time, PathBuf::from(rel))
        });
        if self.reverse {
            entries.reverse();
        }
    }
}

/// Read the timestamp `--sort` selected, if the platform records it.
fn time_key(path: &Path, key: SortKey) -> Option<std::time::SystemTime> {
    let md = std::fs::metadata(path).ok()?;
    match key {
        SortKey::Modified => md.modified().ok(),
        SortKey::Accessed => md.accessed().ok(),
        SortKey::Created => md.created().ok(),
        SortKey::Path => None,
    }
}

impl SearchOptions {
    /// Whether the search ignores case. `-i` wins over `-s`, which wins over
    /// `-S`, matching ripgrep. Like ripgrep, smart case looks at every pattern
    /// together (`-e`, `-f`, `--ident`, and the `--with`/`--without` conditions
    /// too) and ignores case only when some pattern matches a literal and none
    /// of those literals is uppercase; see [`query::LiteralCase`] for what
    /// counts as a literal under each engine.
    pub fn effective_case_insensitive(&self) -> bool {
        if self.case_insensitive {
            return true;
        }
        if !self.smart_case || self.case_sensitive {
            return false;
        }
        let pcre = self.engine == crate::matching::RegexEngine::Pcre2;
        // The main patterns form one alternation and share an engine; each
        // file condition is compiled on its own.
        let patterns: Vec<&str> = self.pattern_refs().collect();
        let mut case = query::LiteralCase::of_all(&patterns, self.fixed_string, pcre);
        for condition in self.with_patterns.iter().chain(&self.without_patterns) {
            case = case.merge(query::LiteralCase::of_all(
                &[condition.as_str()],
                self.fixed_string,
                pcre,
            ));
        }
        case.ignores_case()
    }

    /// Whether `.` should match a newline. ripgrep keeps this separate from
    /// `-U`, so `-U` alone does not turn `.` into a line-crossing wildcard.
    pub fn dotall(&self) -> bool {
        self.multiline_dotall
    }

    /// Walk options shared by every non-indexed traversal.
    ///
    /// Unlike index building, searching has no size cap by default: ripgrep
    /// searches files of any size, so a cap here silently hides results.
    pub fn walk_options(&self) -> walker::WalkOptions {
        walker::WalkOptions {
            include_hidden: self.hidden,
            no_ignore: self.no_ignore,
            search_binary: self.text || self.binary,
            follow_links: self.follow,
            max_file_size: self.max_filesize,
            max_depth: self.max_depth,
            same_file_system: self.one_file_system,
            ignore_files: self.ignore_files.iter().map(Into::into).collect(),
            ignore_files_case_insensitive: self.ignore_file_case_insensitive,
            no_ignore_dot: self.no_ignore_dot,
            no_ignore_exclude: self.no_ignore_exclude,
            no_ignore_global: self.no_ignore_global,
            no_ignore_parent: self.no_ignore_parent,
            no_ignore_vcs: self.no_ignore_vcs,
            no_require_git: self.no_require_git,
            // ripgrep gates ignore-file errors on both flags: `--no-messages`
            // suppresses them too, "regardless of" `--no-ignore-messages`.
            no_ignore_messages: self.no_messages || self.no_ignore_messages,
            threads: self.threads,
            ..Default::default()
        }
    }

    pub(crate) fn glob_filter(&self) -> Result<crate::glob_filter::GlobFilter> {
        Ok(crate::glob_filter::GlobFilter::new(
            &self.glob,
            &self.iglob,
            self.glob_case_insensitive,
        )?
        .restricted_to(self.only_paths.clone()))
    }

    /// Whether the reply has to carry per-match spans and columns.
    ///
    /// Every consumer of that data is listed here; anything else prints the
    /// line as the server rendered it, so the arrays would be built, sent,
    /// parsed and dropped. The check is conservative — when in doubt the data
    /// is requested, because missing spans degrade output silently.
    pub(crate) fn wants_match_detail(&self) -> bool {
        self.color.is_enabled()      // highlighting brackets each span
            || self.json             // reports them as `submatches`
            || self.vimgrep          // one row per column
            || self.column           // leading column field
            || self.count_matches    // counts matches, not matching lines
            || self.only_matching    // prints the spans themselves
            || self.replace.is_some() // rewrites the matched text
            || self.stats // reports a match total, not a line total
            || self.all_spans // captured for reshaping
    }

    /// Whether the reply has to carry per-row `offset` and `term`.
    ///
    /// JSON needs source offsets and exact line termination, just as explicit
    /// byte-offset and maximum-column output need their positional metadata.
    fn wants_position_detail(&self) -> bool {
        self.json || self.byte_offset || self.max_columns.is_some() || self.offsets
    }

    /// `--only-matching` as the search should actually apply it.
    ///
    /// ripgrep implements `-o` in its standard printer only: the JSON printer
    /// always reports whole lines with one `submatch` per hit, so `--json -o`
    /// and `--json` produce byte-identical streams. Fanning a line out into one
    /// event per match here would both reshape the stream and inflate
    /// `matched_lines`, which counts distinct lines.
    pub(crate) fn effective_only_matching(&self) -> bool {
        self.only_matching && !self.json
    }

    fn effective_passthru(&self) -> bool {
        // ripgrep treats `-m 0` as suppressing all search output, including
        // passthru context.
        self.passthru
            && self.max_count != Some(0)
            && !self.files_only
            && !self.files_without_match
            && !self.count
            && !self.count_matches
            && !self.quiet
    }

    /// Whether one matching line fully decides this file's output.
    ///
    /// `-l` prints the file name once whatever else matches, so the rest of the
    /// file need not be searched. `--stats` still wants every match counted,
    /// and `-m 0` has to keep suppressing the file.
    fn stops_at_first_line(&self) -> bool {
        self.files_only
            && !self.count
            && !self.count_matches
            && !self.stats
            && !self.invert_match
            && self.max_count != Some(0)
    }

    /// The matching-relevant subset, shared with the server path.
    pub(crate) fn match_options(&self) -> crate::matching::MatchOptions {
        crate::matching::MatchOptions {
            invert_match: self.invert_match,
            multiline: self.multiline,
            only_matching: self.effective_only_matching(),
            before_context: self.before_ctx(),
            after_context: self.after_ctx(),
            // `-q`, `--files-without-match` and `-l` only need to know whether
            // the file matched at all, so stop at the first hit.
            max_count: if self.quiet || self.files_without_match || self.stops_at_first_line() {
                Some(1)
            } else {
                self.max_count
            },
            // `--passthru` prints the whole file, so it cannot also stop early
            // at a match limit, and it is meaningless when only file names or
            // counts are printed.
            passthru: self.effective_passthru(),
            replace: self.replace.clone(),
            stop_on_nonmatch: self.stop_on_nonmatch,
            vimgrep: self.vimgrep,
            all_spans: self.wants_match_detail(),
        }
    }

    pub(crate) fn matcher(&self, ci: bool) -> Result<SearchMatcher> {
        crate::matching::build_search_matcher(&self.all_patterns(), &self.matcher_config(ci))
    }

    pub(crate) fn file_filter(&self, ci: bool) -> Result<Option<crate::matching::FileFilter>> {
        crate::matching::FileFilter::build(
            &self.with_patterns,
            &self.without_patterns,
            &self.matcher_config(ci),
        )
    }

    /// Pattern-compilation settings, shared with the server path.
    pub fn matcher_config(&self, ci: bool) -> crate::matching::MatcherConfig {
        crate::matching::MatcherConfig {
            case_insensitive: ci,
            fixed_string: self.fixed_string,
            word_boundary: self.word_boundary,
            line_regexp: self.line_regexp,
            multiline: self.multiline,
            dotall: self.dotall(),
            no_unicode: self.no_unicode,
            engine: self.engine,
            regex_size_limit: self.regex_size_limit,
            dfa_size_limit: self.dfa_size_limit,
        }
    }

    /// Compile `--type-add`/`--type-clear` into the effective definitions, then
    /// build the `-t`/`-T` matcher. Shared with the server via the same fields
    /// so both sides derive an identical filter.
    pub fn type_filter(&self) -> Result<filetypes::TypeFilter> {
        build_type_filter(
            &self.type_add,
            &self.type_clear,
            &self.types,
            &self.types_not,
        )
    }

    /// Effective after-context lines.
    pub fn after_ctx(&self) -> usize {
        context_lines(self.after_context, self.context)
    }
    /// Effective before-context lines.
    pub fn before_ctx(&self) -> usize {
        context_lines(self.before_context, self.context)
    }
    /// Whether lines around the matches are printed.
    pub fn has_context(&self) -> bool {
        self.before_ctx() > 0 || self.after_ctx() > 0
    }

    fn make_output_config(&self) -> OutputConfig {
        let format = if self.json {
            OutputFormat::Json
        } else if self.files_only {
            OutputFormat::FilesOnly
        } else if self.count || self.count_matches {
            OutputFormat::Count
        } else if self.vimgrep {
            OutputFormat::Vimgrep
        } else {
            // Heading vs flat is resolved in the writer.
            OutputFormat::Heading
        };
        OutputConfig {
            format,
            color: self.color,
            heading: self.heading,
            null: self.null,
            trim: self.trim,
            no_filename: self.no_filename,
            no_line_number: self.no_line_number,
            context: self.has_context(),
            column: self.column,
            byte_offset: self.byte_offset,
            max_columns: self.max_columns,
            max_columns_preview: self.max_columns_preview,
            context_separator: self.context_separator.clone(),
            field_match_separator: self.field_match_separator.clone(),
            field_context_separator: self.field_context_separator.clone(),
            path_separator: self.path_separator.clone(),
            path_display: self.path_display.clone(),
            line_buffered: self.line_buffered,
            limit: self.max_results,
            after_context: self.after_ctx(),
        }
    }

    /// Every pattern (primary, `-e`/`--ident`, `-f`), borrowed.
    fn pattern_refs(&self) -> impl Iterator<Item = &str> {
        let has_extra = !self.extra_patterns.is_empty() || self.file_patterns.is_some();
        // `-e`/`-f` move the positional argument into the path list, leaving the
        // base pattern empty. Keeping it would contribute an empty alternative
        // that matches every line.
        let primary = (!has_extra || !self.pattern.is_empty()).then_some(self.pattern.as_str());
        primary
            .into_iter()
            .chain(self.extra_patterns.iter().map(String::as_str))
            .chain(self.file_patterns.iter().flatten().map(String::as_str))
    }

    /// Collect all patterns (primary + -e extras + -f file patterns).
    pub(crate) fn all_patterns(&self) -> Vec<String> {
        self.pattern_refs().map(str::to_string).collect()
    }

    /// Whether there is any pattern at all. There is none when every pattern
    /// source was empty (an empty `-f` file): then nothing can match, and `-v`
    /// selects every line.
    pub fn has_patterns(&self) -> bool {
        self.pattern_refs().next().is_some()
    }

    /// Build the glob, type and `--with`/`--without` filters, reporting the
    /// first invalid one, without searching: a bad argument is rejected up
    /// front, even for a search that needs no work, as ripgrep does.
    pub fn check_filters(&self, ci: bool) -> Result<()> {
        self.glob_filter()?;
        self.type_filter()?;
        self.file_filter(ci)?;
        Ok(())
    }
}

/// The patterns in a `-f` file, or standard input for `-`, as ripgrep reads
/// them: one per line, `\r\n` or `\n` terminated, spaces kept, and a blank
/// line an empty pattern that matches every line.
pub fn read_pattern_file(path: &str) -> std::io::Result<Vec<String>> {
    let text = if path == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(path)?
    };
    Ok(text.lines().map(str::to_string).collect())
}

/// A regex matching `name` in every common naming convention.
///
/// The name is split into words at `_`, `-`, spaces, lower-to-upper case
/// changes and the end of an acronym (`HTTPServer` is `HTTP` + `Server`), and
/// the words are rejoined allowing an optional `_` or `-` between each, case
/// insensitively. `getUserId`, `get_user_id`, `GetUserID`, `get-user-id` and
/// `GET_USER_ID` all produce, and all match, `(?i:get[_-]?user[_-]?id)`. The
/// literal words stay in the pattern, so indexed searches still narrow by them.
pub fn identifier_pattern(name: &str) -> String {
    let words = identifier_words(name);
    if words.is_empty() {
        return regex::escape(name);
    }
    let body: Vec<String> = words.iter().map(|w| regex::escape(w)).collect();
    format!("(?i:{})", body.join("[_-]?"))
}

fn identifier_words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' || c.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let boundary = c.is_uppercase()
            && prev.is_some_and(|p| {
                p.is_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_uppercase() && next.is_some_and(char::is_lowercase))
            });
        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Build the [`OutputWriter`] that spans a whole invocation.
///
/// Kept here so `make_output_config` stays private to this module.
pub fn new_writer(opts: &SearchOptions) -> OutputWriter {
    OutputWriter::new(opts.make_output_config())
}

/// The index directory for a search of `root`: `--index-path`, else
/// `.ogrep` in the (canonical) root.
pub(crate) fn index_dir_for(root: &Path, index_path: Option<&Path>) -> PathBuf {
    index_path.map(Path::to_path_buf).unwrap_or_else(|| {
        builder::default_index_dir(
            &std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
        )
    })
}

/// Context lines on one side: -A or -B, else -C, else none.
pub fn context_lines(side: Option<usize>, both: Option<usize>) -> usize {
    side.or(both).unwrap_or(0)
}

/// A writer that records what `opts` finds instead of printing it, keeping
/// binary files' lines only for reshaping (`opts.reshaping`).
pub fn new_capturing_writer(opts: &SearchOptions) -> OutputWriter {
    OutputWriter::capturing(opts.make_output_config(), opts.reshaping)
}

impl SearchOptions {
    /// These options with every output-shaping flag off, for a search whose
    /// lines are captured and reshaped afterwards (`--rank`, `--scope`, the
    /// graph, history and log tools): counts, file lists and JSON are derived
    /// from the captured lines, never produced by the search itself.
    pub fn for_capture(&self) -> SearchOptions {
        SearchOptions {
            files_only: false,
            files_without_match: false,
            count: false,
            count_matches: false,
            // Whole lines: reshaping needs them, and -o is applied on output.
            only_matching: false,
            reshaping: true,
            json: false,
            vimgrep: false,
            quiet: false,
            all_spans: true,
            ..self.clone()
        }
    }
}

/// Run the search for `opts` with output shaping off (see
/// [`SearchOptions::for_capture`]) and return its matching lines.
pub fn captured_matches(
    root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
) -> Result<Vec<crate::output::Match>> {
    // Callers use the lines, not their spans or anything that shapes output.
    let opts = SearchOptions {
        all_spans: false,
        reshaping: false,
        color: crate::output::ColorMode::Never,
        column: false,
        only_matching: false,
        stats: false,
        ..opts.for_capture()
    };
    let mut capture = new_capturing_writer(&opts);
    run(root, index_path, &opts, ci, &mut capture)?;
    // The capture keeps no binary file's lines: they are not text these
    // callers can use.
    Ok(capture
        .take_captured()
        .into_iter()
        .filter_map(|c| match c {
            crate::output::Captured::Match(m) => Some(m),
            _ => None,
        })
        .collect())
}

/// List files that would be searched (`--files` mode).
pub fn list_files(root: &Path, index_path: Option<&Path>, opts: &SearchOptions) -> Result<()> {
    let start = Instant::now();
    let root = match std::fs::canonicalize(root) {
        Ok(root) => root,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let glob_filter = opts.glob_filter()?;
    let type_filter = opts.type_filter()?;

    if root.is_file() {
        let rel_path = explicit_file_display_path(&root);

        if passes_filters(&rel_path, &glob_filter, &type_filter) {
            let mut writer = OutputWriter::new(opts.make_output_config());
            writer.write_file(&rel_path)?;
            writer.flush()?;
        }
        return Ok(());
    }

    let index_dir = index_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| builder::default_index_dir(&root));

    if filename_index_compatible(opts) {
        if let Ok(info) = ServerInfo::load(&index_dir)
            && let Some((index_root, scope)) = resolve_scope(&index_dir, &root)
        {
            let paths = match list_files_via_server(&info, &scope, opts) {
                Ok(paths) => Some(paths),
                Err(error) if error.is::<ServerOverloaded>() => return Err(error),
                Err(_) => None,
            };
            if let Some(paths) = paths {
                write_indexed_file_paths(
                    &index_root,
                    &root,
                    &scope,
                    paths,
                    None,
                    &glob_filter,
                    &type_filter,
                    opts,
                )?;
                if opts.stats {
                    eprintln!(
                        "Filename search completed in {:.1}ms (via server)",
                        start.elapsed().as_secs_f64() * 1000.0
                    );
                }
                return Ok(());
            }
        }

        match load_indexed_file_paths(&index_dir) {
            Ok(Some((paths, visibility))) => {
                if let Some((index_root, scope)) = resolve_scope(&index_dir, &root) {
                    write_indexed_file_paths(
                        &index_root,
                        &root,
                        &scope,
                        paths,
                        Some(&visibility),
                        &glob_filter,
                        &type_filter,
                        opts,
                    )?;
                    if opts.stats {
                        eprintln!(
                            "Filename search completed in {:.1}ms (via local index)",
                            start.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    return Ok(());
                }
            }
            Ok(None) => {}
            Err(error) => {
                if !opts.no_messages {
                    eprintln!("warning: filename index unavailable ({error}) - walking filesystem");
                }
            }
        }
    }

    // `--files` lists what would be searched without reading anything, so it
    // must not drop files on an extension guess the way a search does.
    let walk_opts = walker::WalkOptions {
        search_binary: true,
        exclude_paths: index_storage_exclusions(&index_dir)?,
        overrides: Some(glob_filter.walk_overrides(&root)?),
        types: type_filter.walk_types()?,
        ..opts.walk_options()
    };
    let walk = walker::walk_dir(&root, &walk_opts);
    let paths = walk.files.iter().map(|path| {
        path.strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    });
    write_indexed_file_paths(
        &root,
        &root,
        &IndexScope::Whole,
        paths,
        None,
        &glob_filter,
        &type_filter,
        opts,
    )?;
    if opts.stats {
        eprintln!(
            "Filename search completed in {:.1}ms (via filesystem walk)",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(())
}

/// Whether an existing index was built under traversal rules that can answer
/// this invocation without silently omitting files.
fn filename_index_compatible(opts: &SearchOptions) -> bool {
    !opts.no_index
        && !opts.no_ignore
        && !opts.no_ignore_dot
        && !opts.no_ignore_exclude
        && !opts.no_ignore_global
        && !opts.no_ignore_parent
        && !opts.no_ignore_vcs
        && !opts.no_require_git
        && !opts.follow
        && !opts.one_file_system
        && opts.ignore_files.is_empty()
        && !opts.ignore_file_case_insensitive
        && !opts.max_filesize_requested
}

/// Load a complete local filename index, including the paths deliberately
/// omitted from the content index.
fn load_indexed_file_paths(
    index_dir: &Path,
) -> Result<Option<(Vec<String>, ogrep_core::visibility::PathVisibility)>> {
    if !index_dir.join("lookup.bin").exists() {
        return Ok(None);
    }
    let filename_index = match ogrep_core::path_index::read_filename_index(index_dir) {
        Ok(Some(index)) => index,
        Ok(None) | Err(_) => return Ok(None),
    };
    let Some(visibility) = filename_index.visibility else {
        return Ok(None);
    };
    let mut extra_paths = filename_index.paths;
    let meta = IndexMeta::load(index_dir)?;
    if !meta.complete
        || !meta.hidden_complete
        || meta.version != ogrep_core::meta::INDEX_FORMAT_VERSION
        || !visibility.hidden_complete
    {
        return Ok(None);
    }

    let reader = IndexReader::open(index_dir)?;
    if !visibility.covers_index(&meta, reader.file_table_id()) {
        return Ok(None);
    }
    let mut paths = Vec::with_capacity(reader.num_files() + extra_paths.len());
    paths.extend(reader.all_paths().iter().map(str::to_string));
    paths.append(&mut extra_paths);
    Ok(Some((paths, visibility.paths)))
}

/// Apply search-root scoping and CLI filters to index-root-relative paths.
#[allow(clippy::too_many_arguments)]
fn write_indexed_file_paths(
    index_root: &Path,
    search_root: &Path,
    scope: &IndexScope,
    paths: impl IntoIterator<Item = String>,
    visibility: Option<&ogrep_core::visibility::PathVisibility>,
    glob_filter: &crate::glob_filter::GlobFilter,
    type_filter: &filetypes::TypeFilter,
    opts: &SearchOptions,
) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut paths: Vec<String> = paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .filter(|path| {
            visibility.is_none_or(|visibility| {
                visibility.is_visible_with_file_whitelist(
                    path,
                    scope.prefix(),
                    opts.hidden,
                    type_filter.has_selection(),
                )
            })
        })
        .filter_map(|path| scope.relativize(&path, search_root))
        .filter(|path| within_max_depth(path, opts))
        .filter(|path| passes_filters(path, glob_filter, type_filter))
        .collect();

    if let Some(sort) = opts.sort {
        sort.apply_indexed(&mut paths, index_root, scope, String::as_str);
    }

    let mut writer = OutputWriter::new(opts.make_output_config());
    for path in paths {
        writer.write_file(&path)?;
    }
    writer.flush()?;
    Ok(())
}

fn list_files_via_server(
    info: &ServerInfo,
    scope: &IndexScope,
    opts: &SearchOptions,
) -> Result<Vec<String>> {
    let mut stream = info.connect(None)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(300)))?;
    crate::serve::write_line(
        &mut stream,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "files",
            "params": { "hidden": opts.hidden, "scope": scope.prefix(), "whitelist_files": !opts.types.is_empty() },
            "id": 1,
        })
        .to_string(),
    )?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response: serde_json::Value = serde_json::from_str(&line)?;
    if let Some(error) = response.get("error") {
        return Err(server_error(error));
    }

    let result = response
        .get("result")
        .ok_or_else(|| anyhow::anyhow!("server response did not contain a result"))?;
    // `--files` never carries content conditions (they are rejected up front).
    require_hidden_coverage(result.get("hidden_complete").and_then(|v| v.as_bool()))?;
    result
        .get("files")
        .and_then(|files| files.as_array())
        .ok_or_else(|| anyhow::anyhow!("server response did not contain files"))?
        .iter()
        .map(|path| {
            path.as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("server returned a non-string file path"))
        })
        .collect()
}

/// Refuse a reply that does not cover hidden files. Older servers ignore
/// request fields they do not know, so without this explicit flag they would
/// silently skip what `--hidden` asked for.
fn require_hidden_coverage(hidden_complete: Option<bool>) -> Result<()> {
    if hidden_complete != Some(true) {
        anyhow::bail!("server does not support complete hidden-file coverage");
    }
    Ok(())
}

/// Refuse a search reply the server could not answer completely: hidden-file
/// coverage, and `--with`/`--without`, which an older server would ignore.
fn require_server_support(
    opts: &SearchOptions,
    hidden_complete: Option<bool>,
    file_filters: Option<bool>,
) -> Result<()> {
    require_hidden_coverage(hidden_complete)?;
    if (!opts.with_patterns.is_empty() || !opts.without_patterns.is_empty())
        && file_filters != Some(true)
    {
        anyhow::bail!("server does not support --with/--without");
    }
    Ok(())
}

/// [`require_server_support`] for a reply parsed as an untyped `result`.
fn require_server_support_in(opts: &SearchOptions, result: &serde_json::Value) -> Result<()> {
    let flag = |key| result.get(key).and_then(serde_json::Value::as_bool);
    require_server_support(opts, flag("hidden_complete"), flag("file_filters"))
}

/// Run one search path's worth of work, writing through the caller's `writer`.
///
/// The writer is owned by the caller and shared across every path argument:
/// ripgrep emits one JSON `summary` for the whole invocation, so stats have to
/// accumulate across paths and the summary is emitted by
/// [`OutputWriter::finish`] once the last path is done.
/// `ci` is [`SearchOptions::effective_case_insensitive`], decided once by the
/// caller for every path argument: smart case parses every pattern.
pub fn run(
    root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
    writer: &mut OutputWriter,
) -> Result<bool> {
    writer.reset_match_totals();
    let root = match std::fs::canonicalize(root) {
        Ok(root) => root,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let index_dir = index_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| builder::default_index_dir(&root));

    if let Some(rev) = &opts.at_rev {
        return crate::history_search::search_at(&root, rev, opts, ci, writer);
    }

    // A non-default `--encoding` re-decodes files into text the index never
    // saw. Worse, a file the indexer classified as binary (BOM-less UTF-16 is
    // nothing but NUL-interleaved bytes) is absent from the index entirely, so
    // even a full-scan plan cannot reach it. The only way `-E` means the same
    // thing with and without an index is to walk the tree. `-a`/`--binary` are
    // the same story, as is explicitly disabling ignore rules. So is `-L`: the
    // index holds no file reached through a link, and reads by indexed name
    // refuse links rather than follow them.
    // Globs and --hidden only filter the indexed corpus. Use --no-index for
    // positive glob overrides that reinclude ignored files.
    //
    // A single named file is bypassed too. The index deliberately omits binary
    // and ignored files, but naming one explicitly is exactly how ripgrep asks
    // for it — and reading one file directly is cheaper than loading an index.
    let bypass_index = root.is_file()
        || opts.encoding.may_differ_from_index()
        || opts.text
        || opts.binary
        || opts.follow
        || opts.no_ignore
        || opts.no_ignore_dot
        || opts.no_ignore_exclude
        || opts.no_ignore_global
        || opts.no_ignore_parent
        || opts.no_ignore_vcs;

    // Try to delegate to a running server (skip for files_without_match
    // since the server only returns matching files). `--include-zero` needs
    // the same bypass: a zero-count row can only come from a file the server
    // never reports.
    // Nor for `--changed-since`, whose path set the server has no field for;
    // the local index answers a search over a few changed files quickly.
    if !opts.no_index
        && !bypass_index
        && opts.only_paths.is_none()
        && !opts.effective_passthru()
        && !opts.files_without_match
        && !opts.include_zero
        && opts.has_patterns()
        && let Ok(info) = ServerInfo::load(&index_dir)
        && let Some((index_root, scope)) = resolve_scope(&index_dir, &root)
    {
        match search_via_server(&info, &root, &index_root, &scope, opts, ci, writer) {
            Ok(had_matches) => return Ok(had_matches),
            Err(error) if error.is::<ServerOverloaded>() || is_broken_pipe(&error) => {
                return Err(error);
            }
            Err(_) => {}
        }
        eprintln!("Server unavailable or index coverage not ready, falling back to local index");
    }

    // No server — use on-disk index directly (or brute force)
    if opts.no_index || bypass_index {
        return brute_force_search(&root, &index_dir, opts, ci, writer);
    }
    if !index_dir.join("lookup.bin").exists() {
        warn_missing_index(&index_dir, index_path.is_some(), opts.quiet);
        return brute_force_search(&root, &index_dir, opts, ci, writer);
    }

    search_local_index(&root, &index_dir, opts, ci, writer)
}

/// Render a path the way the user typed it.
///
/// `std::fs::canonicalize` returns Windows extended-length paths (`\\?\C:\...`,
/// or `\\?\UNC\server\share` for network paths). That prefix is an internal
/// Win32 detail and only confuses people when it shows up in a diagnostic.
fn display_path(p: &Path) -> String {
    strip_verbatim_prefix(&p.display().to_string())
}

/// Drop the Windows extended-length (`\\?\`) prefix from an already-rendered
/// path.
///
/// Kept separate from [`display_path`] because match output also needs it, and
/// there the string has already been through `strip_prefix`/`to_string_lossy`.
fn strip_verbatim_prefix(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

/// Warn when a search silently degrades to scanning every file.
///
/// This is the difference between a sub-second query and a multi-minute one on
/// a large tree, so it must never be silent. The most common cause is starting
/// `serve` with `--index-path` but omitting it on the search: the flag is
/// global, meaning it is accepted on every subcommand, not remembered between
/// invocations. The client then looks for `serve.json` in the default location,
/// finds nothing, and falls through to here without ever contacting the server.
fn warn_missing_index(index_dir: &Path, explicit_path: bool, quiet: bool) {
    if quiet {
        return;
    }
    let dir = display_path(index_dir);
    eprintln!("warning: no index at {dir} - scanning every file (slow on large trees)");
    if explicit_path {
        eprintln!("note: build one with `ogrep index --index-path {dir}`");
    } else {
        eprintln!(
            "note: if a server is running with `--index-path <dir>`, pass the same \
             --index-path here; otherwise build an index with `ogrep index`"
        );
    }
}

fn search_via_server(
    info: &ServerInfo,
    root: &Path,
    index_root: &Path,
    scope: &IndexScope,
    opts: &SearchOptions,
    ci: bool,
    writer: &mut OutputWriter,
) -> Result<bool> {
    // `--sort` reorders the reply, so the server cannot know which rows come
    // first; the writer still applies the limit.
    let limit = opts
        .max_results
        .filter(|_| opts.sort.is_none() && !opts.quiet);
    match search_via_server_limited(info, root, index_root, scope, opts, ci, writer, limit) {
        Err(error) if error.is::<LimitShort>() => {
            search_via_server_limited(info, root, index_root, scope, opts, ci, writer, None)
        }
        result => result,
    }
}

#[allow(clippy::too_many_arguments)]
fn search_via_server_limited(
    info: &ServerInfo,
    root: &Path,
    index_root: &Path,
    scope: &IndexScope,
    opts: &SearchOptions,
    ci: bool,
    writer: &mut OutputWriter,
    limit: Option<u64>,
) -> Result<bool> {
    let mut stream = info.connect(None)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(300)))?;

    // Send every pattern, `-f`'s included (read once by the caller), rather
    // than a path: the server has a different working directory and no file
    // parameter, so forwarding only `pattern`/`extra_patterns` would silently
    // drop them and make results depend on whether a daemon happens to run.
    // The caller keeps pattern-less searches off the server: an empty list
    // would reach it as the empty pattern, which matches every line.
    let mut patterns = opts.pattern_refs();
    let primary = patterns
        .next()
        .expect("server searches always have a pattern");
    let extras: Vec<&str> = patterns.collect();

    // Context lines are pure presentation; requesting them while counting would
    // stream rows the count path has to filter back out.
    let wants_context = !(opts.count || opts.count_matches || opts.files_only || opts.quiet);

    // Spans and columns are the bulk of a reply — a nested array for every
    // match — and most searches never read them. Asking for them only when
    // something will consume them keeps a 21k-hit query from building, sending
    // and parsing 21k arrays that are then dropped.
    let detail = opts.wants_match_detail() || opts.reshaping;
    let positions = opts.wants_position_detail();
    // A plain `-l` listing needs only paths: ask for them as one string array,
    // which is far cheaper to build, send and parse than an object per file.
    let file_list =
        opts.files_only && !opts.quiet && !opts.stats && !opts.json && opts.sort.is_none();

    let mut request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "search",
        "params": {
            "pattern": primary,
            "extra_patterns": extras,
            "case_insensitive": ci,
            "fixed_string": opts.fixed_string,
            "files_only": opts.files_only,
            "compact_files": opts.files_only,
            "file_list": file_list,
            "word_boundary": opts.word_boundary,
            "max_count": opts.match_options().max_count,
            "glob": opts.glob,  // sent as JSON array
            "iglob": opts.iglob,
            "glob_case_insensitive": opts.glob_case_insensitive,
            "types": opts.types,
            "types_not": opts.types_not,
            "type_add": opts.type_add,
            "type_clear": opts.type_clear,
            "invert_match": opts.invert_match,
            "only_matching": opts.effective_only_matching(),
            "after_context": if wants_context { opts.after_ctx() } else { 0 },
            "before_context": if wants_context { opts.before_ctx() } else { 0 },
            "multiline": opts.multiline,
            "multiline_dotall": opts.dotall(),
            "text": opts.text,
            // JSON output reports binary matches the same way ripgrep does —
            // the matching lines plus a `binary_offset` — so ask the server to
            // send the lines alongside the marker. An older server ignores this
            // and returns the marker alone. A capture that keeps them asks too.
            "binary_lines": opts.json || opts.reshaping,
            "max_filesize": opts.max_filesize,
            "encoding": opts.encoding.label(),
            "line_regexp": opts.line_regexp,
            "no_unicode": opts.no_unicode,
            "engine": opts.engine.label(),
            "regex_size_limit": opts.regex_size_limit,
            "dfa_size_limit": opts.dfa_size_limit,
            "replace": opts.replace,
            "passthru": opts.match_options().passthru,
            "stop_on_nonmatch": opts.stop_on_nonmatch,
            "vimgrep": opts.vimgrep,
            "detail": detail,
            "positions": positions,
            "stats": opts.stats,
        },
        "id": 1,
    });
    if let Some(limit) = limit {
        // Stop once this many results are found, counting files for -l/-c.
        request["params"]["limit"] = limit.into();
        request["params"]["limit_files"] =
            (opts.files_only || opts.count || opts.count_matches).into();
    }
    // Every match (and its column), by the local search's rule for
    // `MatchOptions::all_spans`, or only each line's first for a capture
    // that needs no more (see `SearchOptions::reshaping`).
    request["params"]["all_spans"] = opts.wants_match_detail().into();
    request["params"]["hidden"] = serde_json::json!(opts.hidden);
    request["params"]["with"] = serde_json::json!(opts.with_patterns);
    request["params"]["without"] = serde_json::json!(opts.without_patterns);
    request["params"]["scope"] = serde_json::json!(scope.prefix());
    request["params"]["max_depth"] = serde_json::json!(opts.max_depth);
    crate::serve::write_line(&mut stream, request.to_string())?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    if file_list
        && let Some(had_matches) = write_file_list_reply(&line, root, scope, opts, writer, limit)?
    {
        writer.flush()?;
        return Ok(had_matches);
    }

    let response: serde_json::Value = serde_json::from_str(&line)?;

    if let Some(error) = response.get("error") {
        return Err(server_error(error));
    }

    let result = response
        .get("result")
        .ok_or_else(|| anyhow::anyhow!("no result in response"))?;
    // Validate before writing any rows, so an old or partial server can never
    // silently answer --hidden or --with/--without.
    require_server_support_in(opts, result)?;

    let empty_vec = Vec::new();
    let matches = result
        .get("matches")
        .and_then(|m| m.as_array())
        .unwrap_or(&empty_vec);

    // The server scopes candidates before searching but still reports paths
    // relative to the index root. Translate those paths for output and apply
    // the same scope defensively to all reply rows.
    //
    // ripgrep only surfaces a binary file when the user named it explicitly, so
    // `binary` rows for anything reached by traversal are dropped here — along
    // with the match rows a `--json` client asked to be sent with them, which
    // would otherwise leak the file's raw contents.
    let drop_binary = !opts.binary
        && !matches!(scope, IndexScope::File(_))
        && matches
            .iter()
            .any(|m| m.get("type").and_then(|t| t.as_str()) == Some("binary"));
    let dropped_binary_files: std::collections::HashSet<&str> = if drop_binary {
        matches
            .iter()
            .filter(|m| m.get("type").and_then(|t| t.as_str()) == Some("binary"))
            .filter_map(|m| m.get("file").and_then(|f| f.as_str()))
            .collect()
    } else {
        std::collections::HashSet::new()
    };
    let scoped;
    let matches = if matches!(scope, IndexScope::Whole) && opts.max_depth.is_none() && !drop_binary
    {
        matches
    } else {
        scoped = matches
            .iter()
            .filter_map(|m| {
                let file = m.get("file")?.as_str()?;
                if dropped_binary_files.contains(file) {
                    return None;
                }
                let rel = scope.relativize(file, root)?;
                within_max_depth(&rel, opts).then_some(())?;
                let mut m = m.clone();
                m["file"] = serde_json::Value::String(rel);
                Some(m)
            })
            .collect::<Vec<_>>();
        &scoped
    };

    // `--sort` reorders whole files. Ranking distinct files once (one metadata
    // read each) and sorting rows by rank keeps every file's rows in the
    // server's line order. The key is a `PathBuf` so the order matches the
    // brute-force walk, which compares paths component-wise.
    let sorted;
    let matches = match opts.sort {
        None => matches,
        Some(sort) => {
            let mut ranked = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for m in matches {
                let Some(rel) = m.get("file").and_then(|f| f.as_str()) else {
                    continue;
                };
                if !seen.insert(rel.to_string()) {
                    continue;
                }
                ranked.push(rel.to_string());
            }
            sort.apply_indexed(&mut ranked, index_root, scope, String::as_str);
            let rank: std::collections::HashMap<&str, usize> = ranked
                .iter()
                .enumerate()
                .map(|(i, file)| (file.as_str(), i))
                .collect();

            let mut rows = matches.clone();
            rows.sort_by_key(|m| {
                m.get("file")
                    .and_then(|f| f.as_str())
                    .and_then(|f| rank.get(f).copied())
                    .unwrap_or(usize::MAX)
            });
            sorted = rows;
            &sorted
        }
    };

    if let Some(n) = limit
        && result.get("limited").and_then(|l| l.as_bool()) == Some(true)
        && reply_results(matches, opts) < n
    {
        return Err(LimitShort.into());
    }

    let had_matches = !matches.is_empty();

    if opts.quiet {
        // No output, but still report stats for the first matching file.
    } else if opts.files_only {
        let mut seen = std::collections::HashSet::new();
        for m in matches {
            if let Some(file) = m.get("file").and_then(|f| f.as_str())
                && seen.insert(file.to_string())
            {
                writer.write_file(file)?;
            }
        }
    } else if opts.count || opts.count_matches {
        // `-c` counts matching *lines*, so collapse rows that share a line
        // (`-o` emits one per match) and ignore context rows. This is what the
        // local path reports via `FileMatches::matched_lines`.
        // `--count-matches` instead sums the spans on each row, which is what
        // `FileMatches::match_count` reports locally.
        let mut order: Vec<String> = Vec::new();
        let mut lines: std::collections::HashMap<String, std::collections::HashSet<usize>> =
            std::collections::HashMap::new();
        let mut spans: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        // A binary file is summarised as a single `binary` row rather than one
        // row per match, so it carries its own line count. The local path still
        // prints a real count for it because `-c` returns before the binary
        // guard, and dropping these rows here would silently omit the file.
        let mut exact: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for m in matches {
            let mtype = m.get("type").and_then(|t| t.as_str()).unwrap_or("match");
            if mtype != "match" && mtype != "binary" {
                continue;
            }
            let Some(file) = m.get("file").and_then(|f| f.as_str()) else {
                continue;
            };
            if !lines.contains_key(file) && !exact.contains_key(file) {
                order.push(file.to_string());
            }
            if mtype == "binary" {
                let n = m.get("lines").and_then(|l| l.as_u64()).unwrap_or(0) as usize;
                exact.insert(file.to_string(), n);
            } else {
                let line = m.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as usize;
                lines.entry(file.to_string()).or_default().insert(line);
                // A row with no spans is still one match (`-v` reports inverted
                // lines with an empty span list).
                let n = m
                    .get("spans")
                    .and_then(|s| s.as_array())
                    .map(|s| s.len().max(1))
                    .unwrap_or(1);
                *spans.entry(file.to_string()).or_default() += n;
            }
        }
        // Emit in response order; iterating the maps would randomise the file
        // order on every run, unlike the local path which prints in walk order.
        for file in &order {
            let n = exact
                .get(file)
                .copied()
                .or_else(|| {
                    if opts.count_matches {
                        spans.get(file).copied()
                    } else {
                        lines.get(file).map(|l| l.len())
                    }
                })
                .unwrap_or(0);
            writer.write_count(file, n)?;
        }
    } else {
        // A binary file's rows follow its marker; they are written only
        // where the marker says a plain search would write them.
        let mut suppressed: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for m in matches {
            let file = m.get("file").and_then(|f| f.as_str()).unwrap_or("");
            if suppressed.contains(file) {
                continue;
            }
            let line = m.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as usize;
            let content = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let mtype = m.get("type").and_then(|t| t.as_str()).unwrap_or("match");
            let offset = m.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
            // `-M/--max-columns` counts the line terminator, which only the
            // server knows; an older server omits it and simply measures the
            // visible text.
            let terminator_len = m.get("term").and_then(|t| t.as_u64()).unwrap_or(0) as usize;
            if mtype == "context" {
                writer.write_context_separator(file, line)?;
                writer.write_context_line(&ContextLine {
                    file: file.to_string(),
                    line_number: line,
                    content: content.to_string(),
                    absolute_offset: offset,
                    terminator_len,
                })?;
            } else if mtype == "binary" {
                if !writer.write_binary_note(file, offset)? {
                    suppressed.insert(file);
                }
            } else {
                let spans = m
                    .get("spans")
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| {
                                let p = p.as_array()?;
                                Some((p.first()?.as_u64()? as usize, p.get(1)?.as_u64()? as usize))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                writer.write_context_separator(file, line)?;
                writer.write_match(&Match {
                    file: file.to_string(),
                    line_number: line,
                    content: content.to_string(),
                    // Absent when a reshaping capture asked for first spans
                    // only; it prints no columns.
                    columns: m
                        .get("columns")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.as_u64().map(|c| c as usize))
                                .collect()
                        })
                        .unwrap_or_default(),
                    spans,
                    absolute_offset: offset,
                    terminator_len,
                })?;
            }
        }
    }

    if opts.stats
        && let Some(elapsed) = result.get("elapsed_ms").and_then(|e| e.as_f64())
    {
        // Match output is buffered on stdout, while stats go straight to
        // stderr. Flush first so a combined stdout/stderr stream reports the
        // summary after the matches, as ripgrep does.
        flush_before_stats(writer)?;
        // Apply the same scope to per-file totals as to output rows. The
        // server's `num_matches` includes context rows, and
        // rendered spans can split or merge the original matches.
        let (num, lines) = if let Some(stats) = result.get("file_stats") {
            let stats: Vec<FileMatchStats> = serde_json::from_value(stats.clone())?;
            let first_file = matches
                .first()
                .and_then(|m| m.get("file"))
                .and_then(|f| f.as_str());
            stats
                .iter()
                .filter_map(|s| {
                    if dropped_binary_files.contains(s.file.as_str()) {
                        return None;
                    }
                    let rel = scope.relativize(&s.file, root)?;
                    if !within_max_depth(&rel, opts)
                        || (opts.quiet && first_file != Some(rel.as_str()))
                    {
                        return None;
                    }
                    Some((s.matches, s.matched_lines))
                })
                .fold((0, 0), |(m, l), (fm, fl)| (m + fm, l + fl))
        } else {
            // Older servers only provide rendered rows.
            count_reported_matches(matches)
        };
        let mut narrowing_note = "";
        if let Some(stats) = result.get("index_stats")
            && let (Some(plan), Some(raw), Some(candidates), Some(total)) = (
                stats.get("query_plan").and_then(|v| v.as_str()),
                stats.get("raw_candidates").and_then(|v| v.as_u64()),
                stats.get("candidates").and_then(|v| v.as_u64()),
                stats.get("total_files").and_then(|v| v.as_u64()),
            )
        {
            print_index_stats(plan, raw, candidates, total);
            if let Some(work) = stats.get("work") {
                eprintln!("Index work: {work}");
            }
            narrowing_note = index_narrowing_note(raw, total);
        }
        eprintln!(
            "{num} matches ({lines} matched lines) in {elapsed:.1}ms (via server){narrowing_note}"
        );
    }

    writer.flush()?;
    Ok(had_matches)
}

#[derive(serde::Deserialize)]
struct FileListEnvelope {
    result: Option<FileListResult>,
    error: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct FileListResult {
    files: Option<Vec<String>>,
    #[serde(default)]
    matches: Vec<serde_json::Value>,
    hidden_complete: Option<bool>,
    file_filters: Option<bool>,
    /// The server stopped at the request's `limit`.
    #[serde(default)]
    limited: bool,
}

/// How many results the writer will count in these reply rows (see
/// `--max-results`): files for -l/-c, else matching lines, where a binary
/// file's note stands for its lines outside JSON.
fn reply_results(rows: &[serde_json::Value], opts: &SearchOptions) -> u64 {
    use std::collections::HashSet;
    fn field<'a>(m: &'a serde_json::Value, key: &str) -> Option<&'a str> {
        m.get(key).and_then(|v| v.as_str())
    }
    let files_of = |kind: &[&str]| -> HashSet<&str> {
        rows.iter()
            .filter(|m| field(m, "type").is_some_and(|t| kind.contains(&t)))
            .filter_map(|m| field(m, "file"))
            .collect()
    };
    if opts.files_only || opts.count || opts.count_matches {
        return files_of(&["match", "binary"]).len() as u64;
    }
    let binary = files_of(&["binary"]);
    let lines = rows
        .iter()
        .filter(|m| field(m, "type") == Some("match"))
        .filter(|m| opts.json || field(m, "file").is_none_or(|f| !binary.contains(f)))
        .count();
    let notes = if opts.json { 0 } else { binary.len() };
    (lines + notes) as u64
}

/// Print a `file_list` reply, or `None` when the server predates it and sent
/// ordinary rows instead.
///
/// The reply lists every matching file in candidate order, binary ones
/// included, with `matches` carrying only the binary markers, so the same
/// visibility rule as the row path applies: traversal hides binary files.
fn write_file_list_reply(
    line: &str,
    root: &Path,
    scope: &IndexScope,
    opts: &SearchOptions,
    writer: &mut OutputWriter,
    limit: Option<u64>,
) -> Result<Option<bool>> {
    let envelope: FileListEnvelope = serde_json::from_str(line)?;
    if let Some(error) = envelope.error {
        return Err(server_error(&error));
    }
    let result = envelope
        .result
        .ok_or_else(|| anyhow::anyhow!("no result in response"))?;
    let Some(files) = result.files else {
        return Ok(None);
    };
    require_server_support(opts, result.hidden_complete, result.file_filters)?;
    let hidden_binary: std::collections::HashSet<&str> =
        if opts.binary || matches!(scope, IndexScope::File(_)) {
            Default::default()
        } else {
            result
                .matches
                .iter()
                .filter(|m| m.get("type").and_then(|t| t.as_str()) == Some("binary"))
                .filter_map(|m| m.get("file").and_then(|f| f.as_str()))
                .collect()
        };
    let whole = matches!(scope, IndexScope::Whole) && opts.max_depth.is_none();
    let shown: Vec<std::borrow::Cow<str>> = files
        .iter()
        .filter(|file| !hidden_binary.contains(file.as_str()))
        .filter_map(|file| {
            if whole {
                return Some(std::borrow::Cow::Borrowed(file.as_str()));
            }
            let rel = scope.relativize(file, root)?;
            within_max_depth(&rel, opts).then_some(std::borrow::Cow::Owned(rel))
        })
        .collect();
    if result.limited && limit.is_some_and(|n| (shown.len() as u64) < n) {
        return Err(LimitShort.into());
    }
    for file in &shown {
        writer.write_file(file)?;
    }
    Ok(Some(!shown.is_empty()))
}

fn search_local_index(
    root: &Path,
    index_dir: &Path,
    opts: &SearchOptions,
    ci: bool,
    writer: &mut OutputWriter,
) -> Result<bool> {
    let start = Instant::now();
    let meta = match IndexMeta::load(index_dir) {
        Ok(meta) => meta,
        Err(error) => {
            if !opts.quiet && !opts.no_messages {
                eprintln!(
                    "warning: index coverage metadata unavailable ({error}) - scanning filesystem"
                );
            }
            return brute_force_search(root, index_dir, opts, ci, writer);
        }
    };
    if !meta.complete
        || !meta.hidden_complete
        || meta.version != ogrep_core::meta::INDEX_FORMAT_VERSION
    {
        if !opts.quiet && !opts.no_messages {
            eprintln!(
                "warning: index lacks complete hidden-file coverage - scanning filesystem; rebuild with `ogrep index` or upgrade with `ogrep serve`"
            );
        }
        return brute_force_search(root, index_dir, opts, ci, writer);
    }
    let reader = IndexReader::open(index_dir)?;
    let filename_visibility = match ogrep_core::path_index::read_filename_index(index_dir) {
        Ok(Some(index)) => index.visibility,
        Ok(None) | Err(_) => None,
    };
    let Some(filename_visibility) = filename_visibility else {
        if !opts.quiet && !opts.no_messages {
            eprintln!("warning: atomic index visibility unavailable - scanning filesystem");
        }
        return brute_force_search(root, index_dir, opts, ci, writer);
    };
    if !filename_visibility.covers_index(&meta, reader.file_table_id()) {
        if !opts.quiet && !opts.no_messages {
            eprintln!(
                "warning: index visibility, coverage, and path table differ - scanning filesystem"
            );
        }
        return brute_force_search(root, index_dir, opts, ci, writer);
    }
    let visibility = filename_visibility.paths;

    // Index paths are relative to the root the index was built for, which is
    // not necessarily the directory being searched. Without translating between
    // the two, `ogrep --index-path IDX foo src` looks for `src/src/lib.rs` and
    // silently reports nothing.
    let Some((index_root, scope)) = resolve_scope(index_dir, root) else {
        // The index covers an unrelated tree, so it cannot answer this search.
        return brute_force_search(root, index_dir, opts, ci, writer);
    };

    let _profile = crate::profile::Session::start(start);
    let compile_span = crate::profile::span(crate::profile::Phase::Compile);
    let glob_filter = opts.glob_filter()?;
    let type_filter = opts.type_filter()?;

    let matcher = opts.matcher(ci)?;
    let filter = opts.file_filter(ci)?;

    // A non-default `--encoding` re-decodes files into text the index never
    // saw, so no trigram may narrow it; the caller scans those directly.
    debug_assert!(!opts.encoding.may_differ_from_index());

    // Narrow candidates using every pattern, not just the primary one.
    // `--passthru` must visit files without any pattern trigrams, so it cannot
    // use the trigram plan.
    //
    // A PCRE-style pattern is not parseable by `regex-syntax`, but relaxing it
    // (dropping lookarounds and the like) yields one that is, and that matches a
    // superset — so its trigrams remain mandatory for the original.
    //
    // `-v` inverts at the *line* level: a file matches when some line does not.
    // The pattern's trigrams select files that contain it, which is exactly the
    // wrong filter. Same for `--files-without-match` and `--include-zero`, which
    // both have to report files the pattern excluded. `--with` still narrows.
    let plan = if opts.effective_passthru()
        || opts.files_without_match
        || opts.invert_match
        || opts.include_zero
    {
        QueryPlan::MatchAll
    } else {
        matcher.index_plan(&opts.all_patterns(), opts.fixed_string, ci)?
    };
    // A file failing `--with` produces no output in any mode, so every `--with`
    // pattern's trigrams narrow the candidates too.
    let plan = query::require_patterns(plan, &opts.with_patterns, opts.fixed_string, ci);
    drop(compile_span);

    let mut query_work = query::QueryWork::default();
    let postings_span = crate::profile::span(crate::profile::Phase::Postings);
    let candidate_ids = if plan.is_match_all() {
        reader.all_file_ids()
    } else {
        query::execute_plan_indexed(&plan, &reader, &mut query_work)
    };
    drop(postings_span);

    // Drop everything outside the searched subtree and re-express the survivors
    // relative to it, so globs, `--max-depth` and printed paths all agree with
    // what the brute-force walk of the same directory would produce.
    let filter_span = crate::profile::span(crate::profile::Phase::Filter);
    let mut candidates: Vec<(u32, String)> = candidate_ids
        .iter()
        .filter_map(|&fid| {
            let indexed = reader.file_path(fid)?;
            visibility
                .is_visible_with_file_whitelist(
                    indexed,
                    scope.prefix(),
                    opts.hidden,
                    type_filter.has_selection(),
                )
                .then_some(())?;
            let rel = scope.relativize(indexed, root)?;
            within_max_depth(&rel, opts).then_some(())?;
            passes_filters(&rel, &glob_filter, &type_filter).then_some(())?;
            Some((fid, rel))
        })
        .collect();

    if let Some(sort) = opts.sort {
        sort.apply_indexed(&mut candidates, &index_root, &scope, |(_, rel)| rel);
    }
    drop(filter_span);

    let mut had_matches = false;
    // A single-file search root is a file the user named on the command line,
    // which is what makes binary files visible in ripgrep.
    let explicit = matches!(scope, IndexScope::File(_));
    // Opened once: every candidate is read relative to it.
    let rooted = ogrep_core::rooted::RootDir::open(&index_root)?;

    let threads = local_verify_threads(opts, candidates.len());
    if threads > 1 && parallel_prefilter_is_exact(opts) {
        had_matches = verify_candidates_parallel(
            &candidates,
            threads,
            &index_root,
            &rooted,
            &scope,
            &matcher,
            filter.as_ref(),
            opts,
            writer,
            explicit,
        )?;
    } else {
        for (_, rel_path) in &candidates {
            let metadata_span = crate::profile::span(crate::profile::Phase::Metadata);
            let full_path = scope.full_path(&index_root, rel_path);
            if exceeds_max_filesize(&full_path, opts, explicit) {
                continue;
            }
            drop(metadata_span);
            let read = match read_indexed_text(&rooted, &full_path, opts.encoding) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if search_file(
                &read,
                &matcher,
                filter.as_ref(),
                rel_path,
                opts,
                writer,
                explicit,
            )? {
                had_matches = true;
                if opts.quiet || writer.is_full() {
                    break;
                }
            }
        }
    }

    if opts.stats {
        let elapsed = start.elapsed();
        // Match output is buffered on stdout, while stats go straight to
        // stderr. Flush first so a combined stdout/stderr stream reports the
        // summary after the matches, as ripgrep does.
        flush_before_stats(writer)?;
        let raw = candidate_ids.len() as u64;
        let total = reader.num_files() as u64;
        print_index_stats(&plan_summary(&plan), raw, candidates.len() as u64, total);
        eprintln!("Index work: {}", serde_json::to_string(&query_work)?);
        let narrowing_note = index_narrowing_note(raw, total);
        let (matches, matched_lines) = writer.match_totals();
        eprintln!(
            "Search completed in {:.1}ms: {matches} matches ({matched_lines} matched lines){narrowing_note}",
            elapsed.as_secs_f64() * 1000.0
        );
    }

    let _flush_span = crate::profile::span(crate::profile::Phase::Flush);
    writer.flush()?;
    Ok(had_matches)
}

/// Candidates below this many are verified on the calling thread: building a
/// pool and handing work to it costs more than a few small files do.
const PARALLEL_VERIFY_MIN_CANDIDATES: usize = 48;

/// Candidates handed to the pool at once. Output is rendered in candidate order
/// after each batch, so this bounds how many files' contents are held at once.
const PARALLEL_VERIFY_BATCH: usize = 512;

fn local_verify_threads(opts: &SearchOptions, candidates: usize) -> usize {
    if candidates < PARALLEL_VERIFY_MIN_CANDIDATES {
        return 1;
    }
    let available = std::thread::available_parallelism().map_or(1, |n| n.get());
    opts.threads.unwrap_or(available).clamp(1, available.max(1))
}

/// Whether a file with no matching line is certain to produce no output and no
/// state change, so worker threads may drop it before the ordered render.
///
/// Modes that report non-matching files (`--files-without-match`,
/// `--include-zero`), print them anyway (`--passthru`), invert the line test,
/// or account for every searched byte (`--stats`) keep the sequential path.
fn parallel_prefilter_is_exact(opts: &SearchOptions) -> bool {
    !opts.files_without_match
        && !opts.include_zero
        && !opts.invert_match
        && !opts.effective_passthru()
        && !opts.stats
        && opts.max_count != Some(0)
}

/// What a worker learned about one candidate.
enum Verified {
    /// Never searched: unreadable, over the size cap, a binary file traversal
    /// hides, or failing `--with`/`--without`.
    Skipped,
    /// Searched these many bytes without a selected line.
    NoMatch(u64),
    /// `-l`/`-q`: a line matched in these many searched bytes, which is all
    /// the output needs.
    Matched(u64),
    /// Some line matched; render the file in order on the calling thread.
    Render(FileRead),
}

/// Read and test candidates on a pool, then render survivors in candidate
/// order, so output is identical to the sequential loop.
#[allow(clippy::too_many_arguments)]
fn verify_candidates_parallel(
    candidates: &[(u32, String)],
    threads: usize,
    index_root: &Path,
    rooted: &ogrep_core::rooted::RootDir,
    scope: &IndexScope,
    matcher: &SearchMatcher,
    filter: Option<&crate::matching::FileFilter>,
    opts: &SearchOptions,
    writer: &mut OutputWriter,
    explicit: bool,
) -> Result<bool> {
    use rayon::prelude::*;

    // One pool per invocation, reused for every path argument; `threads` comes
    // from `-j` and does not change between them.
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    let pool = match POOL.get() {
        Some(pool) => pool,
        None => {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .map_err(|e| anyhow::anyhow!("failed to start search threads: {e}"))?;
            POOL.get_or_init(|| pool)
        }
    };
    let decides = opts.quiet || opts.stops_at_first_line();
    // `-q` needs a single match: once any worker finds one, the others stop
    // reading, as the sequential loop stops at its first matching file.
    let quiet_done = std::sync::atomic::AtomicBool::new(false);
    let mut probe = opts.match_options();
    probe.max_count = Some(1);
    probe.all_spans = false;
    probe.before_context = 0;
    probe.after_context = 0;
    let verify = |rel_path: &String| -> Result<Verified> {
        if opts.quiet && quiet_done.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Verified::Skipped);
        }
        let full_path = scope.full_path(index_root, rel_path);
        if exceeds_max_filesize(&full_path, opts, explicit) {
            return Ok(Verified::Skipped);
        }
        let Ok(read) = read_indexed_text(rooted, &full_path, opts.encoding) else {
            return Ok(Verified::Skipped);
        };
        let binary_offset = read.binary_offset(opts);
        if hides_binary(binary_offset, explicit, opts) {
            return Ok(Verified::Skipped);
        }
        let Some(found) = crate::matching::FileMatches::find_filtered(
            read.text.as_str(),
            filter,
            matcher,
            &probe,
        )?
        else {
            return Ok(Verified::Skipped);
        };
        let searched = read.bytes_searched(binary_offset);
        Ok(if found.is_empty() {
            Verified::NoMatch(searched)
        } else if decides {
            if opts.quiet {
                quiet_done.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Verified::Matched(searched)
        } else {
            drop(found);
            Verified::Render(read)
        })
    };

    let mut had_matches = false;
    // Under --max-results the first files usually suffice: start with small
    // batches so the pool reads few files past the last one printed.
    let mut size = if opts.max_results.is_some() {
        64
    } else {
        PARALLEL_VERIFY_BATCH
    };
    let mut rest = candidates;
    while !rest.is_empty() {
        let (batch, tail) = rest.split_at(size.min(rest.len()));
        rest = tail;
        size = (size * 2).min(PARALLEL_VERIFY_BATCH);
        let results: Vec<Result<Verified>> =
            pool.install(|| batch.par_iter().map(|(_, rel)| verify(rel)).collect());
        for ((_, rel_path), result) in batch.iter().zip(results) {
            // Record searched bytes in candidate order, as the sequential loop
            // does, for the JSON summary.
            match result? {
                Verified::Skipped => {}
                Verified::NoMatch(searched) => writer.note_bytes_searched(searched),
                Verified::Matched(searched) => {
                    writer.note_bytes_searched(searched);
                    had_matches = true;
                    if opts.quiet {
                        return Ok(true);
                    }
                    writer.write_file(rel_path)?;
                }
                Verified::Render(read) => {
                    // The worker already applied `--with`/`--without`, and `-q`
                    // always decides in the worker, so it never renders.
                    had_matches |=
                        search_file(&read, matcher, None, rel_path, opts, writer, explicit)?;
                }
            }
            if writer.is_full() {
                return Ok(had_matches);
            }
        }
    }
    Ok(had_matches)
}

/// Which slice of an index a search root covers.
///
/// An index stores paths relative to the root it was built for. A search root
/// may be that same directory, a directory inside it, or a single file inside
/// it; each needs a different translation to and from index-relative paths.
enum IndexScope {
    Whole,
    Subtree(String),
    File(String),
}

impl IndexScope {
    fn prefix(&self) -> &str {
        match self {
            Self::Subtree(prefix) => prefix,
            Self::Whole | Self::File(_) => "",
        }
    }

    /// `None` when `search_root` lies outside the indexed tree, which means the
    /// index cannot answer the search at all.
    fn resolve(index_root: &Path, search_root: &Path) -> Option<Self> {
        let rel = search_root.strip_prefix(index_root).ok()?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            Some(Self::Whole)
        } else if search_root.is_file() {
            Some(Self::File(rel))
        } else {
            Some(Self::Subtree(format!("{rel}/")))
        }
    }

    /// Re-express an index-relative path relative to the search root, or `None`
    /// when the file is outside it.
    fn relativize(&self, indexed: &str, search_root: &Path) -> Option<String> {
        match self {
            Self::Whole => Some(indexed.to_string()),
            Self::Subtree(prefix) => indexed.strip_prefix(prefix).map(str::to_string),
            // A named file prints as the user reached it, exactly as the
            // brute-force path does.
            Self::File(f) => (f == indexed).then(|| explicit_file_display_path(search_root)),
        }
    }

    fn full_path(&self, index_root: &Path, rel: &str) -> PathBuf {
        use ogrep_core::hybrid::join_indexed_path;
        match self {
            Self::Whole => join_indexed_path(index_root, rel),
            Self::Subtree(prefix) => join_indexed_path(index_root, &format!("{prefix}{rel}")),
            // `rel` is the path as the user typed it, so go back to the one the
            // index stores.
            Self::File(f) => join_indexed_path(index_root, f),
        }
    }
}

/// The slice of the index at `index_dir` that covers `root`, with the absolute
/// root the index was built for.
fn resolve_scope(index_dir: &Path, root: &Path) -> Option<(PathBuf, IndexScope)> {
    let index_root = IndexMeta::load(index_dir)
        .ok()
        .and_then(|m| std::fs::canonicalize(m.root_path).ok())
        .unwrap_or_else(|| root.to_path_buf());
    let scope = IndexScope::resolve(&index_root, root)?;
    Some((index_root, scope))
}

/// `true` when the path is shallow enough to keep under `--max-depth`.
///
/// The walker applies this while descending; the indexed paths never went
/// through it, so it has to be re-applied by counting components.
pub(crate) fn within_max_depth(rel: &str, opts: &SearchOptions) -> bool {
    match opts.max_depth {
        // `rel` is search-root-relative, so its separator count is the number
        // of directory levels below the root; a depth of 1 means "directly in
        // the root", matching ripgrep's `--max-depth`.
        Some(max) => rel.matches('/').count() < max,
        None => true,
    }
}

fn index_storage_exclusions(index_dir: &Path) -> Result<Vec<PathBuf>> {
    match std::fs::canonicalize(index_dir) {
        Ok(path) => Ok(vec![path]),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        // Not a file being searched: the whole search stops, so it must not
        // pass for a per-file error that --no-messages hides.
        Err(error) => Err(anyhow::anyhow!(
            "cannot resolve index path {}: {error}",
            index_dir.display()
        )),
    }
}

fn brute_force_search(
    root: &Path,
    index_dir: &Path,
    opts: &SearchOptions,
    ci: bool,
    writer: &mut OutputWriter,
) -> Result<bool> {
    let start = Instant::now();
    let glob_filter = opts.glob_filter()?;
    let type_filter = opts.type_filter()?;

    let matcher = opts.matcher(ci)?;
    let filter = opts.file_filter(ci)?;

    let mut had_matches = false;

    if root.is_file() {
        let rel_path = explicit_file_display_path(root);
        if passes_filters(&rel_path, &glob_filter, &type_filter)
            && !exceeds_max_filesize(root, opts, true)
        {
            let read = read_text_lossy(root, opts.encoding)?;
            had_matches = search_file(
                &read,
                &matcher,
                filter.as_ref(),
                &rel_path,
                opts,
                writer,
                true,
            )?;
        }

        if opts.stats {
            let elapsed = start.elapsed();
            // Match output is buffered on stdout, while stats go straight to
            // stderr. Flush first so a combined stdout/stderr stream reports
            // the summary after the matches, as ripgrep does.
            flush_before_stats(writer)?;
            let (matches, matched_lines) = writer.match_totals();
            eprintln!(
                "Brute-force search completed in {:.1}ms (1 file): {matches} matches ({matched_lines} matched lines)",
                elapsed.as_secs_f64() * 1000.0,
            );
        }

        writer.flush()?;
        return Ok(had_matches);
    }

    let mut walk = walker::walk_dir(
        root,
        &walker::WalkOptions {
            exclude_paths: index_storage_exclusions(index_dir)?,
            overrides: Some(glob_filter.walk_overrides(root)?),
            types: type_filter.walk_types()?,
            ..opts.walk_options()
        },
    );
    if let Some(sort) = opts.sort {
        sort.apply(&mut walk.files);
    }

    for path in &walk.files {
        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        if !passes_filters(&rel_path, &glob_filter, &type_filter) {
            continue;
        }

        let read = match read_text_lossy(path, opts.encoding) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if search_file(
            &read,
            &matcher,
            filter.as_ref(),
            &rel_path,
            opts,
            writer,
            false,
        )? {
            had_matches = true;
            if opts.quiet || writer.is_full() {
                break;
            }
        }
    }

    if opts.stats {
        let elapsed = start.elapsed();
        // Match output is buffered on stdout, while stats go straight to
        // stderr. Flush first so a combined stdout/stderr stream reports the
        // summary after the matches, as ripgrep does.
        flush_before_stats(writer)?;
        let (matches, matched_lines) = writer.match_totals();
        eprintln!(
            "Brute-force search completed in {:.1}ms ({} files): {matches} matches ({matched_lines} matched lines)",
            elapsed.as_secs_f64() * 1000.0,
            walk.files.len()
        );
    }

    writer.flush()?;
    Ok(had_matches)
}

/// Smallest file worth memory-mapping instead of reading.
///
/// Mapping costs a syscall pair and a page-table setup per file, which is a bad
/// trade for the small files that dominate a repository walk — a kernel tree is
/// ~94k files averaging well under 64 KB. Above this size the whole-file read is
/// the dominant cost and mapping wins outright, so the threshold buys the
/// large-file case without taxing the common one.
///
/// 1 MiB is far above the size of real source — only 110 of the kernel's 94,747
/// files reach it — so ordinary searches never map, while the large generated
/// files that actually cost memory always do.
const MMAP_MIN_BYTES: u64 = 1024 * 1024;

/// The text of a file to search, either owned on the heap or borrowed from a
/// memory map.
///
/// Reading a file with `fs::read` costs its full size in heap, per file in
/// flight. ripgrep searches a 192 MB file in under 10 MiB because its memory is
/// decoupled from file size; mapping large files gives the same property here,
/// since mapped pages are file-backed and the OS can evict them under pressure
/// instead of the process holding an allocation it cannot give back.
enum FileText {
    Owned(String),
    /// Mapped bytes verified as UTF-8 when the map was created.
    Mapped(memmap2::Mmap),
}

impl FileText {
    fn as_str(&self) -> &str {
        match self {
            Self::Owned(text) => text,
            // SAFETY: `Mapped` is only constructed by `try_map_text`, which
            // validates the entire mapping with `str::from_utf8` first, so the
            // bytes were UTF-8 when the map was taken. A concurrent writer
            // could still invalidate them, which is the same exposure ripgrep
            // accepts when it maps a file it is searching.
            Self::Mapped(map) => unsafe { std::str::from_utf8_unchecked(map) },
        }
    }
}

/// A file's text together with what the read already learned about it.
struct FileRead {
    text: FileText,
    fixups: ogrep_core::encoding::LossyFixups,
    /// Offset of the first NUL byte in `text`, which is what makes the file
    /// binary. Found during the pass that decoded or validated the bytes, not
    /// in a pass of its own — see [`validate_utf8_and_find_nul`].
    first_nul: Option<usize>,
}

impl FileRead {
    fn binary_offset(&self, opts: &SearchOptions) -> Option<usize> {
        binary_source_offset(self.first_nul, &self.fixups, opts.text)
    }

    /// Bytes the summary counts as searched. A binary file reports the
    /// detection offset, not a regex cutoff: ripgrep with memory-mapped input
    /// can count matches beyond the NUL while reporting that offset. Both
    /// counts refer to the source on disk.
    fn bytes_searched(&self, binary_offset: Option<usize>) -> u64 {
        binary_offset.unwrap_or_else(|| self.fixups.to_source_offset(self.text.as_str().len()))
            as u64
    }
}

/// Where a file turned binary, as an offset in the file on disk, unless
/// `--text` asked to treat it as text.
///
/// The NUL is located in the repaired text but reported in terms of the file
/// on disk, so it goes back through `fixups`: repairing invalid UTF-8 ahead of
/// the NUL widens every bad byte to a three-byte U+FFFD, which would otherwise
/// push the reported offset past where the byte actually is.
pub(crate) fn binary_source_offset(
    first_nul: Option<usize>,
    fixups: &ogrep_core::encoding::LossyFixups,
    text: bool,
) -> Option<usize> {
    if text {
        None
    } else {
        first_nul.map(|off| fixups.to_source_offset(off))
    }
}

/// ripgrep only surfaces a binary file when the user named it explicitly (or
/// passed `--binary`).
fn hides_binary(binary_offset: Option<usize>, explicit: bool, opts: &SearchOptions) -> bool {
    binary_offset.is_some() && !explicit && !opts.binary
}

/// Validate `bytes` as UTF-8 and locate the first NUL byte in one traversal.
///
/// Returns `None` if the bytes are not valid UTF-8, which is the caller's
/// signal to fall back to reading and repairing them.
///
/// Both questions need every byte, and asking them separately means walking the
/// file twice. That is nearly free while it stays in the page cache and is
/// anything but on a file larger than the cache, where the second walk is
/// another trip to disk: on a 13.4 GiB file, validating cost 8.7 s and a
/// separate `memchr` for the NUL cost another 6.2 s. Interleaving them at
/// chunk granularity keeps each page hot for both.
///
/// The chunking is what makes that possible, and it is why this cannot simply
/// call `str::from_utf8` per chunk: a multi-byte sequence can straddle a chunk
/// boundary, where `from_utf8` reports an incomplete sequence. That is not an
/// error mid-file — it means "resume here" — so the scan restarts at
/// `valid_up_to` and lets the next chunk carry the sequence. At the *last*
/// chunk the same report is a real truncation and does fail.
fn validate_utf8_and_find_nul(bytes: &[u8]) -> Option<Option<usize>> {
    /// Large enough that the ≤3 carried-over bytes are noise, small enough to
    /// stay in cache between the two scans of it.
    const CHUNK: usize = 1 << 20;

    let mut pos = 0;
    let mut first_nul = None;
    while pos < bytes.len() {
        let end = (pos + CHUNK).min(bytes.len());
        let chunk = &bytes[pos..end];
        let valid_len = match std::str::from_utf8(chunk) {
            Ok(_) => chunk.len(),
            Err(e) if e.error_len().is_none() && end < bytes.len() => e.valid_up_to(),
            Err(_) => return None,
        };
        // A chunk starts on a character boundary and a UTF-8 sequence is at
        // most 4 bytes, so a chunk this size always validates at least part of
        // itself unless it opens with a genuinely invalid byte. Bailing keeps
        // the loop from spinning if that reasoning ever stops holding.
        if valid_len == 0 {
            return None;
        }
        if first_nul.is_none()
            && let Some(i) = memchr::memchr(0, &chunk[..valid_len])
        {
            first_nul = Some(pos + i);
        }
        pos += valid_len;
    }
    Some(first_nul)
}

/// Map a file for searching, or `None` to fall back to reading it.
///
/// Declines whenever the mapped pages would not be exactly the bytes to search:
/// a file below the threshold, an encoding that transcodes or strips a BOM, or
/// content that is not already valid UTF-8 and so needs lossy repair.
fn try_map_text(
    file: &std::fs::File,
    encoding: ogrep_core::encoding::EncodingMode,
) -> Option<(FileText, Option<usize>)> {
    if file.metadata().ok()?.len() < MMAP_MIN_BYTES {
        return None;
    }
    // SAFETY: the map is read-only, owned by the returned `FileText`, and
    // dropped before this function's caller finishes with the file. Mapping is
    // undefined behaviour if another process truncates the file underneath us;
    // that is inherent to searching by map and is the tradeoff ripgrep makes.
    let map = unsafe { memmap2::Mmap::map(file).ok()? };
    if !ogrep_core::encoding::borrows_whole_input(&map, encoding) {
        return None;
    }
    let first_nul = validate_utf8_and_find_nul(&map)?;
    Some((FileText::Mapped(map), first_nul))
}

/// Read a file as text, applying `--encoding` and replacing invalid UTF-8
/// rather than failing.
///
/// ripgrep searches files that are not valid UTF-8; refusing them makes
/// UTF-16, Latin-1 and mixed-encoding sources silently invisible.
fn read_text_lossy(
    path: &Path,
    encoding: ogrep_core::encoding::EncodingMode,
) -> std::io::Result<FileRead> {
    let read_span = crate::profile::span(crate::profile::Phase::Read);
    let file = std::fs::File::open(path)?;
    read_open_text(file, encoding, read_span)
}

/// [`read_text_lossy`] for a path the index recorded below `rooted`.
///
/// The name may now run through a link that was not there when the file was
/// indexed, and following it would search content outside the tree. A scan
/// never sees that content, since the walker does not follow links.
fn read_indexed_text(
    rooted: &ogrep_core::rooted::RootDir,
    path: &Path,
    encoding: ogrep_core::encoding::EncodingMode,
) -> std::io::Result<FileRead> {
    let read_span = crate::profile::span(crate::profile::Phase::Read);
    let file = rooted.open_file(path)?;
    read_open_text(file, encoding, read_span)
}

fn read_open_text(
    mut file: std::fs::File,
    encoding: ogrep_core::encoding::EncodingMode,
    read_span: crate::profile::Span,
) -> std::io::Result<FileRead> {
    // A mapped file is always already-valid UTF-8, so it needs no fixups.
    if let Some((text, first_nul)) = try_map_text(&file, encoding) {
        return Ok(FileRead {
            text,
            fixups: ogrep_core::encoding::LossyFixups::default(),
            first_nul,
        });
    }
    // Mapping does not advance the file position. Keep this handle when a
    // small file, encoding or invalid UTF-8 requires an owned buffer, instead
    // of closing it and resolving/opening the path a second time.
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    drop(read_span);
    let _decode_span = crate::profile::span(crate::profile::Phase::Decode);
    let (text, fixups) = ogrep_core::encoding::decode_owned_with_fixups(bytes, encoding);
    // The read path already walks these bytes at least once and they are under
    // the mapping threshold or repaired, so a separate scan here is cheap. It
    // has to run over the *repaired* text, since that is what offsets are
    // reported against.
    let first_nul = memchr::memchr(0, text.as_bytes());
    Ok(FileRead {
        text: FileText::Owned(text),
        fixups,
        first_nul,
    })
}

/// Search text that did not come from a file on disk (a blob at `--at REV`),
/// exactly as if it had been read from `rel_path`.
pub(crate) fn search_bytes(
    bytes: Vec<u8>,
    rel_path: &str,
    matcher: &SearchMatcher,
    filter: Option<&crate::matching::FileFilter>,
    opts: &SearchOptions,
    writer: &mut OutputWriter,
) -> Result<bool> {
    let (text, fixups) = ogrep_core::encoding::decode_owned_with_fixups(bytes, opts.encoding);
    let first_nul = memchr::memchr(0, text.as_bytes());
    let read = FileRead {
        text: FileText::Owned(text),
        fixups,
        first_nul,
    };
    search_file(&read, matcher, filter, rel_path, opts, writer, false)
}

/// Whether `--max-filesize` excludes this file.
///
/// The brute-force walk applies the limit while walking, but the indexed path
/// takes candidates straight from the index and the explicit-file path skips
/// the walk entirely. Both check here so the flag means the same thing on every
/// path instead of silently doing nothing on the default (indexed) one.
///
/// `explicit` marks a file the user named on the command line. Those are exempt
/// from the *inherited* [`ogrep_core::walker::DEFAULT_MAX_FILE_SIZE`], because
/// naming a file is an unambiguous request to search it and answering "no
/// match" would be a lie. A limit the user actually passed still applies, since
/// then the limit is itself the request.
fn exceeds_max_filesize(path: &Path, opts: &SearchOptions, explicit: bool) -> bool {
    if explicit && !opts.max_filesize_requested {
        return false;
    }
    let Some(limit) = opts.max_filesize else {
        return false;
    };
    std::fs::metadata(path).is_ok_and(|md| md.len() > limit)
}

/// Map line-relative columns from decoded text to the source bytes.
///
/// Columns count bytes from the start of the line, so both ends have to be
/// mapped before subtracting.
pub(crate) fn to_source_columns(
    columns: &[usize],
    line_offset: usize,
    fixups: &ogrep_core::encoding::LossyFixups,
) -> Vec<usize> {
    if fixups.is_empty() {
        return columns.to_vec();
    }
    let line = fixups.to_source_offset(line_offset);
    columns
        .iter()
        .map(|&c| fixups.to_source_offset(line_offset + c - 1) - line + 1)
        .collect()
}

/// Map one emitted match's columns and absolute offset onto the bytes on disk.
///
/// Shared by the brute-force/index path and the server so the two can never
/// disagree about coordinates. Columns arrive indexing the decoded source line
/// and are mapped back through `fixups`; `--replace` then moves each of them by
/// its entry in `column_shifts`, because ripgrep reports positions in the
/// rewritten line rather than in the original one.
///
/// The shifts are measured on the decoded text, so they are exact unless a
/// *replaced* match itself covered bytes that failed to decode — a case with no
/// ripgrep equivalent, since ripgrep searches raw bytes and never matches the
/// `U+FFFD` that repairing them produces. Columns are clamped to stay 1-based.
pub(crate) fn to_source_positions(
    columns: &[usize],
    column_shifts: &[isize],
    absolute_offset: usize,
    offset_shift: isize,
    line_offset: usize,
    fixups: &ogrep_core::encoding::LossyFixups,
) -> (Vec<usize>, usize) {
    let mut columns = to_source_columns(columns, line_offset, fixups);
    for (col, &shift) in columns.iter_mut().zip(column_shifts) {
        *col = col.saturating_add_signed(shift).max(1);
    }
    let offset = fixups
        .to_source_offset(absolute_offset)
        .saturating_add_signed(offset_shift);
    (columns, offset)
}

/// What happened to one file during a search.
///
/// `Skipped` exists because ripgrep hides binary files found by traversal
/// entirely: they must not surface as "no match" either, or they would leak
/// into `--files-without-match`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileOutcome {
    Matched,
    NoMatch,
    Skipped,
}

/// Whether a searched file satisfies the requested mode. Skipped files are
/// never selected, even by `--files-without-match`.
fn search_file(
    read: &FileRead,
    matcher: &SearchMatcher,
    filter: Option<&crate::matching::FileFilter>,
    rel_path: &str,
    opts: &SearchOptions,
    writer: &mut OutputWriter,
    explicit: bool,
) -> Result<bool> {
    match search_decoded_file(read, matcher, filter, rel_path, opts, writer, explicit)? {
        FileOutcome::Matched => Ok(!opts.files_without_match),
        FileOutcome::NoMatch if opts.files_without_match => {
            if !opts.quiet {
                writer.write_file(rel_path)?;
            }
            Ok(true)
        }
        FileOutcome::NoMatch | FileOutcome::Skipped => Ok(false),
    }
}

/// Search one file's decoded text, mapping reported offsets back to the bytes
/// on disk using `fixups`.
///
/// `explicit` is true when the user named this file directly on the command
/// line. ripgrep only surfaces binary files that were named explicitly (or
/// when `--binary` is given); binary files reached by directory traversal are
/// skipped silently, so they appear in neither `-l`, `-c`, `-L`, nor the plain
/// output.
fn search_decoded_file(
    read: &FileRead,
    matcher: &SearchMatcher,
    filter: Option<&crate::matching::FileFilter>,
    rel_path: &str,
    opts: &SearchOptions,
    writer: &mut OutputWriter,
    explicit: bool,
) -> Result<FileOutcome> {
    let _verify_span = crate::profile::span(crate::profile::Phase::VerifyRender);
    let content = read.text.as_str();
    let fixups = &read.fixups;

    // `first_nul` was found by the pass that read the file, so this costs
    // nothing here.
    let binary_offset = read.binary_offset(opts);
    if hides_binary(binary_offset, explicit, opts) {
        return Ok(FileOutcome::Skipped);
    }
    // `--with`/`--without` decide which files are searched at all, so a file
    // that fails them is absent from every listing, `--files-without-match`
    // included.
    let match_opts = opts.match_options();
    let Some(found) =
        crate::matching::FileMatches::find_filtered(content, filter, matcher, &match_opts)?
    else {
        return Ok(FileOutcome::Skipped);
    };
    writer.note_bytes_searched(read.bytes_searched(binary_offset));
    if opts.stats {
        let (matches, matched_lines) = found.match_totals();
        writer.note_matches(matches, matched_lines);
    }

    let has_matches = !found.is_empty();
    if !has_matches {
        // `--include-zero` still reports files that were searched but had no
        // match, which is the only way to tell them apart from files that were
        // never looked at.
        if opts.include_zero && (opts.count || opts.count_matches) && !opts.quiet {
            writer.write_count(rel_path, 0)?;
        }
        // Passthru still emits every line of a text file when none matched, but
        // ripgrep does not expose a binary file merely because passthru is set.
        if !match_opts.passthru || binary_offset.is_some() {
            return Ok(FileOutcome::NoMatch);
        }
    }

    // For quiet/files_without_match, we only need the outcome.
    if opts.quiet || opts.files_without_match {
        return Ok(FileOutcome::Matched);
    }

    if opts.files_only {
        writer.write_file(rel_path)?;
        return Ok(FileOutcome::Matched);
    }

    if opts.count || opts.count_matches {
        let n = if opts.count_matches {
            found.match_count()
        } else {
            found.matched_lines()
        };
        writer.write_count(rel_path, n)?;
        return Ok(FileOutcome::Matched);
    }

    // Never dump raw binary to a terminal; ripgrep reports a note instead.
    if let Some(off) = binary_offset
        && !writer.write_binary_note(rel_path, off)?
    {
        return Ok(FileOutcome::Matched);
    }

    found.for_each(&match_opts, matcher, |emit| -> Result<()> {
        match emit {
            crate::matching::Emit::Match {
                line_number,
                content,
                columns,
                spans,
                absolute_offset,
                line_offset,
                column_shifts,
                offset_shift,
                terminator_len,
            } => {
                writer.write_context_separator(rel_path, line_number)?;
                let (columns, absolute_offset) = to_source_positions(
                    &columns,
                    &column_shifts,
                    absolute_offset,
                    offset_shift,
                    line_offset,
                    fixups,
                );
                writer.write_match(&Match {
                    file: rel_path.to_string(),
                    line_number,
                    content: content.into_owned(),
                    columns,
                    spans,
                    absolute_offset,
                    terminator_len,
                })?;
            }
            crate::matching::Emit::Context {
                line_number,
                content,
                absolute_offset,
                terminator_len,
            } => {
                writer.write_context_separator(rel_path, line_number)?;
                writer.write_context_line(&ContextLine {
                    file: rel_path.to_string(),
                    line_number,
                    content: content.to_string(),
                    absolute_offset: fixups.to_source_offset(absolute_offset),
                    terminator_len,
                })?;
            }
        }
        Ok(())
    })?;

    Ok(if has_matches {
        FileOutcome::Matched
    } else {
        FileOutcome::NoMatch
    })
}

/// Path to print for a file the user named directly on the command line.
///
/// Relative to the cwd when the file lives under it, otherwise absolute. Either
/// way the `\\?\` prefix is stripped first: `--vimgrep` and `--json` consumers
/// feed these straight back to an editor, and no editor opens `//?/C:/...`.
fn explicit_file_display_path(path: &Path) -> String {
    let display_path = std::env::current_dir()
        .ok()
        .and_then(|cwd| std::fs::canonicalize(cwd).ok())
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| path.to_path_buf());

    strip_verbatim_prefix(&display_path.to_string_lossy()).replace('\\', "/")
}

fn passes_filters(
    rel_path: &str,
    glob_filter: &crate::glob_filter::GlobFilter,
    type_filter: &filetypes::TypeFilter,
) -> bool {
    type_filter.matches(rel_path) && glob_filter.matches(rel_path)
}

/// Apply `--type-clear` then `--type-add` to the built-in definitions and
/// compile the `-t`/`-T` selections. Lives here rather than on `SearchOptions`
/// so the server can reuse it verbatim from its own request fields.
pub fn build_type_filter(
    type_add: &[String],
    type_clear: &[String],
    types: &[String],
    types_not: &[String],
) -> Result<filetypes::TypeFilter> {
    if type_add.is_empty() && type_clear.is_empty() && types.is_empty() && types_not.is_empty() {
        return Ok(filetypes::TypeFilter::default());
    }
    let mut defs = filetypes::TypeDefs::builtin();
    // Clears run first so `--type-clear foo --type-add foo:*.x` redefines a
    // type from scratch, which is the documented ripgrep idiom.
    for name in type_clear {
        defs.clear(name);
    }
    for spec in type_add {
        defs.add(spec)?;
    }
    defs.build_filter(types, types_not)
}

/// Optional per-file RPC metadata, independent of rendered match/context rows.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct FileMatchStats {
    pub file: String,
    pub matches: u64,
    pub matched_lines: u64,
}

/// Legacy match and matched-line totals for `--stats`, counted from the rows the
/// client is actually going to print.
///
/// ripgrep reports these two numbers separately and neither of them counts
/// context lines, so `-C 2` must not change either total. `-o` makes the server
/// emit one row per match, so matches are summed from each row's spans while
/// matched lines are de-duplicated by file and line.
///
/// An inverted row (`-v`) carries an empty span list: ripgrep reports those as
/// matched lines but as zero matches, because no part of the line matched.
/// `--stats` implies `wants_match_detail`, so the spans are always present here
/// and an absent list means the row genuinely had none.
fn count_reported_matches(rows: &[serde_json::Value]) -> (u64, u64) {
    let mut matches = 0u64;
    let mut lines: std::collections::HashSet<(&str, u64)> = std::collections::HashSet::new();
    let mut binary_lines = 0u64;

    for row in rows {
        let file = row.get("file").and_then(|f| f.as_str()).unwrap_or("");
        match row.get("type").and_then(|t| t.as_str()).unwrap_or("match") {
            // A binary file is summarised as one row carrying its own count
            // rather than one row per match, which is what `-c` reports for it.
            "binary" => {
                let n = row.get("lines").and_then(|l| l.as_u64()).unwrap_or(0);
                matches += n;
                binary_lines += n;
            }
            "match" => {
                matches += row
                    .get("spans")
                    .and_then(|s| s.as_array())
                    .map_or(0, |s| s.len() as u64);
                lines.insert((file, row.get("line").and_then(|l| l.as_u64()).unwrap_or(0)));
            }
            // "context" and anything else is not a match.
            _ => {}
        }
    }

    (matches, lines.len() as u64 + binary_lines)
}

fn print_index_stats(plan: &str, raw: u64, candidates: u64, total: u64) {
    eprintln!(
        "Query plan: {plan} (candidates: {candidates}/{total}; raw candidates: {raw}/{total})"
    );
}

fn index_narrowing_note(raw: u64, total: u64) -> &'static str {
    if total > 0 && raw == total {
        " (no index narrowing)"
    } else {
        ""
    }
}

pub(crate) fn plan_summary(plan: &QueryPlan) -> String {
    match plan {
        // Sparse-gram queries ride along in every literal plan but are used only
        // against an index that has them; the summary keeps counting trigrams.
        QueryPlan::And(queries) => format!(
            "AND({} trigrams)",
            queries
                .iter()
                .filter(|q| !ogrep_core::sparse::is_gram_key(q.hash))
                .count()
        ),
        QueryPlan::All(plans) => format!(
            "ALL({})",
            plans
                .iter()
                .map(plan_summary)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        QueryPlan::Or(plans) => {
            let subs: Vec<String> = plans.iter().map(plan_summary).collect();
            format!("OR({})", subs.join(", "))
        }
        QueryPlan::MatchAll => "MatchAll (full scan)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    /// The client counts a reply's results as its writer will, so a server
    /// reply cut at `--max-results` is retried exactly when it falls short.
    #[test]
    fn reply_results_counts_as_the_writer_does() {
        let rows: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"type":"match","file":"a","line":1},{"type":"context","file":"a","line":2},
                {"type":"match","file":"a","line":3},{"type":"binary","file":"b","offset":0},
                {"type":"match","file":"b","line":1},{"type":"match","file":"c","line":9}]"#,
        )
        .unwrap();
        let mut opts = super::SearchOptions::default();
        // Lines, a binary file's note standing for its lines.
        assert_eq!(super::reply_results(&rows, &opts), 4);
        opts.json = true;
        assert_eq!(super::reply_results(&rows, &opts), 4);
        opts.json = false;
        opts.files_only = true;
        assert_eq!(super::reply_results(&rows, &opts), 3);
    }

    #[test]
    fn identifier_pattern_matches_every_naming_convention() {
        for name in [
            "getUserId",
            "get_user_id",
            "GetUserID",
            "get-user-id",
            "GET_USER_ID",
        ] {
            let pattern = super::identifier_pattern(name);
            assert_eq!(pattern, "(?i:get[_-]?user[_-]?id)", "{name}");
            let re = regex::Regex::new(&pattern).unwrap();
            for hay in [
                "let x = getUserId();",
                "fn get_user_id()",
                "GetUserID",
                "get-user-id",
                "GET_USER_ID = 3",
            ] {
                assert!(re.is_match(hay), "{name} vs {hay}");
            }
            assert!(!re.is_match("get_user_name"));
        }
        assert_eq!(
            super::identifier_pattern("HTTPServer"),
            "(?i:http[_-]?server)"
        );
        assert_eq!(
            super::identifier_pattern("parseV2Header"),
            "(?i:parse[_-]?v2[_-]?header)"
        );
        assert_eq!(super::identifier_pattern("a.b"), r"(?i:a\.b)");
        assert_eq!(super::identifier_pattern("__"), "__");
    }

    use super::*;

    #[test]
    fn only_a_nonempty_full_corpus_gets_a_no_narrowing_note() {
        assert_eq!(index_narrowing_note(2, 2), " (no index narrowing)");
        assert_eq!(index_narrowing_note(1, 2), "");
        assert_eq!(index_narrowing_note(0, 2), "");
        assert_eq!(index_narrowing_note(0, 0), "");
    }

    #[test]
    fn indexed_sort_preserves_entries_and_component_order() {
        let entries = [
            (1, "src.rs"),
            (2, "src/lib.rs"),
            (3, "a.rs"),
            (4, "src/lib.rs"),
        ];
        for (reverse, expected) in [(false, [3, 2, 4, 1]), (true, [1, 4, 2, 3])] {
            let mut sorted = entries;
            SortMode {
                key: SortKey::Path,
                reverse,
            }
            .apply_indexed(
                &mut sorted,
                Path::new("unused-index-root"),
                &IndexScope::Whole,
                |(_, rel)| rel,
            );
            assert_eq!(sorted.map(|(id, _)| id), expected);
        }
    }

    #[test]
    fn indexed_sort_uses_scoped_timestamps_and_orders_missing_metadata() {
        let dir = tempfile::TempDir::new().unwrap();
        let subtree = dir.path().join("nested");
        std::fs::create_dir(&subtree).unwrap();
        for (name, seconds) in [("new.txt", 60), ("z-old.txt", 0), ("a-old.txt", 0)] {
            let time =
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + seconds);
            std::fs::File::create(subtree.join(name))
                .unwrap()
                .set_modified(time)
                .unwrap();
        }

        for reverse in [false, true] {
            let mut paths = ["new.txt", "z-old.txt", "missing.txt", "a-old.txt"];
            SortMode {
                key: SortKey::Modified,
                reverse,
            }
            .apply_indexed(
                &mut paths,
                dir.path(),
                &IndexScope::Subtree("nested/".to_string()),
                |path| path,
            );
            let mut expected = ["missing.txt", "a-old.txt", "z-old.txt", "new.txt"];
            if reverse {
                expected.reverse();
            }
            assert_eq!(paths, expected);
        }
    }

    #[test]
    fn quiet_file_selection_distinguishes_nonmatching_and_skipped_files() {
        let matcher = SearchMatcher::Standard(
            regex::Regex::new("needle").unwrap(),
            crate::matching::Prescan::Direct,
        );
        for (content, explicit, binary, matched, without_match) in [
            ("needle\n", false, false, true, false),
            ("other\n", false, false, false, true),
            ("", false, false, false, true),
            ("needle\0\n", false, false, false, false),
            ("other\0\n", false, false, false, false),
            ("needle\0\n", true, false, true, false),
            ("other\0\n", true, false, false, true),
            ("needle\0\n", false, true, true, false),
            ("other\0\n", false, true, false, true),
        ] {
            let read = FileRead {
                text: FileText::Owned(content.to_string()),
                fixups: Default::default(),
                first_nul: content.find('\0'),
            };
            for files_without_match in [false, true] {
                let opts = SearchOptions {
                    quiet: true,
                    files_without_match,
                    binary,
                    ..Default::default()
                };
                let mut writer = new_writer(&opts);
                let selected = search_file(
                    &read,
                    &matcher,
                    None,
                    "file.txt",
                    &opts,
                    &mut writer,
                    explicit,
                )
                .unwrap();
                let expected = if files_without_match {
                    without_match
                } else {
                    matched
                };
                assert_eq!(
                    selected, expected,
                    "content={content:?}, explicit={explicit}, binary={binary}, \
                     files_without_match={files_without_match}"
                );
            }
        }
    }

    // --- fused UTF-8 validation and NUL scan --------------------------------
    //
    // `validate_utf8_and_find_nul` walks the file in chunks so that validating
    // it and finding the NUL that marks it binary share one traversal. The
    // chunking is the delicate part: a multi-byte sequence that straddles a
    // chunk boundary must be carried into the next chunk rather than reported
    // as invalid. These pin that against the whole-buffer answer it replaces.

    /// What the two separate passes used to produce, for differential testing.
    fn reference(bytes: &[u8]) -> Option<Option<usize>> {
        std::str::from_utf8(bytes).ok()?;
        Some(memchr::memchr(0, bytes))
    }

    fn check(bytes: &[u8]) {
        assert_eq!(
            validate_utf8_and_find_nul(bytes),
            reference(bytes),
            "disagreed with a whole-buffer scan on {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn fused_scan_handles_empty_and_ascii() {
        check(b"");
        check(b"hello world");
        check(b"hello\0world");
        check(b"\0");
    }

    #[test]
    fn fused_scan_rejects_invalid_utf8() {
        check(&[0xFF]);
        check(b"ok\xFFbad");
        // A truncated sequence at the very end is a real error, not a chunk
        // boundary to resume across.
        check(&[0xE2, 0x82]);
    }

    #[test]
    fn fused_scan_carries_a_sequence_across_a_chunk_boundary() {
        // Place every multi-byte width so that it straddles the boundary at
        // each possible split, which is exactly the case a per-chunk
        // `from_utf8` would reject.
        const CHUNK: usize = 1 << 20;
        for seq in ["\u{00E9}", "\u{20AC}", "\u{1F600}"] {
            for offset in 1..seq.len() {
                let mut bytes = vec![b'a'; CHUNK - offset];
                bytes.extend_from_slice(seq.as_bytes());
                bytes.extend_from_slice(b"tail");
                check(&bytes);
                // The same, with a NUL on the far side of the boundary, so the
                // reported offset has to survive the carry.
                let mut with_nul = bytes.clone();
                with_nul.push(0);
                with_nul.extend_from_slice(b"more");
                check(&with_nul);
            }
        }
    }

    #[test]
    fn fused_scan_reports_the_first_nul_not_a_later_one() {
        const CHUNK: usize = 1 << 20;
        // One NUL in the first chunk and one in the third: the scan must stop
        // updating after the first, even though later chunks also match.
        let mut bytes = vec![b'a'; CHUNK * 3];
        bytes[7] = 0;
        bytes[CHUNK * 2 + 11] = 0;
        check(&bytes);
        assert_eq!(validate_utf8_and_find_nul(&bytes), Some(Some(7)));
    }

    #[test]
    fn fused_scan_finds_a_nul_exactly_on_a_chunk_boundary() {
        const CHUNK: usize = 1 << 20;
        for at in [CHUNK - 1, CHUNK, CHUNK + 1] {
            let mut bytes = vec![b'a'; CHUNK * 2];
            bytes[at] = 0;
            check(&bytes);
            assert_eq!(validate_utf8_and_find_nul(&bytes), Some(Some(at)));
        }
    }

    #[test]
    fn fused_scan_spans_many_chunks_without_a_nul() {
        const CHUNK: usize = 1 << 20;
        // Multi-byte content throughout, so every chunk ends mid-sequence at
        // some point and the loop has to keep making progress.
        let unit = "héllo wörld €";
        let bytes = unit.repeat((CHUNK * 3) / unit.len()).into_bytes();
        check(&bytes);
        assert_eq!(validate_utf8_and_find_nul(&bytes), Some(None));
    }

    // --- `--stats` counting -------------------------------------------------
    //
    // These pin the reply-row semantics that `--stats` reports over the server.
    // The server counts every row it built, so
    // reporting its `num_matches` made `--stats` disagree with both the printed
    // output and ripgrep. Verified against ripgrep 15.2.0.

    fn row(kind: &str, file: &str, line: u64, spans: usize) -> serde_json::Value {
        serde_json::json!({
            "type": kind,
            "file": file,
            "line": line,
            "spans": (0..spans).map(|i| serde_json::json!([i, i + 1])).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn server_response_requires_explicit_complete_hidden_coverage() {
        let opts = SearchOptions::default();
        for response in [
            serde_json::json!({}),
            serde_json::json!({"hidden_complete": false}),
            serde_json::json!({"hidden_complete": null}),
        ] {
            assert!(require_server_support_in(&opts, &response).is_err());
        }
        assert!(
            require_server_support_in(&opts, &serde_json::json!({"hidden_complete": true})).is_ok()
        );
        // `--with` needs a server that says it applied the file conditions.
        let with = SearchOptions {
            with_patterns: vec!["x".into()],
            ..SearchOptions::default()
        };
        assert!(
            require_server_support_in(&with, &serde_json::json!({"hidden_complete": true}))
                .is_err()
        );
        assert!(
            require_server_support_in(
                &with,
                &serde_json::json!({"hidden_complete": true, "file_filters": true})
            )
            .is_ok()
        );
    }

    #[test]
    fn stats_counts_one_match_per_span() {
        // `rg --stats` on a line with 3 hits: "3 matches / 1 matched lines".
        let rows = vec![row("match", "a.rs", 1, 3)];
        assert_eq!(count_reported_matches(&rows), (3, 1));
    }

    #[test]
    fn stats_ignores_context_rows() {
        // `-C 2` must not change either total; counting raw rows made a
        // 90-match search report 445.
        let rows = vec![
            row("context", "a.rs", 1, 0),
            row("match", "a.rs", 2, 1),
            row("context", "a.rs", 3, 0),
        ];
        assert_eq!(count_reported_matches(&rows), (1, 1));
    }

    #[test]
    fn stats_deduplicates_lines_across_rows() {
        // `-o` emits one row per match, so the same line arrives twice; ripgrep
        // still reports it as a single matched line.
        let rows = vec![row("match", "a.rs", 7, 1), row("match", "a.rs", 7, 1)];
        assert_eq!(count_reported_matches(&rows), (2, 1));
    }

    #[test]
    fn stats_keeps_same_line_number_in_different_files_apart() {
        let rows = vec![row("match", "a.rs", 4, 1), row("match", "b.rs", 4, 1)];
        assert_eq!(count_reported_matches(&rows), (2, 2));
    }

    #[test]
    fn stats_reports_no_matches_for_inverted_rows() {
        // `rg -v --stats` reports "0 matches" with a non-zero matched-line
        // count, because no part of an inverted line matched.
        let rows = vec![row("match", "a.rs", 1, 0), row("match", "a.rs", 2, 0)];
        assert_eq!(count_reported_matches(&rows), (0, 2));
    }

    #[test]
    fn stats_uses_the_line_count_a_binary_row_carries() {
        // A binary file is summarised as one row rather than one row per match.
        let rows = vec![serde_json::json!({
            "type": "binary", "file": "a.bin", "lines": 5,
        })];
        assert_eq!(count_reported_matches(&rows), (5, 5));
    }

    #[test]
    fn stats_on_no_rows_is_zero() {
        assert_eq!(count_reported_matches(&[]), (0, 0));
    }

    #[test]
    fn stats_treats_a_row_without_spans_as_no_matches() {
        // `--stats` implies `wants_match_detail`, so a missing span list means
        // the row genuinely had none rather than that detail was suppressed.
        let rows = vec![serde_json::json!({
            "type": "match", "file": "a.rs", "line": 1,
        })];
        assert_eq!(count_reported_matches(&rows), (0, 1));
    }

    #[test]
    fn stats_requests_match_detail_so_spans_are_available_to_count() {
        let mut opts = SearchOptions {
            stats: true,
            ..Default::default()
        };
        assert!(
            opts.wants_match_detail(),
            "--stats must request spans, or the match total silently reads 0"
        );
        opts.stats = false;
        assert!(!opts.wants_match_detail());
    }

    #[test]
    fn display_path_strips_extended_length_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\src\repo\.ogrep")),
            r"C:\src\repo\.ogrep"
        );
    }

    #[test]
    fn display_path_restores_unc_paths() {
        // Naively stripping `\\?\` would leave a bogus `UNC\server\...` path.
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\idx")),
            r"\\server\share\idx"
        );
    }

    #[test]
    fn display_path_leaves_ordinary_paths_alone() {
        assert_eq!(display_path(Path::new(r"C:\src\repo")), r"C:\src\repo");
        assert_eq!(
            display_path(Path::new("/home/user/repo")),
            "/home/user/repo"
        );
    }

    #[test]
    fn explicit_file_path_outside_cwd_is_editor_openable() {
        // A file outside the cwd keeps its absolute path, but must not leak
        // `\\?\` — rendered with forward slashes that becomes `//?/C:/...`,
        // which no editor or `xargs` consumer can open.
        let rendered = explicit_file_display_path(Path::new(r"\\?\D:\other\tree\main.rs"));
        assert_eq!(rendered, "D:/other/tree/main.rs");
        assert!(!rendered.contains("//?/"), "got: {rendered}");
    }

    #[test]
    fn explicit_file_path_keeps_unc_shares_usable() {
        assert_eq!(
            explicit_file_display_path(Path::new(r"\\?\UNC\server\share\main.rs")),
            "//server/share/main.rs"
        );
    }
}
