# Refresh completion work

Status: development, 2026-09-27. The refresh RPC is implemented with unit coverage; CLI, agent integration and live-daemon evaluation remain pending. This document records the contract and prerequisite evidence; it does not qualify a release.

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
