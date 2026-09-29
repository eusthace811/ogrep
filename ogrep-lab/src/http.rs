//! A small HTTP/1.1 server: loopback only, one thread per connection, every
//! response closes its connection. Standard library only, so the lab adds no
//! network stack to audit.
//!
//! Access control lives here too, since every route needs it:
//! - The `Host` header must name the loopback address and our port. A page on
//!   another site that rebinds its DNS name to 127.0.0.1 sends its own name,
//!   so it is refused (DNS rebinding).
//! - A request must carry the session cookie, which the server sets only when
//!   the launch URL's token is presented. The cookie is `SameSite=Strict`, so a
//!   cross-site page cannot make a browser send it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const MAX_HEAD: usize = 32 * 1024;
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 64;
pub const COOKIE: &str = "ogrep_lab";

pub struct Request {
    pub method: String,
    pub path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    #[cfg(test)]
    pub fn for_test(method: &str, path: &str, query: Vec<(String, String)>) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            query,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// A query parameter, percent-decoded.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// A query parameter that must be present and non-empty.
    pub fn required(&self, name: &str) -> Result<&str, Response> {
        match self.param(name) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => Err(Response::error(400, &format!("missing parameter `{name}`"))),
        }
    }

    /// A flag parameter: present with `1`, `true` or `on`.
    pub fn flag(&self, name: &str) -> bool {
        matches!(self.param(name), Some("1" | "true" | "on"))
    }

    pub fn number(&self, name: &str, default: usize, max: usize) -> usize {
        self.param(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
            .min(max)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?.split(';').find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then_some(v)
        })
    }
}

pub struct Response {
    status: u16,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Response {
    #[cfg(test)]
    pub fn status(&self) -> u16 {
        self.status
    }

    #[cfg(test)]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn json(value: &serde_json::Value) -> Self {
        Self::bytes(200, "application/json", value.to_string().into_bytes())
    }

    pub fn bytes(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            headers: Vec::new(),
            body,
        }
    }

    /// An error as `{"error": message}`, which the web client shows in place.
    pub fn error(status: u16, message: &str) -> Self {
        Self::bytes(
            status,
            "application/json",
            serde_json::json!({ "error": message })
                .to_string()
                .into_bytes(),
        )
    }

    fn redirect(location: &str) -> Self {
        let mut r = Self::bytes(303, "text/plain; charset=utf-8", Vec::new());
        r.headers.push(("Location", location.to_string()));
        r
    }

    fn with_header(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }

    fn write_to(&self, out: &mut impl Write) -> std::io::Result<()> {
        let reason = match self.status {
            200 => "OK",
            303 => "See Other",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Payload Too Large",
            503 => "Service Unavailable",
            _ => "Error",
        };
        let mut head = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
             Content-Security-Policy: default-src 'self'; img-src 'self' data:; style-src 'self'; frame-ancestors 'none'; form-action 'self'\r\n",
            self.status,
            self.content_type,
            self.body.len()
        );
        for (k, v) in &self.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        out.write_all(&self.body)?;
        out.flush()
    }
}

pub type Handler = dyn Fn(&Request) -> Response + Send + Sync;

pub struct Server {
    pub listener: TcpListener,
    pub port: u16,
    pub token: String,
}

impl Server {
    /// Bind to the loopback interface. Port 0 picks a free one.
    pub fn bind(port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            port,
            token: random_token(),
        })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?token={}", self.port, self.token)
    }

    pub fn run(self, handler: Arc<Handler>) {
        let active = Arc::new(AtomicUsize::new(0));
        let token = Arc::new(self.token);
        for stream in self.listener.incoming() {
            let Ok(stream) = stream else { continue };
            if active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                let _ = Response::error(503, "busy").write_to(&mut &stream);
                continue;
            }
            active.fetch_add(1, Ordering::Relaxed);
            let (handler, active, token) = (handler.clone(), active.clone(), token.clone());
            let port = self.port;
            std::thread::spawn(move || {
                let _ = connection(stream, port, &token, &*handler);
                active.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }
}

fn connection(stream: TcpStream, port: u16, token: &str, handler: &Handler) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut reader = BufReader::new(&stream);
    let response = match read_request(&mut reader) {
        Ok(req) => respond(&req, port, token, handler),
        Err(r) => r,
    };
    response.write_to(&mut &stream)
}

