//! `--cards`: print each ranked file as a compact card instead of its lines.
//!
//! A card is what a reader (a person, an agent, a model choosing among
//! files) needs to decide whether a file is the one: its path, a one-line
//! summary from the file's own words, its outline (definitions, or headings
//! for prose) with the definitions around the matches first, and the matching
//! lines, all in line order and within a token budget. Everything comes from
//! the file's text, so a card is exact, costs nothing to build and is never
//! stale; only the files ranking keeps are read.

use std::path::Path;

/// One outline or match line of a card.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CardLine {
    pub line: usize,
    /// Nesting depth, for indentation: 0 for top-level definitions and
    /// first-level headings.
    pub depth: usize,
    pub text: String,
    /// Whether this is a matching line rather than an outline entry.
    pub matched: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Card {
    pub summary: Option<String>,
    pub lines: Vec<CardLine>,
}

/// Outline entries kept per card, and the longest line shown.
const MAX_OUTLINE: usize = 12;
const MAX_TEXT: usize = 120;

impl Card {
    /// Approximate tokens the card costs to print (4 characters a token).
    pub fn tokens(&self, path: &str) -> usize {
        let chars = path.len()
            + self.summary.as_ref().map_or(0, |s| s.len() + 3)
            + self
                .lines
                .iter()
                .map(|l| l.text.len() + 10 + 2 * l.depth)
                .sum::<usize>();
        chars.div_ceil(4)
    }
}

fn clip(text: &str) -> String {
    let t = text.trim();
    match t.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.to_string(),
    }
}

pub fn is_prose(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".md", ".markdown", ".rst", ".adoc", ".txt"]
        .iter()
        .any(|e| p.ends_with(e))
}

/// Build the card for one file from its text and the line numbers of the
/// matches shown.
pub fn build(path: &str, text: &str, matches: &[(usize, String)]) -> Card {
    let lines: Vec<&str> = text.lines().collect();
    let (summary, outline) = if is_prose(path) {
        (prose_summary(&lines), headings(path, &lines))
    } else {
        (code_summary(&lines), definitions(path, text, &lines))
    };
    let matched: Vec<usize> = matches.iter().map(|(l, _)| *l).collect();
    // A match sits one level inside the innermost entry enclosing it, or at
    // that entry's own level when it is the entry's first line.
    let depth_at = |line: usize| {
        outline
            .iter()
            .filter(|e| e.0 <= line && line <= e.3)
            .max_by_key(|e| (e.1, e.0))
            .map_or(0, |e| if e.0 == line { e.1 } else { e.1 + 1 })
    };
    let depths: Vec<usize> = matched.iter().map(|&l| depth_at(l)).collect();
    let mut out: Vec<CardLine> = choose_outline(outline, &matched);
    for ((line, text), depth) in matches.iter().zip(depths) {
        // A matching line that is itself an outline entry is shown once, as a
        // match.
        out.retain(|o| o.line != *line);
        out.push(CardLine {
            line: *line,
            depth,
            text: clip(text),
            matched: true,
        });
    }
    out.sort_by_key(|l| (l.line, l.matched));
    Card {
        summary,
        lines: out,
    }
}

/// Outline entries: (line, depth, text, end line). The end line is the last
/// line the entry covers, so the entries enclosing a match can be told apart.
type Entry = (usize, usize, String, usize);

/// Keep at most [`MAX_OUTLINE`] entries: those enclosing a match first (the
/// innermost last), then top-level ones, then the rest, each group by line.
fn choose_outline(entries: Vec<Entry>, matched: &[usize]) -> Vec<CardLine> {
    let encloses = |e: &Entry| matched.iter().any(|&m| e.0 <= m && m <= e.3);
    let mut order: Vec<(u8, usize, &Entry)> = entries
        .iter()
        .map(|e| {
            let group = if encloses(e) {
                0
            } else if e.1 == 0 {
                1
            } else {
                2
            };
            (group, e.0, e)
        })
        .collect();
    order.sort_by_key(|&(g, line, _)| (g, line));
    order.truncate(MAX_OUTLINE);
    let mut kept: Vec<CardLine> = order
        .into_iter()
        .map(|(_, _, e)| CardLine {
            line: e.0,
            depth: e.1,
            text: clip(&e.2),
            matched: false,
        })
        .collect();
    kept.sort_by_key(|l| l.line);
    kept
}

