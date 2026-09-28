//! `-f` may name a pipe that can be read only once, so its patterns must be
//! read once and shared by everything that needs them: the smart-case
//! decision, the matcher, the index plan and every path argument.
use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[cfg(unix)]
#[test]
fn a_pattern_pipe_is_read_once_for_every_use() {
    use std::io::Write;

    let temp = TempDir::new().unwrap();
    let dir = temp.path();
    fs::write(dir.join("a.txt"), "foo\nFOO\nbar\n").unwrap();
    fs::write(dir.join("b.txt"), "foo\nzzz\n").unwrap();
    let fifo = dir.join("patterns");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let writer = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            let mut pipe = fs::OpenOptions::new().write(true).open(fifo).unwrap();
            pipe.write_all(b"foo\n").unwrap();
        })
    };

    let output = Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(dir)
        .args(["--no-config", "--no-index", "-n", "-S", "-f"])
        .arg(&fifo)
        .args(["a.txt", "b.txt"])
        .timeout(std::time::Duration::from_secs(30))
        .output()
        .unwrap();
    // The writer blocks in open() until something reads the pipe. If ogrep
    // never did, open it here so the test fails instead of hanging.
    let started = std::time::Instant::now();
    while !writer.is_finished() && started.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !writer.is_finished() {
        let _ = fs::read(&fifo);
    }
    writer.join().unwrap();
    let mut lines: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    // Smart case: `foo` is all lowercase, so FOO matches too, and b.txt, the
    // second path, still gets the patterns.
    assert_eq!(lines, ["a.txt:1:foo", "a.txt:2:FOO", "b.txt:1:foo"]);
}

