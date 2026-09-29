//! Relatedness evaluation for the file graph: files changed together are
//! related. For recent commits touching 2-8 indexed files, each file seeds the
//! graph and the other files of the commit are the answers; recall@10 and MRR
//! compare scoring variants.
//!
//! Usage: cargo run --release -p ogrep-core --example near_eval -- REPO INDEX_DIR [COMMITS]
use std::collections::HashSet;
use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let repo = args.next().expect("repo");
    let index = args.next().expect("index dir");
    let limit: usize = args.next().map_or(400, |n| n.parse().unwrap());
    let (g, _) = ogrep_core::graph::open(Path::new(&index)).expect("graph");
    let log = std::process::Command::new("git")
        .args([
            "-C",
            &repo,
            "log",
            "--no-merges",
            "--format=format:@@",
            "--name-only",
            "-n",
            "4000",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&log.stdout);
    let mut cases: Vec<Vec<u32>> = Vec::new();
    for block in text.split("@@").skip(1) {
        let files: Vec<u32> = block
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| g.file(l.trim()))
            .collect();
        if (2..=8).contains(&files.len()) {
            cases.push(files);
        }
        if cases.len() >= limit {
            break;
        }
    }
    // Total link weight touching each file, for degree normalisation.
    let mut degree = vec![0f64; g.files.len()];
    for a in 0..g.files.len() as u32 {
        for (t, w) in g.out_links(a) {
            degree[a as usize] += w as f64;
            degree[t as usize] += w as f64;
        }
    }
    let one_hop = |f: u32| {
        let mut s = vec![0f64; g.files.len()];
        for (t, w) in g.out_links(f) {
            s[t as usize] += w as f64;
        }
        for a in 0..g.files.len() as u32 {
            for (t, w) in g.out_links(a) {
                if t == f {
                    s[a as usize] += w as f64;
                }
            }
        }
        s
    };
    type Scorer<'a> = &'a dyn Fn(u32) -> Vec<f64>;
    let variants: [(&str, Scorer); 6] = [
        ("1hop/sqrtdeg", &|f| {
            one_hop(f)
                .iter()
                .zip(&degree)
                .map(|(s, d)| s / d.max(1.0).sqrt())
                .collect()
        }),
        ("1hop/logdeg", &|f| {
            one_hop(f)
                .iter()
                .zip(&degree)
                .map(|(s, d)| s / (2.0 + d).ln())
                .collect()
        }),
        ("1hop+lift", &|f| {
            let h = one_hop(f);
            let l = g.related(&[(f, 1.0)]);
            let (hm, lm) = (
                h.iter().cloned().fold(1e-12, f64::max),
                l.iter().cloned().fold(1e-12, f64::max),
            );
            h.iter()
                .zip(&l)
                .map(|(a, b)| a / hm + 0.3 * b / lm)
                .collect()
        }),
        ("ppr", &|f| g.propagate(Some(&[(f, 1.0)]))),
        ("lift", &|f| g.related(&[(f, 1.0)])),
        ("1-hop", &|f| {
            let mut s = vec![0f64; g.files.len()];
            for (t, w) in g.out_links(f) {
                s[t as usize] += w as f64;
            }
            for a in 0..g.files.len() as u32 {
                for (t, w) in g.out_links(a) {
                    if t == f {
                        s[a as usize] += w as f64;
                    }
                }
            }
            s
        }),
    ];
    for (name, score) in variants {
        let (mut recall, mut mrr, mut n) = (0.0, 0.0, 0.0);
        for case in &cases {
            for &seed in case {
                let s = score(seed);
                let mut order: Vec<u32> =
                    (0..g.files.len() as u32).filter(|&f| f != seed).collect();
                order.sort_by(|&a, &b| s[b as usize].total_cmp(&s[a as usize]).then(a.cmp(&b)));
                let truth: HashSet<u32> = case.iter().copied().filter(|&f| f != seed).collect();
                let top: Vec<u32> = order.iter().take(10).copied().collect();
                recall +=
                    top.iter().filter(|f| truth.contains(f)).count() as f64 / truth.len() as f64;
                mrr += order
                    .iter()
                    .position(|f| truth.contains(f))
                    .map_or(0.0, |p| 1.0 / (p + 1) as f64);
                n += 1.0;
            }
        }
        println!(
            "{name:>6}: {} seeds, recall@10 {:.3}, MRR {:.3}",
            n,
            recall / n,
            mrr / n
        );
    }
}
