# Phase 2 touch completion checkpoint

- Baseline: `0b74a9e43bc673f2b83909cda8518d943c36068e`, branch `feat/pocketjs-live-feed-poc`.
- Implementation: front-panel contacts from the existing `TouchFrame` are clamped to 960×544, converted once to logical 480×272 coordinates, copied into a six-contact fixed-capacity snapshot, published to the packaged guest, and consumed before its existing frame advance. The guest uses the same frame deadline; no event queue, orchestration loop, or inertia was added.
- Timeline touch behavior: a single contact must begin in the logical right rail `[456,480) × [40,264)`. Capture is by contact ID and continues off-rail. Drag delta modifies the existing pixel scroll offset, just as a signed scroll input does. A contact that only taps produces no scroll/click; other contacts cannot acquire an active capture. Once multitouch overlaps capture, that gesture remains cancelled until all contacts clear. D-pad controls remain in the existing path. Exiting or unmounting disposes the guest and therefore its gesture state.
- Focused coverage: fixed six-contact snapshot/truncation and stable IDs in Rust; packaged runtime snapshot consumption with D-pad frame advancement; UI coordinate edges, outside-rail capture, release, cancellation, multitouch non-stealing and native-to-logical density conversion.

## Verification

| Command | Result |
| --- | --- |
| `cargo test -p bsky-pocket` | PASS: 23 unit tests, 2 guest-core tests, 3 containment tests, 8 runtime tests, doc tests. |
| `bun run check` (from `pocket-ui/`) | PASS: TypeScript no-emit check and packaged guest build. |
| `bun run test` (from `pocket-ui/`) | PASS: 17 tests and package inspection (1 test). |
| `make test` | PASS: workspace tests, excluding the Vita binary as configured. |
| `source /home/evelyn/dev/vitasdk/env.sh && make build-pocket` | PASS: release Vita build/link and `target/armv7-sony-vita-newlibeabihf/release/bsky-vita-pocketjs-poc.vpk` packaging. Artifact SHA-256: `320388c3e8c4eef437421f0ff16299eae3d3316684bbfc3c0991a3f4eef53d47`. |
| `cargo check --manifest-path app/Cargo.toml --target armv7-sony-vita-newlibeabihf --features pocketjs-poc` | NOT PASS: standalone Cargo check does not use cargo-vita's `-Z build-std`; target core/std are not installed. The cargo-vita release build above did compile and link the feature successfully. |
| `cargo check --manifest-path app/Cargo.toml --features pocketjs-poc` | NOT RUN successfully: native build scripts require VITASDK and target wrapper dependencies; command was run without exported SDK and failed. |

Host tests prove bounded protocol behavior, not Vita touch sampling, rendered scrolling or input latency. No device interaction or touch hardware evidence was performed; device behavior remains unverified.

## Candidate patch identity

- Patch is uncommitted on `feat/pocketjs-live-feed-poc` relative to the stated baseline.
- Scoped implementation changes: `app/src/main.rs`, `app/src/pocket.rs`, `crates/bsky-pocket/src/adapters.rs`, `crates/bsky-pocket/src/lib.rs`, `crates/bsky-pocket/src/quickjs.rs`, `crates/bsky-pocket/tests/runtime.rs`, `pocket-ui/src/Timeline.tsx`, `pocket-ui/src/visible_text.ts`, `pocket-ui/tests/visible_text.test.ts`, and this evidence file.
- Patch SHA-256 for those implementation files before this evidence file: `17700cc1ec83207d0db09f41bcf74564ddc6843861292329946205d02b4b1205`.
- Existing unrelated dirty files `crates/bsky-render/src/emoji_font.rs`, `crates/bsky-render/src/video_shader.rs`, and untracked `pocketjs-poc-build.log` were preserved unchanged by this work. Their content is excluded from the patch identity above.
