//! Bounded embedding of the PocketJS runtime beside the existing renderer.
pub mod adapters;
pub use bsky_feed as feed;
pub use bsky_feed::moderation;
pub use bsky_feed::{
    AuthorDto, ContentState, DisplayDecision, FacetRange, FeedCommand, FeedCoordinator,
    FeedCorrelation, FeedDelivery, FeedEffect, FeedEvent, FeedHistory, ImageMetadata,
    ModerationPolicy, PostDto,
};
mod quickjs;
pub mod scrolling;
pub mod text_batch;
pub mod visible_text;
pub use adapters::{TouchContact, TouchSnapshot};
pub use pocket_runtime_cabi;
use pocketjs_core::{DrawList, Ui};
pub use quickjs::{Clock, EmbeddedGuest};
use std::time::Duration;
pub const GUEST_HEAP_LIMIT: usize = 16 * 1024 * 1024;
pub const FRAME_BUDGET: Duration = Duration::from_millis(4);
pub const STARTUP_BUDGET: Duration = Duration::from_millis(2_000);
pub const MAX_NATIVE_COMMANDS: usize = 8;
pub const MAX_NATIVE_COMMAND_BYTES: usize = 4 * 1024;
pub const MAX_NATIVE_TEXT_BYTES: usize = 1024;
pub const MAX_NATIVE_IMAGE_SIDE: u32 = 64;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeCommand {
    Text {
        x: f32,
        y: f32,
        scale: f32,
        color: u32,
        text: String,
        stamp: Option<visible_text::Stamp>,
        line: u32,
        line_height: u16,
    },
    Image {
        generation: u32,
        x: f32,
        y: f32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeImage {
    pub generation: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBatch {
    pub commands: Vec<NativeCommand>,
    pub image: Option<NativeImage>,
}

impl NativeCommand {
    pub fn encoded_size(&self) -> usize {
        match self {
            NativeCommand::Text { text, .. } => 64 + text.len(),
            NativeCommand::Image { .. } => 64,
        }
    }
}

impl NativeBatch {
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|_| "native command serialization failed".to_string())
    }

    pub fn from_json(
        value: &str,
        viewport_width: u32,
        viewport_height: u32,
    ) -> Result<Self, String> {
        if value.len() > MAX_NATIVE_COMMAND_BYTES {
            return Err("native command JSON exceeds limit".into());
        }
        let batch: Self =
            serde_json::from_str(value).map_err(|_| "invalid native command JSON".to_string())?;
        batch.validate(viewport_width, viewport_height)?;
        Ok(batch)
    }

    pub fn validate(&self, viewport_width: u32, viewport_height: u32) -> Result<(), String> {
        if self.commands.len() > MAX_NATIVE_COMMANDS {
            return Err("native command count exceeds limit".into());
        }
        let mut bytes = 0usize;
        for command in &self.commands {
            match command {
                NativeCommand::Text {
                    x, y, scale, text, ..
                } => {
                    if !x.is_finite()
                        || !y.is_finite()
                        || !scale.is_finite()
                        || *scale < 0.75
                        || *scale > 1.5
                        || *x < 0.0
                        || *y < 0.0
                        || *x >= viewport_width as f32
                        || *y >= viewport_height as f32
                        || text.len() > MAX_NATIVE_TEXT_BYTES
                        || text.as_bytes().contains(&0)
                    {
                        return Err("invalid native text command".into());
                    }
                    bytes = bytes
                        .checked_add(48 + text.len())
                        .ok_or("native batch size overflow")?;
                }
                NativeCommand::Image { generation, x, y } => {
                    if !x.is_finite()
                        || !y.is_finite()
                        || *x < 0.0
                        || *y < 0.0
                        || *x >= viewport_width as f32
                        || *y >= viewport_height as f32
                        || self
                            .image
                            .as_ref()
                            .is_none_or(|image| image.generation != *generation)
                    {
                        return Err("invalid or stale native image command".into());
                    }
                    bytes = bytes.checked_add(12).ok_or("native batch size overflow")?;
                }
            }
        }
        if bytes > MAX_NATIVE_COMMAND_BYTES {
            return Err("native command batch exceeds byte limit".into());
        }
        if let Some(image) = &self.image {
            if self
                .commands
                .iter()
                .all(|command| !matches!(command, NativeCommand::Image { .. }))
            {
                return Err("unreferenced native image handle".into());
            }
            if image.width == 0
                || image.height == 0
                || image.width > MAX_NATIVE_IMAGE_SIDE
                || image.height > MAX_NATIVE_IMAGE_SIDE
                || image.generation == 0
                || image.width.checked_mul(image.height).is_none()
            {
                return Err("native image dimensions exceed limit".into());
            }
        }
        Ok(())
    }
}
pub fn frame_budget() -> Duration {
    configured_budget("BSKY_POCKET_FRAME_BUDGET_MS", FRAME_BUDGET)
}
pub fn startup_budget() -> Duration {
    configured_budget("BSKY_POCKET_STARTUP_BUDGET_MS", STARTUP_BUDGET)
}
fn configured_budget(name: &str, default: Duration) -> Duration {
    configured_budget_value(std::env::var(name).ok().as_deref(), default)
}
fn configured_budget_value(value: Option<&str>, default: Duration) -> Duration {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .map_or(default, |configured| configured.min(default))
}
#[cfg(test)]
mod tests {
    use super::{
        MAX_NATIVE_COMMAND_BYTES, MAX_NATIVE_COMMANDS, MAX_NATIVE_TEXT_BYTES, NativeBatch,
        NativeCommand, NativeImage, configured_budget_value,
    };
    use std::time::Duration;

    #[test]
    fn native_batch_json_roundtrips_unicode_and_rejects_stale_or_oversized_data() {
        let batch = NativeBatch {
            commands: vec![NativeCommand::Text {
                x: 4.0,
                y: 8.0,
                scale: 1.0,
                color: u32::MAX,
                text: "Probe 🛰".into(),
                stamp: None,
                line: 0,
                line_height: 18,
            }],
            image: None,
        };
        let encoded = batch.to_json().unwrap();
        assert_eq!(NativeBatch::from_json(&encoded, 480, 272).unwrap(), batch);
        assert!(
            NativeBatch::from_json(&" ".repeat(MAX_NATIVE_COMMAND_BYTES + 1), 480, 272).is_err()
        );
    }

    #[test]
    fn native_batch_rejects_bounds_and_stale_image_generation() {
        let stale = NativeBatch {
            commands: vec![NativeCommand::Image {
                generation: 2,
                x: 1.0,
                y: 1.0,
            }],
            image: Some(NativeImage {
                generation: 1,
                width: 1,
                height: 1,
            }),
        };
        assert!(stale.validate(480, 272).is_err());
        let oversized = NativeBatch {
            commands: vec![NativeCommand::Text {
                x: 1.0,
                y: 1.0,
                scale: 1.0,
                color: 0,
                text: "x".repeat(MAX_NATIVE_TEXT_BYTES + 1),
                stamp: None,
                line: 0,
                line_height: 18,
            }],
            image: None,
        };
        assert!(oversized.validate(480, 272).is_err());
        for scale in [0.74, 1.51, f32::INFINITY] {
            let out_of_range = NativeBatch {
                commands: vec![NativeCommand::Text {
                    x: 1.0,
                    y: 1.0,
                    scale,
                    color: 0,
                    text: "bounded scale".into(),
                    stamp: None,
                    line: 0,
                    line_height: 18,
                }],
                image: None,
            };
            assert!(out_of_range.validate(480, 272).is_err());
        }
        let bad_image = NativeBatch {
            commands: vec![],
            image: Some(NativeImage {
                generation: 3,
                width: 65,
                height: 1,
            }),
        };
        assert!(bad_image.validate(480, 272).is_err());
        let invalid = NativeBatch {
            commands: vec![NativeCommand::Text {
                x: f32::NAN,
                y: 0.0,
                scale: 1.0,
                color: 0,
                text: "x".into(),
                stamp: None,
                line: 0,
                line_height: 18,
            }],
            image: None,
        };
        assert!(invalid.validate(480, 272).is_err());
        let too_many = NativeBatch {
            commands: (0..=MAX_NATIVE_COMMANDS)
                .map(|_| NativeCommand::Text {
                    x: 0.0,
                    y: 0.0,
                    scale: 1.0,
                    color: 0,
                    text: String::new(),
                    stamp: None,
                    line: 0,
                    line_height: 18,
                })
                .collect(),
            image: None,
        };
        assert!(too_many.validate(480, 272).is_err());
    }

    #[test]
    fn configured_budgets_keep_defaults_and_never_raise_the_cap() {
        let default = Duration::from_millis(2000);
        assert_eq!(configured_budget_value(None, default), default);
        assert_eq!(configured_budget_value(Some("invalid"), default), default);
        assert_eq!(
            configured_budget_value(Some("250"), default),
            Duration::from_millis(250)
        );
        assert_eq!(configured_budget_value(Some("5000"), default), default);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestState {
    Active,
    Failed,
    Disposed,
}
pub fn new_ui(width: f32, height: f32) -> Ui {
    let mut ui = Ui::new();
    ui.set_viewport(width, height);
    ui
}
pub fn draw(ui: &mut Ui) -> &DrawList {
    ui.draw()
}
