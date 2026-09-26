# Local search RPC extensions

The server uses newline-delimited JSON-RPC over loopback TCP. Existing clients remain supported. This document describes extensions added after 0.2.0; see the operations guide for the local-service deployment scope.

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
