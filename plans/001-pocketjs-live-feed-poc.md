# Plan 001: Establish a live PocketJS timeline and validate migration risks

## Status and ownership

- Status: In progress. M0 admission recorded by Astra; current dispatch covers embedded guest host and packaged Solid M0 probe.
- Priority P1; effort L; risk HIGH; category migration; depends on none.
- Planned 2026-10-24 against bsky-vita commit `cff2cf6ae2d1fadbc4ea041b609e70588d9fc645`.
- Upstream research baseline: PocketJS `1c248fb18f976e91a313f11e0dbb35273c9e48dd` (MIT).
- Implementer: `openai/gpt-6-luna` (OpenAI).
- Independent reviewer: `openai/gpt-6.1-sol` (OpenAI).
- Supervisor: `openai/gpt-6-astra` (OpenAI), confirmed by the user. Do not silently substitute models/providers.
- Writable implementation workspace: `/home/evelyn/git/atprotocol/bsky-vita`.
- Implementation branch: `feat/pocketjs-live-feed-poc`. Create before source edits; do not implement on `fix/noto-color-emoji`.

## Outcome and non-goals

Build the first reusable slice of a replacement frontend, not a disconnected visual demo. A PocketJS timeline must consume the existing authenticated `bsky-worker` feed operations and prove text/emoji, variable-height virtualization, controller/touch input, bounded resources, and GPU/presentation safety. Live data is mandatory; deterministic synthetic cases supplement it.

Retain Rust authentication, OAuth, transport, session invalidation, worker operations, image decoding, native IME, camera and video services. Do not migrate every screen, add write operations (likes/posts/follows), replace TLS, modify the OAuth broker, or promise broader Unicode support than the measured font coverage. Unsupported embeds have explicit placeholders. The old UI remains the default and usable fallback while the PoC is opt-in.

## Workflow for the three models

Astra owns architectural decisions, milestone admission, permitted-file changes and final evidence assessment. GPT-6 Luna is the sole source writer. Sol reviews each milestone's actual diff, tests and evidence independently; it must not implement fixes. Astra sends actionable review findings back to GPT-6 Luna and requires a re-review. No concurrent writers in this checkout. Do not create a worktree unless an actual concurrent-write conflict requires isolation and the reason is recorded.

At each milestone GPT-6 Luna returns: baseline/head and uncommitted diff, files changed, commands and exit results, tests added, limitations, and hardware evidence status. Sol returns PASS / CHANGES REQUIRED / BLOCKED with file:line findings. Astra records acceptance separately from implementation status. A missing device result means PARTIAL, never PASS. If any requested model is inaccessible, report the exact provider/model error; do not substitute.

No automatic commits, pushes, PRs, merges, installs or device operations. Hardware deployment needs a separately authorized safe workflow. Preserve pre-existing modifications in `crates/bsky-media/Cargo.toml`, `crates/bsky-video/Cargo.toml`, and the untracked `AGENTS.md`. Never stash/reset/clean them. Recheck live status; this list is only the planning snapshot.

## Current state and drift checks

Run before implementation:

```sh
git status --short
git branch --show-current
git rev-parse HEAD
git diff cff2cf6ae2d1fadbc4ea041b609e70588d9fc645..HEAD -- app crates/bsky-worker crates/bsky-ui crates/bsky-render Cargo.toml Makefile
```

Also inspect unstaged/staged differences: committed drift checks do not capture the dirty baseline. Compare these excerpts with live code, and ask Astra to rebase the plan if semantics changed.

- `Cargo.toml:1-4`: `members = ["app", "crates/*"]`. New bridge crates join host tests; native dependencies must remain target-conditional. Root ring patch and dependency feature restrictions must survive.
- `app/src/main.rs:120-159`: polls input, creates `UiCtx`, then `let mut frame = render.begin_frame();` and calls `top.frame(&mut frame, &font, &ctx, &mut ime)`. Frame destruction presents. IME pumping ends native drawing. A second autonomous PocketJS presenter cannot be nested inside this block.
- `app/src/main.rs:173-220`: drains worker results after presentation, decodes/caches images, then calls `top.handle_worker_response(resp)`. Results go to the current top screen, not necessarily their requester.
- `crates/bsky-ui/src/screen.rs:39-49`:

