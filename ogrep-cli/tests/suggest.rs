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

// The probe is a pattern of ogrep's own, so the user's engine must not reject it.
#[test]
fn a_pcre2_search_still_gets_suggestions() {
    let dir = fixture();
    for engine in [&["-P"][..], &["--engine", "pcre2"]] {
        let (code, _, err) = run(&dir, &[engine, &["enabel_feature", "."]].concat());
        assert_eq!(code, Some(1));
        assert!(
            err.contains("similar names: enable_feature"),
            "{engine:?}: {err}"
        );
    }
}

// The probe is a second search over the same roots, so it would repeat any
// warning the main search printed about how it ran.
#[test]
fn the_probe_does_not_repeat_the_main_search_warnings() {
    let dir = fixture();
    let search = || {
        let out = Command::cargo_bin("ogrep")
            .unwrap()
            .current_dir(dir.path())
            .args(["enabel_feature", "."])
            .output()
            .unwrap();
        String::from_utf8(out.stderr).unwrap()
    };
    let err = search();
    assert_eq!(err.matches("warning: no index").count(), 1, "{err}");
    assert!(err.contains("similar names: enable_feature"), "{err}");

    // A server that has gone away leaves its `serve.json` behind.
    Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(dir.path())
        .args(["index", "."])
        .assert()
        .success();
    fs::write(
        dir.path().join(".ogrep/serve.json"),
        r#"{"pid":999999,"port":1}"#,
    )
    .unwrap();
    let err = search();
    assert_eq!(err.matches("Server unavailable").count(), 1, "{err}");
    assert!(err.contains("similar names: enable_feature"), "{err}");
}

// Only the probe's repeat is dropped: each root still reports its own fallback.
#[test]
fn each_root_keeps_its_own_warning() {
    let dirs = [fixture(), fixture()];
    for dir in &dirs {
        Command::cargo_bin("ogrep")
            .unwrap()
            .current_dir(dir.path())
            .args(["index", "."])
            .assert()
            .success();
        fs::write(
            dir.path().join(".ogrep/serve.json"),
            r#"{"pid":999999,"port":1}"#,
        )
        .unwrap();
    }
    let out = Command::cargo_bin("ogrep")
        .unwrap()
        .args(["enabel_feature"])
        .args(dirs.iter().map(|d| d.path()))
        .output()
        .unwrap();
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(err.matches("Server unavailable").count(), 2, "{err}");

    // One index for both roots: each still says it scanned.
    let missing = dirs[0].path().join("no-such-index");
    let out = Command::cargo_bin("ogrep")
        .unwrap()
        .arg("--index-path")
        .arg(&missing)
        .arg("enabel_feature")
        .args(dirs.iter().map(|d| d.path()))
        .output()
        .unwrap();
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(err.matches("warning: no index").count(), 2, "{err}");
}
