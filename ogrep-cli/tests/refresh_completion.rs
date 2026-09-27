use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(root: &Path, index: &Path, mode: &str) -> Server {
    let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin("ogrep"));
    command
        .arg("serve")
        .arg(root)
        .arg("--index-path")
        .arg(index);
    match mode {
        "disabled" => {
            command.arg("--no-watch");
        }
        "poll" => {
            command.args(["--watch-mode", "poll", "--poll-interval", "120"]);
        }
        "native" => {}
        _ => unreachable!(),
    }
    let mut server = Server(
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited before discovery"
        );
        if let Ok(bytes) = std::fs::read(index.join("serve.json"))
            && let Ok(info) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && info["pid"] == server.0.id()
        {
            return server;
        }
        assert!(
            Instant::now() < deadline,
            "server did not publish discovery"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn refresh(root: &Path, index: &Path) -> serde_json::Value {
    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .arg("refresh")
        .arg(root)
        .arg("--index-path")
        .arg(index)
        .args(["--json", "--timeout-secs", "10"])
        .timeout(Duration::from_secs(12))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["status"], "refreshed");
    assert_eq!(
        value["root"],
        std::fs::canonicalize(root).unwrap().to_str().unwrap()
    );
    value
}

fn search(root: &Path, index: &Path, pattern: &str) {
    Command::cargo_bin("ogrep")
        .unwrap()
        .args(["-l", "--stats"])
        .arg("--index-path")
        .arg(index)
        .arg("--")
        .arg(pattern)
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("source.rs"))
        .stderr(predicate::str::contains("(via server)"));
}

#[test]
fn cli_receipt_precedes_searchable_edits_and_survives_server_restart() {
    for mode in ["disabled", "native", "poll"] {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("root");
        let index = tmp.path().join("index");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("source.rs"), "initial_marker\n").unwrap();
        let server = start(&root, &index, mode);
        refresh(&root, &index); // Also waits for an initially absent index.
        std::fs::write(root.join("source.rs"), "after_edit_marker\n").unwrap();
        refresh(&root, &index);
        search(&root, &index, "after_edit_marker");
        // A custom index pointing to a different root must fail before work.
        let wrong = tmp.path().join("wrong");
        std::fs::create_dir(&wrong).unwrap();
        Command::cargo_bin("ogrep")
            .unwrap()
            .arg("refresh")
            .arg(&wrong)
            .arg("--index-path")
            .arg(&index)
            .assert()
            .code(2)
            .stdout("")
            .stderr(predicate::str::contains("root does not match"));
        drop(server);
        Command::cargo_bin("ogrep")
            .unwrap()
            .arg("refresh")
            .arg(&root)
            .arg("--index-path")
            .arg(&index)
            .assert()
            .code(2)
            .stdout("")
            .stderr(predicate::str::contains("falling back").not());
        std::fs::write(root.join("source.rs"), "after_restart_marker\n").unwrap();
        let _restarted = start(&root, &index, mode);
        refresh(&root, &index);
        search(&root, &index, "after_restart_marker");
    }
}

#[test]
fn refresh_rejects_invalid_deadlines_and_requires_a_running_server() {
    for timeout in ["0", "301", "-1", "invalid"] {
        Command::cargo_bin("ogrep")
            .unwrap()
            .args(["refresh", "--timeout-secs", timeout])
            .assert()
            .code(2)
            .stdout("");
    }
    let tmp = tempfile::TempDir::new().unwrap();
    Command::cargo_bin("ogrep")
        .unwrap()
        .arg("refresh")
        .arg(tmp.path())
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("requires a running server"));
}

fn fake_server(root: &Path, reply: Option<serde_json::Value>) -> std::thread::JoinHandle<()> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let index = root.join(".ogrep");
    std::fs::create_dir_all(&index).unwrap();
    std::fs::write(
        index.join("serve.json"),
        serde_json::json!({"pid":std::process::id(),"port":listener.local_addr().unwrap().port()})
            .to_string(),
    )
    .unwrap();
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && started.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("client did not connect: {error}"),
            }
        };
        // Windows hands out accepted sockets non-blocking like their listener,
        // which makes the read below fail with WouldBlock instead of waiting.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "refresh");
        assert!(request["params"]["root"].as_str().is_some());
        assert!(request["params"]["timeout_ms"].as_u64().unwrap() <= 1000);
        if let Some(reply) = reply {
            writeln!(stream, "{reply}").unwrap();
        } else {
            // Continual partial input must not renew the total deadline.
            for _ in 0..20 {
                if stream.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    })
}

#[test]
fn cli_rejects_errors_invalid_receipts_and_disconnected_servers() {
    let tmp = tempfile::TempDir::new().unwrap();
    for response in [
        serde_json::json!({"jsonrpc":"2.0", "id":1,"error":{"code":-32601,"message":"Method not found"}}),
        serde_json::json!({"jsonrpc":"2.0", "id":1,"error":{"code":-32003,"message":"Incomplete"}}),
        serde_json::json!({"jsonrpc":"2.0", "id":1,"error":{"code":-32004,"message":"Timeout"}}),
        serde_json::json!({"jsonrpc":"2.0", "id":null,"error":{"code":-32002,"message":"Busy"}}),
        serde_json::json!({"jsonrpc":"2.0", "id":1,"result":{"status":"reloaded"}}),
        serde_json::json!({"jsonrpc":"2.0", "id":1,"result":{"status":"refreshed","root":"/wrong"}}),
        serde_json::json!({"jsonrpc":"2.0", "id":2,"result":{"status":"refreshed"}}),
    ] {
        let peer = fake_server(tmp.path(), Some(response));
        Command::cargo_bin("ogrep")
            .unwrap()
            .arg("refresh")
            .arg(tmp.path())
            .args(["--json", "--timeout-secs", "1"])
            .timeout(Duration::from_secs(3))
            .assert()
            .code(2)
            .stdout("")
            .stderr(predicate::str::contains("falling back").not());
        peer.join().unwrap();
    }
}

#[test]
fn trickling_reply_cannot_extend_client_deadline() {
    let tmp = tempfile::TempDir::new().unwrap();
    let peer = fake_server(tmp.path(), None);
    let started = Instant::now();
    Command::cargo_bin("ogrep")
        .unwrap()
        .arg("refresh")
        .arg(tmp.path())
        .args(["--timeout-secs", "1"])
        .timeout(Duration::from_secs(3))
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(2));
    peer.join().unwrap();
}
