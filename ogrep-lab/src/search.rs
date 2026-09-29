//! Searches and the other pattern-driven features, run through `ogrep --json`.

use serde_json::{Value, json};

use crate::cli;
use crate::http::Request;
use crate::plan::{Case, Pattern};
use crate::workspace::Workspace;

/// Matching options shared by search, plan explain and history.
pub fn pattern(req: &Request) -> Result<Pattern<'_>, String> {
    let text = req
        .param("q")
        .filter(|q| !q.is_empty())
        .ok_or("type a pattern")?;
    Ok(Pattern {
        text,
        fixed: req.flag("fixed"),
        case: match req.param("case") {
            Some("insensitive") => Case::Insensitive,
            Some("sensitive") => Case::Sensitive,
            _ => Case::Smart,
        },
        word: req.flag("word"),
    })
}

/// The ogrep flags for a pattern's matching options, ending with the pattern.
fn matching_args(p: &Pattern) -> Vec<String> {
    let mut args = Vec::new();
    if p.fixed {
        args.push("-F".into());
    }
    args.push(
        match p.case {
            Case::Sensitive => "-s",
            Case::Insensitive => "-i",
            Case::Smart => "-S",
        }
        .into(),
    );
    if p.word {
        args.push("-w".into());
    }
    args
}

/// `-e PATTERN` keeps a pattern that starts with `-` from reading as a flag.
fn pattern_arg(p: &Pattern, args: &mut Vec<String>) {
    args.push("-e".into());
    args.push(p.text.to_string());
}