/// `-f` as ripgrep reads it (expected output taken from rg 15.2): spaces
/// kept, `\r\n` stripped, a blank line an empty pattern, repeatable, `-` for
/// standard input, and an empty file searching for nothing.
#[test]
fn pattern_files_read_like_ripgrep_in_scan_and_index_modes() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    let index = temp.path().join("index");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("t.txt"), "foo\n foo\nbar\na  b\nab\n").unwrap();
    let pats = temp.path();
    fs::write(pats.join("empty"), "").unwrap();
    fs::write(pats.join("sp"), " foo\n").unwrap();
    fs::write(pats.join("blank"), "foo\n\n").unwrap();
    fs::write(pats.join("crlf"), "foo\r\nbar\r\n").unwrap();
    Command::cargo_bin("ogrep")
        .unwrap()
        .args(["index", root.to_str().unwrap(), "--index-path"])
        .arg(&index)
        .assert()
        .success();

    let all = "1:foo\n2: foo\n3:bar\n4:a  b\n5:ab\n";
    // The empty file is the only pattern source and -v is off.
    let no_match_possible = |args: &[&str]| {
        args.contains(&"../empty") && !args.contains(&"-e") && !args.contains(&"-v")
    };
    let cases: &[(&[&str], &str, i32)] = &[
        (&["-f", "../empty"], "", 1),
        (&["-L", "-f", "../empty"], "", 1),
        (&["--passthru", "-f", "../empty"], "", 1),
        (&["-c", "--include-zero", "-f", "../empty"], "", 1),
        // -F still applies to --with when -f is empty.
        (&["-F", "-v", "-f", "../empty", "--with", "a  b"], all, 0),
        (&["-F", "-v", "-f", "../empty", "--with", "a.b"], "", 1),
        (&["-v", "-f", "../empty"], all, 0),
        (&["-f", "../empty", "-e", "bar"], "3:bar\n", 0),
        (&["-f", "../sp"], "2: foo\n", 0),
        (&["-f", "../blank"], all, 0),
        (&["-f", "../crlf"], "1:foo\n2: foo\n3:bar\n", 0),
        (
            &["-f", "../sp", "-f", "../crlf"],
            "1:foo\n2: foo\n3:bar\n",
            0,
        ),
    ];
    for (args, expected, code) in cases {
        for indexed in [false, true] {
            let mut cmd = Command::cargo_bin("ogrep").unwrap();
            cmd.current_dir(&root)
                .args(["--no-config", "-n", "--index-path"])
                .arg(&index);
            if !indexed {
                cmd.arg("--no-index");
            }
            // A directory, not the file itself: a named file bypasses the index.
            cmd.args(*args)
                .args(["--no-filename", "--stats", "."])
                .assert()
                .code(*code)
                .stdout(*expected)
                .stderr(if no_match_possible(args) {
                    // No match is possible: like rg, nothing is searched or
                    // reported, --stats included.
                    predicates::str::is_empty().boxed()
                } else {
                    predicates::str::contains(if indexed {
                        "Query plan:"
                    } else {
                        "Brute-force search completed"
                    })
                    .boxed()
                });
        }
    }

    // Standard input can supply one -f only, as in rg, and -f files are read
    // in order, so an earlier missing one is what gets reported.
    Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&root)
        .args(["--no-config", "--no-index", "-f", "-", "-f", "-", "t.txt"])
        .write_stdin("foo\n")
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "standard input can be read only once",
        ));
    Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&root)
        .args([
            "--no-config",
            "--no-index",
            "-f",
            "../nope",
            "-f",
            "-",
            "-f",
            "-",
        ])
        .arg("t.txt")
        .write_stdin("foo\n")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("../nope"));

    // A bad argument is an error before anything is searched, even under
    // --no-messages (which rg does not apply to argument errors).
    for (bad, message) in [
        (&["-g", "["][..], "error parsing glob"),
        (&["-t", "nosuchtype"], "unrecognized file type"),
    ] {
        Command::cargo_bin("ogrep")
            .unwrap()
            .current_dir(&root)
            .args(["--no-config", "--no-index", "--no-messages", "-e", "foo"])
            .args(bad)
            .arg(".")
            .assert()
            .code(2)
            .stderr(predicates::str::contains(message));
        // --files checks them too, before looking at any path.
        Command::cargo_bin("ogrep")
            .unwrap()
            .current_dir(&root)
            .args(["--no-config", "--no-messages", "--files"])
            .args(bad)
            .arg("nosuch")
            .assert()
            .code(2)
            .stderr(predicates::str::contains(message));
    }
    // A failure while matching a file (here the backtracking engine's limit)
    // is a search error, which --no-messages hides as rg does.
    fs::write(root.join("aaa.txt"), format!("{}\n", "a".repeat(40))).unwrap();
    for (quiet, silent) in [(false, false), (true, true)] {
        let mut cmd = Command::cargo_bin("ogrep").unwrap();
        cmd.current_dir(&root).args(["--no-config", "--no-index"]);
        if quiet {
            cmd.arg("--no-messages");
        }
        let assert = cmd.args([r"(a|a)*\1b", "aaa.txt"]).assert().code(2);
        if silent {
            assert.stderr("");
        } else {
            assert.stderr(predicates::str::contains("regex match error"));
        }
    }
    fs::remove_file(root.join("aaa.txt")).unwrap();

    // A bad regex is not a file error: --no-messages does not hide it.
    Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&root)
        .args(["--no-config", "--no-index", "--no-messages", "(", "."])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("regex"));

    // No match is possible, but a bad argument is still an error.
    for bad in [
        &["-t", "nosuchtype"][..],
        &["-g", "["],
        &["--with", "("],
        &["--without", "("],
    ] {
        Command::cargo_bin("ogrep")
            .unwrap()
            .current_dir(&root)
            .args(["--no-config", "--no-index", "-f", "../empty"])
            .args(bad)
            .arg(".")
            .assert()
            .code(2)
            .stderr(predicates::str::is_empty().not());
    }

    Command::cargo_bin("ogrep")
        .unwrap()
        .current_dir(&root)
        .args(["--no-config", "--no-index", "-n", "-f", "-", "t.txt"])
        .write_stdin("foo\n")
        .assert()
        .code(0)
        .stdout("1:foo\n2: foo\n");
}
