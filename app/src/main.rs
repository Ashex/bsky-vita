//! bsky-vita: PS Vita homebrew Bluesky client.
//!
//! Phase 4.1: navigation stack + tab bar + selection model.
//!
//! - `LoginScreen` (initial) checks for `session.json` on its first
//!   frame; resumes if possible, otherwise shows the login form. Tap
//!   Login → "Authenticating…" → emits `ScreenAction::AuthComplete`,
//!   carrying the `Arc<AuthClient>` we use to spawn the worker thread
//!   and retain for top-level screen construction (`SwitchTab`).
//! - Top-level screens (TimelineScreen, ProfileScreen-of-self,
//!   NotificationsScreen, SearchScreen) render the bottom tab bar and
//!   live in the `screen_stack`. Tab tap → `SwitchTab(target)` →
//!   stack truncates to (or pushes a fresh) instance of that target.
//! - Sub-screens (Compose, Thread, ProfileScreen-of-other) push onto
//!   the stack; CIRCLE pops them.
//!
//! Network calls happen on a background worker thread; the render loop
//! drains responses via `worker.try_recv()` each frame and never
//! blocks. `Screen::after_present` survives for the pre-worker
//! LoginScreen path (resume / login) — those run *before* the worker
//! exists.

use std::sync::Arc;

mod action_dispatch;

#[cfg(feature = "pocketjs-poc")]
mod pocket;

#[cfg(feature = "pocketjs-poc")]
mod pocket_input;

#[cfg(feature = "pocketjs-poc")]
mod pocket_media;

use bsky_auth::AuthClient;
use bsky_input::{buttons, Pad, Touch};
use bsky_render::{EmojiAtlas, IconAtlas, Render, Texture, TextureCache};
use bsky_ui::{
    ConversationListScreen, HintOverlay, LoginScreen, NotificationsScreen, ProfileScreen, Screen,
    ScreenAction, SearchScreen, TimelineScreen, TopLevel, UiCtx,
};
#[cfg(feature = "pocketjs-poc")]
use bsky_worker::WorkRequest;
use bsky_worker::{WorkResponse, Worker};

#[cfg(feature = "pocketjs-poc")]
fn run_pocket_effects(
    effects: Vec<bsky_feed::FeedEffect>,
    worker: Option<&Worker>,
    mut shell: Option<&mut pocket::PocketShell>,
) {
    for effect in effects {
        match effect {
            bsky_feed::FeedEffect::FetchPage(correlation) => {
                if let Some(worker) = worker {
                    worker.send(WorkRequest::FetchFeedProjected {
                        source: bsky_worker::FeedSource::Following,
                        cursor: correlation.cursor.clone(),
                        correlation: bsky_feed::FeedCorrelation {
                            schema: correlation.schema,
                            auth_generation: correlation.auth_generation,
                            mount_generation: correlation.mount_generation,
                            request_id: correlation.request_id,
                            source: correlation.source,
                            cursor: correlation.cursor,
                        },
                        deadline: None,
                    });
                }
            }
            view_effect => {
                if let Some(shell) = shell.as_deref_mut() {
                    shell.accept_effects(vec![view_effect]);
                }
            }
        }
    }
}