pub fn search(ws: &Workspace, req: &Request) -> Result<Value, String> {
    let p = pattern(req)?;
    let mut args = matching_args(&p);
    let rank = req.flag("rank");
    if rank {
        args.extend([
            "--rank".into(),
            "--budget".into(),
            req.number("budget", 30, 500).to_string(),
        ]);
    }
    if req.flag("scope") {
        args.push("--scope".into());
    }
    if let Some(k @ ("def" | "ref")) = req.param("kind") {
        args.push(format!("--kind={k}"));
    }
    if let Some(z @ ("code" | "comments" | "strings")) = req.param("in") {
        args.push(format!("--in={z}"));
    }
    let context = req.number("context", 0, 10);
    if context > 0 {
        args.push(format!("-C{context}"));
    }
    for g in req
        .param("glob")
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        args.push(format!("--glob={g}"));
    }
    pattern_arg(&p, &mut args);
    let out = cli::json(ws, &args, 20_000)?;

    let mut files: Vec<Value> = Vec::new();
    let mut ranks: std::collections::HashMap<String, Value> = Default::default();
    let mut summary = Value::Null;
    for rec in out.records {
        let data = &rec["data"];
        match rec["type"].as_str() {
            Some("rank") => {
                if let Some(path) = text(&data["path"]) {
                    ranks.insert(path, data.clone());
                }
            }
            Some("begin") => files.push(json!({ "path": text(&data["path"]), "items": [] })),
            Some(kind @ ("match" | "context" | "scope")) => {
                let Some(file) = files.last_mut() else {
                    continue;
                };
                let spans: Vec<[u64; 2]> = data["submatches"]
                    .as_array()
                    .map(|s| {
                        s.iter()
                            .filter_map(|m| Some([m["start"].as_u64()?, m["end"].as_u64()?]))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut item = json!({
                    "kind": kind,
                    "line": data["line_number"],
                    "text": text(&data["lines"]).map(|t| t.trim_end_matches(['\n', '\r']).to_string()),
                    "spans": spans,
                });
                if kind == "scope" {
                    item["chain"] = data["chain"].clone();
                }
                file["items"].as_array_mut().expect("array").push(item);
            }
            Some("summary") => summary = data["stats"].clone(),
            _ => {}
        }
    }
    for f in &mut files {
        if let Some(r) = f["path"].as_str().and_then(|p| ranks.remove(p)) {
            f["rank"] = r;
        }
    }
    Ok(json!({
        "files": files,
        "summary": summary,
        "truncated": out.truncated,
        "elapsed_ms": out.elapsed.as_secs_f64() * 1000.0,
        "notes": out.notes,
        "command": command_line(&args),
    }))
}

/// ogrep's JSON gives text as `{"text": ...}`, or `{"bytes": base64}` when it
/// is not UTF-8.
fn text(v: &Value) -> Option<String> {
    v.get("text").and_then(Value::as_str).map(str::to_string)
}

/// The features that take a pattern (or a name, path or service) and print
/// records of their own. The mode names are ogrep's flag names.
const MODES: &[&str] = &[
    "near",
    "map",
    "callers",
    "callees",
    "history",
    "introduced",
    "related-tests",
    "similar",
    "entities",
    "blast",
    "suspects",
    "timeline",
    "similar-incidents",
    "log-templates",
    "doc-drift",
    "conflicts",
];

pub fn feature(ws: &Workspace, req: &Request) -> Result<Value, String> {
    let mode = req.param("mode").unwrap_or("");
    if !MODES.contains(&mode) {
        return Err(format!("unknown mode `{mode}`"));
    }
    let mut args = Vec::new();
    match mode {
        "map" => args.push(format!("--map={}", req.number("tokens", 1024, 100_000))),
        m => args.push(format!("--{m}")),
    }
    if matches!(
        mode,
        "near" | "related-tests" | "similar" | "similar-incidents"
    ) {
        args.extend(["--budget".into(), req.number("budget", 20, 500).to_string()]);
    }
    for opt in ["since", "until", "author", "new-since"] {
        if let Some(v) = req.param(opt).filter(|v| !v.is_empty()) {
            args.push(format!("--{opt}={v}"));
        }
    }
    // Modes whose argument is a name, path or service rather than a regex take
    // it verbatim; the rest accept the search options.
    let verbatim = matches!(
        mode,
        "related-tests"
            | "similar"
            | "blast"
            | "suspects"
            | "timeline"
            | "entities"
            | "similar-incidents"
    );
    let q = req.param("q").unwrap_or("");
    match mode {
        "doc-drift" | "conflicts" => {}
        _ if verbatim => {
            if q.is_empty() {
                return Err("type a name".into());
            }
            args.extend(["--".into(), q.to_string()]);
        }
        _ => {
            let p = pattern(req)?;
            args.extend(matching_args(&p));
            pattern_arg(&p, &mut args);
        }
    }
    let out = cli::json(ws, &args, req.number("limit", 2000, 20_000))?;
    Ok(json!({
        "mode": mode,
        "records": out.records,
        "truncated": out.truncated,
        "elapsed_ms": out.elapsed.as_secs_f64() * 1000.0,
        "notes": out.notes,
        "command": command_line(&args),
    }))
}

/// The equivalent command line, to copy into a terminal.
fn command_line(args: &[String]) -> String {
    let mut s = String::from("ogrep");
    for a in args.iter().filter(|a| *a != "--json") {
        s.push(' ');
        if a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=./,:".contains(c))
        {
            s.push_str(a);
        } else {
            s.push('\'');
            s.push_str(&a.replace('\'', r"'\''"));
            s.push('\'');
        }
    }
    s
}

/// A commit's message and diff, from git.
pub fn commit(ws: &Workspace, sha: &str) -> Result<Value, String> {
    if !(7..=40).contains(&sha.len()) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("not a commit id".into());
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&ws.root)
        .args([
            "-c",
            "core.quotepath=off",
            "show",
            "--no-color",
            "--no-ext-diff",
            "--format=%H%x00%an <%ae>%x00%at%x00%B%x00",
            "--patch",
            "--stat=200",
            sha,
        ])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    const CAP: usize = 600 * 1024;
    let truncated = out.stdout.len() > CAP;
    let raw = String::from_utf8_lossy(&out.stdout[..out.stdout.len().min(CAP)]).into_owned();
    let mut parts = raw.splitn(5, '\0');
    let (sha, author, time, message, rest) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    Ok(json!({
        "sha": sha,
        "author": author,
        "time": time.parse::<i64>().ok(),
        "message": message.trim_end(),
        "diff": rest.trim_start_matches('\n'),
        "truncated": truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_what_the_shell_needs() {
        let args: Vec<String> = ["--json", "-S", "-e", "fn foo(", "--glob=*.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(command_line(&args), "ogrep -S -e 'fn foo(' '--glob=*.rs'");
        let q: Vec<String> = vec!["it's".into()];
        assert_eq!(command_line(&q), r"ogrep 'it'\''s'");
    }
}
