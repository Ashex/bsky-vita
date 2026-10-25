# Working in bsky-vita

Native Rust Bluesky client for homebrew PlayStation Vita, plus a separate TypeScript Cloudflare Worker for OAuth callback relay. The application is not a desktop GUI: host library tests exercise portable logic and hardware stubs, not the actual renderer or console integration.

## Commands and target selection

Run Rust workspace commands from the repository root. `app/.cargo/config.toml` selects `armv7-sony-vita-newlibeabihf` and `arm-vita-eabi-gcc` only when Cargo runs from that directory. The Makefile deliberately enters `app/` for console builds and stays at the root for tests.

| Command | Purpose / constraints |
| --- | --- |
| `make test` | `cargo test --workspace --exclude bsky-vita-app`; excludes only the console binary, not the hardware-wrapper library crates. |
| `make build` | Release VPK using `cargo vita build vpk -- --release` from `app/`. Output: `target/armv7-sony-vita-newlibeabihf/release/bsky-vita.vpk`. |
| `make build-fast` | Dev-profile VPK, optimized at level 1 without release LTO. Useful for iteration, not representative performance testing. |
| `make install` | Builds and uploads the full VPK to the device download directory; installation still requires VitaShell. |
| `make run` / `make run-fast` | Build, update the executable on an already-installed device, and launch it. These are hardware operations, not local verification. |
| `make push-font`, `make push-emoji`, `make push-mask`, `make push-gxp` | Explicitly update installed assets; executable-only updates do not refresh them. |
| `make fetch-log` | Print the device runtime log via FTP. |
| `make fetch-gxp` | Download captured shader binaries into `app/static/`, modifying repository assets. |
| `make push-creds` | Upload local, gitignored `credentials.toml`; see `credentials.toml.example`. Use an app password, not the account password. |
| `make help` | Full device workflow and target descriptions. |

Console builds require VitaSDK with `VITASDK` set and `cargo-vita`. `rust-toolchain.toml` pins `nightly-2026-06-01` with rust-src, clippy, and rustfmt. The Vita standard library is built through cargo-vita / `-Z build-std`, not a rustup-installed target. Device transfer/launch commands require `VITA_IP`; FTP helper targets use curl.

Host tests do not require VitaSDK: native SDK dependencies in wrapper crates belong under `[target.'cfg(target_os = "vita")'.dependencies]`. Unconditional `vitasdk-sys` dependencies leak ARM library search paths into Linux linking and can select the SDK's ARM `libpthread.a` instead of host libraries. `DOCS_RS=1` bypasses the SDK build-script check but does not make native libraries host-compatible.

### Verified local environment

This workstation has VitaSDK at `/home/evelyn/dev/vitasdk`, extracted from the official `vitasdk/vitasdk:2026.08` Docker image (digest `sha256:8227da1b596d981f4513cd9b36bda1e43857f6dae5bcf4312c1cd40167f7d9bb`), and `cargo-vita` 0.2.2. SDK status reports release 2026.08, sequence 2026092555, GCC 15.2.0. Activate it for the current shell with:

```sh
source /home/evelyn/dev/vitasdk/env.sh
make test
make build
```

The activation script sets `VITASDK`, prepends SDK tools to `PATH`, and selects disk-backed `/home/evelyn` for `TMPDIR`; `/tmp` hit a user quota during native compilation despite reporting free filesystem space. Shell startup files were not modified. All 44 host unit tests and release VPK packaging passed with this setup; actual GPU behavior still requires a Vita. A stopped Docker container named `bsky-vita-sdk-setup` was used only to extract the SDK.

The repository has no checked-in CI workflow or dedicated Rust lint target. Do not confuse available rustfmt/clippy toolchain components with an established passing lint gate.

### Broker commands

Run from `broker/`; its README specifies Node 22+ and `npm install`.

- `npm run dev`: local Wrangler development server.
- `npm run deploy`: deploy the Worker. The checked-in configuration names an actual custom domain and KV namespace; do not use deployment as a test.
- `npm run types`: generate Wrangler bindings, **not** a typecheck.

`broker/package.json` contains no test, lint, or typecheck script. `broker/tsconfig.json` enables strict TypeScript, `noUncheckedIndexedAccess`, and `noEmit`.

## Architecture and change entry points

