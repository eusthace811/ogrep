//! Services, their dependencies and alerts, read from the operations files a
//! repository already has: Docker Compose files, Kubernetes manifests,
//! Backstage `catalog-info.yaml`, Prometheus alert rules, and `.env` files.
//!
//! Every dependency edge records where it came from:
//!
//! - **declared**: the file states it (`depends_on`, Backstage `dependsOn`);
//! - **inferred**: a service's configuration points at another service, such as
//!   `CART_ADDR=cart:7070` or `http://payment:8080`, after expanding `${VAR}`
//!   references through `.env` files.
//!
//! and the file and line that are its evidence. The model answers blast radius
//! (who depends on a service, directly or not) and suspects (which services
//! best explain a set of failing ones, weighted by recent changes).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    Declared,
    Inferred,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Location {
    pub file: String,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entity {
    pub kind: String,
    pub id: String,
    pub locations: Vec<Location>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// `depends_on`, `calls`, `telemetry` (where a service sends traces, metrics
    /// or logs), `observes` (what a telemetry collector scrapes), or `alerts_on`.
    pub relation: String,
    pub provenance: Provenance,
    pub evidence: Location,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct OpsModel {
    pub entities: Vec<Entity>,
    pub edges: Vec<Edge>,
    /// Service -> directories holding its code or configuration, for
    /// attributing commits.
    pub paths: BTreeMap<String, Vec<String>>,
}

fn is_yaml(path: &str) -> bool {
    path.ends_with(".yaml") || path.ends_with(".yml")
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_compose(path: &str) -> bool {
    let name = file_name(path);
    is_yaml(name) && (name.starts_with("compose") || name.starts_with("docker-compose"))
}

/// Variables from `.env` files, with `${VAR}` and `${VAR:-default}` expanded.
fn env_vars(root: &Path, paths: &[String]) -> HashMap<String, String> {
    let mut raw = HashMap::new();
    for p in paths
        .iter()
        .filter(|p| file_name(p) == ".env" || file_name(p).ends_with(".env"))
    {
        let Ok(text) = crate::rooted::read_indexed_to_string(root, p) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                raw.entry(k.trim().to_string())
                    .or_insert_with(|| v.to_string());
            }
        }
    }
    let keys: Vec<String> = raw.keys().cloned().collect();
    let mut out = HashMap::new();
    for k in keys {
        out.insert(k.clone(), expand(&raw[&k], &raw, 0));
    }
    out
}

fn expand(value: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    if depth > 8 || !value.contains('$') {
        return value.to_string();
    }
    let mut out = String::new();
    let mut rest = value;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let (name, default, used) = if let Some(body) = after.strip_prefix('{') {
            match body.find('}') {
                Some(end) => {
                    let inner = &body[..end];
                    let (n, d) = inner
                        .split_once(":-")
                        .map_or((inner, None), |(n, d)| (n, Some(d)));
                    (n, d, end + 2)
                }
                None => ("", None, 1),
            }
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            (&after[..end], None, end)
        };
        match vars.get(name) {
            Some(v) => out.push_str(&expand(v, vars, depth + 1)),
            None => out.push_str(default.unwrap_or("")),
        }
        rest = &after[used.min(after.len())..];
    }
    out.push_str(rest);
    out
}

/// Host names a configuration value points at: `cart:7070`,
/// `http://payment:8080/x`, `grpc://checkout.shop.svc.cluster.local`.
fn hosts(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in value.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let part = part.trim_matches(|c| c == '"' || c == '\'');
        let rest = part.split_once("://").map_or(part, |(_, r)| r);
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let host = rest.split(['/', '?']).next().unwrap_or("");
        let host = host.split(':').next().unwrap_or("");
        let host = host.strip_suffix(".svc.cluster.local").unwrap_or(host);
        // `svc.namespace` -> `svc`
        let host = host.split('.').next().unwrap_or("");
        if !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            out.push(host.to_ascii_lowercase());
        }
    }
    out
}

/// 1-based line of the first line after `from` (0-based) whose trimmed text
/// starts with `needle`.
fn find_line(lines: &[&str], from: usize, until: usize, needle: &str) -> Option<usize> {
    (from..until.min(lines.len()))
        .find(|&i| {
            lines[i]
                .trim_start()
                .trim_start_matches("- ")
                .starts_with(needle)
        })
        .map(|i| i + 1)
}

/// A configuration value of a service: `(line, variable name, expanded value)`.
type EnvRef = (usize, String, String);

