# Refresh completion work

Status: development, 2026-09-27. The refresh CLI/RPC and agent tool are not implemented yet. This document records the contract and prerequisite evidence; it does not qualify a release.

## Contract to implement

A successful explicit refresh must follow searchable publication of completed edits to eligible files whose contents and eligibility settled before the request and remained stable through completion. This includes creates, edits, deletes, renames and ignore-rule changes. Concurrent filesystem mutations may or may not be included; the operation is not an atomic filesystem snapshot. Ordinary indexed searches retain their existing asynchronous freshness behavior.

The initial portable implementation will traverse and reread eligible content, including paths with apparently unchanged metadata and previously memoized read failures. It will use the existing memory-bounded delta builder and serialized publication. An unreadable entry, incomplete traversal or failed publication must prevent a success receipt. A metadata-only reconcile, watcher receipt, `indexing: false`, or the existing `reload` response cannot establish this contract.

Use a bounded manual refresh worker with at most one active and one pending pass. Calls accepted after an active pass began must join the next pass, since the current traversal may already have visited their edits. Waiters need their own pass's result, and an explicit deadline covering initial-build and queue waits. A timeout must not report success; an already running pass may finish after its caller times out. Control requests must remain available within the documented global connection limit. Agent integration must retain `freshness: current` as a direct scan.

## Why not start with watcher cookies?

[Watchman's synchronization design](https://facebook.github.io/watchman/docs/cookies) uses a cookie and lock discipline to wait for earlier notifications. Its documentation explicitly describes FSEvents ordering failures under load. [Notify's documented limitations](https://docs.rs/notify/latest/notify/#known-problems) include missing native events and different editor save patterns. Both pages were checked on 2026-09-27. These support using a content reconciliation first; they do not prove zgrep's publication correctness. Cookies can be investigated later as a platform-specific fast path with a fallback.

## Delayed-event prerequisite

A source audit found two related gaps. Direct handling of a delayed Remove event tombstoned a currently existing replacement, even if a reconciliation had already published it. The normal burst queue already reconstructed a current-state check for files, so that particular direct-handler bug was not the ordinary queued-file route. However, both burst coalescing and initial-build deferral lost subtree-discovery intent for Remove events. A directory recreated before event processing could therefore remain undiscovered if its Create event did not arrive.

The handler now decides deletion from the current filesystem result. Remove hints retain subtree discovery through direct, burst and deferred paths. Existing subtree handling re-registers directories and applies normal containment and ignore rules; ordinary metadata changes still do not trigger recursive discovery.

Four deterministic tests failed before the change and pass afterwards. One publishes a recreated file before delivering its earlier removal. Three deliver a removal through each event route after recreating a directory with a new nested file, without allowing a later native Create callback to heal the result. Existing real-removal, unreadability, symlink containment and metadata-only replay tests are retained.

Evidence: [before](../evidence/next-release/event-freshness-before.log), [after](../evidence/next-release/event-freshness-after.log), [workspace](../evidence/next-release/event-freshness-workspace.log), [Clippy](../evidence/next-release/event-freshness-clippy.log). These are correctness tests, not speed benchmarks or proof that refresh completion is implemented.
