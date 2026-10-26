# PocketJS media pipeline: build state and handoff

Branch: `feat/pocketjs-live-feed-poc`
Last commit at time of writing: `cedc58b`

## What is committed and verified

Software-only verification only. No device run has been performed since
`de814bb` (phase 2). Every commit below passed the host workspace suite
(`cargo test --workspace --exclude bsky-vita-app`), the guest suite
(`cd pocket-ui && bun run check && bun test`), `git diff --check`, and the
feature-enabled Vita build (`make build-pocket`).

| Commit | Scope |
| --- | --- |
| `0b74a9e` | Pixel scrolling, clipping, bounded wrap cache (phase 1) |
| `de814bb` | Right-strip touch drag sharing D-pad scroll state (phase 2) |
| `8f2f79f` | Native media envelope; account/slot-bound descriptors (phase 3a) |
| `eedfc0a` | Deadline-aware capped media GET (phase 3b) |
| `cee77dc` | Isolated bounded media lane with byte/dedup caps (phase 3c) |
| `2bebbf6` | Account-scoped registry wired to accepted envelopes (phase 3d) |
| `6fbf076` | Persistent on-disk asset cache with TTL and LRU quota (phase 4) |
| `3b27e57` | Handle-keyed image draw command (phase 5a) |
| `985f104` | IconaMoon icon atlas + generated table + build tool (phase 5b) |
| `dce903f` | Descriptor lookup and bounded visible submit (phase 5c) |
| `cedc58b` | Texture-cache clear on account teardown; bounded cache filenames |

Test counts at `cedc58b`: `bsky-worker` 51, `bsky-pocket` 24 unit + 2
containment + 3 QuickJS + 8 runtime, `bsky-render` 13, `bsky-media` 12,
guest TS 20.

## Integration is NOT complete

An independent review at `dce903f` established that the media machinery is
correct in isolation but not yet connected end to end. Treat these as the
remaining work before the feature is functional:

- **Completions are discarded.** `app/src/main.rs` calls
  `drop(pocket_media.drain())`. Fetched bytes never reach the texture cache,
  so no avatar or image ever appears from the new path.
- **Nothing schedules a fetch.** `PocketMediaRegistry::submit` /
  `submit_visible` are never called from `app/`. `drain()` therefore returns
  empty in practice.
- **The disk cache is never constructed in production.** `AssetCache` is
  only built in its own tests, and the app never calls it.
- **The handle map is never populated.** `ImageHandleMap::insert` is called
  only in render tests, so `draw_image_handle` always resolves to `None` and
  draws nothing.
- **The icon atlas has no consumer.** `pocket_icon_table` is registered and
  compiles, but no command or draw path uses it.

Until those are wired, `NativeCommand::ImageHandle` is inert and the icons
are dead weight.

## Known remaining defects from review

- **P3** disk quota can overshoot by up to one image between write and evict.
- **P3** each media job reserves the full 24 MiB image cap regardless of real
  size, capping concurrency at roughly four fetches.
- **P3** `ImageHandleMap` rejects at capacity but has no `clear`/`remove`, so
  handle reuse across frames needs deliberate handling when wired.

## Requires a device

None of the following can be validated on the host:

- actual GPU rendering of `draw_image_handle` / `draw_texture_scale`, and
  whether avatars mask cleanly (the earlier black/red stripe defect class);
- real GNUP/GXM memory pressure from the 24 MiB cap and cache eviction;
- disk-cache behavior on ux0 including `set_times`, atomic rename, and quota
  eviction under real filesystem semantics;
- CDN transfers over the Vita network stack (TLS, DNS, redirects);
- thread teardown timing and touch feel.

## Handoff

The PoC VPK is produced at:

```
target/armv7-sony-vita-newlibeabihf/release/bsky-vita-pocketjs-poc.vpk
```

Install it with VitaShell. Do not use `make install` or `make run` for this;
both rebuild the default UI without the `pocketjs-poc` feature.
