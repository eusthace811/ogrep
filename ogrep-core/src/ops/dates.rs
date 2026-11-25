//! When each entity and edge of the operations files entered them: the first
//! commit of the unbroken run, along HEAD's first-parent history, in which the
//! files have held it. A dependency removed and added back is dated by the
//! commit that added it back.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hash;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Stdio};

use serde::Serialize;

use super::{OpsModel, read_by_scan, scan_with};
use crate::history::{git, git_output, shallow_boundary};
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Since {
    pub commit: String,
    /// Commit time, in seconds since the Unix epoch.
    pub time: i64,
}

/// What a commit's files hold, each with the commit since which they have
/// held it: `None` when that is the boundary of a shallow clone, whose earlier
/// history is missing.
struct Held<K>(HashMap<K, Option<Since>>);

impl<K: Eq + Hash> Held<K> {
    fn step(&mut self, now: impl Iterator<Item = K>, at: &Option<Since>) {
        let now: HashSet<K> = now.collect();
        self.0.retain(|k, _| now.contains(k));
        for k in now {
            self.0.entry(k).or_insert_with(|| at.clone());
        }
    }

    fn since(&self, k: &K) -> Option<Since> {
        self.0.get(k).cloned().flatten()
    }
}

/// Blob texts from one `git cat-file --batch`. A blob the scan does not read is
/// kept as `None`.
struct Blobs {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    texts: HashMap<String, Option<String>>,
}

impl Blobs {
    fn fetch(&mut self, sha: &str, path: &str) -> Result<()> {
        if self.texts.contains_key(sha) {
            return Ok(());
        }
        writeln!(self.stdin, "{sha}")?;
        self.stdin.flush()?;
        let mut header = String::new();
        self.stdout.read_line(&mut header)?;
        let size: usize = match header.split_whitespace().collect::<Vec<_>>()[..] {
            [_, "blob", size] => size
                .parse()
                .map_err(|_| Error::Io(std::io::Error::other(format!("git cat-file: {header}"))))?,
            _ => {
                self.texts.insert(sha.to_string(), None);
                return Ok(());
            }
        };
        let mut bytes = vec![0; size + 1];
        self.stdout.read_exact(&mut bytes)?;
        bytes.pop();
        let text = String::from_utf8(bytes)
            .ok()
            .filter(|t| read_by_scan(path, t));
        self.texts.insert(sha.to_string(), text);
        Ok(())
    }

    fn read_by_scan(&self, sha: &str) -> bool {
        self.texts.get(sha).is_some_and(Option::is_some)
    }
}

/// One first-parent commit that changed an operations file: its id, time, and
/// each changed path with its blob after the commit (`None` when deleted).
type Step = (String, i64, Vec<(String, Option<String>)>);

fn steps(repo: &Path) -> Result<Vec<Step>> {
    let log = git_output(
        repo,
        &[
            "log",
            "--first-parent",
            "--reverse",
            "--root",
            "--diff-merges=first-parent",
            "--raw",
            "--no-renames",
            "--no-abbrev",
            "--format=%x01%H %ct",
            "HEAD",
            "--",
            "*.yaml",
            "*.yml",
            "*.env",
        ],
    )?;
    let mut out = Vec::new();
    for chunk in log.split('\x01').filter(|c| !c.trim().is_empty()) {
        let mut lines = chunk.lines();
        let Some((sha, time)) = lines.next().and_then(|l| l.split_once(' ')) else {
            continue;
        };
        let Ok(time) = time.trim().parse() else {
            continue;
        };
        let changes = lines
            .filter_map(|l| {
                let (meta, path) = l.strip_prefix(':')?.split_once('\t')?;
                let fields: Vec<&str> = meta.split(' ').collect();
                let (blob, status) = (fields.get(3)?, fields.get(4)?);
                let blob = (!status.starts_with('D')).then(|| blob.to_string());
                Some((path.to_string(), blob))
            })
            .collect();
        out.push((sha.to_string(), time, changes));
    }
    Ok(out)
}

/// Sets `since` on every entity and edge of `model` (the files as they are now)
/// that HEAD also holds. One that only the work tree holds, or that a shallow
/// clone holds since its boundary, keeps none. A repository without commits
/// dates nothing.
pub fn date(repo: &Path, model: &mut OpsModel) -> Result<()> {
    if git_output(repo, &["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
        return Ok(());
    }
    let steps = steps(repo)?;
    let boundary = shallow_boundary(repo);
    let mut child = git(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut blobs = Blobs {
        stdin: child.stdin.take().expect("stdin is piped"),
        stdout: BufReader::new(child.stdout.take().expect("stdout is piped")),
        texts: HashMap::new(),
    };
    let mut tree: BTreeMap<String, String> = BTreeMap::new();
    let mut entities = Held(HashMap::new());
    let mut edges = Held(HashMap::new());
    for (sha, time, changes) in steps {
        // A commit that changes only files the scan skips leaves the model as it was.
        let mut changed = false;
        for (path, blob) in changes {
            changed |= tree.get(&path).is_some_and(|old| blobs.read_by_scan(old));
            match blob {
                Some(blob) => {
                    blobs.fetch(&blob, &path)?;
                    changed |= blobs.read_by_scan(&blob);
                    tree.insert(path, blob);
                }
                None => {
                    tree.remove(&path);
                }
            }
        }
        if !changed {
            continue;
        }
        let paths: Vec<String> = tree.keys().cloned().collect();
        let read = |p: &str| {
            tree.get(p)
                .and_then(|b| blobs.texts.get(b))
                .cloned()
                .flatten()
        };
        let m = scan_with(&paths, &read);
        let at = (!boundary.contains(&sha)).then_some(Since { commit: sha, time });
        entities.step(m.entities.into_iter().map(|e| (e.kind, e.id)), &at);
        edges.step(m.edges.into_iter().map(|e| (e.from, e.to, e.relation)), &at);
    }
    drop(blobs);
    child.wait()?;
    for e in &mut model.entities {
        e.since = entities.since(&(e.kind.clone(), e.id.clone()));
    }
    for e in &mut model.edges {
        e.since = edges.since(&(e.from.clone(), e.to.clone(), e.relation.clone()));
    }
    Ok(())
}
