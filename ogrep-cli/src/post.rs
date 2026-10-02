//! Searches whose output is reshaped after the search runs: `--scope`,
//! `--kind`, `--in` and `--rank`.
//!
//! The ordinary search runs into a capturing writer. Its results are grouped by
//! file, the files that matched are parsed for structure when a flag needs it
//! (in parallel, and only those files), matches are filtered by kind or zone,
//! and the survivors are printed in the order the search found them or, with
//! `--rank`, ranked. Every printed line is a real match.

use ogrep_core::encoding::EncodingMode;
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

    /// Whether matches are placed in their file's parsed structure.
    pub fn parses(&self) -> bool {
        self.scope || self.filters_matches()
    }

    /// Whether --kind or --in picks among the matches.
    pub fn filters_matches(&self) -> bool {
        self.kind.is_some() || self.zone.is_some()
    }

    /// Whether a match that is (`def`) or is not a definition, in `zone`,
    /// passes --kind and --in.
    fn passes(&self, def: bool, zone: ZoneFilter) -> bool {
        let kind = match self.kind {
            None => true,
            Some(KindFilter::Def) => def,
            Some(KindFilter::Ref) => !def,
        };
        kind && self.zone.is_none_or(|z| z == zone)
    }

    /// `--callers` needs each use's enclosing definition.
    pub fn with_callers(mut self) -> Self {
        if self.callers {
            self.scope = true;
            self.kind = Some(KindFilter::Ref);
            // A name in a string or comment is a mention, not a use.
            self.zone.get_or_insert(ZoneFilter::Code);
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
use ogrep_core::structure::{Cached, lookup_cached, parse_miss};

/// Stand-ins when structure support is compiled out: nothing is cached or
/// parsed.
#[cfg(not(feature = "structure"))]
#[allow(dead_code)]
enum Cached {
    Found(Structure),
    Miss(ogrep_core::meta::ContentId),
    Unsupported,
}

#[cfg(not(feature = "structure"))]
fn lookup_cached(
    _rel: &str,
    _text: &[u8],
    _cache: Option<&ogrep_core::facts::FactStore>,
) -> Cached {
    Cached::Unsupported
}

#[cfg(not(feature = "structure"))]
fn parse_miss(
    _rel: &str,
    _text: &[u8],
    _id: ogrep_core::meta::ContentId,
    _caching: bool,
) -> (Option<Structure>, Option<NewFact>) {
    (None, None)
}

use ogrep_core::facts::NewFact;
/// A match or context line of one file, by line number, for printing.
type Row<'a> = (
    usize,
    Result<(usize, &'a Match), &'a crate::output::ContextLine>,
);

/// What the structure says about each match on a line: whether it names a
/// definition, and its zone.
type MatchFacts = Vec<(bool, ZoneFilter)>;

/// What the structure says about each match of `m`.
#[cfg(feature = "structure")]
fn facts(structure: &Structure, m: &Match) -> MatchFacts {
    use ogrep_core::structure::Zone;
    m.spans
        .iter()
        .map(|&(s, e)| {
            let (s, e) = (m.absolute_offset + s, m.absolute_offset + e);
            let zone = match structure.zone_at(s) {
                Zone::Code => ZoneFilter::Code,
                Zone::Comment => ZoneFilter::Comments,
                Zone::String => ZoneFilter::Strings,
            };
            (structure.defines(s, e), zone)
        })
        .collect()
}

/// Whether ogrep can parse `rel`'s language.
#[cfg(feature = "structure")]
fn has_grammar(rel: &str) -> bool {
    ogrep_core::structure::supports(rel)
}

#[cfg(not(feature = "structure"))]
fn has_grammar(_rel: &str) -> bool {
    false
}

/// The definitions enclosing `offset`, built once per innermost one.
#[cfg(feature = "structure")]
fn scope_at(
    structure: &Structure,
    offset: usize,
    built: &mut std::collections::HashMap<usize, Scope>,
) -> Option<Scope> {
    let innermost = structure.innermost(offset)?;
    if let Some(scope) = built.get(&innermost) {
        return Some(scope.clone());
    }
    let chain = structure.chain(Some(innermost));
    let inner = &structure.symbols[innermost];
    let json = chain
        .iter()
        .map(|s| {
            serde_json::json!({
                "kind": s.kind, "name": s.name,
                "start_line": s.start_line, "end_line": s.end_line,
            })
        })
        .collect::<Vec<_>>();
    let scope = Scope {
        start: inner.start_line,
        end: inner.end_line,
        chain: std::sync::Arc::new(serde_json::Value::from(json)),
    };
    built.insert(innermost, scope.clone());
    Some(scope)
}

#[cfg(not(feature = "structure"))]
fn facts(_structure: &Structure, _m: &Match) -> MatchFacts {
    unreachable!("no structure without the feature")
}

#[cfg(not(feature = "structure"))]
fn scope_at(
    _structure: &Structure,
    _offset: usize,
    _built: &mut std::collections::HashMap<usize, Scope>,
) -> Option<Scope> {
    unreachable!("no structure without the feature")
}

struct Prepared {
    rel: String,
    hits: FileHits,
    /// Per remaining match: its scope, when `--scope` asked for it.
    scopes: Vec<Option<Scope>>,
}

/// The innermost definition enclosing a match, and the chain to it. Matches
/// in the same definition share one.
#[derive(Clone)]
pub(crate) struct Scope {
    /// First and last line of the innermost definition.
    start: usize,
    end: usize,
    /// Every enclosing definition, outermost first, as JSON.
    chain: std::sync::Arc<serde_json::Value>,
}

/// A matched file's text and, in a parsed language, its structure.
#[derive(Default)]
struct Loaded {
    text: Option<String>,
    structure: Option<Structure>,
}

/// A matched file's text as the search read it, when that is the file's own
/// UTF-8 (less any byte-order mark the search dropped): match offsets are
/// only comparable then, and that is what the parsers take. Transcoded text
/// is not returned.
fn read_text(rel: &str, root: &Path, encoding: EncodingMode) -> Option<String> {
    let path = if root.is_file() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let mut bytes = std::fs::read(&path).ok()?;
    let skipped = match ogrep_core::encoding::decode_bytes(&bytes, encoding) {
        std::borrow::Cow::Borrowed(body) => bytes.len() - body.len(),
        std::borrow::Cow::Owned(_) => return None,
    };
    bytes.drain(..skipped);
    String::from_utf8(bytes).ok()
}

/// Annotate and filter one file's matches. `None` when no match survives.
fn prepare(
    rel: String,
    mut hits: FileHits,
    loaded: Loaded,
    post: &PostOptions,
    opts: &crate::search::SearchOptions,
) -> Option<Prepared> {
    let Loaded { text, structure } = loaded;
    // Per-match facts serve --kind, --in and ranking; --scope alone needs
    // only where each kept line's first match is.
    let wants_facts = post.filters_matches() || post.rank.is_some();
    let facts: Vec<Option<MatchFacts>> = hits
        .matches
        .iter()
        .map(|m| {
            structure
                .as_ref()
                .filter(|_| wants_facts)
                .map(|s| facts(s, m))
        })
        .collect();

    // A match passes when it is of the kind and in the zone asked for, and
    // a line is kept when one of its matches passes; the matches that fail
    // are dropped from the kept line (-o, --count-matches and ranking must
    // not see them). Each match's kind and zone come from the file's
    // structure; without it the kind comes from a keyword heuristic before
    // the match (only when --kind asks), and the zone is taken to be code
    // (never claiming a comment or string). A line is a definition by its
    // first kept match, as ranking goes.
    let filtered = post.filters_matches();
    let mut scopes = Vec::new();
    let mut built_scopes = std::collections::HashMap::new();
    let mut defs = Vec::new();
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for (mut m, f) in hits.matches.drain(..).zip(facts) {
        let def = if filtered {
            let spans: std::borrow::Cow<[(bool, ZoneFilter)]> = match &f {
                Some(f) => f.as_slice().into(),
                None => m
                    .spans
                    .iter()
                    .map(|&(s, _)| {
                        let def = post.kind.is_some() && rank::is_definition(&m.content, s);
                        (def, ZoneFilter::Code)
                    })
                    .collect::<Vec<_>>()
                    .into(),
            };
            // Every captured match has spans (-v, which has none, is refused
            // with --kind and --in).
            debug_assert!(!m.spans.is_empty());
            let keep: Vec<bool> = spans.iter().map(|&(d, z)| post.passes(d, z)).collect();
            if !keep.contains(&true) {
                dropped.push(m);
                continue;
            }
            m.retain_matches(&keep);
            keep.iter().position(|&k| k).is_some_and(|i| spans[i].0)
        } else {
            f.as_ref().and_then(|f| f.first()).is_some_and(|&(d, _)| d)
        };
        // The scope is where the first kept match starts.
        scopes.push(match &structure {
            Some(s) if post.scope => {
                let offset = m.absolute_offset + m.spans.first().map_or(0, |s| s.0);
                scope_at(s, offset, &mut built_scopes)
            }
            _ => None,
        });
        defs.push(def);
        kept.push(m);
    }
    if kept.is_empty() {
        return None;
    }
    hits.matches = kept;
    // Dropped matches and the text are kept only for what will print them.
    if opts.has_context() {
        hits.dropped = dropped;
    }
    hits.defs = (structure.is_some() && wants_facts).then_some(defs);
    hits.text = text.filter(|_| post.scope);
    Some(Prepared { rel, hits, scopes })
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
            .hits
            .text
            .as_deref()
            .map(|t| t.lines().collect())
            .unwrap_or_default();
        let mut uses: std::collections::BTreeMap<usize, (usize, Scope)> = Default::default();
        // A use is a line with a kept match, placed by that match (counting
        // spans would credit a line's other matches to the first one's
        // definition). A -U match counts once for each line it covers, as
        // --count-matches does.
        for scope in file.scopes.into_iter().flatten() {
            uses.entry(scope.start).or_insert((0, scope)).0 += 1;
        }
        writer.set_path_display(file.hits.display.clone());
        for (line, (count, Scope { chain, .. })) in uses {
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
/// them, scope headers read from the file's text.
pub(crate) fn emit_file(
    rel: &str,
    hits: &FileHits,
    shown: &[usize],
    scopes: &[Option<Scope>],
    opts: &crate::search::SearchOptions,
    writer: &mut OutputWriter,
) -> Result<()> {
    writer.set_path_display(hits.display.clone());
    if writer.is_files_only() {
        writer.write_file(rel)?;
        return Ok(());
    }
    if writer.is_count() {
        let n = if opts.count_matches {
            hits.matches.iter().map(|m| m.spans.len().max(1)).sum()
        } else {
            hits.matches.len()
        };
        writer.write_count(rel, n)?;
        return Ok(());
    }
    // A binary file's lines are printed only where a plain search would
    // print them (the capture keeps them for counting and filtering).
    if let Some(offset) = hits.binary
        && !writer.write_binary_note(rel, offset)?
    {
        return Ok(());
    }
    let lines: Vec<&str> = hits
        .text
        .as_deref()
        .map(|t| t.lines().collect())
        .unwrap_or_default();
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
    // Lines that matched but are not shown (--rank picked others, --kind or
    // --in dropped them) are context like any other line near a shown one.
    let unshown: Vec<crate::output::ContextLine> = if opts.has_context() {
        // A shown match's line is in `shown_lines`, so the filter below
        // leaves it out.
        let mut seen = std::collections::HashSet::new();
        hits.matches
            .iter()
            .chain(&hits.dropped)
            .filter(|m| {
                shown_lines.binary_search(&m.line_number).is_err()
                    && near(m.line_number)
                    && seen.insert(m.line_number)
            })
            .map(|m| crate::output::ContextLine {
                file: m.file.clone(),
                line_number: m.line_number,
                content: m.content.clone(),
                absolute_offset: m.absolute_offset,
                terminator_len: m.terminator_len,
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut rows: Vec<Row> = shown
        .iter()
        .map(|&i| (hits.matches[i].line_number, Ok((i, &hits.matches[i]))))
        .collect();
    rows.extend(
        hits.contexts
            .iter()
            .filter(|c| shown_lines.binary_search(&c.line_number).is_err() && near(c.line_number))
            .chain(&unshown)
            .map(|c| (c.line_number, Err(c))),
    );
    rows.sort_by_key(|(line, _)| *line);
    // A context line belongs to the innermost of the scopes of the matches
    // before and after it that contain it, and to none if neither does (no
    // header is printed for it).
    let scope_of = |row: &Row| -> Option<&Scope> {
        match row.1 {
            Ok((i, _)) => scopes.get(i).and_then(Option::as_ref),
            Err(_) => None,
        }
    };
    fn inside(scope: Option<&Scope>, line: usize) -> Option<&Scope> {
        scope.filter(|s| (s.start..=s.end).contains(&line))
    }
    let mut row_scopes: Vec<Option<&Scope>> = vec![None; rows.len()];
    let mut next = None;
    for (k, row) in rows.iter().enumerate().rev() {
        if row.1.is_ok() {
            next = scope_of(row);
            row_scopes[k] = next;
        } else {
            row_scopes[k] = inside(next, row.0);
        }
    }
    let mut prev = None;
    for (k, row) in rows.iter().enumerate() {
        if row.1.is_ok() {
            prev = scope_of(row);
        } else if let Some(p) = inside(prev, row.0)
            && row_scopes[k].is_none_or(|n| n.start <= p.start)
        {
            row_scopes[k] = Some(p);
        }
    }
    let only_matching = opts.effective_only_matching();
    let mut last_scope: Option<usize> = None;
    let mut last_line: Option<usize> = None;
    for ((line, row), scope) in rows.into_iter().zip(row_scopes) {
        // A gap is marked before the header that opens the next block, and
        // a block after a gap names its scope again.
        if opts.has_context() && last_line.is_some_and(|l| line > l + 1) {
            last_scope = None;
        }
        last_line = Some(line);
        writer.write_context_separator(rel, line)?;
        if let Some(scope) = scope
            && last_scope != Some(scope.start)
        {
            last_scope = Some(scope.start);
            let header = lines.get(scope.start - 1).copied().unwrap_or("");
            writer.write_scope(rel, scope.start, header, &scope.chain)?;
        }
        match row {
            // -o prints each match on its own row, as the search does.
            Ok((_, m)) if only_matching && !m.spans.is_empty() => {
                for row in m.only_matching_rows() {
                    writer.write_match(&row)?;
                }
            }
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
        emit_file(&file.rel, &file.hits, &shown, &file.scopes, opts, writer)?;
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
    let parses = post.parses();
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
    // Reading and cache lookups are cheap: they run in turn unless there are
    // thousands of files (on django, 500-1,700 matched files were looked up
    // faster in turn than on the thread pool; 20,000 small ones were not).
    // Parsing the misses runs on all threads when there is more than one.
    let lookup = |(rel, _, root, slot): &(String, FileHits, PathBuf, usize)| {
        // A file in a language without a grammar has nothing to parse.
        let text = (parses && has_grammar(rel))
            .then(|| read_text(rel, root, opts.encoding))
            .flatten();
        let cached = text
            .as_ref()
            .map(|t| lookup_cached(rel, t.as_bytes(), caches[*slot].1.as_ref()));
        (text, cached)
    };
    // -j1 keeps all of it on one thread.
    let parallel = opts.threads != Some(1);
    let looked_up: Vec<_> = if parallel && parses && inputs.len() > 4096 {
        inputs.par_iter().map(lookup).collect()
    } else {
        inputs.iter().map(lookup).collect()
    };
    let mut loaded: Vec<(Loaded, Option<NewFact>)> = Vec::with_capacity(inputs.len());
    let mut misses: Vec<(usize, ogrep_core::meta::ContentId)> = Vec::new();
    for (i, (text, cached)) in looked_up.into_iter().enumerate() {
        let structure = match cached {
            Some(Cached::Found(found)) => Some(found),
            Some(Cached::Miss(id)) => {
                misses.push((i, id));
                None
            }
            _ => None,
        };
        loaded.push((Loaded { text, structure }, None));
    }
    let parse = |&(i, id): &(usize, ogrep_core::meta::ContentId)| {
        let (rel, _, _, slot) = &inputs[i];
        let text = loaded[i].0.text.as_deref().unwrap_or_default();
        (
            i,
            parse_miss(rel, text.as_bytes(), id, caches[*slot].1.is_some()),
        )
    };
    let parsed: Vec<_> = if parallel && misses.len() > 1 {
        misses.par_iter().map(parse).collect()
    } else {
        misses.iter().map(parse).collect()
    };
    for (i, (structure, fact)) in parsed {
        loaded[i].0.structure = structure;
        loaded[i].1 = fact;
    }
    // New parses go to the cache for next time; a failure only costs a re-parse.
    let mut new_facts: Vec<Vec<NewFact>> = vec![Vec::new(); caches.len()];
    let prepared: Vec<Prepared> = inputs
        .into_iter()
        .zip(loaded)
        .filter_map(|((rel, hits, _, slot), (l, fact))| {
            new_facts[slot].extend(fact);
            prepare(rel, hits, l, post, opts)
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
