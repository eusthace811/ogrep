//! Incident and operations commands: `--entities`, `--blast`, `--suspects`,
//! `--timeline`, `--trace`, `--log-templates` and `--similar-incidents`.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use ogrep_core::ops::{self, OpsModel};

use crate::search::SearchOptions;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Entities {
        dated: bool,
    },
    Blast,
    Suspects,
    Timeline,
    Trace,
    LogTemplates {
        new_since: Option<String>,
        to_code: bool,
    },
    /// With the number of incidents to list (`--budget`).
    SimilarIncidents {
        budget: usize,
    },
}

use crate::extras::repo_of;

fn model(repo: &Path) -> Result<(OpsModel, Vec<String>)> {
    let files = ogrep_core::history::ls_files(repo, true)?;
    Ok((ops::scan(repo, &files), files))
}

/// The commits `--since` and `--until` describe. Each is a revision, bounding
/// by ancestry, or else a time. Without `--since` the range is `default_secs`
/// long and ends at `--until` (for a revision, at its commit time), or now.
fn range(
    repo: &Path,
    since: Option<&str>,
    until: Option<&str>,
    default_secs: i64,
) -> Result<ops::Range> {
    use crate::history_search::{is_revision, resolve_when};
    use ops::Bound;
    let bound = |value: &str, is_until: bool| -> Result<Bound> {
        if is_revision(repo, value) {
            return Ok(Bound::Rev(value.to_string()));
        }
        let flag = if is_until { "--until" } else { "--since" };
        let time = resolve_when(repo, value, is_until).map_err(|e| e.context(flag))?;
        Ok(Bound::Time(time))
    };
    let until = until.map(|u| bound(u, true)).transpose()?;
    let since = match (since, &until) {
        (Some(s), _) => bound(s, false)?,
        (None, Some(Bound::Time(until))) => Bound::Time(until - default_secs),
        (None, Some(Bound::Rev(rev))) => {
            let time = ogrep_core::history::git_output(repo, &["log", "-1", "--format=%ct", rev])?;
            let time: i64 = time.trim().parse().context("reading a commit time")?;
            Bound::Time(time - default_secs)
        }
        (None, None) => Bound::Time(resolve_when(
            repo,
            &format!("{default_secs} seconds ago"),
            false,
        )?),
    };
    if let (Bound::Time(since), Some(Bound::Time(until))) = (&since, &until)
        && since > until
    {
        bail!("--since is later than --until; a duration such as 2h counts back from now");
    }
    Ok(ops::Range { since, until })
}

const DAY: i64 = 86_400;

fn ago(time: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let d = (now - time).max(0);
    match d {
        0..3600 => format!("{}m ago", d / 60),
        3600..172_800 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86_400),
    }
}

