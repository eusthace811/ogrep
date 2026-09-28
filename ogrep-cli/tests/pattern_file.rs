//! `-f` may name a pipe that can be read only once, so its patterns must be
//! read once and shared by everything that needs them: the smart-case
//! decision, the matcher, the index plan and every path argument.
#![cfg(unix)]

use assert_cmd::Command;
use std::fs;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn a_pattern_pipe_is_read_once_for_every_use() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();
    fs::write(dir.join("a.txt"), "foo\nFOO\nbar\n").unwrap();
    fs::write(dir.join("b.txt"), "foo\nzzz\n").unwrap();
    let fifo = dir.join("patterns");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let writer = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            let mut pipe = fs::OpenOptions::new().write(true).open(fifo).unwrap();
            pipe.write_all(b"foo\n").unwrap();
        })
    };

    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(dir)
        .args(["--no-config", "--no-index", "-n", "-S", "-f"])
        .arg(&fifo)
        .args(["a.txt", "b.txt"])
        .timeout(std::time::Duration::from_secs(30))
        .output()
        .unwrap();
    writer.join().unwrap();
    let mut lines: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    // Smart case: `foo` is all lowercase, so FOO matches too, and b.txt, the
    // second path, still gets the patterns.
    assert_eq!(lines, ["a.txt:1:foo", "a.txt:2:FOO", "b.txt:1:foo"]);
}
