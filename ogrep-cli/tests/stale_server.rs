//! A server that was killed leaves `serve.json` behind. Clients must not
//! connect to it once its process is gone: on Windows a refused localhost
//! connection takes about two seconds, which every search would pay.

use assert_cmd::Command;
use std::fs;
use std::net::TcpListener;
use std::time::Duration;
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

#[test]
fn a_dead_servers_serve_json_is_not_contacted() {
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

    // A listener that would answer if the client connected to it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    fs::write(
        index.join("serve.json"),
        serde_json::json!({"pid": dead_pid(), "port": port}).to_string(),
    )
    .unwrap();

    let output = ogrep()
        .current_dir(&root)
        .args(["--no-config", "--stats", "-l", "--index-path"])
        .arg(&index)
        .args(["--", "stale_server_marker", "."])
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("a.txt"), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("via server"), "{stderr}");
    assert!(
        listener.accept().is_err(),
        "the client connected to a server whose process is gone"
    );

    // refresh needs a live server, and says so without connecting.
    ogrep()
        .args(["refresh", root.to_str().unwrap(), "--index-path"])
        .arg(&index)
        .timeout(Duration::from_secs(30))
        .assert()
        .code(2);
    assert!(listener.accept().is_err());
}
