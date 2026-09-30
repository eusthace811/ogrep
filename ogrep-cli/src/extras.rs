//! `--related-tests`, `--similar`, `--conflicts` and `--doc-drift`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::search::SearchOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// With the number of tests to list (`--budget`).
    RelatedTests {
        budget: usize,
    },
    /// With the number of regions to list (`--budget`).
    Similar {
        budget: usize,
    },
    Conflicts,
    DocDrift,
}

/// The repository containing `root`.
pub(crate) fn repo_of(root: &Path) -> Result<PathBuf> {
    ogrep_core::history::repo_root(root).map_err(|e| anyhow!("this needs a git repository: {e}"))
}

use ogrep_core::history::git_output;

pub fn run(
    root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
    mode: Mode,
) -> Result<bool> {
    let repo = repo_of(root)?;
    let idx = crate::search::index_dir_for(&repo, index_path);
    let mut out = BufWriter::new(std::io::stdout().lock());
    let found = match mode {
        Mode::RelatedTests { budget } => related_tests(&repo, &idx, opts, budget, &mut out)?,
        Mode::Similar { budget } => similar(&repo, &idx, opts, budget, &mut out)?,
        Mode::Conflicts => conflicts(&repo, opts, &mut out)?,
        Mode::DocDrift => doc_drift(&repo, &idx, opts, ci, &mut out)?,
    };
    out.flush()?;
    Ok(found)
}

/// A path relative to the repository root, from an argument given relative to
/// the current directory.
fn repo_relative(repo: &Path, arg: &str) -> String {
    let full = std::fs::canonicalize(arg).unwrap_or_else(|_| PathBuf::from(arg));
    let repo = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    full.strip_prefix(&repo)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| arg.trim_start_matches("./").to_string())
}

/// A file name without extensions (`query` for `query.test.ts`); a dotfile
/// keeps its name (`.eslintrc` for `.eslintrc.js`), never an empty stem.
fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let dot = name.char_indices().skip(1).find(|&(_, c)| c == '.');
    dot.map_or(name, |(i, _)| &name[..i])
}

// ---------------------------------------------------------------- tests

fn related_tests(
    repo: &Path,
    idx: &Path,
    opts: &SearchOptions,
    budget: usize,
    out: &mut impl Write,
) -> Result<bool> {
    let target = repo_relative(repo, &opts.pattern);
    let files = ogrep_core::history::ls_files(repo, false)?;
    if !files.contains(&target) {
        bail!("{target} is not a tracked file");
    }
    let s = stem(&target).to_ascii_lowercase();
    let mut score: BTreeMap<String, (f64, Vec<String>)> = BTreeMap::new();
    let mut add = |path: &str, points: f64, why: String| {
        let e = score.entry(path.to_string()).or_insert((0.0, Vec::new()));
        e.0 += points;
        e.1.push(why);
    };
    // Names: test_query.py, query_test.go, query.test.ts, QueryTests.java.
    for f in files
        .iter()
        .filter(|f| crate::rank::is_test_path(f) && **f != target)
    {
        let t = stem(f).to_ascii_lowercase();
        let exact = [
            format!("test_{s}"),
            format!("{s}_test"),
            format!("{s}_tests"),
            format!("{s}test"),
            format!("{s}tests"),
            format!("test{s}"),
            s.clone(),
        ];
        if exact.contains(&t) {
            add(f, 3.0, "named after it".into());
        } else if s.len() >= 4 && t.contains(&s) {
            add(f, 1.5, "name mentions it".into());
        }
    }
    // The graph: test files that use names this file defines.
    if let Ok((g, _)) = ogrep_core::graph::open(idx)
        && let Some(file) = g.file(&target)
    {
        let mut links: Vec<(String, f64)> = Vec::new();
        for a in 0..g.files.len() as u32 {
            if let Some((_, w)) = g.out_links(a).find(|(t, _)| *t == file) {
                let path = &g.files[a as usize];
                if crate::rank::is_test_path(path) {
                    links.push((path.clone(), w as f64));
                }
            }
        }
        let top = links.iter().map(|l| l.1).fold(1e-9, f64::max);
        for (path, w) in links {
            add(&path, 3.0 * w / top, "uses its definitions".into());
        }
    }
    // History: test files changed in the same commits.
    // `--full-diff`: every file of the commits that touched the target, not
    // just the target.
    let log = git_output(
        repo,
        &[
            "log",
            "--no-merges",
            "--format=format:@@",
            "--name-only",
            "--full-diff",
            "-n",
            "400",
            "--",
            &target,
        ],
    )
    .unwrap_or_default();
    let mut co: HashMap<String, usize> = HashMap::new();
    for block in log.split("@@") {
        for f in block.lines().map(str::trim).filter(|f| !f.is_empty()) {
            if f != target && crate::rank::is_test_path(f) {
                *co.entry(f.to_string()).or_default() += 1;
            }
        }
    }
    for (path, n) in co {
        if files.contains(&path) {
            add(
                &path,
                1.5 * (1.0 + n as f64).ln(),
                format!("changed with it {n}x"),
            );
        }
    }
    let mut ranked: Vec<(String, f64, Vec<String>)> =
        score.into_iter().map(|(p, (s, w))| (p, s, w)).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    for (path, s, why) in ranked.iter().take(budget) {
        if opts.json {
            let msg =
                serde_json::json!({"type": "test", "data": {"path": path, "score": s, "why": why}});
            writeln!(out, "{msg}")?;
        } else {
            writeln!(out, "{s:5.2} {path}  ({})", why.join("; "))?;
        }
    }
    Ok(!ranked.is_empty())
}

