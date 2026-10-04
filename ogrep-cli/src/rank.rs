//! `--rank`: order a search's files by relevance, keep the best few, and show
//! the most telling lines of each.
//!
//! The search itself runs unchanged into a capturing writer; ranking only
//! reorders and trims what it found, so every match printed is a real match and
//! every file an ordinary search would print is a candidate. Without `--rank`
//! none of this runs.
//!
//! ## Score
//!
//! A query-induced BM25F, the scheme Zoekt uses, computed from the matches
//! alone. Each file has three fields:
//!
//! - `def`: matching lines where the match is being defined (`fn name`,
//!   `def name`, `class Name`, ...);
//! - `body`: every other matching line;
//! - `path`: the match text occurs in the file's name (full weight) or
//!   directory (half).
//!
//! The weighted fields are summed and saturated: `tf (k1 + 1) / (tf + k1)`.
//! There is no length normalisation (see the constants). With several
//! patterns (`-e`), each pattern contributes its own saturated score weighted by
//! its inverse document frequency among the candidate files, so a rare pattern
//! counts for more than a common one. A multiplicative prior demotes tests,
//! vendored or generated code, and prose.
//!
//! Scores are rounded to millionths before comparing, and ties break by path,
//! so the order is a strict total order that does not depend on which file a
//! parallel search happened to report first.

use std::collections::HashMap;

use anyhow::Result;
use rayon::prelude::*;

use crate::output::{Captured, ContextLine, Match, OutputWriter, PathDisplay};

/// Scoring constants, tuned on the localization sets in `scripts/eval` (600
/// tasks mined from django and rust history; see `rank_sweep.py`). The best
/// settings formed a broad plateau, and these sit in its middle:
///
/// - Length normalisation only hurt (every `b > 0` scored lower), because the
///   file that defines a name is often a large one. File size is not used.
/// - A definition line counts ten ordinary matching lines.
/// - A name in the file's path adds five.
/// - Tests count 0.3 of other code, vendored or generated code 0.3, prose 0.8.
/// - File-graph centrality (PageRank) is not used: as a multiplier
///   `1 + λ·centrality` it lowered MRR at every λ tried (0.5 to 8), because the
///   file a change is about is rarely the most central one.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Weights {
    pub k1: f64,
    pub def: f64,
    pub body: f64,
    pub path: f64,
    pub test: f64,
    pub vendored: f64,
    pub prose: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            k1: 1.0,
            def: 10.0,
            body: 1.0,
            path: 5.0,
            test: 0.3,
            vendored: 0.3,
            prose: 0.8,
        }
    }
}

/// File, beside the index, holding a repository's own weights (`--rank-fit`).
pub const WEIGHTS_FILE: &str = "rank-weights.json";

impl Weights {
    /// The repository's fitted weights, or the defaults.
    pub fn for_index(index_dir: Option<&std::path::Path>) -> Self {
        index_dir
            .and_then(|d| std::fs::read(d.join(WEIGHTS_FILE)).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
}

/// How many files to keep after ranking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cutoff {
    /// Stop where scores fall away: below a quarter of the best, or at a drop of
    /// half from the previous file. The top three files are always kept.
    Auto,
    /// Keep everything up to the budget.
    None,
}

#[derive(Clone, Debug)]
pub struct RankOptions {
    /// Maximum number of files printed.
    pub budget: usize,
    /// Maximum number of matching lines printed per file.
    pub per_file: usize,
    pub cutoff: Cutoff,
    /// Print each file's score and features.
    pub explain: bool,
    pub weights: Weights,
    /// `--about`: the patterns are a description's words, scored as a bag of
    /// words (see [`score_about`]) instead of as names.
    pub about: bool,
    /// How many files the index holds, for `--about`'s inverse document
    /// frequencies; 0 when unknown.
    pub corpus_files: u64,
    /// `--cards`: print each file as a card, within about this many tokens.
    pub cards: Option<usize>,
}

impl Default for RankOptions {
    fn default() -> Self {
        Self {
            budget: 20,
            per_file: 3,
            cutoff: Cutoff::None,
            explain: false,
            weights: Weights::default(),
            about: false,
            corpus_files: 0,
            cards: None,
        }
    }
}

/// Everything one file contributed to the capture.
#[derive(Default)]
pub(crate) struct FileHits {
    pub(crate) display: PathDisplay,
    pub(crate) matches: Vec<Match>,
    pub(crate) contexts: Vec<ContextLine>,
    pub(crate) binary: Option<usize>,
    /// Per match, whether it defines the matched name, from the file's parsed
    /// structure. `None` where the language is not parsed; the keyword
    /// heuristic [`is_definition`] decides instead.
    pub(crate) defs: Option<Vec<bool>>,
    /// Matching lines --kind or --in dropped: context, if near a shown match.
    pub(crate) dropped: Vec<Match>,
    /// The file's text, when loaded for scope headers.
    pub(crate) text: Option<String>,
    /// The file's size in bytes, read for `--about`'s length normalisation.
    pub(crate) size: Option<u64>,
    /// Where the file is on disk, for `--about` and `--cards`.
    pub(crate) disk: Option<std::path::PathBuf>,
}

/// Per-file scoring inputs, reported by `--explain-rank`.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Features {
    pub def_lines: usize,
    pub body_lines: usize,
    pub path: f64,
    pub prior: f64,
    pub patterns: usize,
}

