//! Navigating the indexed tree: directory listings, a file with its outline,
//! the files it uses and is used by (with the names that link them), and the
//! graph around a file.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Result, anyhow};
use ogrep_core::graph::Def;
use rayon::prelude::*;
use serde_json::{Value, json};

use crate::workspace::{GraphView, Workspace};

/// Files larger than this are shown truncated.
const MAX_SHOWN: usize = 2 << 20;

pub fn kind_name(kind: u8) -> &'static str {
    // The order of `ogrep_core::structure`'s kinds, as ogrep prints them.
    [
        "function",
        "method",
        "class",
        "interface",
        "module",
        "macro",
        "constant",
        "type",
        "impl",
        "other",
    ]
    .get(usize::from(kind))
    .copied()
    .unwrap_or("other")
}

/// The entries of one directory: subdirectories with their file counts, then
/// files.
pub fn tree(ws: &Workspace, dir: &str) -> Result<Value> {
    let index = ws.index()?;
    let prefix = if dir.is_empty() || dir.ends_with('/') {
        dir.to_string()
    } else {
        format!("{dir}/")
    };
    let mut dirs: BTreeMap<&str, usize> = BTreeMap::new();
    let mut files = Vec::new();
    for (path, _) in index.under(&prefix) {
        let rest = &path[prefix.len()..];
        match rest.split_once('/') {
            Some((d, _)) => *dirs.entry(d).or_default() += 1,
            None => files.push(rest),
        }
    }
    Ok(json!({
        "dir": prefix,
        "dirs": dirs.iter().map(|(name, n)| json!({ "name": name, "files": n })).collect::<Vec<_>>(),
        "files": files,
    }))
}

/// Paths containing every space-separated term, shortest first: a quick
/// "go to file".
pub fn find(ws: &Workspace, query: &str, limit: usize) -> Result<Value> {
    let index = ws.index()?;
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return Ok(json!({ "paths": [] }));
    }
    // Prefer matches in the file name, then short names, then short paths.
    let mut hits: Vec<(usize, usize, usize, &str)> = index
        .sorted
        .par_iter()
        .filter_map(|(p, _)| {
            let lower = p.to_lowercase();
            if !terms.iter().all(|t| lower.contains(t.as_str())) {
                return None;
            }
            let name = lower.rsplit('/').next().unwrap_or(&lower);
            let missing = terms.iter().filter(|t| !name.contains(t.as_str())).count();
            Some((missing, name.len(), p.len(), p.as_str()))
        })
        .collect();
    hits.sort_unstable();
    hits.truncate(limit);
    Ok(json!({ "paths": hits.iter().map(|h| h.3).collect::<Vec<_>>() }))
}

fn def_json(d: &Def) -> Value {
    json!({
        "name": d.name,
        "kind": kind_name(d.kind),
        "line": d.line,
        "end_line": d.end_line,
        "signature": d.signature,
    })
}

/// A file's text, outline and links.
pub fn file(ws: &Workspace, path: &str) -> Result<Value> {
    let full = ws.indexed_file(path)?;
    let data = std::fs::read(&full).map_err(|e| anyhow!("cannot read {path}: {e}"))?;
    let binary = memchr::memchr(0, &data[..data.len().min(8192)]).is_some();
    let truncated = data.len() > MAX_SHOWN;
    let text = if binary {
        String::new()
    } else {
        String::from_utf8_lossy(&data[..data.len().min(MAX_SHOWN)]).into_owned()
    };

    let graph = ws.graph();
    let (outline, uses, used_by, pagerank) = match graph
        .as_deref()
        .zip(graph.as_ref().and_then(|g| g.graph.file(path)))
    {
        Some((g, id)) => {
            let outline: Vec<Value> = g.graph.defs_in(id).into_iter().map(def_json).collect();
            let words = identifier_set(&text);
            let uses: Vec<Value> = strongest(g.graph.out_links(id).collect(), 40)
                .into_iter()
                .map(|(to, w)| {
                    let via = linking_names(g, to, &words);
                    json!({ "path": g.graph.files[to as usize], "weight": w, "via": via })
                })
                .collect();
            let used_by = incoming_with_evidence(ws, g, id, 40);
            (outline, uses, used_by, Some(g.graph.pagerank[id as usize]))
        }
        None => (Vec::new(), Vec::new(), Vec::new(), None),
    };

    Ok(json!({
        "path": path,
        "bytes": data.len(),
        "binary": binary,
        "truncated": truncated,
        "text": text,
        "graph": graph.is_some(),
        "graph_current": graph.as_ref().map(|g| g.current),
        "pagerank": pagerank,
        "outline": outline,
        "uses": uses,
        "used_by": used_by,
    }))
}

fn strongest(mut links: Vec<(u32, f32)>, n: usize) -> Vec<(u32, f32)> {
    links.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    links.truncate(n);
    links
}

/// Identifiers of three or more characters, the graph's own rule.
fn identifier_set(text: &str) -> HashSet<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.len() >= 3 && !w.as_bytes()[0].is_ascii_digit())
        .collect()
}