```rust
fn frame(
    &mut self,
    frame: &mut Frame,
    font: &Font,
    ctx: &UiCtx,
    ime: &mut Ime,
) -> ScreenAction;
fn handle_worker_response(&mut self, _resp: WorkResponse) {}
```

- `crates/bsky-worker/src/lib.rs:126-129`: `FetchFeed { source: FeedSource, cursor: Option<String> }`.
- `crates/bsky-worker/src/lib.rs:378-390`: `FeedPage { source: FeedSource, batch: Result<TimelineBatch, String> }` and `Image { url: String, bytes: Result<Vec<u8>, String> }`. These do not currently carry request IDs.
- `crates/bsky-worker/src/lib.rs:545-547,565-597`: sequential worker, unbounded request channel, bounded response channel, image coalescing by URL/path. Do not bypass `try_recv` coalescing cleanup.
- `crates/bsky-ui/src/timeline.rs:98-145`: existing timeline loading/loaded/error state, source selection, pagination and cached variable row heights. Treat executable code, not its outdated introductory comments, as authoritative.
- `crates/bsky-render/src/lib.rs:1701-1906`: emoji-aware measure/draw/wrap functions; inspect their implementations and `emoji_font.rs` / `emoji_layout.rs` before designing a text adapter. Their existence is not proof of unrestricted script shaping.
- `app/Cargo.toml:8-16`: title BSKY00001, static asset packaging, Vita heap metadata. Preserve user data/install identity unless Astra explicitly approves an alternative.

Read AGENTS.md and relevant complete small files before editing; inspect symbol references before shared API changes. Follow nearby inline Rust tests (for example bounded I/O tests in bsky-net); do not add comments or unrelated formatting. Test immediately after each logical modification.

## Upstream facts to reproduce at the pinned revision

Use `gh` for GitHub inspection. Repository: https://github.com/pocket-nexus/pocketjs. Documentation: https://pocketjs.dev/docs/architecture/ and https://pocketjs.dev/docs/native-contract/.

Inspect pinned `profiles/vita.ts`, `programs/vita/Cargo.toml`, `programs/vita/src/{lib,instance,scene,service_bindings}.rs`, `devices/vita/pocket-vita-render/{README.md,src/lib.rs}`, `framework/src/virtual-list.ts`, `site/content/docs/microts-boundaries.md` and actual package/toolchain commands. Do not copy old blog paths (`hosts/vita`) into build automation; current code uses `programs/vita` and `devices/vita`.

Observed constraints:

- Stock Vita profile: physical 960x544, logical 480x272, density 2, baked glyph capability only.
- Vita GXM renderer says `TEXT_RUN` unsupported. Baked UI strings do not cover arbitrary remote posts.
- Stock VirtualList's `rowHeight: number` is uniform; no variable-height feed guarantee.
- Runtime and renderer are modular, but lifecycle/presentation ownership needs source-level verification. Upstream READMEs differ on frame-wait details; actual code wins.
- Vita renderer's default cache limits are candidate defaults pending hardware acceptance, not our application's measured budget.
- QuickJS host already exists. MicroTS can generate Rust views/models but native Vita AOT integration is not assumed turnkey. Start with Solid TSX plus QuickJS; switching to AOT requires an Astra-approved amendment, not a mid-task rewrite.

## Permitted files

Initial implementation scope (new paths are proposals, not existing APIs):

- `plans/README.md`, this plan, `plans/pocketjs-poc-evidence.md` (sanitized receipts/status).
- Root `Cargo.toml`, lockfile if tracked or required, `.gitignore`, `Makefile` for explicit PoC-only build/test targets.
- New `pocket-ui/` (Solid TSX components, fixture corpus, pinned JS dependencies, build/check/test scripts, assets metadata).
- New `crates/bsky-pocket/` (portable DTO/state/list/bridge tests; target-conditional runtime integration).
- `app/Cargo.toml`, `app/src/main.rs`, new `app/src/pocket.rs`, and generated PoC assets under `app/static/pocket/` only.
- `crates/bsky-worker/src/lib.rs` and directly affected worker callers under `crates/bsky-ui/src/` ONLY for additive request correlation integration.
- `crates/bsky-render/src/` and `crates/bsky-render/Cargo.toml` ONLY for narrowly necessary text/image/composition adapters; preserve old rendering behavior.

