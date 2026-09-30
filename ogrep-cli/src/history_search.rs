//! `--history` and `--introduced`: search the lines commits added or removed.
//!
//! The history index ([`ogrep_core::history`]) narrows the commits that can
//! match; each candidate is re-diffed by git and its changed lines searched with
//! the same matcher as a normal search, so results equal `git log -G` for the
//! commits the index covers.

use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use ogrep_core::history::{self, ChangedLine, HistoryIndex};

use crate::search::SearchOptions;

/// What `--history` prints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every commit with a matching changed line, newest first.
    Log,
    /// When the pattern first appeared and last disappeared.
    Introduced,
}

/// Commit filters, already resolved to UNIX times.
#[derive(Default)]
pub struct Filters {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub author: Option<regex::Regex>,
}

/// A matching line of a commit, with the byte ranges that matched.
struct Hit {
    line: ChangedLine,
    spans: Vec<(usize, usize)>,
}

struct CommitHits {
    sha: String,
    time: i64,
    hits: Vec<Hit>,
    author: String,
    subject: String,
}

/// Resolve a `--since`/`--until` value to a UNIX time with git's own date
/// parser, so every form `git log --since` takes works. Short durations such as
/// `2h`, `3d`, `2w`, `6mo` and `1y` mean that long ago.
pub fn resolve_when(root: &Path, when: &str, until: bool) -> Result<i64> {
    let expanded = expand_duration(when).unwrap_or_else(|| when.to_string());
    let flag = if until { "--until" } else { "--since" };
    let out = history::git(root)
        .args(["rev-parse", &format!("{flag}={expanded}")])
        .output()
        .context("running git")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let value = text
        .trim()
        .rsplit('=')
        .next()
        .and_then(|v| v.parse::<i64>().ok());
    match value {
        Some(v) if out.status.success() => Ok(v),
        _ => bail!("cannot read {flag} value {when:?} as a date"),
    }
}

/// Short duration units: what the user types, git's word for it, and its
/// length in seconds (months and years approximate, for use without git).
const UNITS: &[(&[&str], &str, i64)] = &[
    (&["s"], "seconds", 1),
    (&["m", "min"], "minutes", 60),
    (&["h"], "hours", 3_600),
    (&["d"], "days", 86_400),
    (&["w"], "weeks", 604_800),
    (&["mo"], "months", 2_592_000),
    (&["y"], "years", 31_536_000),
];

/// `6mo` → (6, the months row of [`UNITS`]).
fn split_duration(when: &str) -> Option<(i64, &'static str, i64)> {
    let split = when.find(|c: char| !c.is_ascii_digit())?;
    let (n, unit) = when.split_at(split);
    let n = n.parse().ok()?;
    let &(_, name, secs) = UNITS.iter().find(|(names, ..)| names.contains(&unit))?;
    Some((n, name, secs))
}

/// `2w` as git's `2 weeks ago`.
fn expand_duration(when: &str) -> Option<String> {
    split_duration(when).map(|(n, name, _)| format!("{n} {name} ago"))
}

/// `2w` as seconds, for times worked out without git.
pub(crate) fn duration_secs(when: &str) -> Option<i64> {
    split_duration(when).map(|(n, _, secs)| n * secs)
}

