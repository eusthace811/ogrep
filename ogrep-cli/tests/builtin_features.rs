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

/// Text output with `/` between path parts: on Windows ogrep prints `\`, as ripgrep
/// does. JSON output escapes `\`, so it is read with `raw_stdout`.
fn stdout(cmd: &mut Command) -> String {
    let out = raw_stdout(cmd);
    if cfg!(windows) {
        out.replace('\\', "/")
    } else {
        out
    }
}

fn raw_stdout(cmd: &mut Command) -> String {
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

/// Run git in `dir` as a throwaway identity, failing the test on error.
fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .current_dir(dir)
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
}

/// A git repository with one commit per `(path, contents)` step.
fn repo(steps: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    let git = |args: &[&str]| git(dir.path(), args);
    git(&["init", "-q", "-b", "main"]);
    for (i, (path, text)) in steps.iter().enumerate() {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
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

#[test]
fn repository_tools_read_non_ascii_paths() {
    // git quotes such names ("src/caf\303\251.py") unless told not to.
    let dir = repo(&[
        ("café.py", "def brew_coffee():\n    return 1\n"),
        (
            "test_café.py",
            "from café import brew_coffee\n\ndef test_brew():\n    brew_coffee()\n",
        ),
    ]);
    ogrep(dir.path())
        .args(["index", "--graph", "."])
        .assert()
        .success();
    let out = raw_stdout(ogrep(dir.path()).args(["--related-tests", "café.py"]));
    assert!(out.contains("test_café.py"), "{out:?}");
    assert!(!out.contains("\\303"), "{out:?}");
}

#[test]
fn cached_structure_is_per_language_not_just_per_content() {
    // The same bytes are a definition in TypeScript and a parse error in
    // JavaScript; whichever is parsed first must not decide for the other.
    let dir = TempDir::new().unwrap();
    let text = "interface Widget {\n  size: number;\n}\n";
    fs::write(dir.path().join("a.js"), text).unwrap();
    fs::write(dir.path().join("b.ts"), text).unwrap();
    ogrep(dir.path()).args(["index", "."]).assert().success();

    let ts = stdout(ogrep(dir.path()).args(["--kind", "def", "-g", "*.ts", "Widget", "."]));
    assert!(ts.contains("b.ts"), "{ts:?}");
    let js = stdout(ogrep(dir.path()).args(["--kind", "def", "-g", "*.js", "Widget", "."]));
    assert_eq!(js, "", "the TypeScript parse must not be reused for a.js");
}

#[test]
fn stack_trace_scopes_are_right_in_crlf_files() {
    let body: Vec<String> = std::iter::once("def first():".to_string())
        .chain((0..40).map(|i| format!("    x = {i}")))
        .chain(
            [
                "    return x",
                "",
                "def second():",
                "    raise ValueError('boom')",
                "",
            ]
            .map(String::from),
        )
        .collect();
    let dir = repo(&[("app.py", &body.join("\r\n"))]);
    let line = body.iter().position(|l| l.contains("raise")).unwrap() + 1;
    fs::write(
        dir.path().join("trace.txt"),
        format!("Traceback (most recent call last):\n  File \"app.py\", line {line}, in second\n    raise ValueError('boom')\n"),
    )
    .unwrap();
    let out = stdout(ogrep(dir.path()).args(["--stack-trace", "trace.txt"]));
    assert!(out.contains("[second]"), "{out:?}");
}

#[test]
fn history_is_found_where_index_history_put_it() {
    let dir = repo(&[("top.txt", "needle_x\n"), ("sub/f.txt", "needle_x\n")]);
    ogrep(dir.path())
        .args(["index", "--history", "sub"])
        .assert()
        .success();

    let out = stdout(ogrep(dir.path()).args(["--history", "needle_x", "sub"]));
    assert!(out.contains("sub/f.txt:1:+needle_x"), "{out:?}");
}

#[test]
fn at_revision_skips_hidden_files_like_a_working_tree_search() {
    let dir = repo(&[
        ("src/a.txt", "TODO_MARK a\n"),
        (".github/ci.yml", "TODO_MARK ci\n"),
    ]);
    let at = stdout(ogrep(dir.path()).args(["--at", "HEAD", "TODO_MARK"]));
    assert_eq!(at, "src/a.txt:TODO_MARK a\n");
    let hidden =
        stdout(ogrep(dir.path()).args(["--at", "HEAD", "--hidden", "--sort=path", "TODO_MARK"]));
    assert!(hidden.contains(".github/ci.yml"), "{hidden:?}");
}

#[test]
fn doc_drift_filters_names_with_the_search_options() {
    let dir = repo(&[
        (
            "guide.md",
            "Call `load_config()` and then `Parse_Header`.\n",
        ),
        ("lib.py", "def other():\n    pass\n"),
    ]);
    // -F takes the pattern literally: `(` is not a regex error.
    ogrep(dir.path())
        .args(["--doc-drift", "-F", "load_config("])
        .assert()
        .code(1);
    // -i applies to the name filter.
    let out = stdout(ogrep(dir.path()).args(["--doc-drift", "-i", "PARSE_HEADER"]));
    assert!(out.contains("Parse_Header"), "{out:?}");
}

#[test]
fn doc_drift_skips_release_notes_by_name_or_directory_only() {
    let dir = repo(&[
        ("docs/history/guide.md", "Call `missing_in_history`.\n"),
        ("src/exchanges/README.md", "Call `missing_in_exchanges`.\n"),
        ("changelog.d/123.md", "Removed `missing_in_fragment`.\n"),
        ("docs/.changes/notes.md", "Removed `missing_in_changes`.\n"),
        ("HISTORY.md", "Removed `missing_in_history_file`.\n"),
        ("docs/exchanges.md", "Call `missing_in_exchanges_doc`.\n"),
        ("docs/release_notes/v2.md", "Removed `missing_in_notes`.\n"),
        ("MIGRATING.md", "Replace `missing_in_migrating`.\n"),
        (
            "docs/history-1.x.md",
            "Removed `missing_in_history_notes`.\n",
        ),
        (
            "docs/history-search.md",
            "Call `missing_in_history_guide`.\n",
        ),
        ("docs/migrate.md", "Call `missing_in_migrate_guide`.\n"),
        ("lib.py", "def other():\n    pass\n"),
    ]);
    let out = stdout(ogrep(dir.path()).args(["--doc-drift", "."]));
    assert!(out.contains("`missing_in_history`"), "{out:?}");
    assert!(out.contains("`missing_in_exchanges`"), "{out:?}");
    assert!(out.contains("`missing_in_exchanges_doc`"), "{out:?}");
    for skipped in [
        "missing_in_fragment",
        "missing_in_changes",
        "missing_in_history_file",
        "missing_in_notes",
        "missing_in_migrating",
        "missing_in_history_notes",
        "missing_in_history_guide",
        "missing_in_migrate_guide",
    ] {
        assert!(!out.contains(&format!("`{skipped}`")), "{out:?}");
    }
}

#[test]
fn conflicts_name_files_with_spaces_cleanly() {
    let dir = repo(&[("x y.txt", "l1\nl2\nl3\n")]);
    let run = |args: &[&str]| git(dir.path(), args);
    run(&["checkout", "-qb", "other"]);
    fs::write(dir.path().join("x y.txt"), "l1\nOTHER\nl3\n").unwrap();
    run(&["commit", "-qam", "other"]);
    run(&["checkout", "-q", "main"]);
    run(&["checkout", "-qb", "mine"]);
    fs::write(dir.path().join("x y.txt"), "l1\nMINE\nl3\n").unwrap();
    run(&["commit", "-qam", "mine"]);

    let out = stdout(ogrep(dir.path()).args(["--conflicts", "."]));
    assert!(out.contains("other: x y.txt  overlapping"), "{out:?}");
    assert!(!out.contains('\t'), "{out:?}");
}

#[test]
fn conflicts_do_not_read_removed_comment_lines_as_headers() {
    // With --unified=0 a removed "-- comment" line reads "--- comment".
    let dir = repo(&[("a.sql", "-- drop old index\nselect 1;\nx\ny\nz\n")]);
    let run = |args: &[&str]| git(dir.path(), args);
    run(&["checkout", "-qb", "other"]);
    fs::write(dir.path().join("a.sql"), "select 1;\nx\nOTHER\nz\n").unwrap();
    run(&["commit", "-qam", "other"]);
    run(&["checkout", "-q", "main"]);
    run(&["checkout", "-qb", "mine"]);
    fs::write(dir.path().join("a.sql"), "select 1;\nx\nMINE\nz\n").unwrap();
    run(&["commit", "-qam", "mine"]);

    let out = stdout(ogrep(dir.path()).args(["--conflicts", "."]));
    assert_eq!(
        out,
        "other: a.sql  overlapping lines  (here 1,4; there 1,4)\n"
    );
}

#[test]
fn changed_since_is_refused_by_tools_that_would_ignore_it() {
    let dir = TempDir::new().unwrap();
    ogrep(dir.path())
        .args(["--changed-since", "HEAD", "--near", "x"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "--changed-since cannot be combined with --near",
        ));
}

#[test]
fn at_revision_refuses_what_would_read_the_working_tree() {
    let dir = repo(&[("a.py", "def f():\n    return 1\n")]);
    for flags in [
        &["--scope"][..],
        &["--kind", "def"],
        &["--files"],
        &["--near"],
    ] {
        ogrep(dir.path())
            .args(["--at", "HEAD"])
            .args(flags)
            .arg("f")
            .assert()
            .code(2)
            .stderr(predicates::str::contains("--at cannot be combined with"));
    }
    ogrep(dir.path())
        .args(["--at", "HEAD", "--rank", "f"])
        .assert()
        .code(0);
}

#[test]
fn history_filters_and_limits_like_a_search() {
    let dir = repo(&[
        ("src/lib.rs", "fn foo_mark() {}\n"),
        ("src/models/x.py", "foo_mark here\n"),
    ]);
    ogrep(dir.path())
        .args(["index", "--history", "."])
        .assert()
        .success();
    // Globs see paths from the search root, as a working-tree search does.
    let out = stdout(ogrep(dir.path()).args(["--history", "-g", "models/**", "foo_mark", "src"]));
    assert!(out.contains("src/models/x.py"), "{out:?}");
    assert!(!out.contains("src/lib.rs"), "{out:?}");
    // -m 0 shows nothing and matches nothing, as in ripgrep.
    ogrep(dir.path())
        .args(["--history", "-m", "0", "foo_mark", "."])
        .assert()
        .code(1)
        .stdout("");
    // --since works with a single file as the path.
    ogrep(dir.path())
        .args(["--history", "--since", "2w", "foo_mark", "src/lib.rs"])
        .assert()
        .code(0);
}

#[test]
fn at_revision_filters_a_named_file_by_its_name() {
    let dir = repo(&[("src/lib.rs", "fn foo_mark() {}\n")]);
    ogrep(dir.path())
        .args(["--at", "HEAD", "-t", "rust", "foo_mark", "src/lib.rs"])
        .assert()
        .code(0);
}

#[test]
fn near_works_below_the_index_root() {
    let dir = repo(&[
        ("src/core.rs", "pub fn shared_helper() {}\n"),
        ("src/user.rs", "fn run() { shared_helper(); }\n"),
    ]);
    ogrep(dir.path())
        .args(["index", "--graph", "."])
        .assert()
        .success();
    // Matches come back relative to src/, the graph's paths relative to the
    // repository: both must meet.
    let out = stdout(ogrep(dir.path()).args([
        "--index-path",
        ".ogrep",
        "--near",
        "-F",
        "pub fn shared_helper",
        "src",
    ]));
    assert!(out.contains("src/user.rs"), "{out:?}");
}

#[test]
fn reshaped_searches_see_every_match_and_keep_context_directional() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("t.txt"), "foo foo foo\n").unwrap();
    fs::write(
        dir.path().join("z.py"),
        "call(zork)  # zork is deprecated\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("k.py"),
        "x = 1\nref foo_ref\nctx1\nctx2\ndef foo_ref():\n    pass\n",
    )
    .unwrap();
    fs::write(dir.path().join("bin.dat"), b"needle\0binary\n").unwrap();
    let run = |args: &[&str]| stdout(ogrep(dir.path()).arg("--no-index").args(args));
    // --count-matches counts matches, not lines, under --rank too.
    assert_eq!(run(&["--rank", "--count-matches", "foo", "t.txt"]), "3\n");
    // A line is in comments when any of its matches is.
    assert!(run(&["--in", "comments", "zork", "z.py"]).contains("# zork"));
    // -A context of a filtered-out match is not before-context of the next.
    assert_eq!(
        run(&["--kind", "def", "-A2", "foo_ref", "k.py"]),
        "def foo_ref():\n    pass\n"
    );
    // A file that matches only as binary is still reported.
    assert!(run(&["--rank", "needle", "bin.dat"]).contains("binary file matches"));
}

