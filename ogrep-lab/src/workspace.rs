//! The repository and index the lab looks at, with the loaded index, graph and
//! history cached until their files change on disk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};
use ogrep_core::graph::Graph;
use ogrep_core::history::HistoryIndex;
use ogrep_core::meta::IndexMeta;
use ogrep_core::reader::IndexReader;

pub struct Workspace {
    /// The indexed tree, as the index recorded it.
    pub root: PathBuf,
    pub index_dir: PathBuf,
    /// The `ogrep` binary the search, history and evaluation features run.
    pub ogrep: Option<PathBuf>,
    pub ogrep_version: Option<String>,
    index: Cached<Index>,
    graph: Cached<GraphView>,
    history: Cached<HistoryIndex>,
}

/// A loaded index with its paths sorted for directory listings.
pub struct Index {
    pub reader: IndexReader,
    pub meta: IndexMeta,
    /// `(path, file id)` in path order.
    pub sorted: Vec<(String, u32)>,
}

impl Index {
    /// File ids whose path starts with `prefix`, as a range of `sorted`.
    pub fn under(&self, prefix: &str) -> &[(String, u32)] {
        let lo = self.sorted.partition_point(|(p, _)| p.as_str() < prefix);
        let hi = lo + self.sorted[lo..].partition_point(|(p, _)| p.starts_with(prefix));
        &self.sorted[lo..hi]
    }

    pub fn contains(&self, path: &str) -> bool {
        self.sorted
            .binary_search_by(|(p, _)| p.as_str().cmp(path))
            .is_ok()
    }
}

/// The file graph plus what it does not store: links into each file.
pub struct GraphView {
    pub graph: Graph,
    /// `incoming[f]`: `(source, weight)` of every link into `f`.
    pub incoming: Vec<Vec<(u32, f32)>>,
    /// Whether the graph was built from the current file table.
    pub current: bool,
}

impl GraphView {
    fn new(graph: Graph, current: bool) -> Self {
        let mut incoming = vec![Vec::new(); graph.files.len()];
        for a in 0..graph.files.len() as u32 {
            for (b, w) in graph.out_links(a) {
                incoming[b as usize].push((a, w));
            }
        }
        Self {
            graph,
            incoming,
            current,
        }
    }
}

/// A value loaded from files, reloaded when the modification time of its
/// marker file changes. A failed load is remembered too, so a broken file is
/// reported once rather than re-read on every request.
struct Cached<T> {
    /// The marker's modification time when loaded, and what the load gave.
    slot: Mutex<Option<(Option<SystemTime>, Loaded<T>)>>,
}

type Loaded<T> = Result<Arc<T>, String>;

impl<T> Cached<T> {
    fn new() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }

    fn get(&self, marker: &Path, load: impl FnOnce() -> Result<T>) -> Result<Arc<T>> {
        let stamp = std::fs::metadata(marker).and_then(|m| m.modified()).ok();
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((s, value)) = slot.as_ref()
            && *s == stamp
        {
            return value.clone().map_err(|e| anyhow!(e));
        }
        let value = load().map(Arc::new).map_err(|e| {
            let msg = format!("{e:#}");
            eprintln!("ogrep-lab: {msg}");
            msg
        });
        *slot = Some((stamp, value.clone()));
        value.map_err(|e| anyhow!(e))
    }

    /// Why the last load failed, if it did.
    fn error(&self) -> Option<String> {
        let slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_ref().and_then(|(_, v)| v.as_ref().err().cloned())
    }
}

