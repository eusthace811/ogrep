//! A server stopped by SIGTERM (`kill`, service managers) or SIGHUP removes
//! `serve.json` as Ctrl-C does, so later searches use the local index quietly;
//! a signal ignored at start (`nohup`) stays ignored.
#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn ogrep_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("ogrep")
}

/// A server on `dir`, started with `ignored` set to be ignored (as `nohup`
/// does for SIGHUP), once it has written `serve.json`.
fn start(dir: &Path, idx: &Path, ignored: Option<i32>) -> Child {
    let mut cmd = Command::new(ogrep_bin());
    cmd.args([
        "serve",
        "--no-watch",
        "--index-path",
        idx.to_str().unwrap(),
        ".",
    ])
    .current_dir(dir)
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    if let Some(sig) = ignored {
        // SAFETY: signal(2) is async-signal-safe, so it may run between fork
        // and exec; an ignored disposition survives exec.
        unsafe {
            cmd.pre_exec(move || {
                libc::signal(sig, libc::SIG_IGN);
                Ok(())
            });
        }
    }
    let server = cmd.spawn().unwrap();
    let start = Instant::now();
    while !idx.join("serve.json").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "server did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    server
}

fn stop_with(signal: i32) {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.rs"), "fn needle() {}\n").unwrap();
    let idx = dir.path().join(".idx");
    let mut server = start(dir.path(), &idx, None);
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

#[test]
fn an_ignored_sighup_stays_ignored() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.rs"), "fn needle() {}\n").unwrap();
    let idx = dir.path().join(".idx");
    let mut server = start(dir.path(), &idx, Some(libc::SIGHUP));
    // SAFETY: kill(2) on our own child's PID.
    assert_eq!(unsafe { libc::kill(server.id() as i32, libc::SIGHUP) }, 0);
    std::thread::sleep(Duration::from_millis(300));
    let survived = server.try_wait().unwrap().is_none() && idx.join("serve.json").exists();
    // Stop it before asserting: a server that ignores SIGHUP would outlive a
    // failed test otherwise. SAFETY: as above.
    unsafe { libc::kill(server.id() as i32, libc::SIGTERM) };
    let status = server.wait().unwrap();
    assert!(survived, "nohup'd server exited");
    assert!(status.success(), "{status:?}");
    assert!(!idx.join("serve.json").exists(), "serve.json left behind");
}
