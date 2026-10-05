//! `ogrep ... | head`: a reader that closes the pipe early ends the search
//! quietly with status 0, as in ripgrep, with the local index and the server.
#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn ogrep_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("ogrep")
}

/// Far more matching output than a pipe buffers.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let line = "let needle_value = compute_needle(42);\n".repeat(200);
    for i in 0..200 {
        fs::write(dir.path().join(format!("f{i}.rs")), &line).unwrap();
    }
    dir
}

/// Read one line of `ogrep needle_value .`, close the pipe, and return the
/// exit status and stderr.
fn search_then_close(root: &Path, index: &Path) -> (Option<i32>, String) {
    let mut child = Command::new(ogrep_bin())
        .args(["--index-path", index.to_str().unwrap(), "needle_value", "."])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert!(first.contains("needle_value"), "{first}");
    let out = child.wait_with_output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn closed_pipe_ends_a_local_search_quietly() {
    let dir = fixture();
    let index = dir.path().join(".idx");
    let ok = Command::new(ogrep_bin())
        .args(["index", ".", "--index-path", index.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok);
    let (code, stderr) = search_then_close(dir.path(), &index);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert!(stderr.is_empty(), "stderr: {stderr}");
}

/// Kills the server even when an assertion fails first.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn closed_pipe_is_not_a_server_failure() {
    let dir = fixture();
    let index = dir.path().join(".idx");
    let _server = Server(
        Command::new(ogrep_bin())
            .args([
                "serve",
                "--no-watch",
                "--index-path",
                index.to_str().unwrap(),
                ".",
            ])
            .current_dir(dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    while !index.join("serve.json").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "server did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Wait until the server answers, so the search below is not a fallback.
    let start = Instant::now();
    loop {
        let out = Command::new(ogrep_bin())
            .args([
                "--index-path",
                index.to_str().unwrap(),
                "--stats",
                "-c",
                "compute_needle",
                ".",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        if String::from_utf8_lossy(&out.stderr).contains("via server") {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "server not ready"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let (code, stderr) = search_then_close(dir.path(), &index);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert!(
        !stderr.contains("falling back"),
        "a closed pipe must not send the search to the local index: {stderr}"
    );
    assert!(stderr.is_empty(), "stderr: {stderr}");
}