fn main() {
    bsky_log::init("ux0:/data/BSKY00001/run.log");
    #[cfg(feature = "pocketjs-poc")]
    bsky_log::log!("pocketjs-poc feature build active (START opens the PocketJS shell)");
    // Grant access to the OS photo gallery (ux0:picture/ albums) — without
    // this the sandbox hides CAMERA/SCREENSHOT/etc. Uses a 'static
    // mount-point string (the mount retains the pointer).
    bsky_media::fs::mount_photo_gallery();
    let mut render = match Render::init() {
        Ok(r) => r,
        Err(e) => {
            bsky_log::log!("vita2d_init failed: {e}; sleeping forever");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
    };
    render.set_clear_color(bsky_render::theme::BACKGROUND);
    let font = match render.load_inter_ttf("app0:Inter-Regular.ttf") {
        Ok(f) => {
            bsky_log::log!("font backend: Inter TTF loaded successfully");
            f
        }
        Err(e) => {
            bsky_log::log!("font backend: Inter TTF load failed ({e}); falling back to PGF");
            eprintln!(
                "Inter TTF load failed ({e}); falling back to PGF — \
                 add app/static/Inter-Regular.ttf and rebuild for crisp text"
            );
            render
                .load_default_pgf()
                .expect("PGF fallback also failed; can't render text")
        }
    };

    let emoji_atlas: Option<EmojiAtlas> = match EmojiAtlas::from_path("app0:NotoColorEmoji.ttf") {
        Ok(a) => {
            bsky_log::log!("emoji font loaded successfully");
            Some(a)
        }
        Err(e) => {
            bsky_log::log!("emoji font load failed ({e}); using stable placeholder fallback");
            eprintln!("Noto Color Emoji load failed ({e}); emoji will use placeholders");
            None
        }
    };

    let avatar_mask: Option<Texture> = match Texture::from_png_file("app0:avatar_mask_96.png") {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("avatar mask load failed ({e}); avatars will render as squares");
            None
        }
    };
    let avatar_mask_field: Option<Texture> =
        match Texture::from_png_file("app0:avatar_mask_field_96.png") {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!(
                    "avatar mask (field-bg) load failed ({e}); selected-row avatars will \
                     render as squares"
                );
                None
            }
        };

    let icon_atlas: Option<IconAtlas> = match IconAtlas::load("app0:pocket_icons.png") {
        Ok(a) => Some(a),
        Err(e) => {
            eprintln!("icon atlas load failed ({e}); control hints will use text labels");
            None
        }
    };

    let mut pad = Pad::init();
    let touch = Touch::init();
    let mut ime = bsky_ime::Ime::new();

    // Navigation stack. Top of the stack is the currently-rendered
    // screen. LoginScreen is the initial root; AuthComplete clears the
    // stack and pushes the post-auth top-level screen.
    let mut screen_stack: Vec<Box<dyn Screen>> = vec![Box::new(LoginScreen::new())];

    // Worker is spawned the first time a screen returns `AuthComplete`.
    let mut worker: Option<Worker> = None;
    // Held alongside the worker so we can construct fresh top-level
    // screens (TimelineScreen, ProfileScreen, …) when a `SwitchTab`
    // action lands on a tab without an existing instance in the stack.
    let mut auth_client: Option<Arc<AuthClient>> = None;
    #[cfg(feature = "pocketjs-poc")]
    let mut auth_generation: u64 = 0;

    let mut texture_cache = TextureCache::new(64);

    // Auto-hiding control-hints bar; shown on entry to each screen and
    // recalled with SELECT.
    let mut hints = HintOverlay::new();
    #[cfg(feature = "pocketjs-poc")]
    let mut pocket_shell: Option<pocket::PocketShell> = None;
    #[cfg(feature = "pocketjs-poc")]
    let mut pocket_status: Option<String> = None;
    #[cfg(feature = "pocketjs-poc")]
    let mut pocket_media = bsky_worker::media_registry::PocketMediaRegistry::new();
    #[cfg(feature = "pocketjs-poc")]
    let media_disk_cache =
        bsky_media::asset_cache::AssetCache::new(format!("{}/media-cache", bsky_auth::DATA_DIR));
    #[cfg(feature = "pocketjs-poc")]
    let pocket_coordinator =
        std::sync::Arc::new(std::sync::Mutex::new(bsky_feed::FeedCoordinator::default()));

    loop {
        if let Some(atlas) = emoji_atlas.as_ref() {
            atlas.service_pending(8);
        }
        let pf = pad.poll();
        let tf = touch.poll();
        let ctx = UiCtx {
            touches: &tf.points,
            pad: &pf,
            worker: worker.as_ref(),
            emoji: emoji_atlas.as_ref(),
            texture_cache: &texture_cache,
            avatar_mask: avatar_mask.as_ref(),
            avatar_mask_field: avatar_mask_field.as_ref(),
        };

        // Render + collect transition action. The Frame's Drop happens
        // when this block ends, which presents the buffer.
        #[cfg(feature = "pocketjs-poc")]
        let mut pocket_started_this_frame = false;
        #[cfg(feature = "pocketjs-poc")]
        if pocket_shell.is_none() && pf.just_pressed(buttons::START) && !ime.is_active() {
            match pocket::PocketShell::new(pocket_coordinator.clone()) {
                Ok(mut shell) => {
                    let auth_effects = shell.bind_auth(auth_generation);
                    shell.open_mount();
                    let effects = shell.request_first();
                    pocket_shell = Some(shell);
                    pocket_status = None;
                    pocket_started_this_frame = true;
                    run_pocket_effects(auth_effects, worker.as_ref(), pocket_shell.as_mut());
                    run_pocket_effects(effects, worker.as_ref(), pocket_shell.as_mut());
                }
                Err(e) => {
                    bsky_log::log!("PocketJS startup failed: {e}");
                    pocket_status = Some(format!("PocketJS startup failed: {e}"));
                }
            }
        }
        // The first frame after opening the shell has to boot the guest,
        // request the first page, and present an initial raster. Give it the
        // startup budget; subsequent frames use the tighter per-frame budget.
        #[cfg(feature = "pocketjs-poc")]
        let pocket_frame_deadline = std::time::Instant::now()
            + if pocket_started_this_frame {
                bsky_pocket::startup_budget()
            } else {
                bsky_pocket::frame_budget()
            };
        #[cfg(feature = "pocketjs-poc")]
        let auth_effects = pocket_shell
            .as_mut()
            .map(|shell| shell.bind_auth(auth_generation))
            .unwrap_or_default();
        #[cfg(feature = "pocketjs-poc")]
        run_pocket_effects(auth_effects, worker.as_ref(), pocket_shell.as_mut());
        #[cfg(feature = "pocketjs-poc")]
        let pocket_active = pocket_shell.is_some();
        #[cfg(feature = "pocketjs-poc")]
        if let Some(shell) = pocket_shell.as_mut() {
            if shell.wants_exit(&pf) {
                pocket_coordinator
                    .lock()
                    .unwrap()
                    .abandon_mount(shell.mount_generation());
                bsky_render::wait_rendering_done();
                pocket_shell = None;
            } else if let Err(e) = shell.prepare_until(&pf, &tf, &font, pocket_frame_deadline) {
                pocket_coordinator
                    .lock()
                    .unwrap()
                    .abandon_mount(shell.mount_generation());
                bsky_log::log!("PocketJS returned to native UI: {e}");
                pocket_status = Some(format!("PocketJS returned to native UI: {e}"));
                bsky_render::wait_rendering_done();
                pocket_shell = None;
            }
        }
        #[cfg(feature = "pocketjs-poc")]
        let pocket_drawing = pocket_active && pocket_shell.is_some();
        #[cfg(feature = "pocketjs-poc")]
        if pocket_drawing {
            if let Some(shell) = pocket_shell.as_mut() {
                match shell.poll_guest_commands_until(pocket_frame_deadline) {
                    Ok(effects) => run_pocket_effects(effects, worker.as_ref(), Some(shell)),
                    Err(e) => bsky_log::log!("PocketJS command poll failed: {e}"),
                }
            }
        }

        let action = {
            let top = screen_stack
                .last_mut()
                .expect("screen stack is never empty");
            let mut frame = render.begin_frame();
            #[cfg(feature = "pocketjs-poc")]
            let action = if pocket_drawing {
                pocket_shell.as_ref().expect("active PocketJS shell").draw(
                    &mut frame,
                    &font,
                    &texture_cache,
                );
                ScreenAction::None
            } else {
                top.frame(&mut frame, &font, &ctx, &mut ime)
            };
            #[cfg(not(feature = "pocketjs-poc"))]
            let action = top.frame(&mut frame, &font, &ctx, &mut ime);
            // Surface a PocketJS startup failure on screen; otherwise it is
            // only visible in run.log and START looks like a dead button.
            #[cfg(feature = "pocketjs-poc")]
            if !pocket_drawing {
                if let Some(status) = pocket_status.as_deref() {
                    frame.fill_rect(0.0, 0.0, 960.0, 40.0, bsky_render::theme::FIELD_BG);
                    frame.draw_text(
                        &font,
                        12,
                        28,
                        bsky_render::theme::TEXT_PRIMARY,
                        0.9,
                        status,
                    );
                }
            }
            // Control-hints bar, composited on top of the screen. Drawn
            // before pump_ime (which ends frame drawing) and skipped while
            // the IME is up (the keyboard covers the screen anyway).
            hints.tick();
            if pf.just_pressed(buttons::SELECT) {
                hints.toggle();
            }
            if !ime.is_active() && !cfg!(feature = "pocketjs-poc") {
                let hint_list = top.control_hints();
                hints.draw_with_icons(
                    &mut frame,
                    &font,
                    &hint_list,
                    top.top_level().is_some(),
                    icon_atlas.as_ref(),
                );
            }
            if ime.is_active() {
                frame.pump_ime();
            }
            action
        };

        // Pre-worker blocking work for LoginScreen.
        if let Some(top) = screen_stack.last_mut() {
            top.after_present();
        }

        // Drain any worker responses produced since the last frame and
        // hand them to the TOP screen. For `Image` responses, decode
        // and insert into the cache before forwarding so screens can
        // clear inflight tracking after the cache is already populated.
        // Set when a worker response shows the session's tokens are dead
        // (expired/revoked) → fall back to login after the drain.
        let mut session_dead = false;
        #[cfg(feature = "pocketjs-poc")]
        let frame_auth_generation = auth_generation;
        if let Some(w) = worker.as_ref() {
            while let Some(resp) = w.try_recv() {
                let resp = if let WorkResponse::Image { url, bytes: Ok(b) } = &resp {
                    if !url.starts_with("http") {
                        // Local file read (picker thumbnail / compose
                        // preview): forward the raw bytes untouched. The
                        // screen downscales via Texture::decode_scaled so
                        // a multi-megapixel decode never lands in the
                        // shared avatar/embed cache.
                        resp
                    } else {
                        // Avatars are alpha-masked into circles, so they must
                        // be 4 bpp RGBA. vita2d decodes JPEG to 3 bpp RGB
                        // (no alpha channel); under the mask that corrupts the
                        // pixels (the vertical black/red stripe bug). insert_rgba
                        // forces a 4 bpp copy; other images stay 3 bpp via insert.
                        let is_avatar = url.contains("/avatar_thumbnail/");
                        let inserted = if is_avatar {
                            texture_cache.insert_rgba(url.clone(), b)
                        } else {
                            texture_cache.insert(url.clone(), b)
                        };
                        match inserted {
                            Ok(()) => {
                                // Circular alpha mask so avatars composite
                                // cleanly over arbitrary backgrounds (banners).
                                if is_avatar {
                                    if let Some(tex) = texture_cache.get(url) {
                                        tex.apply_circular_mask();
                                    }
                                }
                                resp
                            }
                            Err(e) => {
                                bsky_log::log!("decode failed for {url}: {e}");
                                WorkResponse::Image {
                                    url: url.clone(),
                                    bytes: Err(format!("decode: {e}")),
                                }
                            }
                        }
                    }
                } else {
                    resp
                };
                let dead = resp.auth_failed();
                #[cfg(feature = "pocketjs-poc")]
                let dead = match &resp {
                    WorkResponse::FeedPageCorrelated { correlation, .. }
                    | WorkResponse::FeedPageProjected { correlation, .. } => {
                        dead && correlation.auth_generation == frame_auth_generation
                    }
                    _ => dead,
                };
                #[cfg(feature = "pocketjs-poc")]
                if let WorkResponse::FeedPageProjected { correlation, batch } = &resp {
                    let converted = bsky_feed::FeedCorrelation {
                        schema: correlation.schema,
                        auth_generation: correlation.auth_generation,
                        mount_generation: correlation.mount_generation,
                        request_id: correlation.request_id,
                        source: correlation.source.clone(),
                        cursor: correlation.cursor.clone(),
                    };
                    let result = batch
                        .as_ref()
                        .map(|projected| projected.delivery.clone())
                        .map_err(Clone::clone);
                    let effects = pocket_coordinator.lock().unwrap().handle(
                        bsky_feed::FeedEvent::Completed {
                            correlation: converted,
                            result,
                        },
                    );
                    if correlation.auth_generation == auth_generation {
                        if let (Ok(projected), Some(client)) = (batch, auth_client.as_ref()) {
                            pocket_media.install_accepted(
                                bsky_feed::AccountScope {
                                    account_did: client.resolved.did.to_string(),
                                    auth_generation,
                                },
                                projected,
                                &effects,
                            );
                        }
                        run_pocket_effects(effects, worker.as_ref(), pocket_shell.as_mut());
                    }
                }
                if let Some(top) = screen_stack.last_mut() {
                    top.handle_worker_response(resp);
                }
                if dead {
                    bsky_log::log!("main: session auth failed — returning to login");
                    session_dead = true;
                    break;
                }
            }
        }

        #[cfg(feature = "pocketjs-poc")]
        if let Some(shell) = pocket_shell.as_mut() {
            shell.bind_avatar_slots(&pocket_media);
            let visible: Vec<String> = shell.visible_uris().to_vec();
            pocket_media::service(
                &pocket_media,
                &mut texture_cache,
                &media_disk_cache,
                shell.image_handles_mut(),
                &visible,
            );
        }

        // An auth failure from this frame's session must win over an
        // AuthComplete action produced by the same presentation.
        #[cfg(feature = "pocketjs-poc")]
        if session_dead {
            pocket_media.invalidate();
            texture_cache.clear();
            auth_generation = auth_generation.wrapping_add(1).max(1);
            let effects = pocket_coordinator
                .lock()
                .unwrap()
                .auth_changed(auth_generation);
            run_pocket_effects(effects, worker.as_ref(), pocket_shell.as_mut());
            pocket_shell = None;
            teardown_to_login(&mut worker, &mut auth_client, &mut screen_stack);
        }

        // A failed session invalidates any screen action produced before the
        // worker response was drained. In particular, do not let a stale
        // AuthComplete reinstall the session after teardown. Exit/navigation
        // actions are suppressed too; the existing loop exit behavior is
        // unchanged because ScreenAction has no exit variant.
        if action_dispatch::allowed(
            session_dead,
            if matches!(action, ScreenAction::AuthComplete { .. }) {
                action_dispatch::ActionKind::AuthComplete
            } else {
                action_dispatch::ActionKind::Other
            },
        ) {
            // Any screen change re-shows the hint bar on the new page.
            if !matches!(action, ScreenAction::None) {
                hints.show();
            }
            match action {
                ScreenAction::None => {}
                ScreenAction::Push(next) => {
                    screen_stack.push(next);
                }
                ScreenAction::Pop => {
                    if screen_stack.len() > 1 {
                        bsky_render::wait_rendering_done();
                        screen_stack.pop();
                    }
                }
                ScreenAction::SwitchTab(target) => {
                    handle_switch_tab(target, &mut screen_stack, auth_client.as_ref());
                }
                ScreenAction::AuthComplete { client, next } => {
                    #[cfg(feature = "pocketjs-poc")]
                    {
                        pocket_media.invalidate();
                        texture_cache.clear();
                        auth_generation = auth_generation.wrapping_add(1).max(1);
                        let effects = pocket_coordinator
                            .lock()
                            .unwrap()
                            .auth_changed(auth_generation);
                        run_pocket_effects(effects, worker.as_ref(), pocket_shell.as_mut());
                        pocket_shell = None;
                    }
                    worker = Some(Worker::spawn(Arc::clone(&client)));
                    auth_client = Some(client);
                    screen_stack.clear();
                    screen_stack.push(next);
                }
                ScreenAction::Logout => {
                    #[cfg(feature = "pocketjs-poc")]
                    {
                        pocket_media.invalidate();
                        texture_cache.clear();
                        auth_generation = auth_generation.wrapping_add(1).max(1);
                        let effects = pocket_coordinator
                            .lock()
                            .unwrap()
                            .auth_changed(auth_generation);
                        run_pocket_effects(effects, worker.as_ref(), pocket_shell.as_mut());
                        pocket_shell = None;
                    }
                    teardown_to_login(&mut worker, &mut auth_client, &mut screen_stack);
                }
            }
        }

        // A resumed (or in-session) token that failed auth — drop the dead
        // session and return to login. This is the recovery path for an
        // expired/unrefreshed session that would otherwise strand the user
        // on a broken screen. Same teardown as an explicit logout.
        #[cfg(not(feature = "pocketjs-poc"))]
        if session_dead {
            teardown_to_login(&mut worker, &mut auth_client, &mut screen_stack);
        }
    }
}

