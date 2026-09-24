//! Research-only sparse gram experiment; not a production index format.
//! Stack algorithms adapted from Danila Kutenin's sparse_ngrams (MIT), commit
//! 430f3befac3bce593c2aa77d2fd0a727d2089d41. See experiments/LICENSE-sparse-ngrams.
//! Uses a portable unsigned-byte weight instead of C++'s platform-dependent char.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::Instant;
use zgrep_core::{live::LiveIndex, query, reader::IndexReader};

fn weight(pair: &[u8]) -> u32 {
    let a = (pair[0] as u64)
        .wrapping_mul(0xc6a4a7935bd1e995)
        .wrapping_add((pair[1] as u64).wrapping_mul(0x0228876a7198b743));
    a.wrapping_add(!a >> 47) as u32
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}

fn all(bytes: &[u8]) -> Vec<u64> {
    let mut stack: Vec<(u32, usize)> = Vec::new();
    let mut grams = Vec::new();
    for i in 0..bytes.len().saturating_sub(1) {
        let w = weight(&bytes[i..i + 2]);
        while stack.last().is_some_and(|&(v, _)| w > v) {
            let start = stack.last().unwrap().1;
            if i + 2 - start <= 32 {
                grams.push(hash(&bytes[start..i + 2]));
            }
            while stack.len() > 1 && stack[stack.len() - 1].0 == stack[stack.len() - 2].0 {
                stack.pop();
            }
            stack.pop();
        }
        if let Some(&(_, start)) = stack.last()
            && i + 2 - start <= 32
        {
            grams.push(hash(&bytes[start..i + 2]));
        }
        stack.push((w, i));
    }
    grams
}

