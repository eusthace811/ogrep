//! A graph of which files use names that other files define.
//!
//! Built from the index's file list: each parsed file contributes its
//! definitions ([`crate::structure`]), and every text file contributes the
//! identifiers it mentions outside comments and strings. File A links to file B
//! when A mentions a name B defines. A name defined in many files says little
//! about which one is meant, so each link is weighted by the name's rarity
//! (`ln(files / files defining it)`), names defined in more than
//! [`MAX_DEFINERS`] files are ignored, and each file keeps its
//! [`MAX_EDGES`] strongest links.
//!
//! PageRank over the links (importance flows from a user to what it uses) gives
//! a global prior. The files most related to a search's matches (`--near`,
//! `--map`) are found by [`Graph::related`], the direct link weight to them,
//! which beat personalized PageRank on co-change.
//!
//! The graph is stored under `<index>/graph/` with the identity of the file
//! table it was built from. A search against a newer index still uses it, since
//! relatedness changes slowly, and says so; `ogrep index --graph` rebuilds it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use rayon::prelude::*;

use crate::reader::IndexReader;
use crate::{Error, Result};

pub const GRAPH_DIR: &str = "graph";
const GRAPH_FILE: &str = "graph.bin";
/// Bump when the build or the file layout changes.
pub const VERSION: u32 = 2;
/// Names defined in more files than this are too generic to link files.
pub const MAX_DEFINERS: usize = 16;
/// Strongest outgoing links kept per file.
pub const MAX_EDGES: usize = 48;
const DAMPING: f64 = 0.85;
const ITERATIONS: usize = 30;
const MAX_FILE: u64 = 4 << 20;

/// A definition, for maps and caller/callee listings.
#[derive(Clone, Debug, PartialEq)]
pub struct Def {
    pub file: u32,
    /// A symbol kind, by position in the structure module's order; see
    /// [`kind_name`].
    pub kind: u8,
    pub name: String,
    pub line: u32,
    pub end_line: u32,
    /// The definition's first line, trimmed and shortened.
    pub signature: String,
    /// Specific enough to link files: not a protocol name, and defined in at
    /// most [`MAX_DEFINERS`] files.
    pub linkable: bool,
}

pub struct Graph {
    pub file_table_id: [u8; 32],
    pub files: Vec<String>,
    pub pagerank: Vec<f32>,
    offsets: Vec<u32>,
    targets: Vec<u32>,
    weights: Vec<f32>,
    pub defs: Vec<Def>,
    /// Definitions by name; built on first use, since only some queries need it.
    by_name: std::sync::OnceLock<HashMap<String, Vec<u32>>>,
    by_path: HashMap<String, u32>,
    /// Definition indices of each file, in line order.
    by_file: Vec<Vec<u32>>,
}

