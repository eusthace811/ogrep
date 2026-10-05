//! An identifier search that finds nothing names the closest identifiers on
//! stderr; output and exit status stay those of the search.

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("lib.rs"),
        "fn enable_feature() {}\nfn enable_features() {}\nstruct Configuration;\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("main.rs"),
        "enable_feature();\nlet c = Configuration;\n",
    )
    .unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(dir.path())
        .arg("--no-index")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn a_misspelt_name_gets_the_closest_names() {
    let dir = fixture();
    let (code, out, err) = run(&dir, &["enabel_feature", "."]);
    assert_eq!(code, Some(1));
    assert!(out.is_empty());
    assert_eq!(
        err,
        "ogrep: no matches for \"enabel_feature\"; similar names: enable_feature, enable_features\n"
    );
    let (_, _, err) = run(&dir, &["Configuratoin", "."]);
    assert!(err.contains("similar names: Configuration"), "{err}");
}

#[test]
fn no_suggestion_when_it_does_not_apply() {
    let dir = fixture();
    for args in [
        &["enable_feature", "."][..],   // it matched
        &["-q", "enabel_feature", "."], // quiet
        &["--no-messages", "enabel_feature", "."],
        &["enabel.feature", "."],       // a regex, not an identifier
        &["-v", "enabel_feature", "."], // inverted
        &["zzqqxxvvkk", "."],           // nothing close
    ] {
        let (_, _, err) = run(&dir, args);
        assert!(!err.contains("similar names"), "{args:?}: {err}");
    }
}
