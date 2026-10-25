# Pocket context layout delta — verification snapshot

Baseline commit: `cff2cf6ae2d1fadbc4ea041b609e70588d9fc645` on `feat/pocketjs-live-feed-poc` in `/home/evelyn/git/atprotocol/bsky-vita`. This snapshot records the scoped layout delta only; all changes remain uncommitted.

## Change

- Context markers are joined from the live feed post by URI before the existing `layoutRows` calculation. The measurement and placement message contracts are unchanged.
- Each visible context row adds the 12 px label plus 2 px gap to card height and body origin once. Hidden rows receive no context marker.
- Measured width is treated as inner body width; cards add 8 px on each side, capped to the 456 px available outer width. Native placement uses `body_x`/`body_y` from the same row layout.
- TS layout regression covers mixed contextual clean and hidden rows, one/two-line body metrics, exact body/card deltas, width, placement equality and row non-overlap. The QuickJS runtime test delivers context-bearing feed DTOs and exercises the measurement callback contract with a controlled runtime harness. The packaged guest is rebuilt by the guest checks; the harness does not claim to execute that packaged callback.

## Verification

Pending. Record exact guest rebuild and test commands/results here before handoff. No device operation is included.
