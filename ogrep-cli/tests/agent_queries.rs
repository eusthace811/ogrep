//! `--with`/`--without` file conditions and `--ident` naming-convention search
//! must answer the same through a filesystem scan, the local index and a
//! running server.

use assert_cmd::Command;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn ogrep() -> Command {
    Command::cargo_bin("ogrep").unwrap()
}

fn ogrep_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("ogrep")
}

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_server(root: &Path, index: &Path) -> ServerGuard {
    let mut child = std::process::Command::new(ogrep_bin())
        .args([
            "serve",
            "--no-watch",
            "--index-path",
            index.to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let serve_json = index.join("serve.json");
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "server did not start"
        );
        assert!(child.try_wait().unwrap().is_none(), "server exited early");
        if let Ok(data) = fs::read_to_string(&serve_json)
            && let Ok(info) = serde_json::from_str::<serde_json::Value>(&data)
            && let Some(port) = info.get("port").and_then(serde_json::Value::as_u64)
            && TcpStream::connect(("127.0.0.1", port as u16)).is_ok()
        {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    ServerGuard(child)
}

fn fixture() -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    let index = temp.path().join("index");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    let files = [
        ("src/a.rs", "use std::sync::Mutex;\nasync fn run() {}\n"),
        ("src/b.rs", "use std::sync::Mutex;\nfn run() {}\n"),
        (
            "src/c.rs",
            "use std::sync::Mutex;\nasync fn run() {}\n#[cfg(test)]\nmod tests {}\n",
        ),
        ("src/d.rs", "async fn only_async() {}\n"),
        ("src/e.rs", "let id = getUserId();\n"),
        ("src/f.py", "def get_user_id():\n    return GET_USER_ID\n"),
        ("src/g.ts", "const x = get-user-id;\nGetUserID();\n"),
        ("src/h.rs", "get_user_name();\n"),
    ];
    for (path, content) in files {
        fs::write(root.join(path), content).unwrap();
    }
    ogrep()
        .args([
            "index",
            root.to_str().unwrap(),
            "--index-path",
            index.to_str().unwrap(),
        ])
        .assert()
        .success();
    (temp, root, index)
}

fn lines(root: &Path, index: &Path, mode: &str, args: &[&str]) -> Vec<String> {
    let mut cmd = ogrep();
    cmd.current_dir(root).args([
        "--no-config",
        "--color",
        "never",
        "--index-path",
        index.to_str().unwrap(),
    ]);
    if mode == "scan" {
        cmd.arg("--no-index");
    }
    let output = cmd.args(args).output().unwrap();
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "{mode} {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut rows: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| l.replace('\\', "/"))
        .map(|l| l.strip_prefix("./").map(str::to_string).unwrap_or(l))
        .collect();
    rows.sort();
    rows
}

#[test]
fn file_conditions_and_identifier_search_agree_in_every_mode() {
    let (_temp, root, index) = fixture();
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (
            vec!["-l", "--with", "async", "--", "Mutex", "."],
            vec!["src/a.rs", "src/c.rs"],
        ),
        (
            vec![
                "-l",
                "--with",
                "async",
                "--without",
                r"#\[cfg\(test",
                "--",
                "Mutex",
                ".",
            ],
            vec!["src/a.rs"],
        ),
        (
            vec!["-l", "--without", "async", "--", "Mutex", "."],
            vec!["src/b.rs"],
        ),
        (
            vec!["-c", "--with", "Mutex", "--", "run", "."],
            vec!["src/a.rs:1", "src/b.rs:1", "src/c.rs:1"],
        ),
        (
            vec!["-n", "--with", "cfg", "--", "Mutex", "."],
            vec!["src/c.rs:1:use std::sync::Mutex;"],
        ),
        (
            vec!["-l", "--ident", "getUserId", "."],
            vec!["src/e.rs", "src/f.py", "src/g.ts"],
        ),
        (
            vec!["-c", "--ident", "get_user_id", "."],
            vec!["src/e.rs:1", "src/f.py:2", "src/g.ts:2"],
        ),
        (
            vec!["-l", "--with", "nothing_has_this", "--", "Mutex", "."],
            vec![],
        ),
        // `-U`: a condition may span lines.
        (
            vec!["-l", "-U", "--with", r"Mutex;\nasync", "--", "run", "."],
            vec!["src/a.rs", "src/c.rs"],
        ),
        // Modes the pattern's trigrams cannot narrow; `--with` still does.
        (
            vec![
                "--files-without-match",
                "--with",
                "async",
                "--",
                "Mutex",
                ".",
            ],
            vec!["src/d.rs"],
        ),
        (
            vec!["-l", "-v", "--with", "Mutex", "--", "zzz", "."],
            vec!["src/a.rs", "src/b.rs", "src/c.rs"],
        ),
        (
            vec![
                "-c",
                "--include-zero",
                "--with",
                "only_async",
                "--",
                "Mutex",
                ".",
            ],
            vec!["src/d.rs:0"],
        ),
    ];

    for mode in ["scan", "local"] {
        for (args, expected) in &cases {
            assert_eq!(
                &lines(&root, &index, mode, args),
                expected,
                "{mode} {args:?}"
            );
        }
    }
    let _server = start_server(&root, &index);
    for (args, expected) in &cases {
        let mut with_stats = vec!["--stats"];
        with_stats.extend(args.iter());
        let output = ogrep()
            .current_dir(&root)
            .args([
                "--no-config",
                "--color",
                "never",
                "--index-path",
                index.to_str().unwrap(),
            ])
            .args(&with_stats)
            .output()
            .unwrap();
        // The client answers these two modes from the index itself.
        let local_only = args
            .iter()
            .any(|a| *a == "--files-without-match" || *a == "--include-zero");
        assert!(
            local_only || String::from_utf8_lossy(&output.stderr).contains("(via server)"),
            "{args:?} did not use the server: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            &lines(&root, &index, "server", args),
            expected,
            "server {args:?}"
        );
    }
}

#[test]
fn file_listing_refuses_content_conditions() {
    for flag in ["--with", "--without"] {
        ogrep().args(["--files", flag, "x", "."]).assert().code(2);
    }
}

#[test]
fn ident_alone_searches_the_current_directory() {
    let (_temp, root, index) = fixture();
    let mut rows = lines(&root, &index, "local", &["-l", "--ident", "getUserId"]);
    rows.sort();
    assert_eq!(rows, ["src/e.rs", "src/f.py", "src/g.ts"]);
}

#[test]
fn ident_refuses_fixed_strings() {
    ogrep()
        .args(["--ident", "getUserId", "-F", "."])
        .assert()
        .code(2);
}
