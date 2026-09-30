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

fn stdout(cmd: &mut Command) -> String {
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
    let out = stdout(ogrep(dir.path()).args(["--related-tests", "café.py"]));
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
    ];
    for args in refused {
        ogrep(dir.path()).args(args).assert().code(2).stderr(
            predicates::str::contains("cannot be combined")
                .or(predicates::str::contains("takes its subject")),
        );
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
