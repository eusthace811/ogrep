//! Deterministic query samples drawn from the repository itself, so the
//! evaluations work on any codebase without a hand-made query set.

use std::collections::BTreeSet;

use crate::workspace::Workspace;

/// FNV-1a: a fixed hash, so the same repository gives the same sample on
/// every machine and every run.
pub fn stable_hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Whether a path is a test file, by the usual naming conventions.
pub fn is_test(path: &str) -> bool {
    let p = path.to_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    p.starts_with("test/")
        || p.starts_with("tests/")
        || p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/testing/")
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("tests.py")
}

/// Whether a word is specific enough to be a realistic query: an identifier
/// of at least six characters with an underscore or a lower-to-upper hump.
pub fn is_specific(word: &str) -> bool {
    let b = word.as_bytes();
    b.len() >= 6
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        && (word.trim_matches('_').contains('_')
            || b.windows(2)
                .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase()))
}

/// `n` identifiers: names the graph knows as definitions when there is a
/// graph, else words read from a fixed sample of files.
pub fn identifiers(ws: &Workspace, n: usize) -> Vec<String> {
    let mut words: BTreeSet<String> = BTreeSet::new();
    if let Some(g) = ws.graph() {
        // Names defined outside tests: test names (`test_addition`) are not
        // what anyone searches for.
        for d in &g.graph.defs {
            if d.linkable
                && is_specific(&d.name)
                && !d.name.to_ascii_lowercase().starts_with("test")
                && !is_test(&g.graph.files[d.file as usize])
            {
                words.insert(d.name.clone());
            }
        }
    }
    if words.len() < n
        && let Ok(index) = ws.index()
    {
        let mut paths: Vec<&str> = index.sorted.iter().map(|(p, _)| p.as_str()).collect();
        paths.sort_by_cached_key(|p| stable_hash(p));
        for p in paths.into_iter().filter(|p| !is_test(p)).take(400) {
            let Ok(full) = ws.indexed_file(p) else {
                continue;
            };
            let Ok(data) = std::fs::read(full) else {
                continue;
            };
            if data.len() > 1 << 20 || memchr::memchr(0, &data).is_some() {
                continue;
            }
            let text = String::from_utf8_lossy(&data);
            for w in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if is_specific(w) && !w.to_ascii_lowercase().starts_with("test") {
                    words.insert(w.to_string());
                }
            }
        }
    }
    // A stable sort of the set's alphabetical order breaks hash ties by name.
    let mut words: Vec<String> = words.into_iter().collect();
    words.sort_by_cached_key(|w| stable_hash(w));
    words.truncate(n);
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specific_words() {
        assert!(is_specific("bulk_update"));
        assert!(is_specific("QuerySet"));
        assert!(!is_specific("plainword"));
        assert!(is_specific("getValue"));
        assert!(!is_specific("_private"));
        assert!(!is_specific("short"));
        assert!(!is_specific("9_lives_x"));
    }

    #[test]
    fn recognises_test_files() {
        assert!(is_test("tests/queries/tests.py"));
        assert!(is_test("src/app/cart.spec.ts"));
        assert!(is_test("pkg/server/handler_test.go"));
        assert!(!is_test("django/db/models/query.py"));
    }

    #[test]
    fn hash_is_fixed() {
        assert_eq!(stable_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
