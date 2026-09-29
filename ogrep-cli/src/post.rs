//! Searches whose output is reshaped after the search runs: `--scope`,
//! `--kind`, `--in` and `--rank`.
//!
//! The ordinary search runs into a capturing writer. Its results are grouped by
//! file, the files that matched are parsed for structure when a flag needs it
//! (in parallel, and only those files), matches are filtered by kind or zone,
//! and the survivors are printed in the order the search found them or, with
//! `--rank`, ranked. Every printed line is a real match.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rayon::prelude::*;

use crate::output::{Match, OutputWriter, PathDisplay};
use crate::rank::{self, FileHits, RankOptions};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindFilter {
    /// Matches where the matched name is defined.
    Def,
    /// Every other match.
    Ref,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneFilter {
    Code,
    Comments,
    Strings,
}

#[derive(Clone, Debug, Default)]
pub struct PostOptions {
    pub rank: Option<RankOptions>,
    pub scope: bool,
    pub kind: Option<KindFilter>,
    pub zone: Option<ZoneFilter>,
    /// Print the definitions that use the pattern, once each, instead of the
    /// matches (`--callers`). Implies `scope` and `kind = Ref`.
    pub callers: bool,
}

impl PostOptions {
    pub fn active(&self) -> bool {
        self.rank.is_some()
            || self.scope
            || self.kind.is_some()
            || self.zone.is_some()
            || self.callers
    }

    /// `--callers` needs each use's enclosing definition.
    pub fn with_callers(mut self) -> Self {
        if self.callers {
            self.scope = true;
            self.kind = Some(KindFilter::Ref);
        }
        self
    }
}

#[cfg(feature = "structure")]
type Structure = ogrep_core::structure::FileStructure;

/// Stand-in when structure support is compiled out: never constructed.
#[cfg(not(feature = "structure"))]
struct Structure;

/// A file's structure: from the fact cache when this exact text was parsed
/// before, otherwise parsed now, with the new entry to add to the cache.
#[cfg(feature = "structure")]
fn analyze(
    rel: &str,
    text: &[u8],
    cache: Option<&ogrep_core::facts::FactStore>,
) -> (
    Option<Structure>,
    Option<(ogrep_core::meta::ContentId, Vec<u8>)>,
) {
    use ogrep_core::structure::{self, FileStructure};
    if !structure::supports(rel) {
        return (None, None);
    }
    let id = ogrep_core::meta::ContentId::from_indexed_bytes(text);
    if let Some(found) = cache
        .and_then(|c| c.get(&id))
        .and_then(FileStructure::decode)
    {
        return (Some(found), None);
    }
    let parsed = structure::analyze(rel, text);
    let entry = match (&parsed, cache) {
        (Some(p), Some(_)) => Some((id, p.encode())),
        _ => None,
    };
    (parsed, entry)
}

#[cfg(not(feature = "structure"))]
fn analyze(
    _rel: &str,
    _text: &[u8],
    _cache: Option<&ogrep_core::facts::FactStore>,
) -> (
    Option<Structure>,
    Option<(ogrep_core::meta::ContentId, Vec<u8>)>,
) {
    (None, None)
}

/// A fact-cache entry: content id and encoded structure.
type NewFact = (ogrep_core::meta::ContentId, Vec<u8>);
/// A match or context line of one file, by line number, for printing.
type Row<'a> = (
    usize,
    Result<(usize, &'a Match), &'a crate::output::ContextLine>,
);

/// What the structure says about one match.
struct MatchFacts {
    def: bool,
    zone: ZoneFilter,
    /// Innermost enclosing definition: `(first line, chain as JSON)`.
    scope: Option<(usize, serde_json::Value)>,
}

#[cfg(feature = "structure")]
fn facts(structure: &Structure, m: &Match) -> MatchFacts {
    use ogrep_core::structure::Zone;
    let offset = m.absolute_offset + m.spans.first().map_or(0, |s| s.0);
    let def = m
        .spans
        .iter()
        .any(|&(s, e)| structure.defines(m.absolute_offset + s, m.absolute_offset + e));
    let zone = match structure.zone_at(offset) {
        Zone::Code => ZoneFilter::Code,
        Zone::Comment => ZoneFilter::Comments,
        Zone::String => ZoneFilter::Strings,
    };
    let chain = structure.enclosing(offset);
    let scope = chain.last().map(|inner| {
        let json = chain
            .iter()
            .map(|s| {
                serde_json::json!({
                    "kind": s.kind, "name": s.name,
                    "start_line": s.start_line, "end_line": s.end_line,
                })
            })
            .collect::<Vec<_>>();
        (inner.start_line, serde_json::Value::from(json))
    });
    MatchFacts { def, zone, scope }
}

#[cfg(not(feature = "structure"))]
fn facts(_structure: &Structure, _m: &Match) -> MatchFacts {
    unreachable!("no structure without the feature")
}

struct Prepared {
    rel: String,
    hits: FileHits,
    /// A newly parsed structure for the fact cache.
    new_fact: Option<(ogrep_core::meta::ContentId, Vec<u8>)>,
    /// Per remaining match: its scope, when `--scope` asked for it.
    scopes: Vec<Option<(usize, serde_json::Value)>>,
    /// The file's lines, for printing scope headers.
    text: Option<String>,
}

/// A matched file's text and, when the cache had it, its structure.
struct Loaded {
    text: Option<String>,
    structure: Option<Structure>,
    new_fact: Option<(ogrep_core::meta::ContentId, Vec<u8>)>,
    /// The file is in a parsed language, is valid UTF-8, and missed the cache.
    needs_parse: bool,
}

/// Read a matched file and look its structure up in the cache. Cheap; the
/// expensive parse of cache misses happens afterwards, in parallel.
fn load(rel: &str, root: &Path, cache: Option<&ogrep_core::facts::FactStore>) -> Loaded {
    let path = if root.is_file() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    // Offsets are only comparable when the searched text is the file's bytes:
    // valid UTF-8, which is also what the parsers take.
    let Some(text) = std::fs::read(&path)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
    else {
        return Loaded {
            text: None,
            structure: None,
            new_fact: None,
            needs_parse: false,
        };
    };
    let cached = cached_structure(rel, text.as_bytes(), cache);
    let needs_parse = cached.is_none() && supported(rel);
    Loaded {
        text: Some(text),
        structure: cached,
        new_fact: None,
        needs_parse,
    }
}

#[cfg(feature = "structure")]
fn supported(rel: &str) -> bool {
    ogrep_core::structure::supports(rel)
}

#[cfg(not(feature = "structure"))]
fn supported(_rel: &str) -> bool {
    false
}

#[cfg(feature = "structure")]
fn cached_structure(
    rel: &str,
    text: &[u8],
    cache: Option<&ogrep_core::facts::FactStore>,
) -> Option<Structure> {
    if !ogrep_core::structure::supports(rel) {
        return None;
    }
    let id = ogrep_core::meta::ContentId::from_indexed_bytes(text);
    cache?
        .get(&id)
        .and_then(ogrep_core::structure::FileStructure::decode)
}

#[cfg(not(feature = "structure"))]
fn cached_structure(
    _rel: &str,
    _text: &[u8],
    _cache: Option<&ogrep_core::facts::FactStore>,
) -> Option<Structure> {
    None
}

/// Annotate and filter one file's matches. `None` when no match survives.
fn prepare(
    rel: String,
    mut hits: FileHits,
    loaded: Loaded,
    post: &PostOptions,
) -> Option<Prepared> {
    let Loaded {
        text,
        structure,
        new_fact,
        ..
    } = loaded;
    let facts: Vec<Option<MatchFacts>> = hits
        .matches
        .iter()
        .map(|m| structure.as_ref().map(|s| facts(s, m)))
        .collect();

    let mut keep = Vec::with_capacity(hits.matches.len());
    for (m, f) in hits.matches.iter().zip(&facts) {
        let def = f.as_ref().map_or_else(
            || {
                m.spans
                    .iter()
                    .any(|&(s, _)| rank::is_definition(&m.content, s))
            },
            |f| f.def,
        );
        let kind_ok = match post.kind {
            None => true,
            Some(KindFilter::Def) => def,
            Some(KindFilter::Ref) => !def,
        };
        // Without structure the zone is unknown: count it as code, and never
        // claim a comment or string.
        let zone = f.as_ref().map_or(ZoneFilter::Code, |f| f.zone);
        let zone_ok = post.zone.is_none_or(|z| z == zone);
        keep.push((kind_ok && zone_ok, def));
    }
    let mut scopes = Vec::new();
    let mut defs = Vec::new();
    let mut kept = Vec::new();
    for ((m, f), (ok, def)) in hits.matches.drain(..).zip(facts).zip(keep) {
        if ok {
            scopes.push(if post.scope {
                f.and_then(|f| f.scope)
            } else {
                None
            });
            defs.push(def);
            kept.push(m);
        }
    }
    if kept.is_empty() {
        return None;
    }
    hits.matches = kept;
    hits.defs = structure.is_some().then_some(defs);
    Some(Prepared {
        rel,
        hits,
        new_fact,
        scopes,
        text: if post.scope { text } else { None },
    })
}

/// `--callers`: each definition enclosing a use, once, with its number of uses.
fn emit_callers(
    files: Vec<Prepared>,
    opts: &crate::search::SearchOptions,
    writer: &mut OutputWriter,
) -> Result<bool> {
    let mut printed = false;
    for file in files {
        let lines: Vec<&str> = file
            .text
            .as_deref()
            .map(|t| t.lines().collect())
            .unwrap_or_default();
        let mut uses: std::collections::BTreeMap<usize, (usize, serde_json::Value)> =
            Default::default();
        for scope in file.scopes.into_iter().flatten() {
            let entry = uses.entry(scope.0).or_insert((0, scope.1));
            entry.0 += 1;
        }
        writer.set_path_display(file.hits.display.clone());
        for (line, (count, chain)) in uses {
            printed = true;
            let header = lines.get(line - 1).copied().unwrap_or("").trim();
            if opts.json {
                writer.write_scope(&file.rel, line, header, &chain)?;
            } else {
                let plural = if count == 1 { "" } else { "s" };
                writer.write_scope(
                    &file.rel,
                    line,
                    &format!("{header}  ({count} use{plural})"),
                    &chain,
                )?;
            }
        }
        writer.flush_file()?;
    }
    Ok(printed)
}

/// Print prepared files in search order, with scope headers under `--scope`.
fn emit_plain(
    files: Vec<Prepared>,
    opts: &crate::search::SearchOptions,
    context_reach: usize,
    writer: &mut OutputWriter,
) -> Result<bool> {
    let mut printed = false;
    for file in files {
        let Prepared {
            rel,
            hits,
            scopes,
            text,
            ..
        } = file;
        writer.set_path_display(hits.display.clone());
        printed = true;
        if let Some(offset) = hits.binary {
            writer.write_binary_note(&rel, offset)?;
        }
        if writer.is_files_only() {
            writer.write_file(&rel)?;
            continue;
        }
        if writer.is_count() {
            let n = if opts.count_matches {
                hits.matches.iter().map(|m| m.spans.len().max(1)).sum()
            } else {
                hits.matches.len()
            };
            writer.write_count(&rel, n)?;
            continue;
        }
        let lines: Vec<&str> = text
            .as_deref()
            .map(|t| t.lines().collect())
            .unwrap_or_default();
        let shown: std::collections::HashSet<usize> =
            hits.matches.iter().map(|m| m.line_number).collect();
        let mut rows: Vec<Row> = hits
            .matches
            .iter()
            .enumerate()
            .map(|(i, m)| (m.line_number, Ok((i, m))))
            .collect();
        rows.extend(
            hits.contexts
                .iter()
                .filter(|c| {
                    !shown.contains(&c.line_number)
                        && shown
                            .iter()
                            .any(|&s| c.line_number.abs_diff(s) <= context_reach)
                })
                .map(|c| (c.line_number, Err(c))),
        );
        rows.sort_by_key(|(line, _)| *line);
        let mut last_scope: Option<usize> = None;
        for (line, row) in rows {
            if let Ok((i, _)) = &row
                && let Some(Some((start, chain))) = scopes.get(*i)
                && last_scope != Some(*start)
            {
                last_scope = Some(*start);
                let header = lines.get(start - 1).copied().unwrap_or("");
                writer.write_scope(&rel, *start, header, chain)?;
            }
            writer.write_context_separator(&rel, line)?;
            match row {
                Ok((_, m)) => writer.write_match(m)?,
                Err(c) => writer.write_context_line(c)?,
            }
        }
        writer.flush_file()?;
    }
    Ok(printed)
}

/// One path argument's captured results and where its files live.
pub struct Captured {
    pub display: PathDisplay,
    pub root: PathBuf,
    /// The index directory, when one exists, which also holds the fact cache.
    pub index_dir: Option<PathBuf>,
    pub events: Vec<crate::output::Captured>,
}

/// Reshape and print captured results. Returns whether anything was printed.
pub fn emit(
    captured: Vec<Captured>,
    opts: &crate::search::SearchOptions,
    ci: bool,
    post: &PostOptions,
    writer: &mut OutputWriter,
) -> Result<bool> {
    let context_reach = opts.after_ctx().max(opts.before_ctx());
    let parses = post.scope || post.kind.is_some() || post.zone.is_some();
    let mut inputs: Vec<(String, FileHits, PathBuf, usize)> = Vec::new();
    let mut caches: Vec<(Option<PathBuf>, Option<ogrep_core::facts::FactStore>)> = Vec::new();
    for c in captured {
        #[cfg(feature = "structure")]
        let store = parses
            .then_some(c.index_dir.as_deref())
            .flatten()
            .map(|dir| ogrep_core::facts::FactStore::open(dir, ogrep_core::structure::FACTS_NAME));
        #[cfg(not(feature = "structure"))]
        let store: Option<ogrep_core::facts::FactStore> = None;
        let slot = caches.len();
        caches.push((c.index_dir, store));
        for (rel, hits) in rank::group(vec![(c.display, c.events)]) {
            inputs.push((rel, hits, c.root.clone(), slot));
        }
    }
    // Reading and cache lookups are cheap and run in order; parsing the cache
    // misses is spread over threads when there is more than one.
    let mut loaded: Vec<Loaded> = inputs
        .iter()
        .map(|(rel, _, root, slot)| {
            if parses {
                load(rel, root, caches[*slot].1.as_ref())
            } else {
                Loaded {
                    text: None,
                    structure: None,
                    new_fact: None,
                    needs_parse: false,
                }
            }
        })
        .collect();
    let misses: Vec<usize> = (0..loaded.len())
        .filter(|&i| loaded[i].needs_parse)
        .collect();
    let parse = |i: usize| {
        let text = loaded[i].text.as_deref().unwrap_or("").as_bytes();
        let (structure, fact) = analyze(&inputs[i].0, text, caches[inputs[i].3].1.as_ref());
        (i, structure, fact)
    };
    let parsed: Vec<(usize, Option<Structure>, Option<NewFact>)> = if misses.len() > 1 {
        misses.par_iter().map(|&i| parse(i)).collect()
    } else {
        misses.iter().map(|&i| parse(i)).collect()
    };
    for (i, structure, fact) in parsed {
        loaded[i].structure = structure;
        loaded[i].new_fact = fact;
    }
    let prepared: Vec<(usize, Prepared)> = inputs
        .into_iter()
        .zip(loaded)
        .filter_map(|((rel, hits, _, slot), l)| prepare(rel, hits, l, post).map(|p| (slot, p)))
        .collect();
    // New parses go to the cache for next time; a failure only costs a re-parse.
    let mut new_facts: Vec<Vec<(ogrep_core::meta::ContentId, Vec<u8>)>> =
        vec![Vec::new(); caches.len()];
    let prepared: Vec<Prepared> = prepared
        .into_iter()
        .map(|(slot, mut p)| {
            if let Some(fact) = p.new_fact.take() {
                new_facts[slot].push(fact);
            }
            p
        })
        .collect();
    #[cfg(feature = "structure")]
    for ((dir, _), facts) in caches.iter().zip(new_facts) {
        if let Some(dir) = dir {
            let _ =
                ogrep_core::facts::FactStore::put(dir, ogrep_core::structure::FACTS_NAME, facts);
        }
    }
    #[cfg(not(feature = "structure"))]
    drop(new_facts);
    match &post.rank {
        Some(rank_opts) => {
            let files: Vec<(String, FileHits)> =
                prepared.into_iter().map(|p| (p.rel, p.hits)).collect();
            rank::emit(
                files,
                &opts.all_patterns(),
                opts.fixed_string,
                ci,
                rank_opts,
                context_reach,
                writer,
            )
        }
        None if post.callers => emit_callers(prepared, opts, writer),
        None => emit_plain(prepared, opts, context_reach, writer),
    }
}
