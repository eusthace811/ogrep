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

fn fixture() -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    let index = temp.path().join("index");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src").join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("src").join("notes.txt"), "notes\n").unwrap();
    fs::write(root.join("asset.bin"), [1, 2, 3]).unwrap();
    fs::write(root.join("binary.txt"), b"before\0after").unwrap();
    (temp, root, index)
}

fn build_index(root: &Path, index: &Path) {
    ogrep()
        .args([
            "index",
            root.to_str().unwrap(),
            "--index-path",
            index.to_str().unwrap(),
        ])
        .assert()
        .success();
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
        assert!(
            child.try_wait().unwrap().is_none(),
            "server exited before accepting connections"
        );
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

fn output_lines(command: &mut Command) -> Vec<String> {
    let output = command.assert().success().get_output().stdout.clone();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| line.replace('\\', "/"))
        .collect()
}

#[test]
fn indexed_files_include_non_content_paths_and_use_the_snapshot() {
    let (_temp, root, index) = fixture();
    build_index(&root, &index);
    fs::write(root.join("created-after-index.rs"), "fn late() {}\n").unwrap();

    let indexed = output_lines(ogrep().args([
        "--files",
        "--sort",
        "path",
        root.to_str().unwrap(),
        "--index-path",
        index.to_str().unwrap(),
    ]));
    assert!(indexed.iter().any(|path| path.ends_with("/src/main.rs")));
    assert!(indexed.iter().any(|path| path.ends_with("/asset.bin")));
    assert!(indexed.iter().any(|path| path.ends_with("/binary.txt")));
    assert!(
        !indexed
            .iter()
            .any(|path| path.ends_with("/created-after-index.rs")),
        "the indexed path unexpectedly walked the live tree: {indexed:?}"
    );

    let walked = output_lines(ogrep().args([
        "--files",
        "--no-index",
        root.to_str().unwrap(),
        "--index-path",
        index.to_str().unwrap(),
    ]));
    assert!(
        walked
            .iter()
            .any(|path| path.ends_with("/created-after-index.rs"))
    );

    let case_insensitive_ignore = output_lines(ogrep().args([
        "--files",
        "--ignore-file-case-insensitive",
        root.to_str().unwrap(),
        "--index-path",
        index.to_str().unwrap(),
    ]));
    assert!(
        case_insensitive_ignore
            .iter()
            .any(|path| path.ends_with("/created-after-index.rs")),
        "the traversal-changing ignore flag unexpectedly used the snapshot"
    );
}

#[test]
fn legacy_filename_index_falls_back_and_indexed_scope_still_filters() {
    let (_temp, root, index) = fixture();
    build_index(&root, &index);

    let scoped = output_lines(ogrep().args([
        "--files",
        "--sort",
        "path",
        "-t",
        "rust",
        root.join("src").to_str().unwrap(),
        "--index-path",
        index.to_str().unwrap(),
    ]));
    assert_eq!(scoped.len(), 1, "unexpected scoped output: {scoped:?}");
    assert!(scoped[0].ends_with("/src/main.rs"));

    fs::remove_file(index.join(ogrep_core::path_index::EXTRA_PATHS_FILENAME)).unwrap();
    fs::write(root.join("legacy-live.rs"), "fn legacy() {}\n").unwrap();
    let fallback = output_lines(ogrep().args([
        "--files",
        root.to_str().unwrap(),
        "--index-path",
        index.to_str().unwrap(),
    ]));
    assert!(
        fallback
            .iter()
            .any(|path| path.ends_with("/legacy-live.rs")),
        "a legacy index did not fall back to walking: {fallback:?}"
    );
}

