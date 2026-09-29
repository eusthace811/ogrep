//! Running `ogrep` itself. Search, ranking, history and the graph queries go
//! through the real binary with `--json`, so the lab shows exactly what the
//! command line would print and needs no copy of that logic.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::workspace::Workspace;

/// What one ogrep run printed.
pub struct Run {
    pub lines: Vec<String>,
    /// Stopped at the line limit; the rest was not read.
    pub truncated: bool,
    pub elapsed: Duration,
    /// Standard error, for notes such as a stale graph.
    pub notes: String,
}

/// Run `ogrep --index-path IDX --json ARGS` in the repository root with the
/// lab's own binary, and parse its JSON lines, reading at most `limit`.
pub fn json(ws: &Workspace, args: &[String], limit: usize) -> Result<(Vec<Value>, Run), String> {
    let bin = ws
        .ogrep
        .as_deref()
        .ok_or("the ogrep binary was not found next to ogrep-lab or on PATH; pass --ogrep")?;
    let mut all = vec!["--json".to_string()];
    all.extend_from_slice(args);
    let run = run(bin, ws, &all, limit, Duration::from_secs(120))?;
    let records = run
        .lines
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    Ok((records, run))
}

/// Wall time of one ogrep invocation with its output discarded, as a caller
/// that reads the whole output would see it.
pub fn timed(bin: &Path, ws: &Workspace, args: &[String]) -> Result<Duration, String> {
    let start = Instant::now();
    let status = base(bin, ws)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
    let elapsed = start.elapsed();
    if status.code().is_some_and(|c| c >= 2) {
        return Err(format!("ogrep exited with {status}"));
    }
    Ok(elapsed)
}

fn base(bin: &Path, ws: &Workspace) -> Command {
    let mut cmd = Command::new(bin);
    cmd.current_dir(&ws.root)
        .arg("--index-path")
        .arg(&ws.index_dir)
        .arg("--color=never")
        .stdin(Stdio::null());
    cmd
}

/// Run `bin` with `args`, reading at most `limit` lines of its output.
///
/// Exit status 1 means no match, as with grep; only 2 and above are errors.
pub fn run(
    bin: &Path,
    ws: &Workspace,
    args: &[String],
    limit: usize,
    timeout: Duration,
) -> Result<Run, String> {
    let start = Instant::now();
    let mut child = base(bin, ws)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.by_ref().take(64 * 1024).read_to_string(&mut s);
        // Keep draining past the cap, or a chatty child would block on a
        // full pipe and never finish its standard output.
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        s
    });
    let deadline = start + timeout;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = Vec::new();
        let mut truncated = false;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if lines.len() >= limit {
                truncated = true;
                break;
            }
            lines.push(line);
        }
        let _ = tx.send((lines, truncated));
    });
    let (lines, truncated) =
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(v) => v,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "ogrep took longer than {}s and was stopped",
                    timeout.as_secs()
                ));
            }
        };
    if truncated {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let elapsed = start.elapsed();
    let notes = err_reader.join().unwrap_or_default();
    if !truncated && status.code().is_none_or(|c| c >= 2) {
        let msg = notes.trim().trim_start_matches("ogrep: ");
        return Err(if msg.is_empty() {
            format!("ogrep failed ({status})")
        } else {
            msg.to_string()
        });
    }
    Ok(Run {
        lines,
        truncated,
        elapsed,
        notes: notes.trim().to_string(),
    })
}
