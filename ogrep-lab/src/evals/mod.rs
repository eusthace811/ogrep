//! Evaluations that run against any repository with an index, in-process and
//! without Python: how well `--rank` finds the file a change was about, how
//! long searches take, and whether `--history` agrees with `git log -G`.
//!
//! Each run is saved as one JSON file under `<index>/lab/runs/`, with the
//! ogrep version, repository HEAD and index it ran against, so runs from
//! different builds or days can be charted and compared.

mod history;
mod latency;
mod localize;
pub mod sample;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::workspace::Workspace;

/// How a suite's metrics read: for charts and for colouring differences.
pub struct Metric {
    pub key: &'static str,
    pub label: &'static str,
    /// `pct` (0..1 shown as percent), `ms`, `count`, `ratio`.
    pub unit: &'static str,
    pub higher_is_better: bool,
}

pub struct Suite {
    pub id: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    pub metrics: &'static [Metric],
    pub run: fn(&Workspace, &Value, &Progress) -> Result<Outcome>,
    /// Whether the suite can run here, with the reason when not.
    pub ready: fn(&Workspace) -> Result<(), String>,
}

pub struct Outcome {
    /// Variant name -> metric key -> value.
    pub summary: Map<String, Value>,
    pub items: Vec<Value>,
    /// Parameters as actually used, defaults filled in.
    pub params: Value,
}

pub const SUITES: &[Suite] = &[localize::SUITE, latency::SUITE, history::SUITE];

pub fn suite(id: &str) -> Option<&'static Suite> {
    SUITES.iter().find(|s| s.id == id)
}

