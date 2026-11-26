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

/// `ogrep serve . --no-watch` in the fixture, ready to answer once started:
/// its startup stale check is done (it would otherwise undo a change made to
/// the tree for the test) and its coverage is published.
struct Server(std::process::Child);

impl Server {
    fn start(fixture: &Fixture) -> Self {
        Self::start_in(fixture, &fixture.root)
    }

    fn start_in(fixture: &Fixture, root: &Path) -> Self {
        // Outside the tree, or the log would be a new file in the stale walk.
        let log = fixture.temp.path().join("serve.log");
        let server = Server(
            std::process::Command::new(assert_cmd::cargo::cargo_bin("ogrep"))
                .args(["serve", ".", "--no-watch"])
                .current_dir(root)
                .stderr(fs::File::create(&log).unwrap())
                .spawn()
                .unwrap(),
        );
        // It ends one of two ways, depending on whether the build's mtimes
        // need a recheck.
        let start = std::time::Instant::now();
        let finished = |log: &str| {
            log.contains("stale check: index is up-to-date")
                || log.contains("stale check: streamed")
        };
        while !finished(&fs::read_to_string(&log).unwrap_or_default()) {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "the server did not finish its stale check: {}",
                fs::read_to_string(&log).unwrap_or_default()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        while !String::from_utf8(
            Command::cargo_bin("ogrep")
                .unwrap()
                .args(["status", "."])
                .current_dir(root)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .contains("Hidden coverage: complete")
        {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "the server did not publish its coverage"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        server
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A server reads a cache miss by the indexed name too. With no watcher to
/// notice the swap, the name now runs through the link, and following it put
/// the outside file in the response and the shared cache.
#[test]
fn server_search_does_not_follow_a_directory_swapped_for_a_link() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    let dir = fixture.root.join("nested/dir");
    fs::rename(&dir, fixture.temp.path().join("saved")).unwrap();
    std::os::unix::fs::symlink(fixture.outside(), &dir).unwrap();

    let (stdout, stderr) = fixture.search(Path::new("."), &["-F", "--", "needle"]);
    assert!(stdout.contains("kept"), "{stdout}{stderr}");
    assert!(!stdout.contains(SENTINEL), "{stdout}");
    assert!(
        !stderr.contains("falling back"),
        "the search must be answered by the server: {stderr}"
    );
}

/// A server that cannot open its root answers with an error rather than an
/// answer missing every file it would read, and the fallback says why.
#[test]
fn a_server_that_cannot_open_its_root_says_so() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    // The tree moves away from the root the server opens; its index, and the
    // server's address in it, move along, so the client still asks it.
    let moved = fixture.temp.path().join("moved");
    fs::rename(&fixture.root, &moved).unwrap();
    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(fixture.temp.path())
        .args(["-F", "--", "needle", "moved"])
        .output()
        .unwrap();
    let (stdout, stderr) = (
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    );
    assert!(
        stderr.contains("falling back to local index: cannot open the served root"),
        "{stderr}"
    );
    assert!(
        stdout.contains("kept"),
        "the fallback still answers: {stdout}"
    );
    // Quietly under -q, as the warnings beside it.
    let quiet = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(fixture.temp.path())
        .args(["-q", "-F", "--", "needle", "moved"])
        .output()
        .unwrap();
    assert_eq!(quiet.status.code(), Some(0));
    let stderr = String::from_utf8(quiet.stderr).unwrap();
    assert!(!stderr.contains("falling back"), "{stderr}");
}

/// A tree copied with its index takes along the address of the original's
/// server. That server serves the original, so it must not answer for the
/// copy, beside the original or inside it.
#[test]
fn a_copied_tree_is_not_answered_by_the_server_of_its_original() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    for copy in [fixture.temp.path().join("copy"), fixture.root.join("copy")] {
        fs::create_dir(&copy).unwrap();
        fs::write(copy.join("kept.txt"), "needle copied\n").unwrap();
        fs::create_dir(copy.join(".ogrep")).unwrap();
        for entry in fs::read_dir(fixture.root.join(".ogrep")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), copy.join(".ogrep").join(entry.file_name())).unwrap();
            }
        }
        assert!(copy.join(".ogrep/serve.json").is_file());
        let (stdout, _) = fixture.search(&copy, &["-F", "--", "needle"]);
        assert!(stdout.contains("needle copied"), "{stdout}");
        assert!(!stdout.contains("needle kept"), "{stdout}");
        assert!(!stdout.contains("nested"), "{stdout}");
        // Nor is it reported as the copy's server.
        let status = Command::cargo_bin("ogrep")
            .unwrap()
            .current_dir(&copy)
            .args(["status", "."])
            .output()
            .unwrap();
        let status = String::from_utf8(status.stdout).unwrap();
        assert!(status.contains("Server:     not running"), "{status}");
    }
}

/// Indexing a copy again leaves the original's address in it. The original's
/// server still serves only the original, so it does not answer for the copy.
#[test]
fn a_copy_indexed_again_is_not_answered_by_the_server_of_its_original() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    let copy = fixture.temp.path().join("copy");
    fs::create_dir(&copy).unwrap();
    fs::write(copy.join("kept.txt"), "needle copied\n").unwrap();
    fs::create_dir(copy.join(".ogrep")).unwrap();
    fs::copy(
        fixture.root.join(".ogrep/serve.json"),
        copy.join(".ogrep/serve.json"),
    )
    .unwrap();
    Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&copy)
        .args(["index", "."])
        .assert()
        .success();
    assert!(copy.join(".ogrep/serve.json").is_file());
    let (stdout, _) = fixture.search(&copy, &["-F", "--", "needle"]);
    assert!(stdout.contains("needle copied"), "{stdout}");
    assert!(!stdout.contains("needle kept"), "{stdout}");
}

