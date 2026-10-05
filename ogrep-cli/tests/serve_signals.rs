//! A server stopped by SIGTERM (`kill`, service managers) or SIGHUP removes
//! `serve.json` as Ctrl-C does, so later searches use the local index quietly.
#![cfg(unix)]

use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn ogrep_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("ogrep")
}

fn stop_with(signal: i32) {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.rs"), "fn needle() {}\n").unwrap();
    let idx = dir.path().join(".idx");
    let mut server = Command::new(ogrep_bin())
        .args([
            "serve",
            "--no-watch",
            "--index-path",
            idx.to_str().unwrap(),
            ".",
        ])
        .current_dir(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !idx.join("serve.json").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "server did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: kill(2) on our own child's PID.
    assert_eq!(unsafe { libc::kill(server.id() as i32, signal) }, 0);
    let status = server.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert!(!idx.join("serve.json").exists(), "serve.json left behind");

    let out = Command::new(ogrep_bin())
        .args(["--index-path", idx.to_str().unwrap(), "needle", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn sigterm_removes_serve_json() {
    stop_with(libc::SIGTERM);
}

#[test]
fn sighup_removes_serve_json() {
    stop_with(libc::SIGHUP);
}