pub fn run(
    root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
    mode: &Mode,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<bool> {
    let mut out = BufWriter::new(std::io::stdout().lock());
    if let Mode::LogTemplates { new_since, to_code } = mode {
        // Logs usually live outside any repository; the code that writes them
        // is in the one around the current directory, if any.
        let repo = repo_of(root).or_else(|_| repo_of(Path::new("."))).ok();
        let found = log_templates(
            repo.as_deref(),
            root,
            index_path,
            opts,
            ci,
            new_since.as_deref(),
            *to_code,
            &mut out,
        )?;
        out.flush()?;
        return Ok(found);
    }
    let repo = repo_of(root)?;
    let found = match mode {
        Mode::Entities { dated } => entities(&repo, opts, ci, *dated, &mut out)?,
        Mode::Blast => blast(&repo, opts, &mut out)?,
        Mode::Suspects => suspects(&repo, opts, since, until, &mut out)?,
        Mode::Timeline => timeline(&repo, opts, since, until, &mut out)?,
        Mode::Trace => trace(&repo, opts, &mut out)?,
        Mode::LogTemplates { .. } => unreachable!("handled above"),
        Mode::SimilarIncidents { budget } => similar_incidents(&repo, opts, *budget, &mut out)?,
    };
    out.flush()?;
    Ok(found)
}

fn entities(
    repo: &Path,
    opts: &SearchOptions,
    ci: bool,
    dated: bool,
    out: &mut impl Write,
) -> Result<bool> {
    let (mut m, _) = model(repo)?;
    if dated {
        ops::date(repo, &mut m)?;
    }
    let matcher = opts.matcher(ci)?;
    let mut any = false;
    for e in &m.entities {
        let mut named = matcher.is_match(&e.id)?;
        for alias in &e.aliases {
            named = named || matcher.is_match(alias)?;
        }
        if !named {
            continue;
        }
        any = true;
        let deps: Vec<&ops::Edge> = m.edges.iter().filter(|x| x.from == e.id).collect();
        if opts.json {
            let msg = serde_json::json!({"type": "entity", "data": {"entity": e, "edges": deps}});
            writeln!(out, "{msg}")?;
            continue;
        }
        let at = e
            .locations
            .first()
            .map_or(String::new(), |l| format!("{}:{}", l.file, l.line));
        let targets: Vec<String> = {
            let mut t: Vec<String> = deps
                .iter()
                .map(|d| match &d.since {
                    Some(s) => format!("{} (since {:.7}, {})", d.to, s.commit, ago(s.time)),
                    None => d.to.clone(),
                })
                .collect();
            t.sort();
            t.dedup();
            t
        };
        write!(out, "{:<8} {:<24} {at}", e.kind, e.id)?;
        if !targets.is_empty() {
            write!(out, "  -> {}", targets.join(", "))?;
        }
        if !e.aliases.is_empty() {
            write!(out, "  (also {})", e.aliases.join(", "))?;
        }
        writeln!(out)?;
    }
    Ok(any)
}

fn one_service<'m>(m: &'m OpsModel, name: &str) -> Result<&'m str> {
    let services = m.services();
    if let Some(s) = services.iter().find(|s| s.eq_ignore_ascii_case(name)) {
        return Ok(s);
    }
    let alias_of = m
        .entities
        .iter()
        .find(|e| e.kind == "service" && e.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)));
    if let Some(e) = alias_of {
        return Ok(&e.id);
    }
    let close: Vec<&&str> = services.iter().filter(|s| s.contains(name)).collect();
    match close.as_slice() {
        [one] => Ok(one),
        [] => bail!("no service named {name:?}; `ogrep --entities .` lists them"),
        many => bail!(
            "{name:?} matches several services: {}",
            many.iter().map(|s| **s).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// The services a comma-separated list names.
fn services_named(m: &OpsModel, list: &str) -> Result<Vec<String>> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| one_service(m, s).map(String::from))
        .collect()
}

fn blast(repo: &Path, opts: &SearchOptions, out: &mut impl Write) -> Result<bool> {
    let (m, _) = model(repo)?;
    let target = one_service(&m, &opts.pattern)?;
    let hits = m.blast(target);
    if opts.json {
        for (depth, e) in &hits {
            let msg = serde_json::json!({"type": "blast", "data": {"target": target, "depth": depth, "service": e.from, "edge": e}});
            writeln!(out, "{msg}")?;
        }
        return Ok(!hits.is_empty());
    }
    writeln!(out, "blast radius of {target}: {} service(s)", hits.len())?;
    for (depth, e) in &hits {
        let prov = match e.provenance {
            ops::Provenance::Declared => "declared",
            ops::Provenance::Inferred => "inferred",
        };
        writeln!(
            out,
            "{depth} {:<24} {} {:<8} {}:{}  {}",
            e.from, e.relation, prov, e.evidence.file, e.evidence.line, e.text
        )?;
    }
    Ok(!hits.is_empty())
}

fn suspects(
    repo: &Path,
    opts: &SearchOptions,
    since: Option<&str>,
    until: Option<&str>,
    out: &mut impl Write,
) -> Result<bool> {
    let (m, _) = model(repo)?;
    let failing = services_named(&m, &opts.pattern)?;
    let changes = ops::recent_changes(repo, &range(repo, since, until, DAY)?);
    let ranked = m.suspects(&failing, &changes);
    for (i, s) in ranked.iter().take(10).enumerate() {
        if opts.json {
            let msg = serde_json::json!({"type": "suspect", "data": {"rank": i + 1, "suspect": s}});
            writeln!(out, "{msg}")?;
            continue;
        }
        let change = s.changes.first().map_or(String::new(), |c| {
            format!(
                "  changed {} ({} {}: {})",
                ago(c.time),
                &c.sha[..c.sha.len().min(10)],
                if s.changes.len() > 1 {
                    format!("+{} more", s.changes.len() - 1)
                } else {
                    String::new()
                },
                c.subject
            )
        });
        writeln!(
            out,
            "{:>2}. {:<24} score {:.2}  explains {}/{} ({}){change}",
            i + 1,
            s.service,
            s.score,
            s.explains.len(),
            failing.len(),
            s.explains.join(", ")
        )?;
    }
    Ok(!ranked.is_empty())
}

