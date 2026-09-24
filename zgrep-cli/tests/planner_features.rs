use assert_cmd::Command;

#[test]
fn indexed_fixed_case_insensitive_search_keeps_unicode_equivalents() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    for (file, text) in [
        ("kelvin.txt", "KELVIN"),
        ("shell.txt", "ſHELL"),
        ("cafe.txt", "CAFÉ"),
        ("istanbul.txt", "İSTANBUL"),
    ] {
        std::fs::write(root.path().join(file), format!("{text}\n")).unwrap();
    }
    Command::cargo_bin("zgrep")
        .unwrap()
        .arg("index")
        .arg(root.path())
        .assert()
        .success();
    for (pattern, expected) in [
        ("kelvin", "kelvin.txt"),
        ("shell", "shell.txt"),
        ("café", "cafe.txt"),
        ("İstanbul", "istanbul.txt"),
    ] {
        for indexed in [true, false] {
            let mut command = Command::cargo_bin("zgrep").unwrap();
            command
                .current_dir(root.path())
                .args(["-F", "-i", "-l", "--color", "never"]);
            if !indexed {
                command.arg("--no-index");
            }
            let result = command.args(["--", pattern, "."]).output().unwrap();
            assert!(result.status.success(), "{pattern}: {result:?}");
            let output = String::from_utf8(result.stdout).unwrap();
            assert_eq!(output.lines().count(), 1, "{pattern}: {output}");
            assert!(output.trim_end().ends_with(expected), "{pattern}: {output}");
        }
    }
}

#[test]
fn explain_reports_real_work_without_changing_match_output() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    std::fs::write(root.path().join("hit.txt"), "abcde\n").unwrap();
    std::fs::write(root.path().join("scattered.txt"), "abcdXbcde\n").unwrap();
    Command::cargo_bin("zgrep")
        .unwrap()
        .arg("index")
        .arg(root.path())
        .assert()
        .success();
    let query = |explain| {
        let mut cmd = Command::cargo_bin("zgrep").unwrap();
        cmd.current_dir(root.path())
            .args(["-F", "-l", "--color", "never"]);
        if explain {
            cmd.arg("--explain");
        }
        cmd.args(["--", "abcde", "."]).output().unwrap()
    };
    let normal = query(false);
    let explain = query(true);
    assert!(explain.status.success());
    assert_eq!(normal.stdout, explain.stdout);
    let stderr = String::from_utf8(explain.stderr).unwrap();
    let line = stderr
        .lines()
        .find_map(|s| s.strip_prefix("Index work: "))
        .unwrap();
    let work: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(work["position_rejections"], 1);
    assert!(work["lists_loaded"].as_u64().unwrap() > 0);
}