fn respond(req: &Request, port: u16, token: &str, handler: &Handler) -> Response {
    let host = req.header("host").unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Response::error(403, "unexpected Host header");
    }
    if req.method == "GET" && req.path == "/" && req.param("token").is_some() {
        // The launch URL: trade the token for a cookie and drop it from the
        // address bar, so it is not kept in history or shown on screen.
        if !constant_time_eq(
            req.param("token").unwrap_or("").as_bytes(),
            token.as_bytes(),
        ) {
            return Response::error(403, "wrong token; use the URL ogrep-lab printed");
        }
        return Response::redirect("/").with_header(
            "Set-Cookie",
            format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict"),
        );
    }
    let authorized = req
        .cookie(COOKIE)
        .is_some_and(|c| constant_time_eq(c.as_bytes(), token.as_bytes()));
    if !authorized {
        return Response::bytes(
            403,
            "text/plain; charset=utf-8",
            b"Open the URL that ogrep-lab printed when it started; it carries the access token.\n"
                .to_vec(),
        );
    }
    if req.method == "POST" && req.header("content-type") != Some("application/json") {
        return Response::error(400, "POST bodies must be application/json");
    }
    handler(req)
}

fn read_request(reader: &mut impl BufRead) -> Result<Request, Response> {
    let bad = |m: &str| Response::error(400, m);
    let mut head = Vec::new();
    loop {
        let before = head.len();
        let n = reader
            .take((MAX_HEAD - head.len()) as u64 + 1)
            .read_until(b'\n', &mut head)
            .map_err(|_| bad("unreadable request"))?;
        if n == 0 {
            return Err(bad("incomplete request"));
        }
        if head.len() > MAX_HEAD {
            return Err(Response::error(413, "request head too large"));
        }
        let line = &head[before..];
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let text = std::str::from_utf8(&head).map_err(|_| bad("request head is not UTF-8"))?;
    let mut lines = text.lines();
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target) = match (first.next(), first.next()) {
        (Some(m), Some(t)) => (m.to_string(), t),
        _ => return Err(bad("malformed request line")),
    };
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut req = Request {
        method,
        path: percent_decode(path),
        query: parse_query(query),
        headers,
        body: Vec::new(),
    };
    let length: usize = req
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if length > MAX_BODY {
        return Err(Response::error(413, "request body too large"));
    }
    if length > 0 {
        req.body.resize(length, 0);
        reader
            .read_exact(&mut req.body)
            .map_err(|_| bad("incomplete body"))?;
    }
    Ok(req)
}

pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 128 random bits as hex. `RandomState` is seeded from the operating system's
/// random source on every platform std supports, and each instance draws new
/// keys, so two of them give 128 bits without a dependency.
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_and_plus() {
        assert_eq!(percent_decode("a%20b+c%2Fd"), "a b c/d");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%e2%9c%93"), "\u{2713}");
    }

    #[test]
    fn parses_a_request_with_a_body() {
        let raw =
            b"POST /api/x?q=a%2Bb&n=3 HTTP/1.1\r\nHost: 127.0.0.1:9\r\nContent-Length: 2\r\n\r\n{}";
        let req = read_request(&mut &raw[..]).ok().unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/api/x");
        assert_eq!(req.param("q"), Some("a+b"));
        assert_eq!(req.number("n", 0, 10), 3);
        assert_eq!(req.body, b"{}");
    }

    #[test]
    fn refuses_foreign_hosts_and_missing_cookies() {
        let handler = |_: &Request| Response::json(&serde_json::json!({"ok": true}));
        let get = |host: &str, cookie: Option<&str>, target: &str| {
            let mut raw = format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n");
            if let Some(c) = cookie {
                raw.push_str(&format!("Cookie: {c}\r\n"));
            }
            raw.push_str("\r\n");
            let req = read_request(&mut raw.as_bytes()).ok().unwrap();
            respond(&req, 9, "t0k", &handler).status
        };
        assert_eq!(get("evil.example:9", Some("ogrep_lab=t0k"), "/api/x"), 403);
        assert_eq!(get("127.0.0.1:9", None, "/api/x"), 403);
        assert_eq!(get("127.0.0.1:9", Some("ogrep_lab=nope"), "/api/x"), 403);
        assert_eq!(
            get("127.0.0.1:9", Some("a=b; ogrep_lab=t0k"), "/api/x"),
            200
        );
        assert_eq!(get("localhost:9", None, "/?token=t0k"), 303);
        assert_eq!(get("localhost:9", None, "/?token=bad"), 403);
    }

    #[test]
    fn tokens_are_unique() {
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 32);
    }
}