/// Definitions from the parsed structure when the language has a grammar,
/// else from definition keywords at the start of a line.
fn definitions(path: &str, text: &str, lines: &[&str]) -> Vec<Entry> {
    #[cfg(feature = "structure")]
    if let Some(s) = ogrep_core::structure::analyze(path, text.as_bytes()) {
        return s
            .symbols
            .iter()
            .map(|sym| {
                let mut depth = 0;
                let mut parent = sym.parent;
                while let Some(p) = parent {
                    depth += 1;
                    parent = s.symbols.get(p).and_then(|x| x.parent);
                }
                let line = sym.start_line;
                let source = lines.get(line.saturating_sub(1)).copied().unwrap_or("");
                (
                    line,
                    depth.min(4),
                    source.to_string(),
                    sym.end_line.max(line),
                )
            })
            .collect();
    }
    let _ = (path, text);
    keyword_definitions(lines)
}

const DEF_STARTS: &[&str] = &[
    "def ",
    "async def ",
    "class ",
    "fn ",
    "pub fn ",
    "pub(crate) fn ",
    "async fn ",
    "pub async fn ",
    "struct ",
    "pub struct ",
    "enum ",
    "pub enum ",
    "trait ",
    "pub trait ",
    "impl ",
    "impl<",
    "func ",
    "function ",
    "export function ",
    "export class ",
    "export default function ",
    "interface ",
    "export interface ",
    "type ",
    "module ",
    "package ",
];

fn keyword_definitions(lines: &[&str]) -> Vec<Entry> {
    let defs: Vec<(usize, usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let trimmed = l.trim_start();
            DEF_STARTS.iter().any(|k| trimmed.starts_with(k)).then(|| {
                let indent = l.len() - trimmed.len();
                (i + 1, (indent / 4).min(4), (*l).to_string())
            })
        })
        .collect();
    // A definition covers the lines up to the next one at its depth or
    // shallower.
    defs.iter()
        .enumerate()
        .map(|(i, (line, depth, text))| {
            let end = defs[i + 1..]
                .iter()
                .find(|d| d.1 <= *depth)
                .map_or(lines.len(), |d| d.0 - 1);
            (*line, *depth, text.clone(), end)
        })
        .collect()
}