fn timeline(
    repo: &Path,
    opts: &SearchOptions,
    since: Option<&str>,
    until: Option<&str>,
    out: &mut impl Write,
) -> Result<bool> {
    let (m, _) = model(repo)?;
    let targets = services_named(&m, &opts.pattern)?;
    if targets.is_empty() {
        bail!("--timeline needs a service name; `ogrep --entities .` lists them");
    }
    let changes = ops::recent_changes(repo, &range(repo, since, until, 7 * DAY)?);
    // Each service asked for and what it depends on directly, once.
    let scope: BTreeSet<String> = targets
        .iter()
        .flat_map(|t| m.dependencies(t))
        .filter(|(_, distance)| *distance <= 1)
        .map(|(svc, _)| svc)
        .collect();
    let mut events: Vec<(i64, &str, &ops::Change)> = Vec::new();
    for svc in &scope {
        for c in m.attribute(svc, &changes) {
            events.push((c.time, svc.as_str(), c));
        }
    }
    events.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    events.dedup_by(|b, a| a.2.sha == b.2.sha && a.1 == b.1);
    for (time, svc, c) in &events {
        if opts.json {
            let msg = serde_json::json!({"type": "event", "data": {"time": time, "service": svc, "commit": c}});
            writeln!(out, "{msg}")?;
        } else {
            writeln!(
                out,
                "{:<9} {:<24} {} {}",
                ago(*time),
                svc,
                &c.sha[..c.sha.len().min(10)],
                c.subject
            )?;
        }
    }
    Ok(!events.is_empty())
}

fn trace(repo: &Path, opts: &SearchOptions, out: &mut impl Write) -> Result<bool> {
    let text = if opts.pattern == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(&opts.pattern)
            .with_context(|| format!("reading {}", opts.pattern))?
    };
    let files = ogrep_core::history::ls_files(repo, true)?;
    let frames = ogrep_core::trace::frames(&text);
    let mut found = false;
    let read = ogrep_core::rooted::indexed_text_reader(repo);
    for f in &frames {
        let Some(path) = ogrep_core::trace::resolve(&f.raw_path, &files) else {
            continue;
        };
        found = true;
        let source = read(path).unwrap_or_default();
        let line_text = source
            .lines()
            .nth(f.line.saturating_sub(1))
            .unwrap_or("")
            .trim();
        let scope = scope_of(path, &source, f.line);
        if opts.json {
            let msg = serde_json::json!({"type": "frame", "data": {
                "index": f.index, "path": path, "line_number": f.line, "function": f.function,
                "text": line_text, "scope": scope,
            }});
            writeln!(out, "{msg}")?;
        } else {
            let scope = scope.map_or(String::new(), |s| format!("  [{s}]"));
            writeln!(out, "#{:<2} {path}:{}: {line_text}{scope}", f.index, f.line)?;
        }
    }
    if !found && !opts.no_messages {
        eprintln!(
            "ogrep: no frame of the trace is in this repository ({} frame(s) read)",
            frames.len()
        );
    }
    Ok(found)
}

#[cfg(feature = "structure")]
fn scope_of(path: &str, source: &str, line: usize) -> Option<String> {
    let fs = ogrep_core::structure::analyze(path, source.as_bytes())?;
    // Byte offset of the line's start; `lines()` would drop each `\r`.
    let offset = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum();
    let chain = fs.enclosing(offset);
    (!chain.is_empty()).then(|| {
        chain
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join("::")
    })
}

#[cfg(not(feature = "structure"))]
fn scope_of(_path: &str, _source: &str, _line: usize) -> Option<String> {
    None
}

