//! Query plan explain: which keys a pattern turns into, how long each posting
//! list is, the order the planner actually read them in, and how many
//! candidate files survive to verification.
//!
//! The plan is built and run with ogrep-core's own planner, against the index
//! on disk. A recording posting source logs every list the planner loads and
//! every probe it makes, so the trace is the real execution, not a model of it.
//! (A running `ogrep serve` also searches unsaved changes from its overlay,
//! which this view does not include.)

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::Result;
use ogrep_core::PostingEntry;
use ogrep_core::query::{self, PostingSource, QueryPlan, QueryWork, TrigramQuery};
use ogrep_core::reader::IndexReader;
use ogrep_core::sparse;
use rayon::prelude::*;
use serde_json::{Value, json};

use crate::workspace::Workspace;

/// Verification reads at most this many candidate files.
const VERIFY_CAP: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Sensitive,
    Insensitive,
    Smart,
}

pub struct Pattern<'a> {
    pub text: &'a str,
    pub fixed: bool,
    pub case: Case,
    pub word: bool,
}

impl Pattern<'_> {
    pub fn ignores_case(&self) -> bool {
        match self.case {
            Case::Sensitive => false,
            Case::Insensitive => true,
            Case::Smart => !self.text.chars().any(char::is_uppercase),
        }
    }

    /// The regex the verifier runs, with ripgrep's `-w` rule: the match must
    /// start and end at a non-word character or the line's edge.
    fn regex(&self) -> Result<regex::bytes::Regex, String> {
        let body = if self.fixed {
            regex::escape(self.text)
        } else {
            self.text.to_string()
        };
        let body = if self.word {
            format!(r"(?:^|\W)(?:{body})(?:$|\W)")
        } else {
            body
        };
        regex::bytes::RegexBuilder::new(&body)
            .case_insensitive(self.ignores_case())
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string())
    }
}

enum Step {
    Load {
        key: u32,
        postings: usize,
    },
    Probe {
        key: u32,
        candidates: usize,
        hits: usize,
    },
}

/// Passes every call through to the reader and records loads and probes.
struct Recorder<'a> {
    inner: &'a IndexReader,
    steps: RefCell<Vec<Step>>,
}

impl PostingSource for Recorder<'_> {
    fn has_sparse_grams(&self) -> bool {
        PostingSource::has_sparse_grams(self.inner)
    }
    fn gram_dropped(&self, key: u32) -> bool {
        PostingSource::gram_dropped(self.inner, key)
    }
    fn posting_len(&self, key: u32) -> usize {
        self.inner.posting_len(key)
    }
    fn posting_list(&self, key: u32) -> Vec<PostingEntry> {
        let list = self.inner.posting_list(key);
        self.steps.borrow_mut().push(Step::Load {
            key,
            postings: list.len(),
        });
        list
    }
    fn probe(&self, key: u32, candidates: &[u32]) -> Vec<PostingEntry> {
        let hits = self.inner.probe(key, candidates);
        self.steps.borrow_mut().push(Step::Probe {
            key,
            candidates: candidates.len(),
            hits: hits.len(),
        });
        hits
    }
}

pub fn explain(ws: &Workspace, pat: &Pattern) -> Result<Value, String> {
    let index = ws.index().map_err(|e| e.to_string())?;
    let reader = &index.reader;
    let ci = pat.ignores_case();

    let t_plan = Instant::now();
    let plan = if pat.fixed {
        query::build_literal_plan(pat.text, ci)
    } else {
        query::build_query_plan(pat.text, ci)?
    };
    let plan_us = t_plan.elapsed().as_micros();
    let verifier = pat.regex()?;

    let names = gram_names(pat);
    let recorder = Recorder {
        inner: reader,
        steps: RefCell::new(Vec::new()),
    };
    let mut work = QueryWork::default();
    let t_index = Instant::now();
    let candidates = if plan.is_match_all() {
        reader.all_file_ids()
    } else {
        query::execute_plan_indexed(&plan, &recorder, &mut work)
    };
    let index_us = t_index.elapsed().as_micros();
    let steps = recorder.steps.into_inner();

    let touched: HashSet<u32> = steps
        .iter()
        .map(|s| match s {
            Step::Load { key, .. } | Step::Probe { key, .. } => *key,
        })
        .collect();
    let describe = |key: u32| -> Value {
        let (kind, text) = key_text(key, &names);
        json!({ "key": format!("{key:08x}"), "kind": kind, "text": text })
    };
    let trace: Vec<Value> = steps
        .iter()
        .map(|s| match *s {
            Step::Load { key, postings } => {
                let mut v = describe(key);
                v["op"] = json!("load");
                v["postings"] = json!(postings);
                v
            }
            Step::Probe {
                key,
                candidates,
                hits,
            } => {
                let mut v = describe(key);
                v["op"] = json!("probe");
                v["postings"] = json!(reader.posting_len(key));
                v["candidates"] = json!(candidates);
                v["hits"] = json!(hits);
                v
            }
        })
        .collect();

    // Verification: the step that makes the answer exact. Its cost is what the
    // index exists to avoid, so the share of candidates that match is the
    // planner's precision.
    let t_verify = Instant::now();
    let sample = candidates.len() > VERIFY_CAP;
    let checked = &candidates[..candidates.len().min(VERIFY_CAP)];
    let (matched, lines, bytes) = checked
        .par_iter()
        .filter_map(|&id| {
            let rel = reader.file_path(id)?;
            let data = std::fs::read(ogrep_core::hybrid::join_indexed_path(&ws.root, rel)).ok()?;
            let n = count_matching_lines(&verifier, &data);
            Some(((n > 0) as usize, n, data.len()))
        })
        .reduce(|| (0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));
    let verify_ms = t_verify.elapsed().as_secs_f64() * 1000.0;

    Ok(json!({
        "pattern": pat.text,
        "ignore_case": ci,
        "sparse_index": PostingSource::has_sparse_grams(reader),
        "match_all": plan.is_match_all(),
        "plan": node(&plan, reader, &touched, &describe),
        "trace": trace,
        "funnel": {
            "files": reader.num_files(),
            "candidates": candidates.len(),
            "verified": checked.len(),
            "matched": matched,
            "lines": lines,
            "bytes_read": bytes,
            "sample": sample,
        },
        "work": work,
        "timing": { "plan_us": plan_us, "index_us": index_us, "verify_ms": verify_ms },
    }))
}