Astra must approve concrete file scope and adapter design before GPT-6 Luna starts each milestone. No broad permission to rewrite all UI files. New upstream patch files/vendor paths need explicit admission and a license/reproducibility strategy; never leave a dependency as an unrecorded dirty external checkout. No credential/session files, broker, existing font binaries/licenses or media/video manifest edits.

## Milestone 0: Establish baseline and choose one renderer owner

1. Record dirty baseline and baseline tests/build. Establish task branch safely in place.
2. Inspect actual pinned PocketJS code and prove a minimal Vita compile/link with this workstation's SDK. Check Rust toolchain mismatch (local nightly-2026-06-01; upstream documentation mentions nightly-2026-05-28). Do not globally change toolchains or remove required patches.
3. Choose a reproducible dependency strategy: exact-revision Git dependency if packages resolve correctly, otherwise explicit vendoring/patch strategy approved by Astra. Pin JS lock and QuickJS dependencies too. Confirm Bun and package manager requirements from upstream files, not guesses.
4. Produce an explicit ownership diagram: which component initializes vita2d, starts/ends scenes, presents, waits for GPU completion, allocates/retires textures, measures text, and pumps IME. Preferred approach: PocketJS core/guest embedded in our native shell with one coordinated renderer; use stock backend only if its lifecycle composes safely. Do not run two independently initialized native renderers.
5. Prototype one runtime-driven node with input and native text/image adapter probes. This gate establishes feasibility, not finished UI.

Verify: baseline `make test` and `make build` results recorded; exact pinned upstream build command exits 0; new opt-in PoC build emits a VPK; old default build still passes. Sol reviews linkage, ownership and adapter feasibility before Astra admits milestone 1. If text cannot be measured and painted consistently without a substantial upstream fork, report a bounded alternative and cost before proceeding.

## Milestone 1: Connect a correlated, bounded live-feed bridge

1. Create portable typed DTOs for post identity, author, display name/handle, full text, supported facets with UTF-8 byte range semantics, counts, context, quote summary and image metadata. Preserve moderation/content-warning state; hide unsupported/blocked content explicitly rather than accidentally disclosing it. Avoid copying Atrium internals into the JS API. Unsupported embed types receive named placeholders.
2. Add an additive correlated feed request/response variant or envelope with request ID plus mount/session generation, source and cursor. Preserve legacy requests/callers. Source equality alone is insufficient for refresh A/B races and leave/re-enter races. Assign/echo identity where the request is executed, not after dequeue. Do not retrofit IDs onto coalesced image responses unless their multi-consumer semantics are preserved.
3. Bridge JS commands to the existing WorkRequest execution, never JS-side fetch or a new auth stack. Whitelist commands: first page, next page, retry, refresh, visible image requests, leave PoC. No generic Rust-function/RPC/file/network access.
4. Consume worker responses once at a defined main-thread boundary; distribute to native global auth handling and the matching active mount. Preserve auth_failed detection even for stale results. Queue immutable DTO updates for the next PocketJS frame; never call QuickJS from worker threads or during native draw.
5. Bound accepted feed history (initial target 200 posts with anchor-safe trimming), pending feed requests (one active plus one coalesced latest refresh intent), image work (initial cap 8 pending unique assets), and command/DTO bytes. Define concrete limits and overflow behavior in code/tests. Native image bytes never enter the JS heap. Do not let stale generations monopolize the unbounded worker queue.
6. Retain old login/session lifetime and teardown. On logout invalidate/delete sessions before releasing worker/client; clear queued guest results and asset handles. Keep the PoC opt-in through a non-secret feature/launch selection with a clear return path to native UI. No accidental default UI replacement.

Verify: `cargo test -p bsky-pocket` and `cargo test -p bsky-worker` exit 0 with tests for successful/error/empty feed, end cursor, deduplication, malformed records/facets, same-source refresh race, switch/back race, stale image completion, logout with in-flight work, overflow/coalescing, and both auth dispatch paths preserved. `make test` passes. A live device session must request and display a real page through existing worker dispatch; no fixtures standing in for this result.

## Milestone 2: Close text, emoji and image integration gaps

