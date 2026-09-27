use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

#[test]
fn explicit_overload_is_an_error_without_local_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let index = tmp.path().join("index");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), "admission_marker\n").unwrap();
    Command::cargo_bin("ogrep")
        .unwrap()
        .arg("index")
        .arg(&root)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();
    for files in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        std::fs::write(
            index.join("serve.json"),
            serde_json::json!({
                "pid": std::process::id(), "port": listener.local_addr().unwrap().port(),
            })
            .to_string(),
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            let start = Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(5) =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("client did not connect: {e}"),
                }
            };
            // Windows hands out accepted sockets non-blocking like their listener,
            // which makes the read below fail with WouldBlock instead of waiting.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["method"], if files { "files" } else { "search" });
            writeln!(
                stream,
                "{}",
                serde_json::json!({
                    "jsonrpc":"2.0", "id": request["id"],
                    "error": {"code": -32002, "message":"server search queue is full"},
                })
            )
            .unwrap();
        });
        let mut command = Command::cargo_bin("ogrep").unwrap();
        command.arg("--index-path").arg(&index);
        if files {
            command.arg("--files").arg(&root);
        } else {
            command.args(["-l", "--", "admission_marker"]).arg(&root);
        }
        command
            .timeout(Duration::from_secs(10))
            .assert()
            .code(2)
            .stdout("")
            .stderr(predicate::str::contains("server busy"))
            .stderr(predicate::str::contains("falling back").not());
        server.join().unwrap();
    }
}

#[test]
fn rejects_invalid_search_concurrency() {
    for value in ["0", "33", "-1"] {
        Command::cargo_bin("ogrep")
            .unwrap()
            .args(["serve", "--search-concurrency", value])
            .assert()
            .code(2);
    }
}

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn status(stream: &mut std::net::TcpStream) -> serde_json::Value {
    writeln!(stream, r#"{{"method":"status","id":1}}"#).unwrap();
    let mut line = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn bounds_real_connections_and_reuses_slots_after_disconnect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let index = tmp.path().join("index");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), "marker\n").unwrap();
    let mut server = Server(
        std::process::Command::new(assert_cmd::cargo::cargo_bin("ogrep"))
            .arg("serve")
            .arg(&root)
            .arg("--index-path")
            .arg(&index)
            .args(["--no-watch", "--search-concurrency", "2"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    let port = loop {
        assert!(server.0.try_wait().unwrap().is_none());
        assert!(start.elapsed() < Duration::from_secs(10));
        if let Ok(bytes) = std::fs::read(index.join("serve.json"))
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        {
            break value["port"].as_u64().unwrap() as u16;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let connect = || {
        let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
    };
    let mut clients = Vec::new();
    for _ in 0..64 {
        let mut stream = connect();
        let reply = status(&mut stream);
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(reply["result"]["search_admission"]["capacity"], 2);
        clients.push(stream);
    }
    let mut excess = connect();
    let mut line = String::new();
    BufReader::new(&mut excess).read_line(&mut line).unwrap();
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["error"]["code"], -32002);
    assert_eq!(
        status(&mut clients[0])["result"]["connection_admission"]["active"],
        64
    );
    clients
        .pop()
        .unwrap()
        .shutdown(std::net::Shutdown::Both)
        .unwrap();
    let start = Instant::now();
    while status(&mut clients[0])["result"]["connection_admission"]["active"] == 64 {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut replacement = connect();
    assert!(status(&mut replacement).get("error").is_none());
}
