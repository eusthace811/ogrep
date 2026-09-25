//! Measure warm index-opening stages separately from candidate execution.
use std::path::Path;
use std::time::Instant;
use zgrep_core::{meta::IndexMeta, path_index, query, reader::IndexReader};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 2, "profile_open INDEX");
    let index = Path::new(&args[1]);
    let identity: Vec<_> = [
        "meta.json",
        "files.bin",
        "files-extra.bin",
        "lookup.bin",
        "index.bin",
    ]
    .iter()
    .map(|name| {
        let bytes = std::fs::read(index.join(name)).unwrap();
        serde_json::json!({"file": name, "bytes": bytes.len(),
                "blake3": blake3::hash(&bytes).to_hex().to_string()})
    })
    .collect();
    let plan = query::build_literal_plan("ZGREP_ABSENT_OPEN_PROFILE_293847982", false);
    let mut rows = Vec::new();
    for iteration in 0..102 {
        let start = Instant::now();
        let meta = IndexMeta::load(index).unwrap();
        let meta_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        let reader = IndexReader::open(index).unwrap();
        let reader_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        let filenames = path_index::read_filename_index(index).unwrap().unwrap();
        assert!(
            filenames
                .visibility
                .as_ref()
                .unwrap()
                .covers_index(&meta, reader.file_table_id())
        );
        let filenames_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        let scope_meta = IndexMeta::load(index).unwrap();
        let meta_reload_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        let mut work = query::QueryWork::default();
        let ids = query::execute_plan_indexed(&plan, &reader, &mut work);
        let absent_plan_ns = start.elapsed().as_nanos();
        assert!(ids.is_empty());
        let files = reader.num_files();
        let start = Instant::now();
        drop((scope_meta, filenames, reader, meta));
        let drop_ns = start.elapsed().as_nanos();
        if iteration >= 2 {
            rows.push(
                serde_json::json!({"meta_ns": meta_ns, "reader_ns": reader_ns,
                "filenames_ns": filenames_ns, "meta_reload_ns": meta_reload_ns,
                "absent_plan_ns": absent_plan_ns, "drop_ns": drop_ns, "files": files,
                "query_work": work}),
            );
        }
    }
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "description": "Warm in-process opening stages of an existing index snapshot; 2 warmups, 100 samples. Identity reads warm the OS cache. No filesystem matching, CLI startup, or cold-cache claim.",
        "index": index, "identity": identity, "rows": rows
    })).unwrap());
}