/// Names every type in a language may define (`__init__`, `__eq__`): they say
/// nothing about which file is meant.
pub fn is_protocol_name(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// Identifiers of at least three characters, as a sorted, deduplicated list.
fn identifiers(text: &[u8], skip: &dyn Fn(usize) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < text.len() && (text[i].is_ascii_alphanumeric() || text[i] == b'_') {
                i += 1;
            }
            if i - start >= 3 && i - start <= 80 && !skip(start) {
                out.push(String::from_utf8_lossy(&text[start..i]).into_owned());
            }
        } else if c.is_ascii_digit() {
            while i < text.len() && (text[i].is_ascii_alphanumeric() || text[i] == b'_') {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

struct Scanned {
    defs: Vec<Def>,
    idents: Vec<String>,
}

/// The name of a [`Def::kind`], as `SymbolKind` serialises it ("function",
/// "class", ...). Kept here, outside the parser-gated structure module, so
/// readers built without the parsers can name kinds too.
pub fn kind_name(kind: u8) -> &'static str {
    const NAMES: &[&str] = &[
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
    ];
    NAMES.get(usize::from(kind)).copied().unwrap_or("other")
}

#[cfg(feature = "structure")]
fn scan(
    file: u32,
    path: &str,
    text: &[u8],
    cache: &crate::facts::FactStore,
) -> (Scanned, Option<crate::facts::NewFact>) {
    use crate::structure::{self, SymbolKind, Zone};
    let (parsed, fact) = structure::analyze_cached(path, text, Some(cache));
    let mut defs = Vec::new();
    if let Some(fs) = &parsed {
        let lines: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
        for s in &fs.symbols {
            if s.kind == SymbolKind::Impl || s.name.len() < 2 {
                continue;
            }
            let raw = lines
                .get(s.start_line.saturating_sub(1))
                .copied()
                .unwrap_or(b"");
            let sig = String::from_utf8_lossy(raw)
                .trim()
                .chars()
                .take(160)
                .collect();
            defs.push(Def {
                file,
                kind: s.kind as u8,
                name: s.name.clone(),
                line: s.start_line as u32,
                end_line: s.end_line as u32,
                signature: sig,
                linkable: false,
            });
        }
    }
    let skip = |at: usize| {
        parsed
            .as_ref()
            .is_some_and(|fs| fs.zone_at(at) != Zone::Code)
    };
    (
        Scanned {
            defs,
            idents: identifiers(text, &skip),
        },
        fact,
    )
}

#[cfg(not(feature = "structure"))]
fn scan(
    _file: u32,
    _path: &str,
    text: &[u8],
    _cache: &crate::facts::FactStore,
) -> (Scanned, Option<crate::facts::NewFact>) {
    (
        Scanned {
            defs: Vec::new(),
            idents: identifiers(text, &|_| false),
        },
        None,
    )
}

/// Build the graph for the index in `index_dir` over the files under `root`,
/// and store it.
pub fn build(root: &Path, index_dir: &Path) -> Result<Graph> {
    let reader = IndexReader::open(index_dir)?;
    let files: Vec<String> = reader.all_paths().iter().map(|s| s.to_string()).collect();
    let cache = crate::facts::FactStore::open(index_dir, crate::facts::STRUCTURE);
    let rooted = crate::rooted::RootDir::open(root)?;
    let scanned: Vec<(Scanned, Option<crate::facts::NewFact>)> = files
        .par_iter()
        .enumerate()
        .map(|(i, path)| {
            let full = crate::hybrid::join_indexed_path(root, path);
            let text = rooted
                .open_file(&full)
                .ok()
                .filter(|f| f.metadata().is_ok_and(|m| m.len() <= MAX_FILE))
                .and_then(|mut f| {
                    let mut bytes = Vec::new();
                    std::io::Read::read_to_end(&mut f, &mut bytes).ok()?;
                    Some(bytes)
                })
                .filter(|t| !crate::trigram::is_binary(t))
                .unwrap_or_default();
            scan(i as u32, path, &text, &cache)
        })
        .collect();
    let mut new_facts = Vec::new();
    let mut defs = Vec::new();
    let mut idents = Vec::with_capacity(scanned.len());
    for (s, fact) in scanned {
        defs.extend(s.defs);
        idents.push(s.idents);
        new_facts.extend(fact);
    }
    drop(cache);
    let _ = crate::facts::FactStore::put(index_dir, crate::facts::STRUCTURE, new_facts);

    // name -> files defining it (distinct, capped).
    let mut definers: HashMap<&str, Vec<u32>> = HashMap::new();
    for d in defs.iter().filter(|d| !is_protocol_name(&d.name)) {
        let entry = definers.entry(d.name.as_str()).or_default();
        if entry.last() != Some(&d.file) {
            entry.push(d.file);
        }
    }
    definers.retain(|_, files| {
        files.dedup();
        files.len() <= MAX_DEFINERS
    });
    let linkable: Vec<bool> = defs
        .iter()
        .map(|d| definers.contains_key(d.name.as_str()))
        .collect();
    let n = files.len().max(1) as f64;
    let edges: Vec<Vec<(u32, f32)>> = idents
        .par_iter()
        .enumerate()
        .map(|(a, names)| {
            let mut weight: HashMap<u32, f32> = HashMap::new();
            for name in names {
                if let Some(ds) = definers.get(name.as_str()) {
                    let idf = (n / ds.len() as f64).ln() as f32;
                    for &b in ds {
                        if b as usize != a {
                            *weight.entry(b).or_default() += idf;
                        }
                    }
                }
            }
            let mut out: Vec<(u32, f32)> = weight.into_iter().collect();
            out.sort_unstable_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            out.truncate(MAX_EDGES);
            out.sort_unstable_by_key(|e| e.0);
            out
        })
        .collect();
    let mut offsets = Vec::with_capacity(files.len() + 1);
    let mut targets = Vec::new();
    let mut weights = Vec::new();
    offsets.push(0u32);
    for out in &edges {
        for &(t, w) in out {
            targets.push(t);
            weights.push(w);
        }
        offsets.push(targets.len() as u32);
    }
    for (d, l) in defs.iter_mut().zip(linkable) {
        d.linkable = l;
    }
    let mut graph = Graph {
        file_table_id: reader.file_table_id(),
        files,
        pagerank: Vec::new(),
        offsets,
        targets,
        weights,
        defs,
        by_name: std::sync::OnceLock::new(),
        by_path: HashMap::new(),
        by_file: Vec::new(),
    };
    graph.pagerank = graph
        .propagate(None)
        .into_iter()
        .map(|x| x as f32)
        .collect();
    graph.save(index_dir)?;
    graph.index_lookups();
    Ok(graph)
}

impl Graph {
    fn index_lookups(&mut self) {
        self.by_path = self
            .files
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i as u32))
            .collect();
        self.by_file = vec![Vec::new(); self.files.len()];
        for (i, d) in self.defs.iter().enumerate() {
            if let Some(slot) = self.by_file.get_mut(d.file as usize) {
                slot.push(i as u32);
            }
        }
        for slot in &mut self.by_file {
            slot.sort_by_key(|&i| self.defs[i as usize].line);
        }
    }

    pub fn edges(&self) -> usize {
        self.targets.len()
    }

    /// Index of a file by its index path.
    pub fn file(&self, path: &str) -> Option<u32> {
        self.by_path.get(path).copied()
    }

    /// How related each file is to the seeds: the weight of the links between
    /// it and the seeds, either way, divided by `ln(2 + its total link
    /// weight)` so a hub everything links to does not win by volume.
    ///
    /// Chosen on co-change (files changed in the same commit count as
    /// related; `examples/near_eval.rs`): over django and rust it beat
    /// personalized PageRank (recall@10 0.26 against 0.16) and PageRank divided
    /// by global rank (0.24), since diffusion drifts to generic hubs.
    pub fn related(&self, seeds: &[(u32, f64)]) -> Vec<f64> {
        let n = self.files.len();
        let mut weight = vec![0f64; n];
        for &(seed, w) in seeds {
            weight[seed as usize] += w;
        }
        let mut score = vec![0f64; n];
        let mut degree = vec![0f64; n];
        for a in 0..n {
            for (b, w) in self.out_links(a as u32) {
                let (w, b) = (w as f64, b as usize);
                degree[a] += w;
                degree[b] += w;
                score[b] += weight[a] * w;
                score[a] += weight[b] * w;
            }
        }
        score
            .iter()
            .zip(&degree)
            .map(|(s, d)| s / (2.0 + d).ln())
            .collect()
    }

    /// Definitions of `name`.
    pub fn defs_named(&self, name: &str) -> impl Iterator<Item = &Def> {
        self.by_name
            .get_or_init(|| {
                let mut by_name: HashMap<String, Vec<u32>> = HashMap::new();
                for (i, d) in self.defs.iter().enumerate() {
                    by_name.entry(d.name.clone()).or_default().push(i as u32);
                }
                by_name
            })
            .get(name)
            .into_iter()
            .flatten()
            .map(|&i| &self.defs[i as usize])
    }

    /// Definitions in a file, in line order.
    pub fn defs_in(&self, file: u32) -> Vec<&Def> {
        self.by_file
            .get(file as usize)
            .into_iter()
            .flatten()
            .map(|&i| &self.defs[i as usize])
            .collect()
    }

    /// Links of a file: `(target, weight)`.
    pub fn out_links(&self, file: u32) -> impl Iterator<Item = (u32, f32)> + '_ {
        let (s, e) = (
            self.offsets[file as usize] as usize,
            self.offsets[file as usize + 1] as usize,
        );
        self.targets[s..e]
            .iter()
            .copied()
            .zip(self.weights[s..e].iter().copied())
    }

    /// PageRank, global (`seeds = None`) or personalized to `seeds` (file,
    /// weight). Personalized walks follow links both ways: a file is related
    /// to what it uses and to what uses it.
    pub fn propagate(&self, seeds: Option<&[(u32, f64)]>) -> Vec<f64> {
        let n = self.files.len();
        if n == 0 {
            return Vec::new();
        }
        let undirected = seeds.is_some();
        // Row-normalised weights.
        let mut out_sum = vec![0f64; n];
        for a in 0..n {
            for (b, w) in self.out_links(a as u32) {
                out_sum[a] += w as f64;
                if undirected {
                    out_sum[b as usize] += w as f64;
                }
            }
        }
        let restart: Vec<f64> = match seeds {
            Some(seeds) if !seeds.is_empty() => {
                let total: f64 = seeds.iter().map(|s| s.1).sum::<f64>().max(1e-12);
                let mut r = vec![0f64; n];
                for &(f, w) in seeds {
                    r[f as usize] += w / total;
                }
                r
            }
            _ => vec![1.0 / n as f64; n],
        };
        let mut rank = restart.clone();
        for _ in 0..ITERATIONS {
            let mut next = vec![0f64; n];
            let mut dangling = 0f64;
            for a in 0..n {
                if out_sum[a] == 0.0 {
                    dangling += rank[a];
                    continue;
                }
                let share = rank[a] / out_sum[a];
                for (b, w) in self.out_links(a as u32) {
                    next[b as usize] += share * w as f64;
                }
            }
            if undirected {
                for (a, slot) in next.iter_mut().enumerate() {
                    for (b, w) in self.out_links(a as u32) {
                        let b = b as usize;
                        if out_sum[b] > 0.0 {
                            *slot += rank[b] / out_sum[b] * w as f64;
                        }
                    }
                }
            }
            for (i, v) in next.iter_mut().enumerate() {
                *v = DAMPING * (*v + dangling * restart[i]) + (1.0 - DAMPING) * restart[i];
            }
            rank = next;
        }
        rank
    }

    fn save(&self, index_dir: &Path) -> Result<()> {
        let dir = index_dir.join(GRAPH_DIR);
        std::fs::create_dir_all(&dir)?;
        let mut buf: Vec<u8> = Vec::new();
        let put = |buf: &mut Vec<u8>, v: u32| buf.extend_from_slice(&v.to_le_bytes());
        let put_str = |buf: &mut Vec<u8>, s: &str| {
            buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
        };
        buf.extend_from_slice(b"OGRAPH\0\0");
        put(&mut buf, VERSION);
        buf.extend_from_slice(&self.file_table_id);
        put(&mut buf, self.files.len() as u32);
        for f in &self.files {
            put_str(&mut buf, f);
        }
        for &r in &self.pagerank {
            buf.extend_from_slice(&r.to_le_bytes());
        }
        put(&mut buf, self.targets.len() as u32);
        for &o in &self.offsets {
            put(&mut buf, o);
        }
        for &t in &self.targets {
            put(&mut buf, t);
        }
        for &w in &self.weights {
            buf.extend_from_slice(&w.to_le_bytes());
        }
        put(&mut buf, self.defs.len() as u32);
        for d in &self.defs {
            put(&mut buf, d.file);
            buf.push(d.kind);
            put(&mut buf, d.line);
            put(&mut buf, d.end_line);
            buf.push(u8::from(d.linkable));
            put_str(&mut buf, &d.name);
            put_str(&mut buf, &d.signature);
        }
        // A temporary name per process, synced before it replaces the graph:
        // two builds at once must not publish each other's half-written file,
        // nor a crash leave a truncated one.
        let tmp = dir.join(format!("graph.bin.{}.tmp", std::process::id()));
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&buf)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(tmp, dir.join(GRAPH_FILE))?;
        Ok(())
    }

    /// Load the stored graph. `None` when there is none or it is unreadable.
    pub fn load(index_dir: &Path) -> Option<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(index_dir.join(GRAPH_DIR).join(GRAPH_FILE))
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        struct Cursor<'a>(&'a [u8]);
        impl<'a> Cursor<'a> {
            fn take(&mut self, n: usize) -> Option<&'a [u8]> {
                let (h, t) = self.0.split_at_checked(n)?;
                self.0 = t;
                Some(h)
            }
            fn u32(&mut self) -> Option<u32> {
                Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
            }
            fn f32(&mut self) -> Option<f32> {
                Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
            }
            fn string(&mut self) -> Option<String> {
                let n = self.u32()? as usize;
                String::from_utf8(self.take(n)?.to_vec()).ok()
            }
        }
        let mut c = Cursor(&bytes);
        if c.take(8)? != b"OGRAPH\0\0" || c.u32()? != VERSION {
            return None;
        }
        let file_table_id: [u8; 32] = c.take(32)?.try_into().ok()?;
        let n = c.u32()? as usize;
        // Counts come from the file: never reserve more than its bytes could
        // hold (a corrupt count would otherwise ask for gigabytes).
        let cap = |count: usize, each: usize| count.min(bytes.len() / each);
        let mut files = Vec::with_capacity(cap(n, 4));
        for _ in 0..n {
            files.push(c.string()?);
        }
        let mut pagerank = Vec::with_capacity(cap(n, 4));
        for _ in 0..n {
            pagerank.push(c.f32()?);
        }
        let m = c.u32()? as usize;
        let mut offsets = Vec::with_capacity(cap(n + 1, 4));
        for _ in 0..=n {
            offsets.push(c.u32()?);
        }
        let mut targets = Vec::with_capacity(cap(m, 4));
        for _ in 0..m {
            targets.push(c.u32()?);
        }
        let mut weights = Vec::with_capacity(cap(m, 4));
        for _ in 0..m {
            weights.push(c.f32()?);
        }
        let d = c.u32()? as usize;
        let mut defs = Vec::with_capacity(cap(d, 18));
        for _ in 0..d {
            let file = c.u32()?;
            let kind = c.take(1)?[0];
            let line = c.u32()?;
            let end_line = c.u32()?;
            let linkable = c.take(1)?[0] != 0;
            let name = c.string()?;
            let signature = c.string()?;
            defs.push(Def {
                file,
                kind,
                name,
                line,
                end_line,
                signature,
                linkable,
            });
        }
        // Queries index by these without checks: a corrupt file must fail to
        // load ("unreadable"), not panic later.
        let links_ok = offsets.first() == Some(&0)
            && offsets.last().is_some_and(|&last| last as usize == m)
            && offsets.windows(2).all(|w| w[0] <= w[1])
            && targets.iter().all(|&t| (t as usize) < n)
            && defs.iter().all(|d| (d.file as usize) < n);
        if !links_ok {
            return None;
        }
        let mut graph = Graph {
            file_table_id,
            files,
            pagerank,
            offsets,
            targets,
            weights,
            defs,
            by_name: std::sync::OnceLock::new(),
            by_path: HashMap::new(),
            by_file: Vec::new(),
        };
        graph.index_lookups();
        Some(graph)
    }
}

