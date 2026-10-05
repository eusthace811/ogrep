//! `--max-results N` prints exactly the first N results of the full output,
//! through the local index and the server alike.

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn ogrep_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("ogrep")
}

/// Twelve files with five `needle` lines each, two of them adjacent, plus a
/// deeper directory that only `--max-depth` hides.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let body = "needle one\nplain\nneedle two\nneedle three\nplain\nplain\nneedle four\nplain\nneedle five\n";
    for i in 0..12 {
        fs::write(dir.path().join(format!("f{i:02}.txt")), body).unwrap();
    }
    fs::create_dir_all(dir.path().join("deep/er")).unwrap();
    for i in 0..30 {
        fs::write(dir.path().join(format!("deep/er/d{i:02}.txt")), body).unwrap();
    }
    dir
}

fn index(dir: &Path) -> std::path::PathBuf {
    let idx = dir.join(".idx");
    let ok = Command::new(ogrep_bin())
        .args(["index", ".", "--index-path", idx.to_str().unwrap()])
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok);
    idx
}

fn run(dir: &Path, idx: &Path, args: &[&str]) -> (String, String) {
    let out = Command::new(ogrep_bin())
        .args([
            "--index-path",
            idx.to_str().unwrap(),
            "--no-heading",
            "--sort=path",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.code().is_some_and(|c| c <= 1), "{out:?}");
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn head(text: &str, n: usize) -> String {
    text.lines().take(n).map(|l| format!("{l}\n")).collect()
}

/// Every mode, through whatever answers searches right now.
fn check_prefixes(dir: &Path, idx: &Path) {
    for mode in [
        &["-n"][..],
        &["-l"],
        &["-c"],
        &["-o", "-n"],
        &["-n", "-m", "2"],
    ] {
        let full = run(dir, idx, &[mode, &["needle", "."]].concat()).0;
        for n in [1, 4, 13, 1000] {
            let limited = run(
                dir,
                idx,
                &[mode, &["--max-results", &n.to_string(), "needle", "."]].concat(),
            )
            .0;
            assert_eq!(limited, head(&full, n), "{mode:?} --max-results {n}");
        }
    }
}

#[test]
fn local_output_is_the_head_of_the_full_output() {
    let dir = fixture();
    let idx = index(dir.path());
    check_prefixes(dir.path(), &idx);
}

#[test]
fn trailing_context_stops_at_the_next_match() {
    let dir = fixture();
    let idx = index(dir.path());
    // The second match (line 3) is followed by context it shares with the
    // third (line 4), which is past the limit: context ends there.
    let (out, _) = run(
        dir.path(),
        &idx,
        &["-n", "-A1", "--max-results", "2", "needle", "f00.txt"],
    );
    assert_eq!(out, "1:needle one\n2-plain\n3:needle two\n");
    // The last match's own trailing context is kept.
    let (out, _) = run(
        dir.path(),
        &idx,
        &["-n", "-A2", "--max-results", "4", "needle", "f00.txt"],
    );
    assert_eq!(
        out,
        "1:needle one\n2-plain\n3:needle two\n4:needle three\n5-plain\n6-plain\n7:needle four\n8-plain\n"
    );
}

#[test]
fn the_limit_spans_every_path_argument() {
    let dir = fixture();
    let idx = index(dir.path());
    let (out, _) = run(
        dir.path(),
        &idx,
        &[
            "-l",
            "--max-results",
            "3",
            "needle",
            "f00.txt",
            "f01.txt",
            "f02.txt",
            "f03.txt",
        ],
    );
    assert_eq!(out, "f00.txt\nf01.txt\nf02.txt\n");
}

#[test]
fn ranked_output_refuses_the_limit() {
    let dir = fixture();
    let idx = index(dir.path());
    let out = Command::new(ogrep_bin())
        .args([
            "--index-path",
            idx.to_str().unwrap(),
            "--rank",
            "--max-results",
            "3",
            "needle",
            ".",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--budget"));
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(dir: &Path, idx: &Path) -> Server {
    let server = Server(
        Command::new(ogrep_bin())
            .args([
                "serve",
                "--no-watch",
                "--index-path",
                idx.to_str().unwrap(),
                ".",
            ])
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    loop {
        let out = Command::new(ogrep_bin())
            .args([
                "--index-path",
                idx.to_str().unwrap(),
                "--stats",
                "-c",
                "needle",
                ".",
            ])
            .current_dir(dir)
            .output()
            .unwrap();
        if String::from_utf8_lossy(&out.stderr).contains("via server") {
            return server;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "server not ready"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn server_output_is_the_head_of_the_full_output() {
    let dir = fixture();
    let idx = dir.path().join(".idx");
    let _server = serve(dir.path(), &idx);
    check_prefixes(dir.path(), &idx);
    // Without --sort the server stops early; the reply must still be the
    // head of the full output, files in candidate order.
    for mode in [&["-n"][..], &["-l"]] {
        let plain = |extra: &[&str]| {
            let out = Command::new(ogrep_bin())
                .args(["--index-path", idx.to_str().unwrap(), "--no-heading"])
                .args(mode)
                .args(extra)
                .args(["needle", "."])
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                out.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        let full = plain(&[]);
        for n in [1, 7, 40] {
            assert_eq!(
                plain(&["--max-results", &n.to_string()]),
                head(&full, n),
                "{mode:?} {n}"
            );
        }
    }
}

#[test]
fn server_limit_counts_only_files_within_max_depth() {
    // Thirty deeper files match too; stopping on them would leave the output
    // short. (A reply that falls short anyway is asked for again whole.)
    let dir = fixture();
    let idx = dir.path().join(".idx");
    let _server = serve(dir.path(), &idx);
    let list = |extra: &[&str]| {
        let out = Command::new(ogrep_bin())
            .args([
                "--index-path",
                idx.to_str().unwrap(),
                "-l",
                "--max-depth",
                "1",
            ])
            .args(extra)
            .args(["needle", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let full = list(&[]);
    assert_eq!(full.lines().count(), 12);
    assert_eq!(list(&["--max-results", "10"]), head(&full, 10));
}