pub struct Ranked {
    /// Position in the grouped files: two path arguments can hold files with
    /// the same relative path, so the path alone does not identify one.
    pub file: usize,
    pub path: String,
    pub score: f64,
    pub features: Features,
    /// `--about` on a prose file: the heading section that gave its score.
    pub section: Option<crate::cards::Section>,
}

/// Group captured results by file, in capture order.
pub(crate) fn group(captures: Vec<(PathDisplay, Vec<Captured>)>) -> Vec<(String, FileHits)> {
    // (key, file name), in capture order: a file can have only a binary
    // note, and no match or context line to take its name from.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut files: HashMap<String, FileHits> = HashMap::new();
    for (display, events) in captures {
        for event in events {
            let file = match &event {
                Captured::Match(m) => m.file.clone(),
                Captured::Context(c) => c.file.clone(),
                Captured::Binary { file, .. } => file.clone(),
            };
            // The same relative path under two path arguments is two files.
            let key = format!("{display:?}\0{file}");
            let hits = files.entry(key.clone()).or_insert_with(|| {
                order.push((key.clone(), file.clone()));
                FileHits {
                    display: display.clone(),
                    ..Default::default()
                }
            });
            match event {
                Captured::Match(m) => hits.matches.push(m),
                Captured::Context(c) => hits.contexts.push(c),
                Captured::Binary { offset, .. } => hits.binary = Some(offset),
            }
        }
    }
    order
        .into_iter()
        .map(|(key, rel)| (rel, files.remove(&key).unwrap()))
        .collect()
}

/// Whether the match starting at byte `start` of `line` is a definition of the
/// matched name: the word before it is a definition keyword.
pub fn is_definition(line: &str, start: usize) -> bool {
    let before = line.get(..start).unwrap_or("").trim_end();
    if before.len() == line.get(..start).unwrap_or("").len() && !before.is_empty() {
        // No whitespace between the previous token and the match: `foo.bar`,
        // `self.bar`, `Foo::bar` are uses, not definitions.
        return false;
    }
    let word = before
        .rsplit(|c: char| c.is_whitespace() || c == '(' || c == '{')
        .next()
        .unwrap_or("");
    // A match on the word, not a scan of a keyword list: this runs for
    // every match on a line that has no parsed structure.
    matches!(
        word,
        "fn" | "def"
            | "class"
            | "struct"
            | "enum"
            | "trait"
            | "type"
            | "interface"
            | "func"
            | "function"
            | "const"
            | "let"
            | "var"
            | "val"
            | "impl"
            | "module"
            | "mod"
            | "macro_rules!"
            | "#define"
            | "union"
            | "record"
            | "object"
            | "protocol"
            | "extension"
            | "typedef"
            | "sub"
            | "procedure"
            | "method"
            | "static"
            | "async"
    )
}

pub(crate) fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    let dir = |d: &str| p.starts_with(&format!("{d}/")) || p.contains(&format!("/{d}/"));
    ["test", "tests", "testing", "spec", "__tests__"]
        .into_iter()
        .any(dir)
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_spec.")
        || name.ends_with("_tests.rs")
        || name.ends_with("tests.py")
}

fn is_vendored_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    [
        "vendor/",
        "third_party/",
        "third-party/",
        "node_modules/",
        "external/",
        "deps/",
    ]
    .iter()
    .any(|d| p.starts_with(d) || p.contains(&format!("/{d}")))
        || name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || name.ends_with(".lock")
        || name.ends_with(".pb.go")
        || name.ends_with("_pb2.py")
        || name.contains(".generated.")
}

fn is_prose_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".md", ".rst", ".txt", ".adoc", ".po", ".html", ".tex"]
        .iter()
        .any(|e| p.ends_with(e))
}

fn prior(path: &str, w: &Weights) -> f64 {
    if is_vendored_path(path) {
        w.vendored
    } else if is_test_path(path) {
        w.test
    } else if is_prose_path(path) {
        w.prose
    } else {
        1.0
    }
}

fn round(score: f64) -> i64 {
    (score * 1e6).round() as i64
}

fn saturate(tf: f64, k1: f64) -> f64 {
    tf * (k1 + 1.0) / (tf + k1)
}

/// Which pattern produced a match text. With one pattern it is always 0.
struct PatternSet {
    /// One per pattern, in order; `None` where the plain engine cannot
    /// compile it (a -P lookaround), so later patterns keep their index.
    regexes: Vec<Option<regex::Regex>>,
}

impl PatternSet {
    fn new(patterns: &[String], fixed: bool, ci: bool) -> Self {
        let regexes = if patterns.len() < 2 {
            Vec::new()
        } else {
            patterns
                .iter()
                .map(|p| {
                    let p = if fixed { regex::escape(p) } else { p.clone() };
                    regex::RegexBuilder::new(&p)
                        .case_insensitive(ci)
                        .build()
                        .ok()
                })
                .collect()
        };
        Self { regexes }
    }

    fn len(&self) -> usize {
        self.regexes.len().max(1)
    }

