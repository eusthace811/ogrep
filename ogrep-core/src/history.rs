//! A searchable index of the lines each commit added or removed.
//!
//! `git log -G PATTERN` answers "which commits changed a line matching this?"
//! by diffing every commit, which takes seconds to minutes on a large history.
//! This module indexes those diffs once: every non-merge commit reachable from
//! HEAD becomes one document holding the text of its added and removed lines,
//! in an ordinary ogrep index under `<index>/history/`. A search then diffs only
//! the few commits the index says can match.
//!
//! ## Documents
//!
//! A commit's document is its changed lines, one per line, without the `+`/`-`
//! marker, so patterns anchored with `^` behave as they do on source files. The
//! document is regenerated from git at search time to verify candidates and to
//! recover each line's file, side and line number, so no copy of the history is
//! stored. Both passes run [`git_diff`] with the same fixed options (rename
//! detection, no external diff drivers or text conversion, no context lines),
//! which makes the text identical.
//!
//! Document names in `files.bin` are `<commit time, 10 digits>/<sha>`: sorting
//! names orders commits by time, and a time filter needs no git call.
//!
//! Merge commits are skipped, as `git log` does by default, so results match
//! `git log -G`. A document is capped at [`DOC_CAP_BYTES`]; lines past the cap
//! are left out and the commit is marked truncated. Binary files are skipped.
//!
//! ## Updates
//!
//! `history.json` records the newest indexed commit. An update indexes the
//! commits since then as a delta and merges it in; if that commit is no longer
//! an ancestor of HEAD (a rewritten branch), the history is rebuilt.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::reader::IndexReader;
use crate::{Error, Result};

/// Subdirectory of an index directory holding the history index.
pub const HISTORY_DIR: &str = "history";
const META_FILE: &str = "history.json";
/// Document format version. Bump when [`CommitDoc::text`] or [`git_diff`] changes.
pub const VERSION: u32 = 1;
/// Commits indexed by default: the most recent ones, newest first.
pub const DEFAULT_DEPTH: usize = 20_000;
/// Largest document indexed per commit, the same limit as the file index's
/// default `--max-filesize`. Mass reformatting and vendoring commits reach tens
/// of megabytes (django's Black reformat is 20 MB); below this cap they are
/// indexed whole, so results still equal `git log -G`. Past it, the rest of the
/// commit is left out and the commit is counted as truncated.
pub const DOC_CAP_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMeta {
    pub version: u32,
    /// Newest commit indexed (HEAD when the history was built or updated).
    pub head: String,
    /// Number of commits indexed.
    pub commits: usize,
    /// Commit limit of the initial build (`None`: every commit).
    pub depth: Option<usize>,
    /// Date limit of the initial build, as given.
    pub since: Option<String>,
    /// Commits whose document was cut at [`DOC_CAP_BYTES`].
    pub truncated: usize,
}

/// How much history a build covers.
#[derive(Debug, Clone, Default)]
pub struct HistoryOptions {
    pub depth: Option<usize>,
    pub since: Option<String>,
}

/// One added or removed line of a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedLine {
    /// Path after the change (before it, for a deleted file).
    pub path: String,
    pub added: bool,
    /// Line number in the new file (added) or the old file (removed).
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct CommitDoc {
    pub sha: String,
    pub time: i64,
    pub lines: Vec<ChangedLine>,
    pub truncated: bool,
    /// The files whose lines `lines` holds, in order: each with its path before
    /// the change when that differs (a rename), and the index of its first line.
    pub files: Vec<DiffFile>,
}

/// A file with changed lines in a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub old_path: Option<String>,
    pub first_line: usize,
}

/// A slice of a large commit indexed as its own document (see
/// [`CommitDoc::chunks`]).
pub struct Chunk {
    pub name: String,
    pub text: String,
    /// Paths that reproduce the chunk's diff when passed to `git diff-tree`:
    /// every file's path, and its old path too for a rename, so git still pairs
    /// the two sides.
    pub paths: Vec<String>,
}

