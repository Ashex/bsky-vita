use bsky_input::{buttons, PadFrame, TouchFrame};
use bsky_pocket::text_batch::pack_lines;
use bsky_pocket::{EmbeddedGuest, NativeCommand};
use bsky_render::{Font, Frame, ImageHandleMap, Texture, TextureCache};

pub struct PocketShell {
    coordinator: std::sync::Arc<std::sync::Mutex<bsky_feed::FeedCoordinator>>,
    guest: EmbeddedGuest,
    image_texture: Option<Texture>,
    image_handles: ImageHandleMap,
    framebuffer_texture: Option<Texture>,
    native_batch: bsky_pocket::NativeBatch,
    authorized_stamp: Option<bsky_pocket::visible_text::Stamp>,
    texture_generation: u32,
    auth_generation: u64,
    mount_generation: u64,
    layout_id: u64,
    wrap_cache: bsky_pocket::visible_text::WrapCache,
    measurement_queue: std::collections::VecDeque<String>,
    measured_lines: std::collections::HashMap<String, (std::sync::Arc<Vec<String>>, bool)>,
    measured_widths: std::collections::HashMap<(String, u64), u16>,
    pending_delivery: Option<bsky_feed::FeedDelivery>,
    pending_status: Option<String>,
    visible_uris: Vec<String>,
    avatar_slots: Vec<(u32, String)>,
    avatar_positions: std::collections::HashMap<String, (f32, f32)>,
}

impl PocketShell {
    pub fn new(
        coordinator: std::sync::Arc<std::sync::Mutex<bsky_feed::FeedCoordinator>>,
    ) -> Result<Self, String> {
        Ok(Self {
            coordinator,
            guest: EmbeddedGuest::new(480, 272)?,
            image_texture: None,
            image_handles: ImageHandleMap::new(bsky_pocket::MAX_NATIVE_IMAGE_HANDLES),
            framebuffer_texture: None,
            native_batch: bsky_pocket::NativeBatch {
                commands: Vec::new(),
                image: None,
            },
            authorized_stamp: None,
            texture_generation: 0,
            auth_generation: 0,
            mount_generation: 0,
            layout_id: 0,
            wrap_cache: bsky_pocket::visible_text::WrapCache::default(),
            measurement_queue: std::collections::VecDeque::new(),
            measured_lines: std::collections::HashMap::new(),
            measured_widths: std::collections::HashMap::new(),
            pending_delivery: None,
            pending_status: None,
            visible_uris: Vec::new(),
            avatar_slots: Vec::new(),
            avatar_positions: std::collections::HashMap::new(),
        })
    }