    fn of(&self, text: &str) -> usize {
        self.regexes
            .iter()
            .position(|re| re.as_ref().is_some_and(|re| re.is_match(text)))
            // No compiled pattern matched: credit a pattern the plain engine
            // could not compile, since the search matched it somehow.
            .or_else(|| self.regexes.iter().position(Option::is_none))
            .unwrap_or(0)
    }
}

/// Whether line `i` of a file defines what it matches: from the parsed
/// structure (`defs`) when there is one, else by the keyword heuristic, and
/// in both cases by the line's first match.
fn line_is_definition(defs: Option<&[bool]>, i: usize, m: &Match) -> bool {
    defs.and_then(|d| d.get(i).copied()).unwrap_or_else(|| {
        m.spans
            .first()
            .is_some_and(|&(s, _)| is_definition(&m.content, s))
    })
}

/// Score and order grouped files.
fn score_files(files: &[(String, FileHits)], patterns: &PatternSet, w: &Weights) -> Vec<Ranked> {
    let n = patterns.len();
    // Per file, per pattern: (def lines, body lines); plus matched texts.
    struct Tally {
        lines: Vec<(usize, usize)>,
        texts: Vec<String>,
    }
    let tallies: Vec<Tally> = files
        .iter()
        .map(|(_, hits)| {
            let mut lines = vec![(0usize, 0usize); n];
            let mut texts = Vec::new();
            // Each text is attributed to a pattern, and kept, once.
            let mut pattern_of: std::collections::HashMap<&str, usize> = Default::default();
            for (i, m) in hits.matches.iter().enumerate() {
                // A line is scored by its first match, whatever else the
                // output asked the search to collect, so the order never
                // depends on output flags.
                let p = match m.spans.first().and_then(|&(s, e)| m.content.get(s..e)) {
                    Some(text) => *pattern_of.entry(text).or_insert_with(|| {
                        if text.len() >= 3 {
                            texts.push(text.to_ascii_lowercase());
                        }
                        patterns.of(text)
                    }),
                    None => 0,
                };
                let line = &mut lines[p];
                if line_is_definition(hits.defs.as_deref(), i, m) {
                    line.0 += 1;
                } else {
                    line.1 += 1;
                }
            }
            texts.sort();
            texts.dedup();
            Tally { lines, texts }
        })
        .collect();

    let total = files.len() as f64;
    let idf: Vec<f64> = (0..n)
        .map(|p| {
            if n == 1 {
                return 1.0;
            }
            let df = tallies
                .iter()
                .filter(|t| t.lines[p].0 + t.lines[p].1 > 0)
                .count() as f64;
            (1.0 + (total - df + 0.5) / (df + 0.5)).ln()
        })
        .collect();

    let mut ranked: Vec<Ranked> = files
        .iter()
        .zip(&tallies)
        .enumerate()
        .map(|(file, ((rel, _), tally))| {
            let lower = rel.to_ascii_lowercase();
            let (dir, name) = lower.rsplit_once('/').unwrap_or(("", &lower));
            let path = if tally.texts.iter().any(|t| name.contains(t.as_str())) {
                1.0
            } else if tally.texts.iter().any(|t| dir.contains(t.as_str())) {
                0.5
            } else {
                0.0
            };
            let mut score = 0.0;
            for (&(d, b), &weight) in tally.lines.iter().zip(&idf) {
                if d + b == 0 {
                    continue;
                }
                let tf = w.def * d as f64 + w.body * b as f64 + w.path * path;
                score += weight * saturate(tf, w.k1);
            }
            let prior = prior(rel, w);
            let features = Features {
                def_lines: tally.lines.iter().map(|l| l.0).sum(),
                body_lines: tally.lines.iter().map(|l| l.1).sum(),
                path,
                prior,
                patterns: tally.lines.iter().filter(|l| l.0 + l.1 > 0).count(),
            };
            Ranked {
                file,
                path: rel.clone(),
                score: score * prior,
                features,
                section: None,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        round(b.score)
            .cmp(&round(a.score))
            .then_with(|| a.path.cmp(&b.path))
    });
    ranked
}

/// How many of the ranked files to keep.
fn keep_count(ranked: &[Ranked], opts: &RankOptions) -> usize {
    let mut keep = ranked.len().min(opts.budget);
    if opts.cutoff == Cutoff::Auto
        && let Some(top) = ranked.first().map(|r| r.score)
    {
        for i in 3..keep {
            let (score, prev) = (ranked[i].score, ranked[i - 1].score);
            if score < 0.25 * top || score < 0.5 * prev {
                keep = i;
                break;
            }
        }
    }
    keep
}

/// Pick the lines to show for one file: definitions first, then the earliest,
/// skipping lines that differ from a kept one only in whitespace or digits.
fn select_lines(matches: &[Match], defs: Option<&[bool]>, per_file: usize) -> Vec<usize> {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && !c.is_ascii_digit())
            .collect()
    };
    let mut order: Vec<(usize, &Match)> = matches.iter().enumerate().collect();
    order.sort_by_cached_key(|&(i, m)| (!line_is_definition(defs, i, m), m.line_number));
    let mut seen = std::collections::HashSet::new();
    let mut picked: Vec<usize> = Vec::new();
    for (i, m) in order {
        if picked.len() >= per_file {
            break;
        }
        if seen.insert(norm(&m.content)) {
            picked.push(i);
        }
    }
    picked.sort_by_key(|&i| matches[i].line_number);
    picked
}