/// Reset to a fresh login. Bumps the session generation and clears both
/// auth-path session files (+ `.tmp` sidecars) via
/// [`bsky_oauth::atomic_json::invalidate_and_delete_sessions`] BEFORE dropping
/// the worker — this, not the drop order, is what stops a worker mid-refresh
/// from re-persisting (resurrecting) the session: the generation bump makes its
/// Arc-shared store stale so the pending write is skipped, and the gate
/// serializes any in-flight write behind the delete. Then drops the worker
/// (closing its channel → the thread exits after its current request) and our
/// own client handle. `LoginScreen::idle()` shows the form without
/// auto-resuming (avoids a resume→fail→reset loop). Shared by explicit logout
/// and the auth-failure fallback.
fn teardown_to_login(
    worker: &mut Option<Worker>,
    auth_client: &mut Option<Arc<AuthClient>>,
    screen_stack: &mut Vec<Box<dyn Screen>>,
) {
    // Bump the session generation and delete both session files (+ `.tmp`
    // sidecars) atomically w.r.t. any in-flight write, BEFORE dropping the
    // worker. This is what actually prevents a worker mid-refresh from
    // re-persisting tokens after deletion: the bump makes its Arc-shared store
    // stale (its pending write is skipped), and a write already in progress is
    // serialized behind this call, then removed.
    bsky_oauth::atomic_json::invalidate_and_delete_sessions(&[
        std::path::Path::new(bsky_auth::SESSION_PATH),
        std::path::Path::new(bsky_oauth::OAUTH_SESSION_PATH),
    ]);
    *worker = None;
    *auth_client = None;
    screen_stack.clear();
    screen_stack.push(Box::new(LoginScreen::idle()));
}

