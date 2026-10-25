# Pocket visible-text layout repairs — verification snapshot

Baseline: commit `cff2cf6ae2d1fadbc4ea041b609e70588d9fc645`, branch `feat/pocketjs-live-feed-poc`, candidate `/home/evelyn/git/atprotocol/bsky-vita`. The supplied workspace was already dirty, including all six implementation/test paths below as untracked or part of its pre-existing PocketJS PoC. No branch/index/config changes, commit, push, device install or device run was performed.

## Repairs

- Retrieve the guest native batch before applying host visible-text commands, so the subsequent batch read cannot erase the overlay; batch command bounds remain enforced.
- Wrap measured prefixes instead of suffixes, cap source inspection at 256 UTF-8 bytes, retain the existing per-row/per-frame measurement budgets and 128-byte emitted-line limit, and check the shared deadline before native measurements. Normalize the logical-to-physical width comparison by doubling requested logical width.
- Derive line height from native font measurement rather than a fixed row-height constant. Carry measured `body_height` through validated placement to the guest timeline row container.
- Tag native overlay commands and clear/reject stale text on empty history, auth/mount/revision/layout changes. Native image commands and ownership were not changed.
- Add host batch serialization and guest placement-height regression coverage.

## Verification

- `rustfmt --edition 2021 --config skip_children=true app/src/pocket.rs crates/bsky-pocket/src/visible_text.rs crates/bsky-pocket/tests/runtime.rs` — exit 0.
- `cargo test -p bsky-pocket` — exit 0; unit, containment, feed, QuickJS, and runtime integration suites passed (13 unit, 2 containment, 10 feed, 3 QuickJS, 1 runtime).
- `bun run check` (from `pocket-ui/`) — exit 0; TypeScript check and PocketJS Vita package build passed.
- `bun run test` (from `pocket-ui/`) — exit 0; 7 tests passed.
- `source /home/evelyn/dev/vitasdk/env.sh && make test` — exit 0; complete workspace host suite passed. Existing dependency/compiler dead-code warnings remain.
- `source /home/evelyn/dev/vitasdk/env.sh && make build-pocket` — exit 0; release Vita VPK built at `target/armv7-sony-vita-newlibeabihf/release/bsky-vita-pocketjs-poc.vpk` using VitaSDK 2026.08 / cargo-vita 0.2.2. No install or hardware check.
- `git diff --check -- <six implementation paths>` — exit 0; the six paths are untracked in the dispatch baseline, so ordinary Git diff reports no patch for them. Candidate contents remain uncommitted.

## Changed scoped files

- `app/src/pocket.rs`
- `crates/bsky-pocket/src/visible_text.rs`
- `crates/bsky-pocket/tests/runtime.rs`
- `pocket-ui/src/Timeline.tsx`
- `pocket-ui/src/visible_text.ts`
- `pocket-ui/tests/visible_text.test.ts`

Hardware GXM rendering, live-feed session, performance, and device lifecycle remain unverified. This is software build/test evidence, not device acceptance.