    /// The URIs of the rows currently laid out in the viewport, in order.
    pub fn visible_uris(&self) -> &[String] {
        &self.visible_uris
    }
    pub fn bind_avatar_slots(
        &mut self,
        registry: &bsky_worker::media_registry::PocketMediaRegistry,
    ) {
        self.avatar_slots = registry
            .visible_handles(&self.visible_uris, bsky_pocket::MAX_NATIVE_IMAGE_HANDLES)
            .iter()
            .enumerate()
            .filter_map(|(slot, handle)| {
                let descriptor = registry.descriptor(handle)?;
                matches!(descriptor.identity, bsky_feed::AssetIdentity::Avatar { .. }).then(|| {
                    (
                        slot as u32 + 1,
                        registry.post_for_handle(handle).unwrap().to_owned(),
                    )
                })
            })
            .collect();
    }
    pub fn image_handles_mut(&mut self) -> &mut ImageHandleMap {
        &mut self.image_handles
    }
    pub fn mount_generation(&self) -> u64 {
        self.mount_generation
    }
    pub fn open_mount(&mut self) {
        self.mount_generation = self.coordinator.lock().unwrap().open_mount();
        self.reset_local();
    }
    fn reset_local(&mut self) {
        self.authorized_stamp = None;
        self.avatar_positions.clear();
        self.avatar_slots.clear();
        self.wrap_cache.clear();
        self.measurement_queue.clear();
        self.measured_lines.clear();
        self.measured_widths.clear();
        self.native_batch
            .commands
            .retain(|c| !matches!(c, NativeCommand::Text { stamp: Some(_), .. }));
    }
    pub fn bind_auth(&mut self, generation: u64) -> Vec<bsky_feed::FeedEffect> {
        if self.auth_generation != generation {
            self.auth_generation = generation;
            self.wrap_cache.clear();
            self.measurement_queue.clear();
            self.measured_lines.clear();
            self.measured_widths.clear();
            let effects = self.coordinator.lock().unwrap().auth_changed(generation);
            self.accept_effects(effects.clone());
            effects
        } else {
            Vec::new()
        }
    }
    pub fn request_first(&mut self) -> Vec<bsky_feed::FeedEffect> {
        self.coordinator
            .lock()
            .unwrap()
            .command(bsky_feed::FeedCommand::FirstPage)
    }
    fn publish_effects(&mut self, effects: Vec<bsky_feed::FeedEffect>) {
        for effect in effects {
            match effect {
                bsky_feed::FeedEffect::FetchPage(_) => {}
                bsky_feed::FeedEffect::PublishDelivery(delivery) => {
                    self.pending_delivery = Some(delivery)
                }
                bsky_feed::FeedEffect::PublishStatus(message) => {
                    self.pending_status = Some(message)
                }
                bsky_feed::FeedEffect::InvalidatePresentation => {
                    self.authorized_stamp = None;
                    self.wrap_cache.clear();
                    self.measurement_queue.clear();
                    self.measured_lines.clear();
                    self.measured_widths.clear();
                    self.native_batch.commands.clear();
                }
            }
        }
    }
    pub fn accept_effects(&mut self, effects: Vec<bsky_feed::FeedEffect>) {
        self.publish_effects(effects);
    }
    pub fn poll_guest_commands(&mut self) -> Result<Vec<bsky_feed::FeedEffect>, String> {
        self.poll_guest_commands_until(std::time::Instant::now() + bsky_pocket::frame_budget())
    }
    pub fn poll_guest_commands_until(
        &mut self,
        deadline: std::time::Instant,
    ) -> Result<Vec<bsky_feed::FeedEffect>, String> {
        let mut effects_out = Vec::new();
        for raw in self.guest.take_feed_commands_with_deadline(deadline)? {
            let command = bsky_feed::FeedCommand::parse(&raw).map_err(str::to_string)?;
            let effects = self.coordinator.lock().unwrap().command(command);
            self.accept_effects(effects.clone());
            effects_out.extend(effects);
        }
        Ok(effects_out)
    }
    pub fn prepare(&mut self, pad: &PadFrame, font: &Font) -> Result<(), String> {
        self.prepare_until(
            pad,
            &TouchFrame::default(),
            font,
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
    }
    pub fn prepare_until(
        &mut self,
        pad: &PadFrame,
        touch: &TouchFrame,
        font: &Font,
        deadline: std::time::Instant,
    ) -> Result<(), String> {
        let mut delivered_feed = false;
        if let Some(delivery) = self.pending_delivery.take() {
            let json = serde_json::to_string(&delivery)
                .map_err(|_| "feed delivery serialization failed")?;
            {
                let coordinator = self.coordinator.lock().unwrap();
                self.guest.set_global_json(
                    "__bskyAuthGeneration",
                    &self.auth_generation.max(1).to_string(),
                    deadline,
                )?;
                self.guest.set_global_json(
                    "__bskyMountGeneration",
                    &self.mount_generation.max(1).to_string(),
                    deadline,
                )?;
                self.guest.set_global_json(
                    "__bskyHistoryRevision",
                    &coordinator.history_revision().max(1).to_string(),
                    deadline,
                )?;
            }
            self.guest
                .deliver_feed_json_with_deadline(&json, deadline)?;
            delivered_feed = true;
        }
        let mut delivered_status = false;
        if let Some(status) = self.pending_status.take() {
            self.guest.deliver_status_with_deadline(&status, deadline)?;
            delivered_status = true;
        }
        if let Some(error) = self.coordinator.lock().unwrap().error().map(str::to_owned) {
            self.guest.deliver_status_with_deadline(&error, deadline)?;
            delivered_status = true;
        }
        if delivered_status {
            self.authorized_stamp = None;
            self.framebuffer_texture = None;
            self.native_batch
                .commands
                .retain(|command| !matches!(command, NativeCommand::Text { stamp: Some(_), .. }));
        }
        if pad.current & (buttons::UP | buttons::DOWN) != 0 {
            self.authorized_stamp = None;
            self.native_batch
                .commands
                .retain(|command| !matches!(command, NativeCommand::Text { stamp: Some(_), .. }));
        }
        if pad.just_pressed(buttons::CIRCLE) {
            let effects = self
                .coordinator
                .lock()
                .unwrap()
                .command(bsky_feed::FeedCommand::Leave);
            self.accept_effects(effects);
        }
        let touch_snapshot =
            bsky_pocket::TouchSnapshot::from_contacts(touch.points.iter().map(|point| {
                bsky_pocket::TouchContact {
                    id: point.id,
                    x: (point.x.clamp(0, 960) / 2) as u16,
                    y: (point.y.clamp(0, 544) / 2) as u16,
                }
            }));
        self.guest.set_touch_snapshot(touch_snapshot, deadline)?;
        self.guest
            .eval_global_with_deadline("globalThis.__bskyTouchFrame()", deadline)
            .map_err(|e| format!("touch frame: {e}"))?;
        self.guest
            .advance_with_deadline(crate::pocket_input::guest_buttons(pad.current), deadline)
            .map_err(|e| format!("advance: {e}"))?;
        self.native_batch = self
            .guest
            .native_batch_with_budget(deadline.saturating_duration_since(std::time::Instant::now()))
            .map_err(|e| format!("native batch: {e}"))?;

        if delivered_feed {
            self.authorized_stamp = None;
            self.framebuffer_texture = None;
            self.native_batch
                .commands
                .retain(|command| !matches!(command, NativeCommand::Text { stamp: Some(_), .. }));
        }
        self.run_visible_text(font, deadline)
            .map_err(|e| format!("visible text: {e}"))?;
        self.native_batch.validate(480, 272)?;
        self.authorized_stamp =
            self.native_batch
                .commands
                .iter()
                .find_map(|command| match command {
                    NativeCommand::Text {
                        stamp: Some(stamp), ..
                    } if stamp.auth_generation == self.auth_generation
                        && stamp.mount_generation == self.mount_generation
                        && stamp.history_revision
                            == self.coordinator.lock().unwrap().history_revision()
                        && stamp.layout_id == self.layout_id =>
                    {
                        Some(stamp.clone())
                    }
                    _ => None,
                });
        if let Some(image) = self.native_batch.image.as_ref() {
            if self.image_texture.is_none() || self.texture_generation != image.generation {
                if let Some(old) = self.image_texture.take() {
                    bsky_render::wait_rendering_done();
                    drop(old);
                }
                let rgba = synthetic_rgba(image.generation);
                self.image_texture = Some(
                    Texture::from_rgba_pixels(image.width, image.height, &rgba)
                        .map_err(|_| "native texture upload failed".to_string())?,
                );
                self.texture_generation = image.generation;
            }
        }
        if std::time::Instant::now() >= deadline {
            self.authorized_stamp = None;
            self.framebuffer_texture = None;
            self.native_batch.commands.clear();
            return Err("pocket frame deadline exceeded before presentation".into());
        }
        self.guest
            .present_with_deadline(deadline)
            .map_err(|e| format!("present: {e}"))?;
        let frame_pixels = self.guest.pixels()?;
        if let Some(old) = self.framebuffer_texture.take() {
            bsky_render::wait_rendering_done();
            drop(old);
        }
        self.framebuffer_texture = Some(
            Texture::from_rgba_pixels(960, 544, &frame_pixels)
                .map_err(|_| "native framebuffer upload failed".to_string())?,
        );
        Ok(())
    }
    fn run_visible_text(
        &mut self,
        font: &Font,
        deadline: std::time::Instant,
    ) -> Result<(), String> {
        use bsky_pocket::visible_text::{
            MeasureReply, MeasureRequest, MeasureResult, MeasureStatus, PlacementMessage, Stamp,
        };
        if std::time::Instant::now() >= deadline {
            self.authorized_stamp = None;
            self.framebuffer_texture = None;
            self.native_batch
                .commands
                .retain(|c| !matches!(c, NativeCommand::Text { stamp: Some(_), .. }));
            return Ok(());
        }
        let (revision, rows) = {
            let coordinator = self.coordinator.lock().unwrap();
            let history = coordinator.history().posts();
            let _ = self.guest.set_global_json(
                "__bskyAuthGeneration",
                &self.auth_generation.max(1).to_string(),
                deadline,
            );
            let _ = self.guest.set_global_json(
                "__bskyMountGeneration",
                &self.mount_generation.max(1).to_string(),
                deadline,
            );
            let _ = self.guest.set_global_json(
                "__bskyHistoryRevision",
                &coordinator.history_revision().max(1).to_string(),
                deadline,
            );
            let raw = self
                .guest
                .eval_global_with_deadline("globalThis.__bskyVisibleWindow || ''", deadline)
                .unwrap_or_default();
            let proposed: Option<MeasureRequest> = serde_json::from_str(&raw).ok();
            let requested = proposed
                .filter(|request| request.validate().is_ok())
                .filter(|request| !request.rows.is_empty())
                .filter(|request| {
                    request.stamp.auth_generation == self.auth_generation.max(1)
                        && request.stamp.mount_generation == self.mount_generation.max(1)
                        && request.stamp.history_revision == coordinator.history_revision().max(1)
                })
                .and_then(|request| {
                    let uris: Vec<String> = request.rows.into_iter().map(|row| row.uri).collect();
                    let start = history.iter().position(|post| post.uri == uris[0])?;
                    let window: Vec<_> = history.iter().skip(start).take(uris.len()).collect();
                    (window.len() == uris.len()
                        && window.iter().zip(&uris).all(|(post, uri)| post.uri == *uri))
                    .then(|| uris)
                });
            // Refresh a bounded, ordered work queue from the current viewport.
            // Rotating its head after each frame prevents long posts from starving
            // later rows while preserving the three-row admission cap.
            let requested = requested.unwrap_or_else(|| {
                let layout_raw = self
                    .guest
                    .eval_global_with_deadline(
                        "globalThis.__bskyVisibleLayoutRequest || ''",
                        deadline,
                    )
                    .unwrap_or_default();
                let layout: Option<MeasureRequest> = serde_json::from_str(&layout_raw).ok();
                layout
                    .filter(|layout| {
                        layout.validate().is_ok()
                            && layout.stamp.auth_generation == self.auth_generation.max(1)
                            && layout.stamp.mount_generation == self.mount_generation.max(1)
                            && layout.stamp.history_revision
                                == coordinator.history_revision().max(1)
                    })
                    .filter(|layout| {
                        layout
                            .rows
                            .iter()
                            .all(|row| history.iter().any(|post| post.uri == row.uri))
                    })
                    .map(|layout| layout.rows.into_iter().map(|row| row.uri).collect())
                    .unwrap_or_default()
            });
            self.visible_uris = requested.clone();
            self.measurement_queue.retain(|uri| requested.contains(uri));
            for uri in requested {
                if !self.measurement_queue.contains(&uri) {
                    self.measurement_queue.push_back(uri);
                }
            }
            self.measurement_queue
                .truncate(bsky_pocket::visible_text::MAX_LAYOUT_ROWS);
            let revision = coordinator.history_revision().max(1);
            let layout_raw = self
                .guest
                .eval_global_with_deadline("globalThis.__bskyVisibleLayoutRequest || ''", deadline)
                .unwrap_or_default();
            let layout_request: Option<MeasureRequest> = serde_json::from_str(&layout_raw).ok();
            if let Some(layout) = layout_request.filter(|layout| {
                layout.validate().is_ok()
                    && layout.stamp.auth_generation == self.auth_generation.max(1)
                    && layout.stamp.mount_generation == self.mount_generation.max(1)
                    && layout.stamp.history_revision == revision
            }) {
                if layout
                    .rows
                    .iter()
                    .all(|row| history.iter().any(|post| post.uri == row.uri))
                {
                    self.guest
                        .set_global_json(
                            "__bskyVisibleLayoutRequest",
                            &serde_json::to_string(&layout)
                                .map_err(|_| "layout serialization failed")?,
                            deadline,
                        )
                        .ok();
                }
            }
            let rows: Vec<String> = self
                .measurement_queue
                .iter()
                .take(bsky_pocket::visible_text::MAX_MEASURE_BATCH)
                .cloned()
                .collect();
            let rows: Vec<bsky_pocket::visible_text::MeasureRow> = rows
                .iter()
                .map(|uri| bsky_pocket::visible_text::MeasureRow {
                    uri: uri.clone(),
                    width: 440,
                })
                .collect();
            (coordinator.history_revision().max(1), rows)
        };
        let needs_measurement = !rows.is_empty()
            && rows
                .iter()
                .any(|row| !self.measured_lines.contains_key(&row.uri));
        let request = if needs_measurement {
            self.layout_id = self.layout_id.wrapping_add(1).max(1);
            let request = MeasureRequest {
                schema: bsky_pocket::visible_text::VISIBLE_SCHEMA,
                stamp: Stamp {
                    auth_generation: self.auth_generation.max(1),
                    mount_generation: self.mount_generation.max(1),
                    history_revision: revision,
                    layout_id: self.layout_id,
                },
                rows,
            };
            request.validate().map_err(str::to_string)?;
            let json = serde_json::to_string(&request)
                .map_err(|_| "visible text request serialization failed")?;
            self.guest
                .set_global_json("__bskyVisibleRequest", &json, deadline)?;
            let raw = self.guest.eval_global_with_deadline(
                "globalThis.__bskyVisibleCandidates(JSON.stringify(globalThis.__bskyVisibleRequest))",
                deadline,
            )?;
            if raw.is_empty() {
                return Ok(());
            }
            let candidates: serde_json::Value =
                serde_json::from_str(&raw).map_err(|_| "invalid visible candidates")?;
            if candidates.get("stamp") != Some(&serde_json::to_value(&request.stamp).unwrap()) {
                return Ok(());
            }
            let values = candidates
                .get("rows")
                .and_then(serde_json::Value::as_array)
                .ok_or("invalid visible rows")?;
            if values.len() != request.rows.len() {
                return Ok(());
            }
            Some((request, values.clone()))
        } else {
            None
        };
        let (request, values) = request.unwrap_or_else(|| {
            (
                MeasureRequest {
                    schema: bsky_pocket::visible_text::VISIBLE_SCHEMA,
                    stamp: Stamp {
                        auth_generation: self.auth_generation.max(1),
                        mount_generation: self.mount_generation.max(1),
                        history_revision: revision,
                        layout_id: self.layout_id.max(1),
                    },
                    rows: Vec::new(),
                },
                Vec::new(),
            )
        });
        let mut results = Vec::new();
        let mut lines_by_row = Vec::new();
        let mut budget = bsky_pocket::visible_text::MeasureBudget::new(
            bsky_pocket::visible_text::MAX_MEASURE_CALLS_TOTAL,
            bsky_pocket::visible_text::MAX_MEASURE_BYTES_TOTAL,
        );
        let mut provider_budget = bsky_pocket::visible_text::MeasureBudget::new(
            bsky_pocket::visible_text::MAX_MEASURE_CALLS_TOTAL,
            bsky_pocket::visible_text::MAX_MEASURE_BYTES_TOTAL,
        );
        let font_key = font as *const Font as usize as u64;
        if !provider_budget.charge(2) {
            return Ok(());
        }
        let (_, native_height) = bsky_render::measure_native_text(font, 1.5, "Ag");
        let metric_line_height = (((native_height.max(0).saturating_add(1) / 2).saturating_add(1))
            & !1)
            .clamp(1, u16::MAX as i32) as u16;
        let mut pending_uri = None;
        for (candidate, expected) in values.iter().zip(&request.rows) {
            if std::time::Instant::now() >= deadline {
                return Ok(());
            }
            let uri = candidate
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .ok_or("invalid visible URI")?;
            if uri != expected.uri {
                return Ok(());
            }
            let text = candidate
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if text.len() > bsky_pocket::visible_text::MAX_INPUT_BYTES_ROW {
                return Ok(());
            }
            let status = match candidate.get("status").and_then(serde_json::Value::as_str) {
                Some("ready") => MeasureStatus::Ready,
                Some("placeholder") => MeasureStatus::Placeholder,
                _ => return Ok(()),
            };
            let (lines, continuation) = if status == MeasureStatus::Placeholder {
                (
                    std::sync::Arc::new(vec!["Content unavailable".to_string()]),
                    false,
                )
            } else {
                let wrapped = self.wrap_cache.step(
                    uri,
                    text,
                    expected.width,
                    font_key,
                    &mut budget,
                    |line| {
                        if std::time::Instant::now() >= deadline
                            || !provider_budget.charge(line.len())
                        {
                            return None;
                        }
                        Some(
                            ((bsky_render::measure_native_text(font, 1.5, line)
                                .0
                                .max(0)
                                .saturating_add(1)
                                / 2)
                            .min(u16::MAX as i32)) as u16,
                        )
                    },
                );
                let Some((lines, continuation)) = wrapped else {
                    pending_uri = Some(uri.to_owned());
                    break;
                };
                (lines, continuation)
            };
            let count = u32::try_from(lines.len()).map_err(|_| "line count overflow")?;
            let line_height = metric_line_height;
            let baseline_offset = line_height;
            results.push(MeasureResult {
                uri: uri.into(),
                status,
                width: expected.width,
                line_count: count,
                line_height,
                baseline_offset,
                body_height: count
                    .checked_mul(u32::from(line_height))
                    .ok_or("body height overflow")?
                    .max(u32::from(line_height)),

                continuation,
            });
            self.measured_lines
                .insert(uri.to_owned(), (lines.clone(), continuation));
            lines_by_row.push(lines);
            self.measurement_queue.retain(|queued| queued != uri);
        }
        if let Some(uri) = pending_uri {
            if let Some(index) = self
                .measurement_queue
                .iter()
                .position(|queued| queued == &uri)
            {
                self.measurement_queue.rotate_left(index + 1);
            }
        }
        let reply = MeasureReply {
            schema: bsky_pocket::visible_text::VISIBLE_SCHEMA,
            stamp: request.stamp.clone(),
            rows: results,
        };
        if !reply.rows.is_empty() {
            reply.validate_for(&request).map_err(str::to_string)?;
            let encoded = reply.to_bounded_json().map_err(str::to_string)?;
            self.guest
                .set_global_json("__bskyVisibleMeasurement", &encoded, deadline)?;
            self.guest.eval_global_with_deadline(
                "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
                deadline,
            )?;
        }
        if std::time::Instant::now() >= deadline {
            self.authorized_stamp = None;
            self.framebuffer_texture = None;
            self.native_batch
                .commands
                .retain(|c| !matches!(c, NativeCommand::Text { stamp: Some(_), .. }));
            return Ok(());
        }
        let placed = self
            .guest
            .eval_global_with_deadline("globalThis.__bskyVisiblePlacement || ''", deadline)?;
        if placed.is_empty() {
            return Ok(());
        }
        let placement: PlacementMessage =
            serde_json::from_str(&placed).map_err(|_| "invalid visible placement")?;
        let current = self
            .guest
            .eval_global_with_deadline("globalThis.__bskyVisibleLayoutRequest || ''", deadline)?;
        let viewport: MeasureRequest = serde_json::from_str(&current)
            .map_err(|_| "invalid current viewport placement contract")?;
        viewport.validate().map_err(str::to_string)?;
        if viewport.stamp != request.stamp
            || viewport.rows.len() > bsky_pocket::visible_text::MAX_LAYOUT_ROWS
        {
            return Ok(());
        }
        let known = self
            .guest
            .eval_global_with_deadline("globalThis.__bskyVisibleMetrics || '[]'", deadline)?;
        let known: Vec<MeasureResult> =
            serde_json::from_str(&known).map_err(|_| "invalid cached placement metrics")?;
        let measured_viewport = bsky_pocket::visible_text::MeasureRequest {
            schema: viewport.schema,
            stamp: viewport.stamp.clone(),
            rows: viewport
                .rows
                .iter()
                .filter(|row| known.iter().any(|metric| metric.uri == row.uri))
                .cloned()
                .collect(),
        };
        if measured_viewport.rows.is_empty() {
            return Ok(());
        }
        placement
            .validate_for(&measured_viewport, &known)
            .map_err(str::to_string)?;
        self.avatar_positions.clear();
        for position in &placement.rows {
            let context = self
                .coordinator
                .lock()
                .unwrap()
                .history()
                .posts()
                .iter()
                .find(|post| post.uri == position.uri)
                .and_then(|post| post.context);
            // Body starts after the 22px header, 4px gap and optional context.
            let y = position.y as f32 - 26.0 - if context.is_some() { 14.0 } else { 0.0 };
            self.avatar_positions
                .insert(position.uri.clone(), (position.x as f32, y));
        }
        let mut commands = Vec::new();
        for position in &placement.rows {
            let Some(metrics) = known.iter().find(|metric| metric.uri == position.uri) else {
                continue;
            };
            let lines = request
                .rows
                .iter()
                .position(|row| row.uri == position.uri)
                .and_then(|index| lines_by_row.get(index).cloned())
                .or_else(|| {
                    self.measured_lines
                        .get(&position.uri)
                        .map(|entry| entry.0.clone())
                });
            let Some(lines) = lines else {
                continue;
            };
            let body_top = i64::from(position.y);
            let body_bottom = body_top + i64::from(metrics.body_height);
            if body_bottom <= bsky_pocket::visible_text::VIEWPORT_TOP
                || body_top >= bsky_pocket::visible_text::VIEWPORT_BOTTOM
            {
                continue;
            }
            let line_height = i64::from(metrics.line_height.max(1));
            let first_line = ((bsky_pocket::visible_text::VIEWPORT_TOP - body_top)
                .div_euclid(line_height))
            .max(0) as usize;
            let last_line = ((bsky_pocket::visible_text::VIEWPORT_BOTTOM - 1 - body_top)
                .div_euclid(line_height))
            .max(0) as usize;
            for (line, text) in lines.iter().enumerate().take(metrics.line_count as usize) {
                if line < first_line || line > last_line {
                    continue;
                }
                if std::time::Instant::now() >= deadline {
                    return Ok(());
                }
                let y = body_top
                    .saturating_add(i64::from(metrics.baseline_offset))
                    .saturating_add((line as i64).saturating_mul(line_height));
                if !(0..272).contains(&y) {
                    continue;
                }
                commands.push(NativeCommand::Text {
                    x: position.x as f32,
                    y: y as f32,
                    scale: 1.5,
                    color: 0xffffffff,
                    text: text.to_owned(),
                    stamp: Some(request.stamp.clone()),
                    line: line as u32,
                    line_height: metrics.line_height,
                });
            }
        }
        let base_commands: Vec<_> = self
            .native_batch
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    NativeCommand::Text { stamp: None, .. }
                        | NativeCommand::Image { .. }
                        | NativeCommand::ImageHandle { .. }
                )
            })
            .cloned()
            .collect();
        let lines: Vec<_> = commands
            .into_iter()
            .filter_map(|command| match command {
                NativeCommand::Text {
                    x,
                    y,
                    scale,
                    color,
                    text,
                    stamp,
                    line,
                    line_height,
                } => Some((x, y, scale, color, text, stamp, line, line_height)),
                NativeCommand::Image { .. } | NativeCommand::ImageHandle { .. } => None,
            })
            .collect();
        let line_height = 18.0;
        let packed = pack_lines(
            &base_commands,
            &lines,
            self.native_batch.image.clone(),
            line_height,
        )?;
        let mut final_batch = self.native_batch.clone();
        final_batch.commands = packed;
        final_batch.validate(480, 272)?;
        self.authorized_stamp = Some(request.stamp.clone());
        self.native_batch.commands = final_batch.commands;
        Ok(())
    }
    pub fn tick(&mut self, pad: &PadFrame, font: &Font) -> Result<(), String> {
        self.prepare(pad, font)
    }
    pub fn wants_exit(&self, pad: &PadFrame) -> bool {
        pad.just_pressed(buttons::CIRCLE)
    }
    pub fn draw(&self, frame: &mut Frame<'_>, font: &Font, cache: &TextureCache) {
        if let Some(texture) = self.framebuffer_texture.as_ref() {
            frame.draw_pocket_framebuffer(texture);
        }
        frame.with_clip_rectangle((0, 80, 960, 448), |frame| {
            for (slot, uri) in &self.avatar_slots {
                if let Some((x, y)) = self.avatar_positions.get(uri) {
                    let _ = frame.draw_image_handle(
                        cache,
                        &self.image_handles,
                        *slot,
                        (x - 6.0) * 2.0,
                        *y * 2.0,
                        20.0,
                        20.0,
                    );
                }
            }
            for command in &self.native_batch.commands {
                match command {
                    NativeCommand::Text {
                        stamp: Some(stamp), ..
                    } if self.authorized_stamp.as_ref() != Some(stamp) => continue,
                    NativeCommand::Text {
                        x,
                        y,
                        scale,
                        color,
                        text,
                        line_height,
                        ..
                    } => frame.draw_pocket_text(
                        font,
                        (*x * 2.0) as i32,
                        (*y * 2.0) as i32,
                        bsky_render::Color(*color),
                        *scale,
                        text,
                        i32::from(*line_height) * 2,
                    ),
                    NativeCommand::Image { generation, x, y }
                        if *generation == self.texture_generation =>
                    {
                        if let Some(texture) = self.image_texture.as_ref() {
                            frame.draw_texture(texture, *x * 2.0, *y * 2.0);
                        }
                    }
                    NativeCommand::ImageHandle {
                        handle,
                        x,
                        y,
                        width,
                        height,
                    } => {
                        let _ = frame.draw_image_handle(
                            cache,
                            &self.image_handles,
                            *handle,
                            *x * 2.0,
                            *y * 2.0,
                            *width * 2.0,
                            *height * 2.0,
                        );
                    }
                    NativeCommand::Image { .. } => {}
                }
            }
        });
    }
}

fn synthetic_rgba(generation: u32) -> Vec<u8> {
    let mut pixels = vec![0; 64 * 64 * 4];
    for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        let x = (index % 64) as u32;
        let y = (index / 64) as u32;
        let inside = x > 3 && y > 3 && x < 60 && y < 60;
        pixel.copy_from_slice(&[
            (40 + (generation.wrapping_mul(37) % 160)) as u8,
            if inside { 160 } else { 40 },
            240,
            if inside { 255 } else { 0 },
        ]);
    }
    pixels
}
