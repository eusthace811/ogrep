Historical research memo, written before the experiments. For measured results and final decisions, see [BENCHMARKS.md](../BENCHMARKS.md). Proposed commands below are not shipped features.

Research date: 23 September 2026. Local baseline: `tgrep/`, commit `56f8c62`, workspace version 1.0.10.

**Recommendation: develop an improved fork around three capabilities: efficient exact search, explicit freshness, and useful code context.** Start by improving the existing query engine, then benchmark alternative indexes. Add structural navigation before deciding whether natural-language retrieval justifies its additional resources.

This is a research and source-inspection assessment. No implementation benchmarks were run, and projected benefits below are hypotheses, not measured tgrep speedups. “New” includes established algorithms that would be new to this implementation, alongside recent research.

**What this checkout already provides**

The project already has mmap-backed trigram lookup, next-byte and position masks, parallel verification, a live overlay, background indexing, bounded content caching, watcher recovery, generation publication, and an MCP integration. Adding “an index,” “a watcher,” or “MCP support” would duplicate existing work. See the local [README](tgrep/README.md), [agent guidance](tgrep/AGENTS.md), and [integration guide](tgrep/scripts/agent/README.md).

Four concrete observations make the recommendations specific:

- In `tgrep-core/src/query.rs`, `execute_plan_with_masks` fetches every posting list into a vector before sorting lists by length. Rarest-first intersection exists, but lazy loading does not exist in this execution path.
- `PostingEntry` uses six bytes on disk: four for the file ID and two for masks. Lookup entries already contain posting counts. See `tgrep-core/src/ondisk.rs` and `reader.rs`.
- The execution path carries `loc_mask` values without testing positional consistency. `check_adjacency` exists in `trigram.rs`, but the inspected executor does not call it. The plan also lacks literal-run IDs and offsets needed to apply this safely after reordering.
- The regex plan has `And(Vec<TrigramQuery>)`, `Or(Vec<QueryPlan>)`, and `MatchAll`. Concatenation discards `Or` children as index constraints. This is conservative for correctness but can lose useful filtering.

The MCP adapter already distinguishes indexed results from a current filesystem scan and reports truncation. New features should build on that distinction.

**Candidate improvements, in practical order**

| Improvement | What the user gains | Relative effort | Main uncertainty |
|---|---|---|---|
| Lazy, cost-aware Boolean planner | Lower query latency and allocation cost | Medium | Whether posting work dominates real queries |
| Use existing positional masks safely | Fewer files opened for long literals | Medium | Mask saturation on repetitive files |
| Sparse variable-length n-grams | Better filtering for identifiers and long strings | High | Build time and index size |
| Adaptive posting compression | Smaller index and less memory traffic | Medium–high | Decode overhead and payload layout |
| Explicit refresh acknowledgements | Reliable searches after an agent's edits | Medium–high | Cross-platform watcher guarantees |
| Structural search and symbol context | Functions and definitions instead of isolated lines | Medium–high | Grammar coverage and extraction quality |
| SCIP-backed references | More precise symbol navigation | High | Language-specific indexing and freshness |
| BM25 plus optional vector retrieval | Discovery when the identifier is unknown | High | Relevance versus model and index cost |
| Shared content across worktrees | Less duplicated indexing and storage | High | Version and path membership correctness |
| Fuzzy symbol/path lookup | Recover from spelling and naming uncertainty | Medium | Ranking quality and automaton cost |

**1. Improve planning before changing the index format**

Read posting counts first, then open iterators in estimated cost order. A missing required gram should end that branch immediately. Intersect incrementally and stop when the candidate set is empty. When few candidates remain, compare the estimated cost of further filtering with simply verifying those files. Dropping a necessary-condition filter broadens candidates; exact verification must still decide every returned match.

