# Phase 1 variable-height scrolling evidence

Baseline: commit `916cb813857c35faecdd0b0a775c550ab2e8b989`, branch `feat/pocketjs-live-feed-poc`; baseline working tree contained pre-existing task changes and untracked files. All subsequent changes remain uncommitted.

## Current checkpoint

- Incremental wrapping uses a shared bounded cache, with work capped at 64 measurement calls and 8 KiB total per request; candidate work is also limited to 3 rows per frame.
- Signed viewport coordinates and clipping bound visible drawing to y=40..264.
- Pixel-based D-pad scrolling traverses full posts, with packaged-runtime coverage for scrolling beyond the initial window, preserving measured placements, and returning through content.
- Native multiline measurement uses a batch of at most 8 commands and a 4 KiB message limit.
- The packaged actual-runtime suite has 7 tests; the guest TypeScript suite has 16 tests.
- Supervisor disposition: narrow software GO only. No device execution or hardware acceptance is implied.

## Final verification

- `source /home/evelyn/dev/vitasdk/env.sh && timeout 600s make build-pocket` — passed. Generated `pocket-ui/dist/timeline-probe.pocket` (681112 bytes) and completed the release VPK build at `target/armv7-sony-vita-newlibeabihf/release/bsky-vita-pocketjs-poc.vpk`. Build emitted compiler warnings, including upstream QuickJS warnings.
- `timeout --kill-after=5s 180s cargo test --workspace --exclude bsky-vita-app --quiet && git diff --check` — passed (exit 0): 22 workspace unit tests and 7 runtime tests passed; the established host gate excludes only the console binary `bsky-vita-app`.
- The earlier `cargo test --workspace` attempt included `bsky-vita-app` and failed while linking its host tests against the Vita ARM `libpthread.a`. This was an unsupported command for the documented host gate, not a failure of the established host gate.
- Latest `make build-pocket` — passed.

No formatter was run and no source files were changed for this evidence checkpoint. No device install or hardware runtime validation was performed. No commit or push was made. The pre-existing untracked `pocketjs-poc-build.log` was preserved.
