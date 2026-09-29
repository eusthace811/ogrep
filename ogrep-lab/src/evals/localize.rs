//! Localization: given one identifier from a commit subject, where in the
//! results does a file the commit changed first appear?
//!
//! The dataset is mined from the repository's own history, as in
//! `scripts/eval/localize.py`: for each recent non-merge commit that changed
//! one to four non-test source files, take the most specific identifier in its
//! subject. The files it changed that still contain the identifier at HEAD are
//! the answers. That is the task an agent faces with a name from a bug report.
//!
//! Each variant lists the matching files in some order; the metric is the
//! position of the first relevant one.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Result, anyhow};
use rayon::prelude::*;
use serde_json::{Map, Value, json};

use super::sample::is_test;
use super::{Metric, Outcome, Progress, Suite, median, ogrep_for, param_usize, pool};
use crate::cli;
use crate::workspace::Workspace;

pub const SUITE: Suite = Suite {
    id: "localize",
    title: "Ranking: localization",
    about: "Mines commits from git history (subject identifier → files changed) and measures where the first changed file lands in each result order. Plain is ogrep's default order; rank is --rank.",
    metrics: &[
        Metric {
            key: "acc@1",
            label: "Acc@1",
            unit: "pct",
            higher_is_better: true,
        },
        Metric {
            key: "acc@5",
            label: "Acc@5",
            unit: "pct",
            higher_is_better: true,
        },
        Metric {
            key: "acc@10",
            label: "Acc@10",
            unit: "pct",
            higher_is_better: true,
        },
        Metric {
            key: "mrr",
            label: "MRR",
            unit: "ratio",
            higher_is_better: true,
        },
        Metric {
            key: "found",
            label: "Found",
            unit: "pct",
            higher_is_better: true,
        },
        Metric {
            key: "median_files",
            label: "Median files listed",
            unit: "count",
            higher_is_better: false,
        },
    ],
    run,
    ready,
};

const CODE_EXT: &[&str] = &[
    "py", "rs", "c", "h", "cc", "cpp", "hpp", "go", "js", "jsx", "ts", "tsx", "java", "kt", "rb",
    "php", "cs", "swift", "scala",
];
const STOP: &[&str] = &[
    "Refs",
    "Fixed",
    "Fixes",
    "Added",
    "Removed",
    "Changed",
    "Updated",
    "Improved",
    "Corrected",
    "Reverted",
    "Merge",
    "Revert",
    "Cleanup",
    "Rollup",
];
pub const VARIANTS: &[&str] = &["plain", "path", "count", "rank"];

fn ready(ws: &Workspace) -> Result<(), String> {
    if !ws.root.join(".git").exists() {
        return Err("needs a git repository".into());
    }
    Ok(())
}

/// The subject's identifiers that name something specific (snake_case,
/// camelCase or dotted, six or more characters), most specific first.
fn specific_identifiers(subject: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = subject.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
        {
            i += 1;
        }
        let token = subject[start..i].trim_end_matches('.');
        let last = token.rsplit('.').next().unwrap_or(token);
        if last.len() < 6 || STOP.contains(&last) || last.as_bytes()[0].is_ascii_digit() {
            continue;
        }
        let snake = last.trim_matches('_').contains('_');
        let camel = last
            .as_bytes()
            .windows(2)
            .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase());
        if snake || camel || token.contains('.') {
            out.push(last.to_string());
        }
    }
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    out.dedup();
    out
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn mine(ws: &Workspace, n: usize, all_parents: bool, progress: &Progress) -> Result<Vec<Value>> {
    progress.phase("mining commits from git history", 0);
    let mut args = vec![
        "log",
        "--no-merges",
        "--format=%x01%H%x02%s",
        "--name-only",
        "-n",
        "20000",
    ];
    if !all_parents {
        args.push("--first-parent");
    }
    let log = git(&ws.root, &args)?;
    let head: HashSet<String> = git(&ws.root, &["ls-files"])?
        .lines()
        .map(str::to_string)
        .collect();
    let mut items = Vec::new();
    for block in log.split('\u{1}').skip(1) {
        let (header, names) = block.split_once('\n').unwrap_or((block, ""));
        let (sha, subject) = header.split_once('\u{2}').unwrap_or((header, ""));
        let src: Vec<&str> = names
            .lines()
            .map(str::trim)
            .filter(|f| {
                !f.is_empty()
                    && Path::new(f)
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| CODE_EXT.contains(&e))
                    && !is_test(f)
                    && head.contains(*f)
            })
            .collect();
        if !(1..=4).contains(&src.len()) {
            continue;
        }
        for ident in specific_identifiers(subject) {
            let relevant: Vec<&str> = src
                .iter()
                .copied()
                .filter(|f| {
                    std::fs::read(ws.root.join(f))
                        .is_ok_and(|b| memchr::memmem::find(&b, ident.as_bytes()).is_some())
                })
                .collect();
            if !relevant.is_empty() {
                let mut relevant = relevant;
                relevant.sort_unstable();
                items.push(json!({ "commit": sha, "subject": subject, "query": ident, "relevant": relevant }));
                break;
            }
        }
        if items.len() >= n {
            break;
        }
    }
    progress.log(&format!("{} tasks mined", items.len()));
    Ok(items)
}

