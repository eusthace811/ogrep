//! `--near`, `--map` and `--callees`: answers from the file graph
//! ([`ogrep_core::graph`]), seeded by an ordinary search.

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Result, anyhow};
use ogrep_core::graph::{self, Def, Graph};

use crate::search::SearchOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Files related to the files that matched.
    Near { budget: usize },
    /// An outline of the most related files within a token budget.
    Map { tokens: usize },
    /// Names used by the definitions of the pattern, and where they are defined.
    Callees,
}

use ogrep_core::graph::kind_name;

/// Every matching line as (path relative to the index root, line number).
fn matched_lines(
    root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
) -> Result<Vec<(String, usize)>> {
    Ok(crate::search::captured_matches(root, index_path, opts, ci)?
        .into_iter()
        .map(|m| (m.file, m.line_number))
        .collect())
}

fn no_graph(index_dir: &Path) -> anyhow::Error {
    anyhow!(
        "no file graph in {}; build it with `ogrep index --graph`",
        index_dir.display()
    )
}

/// The graph in `index_dir`, with a note when it predates the index.
fn load_graph(index_dir: &Path, quiet: bool) -> Result<Graph> {
    let (g, current) = graph::open(index_dir).map_err(|_| no_graph(index_dir))?;
    if !current && !quiet {
        eprintln!("note: the file graph predates the index; `ogrep index --graph` refreshes it");
    }
    Ok(g)
}

