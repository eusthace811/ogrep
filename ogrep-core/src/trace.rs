//! Stack traces to code: find the frames of a pasted trace in the repository.
//!
//! Recognises the frame formats of Python (`File "x.py", line 12, in f`), Java
//! and Kotlin (`at pkg.Cls.m(Cls.java:12)`), Go (`/src/x.go:12 +0x1f`), Rust
//! (`at src/x.rs:12:5`), JavaScript/TypeScript (`at f (src/x.ts:12:5)`), C#
//! (`at Ns.Cls.M(String a) in /src/Cls.cs:line 12`), and bare `path:line`
//! references. Paths in a trace are often absolute or from
//! another machine; a frame resolves to the indexed file with the longest
//! matching path suffix.

use regex::Regex;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Frame {
    /// Position of the frame in the trace, from the top.
    pub index: usize,
    /// The path as the trace spelled it.
    pub raw_path: String,
    pub line: usize,
    pub function: Option<String>,
}

/// The frames of `trace`, in order.
pub fn frames(trace: &str) -> Vec<Frame> {
    let patterns = [
        // Python
        Regex::new(r#"File "([^"]+)", line (\d+)(?:, in (\S+))?"#).unwrap(),
        // Java / Kotlin / Scala: at a.b.C.m(C.java:12)
        Regex::new(r"at ([\w$.<>]+)\(([\w$.-]+\.(?:java|kt|scala|groovy)):(\d+)\)").unwrap(),
        // JavaScript / TypeScript: at fn (path:12:3)  or  at path:12:3
        // (also `at async ...`, `at new Foo (...)`; file:// URLs are
        // turned into paths first)
        Regex::new(r"at (?:async |new )?(?:(\S+) \()?((?:[A-Za-z]:)?[^\s():]+\.(?:js|mjs|cjs|ts|tsx|jsx)):(\d+)(?::\d+)?\)?").unwrap(),
        // Generic path:line[:col] (Go, Rust, C, Ruby, PHP, compilers)
        Regex::new(r"((?:[A-Za-z]:)?[\w./\\-]+\.(?:go|rs|c|cc|cpp|h|hpp|rb|php|py|cs|swift|m|mm|ex|exs|erl|clj|lua|pl)):(\d+)").unwrap(),
    ];
    // C#: at Ns.Cls.M(String a) in /src/Cls.cs:line 12. The path may hold
    // spaces and a drive letter.
    let csharp = Regex::new(r"at (\S+?) ?\(.*\) in (.+?):line (\d+)").unwrap();
    let file_url = Regex::new(r"file://(?:/([A-Za-z]:))?").unwrap();
    let mut out = Vec::new();
    for line in trace.lines() {
        // file:///C:/app/x.js -> C:/app/x.js, file:///app/x.js -> /app/x.js
        let line = file_url.replace_all(line, "$1");
        let line = line.as_ref();
        let found = if let Some(c) = patterns[0].captures(line) {
            Some((
                c[1].to_string(),
                c[2].parse().unwrap_or(0),
                c.get(3).map(|m| m.as_str().to_string()),
            ))
        } else if let Some(c) = patterns[1].captures(line) {
            // Java names the file only; rebuild a path from the package.
            let qualified = &c[1];
            let file = &c[2];
            let package: Vec<&str> = qualified.split('.').collect();
            let dir = package[..package.len().saturating_sub(2)].join("/");
            let path = if dir.is_empty() {
                file.to_string()
            } else {
                format!("{dir}/{file}")
            };
            Some((path, c[3].parse().unwrap_or(0), Some(qualified.to_string())))
        } else if let Some(c) = csharp.captures(line) {
            Some((
                c[2].to_string(),
                c[3].parse().unwrap_or(0),
                Some(c[1].to_string()),
            ))
        } else if let Some(c) = patterns[2].captures(line) {
            Some((
                c[2].to_string(),
                c[3].parse().unwrap_or(0),
                c.get(1).map(|m| m.as_str().to_string()),
            ))
        } else {
            patterns[3]
                .captures(line)
                .map(|c| (c[1].to_string(), c[2].parse().unwrap_or(0), None))
        };
        if let Some((raw_path, line, function)) = found
            && line > 0
        {
            out.push(Frame {
                index: out.len(),
                raw_path: raw_path.replace('\\', "/"),
                line,
                function,
            });
        }
    }
    out
}

/// The indexed path that best matches a frame's path: the longest common
/// suffix of whole path components, at least the file name.
pub fn resolve<'p>(raw: &str, paths: &'p [String]) -> Option<&'p str> {
    let want: Vec<&str> = raw
        .trim_start_matches("./")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let name = *want.last()?;
    let mut best: Option<(&str, usize)> = None;
    for p in paths {
        if !p.ends_with(name) {
            continue;
        }
        let have: Vec<&str> = p.split('/').collect();
        let common = want
            .iter()
            .rev()
            .zip(have.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if common == 0 {
            continue;
        }
        if best.is_none_or(|(b, n)| common > n || (common == n && p.len() < b.len())) {
            best = Some((p, common));
        }
    }
    best.map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_async_and_constructor_frames_are_read() {
        let trace = "Error: boom\n    at async handler (/app/src/api/routes.ts:12:7)\n    at new Cart (file:///app/src/cart.js:5:3)\n    at async file:///app/src/main.mjs:9:1\n    at f (file:///C:/app/x.js:1:1)\n";
        let found: Vec<(String, usize, Option<String>)> = frames(trace)
            .into_iter()
            .map(|f| (f.raw_path, f.line, f.function))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "/app/src/api/routes.ts".to_string(),
                    12,
                    Some("handler".to_string())
                ),
                ("/app/src/cart.js".to_string(), 5, Some("Cart".to_string())),
                ("/app/src/main.mjs".to_string(), 9, None),
                ("C:/app/x.js".to_string(), 1, Some("f".to_string())),
            ]
        );
    }

    #[test]
    fn csharp_frames_are_read() {
        let trace = "Unhandled exception. System.InvalidOperationException: boom\n   at Shop.Cart.CartStore.AddItemAsync(String userId, Int32 quantity) in /usr/src/app/src/cartstore/CartStore.cs:line 42\n   at Shop.Cart.Program.<Main>d__0.MoveNext() in C:\\src\\My App\\Program.cs:line 7\n   at System.Threading.Tasks.Task.Wait()\n";
        let found: Vec<(String, usize, Option<String>)> = frames(trace)
            .into_iter()
            .map(|f| (f.raw_path, f.line, f.function))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "/usr/src/app/src/cartstore/CartStore.cs".to_string(),
                    42,
                    Some("Shop.Cart.CartStore.AddItemAsync".to_string())
                ),
                (
                    "C:/src/My App/Program.cs".to_string(),
                    7,
                    Some("Shop.Cart.Program.<Main>d__0.MoveNext".to_string())
                ),
            ]
        );
        let paths = [
            "src/cart/src/cartstore/CartStore.cs".to_string(),
            "src/cart/src/Program.cs".to_string(),
            "src/accounting/Program.cs".to_string(),
        ];
        assert_eq!(resolve(&found[0].0, &paths), Some(paths[0].as_str()));
    }

    #[test]
    fn parses_common_frame_formats() {
        let trace = r#"Traceback (most recent call last):
  File "/srv/app/django/db/models/query.py", line 1020, in bulk_update
java.lang.IllegalStateException: boom
    at com.shop.cart.CartService.add(CartService.java:88)
    at handler (/app/src/api/routes.ts:12:7)
goroutine 1 [running]:
main.main()
	/home/ci/src/checkout/main.go:45 +0x1d
thread 'main' panicked at src/lib.rs:7:5
"#;
        let f = frames(trace);
        let got: Vec<(&str, usize)> = f.iter().map(|f| (f.raw_path.as_str(), f.line)).collect();
        assert_eq!(
            got,
            vec![
                ("/srv/app/django/db/models/query.py", 1020),
                ("com/shop/cart/CartService.java", 88),
                ("/app/src/api/routes.ts", 12),
                ("/home/ci/src/checkout/main.go", 45),
                ("src/lib.rs", 7),
            ]
        );
        assert_eq!(f[0].function.as_deref(), Some("bulk_update"));
    }

    #[test]
    fn resolves_by_longest_path_suffix() {
        let paths: Vec<String> = [
            "django/db/models/query.py",
            "django/db/models/sql/query.py",
            "tests/query.py",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            resolve("/srv/app/django/db/models/query.py", &paths),
            Some("django/db/models/query.py")
        );
        assert_eq!(
            resolve("sql/query.py", &paths),
            Some("django/db/models/sql/query.py")
        );
        assert_eq!(resolve("/x/nothere.py", &paths), None);
    }
}
