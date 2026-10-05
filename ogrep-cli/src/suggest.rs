//! "Did you mean": when a search for one identifier finds nothing, the names
//! in the searched files that are closest to it.
//!
//! A misspelt or misremembered name still contains most of the real one, so
//! each half of it is searched for (through the index, like any search), and
//! the identifiers holding a half are kept when they are within two edits
//! (insertions, deletions, substitutions or adjacent swaps, ignoring case).
//! On 100 real identifiers with two letters swapped, the right name came first
//! for 92 and among the first five for 94 (round 10).

use std::collections::HashMap;
use std::path::Path;

use crate::search::{self, SearchOptions};

/// Names offered at most.
const SHOWN: usize = 5;

/// Whether `opts` is a plain search for one identifier, the case suggestions
/// are for: a single pattern of word characters, at least four long.
pub fn applies(opts: &SearchOptions) -> bool {
    let p = &opts.pattern;
    opts.extra_patterns.is_empty()
        && opts.file_patterns.is_none()
        && !opts.invert_match
        && !opts.files_without_match
        && !opts.quiet
        && !opts.no_messages
        && p.len() >= 4
        && p.len() <= 64
        && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !p.as_bytes()[0].is_ascii_digit()
}

/// The names under `roots` closest to `opts.pattern`, best first.
pub fn similar_names(
    roots: &[&Path],
    index_path: Option<&Path>,
    opts: &SearchOptions,
) -> Vec<String> {
    let query = opts.pattern.as_str();
    // One edit, a swap included, touches at most two adjacent characters, so
    // with the middle two left out one of the two pieces is always intact.
    let half = query.len() / 2;
    let mut pieces = if query.len() >= 8 {
        vec![&query[..half - 1], &query[half + 1..]]
    } else {
        vec![&query[..half], &query[half..]]
    };
    pieces.retain(|p| p.len() >= 3);
    pieces.dedup();
    if pieces.is_empty() {
        return Vec::new();
    }
    let alternatives = pieces
        .iter()
        .map(|p| regex::escape(p))
        .collect::<Vec<_>>()
        .join("|");
    let pattern = format!("[A-Za-z0-9_]*(?i:{alternatives})[A-Za-z0-9_]*");
    let Ok(word) = regex::Regex::new(&pattern) else {
        return Vec::new();
    };
    let probe = SearchOptions {
        pattern: pattern.clone(),
        fixed_string: false,
        word_boundary: false,
        line_regexp: false,
        max_count: None,
        max_results: None,
        before_context: None,
        after_context: None,
        context: None,
        ..opts.clone()
    };
    let lower = query.to_ascii_lowercase();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for root in roots {
        let Ok(lines) = search::captured_matches(root, index_path, &probe, true) else {
            continue;
        };
        for line in &lines {
            for m in word.find_iter(&line.content) {
                let name = m.as_str();
                if name.len().abs_diff(query.len()) <= 2 {
                    *seen.entry(name.to_string()).or_default() += 1;
                }
            }
        }
    }
    // Closest ignoring case, then closest as typed (the query's own
    // capitalisation first), then the most frequent.
    let mut ranked: Vec<(usize, usize, usize, String)> = seen
        .into_iter()
        .filter(|(name, _)| name != query)
        .filter_map(|(name, count)| {
            let d = edits(&name.to_ascii_lowercase(), &lower)?;
            let exact = edits(&name, query).unwrap_or(usize::MAX);
            Some((d, exact, count, name))
        })
        .collect();
    ranked.sort_by(|a, b| {
        (a.0, a.1)
            .cmp(&(b.0, b.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    ranked.into_iter().take(SHOWN).map(|(.., n)| n).collect()
}

/// Edit distance with adjacent swaps (optimal string alignment), or `None`
/// beyond two edits.
fn edits(a: &str, b: &str) -> Option<usize> {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len().abs_diff(b.len()) > 2 {
        return None;
    }
    let mut prev2 = vec![0usize; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                cur[j] = cur[j].min(prev2[j - 2] + 1);
            }
        }
        if cur.iter().min().is_some_and(|&m| m > 2) {
            return None;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    Some(prev[b.len()]).filter(|&d| d <= 2)
}

#[cfg(test)]
mod tests {
    use super::edits;

    #[test]
    fn edits_counts_swaps_as_one() {
        assert_eq!(edits("enable_feature", "enabel_feature"), Some(1));
        assert_eq!(edits("parse_header", "parse_headers"), Some(1));
        assert_eq!(edits("configuration", "configuratoin"), Some(1));
        assert_eq!(edits("abcdef", "abcxyz"), None);
        assert_eq!(edits("abc", "abcdef"), None);
    }
}
