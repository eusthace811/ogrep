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
        assert!(
            actual.iter().all(|id| expected.binary_search(id).is_ok()),
            "positional result must be a subset of the eager candidates for {pattern}"
        );
        let disk_regex = regex::Regex::new(pattern).unwrap();
        if !plan.is_match_all() {
            for id in reader.all_file_ids() {
                let text = std::fs::read_to_string(dir.path().join(reader.file_path(id).unwrap()))
                    .unwrap();
                if disk_regex.is_match(&text) {
                    assert!(actual.contains(&id), "disk false negative for {pattern}");
                }
            }
        }
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

#[test]
fn generated_plans_never_exclude_a_regex_match() {
    use zgrep_core::live::LiveIndex;
    let atoms = [
        "alpha", "beta", "gamma", "connect", "timeout", "foobar", "café", "ſhell", "Kelvin", "aa",
        "x",
    ];
    let mut live = LiveIndex::new();
    let mut texts = vec![
        "CAFÉ".to_string(),
        "KELVIN".to_string(),
        "ſHELL".to_string(),
    ];
    let mut seed = 0x981a2375u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };
    for _ in 0..256 {
        let mut text = String::new();
        for _ in 0..8 {
            text.push_str(atoms[next() % atoms.len()]);
            text.push_str([" ", "\n", "", "-"][next() % 4]);
        }
        texts.push(text);
    }
    for (i, text) in texts.iter().enumerate() {
        live.upsert_file(&i.to_string(), text.as_bytes());
    }
    let mut patterns: Vec<String> = atoms.iter().map(|s| s.to_string()).collect();
    for _ in 0..300 {
        let a = atoms[next() % atoms.len()];
        let b = atoms[next() % atoms.len()];
        let c = atoms[next() % atoms.len()];
        patterns.push(match next() % 6 {
            0 => format!("({a}|{b}).*{c}"),
            1 => format!("{a}.{{0,12}}({b}|{c})"),
            2 => format!("(?:{a})?(?:{b})+"),
            3 => format!("({a}|x)({b}|{c})"),
            4 => format!("{a}[ -z]*{b}"),
            _ => format!("{a}{b}"),
        });
    }
    for ci in [false, true] {
        for pattern in &patterns {
            let re = regex::RegexBuilder::new(pattern)
                .case_insensitive(ci)
                .build()
                .unwrap();
            let plan = query::build_query_plan(pattern, ci).unwrap();
            let candidates = if plan.is_match_all() {
                live.all_file_ids()
            } else {
                query::execute_plan_indexed(&plan, &live, &mut QueryWork::default())
            };
            let paths: BTreeSet<_> = candidates
                .iter()
                .map(|&id| live.file_path(id).unwrap())
                .collect();
            for (i, text) in texts.iter().enumerate() {
                if re.is_match(text) {
                    assert!(
                        paths.contains(i.to_string().as_str()),
                        "lost {pattern:?} ci={ci} in {text:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn fixed_unicode_case_folds_are_sound() {
    let mut live = zgrep_core::live::LiveIndex::new();
    for (pattern, text) in [
        ("kelvin", "KELVIN"),
        ("shell", "ſHELL"),
        ("café", "CAFÉ"),
        ("İstanbul", "İSTANBUL"),
    ] {
        live.upsert_file("sample", text.as_bytes());
        let regex = regex::RegexBuilder::new(&regex::escape(pattern))
            .case_insensitive(true)
            .build()
            .unwrap();
        assert!(regex.is_match(text));
        let plan = query::build_literal_plan(pattern, true);
        let ids = if plan.is_match_all() {
            live.all_file_ids()
        } else {
            query::execute_plan_indexed(&plan, &live, &mut QueryWork::default())
        };
        assert_eq!(
            ids.len(),
            1,
            "fixed-string index excluded {pattern:?} in {text:?}"
        );
    }
}

#[test]
fn positions_reject_scattered_grams_but_never_join_separate_runs() {
    let mut live = zgrep_core::live::LiveIndex::new();
    live.upsert_file("scattered", b"abcdXbcde");
    live.upsert_file("adjacent", b"abcde");
    let plan = query::build_literal_plan("abcde", false);
    let eager = query::execute_plan_with_masks(&plan, &|h| live.lookup_trigram_with_masks(h));
    assert_eq!(eager.len(), 2, "fixture must get through next-byte masks");
    let mut work = QueryWork::default();
    let ids = query::execute_plan_indexed(&plan, &live, &mut work);
    assert_eq!(ids.len(), 1);
    assert_eq!(live.file_path(ids[0]), Some("adjacent"));
    assert_eq!(work.position_rejections, 1);
    live.upsert_file("separate", b"abcde------abcde");
    let plan = query::build_query_plan("abcde.*abcde", false).unwrap();
    let ids = query::execute_plan_indexed(&plan, &live, &mut QueryWork::default());
    assert!(ids.contains(&live.file_id_for_path("separate").unwrap()));
}