#[test]
fn shapes_that_would_be_ignored_are_refused() {
    let dir = TempDir::new().unwrap();
    for args in [&["--rank", "--scope", "x"][..], &["--callers", "-c", "x"]] {
        ogrep(dir.path())
            .args(args)
            .assert()
            .code(2)
            .stderr(predicates::str::contains("cannot be combined"));
    }
}

#[test]
fn multi_document_manifests_and_cronjobs_are_read() {
    let manifest = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: checkout\nspec:\n  template:\n    spec:\n      containers:\n        - name: c\n          env:\n            - name: CART_URL\n              value: http://cart:7070\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: cart\n---\napiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: nightly-sync\nspec:\n  jobTemplate:\n    spec:\n      template:\n        spec:\n          containers:\n            - name: j\n              env:\n                - name: CART_URL\n                  value: http://cart:7070\n---\napiVersion: monitoring.coreos.com/v1\nkind: PrometheusRule\nmetadata:\n  name: rules\nspec:\n  groups:\n    - name: g\n      rules:\n        - alert: CartDown\n          expr: up{job=\"cart\"} == 0\n";
    let dir = repo(&[("deploy/all.yaml", manifest)]);
    let entities = stdout(ogrep(dir.path()).args(["--entities", "."]));
    assert!(entities.contains("checkout"), "{entities:?}");
    let blast = stdout(ogrep(dir.path()).args(["--blast", "cart"]));
    assert!(
        blast.contains("checkout") && blast.contains("nightly-sync"),
        "{blast:?}"
    );
}