#[allow(clippy::too_many_arguments)]
fn log_templates(
    repo: Option<&Path>,
    root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
    new_since: Option<&str>,
    to_code: bool,
    out: &mut impl Write,
) -> Result<bool> {
    // The pattern picks the lines; the path argument picks the logs. Logs are
    // usually not indexed, so read them directly.
    let logs = SearchOptions {
        no_index: true,
        ..opts.clone()
    };
    let mut drain = ogrep_core::logs::Drain::default();
    for m in crate::search::captured_matches(root, index_path, &logs, ci)? {
        drain.add(&m.file, m.line_number, &m.content);
    }
    let cutoff = match new_since {
        Some(w) => Some(parse_when(repo, w).map_err(|e| e.context("--new-since"))?),
        None => None,
    };
    let templates: Vec<_> = drain
        .sorted()
        .into_iter()
        .filter(|t| cutoff.is_none_or(|c| t.first_seen.is_some_and(|f| f >= c)))
        .collect();
    let codes: Vec<Option<(String, usize, String)>> = match (to_code, repo) {
        (true, Some(repo)) => emitters(repo, &templates, opts),
        _ => vec![None; templates.len()],
    };
    for (t, code) in templates.iter().zip(codes) {
        if opts.json {
            let msg = serde_json::json!({"type": "template", "data": {
                "template": t.text(), "count": t.count, "first_seen": t.first_seen, "last_seen": t.last_seen,
                "example": {"path": t.example.0, "line_number": t.example.1}, "code": code,
            }});
            writeln!(out, "{msg}")?;
            continue;
        }
        let seen = match (t.first_seen, t.last_seen) {
            (Some(f), Some(l)) => format!(
                "  first {} last {}",
                crate::history_search::iso(f),
                crate::history_search::iso(l)
            ),
            _ => String::new(),
        };
        writeln!(out, "{:>7}  {}{seen}", t.count, t.text())?;
        if let Some((path, line, text)) = code {
            writeln!(out, "         <- {path}:{line}: {text}")?;
        }
    }
    Ok(!templates.is_empty())
}

/// A time given as a duration (`2h`), an ISO 8601 date or timestamp, or (inside
/// a repository) anything `git log --since` takes.
fn parse_when(repo: Option<&Path>, when: &str) -> Result<i64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    // Inside a repository git reads durations, with calendar months and
    // years; outside, months and years are approximate.
    if repo.is_none()
        && let Some(ago) = crate::history_search::duration_secs(when)
    {
        return Ok(now.saturating_sub(ago));
    }
    let padded = if when.len() == 10 {
        format!("{when}T00:00:00Z")
    } else {
        when.to_string()
    };
    if let (Some(t), _) = ogrep_core::logs::leading_timestamp(&padded) {
        return Ok(t);
    }
    match repo {
        Some(repo) => crate::history_search::resolve_when(repo, when, false),
        None => bail!("cannot read {when:?} as a time (use 2h, 3d, 6mo, or an ISO 8601 date)"),
    }
}

const LEVELS: &[&str] = &[
    "TRACE", "DEBUG", "INFO", "NOTICE", "WARN", "WARNING", "ERROR", "ERR", "FATAL", "CRITICAL",
    "SEVERE",
];

