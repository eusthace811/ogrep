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
}

impl Default for RankOptions {
    fn default() -> Self {
        Self {
            budget: 20,
            per_file: 3,
            cutoff: Cutoff::None,
            explain: false,
            weights: Weights::default(),
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
    const KEYWORDS: &[&str] = &[
        "fn",
        "def",
        "class",
        "struct",
        "enum",
        "trait",
        "type",
        "interface",
        "func",
        "function",
        "const",
        "let",
        "var",
        "val",
        "impl",
        "module",
        "mod",
        "macro_rules!",
        "#define",
        "union",
        "record",
        "object",
        "protocol",
        "extension",
        "typedef",
        "sub",
        "procedure",
        "method",
        "static",
        "async",
    ];
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
    KEYWORDS.contains(&word)
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
            for (i, m) in hits.matches.iter().enumerate() {
                let parsed_def = hits.defs.as_ref().and_then(|d| d.get(i).copied());
                let mut seen = vec![false; n];
                let mut def = vec![false; n];
                for &(s, e) in &m.spans {
                    let Some(text) = m.content.get(s..e) else {
                        continue;
                    };
                    let p = patterns.of(text);
                    seen[p] = true;
                    def[p] |= parsed_def.unwrap_or_else(|| is_definition(&m.content, s));
                    if text.len() >= 3 {
                        texts.push(text.to_ascii_lowercase());
                    }
                }
                if m.spans.is_empty() {
                    seen[0] = true;
                }
                for ((line, &seen), &def) in lines.iter_mut().zip(&seen).zip(&def) {
                    if seen {
                        if def {
                            line.0 += 1;
                        } else {
                            line.1 += 1;
                        }
                    }
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
    order.sort_by_key(|&(i, m)| {
        let def = defs
            .and_then(|d| d.get(i).copied())
            .unwrap_or_else(|| m.spans.iter().any(|&(s, _)| is_definition(&m.content, s)));
        (!def, m.line_number)
    });
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
    let pattern_set = PatternSet::new(patterns, fixed, ci);
    let ranked = score_files(&files, &pattern_set, &opts.weights);
    let keep = keep_count(&ranked, opts);
    for (rank, r) in ranked.iter().take(keep).enumerate() {
        let hits = &files[r.file].1;
        writer.set_path_display(hits.display.clone());
        writer.write_rank_record(&r.path, rank + 1, r.score, &r.features, opts.explain)?;
        let shown = select_lines(&hits.matches, hits.defs.as_deref(), opts.per_file);
        crate::post::emit_file(&r.path, hits, &shown, &[], search, writer)?;
    }
    Ok(keep > 0)
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