impl CommitDoc {
    /// The indexed text: each changed line followed by a newline.
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.lines.iter().map(|l| l.text.len() + 1).sum());
        for line in &self.lines {
            text.push_str(&line.text);
            text.push('\n');
        }
        text
    }

    /// Name of the commit's document in `files.bin`.
    pub fn name(&self) -> String {
        doc_name(self.time, &self.sha)
    }

    /// Split a commit whose changed text exceeds [`CHUNK_THRESHOLD`] into
    /// documents of whole files, about [`CHUNK_TARGET`] bytes or
    /// [`CHUNK_MAX_FILES`] files each. A search re-diffs only the chunks that
    /// can match, limited to their own files, instead of the whole commit: the
    /// handful of mass-edit commits in a history otherwise make every search
    /// re-diff hundreds of megabytes. `None` for a commit small enough to be one
    /// document.
    pub fn chunks(&self) -> Option<Vec<Chunk>> {
        let size: usize = self.lines.iter().map(|l| l.text.len() + 1).sum();
        if size <= CHUNK_THRESHOLD {
            return None;
        }
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut text = String::new();
        let mut paths: Vec<String> = Vec::new();
        let mut files_in_chunk = 0;
        for (i, file) in self.files.iter().enumerate() {
            let end = self
                .files
                .get(i + 1)
                .map_or(self.lines.len(), |f| f.first_line);
            for line in &self.lines[file.first_line..end] {
                text.push_str(&line.text);
                text.push('\n');
            }
            paths.push(file.path.clone());
            if let Some(old) = &file.old_path {
                paths.push(old.clone());
            }
            files_in_chunk += 1;
            if text.len() >= CHUNK_TARGET || files_in_chunk >= CHUNK_MAX_FILES {
                chunks.push(Chunk {
                    name: format!("{}#{}", self.name(), chunks.len()),
                    text: std::mem::take(&mut text),
                    paths: std::mem::take(&mut paths),
                });
                files_in_chunk = 0;
            }
        }
        if !paths.is_empty() {
            chunks.push(Chunk {
                name: format!("{}#{}", self.name(), chunks.len()),
                text,
                paths,
            });
        }
        Some(chunks)
    }
}

/// Commits with more changed text than this are indexed in chunks.
pub const CHUNK_THRESHOLD: usize = 1 << 20;
/// Target size of a chunk.
pub const CHUNK_TARGET: usize = 512 << 10;
/// Most files in a chunk, which bounds the pathspec a re-diff passes to git.
pub const CHUNK_MAX_FILES: usize = 200;
const CHUNKS_FILE: &str = "chunks.json";

pub fn doc_name(time: i64, sha: &str) -> String {
    format!("{:010}/{sha}", time.max(0))
}

/// Split a document name back into `(time, sha)`, ignoring a chunk suffix.
pub fn parse_doc_name(name: &str) -> Option<(i64, &str)> {
    let (time, rest) = name.split_once('/')?;
    let sha = rest.split_once('#').map_or(rest, |(sha, _)| sha);
    Some((time.parse().ok()?, sha))
}

/// Directory of the history index for an index directory.
pub fn history_dir(index_dir: &Path) -> PathBuf {
    index_dir.join(HISTORY_DIR)
}

/// Whether `index_dir` holds a history index (cheap: one file check).
pub fn has_history(index_dir: &Path) -> bool {
    history_dir(index_dir).join(META_FILE).is_file()
}

/// `git -C ROOT` with paths printed unquoted (non-ASCII names come back as
/// UTF-8, not `"caf\303\251"`), no colour and no standard input. Every git
/// call ogrep makes starts here.
pub fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off", "-c", "color.ui=never"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    cmd
}