#[test]
fn tool_flags_are_exclusive_and_parse_as_documented() {
    use predicates::prelude::PredicateBooleanExt;
    let dir = TempDir::new().unwrap();
    let refused = [
        &["--near", "--entities", "x"][..],
        &["--blast", "-e", "api", "."],
        &["-q", "--entities", "."],
        &["--callers", "--kind", "def", "x"],
        &["--history", "--multiline-dotall", "a.b", "."],
        &["--in", "comments", "-r", "Z", "x", "."],
        &["--rank", "-r", "Z", "x", "."],
        &["--callers", "--vimgrep", "x", "."],
        &["--callers", "-o", "x", "."],
    ];
    for args in refused {
        ogrep(dir.path()).args(args).assert().code(2).stderr(
            predicates::str::contains("cannot be combined")
                .or(predicates::str::contains("takes its subject")),
        );
    }
    let refused = [
        (
            &["--conflicts", "-e", "x"][..],
            "--conflicts takes no pattern",
        ),
        (
            &["--introduced", "x", "a", "b"],
            "--introduced takes one path at a time",
        ),
        (
            &["--near", "x", "a", "b"],
            "--near takes one path at a time",
        ),
        (
            &["--rank-fit", "q.json", "-e", "x"],
            "--rank-fit takes no pattern",
        ),
        (
            &["--rank-fit", "q.json", ".", "b"],
            "--rank-fit takes one path at a time",
        ),
        (&["--rank-fit", "q.json", "missing"], "is not a directory"),
    ];
    for (args, message) in refused {
        ogrep(dir.path())
            .args(args)
            .assert()
            .code(2)
            .stderr(predicates::str::contains(message));
    }
    // --map takes its budget only as --map=N, so the pattern after it is a
    // pattern: this fails for want of a graph, not as a bad --map value.
    ogrep(dir.path())
        .args(["--map", "Foo", "."])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("no file graph"));
}

#[test]
fn binary_files_count_the_same_with_and_without_reshaping() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("b.bin"), b"hello world\nhello\0x\nhello\n").unwrap();
    for extra in [&[][..], &["--scope"], &["--rank"]] {
        let out = stdout(
            ogrep(dir.path())
                .args(["--no-index", "-c"])
                .args(extra)
                .args(["hello", "b.bin"]),
        );
        assert_eq!(out, "3\n", "with {extra:?}");
    }
}

