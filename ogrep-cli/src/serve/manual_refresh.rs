//! Explicit publication barriers. A later caller never joins an already
//! started pass: its edit may be behind that pass's traversal cursor.
use super::*;
use std::sync::Condvar;

const INCOMPLETE: i32 = -32003;
const TIMEOUT: i32 = -32004;

#[derive(Clone, Serialize)]
struct Receipt {
    status: &'static str,
    root: String,
    elapsed_ms: u128,
    num_files: usize,
}

#[derive(Default)]
struct Job {
    result: Mutex<Option<std::result::Result<Receipt, String>>>,
    finished: Condvar,
}

impl Job {
    fn finish(&self, result: std::result::Result<Receipt, String>) {
        *self.result.lock().unwrap() = Some(result);
        self.finished.notify_all();
    }
}

#[derive(Default)]
struct Queue {
    running: bool,
    pending: Option<Arc<Job>>,
}

#[derive(Default)]
pub(super) struct Control {
    queue: Mutex<Queue>,
}

impl Control {
    pub(super) fn status(&self) -> serde_json::Value {
        let queue = self.queue.lock().unwrap();
        serde_json::json!({"running": queue.running, "pending": queue.pending.is_some()})
    }

    fn enqueue(state: &Arc<ServerState>) -> Arc<Job> {
        let mut queue = state.manual_refresh.queue.lock().unwrap();
        let job = Arc::clone(
            queue
                .pending
                .get_or_insert_with(|| Arc::new(Job::default())),
        );
        if !queue.running {
            queue.running = true;
            let worker_state = Arc::clone(state);
            if let Err(error) = thread::Builder::new()
                .name("ogrep-manual-refresh".into())
                .spawn(move || worker(worker_state))
            {
                queue.running = false;
                queue
                    .pending
                    .take()
                    .unwrap()
                    .finish(Err(format!("could not start refresh worker: {error}")));
            }
        }
        job
    }
}

fn worker(state: Arc<ServerState>) {
    loop {
        let job = {
            let mut queue = state.manual_refresh.queue.lock().unwrap();
            match queue.pending.take() {
                Some(job) => job,
                None => {
                    queue.running = false;
                    return;
                }
            }
        };
        // Keep waiters informed even if an internal invariant panics. A poisoned
        // index lock is not healed or reported as a successful refresh.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reconcile(&state)))
            .unwrap_or_else(|_| Err("refresh worker panicked; restart the server".into()));
        job.finish(result);
    }
}

fn reconcile(state: &Arc<ServerState>) -> std::result::Result<Receipt, String> {
    let started = Instant::now();
    // Initial construction does not hold snapshot_gate throughout its build.
    // Recheck after acquiring both mutation locks to avoid overtaking a reload.
    let (refresh, gate) = loop {
        while state.indexing.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(10));
        }
        let refresh = state.stale_refresh_lock.lock().unwrap();
        let gate = state.snapshot_gate.write().unwrap();
        if !state.indexing.load(Ordering::SeqCst) {
            break (refresh, gate);
        }
        drop(gate);
        drop(refresh);
    };
    let root = &state.root;
    let ignorecase = frozen_tracked_membership(state, root);
    #[cfg(test)]
    run_stale_refresh_hook(state, StaleRefreshPhase::BeforeWalk);
    let since = SystemTime::now();
    let mut newly_watched = Vec::new();
    let published = refresh_stale_locked(
        state,
        root,
        &state.index_dir,
        ReconcileScope::Content,
        &mut newly_watched,
        ignorecase,
        true,
    );
    if published {
        reindex_files_in(state, root, &newly_watched, since);
    }
    let membership_changed = tracked_membership_changed(state);
    let complete = published
        && !membership_changed
        && state.unreadable.read().unwrap().is_empty()
        && state.filename_index_ready.load(Ordering::SeqCst)
        && !state.filename_index_dirty.load(Ordering::SeqCst)
        && state.hidden_complete.load(Ordering::SeqCst);
    let result = if complete {
        Ok(Receipt {
            status: "refreshed",
            root: root.to_string_lossy().into_owned(),
            elapsed_ms: started.elapsed().as_millis(),
            num_files: state.index.read().unwrap().num_files(),
        })
    } else {
        Err("refresh incomplete: traversal, reads, coverage or publication failed, or tracked membership changed; see server logs and retry".into())
    };
    drop(gate);
    drop(refresh);
    schedule_tracked_membership_correction(state, root, membership_changed);
    result
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    /// Bind discovery to a repository; a stale port must not acknowledge a
    /// different server's tree. Callers supply the canonical absolute path.
    root: Option<PathBuf>,
}

