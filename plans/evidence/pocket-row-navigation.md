# Pocket row-navigation repair evidence

## Baseline and scope

Candidate: `/home/evelyn/git/atprotocol/bsky-vita`, branch `feat/pocketjs-live-feed-poc`, baseline commit `1a0abda4e711b8ebae383019aebd566342c4ced7`. The supplied baseline was already dirty in unrelated app/native/worker/UI files and contained an untracked build log; these were preserved. This task changed only `crates/bsky-pocket/tests/runtime.rs`, `pocket-ui/src/Timeline.tsx`, and this evidence file. Changes remain uncommitted.

## Coverage

The packaged guest runtime is fed D-pad `ButtonSnapshot` bitmasks using `EmbeddedGuest::advance_with_deadline`, with `present_with_deadline` completing each frame and zero-button frames explicitly releasing each press. Coverage establishes first-row selection, verifies UP clamps at the first row, traverses 150 DOWN edges into a 200-post feed, moves two more rows and then back UP, and asserts the visible three-row window. Direct test-only select/move setters were removed. Feed replacement with the selected URI retained preserves selection; replacement without that URI selects the first available post. A status-error callback retains the selected visible window.

## Phase ordering and verification

For each input snapshot, the test calls `advance_with_deadline(buttons, deadline)` before `present_with_deadline(deadline)`; both phases consume the same absolute `Instant` deadline (no per-phase budget refresh). The regression test advances within budget, then supplies an expired presentation deadline and verifies it fails closed with `guest deadline exceeded`. Assertions inspect the guest's published visible window only after presentation. This follows the production advance-then-present lifecycle and avoids treating a direct callback invocation as input verification.

- `bun run check` from `pocket-ui/`: exit 0; TypeScript no-emit check and Vita-target package rebuild passed. Output `dist/timeline-probe.pocket` was 672120 bytes.
- `cargo fmt -- crates/bsky-pocket/tests/runtime.rs`: exit 0.
- `cargo test -p bsky-pocket --test runtime`: final run exit 0; 3/3 runtime tests.
- `cargo test -p bsky-pocket`: exit 0; 14 unit, 2 containment, 3 QuickJS, 3 runtime tests; doc tests 0.

The first focused runtime run failed at the newly added assumption that the feed callback left the visible-window string empty; that assumption was removed. Follow-up runs exposed an invalid 201-post delivery and stale-string comparison in the expanded assertions; inputs and comparison were corrected. Final runtime and crate suites passed.

No full workspace test, device/Vita VPK build, race test, or hardware validation was run. Guest runtime checks are software-only; host tests cannot validate GPU/native timing, and runtime deadlines are not hard preemption of native operations. No commit or push was performed.