#[test]
fn scope_is_taken_at_the_match_that_passes() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("t.js"),
        "function a() { return \"x\" } function b() { x() }\n",
    )
    .unwrap();
    let out = raw_stdout(ogrep(dir.path()).args([
        "--no-index",
        "--json",
        "--scope",
        "--in",
        "code",
        "x",
        "t.js",
    ]));
    assert!(out.contains(r#""name":"b""#), "{out}");
    assert!(!out.contains(r#""name":"a""#), "{out}");
}

#[test]
fn kind_and_zone_filters_decide_per_match() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("a.py"),
        "class A:\n    def get_qs(self): return super().get_qs()\n\nx = \"token\"  # token here\n",
    )
    .unwrap();
    let run = |args: &[&str]| stdout(ogrep(dir.path()).arg("--no-index").args(args).arg("a.py"));
    // The line defines get_qs and also calls it.
    assert!(run(&["--kind", "ref", "get_qs"]).contains("super().get_qs()"));
    // Ranking sees only the kept reference, not the definition beside it.
    let ranked = run(&["--kind", "ref", "--rank", "--explain-rank", "get_qs"]);
    assert!(ranked.contains("def_lines=0"), "{ranked}");
    // Nor do columns report the dropped definition.
    let rows = run(&["--kind", "ref", "--vimgrep", "get_qs"]);
    assert_eq!(rows.lines().count(), 1, "{rows}");
    assert!(rows.starts_with("a.py:2:38:"), "{rows}");
    // Without a grammar the keyword heuristic also judges each match.
    fs::write(dir.path().join("b.zz"), "x = foo(); fn foo() {}\n").unwrap();
    let unparsed =
        |args: &[&str]| stdout(ogrep(dir.path()).arg("--no-index").args(args).arg("b.zz"));
    assert_eq!(
        unparsed(&["--kind", "def", "--count-matches", "foo"]),
        "1\n"
    );
    assert_eq!(unparsed(&["--kind", "ref", "-o", "foo"]), "foo\n");
    let json = unparsed(&["--kind", "def", "--json", "foo"]);
    // The definition (bytes 14..17) is kept, not the call.
    assert!(
        json.contains(r#""submatches":[{"end":17,"match":{"text":"foo"},"start":14}]"#),
        "{json}"
    );

    // The line has the word in a string and in a comment.
    assert!(run(&["--in", "comments", "token"]).contains("# token here"));
    let json = run(&["--in", "comments", "--json", "token"]);
    assert!(
        json.contains(r#""submatches":[{"end":20,"match":{"text":"token"},"start":15}]"#),
        "{json}"
    );
}

#[test]
fn reshaped_output_keeps_structure_context_and_callers_straight() {
    let dir = TempDir::new().unwrap();
    let write = |name: &str, text: &[u8]| fs::write(dir.path().join(name), text).unwrap();
    write(
        "bom.rs",
        b"\xef\xbb\xbf// x\nfn k() { let a = \"foo\"; }\nfn foo() {}\n",
    );
    write(
        "b.py",
        b"def foo():\n    return \"foo\"\n\ndef beta():\n    foo()\n",
    );
    write(
        "c.rs",
        b"fn crlf2() {\n    let s = \"x foo\";\n    foo();\n}\n",
    );
    let run = |args: &[&str]| stdout(ogrep(dir.path()).arg("--no-index").args(args));
    // A byte-order mark does not shift where matches are looked up.
    assert!(run(&["--in", "strings", "foo", "bom.rs"]).contains("let a"));
    assert!(run(&["--kind", "def", "foo", "bom.rs"]).contains("fn foo()"));
    // A name in a string is not a use.
    assert_eq!(
        run(&["--callers", "foo", "b.py"]),
        "4=def beta():  (1 use)\n"
    );
    // Uses are counted by line; zero context is accepted.
    write("u.py", b"def twice():\n    foo(); foo()\n");
    assert_eq!(
        run(&["--callers", "-C0", "foo", "u.py"]),
        "1=def twice():  (1 use)\n"
    );
    // -q prints nothing for reshaped output either; the exit code answers.
    for shape in [&["--callers"][..], &["--scope"], &["--rank"]] {
        ogrep(dir.path())
            .arg("--no-index")
            .arg("-q")
            .args(shape)
            .args(["foo", "u.py"])
            .assert()
            .code(0)
            .stdout("");
    }
    // --vimgrep shapes output only: --rank orders files the same with it.
    write("one.txt", b"x foo\nfoo\n");
    write("many.txt", b"start foo\nbar\nbaz\nqux\nend\n");
    let ranked = |extra: &[&str]| {
        stdout(
            ogrep(dir.path())
                .args(["--no-index", "-U", "--rank", "-l"])
                .args(extra)
                .args([r"foo(\n[a-z]+)*", "one.txt", "many.txt"]),
        )
    };
    assert_eq!(ranked(&[]), ranked(&["--vimgrep"]));
    // A matching line that --in dropped is still context.
    assert_eq!(
        run(&["-n", "--in", "strings", "-A1", "foo", "c.rs"]),
        "2:    let s = \"x foo\";\n3-    foo();\n"
    );
    // The scope header comes before the context inside the scope, never
    // before lines above it.
    assert_eq!(
        run(&["-n", "--scope", "-C1", "foo", "c.rs"]),
        "1=fn crlf2() {\n1-fn crlf2() {\n2:    let s = \"x foo\";\n3:    foo();\n4-}\n"
    );
    write("e.rs", b"// top\nfn e() {\n    foo();\n}\n");
    assert_eq!(
        run(&["-n", "--scope", "-B2", "foo", "e.rs"]),
        "1-// top\n2=fn e() {\n2-fn e() {\n3:    foo();\n"
    );
    // -E none searches the byte-order mark, so it stays in place.
    assert!(run(&["-E", "none", "--kind", "def", "foo", "bom.rs"]).contains("fn foo()"));
    // Under -o, rows on different lines are not duplicates; --per-file
    // counts lines, each shown with all its matches.
    assert_eq!(
        run(&["--rank", "-n", "-o", "foo", "c.rs"]),
        "2:foo\n3:foo\n"
    );
    write("f.txt", b"foo foo\nbar\nfoo\n");
    assert_eq!(
        run(&["--rank", "--per-file", "1", "-n", "-o", "foo", "f.txt"]),
        "1:foo\n1:foo\n"
    );
    // A context line stays under the scope it is in.
    write(
        "n.py",
        b"class C:\n    def m(self):\n        foo()\n        return 1\n\n    x = foo\n",
    );
    assert_eq!(
        run(&["-n", "--scope", "-C2", "foo", "n.py"]),
        "1-class C:\n2=    def m(self):\n2-    def m(self):\n3:        foo()\n4-        return 1\n1=class C:\n5-\n6:    x = foo\n"
    );
    // The separator comes before the header that opens a block.
    write(
        "g.rs",
        b"fn a() {\n    foo();\n}\n\nfn b() {\n    foo();\n}\n",
    );
    assert_eq!(
        run(&["-n", "--scope", "-B1", "foo", "g.rs"]),
        "1=fn a() {\n1-fn a() {\n2:    foo();\n--\n5=fn b() {\n5-fn b() {\n6:    foo();\n"
    );
    // A nested scope's header comes before its own lines, and a block after
    // a gap names its scope again.
    write(
        "i.rs",
        b"impl T {\n    fn a() {\n        x;\n    }\n    // zz\n    fn b() {\n        y;\n        zz;\n    }\n}\n",
    );
    assert_eq!(
        run(&["-n", "--scope", "-B2", "zz", "i.rs"]),
        "1=impl T {\n3-        x;\n4-    }\n5:    // zz\n6=    fn b() {\n6-    fn b() {\n7-        y;\n8:        zz;\n"
    );
    write(
        "j.rs",
        b"fn a() {\n    qq();\n    x;\n    y;\n    z;\n    qq();\n}\n",
    );
    assert_eq!(
        run(&["-n", "--scope", "-A1", "qq", "j.rs"]),
        "1=fn a() {\n2:    qq();\n3-    x;\n--\n1=fn a() {\n6:    qq();\n7-}\n"
    );
    // Under -o, reshaping still sees whole lines: --rank finds the
    // definition, and JSON reports whole lines, as without reshaping.
    write("k.rs", b"let x = foo;\nfn foo() {}\n");
    assert_eq!(
        run(&["--rank", "--per-file", "1", "-n", "-o", "foo", "k.rs"]),
        "2:foo\n"
    );
    let json =
        raw_stdout(ogrep(dir.path()).args(["--no-index", "--rank", "--json", "-o", "foo", "k.rs"]));
    assert!(json.contains(r#""text":"fn foo() {}\n""#), "{json}");
    // Under -o, a dropped matching line is context, as a whole line.
    write("h.rs", b"fn foo() {\n    foo();\n    x\n}\n");
    assert_eq!(
        run(&["-n", "--kind", "def", "-o", "-A2", "foo", "h.rs"]),
        "1:foo\n2-    foo();\n3-    x\n"
    );
}

#[test]
fn template_to_code_finds_the_writer_despite_filled_in_words() {
    // The logger name and a non-numeric value are not template variables to
    // Drain, so the whole constant text appears nowhere in the code; two-word
    // windows of it still do, and the file holding the rarer ones wins over
    // one that shares a common phrase. Prose quoting the message is skipped.
    let dir = repo(&[
        (
            "src/pool.py",
            "def take():\n    log.error(\"pool exhausted for tenant %s after %d waits\", t, n)\n",
        ),
        ("src/other.py", "log.info(\"for tenant %s\", t)\n"),
        (
            "docs/ops.md",
            "If you see pool exhausted for tenant, raise the limit.\n",
        ),
    ]);
    fs::write(
        dir.path().join("app.log"),
        "2026-10-03T10:00:00Z ERROR payments.db pool exhausted for tenant acme after 17 waits\n",
    )
    .unwrap();
    let out = stdout(ogrep(dir.path()).args([
        "--no-index",
        "--log-templates",
        "--template-to-code",
        "ERROR",
        "app.log",
    ]));
    assert!(
        out.contains("<- src/pool.py:2:"),
        "the writer is found: {out:?}"
    );
}

#[test]
fn about_ranks_files_by_the_words_of_a_description() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("admin")).unwrap();
    fs::write(
        dir.path().join("admin/main.py"),
        "class ChangeList:\n    def date_hierarchy(self, year):\n        if year > 9999:\n            raise ValueError\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("admin/forms.py"),
        "# admin forms\nclass AdminForm: pass\n",
    )
    .unwrap();
    fs::write(dir.path().join("notes.txt"), "nothing relevant here\n").unwrap();
    let out = stdout(ogrep(dir.path()).args([
        "--no-index",
        "--about",
        "Handled out-of-range year in admin date_hierarchy.",
        "-l",
    ]));
    let files: Vec<&str> = out.lines().collect();
    assert_eq!(files, ["admin/main.py", "admin/forms.py"], "{out:?}");
    // The text is the query: every positional argument is a path, and a
    // description with no searchable word is an error.
    let scoped = stdout(ogrep(dir.path()).args([
        "--no-index",
        "--about",
        "date_hierarchy year",
        "-l",
        "admin",
    ]));
    assert_eq!(scoped, "admin/main.py\n");
    ogrep(dir.path())
        .args(["--about", "fix it in the", "-l"])
        .assert()
        .code(2);
    ogrep(dir.path())
        .args(["--about", "date_hierarchy", "-e", "x"])
        .assert()
        .code(2);
}

#[test]
fn about_scores_prose_by_its_best_section_and_prints_it() {
    let dir = TempDir::new().unwrap();
    // A long reference page mentions every term somewhere, but in different
    // sections; a short page has them all in one section.
    let mut long = String::from("Reference\n=========\n\n");
    for (i, word) in ["cache", "timeout", "eviction"].iter().enumerate() {
        long.push_str(&format!("Part {i}\n------\n\n"));
        for _ in 0..40 {
            long.push_str(&format!(
                "The {word} setting is described here at length.\n"
            ));
        }
        long.push('\n');
    }
    fs::write(dir.path().join("reference.txt"), long).unwrap();
    fs::write(
        dir.path().join("caching.txt"),
        "Caching\n=======\n\nIntro text.\n\nEviction\n--------\n\nThe cache evicts an entry after its timeout; eviction is lazy.\n",
    )
    .unwrap();
    let out =
        stdout(ogrep(dir.path()).args(["--no-index", "-n", "--about", "cache timeout eviction"]));
    let mut lines = out.lines();
    // The short page's section wins, and is printed as the scope of its lines.
    assert_eq!(lines.next(), Some("caching.txt=6=Eviction"), "{out}");
    // The heading holds a term too, so it is also a shown match.
    assert_eq!(lines.next(), Some("caching.txt:6:Eviction"), "{out}");
    assert!(
        lines
            .next()
            .unwrap()
            .starts_with("caching.txt:9:The cache evicts"),
        "{out}"
    );
    // JSON names the section and the headings above it.
    let json = raw_stdout(ogrep(dir.path()).args([
        "--no-index",
        "--json",
        "--about",
        "cache timeout eviction",
    ]));
    let scope = json
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "scope")
        .unwrap();
    assert_eq!(scope["data"]["line_number"], 6);
    let names: Vec<&str> = scope["data"]["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Caching", "Eviction"]);
    // Code files are scored and printed as before: no scope.
    fs::write(
        dir.path().join("cache.py"),
        "def cache_timeout():\n    return 1\n",
    )
    .unwrap();
    let code = raw_stdout(ogrep(dir.path()).args([
        "--no-index",
        "--json",
        "--about",
        "cache_timeout",
        "-g",
        "*.py",
    ]));
    assert!(!code.contains("\"type\":\"scope\""), "{code}");
}

#[test]
fn about_prints_a_section_only_above_its_own_lines() {
    let dir = TempDir::new().unwrap();
    // The short subsection holds no match of its own, only its parent's
    // heading does: it must not be printed as the section of line 1.
    let mut page = String::from("# Caching\n");
    for i in 0..40 {
        page.push_str(&format!(
            "Plain words about nothing in particular, line {i}.\n"
        ));
    }
    page.push_str("## Eviction\nshort\n");
    fs::write(dir.path().join("guide.md"), page).unwrap();
    let out = stdout(ogrep(dir.path()).args(["--no-index", "-n", "--about", "caching"]));
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        ["guide.md=1=Caching", "guide.md:1:# Caching"],
        "{out}"
    );
}