fn default_timeout() -> u64 {
    30_000
}

pub(super) fn handle(
    id: Option<serde_json::Value>,
    params: &serde_json::Value,
    state: &Arc<ServerState>,
) -> String {
    let params = if params.is_null() {
        Params {
            timeout_ms: default_timeout(),
            root: None,
        }
    } else if !params.is_object() {
        return json_rpc_error(id, -32602, "refresh params must be an object");
    } else {
        match serde_json::from_value::<Params>(params.clone()) {
            Ok(params) => params,
            Err(error) => return json_rpc_error(id, -32602, &error.to_string()),
        }
    };
    if !(1..=300_000).contains(&params.timeout_ms) {
        return json_rpc_error(id, -32602, "timeout_ms must be between 1 and 300000");
    }
    if params.root.as_ref().is_some_and(|root| root != &state.root) {
        return json_rpc_error(id, -32602, "refresh root does not match this server");
    }
    let deadline = Instant::now() + Duration::from_millis(params.timeout_ms);
    let job = Control::enqueue(state);
    let mut result = job.result.lock().unwrap();
    loop {
        if Instant::now() >= deadline {
            return json_rpc_error(
                id,
                TIMEOUT,
                "refresh wait timed out; the pass may still complete; retry before relying on indexed freshness",
            );
        }
        if let Some(result) = result.as_ref() {
            return match result {
                Ok(receipt) => json_rpc_result(id, serde_json::to_value(receipt).unwrap()),
                Err(error) => json_rpc_error(id, INCOMPLETE, error),
            };
        }
        result = job
            .finished
            .wait_timeout(result, deadline.saturating_duration_since(Instant::now()))
            .unwrap()
            .0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Arc<ServerState>) {
        let tmp = TempDir::new().unwrap();
        let mut state =
            super::super::tests::test_server_state(tmp.path(), &tmp.path().join(".ogrep"));
        let unique = Arc::get_mut(&mut state).unwrap();
        unique.watch_enabled = false;
        unique.no_require_git = true;
        (tmp, state)
    }

    fn rpc(state: &Arc<ServerState>, timeout: u64) -> serde_json::Value {
        let (response, permit) = process_request(
            &serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "refresh", "params": {"timeout_ms": timeout}}).to_string(),
            state,
        );
        assert!(permit.is_none());
        let value: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["id"], 7);
        value
    }

    fn refresh(state: &Arc<ServerState>) {
        let value = rpc(state, 5000);
        assert_eq!(value["result"]["status"], "refreshed", "{value}");
    }

    fn hit(state: &Arc<ServerState>, pattern: &str, expected: u64) {
        let value: serde_json::Value = serde_json::from_str(&handle_search(
            None,
            &serde_json::json!({"pattern": pattern}),
            state,
        ))
        .unwrap();
        assert_eq!(value["result"]["num_matches"], expected, "{value}");
    }

    fn wait(job: &Job) -> std::result::Result<Receipt, String> {
        let (result, _) = job
            .finished
            .wait_timeout_while(
                job.result.lock().unwrap(),
                Duration::from_secs(5),
                |result| result.is_none(),
            )
            .unwrap();
        result.clone().expect("refresh worker must settle")
    }

    #[test]
    fn refresh_publishes_completed_edits_and_membership_in_every_mode() {
        for mode in ["disabled", "native", "poll"] {
            let (tmp, mut state) = fixture();
            let unique = Arc::get_mut(&mut state).unwrap();
            unique.watch_enabled = mode != "disabled";
            unique.refresh = RefreshControl::new(
                if mode == "poll" {
                    WatchMode::Poll
                } else {
                    WatchMode::Auto
                },
                Duration::from_secs(120),
                8192,
            );
            let root = tmp.path();
            std::fs::write(root.join("old.rs"), "original_marker\n").unwrap();
            std::fs::write(root.join("deleted.rs"), "deleted_marker\n").unwrap();
            refresh(&state);
            std::fs::rename(root.join("old.rs"), root.join("renamed.rs")).unwrap();
            std::fs::write(root.join("renamed.rs"), "replacement_marker\n").unwrap();
            std::fs::remove_file(root.join("deleted.rs")).unwrap();
            std::fs::write(root.join("new.rs"), "created_marker\n").unwrap();
            std::fs::write(root.join("excluded.rs"), "excluded_marker\n").unwrap();
            std::fs::write(root.join(".ignore"), "excluded.rs\n").unwrap();
            std::fs::write(root.join("asset.bin"), [0, 1, 2]).unwrap();
            refresh(&state);
            hit(&state, "replacement_marker", 1);
            hit(&state, "created_marker", 1);
            for absent in ["original_marker", "deleted_marker", "excluded_marker"] {
                hit(&state, absent, 0);
            }
            let value: serde_json::Value =
                serde_json::from_str(&handle_files(None, &serde_json::Value::Null, &state))
                    .unwrap();
            assert_eq!(
                value["result"]["files"],
                serde_json::json!(["asset.bin", "new.rs", "renamed.rs"]),
                "{mode}: {value}"
            );
        }
    }

    #[test]
    fn refresh_rereads_even_matching_evidence_and_memoized_failures() {
        let (tmp, state) = fixture();
        let path = tmp.path().join("source.rs");
        std::fs::write(&path, "original_marker\n").unwrap();
        refresh(&state);
        let old_time = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "replaced_marker\n").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        // Model a filesystem whose observed evidence cannot distinguish the
        // two contents. The published postings deliberately remain original.
        let version = builder::file_version(&std::fs::metadata(&path).unwrap());
        state.file_evidence.write().unwrap().insert_verified(
            "source.rs".into(),
            version.stamp().clone(),
            None,
            Some(version.clone()),
        );
        state
            .unreadable
            .write()
            .unwrap()
            .insert("source.rs".into(), Some(version));
        refresh(&state);
        hit(&state, "replaced_marker", 1);
        hit(&state, "original_marker", 0);
        assert!(state.unreadable.read().unwrap().is_empty());
    }

    #[test]
    fn refresh_read_failure_is_an_error_and_retry_can_succeed() {
        let (tmp, state) = fixture();
        let path = tmp.path().join("source.rs");
        std::fs::write(&path, "original_marker\n").unwrap();
        refresh(&state);
        let vanished = path.clone();
        *state.stale_refresh_hook.lock().unwrap() = Some(Arc::new(move |phase| {
            if matches!(phase, StaleRefreshPhase::AfterMatcherPublish) {
                std::fs::remove_file(&vanished).unwrap();
            }
        }));
        let value = rpc(&state, 5000);
        assert_eq!(value["error"]["code"], INCOMPLETE, "{value}");
        *state.stale_refresh_hook.lock().unwrap() = None;
        std::fs::write(path, "recovered_marker\n").unwrap();
        refresh(&state);
        hit(&state, "recovered_marker", 1);
    }

    #[test]
    fn failed_publication_cannot_return_a_refresh_receipt() {
        let (tmp, state) = fixture();
        let path = tmp.path().join("source.rs");
        std::fs::write(&path, "original_marker\n").unwrap();
        refresh(&state);
        let evidence = state.file_evidence.read().unwrap().clone();
        // Fail the atomic publisher's backup-directory creation after the
        // content delta has been built, without depending on user permissions.
        let retired = state.index_dir.join(".retired");
        std::fs::remove_dir_all(&retired).unwrap();
        std::fs::write(&retired, "block publication").unwrap();
        std::fs::write(&path, "replacement_marker\n").unwrap();
        let value = rpc(&state, 5000);
        assert_eq!(value["error"]["code"], INCOMPLETE, "{value}");
        assert_eq!(state.file_evidence.read().unwrap().stamps, evidence.stamps);
        hit(&state, "replacement_marker", 0);
        std::fs::remove_file(retired).unwrap();
        refresh(&state);
        hit(&state, "replacement_marker", 1);
    }

    #[test]
    fn refresh_can_publish_an_empty_tree_after_deletion() {
        let (tmp, state) = fixture();
        let path = tmp.path().join("source.rs");
        std::fs::write(&path, "original_marker\n").unwrap();
        refresh(&state);
        std::fs::remove_file(path).unwrap();
        refresh(&state);
        hit(&state, "original_marker", 0);
        let value: serde_json::Value =
            serde_json::from_str(&handle_files(None, &serde_json::Value::Null, &state)).unwrap();
        assert_eq!(value["result"]["files"], serde_json::json!([]));
        refresh(&state);
    }

    #[test]
    fn calls_during_a_walk_join_one_later_pass_and_status_stays_available() {
        let (tmp, state) = fixture();
        std::fs::write(tmp.path().join("first.rs"), "first_marker\n").unwrap();
        let (arrived, started) = std::sync::mpsc::sync_channel(1);
        let (release, released) = std::sync::mpsc::sync_channel(1);
        let released = Mutex::new(released);
        let passes = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&passes);
        *state.stale_refresh_hook.lock().unwrap() = Some(Arc::new(move |phase| {
            if matches!(phase, StaleRefreshPhase::AfterMatcherPublish)
                && count.fetch_add(1, Ordering::SeqCst) == 0
            {
                arrived.send(()).unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
        }));
        let first = Control::enqueue(&state);
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        std::fs::write(tmp.path().join("later.rs"), "later_marker\n").unwrap();
        let next = Control::enqueue(&state);
        let shared = Control::enqueue(&state);
        assert!(!Arc::ptr_eq(&first, &next));
        assert!(Arc::ptr_eq(&next, &shared));
        let value: serde_json::Value = serde_json::from_str(&handle_status(None, &state)).unwrap();
        assert_eq!(
            value["result"]["manual_refresh"],
            serde_json::json!({"running": true, "pending": true})
        );
        release.send(()).unwrap();
        assert_eq!(wait(&first).unwrap().num_files, 1);
        assert_eq!(wait(&next).unwrap().num_files, 2);
        assert_eq!(passes.load(Ordering::SeqCst), 2);
        hit(&state, "later_marker", 1);
    }

    #[test]
    fn timeout_covers_initial_build_wait_and_does_not_acknowledge_success() {
        let (tmp, state) = fixture();
        std::fs::write(tmp.path().join("source.rs"), "after_build_marker\n").unwrap();
        state.indexing.store(true, Ordering::SeqCst);
        let value = rpc(&state, 10);
        assert_eq!(value["error"]["code"], TIMEOUT, "{value}");
        state.indexing.store(false, Ordering::SeqCst);
        refresh(&state);
        hit(&state, "after_build_marker", 1);
    }

    #[test]
    fn invalid_refresh_parameters_do_not_schedule_work() {
        let (_tmp, state) = fixture();
        for params in [
            serde_json::json!({"timeout_ms": 0}),
            serde_json::json!({"timeout_ms": 300001}),
            serde_json::json!({"timeout_ms": -1}),
            serde_json::json!({"timeout_ms": "100"}),
            serde_json::json!({"typo": 1}),
            serde_json::json!([]),
            serde_json::json!({"root": "/not/the/served/root"}),
        ] {
            let value: serde_json::Value =
                serde_json::from_str(&handle(None, &params, &state)).unwrap();
            assert_eq!(value["error"]["code"], -32602, "{value}");
        }
        assert_eq!(
            state.manual_refresh.status(),
            serde_json::json!({"running": false, "pending": false})
        );
    }

    #[test]
    fn refresh_bypasses_occupied_search_slots() {
        let (tmp, state) = fixture();
        std::fs::write(tmp.path().join("source.rs"), "control_marker\n").unwrap();
        let _held: Vec<_> = (0..4)
            .map(|_| state.searches.acquire(Duration::ZERO).unwrap())
            .collect();
        refresh(&state);
        hit(&state, "control_marker", 1);
    }
}
