//! End-to-end checks against a real index built in a temporary directory:
//! the workspace, the JSON API and the HTTP server with its access rules.
//! Features that run the `ogrep` binary are covered where one is built.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::api::App;
use crate::http::{Request, Server, parse_query};
use crate::plan::{Case, Pattern};
use crate::workspace::Workspace;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    index: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join("src/db")).unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(
        root.join("src/db/query.py"),
        "class QuerySet:\n    def bulk_update(self, objs):\n        return objs\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/db/state.py"),
        "from query import QuerySet\n\ndef reload():\n    QuerySet().bulk_update([])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("docs/guide.txt"),
        "Use bulk_update for many rows.\n",
    )
    .unwrap();
    std::fs::write(root.join("README"), "nothing to see\n").unwrap();
    let index = dir.path().join("idx");
    ogrep_core::builder::build_index(&root, Some(&index), false, false, &[]).unwrap();
    Fixture {
        _dir: dir,
        root,
        index,
    }
}

fn open(f: &Fixture) -> Workspace {
    // No ogrep binary: the in-process features must work without one.
    Workspace::open(
        &f.root,
        Some(&f.index),
        Some(PathBuf::from("/nonexistent/ogrep")),
    )
    .unwrap()
}

fn get(app: &App, path_and_query: &str) -> (u16, Value) {
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    let req = Request::for_test("GET", path, parse_query(query));
    let res = app.handle(&req);
    (
        res.status(),
        serde_json::from_slice(res.body()).unwrap_or(Value::Null),
    )
}

#[test]
fn overview_counts_files_and_storage() {
    let f = fixture();
    let ws = open(&f);
    let o = crate::overview::overview(&ws).unwrap();
    assert_eq!(o["meta"]["files"], 4);
    assert!(
        o["meta"]["sparse_grams"].is_number(),
        "new builds carry sparse grams"
    );
    let parts: Vec<&str> = o["storage"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["part"].as_str().unwrap())
        .collect();
    assert!(parts.contains(&"postings") && parts.contains(&"lookup"));
    assert!(o["history"].is_null() && o["graph"].is_null());
    let dirs: Vec<&str> = o["directories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert!(dirs.contains(&"src/") && dirs.contains(&"(root files)"));
}

#[test]
fn an_unreadable_graph_is_reported_not_hidden() {
    let f = fixture();
    std::fs::create_dir_all(f.index.join("graph")).unwrap();
    std::fs::write(f.index.join("graph/graph.bin"), b"not a graph").unwrap();
    let ws = open(&f);
    let o = crate::overview::overview(&ws).unwrap();
    assert!(o["graph"].is_null());
    let errors = o["load_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].as_str().unwrap().contains("graph"));
}

#[test]
fn plan_explain_narrows_to_the_matching_files() {
    let f = fixture();
    let ws = open(&f);
    let pat = Pattern {
        text: "bulk_update",
        fixed: true,
        case: Case::Sensitive,
        word: false,
    };
    let p = crate::plan::explain(&ws, &pat).unwrap();
    let funnel = &p["funnel"];
    assert_eq!(funnel["files"], 4);
    assert_eq!(funnel["matched"], 3);
    assert_eq!(funnel["lines"], 3);
    assert!(funnel["candidates"].as_u64().unwrap() >= 3);
    assert!(
        !p["trace"].as_array().unwrap().is_empty(),
        "the planner read some list"
    );
    assert_eq!(p["match_all"], false);

    // A pattern with no literal is a full scan, still exact.
    let scan = Pattern {
        text: ".",
        fixed: false,
        case: Case::Smart,
        word: false,
    };
    let p = crate::plan::explain(&ws, &scan).unwrap();
    assert_eq!(p["match_all"], true);
    assert_eq!(p["funnel"]["candidates"], 4);

    let bad = Pattern {
        text: "(unclosed",
        fixed: false,
        case: Case::Smart,
        word: false,
    };
    assert!(crate::plan::explain(&ws, &bad).is_err());
}

#[test]
fn api_lists_finds_and_reads_only_indexed_files() {
    let f = fixture();
    let app = App {
        ws: Arc::new(open(&f)),
        jobs: Default::default(),
    };
    let (status, tree) = get(&app, "/api/tree?dir=src/db");
    assert_eq!(status, 200);
    assert_eq!(tree["files"], serde_json::json!(["query.py", "state.py"]));

    let (_, found) = get(&app, "/api/find?q=query");
    assert_eq!(found["paths"][0], "src/db/query.py");

    let (status, file) = get(&app, "/api/file?path=src/db/query.py");
    assert_eq!(status, 200);
    assert!(file["text"].as_str().unwrap().contains("def bulk_update"));
    assert_eq!(file["graph"], false);

    for escape in ["../idx/meta.json", "/etc/passwd", "src/../../idx/meta.json"] {
        let (status, body) = get(&app, &format!("/api/file?path={escape}"));
        assert_eq!(status, 404, "{escape} must not be readable");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("not an indexed file")
        );
    }

    let (status, _) = get(&app, "/api/graph?path=src/db/query.py");
    assert_eq!(status, 404, "no graph was built");
    let (status, body) = get(&app, "/api/search?q=bulk_update");
    assert_eq!(status, 400, "no ogrep binary: {body}");
    let (status, _) = get(&app, "/api/nope");
    assert_eq!(status, 404);
}

#[test]
fn evaluation_runs_are_listed_and_ids_are_checked() {
    let f = fixture();
    let ws = open(&f);
    let dir = ws.runs_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("20260101-000000-latency.json"),
        r#"{"id":"20260101-000000-latency","suite":"latency","items":[{"query":"x"}],"summary":{}}"#,
    )
    .unwrap();
    let runs = crate::evals::list_runs(&ws);
    assert_eq!(runs[0]["id"], "20260101-000000-latency");
    assert_eq!(runs[0]["item_count"], 1);
    assert!(
        runs[0].get("items").is_none(),
        "the list leaves out per-item rows"
    );
    assert!(crate::evals::load_run(&ws, "../../etc/passwd").is_err());
    assert!(crate::evals::delete_run(&ws, "20260101-000000-latency").is_ok());
    assert_eq!(crate::evals::list_runs(&ws), serde_json::json!([]));
}

#[test]
fn server_trades_the_token_for_a_cookie() {
    let f = fixture();
    let app = Arc::new(App {
        ws: Arc::new(open(&f)),
        jobs: Default::default(),
    });
    let server = Server::bind(0).unwrap();
    let (port, token) = (server.port, server.token.clone());
    std::thread::spawn(move || server.run(Arc::new(move |req| app.handle(req))));

    let request = |target: &str, cookie: Option<&str>| -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut raw = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
        if let Some(c) = cookie {
            raw.push_str(&format!("Cookie: {c}\r\n"));
        }
        raw.push_str("\r\n");
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    assert!(request("/api/overview", None).starts_with("HTTP/1.1 403"));
    let launch = request(&format!("/?token={token}"), None);
    assert!(launch.starts_with("HTTP/1.1 303"));
    assert!(launch.contains(&format!(
        "Set-Cookie: ogrep_lab={token}; Path=/; HttpOnly; SameSite=Strict"
    )));
    let cookie = format!("ogrep_lab={token}");
    let page = request("/", Some(&cookie));
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("<title>ogrep lab</title>"));
    assert!(page.contains("Content-Security-Policy: default-src 'self'"));
    let api = request("/api/overview", Some(&cookie));
    assert!(api.starts_with("HTTP/1.1 200") && api.contains("\"files\":4"));
}