/// `YYYY-MM-DD` for a UNIX time, in UTC.
fn date(time: i64) -> String {
    // Howard Hinnant's days-to-civil.
    let days = time.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The index holding the history: the nearest `.ogrep` with one, from the
/// searched path up to the repository root, else the root's. History covers
/// the whole repository with repository-relative paths, whichever directory
/// `ogrep index --history` was given.
fn history_index_dir(search_root: &Path, repo: &Path) -> std::path::PathBuf {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (start, repo) = (canonical(search_root), canonical(repo));
    start
        .ancestors()
        .take_while(|dir| dir.starts_with(&repo))
        .map(ogrep_core::builder::default_index_dir)
        .find(|dir| history::has_history(dir))
        .unwrap_or_else(|| ogrep_core::builder::default_index_dir(&repo))
}

/// Author and subject for each commit, from one `git log --no-walk`.
fn describe(root: &Path, commits: &mut [CommitHits]) -> Result<()> {
    if commits.is_empty() {
        return Ok(());
    }
    let mut child = history::git(root)
        .args([
            "log",
            "--no-walk=unsorted",
            "--stdin",
            "--format=%H%x00%an <%ae>%x00%s",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let input: String = commits.iter().map(|c| format!("{}\n", c.sha)).collect();
    let mut stdin = child.stdin.take().expect("piped");
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let out = child.wait_with_output()?;
    let _ = feeder.join();
    let mut info = std::collections::HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(3, '\0');
        if let (Some(sha), Some(author), Some(subject)) = (parts.next(), parts.next(), parts.next())
        {
            info.insert(sha.to_string(), (author.to_string(), subject.to_string()));
        }
    }
    for c in commits.iter_mut() {
        if let Some((author, subject)) = info.remove(&c.sha) {
            c.author = author;
            c.subject = subject;
        }
    }
    Ok(())
}

/// The search root relative to the repository root, `/`-separated, empty for
/// the whole repository.
fn scope_prefix(repo: &Path, search_root: &Path) -> String {
    let canon = std::fs::canonicalize(search_root).unwrap_or_else(|_| search_root.to_path_buf());
    let repo = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    match canon.strip_prefix(&repo) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().replace('\\', "/"),
        _ => String::new(),
    }
}

fn in_scope(prefix: &str, path: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Search the history index. Returns whether anything matched.
pub fn run(
    search_root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
    mode: Mode,
    filters: &Filters,
) -> Result<bool> {
    let repo = history::repo_root(search_root)
        .map_err(|e| anyhow::anyhow!("--history needs a git repository: {e}"))?;
    let index_dir = index_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| history_index_dir(search_root, &repo));
    let hist = HistoryIndex::open(&index_dir).map_err(|_| {
        anyhow::anyhow!(
            "no history index in {}; build it with `ogrep index --history`",
            index_dir.display()
        )
    })?;
    if !opts.no_messages
        && let Ok(head) = history::head(&repo)
        && head != hist.meta.head
    {
        eprintln!(
            "note: the history index ends at {}, not HEAD; `ogrep index --history` adds the newer commits",
            &hist.meta.head[..hist.meta.head.len().min(12)]
        );
    }

    let matcher = opts.matcher(ci)?;
    let patterns = opts.all_patterns();
    let plan = matcher.index_plan(&patterns, opts.fixed_string, ci)?;
    let glob_filter = opts.glob_filter()?;
    let type_filter = opts.type_filter()?;
    let prefix = scope_prefix(&repo, search_root);
    let mut match_opts = opts.match_options();
    // Every matching changed line of a commit is reported; `-m` caps them per
    // commit after filtering, below.
    match_opts.max_count = None;
    match_opts.before_context = 0;
    match_opts.after_context = 0;
    match_opts.all_spans = true;

    let candidates: Vec<history::Candidate> = hist
        .candidates(&plan)
        .into_iter()
        .filter(|c| filters.since.is_none_or(|s| c.time >= s))
        .filter(|c| filters.until.is_none_or(|u| c.time <= u))
        .collect();

    // Re-diffing candidates is the expensive part. Whole commits go to
    // parallel `git diff-tree --stdin` workers in contiguous batches; each
    // candidate chunk of a large commit is re-diffed on its own, limited to
    // its files.
    enum Work {
        Commits(Vec<String>),
        Chunk(String, Vec<String>),
    }
    let mut seen = std::collections::HashSet::new();
    let mut whole: Vec<String> = Vec::new();
    let mut work: Vec<Work> = Vec::new();
    for c in candidates {
        match c.chunk_paths {
            Some(paths) if !paths.is_empty() => work.push(Work::Chunk(c.sha, paths)),
            _ => {
                if seen.insert(c.sha.clone()) {
                    whole.push(c.sha);
                }
            }
        }
    }
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8);
    let batch = whole.len().div_ceil(workers).max(1);
    work.extend(whole.chunks(batch).map(|b| Work::Commits(b.to_vec())));

    let search_commit = |doc: history::CommitDoc| -> Result<Option<CommitHits>> {
        let text = doc.text();
        let Some(found) =
            crate::matching::FileMatches::find_filtered(&text, None, &matcher, &match_opts)?
        else {
            return Ok(None);
        };
        let mut hits = Vec::new();
        found.for_each(&match_opts, &matcher, |emit| -> Result<()> {
            if let crate::matching::Emit::Match {
                line_number, spans, ..
            } = emit
                && let Some(line) = doc.lines.get(line_number - 1)
                && in_scope(&prefix, &line.path)
                && type_filter.matches(&line.path)
                && glob_filter.matches(&line.path)
            {
                hits.push(Hit {
                    line: line.clone(),
                    spans,
                });
            }
            Ok(())
        })?;
        Ok((!hits.is_empty()).then(|| CommitHits {
            sha: doc.sha,
            time: doc.time,
            hits,
            author: String::new(),
            subject: String::new(),
        }))
    };
    let results: Vec<Result<Vec<CommitHits>>> = {
        use rayon::prelude::*;
        work.par_iter()
            .map(|item| -> Result<Vec<CommitHits>> {
                let mut found = Vec::new();
                let mut failure = None;
                // The first error ends the diff; nothing after it is used.
                let each = |doc| match search_commit(doc) {
                    Ok(hits) => {
                        found.extend(hits);
                        std::ops::ControlFlow::Continue(())
                    }
                    Err(e) => {
                        failure = Some(e);
                        std::ops::ControlFlow::Break(())
                    }
                };
                match item {
                    Work::Commits(shas) => history::git_diff(&repo, shas, each)?,
                    Work::Chunk(sha, paths) => history::git_diff_paths(&repo, sha, paths, each)?,
                }
                failure.map_or(Ok(found), Err)
            })
            .collect()
    };
    // Chunks of one commit come back separately: merge them, in path order,
    // which is the order git reports a whole commit's files in.
    let mut by_sha: std::collections::HashMap<String, CommitHits> =
        std::collections::HashMap::new();
    for result in results {
        for c in result? {
            match by_sha.get_mut(&c.sha) {
                Some(existing) => existing.hits.extend(c.hits),
                None => {
                    by_sha.insert(c.sha.clone(), c);
                }
            }
        }
    }
    let mut commits: Vec<CommitHits> = by_sha.into_values().collect();
    for c in &mut commits {
        c.hits.sort_by(|a, b| a.line.path.cmp(&b.line.path));
        if let Some(max) = opts.max_count {
            c.hits.truncate(max);
        }
    }
    describe(&repo, &mut commits)?;
    if let Some(author) = &filters.author {
        commits.retain(|c| author.is_match(&c.author));
    }
    commits.sort_by(|a, b| (b.time, &b.sha).cmp(&(a.time, &a.sha)));

    if opts.quiet {
        return Ok(!commits.is_empty());
    }
    let color = opts.color.is_enabled() && crate::output::atty_check()
        || opts.color == crate::output::ColorMode::Always;
    let mut out = BufWriter::new(std::io::stdout().lock());
    match mode {
        Mode::Log => print_log(&mut out, &commits, opts, color)?,
        Mode::Introduced => {
            let at_head = present_at_head(search_root, index_path, opts, ci);
            print_introduced(&mut out, &commits, opts, at_head)?
        }
    }
    out.flush()?;
    Ok(!commits.is_empty())
}

