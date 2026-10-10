//! An indexed search reads files by the names the index recorded. A directory
//! or file replaced by a link since the build must not hand back content from
//! outside the tree: a scan never follows that link, so neither may the index.
#![cfg(unix)]

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const SENTINEL: &str = "OUTSIDE_SENTINEL";

struct Fixture {
    temp: TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("nested/dir")).unwrap();
        fs::write(root.join("nested/dir/file.txt"), "needle inside\n").unwrap();
        fs::write(root.join("kept.txt"), "needle kept\n").unwrap();
        fs::create_dir(temp.path().join("outside")).unwrap();
        fs::write(
            temp.path().join("outside/file.txt"),
            format!("needle {SENTINEL}\n"),
        )
        .unwrap();
        Command::cargo_bin("ogrep")
            .unwrap()
            .args(["index", "."])
            .current_dir(&root)
            .assert()
            .success();
        Self { temp, root }
    }

    fn outside(&self) -> PathBuf {
        self.temp.path().join("outside")
    }

    fn search(&self, root: &Path, args: &[&str]) -> (String, String) {
        let output = Command::cargo_bin("ogrep")
            .unwrap()
            .current_dir(&self.root)
            .arg("--stats")
            .args(args)
            .arg(root)
            .output()
            .unwrap();
        (
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }

    fn assert_contained(&self) {
        for args in [
            vec!["-F", "--", "needle"],
            vec!["--", "."],
            vec!["--passthru", "--", "needle"],
            vec!["-c", "--", "needle"],
            vec!["-l", "--", "needle"],
            vec!["-j1", "--", "needle"],
            vec!["--sort", "modified", "--", "needle"],
        ] {
            let (stdout, stderr) = self.search(Path::new("."), &args);
            assert!(!stdout.contains(SENTINEL), "{args:?}: {stdout}");
            assert!(stdout.contains("kept"), "{args:?}: {stdout}");
            assert!(
                stderr.contains("Query plan:"),
                "{args:?} must use the index: {stderr}"
            );
        }
    }
}

#[test]
fn indexed_search_does_not_follow_a_directory_swapped_for_a_link() {
    let fixture = Fixture::new();
    let dir = fixture.root.join("nested/dir");
    fs::rename(&dir, fixture.temp.path().join("saved")).unwrap();
    std::os::unix::fs::symlink(fixture.outside(), &dir).unwrap();
    fixture.assert_contained();
}

#[test]
fn indexed_search_does_not_follow_a_file_swapped_for_a_link() {
    let fixture = Fixture::new();
    let file = fixture.root.join("nested/dir/file.txt");
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(fixture.outside().join("file.txt"), &file).unwrap();
    fixture.assert_contained();
}

/// Git records a symlink as a symlink, so a tracked operations file can point
/// anywhere. `--entities` reads the files Git lists and must refuse that link.
#[test]
fn entities_do_not_follow_a_tracked_symlink() {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("outside")).unwrap();
    fs::write(
        temp.path().join("outside/compose.yml"),
        "services:\n  outside_sentinel:\n    image: x\n",
    )
    .unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("compose.yml"),
        "services:\n  web:\n    image: y\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("../outside/compose.yml", root.join("docker-compose.yml")).unwrap();
    for args in [
        &["init", "-q"][..],
        &["add", "."],
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "i",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
    }
    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&root)
        .args(["--entities", "."])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("web"), "{stdout}");
    assert!(!stdout.contains("outside_sentinel"), "{stdout}");
}

/// `-L` asks for links to be followed, which the index cannot answer, so it
/// walks the tree as ripgrep does.
#[test]
fn follow_walks_the_tree_instead_of_using_the_index() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink(fixture.outside(), fixture.root.join("linked")).unwrap();
    let (stdout, _) = fixture.search(Path::new("."), &["-L", "--sort", "path", "--", "needle"]);
    assert!(stdout.contains("./linked/file.txt"), "{stdout}");
    assert!(stdout.contains("./kept.txt"), "{stdout}");
}