/// Identifiers mentioned in the seed files.
fn words(texts: &[String]) -> std::collections::HashSet<&str> {
    texts
        .iter()
        .flat_map(|t| t.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')))
        .filter(|w| w.len() >= 3)
        .collect()
}

/// Names defined in `target` that the seed files mention: why it is related.
fn evidence(g: &Graph, target: u32, seed_words: &std::collections::HashSet<&str>) -> Vec<String> {
    let mut names: Vec<String> = g
        .defs_in(target)
        .into_iter()
        .filter(|d| d.linkable && seed_words.contains(d.name.as_str()))
        .map(|d| d.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names.truncate(4);
    names
}

pub fn run(
    root: &Path,
    index_path: Option<&Path>,
    opts: &SearchOptions,
    ci: bool,
    mode: Mode,
) -> Result<bool> {
    // Loading the graph takes longest; the seed search runs meanwhile. A
    // missing graph is reported first, without paying for the search.
    let dir = crate::search::index_dir_for(root, index_path);
    if !graph::exists(&dir) {
        return Err(no_graph(&dir));
    }
    let (g, matched) = std::thread::scope(|scope| {
        let search = (!matches!(mode, Mode::Callees))
            .then(|| scope.spawn(|| matched_lines(root, index_path, opts, ci)));
        let g = load_graph(&dir, opts.no_messages);
        let matched = search.map(|s| s.join().expect("seed search panicked"));
        (g, matched)
    });
    let mut g = g?;
    // The graph's paths are relative to the index root, a search's to the
    // path searched, which may be below it (with --index-path): translate.
    // A tree moved or copied since it was indexed is read where this index
    // is, not where it recorded.
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let index_root =
        crate::search::index_tree_root(&dir, root).unwrap_or_else(|| root.to_path_buf());
    let below = canonical(root)
        .strip_prefix(canonical(&index_root))
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    // Or the index was built for a directory below the one searched.
    let above = canonical(&index_root)
        .strip_prefix(canonical(root))
        .map(|p| format!("{}/", p.to_string_lossy().replace('\\', "/")))
        .ok()
        .filter(|p| p != "/");
    let named_file = root.is_file();
    let in_graph = |rel: &str| -> Option<String> {
        if named_file {
            Some(below.clone())
        } else if let Some(above) = &above {
            rel.strip_prefix(above.as_str()).map(str::to_string)
        } else if below.is_empty() {
            Some(rel.to_string())
        } else {
            Some(format!("{below}/{rel}"))
        }
    };
    // Matched lines per graph file; each file weighs by how many matched.
    let mut lines: HashMap<u32, Vec<usize>> = HashMap::new();
    for (path, line) in matched.transpose()?.unwrap_or_default() {
        if let Some(f) = in_graph(&path).and_then(|p| g.file(&p)) {
            lines.entry(f).or_default().push(line);
        }
    }
    // From above the index root, its files are shown, and read, as paths
    // below the searched one.
    let root = match &above {
        Some(above) => {
            g.prefix_paths(above);
            root
        }
        None => index_root.as_path(),
    };
    let mut seeds: Vec<(u32, f64)> = lines
        .iter()
        .map(|(&f, l)| (f, (1.0 + l.len() as f64).ln()))
        .collect();
    let mut out = BufWriter::new(std::io::stdout().lock());
    let found = match mode {
        Mode::Callees => callees(&g, root, opts, &mut out)?,
        Mode::Near { budget } => {
            seeds.sort_by(|a, b| g.files[a.0 as usize].cmp(&g.files[b.0 as usize]));
            near(&g, root, &seeds, budget, opts, &mut out)?
        }
        Mode::Map { tokens } => {
            seeds.sort_by_key(|s| s.0);
            map(&g, root, &seeds, &lines, tokens, opts, &mut out)?
        }
    };
    out.flush()?;
    Ok(found)
}

fn near(
    g: &Graph,
    root: &Path,
    seeds: &[(u32, f64)],
    budget: usize,
    opts: &SearchOptions,
    out: &mut impl Write,
) -> Result<bool> {
    if seeds.is_empty() {
        return Ok(false);
    }
    let rank = g.related(seeds);
    let seed_set: std::collections::HashSet<u32> = seeds.iter().map(|s| s.0).collect();
    // Unrelated files (no link to any seed) are not results.
    let mut order: Vec<u32> = (0..g.files.len() as u32)
        .filter(|f| !seed_set.contains(f) && rank[*f as usize] > 0.0)
        .collect();
    order.sort_by(|&a, &b| {
        rank[b as usize]
            .total_cmp(&rank[a as usize])
            .then_with(|| g.files[a as usize].cmp(&g.files[b as usize]))
    });
    order.truncate(budget);
    // A root that cannot be opened (the tree has moved) leaves the evidence out.
    let read = ogrep_core::rooted::indexed_text_reader(root);
    let seed_texts: Vec<String> = seeds
        .iter()
        .take(64)
        .filter_map(|s| read(&g.files[s.0 as usize]).ok())
        .collect();
    let seed_words = words(&seed_texts);
    let top = order.first().map_or(1.0, |&f| rank[f as usize]).max(1e-12);
    for f in &order {
        let via = evidence(g, *f, &seed_words);
        let score = rank[*f as usize] / top;
        let path = &g.files[*f as usize];
        if opts.json {
            let msg = serde_json::json!({"type": "near", "data": {"path": path, "score": (score * 1e6).round() / 1e6, "via": via}});
            writeln!(out, "{msg}")?;
        } else if via.is_empty() {
            writeln!(out, "{score:.3} {path}")?;
        } else {
            writeln!(out, "{score:.3} {path}  (via {})", via.join(", "))?;
        }
    }
    Ok(!order.is_empty())
}

fn map(
    g: &Graph,
    root: &Path,
    seeds: &[(u32, f64)],
    matched_lines: &HashMap<u32, Vec<usize>>,
    tokens: usize,
    opts: &SearchOptions,
    out: &mut impl Write,
) -> Result<bool> {
    // Matched files first (most matches first), then files related to them,
    // then the globally central ones.
    let related = g.related(seeds);
    let seed_weight: HashMap<u32, f64> = seeds.iter().copied().collect();
    let rank: Vec<(f64, f64, f64)> = (0..g.files.len())
        .map(|f| {
            (
                seed_weight.get(&(f as u32)).copied().unwrap_or(0.0),
                related[f],
                g.pagerank[f] as f64,
            )
        })
        .collect();
    // A root that cannot be opened (the tree has moved) leaves the evidence out.
    let read = ogrep_core::rooted::indexed_text_reader(root);
    let seed_texts: Vec<String> = seeds
        .iter()
        .take(32)
        .filter_map(|s| read(&g.files[s.0 as usize]).ok())
        .collect();
    let seed_words = words(&seed_texts);
    let mut order: Vec<u32> = (0..g.files.len() as u32).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (rank[a as usize], rank[b as usize]);
        y.0.total_cmp(&x.0)
            .then(y.1.total_cmp(&x.1))
            .then(y.2.total_cmp(&x.2))
            .then_with(|| g.files[a as usize].cmp(&g.files[b as usize]))
    });
    let budget_chars = tokens.saturating_mul(4);
    let mut used = 0usize;
    let mut printed = false;
    for f in order {
        // Definitions around the matches first, then those the matched files
        // mention, then the rest, each group in line order.
        let lines = matched_lines.get(&f);
        let mut defs: Vec<(u8, &Def)> = g
            .defs_in(f)
            .into_iter()
            .filter(|d| !matches!(kind_name(d.kind), "constant" | "other"))
            .map(|d| {
                let around = lines.is_some_and(|ls| {
                    ls.iter()
                        .any(|&l| (d.line as usize..=d.end_line as usize).contains(&l))
                });
                let mentioned =
                    !graph::is_protocol_name(&d.name) && seed_words.contains(d.name.as_str());
                (
                    if around {
                        0
                    } else if mentioned {
                        1
                    } else {
                        2
                    },
                    d,
                )
            })
            .collect();
        defs.sort_by_key(|&(p, d)| (p, d.line));
        defs.truncate(8);
        defs.sort_by_key(|&(_, d)| d.line);
        let defs: Vec<&Def> = defs.into_iter().map(|(_, d)| d).collect();
        if defs.is_empty() {
            continue;
        }
        let path = &g.files[f as usize];
        let cost = path.len() + 2 + defs.iter().map(|d| d.signature.len() + 8).sum::<usize>();
        if used + cost > budget_chars && printed {
            break;
        }
        used += cost;
        printed = true;
        if opts.json {
            let msg = serde_json::json!({
                "type": "map",
                "data": {
                    "path": path,
                    "score": rank[f as usize].1,
                    "defs": defs.iter().map(|d| serde_json::json!({
                        "kind": kind_name(d.kind), "name": d.name, "line": d.line, "signature": d.signature,
                    })).collect::<Vec<_>>(),
                },
            });
            writeln!(out, "{msg}")?;
        } else {
            writeln!(out, "{path}:")?;
            for d in defs {
                writeln!(out, "{:>6}: {}", d.line, d.signature)?;
            }
        }
    }
    Ok(printed)
}