/// Markdown `#` headings, AsciiDoc `=` headings and underlined titles
/// (reStructuredText, Markdown setext), with their depth, each covering the
/// lines up to the next heading at its depth or shallower. In a .txt file
/// (reStructuredText, or plain notes such as a CMakeLists.txt) a line starting
/// with `#` is a comment, not a heading.
fn headings(path: &str, lines: &[&str]) -> Vec<Entry> {
    let lower = path.to_ascii_lowercase();
    // The character repeated before a one-line heading's title, if any.
    let prefix = if lower.ends_with(".txt") {
        None
    } else if lower.ends_with(".adoc") {
        Some('=')
    } else {
        Some('#')
    };
    // ~~~ also opens a Markdown code fence; elsewhere it is a title underline.
    let fences: &[&str] = if lower.ends_with(".md") || lower.ends_with(".markdown") {
        &["```", "~~~"]
    } else {
        &["```"]
    };
    let mut found: Vec<(usize, usize, String)> = Vec::new();
    let mut rst_levels: Vec<char> = Vec::new();
    // The open code fence: its lines are code.
    let mut fence: Option<&str> = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_end();
        if let Some(&f) = fences.iter().find(|f| t.trim_start().starts_with(*f)) {
            match fence {
                None => fence = Some(f),
                Some(open) if open == f => fence = None,
                Some(_) => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if let Some(p) = prefix {
            let level = t.chars().take_while(|&c| c == p).count();
            if (1..=6).contains(&level) && t[level..].starts_with(' ') {
                found.push((i + 1, level - 1, t.to_string()));
                continue;
            }
        }
        // reStructuredText: a title line followed by an underline of one
        // punctuation character at least as long.
        if let Some(next) = lines.get(i + 1) {
            let n = next.trim_end();
            let c = n.chars().next().unwrap_or(' ');
            if !t.trim().is_empty()
                && !t.starts_with(' ')
                && "=-~^*+#\"'`".contains(c)
                && n.chars().count() >= t.chars().count()
                && n.len() >= 3
                && n.chars().all(|x| x == c)
            {
                let depth = match rst_levels.iter().position(|&x| x == c) {
                    Some(d) => d,
                    None => {
                        rst_levels.push(c);
                        rst_levels.len() - 1
                    }
                };
                found.push((i + 1, depth.min(4), t.to_string()));
            }
        }
    }
    found
        .iter()
        .enumerate()
        .map(|(i, (line, depth, text))| {
            let end = found[i + 1..]
                .iter()
                .find(|h| h.1 <= *depth)
                .map_or(lines.len(), |h| h.0 - 1);
            (*line, *depth, text.clone(), end)
        })
        .collect()
}

/// A heading-delimited part of a prose file: a heading and its own text, up
/// to the next heading at any depth. `--about` scores prose by its best
/// section, so a long page is not diluted by everything else it covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// The heading's line; 1 for the text before the first heading.
    pub line: usize,
    /// The section's last line.
    pub end: usize,
    /// The headings enclosing the section, outermost first and ending with
    /// its own: (line, last line covered with subsections, title). Empty for
    /// the text before the first heading.
    pub chain: Vec<(usize, usize, String)>,
    /// The section's size in bytes.
    pub bytes: usize,
}

