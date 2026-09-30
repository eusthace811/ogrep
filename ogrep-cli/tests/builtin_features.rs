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

#[test]
fn changed_since_limits_the_search_not_just_the_output() {
    let dir = repo(&[("a.txt", "needle old\n"), ("b.txt", "other\n")]);
    fs::write(dir.path().join("b.txt"), "needle new\n").unwrap();
    ogrep(dir.path()).args(["index", "."]).assert().success();

    let out = stdout(ogrep(dir.path()).args(["--changed-since", "HEAD", "needle"]));
    assert_eq!(out, "b.txt:needle new\n");
    // -q stops at the first match without printing; it used to report no
    // match because nothing had passed the output filter.
    ogrep(dir.path())
        .args(["-q", "--changed-since", "HEAD", "needle"])
        .assert()
        .code(0);
    // A named file is searched when it changed, and matches nothing when not.
    let named = stdout(ogrep(dir.path()).args(["--changed-since", "HEAD", "needle", "b.txt"]));
    assert_eq!(named, "needle new\n");
    ogrep(dir.path())
        .args(["--changed-since", "HEAD", "needle", "a.txt"])
        .assert()
        .code(1);
}

#[test]
fn tools_capture_lines_whatever_the_output_flags() {
    // Tools read the search's matching lines; a flag that shapes ordinary
    // output (here --count-matches) must not leak into theirs.
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("app.log"),
        "ERROR db timeout 12\nERROR db timeout 40\nINFO ok\n",
    )
    .unwrap();
    let plain = stdout(ogrep(dir.path()).args(["--log-templates", "ERROR", "app.log"]));
    assert!(plain.contains("ERROR db timeout <*>"), "{plain:?}");
    ogrep(dir.path())
        .args(["--log-templates", "--count-matches", "ERROR", "app.log"])
        .assert()
        .code(0)
        .stdout(plain);
}
