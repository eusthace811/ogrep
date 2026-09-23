//! Differential checks against the original eager executor and exact regexes.
use std::collections::BTreeSet;
use zgrep_core::query::{self, PostingSource, QueryWork};
use zgrep_core::{hybrid::HybridIndex, reader::IndexReader};

fn corpus() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    for i in 0..192 {
        let text = format!(
            "fn common_function_{i}() {{ let alpha = {i}; }}\n{}\n{}\n",
            if i % 17 == 0 {
                "connect timeout PyUnicode_DecodeUTF8Stateful"
            } else {
                "accept return common_function"
            },
            if i % 11 == 0 {
                "FOOBAR Kelvin ſhell café\nalpha beta"
            } else {
                "unrelated gamma"
            }
        );
        std::fs::write(dir.path().join(format!("file_{i:04}.rs")), text).unwrap();
    }
    let index = dir.path().join(".zgrep");
    zgrep_core::builder::build_index(dir.path(), Some(&index), false, false, &[]).unwrap();
    (dir, index)
}

#[test]
fn disk_and_live_results_cover_exact_matches() {
    let (dir, index) = corpus();
    let reader = IndexReader::open(&index).unwrap();
    let mut hybrid = HybridIndex::open(&index, dir.path()).unwrap();
    // A rare gram that only exists in the overlay must not be mistaken for an
    // absent disk gram. Old versions and deleted paths must not leak through.
    hybrid
        .live
        .upsert_file("new.rs", b"unique_overlay_token connect timeout");
    hybrid
        .live
        .upsert_file("file_0000.rs", b"changed_file new contents");
    hybrid.live.delete_file("file_0017.rs");
    let patterns = [
        "common_function",
        "DecodeUTF8Stateful",
        "missing_QZX999",
        "(connect|accept).*timeout",
        "(alpha|gamma).*(beta|unrelated)",
        "(?i)foobar",
        "(?i)kelvin",
        "(?i)shell",
        "café",
        "unique_overlay_token",
        "alpha.*beta",
        "common.*common",
        "foo|a",
        ".*",
        "(foo)?bar",
        "(?:connect|missing)+",
        "alpha.{0,5}beta",
    ];
    for pattern in patterns {
        let plan = query::build_query_plan(pattern, false).unwrap();
        let expected =
            query::execute_plan_with_masks(&plan, &|h| reader.lookup_trigram_with_masks(h));
        let actual = query::execute_plan_indexed(&plan, &reader, &mut QueryWork::default());
        assert_eq!(actual, expected, "eager/lazy mismatch for {pattern}");
        let (ids, snapshot) = hybrid.execute_query_with_masks(&plan);
        let paths: BTreeSet<_> = ids
            .iter()
            .map(|&id| hybrid.resolve_path(id, &snapshot).unwrap())
            .collect();
        let regex = regex::Regex::new(pattern).unwrap();
        for path in hybrid.all_paths() {
            let bytes = if path == "new.rs" {
                b"unique_overlay_token connect timeout".to_vec()
            } else if path == "file_0000.rs" {
                b"changed_file new contents".to_vec()
            } else {
                std::fs::read(dir.path().join(&path)).unwrap()
            };
            if regex.is_match(std::str::from_utf8(&bytes).unwrap()) {
                assert!(paths.contains(&path), "false negative: {pattern} in {path}");
            }
        }
        assert!(!paths.contains("file_0017.rs"));
    }
}

#[test]
fn sparse_probes_equal_full_posting_membership() {
    let (_dir, index) = corpus();
    let reader = IndexReader::open(&index).unwrap();
    for i in 0..reader.num_trigrams() {
        let (hash, all) = reader.trigram_posting_at(i);
        for stride in [1, 7, 41, 999] {
            let candidates: Vec<u32> = (0..256).step_by(stride).collect();
            let expected: Vec<_> = all
                .iter()
                .filter(|p| candidates.binary_search(&p.file_id).is_ok())
                .map(|p| (p.file_id, p.loc_mask, p.next_mask))
                .collect();
            let actual: Vec<_> = reader
                .probe(hash, &candidates)
                .iter()
                .map(|p| (p.file_id, p.loc_mask, p.next_mask))
                .collect();
            assert_eq!(actual, expected);
        }
    }
    assert!(reader.probe(u32::MAX, &[0, 1, u32::MAX]).is_empty());
    assert!(reader.probe(0, &[]).is_empty());
}

#[test]
fn missing_required_gram_does_not_load_other_lists() {
    let (_dir, index) = corpus();
    let reader = IndexReader::open(&index).unwrap();
    let plan = query::build_literal_plan("common_function_QZX999", false);
    let mut work = QueryWork::default();
    assert!(query::execute_plan_indexed(&plan, &reader, &mut work).is_empty());
    assert_eq!(work.lists_loaded, 0);
    assert_eq!(work.postings_copied, 0);
}
