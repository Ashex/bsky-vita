use bsky_input::{buttons, PadFrame};
use bsky_pocket::{EmbeddedGuest, NativeCommand};
use bsky_render::{Font, Frame, Texture};

pub struct PocketShell {
    coordinator: std::sync::Arc<std::sync::Mutex<bsky_feed::FeedCoordinator>>,
    guest: EmbeddedGuest,
    image_texture: Option<Texture>,
    framebuffer_texture: Option<Texture>,
    native_batch: bsky_pocket::NativeBatch,
    authorized_stamp: Option<bsky_pocket::visible_text::Stamp>,
    texture_generation: u32,
    auth_generation: u64,
    mount_generation: u64,
    layout_id: u64,
    pending_delivery: Option<bsky_feed::FeedDelivery>,
    pending_status: Option<String>,
}

impl PocketShell {
    pub fn new(
        coordinator: std::sync::Arc<std::sync::Mutex<bsky_feed::FeedCoordinator>>,
    ) -> Result<Self, String> {
        Ok(Self {
            coordinator,
            guest: EmbeddedGuest::new(480, 272)?,
            image_texture: None,
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
            pending_delivery: None,
            pending_status: None,
        })
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
        self.native_batch
            .commands
            .retain(|c| !matches!(c, NativeCommand::Text { stamp: Some(_), .. }));
    }
    pub fn bind_auth(&mut self, generation: u64) -> Vec<bsky_feed::FeedEffect> {
        if self.auth_generation != generation {
            self.auth_generation = generation;
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
            font,
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
    }
    pub fn prepare_until(
        &mut self,
        pad: &PadFrame,
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
        self.guest
            .advance_with_deadline(crate::pocket_input::guest_buttons(pad.current), deadline)?;
        self.native_batch = self.guest.native_batch_with_budget(
            deadline.saturating_duration_since(std::time::Instant::now()),
        )?;

        if delivered_feed {
            self.authorized_stamp = None;
            self.framebuffer_texture = None;
            self.native_batch
                .commands
                .retain(|command| !matches!(command, NativeCommand::Text { stamp: Some(_), .. }));
        }
        self.run_visible_text(font, deadline)?;
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
        self.guest.present_with_deadline(deadline)?;
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
            let history: Vec<String> = coordinator
                .history()
                .posts()
                .iter()
                .map(|post| post.uri.clone())
                .collect();
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
                .filter(|request| {
                    request.stamp.auth_generation == self.auth_generation.max(1)
                        && request.stamp.mount_generation == self.mount_generation.max(1)
                        && request.stamp.history_revision == coordinator.history_revision().max(1)
                })
                .and_then(|request| {
                    let uris: Vec<String> = request.rows.into_iter().map(|row| row.uri).collect();
                    bsky_pocket::visible_text::validate_window(&history, &uris)
                });
            let rows = requested.unwrap_or_else(|| {
                history
                    .iter()
                    .take(bsky_pocket::visible_text::MAX_ROWS)
                    .cloned()
                    .collect()
            });
            let rows: Vec<bsky_pocket::visible_text::MeasureRow> = rows
                .iter()
                .map(|uri| bsky_pocket::visible_text::MeasureRow {
                    uri: uri.clone(),
                    width: 440,
                })
                .collect();
            (coordinator.history_revision().max(1), rows)
        };
        if rows.is_empty() {
            self.authorized_stamp = None;
            self.framebuffer_texture = None;
            self.native_batch
                .commands
                .retain(|c| !matches!(c, NativeCommand::Text { stamp: Some(_), .. }));
            return Ok(());
        }
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
        let mut results = Vec::new();
        let mut lines_by_row = Vec::new();
        let mut total_calls = 0usize;
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
            if text.len() > bsky_pocket::visible_text::MAX_TEXT_BYTES {
                return Ok(());
            }
            let status = match candidate.get("status").and_then(serde_json::Value::as_str) {
                Some("ready") => MeasureStatus::Ready,
                Some("placeholder") => MeasureStatus::Placeholder,
                _ => return Ok(()),
            };
            let (lines, continuation) = if status == MeasureStatus::Placeholder {
                (vec!["Content unavailable".to_string()], false)
            } else {
                let mut calls = 0;
                let wrapped = bsky_pocket::visible_text::wrap_bounded(
                    text,
                    expected.width.saturating_mul(2),
                    |line| {
                        if std::time::Instant::now() >= deadline {
                            return u16::MAX;
                        }
                        calls += 1;
                        total_calls += 1;
                        bsky_render::measure_native_text(font, 1.5, line)
                            .0
                            .max(0)
                            .min(u16::MAX as i32) as u16
                    },
                );
                if calls > bsky_pocket::visible_text::MAX_MEASURE_CALLS_ROW
                    || total_calls > bsky_pocket::visible_text::MAX_MEASURE_CALLS_TOTAL
                {
                    return Ok(());
                }
                (wrapped.0.into_iter().take(2).collect::<Vec<_>>(), wrapped.1)
            };
            if lines.iter().map(String::len).sum::<usize>()
                > bsky_pocket::visible_text::MAX_ROW_BYTES
            {
                return Ok(());
            }
            let count = lines.len() as u8;
            let (_, native_height) = bsky_render::measure_native_text(font, 0.75, "Ag");
            let line_height = native_height.clamp(1, u16::MAX as i32) as u16;
            let baseline_offset = line_height;
            results.push(MeasureResult {
                uri: uri.into(),
                status,
                width: expected.width,
                line_count: count,
                line_height,
                baseline_offset,
                body_height: (count as u16 * line_height).max(line_height),
                continuation,
            });
            lines_by_row.push(lines);
        }
        let reply = MeasureReply {
            schema: bsky_pocket::visible_text::VISIBLE_SCHEMA,
            stamp: request.stamp.clone(),
            rows: results,
        };
        reply.validate_for(&request).map_err(str::to_string)?;
        let encoded = reply.to_bounded_json().map_err(str::to_string)?;
        self.guest
            .set_global_json("__bskyVisibleMeasurement", &encoded, deadline)?;
        self.guest.eval_global_with_deadline(
            "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
            deadline,
        )?;
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
        placement.validate_for(&request).map_err(str::to_string)?;
        let mut commands = Vec::new();
        for ((position, metrics), lines) in
            placement.rows.iter().zip(&reply.rows).zip(&lines_by_row)
        {
            for (line, text) in lines.iter().take(metrics.line_count as usize).enumerate() {
                if std::time::Instant::now() >= deadline {
                    return Ok(());
                }
                let y = position.y + metrics.baseline_offset + line as u16 * metrics.line_height;
                if y >= 272 {
                    continue;
                }
                commands.push(NativeCommand::Text {
                    x: position.x as f32,
                    y: y as f32,
                    scale: 0.75,
                    color: 0xffffffff,
                    text: text.clone(),
                    stamp: Some(request.stamp.clone()),
                    line: line as u8,
                });
            }
        }
        if self.native_batch.commands.len() + commands.len() <= bsky_pocket::MAX_NATIVE_COMMANDS {
            self.authorized_stamp = Some(request.stamp.clone());
            self.native_batch
                .commands
                .retain(|c| !matches!(c, NativeCommand::Text { stamp: Some(_), .. }));
            self.native_batch.commands.extend(commands);
        }
        Ok(())
    }
    pub fn tick(&mut self, pad: &PadFrame, font: &Font) -> Result<(), String> {
        self.prepare(pad, font)
    }
    pub fn wants_exit(&self, pad: &PadFrame) -> bool {
        pad.just_pressed(buttons::CIRCLE)
    }
    pub fn draw(&self, frame: &mut Frame<'_>, font: &Font) {
        if let Some(texture) = self.framebuffer_texture.as_ref() {
            frame.draw_pocket_framebuffer(texture);
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
                    ..
                } => frame.draw_pocket_text(
                    font,
                    (*x * 2.0) as i32,
                    (*y * 2.0) as i32,
                    bsky_render::Color(*color),
                    *scale * 2.0,
                    text,
                ),
                NativeCommand::Image { generation, x, y }
                    if *generation == self.texture_generation =>
                {
                    if let Some(texture) = self.image_texture.as_ref() {
                        frame.draw_texture(texture, *x * 2.0, *y * 2.0);
                    }
                }
                NativeCommand::Image { .. } => {}
            }
        }
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
