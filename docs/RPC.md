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
