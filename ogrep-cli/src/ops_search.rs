//! Incident and operations commands: `--entities`, `--blast`, `--suspects`,
//! `--timeline`, `--trace`, `--log-templates` and `--similar-incidents`.

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use ogrep_core::ops::{self, OpsModel};

use crate::search::SearchOptions;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Entities,
    Blast,
    Suspects,
    Timeline,
    Trace,
    LogTemplates {
        new_since: Option<String>,
        to_code: bool,
    },
    SimilarIncidents,
}

fn tracked_files(repo: &Path) -> Result<Vec<String>> {
    Ok(ogrep_core::history::ls_files(repo, true)?)
}

use crate::extras::repo_of;

fn model(repo: &Path) -> Result<(OpsModel, Vec<String>)> {
    let files = tracked_files(repo)?;
    Ok((ops::scan(repo, &files), files))
}

fn when(repo: &Path, value: Option<&str>, default: &str) -> Result<i64> {
    crate::history_search::resolve_when(repo, value.unwrap_or(default), false)
}

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
        Mode::Entities => entities(&repo, opts, ci, &mut out)?,
        Mode::Blast => blast(&repo, opts, &mut out)?,
        Mode::Suspects => suspects(&repo, opts, since, &mut out)?,
        Mode::Timeline => timeline(&repo, opts, since, &mut out)?,
        Mode::Trace => trace(&repo, opts, &mut out)?,
        Mode::LogTemplates { .. } => unreachable!("handled above"),
        Mode::SimilarIncidents => similar_incidents(&repo, opts, &mut out)?,
    };
    out.flush()?;
    Ok(found)
}

fn entities(repo: &Path, opts: &SearchOptions, ci: bool, out: &mut impl Write) -> Result<bool> {
    let (m, _) = model(repo)?;
    let matcher = opts.matcher(ci)?;
    let mut any = false;
    for e in &m.entities {
        if !matcher.is_match(&e.id)? {
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
            let mut t: Vec<String> = deps.iter().map(|d| d.to.clone()).collect();
            t.sort();
            t.dedup();
            t
        };
        if targets.is_empty() {
            writeln!(out, "{:<8} {:<24} {at}", e.kind, e.id)?;
        } else {
            writeln!(
                out,
                "{:<8} {:<24} {at}  -> {}",
                e.kind,
                e.id,
                targets.join(", ")
            )?;
        }
    }
    Ok(any)
}

