use super::*;
use tempfile::TempDir;

fn removal(path: PathBuf) -> Event {
    Event::new(EventKind::Remove(notify::event::RemoveKind::Any)).add_path(path)
}

#[test]
fn delayed_removal_keeps_recreated_published_file_searchable() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let path = root.join("source.rs");
    let state = tests::test_server_state(root, &root.join(".ogrep"));
    state.gitignore_pending.store(false, Ordering::SeqCst);
    std::fs::write(&path, "original_marker\n").unwrap();
    reindex_file(&state, &path, "source.rs", true);
    std::fs::remove_file(&path).unwrap();
    let delayed = removal(path.clone());
    std::fs::write(&path, "replacement_marker\n").unwrap();
    assert!(background_refresh_stale(
        &state,
        root,
        &state.index_dir,
        true
    ));

    handle_fs_event(&state, root, &delayed);
    let response: serde_json::Value = serde_json::from_str(&handle_search(
        None,
        &serde_json::json!({"pattern": "replacement_marker"}),
        &state,
    ))
    .unwrap();
    assert_eq!(response["result"]["num_matches"], 1, "{response}");
}

fn recreated_subtree(route: &str) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let dir = root.join("source");
    let state = tests::test_server_state(root, &root.join(".ogrep"));
    state.gitignore_pending.store(false, Ordering::SeqCst);
    // Use real registrations with a discarded callback: this test controls
    // exactly which notifications arrive and cannot be healed by a later
    // Create event from the host filesystem.
    let watcher = notify::recommended_watcher(|_: notify::Result<Event>| {}).unwrap();
    *state.watch_registry.lock().unwrap() = Some(WatchRegistry {
        watcher,
        root: root.to_path_buf(),
        watched: Default::default(),
        budget: 8192,
        failure: None,
        fail_after: None,
        polling: Arc::clone(&state.refresh.polling),
    });
    std::fs::create_dir(&dir).unwrap();
    {
        let _gate = state.snapshot_gate.read().unwrap();
        watch_new_subtree(&state, root, &dir);
    }
    std::fs::remove_dir(&dir).unwrap();
    let event = removal(dir.clone());
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::write(dir.join("nested/new.rs"), "recreated_subtree_marker\n").unwrap();

    match route {
        "direct" => handle_fs_event(&state, root, &event),
        "burst" => {
            for path in FsEventBurst::with_limits(event, 10, 10).into_paths() {
                handle_fs_event(&state, root, &path.into_event());
            }
        }
        "deferred" => {
            state.indexing.store(true, Ordering::SeqCst);
            assert!(defer_events_during_build(&state, &event));
            state.indexing.store(false, Ordering::SeqCst);
            replay_deferred_events(&state, root);
        }
        _ => unreachable!(),
    }
    let response: serde_json::Value = serde_json::from_str(&handle_search(
        None,
        &serde_json::json!({"pattern": "recreated_subtree_marker"}),
        &state,
    ))
    .unwrap();
    assert_eq!(response["result"]["num_matches"], 1, "{route}: {response}");
    if PER_DIRECTORY_WATCHES {
        let registry = state.watch_registry.lock().unwrap();
        assert!(registry.as_ref().unwrap().is_watched(&dir.join("nested")));
    }
}

#[test]
fn delayed_directory_removal_recovers_direct() {
    recreated_subtree("direct");
}

#[test]
fn delayed_directory_removal_recovers_burst() {
    recreated_subtree("burst");
}

#[test]
fn delayed_directory_removal_recovers_deferred() {
    recreated_subtree("deferred");
}