/// Rank captured results and print them through `writer`.
///
/// Returns whether anything was printed.
pub fn emit(
    files: Vec<(String, FileHits)>,
    patterns: &[String],
    fixed: bool,
    ci: bool,
    opts: &RankOptions,
    search: &crate::search::SearchOptions,
    writer: &mut OutputWriter,
) -> Result<bool> {
    if files.is_empty() {
        return Ok(false);
    }
    let ranked = if opts.about {
        score_about(&files, patterns, opts.corpus_files, &opts.weights)
    } else {
        score_files(&files, &PatternSet::new(patterns, fixed, ci), &opts.weights)
    };
    let keep = keep_count(&ranked, opts);
    let confidence = opts.about.then(|| about_confidence(&ranked)).flatten();
    let printed = match opts.cards {
        Some(budget) => emit_cards(
            &files,
            &ranked[..keep],
            patterns,
            opts,
            budget,
            confidence.as_ref(),
            writer,
        )?,
        None => emit_ranked(
            &files,
            &ranked[..keep],
            patterns,
            opts,
            confidence.as_ref(),
            search,
            writer,
        )?,
    };
    if let Some(c) = &confidence
        && c.level == "unsure"
        && printed
        && !writer.is_json()
        && !search.quiet
        && !search.no_messages
    {
        eprintln!(
            "ogrep: --about: unsure: the first two files score within {:.0}% of each other; \
             read the top five, or add a name from the code to the description",
            ABOUT_UNSURE_MARGIN * 100.0
        );
    }
    Ok(printed)
}

fn emit_ranked(
    files: &[(String, FileHits)],
    ranked: &[Ranked],
    patterns: &[String],
    opts: &RankOptions,
    confidence: Option<&Confidence>,
    search: &crate::search::SearchOptions,
    writer: &mut OutputWriter,
) -> Result<bool> {
    for (rank, r) in ranked.iter().enumerate() {
        let hits = &files[r.file].1;
        writer.set_path_display(hits.display.clone());
        let first = confidence.filter(|_| rank == 0);
        writer.write_rank_record(&r.path, rank + 1, r.score, &r.features, opts.explain, first)?;
        let shown = shown_lines(hits, r, patterns, opts);
        // A prose file's best section is printed as the scope of its lines.
        let scopes: Vec<Option<crate::post::Scope>> =
            match r.section.as_ref().and_then(crate::post::Scope::section) {
                Some(scope) => {
                    let mut scopes = vec![None; hits.matches.len()];
                    for &i in &shown {
                        scopes[i] = Some(scope.clone());
                    }
                    scopes
                }
                None => Vec::new(),
            };
        crate::post::emit_file(&r.path, hits, &shown, &scopes, search, writer)?;
    }
    Ok(!ranked.is_empty())
}

/// How far `--about` trusts its first file: `unsure` when the first two files
/// score within this fraction of the first one's score. On the localization
/// sets in `scripts/eval` the 30% of queries below it found a changed file
/// first 41-53% of the time, against 79-80% above it, with one threshold set
/// on django and kept for llama.cpp and rust (`research_notes`, round 8).
pub const ABOUT_UNSURE_MARGIN: f64 = 0.05;

/// `--about`'s confidence in its first file, reported with it.
#[derive(Debug, serde::Serialize)]
pub struct Confidence {
    /// `confident` or `unsure`.
    pub level: &'static str,
    /// (first score - second score) / first score; 1 when one file matched.
    pub margin: f64,
}

fn about_confidence(ranked: &[Ranked]) -> Option<Confidence> {
    let first = ranked.first()?.score;
    let margin = match ranked.get(1) {
        Some(second) if first > 0.0 => (first - second.score) / first,
        Some(_) => 0.0,
        None => 1.0,
    };
    Some(Confidence {
        level: if margin < ABOUT_UNSURE_MARGIN {
            "unsure"
        } else {
            "confident"
        },
        margin: (margin * 1e4).round() / 1e4,
    })
}

/// `--about`'s scoring constants, chosen on the localization sets in
/// `scripts/eval` with the description (the commit subject) as the query, over
/// every indexed file: plain BM25 (k1 1.2) with mild length normalisation
/// (b 0.3 beat 0, 0.5 and 0.75 on both django and rust), a term found in the
/// file's path adding five matching lines, and the same priors as `--rank`.
/// Every term is kept, however common: dropping terms found in more than a
/// fifth of the files lost accuracy.
const ABOUT_K1: f64 = 1.2;
const ABOUT_B: f64 = 0.3;
const ABOUT_PATH: f64 = 5.0;