impl Workspace {
    /// Find the index for `path`: `index_path` if given, else the nearest
    /// `.ogrep` directory at or above `path`.
    pub fn open(path: &Path, index_path: Option<&Path>, ogrep: Option<PathBuf>) -> Result<Self> {
        let index_dir = match index_path {
            Some(p) => p.to_path_buf(),
            None => {
                let start = std::fs::canonicalize(path)
                    .with_context(|| format!("no such directory: {}", path.display()))?;
                start
                    .ancestors()
                    .map(ogrep_core::builder::default_index_dir)
                    .find(|d| d.join("meta.json").is_file())
                    .ok_or_else(|| {
                        anyhow!(
                            "no ogrep index at or above {}; build one with `ogrep index {}`",
                            start.display(),
                            path.display()
                        )
                    })?
            }
        };
        let meta = IndexMeta::load(&index_dir)
            .map_err(|e| anyhow!("cannot read the index in {}: {e}", index_dir.display()))?;
        let root = PathBuf::from(&meta.root_path);
        let ogrep = ogrep.or_else(find_ogrep);
        let ogrep_version = ogrep.as_deref().and_then(ogrep_version);
        Ok(Self {
            root,
            index_dir,
            ogrep,
            ogrep_version,
            index: Cached::new(),
            graph: Cached::new(),
            history: Cached::new(),
        })
    }

    pub fn index(&self) -> Result<Arc<Index>> {
        self.index.get(&self.index_dir.join("meta.json"), || {
            let reader = IndexReader::open(&self.index_dir)
                .map_err(|e| anyhow!("cannot open the index: {e}"))?;
            let meta = IndexMeta::load(&self.index_dir).map_err(|e| anyhow!("{e}"))?;
            let mut sorted: Vec<(String, u32)> = reader
                .all_paths()
                .iter()
                .enumerate()
                .map(|(i, p)| (p.to_string(), i as u32))
                .collect();
            sorted.sort_unstable();
            Ok(Index {
                reader,
                meta,
                sorted,
            })
        })
    }

    /// The file graph, or `None` when `ogrep index --graph` has not been run.
    pub fn graph(&self) -> Option<Arc<GraphView>> {
        let marker = self.index_dir.join("graph").join("graph.bin");
        if !marker.is_file() {
            return None;
        }
        self.graph
            .get(&marker, || {
                let (graph, current) = ogrep_core::graph::open(&self.index_dir)
                    .map_err(|e| anyhow!("cannot read the graph: {e}"))?;
                Ok(GraphView::new(graph, current))
            })
            .ok()
    }

    /// The history index, or `None` when `ogrep index --history` has not been run.
    pub fn history(&self) -> Option<Arc<HistoryIndex>> {
        let marker = self.index_dir.join("history").join("history.json");
        if !marker.is_file() {
            return None;
        }
        self.history
            .get(&marker, || {
                HistoryIndex::open(&self.index_dir).map_err(|e| anyhow!("{e}"))
            })
            .ok()
    }

    /// Parts of the index that exist but could not be read, such as a graph
    /// written by a newer ogrep. Without this they would look unbuilt.
    pub fn load_errors(&self) -> Vec<String> {
        [self.graph.error(), self.history.error()]
            .into_iter()
            .flatten()
            .collect()
    }

    /// Where evaluation runs are kept: inside the index, which ogrep's own
    /// rebuilds leave alone (they replace files, not subdirectories).
    pub fn runs_dir(&self) -> PathBuf {
        self.index_dir.join("lab").join("runs")
    }

    /// An indexed path as a file under the root. The path must be in the
    /// index, which is what keeps requests inside the repository.
    pub fn indexed_file(&self, path: &str) -> Result<PathBuf> {
        let index = self.index()?;
        if !index.contains(path) {
            return Err(anyhow!("{path} is not an indexed file"));
        }
        Ok(ogrep_core::hybrid::join_indexed_path(&self.root, path))
    }
}

/// What `BIN --version` reports, without the program name.
pub fn ogrep_version(bin: &Path) -> Option<String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.trim().trim_start_matches("ogrep ").to_string())
}

/// `ogrep` next to this binary (they are installed together), else on `PATH`.
fn find_ogrep() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ogrep.exe" } else { "ogrep" };
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(name)));
    if let Some(s) = sibling.filter(|s| s.is_file()) {
        return Some(s);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}