1. Pick a single text provider contract returning layout metrics and rendering output for exactly the same runs, width, font size and raster density. Reuse existing native text/emoji implementation where possible, extending PocketJS resources/backend narrowly. Whole-feed screenshots or a native timeline hidden behind one PocketJS image do not count as a rebuilt UI. Bounded cached per-run/per-row text surfaces are acceptable only with explicit ownership, matching hit/layout metrics, invalidation and measured costs.
2. Establish behavior for text not supported by bundled fonts. Test Latin accents, combining marks, CJK and RTL samples, long unbroken strings, embedded NUL, malformed byte/facet ranges, newlines, variation selectors, skin tones, flags and ZWJ emoji. Do not label existing font limitations solved by a replacement-character fallback. Preserve grapheme boundaries in truncation; never split facet UTF-8 offsets as JS UTF-16 indices.
3. Validate 480x272 logical density-2 typography against current UI readability and list information density; keep layout/hit-test/raster conversions centralized. Alter profile only through an explicit supported custom-host contract, not scattered factors of two.
4. Fetch images through worker, bound CPU decode dimensions before GPU upload, retain native generation-tagged handles, and budget GPU residency by bytes as well as entry count. Initial PoC targets: combined image/text/emoji GPU cache <=48 MiB; bridge-owned queued payloads <=4 MiB; guest heap <=16 MiB. Astra may amend these before testing with evidence; never quietly raise them to make tests pass.
5. Preserve GPU completion before retirement and avatar RGBA requirements. Error/oversize images get placeholders and bounded retry, not frame stalls or request storms.

Verify: targeted Rust text/resource tests plus guest typecheck/tests exit 0; corpus proves measure/paint line breaks and clipping agree at density 2; image size limits, generation reuse and repeated eviction tests pass. Real Vita captures must establish color/sequence correctness and readable text. Browser/software raster results do not prove GXM parity.

## Milestone 3: Implement variable-height timeline and input

1. Build Solid TSX header, status/retry view, post rows, quote/image placeholders and pagination footer. Read-only: no account mutations. Expose original post IDs as stable row identity.
2. Implement variable-height windowing rather than passing estimates to uniform VirtualList. Maintain measured/estimated height index, overscan and mount bounds; preserve anchor post ID and offset when measurements/images change or old history is trimmed. Empty, single huge row and exhausted feed must work.
3. Measure using the same text provider as painting. Keep visible nodes bounded by viewport plus overscan; do not mount the retained 200-post history. Initial acceptance: <=30 mounted row roots in the reference corpus, independent of total history length.
4. D-pad moves selection, stick scrolls with deadzone, touch pan/tap and focus repair work together. Define Cross as confirm and Circle as back to match this client, overriding PocketJS defaults where needed. Preserve selection as offscreen rows unmount; test coordinate edges at density 2.
5. Native IME/camera/video remain outside PoC presentation scope. Ensure leaving the PoC and returning to existing native screens is safe; do not claim those native flows migrated. Rich facets need correct styling/layout and hit ranges; external link activation can remain disabled and explicitly marked.

Verify: deterministic tests for variable heights, delayed image size, append/prepend/trim anchoring (<=1 logical pixel drift), boundary hit tests, D-pad selection across unmount, stick drift, touch drag-vs-tap, loading/retry and all cursor states. Each new package must provide verified `check` and `test` scripts; run them plus `make test` and both default/PoC release builds. Fixture tests are mandatory but do not replace the live worker path.

## Milestone 4: Collect evidence and decide migration admission

1. Use the same deterministic corpus and scripted input on software preview, Vita3K if installed, and hardware. Label each evidence source honestly. Keep private feed content out of committed screenshots/traces; use synthetic content for public visual receipts and sanitized numeric logs for live feed.
2. Live acceptance: authenticate with each supported auth method where credentials/setup are available; following feed first page and at least two subsequent pages; refresh while a page is pending; retry after offline/error; leave/re-enter with request pending; logout/auth failure. Missing OAuth setup is a specifically recorded untested case, not an inferred pass.
3. Hardware soak: 10 minutes scrolling live pages/refreshing/leaving/re-entering and a separate repeated synthetic corpus run. Sample CPU frame time, missed present intervals, worker-boundary update cost, guest heap, native allocations, GPU bytes, mounted rows, pending work and post count. Exclude startup and explicit capture stalls; report them separately.
4. Targets for warm synthetic scrolling: p95 CPU frame <=16.67 ms, p99 <=33.3 ms; idle without input/work must not mutate/re-layout continuously; no unbounded rise in resources across 20 repeated equivalent cycles after warm-up (<=5% post-GC retained memory drift within configured caps). CPU time is not GPU time or input-to-display latency. Compare default UI with identical corpus where possible; report unavailable comparisons.
5. Add recorded fake-worker replay for delayed/out-of-order results; test the production DTO/state/list path, not a parallel mock implementation. Hardware evidence must include GXM output; CPU goldens alone are insufficient.
6. Sol performs final scope/security/lifetime review; Astra reports GO / CONDITIONAL GO / NO GO for wider UI migration, separately from tests. List upstream patches, maintenance cost and remaining blockers. No automatic migration of other screens.