fn count_matching_lines(re: &regex::bytes::Regex, data: &[u8]) -> usize {
    let mut n = 0;
    let mut last_line_start = usize::MAX;
    for m in re.find_iter(data) {
        let start = memchr::memrchr(b'\n', &data[..m.start()]).map_or(0, |i| i + 1);
        if start != last_line_start {
            n += 1;
            last_line_start = start;
        }
    }
    n
}

fn node(
    plan: &QueryPlan,
    reader: &IndexReader,
    touched: &HashSet<u32>,
    describe: &dyn Fn(u32) -> Value,
) -> Value {
    match plan {
        QueryPlan::And(queries) => {
            let keys: Vec<Value> = queries
                .iter()
                .map(|q: &TrigramQuery| {
                    let mut v = describe(q.hash);
                    let gram = sparse::is_gram_key(q.hash);
                    v["postings"] = json!(reader.posting_len(q.hash));
                    v["used"] = json!(touched.contains(&q.hash));
                    v["fallback"] = json!(q.fallback);
                    v["dropped"] = json!(gram && reader.gram_dropped(q.hash));
                    v
                })
                .collect();
            json!({ "op": "and", "keys": keys })
        }
        QueryPlan::Or(ps) => json!({
            "op": "or",
            "children": ps.iter().map(|p| node(p, reader, touched, describe)).collect::<Vec<_>>(),
        }),
        QueryPlan::All(ps) => json!({
            "op": "all",
            "children": ps.iter().map(|p| node(p, reader, touched, describe)).collect::<Vec<_>>(),
        }),
        QueryPlan::MatchAll => json!({ "op": "scan" }),
    }
}

/// Gram keys are hashes, so their text is recovered by hashing every
/// substring of the pattern that could be a gram. Escapes are removed first so
/// `foo\.bar` also yields the grams of `foo.bar`.
fn gram_names(pat: &Pattern) -> HashMap<u32, String> {
    let mut texts = vec![pat.text.to_string()];
    if !pat.fixed {
        let mut plain = String::new();
        let mut chars = pat.text.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(n) = chars.next() {
                    plain.push(n);
                }
            } else {
                plain.push(c);
            }
        }
        texts.push(plain);
    }
    let mut names = HashMap::new();
    for t in &texts {
        let b = t.as_bytes();
        for start in 0..b.len() {
            for len in sparse::MIN_LEN..=sparse::MAX_LEN {
                let Some(gram) = b.get(start..start + len) else {
                    break;
                };
                names.entry(sparse::key_folding(gram)).or_insert_with(|| {
                    String::from_utf8_lossy(&gram.to_ascii_lowercase()).into_owned()
                });
            }
        }
    }
    names
}

fn key_text(key: u32, names: &HashMap<u32, String>) -> (&'static str, Option<String>) {
    if sparse::is_gram_key(key) {
        ("gram", names.get(&key).cloned())
    } else {
        let bytes = [(key >> 16) as u8, (key >> 8) as u8, key as u8];
        let text = bytes
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() || b == b' ' {
                    (b as char).to_string()
                } else {
                    format!("\\x{b:02x}")
                }
            })
            .collect();
        ("trigram", Some(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_each_matching_line_once() {
        let re = regex::bytes::Regex::new("ab").unwrap();
        assert_eq!(count_matching_lines(&re, b"ab ab\nx\nab\n"), 2);
        assert_eq!(count_matching_lines(&re, b"none"), 0);
    }

    #[test]
    fn names_grams_from_the_pattern() {
        let pat = Pattern {
            text: r"bulk\_Update",
            fixed: false,
            case: Case::Sensitive,
            word: false,
        };
        let names = gram_names(&pat);
        let key = sparse::key_folding(b"_upd");
        assert_eq!(names.get(&key).map(String::as_str), Some("_upd"));
    }

    #[test]
    fn word_rule_needs_non_word_edges() {
        let pat = Pattern {
            text: "cat",
            fixed: true,
            case: Case::Sensitive,
            word: true,
        };
        let re = pat.regex().unwrap();
        assert!(re.is_match(b"a cat."));
        assert!(!re.is_match(b"concat"));
    }
}
