//! History parity: `ogrep --history` must list exactly the commits
//! `git log -G` lists, as in `scripts/eval/history_check.py`.
//!
//! Patterns are identifiers (no regex metacharacters, so git's and ogrep's
//! regex dialects agree). git's answer is limited to the commits the history
//! index covers: up to the head it was built at, within its depth, without a
//! shallow clone's boundary commits.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use rayon::prelude::*;
use serde_json::{Map, Value, json};

use super::{Metric, Outcome, Progress, Suite, git, median, ogrep_for, param_usize, pool, sample};
use crate::cli;
use crate::workspace::Workspace;

pub const SUITE: Suite = Suite {
    id: "history",
    title: "History parity",
    about: "Checks that ogrep --history finds exactly the commits git log -G finds, for identifiers sampled from this repository, and times both.",
    metrics: &[
        Metric {
            key: "agreement",
            label: "Agreement",
            unit: "pct",
            higher_is_better: true,
        },
        Metric {
            key: "mismatches",
            label: "Mismatches",
            unit: "count",
            higher_is_better: false,
        },
        Metric {
            key: "ogrep_ms",
            label: "ogrep median",
            unit: "ms",
            higher_is_better: false,
        },
        Metric {
            key: "git_ms",
            label: "git log -G median",
            unit: "ms",
            higher_is_better: false,
        },
    ],
    run,
    ready,
};

fn ready(ws: &Workspace) -> Result<(), String> {
    if ws.history().is_none() {
        return Err("needs the history index: run `ogrep index --history`".into());
    }
    Ok(())
}

fn run(ws: &Workspace, params: &Value, progress: &Progress) -> Result<Outcome> {
    let n = param_usize(params, "patterns", 20, 500);
    let bin = ogrep_for(ws, params)?;
    let history = ws.history().ok_or_else(|| anyhow!("no history index"))?;
    let meta = &history.meta;

    progress.phase("listing the commits the history index covers", 0);
    let depth = meta.depth.map(|d| format!("--max-count={d}"));
    let mut args = vec!["log", "--no-merges", "--format=%H %ct"];
    if let Some(d) = &depth {
        args.push(d);
    }
    args.push(&meta.head);
    let listing = git(&ws.root, &args)?;
    let listed: Vec<(&str, i64)> = listing
        .lines()
        .filter_map(|l| {
            let (sha, t) = l.split_once(' ')?;
            Some((sha, t.parse().ok()?))
        })
        .collect();
    let boundary: HashSet<String> = ogrep_core::history::shallow_boundary(&ws.root);
    let covered: HashSet<&str> = listed
        .iter()
        .map(|r| r.0)
        .filter(|s| !boundary.contains(*s))
        .collect();
    // Bound git's walk to the covered period, plus a day of slack.
    let since = meta
        .depth
        .and_then(|_| listed.iter().map(|r| r.1).min())
        .map(|t| format!("--since={}", t - 86_400));

    let patterns: Vec<String> = sample::identifiers(ws, n);
    progress.phase(
        &format!("comparing {} patterns", patterns.len()),
        patterns.len(),
    );
    let jobs = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(2, 8);
    let rows: Vec<Result<Value, String>> = pool(jobs).install(|| {
        patterns
            .par_iter()
            .map(|q| {
                if progress.cancelled().is_err() {
                    return Err("cancelled".to_string());
                }
                let t0 = Instant::now();
                let args: Vec<String> = ["--history", "--json", "--", q.as_str()]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let lines = cli::run(&bin, ws, &args, 1_000_000, Duration::from_secs(300))?.lines;
                let ogrep_ms = t0.elapsed().as_secs_f64() * 1000.0;
                let ours: HashSet<String> = lines
                    .iter()
                    .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                    .filter_map(|v| v["data"]["sha"].as_str().map(str::to_string))
                    .collect();
                let t1 = Instant::now();
                let mut gargs = vec!["log", "--no-merges", "--format=%H"];
                if let Some(s) = &since {
                    gargs.push(s);
                }
                gargs.extend(["-G", q.as_str(), meta.head.as_str()]);
                let theirs: HashSet<String> = git(&ws.root, &gargs)
                    .map_err(|e| e.to_string())?
                    .split_whitespace()
                    .filter(|s| covered.contains(s))
                    .map(str::to_string)
                    .collect();
                let git_ms = t1.elapsed().as_secs_f64() * 1000.0;
                let mut missing: Vec<&String> = theirs.difference(&ours).collect();
                let mut extra: Vec<&String> = ours.difference(&theirs).collect();
                missing.sort();
                extra.sort();
                progress.step();
                Ok(json!({
                    "query": q,
                    "commits": theirs.len(),
                    "ogrep_commits": ours.len(),
                    "missing": missing.iter().take(5).collect::<Vec<_>>(),
                    "extra": extra.iter().take(5).collect::<Vec<_>>(),
                    "agree": missing.is_empty() && extra.is_empty(),
                    "ogrep_ms": ogrep_ms,
                    "git_ms": git_ms,
                }))
            })
            .collect()
    });
    progress.cancelled()?;
    let items: Vec<Value> = rows
        .into_iter()
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow!(e))?;
    let agree = items
        .iter()
        .filter(|r| r["agree"].as_bool() == Some(true))
        .count();
    let mut o: Vec<f64> = items
        .iter()
        .filter_map(|r| r["ogrep_ms"].as_f64())
        .collect();
    let mut g: Vec<f64> = items.iter().filter_map(|r| r["git_ms"].as_f64()).collect();
    let mut summary = Map::new();
    summary.insert(
        "history".into(),
        json!({
            "agreement": agree as f64 / items.len().max(1) as f64,
            "mismatches": items.len() - agree,
            "ogrep_ms": median(&mut o),
            "git_ms": median(&mut g),
        }),
    );
    Ok(Outcome {
        summary,
        items,
        params: json!({ "patterns": patterns.len(), "history_head": meta.head, "commits": covered.len(), "ogrep": bin, "git_jobs": jobs }),
    })
}