#[test]
fn indexed_sort_agrees_for_scoped_search_and_file_listing() {
    let (_temp, root, index) = fixture();
    let subtree = root.join("src");
    fs::write(subtree.join("lib.rs"), "fn lib() {}\n").unwrap();
    fs::write(root.join("src.rs"), "fn outside() {}\n").unwrap();
    for (name, seconds) in [("main.rs", 0), ("lib.rs", 60)] {
        let time = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds);
        fs::OpenOptions::new()
            .write(true)
            .open(subtree.join(name))
            .unwrap()
            .set_modified(time)
            .unwrap();
    }
    build_index(&root, &index);

    for via_server in [false, true] {
        let _server = via_server.then(|| start_server(&root, &index));
        for (key, names) in [
            ("path", ["lib.rs", "main.rs"]),
            ("modified", ["main.rs", "lib.rs"]),
        ] {
            for flag in ["--sort", "--sortr"] {
                let mut expected: Vec<_> = names
                    .iter()
                    .map(|name| subtree.join(name).to_string_lossy().replace('\\', "/"))
                    .collect();
                if flag == "--sortr" {
                    expected.reverse();
                }

                let listed = output_lines(ogrep().args([
                    "--files",
                    "-t",
                    "rust",
                    flag,
                    key,
                    subtree.to_str().unwrap(),
                    "--index-path",
                    index.to_str().unwrap(),
                ]));
                assert_eq!(listed, expected, "{flag} {key}, server={via_server}");

                let output = ogrep()
                    .args([
                        "--stats",
                        "-l",
                        "-t",
                        "rust",
                        flag,
                        key,
                        "fn",
                        subtree.to_str().unwrap(),
                        "--index-path",
                        index.to_str().unwrap(),
                    ])
                    .assert()
                    .success()
                    .get_output()
                    .clone();
                let searched: Vec<_> = String::from_utf8(output.stdout)
                    .unwrap()
                    .lines()
                    .map(|line| line.replace('\\', "/"))
                    .collect();
                assert_eq!(searched, expected, "{flag} {key}, server={via_server}");
                let stderr = String::from_utf8(output.stderr).unwrap();
                assert_eq!(stderr.contains("(via server)"), via_server, "{stderr}");
            }
        }
    }
}

#[test]
fn files_uses_the_live_server_filename_index() {
    let (_temp, root, index) = fixture();
    build_index(&root, &index);
    let _server = start_server(&root, &index);

    // Make the local sidecar incomplete after the server has loaded its copy.
    // The binary path below can therefore come only from the files RPC.
    ogrep_core::path_index::write_extra_paths(&index, &[]).unwrap();
    let listed = output_lines(ogrep().args([
        "--files",
        root.to_str().unwrap(),
        "--index-path",
        index.to_str().unwrap(),
    ]));

    assert!(
        listed.iter().any(|path| path.ends_with("/asset.bin")),
        "the client did not use the server's filename index: {listed:?}"
    );
}

#[test]
fn reshaped_searches_through_the_server_place_matches_in_the_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    let index = temp.path().join("index");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("d.rs"),
        "fn alpha() {}\n\nfn beta() {\n    foo();\n}\nconst S: &str = \"foo\";\n// foo foo\n",
    )
    .unwrap();
    build_index(&root, &index);
    let _server = start_server(&root, &index);
    let run = |args: &[&str]| {
        output_lines(
            ogrep()
                .args(args)
                .arg(root.to_str().unwrap())
                .args(["--index-path", index.to_str().unwrap()]),
        )
        .join("\n")
    };
    // Zones, definitions and scopes are looked up where each match is, which
    // needs the server to send every row's offset.
    assert!(run(&["--in", "strings", "foo"]).ends_with(r#"const S: &str = "foo";"#));
    assert!(run(&["--kind", "def", "beta"]).ends_with("fn beta() {"));
    let scoped = run(&["--scope", "foo"]);
    assert!(scoped.contains("=3=fn beta() {"), "{scoped}");
    assert!(!scoped.contains("alpha"), "{scoped}");
    let ranked = run(&["--json", "--rank", "foo"]);
    assert!(ranked.contains(r#""absolute_offset":27"#), "{ranked}");
    // Ranking sees each line's first match, so it finds the definition.
    let explained = run(&["--rank", "--explain-rank", "beta"]);
    assert!(explained.contains("def_lines=1"), "{explained}");
    // Plain searches through the server still see every match on a line.
    assert!(
        run(&["--count-matches", "foo"]).ends_with(":4"),
        "{}",
        run(&["--count-matches", "foo"])
    );
    assert_eq!(run(&["-o", "foo"]).lines().count(), 4);
    // And the server answered them.
    let stats = ogrep()
        .args(["--stats", "--scope", "foo"])
        .arg(root.to_str().unwrap())
        .args(["--index-path", index.to_str().unwrap()])
        .output()
        .unwrap();
    let stats = String::from_utf8_lossy(&stats.stderr);
    assert!(stats.contains("(via server)"), "{stats}");
}
