# Agent integration installer

Install ogrep's stdio MCP tools, startup prewarming and search guidance together.
Supports Linux and macOS with Python 3.11+. Run from a ogrep checkout; no Python
packages or separate MCP bridge need to be installed. This is a local installer,
not a `curl | sh` endpoint.

```bash
# Interactive selection (agent and project/user scope).
./install-agent.sh

# Install for one or both agents in another repository.
./install-agent.sh install --agent codex,pi --root /path/to/repository

# Install for all your sessions, resolving each session's repository at runtime.
./install-agent.sh install --agent codex --scope user

# Use a specific ogrep binary or index settings.
./install-agent.sh install --agent pi --binary /path/to/ogrep --max-filesize 8M

# Validate, restore missing managed files, or uninstall.
./install-agent.sh doctor --agent codex,pi --root /path/to/repository
./install-agent.sh repair --agent codex,pi --root /path/to/repository
./install-agent.sh uninstall --agent codex,pi --root /path/to/repository
```

Pass the same `--scope` and `--root` when managing an existing installation.
Noninteractive runs require `--agent`. Project scope is the default. Git
subdirectories resolve to the worktree root; outside Git the supplied/current
directory is the root. User installations rely on the MCP host launching tools
in the session's working directory. A project installation has a fixed root.

The updated adapter requires a binary with `refresh --timeout-secs` support;
older binaries must be rebuilt or replaced. The installer checks this capability.

The installer uses `--binary`, an executable on `PATH`, or a private copy of the
checkout's release binary, in that order. If none is available it builds the
checkout using `cargo build --release --locked -p ogrep-cli`; Cargo must be
installed. It does not install agent applications or modify your shell PATH.
Installation paths are absolute: run `repair --root /new/project` after moving
or copying a project, or reinstall/`repair` after changing the Python
executable. Repair rebases owned file locations and regenerates configuration
for the selected agents without reading or changing the old project. Run it for
every installed agent; `doctor` reports integrations still pointing to the old
root and fails when the interpreter recorded at installation time no longer
runs. Reinstall preserves the previous binary and index options unless
replacements are supplied.

## What gets installed

**Codex:** an `mcp_servers.ogrep` entry, a `SessionStart` command hook and a short
managed section in `AGENTS.md`. Existing JSON hooks are merged; if the config
already uses inline TOML hooks, the installer uses that representation even if
`hooks.json` also exists, leaving that JSON file unchanged. Other
MCP servers, hooks and instructions remain intact.

`mcp_servers` and `hooks` have to be written as `[mcp_servers.<name>]` and
`[[hooks.SessionStart]]` tables. TOML cannot extend an inline
(`mcp_servers = { ... }`) or dotted definition, so installation stops with an
explicit error instead of appending a block that would not parse.

