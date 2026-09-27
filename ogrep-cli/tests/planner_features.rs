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
    Command::cargo_bin("ogrep")
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
            let mut command = Command::cargo_bin("ogrep").unwrap();
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
    Command::cargo_bin("ogrep")
        .unwrap()
        .arg("index")
        .arg(root.path())
        .assert()
        .success();
    let query = |explain| {
        let mut cmd = Command::cargo_bin("ogrep").unwrap();
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

#[test]
fn indexed_and_scanned_crlf_output_preserves_source_bytes_and_json_offsets() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    std::fs::write(root.path().join("crlf.rs"), b"first\r\nneedle\r\nlast\r").unwrap();
    Command::cargo_bin("ogrep")
        .unwrap()
        .args(["index", root.path().to_str().unwrap()])
        .assert()
        .success();
    for indexed in [true, false] {
        let run = |flags: &[&str], pattern: &str| {
            let mut cmd = Command::cargo_bin("ogrep").unwrap();
            cmd.current_dir(root.path()).args([
                "--no-config",
                "--no-heading",
                "--color",
                "never",
                "-n",
                "-H",
            ]);
            if !indexed {
                cmd.arg("--no-index");
            }
            cmd.args(flags)
                .args(["--", pattern, "crlf.rs"])
                .output()
                .unwrap()
        };
        let plain = run(&[], "needle");
        assert!(plain.status.success());
        assert_eq!(plain.stdout, b"crlf.rs:2:needle\r\n");
        let cr = run(&[], r"\r$");
        assert!(cr.status.success());
        assert_eq!(
            cr.stdout,
            b"crlf.rs:1:first\r\ncrlf.rs:2:needle\r\ncrlf.rs:3:last\r\n"
        );
        let anchored = run(&[], "needle$");
        assert_eq!(anchored.status.code(), Some(1));
        assert!(anchored.stdout.is_empty());
        let json = run(&["--json"], "needle");
        assert!(json.status.success());
        let records: Vec<serde_json::Value> = String::from_utf8(json.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let hit = records.iter().find(|r| r["type"] == "match").unwrap();
        assert_eq!(hit["data"]["lines"]["text"], "needle\r\n");
        assert_eq!(hit["data"]["absolute_offset"], 7);
        assert_eq!(hit["data"]["submatches"][0]["start"], 0);
        assert_eq!(hit["data"]["submatches"][0]["end"], 6);
        let json = run(&["--json", "-C", "1"], "needle");
        assert!(json.status.success());
        let records: Vec<serde_json::Value> = String::from_utf8(json.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let last = records
            .iter()
            .find(|r| r["type"] == "context" && r["data"]["line_number"] == 3)
            .unwrap();
        assert_eq!(last["data"]["lines"]["text"], "last\r");
        let json = run(&["--json"], "last");
        let hit: serde_json::Value = String::from_utf8(json.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|r| r["type"] == "match")
            .unwrap();
        assert_eq!(hit["data"]["lines"]["text"], "last\r");
    }
}

#[test]
fn server_json_keeps_source_offsets_and_line_terminators() {
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    std::fs::write(root.path().join("source.rs"), b"before\r\nneedle\r\nafter").unwrap();
    Command::cargo_bin("ogrep")
        .unwrap()
        .args(["index", root.path().to_str().unwrap()])
        .assert()
        .success();
    let mut server = Server(
        std::process::Command::new(assert_cmd::cargo::cargo_bin("ogrep"))
            .args(["serve", "--no-watch", root.path().to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = std::time::Instant::now();
    loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited early"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "server did not become ready"
        );
        let ready = std::fs::read(root.path().join(".ogrep/serve.json"))
            .ok()
            .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
            .is_some_and(|info| info["pid"].as_u64() == Some(server.0.id().into()));
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(root.path())
        .args([
            "--no-config",
            "--json",
            "--stats",
            "-C",
            "1",
            "--",
            "needle",
            ".",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("(via server)"));
    let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let hit = records.iter().find(|r| r["type"] == "match").unwrap();
    assert_eq!(hit["data"]["absolute_offset"], 8);
    assert_eq!(hit["data"]["lines"]["text"], "needle\r\n");
    let last = records
        .iter()
        .find(|r| r["type"] == "context" && r["data"]["line_number"] == 3)
        .unwrap();
    assert_eq!(last["data"]["absolute_offset"], 16);
    assert_eq!(last["data"]["lines"]["text"], "after");
}
