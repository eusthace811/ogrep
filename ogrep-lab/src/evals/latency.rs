//! Latency: wall time of one ogrep invocation per query, as a script or agent
//! calling the command line sees it (process start and output included).
//!
//! As in `scripts/freeze/bench.py`, each query runs several rounds, with the
//! variants shuffled inside each round by a seeded generator, so drift in
//! machine load hits every variant alike. The figure per query is its median;
//! the summary gives percentiles of those medians.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use super::{Metric, Outcome, Progress, Suite, median, ogrep_for, param_usize, percentile, sample};
use crate::cli;
use crate::workspace::Workspace;

pub const SUITE: Suite = Suite {
    id: "latency",
    title: "Latency",
    about: "Times ogrep command-line invocations on identifiers sampled from this repository, several rounds each with variants interleaved. Includes process start, as a script or agent sees it.",
    metrics: &[
        Metric {
            key: "p50_ms",
            label: "p50",
            unit: "ms",
            higher_is_better: false,
        },
        Metric {
            key: "p95_ms",
            label: "p95",
            unit: "ms",
            higher_is_better: false,
        },
        Metric {
            key: "mean_ms",
            label: "Mean",
            unit: "ms",
            higher_is_better: false,
        },
        Metric {
            key: "max_ms",
            label: "Max",
            unit: "ms",
            higher_is_better: false,
        },
    ],
    run,
    ready: |_| Ok(()),
};

/// Variant name, then the flags it adds before `-F -- QUERY`.
pub const VARIANTS: &[(&str, &[&str])] = &[
    ("plain", &["-n"]),
    ("files", &["-l"]),
    ("ignore-case", &["-n", "-i"]),
    ("rank", &["--rank"]),
    ("scope", &["--scope"]),
    ("near", &["--near"]),
];

/// A small seeded generator (SplitMix64) for the shuffles.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

fn run(ws: &Workspace, params: &Value, progress: &Progress) -> Result<Outcome> {
    let n = param_usize(params, "queries", 40, 1000);
    let rounds = param_usize(params, "rounds", 5, 50);
    let wanted: Vec<&str> = params["variants"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let variants: Vec<(&str, &[&str])> = VARIANTS
        .iter()
        .copied()
        .filter(|(name, _)| {
            if wanted.is_empty() {
                matches!(*name, "plain" | "rank")
            } else {
                wanted.contains(name)
            }
        })
        .collect();
    if variants.is_empty() {
        return Err(anyhow!("choose at least one variant"));
    }
    let bin = ogrep_for(ws, params)?;
    progress.phase("sampling queries", 0);
    let queries = sample::identifiers(ws, n);
    if queries.is_empty() {
        return Err(anyhow!("no identifiers found to query"));
    }
    let argv = |flags: &[&str], q: &str| -> Vec<String> {
        flags
            .iter()
            .map(|s| s.to_string())
            .chain(["-F".to_string(), "--".to_string(), q.to_string()])
            .collect()
    };

    // Warm the page cache and any lazily built caches once.
    progress.phase("warming up", queries.len().min(10) * variants.len());
    for q in queries.iter().take(10) {
        for (_, flags) in &variants {
            cli::timed(&bin, ws, &argv(flags, q)).map_err(|e| anyhow!(e))?;
            progress.step();
        }
    }

    let mut times: BTreeMap<(&str, &str), Vec<f64>> = BTreeMap::new();
    let mut rng = Rng(11);
    progress.phase(
        &format!(
            "{} queries x {} variants x {rounds} rounds",
            queries.len(),
            variants.len()
        ),
        queries.len() * rounds,
    );
    for _ in 0..rounds {
        for q in &queries {
            progress.cancelled()?;
            let mut order = variants.clone();
            rng.shuffle(&mut order);
            for (name, flags) in order {
                let d = cli::timed(&bin, ws, &argv(flags, q)).map_err(|e| anyhow!(e))?;
                times
                    .entry((name, q.as_str()))
                    .or_default()
                    .push(d.as_secs_f64() * 1000.0);
            }
            progress.step();
        }
    }

    let medians: BTreeMap<(&str, &str), f64> = times
        .into_iter()
        .map(|(k, mut v)| (k, median(&mut v)))
        .collect();
    let mut summary = Map::new();
    for (name, _) in &variants {
        let mut m: Vec<f64> = queries
            .iter()
            .map(|q| medians[&(*name, q.as_str())])
            .collect();
        let mean = m.iter().sum::<f64>() / m.len() as f64;
        summary.insert(
            name.to_string(),
            json!({
                "p50_ms": percentile(&mut m, 0.5),
                "p95_ms": percentile(&mut m, 0.95),
                "mean_ms": mean,
                "max_ms": m.last().copied().unwrap_or(0.0),
            }),
        );
    }
    let items: Vec<Value> = queries
        .iter()
        .map(|q| {
            let ms: Map<String, Value> = variants
                .iter()
                .map(|(name, _)| (name.to_string(), json!(medians[&(*name, q.as_str())])))
                .collect();
            json!({ "query": q, "ms": ms })
        })
        .collect();
    Ok(Outcome {
        summary,
        items,
        params: json!({
            "queries": queries.len(),
            "rounds": rounds,
            "variants": variants.iter().map(|v| v.0).collect::<Vec<_>>(),
            "ogrep": bin,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffle_is_seeded_and_complete() {
        let mut a: Vec<u32> = (0..10).collect();
        let mut b = a.clone();
        Rng(11).shuffle(&mut a);
        Rng(11).shuffle(&mut b);
        assert_eq!(a, b);
        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
    }
}
