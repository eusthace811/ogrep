//! Isolate candidate planning, excluding process startup, parsing and file I/O.
//! The eager comparison uses the SAME plan; it isolates execution, not parser changes.
use ogrep_core::{query, reader::IndexReader};
use std::cell::Cell;
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3, "profile_plan INDEX QUERIES_JSON");
    let reader = IndexReader::open(std::path::Path::new(&args[1])).unwrap();
    let queries: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&args[2]).unwrap()).unwrap();
    let mut rows = Vec::new();
    for q in queries.as_array().unwrap() {
        let has = |flag: &str| {
            q["flags"]
                .as_array()
                .is_some_and(|fs| fs.iter().any(|f| f.as_str() == Some(flag)))
        };
        let pattern = q["pattern"].as_str().unwrap();
        let plan = if has("-F") {
            query::build_literal_plan(pattern, has("-i"))
        } else {
            query::build_query_plan(pattern, has("-i")).unwrap()
        };
        if plan.is_match_all() {
            continue;
        }
        let copied = Cell::new(0);
        let eager = query::execute_plan_with_masks(&plan, &|hash| {
            let entries = reader.lookup_trigram_with_masks(hash);
            copied.set(copied.get() + entries.len());
            entries
        });
        let mut work = query::QueryWork::default();
        let optimized = query::execute_plan_indexed(&plan, &reader, &mut work);
        assert!(optimized.iter().all(|id| eager.binary_search(id).is_ok()));
        let mut samples = [Vec::new(), Vec::new()];
        for i in 0..102 {
            for mode in [i % 2, (i + 1) % 2] {
                let start = Instant::now();
                let ids = if mode == 0 {
                    query::execute_plan_with_masks(&plan, &|hash| {
                        reader.lookup_trigram_with_masks(hash)
                    })
                } else {
                    query::execute_plan_indexed(&plan, &reader, &mut query::QueryWork::default())
                };
                std::hint::black_box(ids);
                if i >= 2 {
                    samples[mode].push(start.elapsed().as_nanos() as u64);
                }
            }
        }
        rows.push(
            serde_json::json!({"name": q["name"], "eager_candidates": eager.len(),
            "optimized_candidates": optimized.len(), "eager_postings_copied": copied.get(),
            "optimized_work": work, "eager_ns": samples[0], "optimized_ns": samples[1]}),
        );
    }
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({"description": "Warm candidate execution on the same new plan; excludes parsing, path filters and verification", "rows": rows})).unwrap());
}