Verify: all test/build gates pass and evidence table includes command, revision, SDK/toolchain, platform, expected outcome, actual outcome and receipt location. Hardware missing => overall PARTIAL/BLOCKED device validation, with all unblocked software checks still completed.

## Established verification commands

From repository root, one shell per invocation:

```sh
source /home/evelyn/dev/vitasdk/env.sh && make test
source /home/evelyn/dev/vitasdk/env.sh && make build
```

`make test` runs `cargo test --workspace --exclude bsky-vita-app`. `make build` enters app and runs `cargo vita build vpk -- --release`; expected artifact `target/armv7-sony-vita-newlibeabihf/release/bsky-vita.vpk`. Standard target/build artifacts are acceptable. Keep disk-backed TMPDIR from env.sh; /tmp previously hit quota. No established lint gate exists; run scoped rustfmt/clippy checks only after establishing baseline and do not reformat unrelated files.

Future commands `cargo test -p bsky-pocket` and guest `check`/`test` scripts apply only after creating those packages. At milestone 0 GPT-6 Luna must record exact verified Bun version, dependency install, upstream build, guest check/test and PoC VPK commands in the evidence file. They are intentionally not invented here. PoC build must be explicit and not alter default `make build` behavior.

## Mandatory review amendments (Sol review and Astra supervision)

These requirements refine the milestones above and take precedence over ambiguous admission wording. Plan review is not implementation authorization.

### Admission and scope

Astra's initial verdict is CONDITIONAL GO for milestone 0 only after these amendments; not approval for a full rewrite. A software gate may pass and the next software milestone proceed while named hardware checks remain deferred. Record each deferred device check, risk, owner and required receipt in an evidence ledger. Overall device validation stays PARTIAL; unconditional migration GO requires hardware GXM, live-feed, lifecycle, resource and supported-auth evidence. GPT-6 Luna must finish independent safe software checks when hardware is absent, rather than wait idle or fabricate a pass.

Milestone 0 must produce a concrete dependency/file map. Transport/auth/media/input changes needed for the safety requirements below are NOT implicitly permitted by the correlation-only worker scope. Astra must approve narrowly named additional files before edits; preserve both dirty media/video manifests. Sol reviews the revised scope against original intent. Substantial upstream forks, AOT changes, raised budgets or reduced acceptance coverage need explicit Astra approval.

### Guest containment and lifetime

At milestone 0 prove pinned Vita QuickJS allocation limits and effective interruption/deadlines. Start with a configurable 4 ms guest execution budget per frame, reviewed against measured hardware results, plus the 16 MiB guest heap cap. Interrupt handling must cover promise-job draining, not only the main frame callback. Bound native callbacks separately: JS interruption cannot stop a blocking native decode/network call. No network or image decode runs inside guest callbacks; perform bounded, budgeted native work outside drawing. Record any unavoidable uninterruptible work and its worst-case size/time; unresolved unbounded work blocks admission.

Contain runtime exceptions, allocation exhaustion and infinite callbacks: stop guest scheduling, revoke commands, release subscriptions/jobs, invalidate guest-owned handles, wait for outstanding GPU readers, dispose runtime exactly once, and return to native UI with a non-secret failure notice. Do not unwind through C/FFI. Test infinite callback, promise-job loop, heap exhaustion, callback exception, partial initialization failure and repeated mount/dispose with queued worker results. Deadlines and failure injection need an injectable host clock/test hook; do not claim host stubs prove device enforcement.

### Authentication and protocol ownership

Keep three distinct identities: authentication generation, UI mount generation and request ID. A stale mount result within the CURRENT authentication generation still participates in global auth-failure handling. An obsolete authentication generation must NEVER invalidate the current session, delete newer session files or deliver content to a newer guest. Test old-request auth failure after successful logout/reauthentication, including cleanup of all sidecars. Preserve existing invalidation write-gate ordering.