/// Run `git ARGS` in `root` and return its output, or its complaint as the
/// error.
pub fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let out = git(root).args(args).output()?;
    if !out.status.success() {
        return Err(Error::Io(std::io::Error::other(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Files git knows in the work tree: tracked ones, plus untracked ones not
/// ignored when `untracked` is set. Paths are relative to `repo`.
pub fn ls_files(repo: &Path, untracked: bool) -> Result<Vec<String>> {
    let mut args = vec!["ls-files", "-z", "--cached"];
    if untracked {
        args.extend(["--others", "--exclude-standard"]);
    }
    Ok(git_output(repo, &args)?
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect())
}

/// The work tree's top directory.
pub fn repo_root(path: &Path) -> Result<PathBuf> {
    let dir = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    Ok(PathBuf::from(
        git_output(dir, &["rev-parse", "--show-toplevel"])?.trim(),
    ))
}

/// HEAD's commit id.
pub fn head(root: &Path) -> Result<String> {
    Ok(git_output(root, &["rev-parse", "HEAD"])?.trim().to_string())
}

/// The boundary commits of a shallow clone. Their parents are missing, so git
/// diffs them against nothing: each looks like one commit that added the whole
/// tree. They are artifacts of the clone, not changes, and are not indexed.
pub fn shallow_boundary(root: &Path) -> HashSet<String> {
    git_output(root, &["rev-parse", "--git-path", "shallow"])
        .ok()
        .map(|path| {
            let path = PathBuf::from(path.trim());
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            std::fs::read_to_string(path).unwrap_or_default()
        })
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Non-merge commits as `(sha, commit time)`, newest first, without a shallow
/// clone's boundary commits. `range` is a revision range such as `HEAD` or
/// `old..HEAD`.
pub fn list_commits(root: &Path, range: &str, opts: &HistoryOptions) -> Result<Vec<(String, i64)>> {
    let mut args = vec![
        "log".to_string(),
        "--no-merges".into(),
        "--format=%H %ct".into(),
    ];
    if let Some(depth) = opts.depth {
        args.push(format!("--max-count={depth}"));
    }
    if let Some(since) = &opts.since {
        args.push(format!("--since={since}"));
    }
    args.push(range.to_string());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = git_output(root, &args)?;
    let boundary = shallow_boundary(root);
    Ok(out
        .lines()
        .filter_map(|line| {
            let (sha, time) = line.split_once(' ')?;
            Some((sha.to_string(), time.parse().ok()?))
        })
        .filter(|(sha, _)| !boundary.contains(sha))
        .collect())
}

/// `git diff-tree` with the options every document is built with. Index and
/// search must use exactly these, or the text a search verifies against would
/// differ from the text that was indexed.
fn diff_tree(root: &Path) -> Command {
    let mut cmd = git(root);
    cmd.args([
        "diff-tree",
        "-p",
        "--unified=0",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "-M",
        "--root",
        "--diff-merges=first-parent",
        "--format=%x01%H %ct",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    cmd
}

/// Parse a running `git diff-tree`'s output, passing each commit to `each`.
/// Parse `git diff-tree` output from `child`, passing each commit to `each`
/// until it says to stop, which ends git early.
fn read_diffs(
    mut child: std::process::Child,
    mut each: impl FnMut(CommitDoc) -> ControlFlow<()>,
) -> Result<()> {
    let stdout = child.stdout.take().expect("piped stdout");
    let mut parser = DiffParser::default();
    let mut reader = BufReader::with_capacity(1 << 16, stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if let Some(doc) = parser.line(&buf)
            && each(doc).is_break()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
    }
    if let Some(doc) = parser.finish() {
        let _ = each(doc);
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(Error::Io(std::io::Error::other("git diff-tree failed")));
    }
    Ok(())
}

/// Diff each commit against its first parent and pass the parsed result to
/// `each`, in the order given, until it breaks. One `git diff-tree` process
/// serves every commit.
pub fn git_diff(
    root: &Path,
    shas: &[String],
    each: impl FnMut(CommitDoc) -> ControlFlow<()>,
) -> Result<()> {
    if shas.is_empty() {
        return Ok(());
    }
    let mut child = diff_tree(root)
        .arg("--stdin")
        .stdin(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input: Vec<u8> = shas
        .iter()
        .flat_map(|s| format!("{s}\n").into_bytes())
        .collect();
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let result = read_diffs(child, each);
    let _ = feeder.join();
    result
}

/// Diff one commit, limited to `paths` (a chunk's files; see
/// [`CommitDoc::chunks`]). Paths are taken literally, never as globs.
pub fn git_diff_paths(
    root: &Path,
    sha: &str,
    paths: &[String],
    each: impl FnMut(CommitDoc) -> ControlFlow<()>,
) -> Result<()> {
    let child = diff_tree(root)
        .arg(sha)
        .arg("--")
        .args(paths.iter().map(|p| format!(":(literal){p}")))
        .spawn()?;
    read_diffs(child, each)
}

/// Incremental parser for `git diff-tree -p --unified=0` output.
#[derive(Default)]
struct DiffParser {
    doc: Option<CommitDoc>,
    bytes: usize,
    path: Option<String>,
    old_path: Option<String>,
    in_hunk: bool,
    binary: bool,
    old_line: u32,
    new_line: u32,
    /// The current file already has an entry in `CommitDoc::files`.
    file_registered: bool,
}

/// Decode a path from a `---`/`+++` header, which git quotes when it holds
/// special characters. An unquoted path with a space is followed by a tab
/// (so tools can find its end); a real tab in a name would have been quoted.
fn header_path(raw: &[u8], prefix: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let text = text.strip_suffix('\t').unwrap_or(&text);
    if text == "/dev/null" {
        return None;
    }
    let unquoted = if text.starts_with('"') && text.ends_with('"') && text.len() >= 2 {
        unquote(&text[1..text.len() - 1])
    } else {
        text.to_string()
    };
    let prefix = std::str::from_utf8(prefix).unwrap_or("");
    Some(
        unquoted
            .strip_prefix(prefix)
            .unwrap_or(&unquoted)
            .to_string(),
    )
}

/// Undo git's C-style path quoting (`\t`, `\n`, `\"`, `\\`, octal bytes).
fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 == bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let c = bytes[i + 1];
        match c {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'"' => out.push(b'"'),
            b'\\' => out.push(b'\\'),
            b'0'..=b'7' if i + 4 <= bytes.len() => {
                let oct = std::str::from_utf8(&bytes[i + 1..i + 4]).unwrap_or("0");
                out.push(u8::from_str_radix(oct, 8).unwrap_or(b'?'));
                i += 4;
                continue;
            }
            other => out.push(other),
        }
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hunk_starts(line: &[u8]) -> Option<(u32, u32)> {
    // @@ -a[,b] +c[,d] @@ ...
    let text = std::str::from_utf8(line).ok()?;
    let rest = text.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(' ')?;
    let first = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((first(old)?, first(new)?))
}

impl DiffParser {
    fn line(&mut self, line: &[u8]) -> Option<CommitDoc> {
        if let Some(header) = line.strip_prefix(b"\x01") {
            let finished = self.finish();
            let header = String::from_utf8_lossy(header);
            let (sha, time) = header.split_once(' ').unwrap_or((&header, "0"));
            self.doc = Some(CommitDoc {
                sha: sha.to_string(),
                time: time.trim().parse().unwrap_or(0),
                lines: Vec::new(),
                truncated: false,
                files: Vec::new(),
            });
            self.bytes = 0;
            return finished;
        }
        let doc = self.doc.as_mut()?;
        if line.starts_with(b"diff --git ") {
            self.path = None;
            self.old_path = None;
            self.in_hunk = false;
            self.binary = false;
            self.file_registered = false;
            return None;
        }
        if !self.in_hunk {
            if let Some(raw) = line.strip_prefix(b"--- ") {
                self.old_path = header_path(raw, b"a/");
            } else if let Some(raw) = line.strip_prefix(b"+++ ") {
                self.path = header_path(raw, b"b/");
            } else if line.starts_with(b"Binary files ") {
                self.binary = true;
            } else if let Some((old, new)) = hunk_starts(line) {
                self.in_hunk = true;
                self.old_line = old;
                self.new_line = new;
            }
            return None;
        }
        if let Some((old, new)) = hunk_starts(line) {
            self.old_line = old;
            self.new_line = new;
            return None;
        }
        let (added, text) = match line.first() {
            Some(b'+') => (true, &line[1..]),
            Some(b'-') => (false, &line[1..]),
            _ => return None, // "\ No newline at end of file"
        };
        let number = if added {
            let n = self.new_line;
            self.new_line += 1;
            n
        } else {
            let n = self.old_line;
            self.old_line += 1;
            n
        };
        if self.binary || doc.truncated {
            return None;
        }
        if self.bytes + text.len() + 1 > DOC_CAP_BYTES {
            doc.truncated = true;
            return None;
        }
        let path = self
            .path
            .clone()
            .or_else(|| self.old_path.clone())
            .unwrap_or_default();
        if !self.file_registered {
            self.file_registered = true;
            let old_path = self.old_path.clone().filter(|old| *old != path);
            doc.files.push(DiffFile {
                path: path.clone(),
                old_path,
                first_line: doc.lines.len(),
            });
        }
        self.bytes += text.len() + 1;
        doc.lines.push(ChangedLine {
            path,
            added,
            line: number,
            text: String::from_utf8_lossy(text).into_owned(),
        });
        None
    }

    fn finish(&mut self) -> Option<CommitDoc> {
        self.path = None;
        self.old_path = None;
        self.in_hunk = false;
        self.binary = false;
        self.file_registered = false;
        self.doc.take()
    }
}

fn read_meta(dir: &Path) -> Option<HistoryMeta> {
    let meta: HistoryMeta =
        serde_json::from_slice(&std::fs::read(dir.join(META_FILE)).ok()?).ok()?;
    (meta.version == VERSION).then_some(meta)
}

fn write_meta(dir: &Path, meta: &HistoryMeta) -> Result<()> {
    std::fs::write(dir.join(META_FILE), serde_json::to_vec_pretty(meta)?)?;
    Ok(())
}

/// Replace `target` with `staging` (a sibling directory).
fn publish(staging: &Path, target: &Path) -> Result<()> {
    let old = target.with_extension("old");
    let _ = std::fs::remove_dir_all(&old);
    if target.exists() {
        std::fs::rename(target, &old)?;
    }
    std::fs::rename(staging, target)?;
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Outcome of [`build_or_update`].
#[derive(Debug, Clone, Default)]
pub struct HistoryOutcome {
    pub added: usize,
    pub total: usize,
    pub rebuilt: bool,
    pub truncated: usize,
}

/// For each chunk document (see [`CommitDoc::chunks`]), the paths it covers.
type ChunkPaths = std::collections::BTreeMap<String, Vec<String>>;

/// Index `commits` (newest first) into `dir`, returning how many documents
/// were truncated and the paths of each chunk.
fn index_commits(
    root: &Path,
    dir: &Path,
    commits: &[(String, i64)],
    prune_grams: bool,
) -> Result<(usize, ChunkPaths)> {
    let shas: Vec<String> = commits.iter().map(|(sha, _)| sha.clone()).collect();
    let (tx, rx) = std::sync::mpsc::sync_channel::<(String, String, bool)>(256);
    let root_owned = root.to_path_buf();
    let producer = std::thread::spawn(move || -> Result<ChunkPaths> {
        let mut chunk_paths = ChunkPaths::new();
        // A failed send means the index build gave up: stop diffing.
        git_diff(&root_owned, &shas, |doc| {
            let sent = match doc.chunks() {
                Some(chunks) => {
                    let last = chunks.len().saturating_sub(1);
                    chunks.into_iter().enumerate().all(|(i, chunk)| {
                        chunk_paths.insert(chunk.name.clone(), chunk.paths);
                        tx.send((chunk.name, chunk.text, doc.truncated && i == last))
                            .is_ok()
                    })
                }
                None => tx.send((doc.name(), doc.text(), doc.truncated)).is_ok(),
            };
            if sent {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        })?;
        Ok(chunk_paths)
    });
    let mut truncated = 0usize;
    let docs = rx.into_iter().map(|(name, text, cut)| {
        truncated += usize::from(cut);
        (name, text)
    });
    // The build consumes (and on failure drops) the receiver, which stops the
    // producer; join it either way so no git process outlives this call.
    let built = crate::builder::build_index_from_documents(root, dir, docs, prune_grams);
    let produced = producer
        .join()
        .map_err(|_| Error::Io(std::io::Error::other("history producer panicked")))?;
    built?;
    Ok((truncated, produced?))
}

fn read_chunks(dir: &Path) -> ChunkPaths {
    std::fs::read(dir.join(CHUNKS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_chunks(dir: &Path, chunks: &ChunkPaths) -> Result<()> {
    std::fs::write(dir.join(CHUNKS_FILE), serde_json::to_vec(chunks)?)?;
    Ok(())
}

/// Build the history index under `index_dir`, or bring an existing one up to
/// date with HEAD. `force` rebuilds from scratch.
pub fn build_or_update(
    root: &Path,
    index_dir: &Path,
    opts: &HistoryOptions,
    force: bool,
) -> Result<HistoryOutcome> {
    let dir = history_dir(index_dir);
    let head = head(root)?;
    let existing = if force { None } else { read_meta(&dir) };
    let staging = index_dir.join("history.staging");
    let _ = std::fs::remove_dir_all(&staging);

    if let Some(meta) = existing {
        if meta.head == head {
            return Ok(HistoryOutcome {
                total: meta.commits,
                truncated: meta.truncated,
                ..Default::default()
            });
        }
        let ancestor = git(root)
            .args(["merge-base", "--is-ancestor", &meta.head, &head])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ancestor {
            let new = list_commits(
                root,
                &format!("{}..{head}", meta.head),
                &HistoryOptions::default(),
            )?;
            let delta_dir = index_dir.join("history.delta");
            let _ = std::fs::remove_dir_all(&delta_dir);
            // Every gram kept: the delta is merged into a pruned index.
            let (truncated, new_chunks) = index_commits(root, &delta_dir, &new, false)?;
            let reader = IndexReader::open(&dir)?;
            let delta = IndexReader::open(&delta_dir)?;
            crate::builder::merge_index_with_delta(
                root,
                &staging,
                &reader,
                &delta,
                &HashSet::new(),
                true,
            )?;
            drop((reader, delta));
            let _ = std::fs::remove_dir_all(&delta_dir);
            let meta = HistoryMeta {
                head,
                commits: meta.commits + new.len(),
                truncated: meta.truncated + truncated,
                ..meta
            };
            let mut chunks = read_chunks(&dir);
            chunks.extend(new_chunks);
            write_chunks(&staging, &chunks)?;
            write_meta(&staging, &meta)?;
            publish(&staging, &dir)?;
            return Ok(HistoryOutcome {
                added: new.len(),
                total: meta.commits,
                rebuilt: false,
                truncated: meta.truncated,
            });
        }
    }

    let commits = list_commits(root, "HEAD", opts)?;
    let (truncated, chunks) = index_commits(root, &staging, &commits, true)?;
    write_chunks(&staging, &chunks)?;
    let meta = HistoryMeta {
        version: VERSION,
        head,
        commits: commits.len(),
        depth: opts.depth,
        since: opts.since.clone(),
        truncated,
    };
    write_meta(&staging, &meta)?;
    publish(&staging, &dir)?;
    Ok(HistoryOutcome {
        added: commits.len(),
        total: commits.len(),
        rebuilt: true,
        truncated,
    })
}

/// An opened history index.
pub struct HistoryIndex {
    pub reader: IndexReader,
    pub meta: HistoryMeta,
    chunks: ChunkPaths,
}

/// A document the index says can match: a whole commit, or one chunk of a
/// large commit together with the paths that reproduce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub time: i64,
    pub sha: String,
    pub chunk_paths: Option<Vec<String>>,
}

impl HistoryIndex {
    pub fn open(index_dir: &Path) -> Result<Self> {
        let dir = history_dir(index_dir);
        let meta =
            read_meta(&dir).ok_or_else(|| Error::IndexNotFound(dir.display().to_string()))?;
        Ok(Self {
            reader: IndexReader::open(&dir)?,
            meta,
            chunks: read_chunks(&dir),
        })
    }

    /// Candidate documents for a plan, newest commit first.
    pub fn candidates(&self, plan: &crate::query::QueryPlan) -> Vec<Candidate> {
        let ids = if plan.is_match_all() {
            self.reader.all_file_ids()
        } else {
            let mut work = crate::query::QueryWork::default();
            crate::query::execute_plan_indexed(plan, &self.reader, &mut work)
        };
        let mut found: Vec<Candidate> = ids
            .into_iter()
            .filter_map(|id| {
                let name = self.reader.file_path(id)?;
                let (time, sha) = parse_doc_name(name)?;
                let chunk_paths = if name.contains('#') {
                    // A chunk without its paths cannot be re-diffed precisely;
                    // an empty list falls back to the whole commit.
                    Some(self.chunks.get(name).cloned().unwrap_or_default())
                } else {
                    None
                };
                Some(Candidate {
                    time,
                    sha: sha.to_string(),
                    chunk_paths,
                })
            })
            .collect();
        found.sort_unstable_by(|a, b| (b.time, &b.sha).cmp(&(a.time, &a.sha)));
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_paths_drop_the_tab_git_adds_after_spaces() {
        assert_eq!(
            header_path(b"b/x y.txt\t", b"b/").as_deref(),
            Some("x y.txt")
        );
        assert_eq!(
            header_path(b"b/plain.rs", b"b/").as_deref(),
            Some("plain.rs")
        );
        assert_eq!(
            header_path(b"\"b/tab\\there\"", b"b/").as_deref(),
            Some("tab\there")
        );
        assert_eq!(
            header_path(b"\"b/caf\\303\\251\"", b"b/").as_deref(),
            Some("caf\u{e9}")
        );
        assert_eq!(header_path(b"/dev/null", b"b/"), None);
    }

    fn parse(output: &str) -> Vec<CommitDoc> {
        let mut parser = DiffParser::default();
        let mut docs = Vec::new();
        for line in output.split('\n') {
            if let Some(doc) = parser.line(line.as_bytes()) {
                docs.push(doc);
            }
        }
        docs.extend(parser.finish());
        docs
    }

    #[test]
    fn parses_added_and_removed_lines_with_numbers() {
        let out = "\x01abc 1700000000\n\ndiff --git a/src/x.rs b/src/x.rs\nindex 1..2 100644\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -10,2 +10,3 @@ fn f()\n-old one\n-old two\n+new one\n+--- not a header\n+new three\n@@ -40 +41,0 @@\n-gone\n\\ No newline at end of file\ndiff --git a/bin.dat b/bin.dat\nBinary files a/bin.dat and b/bin.dat differ\ndiff --git a/dead.txt b/dead.txt\ndeleted file mode 100644\n--- a/dead.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n\x01def 1600000000\n\ndiff --git \"a/sp ace\\ttab\" \"b/sp ace\\ttab\"\n--- \"a/sp ace\\ttab\"\n+++ \"b/sp ace\\ttab\"\n@@ -0,0 +1 @@\n+hi\n";
        let docs = parse(out);
        assert_eq!(docs.len(), 2);
        let d = &docs[0];
        assert_eq!((d.sha.as_str(), d.time), ("abc", 1_700_000_000));
        let got: Vec<(&str, bool, u32, &str)> = d
            .lines
            .iter()
            .map(|l| (l.path.as_str(), l.added, l.line, l.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("src/x.rs", false, 10, "old one"),
                ("src/x.rs", false, 11, "old two"),
                ("src/x.rs", true, 10, "new one"),
                ("src/x.rs", true, 11, "--- not a header"),
                ("src/x.rs", true, 12, "new three"),
                ("src/x.rs", false, 40, "gone"),
                ("dead.txt", false, 1, "bye"),
            ]
        );
        assert_eq!(
            d.text(),
            "old one\nold two\nnew one\n--- not a header\nnew three\ngone\nbye\n"
        );
        assert_eq!(docs[1].lines[0].path, "sp ace\ttab");
        assert_eq!(docs[1].name(), "1600000000/def");
        assert_eq!(
            parse_doc_name("1600000000/def"),
            Some((1_600_000_000, "def"))
        );
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn commit(dir: &Path, file: &str, body: &str, when: &str) {
        std::fs::write(dir.join(file), body).unwrap();
        run_git(dir, &["add", file]);
        let date = format!("{when} +0000");
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(["commit", "-q", "-m", file])
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn shas_matching(index: &Path, literal: &str) -> Vec<String> {
        let hist = HistoryIndex::open(index).unwrap();
        let plan = crate::query::build_literal_plan(literal, false);
        hist.candidates(&plan).into_iter().map(|c| c.sha).collect()
    }

    #[test]
    fn builds_updates_incrementally_and_rebuilds_after_a_rewrite() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        run_git(root, &["init", "-q", "-b", "main"]);
        commit(root, "a.txt", "alpha_marker one\n", "2020-01-01T00:00:00");
        commit(root, "b.txt", "beta_marker two\n", "2020-01-02T00:00:00");
        let index = tempfile::tempdir().unwrap();
        let first = build_or_update(root, index.path(), &HistoryOptions::default(), false).unwrap();
        assert!(first.rebuilt);
        assert_eq!(first.total, 2);
        assert_eq!(shas_matching(index.path(), "beta_marker").len(), 1);

        // Up to date: nothing to do.
        let same = build_or_update(root, index.path(), &HistoryOptions::default(), false).unwrap();
        assert_eq!((same.added, same.rebuilt), (0, false));

        // New commits are merged in as a delta.
        commit(
            root,
            "a.txt",
            "alpha_marker one\ngamma_marker three\n",
            "2020-01-03T00:00:00",
        );
        let update =
            build_or_update(root, index.path(), &HistoryOptions::default(), false).unwrap();
        assert_eq!((update.added, update.total, update.rebuilt), (1, 3, false));
        assert_eq!(shas_matching(index.path(), "gamma_marker").len(), 1);
        assert_eq!(shas_matching(index.path(), "alpha_marker").len(), 1);

        // A rewritten branch (the indexed head is no longer an ancestor)
        // rebuilds from scratch.
        run_git(root, &["reset", "-q", "--hard", "HEAD~2"]);
        commit(root, "c.txt", "delta_marker four\n", "2020-01-04T00:00:00");
        let rewrite =
            build_or_update(root, index.path(), &HistoryOptions::default(), false).unwrap();
        assert!(rewrite.rebuilt);
        assert_eq!(rewrite.total, 2);
        assert!(shas_matching(index.path(), "gamma_marker").is_empty());
        assert_eq!(shas_matching(index.path(), "delta_marker").len(), 1);
    }

    #[test]
    fn large_commits_split_into_whole_file_chunks() {
        let mut lines = Vec::new();
        let mut files = Vec::new();
        for f in 0..300 {
            files.push(DiffFile {
                path: format!("dir/file{f}.rs"),
                old_path: (f == 7).then(|| "dir/old7.rs".to_string()),
                first_line: lines.len(),
            });
            for n in 0..40 {
                lines.push(ChangedLine {
                    path: format!("dir/file{f}.rs"),
                    added: n % 2 == 0,
                    line: n + 1,
                    text: format!("{f}-{n} {}", "y".repeat(100)),
                });
            }
        }
        let doc = CommitDoc {
            sha: "abc".into(),
            time: 5,
            lines,
            truncated: false,
            files,
        };
        let chunks = doc.chunks().expect("over the threshold");
        assert!(chunks.len() > 1);
        // Every line lands in exactly one chunk, in order.
        let joined: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(joined, doc.text());
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.name, format!("0000000005/abc#{i}"));
            assert!(c.paths.len() <= CHUNK_MAX_FILES + 1);
            assert_eq!(parse_doc_name(&c.name), Some((5, "abc")));
        }
        let renamed = chunks
            .iter()
            .find(|c| c.paths.contains(&"dir/file7.rs".to_string()))
            .unwrap();
        assert!(renamed.paths.contains(&"dir/old7.rs".to_string()));
        // Small commits stay whole.
        let small = CommitDoc {
            lines: doc.lines[..10].to_vec(),
            files: doc.files[..1].to_vec(),
            ..doc.clone()
        };
        assert!(small.chunks().is_none());
    }

    #[test]
    fn documents_are_capped() {
        let mut parser = DiffParser::default();
        for line in [
            "\x01abc 1",
            "",
            "diff --git a/a b/a",
            "--- a/a",
            "+++ b/a",
            "@@ -0,0 +1,9 @@",
        ] {
            assert!(parser.line(line.as_bytes()).is_none());
        }
        let line = format!("+{}", "x".repeat(1 << 20));
        for _ in 0..(DOC_CAP_BYTES >> 20) + 2 {
            parser.line(line.as_bytes());
        }
        let doc = parser.finish().unwrap();
        assert!(doc.truncated);
        assert!(doc.text().len() <= DOC_CAP_BYTES);
    }
}
