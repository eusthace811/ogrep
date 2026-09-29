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

use std::collections::{BTreeMap, HashMap};

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
const K1: f64 = 1.0;
const W_DEF: f64 = 10.0;
const W_BODY: f64 = 1.0;
const W_PATH: f64 = 5.0;
const PRIOR_TEST: f64 = 0.3;
const PRIOR_VENDORED: f64 = 0.3;
const PRIOR_PROSE: f64 = 0.8;

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
}

impl Default for RankOptions {
    fn default() -> Self {
        Self {
            budget: 20,
            per_file: 3,
            cutoff: Cutoff::None,
            explain: false,
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
    pub path: String,
    pub score: f64,
    pub features: Features,
}

/// Group captured results by file, in capture order.
pub(crate) fn group(captures: Vec<(PathDisplay, Vec<Captured>)>) -> Vec<(String, FileHits)> {
    let mut order: Vec<String> = Vec::new();
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
                order.push(key.clone());
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
        .map(|k| {
            let hits = files.remove(&k).unwrap();
            let rel = hits
                .matches
                .first()
                .map(|m| m.file.clone())
                .or_else(|| hits.contexts.first().map(|c| c.file.clone()))
                .unwrap_or_default();
            (rel, hits)
        })
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

fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    p.starts_with("test/")
        || p.starts_with("tests/")
        || p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/testing/")
        || p.contains("/spec/")
        || p.contains("/__tests__/")
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
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

fn prior(path: &str) -> f64 {
    if is_vendored_path(path) {
        PRIOR_VENDORED
    } else if is_test_path(path) {
        PRIOR_TEST
    } else if is_prose_path(path) {
        PRIOR_PROSE
    } else {
        1.0
    }
}

fn round(score: f64) -> i64 {
    (score * 1e6).round() as i64
}

fn saturate(tf: f64) -> f64 {
    tf * (K1 + 1.0) / (tf + K1)
}

/// Which pattern produced a match text. With one pattern it is always 0.
struct PatternSet {
    regexes: Vec<regex::Regex>,
}

impl PatternSet {
    fn new(patterns: &[String], fixed: bool, ci: bool) -> Self {
        let regexes = if patterns.len() < 2 {
            Vec::new()
        } else {
            patterns
                .iter()
                .filter_map(|p| {
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
            .position(|re| re.is_match(text))
            .unwrap_or(0)
    }
}

/// Score and order grouped files.
fn score_files(files: &[(String, FileHits)], patterns: &PatternSet) -> Vec<Ranked> {
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
        .map(|((rel, _), tally)| {
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
                let tf = W_DEF * d as f64 + W_BODY * b as f64 + W_PATH * path;
                score += weight * saturate(tf);
            }
            let prior = prior(rel);
            let features = Features {
                def_lines: tally.lines.iter().map(|l| l.0).sum(),
                body_lines: tally.lines.iter().map(|l| l.1).sum(),
                path,
                prior,
                patterns: tally.lines.iter().filter(|l| l.0 + l.1 > 0).count(),
            };
            Ranked {
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
fn select_lines<'a>(
    matches: &'a [Match],
    defs: Option<&[bool]>,
    per_file: usize,
) -> Vec<&'a Match> {
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
    let order: Vec<&Match> = order.into_iter().map(|(_, m)| m).collect();
    let mut seen = std::collections::HashSet::new();
    let mut picked: Vec<&Match> = Vec::new();
    for m in order {
        if picked.len() >= per_file {
            break;
        }
        if seen.insert(norm(&m.content)) {
            picked.push(m);
        }
    }
    picked.sort_by_key(|m| m.line_number);
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
    context_reach: usize,
    writer: &mut OutputWriter,
) -> Result<bool> {
    if files.is_empty() {
        return Ok(false);
    }
    let pattern_set = PatternSet::new(patterns, fixed, ci);
    let ranked = score_files(&files, &pattern_set);
    let keep = keep_count(&ranked, opts);
    let by_path: BTreeMap<&str, &FileHits> = files
        .iter()
        .map(|(rel, hits)| (rel.as_str(), hits))
        .collect();
    for (rank, r) in ranked.iter().take(keep).enumerate() {
        let hits = by_path[r.path.as_str()];
        writer.set_path_display(hits.display.clone());
        writer.write_rank_record(&r.path, rank + 1, r.score, &r.features, opts.explain)?;
        if let Some(offset) = hits.binary {
            writer.write_binary_note(&r.path, offset)?;
        }
        if writer.is_files_only() {
            writer.write_file(&r.path)?;
            continue;
        }
        if writer.is_count() {
            writer.write_count(&r.path, hits.matches.len())?;
            continue;
        }
        let shown = select_lines(&hits.matches, hits.defs.as_deref(), opts.per_file);
        let shown_lines: std::collections::HashSet<usize> =
            shown.iter().map(|m| m.line_number).collect();
        // Context lines are kept only around the shown matches.
        let mut rows: Vec<(usize, Result<&Match, &ContextLine>)> =
            shown.iter().map(|m| (m.line_number, Ok(*m))).collect();
        if !hits.contexts.is_empty() {
            let near = |line: usize| {
                shown_lines
                    .iter()
                    .any(|&s| line.abs_diff(s) <= context_reach)
            };
            rows.extend(
                hits.contexts
                    .iter()
                    .filter(|c| near(c.line_number) && !shown_lines.contains(&c.line_number))
                    .map(|c| (c.line_number, Err(c))),
            );
            rows.sort_by_key(|(line, _)| *line);
        }
        for (line, row) in rows {
            writer.write_context_separator(&r.path, line)?;
            match row {
                Ok(m) => writer.write_match(m)?,
                Err(c) => writer.write_context_line(c)?,
            }
        }
        writer.flush_file()?;
    }
    Ok(keep > 0)
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
        );
        assert_eq!(ranked[0].path, "a/query.py");
        // Equal scores fall back to path order.
        assert_eq!(ranked[1].path, "b/uses.py");
        assert_eq!(ranked[2].path, "c/other.py");
    }
}