/// A prose file's sections, in order, when it is prose with at least one
/// heading; empty otherwise.
pub fn sections(path: &str, text: &str) -> Vec<Section> {
    if !is_prose(path) {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let heads = headings(path, &lines);
    if heads.is_empty() {
        return Vec::new();
    }
    // Byte offset of each line start, plus the end of the text.
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut offset = 0;
    for l in text.split_inclusive('\n') {
        starts.push(offset);
        offset += l.len();
    }
    starts.push(offset);
    let span_bytes = |first: usize, last: usize| {
        starts.get(last).copied().unwrap_or(offset)
            - starts.get(first - 1).copied().unwrap_or(offset)
    };
    let title = |t: &str| t.trim().trim_start_matches(['#', '=']).trim().to_string();
    let mut out = Vec::with_capacity(heads.len() + 1);
    if heads[0].0 > 1 {
        out.push(Section {
            line: 1,
            end: heads[0].0 - 1,
            chain: Vec::new(),
            bytes: span_bytes(1, heads[0].0 - 1),
        });
    }
    let mut stack: Vec<(usize, usize, usize, String)> = Vec::new();
    for (k, (line, depth, text, covered)) in heads.iter().enumerate() {
        while stack.last().is_some_and(|s| s.1 >= *depth) {
            stack.pop();
        }
        stack.push((*line, *depth, *covered, title(text)));
        let end = heads.get(k + 1).map_or(lines.len(), |h| h.0 - 1).max(*line);
        out.push(Section {
            line: *line,
            end,
            chain: stack.iter().map(|s| (s.0, s.2, s.3.clone())).collect(),
            bytes: span_bytes(*line, end),
        });
    }
    out
}

/// The first line of a module docstring or leading comment that reads as a
/// sentence (three words or more), skipping shebangs, licence headers and
/// editor modelines.
fn code_summary(lines: &[&str]) -> Option<String> {
    // Inside a docstring or block comment opened on an earlier line, every
    // line is comment text.
    let mut in_block = false;
    for l in lines.iter().take(40) {
        let t = l.trim();
        if t.is_empty() || t.starts_with("#!") {
            continue;
        }
        let was_in_block = in_block;
        if was_in_block {
            in_block = !(t.contains("\"\"\"") || t.contains("'''") || t.contains("*/"));
        } else if let Some(open) = ["\"\"\"", "'''", "/*"]
            .into_iter()
            .find(|o| t.starts_with(o))
        {
            let close = if open == "/*" { "*/" } else { open };
            // Left open on this line: the following lines are its text.
            in_block = !t[open.len()..].contains(close);
        }
        let lower = t.to_ascii_lowercase();
        if [
            "copyright",
            "license",
            "licence",
            "spdx",
            "-*-",
            "@generated",
            "eslint",
            "pylint",
        ]
        .iter()
        .any(|w| lower.contains(w))
        {
            continue;
        }
        let body = t
            .trim_start_matches(['/', '#', '*', '!', '-', ';'])
            .trim_start_matches("\"\"\"")
            .trim_start_matches("'''")
            .trim_end_matches("\"\"\"")
            .trim_end_matches("'''")
            .trim_end_matches("*/")
            .trim();
        let comment = was_in_block
            || t.starts_with("//")
            || t.starts_with('#')
            || t.starts_with("/*")
            || t.starts_with('*')
            || t.starts_with("\"\"\"")
            || t.starts_with("'''")
            || t.starts_with("--")
            || t.starts_with(';');
        if !comment {
            // Code started before any summary.
            if !(t.starts_with("package ") || t.starts_with("import ") || t.starts_with("use ")) {
                return None;
            }
            continue;
        }
        if body.split_whitespace().count() >= 3 {
            return Some(clip(body));
        }
    }
    None
}

/// A prose file's first heading, or else its first non-empty line.
fn prose_summary(lines: &[&str]) -> Option<String> {
    lines
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.chars().all(|c| "=-~^*+#\"'`".contains(c)))
        .map(|l| clip(l.trim_start_matches(['#', '=']).trim()))
}