// ---------------------------------------------------------------- similar

/// Token 5-shingles of a code region, as hashes.
fn shingles(text: &str) -> HashSet<u64> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace())
        .flat_map(|w| w.split_inclusive(|c: char| !(c.is_alphanumeric() || c == '_')))
        .filter(|t| !t.is_empty())
        .collect();
    tokens
        .windows(5)
        .map(|w| {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for t in w {
                for b in t.bytes() {
                    h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
                }
                h = (h ^ 0xff).wrapping_mul(0x100_0000_01b3);
            }
            h
        })
        .collect()
}

fn jaccard(a: &HashSet<u64>, b: &HashSet<u64>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    inter as f64 / (a.len() + b.len() - inter) as f64
}

/// Regions of a file to compare: its definitions when parsed, otherwise
/// windows of `lines` lines.
fn regions(path: &str, text: &str, lines: usize) -> Vec<(usize, usize, String)> {
    #[cfg(feature = "structure")]
    if let Some(fs) = ogrep_core::structure::analyze(path, text.as_bytes()) {
        let out: Vec<(usize, usize, String)> = fs
            .symbols
            .iter()
            .filter(|s| s.end_line > s.start_line)
            .map(|s| (s.start_line, s.end_line, s.name.clone()))
            .collect();
        if !out.is_empty() {
            return out;
        }
    }
    let _ = path;
    let total = text.lines().count();
    let step = (lines / 2).max(1);
    (0..total)
        .step_by(step)
        .map(|s| (s + 1, (s + lines).min(total), String::new()))
        .collect()
}