fn highlight(text: &str, spans: &[(usize, usize)], color: bool) -> String {
    if !color || spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + spans.len() * 12);
    let mut pos = 0;
    for &(s, e) in spans {
        if s < pos || e > text.len() || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
            continue;
        }
        out.push_str(&text[pos..s]);
        out.push_str("\x1b[1;31m");
        out.push_str(&text[s..e]);
        out.push_str("\x1b[0m");
        pos = e;
    }
    out.push_str(&text[pos..]);
    out
}

fn print_log(
    out: &mut impl Write,
    commits: &[CommitHits],
    opts: &SearchOptions,
    color: bool,
) -> Result<()> {
    let (yellow, reset) = if color {
        ("\x1b[33m", "\x1b[0m")
    } else {
        ("", "")
    };
    for (i, c) in commits.iter().enumerate() {
        let short = &c.sha[..c.sha.len().min(12)];
        if opts.json {
            let matches: Vec<serde_json::Value> = c
                .hits
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "path": h.line.path,
                        "line_number": h.line.line,
                        "change": if h.line.added { "added" } else { "removed" },
                        "text": h.line.text,
                        "submatches": h.spans.iter().map(|&(s, e)| serde_json::json!({
                            "start": s, "end": e, "match": h.line.text.get(s..e).unwrap_or(""),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let msg = serde_json::json!({
                "type": "commit",
                "data": {
                    "sha": c.sha, "time": c.time, "date": date(c.time),
                    "author": c.author, "subject": c.subject, "matches": matches,
                },
            });
            writeln!(out, "{msg}")?;
            continue;
        }
        if opts.files_only {
            writeln!(
                out,
                "{yellow}{short}{reset} {} {}: {}",
                date(c.time),
                c.author,
                c.subject
            )?;
            continue;
        }
        if opts.count || opts.count_matches {
            let n = if opts.count_matches {
                c.hits.iter().map(|h| h.spans.len().max(1)).sum::<usize>()
            } else {
                c.hits.len()
            };
            writeln!(out, "{short}:{n}")?;
            continue;
        }
        if i > 0 {
            writeln!(out)?;
        }
        writeln!(
            out,
            "{yellow}commit {short}{reset} {} {}",
            date(c.time),
            c.author
        )?;
        writeln!(out, "    {}", c.subject)?;
        for h in &c.hits {
            let sign = if h.line.added { '+' } else { '-' };
            writeln!(
                out,
                "{}:{}:{sign}{}",
                h.line.path,
                h.line.line,
                highlight(&h.line.text, &h.spans, color)
            )?;
        }
    }
    Ok(())
}

/// Whether the pattern still matches at HEAD, as a count of matching files from
/// an ordinary search, or `None` when that search could not run.
fn present_at_head(
    root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
) -> Option<usize> {
    let mut files: Vec<String> = crate::search::captured_matches(root, index_path, opts, ci)
        .ok()?
        .into_iter()
        .map(|m| m.file)
        .collect();
    files.sort();
    files.dedup();
    Some(files.len())
}

fn print_introduced(
    out: &mut impl Write,
    commits: &[CommitHits],
    opts: &SearchOptions,
    at_head: Option<usize>,
) -> Result<()> {
    // Commits are newest first.
    let first_added = commits
        .iter()
        .rev()
        .find_map(|c| c.hits.iter().find(|h| h.line.added).map(|h| (c, h)));
    let last_removed = commits
        .iter()
        .find_map(|c| c.hits.iter().find(|h| !h.line.added).map(|h| (c, h)));
    if opts.json {
        let one = |x: Option<(&CommitHits, &Hit)>| {
            x.map(|(c, h)| {
                serde_json::json!({
                    "sha": c.sha, "time": c.time, "date": date(c.time), "author": c.author,
                    "subject": c.subject, "path": h.line.path, "line_number": h.line.line,
                    "text": h.line.text,
                })
            })
        };
        let msg = serde_json::json!({
            "type": "introduced",
            "data": {
                "first_added": one(first_added),
                "last_removed": one(last_removed),
                "files_at_head": at_head,
                "commits": commits.len(),
            },
        });
        writeln!(out, "{msg}")?;
        return Ok(());
    }
    let line =
        |label: &str, x: Option<(&CommitHits, &Hit)>, out: &mut dyn Write| -> std::io::Result<()> {
            match x {
                Some((c, h)) => writeln!(
                    out,
                    "{label} {} {} {}  {}:{}  {}",
                    &c.sha[..c.sha.len().min(12)],
                    date(c.time),
                    c.author,
                    h.line.path,
                    h.line.line,
                    c.subject
                ),
                None => writeln!(out, "{label} never"),
            }
        };
    line("first added: ", first_added, out)?;
    line("last removed:", last_removed, out)?;
    match at_head {
        Some(0) => writeln!(out, "at HEAD:      absent")?,
        Some(n) => writeln!(
            out,
            "at HEAD:      present in {n} file{}",
            if n == 1 { "" } else { "s" }
        )?,
        None => writeln!(out, "at HEAD:      unknown")?,
    }
    writeln!(
        out,
        "commits:      {} changed a matching line",
        commits.len()
    )?;
    Ok(())
}

/// `--at REV`: search the files of a past revision, read from git's object
/// store, with the same matcher and output as a search of the working tree.
/// Only tracked files exist at a revision, so ignore rules have nothing to add;
/// globs, types, `--max-filesize` and binary detection apply as usual.
pub fn search_at(
    root: &Path,
    rev: &str,
    opts: &SearchOptions,
    ci: bool,
    writer: &mut crate::output::OutputWriter,
) -> Result<bool> {
    use std::io::{BufRead, BufReader, Read};
    let repo = history::repo_root(root)
        .map_err(|e| anyhow::anyhow!("--at needs a git repository: {e}"))?;
    let prefix = scope_prefix(&repo, root);
    let tree = history::git(&repo)
        .args(["ls-tree", "-r", "-z", "-l", "--full-tree", rev])
        .output()
        .context("running git ls-tree")?;
    if !tree.status.success() {
        bail!(
            "--at {rev}: {}",
            String::from_utf8_lossy(&tree.stderr).trim()
        );
    }
    let glob_filter = opts.glob_filter()?;
    let type_filter = opts.type_filter()?;
    let max_size = opts.max_filesize;
    // (blob id, path relative to the search root)
    let mut blobs: Vec<(String, String)> = Vec::new();
    for entry in tree.stdout.split(|&b| b == 0).filter(|e| !e.is_empty()) {
        let entry = String::from_utf8_lossy(entry);
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut fields = meta.split_whitespace();
        let (Some(mode), Some(kind), Some(id), Some(size)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if kind != "blob" || mode == "120000" || !in_scope(&prefix, path) {
            continue;
        }
        if max_size.is_some_and(|max| size.parse::<u64>().is_ok_and(|s| s > max)) {
            continue;
        }
        let rel = if prefix.is_empty() {
            path.to_string()
        } else {
            path[prefix.len()..].trim_start_matches('/').to_string()
        };
        if !type_filter.matches(&rel) || !glob_filter.matches(&rel) {
            continue;
        }
        blobs.push((id.to_string(), rel));
    }
    if opts.sort.is_some() {
        blobs.sort_by(|a, b| a.1.cmp(&b.1));
    }

    let matcher = opts.matcher(ci)?;
    let filter = opts.file_filter(ci)?;
    let mut child = history::git(&repo)
        .args(["cat-file", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("running git cat-file")?;
    let mut stdin = child.stdin.take().expect("piped");
    let ids: String = blobs.iter().map(|(id, _)| format!("{id}\n")).collect();
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(ids.as_bytes());
    });
    let mut reader = BufReader::with_capacity(1 << 20, child.stdout.take().expect("piped"));
    let mut had_match = false;
    let mut header = String::new();
    for (_, rel) in &blobs {
        header.clear();
        reader.read_line(&mut header)?;
        // "<id> blob <size>"
        let size: usize = header
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("unexpected git cat-file output: {header:?}"))?;
        let mut bytes = vec![0u8; size];
        reader.read_exact(&mut bytes)?;
        let mut newline = [0u8; 1];
        reader.read_exact(&mut newline)?;
        if crate::search::search_bytes(bytes, rel, &matcher, filter.as_ref(), opts, writer)? {
            had_match = true;
            if opts.quiet {
                break;
            }
        }
    }
    drop(reader);
    let _ = feeder.join();
    let _ = child.kill();
    let _ = child.wait();
    writer.flush()?;
    Ok(had_match)
}

/// Files changed since `since` (a revision, or a date or duration as for
/// `--since`), relative to the search root: every tracked file that differs
/// between that point and the working tree, including uncommitted changes,
/// plus untracked files that are not ignored. `None` when `since` predates the
/// history, so every file counts as changed.
pub fn changed_paths(
    root: &Path,
    since: &str,
) -> Result<Option<std::collections::HashSet<String>>> {
    let repo = history::repo_root(root)
        .map_err(|e| anyhow::anyhow!("--changed-since needs a git repository: {e}"))?;
    let git = |args: &[&str]| history::git_output(&repo, args);
    let is_rev = history::git(&repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{since}^{{commit}}"),
        ])
        .output()
        .is_ok_and(|o| o.status.success());
    let rev = if is_rev {
        since.to_string()
    } else {
        let time = resolve_when(&repo, since, false)?;
        let before = git(&["rev-list", "-1", &format!("--before={time}"), "HEAD"])?;
        let rev = before.trim().to_string();
        if rev.is_empty() {
            return Ok(None);
        }
        rev
    };
    let prefix = scope_prefix(&repo, root);
    let mut paths = std::collections::HashSet::new();
    for listing in [
        git(&["diff", "--name-only", "-z", "--no-renames", &rev])?,
        git(&["ls-files", "--others", "--exclude-standard", "-z"])?,
    ] {
        for path in listing.split('\0').filter(|p| !p.is_empty()) {
            if !in_scope(&prefix, path) {
                continue;
            }
            let rel = if prefix.is_empty() {
                path.to_string()
            } else {
                path[prefix.len()..].trim_start_matches('/').to_string()
            };
            paths.insert(rel);
        }
    }
    Ok(Some(paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_durations() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_700_000_000), "2023-11-14");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(expand_duration("2h").as_deref(), Some("2 hours ago"));
        assert_eq!(expand_duration("6mo").as_deref(), Some("6 months ago"));
        assert_eq!(expand_duration("2024-05-01"), None);
        assert!(in_scope("", "a/b"));
        assert!(in_scope("django/db", "django/db/models/query.py"));
        assert!(!in_scope("django/db", "django/dbx/models.py"));
    }
}