| Area | Responsibility |
| --- | --- |
| `app/src/main.rs` | Owns device initialization, renderer/assets/cache, input and IME, navigation stack, authenticated client, and worker lifetime. |
| `crates/bsky-ui/src/screen.rs`, `widget.rs`, screen modules | `Screen` / `ScreenAction`, frame context, widgets, and screen-local state. `tabbar.rs` defines top-level navigation; `hints.rs` supplies the shared control overlay. |
| `crates/bsky-worker/src/lib.rs` | Typed `WorkRequest` / `WorkResponse` protocol and sequential background execution of network/media work. |
| `crates/bsky-auth/` | Identity/PDS resolution, app-password login/resume, persistence, and common dispatch over password and OAuth agents. |
| `crates/bsky-oauth/` | OAuth client/flow state, session persistence, broker endpoints, and shared session-file recovery/invalidation helpers. |
| `crates/bsky-net/` | Blocking ureq/rustls transport implementing Atrium's async HTTP interface; bounded response reads and streamed video downloads. |
| `crates/bsky-models/` | Re-exports of Atrium types, rather than a separate competing API model layer. |
| `crates/bsky-render/` | vita2d/GXM FFI, frame/texture ownership, cache, fonts, emoji, theme, and video shaders. |
| `crates/bsky-input/`, `bsky-ime/` | Controller/touch polling and native keyboard dialog integration. |
| `crates/bsky-media/`, `bsky-video/` | Image decode/scale/encode, camera/gallery, and sceAvPlayer video/audio integration. |
| `crates/bsky-log/` | Disk-backed runtime logging. |
| `broker/src/index.ts` | OAuth metadata, callback pages, and short-lived callback pickup via KV. |

`bsky-store` is a placeholder, not the active persistence layer. Session storage is in auth/OAuth; texture caching is in rendering and request coalescing in the worker. Despite references in README/workspace exclusions, there is no current `static-site/` directory: the broker serves metadata and callbacks.

### UI and worker flow

The main loop polls input, calls the top screen's `frame`, presents when `Frame` drops, invokes `after_present`, drains worker responses, then applies navigation actions. Read the implementation in `app/src/main.rs` for exact ordering; introductory comments can lag it.

- Screen rendering/input produces a `ScreenAction`; screen code does not directly own the navigation stack. `AuthComplete` starts the worker and retains an `Arc<AuthClient>`.
- Post-login work goes through `UiCtx.worker` and typed requests. Add the request, response, worker dispatch, and screen response handling together when adding an operation. Login retains an `after_present` path because no authenticated worker exists yet.
- Responses are delivered to the **currently topmost screen**, not necessarily the requester. Account for stale results and screen-local pending state when navigating or reusing response variants.
- Remote image responses populate the shared texture cache before reaching the screen. Local image responses bypass that cache so picker/compose screens can decode scaled previews.
- Tab switching truncates to an existing matching screen in the stack when possible; otherwise it replaces the stack with a new root. It is not a persistent cache of every tab.
- One worker processes requests sequentially. Its request channel is unbounded; its response channel is bounded to 128 items, not a byte budget. Image requests are coalesced until their responses are drained. Upload payloads can contain large byte vectors.
- Dropping the worker closes channels but does not join or cancel a request already running.

## Correctness constraints worth preserving

### Authentication and persistence

- `AuthAgent` has distinct password and OAuth implementations. Worker operations use `bsky_auth::agent_call!` to dispatch typed Atrium calls across both; avoid accidentally implementing a feature for only one authentication method.
- App-password resume re-resolves the user's PDS rather than trusting an endpoint saved before an account migration.
- Keep the originating OAuth client alive through callback completion: its flow state holds PKCE/DPoP material. Consult `bsky-oauth/src/client.rs` and `bsky-ui/src/login.rs` before changing flow/cancellation ownership.
- Logout/auth-failure teardown in `app/src/main.rs` calls `invalidate_and_delete_sessions` **before** releasing the worker/client. The shared generation and write gate stop an in-flight refresh from resurrecting deleted credentials. Merely dropping the worker does not provide this guarantee.
- Session recovery in `bsky-oauth/src/atomic_json.rs` prefers a valid `.tmp` sidecar. Deleting only the primary JSON file can restore a supposedly deleted session. Preserve cleanup of both auth methods and their sidecars, and the corruption/recovery tests. Remove-then-rename is not an atomic filesystem replacement.
- Do not log tokens, app passwords, authorization codes, PKCE/DPoP secrets, or full callback query strings. Broker source explicitly documents request-data logging restrictions and disables observability in Wrangler configuration.

### Network and memory

- Atrium-facing futures wrap blocking ureq I/O, driven with `futures::executor::block_on`; an async signature does not make an operation safe to run during `Screen::frame`. There is no general Tokio runtime to build new code around.
- Preserve HTTP status and error bodies in `VitaHttpClient`: Atrium needs them to recognize auth failures and refresh tokens.
- Transport caps are 16 MiB for XRPC, 24 MiB for images, and 120 MiB for video. Video downloads stream through a `.part` file with cleanup. Read/write timeouts are inactivity timeouts, not total request deadlines, so slow work can hold up the single worker.
- Compressed-image byte limits do not bound decoded memory. Local photo previews must CPU-decode/downscale before GPU upload rather than allocating full-resolution GPU textures. See `bsky-media/src/image.rs` and the scaled texture paths.

