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

#[cfg(feature = "structure")]
use ogrep_core::structure::analyze_cached;

#[cfg(not(feature = "structure"))]
fn analyze_cached(
    _rel: &str,
    _text: &[u8],
    _cache: Option<&ogrep_core::facts::FactStore>,
) -> (Option<Structure>, Option<NewFact>) {
    (None, None)
}

use ogrep_core::facts::NewFact;
/// A match or context line of one file, by line number, for printing.
type Row<'a> = (
    usize,
    Result<(usize, &'a Match), &'a crate::output::ContextLine>,
);

/// What the structure says about one match.
struct MatchFacts {
    def: bool,
    /// The zone of each match on the line.
    zones: Vec<ZoneFilter>,
    /// Innermost enclosing definition: `(first line, chain as JSON)`.
    scope: Option<(usize, serde_json::Value)>,
}

/// What the structure says about `m`; the scope chain only when `--scope`
/// will print it.
#[cfg(feature = "structure")]
fn facts(structure: &Structure, m: &Match, with_scope: bool) -> MatchFacts {
    use ogrep_core::structure::Zone;
    let offset = m.absolute_offset + m.spans.first().map_or(0, |s| s.0);
    let def = m
        .spans
        .iter()
        .any(|&(s, e)| structure.defines(m.absolute_offset + s, m.absolute_offset + e));
    let zones = m
        .spans
        .iter()
        .map(|&(s, _)| match structure.zone_at(m.absolute_offset + s) {
            Zone::Code => ZoneFilter::Code,
            Zone::Comment => ZoneFilter::Comments,
            Zone::String => ZoneFilter::Strings,
        })
        .collect();
    let chain = if with_scope {
        structure.enclosing(offset)
    } else {
        Vec::new()
    };
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
    MatchFacts { def, zones, scope }
}

#[cfg(not(feature = "structure"))]
fn facts(_structure: &Structure, _m: &Match, _with_scope: bool) -> MatchFacts {
    unreachable!("no structure without the feature")
}

struct Prepared {
    rel: String,
    hits: FileHits,
    /// Per remaining match: its scope, when `--scope` asked for it.
    scopes: Vec<Option<(usize, serde_json::Value)>>,
    /// The file's lines, for printing scope headers.
    text: Option<String>,
}

/// A matched file's text and, in a parsed language, its structure.
#[derive(Default)]
struct Loaded {
    text: Option<String>,
    structure: Option<Structure>,
}

/// A matched file's text, when it is valid UTF-8: offsets are only comparable
/// when the searched text is the file's bytes, which is also what the parsers
/// take.
fn read_text(rel: &str, root: &Path) -> Option<String> {
    let path = if root.is_file() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    String::from_utf8(std::fs::read(&path).ok()?).ok()
}