/// Code that writes each template's message.
///
/// A template's constant runs are cut into windows of two words: the pieces
/// code writes literally whatever the values around them, even when the line
/// also carries words the template could not tell were variable (a logger
/// name, a status word, a path). Every window of every template is looked up
/// in one fixed-string search of the repository, through its index when it
/// has one. For each template, a file scores the windows it contains, each
/// worth its length divided by the square root of how many files contain it,
/// so long and rare pieces count most; the best file's line with the most of
/// the template's windows is the answer. Prose and log files are skipped: they
/// quote messages rather than write them.
///
/// On log lines mined from the code that writes them, with values filled in
/// (`scripts/eval/template_eval.py`): the right file for 96.7% of django's,
/// 96.9% of the OpenTelemetry demo's and 89.3% of llama.cpp's (not used for
/// tuning), against 84.0%, 79.5% and 70.7% for looking up the longest constant
/// run whole.
fn emitters(
    repo: &Path,
    templates: &[ogrep_core::logs::Template],
    opts: &SearchOptions,
) -> Vec<Option<(String, usize, String)>> {
    let per_template: Vec<Vec<String>> = templates.iter().map(message_windows).collect();
    let mut all: Vec<String> = per_template.iter().flatten().cloned().collect();
    all.sort();
    all.dedup();
    if all.is_empty() {
        return vec![None; templates.len()];
    }
    let matches = crate::search::captured_matches(repo, None, &code_search(opts, &all), false)
        .unwrap_or_default();
    let skip = |path: &str| {
        let p = path.to_ascii_lowercase();
        [".log", ".txt", ".md", ".rst", ".adoc", ".html", ".po"]
            .iter()
            .any(|e| p.ends_with(e))
    };
    // Per code line, the windows it holds; per window, the files holding it.
    let lines: Vec<(&crate::output::Match, Vec<usize>)> = matches
        .iter()
        .filter(|m| !skip(&m.file))
        .map(|m| {
            let held = (0..all.len())
                .filter(|&w| m.content.contains(all[w].as_str()))
                .collect::<Vec<_>>();
            (m, held)
        })
        .filter(|(_, held)| !held.is_empty())
        .collect();
    let mut holders: Vec<std::collections::BTreeSet<&str>> = vec![Default::default(); all.len()];
    for (m, held) in &lines {
        for &w in held {
            holders[w].insert(m.file.as_str());
        }
    }
    per_template
        .iter()
        .map(|windows| {
            let wanted: Vec<usize> = windows
                .iter()
                .filter_map(|w| all.binary_search(w).ok())
                .collect();
            // Per file: the template's windows it holds, and its best line as
            // (how many of them the line holds, line number, text).
            type Best<'a> = Option<(usize, usize, &'a str)>;
            let mut files: std::collections::BTreeMap<&str, (Vec<usize>, Best)> =
                Default::default();
            for (m, held) in &lines {
                let mine: Vec<usize> = held
                    .iter()
                    .copied()
                    .filter(|w| wanted.contains(w))
                    .collect();
                if mine.is_empty() {
                    continue;
                }
                let entry = files.entry(m.file.as_str()).or_default();
                entry.0.extend(&mine);
                let better = entry.1.is_none_or(|(n, line, _)| {
                    mine.len() > n || (mine.len() == n && m.line_number < line)
                });
                if better {
                    entry.1 = Some((mine.len(), m.line_number, m.content.as_str()));
                }
            }
            let score = |seen: &[usize]| -> i64 {
                let mut seen = seen.to_vec();
                seen.sort_unstable();
                seen.dedup();
                let s: f64 = seen
                    .iter()
                    .map(|&w| all[w].len() as f64 / (holders[w].len() as f64).sqrt())
                    .sum();
                (s * 1e6).round() as i64
            };
            // Highest score first; on a tie the earlier path, so the answer
            // never depends on the order a parallel search reported files in.
            let mut best: Option<(i64, &str, usize, &str)> = None;
            for (file, (seen, line)) in &files {
                let s = score(seen);
                if let Some((_, line, text)) = *line
                    && best.is_none_or(|(b, ..)| s > b)
                {
                    best = Some((s, *file, line, text));
                }
            }
            best.map(|(_, file, line, text)| (file.to_string(), line, text.trim().to_string()))
        })
        .collect()
}

/// Two-word windows over a template's constant runs, as [`emitters`] looks them
/// up. A leading level word or bracketed logger name is not part of what the
/// code writes, so it is dropped; a run of one or two words is kept whole.
/// Windows shorter than eight characters match too much to tell code apart.
fn message_windows(template: &ogrep_core::logs::Template) -> Vec<String> {
    const WORDS: usize = 2;
    const MIN_LEN: usize = 8;
    const MAX_WINDOWS: usize = 32;
    let mut runs = template.constant_runs();
    if let Some(first) = runs.first_mut() {
        let lead = first
            .iter()
            .take_while(|t| {
                let bare = t.trim_matches(|c: char| "[]():".contains(c));
                LEVELS.contains(&bare.to_ascii_uppercase().as_str())
                    || (t.starts_with('[') && t.ends_with(']'))
            })
            .count();
        first.drain(..lead);
    }
    let mut out: Vec<String> = Vec::new();
    for run in runs.iter().filter(|r| !r.is_empty()) {
        let pieces: Vec<String> = if run.len() <= WORDS {
            vec![run.join(" ")]
        } else {
            run.windows(WORDS).map(|w| w.join(" ")).collect()
        };
        for piece in pieces {
            let piece = piece
                .trim_matches(|c: char| " :,.=".contains(c))
                .to_string();
            if piece.len() >= MIN_LEN && !out.contains(&piece) {
                out.push(piece);
            }
        }
    }
    out.truncate(MAX_WINDOWS);
    out
}