### GPU and native resource lifetimes

- Rendering uses global native state on the main thread. Keep decode/upload/cache mutation on that thread; worker payloads are not GPU textures.
- The GPU can still read a texture after a Rust draw call returns. Preserve GPU-completion waits before freeing or evicting drawn textures; Rust ownership alone does not synchronize GPU work.
- Avatar masking requires four-byte RGBA textures. Ordinary JPEG decoding yields three-byte RGB; main's `insert_rgba` avatar path avoids corrupt pixels when applying alpha masks.
- Video frames borrowed from sceAvPlayer are valid only until the next frame fetch. The chroma format is NV12, not planar YUV420. File callbacks can run concurrently; preserve the mutex covering seek plus read.
- Native IME buffers must remain heap-stable for the dialog lifetime. Moving/replacing backing buffers while the dialog retains pointers is unsafe even if ordinary Rust borrows permit it.

## Build and asset traps

- Preserve the root `ring` patch to the Vita-specific branch: upstream ARM assembly selection causes missing symbols on this target.
- Dependency default features are deliberately disabled: ureq gzip pulls a problematic ARM NEON path; Atrium defaults can pull unwanted runtime, resolver, or HTTP-client dependencies. Check target implications before enabling them.
- Native static-library ordering is deliberate in `bsky-render/build.rs` and `bsky-media/build.rs`. Do not reorder link directives as cosmetic cleanup.
- Do not add `strip = "symbols"` to Cargo profiles: removing relocation symbols breaks vita-elf-create. Release retains debug information in the ELF for address lookup; cargo-vita handles packaging/stripping.
- `app/static/` mirrors the VPK asset layout. Installed assets are read under `app0:`; writable application data and `run.log` live under `ux0:/data/BSKY00001/`.
- `python tools/build-avatar-mask.py` requires Pillow and regenerates both background-specific masks. Theme background changes may require regenerating both.
- `python tools/build-twemoji.py` requires Pillow, requests, network access, and Python supporting its union-type syntax. It generates **both** `app/static/twemoji.png` and `crates/bsky-render/src/emoji_table.rs`; keep these synchronized and do not hand-edit the generated lookup table. It fetches `@latest` and skips failed downloads, so a partial download can still overwrite outputs. Do not run it as routine validation.
- Bundled video shader GXP assets avoid requiring a runtime shader compiler on end-user consoles. Recapturing them requires successful compilation on a device with `libshacccg.suprx`, then `make fetch-gxp`; normal installation uses the bundled files.

## Broker contract

The Worker accepts GET routes `/`, `/client_metadata.json`, `/callback`, `/callback-qr`, and `/pop`; the QR pickup page is currently a stub. Non-GET requests return 405, unknown routes 404.

Callbacks require nonempty `state`, `code`, and `iss` with length limits 256/1024/256. KV stores `{code, iss}` under state for 300 seconds. `/pop` reads then deletes: this is best-effort single-use, **not atomic consumption or replay protection**. Preserve this distinction when changing or documenting the protocol.

For self-hosting, changing `BROKER_ORIGIN` alone is insufficient: Rust client metadata/redirect/pop URLs are independent constants in `bsky-oauth/src/lib.rs`. Coordinate those with the Worker's `ORIGIN` in `broker/src/index.ts` and the domain/KV configuration in `broker/wrangler.jsonc`.

## Testing and local conventions

- Existing tests are inline `#[cfg(test)] mod tests` with `#[test]`. Bounded-I/O tests use `std::io::Cursor`; persistence/recovery tests use `tempfile::tempdir()`. Follow nearby tests rather than introducing a new harness.
- Useful regression examples: `bsky-net/src/lib.rs` for bounded reads and partial-file cleanup; `bsky-oauth/src/atomic_json.rs` for corrupt/missing files and session invalidation. Filesystem tests in `bsky-media/src/fs.rs` assume Cargo's crate-root working directory.
- Use host tests for portable logic, but explicitly distinguish them from device validation. Renderer/input/media stubs cannot validate GPU behavior, native dialog lifetimes, video decoding, performance, or Vita linking. Console-facing changes need a Vita build and hardware checks when available.
- Shared dependency versions/features live in root `[workspace.dependencies]`; member manifests opt in via `workspace = true`. The UI uses screen-specific structs and enum state/actions, while native integration is isolated behind wrapper crates and target-conditional implementations.
- Existing comments include historical phase labels and outdated descriptions. Prefer executable code, manifests, and Makefile recipes when they disagree with prose. Avoid unrelated formatting or replacing existing conventions during a focused fix.