/// Words that carry no meaning about where code lives: English function words
/// and the verbs of commit messages and bug reports.
const ABOUT_STOP: &[&str] = &[
    "a",
    "an",
    "the",
    "of",
    "in",
    "on",
    "for",
    "to",
    "and",
    "or",
    "with",
    "by",
    "from",
    "is",
    "are",
    "be",
    "was",
    "were",
    "as",
    "at",
    "it",
    "its",
    "this",
    "that",
    "not",
    "no",
    "into",
    "via",
    "when",
    "if",
    "else",
    "than",
    "then",
    "so",
    "do",
    "does",
    "did",
    "made",
    "make",
    "use",
    "used",
    "using",
    "added",
    "removed",
    "fixed",
    "changed",
    "allowed",
    "handled",
    "corrected",
    "improved",
    "updated",
    "support",
    "supported",
    "refs",
    "fix",
    "fixes",
    "avoid",
    "avoided",
    "ensure",
    "ensured",
    "prevent",
    "prevented",
    "add",
    "adds",
    "remove",
    "removes",
    "change",
    "changes",
    "update",
    "updates",
    "handle",
    "handles",
    "allow",
    "allows",
    "correct",
    "uses",
    "new",
    "old",
    "more",
    "less",
    "only",
    "also",
    "all",
    "any",
    "some",
    "can",
    "cannot",
    "should",
    "would",
    "will",
    "may",
    "might",
    "must",
    "now",
    "after",
    "before",
    "during",
    "without",
    "within",
    "between",
    "over",
    "under",
];

/// The search terms of a description, in order and without repeats: each word
/// lower-cased, identifiers split into their parts (`date_hierarchy` gives
/// `date` and `hierarchy`, `getUserId` gives `get`, `user`, `id`) and also
/// kept whole when six characters or longer, dropping numbers, words under
/// three characters and [`ABOUT_STOP`] words.
pub fn about_terms(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |t: String| {
        if t.len() >= 3
            && !t.bytes().all(|b| b.is_ascii_digit())
            && !ABOUT_STOP.contains(&t.as_str())
            && !out.contains(&t)
        {
            out.push(t);
        }
    };
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for token in text.split(|c: char| !is_word(c)) {
        let token = token.trim_start_matches(|c: char| c.is_ascii_digit());
        if token.is_empty() {
            continue;
        }
        let parts = identifier_parts(token);
        if parts.len() > 1 && token.len() >= 6 {
            push(token.to_ascii_lowercase());
        }
        for part in parts {
            push(part);
        }
    }
    out
}