#[test]
fn about_reports_whether_its_first_file_stands_out() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("pool.py"),
        "def drain_pool(pool):\n    pool.drain()  # drain the connection pool on shutdown\n",
    )
    .unwrap();
    fs::write(dir.path().join("a.txt"), "retry timeout\n").unwrap();
    fs::write(dir.path().join("b.txt"), "retry timeout\n").unwrap();
    let first_rank = |text: &str| -> serde_json::Value {
        let out = raw_stdout(ogrep(dir.path()).args(["--no-index", "--json", "--about", text]));
        let line = out
            .lines()
            .find(|l| l.contains("\"type\":\"rank\""))
            .unwrap()
            .to_string();
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["data"].clone()
    };
    // One file matches: nothing to confuse it with.
    let sure = first_rank("drain the connection pool");
    assert_eq!(sure["confidence"]["level"], "confident", "{sure}");
    assert_eq!(sure["confidence"]["margin"], 1.0);
    // Two identical files tie, so the first one is a coin toss.
    let tie = first_rank("retry timeout");
    assert_eq!(tie["confidence"]["level"], "unsure", "{tie}");
    assert_eq!(tie["confidence"]["margin"], 0.0);
    // Text output stays the same on stdout; the doubt goes to stderr, unless silenced.
    let out = ogrep(dir.path())
        .args(["--no-index", "--about", "retry timeout", "-l"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a.txt\nb.txt\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("--about: unsure"));
    let quiet = ogrep(dir.path())
        .args([
            "--no-index",
            "--about",
            "retry timeout",
            "-l",
            "--no-messages",
        ])
        .output()
        .unwrap();
    assert!(quiet.stderr.is_empty());
    let sure = ogrep(dir.path())
        .args(["--no-index", "--about", "drain the connection pool", "-l"])
        .output()
        .unwrap();
    assert!(sure.stderr.is_empty());
    // --rank on a name reports nothing new.
    let rank =
        raw_stdout(ogrep(dir.path()).args(["--no-index", "--json", "--rank", "-F", "retry"]));
    assert!(!rank.contains("confidence"));
}