/// A plain fixed-string search of the repository for `patterns`, keeping only
/// what decides which files are searched from the user's options (ignore
/// rules, hidden files, size limit), not what shaped the log search.
fn code_search(opts: &SearchOptions, patterns: &[String]) -> SearchOptions {
    SearchOptions {
        pattern: patterns[0].clone(),
        extra_patterns: patterns[1..].to_vec(),
        with_patterns: Vec::new(),
        without_patterns: Vec::new(),
        file_patterns: None,
        case_insensitive: false,
        case_sensitive: true,
        smart_case: false,
        fixed_string: true,
        word_boundary: false,
        line_regexp: false,
        invert_match: false,
        max_count: None,
        no_index: false,
        glob: Vec::new(),
        iglob: Vec::new(),
        types: Vec::new(),
        types_not: Vec::new(),
        only_paths: None,
        at_rev: None,
        multiline: false,
        multiline_dotall: false,
        passthru: false,
        stop_on_nonmatch: false,
        encoding: Default::default(),
        path_display: Default::default(),
        max_depth: None,
        ..opts.clone()
    }
}

const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "was", "were", "are", "but", "not",
    "have", "has", "had", "into", "when", "then", "than", "which", "after", "before", "our", "all",
    "any", "been", "its",
];

fn terms(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| t.len() >= 3 && !STOP.contains(&t.as_str()))
        .collect()
}

fn similar_incidents(
    repo: &Path,
    opts: &SearchOptions,
    budget: usize,
    out: &mut impl Write,
) -> Result<bool> {
    let query = if opts.pattern == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else if Path::new(&opts.pattern).is_file() {
        std::fs::read_to_string(&opts.pattern)?
    } else {
        opts.pattern.clone()
    };
    let (m, files) = model(repo)?;
    let services: Vec<String> = m
        .services()
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let docs: Vec<&String> = files
        .iter()
        .filter(|p| {
            let l = p.to_ascii_lowercase();
            [
                "postmortem",
                "post-mortem",
                "incident",
                "rca",
                "retro",
                "outage",
            ]
            .iter()
            .any(|k| l.contains(k))
                && [".md", ".txt", ".rst", ".adoc"]
                    .iter()
                    .any(|e| l.ends_with(e))
        })
        .collect();
    let q = terms(&query);
    let q_services: Vec<&String> = services.iter().filter(|s| q.contains(s)).collect();
    let read = ogrep_core::rooted::indexed_text_reader(repo);
    let texts: Vec<(String, Vec<String>)> = docs
        .iter()
        .filter_map(|p| read(p).ok().map(|t| ((*p).clone(), terms(&t))))
        .collect();
    let n = texts.len().max(1) as f64;
    let avg = texts.iter().map(|t| t.1.len()).sum::<usize>() as f64 / n;
    // BM25 over the incident notes. Term counts per note, and the number of
    // notes holding each query term, are counted once up front.
    let query_terms: std::collections::BTreeSet<&str> = q.iter().map(String::as_str).collect();
    let counts: Vec<HashMap<&str, usize>> = texts
        .iter()
        .map(|(_, doc)| {
            let mut c = HashMap::new();
            for t in doc {
                *c.entry(t.as_str()).or_default() += 1;
            }
            c
        })
        .collect();
    let idf: Vec<(&str, f64)> = query_terms
        .iter()
        .map(|&t| {
            let df = counts.iter().filter(|c| c.contains_key(t)).count() as f64;
            (t, (1.0 + (n - df + 0.5) / (df + 0.5)).ln())
        })
        .collect();
    let mut scored: Vec<(f64, &str, Vec<&str>)> = texts
        .iter()
        .zip(&counts)
        .map(|((path, doc), count)| {
            let mut score = 0.0;
            let mut hits = Vec::new();
            let norm = 1.0 - 0.75 + 0.75 * doc.len() as f64 / avg.max(1.0);
            for &(t, idf) in &idf {
                let tf = count.get(t).copied().unwrap_or(0) as f64;
                if tf == 0.0 {
                    continue;
                }
                score += idf * tf * 2.2 / (tf + 1.2 * norm);
                hits.push(t);
            }
            // Shared services count twice as much as other shared words.
            let shared = q_services
                .iter()
                .filter(|s| count.contains_key(s.as_str()))
                .count() as f64;
            (score * (1.0 + shared), path.as_str(), hits)
        })
        .filter(|s| s.0 > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(b.1)));
    for (score, path, hits) in scored.iter().take(budget) {
        if opts.json {
            let msg = serde_json::json!({"type": "incident", "data": {"path": path, "score": score, "terms": hits}});
            writeln!(out, "{msg}")?;
        } else {
            writeln!(
                out,
                "{score:6.2} {path}  ({})",
                hits.iter().take(6).copied().collect::<Vec<_>>().join(", ")
            )?;
        }
    }
    Ok(!scored.is_empty())
}