fn order(bin: &Path, ws: &Workspace, variant: &str, q: &str) -> Result<Vec<String>, String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let args = match variant {
        "plain" => s(&["-l", "-F", "--", q]),
        "path" => s(&["-l", "-F", "--sort=path", "--", q]),
        "count" => s(&["-c", "-F", "--", q]),
        "rank" => s(&["-l", "-F", "--rank", "--budget", "100000", "--", q]),
        _ => return Err(format!("unknown variant {variant}")),
    };
    let (lines, ..) = cli::run_with(bin, ws, &args, 1_000_000, Duration::from_secs(120))?;
    let lines = lines
        .into_iter()
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    if variant == "count" {
        let mut counted: Vec<(i64, String)> = lines
            .filter_map(|l| {
                let (path, n) = l.rsplit_once(':')?;
                Some((-n.parse::<i64>().ok()?, path.to_string()))
            })
            .collect();
        counted.sort();
        return Ok(counted.into_iter().map(|(_, p)| p).collect());
    }
    Ok(lines.collect())
}

fn run(ws: &Workspace, params: &Value, progress: &Progress) -> Result<Outcome> {
    let n = param_usize(params, "tasks", 200, 2000);
    let all_parents = params["all_parents"].as_bool().unwrap_or(false);
    let variants: Vec<&str> = match params["variants"].as_array() {
        Some(v) if !v.is_empty() => v
            .iter()
            .filter_map(Value::as_str)
            .filter(|v| VARIANTS.contains(v))
            .collect(),
        _ => VARIANTS.to_vec(),
    };
    let bin = ogrep_for(ws, params)?;
    let items = mine(ws, n, all_parents, progress)?;
    if items.is_empty() {
        return Err(anyhow!(
            "no tasks found: no recent commit changed 1-4 source files with a specific identifier in its subject{}",
            if all_parents {
                ""
            } else {
                "; try following merged branches"
            }
        ));
    }

    progress.phase(
        &format!("running {} tasks x {} orders", items.len(), variants.len()),
        items.len(),
    );
    let workers = pool(4);
    let rows: Vec<Result<Value, String>> = workers.install(|| {
        items
            .par_iter()
            .map(|item| {
                if progress.cancelled().is_err() {
                    return Err("cancelled".to_string());
                }
                let q = item["query"].as_str().unwrap_or("");
                let relevant: HashSet<&str> = item["relevant"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let mut positions = Map::new();
                let mut listed = Map::new();
                for v in &variants {
                    let files = order(&bin, ws, v, q)?;
                    let pos = files
                        .iter()
                        .position(|f| relevant.contains(f.as_str()))
                        .map(|p| p + 1);
                    positions.insert(v.to_string(), json!(pos));
                    listed.insert(v.to_string(), json!(files.len()));
                }
                progress.step();
                let mut row = item.clone();
                row["positions"] = Value::Object(positions);
                row["listed"] = Value::Object(listed);
                Ok(row)
            })
            .collect()
    });
    progress.cancelled()?;
    let rows: Vec<Value> = rows
        .into_iter()
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow!(e))?;

    let mut summary = Map::new();
    for v in &variants {
        let pos: Vec<Option<f64>> = rows.iter().map(|r| r["positions"][v].as_f64()).collect();
        let mut listed: Vec<f64> = rows
            .iter()
            .filter_map(|r| r["listed"][v].as_f64())
            .collect();
        let total = pos.len() as f64;
        let acc = |k: f64| pos.iter().filter(|p| p.is_some_and(|p| p <= k)).count() as f64 / total;
        summary.insert(
            v.to_string(),
            json!({
                "acc@1": acc(1.0),
                "acc@5": acc(5.0),
                "acc@10": acc(10.0),
                "mrr": pos.iter().flatten().map(|p| 1.0 / p).sum::<f64>() / total,
                "found": pos.iter().filter(|p| p.is_some()).count() as f64 / total,
                "median_files": median(&mut listed),
            }),
        );
    }
    Ok(Outcome {
        summary,
        items: rows,
        params: json!({ "tasks": n, "all_parents": all_parents, "variants": variants, "ogrep": bin }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_specific_identifiers_most_specific_first() {
        assert_eq!(
            specific_identifiers("Fixed #3 -- Made QuerySet.bulk_update() respect db_default."),
            vec!["bulk_update", "db_default"]
        );
        assert_eq!(
            specific_identifiers("Refs #1 -- Updated docs."),
            Vec::<String>::new()
        );
        assert_eq!(
            specific_identifiers("Use getValueAt in parser"),
            vec!["getValueAt"]
        );
    }
}