pub fn catalogue(ws: &Workspace) -> Value {
    Value::Array(
        SUITES
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "title": s.title,
                    "about": s.about,
                    "ready": (s.ready)(ws).err(),
                    "metrics": s.metrics.iter().map(|m| json!({
                        "key": m.key, "label": m.label, "unit": m.unit,
                        "higher_is_better": m.higher_is_better,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// Progress of the running job, shared with the web client.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    phase: Mutex<String>,
    log: Mutex<Vec<String>>,
    cancel: AtomicBool,
}

impl Progress {
    pub fn phase(&self, phase: &str, total: usize) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = phase.to_string();
        self.total.store(total, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.log(phase);
    }

    pub fn step(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn log(&self, line: &str) {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        log.push(line.to_string());
        if log.len() > 400 {
            log.drain(..100);
        }
    }

    pub fn cancelled(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(anyhow!("cancelled"))
        } else {
            Ok(())
        }
    }
}

struct Job {
    id: String,
    suite: &'static str,
    started: Instant,
    progress: Arc<Progress>,
    /// `None` while running; the saved run id or the error when finished.
    result: Arc<Mutex<Option<Result<String, String>>>>,
    /// Seconds the job took, once finished.
    took: Arc<Mutex<Option<f64>>>,
}

/// One evaluation at a time: they time processes, and two at once would skew
/// each other.
#[derive(Default)]
pub struct Jobs {
    current: Mutex<Option<Job>>,
}

impl Jobs {
    pub fn start(
        &self,
        ws: Arc<Workspace>,
        suite_id: &str,
        params: Value,
    ) -> Result<Value, String> {
        let suite = suite(suite_id).ok_or_else(|| format!("unknown suite `{suite_id}`"))?;
        (suite.ready)(&ws)?;
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = current.as_ref()
            && job
                .result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none()
        {
            return Err(format!("{} is still running", job.suite));
        }
        let id = run_id(suite.id);
        let progress = Arc::new(Progress::default());
        let result = Arc::new(Mutex::new(None));
        let took = Arc::new(Mutex::new(None));
        let started = Instant::now();
        let job = Job {
            id: id.clone(),
            suite: suite.id,
            started,
            progress: progress.clone(),
            result: result.clone(),
            took: took.clone(),
        };
        std::thread::spawn(move || {
            let outcome = execute(&ws, suite, &id, &params, &progress);
            *took.lock().unwrap_or_else(|e| e.into_inner()) = Some(started.elapsed().as_secs_f64());
            *result.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(outcome.map_err(|e| e.to_string()));
        });
        let status = job_status(&job);
        *current = Some(job);
        Ok(status)
    }

    pub fn status(&self) -> Value {
        match self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            Some(job) => job_status(job),
            None => Value::Null,
        }
    }

    pub fn cancel(&self) {
        if let Some(job) = self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            job.progress.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn job_status(job: &Job) -> Value {
    let p = &job.progress;
    let result = job.result.lock().unwrap_or_else(|e| e.into_inner());
    let log = p.log.lock().unwrap_or_else(|e| e.into_inner());
    json!({
        "id": job.id,
        "suite": job.suite,
        "elapsed_s": job
            .took
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or_else(|| job.started.elapsed().as_secs_f64()),
        "phase": *p.phase.lock().unwrap_or_else(|e| e.into_inner()),
        "done": p.done.load(Ordering::Relaxed),
        "total": p.total.load(Ordering::Relaxed),
        "log": log.iter().rev().take(12).rev().collect::<Vec<_>>(),
        "running": result.is_none(),
        "run": result.as_ref().and_then(|r| r.as_ref().ok()),
        "error": result.as_ref().and_then(|r| r.as_ref().err()),
    })
}

fn execute(
    ws: &Workspace,
    suite: &Suite,
    id: &str,
    params: &Value,
    progress: &Progress,
) -> Result<String> {
    let started_at = now();
    let t = Instant::now();
    let env = environment(ws, params)?;
    let outcome = (suite.run)(ws, params, progress)?;
    let record = json!({
        "id": id,
        "suite": suite.id,
        "title": suite.title,
        "started_at": started_at,
        "duration_s": t.elapsed().as_secs_f64(),
        "params": outcome.params,
        "env": env,
        "summary": outcome.summary,
        "items": outcome.items,
    });
    let dir = ws.runs_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("{id}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(&record)?)?;
    std::fs::rename(&tmp, dir.join(format!("{id}.json")))?;
    progress.log("saved");
    Ok(id.to_string())
}

/// What a run ran against.
fn environment(ws: &Workspace, params: &Value) -> Result<Value> {
    let index = ws.index()?;
    let bin = ogrep_for(ws, params)?;
    let version = std::process::Command::new(&bin)
        .arg("--version")
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .trim_start_matches("ogrep ")
                .to_string()
        });
    Ok(json!({
        "ogrep": bin,
        "ogrep_version": version,
        "head": git_head(ws),
        "index_updated_at": index.meta.updated_at,
        "files": index.meta.num_files,
        "sparse_grams": index.meta.sparse_grams.is_some(),
        "lab_version": env!("CARGO_PKG_VERSION"),
    }))
}

/// The binary a run uses: `params.ogrep` when given (to compare builds), else
/// the lab's own.
pub fn ogrep_for(ws: &Workspace, params: &Value) -> Result<PathBuf> {
    match params["ogrep"].as_str().filter(|s| !s.trim().is_empty()) {
        Some(p) => {
            let p = PathBuf::from(p.trim());
            if p.is_file() {
                Ok(p)
            } else {
                Err(anyhow!("no ogrep binary at {}", p.display()))
            }
        }
        None => ws
            .ogrep
            .clone()
            .ok_or_else(|| anyhow!("the ogrep binary was not found; pass --ogrep to ogrep-lab")),
    }
}

pub fn git_head(ws: &Workspace) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&ws.root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `20260929-141502-localize`: sorts by time, readable in a file listing.
fn run_id(suite: &str) -> String {
    let secs = now() as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}-{suite}",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Days since 1970-01-01 to a UTC calendar date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Saved runs, newest first, without their per-item rows.
pub fn list_runs(ws: &Workspace) -> Value {
    let mut runs: Vec<Value> = std::fs::read_dir(ws.runs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let mut v: Value = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            let items = v.as_object_mut()?.remove("items");
            v["item_count"] = json!(items.and_then(|i| i.as_array().map(Vec::len)).unwrap_or(0));
            Some(v)
        })
        .collect();
    runs.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    Value::Array(runs)
}

fn run_path(ws: &Workspace, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("not a run id".into());
    }
    Ok(ws.runs_dir().join(format!("{id}.json")))
}

pub fn load_run(ws: &Workspace, id: &str) -> Result<Value, String> {
    let path = run_path(ws, id)?;
    let bytes = std::fs::read(&path).map_err(|_| format!("no run {id}"))?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

pub fn delete_run(ws: &Workspace, id: &str) -> Result<(), String> {
    std::fs::remove_file(run_path(ws, id)?).map_err(|_| format!("no run {id}"))
}

/// Median of a slice (sorted in place).
pub fn median(v: &mut [f64]) -> f64 {
    percentile(v, 0.5)
}

/// Linear-interpolated percentile, `p` in 0..=1 (sorts in place).
pub fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let k = (v.len() - 1) as f64 * p;
    let (lo, hi) = (k.floor() as usize, k.ceil() as usize);
    v[lo] + (v[hi] - v[lo]) * (k - lo as f64)
}

/// A worker pool for ogrep and git processes: a few at a time, so they do
/// not starve each other of the page cache or CPU.
pub fn pool(jobs: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .build()
        .expect("thread pool")
}

pub fn param_usize(params: &Value, key: &str, default: usize, max: usize) -> usize {
    params[key]
        .as_u64()
        .map(|v| v as usize)
        .or_else(|| params[key].as_str().and_then(|s| s.trim().parse().ok()))
        .unwrap_or(default)
        .clamp(1, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_725), (2026, 9, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn percentiles_interpolate() {
        let mut v = vec![4.0, 1.0, 3.0, 2.0];
        assert_eq!(median(&mut v), 2.5);
        assert_eq!(percentile(&mut v, 0.0), 1.0);
        assert_eq!(percentile(&mut v, 1.0), 4.0);
        assert_eq!(median(&mut []), 0.0);
    }

    #[test]
    fn run_ids_are_safe_file_names() {
        let id = run_id("latency");
        assert!(id.ends_with("-latency"));
        assert!(id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    }
}
