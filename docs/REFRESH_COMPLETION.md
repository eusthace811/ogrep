# Refresh completion work

Status: development, 2026-09-27. The refresh RPC and CLI are implemented, with focused and live-daemon tests. Agent integration is implemented and tested. Workload evaluation and final release qualification remain pending. This document records the contract and prerequisite evidence; it does not qualify a release.

## Contract

A successful explicit refresh follows searchable publication of completed edits to eligible files whose contents and eligibility settled before the request and remained stable through completion. This includes creates, edits, deletes, renames and ignore-rule changes. Concurrent filesystem mutations may or may not be included; the operation is not an atomic filesystem snapshot. Ordinary indexed searches retain their existing asynchronous freshness behavior.

The initial portable implementation traverses and rereads eligible content, including paths with apparently unchanged metadata and previously memoized read failures. It uses the existing memory-bounded delta builder and serialized publication. An unreadable entry, incomplete traversal or failed publication must prevent a success receipt. A metadata-only reconcile, watcher receipt, `indexing: false`, or the existing `reload` response cannot establish this contract.

The RPC uses a bounded manual refresh worker with at most one active and one pending pass. Calls accepted after an active pass began must join the next pass, since the current traversal may already have visited their edits. Waiters need their own pass's result, and an explicit deadline covering initial-build and queue waits. A timeout must not report success; an already running pass may finish after its caller times out. Control requests must remain available within the documented global connection limit. Agent integration must retain `freshness: current` as a direct scan.

## Why not start with watcher cookies?

[Watchman's synchronization design](https://facebook.github.io/watchman/docs/cookies) uses a cookie and lock discipline to wait for earlier notifications. Its documentation explicitly describes FSEvents ordering failures under load. [Notify's documented limitations](https://docs.rs/notify/latest/notify/#known-problems) include missing native events and different editor save patterns. Both pages were checked on 2026-09-27. These support using a content reconciliation first; they do not prove zgrep's publication correctness. Cookies can be investigated later as a platform-specific fast path with a fallback.

## Delayed-event prerequisite

A source audit found two related gaps. Direct handling of a delayed Remove event tombstoned a currently existing replacement, even if a reconciliation had already published it. The normal burst queue already reconstructed a current-state check for files, so that particular direct-handler bug was not the ordinary queued-file route. However, both burst coalescing and initial-build deferral lost subtree-discovery intent for Remove events. A directory recreated before event processing could therefore remain undiscovered if its Create event did not arrive.

The handler now decides deletion from the current filesystem result. Remove hints retain subtree discovery through direct, burst and deferred paths. Existing subtree handling re-registers directories and applies normal containment and ignore rules; ordinary metadata changes still do not trigger recursive discovery.

Four deterministic tests failed before the change and pass afterwards. One publishes a recreated file before delivering its earlier removal. Three deliver a removal through each event route after recreating a directory with a new nested file, without allowing a later native Create callback to heal the result. Existing real-removal, unreadability, symlink containment and metadata-only replay tests are retained.

Evidence: [before](../evidence/next-release/event-freshness-before.log), [after](../evidence/next-release/event-freshness-after.log), [workspace](../evidence/next-release/event-freshness-workspace.log), [Clippy](../evidence/next-release/event-freshness-clippy.log). These are correctness tests, not speed benchmarks or proof that refresh completion is implemented.

## RPC implementation evidence

`refresh` explicitly selects content reconciliation; ordinary automatic reconciliation retains its metadata-based behavior. The worker waits out initial indexing, serializes with other reconciliations and publication, rereads all eligible paths and checks unreadable entries, filename readiness, hidden coverage and changed tracked membership before returning a receipt. One active job and at most one pending job bound refresh work. Pending callers share a result; callers arriving after a job starts cannot join it. Status and refresh bypass occupied search slots. Timeouts bound waiting, not the lifetime of the worker's already queued work. No durable generation-token API is claimed.

Nine focused tests cover completed edit/create/delete/rename/ignore and binary-filename membership under disabled/native-configured/poll-configured state; rereading despite matching evidence and a memoized failure; a forced read failure and retry; failed publication and retry; an emptied tree; a later caller's separate pass and shared pending work; initial-build timeout; invalid parameters/root mismatch; and refresh with all search slots occupied. The native-configured test directly exercises state without a running OS watcher; real daemon mode, restart, disconnect, agent and timing evaluation remain required.

[Focused log](../evidence/next-release/manual-refresh-focused.log), [workspace log](../evidence/next-release/manual-refresh-workspace.log), [Clippy](../evidence/next-release/manual-refresh-clippy.log). The full workspace log precedes the final root-binding/parameter and occupied-slot checks; the focused tests and Clippy include those additions. Further combined qualification is pending.

## CLI and live-daemon tests

`zgrep-code refresh . --timeout-secs 30 --json` requires the running server for that exact root, discovered at the normal index path or `--index-path`. Timeout accepts 1–300 seconds and includes connection/response waiting. The CLI verifies the response envelope, successful receipt, root, duration and count. It returns exit 2 for unavailable/old servers, errors, invalid receipts and timeout, with no local fallback or readiness substitution. It bounds receipt input to 64 KiB and recomputes the remaining read deadline, including when a peer trickles partial data. Cancellation of the CLI does not cancel queued server work.

The root must be a UTF-8 path for this JSON client; the ordinary search behavior is unchanged. The server owns the scope settings. Use the same root and index path as `serve`; refresh does not accept a narrower subtree or change the server's ignore/exclusion/size configuration.

Four integration tests cover real-daemon initial construction, completed edits, immediate server-routed search, root mismatch, unavailable stale discovery and restart in all three modes; invalid CLI deadlines and missing server; explicit old-server/overload/incomplete/timeout errors and invalid receipts; and a trickling response that cannot extend a one-second client deadline. These tests use deterministic expected markers and filenames. They are not the independent ripgrep workload evaluation or a cross-platform claim.

[Focused tests](../evidence/next-release/refresh-cli-focused.log), [combined workspace tests](../evidence/next-release/refresh-cli-workspace.log), [Clippy](../evidence/next-release/refresh-cli-clippy.log).

## Agent integration

Adapter 1.1.0 exposes `refresh_index` through MCP and `zgrep_refresh_index` through pi. The tool waits for startup within its own budget, then calls the strict CLI and validates the receipt. It accepts 1–300 seconds (default 30); no path override is accepted. Service readiness, local fallback and an old server cannot produce a success receipt. The shared service may keep working after client cancellation. `freshness: current` retains its direct-scan semantics. Installations now probe the binary's refresh capability and reject an incompatible binary.

[42 agent/installer tests](../evidence/next-release/agent-refresh-tests.log) passed, including the real pi→MCP→CLI→server route. A no-watch daemon test proves the new marker is absent from indexed search before refresh, present afterwards, and that a follow-up CLI query actually uses the server. Tests also cover service startup, deadlines, cancellation during startup and CLI work, strict validation, invalid replies, and MCP cancellation/shutdown reaping search and refresh children. This is local adapter evidence, not a hosted agent/model behavior claim.

## Workload evaluation

[Immediate parity under load and pinned-corpus cost](REFRESH_EVALUATION.md) records 225 receipts, 450 immediate comparisons, 10,840 checked concurrent searches and the roughly 3.07-second full Rust-tree refresh cost. Repeated-refresh memory stability and final release qualification remain open.