/// The lower-cased parts of an identifier: split at underscores, at a change
/// from lower to upper case, before the last capital of an acronym followed by
/// lower case (`HTTPServer` gives `http`, `server`), and between letters and
/// digits.
fn identifier_parts(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if let Some(&prev) = cur.chars().last().as_ref() {
            let next = chars.get(i + 1).copied();
            let boundary = (prev.is_ascii_lowercase() && c.is_ascii_uppercase())
                || (prev.is_ascii_uppercase()
                    && c.is_ascii_uppercase()
                    && next.is_some_and(|n| n.is_ascii_lowercase()))
                || (prev.is_ascii_digit() != c.is_ascii_digit());
            if boundary {
                parts.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts.into_iter().map(|p| p.to_ascii_lowercase()).collect()
}

/// `--about` on prose: a section's length is normalised more strongly than
/// a file's (sections vary from a line to pages), a term in the headings
/// enclosing a section counts as this many matching lines, and a prose
/// file's score is its whole-file score times (best section / whole file) to
/// this power. Chosen on django's documentation (commit subject -> the docs
/// section it changed: 0.5 beat 0, 0.25, 0.75 and 1) and checked on held-out
/// sets (`scripts/eval/docs_eval.py`, `research_notes` round 9).
const ABOUT_SECTION_B: f64 = 0.75;
const ABOUT_HEADING: f64 = 3.0;
const ABOUT_SECTION_WEIGHT: f64 = 0.5;

/// Score files for `--about` with BM25 over the description's terms.
///
/// A term's frequency in a file is the number of its matching lines that hold
/// the term (case-insensitively), plus [`ABOUT_PATH`] when the term is in the
/// file's path. Document frequencies are exact: the search matched every
/// file holding any term. File length is the size in bytes, against the mean
/// size of the files scored. The total is multiplied by `--rank`'s prior.
///
/// A prose file with headings is also scored by its best section holding a
/// matching line: the same sum over the section's own matching lines, plus
/// [`ABOUT_HEADING`] for a term in its headings, normalised by the section's
/// size against the mean section size of the prose files scored. The two scores are blended by
/// [`ABOUT_SECTION_WEIGHT`]. Other files score as before.
fn score_about(
    files: &[(String, FileHits)],
    terms: &[String],
    corpus_files: u64,
    w: &Weights,
) -> Vec<Ranked> {
    let lower: Vec<String> = terms.iter().map(|t| t.to_ascii_lowercase()).collect();
    // Per match, the terms its line holds.
    let held = |m: &Match| -> Vec<usize> {
        let line = m.content.to_ascii_lowercase();
        (0..lower.len())
            .filter(|&t| line.contains(lower[t].as_str()))
            .collect()
    };
    // Per file, per term: matching lines holding it.
    let tallies: Vec<Vec<usize>> = files
        .iter()
        .map(|(_, hits)| {
            let mut tf = vec![0usize; lower.len()];
            for m in &hits.matches {
                let line = m.content.to_ascii_lowercase();
                for (t, term) in lower.iter().enumerate() {
                    if line.contains(term.as_str()) {
                        tf[t] += 1;
                    }
                }
            }
            tf
        })
        .collect();
    let n = (corpus_files as f64).max(files.len() as f64);
    let idf: Vec<f64> = (0..lower.len())
        .map(|t| {
            let df = tallies.iter().filter(|tf| tf[t] > 0).count() as f64;
            (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
        })
        .collect();
    let sizes: Vec<f64> = files
        .iter()
        .map(|(_, h)| h.size.unwrap_or(0).max(1) as f64)
        .collect();
    let avg = sizes.iter().sum::<f64>() / sizes.len().max(1) as f64;
    let bm25 = |tf: &[f64], norm: f64| -> f64 {
        tf.iter()
            .zip(&idf)
            .filter(|(f, _)| **f > 0.0)
            .map(|(&f, &idf)| idf * f * (ABOUT_K1 + 1.0) / (f + norm))
            .sum()
    };
    // Prose files' sections, read only for files that are prose.
    let sections: Vec<Vec<crate::cards::Section>> = files
        .par_iter()
        .map(|(rel, h)| {
            if !crate::cards::is_prose(rel) {
                return Vec::new();
            }
            h.disk
                .as_deref()
                .and_then(crate::cards::read)
                .map(|text| crate::cards::sections(rel, &text))
                .unwrap_or_default()
        })
        .collect();
    let (count, total) = sections
        .iter()
        .flatten()
        .fold((0usize, 0usize), |(c, b), s| (c + 1, b + s.bytes));
    let avg_section = (total.max(1) as f64) / (count.max(1) as f64);
    // Prose files' (whole-file score, blended score), to keep prose at its
    // whole-file level against other files.
    let mut blended: Vec<(f64, f64)> = Vec::new();
    let mut ranked: Vec<Ranked> = files
        .iter()
        .zip(&tallies)
        .zip(&sizes)
        .enumerate()
        .map(|(file, (((rel, hits), tf), &size))| {
            let path = rel.to_ascii_lowercase();
            let boost: Vec<f64> = lower
                .iter()
                .map(|term| {
                    if path.contains(term.as_str()) {
                        ABOUT_PATH
                    } else {
                        0.0
                    }
                })
                .collect();
            let in_path = boost.iter().filter(|&&b| b > 0.0).count();
            let found = tf.iter().filter(|&&f| f > 0).count();
            let whole = {
                let norm = ABOUT_K1 * (1.0 - ABOUT_B + ABOUT_B * size / avg);
                let f: Vec<f64> = tf.iter().zip(&boost).map(|(&t, &b)| t as f64 + b).collect();
                bm25(&f, norm)
            };
            let (score, section) = if sections[file].is_empty() {
                (whole, None)
            } else {
                let mut best: Option<(f64, &crate::cards::Section)> = None;
                // The matches in line order, so a section's matches are one run.
                let mut order: Vec<usize> = (0..hits.matches.len()).collect();
                order.sort_by_key(|&i| hits.matches[i].line_number);
                let lines: Vec<usize> =
                    order.iter().map(|&i| hits.matches[i].line_number).collect();
                let terms_of: Vec<Vec<usize>> = hits.matches.iter().map(held).collect();
                for sec in &sections[file] {
                    let mut f = boost.clone();
                    let from = lines.partition_point(|&l| l < sec.line);
                    let to = lines.partition_point(|&l| l <= sec.end);
                    // A section is chosen for its own matching lines, which
                    // are then the lines shown.
                    if from >= to {
                        continue;
                    }
                    for &i in &order[from..to] {
                        for &t in &terms_of[i] {
                            f[t] += 1.0;
                        }
                    }
                    let titles: Vec<String> = sec
                        .chain
                        .iter()
                        .map(|(_, _, title)| title.to_ascii_lowercase())
                        .collect();
                    for (t, term) in lower.iter().enumerate() {
                        if titles.iter().any(|title| title.contains(term.as_str())) {
                            f[t] += ABOUT_HEADING;
                        }
                    }
                    let norm = ABOUT_K1
                        * (1.0 - ABOUT_SECTION_B
                            + ABOUT_SECTION_B * sec.bytes.max(1) as f64 / avg_section);
                    let score = bm25(&f, norm);
                    // The earliest of equal sections wins.
                    if best.is_none_or(|(b, _)| round(score) > round(b)) {
                        best = Some((score, sec));
                    }
                }
                // None only when the file changed since it was searched.
                match best {
                    Some((best, sec)) => {
                        let score = if whole > 0.0 {
                            whole * (best / whole).powf(ABOUT_SECTION_WEIGHT)
                        } else {
                            best
                        };
                        blended.push((whole, score));
                        (score, Some(sec.clone()))
                    }
                    None => (whole, None),
                }
            };
            let prior = prior(rel, w);
            Ranked {
                file,
                path: rel.clone(),
                score: score * prior,
                features: Features {
                    def_lines: 0,
                    body_lines: hits.matches.len(),
                    path: in_path as f64,
                    prior,
                    patterns: found,
                },
                section,
            }
        })
        .collect();
    // Sections score lower than whole files, since a section holds fewer of
    // the terms. Scale prose files so their geometric mean is the whole-file
    // one: sections decide the order among prose files, not how prose
    // compares with code or other files.
    let logs: Vec<f64> = blended
        .iter()
        .filter(|(whole, score)| *whole > 0.0 && *score > 0.0)
        .map(|(whole, score)| whole.ln() - score.ln())
        .collect();
    if !logs.is_empty() {
        let scale = (logs.iter().sum::<f64>() / logs.len() as f64).exp();
        for r in ranked.iter_mut().filter(|r| r.section.is_some()) {
            r.score *= scale;
        }
    }
    ranked.sort_by(|a, b| {
        round(b.score)
            .cmp(&round(a.score))
            .then_with(|| a.path.cmp(&b.path))
    });
    ranked
}

/// The lines to show for one ranked file. For a prose file `--about` scored
/// by a section, they come from that section, which always has some.
fn shown_lines(hits: &FileHits, r: &Ranked, patterns: &[String], opts: &RankOptions) -> Vec<usize> {
    if !opts.about {
        return select_lines(&hits.matches, hits.defs.as_deref(), opts.per_file);
    }
    let candidates: Vec<usize> = match &r.section {
        Some(sec) => (0..hits.matches.len())
            .filter(|&i| (sec.line..=sec.end).contains(&hits.matches[i].line_number))
            .collect(),
        None => (0..hits.matches.len()).collect(),
    };
    select_about_lines(&hits.matches, &candidates, patterns, opts.per_file)
}

/// Pick the lines to show for one `--about` file among `candidates`: those
/// holding the most different terms first, then the earliest, skipping
/// near-duplicates as [`select_lines`] does.
fn select_about_lines(
    matches: &[Match],
    candidates: &[usize],
    terms: &[String],
    per_file: usize,
) -> Vec<usize> {
    let terms: Vec<String> = terms.iter().map(|t| t.to_ascii_lowercase()).collect();
    let held = |m: &Match| {
        let line = m.content.to_ascii_lowercase();
        terms.iter().filter(|t| line.contains(t.as_str())).count()
    };
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && !c.is_ascii_digit())
            .collect()
    };
    let mut order: Vec<(usize, usize, usize)> = candidates
        .iter()
        .map(|&i| (usize::MAX - held(&matches[i]), matches[i].line_number, i))
        .collect();
    order.sort_unstable();
    let mut seen = std::collections::HashSet::new();
    let mut picked: Vec<usize> = Vec::new();
    for (_, _, i) in order {
        if picked.len() >= per_file {
            break;
        }
        if seen.insert(norm(&matches[i].content)) {
            picked.push(i);
        }
    }
    picked.sort_by_key(|&i| matches[i].line_number);
    picked
}

/// Print the kept files as cards (`--cards`), best first, until the token
/// budget is spent; the first card is always printed. A file that cannot be
/// read as text gets a card of its matching lines alone.
fn emit_cards(
    files: &[(String, FileHits)],
    ranked: &[Ranked],
    patterns: &[String],
    opts: &RankOptions,
    budget: usize,
    confidence: Option<&Confidence>,
    writer: &mut OutputWriter,
) -> Result<bool> {
    let mut used = 0usize;
    let mut printed = false;
    for (rank, r) in ranked.iter().enumerate() {
        let hits = &files[r.file].1;
        let shown = shown_lines(hits, r, patterns, opts);
        let matches: Vec<(usize, String)> = shown
            .iter()
            .map(|&i| (hits.matches[i].line_number, hits.matches[i].content.clone()))
            .collect();
        let text = hits.disk.as_deref().and_then(crate::cards::read);
        let card = crate::cards::build(&r.path, text.as_deref().unwrap_or(""), &matches);
        let cost = card.tokens(&r.path);
        if printed && used + cost > budget {
            break;
        }
        used += cost;
        printed = true;
        writer.set_path_display(hits.display.clone());
        writer.write_card(
            &r.path,
            rank + 1,
            r.score,
            &card,
            opts.explain.then_some(&r.features),
            confidence.filter(|_| rank == 0),
        )?;
    }
    Ok(printed)
}

/// One `--rank-fit` example: a query's grouped results and the files that
/// should rank first.
pub struct Example {
    pub files: Vec<(String, FileHits)>,
    pub patterns: Vec<String>,
    pub fixed: bool,
    pub ci: bool,
    pub relevant: std::collections::HashSet<String>,
}

fn mrr(examples: &[Example], w: &Weights) -> f64 {
    let total: f64 = examples
        .iter()
        .map(|e| {
            let ranked = score_files(&e.files, &PatternSet::new(&e.patterns, e.fixed, e.ci), w);
            ranked
                .iter()
                .position(|r| e.relevant.contains(&r.path))
                .map_or(0.0, |p| 1.0 / (p + 1) as f64)
        })
        .sum();
    total / examples.len().max(1) as f64
}

/// Grid-search weights for the best mean reciprocal rank on `examples`.
/// Returns `(weights, default MRR, fitted MRR)`. Deterministic: ties keep the
/// earlier grid point, and the grid includes the defaults.
pub fn fit(examples: &[Example]) -> (Weights, f64, f64) {
    let base = Weights::default();
    let base_mrr = mrr(examples, &base);
    let mut best = (base, base_mrr);
    for def in [3.0, 6.0, 10.0, 20.0] {
        for path in [0.0, 2.0, 5.0, 10.0] {
            for test in [0.1, 0.3, 0.6, 1.0] {
                for prose in [0.5, 0.8, 1.0] {
                    for k1 in [0.6, 1.0, 1.6] {
                        let w = Weights {
                            def,
                            path,
                            test,
                            prose,
                            k1,
                            ..base
                        };
                        let m = mrr(examples, &w);
                        if m > best.1 + 1e-12 {
                            best = (w, m);
                        }
                    }
                }
            }
        }
    }
    (best.0, base_mrr, best.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_recognised_by_the_preceding_keyword() {
        assert!(is_definition("    def bulk_update(self, objs):", 8));
        assert!(is_definition("pub fn parse_header(x: &str)", 7));
        assert!(is_definition("class QuerySet(AltersData):", 6));
        assert!(!is_definition("        qs.bulk_update(objs)", 11));
        assert!(!is_definition("x = bulk_update(objs)", 4));
        assert!(!is_definition("self.bulk_update()", 5));
    }

    #[test]
    fn about_confidence_compares_the_first_two_scores() {
        let ranked = |scores: &[f64]| -> Vec<Ranked> {
            scores
                .iter()
                .enumerate()
                .map(|(file, &score)| Ranked {
                    file,
                    path: format!("f{file}"),
                    score,
                    features: Features::default(),
                    section: None,
                })
                .collect()
        };
        assert!(about_confidence(&[]).is_none());
        let c = about_confidence(&ranked(&[10.0, 8.0, 1.0])).unwrap();
        assert_eq!((c.level, c.margin), ("confident", 0.2));
        let c = about_confidence(&ranked(&[10.0, 9.6])).unwrap();
        assert_eq!((c.level, c.margin), ("unsure", 0.04));
        let c = about_confidence(&ranked(&[10.0, 9.5])).unwrap();
        assert_eq!(
            c.level, "confident",
            "the threshold itself counts as confident"
        );
        let c = about_confidence(&ranked(&[3.0])).unwrap();
        assert_eq!((c.level, c.margin), ("confident", 1.0));
    }

    #[test]
    fn about_terms_split_identifiers_and_drop_noise() {
        assert_eq!(
            about_terms("Handled out-of-range year in admin date_hierarchy."),
            [
                "out",
                "range",
                "year",
                "admin",
                "date_hierarchy",
                "date",
                "hierarchy"
            ]
        );
        assert_eq!(
            about_terms("Fixed #37339 -- Allowed ipaddress objects in GenericIPAddressField."),
            [
                "ipaddress",
                "objects",
                "genericipaddressfield",
                "generic",
                "address",
                "field"
            ]
        );
        assert_eq!(
            about_terms("HTTPServer getUserId"),
            ["httpserver", "http", "server", "getuserid", "get", "user"]
        );
        assert!(about_terms("fix the a to").is_empty());
    }

    #[test]
    fn about_ranks_the_file_holding_the_rare_terms_first() {
        let caps = vec![(
            PathDisplay::Bare,
            vec![
                Captured::Match(m("a/common.py", 1, "the admin view", "admin")),
                Captured::Match(m(
                    "b/hier.py",
                    2,
                    "date_hierarchy year range",
                    "date_hierarchy",
                )),
                Captured::Match(m("c/admin.py", 3, "admin admin", "admin")),
            ],
        )];
        let files = group(caps);
        let terms = about_terms("out-of-range year in admin date_hierarchy");
        let ranked = score_about(&files, &terms, 1000, &Weights::default());
        assert_eq!(ranked[0].path, "b/hier.py");
        // Same input, same order.
        let again = score_about(&files, &terms, 1000, &Weights::default());
        let paths = |r: &[Ranked]| r.iter().map(|x| x.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths(&ranked), paths(&again));
    }

    #[test]
    fn path_priors() {
        assert!(is_test_path("tests/queries/test_bulk_update.py"));
        assert!(is_test_path("src/foo_test.go"));
        assert!(!is_test_path("django/db/models/query.py"));
        assert!(is_vendored_path("vendor/github.com/x/y.go"));
        assert!(is_prose_path("docs/ref/models/querysets.txt"));
    }

    fn m(file: &str, line: usize, content: &str, needle: &str) -> Match {
        let s = content.find(needle).unwrap();
        Match {
            file: file.into(),
            line_number: line,
            content: content.into(),
            columns: vec![s + 1],
            spans: vec![(s, s + needle.len())],
            absolute_offset: 0,
            terminator_len: 1,
        }
    }

    #[test]
    fn a_definition_outranks_scattered_uses_and_order_is_total() {
        let caps = vec![(
            PathDisplay::Bare,
            vec![
                Captured::Match(m("b/uses.py", 3, "  x.bulk_update(a)", "bulk_update")),
                Captured::Match(m(
                    "a/query.py",
                    10,
                    "    def bulk_update(self):",
                    "bulk_update",
                )),
                Captured::Match(m("c/other.py", 1, "  y.bulk_update(b)", "bulk_update")),
            ],
        )];
        let files = group(caps);
        let ranked = score_files(
            &files,
            &PatternSet::new(&["bulk_update".into()], true, false),
            &Weights::default(),
        );
        assert_eq!(ranked[0].path, "a/query.py");
        // Equal scores fall back to path order.
        assert_eq!(ranked[1].path, "b/uses.py");
        assert_eq!(ranked[2].path, "c/other.py");
    }
}