/// Whether `index_dir` holds a file graph (cheap: one file check).
pub fn exists(index_dir: &Path) -> bool {
    index_dir.join(GRAPH_DIR).join(GRAPH_FILE).is_file()
}

/// The stored graph, whether it matches the current index, or an error when
/// there is none.
pub fn open(index_dir: &Path) -> Result<(Graph, bool)> {
    let graph = Graph::load(index_dir)
        .ok_or_else(|| Error::IndexNotFound(format!("{}/{GRAPH_DIR}", index_dir.display())))?;
    let current = IndexReader::open(index_dir)
        .map(|r| r.file_table_id() == graph.file_table_id)
        .unwrap_or(false);
    Ok((graph, current))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_skip_short_numbers_and_masked_ranges() {
        let text = b"fn bulk_update(x: u32) { 123abc; foo_bar(); /* hidden_name */ }";
        let masked_at = text.windows(11).position(|w| w == b"hidden_name").unwrap();
        let ids = identifiers(text, &|at| at == masked_at);
        assert_eq!(ids, vec!["bulk_update", "foo_bar", "u32"]);
    }

    #[test]
    fn builds_links_to_definers_and_ranks_the_used_file_highest() {
        let repo = tempfile::tempdir().unwrap();
        let src = repo.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("core.rs"),
            "pub fn shared_helper() {}\npub struct Engine;\n",
        )
        .unwrap();
        for i in 0..4 {
            std::fs::write(
                src.join(format!("user{i}.rs")),
                "fn run() { shared_helper(); let e = Engine; }\n",
            )
            .unwrap();
        }
        std::fs::write(src.join("lonely.rs"), "fn alone_here() {}\n").unwrap();
        let index = tempfile::tempdir().unwrap();
        crate::builder::build_index(repo.path(), Some(index.path()), false, false, &[]).unwrap();
        let g = build(repo.path(), index.path()).unwrap();
        let core = g.file("src/core.rs").unwrap();
        let user = g.file("src/user0.rs").unwrap();
        assert!(g.out_links(user).any(|(t, _)| t == core));
        let best = (0..g.files.len())
            .max_by(|&a, &b| g.pagerank[a].total_cmp(&g.pagerank[b]))
            .unwrap();
        #[cfg(feature = "structure")]
        assert_eq!(best as u32, core);
        let _ = best;
        // Round trip.
        let (loaded, current) = open(index.path()).unwrap();
        assert!(current);
        assert_eq!(loaded.files, g.files);
        assert_eq!(loaded.edges(), g.edges());
        assert_eq!(loaded.defs, g.defs);
        // Personalized from a user: the core file is the closest neighbour.
        let ppr = loaded.propagate(Some(&[(user, 1.0)]));
        let lonely = loaded.file("src/lonely.rs").unwrap();
        assert!(ppr[core as usize] > ppr[lonely as usize]);

        // A link to a file past the end is corrupt: unreadable, not a panic
        // in a later query. The first target sits right after the offsets.
        let path = index.path().join(GRAPH_DIR).join(GRAPH_FILE);
        let mut bytes = std::fs::read(&path).unwrap();
        let n = g.files.len();
        let names: usize = g.files.iter().map(|f| 4 + f.len()).sum();
        let first_target = 8 + 4 + 32 + 4 + names + 4 * n + 4 + 4 * (n + 1);
        bytes[first_target..first_target + 4].copy_from_slice(&(n as u32).to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(Graph::load(index.path()).is_none());
    }
}
