//! Routes: the web client's files, and a JSON API over the workspace.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::evals::{self, Jobs};
use crate::http::{Request, Response};
use crate::workspace::Workspace;
use crate::{files, overview, plan, search};

/// The web client, compiled in: no files to install, nothing fetched. Each
/// entry is `(path, content type, body)`; `/` serves `/index.html`.
macro_rules! assets {
    ($($file:literal: $ty:expr),* $(,)?) => {
        &[$((concat!("/", $file), $ty, include_str!(concat!("../web/", $file)))),*]
    };
}
const JS: &str = "text/javascript; charset=utf-8";
const ASSETS: &[(&str, &str, &str)] = assets![
    "index.html": "text/html; charset=utf-8",
    "app.css": "text/css; charset=utf-8",
    "favicon.svg": "image/svg+xml",
    "app.js": JS,
    "lib.js": JS,
    "charts.js": JS,
    "graph.js": JS,
    "code.js": JS,
    "views/index.js": JS,
    "views/search.js": JS,
    "views/files.js": JS,
    "views/graph.js": JS,
    "views/history.js": JS,
    "views/ops.js": JS,
    "views/evals.js": JS,
];

pub struct App {
    pub ws: Arc<Workspace>,
    pub jobs: Jobs,
}

impl App {
    pub fn handle(&self, req: &Request) -> Response {
        if !req.path.starts_with("/api/") {
            let path = if req.path == "/" {
                "/index.html"
            } else {
                &req.path
            };
            return match (req.method.as_str(), ASSETS.iter().find(|a| a.0 == path)) {
                ("GET", Some(&(path, ty, body))) => {
                    // For working on the client: serve the files from disk so
                    // a reload shows an edit without rebuilding. Only the
                    // listed paths are served, so nothing else is reachable.
                    let from_disk = std::env::var_os("OGREP_LAB_WEB_DIR").and_then(|dir| {
                        std::fs::read(std::path::Path::new(&dir).join(&path[1..])).ok()
                    });
                    Response::bytes(200, ty, from_disk.unwrap_or_else(|| body.into()))
                }
                ("GET", None) => Response::error(404, "not found"),
                _ => Response::error(405, "method not allowed"),
            };
        }
        let result = match (req.method.as_str(), &req.path[5..]) {
            ("GET", route) => self.get(route, req),
            ("POST", route) => self.post(route, req),
            _ => return Response::error(405, "method not allowed"),
        };
        match result {
            Ok(v) => Response::json(&v),
            Err(r) => r,
        }
    }

    fn get(&self, route: &str, req: &Request) -> Result<Value, Response> {
        let ws = &*self.ws;
        match route {
            "overview" => overview::overview(ws).map_err(internal),
            "plan" => plan::explain(ws, &search::pattern(req).map_err(bad)?).map_err(bad),
            "search" => search::search(ws, req).map_err(bad),
            "feature" => search::feature(ws, req).map_err(bad),
            "tree" => files::tree(ws, req.param("dir").unwrap_or("")).map_err(internal),
            "find" => files::find(
                ws,
                req.param("q").unwrap_or(""),
                req.number("limit", 30, 200),
            )
            .map_err(internal),
            "file" => files::file(ws, req.required("path")?).map_err(missing),
            "graph" => files::neighbourhood(
                ws,
                req.required("path")?,
                req.number("per", 12, 40),
                req.number("hops", 1, 2),
            )
            .map_err(missing),
            "central" => files::central(ws, req.number("limit", 40, 500)).map_err(missing),
            "symbols" => files::symbols(ws, req.required("q")?, req.number("limit", 50, 500))
                .map_err(missing),
            "commit" => search::commit(ws, req.required("sha")?).map_err(bad),
            "evals" => Ok(json!({
                "suites": evals::catalogue(ws),
                "runs": evals::list_runs(ws),
                "job": self.jobs.status(),
            })),
            "evals/run" => evals::load_run(ws, req.required("id")?).map_err(missing_str),
            "job" => Ok(self.jobs.status()),
            _ => Err(Response::error(404, "no such endpoint")),
        }
    }

    fn post(&self, route: &str, req: &Request) -> Result<Value, Response> {
        let body: Value = serde_json::from_slice(&req.body).map_err(|e| bad(e.to_string()))?;
        match route {
            "evals/start" => {
                let suite = body["suite"].as_str().unwrap_or("");
                self.jobs
                    .start(self.ws.clone(), suite, body["params"].clone())
                    .map_err(|e| Response::error(409, &e))
            }
            "evals/cancel" => {
                self.jobs.cancel();
                Ok(self.jobs.status())
            }
            "evals/delete" => evals::delete_run(&self.ws, body["id"].as_str().unwrap_or(""))
                .map(|_| json!({ "deleted": true }))
                .map_err(missing_str),
            _ => Err(Response::error(404, "no such endpoint")),
        }
    }
}

fn bad(e: impl ToString) -> Response {
    Response::error(400, &e.to_string())
}

fn missing(e: anyhow::Error) -> Response {
    Response::error(404, &e.to_string())
}

fn missing_str(e: String) -> Response {
    Response::error(404, &e)
}

fn internal(e: anyhow::Error) -> Response {
    Response::error(500, &e.to_string())
}