/// Read a file for its card. `None` for unreadable or non-UTF-8 files, which
/// keep the ordinary output.
pub fn read(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_card_has_summary_outline_and_marked_matches_in_line_order() {
        let text = "\"\"\"Admin change lists.\"\"\"\n\nclass ChangeList:\n    def get_filters(self):\n        pass\n\n    def date_hierarchy(self, year):\n        if year > 9999:\n            raise ValueError\n\ndef other():\n    pass\n";
        let card = build(
            "admin/main.py",
            text,
            &[(8, "        if year > 9999:".into())],
        );
        assert_eq!(card.summary.as_deref(), Some("Admin change lists."));
        let shown: Vec<(usize, bool)> = card.lines.iter().map(|l| (l.line, l.matched)).collect();
        assert!(shown.contains(&(3, false)), "{shown:?}");
        assert!(
            shown.contains(&(7, false)),
            "the enclosing method: {shown:?}"
        );
        assert!(shown.contains(&(8, true)), "{shown:?}");
        let lines: Vec<usize> = card.lines.iter().map(|l| l.line).collect();
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted);
    }

    #[test]
    fn a_prose_card_outlines_markdown_and_rst_headings() {
        let md = "# Runbook\n\nintro\n\n## Pool exhausted\n\nsteps\n\n```\n# not a heading\n~~~\n```\n~~~sh\n# nor this\n~~~\n## Rollback\n";
        let card = build("ops/runbook.md", md, &[(7, "steps".into())]);
        assert_eq!(card.summary.as_deref(), Some("Runbook"));
        let heads: Vec<(usize, usize)> = card
            .lines
            .iter()
            .filter(|l| !l.matched)
            .map(|l| (l.line, l.depth))
            .collect();
        assert_eq!(heads, [(1, 0), (5, 1), (16, 1)]);
        let rst = "Title\n=====\n\nSection\n-------\n\nSub\n~~~\n\nÉtude\n=====\n";
        let card = build("docs/x.rst", rst, &[]);
        let heads: Vec<(usize, usize)> = card.lines.iter().map(|l| (l.line, l.depth)).collect();
        assert_eq!(heads, [(1, 0), (4, 1), (7, 2), (10, 0)]);
    }

    #[test]
    fn prose_splits_into_heading_sections_with_their_chains() {
        let md = "intro\n# Guide\ntext\n## Install\nsteps\n## Upgrade\nmore\n";
        let secs = sections("docs/guide.md", md);
        let spans: Vec<(usize, usize)> = secs.iter().map(|s| (s.line, s.end)).collect();
        assert_eq!(spans, [(1, 1), (2, 3), (4, 5), (6, 7)]);
        assert!(secs[0].chain.is_empty());
        assert_eq!(
            secs[2].chain,
            [(2, 7, "Guide".to_string()), (4, 5, "Install".to_string())]
        );
        assert_eq!(secs[2].bytes, "## Install\nsteps\n".len());
        // Code has no sections; in a .txt file a `#` line is a comment.
        assert!(sections("src/guide.py", md).is_empty());
        assert!(sections("CMakeLists.txt", "# build\nproject(x)\n").is_empty());
        let secs = sections("notes.txt", "Title\n=====\n# a comment\ntext\n");
        assert_eq!(secs.len(), 1);
        assert_eq!(secs[0].chain, [(1, 4, "Title".to_string())]);
        let rst = "Title\n=====\n\nSection\n-------\ntext\n";
        let adoc = "= Guide\n\n== Install\nsteps\n=== Linux\napt\n";
        let secs = sections("docs/guide.adoc", adoc);
        let spans: Vec<(usize, usize)> = secs.iter().map(|s| (s.line, s.end)).collect();
        assert_eq!(spans, [(1, 2), (3, 4), (5, 6)]);
        assert_eq!(
            secs[2]
                .chain
                .iter()
                .map(|c| c.2.as_str())
                .collect::<Vec<_>>(),
            ["Guide", "Install", "Linux"]
        );
        let secs = sections("docs/x.txt", rst);
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[1].chain.last().unwrap().2, "Section");
    }

    #[test]
    fn a_docstring_opened_on_its_own_line_is_a_summary() {
        let text = "\"\"\"\nThe main QuerySet implementation. This provides the public API.\n\"\"\"\n\nimport x\n";
        assert_eq!(
            code_summary(&text.lines().collect::<Vec<_>>()).as_deref(),
            Some("The main QuerySet implementation. This provides the public API.")
        );
        let c = "/*\n * Hash table with open addressing.\n */\nint x;\n";
        assert_eq!(
            code_summary(&c.lines().collect::<Vec<_>>()).as_deref(),
            Some("Hash table with open addressing.")
        );
    }

    #[test]
    fn matches_are_indented_under_their_definition() {
        let text = "class A:\n    def f(self):\n        x = 1\n";
        let card = build("a.py", text, &[(3, "        x = 1".into())]);
        let m = card.lines.iter().find(|l| l.matched).unwrap();
        assert_eq!(m.depth, 2, "{:?}", card.lines);
    }

    #[test]
    fn licence_headers_are_not_summaries() {
        let text = "// Copyright 2026 Example\n// SPDX-License-Identifier: MIT\n//! Parses request headers into a typed map.\nfn x() {}\n";
        assert_eq!(
            code_summary(&text.lines().collect::<Vec<_>>()).as_deref(),
            Some("Parses request headers into a typed map.")
        );
    }
}