/// Annotate and filter one file's matches. `None` when no match survives.
fn prepare(
    rel: String,
    mut hits: FileHits,
    loaded: Loaded,
    post: &PostOptions,
) -> Option<Prepared> {
    let Loaded { text, structure } = loaded;
    let facts: Vec<Option<MatchFacts>> = hits
        .matches
        .iter()
        .map(|m| structure.as_ref().map(|s| facts(s, m, post.scope)))
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
        // A line is in a zone when any of its matches is. Without structure
        // the zone is unknown: count it as code, and never claim a comment
        // or string.
        let zone_ok = post.zone.is_none_or(|z| match f {
            Some(f) => f.zones.contains(&z),
            None => z == ZoneFilter::Code,
        });
        keep.push((kind_ok && zone_ok, def));
    }
    let mut scopes = Vec::new();
    let mut defs = Vec::new();
    let mut kept = Vec::new();
    for ((mut m, f), (ok, def)) in hits.matches.drain(..).zip(facts).zip(keep) {
        if ok {
            // Under --in, the matches outside the zone are not results: -o,
            // --count-matches and ranking must not see them.
            if let (Some(z), Some(f)) = (post.zone, &f) {
                let mut zones = f.zones.iter();
                m.spans.retain(|_| zones.next() == Some(&z));
            }
            scopes.push(if post.scope {
                f.and_then(|f| f.scope)
            } else {
                None
            });
            defs.push(def);
            kept.push(m);
        }
    }
    // A file that matched only as binary has no lines to classify: it is
    // kept (for its note, -l or rank) unless a filter needs its lines.
    let binary_only = hits.binary.is_some() && post.kind.is_none() && post.zone.is_none();
    if kept.is_empty() && !binary_only {
        return None;
    }
    hits.matches = kept;
    hits.defs = structure.is_some().then_some(defs);
    Some(Prepared {
        rel,
        hits,
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

/// Print one file's result: its binary note, then its path (-l), its count
/// (-c, over all its matches) or the `shown` matches (indices into
/// `hits.matches`) with the context around them and, when `scopes` has
/// them, scope headers read from `text`.
pub(crate) fn emit_file(
    rel: &str,
    hits: &FileHits,
    shown: &[usize],
    scopes: &[Option<(usize, serde_json::Value)>],
    text: Option<&str>,
    opts: &crate::search::SearchOptions,
    writer: &mut OutputWriter,
) -> Result<()> {
    writer.set_path_display(hits.display.clone());
    if writer.is_files_only() {
        writer.write_file(rel)?;
        return Ok(());
    }
    if writer.is_count() {
        // A binary-only match carries no lines to count; print nothing
        // rather than a wrong 0.
        if hits.matches.is_empty() {
            return Ok(());
        }
        let n = if opts.count_matches {
            hits.matches.iter().map(|m| m.spans.len().max(1)).sum()
        } else {
            hits.matches.len()
        };
        writer.write_count(rel, n)?;
        return Ok(());
    }
    // The note belongs to line output, as in a plain search.
    if let Some(offset) = hits.binary {
        writer.write_binary_note(rel, offset)?;
    }
    let lines: Vec<&str> = text.map(|t| t.lines().collect()).unwrap_or_default();
    let mut shown_lines: Vec<usize> = shown.iter().map(|&i| hits.matches[i].line_number).collect();
    shown_lines.sort_unstable();
    // A context line belongs to a shown match up to -B lines below it or -A
    // lines above it, in those directions only, as in the search itself.
    let (before, after) = (opts.before_ctx(), opts.after_ctx());
    let near = |line: usize| {
        let i = shown_lines.partition_point(|&s| s < line);
        shown_lines
            .get(i)
            .is_some_and(|&next| next - line <= before)
            || i.checked_sub(1)
                .is_some_and(|j| line - shown_lines[j] <= after)
    };
    let mut rows: Vec<Row> = shown
        .iter()
        .map(|&i| (hits.matches[i].line_number, Ok((i, &hits.matches[i]))))
        .collect();
    rows.extend(
        hits.contexts
            .iter()
            .filter(|c| shown_lines.binary_search(&c.line_number).is_err() && near(c.line_number))
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
            writer.write_scope(rel, *start, header, chain)?;
        }
        writer.write_context_separator(rel, line)?;
        match row {
            Ok((_, m)) => writer.write_match(m)?,
            Err(c) => writer.write_context_line(c)?,
        }
    }
    writer.flush_file()?;
    Ok(())
}

fn emit_plain(
    files: Vec<Prepared>,
    opts: &crate::search::SearchOptions,
    writer: &mut OutputWriter,
) -> Result<bool> {
    let printed = !files.is_empty();
    for file in files {
        let shown: Vec<usize> = (0..file.hits.matches.len()).collect();
        emit_file(
            &file.rel,
            &file.hits,
            &shown,
            &file.scopes,
            file.text.as_deref(),
            opts,
            writer,
        )?;
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
    let parses = post.scope || post.kind.is_some() || post.zone.is_some();
    let mut inputs: Vec<(String, FileHits, PathBuf, usize)> = Vec::new();
    let mut caches: Vec<(Option<PathBuf>, Option<ogrep_core::facts::FactStore>)> = Vec::new();
    for c in captured {
        // Without the parsers nothing is ever read from or added to it.
        let store = (parses && cfg!(feature = "structure"))
            .then_some(c.index_dir.as_deref())
            .flatten()
            .map(|dir| ogrep_core::facts::FactStore::open(dir, ogrep_core::facts::STRUCTURE));
        let slot = caches.len();
        caches.push((c.index_dir, store));
        for (rel, hits) in rank::group(vec![(c.display, c.events)]) {
            inputs.push((rel, hits, c.root.clone(), slot));
        }
    }
    // Reading, cache lookups and parsing the misses run on all threads when
    // there is more than one file.
    let load = |i: usize| -> (Loaded, Option<NewFact>) {
        let (rel, _, root, slot) = &inputs[i];
        let Some(text) = parses.then(|| read_text(rel, root)).flatten() else {
            return Default::default();
        };
        let (structure, fact) = analyze_cached(rel, text.as_bytes(), caches[*slot].1.as_ref());
        let loaded = Loaded {
            text: Some(text),
            structure,
        };
        (loaded, fact)
    };
    let loaded: Vec<(Loaded, Option<NewFact>)> = if parses && inputs.len() > 1 {
        (0..inputs.len()).into_par_iter().map(load).collect()
    } else {
        (0..inputs.len()).map(load).collect()
    };
    // New parses go to the cache for next time; a failure only costs a re-parse.
    let mut new_facts: Vec<Vec<NewFact>> = vec![Vec::new(); caches.len()];
    let prepared: Vec<Prepared> = inputs
        .into_iter()
        .zip(loaded)
        .filter_map(|((rel, hits, _, slot), (l, fact))| {
            new_facts[slot].extend(fact);
            prepare(rel, hits, l, post)
        })
        .collect();
    // Unmap every store first: put replaces index.bin, which Windows refuses
    // while this process maps it, and several path arguments can share one.
    for (_, store) in &mut caches {
        *store = None;
    }
    for ((dir, _), facts) in caches.iter().zip(new_facts) {
        if let Some(dir) = dir {
            let _ = ogrep_core::facts::FactStore::put(dir, ogrep_core::facts::STRUCTURE, facts);
        }
    }
    match &post.rank {
        Some(rank_opts) => {
            // A repository's fitted weights (`--rank-fit`) replace the defaults.
            let mut rank_opts = rank_opts.clone();
            rank_opts.weights =
                rank::Weights::for_index(caches.iter().find_map(|(d, _)| d.as_deref()));
            let rank_opts = &rank_opts;
            let files: Vec<(String, FileHits)> =
                prepared.into_iter().map(|p| (p.rel, p.hits)).collect();
            rank::emit(
                files,
                &opts.all_patterns(),
                opts.fixed_string,
                ci,
                rank_opts,
                opts,
                writer,
            )
        }
        None if post.callers => emit_callers(prepared, opts, writer),
        None => emit_plain(prepared, opts, writer),
    }
}