fn covering(bytes: &[u8]) -> Option<Vec<u64>> {
    if bytes.len() < 3 {
        return None;
    }
    let mut stack: VecDeque<(u32, usize)> = VecDeque::new();
    let mut ranges = Vec::new();
    for i in 0..bytes.len() - 1 {
        let w = weight(&bytes[i..i + 2]);
        if stack.len() > 1 && i - stack[0].1 + 3 >= 16 {
            ranges.push((stack[0].1, stack[1].1 + 2));
            stack.pop_front();
        }
        while stack.back().is_some_and(|&(v, _)| w > v) {
            if stack.front().unwrap().0 == stack.back().unwrap().0 {
                ranges.push((stack.back().unwrap().1, i + 2));
                while stack.len() > 1 {
                    let end = stack.pop_back().unwrap().1 + 2;
                    ranges.push((stack.back().unwrap().1, end));
                }
            }
            stack.pop_back();
        }
        stack.push_back((w, i));
    }
    while stack.len() > 1 {
        let end = stack.pop_back().unwrap().1 + 2;
        ranges.push((stack.back().unwrap().1, end));
    }
    if ranges.is_empty() || ranges.iter().any(|&(s, e)| e - s > 32) {
        return None;
    }
    Some(
        ranges
            .into_iter()
            .map(|(s, e)| hash(&bytes[s..e]))
            .collect(),
    )
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 4, "sparse_probe ROOT INDEX QUERIES_JSON");
    let reader = IndexReader::open(Path::new(&args[2])).unwrap();
    let mut paths: Vec<_> = reader
        .all_paths()
        .iter()
        .filter(|p| {
            [".rs", ".c", ".h", ".py"]
                .iter()
                .any(|ext| p.ends_with(ext))
        })
        .collect();
    // Independent of iteration order and deterministic across runs.
    paths.sort_by_key(|p| hash(p.as_bytes()));
    let docs: Vec<_> = paths
        .into_iter()
        .filter_map(|p| {
            let data = std::fs::read(Path::new(&args[1]).join(p)).ok()?;
            (data.len() <= 65536).then_some((p, data))
        })
        .take(2048)
        .collect();
    let mut identity = blake3::Hasher::new();
    for (p, d) in &docs {
        identity.update(p.as_bytes());
        identity.update(&[0]);
        identity.update(blake3::hash(d).as_bytes());
    }
    let mut live = LiveIndex::new();
    let start = Instant::now();
    for (id, (_, data)) in docs.iter().enumerate() {
        live.upsert_file(&id.to_string(), data);
    }
    let trigram_seconds = start.elapsed().as_secs_f64();
    let mut sparse: HashMap<u64, Vec<u32>> = HashMap::new();
    let start = Instant::now();
    for (id, (_, data)) in docs.iter().enumerate() {
        let mut keys: HashSet<_> = all(data).into_iter().collect();
        keys.extend(all(&data.to_ascii_lowercase()));
        for key in keys {
            sparse.entry(key).or_default().push(id as u32);
        }
    }
    let sparse_seconds = start.elapsed().as_secs_f64();
    let queries: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&args[3]).unwrap()).unwrap();
    let mut rows = Vec::new();
    for q in queries.as_array().unwrap() {
        let flags = q["flags"].as_array().unwrap();
        if !flags.iter().any(|f| f == "-F") || flags.iter().any(|f| f == "-i") {
            continue;
        }
        let pattern = q["pattern"].as_str().unwrap();
        let plan = query::build_literal_plan(pattern, false);
        let masked = if plan.is_match_all() {
            live.all_file_ids()
        } else {
            query::execute_plan_indexed(&plan, &live, &mut query::QueryWork::default())
        };
        let grams = covering(pattern.as_bytes());
        let mut candidates: Vec<u32> = (0..docs.len() as u32).collect();
        if let Some(grams) = &grams {
            for g in grams {
                if let Some(ids) = sparse.get(g) {
                    candidates.retain(|id| ids.binary_search(id).is_ok());
                } else {
                    candidates.clear();
                    break;
                }
            }
        }
        let mut exact = 0;
        for (id, (_, data)) in docs.iter().enumerate() {
            if data.windows(pattern.len()).any(|w| w == pattern.as_bytes()) {
                exact += 1;
                assert!(
                    candidates.contains(&(id as u32)),
                    "sparse false negative: {pattern}"
                );
                assert!(
                    masked.contains(&((id as u32) | zgrep_core::live::OVERLAY_BIT)),
                    "masked false negative"
                );
            }
        }
        rows.push(serde_json::json!({"name": q["name"], "grams": grams.map(|g| g.len()), "masked_candidates": masked.len(), "sparse_candidates": candidates.len(), "exact_files": exact}));
    }
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "description": "Research prototype on 2048 deterministically selected source files <=64 KiB. In-memory construction costs are not directly equivalent production implementations; no serialized sparse format. Candidate counts exclude path filters.",
        "sample_identity": identity.finalize().to_hex().as_str(), "files": docs.len(), "bytes": docs.iter().map(|(_,d)|d.len()).sum::<usize>(),
        "trigram_keys": live.num_trigrams(), "trigram_postings": live.inverted_index().values().map(HashSet::len).sum::<usize>(), "trigram_build_seconds": trigram_seconds,
        "sparse_keys": sparse.len(), "sparse_postings": sparse.values().map(Vec::len).sum::<usize>(), "sparse_build_seconds": sparse_seconds, "queries": rows
    })).unwrap());
}

#[test]
fn covering_is_present_in_every_embedding() {
    let mut seed = 0x298b42u64;
    for _ in 0..128 {
        let bytes: Vec<_> = (0..64)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                [b'a', b'b', b'c', b'\n', 0xff, b' '][seed as usize % 6]
            })
            .collect();
        let indexed: HashSet<_> = all(&bytes).into_iter().collect();
        for start in 0..bytes.len() {
            for end in start + 3..=bytes.len() {
                if let Some(grams) = covering(&bytes[start..end]) {
                    assert!(
                        grams.iter().all(|g| indexed.contains(g)),
                        "lost substring {start}..{end}"
                    );
                }
            }
        }
    }
}