GitHub describes lazy posting iterators that avoid loading whole lists. That is a directly relevant architectural precedent, though its global service scale does not predict local performance. [GitHub's code-search architecture](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/)

Upgrade the plan to a general Boolean tree with conjunctions containing subplans. For example, `(connect|accept).*timeout` could preserve `(connect OR accept) AND timeout`, instead of retaining only the outer literal's filter. Preserve required literals through nested alternations without expanding the expression into an exponentially large normal form. Bound planner complexity and fall back conservatively. Zoekt documents compositional substring plans for regex search. [Zoekt design](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)

Use linear merging for similarly sized posting lists and benchmark galloping/seek-based intersections for very unequal sizes. SIMD galloping is established research; its reported gains on other datasets are not a tgrep forecast. [Lemire, Boytsov and Kurz](https://arxiv.org/abs/1401.6399)

Add an explain mode reporting selected grams, posting counts, candidates, files opened, verification bytes, fallback reason, and time in each stage. Existing statistics should be extended rather than replaced. This gives every later experiment a diagnosis.

**2. Exploit the position byte already stored**

For grams from the same required literal, retain their byte offsets in the plan. Rotate each occurrence mask back by that offset modulo eight, then intersect the aligned masks. A zero intersection proves that literal cannot occur in the file. Nonzero still requires verification.

This proposal derives from the current mask representation. It must never compare unrelated literal runs as though they were adjacent: `foo.*bar` does not give a fixed distance between `foo` and `bar`. Case transformations, alternations, and repeated grams also require careful treatment. This would change plan metadata and execution while initially keeping the disk format.

Probabilistic masks lose filtering power when their bits saturate. Cursor's March 2026 article explains this limitation and contrasts masks with sparse grams. [Cursor research](https://cursor.com/blog/fast-regex-search)

**3. Prototype sparse n-grams as an optional index**

Fixed trigrams split a long identifier into many common pieces. A variable-length gram can reject substantially more files. GitHub's sparse-gram design selects grams using deterministic weights on adjacent character pairs. A public implementation provides `build_all` and `build_covering` as a starting point for understanding index/query agreement. [Sparse n-gram implementation](https://github.com/danlark1/sparse_ngrams)

Prototype a supplemental sparse index alongside the current trigram baseline. Query it only when the parser can derive sound necessary literals. Retain the existing fallback for short queries and unsupported forms. Boundary handling must guarantee that a query's required gram was emitted whenever that literal occurs in a document; otherwise the optimization silently loses matches.

“Sparse” does not promise fewer index entries. Cursor explicitly describes generating more grams while consulting fewer at query time. Measure build cost, bytes on disk, candidate counts, and cold as well as warm latency. [Cursor's algorithm explanation](https://cursor.com/blog/fast-regex-search)

Another candidate is frequency-based variable-length gram selection. The 2025 study of FREE, BEST, and LPMS compares construction, storage, filtering, and query trade-offs. Its five workloads are not a tgrep monorepo benchmark, so it supports an experiment rather than declaring a universal winner. [2025 evaluation](https://arxiv.org/abs/2504.12251)

**4. Compress posting IDs while preserving useful masks**

Compare plain arrays, delta-encoded block bitpacking, and Roaring bitmaps by posting density. Store masks in aligned payload arrays or an explicitly mapped structure; replacing postings with a set alone would discard information the executor uses. Small lists may work best uncompressed.

Rust implementations are available for both [SIMD bitpacking](https://github.com/quickwit-oss/bitpacking) and [Roaring bitmaps](https://github.com/RoaringBitmap/roaring-rs). Add block skip metadata so a lazy iterator can seek without decoding an entire list. Benchmark the complete intersection-plus-mask-plus-verification path, not just codec throughput.

This requires a versioned index format and migration strategy. Introduce it only after measuring how much query time is actually spent reading postings.

**5. Make freshness a query contract**

Currently an immediate post-edit search can precede watcher processing, while the documented current mode performs a scan. A useful extension is explicit `refresh(paths)` acknowledgement: the editing client identifies changed paths, tgrep indexes their observed versions, and returns a generation token. A subsequent query requests at least that generation. This addresses an agent's known edits without relying solely on event delivery.

For external edits, consider event-clock synchronization with a scan or reconciliation fallback when coverage is uncertain. Watchman provides clocks and synchronization cookies, but documents limitations on macOS FSEvents. A cookie alone is therefore insufficient for a portable “everything is current” promise. [Watchman clock](https://facebook.github.io/watchman/docs/cmd/clock), [synchronization limitations](https://facebook.github.io/watchman/docs/cookies)

Expose observed generation, scope, completeness and truncation separately. “Index built” and “current” must remain separate concepts. A response tied to a generation also needs an explicit content-version policy: current disk reads do not automatically constitute an atomic repository snapshot. The existing reader-generation protections are a foundation, not the complete public contract.

**6. Return syntax-aware context and support structural patterns**

Add a per-file symbol/structure side index: symbol name, kind, byte span, containing symbol, and file content identity. Tree-sitter supports incremental parsing and can form the extraction layer. [Tree-sitter](https://tree-sitter.github.io/tree-sitter/)

Useful new operations include finding definitions, returning the enclosing function for a text match, searching only comments or strings, and matching a call pattern despite whitespace and multiline formatting. ast-grep demonstrates practical AST pattern search, including metavariables and composed rules. [ast-grep documentation](https://ast-grep.github.io/guide/introduction.html)

Combine sound text prefilters with syntax verification. Mandatory identifier text may narrow candidate files, but formatting-sensitive strings or optional pattern components cannot safely serve as mandatory filters. Cache parse results by content identity and grammar version. Unsupported or malformed syntax needs an explicit fallback or coverage signal.

A context response should contain exact spans and provenance, deduplicate overlapping functions, and honor an output budget. This could reduce repeated search/read calls. That benefit needs evaluation on actual tasks; it is not an established token-saving percentage.

**7. Keep precise references distinct from syntactic guesses**

Tree-sitter can identify a call expression but does not by itself resolve imported names, overloaded methods, or dynamic dispatch. For definition/reference/implementation navigation, consider ingesting SCIP indexes. The protocol provides those relations and has language-specific producers. [SCIP](https://github.com/scip-code/scip)

A references command should identify whether its result is compiler-resolved, syntactic, or textual, and what revision the index covers. Begin with one language used heavily in the intended workload. Treat complete call graphs and cross-language impact analysis as a later product commitment.

**8. Add ranked discovery separately from exhaustive grep**

For a question such as “where do requests retry after a timeout?”, index function-sized units with names, paths, comments and bodies. First establish a lexical BM25 baseline with identifier splitting and field boosts. Tantivy offers BM25 and configurable tokenization in Rust. [Tantivy](https://github.com/quickwit-oss/tantivy)

Then compare optional embeddings. Start with exact vector search on a small corpus; add HNSW only when scale warrants approximate nearest-neighbor retrieval. [HNSW paper](https://arxiv.org/abs/1603.09320)

Reciprocal rank fusion can combine independent lexical and vector rankings without equating their raw scores. It is a useful baseline, not a guarantee of improved code relevance. [RRF documentation](https://www.elastic.co/docs/reference/elasticsearch/rest-apis/reciprocal-rank-fusion)

Keep ranked top-k discovery explicit: it cannot support the same absence/completeness claims as exhaustive regex search. Measure retrieval relevance and downstream task outcomes against BM25 alone before accepting embedding memory, inference latency, model distribution, and reindexing costs.

**9. Reuse immutable content across branches and worktrees**

Index each distinct content blob once, with separate maps from repository/revision/path to blobs. Each worktree gets its own mutable overlay and deletion markers. GitHub describes content-addressed deduplication; Zoekt describes branch membership for shared blobs. [GitHub architecture](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/), [Zoekt branches](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)

This could benefit parallel agent worktrees and branch switching. The present integration deliberately keys separate services by root and options, so shared storage would be a substantial change. Retain per-worktree ignore rules and eligibility even when content is shared. Untracked and dirty files also need identities; a Git commit index alone is insufficient.

Persistent versioned trigram indexes are also studied in a 2024 IDE-indexing paper. That is a research lead for startup/version reuse, not evidence that a particular design will outperform the existing overlay. [Persistent IDE indices](https://arxiv.org/abs/2403.03751)

**10. Provide fuzzy name lookup**

A finite-state-transducer dictionary of symbols and paths can support prefix and edit-distance lookup. It would help with misspelled identifiers without scanning all source text. The Rust `fst` crate supports these operations, although its supplied Levenshtein automaton has documented memory limitations. Keep edit distance small and benchmark construction. [fst documentation](https://docs.rs/fst/latest/fst/)

Use explicit fuzzy search and label approximate results. Subsequence matching for abbreviations is a different ranking problem from edit distance and should be evaluated separately.

**Alternatives worth benchmarking, but not the first implementation**

Positional postings store actual occurrences and can compare two selective grams at their required distance. They can substantially narrow verification but increase index storage; Zoekt provides a concrete implementation reference. [Zoekt positional index](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)

Suffix arrays enable substring lookup and can assist regex evaluation, including short literals. They are a credible alternative for relatively static shards, but make incremental mutation and storage design more involved. [Nelson Elhage's implementation account](https://blog.nelhage.com/2015/02/regular-expression-search-with-suffix-arrays/)

Immutable segments plus compaction may eventually reduce update write amplification. The current code already has a live overlay and generation publication, so a segment architecture needs evidence that flush/merge cost is a bottleneck. Avoid rebuilding the storage engine merely to adopt a familiar database pattern.

**What would make our version distinctive**

The strongest product hypothesis is one local service that provides exact text matches, the relevant enclosing code, and an explicit statement of what version and scope were searched. Structural and ranked discovery can share the service without blurring their guarantees. MCP transport alone is already present and does not establish differentiation.

Illustrative future commands, not existing syntax:

```text
ourgrep explain 'connect.*timeout' .
ourgrep symbol --definition RequestHandler .
ourgrep search 'retry' --context function --budget 4000 .
ourgrep ast --lang rust '$VALUE.unwrap()' .
ourgrep refresh src/client.rs
ourgrep search 'timeout' --after-generation TOKEN .
ourgrep discover 'where requests are retried after timeouts' .
```

**Experiments that should decide the design**

1. Record a reproducible baseline using the current checkout, ripgrep, and suitable alternatives such as Zoekt or ast-grep for the capabilities they share. Include small projects and representative monorepos; disclose hardware, versions and options.
2. Measure process startup, warm-server and cold-cache query latency separately. Record p50/p95/p99, first result, completion time, CPU, memory, disk size, build cost, update lag, and output volume. Include index maintenance when evaluating repeated-search break-even cost.
3. Cover rare/common identifiers, no matches, one/two-byte literals, punctuation, Unicode/case variants, nested alternation, broad regexes, large files and many-result queries. Also test edits, creation, deletion, rename, branch switches, ignore changes, and watcher loss.
4. Check candidate soundness against exhaustive matching on the same frozen corpus and eligibility rules. Test every new filter for false negatives, including randomized regex/literal cases. Correctness comes before a speed result.
5. Compare lazy planning, positional masks, sparse grams and compression separately before combining them. Keep query settings and output semantics identical. Publish regressions as well as wins.
6. For discovery/context features, use labelled repository questions and compare relevant results in the top k, successful tasks, tool calls, tokens and elapsed time. Do not substitute regex microbenchmarks for product-quality evidence.

My proposed sequence is planner instrumentation and lazy execution; safe position filtering; a sparse-index experiment; explicit refresh acknowledgements and function context; then optional SCIP, ranked discovery, and worktree sharing according to measured demand. This preserves a working baseline while testing the architectural bets independently.