#[test]
fn cards_show_outline_and_matches_within_the_budget() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("pool.py"),
        "\"\"\"Connection pooling for the payments service.\"\"\"\n\nclass Pool:\n    def take(self):\n        raise PoolExhausted()\n\n    def give(self, c):\n        pass\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("RUNBOOK.md"),
        "# Payments runbook\n\n## Pool exhausted\n\nRaise the pool size.\n",
    )
    .unwrap();
    let out = stdout(ogrep(dir.path()).args([
        "--no-index",
        "--rank",
        "--cards",
        "-i",
        "pool exhausted|PoolExhausted",
    ]));
    assert!(
        out.contains("pool.py — Connection pooling for the payments service."),
        "{out:?}"
    );
    assert!(out.contains("      3  class Pool:"), "the outline: {out:?}");
    assert!(
        out.contains("      5>     raise PoolExhausted()"),
        "the match, nested: {out:?}"
    );
    assert!(out.contains("RUNBOOK.md — Payments runbook"), "{out:?}");
    assert!(out.contains("      3>   ## Pool exhausted"), "{out:?}");
    // The same input gives the same cards; a tiny budget still prints the
    // best card, and only that one.
    let again = stdout(ogrep(dir.path()).args([
        "--no-index",
        "--rank",
        "--cards",
        "-i",
        "pool exhausted|PoolExhausted",
    ]));
    assert_eq!(out, again);
    let one = stdout(ogrep(dir.path()).args([
        "--no-index",
        "--rank",
        "--cards=1",
        "-i",
        "pool exhausted|PoolExhausted",
    ]));
    assert_eq!(
        one.lines().filter(|l| !l.starts_with(' ')).count(),
        1,
        "{one:?}"
    );
    // JSON carries the same card.
    let json = raw_stdout(ogrep(dir.path()).args([
        "--no-index",
        "--rank",
        "--cards",
        "--json",
        "PoolExhausted",
    ]));
    assert!(json.contains("\"type\":\"card\""), "{json:?}");
    // Cards replace lines, so line-shaped output is refused, and so is a
    // past revision, whose files a card would not read.
    ogrep(dir.path())
        .args(["--no-index", "--cards", "-l", "Pool"])
        .assert()
        .code(2);
    ogrep(dir.path())
        .args(["--cards", "--at", "HEAD", "Pool"])
        .assert()
        .code(2);
    ogrep(dir.path())
        .args(["--about", "pool exhausted", "--at", "HEAD"])
        .assert()
        .code(2);
}

/// Commit everything in `dir` with the given author and committer time.
fn commit_at(dir: &Path, time: &str, subject: &str) {
    git(dir, &["add", "."]);
    let ok = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", time)
        .env("GIT_COMMITTER_DATE", time)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", subject])
        .status()
        .unwrap()
        .success();
    assert!(ok, "git commit {subject:?}");
}

/// Checkout depends on cart and on payment; frontend on nothing. One commit
/// to cart (tagged `v1`), checkout and frontend (tagged `v2`) in March 2024,
/// then a later one to cart.
fn incident_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    let write = |path: &str, text: &str| {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    };
    write(
        "compose.yaml",
        "services:\n  checkout:\n    depends_on:\n      - cart\n      - payment\n  cart:\n    image: cart\n  payment:\n    image: payment\n  frontend:\n    image: frontend\n",
    );
    for svc in ["cart", "checkout", "payment", "frontend"] {
        write(&format!("src/{svc}/main.go"), "package main\n");
    }
    commit_at(dir.path(), "2024-01-01T12:00:00Z", "base");
    for (svc, time) in [
        ("cart", "2024-03-01T12:00:00Z"),
        ("checkout", "2024-03-02T12:00:00Z"),
        ("frontend", "2024-03-03T12:00:00Z"),
        ("cart", "2024-03-10T12:00:00Z"),
    ] {
        write(
            &format!("src/{svc}/main.go"),
            &format!("package main // {time}\n"),
        );
        commit_at(dir.path(), time, &format!("{svc} at {time}"));
        match (svc, dir.path().join(".git/refs/tags/v1").exists()) {
            ("cart", false) => git(dir.path(), &["tag", "v1"]),
            ("frontend", _) => git(dir.path(), &["tag", "v2"]),
            _ => {}
        }
    }
    dir
}

