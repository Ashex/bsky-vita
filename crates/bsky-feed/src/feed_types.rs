use std::collections::{HashSet, VecDeque};
use std::io::{self, Write};

fn encoded_size<T: serde::Serialize>(value: &T) -> Option<usize> {
    let mut writer = BoundedCounter {
        size: 0,
        overflow: false,
    };
    serde_json::to_writer(&mut writer, value).ok()?;
    (!writer.overflow).then_some(writer.size)
}

struct FeedDeliveryView<'data, 'posts, P> {
    schema: u8,
    correlation: &'data FeedCorrelation,
    posts: P,
    cursor: Option<&'data str>,
    posts_lifetime: std::marker::PhantomData<&'posts PostDto>,
}
impl<'data, 'posts, P> serde::Serialize for FeedDeliveryView<'data, 'posts, P>
where
    P: Iterator<Item = &'posts PostDto> + Clone,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::{SerializeSeq, SerializeStruct};
        struct Posts<'iter, 'posts, P>(&'iter P, std::marker::PhantomData<&'posts PostDto>);
        impl<'iter, 'posts, P> serde::Serialize for Posts<'iter, 'posts, P>
        where
            P: Iterator<Item = &'posts PostDto> + Clone,
        {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let mut seq = serializer.serialize_seq(None)?;
                for post in self.0.clone() {
                    seq.serialize_element(post)?;
                }
                seq.end()
            }
        }
        let mut state = serializer.serialize_struct("FeedDelivery", 4)?;
        state.serialize_field("schema", &self.schema)?;
        state.serialize_field("correlation", self.correlation)?;
        state.serialize_field("posts", &Posts(&self.posts, std::marker::PhantomData))?;
        state.serialize_field("cursor", &self.cursor)?;
        state.end()
    }
}