/// A server for a moved tree serves it although the index still records the
/// old place, which has been made again: `status` says so and searches use it.
#[test]
fn a_moved_tree_is_served_while_its_old_place_exists() {
    let fixture = Fixture::new();
    let moved = fixture.temp.path().join("moved");
    fs::rename(&fixture.root, &moved).unwrap();
    fs::create_dir(&fixture.root).unwrap();
    let _server = Server::start_in(&fixture, &moved);
    let status = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&moved)
        .args(["status", "."])
        .output()
        .unwrap();
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("Server status for"), "{status}");
    let (stdout, stderr) = fixture.search(&moved, &["-F", "--", "needle"]);
    assert!(stdout.contains("needle kept"), "{stdout}");
    assert!(stderr.contains("(via server)"), "{stderr}");
}

/// A pattern the server rejects is the user's error, reported once by the
/// local search, not a reason the server gives for a fallback.
#[test]
fn a_pattern_the_server_rejects_is_reported_once() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    let (_, stderr) = fixture.search(Path::new("."), &["--", "needle("]);
    assert_eq!(stderr.matches("unclosed group").count(), 1, "{stderr}");
    assert!(!stderr.contains("falling back"), "{stderr}");
}

/// A server whose tree moved away still holds the moved index's lock, so
/// `status` there shows it, with its error, though the old path exists again:
/// another server cannot start until it stops.
#[test]
fn status_shows_the_server_of_a_moved_tree_whose_old_path_exists_again() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    let moved = fixture.temp.path().join("moved");
    fs::rename(&fixture.root, &moved).unwrap();
    fs::create_dir(&fixture.root).unwrap();
    let status = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&moved)
        .args(["status", "."])
        .output()
        .unwrap();
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("Server status for"), "{status}");
    assert!(status.contains("Stop it (pid"), "{status}");
}

/// A server in another namespace (a container sharing the volume, Windows
/// seen from WSL) records a root that is not a path here: it is still used.
#[test]
fn a_server_whose_root_is_named_elsewhere_is_used() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    let info_path = fixture.root.join(".ogrep/serve.json");
    let mut info: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&info_path).unwrap()).unwrap();
    info["root"] = "/nonexistent-namespace/workspace".into();
    fs::write(&info_path, info.to_string()).unwrap();
    let (stdout, stderr) = fixture.search(Path::new("."), &["-F", "--", "needle"]);
    assert!(stdout.contains("needle kept"), "{stdout}");
    assert!(stderr.contains("(via server)"), "{stderr}");
}

/// A server whose tree is gone, as after a move across filesystems, does
/// not answer for a copy of that tree from what it remembers: not a listing,
/// nor a search with no candidate, which a watcher that saw the files go
/// would leave every search with. The user is told, and `status` says why.
#[test]
fn a_server_whose_tree_is_gone_says_so() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    let copy = fixture.temp.path().join("copy");
    fs::create_dir(&copy).unwrap();
    for name in ["kept.txt", "nested"] {
        fs::rename(fixture.root.join(name), copy.join(name)).unwrap();
    }
    fs::rename(fixture.root.join(".ogrep"), copy.join(".ogrep")).unwrap();
    fs::remove_dir(&fixture.root).unwrap();
    for args in [
        &["--files"][..],
        &["-F", "--", "needle"],
        &["-F", "--", "absent"],
    ] {
        let output = Command::cargo_bin("ogrep")
            .unwrap()
            .current_dir(&copy)
            .arg("--stats")
            .args(args)
            .arg(".")
            .output()
            .unwrap();
        let (stdout, stderr) = (
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        );
        assert_eq!(
            stdout.contains("kept"),
            args.last() != Some(&"absent"),
            "{stdout}"
        );
        assert!(!stderr.contains("(via server)"), "{stderr}");
        assert!(stderr.contains("cannot open the served root"), "{stderr}");
    }
    let status = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&copy)
        .args(["status", "."])
        .output()
        .unwrap();
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("cannot open"), "{status}");
}

/// The same once every candidate is cached: whether the root's loss is
/// reported does not depend on what the cache happens to hold. The server
/// runs without a watcher, so nothing evicts what the first search cached,
/// and the second is answered from the cache alone (with the root opened
/// only on a miss, as before, this test fails).
#[test]
fn a_server_that_cannot_open_its_root_says_so_from_a_warm_cache() {
    let fixture = Fixture::new();
    let _server = Server::start(&fixture);
    let (stdout, stderr) = fixture.search(Path::new("."), &["-F", "--", "needle"]);
    assert!(
        stdout.contains("kept") && !stderr.contains("falling back"),
        "{stdout}{stderr}"
    );
    let moved = fixture.temp.path().join("moved");
    fs::rename(&fixture.root, &moved).unwrap();
    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(fixture.temp.path())
        .args(["-F", "--", "needle", "moved"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("falling back to local index: cannot open the served root"),
        "{stderr}"
    );
}

/// A root that may be searched through but not listed (`-wx`) is opened to
/// resolve names under it, which needs no more, as reading a file did.
#[test]
fn indexed_search_needs_only_search_permission_on_the_root() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o311)).unwrap();
    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(fixture.temp.path())
        .args(["-F", "--", "needle"])
        .arg(&fixture.root)
        .output()
        .unwrap();
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o755)).unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("kept"), "{stdout}");
}
