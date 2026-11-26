# Troubleshooting

## Messages on stderr

| Message | Meaning | Action |
|---|---|---|
| `warning: no index at <path>/.ogrep - scanning every file` | No index where ogrep looked (the first path argument) | Search from the root with `-g`, pass `--index-path`, or run `ogrep index .` |
| `Server unavailable or index coverage not ready, falling back to local index` | A `serve.json` exists but the server is gone or still starting | Harmless. Results come from the local index, which may be stale |
| The same message ending in `: cannot open the served root: ...` | The tree was moved, removed or made unreadable under a running server | Start `ogrep serve .` where the tree is now, or fix its permissions |
| The same message ending in `: cannot read an indexed file: ...` or `: server .git flush wait timed out` | The server ran out of file descriptors or memory, or is still applying a large `.git` backlog | Results come from the local index; retry later, or raise the server's descriptor limit |
| `refresh requires a running server; start ogrep serve for this root` | `ogrep refresh` without a server (exit 2) | Start `ogrep serve .`, or use `--no-index` |
| `regex error: ... PCRE-style fallback failed` | The pattern is invalid in both engines (exit 2) | Fix the regex, or use `-F` for literal text |
| `server search queue is full` (exit 2) | More than 4 active and 32 queued searches | Retry after a moment |

## Results that look wrong

**A match you know exists is missing.**
1. The index is stale. Try `ogrep --no-index 'pattern' .`. If that finds it,
   run `ogrep refresh .` (with a server) or `ogrep index .` (without one).
2. The file is ignored, hidden, binary or over 64 MiB. Check
   `ogrep --files . | grep name`, then add `--hidden`, `--no-ignore` or
   `--no-max-filesize`.
3. There is a case mismatch under `-S`. Smart case becomes case-sensitive as soon
   as any pattern (including `--with`, `--without`, `--ident` and `-e`) has an
   uppercase literal character.

**Output differs from `rg`.** It is nearly always one of these:

- **Size limit.** ogrep skips files over 64 MiB, and ripgrep does not.
- **Invalid UTF-8.** ogrep decodes files and replaces undecodable bytes with
  U+FFFD, so `.` can match one of those. `--json` reports `lines.text` where
  ripgrep would send base64 `lines.bytes`. Byte offsets still match ripgrep's.
- **Multiline columns.** Under `-U`, ogrep reports the true column of the match
  on each line. ripgrep reports the first line's column on every line.
- **`-m` statistics.** With `-m`, the `bytes_searched` statistic is higher than
  ripgrep's, because ogrep searches a whole-file buffer. Match counts agree.
- **PCRE errors.** A match failure in the PCRE-style engine stops the search,
  where ripgrep continues.
- **`--no-messages`.** A folder containing an unreadable file exits 0, where
  ripgrep exits 2.

## Recovery

- **Corrupt or odd index:** stop the server, `rm -rf .ogrep`, then `ogrep index .`.
  The index is disposable.
- **Server stuck or from an old build:** `pkill -f 'ogrep serve'`, then start it
  again. It reconciles with the disk on startup.
- **Windows "Access is denied" when reinstalling:** an `ogrep.exe` is still
  running. Stop it with `taskkill //IM ogrep.exe //F`.
