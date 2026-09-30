//! What the index holds: counts, where the bytes go, what is built, and the
//! shape of the tree it covers.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use crate::workspace::Workspace;

pub fn overview(ws: &Workspace) -> Result<Value> {
    let index = ws.index()?;
    let meta = &index.meta;

    let mut storage: BTreeMap<&'static str, u64> = BTreeMap::new();
    walk_sizes(&ws.index_dir, &ws.index_dir, &mut storage);
    let storage: Vec<Value> = [
        (
            "postings",
            "index.bin: posting lists for every trigram and sparse gram",
        ),
        ("lookup", "lookup.bin: key table pointing into the postings"),
        ("paths", "files.bin and files-extra.bin: the path table"),
        (
            "evidence",
            "filestamps and content ids used to detect changes",
        ),
        ("history", "history/: the index over commit diffs"),
        ("graph", "graph/: definitions and links between files"),
        (
            "facts",
            "facts/: cached parse results, keyed by grammar and content",
        ),
        ("lab", "lab/: saved evaluation runs"),
        ("other", "anything else in the index directory"),
    ]
    .iter()
    .filter_map(|&(k, what)| {
        let bytes = *storage.get(k)?;
        Some(json!({ "part": k, "bytes": bytes, "what": what }))
    })
    .collect();

    let history = ws.history().map(|h| {
        let (mut oldest, mut newest) = (i64::MAX, i64::MIN);
        for name in h.reader.all_paths().iter() {
            if let Some((t, _)) = ogrep_core::history::parse_doc_name(name) {
                oldest = oldest.min(t);
                newest = newest.max(t);
            }
        }
        json!({
            "commits": h.meta.commits,
            "head": h.meta.head,
            "depth": h.meta.depth,
            "since": h.meta.since,
            "truncated": h.meta.truncated,
            "oldest": (oldest != i64::MAX).then_some(oldest),
            "newest": (newest != i64::MIN).then_some(newest),
        })
    });

    let graph = ws.graph().map(|g| {
        let built = std::fs::metadata(ws.index_dir.join("graph").join("graph.bin"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        json!({
            "files": g.graph.files.len(),
            "links": g.graph.edges(),
            "definitions": g.graph.defs.len(),
            "current": g.current,
            "built_at": built,
        })
    });

    let mut types: BTreeMap<String, usize> = BTreeMap::new();
    let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
    for (path, _) in &index.sorted {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && ext.len() <= 12 => format!(".{ext}"),
            _ => "(none)".to_string(),
        };
        *types.entry(ext).or_default() += 1;
        let top = match path.split_once('/') {
            Some((d, _)) => format!("{d}/"),
            None => "(root files)".to_string(),
        };
        *dirs.entry(top).or_default() += 1;
    }

    Ok(json!({
        "root": ws.root,
        "index_dir": ws.index_dir,
        "ogrep": ws.ogrep,
        "ogrep_version": ws.ogrep_version,
        "lab_version": env!("CARGO_PKG_VERSION"),
        "meta": {
            "format": meta.version,
            "files": meta.num_files,
            "trigram_keys": meta.num_trigrams,
            "sparse_grams": meta.sparse_grams,
            "sparse_gram_keys": meta.sparse_gram_keys,
            "created_at": meta.created_at,
            "updated_at": meta.updated_at,
            "complete": meta.complete,
        },
        "storage": storage,
        "history": history,
        "graph": graph,
        "load_errors": ws.load_errors(),
        "file_types": top_counts(types, 14),
        "directories": top_counts(dirs, 16),
    }))
}

/// The `n` largest counts, then one `other` row for the rest.
fn top_counts(counts: BTreeMap<String, usize>, n: usize) -> Vec<Value> {
    let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let rest: usize = rows.iter().skip(n).map(|r| r.1).sum();
    let mut out: Vec<Value> = rows
        .into_iter()
        .take(n)
        .map(|(name, count)| json!({ "name": name, "count": count }))
        .collect();
    if rest > 0 {
        out.push(json!({ "name": "other", "count": rest, "other": true }));
    }
    out
}

fn walk_sizes(base: &Path, dir: &Path, into: &mut BTreeMap<&'static str, u64>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk_sizes(base, &path, into);
            continue;
        }
        let rel = path.strip_prefix(base).unwrap_or(&path);
        let first = rel
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let part = match first.as_str() {
            "index.bin" => "postings",
            "lookup.bin" => "lookup",
            "files.bin" | "files-extra.bin" => "paths",
            "filestamps.json" | "evidence.bin" | "meta.json" => "evidence",
            "history" => "history",
            "graph" => "graph",
            "facts" => "facts",
            "lab" => "lab",
            _ => "other",
        };
        *into.entry(part).or_default() += md.len();
    }
}