fn slice_lines(text: &str, start: usize, end: usize) -> String {
    text.lines()
        .skip(start.saturating_sub(1))
        .take(end + 1 - start.min(end + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn similar(
    repo: &Path,
    idx: &Path,
    opts: &SearchOptions,
    budget: usize,
    out: &mut impl Write,
) -> Result<bool> {
    let (file_arg, line) = opts
        .pattern
        .rsplit_once(':')
        .and_then(|(f, l)| Some((f, l.parse::<usize>().ok()?)))
        .ok_or_else(|| anyhow!("--similar takes path:line"))?;
    let target = repo_relative(repo, file_arg);
    let text =
        std::fs::read_to_string(repo.join(&target)).with_context(|| format!("reading {target}"))?;
    // The region: the innermost definition around the line, or 30 lines.
    let all = regions(&target, &text, 30);
    let (start, end, name) = all
        .iter()
        .filter(|(s, e, _)| *s <= line && line <= *e)
        .min_by_key(|(s, e, _)| e - s)
        .cloned()
        .unwrap_or((line.saturating_sub(15).max(1), line + 15, String::new()));
    let snippet = slice_lines(&text, start, end);
    let want = shingles(&snippet);
    if want.is_empty() {
        bail!("the region at {target}:{line} is too short to compare");
    }
    // Candidates: files containing several of the region's longest
    // identifiers, from the index when there is one.
    let mut idents: Vec<&str> = snippet
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.len() >= 6)
        .collect();
    idents.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    idents.dedup();
    idents.truncate(8);
    let reader = ogrep_core::reader::IndexReader::open(idx).ok();
    let mut hits: HashMap<String, usize> = HashMap::new();
    match &reader {
        Some(r) => {
            for id in &idents {
                let plan = ogrep_core::query::build_literal_plan(id, false);
                let mut work = ogrep_core::query::QueryWork::default();
                for f in ogrep_core::query::execute_plan_indexed(&plan, r, &mut work) {
                    if let Some(p) = r.file_path(f) {
                        *hits.entry(p.to_string()).or_default() += 1;
                    }
                }
            }
        }
        None => {
            for id in &idents {
                for p in git_output(repo, &["grep", "-l", "-F", "-I", "--", id])
                    .unwrap_or_default()
                    .lines()
                {
                    *hits.entry(p.to_string()).or_default() += 1;
                }
            }
        }
    }
    let need = idents.len().clamp(1, 2);
    // Most shared identifiers first (then by path, for a stable order), so the
    // cap below keeps the likeliest near-duplicates, not the first by name.
    let mut candidates: Vec<(usize, String)> = hits
        .into_iter()
        .filter(|(_, n)| *n >= need)
        .map(|(p, n)| (n, p))
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let candidates: Vec<String> = candidates.into_iter().map(|(_, p)| p).collect();
    let span = end + 1 - start;
    let mut found: Vec<(f64, String, usize, usize, String)> = Vec::new();
    for path in candidates.iter().take(2000) {
        let Ok(t) = std::fs::read_to_string(repo.join(path)) else {
            continue;
        };
        let mut best: Option<(f64, usize, usize, String)> = None;
        for (s, e, n) in regions(path, &t, span) {
            if path == &target && s <= end && start <= e {
                continue;
            }
            let sim = jaccard(&want, &shingles(&slice_lines(&t, s, e)));
            if best.as_ref().is_none_or(|b| sim > b.0) {
                best = Some((sim, s, e, n));
            }
        }
        if let Some((sim, s, e, n)) = best
            && sim >= 0.2
        {
            found.push((sim, path.clone(), s, e, n));
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    if !opts.json {
        let label = if name.is_empty() {
            String::new()
        } else {
            format!(" ({name})")
        };
        writeln!(out, "similar to {target}:{start}-{end}{label}")?;
    }
    for (sim, path, s, e, n) in found.iter().take(budget) {
        if opts.json {
            let msg = serde_json::json!({"type": "similar", "data": {"path": path, "start_line": s, "end_line": e, "name": n, "similarity": sim}});
            writeln!(out, "{msg}")?;
        } else {
            let n = if n.is_empty() {
                String::new()
            } else {
                format!("  {n}")
            };
            writeln!(out, "{sim:.2} {path}:{s}-{e}{n}")?;
        }
    }
    Ok(!found.is_empty())
}

// ---------------------------------------------------------------- conflicts

/// Changed line ranges per file between two points, in the old side's lines.
fn hunks(repo: &Path, from: &str, to: Option<&str>) -> Result<BTreeMap<String, Vec<(u32, u32)>>> {
    let mut args = vec![
        "diff",
        "--unified=0",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        // Fixed prefixes: diff.mnemonicPrefix would label a commit-to-worktree
        // diff differently from a commit-to-commit one, and paths must match.
        "--src-prefix=a/",
        "--dst-prefix=b/",
        from,
    ];
    if let Some(to) = to {
        args.push(to);
    }
    let diff = git_output(repo, &args)?;
    let mut out: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    let mut file = String::new();
    // `---`/`+++` are headers only between `diff --git` and the file's first
    // hunk; inside a hunk they are removed or added lines ("-- comment").
    let mut in_header = false;
    let path = |raw: &str, side: &[u8]| {
        ogrep_core::history::header_path(raw.as_bytes(), side).unwrap_or_default()
    };
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            file.clear();
            in_header = true;
        } else if in_header && let Some(raw) = line.strip_prefix("--- ") {
            // The old path names the file; a new file's is /dev/null, so its
            // new path does.
            file = path(raw, b"a/");
        } else if in_header
            && let Some(raw) = line.strip_prefix("+++ ")
            && file.is_empty()
        {
            file = path(raw, b"b/");
        } else if let Some(rest) = line.strip_prefix("@@ -") {
            in_header = false;
            let old = rest.split(' ').next().unwrap_or("0");
            let (s, n) = old.split_once(',').map_or((old, "1"), |(s, n)| (s, n));
            let (s, n): (u32, u32) = (s.parse().unwrap_or(0), n.parse().unwrap_or(1));
            // An insertion (n = 0) sits after line s: line 0 at the top.
            let start = s.max(1);
            let end = (s + n.max(1) - 1).max(start);
            if !file.is_empty() {
                out.entry(file.clone()).or_default().push((start, end));
            }
        }
    }
    Ok(out)
}

fn conflicts(repo: &Path, opts: &SearchOptions, out: &mut impl Write) -> Result<bool> {
    let current = git_output(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let branches: Vec<String> = git_output(
        repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?
    .lines()
    .map(String::from)
    .filter(|b| *b != current)
    .collect();
    let mut any = false;
    let mut ours_by_base: BTreeMap<String, BTreeMap<String, Vec<(u32, u32)>>> = BTreeMap::new();
    for branch in branches {
        let Ok(base) = git_output(repo, &["merge-base", "HEAD", &branch]) else {
            continue;
        };
        let base = base.trim().to_string();
        // Ours: committed since the base plus uncommitted edits. Branches
        // usually share a base, so it is computed once per base.
        if !ours_by_base.contains_key(&base) {
            ours_by_base.insert(base.clone(), hunks(repo, &base, None)?);
        }
        let ours = &ours_by_base[&base];
        let base = base.as_str();
        let theirs = hunks(repo, base, Some(&branch))?;
        for (file, theirs_ranges) in &theirs {
            let Some(ours_ranges) = ours.get(file) else {
                continue;
            };
            let overlapping: Vec<((u32, u32), (u32, u32))> = ours_ranges
                .iter()
                .flat_map(|a| {
                    theirs_ranges
                        .iter()
                        .filter(move |b| a.0 <= b.1 && b.0 <= a.1)
                        .map(move |b| (*a, *b))
                })
                .collect();
            any = true;
            let status = if overlapping.is_empty() {
                "same file"
            } else {
                "overlapping lines"
            };
            if opts.json {
                let msg = serde_json::json!({"type": "conflict", "data": {
                    "branch": branch, "path": file, "status": status,
                    "ours": ours_ranges, "theirs": theirs_ranges,
                }});
                writeln!(out, "{msg}")?;
            } else {
                let fmt = |r: &[(u32, u32)]| {
                    r.iter()
                        .map(|(s, e)| {
                            if s == e {
                                s.to_string()
                            } else {
                                format!("{s}-{e}")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                };
                writeln!(
                    out,
                    "{branch}: {file}  {status}  (here {}; there {})",
                    fmt(ours_ranges),
                    fmt(theirs_ranges)
                )?;
            }
        }
    }
    Ok(any)
}

// ---------------------------------------------------------------- doc drift

fn doc_drift(
    repo: &Path,
    idx: &Path,
    opts: &SearchOptions,
    ci: bool,
    out: &mut impl Write,
) -> Result<bool> {
    let files = ogrep_core::history::ls_files(repo, false)?;
    // Release notes and changelogs name removed things on purpose. A file
    // is one when a word of its name (CHANGELOG.md, HISTORY.rst,
    // release_notes_2.0.md, MIGRATING.md) or of a directory it sits in
    // (changelog.d/, .changes/, docs/release-notes/, upgrade-guides/) is one
    // of these. A keyword inside another word (src/exchanges/, prerelease.md)
    // does not count, and a directory named history is documentation about
    // the past, not notes.
    const NOTES: &[&str] = &[
        "release",
        "releases",
        "releasenotes",
        "changelog",
        "changelogs",
        "changes",
        "changeset",
        "news",
        "newsfragments",
        "deprecation",
        "deprecations",
        "upgrade",
        "upgrades",
        "upgrading",
        "migrate",
        "migrating",
        "migration",
        "migrations",
    ];
    let historical = |f: &str| {
        fn words(s: &str) -> impl Iterator<Item = &str> {
            s.split(|c: char| !c.is_ascii_alphanumeric())
        }
        let l = f.to_ascii_lowercase();
        let (dirs, name) = l.rsplit_once('/').unwrap_or(("", &l));
        words(name).any(|w| w == "history" || NOTES.contains(&w))
            || words(dirs).any(|w| NOTES.contains(&w))
    };
    let docs: Vec<&String> = files
        .iter()
        .filter(|f| {
            [".md", ".rst", ".adoc", ".txt"]
                .iter()
                .any(|e| f.ends_with(e))
                && !historical(f)
        })
        .collect();
    let span = regex::Regex::new(r"``?([A-Za-z_][A-Za-z0-9_.:]*[A-Za-z0-9_])(?:\(\))?``?").unwrap();
    let code_like = |s: &str| {
        let last = s.rsplit(['.', ':']).next().unwrap_or(s);
        last.len() >= 5
            && (last.contains('_') || last.chars().skip(1).any(|c| c.is_ascii_uppercase()))
    };
    // name -> doc locations
    let mut mentions: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    // Names the documentation itself defines in its examples.
    let mut examples: HashSet<String> = HashSet::new();
    let defined = regex::Regex::new(r"\b(?:class|def|fn|func|function|struct|interface|type)\s+([A-Za-z_][A-Za-z0-9_]*)|\b([A-Za-z_][A-Za-z0-9_]*)\s*=").unwrap();
    for d in &docs {
        let Ok(text) = std::fs::read_to_string(repo.join(d)) else {
            continue;
        };
        for c in defined.captures_iter(&text) {
            if let Some(m) = c.get(1).or(c.get(2)) {
                examples.insert(m.as_str().to_string());
            }
        }
    }
    for d in docs {
        let Ok(text) = std::fs::read_to_string(repo.join(d)) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            for c in span.captures_iter(line) {
                let full = &c[1];
                if !code_like(full) {
                    continue;
                }
                let name = full.rsplit(['.', ':']).next().unwrap_or(full).to_string();
                mentions.entry(name).or_default().push((d.clone(), i + 1));
            }
        }
    }
    // A name exists if some code file contains it as a word.
    let code: Vec<&String> = files
        .iter()
        .filter(|f| {
            ![".md", ".rst", ".adoc", ".txt", ".po", ".json", ".lock"]
                .iter()
                .any(|e| f.ends_with(e))
        })
        .collect();
    let reader = ogrep_core::reader::IndexReader::open(idx).ok();
    let is_code: HashSet<&str> = code.iter().map(|s| s.as_str()).collect();
    let word = |text: &str, name: &str| {
        let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        text.match_indices(name).any(|(i, _)| {
            !text[..i].chars().next_back().is_some_and(ident)
                && !text[i + name.len()..].chars().next().is_some_and(ident)
        })
    };
    // The pattern picks names with every matching option (-i, -F, -w, ...).
    let filter = opts.matcher(ci)?;
    let mut missing = Vec::new();
    for (name, locs) in &mentions {
        if examples.contains(name) || !filter.is_match(name)? {
            continue;
        }
        let candidates: Vec<String> = match &reader {
            Some(r) => {
                let plan = ogrep_core::query::build_literal_plan(name, false);
                let mut work = ogrep_core::query::QueryWork::default();
                ogrep_core::query::execute_plan_indexed(&plan, r, &mut work)
                    .into_iter()
                    .filter_map(|f| r.file_path(f).map(String::from))
                    .filter(|p| is_code.contains(p.as_str()))
                    .collect()
            }
            None => git_output(repo, &["grep", "-l", "-F", "-w", "-I", "--", name])
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .filter(|p| is_code.contains(p.as_str()))
                .collect(),
        };
        let exists = candidates
            .iter()
            .any(|p| std::fs::read_to_string(repo.join(p)).is_ok_and(|t| word(&t, name)));
        if !exists {
            missing.push((name, locs));
        }
    }
    for (name, locs) in &missing {
        for (doc, line) in locs.iter().take(3) {
            if opts.json {
                let msg = serde_json::json!({"type": "drift", "data": {"name": name, "path": doc, "line_number": line}});
                writeln!(out, "{msg}")?;
            } else {
                writeln!(out, "{doc}:{line}: `{name}` is not in the code")?;
            }
        }
    }
    Ok(!missing.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_regions_are_fully_similar() {
        let a = shingles("fn add(a: i32, b: i32) -> i32 { let c = a + b; c * 2 }");
        let b = shingles("fn add(a: i32, b: i32) -> i32 { let c = a + b; c * 2 }");
        let c = shingles("struct Config { name: String, port: u16, verbose: bool }");
        assert!((jaccard(&a, &b) - 1.0).abs() < 1e-9);
        assert!(jaccard(&a, &c) < 0.1);
        assert_eq!(stem("src/db/query.py"), "query");
    }
}