Version DTO/command schemas; reject unknown commands, invalid numbers, oversized strings/collections and stale/cross-owner handles before work admission. Initial bridge limits: <=64 KiB UTF-8 text per field, <=64 KiB per post DTO, <=1 MiB per serialized delivery, <=128 commands per admitted batch, nesting depth <=16. Oversize remote records become explicit bounded placeholders, not silently truncated facet ranges. Packaged guest code only; feed text never becomes executable code, markup evaluation or module paths. No credentials in guest data or telemetry.

Bound admission BEFORE the unbounded shared worker channel. Add finite request deadlines and bounded stale-work disposition without abandoning existing HTTP response/auth semantics. Preserve existing consumers and image coalescing. Test queue saturation with legacy callers, stale work, pending logout and refresh. Dropping result interest is not cancellation of running native I/O; report actual cancellation/deadline guarantees.

### Preallocation and aggregate resource bounds

Retain existing streaming compressed-byte limits (16 MiB XRPC, 24 MiB image) and verify their enforcement before buffer growth in bsky-net. Validate image headers and checked width*height*channels BEFORE full decode: initial maximum 16 megapixels input and 4 MiB decoded output per preview, one active decode. A codec that cannot honor safe bounded downscaling must reject over-budget images rather than decode first and resize later. Keep worker image responses subject to byte backpressure; pending-count caps alone are not a memory budget. Add large-header, decompression-bomb, integer-overflow and transient-copy tests. If current media APIs cannot enforce this, seek narrow scope admission rather than hide the problem in GPU limits.

In addition to existing cache/heap caps, milestone 0 must establish an aggregate process allocation budget/headroom from available Vita memory and baseline. Include QuickJS native overhead, JSON parsing, transport buffers, decoder scratch, DTO copies, legacy assets and GPU submissions awaiting retirement. Specify deterministic reject/evict behavior and peak measurements. Supervisor approves the numeric whole-process limit before milestone 1; a post-GC snapshot alone does not satisfy it.

### Moderation policy

Inspect existing executable moderation behavior and identify authoritative metadata/relationships returned by current requests; do not assume the worker supplies a moderation engine. PoC policy: blocked/muted authors and unavailable/blocked records do not reveal text, quote bodies or images; content warnings show a placeholder with no reveal action in this PoC; unknown required moderation state fails closed with an explicit unavailable placeholder. Apply independently to outer post and quoted author/content. Filter before sending DTO content to guest AND before scheduling image requests; never recover hidden text in diagnostics. Fixtures must verify both no rendered content and no hidden asset request. If follow/block/mute/label data are insufficient, Astra must admit a narrow data-fetch/policy scope or record live moderation as blocked. Do not declare compliance based on placeholders for every live post.

### Suspend, resume, exit and fallback

Define explicit active/suspended/resuming/disposing states. Pause guest execution on suspend, suppress delivery while paused, retain only bounded results, reset touch/button edges and clock catch-up on resume, and validate/recreate GPU resources before drawing. Session identity still gates queued results. Application exit cancels admission and follows the same idempotent teardown contract. Test repeated transitions, suspension with in-flight feed/image work, runtime fault during resume, and back-to-native navigation. Hardware resume evidence is mandatory for final device PASS; unsupported resume must take a safe native fallback and remain a named migration blocker.

Add regression checks for legacy response routing, both auth modes and native fallback. Record separate default/PoC artifact filenames, feature configuration and hashes without changing installed title/data identity automatically. No deployment is implied by a successful package build.


Complete independent unblocked checks, then ask Astra for a decision if: pinned dependency cannot build after distinct reasonable attempts; required integration exceeds admitted scope; lifecycle requires conflicting renderer owners; text needs a substantial upstream fork; correlation cannot distinguish old same-source responses; decoded assets breach bounds; private credentials/content would enter receipts; dirty baseline conflicts with task branch establishment; or hardware/access is unavailable. Do not hide blockers with Latin-only fixtures, uniform rows, fake live data, a second network stack, disabled auth checks or raised budgets.

## Done criteria and maintenance