Restart Codex after installation. Project configuration must be trusted. Current
Codex requires review/trust of new or changed hooks in `/hooks`; the installer
does not bypass that policy. The MCP query path can start the service even if
the prewarm hook has not been trusted. Administrators can disable hooks or MCP.
`doctor` verifies installed configuration and the MCP subprocess, not host trust
or managed policy. Use a Codex version supporting MCP and `SessionStart` command
hooks. See the [MCP documentation](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)
and [hook documentation](https://learn.chatgpt.com/docs/hooks).

**pi:** `.pi/extensions/ogrep.ts`, or `~/.pi/agent/extensions/ogrep.ts` for user
scope. This extension registers `ogrep_search_code`, `ogrep_find_files` and
`ogrep_refresh_index`,
connects them to the same Python MCP server, prewarms on `session_start`, and
adds concise search guidance. It uses pi's extension API, not a fictional
`.pi/mcp.json` configuration. No built-in tools are overridden. Restart pi or
use `/reload` and follow pi's project trust policy. See
[pi extensions](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md).

Both integrations encourage tool adoption; neither intercepts arbitrary shell
commands or guarantees that a model never uses `rg`.

## Search behavior

`search_code` supports literal text (default), regex, case-insensitive search,
file types, globs, context and content/files/count output. `find_files` filters
the normally searchable file list with a case-sensitive basename or
repository-relative path glob. Its glob is a Python `fnmatch` filter, not a
positive ripgrep override, so it never re-includes ignored files.

Both tools accept:

- `path`: existing path beneath the repository root; escapes and symlink targets
  outside the root are rejected. Directory scans do not follow symlinks.
- `freshness: "indexed"`: ensure the server, then use the CLI's normal indexed
  search/fallback behavior. Results may lag filesystem changes.
- `freshness: "current"`: use `--no-index`, for checking recent edits.
- `hidden`: include hidden non-ignored files.
- `max_results`: 1–1000 output records (default 100). Context rows count too.
- `file_types` and `glob`: at most 100 entries, 16 KB each and 64 KB together.

Results are returned as plain text grouped the way `rg --heading` prints them:
a `# root: ...  mode: ...` line, then each repository-relative path once
followed by its lines as `line:text` for a match and `line-text` for context.
Files output lists one path per line; count output prints `path:count`, with
counts taken from JSON statistics. A path that could be misread (one holding a
newline or control character, starting with `#` or with digits and `:`/`-`, or
with surrounding spaces) is printed as a JSON string. Empty results print
`# no results`; `# truncated: ...` and `# warnings: ...` close the text when
they apply.

The tools return no `structuredContent`: Codex shows the model a tool's
`structuredContent` in place of its text whenever it is present. Over the
localization identifier searches in `scripts/eval/data` (adapter defaults), the
text costs a model 0.32-0.42× the tokens pi read before (one JSON record per
line in the text) and 0.40-0.50× what Codex read (the same records as
`structuredContent`), for the same records.

Output is capped at approximately 48 KB of result records, plus metadata. An
oversized individual line may produce a truncated response with no records;
narrow the query or use files/count output. Queries time out after 30 seconds
(plus bounded service startup); cancellation terminates the query subprocess.
Queries that finish during cleanup are reaped normally; other termination
failures remain errors.
No-match is success, while invalid queries and unreadable paths are errors.
The response identifies truncation and the mode `current_scan` or
`indexed_or_scan`. The latter intentionally does not claim that the index was
used: CLI flags and incomplete coverage may trigger scanning. Stdout carries
only MCP JSON-RPC messages.

## Refresh after edits

`refresh_index` waits for the entire configured repository's eligible content to
be reread and published as searchable. Call it after a batch of completed edits,
wait for success, then issue indexed searches. It accepts `timeout_secs` from
1 to 300 (default 30), including service startup and waiting for the CLI receipt.
It uses the server's configured ignore, exclusion and size rules; it does not
accept an arbitrary root or a subtree override. A receipt contains `status:
"refreshed"`, the repository root, duration, content-file count and
`refresh_mode: "content_reconcile"`. Filename-only assets are also refreshed but
are not included in that count.

This is a full content reconciliation and can be expensive on large trees.
`freshness: "current"` still performs a direct scan for a single search. A
refresh covers completed edits whose contents and eligibility remain stable
through completion; concurrent mutations may or may not appear. It is not an
atomic filesystem snapshot or a promise about later edits.

Startup failure, an old server, an invalid receipt, timeout and cancellation
are errors. Refresh never falls back to a search or treats service readiness as
success. Cancellation and timeout terminate and reap the waiting CLI; queued or
running work in the shared service may still finish. Retry before relying on
indexed freshness. The pi bridge gives this tool its requested wait plus ten
seconds for adapter overhead; MCP queueing can consume that margin. Its abort
signal still cancels promptly.

## Shared service and files

Services and indexes are keyed by canonical repository path and indexing flags:

```text
<repo>/.ogrep-agent/                 Project installation, manifest and backups
$XDG_DATA_HOME/ogrep-agent/          User installation (default ~/.local/share)
$XDG_CACHE_HOME/ogrep-agent/<key>/   Shared service state (default ~/.cache)
  index/                            Index, serve.json and ogrep's serve.lock
  start.lock                        Startup coordination
  serve.log                         Server diagnostics
```

Project installation adds a managed `/.ogrep-agent/` entry to `.gitignore`.
In non-Git session roots, both service and query commands automatically enable
`--no-require-git` so this exclusion (and other `.gitignore` rules) takes effect.
The managed cache directory and the per-repository state directory must not be
symlinks: the installer and the runtime both refuse to write service state
through them.
`CODEX_HOME` and `PI_CODING_AGENT_DIR` are respected for user configuration.
The same configuration across Codex/pi or project/user installs shares a
service. Different worktrees and index options get separate services.
Existing manually managed `.ogrep` indexes/services are not adopted, because
the current discovery file cannot verify all their indexing settings.

The hook and MCP use the same service launcher. `serve` automatically builds or
resumes the index, and queries can scan during startup. There is no separate
`index` invocation and startup does not wait for an initial full build. Startup
locks and ogrep's own single-instance lock prevent competing servers. An explicit server-overload reply is reported as an error and never triggers
a new service or local scan. If
startup fails for other availability reasons, an indexed request uses a current filesystem scan rather than
deliberately querying an abandoned disk index. If a server dies during a query,
the existing CLI search fallback behavior still applies. The explicit refresh
tool always requires a valid server receipt; `current` remains a direct scan.

Servers intentionally survive agent sessions. There is no idle eviction in
this version; logs are retained without automatic rotation. Session shutdown
closes only the pi MCP adapter, not the shared indexing service.

## Ownership, repair and uninstall

The manifest records owned files, checksums, instruction/config blocks and the
exact hook entry. Repeated installation replaces those owned parts. Updates
outside managed blocks survive reinstall and uninstall. Config writes are
atomic per file, with rollback on write failure and recovery snapshots under
`backups/<timestamp>/paths.json`. This is not a crash-atomic transaction across
all files; backups are available if the machine exits during a transaction.

Installation preflights owned paths and rejects symlinks in their existing
components, including configuration directories, state, locks and backups.
Manifest targets are checked against the selected agent's exact destination
allowlist before being read or removed. These checks protect against preexisting
redirects; installation assumes another process is not concurrently replacing
parent directories during the operation.

If you modify an owned runtime, extension or managed block, installation and
uninstallation stop before writing configuration and identify the conflict.
Keep your edits elsewhere, restore the owned content from a backup, then retry.
`repair` restores missing owned files; it does not overwrite user modifications
or guess how to reconstruct a partially edited block.

Uninstall removes only the selected agent's owned integration. It retains
indexes, shared services, private binaries, installation metadata and backups.
It does not restore entire old config files over newer user settings, stop a
shared server, or recursively delete cache directories. There is deliberately
no destructive `--purge` option in this version.

## Development checks

```bash
cargo build --workspace
python3 -B -m unittest discover -s scripts/agent -p 'test_*.py' -v
```

Runtime tests use the checkout's debug/release executable and temporary
repositories. Node 22+ additionally tests the generated pi extension through
real MCP calls without a model/API key. The tests do not install into your
actual agent configuration.
