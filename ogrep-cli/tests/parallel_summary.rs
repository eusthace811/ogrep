//! The parallel local verifier must account for every searched file exactly
//! as the sequential loop does, or `--json`'s summary undercounts.

use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn summary(root: &Path, index: &Path, args: &[&str]) -> (u64, u64, u64) {
    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(root)
        .args([
            "--no-config",
            "--json",
            "--index-path",
            index.to_str().unwrap(),
        ])
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let last: serde_json::Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], "summary", "{stdout}");
    let stats = &last["data"]["stats"];
    let field = |name: &str| stats[name].as_u64().unwrap();
    (
        field("searches"),
        field("searches_with_match"),
        field("bytes_searched"),
    )
}

#[test]
fn json_summary_counts_every_searched_file_in_parallel() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    let index = temp.path().join("index");
    fs::create_dir_all(root.join(".git")).unwrap();
    // Every file holds the pattern's trigrams, so all are candidates; only
    // one has them on the same line.
    for i in 0..80 {
        fs::write(root.join(format!("f{i}.txt")), "needle one\nand two\n").unwrap();
    }
    fs::write(root.join("hit.txt"), "needle and two\n").unwrap();
    Command::cargo_bin("ogrep")
        .unwrap()
        .args(["index", root.to_str().unwrap(), "--index-path"])
        .arg(&index)
        .assert()
        .success();

    let sequential = summary(&root, &index, &["-j", "1", "needle.*two", "."]);
    let parallel = summary(&root, &index, &["-j", "4", "needle.*two", "."]);
    assert_eq!(sequential.0, 81);
    assert_eq!(parallel, sequential);
}
