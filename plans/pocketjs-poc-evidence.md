# Portable feed coordinator recovery evidence (2026-10-25)

Baseline: `/home/evelyn/git/atprotocol/bsky-vita`, branch `feat/pocketjs-live-feed-poc`, commit `cff2cf6ae2d1fadbc4ea041b609e70588d9fc645`. The dirty checkout at dispatch was designated authoritative and preserved. Captured relevant file snapshots, pre-edit hashes, and full staged/unstaged patches are outside the repository at `/home/evelyn/tmp/bsky-feed-amendment-20261025/`.

## Implementation

Added portable `bsky-feed` containing feed DTOs, validation, moderation, history caps, event/effect types, and the sole production coordinator. `bsky-pocket` reexports the portable API. `bsky-worker` now depends on `bsky-feed`, and projection code imports its DTO/policy types; worker dependency inspection contains `bsky-feed` only and no PocketJS/QuickJS dependency. The app keeps one coordinator outside shell lifetime, routes every correlated completion through it (even without a mounted shell), and delegates shell completion to returned presentation effects. Worker correlated request identity remains echoed by the worker. Session generation and auth-failure ordering remain in native main.

The Pocket adapter no longer owns its own cursor, feed history, pending-request bit, refresh intent, or feed command queue. It consumes the coordinator history/delivery/status effects and native presentation resources remain adapter-owned. Retry and refresh continue to request the first page, matching previous behavior. Existing, extensive dirty app/runtime sources and unrelated pre-existing modifications were preserved.

## Verification

| Command | Result |
|---|---|
| `cargo test -p bsky-feed` | PASS, exit 0; 11 tests including validation/caps, moderation, correlation no-ops and closed mount behavior. |
| `cargo test -p bsky-worker` | PASS, exit 0; 7 tests including feed projection and correlated deadlines. |
| `cargo test -p bsky-pocket` | PASS, exit 0; 13 unit, 2 containment, 3 QuickJS containment, 1 runtime integration, and doc tests. QuickJS C compiler emits existing upstream warnings. |
| `cargo tree -p bsky-worker --edges normal` | PASS, exit 0. Direct dependency list shows `bsky-feed`; no PocketJS or QuickJS. |
| `cargo tree -p bsky-worker \| rg 'bsky-(pocket\|feed)\|pocket-runtime\|quickjs'` | PASS; only `bsky-feed` matches. |
| `source /home/evelyn/dev/vitasdk/env.sh && cargo check -p bsky-vita-app --features pocketjs-poc --target armv7-sony-vita-newlibeabihf -Z build-std=std,panic_unwind` | PASS, exit 0; Vita target check. Existing C/QuickJS and dead-code warnings. |
| `cargo test -p bsky-vita-app --no-run` | NOT VERIFIED: fails because `VITASDK` is not set for host build scripts. |
| `rustfmt --edition 2021 ...` | PASS; applied to scoped changed Rust files. |
| `git diff --check` | PASS, exit 0. |

No full workspace suite was run, per instruction about known DNS suite hangs. No VPK build, hardware test, live-feed session, SDK default-profile build, or Sol review was performed. Changes remain uncommitted; no commit, push, or device operation.

## Remaining review notes

The amendment's source-level extraction is implemented, but the focused tests do not yet provide all requested coordinator behaviors: successful completion/latest-refresh end-to-end admission and retry/error state are not comprehensively asserted; app auth replacement/stale-auth races and dual-auth dispatch are not newly tested. `FeedCoordinator::handle` and public event/effect contract are present. Device/live-feed claims remain unverified. Review snapshot and incremental patch are in `/home/evelyn/tmp/bsky-feed-amendment-20261025/review.patch` after final freeze.