fn one_service<'m>(m: &'m OpsModel, name: &str) -> Result<&'m str> {
    let services = m.services();
    if let Some(s) = services.iter().find(|s| s.eq_ignore_ascii_case(name)) {
        return Ok(s);
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
    out: &mut impl Write,
) -> Result<bool> {
    let (m, _) = model(repo)?;
    let failing: Vec<String> = opts
        .pattern
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| one_service(&m, s).map(String::from))
        .collect::<Result<_>>()?;
    let changes = ops::recent_changes(repo, when(repo, since, "24 hours ago")?);
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
    out: &mut impl Write,
) -> Result<bool> {
    let (m, _) = model(repo)?;
    let target = one_service(&m, &opts.pattern)?;
    let changes = ops::recent_changes(repo, when(repo, since, "7 days ago")?);
    // The service and what it depends on directly.
    let mut scope: Vec<(String, usize)> = m
        .dependencies(target)
        .into_iter()
        .filter(|(_, d)| *d <= 1)
        .collect();
    scope.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    let mut events: Vec<(i64, &str, &ops::Change)> = Vec::new();
    for (svc, _) in &scope {
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
    let files = tracked_files(repo)?;
    let frames = ogrep_core::trace::frames(&text);
    let mut found = false;
    for f in &frames {
        let Some(path) = ogrep_core::trace::resolve(&f.raw_path, &files) else {
            continue;
        };
        found = true;
        let full = repo.join(path);
        let source = std::fs::read_to_string(&full).unwrap_or_default();
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
        Some(w) => Some(parse_when(repo, w)?),
        None => None,
    };
    let templates: Vec<_> = drain
        .sorted()
        .into_iter()
        .filter(|t| cutoff.is_none_or(|c| t.first_seen.is_some_and(|f| f >= c)))
        .collect();
    // Each template's search of the code is independent: run them together.
    let codes: Vec<Option<(String, usize, String)>> = match (to_code, repo) {
        (true, Some(repo)) => {
            use rayon::prelude::*;
            templates
                .par_iter()
                .map(|t| emitter(repo, &t.constant_part()))
                .collect()
        }
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
    if let Some(ago) = crate::history_search::duration_secs(when) {
        return Ok(now - ago);
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

/// Code that writes a log message: the first tracked non-log file containing
/// its constant text. The level word and bracketed logger names are not part
/// of what the code writes, so they are dropped, and shorter runs are tried if
/// the whole text is not found (a message may be built from pieces).
fn emitter(repo: &Path, constant: &str) -> Option<(String, usize, String)> {
    let tokens: Vec<&str> = constant
        .split_whitespace()
        .skip_while(|t| {
            let bare = t.trim_matches(|c: char| "[]():".contains(c));
            LEVELS.contains(&bare.to_ascii_uppercase().as_str())
                || (t.starts_with('[') && t.ends_with(']'))
        })
        .collect();
    let mut tries: Vec<String> = vec![tokens.join(" ")];
    for cut in 1..tokens.len().saturating_sub(1).min(4) {
        tries.push(tokens[..tokens.len() - cut].join(" "));
        tries.push(tokens[cut..].join(" "));
    }
    for needle in tries.into_iter().filter(|n| n.len() >= 8) {
        let out = ogrep_core::history::git(repo)
            .args(["grep", "-n", "-F", "-I", "--", &needle])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // Code writes log messages; prose that happens to contain the text
        // does not.
        let prose = [
            ".log:", ".txt:", ".md:", ".rst:", ".adoc:", ".html:", ".po:",
        ];
        let found = text
            .lines()
            .filter(|l| !prose.iter().any(|p| l.contains(p)))
            .find_map(|l| {
                let mut parts = l.splitn(3, ':');
                let (path, line, text) = (parts.next()?, parts.next()?, parts.next()?);
                Some((
                    path.to_string(),
                    line.parse().ok()?,
                    text.trim().to_string(),
                ))
            });
        if found.is_some() {
            return found;
        }
    }
    None
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

fn similar_incidents(repo: &Path, opts: &SearchOptions, out: &mut impl Write) -> Result<bool> {
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
    let texts: Vec<(String, Vec<String>)> = docs
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(repo.join(p))
                .ok()
                .map(|t| ((*p).clone(), terms(&t)))
        })
        .collect();
    let n = texts.len().max(1) as f64;
    let avg = texts.iter().map(|t| t.1.len()).sum::<usize>() as f64 / n;
    // BM25 over the incident notes. Term counts per note, and the number of
    // notes holding each query term, are counted once up front.
    let query_terms: std::collections::BTreeSet<&String> = q.iter().collect();
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
    let df: HashMap<&str, f64> = query_terms
        .iter()
        .map(|t| {
            (
                t.as_str(),
                counts.iter().filter(|c| c.contains_key(t.as_str())).count() as f64,
            )
        })
        .collect();
    let mut scored: Vec<(f64, &str, Vec<&str>)> = texts
        .iter()
        .zip(&counts)
        .map(|((path, doc), count)| {
            let mut score = 0.0;
            let mut hits = Vec::new();
            for t in &query_terms {
                let tf = count.get(t.as_str()).copied().unwrap_or(0) as f64;
                if tf == 0.0 {
                    continue;
                }
                let df = df[t.as_str()];
                let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                let norm = 1.0 - 0.75 + 0.75 * doc.len() as f64 / avg.max(1.0);
                score += idf * tf * 2.2 / (tf + 1.2 * norm);
                hits.push(t.as_str());
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
    for (score, path, hits) in scored.iter().take(10) {
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
