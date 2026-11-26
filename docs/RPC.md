# Server protocol

The server speaks newline-delimited JSON-RPC 2.0 over loopback TCP. A client finds it through `serve.json` in the index directory, which records the server's PID, port and the root it serves; a client sends a server only searches of paths in that root. This document describes compact file listings, first-span replies, admission limits and refresh; see the [operations guide](OPERATIONS.md) for the deployment scope.

## Compact file listings

For `search`, set both `files_only: true` and `compact_files: true` to receive one `{"type":"match","file":"relative/path"}` row per matching text file. `num_matches` remains the number of returned rows. Binary matches retain the existing `binary` marker, source offset and selected-line count, so clients can apply binary visibility rules.

`compact_files` defaults to false and has no effect when `files_only` is false. Legacy callers retain their existing detailed match rows. Older servers ignore this optional parameter and return the detailed representation; new clients must accept either. The CLI requests compaction for `-l` and already accepts both forms.

Compaction changes presentation only. It does not request full-file statistics: ordinary file listings still stop matching after the first selected line. Combining file listings with `stats: true` continues to request original per-file totals and compact rows as before. Compact binary listings return the binary marker alone even when `binary_lines` is true.

Example request:

```json
{"jsonrpc":"2.0","id":1,"method":"search","params":{"pattern":"fn main","types":["rust"],"files_only":true,"compact_files":true}}
```

## Service admission and overload

`serve --search-concurrency N` bounds simultaneous `search` and `files` requests, including response writes. The current default is 4; accepted values are 1–32. A shared FIFO queue holds at most 32 additional requests. Queued requests wait at most 30 seconds for admission. `status` and control methods bypass this search queue. Permits are released after response disposal, including disconnects.

The service accepts at most 64 concurrent connections, with no thread queue behind that limit. Each request frame is limited to 1 MiB including its newline. Socket reads and writes have 30-second idle timeouts; this is not a total wall-clock deadline for an actively transferring connection. Persistent connections may carry multiple requests but must respect the idle timeout. The connection cap also applies to control clients, so a new status connection may be refused when all connection slots are occupied; status on an existing connection remains available.

Queue overflow, queue timeout and connection exhaustion return JSON-RPC error `-32002`. Rejection before a request is read uses a null id. The updated CLI reports an explicit overload response as exit 2 and asks the caller to retry; it does not turn that response into local search work. Older clients may retain their own fallback behavior. Connection loss without an explicit overload response still follows the ordinary unavailable-server fallback.

Oversized frames receive `-32600` and the connection closes. `status` includes `search_admission` and `connection_admission`, each reporting active, queued, capacity and queue_capacity, plus `max_request_bytes`. These are instantaneous counters, not reservations or freshness indicators.

Admission bounds concurrency and pending work, not the memory required by an individual unbounded-result query. Content-cache limits, file-size rules and query output choices continue to matter.

## Explicit refresh completion

`refresh` waits for a content reconciliation and searchable publication of the configured served root. It rereads eligible content even when metadata appears unchanged, and applies the server's ignore, exclusion and size settings. Completed edits whose contents and eligibility remain stable through completion are covered. Concurrent mutations may or may not appear; this is not an atomic filesystem snapshot.

```json
{"jsonrpc":"2.0","id":1,"method":"refresh","params":{"timeout_ms":30000,"root":"/canonical/repository"}}
```

Both parameters are optional. `timeout_ms` defaults to 30000 and accepts integers 1–300000. `root`, when supplied, must equal the server's canonical absolute root; mismatches return `-32602` before scheduling work. Unknown or invalid parameters also return `-32602`. Success returns `status: "refreshed"`, `root`, `elapsed_ms` and `num_files` (content-indexed files; filename-only assets are not included in this count). It is a receipt for this request, not a durable generation token.

Incomplete traversal, read failure, publication failure, incomplete coverage or a detected concurrent tracked-membership change return `-32003`. The old index may remain searchable and partial progress may have been published; an error never acknowledges freshness. A wait deadline returns `-32004`; a queued or running pass may continue afterwards. The caller must retry before relying on indexed freshness. Disconnection likewise does not cancel the pass. Older servers reject the method; callers must not substitute an indexing-complete flag or `reload` success.

A single refresh worker handles at most one active and one pending pass. New callers can share the pending pass, but cannot join one already started. The wait deadline includes initial-build and queue waits. `status.manual_refresh` exposes `running` and `pending`; neither establishes freshness. Refresh bypasses search admission and remains subject to the global connection limit. Rereading and merging the whole eligible tree can be expensive; use it at edit/search boundaries where the completion contract is needed.

## First spans only

`detail` (default true) asks for each match row's `spans` and `columns`. With it, `all_spans: false` (default true) asks for each row's first span only (multiline and `replace` rows keep theirs) and no `columns`: a reshaping client (`--rank`, `--scope`) places and ranks lines by their first match and prints no columns, so the rest only enlarges the reply. It does not apply with `only_matching` (one row per match) or `stats` (match totals), which keep every span and column. Older servers ignore it and send every span with its column; clients must accept either.

## Result limits

A search request may carry `limit` (a positive integer) and `limit_files` (default false); `ogrep --max-results` sends them. The server then verifies candidates in growing batches, in candidate order, and stops once the files verified so far hold `limit` results: files when `limit_files` (`-l`, `-c`), else `match` rows, where a binary file reached by traversal counts for nothing because the client hides it. It always sends whole files and sets `"limited": true` when candidates were left unverified. The client cuts the output at exactly `limit` results itself. If a limited reply holds fewer results than `limit` once the client has dropped the rows it never prints, the client asks again without the limit, so the output never depends on where the server stopped. Older servers ignore both fields and send everything.