- [ ] Opt-in PocketJS timeline uses existing authenticated feed worker and live pagination.
- [ ] Both existing auth methods remain supported; testing coverage is explicitly reported.
- [ ] Correlation/stale-result/auth-teardown tests and bounded queue/cache tests pass.
- [ ] Variable-height scroll anchoring, input and mixed text/emoji corpus pass.
- [ ] Default UI and PoC release VPKs build, all host/guest tests pass.
- [ ] Hardware gaps have measured evidence, or completion is explicitly PARTIAL.
- [ ] Sol review resolved; Astra migration verdict and upstream maintenance costs recorded.
- [ ] No out-of-scope modifications; original dirty files preserved; no commit/push.

Future screens must use the same typed bridge, text provider and presentation owner; do not introduce per-screen runtimes or parallel caches. Keep upstream exact-revision pins and patch inventory synchronized. Never turn fixture results into claims about all Unicode, arbitrary feeds or device performance.

### Critical Files for Implementation

- `app/src/main.rs`
- `crates/bsky-worker/src/lib.rs`
- `crates/bsky-render/src/lib.rs`
- `crates/bsky-pocket/src/lib.rs` (new)
- `pocket-ui/src/Timeline.tsx` (new)

## Superseding recovery amendment: portable feed hexagon

This section supersedes conflicting architecture, sequencing, scope, and acceptance language above for the recovery task. It does not authorize implementation in this plan-amendment task. The live-feed/PocketJS UI work remains unaccepted; architecture recovery is **in progress**, and the original live-feed milestones are not accepted by this amendment.

### Ownership and architecture

- Add portable `crates/bsky-feed`, using only `std`, `serde`, and `serde_json`. It owns feed DTOs, correlation validation, moderation policy, and the **single production `FeedCoordinator`**. The coordinator owns admission and state transitions for mount, request/cursor, retry, history, refresh eligibility/coalescing, and delivery. Keep native integrations out of this crate.
- `bsky-worker` depends on `bsky-feed`, never on `bsky-pocket` or QuickJS. It performs bounded delivery and reports command completion through the feed contract; it does not duplicate feed admission or cursor/history policy.
- The native app owns one persistent coordinator, surviving shell transitions. Authentication epoch, application lifecycle, and session cleanup remain native responsibilities. Authentication/session invalidation ordering remains unchanged.
- The PocketJS shell only translates guest commands/events, draws, and publishes effects. It owns rendering only: no pending-request, cursor, history, or refresh policy clones.

The contract covers auth change, mount/open, close, guest command, command completion, and effects for fetch, bounded delivery, status presentation, and invalidation. Correlation must match exactly before a completion can mutate coordinator state. Stale and duplicate completions are no-ops. Closing a mount does not claim to cancel already-running native work. Refresh intents coalesce to the latest eligible refresh. Extract and preserve current retry behavior; do not add product features as part of this recovery.

### Recovery stages and review gates

1. Snapshot the current relevant files and behavior before editing, preserving the dirty baseline; establish baseline tests and record known failures separately.
2. Extract portable feed DTOs, correlation validation, and moderation policy into `bsky-feed` without changing behavior.
3. Consolidate all production feed admission/state policy into `FeedCoordinator`; make the worker and native shell adapters use it, removing duplicate shell policy.
4. Freeze the candidate for one consolidated Sol review. Implement only the agreed repair batch, then freeze a newly identified snapshot for delta review if edits occur.
5. Run focused package/coordinator tests and dependency-tree checks, then the app host and both default/PoC SDK build configurations as available. Keep hardware validation explicitly deferred.

A known DNS-related test-suite failure/hang is baseline evidence, not an architectural-task success. Record its exact command and outcome; do not claim the full suite passes. After one unproductive attempt to resolve that environment failure, diagnose and report it directly rather than repeating the same redispatch. Do not weaken/remove tests or mask the failure.

### Recovery scope and acceptance

Authorized source scope for implementation of this recovery plan is limited to the new `crates/bsky-feed` crate and the directly required app, Pocket shell, worker manifests/consumers, tests, lockfile, and evidence. No broker, vendored upstream, renderer, or network/transport changes are in scope. If a necessary change falls outside this scope, stop and request the exact extension with rationale.

Acceptance requires production coordinator tests for correlation races, stale/duplicate completion no-mutation, caps/admission and latest-refresh coalescing; a worker dependency tree with no PocketJS/QuickJS dependency; feed package/core tests; and app host tests plus both default and PoC SDK build configurations. Hardware remains deferred and cannot be represented as passed. Do not commit or push.