/// The JSON records of `cmd`, one per output line.
fn records(cmd: &mut Command) -> Vec<serde_json::Value> {
    raw_stdout(cmd)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// `(service, commit subject)` of each `event` record.
fn events(cmd: &mut Command) -> Vec<(String, String)> {
    records(cmd)
        .iter()
        .map(|r| {
            assert_eq!(r["type"], "event");
            let d = &r["data"];
            (
                d["service"].as_str().unwrap().to_string(),
                d["commit"]["subject"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn until_hides_later_commits_from_timeline_and_suspects() {
    let dir = incident_repo();
    let timeline = |args: &[&str]| {
        events(
            ogrep(dir.path())
                .args(["--json", "--timeline", "checkout"])
                .args(args),
        )
    };
    assert_eq!(
        timeline(&[
            "--since",
            "2024-02-01T00:00:00Z",
            "--until",
            "2024-03-05T00:00:00Z"
        ]),
        pairs(&[
            ("checkout", "checkout at 2024-03-02T12:00:00Z"),
            ("cart", "cart at 2024-03-01T12:00:00Z"),
        ])
    );
    // The default seven days count back from --until, not from now.
    assert_eq!(
        timeline(&["--until", "2024-03-12T00:00:00Z"]),
        pairs(&[("cart", "cart at 2024-03-10T12:00:00Z")])
    );
    // An explicit --since is taken as given.
    assert_eq!(
        timeline(&["--since", "2024-02-01T00:00:00Z"]).len(),
        3,
        "without --until every later commit is listed"
    );

    let suspects = |until: &str| {
        records(ogrep(dir.path()).args(["--json", "--suspects", "checkout", "--until", until]))
    };
    let cart = |records: &[serde_json::Value]| {
        records
            .iter()
            .map(|r| r["data"]["suspect"].clone())
            .find(|s| s["service"] == "cart")
            .unwrap()
    };
    // The default 24 hours before --until hold one commit to cart.
    let during = cart(&suspects("2024-03-01T18:00:00Z"));
    let subjects: Vec<&str> = during["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["cart at 2024-03-01T12:00:00Z"]);
    // Before any change, cart has no commits and loses the bonus for them.
    let before = cart(&suspects("2024-02-15T00:00:00Z"));
    assert_eq!(before["changes"].as_array().unwrap().len(), 0);
    assert!(
        during["score"].as_f64() > before["score"].as_f64(),
        "{during} {before}"
    );
}

#[test]
fn until_is_refused_where_it_means_nothing() {
    let dir = incident_repo();
    ogrep(dir.path())
        .args(["--until", "2024-03-05", "--blast", "cart"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--since and --until need"));
    ogrep(dir.path())
        .args(["--timeline", "checkout", "--since", "2024-03-05T00:00:00Z"])
        .args(["--until", "2024-03-01T00:00:00Z"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--since is later than --until"));
}

#[test]
fn timeline_takes_a_list_of_services() {
    let dir = incident_repo();
    let timeline = |services: &str| {
        events(ogrep(dir.path()).args([
            "--json",
            "--timeline",
            services,
            "--since",
            "2024-02-01T00:00:00Z",
        ]))
    };
    let cart_late = ("cart", "cart at 2024-03-10T12:00:00Z");
    let frontend = ("frontend", "frontend at 2024-03-03T12:00:00Z");
    let checkout = ("checkout", "checkout at 2024-03-02T12:00:00Z");
    let cart_early = ("cart", "cart at 2024-03-01T12:00:00Z");
    // One name: the service and what it depends on directly.
    assert_eq!(
        timeline("checkout"),
        pairs(&[cart_late, checkout, cart_early])
    );
    assert_eq!(
        timeline("checkout,frontend"),
        pairs(&[cart_late, frontend, checkout, cart_early])
    );
    // Cart is asked for and is a dependency of checkout: listed once.
    assert_eq!(
        timeline("checkout, cart,checkout"),
        pairs(&[cart_late, checkout, cart_early])
    );
    ogrep(dir.path())
        .args(["--timeline", "checkout,nosuch"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("no service named \"nosuch\""));
}

#[test]
fn csharp_stack_frames_resolve_to_repository_files() {
    let store = "namespace Shop.Cart;\n\npublic class CartStore\n{\n    public void AddItem(string userId)\n    {\n        throw new InvalidOperationException(\"boom\");\n    }\n}\n";
    let program = "namespace Shop.Cart;\n\npublic static class Program\n{\n    public static void Main()\n    {\n        new CartStore().AddItem(\"u\");\n    }\n}\n";
    let dir = repo(&[
        ("src/cart/src/cartstore/CartStore.cs", store),
        ("src/cart/src/Program.cs", program),
    ]);
    // A container path, then a Windows build path.
    fs::write(
        dir.path().join("trace.txt"),
        "Unhandled exception. System.InvalidOperationException: boom\n   at Shop.Cart.CartStore.AddItem(String userId) in /usr/src/app/src/cartstore/CartStore.cs:line 7\n   at Shop.Cart.Program.Main() in C:\\build\\cart\\src\\Program.cs:line 7\n",
    )
    .unwrap();
    let frames: Vec<(String, u64, String)> =
        records(ogrep(dir.path()).args(["--json", "--stack-trace", "trace.txt"]))
            .iter()
            .map(|r| {
                assert_eq!(r["type"], "frame");
                let d = &r["data"];
                (
                    d["path"].as_str().unwrap().to_string(),
                    d["line_number"].as_u64().unwrap(),
                    d["function"].as_str().unwrap().to_string(),
                )
            })
            .collect();
    assert_eq!(
        frames,
        [
            (
                "src/cart/src/cartstore/CartStore.cs".to_string(),
                7,
                "Shop.Cart.CartStore.AddItem".to_string()
            ),
            (
                "src/cart/src/Program.cs".to_string(),
                7,
                "Shop.Cart.Program.Main".to_string()
            ),
        ]
    );
}

/// The shape of Online Boutique's manifests: a Deployment `frontend` behind a
/// Service of the same name and a second, public Service `frontend-external`.
/// `shared` selects two workloads and `orphan` none.
const BOUTIQUE: &str = "apiVersion: apps/v1
kind: Deployment
metadata:
  name: frontend
spec:
  selector:
    matchLabels:
      app: frontend
  template:
    metadata:
      labels:
        app: frontend
        tier: web
    spec:
      containers:
        - name: server
          env:
            - name: CART_SERVICE_ADDR
              value: cartservice:7070
---
apiVersion: v1
kind: Service
metadata:
  name: frontend
spec:
  type: ClusterIP
  selector:
    app: frontend
---
apiVersion: v1
kind: Service
metadata:
  name: frontend-external
spec:
  type: LoadBalancer
  selector:
    app: frontend
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: cartservice
spec:
  template:
    metadata:
      labels:
        app: cartservice
        tier: web
---
apiVersion: v1
kind: Service
metadata:
  name: cartservice
spec:
  selector:
    app: cartservice
---
apiVersion: v1
kind: Service
metadata:
  name: shared
spec:
  selector:
    tier: web
---
apiVersion: v1
kind: Service
metadata:
  name: orphan
spec:
  selector:
    app: nothing
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: loadgenerator
spec:
  template:
    metadata:
      labels:
        app: loadgenerator
    spec:
      containers:
        - name: main
          env:
            - name: FRONTEND_ADDR
              value: frontend-external:80
";

#[test]
fn a_service_selecting_one_workload_is_its_alias() {
    let dir = repo(&[("kubernetes-manifests/all.yaml", BOUTIQUE)]);
    let entities = records(ogrep(dir.path()).args(["--json", "--entities", "."]));
    let ids: Vec<&str> = entities
        .iter()
        .map(|r| r["data"]["entity"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "cartservice",
            "frontend",
            "loadgenerator",
            "orphan",
            "shared"
        ]
    );
    let entity = |id: &str| {
        let record = entities
            .iter()
            .find(|r| r["data"]["entity"]["id"] == id)
            .unwrap();
        &record["data"]
    };
    assert_eq!(
        entity("frontend")["entity"]["aliases"],
        serde_json::json!(["frontend-external"])
    );
    // A Deployment and Service of one name stay one entity without aliases.
    assert!(
        entity("cartservice")["entity"].get("aliases").is_none(),
        "{}",
        entity("cartservice")
    );
    // An address naming the Service is a call to the workload.
    let calls: Vec<&str> = entity("loadgenerator")["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["to"].as_str().unwrap())
        .collect();
    assert_eq!(calls, ["frontend"]);

    let text = stdout(ogrep(dir.path()).args(["--entities", "frontend-external"]));
    assert_eq!(
        text.trim_end(),
        "service  frontend                 kubernetes-manifests/all.yaml:4  -> cartservice  (also frontend-external)"
    );
    let blast = records(ogrep(dir.path()).args(["--json", "--blast", "frontend-external"]));
    let dependents: Vec<(&str, &str)> = blast
        .iter()
        .map(|r| {
            (
                r["data"]["target"].as_str().unwrap(),
                r["data"]["service"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(dependents, [("frontend", "loadgenerator")]);
}

#[test]
fn a_revision_bounds_timeline_and_suspects_by_ancestry() {
    let dir = incident_repo();
    // A commit off main, later than the one to checkout on main.
    git(dir.path(), &["checkout", "-q", "-b", "side", "v1"]);
    fs::write(dir.path().join("src/payment/main.go"), "package side\n").unwrap();
    commit_at(dir.path(), "2024-03-02T18:00:00Z", "payment on side");
    git(dir.path(), &["checkout", "-q", "main"]);

    let timeline = |services: &str, args: &[&str]| {
        events(
            ogrep(dir.path())
                .args(["--json", "--timeline", services])
                .args(args),
        )
    };
    let checkout = ("checkout", "checkout at 2024-03-02T12:00:00Z");
    let frontend = ("frontend", "frontend at 2024-03-03T12:00:00Z");
    let cart_early = ("cart", "cart at 2024-03-01T12:00:00Z");
    let cart_late = ("cart", "cart at 2024-03-10T12:00:00Z");
    // v1..v2, however long ago: the range, not the default window.
    assert_eq!(
        timeline("checkout,frontend", &["--since", "v1", "--until", "v2"]),
        pairs(&[frontend, checkout])
    );
    assert_eq!(
        timeline("checkout", &["--since", "v1"]),
        pairs(&[cart_late, checkout])
    );
    // What `side` reaches, not what is older than it.
    assert_eq!(
        timeline("checkout", &["--since", "v1", "--until", "side"]),
        pairs(&[("payment", "payment on side")])
    );
    // A revision and a time together.
    assert_eq!(
        timeline(
            "checkout,frontend",
            &["--since", "2024-03-02T18:00:00Z", "--until", "v2"]
        ),
        pairs(&[frontend])
    );
    assert_eq!(
        timeline(
            "checkout,frontend",
            &["--since", "v1", "--until", "2024-03-02T18:00:00Z"]
        ),
        pairs(&[checkout])
    );
    // Without --since: the seven days before v2's commit.
    assert_eq!(
        timeline("checkout", &["--until", "v2"]),
        pairs(&[checkout, cart_early])
    );

    let suspects = records(ogrep(dir.path()).args([
        "--json",
        "--suspects",
        "checkout",
        "--since",
        "v1",
        "--until",
        "v2",
    ]));
    let changed: Vec<(&str, usize)> = suspects
        .iter()
        .map(|r| &r["data"]["suspect"])
        .map(|s| {
            (
                s["service"].as_str().unwrap(),
                s["changes"].as_array().unwrap().len(),
            )
        })
        .collect();
    assert_eq!(changed, [("checkout", 1), ("cart", 0), ("payment", 0)]);
}

/// The commit at `rev`.
fn rev(dir: &Path, rev: &str) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn dated_entities_carry_the_commit_since_which_the_files_hold_them() {
    let compose = |deps: &[&str], extra: &str| {
        let list: String = deps.iter().map(|d| format!("      - {d}\n")).collect();
        format!(
            "services:\n  web:\n    image: web\n    depends_on:\n{list}  api:\n    image: api\n  fraud:\n    image: fraud\n{extra}"
        )
    };
    let dir = repo(&[
        ("compose.yaml", &compose(&["api"], "")),
        (".github/workflows/ci.yml", "on: push\n"),
        ("compose.yaml", &compose(&["api", "fraud"], "")),
        ("compose.yaml", &compose(&["fraud"], "")),
        ("compose.yaml", &compose(&["api", "fraud"], "")),
    ]);
    fs::write(
        dir.path().join("compose.yaml"),
        compose(&["api", "fraud", "cache"], "  cache:\n    image: cache\n"),
    )
    .unwrap();
    let entities = records(ogrep(dir.path()).args(["--json", "--entities", "web", "--dated"]));
    let web = &entities[0]["data"];
    assert_eq!(web["entity"]["since"]["commit"], rev(dir.path(), "HEAD~4"));
    let since: Vec<(&str, Option<String>)> = web["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let commit = e
                .get("since")
                .map(|s| s["commit"].as_str().unwrap().to_string());
            (e["to"].as_str().unwrap(), commit)
        })
        .collect();
    // Removed and added back, an edge is dated by the commit that added it
    // back; one only the work tree holds has no date.
    assert_eq!(
        since,
        [
            ("api", Some(rev(dir.path(), "HEAD"))),
            ("cache", None),
            ("fraud", Some(rev(dir.path(), "HEAD~2"))),
        ]
    );
    let plain = records(ogrep(dir.path()).args(["--json", "--entities", "web"]));
    assert!(plain[0]["data"]["edges"][0].get("since").is_none());

    let text = stdout(ogrep(dir.path()).args(["--entities", "web", "--dated"]));
    let head = rev(dir.path(), "HEAD");
    assert!(
        text.contains(&format!("api (since {}, ", &head[..7])),
        "{text}"
    );

    let alone = ogrep(dir.path()).args(["--dated", "web"]).output().unwrap();
    assert!(!alone.status.success(), "--dated needs --entities");
}

/// The graph's paths are relative to the root it recorded. Once that root is
/// gone its files cannot be read, which leaves their text out rather than
/// failing the command.
#[test]
fn graph_tools_survive_a_moved_tree() {
    let temp = TempDir::new().unwrap();
    let old = temp.path().join("proj");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("a.rs"), "fn foo() { bar() }\nfn bar() {}\n").unwrap();
    fs::write(old.join("b.rs"), "fn baz() { foo() }\n").unwrap();
    ogrep(&old)
        .args(["index", ".", "--graph"])
        .assert()
        .success();
    let moved = temp.path().join("moved");
    fs::rename(&old, &moved).unwrap();
    for args in [
        &["--near", "-F", "fn foo", "."][..],
        &["--callees", "-F", "foo", "."],
    ] {
        let out = ogrep(&moved).args(args).output().unwrap();
        let err = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}: {err}");
    }
    // Read from where the tree is now, not where it was indexed.
    let callees = stdout(ogrep(&moved).args(["--callees", "-F", "foo", "."]));
    assert!(callees.contains("bar -> a.rs:2"), "{callees}");
}

/// An index built for a subdirectory reads that subdirectory's files.
#[test]
fn graph_tools_read_an_index_built_for_a_subdirectory() {
    let tree = TempDir::new().unwrap();
    fs::create_dir(tree.path().join("src")).unwrap();
    fs::write(
        tree.path().join("src/a.rs"),
        "fn foo() { bar() }\nfn bar() {}\n",
    )
    .unwrap();
    ogrep(tree.path())
        .args(["index", "src", "--index-path", ".ogrep", "--graph"])
        .assert()
        .success();
    // Shown as paths below the searched one, not the index root.
    let callees = stdout(ogrep(tree.path()).args(["--callees", "-F", "foo", "."]));
    assert!(callees.contains("bar -> src/a.rs:2"), "{callees}");
    // Seeds found from the parent are the graph's files below it.
    fs::write(tree.path().join("src/b.rs"), "fn baz() { bar() }\n").unwrap();
    ogrep(tree.path())
        .args(["index", "src", "--index-path", ".ogrep", "--graph"])
        .assert()
        .success();
    let near = stdout(ogrep(tree.path()).args(["--near", "-F", "fn baz", "."]));
    assert!(near.contains("src/a.rs"), "{near}");
}
