//! `serve.json` records the server's PID. A killed server leaves the file
//! behind, and on Windows a refused localhost connection takes about two
//! seconds, so a client whose recorded PID names no running process waits
//! only briefly. It still tries, because a server in another PID namespace (a
//! Windows server seen from WSL, a container) looks exactly the same.

use assert_cmd::Command;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn ogrep() -> Command {
    Command::cargo_bin("ogrep").unwrap()
}

/// The PID of a process that has already exited.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("ogrep"))
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn indexed_repo() -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    let index = temp.path().join("index");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("a.txt"), "stale_server_marker\n").unwrap();
    ogrep()
        .args(["index", root.to_str().unwrap(), "--index-path"])
        .arg(&index)
        .assert()
        .success();
    (temp, root, index)
}

fn write_serve_json(index: &Path, port: u16) {
    fs::write(
        index.join("serve.json"),
        serde_json::json!({"pid": dead_pid(), "port": port}).to_string(),
    )
    .unwrap();
}

fn search(root: &Path, index: &Path) -> std::process::Output {
    ogrep()
        .current_dir(root)
        .args(["--no-config", "--stats", "-l", "--index-path"])
        .arg(index)
        .args(["--", "stale_server_marker", "."])
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap()
}

#[test]
fn a_killed_servers_serve_json_costs_little() {
    let (_temp, root, index) = indexed_repo();
    // A port nothing listens on any more.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    write_serve_json(&index, port);

    let started = Instant::now();
    let output = search(&root, &index);
    let elapsed = started.elapsed();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("a.txt"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("via server"));
    assert!(
        elapsed < Duration::from_secs(1),
        "a stale serve.json cost {elapsed:?}"
    );

    // refresh needs a live server, and says so just as quickly.
    let started = Instant::now();
    ogrep()
        .args(["refresh", root.to_str().unwrap(), "--index-path"])
        .arg(&index)
        .timeout(Duration::from_secs(30))
        .assert()
        .code(2);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_server_whose_pid_is_not_visible_here_is_still_used() {
    let (_temp, root, index) = indexed_repo();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    write_serve_json(&index, listener.local_addr().unwrap().port());
    // Answer like a live server that is busy, which the client reports
    // instead of falling back: proof that it connected.
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let started = Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        started.elapsed() < Duration::from_secs(10),
                        "the client never contacted the server"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("accept failed: {e}"),
            }
        };
        // Windows hands out accepted sockets non-blocking like their listener.
        stream.set_nonblocking(false).unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        writeln!(
            &stream,
            "{}",
            serde_json::json!({"jsonrpc": "2.0", "id": request["id"],
                "error": {"code": -32002, "message": "server search queue is full"}})
        )
        .unwrap();
    });

    let output = search(&root, &index);
    server
        .join()
        .expect("the client never contacted the server");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("server busy"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