struct Builder<'a> {
    model: OpsModel,
    entity_index: HashMap<(String, String), usize>,
    env: &'a HashMap<String, String>,
}

impl Builder<'_> {
    fn entity(&mut self, kind: &str, id: &str, file: &str, line: usize) {
        let key = (kind.to_string(), id.to_string());
        let loc = Location {
            file: file.to_string(),
            line,
        };
        match self.entity_index.get(&key) {
            Some(&i) => {
                if !self.model.entities[i].locations.contains(&loc) {
                    self.model.entities[i].locations.push(loc);
                }
            }
            None => {
                self.entity_index.insert(key, self.model.entities.len());
                self.model.entities.push(Entity {
                    kind: kind.to_string(),
                    id: id.to_string(),
                    locations: vec![loc],
                });
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn edge(
        &mut self,
        from: &str,
        to: &str,
        relation: &str,
        provenance: Provenance,
        file: &str,
        line: usize,
        text: &str,
    ) {
        if from == to {
            return;
        }
        let edge = Edge {
            from: from.to_string(),
            to: to.to_string(),
            relation: relation.to_string(),
            provenance,
            evidence: Location {
                file: file.to_string(),
                line,
            },
            text: text.trim().to_string(),
        };
        if !self.model.edges.contains(&edge) {
            self.model.edges.push(edge);
        }
    }

    fn compose(
        &mut self,
        file: &str,
        text: &str,
        services_seen: &mut Vec<(String, String, Vec<EnvRef>)>,
    ) {
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(text) else {
            return;
        };
        let Some(services) = doc.get("services").and_then(|s| s.as_mapping()) else {
            return;
        };
        let lines: Vec<&str> = text.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.starts_with("services:"))
            .unwrap_or(0);
        // Service keys sit at one indentation under `services:`; a service name
        // also appears deeper, in other services' `depends_on` lists.
        let child_indent = lines
            .iter()
            .skip(start + 1)
            .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .map_or(2, |l| l.len() - l.trim_start().len());
        for (name, spec) in services {
            let Some(name) = name.as_str() else { continue };
            let key = format!("{name}:");
            let svc_line = (start + 1..lines.len())
                .find(|&i| {
                    let l = lines[i];
                    l.len() - l.trim_start().len() == child_indent
                        && l.trim_start().starts_with(&key)
                })
                .map_or(1, |i| i + 1);
            self.entity("service", name, file, svc_line);
            // The block ends at the next line indented like the service key.
            let indent = lines[svc_line - 1].len() - lines[svc_line - 1].trim_start().len();
            let end = (svc_line..lines.len())
                .find(|&i| {
                    let l = lines[i];
                    !l.trim().is_empty()
                        && !l.trim_start().starts_with('#')
                        && l.len() - l.trim_start().len() <= indent
                })
                .unwrap_or(lines.len());
            let mut env_refs = Vec::new();
            if let Some(deps) = spec.get("depends_on") {
                let names: Vec<String> = match deps {
                    serde_yaml::Value::Sequence(s) => s
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    serde_yaml::Value::Mapping(m) => m
                        .keys()
                        .filter_map(|k| k.as_str().map(String::from))
                        .collect(),
                    _ => Vec::new(),
                };
                let dep_line = find_line(&lines, svc_line, end, "depends_on").unwrap_or(svc_line);
                for dep in names {
                    let line = find_line(&lines, dep_line, end, &dep).unwrap_or(dep_line);
                    self.edge(
                        name,
                        &dep,
                        "depends_on",
                        Provenance::Declared,
                        file,
                        line,
                        lines[line - 1],
                    );
                }
            }
            if let Some(env) = spec.get("environment") {
                let pairs: Vec<(String, Option<String>)> = match env {
                    serde_yaml::Value::Sequence(s) => s
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(|e| match e.split_once('=') {
                            Some((k, v)) => (k.to_string(), Some(v.to_string())),
                            None => (e.to_string(), None),
                        })
                        .collect(),
                    serde_yaml::Value::Mapping(m) => m
                        .iter()
                        .filter_map(|(k, v)| {
                            Some((
                                k.as_str()?.to_string(),
                                v.as_str()
                                    .map(String::from)
                                    .or_else(|| v.as_i64().map(|n| n.to_string())),
                            ))
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                for (k, v) in pairs {
                    let value = match v {
                        Some(v) => expand(&v, self.env, 0),
                        None => self.env.get(&k).cloned().unwrap_or_default(),
                    };
                    let line = find_line(&lines, svc_line, end, &k).unwrap_or(svc_line);
                    env_refs.push((line, k.clone(), value));
                }
            }
            services_seen.push((name.to_string(), file.to_string(), env_refs));
            if let Some(ctx) = spec
                .get("build")
                .and_then(|b| b.get("context"))
                .and_then(|c| c.as_str())
            {
                let dir = ctx.trim_start_matches("./").trim_end_matches('/');
                if !dir.is_empty() && dir != "." {
                    self.model
                        .paths
                        .entry(name.to_string())
                        .or_default()
                        .push(dir.to_string());
                }
            }
        }
    }

    fn kubernetes(
        &mut self,
        file: &str,
        text: &str,
        services_seen: &mut Vec<(String, String, Vec<EnvRef>)>,
    ) {
        const WORKLOADS: &[&str] = &[
            "Deployment",
            "StatefulSet",
            "DaemonSet",
            "Service",
            "Job",
            "CronJob",
            "ReplicaSet",
            "Pod",
        ];
        let lines: Vec<&str> = text.lines().collect();
        for doc in serde_yaml::Deserializer::from_str(text) {
            let Ok(doc) = serde_yaml::Value::deserialize(doc) else {
                continue;
            };
            let Some(kind) = doc.get("kind").and_then(|k| k.as_str()) else {
                continue;
            };
            if !WORKLOADS.contains(&kind) {
                continue;
            }
            let Some(name) = doc
                .get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(|n| n.as_str())
            else {
                continue;
            };
            let line = find_line(&lines, 0, lines.len(), &format!("name: {name}")).unwrap_or(1);
            self.entity("service", name, file, line);
            let mut env_refs = Vec::new();
            // A pod spec sits at spec (Pod), spec.template.spec (Deployment
            // and the like) or spec.jobTemplate.spec.template.spec (CronJob).
            fn pod(s: &serde_yaml::Value) -> Option<&serde_yaml::Value> {
                let s = s
                    .get("jobTemplate")
                    .and_then(|j| j.get("spec"))
                    .unwrap_or(s);
                s.get("template").and_then(|t| t.get("spec")).or(Some(s))
            }
            let containers = doc
                .get("spec")
                .and_then(pod)
                .and_then(|s| s.get("containers"))
                .and_then(|c| c.as_sequence())
                .cloned()
                .unwrap_or_default();
            for c in containers {
                for e in c
                    .get("env")
                    .and_then(|e| e.as_sequence())
                    .cloned()
                    .unwrap_or_default()
                {
                    if let Some(v) = e.get("value").and_then(|v| v.as_str()) {
                        let key = e.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        let l = find_line(&lines, line, lines.len(), &format!("name: {key}"))
                            .unwrap_or(line);
                        env_refs.push((l, key.to_string(), expand(v, self.env, 0)));
                    }
                }
            }
            services_seen.push((name.to_string(), file.to_string(), env_refs));
        }
    }

    fn backstage(&mut self, file: &str, text: &str) {
        let lines: Vec<&str> = text.lines().collect();
        for doc in serde_yaml::Deserializer::from_str(text) {
            let Ok(doc) = serde_yaml::Value::deserialize(doc) else {
                continue;
            };
            let (Some(kind), Some(name)) = (
                doc.get("kind").and_then(|k| k.as_str()),
                doc.get("metadata")
                    .and_then(|m| m.get("name"))
                    .and_then(|n| n.as_str()),
            ) else {
                continue;
            };
            let kind = match kind {
                "Component" => "service",
                other => other,
            }
            .to_ascii_lowercase();
            let line = find_line(&lines, 0, lines.len(), &format!("name: {name}")).unwrap_or(1);
            self.entity(&kind, name, file, line);
            for field in ["dependsOn", "consumesApis"] {
                for dep in doc
                    .get("spec")
                    .and_then(|s| s.get(field))
                    .and_then(|d| d.as_sequence())
                    .cloned()
                    .unwrap_or_default()
                {
                    let Some(dep) = dep.as_str() else { continue };
                    let target = dep
                        .rsplit(':')
                        .next()
                        .unwrap_or(dep)
                        .rsplit('/')
                        .next()
                        .unwrap_or(dep);
                    let l = find_line(&lines, 0, lines.len(), dep).unwrap_or(line);
                    self.edge(
                        name,
                        target,
                        "depends_on",
                        Provenance::Declared,
                        file,
                        l,
                        lines[l - 1],
                    );
                }
            }
        }
    }

    /// Read a Prometheus rules file; false when `text` is not one (a
    /// multi-document manifest that merely mentions `groups:` and `alert:`).
    fn prometheus(&mut self, file: &str, text: &str) -> bool {
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(text) else {
            return false;
        };
        let Some(groups) = doc.get("groups").and_then(|g| g.as_sequence()) else {
            return false;
        };
        let lines: Vec<&str> = text.lines().collect();
        let label =
            regex::Regex::new(r#"(?:job|service|service_name|app)\s*=~?\s*"([A-Za-z0-9_.-]+)""#)
                .unwrap();
        for g in groups {
            for r in g
                .get("rules")
                .and_then(|r| r.as_sequence())
                .cloned()
                .unwrap_or_default()
            {
                let Some(alert) = r.get("alert").and_then(|a| a.as_str()) else {
                    continue;
                };
                let line =
                    find_line(&lines, 0, lines.len(), &format!("alert: {alert}")).unwrap_or(1);
                self.entity("alert", alert, file, line);
                let expr = r.get("expr").and_then(|e| e.as_str()).unwrap_or("");
                for cap in label.captures_iter(expr) {
                    self.edge(
                        alert,
                        &cap[1],
                        "alerts_on",
                        Provenance::Declared,
                        file,
                        line,
                        &format!("expr: {expr}"),
                    );
                }
            }
        }
        true
    }
}

use serde::Deserialize;

/// Read every operations file among `paths` (relative to `root`).
pub fn scan(root: &Path, paths: &[String]) -> OpsModel {
    let env = env_vars(root, paths);
    let mut b = Builder {
        model: OpsModel::default(),
        entity_index: HashMap::new(),
        env: &env,
    };
    let mut services_seen = Vec::new();
    for path in paths.iter().filter(|p| is_yaml(p)) {
        let Ok(text) = crate::rooted::read_indexed_to_string(root, path) else {
            continue;
        };
        if text.len() > 4 << 20 {
            continue;
        }
        if is_compose(path) {
            b.compose(path, &text, &mut services_seen);
        } else if file_name(path).starts_with("catalog-info") {
            b.backstage(path, &text);
        } else if text.contains("groups:") && text.contains("alert:") && b.prometheus(path, &text) {
        } else if text.contains("kind:") && text.contains("apiVersion:") {
            b.kubernetes(path, &text, &mut services_seen);
        }
    }
    // Inferred edges: configuration values naming another known service.
    let known: HashSet<String> = b
        .model
        .entities
        .iter()
        .filter(|e| e.kind == "service")
        .map(|e| e.id.to_ascii_lowercase())
        .collect();
    for (svc, file, refs) in services_seen {
        let lines: Vec<String> = crate::rooted::read_indexed_to_string(root, &file)
            .map(|t| t.lines().map(String::from).collect())
            .unwrap_or_default();
        for (line, key, value) in refs {
            // Where a service sends its telemetry is not something it needs to
            // work.
            let relation = if key.starts_with("OTEL_") {
                "telemetry"
            } else {
                "calls"
            };
            for host in hosts(&value) {
                if known.contains(&host) && host != svc.to_ascii_lowercase() {
                    let text = lines.get(line - 1).cloned().unwrap_or_default();
                    b.edge(
                        &svc,
                        &host,
                        relation,
                        Provenance::Inferred,
                        &file,
                        line,
                        &format!("{} -> {value}", text.trim()),
                    );
                }
            }
        }
    }
    // A telemetry sink (a collector) points at the services it scrapes; those
    // references observe them rather than depend on them.
    let sinks: HashSet<String> = b
        .model
        .edges
        .iter()
        .filter(|e| e.relation == "telemetry")
        .map(|e| e.to.clone())
        .collect();
    for e in &mut b.model.edges {
        if e.provenance == Provenance::Inferred && e.relation == "calls" && sinks.contains(&e.from)
        {
            e.relation = "observes".to_string();
        }
    }
    // Code directories named after services, for commit attribution.
    let dirs: BTreeSet<String> = paths
        .iter()
        .filter_map(|p| {
            let mut parts = p.split('/');
            let (a, b) = (parts.next()?, parts.next()?);
            parts.next()?;
            Some(format!("{a}/{b}"))
        })
        .collect();
    for svc in &known {
        for dir in &dirs {
            if dir.rsplit('/').next() == Some(svc.as_str()) {
                b.model
                    .paths
                    .entry(svc.clone())
                    .or_default()
                    .push(dir.clone());
            }
        }
    }
    for dirs in b.model.paths.values_mut() {
        dirs.sort();
        dirs.dedup();
    }
    b.model
        .entities
        .sort_by(|x, y| (&x.kind, &x.id).cmp(&(&y.kind, &y.id)));
    b.model.edges.sort_by(|x, y| {
        (&x.from, &x.to, &x.relation, x.provenance).cmp(&(
            &y.from,
            &y.to,
            &y.relation,
            y.provenance,
        ))
    });
    b.model
}

impl OpsModel {
    /// Edges a failure travels along: declared dependencies and calls, not
    /// telemetry, observation or alerting.
    fn depends(&self) -> impl Iterator<Item = &Edge> {
        self.edges
            .iter()
            .filter(|e| e.relation == "depends_on" || e.relation == "calls")
    }

    /// Services that depend on `target`, directly or not, by depth, each with
    /// the edge that reached it.
    pub fn blast(&self, target: &str) -> Vec<(usize, &Edge)> {
        let mut seen: HashSet<&str> = HashSet::from([target]);
        let mut queue = VecDeque::from([(target, 0usize)]);
        let mut out = Vec::new();
        while let Some((node, depth)) = queue.pop_front() {
            let mut next: Vec<&Edge> = self.depends().filter(|e| e.to == node).collect();
            next.sort_by(|a, b| (&a.from, a.provenance).cmp(&(&b.from, b.provenance)));
            for e in next {
                if seen.insert(e.from.as_str()) {
                    out.push((depth + 1, e));
                    queue.push_back((e.from.as_str(), depth + 1));
                }
            }
        }
        out
    }

    /// Everything `svc` depends on, directly or not, with distances.
    pub fn dependencies(&self, svc: &str) -> HashMap<String, usize> {
        let mut dist = HashMap::from([(svc.to_string(), 0usize)]);
        let mut queue = VecDeque::from([svc.to_string()]);
        while let Some(node) = queue.pop_front() {
            let d = dist[&node];
            for e in self.depends().filter(|e| e.from == node) {
                if !dist.contains_key(&e.to) {
                    dist.insert(e.to.clone(), d + 1);
                    queue.push_back(e.to.clone());
                }
            }
        }
        dist
    }

    pub fn services(&self) -> Vec<&str> {
        self.entities
            .iter()
            .filter(|e| e.kind == "service")
            .map(|e| e.id.as_str())
            .collect()
    }
}

/// A ranked explanation for a set of failing services.
#[derive(Clone, Debug, Serialize)]
pub struct Suspect {
    pub service: String,
    pub score: f64,
    /// Failing services that are this service or depend on it.
    pub explains: Vec<String>,
    /// Recent commits attributed to this service.
    pub changes: Vec<Change>,
    /// Mean dependency distance from the failing services it explains.
    pub distance: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    pub sha: String,
    pub time: i64,
    pub subject: String,
    pub files: Vec<String>,
}

/// Commits since `since` (UNIX time) and the files each touched.
pub fn recent_changes(root: &Path, since: i64) -> Vec<Change> {
    let out = crate::history::git(root)
        .args([
            "log",
            "--no-merges",
            &format!("--since={since}"),
            "--format=%x01%H %ct %s",
            "--name-only",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .split('\x01')
        .filter_map(|block| {
            let mut lines = block.lines();
            let header = lines.next()?;
            let mut parts = header.splitn(3, ' ');
            let sha = parts.next()?.to_string();
            let time = parts.next()?.parse().ok()?;
            let subject = parts.next().unwrap_or("").to_string();
            let files = lines
                .filter(|l| !l.trim().is_empty())
                .map(String::from)
                .collect();
            Some(Change {
                sha,
                time,
                subject,
                files,
            })
        })
        .collect()
}

impl OpsModel {
    /// Commits that touched a service's code directories or the lines that
    /// configure it.
    pub fn attribute<'c>(&self, svc: &str, changes: &'c [Change]) -> Vec<&'c Change> {
        let dirs = self.paths.get(svc).cloned().unwrap_or_default();
        let config: Vec<&str> = self
            .entities
            .iter()
            .filter(|e| e.id == svc)
            .flat_map(|e| e.locations.iter().map(|l| l.file.as_str()))
            .collect();
        changes
            .iter()
            .filter(|c| {
                c.files.iter().any(|f| {
                    dirs.iter().any(|d| f.starts_with(&format!("{d}/")))
                        || (config.contains(&f.as_str()) && c.subject.contains(svc))
                })
            })
            .collect()
    }

    /// Rank services that could explain `failing`: a candidate is a failing
    /// service or something one depends on. It scores by the share of failing
    /// services it explains, times `1 + 2 * [changed recently]`, discounted by
    /// distance, so a recently changed common dependency ranks first and a
    /// hub everything depends on does not win without evidence.
    pub fn suspects(&self, failing: &[String], changes: &[Change]) -> Vec<Suspect> {
        let reach: Vec<(String, HashMap<String, usize>)> = failing
            .iter()
            .map(|f| (f.clone(), self.dependencies(f)))
            .collect();
        let mut candidates: BTreeSet<String> = BTreeSet::new();
        for (_, deps) in &reach {
            candidates.extend(deps.keys().cloned());
        }
        let mut out: Vec<Suspect> = candidates
            .into_iter()
            .map(|c| {
                let explains: Vec<(String, usize)> = reach
                    .iter()
                    .filter_map(|(f, deps)| deps.get(&c).map(|&d| (f.clone(), d)))
                    .collect();
                let coverage = explains.len() as f64 / failing.len().max(1) as f64;
                let distance =
                    explains.iter().map(|e| e.1 as f64).sum::<f64>() / explains.len().max(1) as f64;
                let attributed: Vec<Change> =
                    self.attribute(&c, changes).into_iter().cloned().collect();
                let changed = if attributed.is_empty() { 0.0 } else { 1.0 };
                let score = coverage * (1.0 + 2.0 * changed) / (1.0 + 0.1 * distance);
                Suspect {
                    service: c,
                    score,
                    explains: explains.into_iter().map(|e| e.0).collect(),
                    changes: attributed,
                    distance,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.distance.total_cmp(&b.distance))
                .then(a.service.cmp(&b.service))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_variables_and_finds_hosts() {
        let vars: HashMap<String, String> = [
            ("CART_PORT".to_string(), "7070".to_string()),
            ("CART_ADDR".to_string(), "cart:${CART_PORT}".to_string()),
        ]
        .into();
        assert_eq!(expand("${CART_ADDR}", &vars, 0), "cart:7070");
        assert_eq!(expand("${MISSING:-x}", &vars, 0), "x");
        assert_eq!(hosts("http://payment:8080/pay"), vec!["payment"]);
        assert_eq!(
            hosts("checkout.shop.svc.cluster.local:5050"),
            vec!["checkout"]
        );
        assert_eq!(hosts("user:pw@db:5432"), vec!["db"]);
    }

    #[test]
    fn compose_model_blast_and_suspects() {
        let repo = tempfile::tempdir().unwrap();
        let r = repo.path();
        std::fs::write(
            r.join(".env"),
            "CART_PORT=7070\nCART_ADDR=cart:${CART_PORT}\n",
        )
        .unwrap();
        std::fs::write(
            r.join("compose.yaml"),
            "services:\n  frontend:\n    environment:\n      - CHECKOUT=http://checkout:5050\n    depends_on:\n      - checkout\n  checkout:\n    environment:\n      - CART_ADDR\n  cart:\n    image: cart\n  collector:\n    image: otel\n",
        )
        .unwrap();
        let paths = vec![
            ".env".to_string(),
            "compose.yaml".to_string(),
            "src/cart/main.go".to_string(),
        ];
        let m = scan(r, &paths);
        assert_eq!(
            m.services(),
            vec!["cart", "checkout", "collector", "frontend"]
        );
        assert!(m.edges.iter().any(|e| e.from == "checkout"
            && e.to == "cart"
            && e.provenance == Provenance::Inferred));
        assert!(m.edges.iter().any(|e| e.from == "frontend"
            && e.to == "checkout"
            && e.provenance == Provenance::Declared));
        let blast: Vec<(usize, &str)> = m
            .blast("cart")
            .iter()
            .map(|(d, e)| (*d, e.from.as_str()))
            .collect();
        assert_eq!(blast, vec![(1, "checkout"), (2, "frontend")]);
        // The recently changed common dependency ranks first.
        let changes = vec![Change {
            sha: "abc".into(),
            time: 0,
            subject: "tweak".into(),
            files: vec!["src/cart/main.go".into()],
        }];
        let s = m.suspects(&["frontend".into(), "checkout".into()], &changes);
        assert_eq!(s[0].service, "cart");
        assert_eq!(s[0].explains.len(), 2);
    }
}