/// Tab-bar tap handler: walk the stack from the bottom up looking for
/// a screen whose `top_level()` matches `target`. If found, truncate
/// the stack to (and including) that screen — preserves its in-memory
/// state. If not found, construct a fresh top-level instance and push
/// it as the new root (replacing the existing stack since top-levels
/// are mutually exclusive at the root level).
fn handle_switch_tab(
    target: TopLevel,
    stack: &mut Vec<Box<dyn Screen>>,
    auth: Option<&Arc<AuthClient>>,
) {
    if let Some(idx) = stack.iter().position(|s| s.top_level() == Some(target)) {
        stack.truncate(idx + 1);
        return;
    }
    let Some(client) = auth else {
        eprintln!("SwitchTab({target:?}) before auth — ignoring");
        return;
    };
    let next = make_top_level(target, Arc::clone(client));
    stack.clear();
    stack.push(next);
}

/// Construct a fresh instance of a top-level screen for `target`.
fn make_top_level(target: TopLevel, client: Arc<AuthClient>) -> Box<dyn Screen> {
    match target {
        TopLevel::Home => Box::new(TimelineScreen::new(client)),
        TopLevel::Profile => Box::new(ProfileScreen::new(client, None)),
        TopLevel::Notifications => Box::new(NotificationsScreen::new(client)),
        TopLevel::Search => Box::new(SearchScreen::new(client)),
        TopLevel::Messages => Box::new(ConversationListScreen::new(client)),
    }
}