fn callees(g: &Graph, root: &Path, opts: &SearchOptions, out: &mut impl Write) -> Result<bool> {
    let name = opts.pattern.as_str();
    let defs: Vec<&Def> = g.defs_named(name).collect();
    let mut printed = false;
    let read = ogrep_core::rooted::indexed_text_reader(root);
    for def in defs {
        let path = &g.files[def.file as usize];
        let Ok(text) = read(path) else {
            continue;
        };
        let body: String = text
            .lines()
            .skip(def.line.saturating_sub(1) as usize)
            .take((def.end_line.saturating_sub(def.line) + 1) as usize)
            .collect::<Vec<_>>()
            .join("\n");
        let mut seen = std::collections::BTreeSet::new();
        let mut rows = Vec::new();
        let linked: std::collections::HashSet<u32> =
            g.out_links(def.file).map(|(t, _)| t).collect();
        for (i, word) in body
            .match_indices(|c: char| c.is_ascii_alphabetic() || c == '_')
            .map(|(i, _)| i)
            .filter(|&i| {
                i == 0
                    || !body[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            })
            .map(|i| {
                let end = body[i..]
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .map_or(body.len(), |e| i + e);
                (end, &body[i..end])
            })
        {
            if word.len() < 3
                || word == def.name
                || graph::is_protocol_name(word)
                || seen.contains(word)
            {
                continue;
            }
            // A function or method is used by calling it; a type may be named
            // bare.
            let called = body[i..].trim_start().starts_with('(');
            let mut targets: Vec<&Def> = g
                .defs_named(word)
                .filter(|t| {
                    called || matches!(kind_name(t.kind), "class" | "interface" | "type" | "module")
                })
                .collect();
            if targets.is_empty() {
                continue;
            }
            // Prefer the definition in the same file, then in files this one
            // links to; otherwise a name with many definitions is ambiguous.
            let near: Vec<&Def> = targets
                .iter()
                .copied()
                .filter(|t| t.file == def.file || linked.contains(&t.file))
                .collect();
            if !near.is_empty() {
                targets = near;
            } else if targets.len() > 3 {
                continue;
            }
            targets.truncate(3);
            // Seen once it is a callee: an earlier mention that was not a
            // call (`let cb = process_item;`) must not hide a later call.
            seen.insert(word.to_string());
            rows.push((word.to_string(), targets));
        }
        if !opts.json {
            writeln!(out, "{path}:{}: {}", def.line, def.signature)?;
        }
        for (word, targets) in &rows {
            for t in targets {
                let tpath = &g.files[t.file as usize];
                if opts.json {
                    let msg = serde_json::json!({"type": "callee", "data": {
                        "from": {"path": path, "line": def.line, "name": def.name},
                        "name": word, "path": tpath, "line": t.line, "kind": kind_name(t.kind), "signature": t.signature,
                    }});
                    writeln!(out, "{msg}")?;
                } else {
                    writeln!(out, "  {word} -> {tpath}:{}: {}", t.line, t.signature)?;
                }
            }
        }
        // Text output prints the definition's header; JSON only its callees.
        printed |= !opts.json || !rows.is_empty();
    }
    Ok(printed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn seed_words_are_identifiers_of_three_or_more() {
        let texts = vec!["x = helper(1); ab".to_string(), "my_helper()".to_string()];
        let w = super::words(&texts);
        assert!(w.contains("helper") && w.contains("my_helper") && !w.contains("ab"));
    }
}
