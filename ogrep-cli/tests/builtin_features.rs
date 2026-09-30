//! End-to-end checks for the built-in structure, ranking, history and
//! repository tools, each pinned to a defect found in review.

use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn ogrep(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ogrep").unwrap();
    cmd.current_dir(dir);
    cmd
}

fn stdout(cmd: &mut Command) -> String {
    String::from_utf8(cmd.output().unwrap().stdout).unwrap()
}

#[test]
fn rank_keeps_same_named_files_under_two_paths_apart() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a")).unwrap();
    fs::create_dir_all(dir.path().join("b")).unwrap();
    fs::write(dir.path().join("a/mod.rs"), "fn zebra_one() {}\n").unwrap();
    fs::write(dir.path().join("b/mod.rs"), "zebra_one(); zebra_one();\n").unwrap();

    let out = stdout(ogrep(dir.path()).args(["--no-index", "--rank", "zebra_one", "a", "b"]));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [
            "a/mod.rs:fn zebra_one() {}",
            "b/mod.rs:zebra_one(); zebra_one();"
        ],
        "the definition ranks first and each file appears once"
    );
}

/// A git repository with one commit per `(path, contents)` step.
fn repo(steps: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .current_dir(dir.path())
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    for (i, (path, text)) in steps.iter().enumerate() {
        fs::write(dir.path().join(path), text).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", &format!("step {i}")]);
    }
    dir
}

#[test]
fn history_paths_with_spaces_match_globs() {
    let dir = repo(&[("x y.txt", "needle_value\n")]);
    ogrep(dir.path())
        .args(["index", "--history", "."])
        .assert()
        .success();

    let out = stdout(ogrep(dir.path()).args(["--history", "-g", "*.txt", "needle_value"]));
    assert!(out.contains("x y.txt:"), "{out:?}");
    assert!(!out.contains('\t'), "no tab left in the path: {out:?}");
}