struct BoundedCounter {
    size: usize,
    overflow: bool,
}
impl Write for BoundedCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = MAX_DELIVERY_BYTES.saturating_sub(self.size);
        let accepted = bytes.len().min(remaining);
        self.size += accepted;
        self.overflow |= accepted != bytes.len();
        if self.overflow {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "delivery size exceeded",
            ))
        } else {
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub const SCHEMA_VERSION: u8 = 1;
pub const MAX_FIELD_BYTES: usize = 64 * 1024;
pub const MAX_POST_BYTES: usize = 64 * 1024;
pub const MAX_DELIVERY_BYTES: usize = 1024 * 1024;
pub const MAX_HISTORY: usize = 200;
pub const MAX_COMMANDS: usize = 128;
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
pub const MAX_NESTING_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedCorrelation {
    pub schema: u8,
    pub auth_generation: u64,
    pub mount_generation: u64,
    pub request_id: u64,
    pub source: String,
    pub cursor: Option<String>,
}
impl FeedCorrelation {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema != SCHEMA_VERSION {
            return Err("unsupported schema");
        }
        if self.request_id == 0 || self.mount_generation == 0 || self.auth_generation == 0 {
            return Err("invalid generation or request id");
        }
        if self.source.len() > MAX_FIELD_BYTES
            || self
                .cursor
                .as_ref()
                .is_some_and(|v| v.len() > MAX_FIELD_BYTES)
        {
            return Err("correlation field exceeds limit");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FacetRange {
    pub start: usize,
    pub end: usize,
    pub feature: String,
}
impl FacetRange {
    pub fn validate(&self, text: &str) -> bool {
        self.start < self.end
            && self.end <= text.len()
            && text.is_char_boundary(self.start)
            && text.is_char_boundary(self.end)
            && self.feature.len() <= MAX_FIELD_BYTES
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorDto {
    pub did: String,
    pub handle: String,
    pub display_name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMetadata {
    pub alt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "state",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ContentState {
    Clean,
    Warning(String),
    Blocked,
    Muted,
    Unavailable,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMarker {
    Reply,
    Repost,
    Quote,
    External,
    Video,
    Unsupported,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostDto {
    pub uri: String,
    pub cid: String,
    pub author: AuthorDto,
    pub text: String,
    pub facets: Vec<FacetRange>,
    pub like_count: u64,
    pub repost_count: u64,
    pub reply_count: u64,
    pub state: ContentState,
    pub quote: Option<Box<PostDto>>,
    pub images: Vec<ImageMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextMarker>,
}
impl PostDto {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_at_depth(0)
    }
    fn validate_at_depth(&self, depth: usize) -> Result<(), &'static str> {
        if depth >= MAX_NESTING_DEPTH {
            return Err("post nesting exceeds limit");
        }
        if [
            self.uri.len(),
            self.cid.len(),
            self.author.did.len(),
            self.author.handle.len(),
            self.author.display_name.len(),
            self.text.len(),
        ]
        .into_iter()
        .any(|n| n > MAX_FIELD_BYTES)
        {
            return Err("field exceeds limit");
        }
        if self.facets.iter().any(|f| !f.validate(&self.text)) {
            return Err("invalid UTF-8 facet range");
        }
        if self.images.iter().any(|i| i.alt.len() > MAX_FIELD_BYTES) {
            return Err("image metadata exceeds limit");
        }
        if self.encoded_size() > MAX_POST_BYTES {
            return Err("post exceeds limit");
        }
        if let Some(q) = &self.quote {
            q.validate_at_depth(depth + 1)?;
        }
        Ok(())
    }
    pub fn encoded_size(&self) -> usize {
        encoded_size(self).unwrap_or(usize::MAX)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedDelivery {
    pub schema: u8,
    pub correlation: FeedCorrelation,
    pub posts: Vec<PostDto>,
    pub cursor: Option<String>,
}
pub fn validate_delivery(value: serde_json::Value) -> Result<FeedDelivery, String> {
    if encoded_size(&value).ok_or("delivery serialization failed")? > MAX_DELIVERY_BYTES {
        return Err("feed delivery exceeds limit".into());
    }
    let delivery: FeedDelivery =
        serde_json::from_value(value).map_err(|_| "invalid feed delivery".to_string())?;
    if delivery.schema != SCHEMA_VERSION || delivery.posts.len() > MAX_HISTORY {
        return Err("invalid feed delivery schema or count".into());
    }
    delivery.correlation.validate().map_err(str::to_string)?;
    if delivery
        .cursor
        .as_ref()
        .is_some_and(|c| c.len() > MAX_FIELD_BYTES)
    {
        return Err("cursor exceeds limit".into());
    }
    for post in &delivery.posts {
        post.validate().map_err(str::to_string)?;
    }
    Ok(delivery)
}
#[derive(Default)]
pub struct FeedHistory {
    posts: VecDeque<PostDto>,
    ids: HashSet<String>,
}
impl FeedHistory {
    pub fn posts(&self) -> &VecDeque<PostDto> {
        &self.posts
    }
    pub fn append(
        &mut self,
        batch: impl IntoIterator<Item = PostDto>,
    ) -> Result<usize, &'static str> {
        self.append_with_envelope(
            batch,
            &FeedCorrelation {
                schema: SCHEMA_VERSION,
                auth_generation: 1,
                mount_generation: 1,
                request_id: 1,
                source: String::new(),
                cursor: None,
            },
            None,
        )
    }
    pub fn append_with_envelope(
        &mut self,
        batch: impl IntoIterator<Item = PostDto>,
        correlation: &FeedCorrelation,
        cursor: Option<&str>,
    ) -> Result<usize, &'static str> {
        let mut accepted = 0;
        for post in batch {
            post.validate()?;
            if Self::delivery_size_with_extra(&VecDeque::new(), &post, correlation, cursor)
                .is_none()
            {
                return Err("post exceeds delivery limit");
            }
            if self.ids.contains(&post.uri) {
                continue;
            }
            while self.posts.len() >= MAX_HISTORY
                || Self::delivery_size_with_extra(&self.posts, &post, correlation, cursor).is_none()
            {
                if let Some(old) = self.posts.pop_front() {
                    self.ids.remove(&old.uri);
                } else {
                    return Err("post exceeds delivery limit");
                }
            }
            self.ids.insert(post.uri.clone());
            self.posts.push_back(post);
            accepted += 1;
        }
        Ok(accepted)
    }
    fn delivery_size_with_extra<'a>(
        posts: &'a VecDeque<PostDto>,
        extra: &'a PostDto,
        correlation: &'a FeedCorrelation,
        cursor: Option<&'a str>,
    ) -> Option<usize> {
        let mut writer = BoundedCounter {
            size: 0,
            overflow: false,
        };
        let rows = posts.iter().chain(std::iter::once(extra));
        let candidate = FeedDeliveryView {
            schema: SCHEMA_VERSION,
            correlation,
            posts: rows,
            cursor,
            posts_lifetime: std::marker::PhantomData,
        };
        serde_json::to_writer(&mut writer, &candidate).ok()?;
        (!writer.overflow).then_some(writer.size)
    }
    pub fn delivery_size(posts: &[PostDto]) -> Option<usize> {
        Self::delivery_size_for(
            posts,
            &FeedCorrelation {
                schema: SCHEMA_VERSION,
                auth_generation: 1,
                mount_generation: 1,
                request_id: 1,
                source: String::new(),
                cursor: None,
            },
            None,
        )
    }
    pub fn delivery_size_for(
        posts: &[PostDto],
        correlation: &FeedCorrelation,
        cursor: Option<&str>,
    ) -> Option<usize> {
        encoded_size(&FeedDeliveryView {
            schema: SCHEMA_VERSION,
            correlation,
            posts: posts.iter(),
            cursor,
            posts_lifetime: std::marker::PhantomData,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedCommand {
    FirstPage,
    NextPage,
    Retry,
    Refresh,
    VisibleImage,
    Leave,
}
impl FeedCommand {
    pub fn parse(name: &str) -> Result<Self, &'static str> {
        match name {
            "first_page" => Ok(Self::FirstPage),
            "next_page" => Ok(Self::NextPage),
            "retry" => Ok(Self::Retry),
            "refresh" => Ok(Self::Refresh),
            "visible_image" => Ok(Self::VisibleImage),
            "leave" => Ok(Self::Leave),
            _ => Err("unknown command"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedEffect {
    FetchPage(FeedCorrelation),
    PublishDelivery(FeedDelivery),
    PublishStatus(String),
    InvalidatePresentation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedEvent {
    AuthChanged {
        generation: u64,
    },
    Open,
    Close,
    Command(FeedCommand),
    Completed {
        correlation: FeedCorrelation,
        result: Result<FeedDelivery, String>,
    },
}
#[derive(Default)]
pub struct FeedCoordinator {
    auth_generation: u64,
    mount_generation: u64,
    request_id: u64,
    active: Option<FeedCorrelation>,
    queued: Option<(FeedCorrelation, FeedCommand)>,
    history: FeedHistory,
    history_revision: u64,
    cursor: Option<String>,
    error: Option<String>,
    failed: Option<FeedCorrelation>,
    closed_mounts: HashSet<u64>,
}
impl FeedCoordinator {
    pub fn auth_generation(&self) -> u64 {
        self.auth_generation
    }
    pub fn mount_generation(&self) -> u64 {
        self.mount_generation
    }
    pub fn active(&self) -> Option<&FeedCorrelation> {
        self.active.as_ref()
    }
    pub fn history(&self) -> &FeedHistory {
        &self.history
    }
    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn open_mount(&mut self) -> u64 {
        self.mount_generation = self.mount_generation.wrapping_add(1).max(1);
        self.closed_mounts.remove(&self.mount_generation);
        self.error = None;
        self.failed = None;
        self.mount_generation
    }
    pub fn close_mount(&mut self, mount: u64) -> Vec<FeedEffect> {
        if mount == self.mount_generation {
            self.closed_mounts.insert(mount);
            self.queued = None;
            self.failed = None;
            vec![FeedEffect::InvalidatePresentation]
        } else {
            Vec::new()
        }
    }
    pub fn auth_changed(&mut self, generation: u64) -> Vec<FeedEffect> {
        if generation == self.auth_generation {
            return Vec::new();
        }
        self.auth_generation = generation;
        self.active = None;
        self.queued = None;
        self.history = FeedHistory::default();
        self.cursor = None;
        self.error = None;
        self.failed = None;
        vec![FeedEffect::InvalidatePresentation]
    }
    fn correlation(&mut self, cursor: Option<String>) -> FeedCorrelation {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        FeedCorrelation {
            schema: SCHEMA_VERSION,
            auth_generation: self.auth_generation,
            mount_generation: self.mount_generation,
            request_id: self.request_id,
            source: "Following".into(),
            cursor,
        }
    }
    pub fn command(&mut self, command: FeedCommand) -> Vec<FeedEffect> {
        if command == FeedCommand::Leave {
            return self.close_mount(self.mount_generation);
        }
        if self.auth_generation == 0 || self.closed_mounts.contains(&self.mount_generation) {
            return Vec::new();
        }
        let command = if command == FeedCommand::Retry {
            match self.failed.as_ref() {
                Some(failed) => {
                    if self.active.is_some() {
                        return Vec::new();
                    }
                    let correlation = self.correlation(failed.cursor.clone());
                    self.failed = None;
                    self.active = Some(correlation.clone());
                    return vec![FeedEffect::FetchPage(correlation)];
                }
                None => return Vec::new(),
            }
        } else {
            command
        };
        let cursor = match command {
            FeedCommand::FirstPage | FeedCommand::Refresh => None,
            FeedCommand::NextPage => match self.cursor.clone() {
                Some(c) => Some(c),
                None => return Vec::new(),
            },
            FeedCommand::Retry | FeedCommand::VisibleImage | FeedCommand::Leave => {
                return Vec::new()
            }
        };
        let correlation = self.correlation(cursor);
        if self.active.is_some() {
            if command == FeedCommand::Refresh
                || self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.mount_generation != correlation.mount_generation)
            {
                self.queued = Some((correlation, command));
            }
            return Vec::new();
        }
        self.active = Some(correlation.clone());
        vec![FeedEffect::FetchPage(correlation)]
    }
    pub fn complete(
        &mut self,
        correlation: &FeedCorrelation,
        result: Result<FeedDelivery, String>,
    ) -> Vec<FeedEffect> {
        if self.active.as_ref() != Some(correlation) {
            return Vec::new();
        }
        self.active = None;
        let current = correlation.auth_generation == self.auth_generation
            && correlation.mount_generation == self.mount_generation
            && !self.closed_mounts.contains(&correlation.mount_generation);
        let mut effects = Vec::new();
        if current {
            match result {
                Ok(delivery) if delivery.correlation == *correlation => {
                    if delivery.schema != SCHEMA_VERSION
                        || delivery.posts.len() > MAX_HISTORY
                        || delivery.posts.iter().any(|p| p.validate().is_err())
                    {
                        self.error = Some("invalid feed delivery".into())
                    } else {
                        self.failed = None;
                        if correlation.cursor.is_none() {
                            self.history = FeedHistory::default()
                        }
                        if self
                            .history
                            .append_with_envelope(
                                delivery.posts,
                                correlation,
                                delivery.cursor.as_deref(),
                            )
                            .is_ok()
                        {
                            self.history_revision = self.history_revision.wrapping_add(1).max(1);
                            self.cursor = delivery.cursor;
                            self.error = None;
                            let posts = self.history.posts().iter().cloned().collect::<Vec<_>>();
                            effects.push(FeedEffect::PublishDelivery(FeedDelivery {
                                schema: SCHEMA_VERSION,
                                correlation: correlation.clone(),
                                posts,
                                cursor: self.cursor.clone(),
                            }))
                        } else {
                            self.error = Some("feed history exceeds limits".into())
                        }
                    }
                }
                Ok(_) => {
                    self.active = Some(correlation.clone());
                    return Vec::new();
                }
                Err(error) => {
                    self.failed = Some(correlation.clone());
                    self.error = Some(error);
                }
            }
            if let Some(error) = &self.error {
                effects.push(FeedEffect::PublishStatus(error.clone()))
            }
        }
        if let Some((mut next, command)) = self.queued.take() {
            if next.auth_generation == self.auth_generation
                && next.mount_generation == self.mount_generation
                && !self.closed_mounts.contains(&next.mount_generation)
            {
                next.cursor = if command == FeedCommand::NextPage {
                    self.cursor.clone()
                } else {
                    None
                };
                self.active = Some(next.clone());
                effects.push(FeedEffect::FetchPage(next))
            }
        }
        effects
    }
    pub fn handle(&mut self, event: FeedEvent) -> Vec<FeedEffect> {
        match event {
            FeedEvent::AuthChanged { generation } => self.auth_changed(generation),
            FeedEvent::Open => {
                self.open_mount();
                vec![FeedEffect::InvalidatePresentation]
            }
            FeedEvent::Close => self.close_mount(self.mount_generation),
            FeedEvent::Command(c) => self.command(c),
            FeedEvent::Completed {
                correlation,
                result,
            } => self.complete(&correlation, result),
        }
    }
    pub fn abandon_mount(&mut self, mount: u64) {
        let _ = self.close_mount(mount);
    }
    pub fn clear(&mut self) {
        self.auth_changed(self.auth_generation.wrapping_add(1).max(1));
    }
}