/// Names `target` defines that appear in `words`: why a link exists.
fn linking_names(g: &GraphView, target: u32, words: &HashSet<&str>) -> Vec<String> {
    let mut names: Vec<String> = g
        .graph
        .defs_in(target)
        .into_iter()
        .filter(|d| d.linkable && words.contains(d.name.as_str()))
        .map(|d| d.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names.truncate(6);
    names
}

/// The files that link to `id`, strongest first, with the names of `id`'s
/// definitions each one mentions. Reading those files is the only way to get
/// the evidence, since the graph stores weights, not names.
fn incoming_with_evidence(ws: &Workspace, g: &GraphView, id: u32, n: usize) -> Vec<Value> {
    let links = strongest(g.incoming[id as usize].clone(), n);
    let defs: Vec<&Def> = g
        .graph
        .defs_in(id)
        .into_iter()
        .filter(|d| d.linkable)
        .collect();
    links
        .par_iter()
        .map(|&(from, w)| {
            let path = &g.graph.files[from as usize];
            let text = ws
                .indexed_file(path)
                .ok()
                .and_then(|f| std::fs::read(f).ok())
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let words = identifier_set(&text);
            let mut via: Vec<&str> = defs
                .iter()
                .filter(|d| words.contains(d.name.as_str()))
                .map(|d| d.name.as_str())
                .collect();
            via.sort_unstable();
            via.dedup();
            via.truncate(6);
            json!({ "path": path, "weight": w, "via": via })
        })
        .collect()
}

/// The neighbourhood of a file for drawing: the file, its strongest links in
/// both directions, optionally their strongest links, and every link among the
/// chosen files.
pub fn neighbourhood(ws: &Workspace, path: &str, per_side: usize, hops: usize) -> Result<Value> {
    let g = ws
        .graph()
        .ok_or_else(|| anyhow!("no file graph; build it with `ogrep index --graph`"))?;
    let center = g
        .graph
        .file(path)
        .ok_or_else(|| anyhow!("{path} is not in the graph"))?;
    let mut chosen: Vec<u32> = vec![center];
    let mut depth: HashMap<u32, usize> = HashMap::from([(center, 0)]);
    let mut frontier = vec![center];
    for hop in 1..=hops {
        let take = if hop == 1 {
            per_side
        } else {
            (per_side / 4).max(2)
        };
        let mut next = Vec::new();
        for &f in &frontier {
            let outs = strongest(g.graph.out_links(f).collect(), take);
            let ins = strongest(g.incoming[f as usize].clone(), take);
            for (n, _) in outs.into_iter().chain(ins) {
                if let std::collections::hash_map::Entry::Vacant(e) = depth.entry(n) {
                    e.insert(hop);
                    chosen.push(n);
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
    let index_of: HashMap<u32, usize> = chosen.iter().enumerate().map(|(i, &f)| (f, i)).collect();
    let mut edges = Vec::new();
    for (i, &f) in chosen.iter().enumerate() {
        for (to, w) in g.graph.out_links(f) {
            if let Some(&j) = index_of.get(&to) {
                edges.push(json!({ "from": i, "to": j, "weight": w }));
            }
        }
    }
    let nodes: Vec<Value> = chosen
        .iter()
        .map(|&f| {
            json!({
                "path": g.graph.files[f as usize],
                "depth": depth[&f],
                "pagerank": g.graph.pagerank[f as usize],
                "defs": g.graph.defs_in(f).len(),
                "out": g.graph.out_links(f).count(),
                "in": g.incoming[f as usize].len(),
            })
        })
        .collect();
    Ok(json!({ "nodes": nodes, "edges": edges, "current": g.current }))
}

/// Files by global PageRank: the ones everything else leans on.
pub fn central(ws: &Workspace, limit: usize) -> Result<Value> {
    let g = ws
        .graph()
        .ok_or_else(|| anyhow!("no file graph; build it with `ogrep index --graph`"))?;
    let mut order: Vec<u32> = (0..g.graph.files.len() as u32).collect();
    order.sort_by(|&a, &b| {
        g.graph.pagerank[b as usize]
            .total_cmp(&g.graph.pagerank[a as usize])
            .then(a.cmp(&b))
    });
    let max = order.first().map_or(1.0, |&f| g.graph.pagerank[f as usize]);
    let rows: Vec<Value> = order
        .iter()
        .take(limit)
        .map(|&f| {
            json!({
                "path": g.graph.files[f as usize],
                "pagerank": g.graph.pagerank[f as usize],
                "relative": g.graph.pagerank[f as usize] / max,
                "in": g.incoming[f as usize].len(),
                "defs": g.graph.defs_in(f).len(),
            })
        })
        .collect();
    Ok(json!({ "files": rows, "current": g.current }))
}

/// Definitions whose name contains `query` (case-insensitive), exact matches
/// first.
pub fn symbols(ws: &Workspace, query: &str, limit: usize) -> Result<Value> {
    let g = ws
        .graph()
        .ok_or_else(|| anyhow!("no file graph; build it with `ogrep index --graph`"))?;
    let q = query.to_lowercase();
    let mut hits: Vec<(u8, usize, &Def)> = g
        .graph
        .defs
        .par_iter()
        .filter_map(|d| {
            let lower = d.name.to_lowercase();
            let rank = if d.name == query {
                0
            } else if lower == q {
                1
            } else if lower.starts_with(&q) {
                2
            } else if lower.contains(&q) {
                3
            } else {
                return None;
            };
            Some((rank, d.name.len(), d))
        })
        .collect();
    hits.sort_by(|a, b| {
        (a.0, a.1, &g.graph.files[a.2.file as usize], a.2.line).cmp(&(
            b.0,
            b.1,
            &g.graph.files[b.2.file as usize],
            b.2.line,
        ))
    });
    let rows: Vec<Value> = hits
        .iter()
        .take(limit)
        .map(|(_, _, d)| {
            let mut v = def_json(d);
            v["path"] = json!(g.graph.files[d.file as usize]);
            v
        })
        .collect();
    Ok(json!({ "symbols": rows, "total": hits.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_follow_the_graph_rule() {
        let words = identifier_set("fn bulk_update(x: u32) { 1abc; ab }");
        assert!(words.contains("bulk_update"));
        assert!(words.contains("u32"));
        assert!(!words.contains("1abc"));
        assert!(!words.contains("ab"));
    }
}
