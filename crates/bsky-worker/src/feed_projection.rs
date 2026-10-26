use atrium_api::app::bsky::feed::defs::{FeedViewPost, PostView, PostViewEmbedRefs};
use atrium_api::app::bsky::feed::post::RecordData;
use atrium_api::app::bsky::richtext::facet::MainFeaturesItem;
use atrium_api::types::{TryFromUnknown, Union};
use bsky_feed::{
    AccountScope, AssetIdentity, AuthorDto, ContentState, ContextMarker, DisplayDecision,
    FacetRange, FeedCorrelation as PocketCorrelation, FeedDelivery, ImageMetadata,
    ModerationPolicy, NativeAssetDescriptor, PostDto, ProjectedFeed, Relationship,
    MAX_ACCOUNT_DID_BYTES, MAX_DELIVERY_BYTES, MAX_FIELD_BYTES, MAX_REVISION_URL_BYTES,
    SCHEMA_VERSION,
};

pub(crate) fn project_page(
    correlation: crate::FeedCorrelation,
    posts: Vec<FeedViewPost>,
    cursor: Option<String>,
) -> Result<FeedDelivery, String> {
    project_page_with_native(correlation, posts, cursor, None).map(|projected| projected.delivery)
}

pub(crate) fn project_page_with_native(
    correlation: crate::FeedCorrelation,
    posts: Vec<FeedViewPost>,
    cursor: Option<String>,
    account_did: Option<String>,
) -> Result<ProjectedFeed, String> {
    let correlation = PocketCorrelation {
        schema: correlation.schema,
        auth_generation: correlation.auth_generation,
        mount_generation: correlation.mount_generation,
        request_id: correlation.request_id,
        source: correlation.source,
        cursor: correlation.cursor,
    };
    correlation.validate().map_err(str::to_string)?;
    if cursor
        .as_ref()
        .is_some_and(|value| value.len() > MAX_FIELD_BYTES)
    {
        return Err("feed cursor exceeds limit".into());
    }
    let policy = ModerationPolicy::default();
    if posts.len() > bsky_feed::MAX_HISTORY {
        return Err("post count exceeds maximum".into());
    }
    let mut projected = Vec::with_capacity(posts.len());
    let scope = account_did.map(|account_did| AccountScope {
        account_did,
        auth_generation: correlation.auth_generation,
    });
    let mut descriptors = Vec::new();
    let mut descriptor_bytes = 0usize;
    const DESCRIPTOR_LIMIT: usize = bsky_feed::MAX_HISTORY * 9;
    const DESCRIPTOR_BYTES_LIMIT: usize = 4 * 1024 * 1024;
    let scope_did_len = scope
        .as_ref()
        .map_or(0, |s| s.account_did.len());
    for (post_index, item) in posts.iter().enumerate() {
        let mut dto = project_post(&item.post, &policy);
        apply_feed_context(&mut dto, &item.data, &policy);
        if dto.validate().is_err()
            || serde_json::to_vec(&dto)
                .map_or(true, |bytes| bytes.len() > bsky_feed::MAX_POST_BYTES)
        {
            dto = placeholder(&dto, ContentState::Unavailable, "post exceeds limits");
        }
        if dto.validate().is_err()
            || serde_json::to_vec(&dto)
                .map_or(true, |bytes| bytes.len() > bsky_feed::MAX_POST_BYTES)
        {
            return Err("bounded placeholder exceeds DTO limits".into());
        }
        if dto.state == ContentState::Clean {
            if let Some(scope) = &scope {
                let author = &item.post.data.author.data;
                if let Some(avatar) = &author.avatar {
                    if avatar.len() > MAX_REVISION_URL_BYTES {
                        return Err("avatar URL exceeds limit".into());
                    }
                    let author_did = author.did.to_string();
                    if author_did.len() > MAX_ACCOUNT_DID_BYTES {
                        return Err("author DID exceeds limit".into());
                    }
                    let handle =
                        format!("asset:{}:{}:avatar", correlation.request_id, post_index);
                    if handle.len() > 256 {
                        return Err("avatar handle exceeds limit".into());
                    }
                    let descriptor_cost = scope_did_len
                        .saturating_add(handle.len())
                        .saturating_add(author_did.len())
                        .saturating_add(avatar.len());
                    if descriptors.len() < DESCRIPTOR_LIMIT
                        && descriptor_bytes
                            .saturating_add(descriptor_cost)
                            <= DESCRIPTOR_BYTES_LIMIT
                    {
                        descriptor_bytes =
                            descriptor_bytes.saturating_add(descriptor_cost);
                        descriptors.push(NativeAssetDescriptor {
                            scope: scope.clone(),
                            identity: AssetIdentity::Avatar {
                                author_did,
                                source_revision: avatar.clone(),
                            },
                            handle,
                            relationship: if author.viewer.as_ref().and_then(|v| v.data.following.as_ref()).is_some() {
                                Relationship::Followed
                            } else {
                                Relationship::Unknown
                            },
                            following_confirmed: author.viewer.as_ref().and_then(|v| v.data.following.as_ref()).is_some(),
                        });
                    }
                }
                let images = post_image_views(item.post.data.embed.as_ref());
                if !images.is_empty() {
                    for (index, image) in images.iter().enumerate() {
                        if image.data.thumb.len() > MAX_REVISION_URL_BYTES
                            || image.data.fullsize.len() > MAX_REVISION_URL_BYTES
                        {
                            break;
                        }
                        let handle = format!(
                            "asset:{}:{}:{}",
                            correlation.request_id, post_index, index
                        );
                        if handle.len() > 256 {
                            break;
                        }
                        let descriptor_cost = scope_did_len
                            .saturating_add(handle.len())
                            .saturating_add(image.data.thumb.len())
                            .saturating_add(image.data.fullsize.len())
                            .saturating_add(dto.uri.len())
                            .saturating_add(dto.cid.len());
                        if descriptors.len() >= DESCRIPTOR_LIMIT
                            || descriptor_bytes.saturating_add(descriptor_cost)
                                > DESCRIPTOR_BYTES_LIMIT
                        {
                            break;
                        }
                        descriptor_bytes = descriptor_bytes.saturating_add(descriptor_cost);
                        descriptors.push(NativeAssetDescriptor {
                            scope: scope.clone(),
                            identity: AssetIdentity::PostImage {
                                post_uri: dto.uri.clone(),
                                cid: dto.cid.clone(),
                                index: index as u32,
                                thumbnail: image.data.thumb.clone(),
                                fullsize: image.data.fullsize.clone(),
                            },
                            handle,
                            relationship: Relationship::Unknown,
                            following_confirmed: false,
                        });
                    }
                }
            }
        }
        projected.push(dto);
    }
    let mut delivery = FeedDelivery {
        schema: SCHEMA_VERSION,
        correlation,
        posts: projected,
        cursor,
    };
    if serialize_size(&delivery)? > MAX_DELIVERY_BYTES {
        delivery.posts = posts
            .iter()
            .map(|item| {
                let mut dto = minimal_identity(&item.post);
                dto.state = ContentState::Unavailable;
                dto
            })
            .collect();
        if serialize_size(&delivery)? > MAX_DELIVERY_BYTES {
            return Err("feed delivery exceeds limit".into());
        }
        descriptors.clear();
    }
    descriptors.retain(|descriptor| match &descriptor.identity {
        AssetIdentity::Avatar { author_did, .. } => delivery
            .posts
            .iter()
            .any(|post| post.state == ContentState::Clean && post.author.did == *author_did),
        AssetIdentity::PostImage { post_uri, .. } => delivery
            .posts
            .iter()
            .any(|post| post.uri == *post_uri && post.state == ContentState::Clean),
    });
    let projected = ProjectedFeed::new(delivery, descriptors);
    if let Some(scope) = &scope {
        if scope.account_did.is_empty()
            || !projected.validate_for(&scope.account_did)
        {
            return Err("projected feed envelope failed validation".into());
        }
    }
    Ok(projected)
}

fn serialize_size(delivery: &FeedDelivery) -> Result<usize, String> {
    serde_json::to_vec(delivery)
        .map(|bytes| bytes.len())
        .map_err(|_| "feed DTO serialization failed".into())
}

fn project_post(post: &PostView, policy: &ModerationPolicy) -> PostDto {
    let data = &post.data;
    let author = &data.author.data;
    let record = RecordData::try_from_unknown(data.record.clone()).ok();
    let state = match author_state(
        data.labels.as_deref(),
        author.viewer.as_ref().map(|viewer| &viewer.data),
        author.labels.as_deref(),
    ) {
        Ok(state) if record.is_some() => {
            let viewer = data.viewer.as_ref().map(|viewer| &viewer.data);
            match viewer {
                Some(viewer) if viewer.embedding_disabled == Some(true) => ContentState::Blocked,
                _ => state,
            }
        }
        Ok(_) | Err(()) => ContentState::Unknown,
    };
    let record_state = match (data.labels.as_ref(), author.labels.as_ref()) {
        (Some(post_labels), Some(author_labels)) => record.as_ref().map(|record| {
            let record_warning = record.labels.as_ref().is_some_and(|labels| match labels {
                Union::Refs(atrium_api::app::bsky::feed::post::RecordLabelsRefs::ComAtprotoLabelDefsSelfLabels(values)) => values.data.values.iter().any(|label| is_warning(&label.data.val)),
                Union::Unknown(_) => true,
            });
            if record_warning { Some(ContentState::Warning("content warning".into())) }
            else if post_labels.iter().chain(author_labels.iter()).any(|label| matches!(label.data.val.as_str(), "!hide" | "!no-unauthenticated")) { Some(ContentState::Blocked) }
            else { None }
        }).flatten(),
        _ => None,
    };
    let warning = record.as_ref().and_then(|record| {
        record.labels.as_ref().and_then(|labels| match labels {
            Union::Refs(
                atrium_api::app::bsky::feed::post::RecordLabelsRefs::ComAtprotoLabelDefsSelfLabels(
                    labels,
                ),
            ) => labels
                .data
                .values
                .iter()
                .any(|label| is_warning(&label.data.val))
                .then(|| "content warning".to_string()),
            Union::Unknown(_) => Some("unsupported record label".into()),
        })
    });
    let state = match record_state {
        Some(state) => state,
        None => warning.map_or(state, ContentState::Warning),
    };
    let mut dto = PostDto {
        uri: bounded(&data.uri),
        cid: bounded(&data.cid.as_ref().to_string()),
        author: AuthorDto {
            did: bounded(&author.did.to_string()),
            handle: bounded(&author.handle.to_string()),
            display_name: bounded(author.display_name.as_deref().unwrap_or_default()),
        },
        text: String::new(),
        facets: Vec::new(),
        like_count: data.like_count.unwrap_or_default().max(0) as u64,
        repost_count: data.repost_count.unwrap_or_default().max(0) as u64,
        reply_count: data.reply_count.unwrap_or_default().max(0) as u64,
        state,
        quote: None,
        images: Vec::new(),
        context: None,
    };
    if policy.decide(&dto) == DisplayDecision::Show {
        if let Some(record) = record {
            if record.text.len() <= MAX_FIELD_BYTES {
                dto.text = record.text;
                dto.facets = record
                    .facets
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .flat_map(|facet| {
                        facet.data.features.iter().map(|feature| {
                            let feature = match feature {
                                Union::Refs(MainFeaturesItem::Mention(_)) => "mention",
                                Union::Refs(MainFeaturesItem::Link(_)) => "link",
                                Union::Refs(MainFeaturesItem::Tag(_)) => "tag",
                                Union::Unknown(_) => "unsupported",
                            };
                            FacetRange {
                                start: facet.data.index.data.byte_start,
                                end: facet.data.index.data.byte_end,
                                feature: feature.into(),
                            }
                        })
                    })
                    .collect();
                dto.images = post_image_views(data.embed.as_ref())
                    .iter()
                    .map(|image| image_metadata(image))
                    .collect();
            } else {
                dto.state = ContentState::Unavailable;
            }
        }
    }
    dto
}

fn apply_feed_context(
    dto: &mut PostDto,
    item: &atrium_api::app::bsky::feed::defs::FeedViewPostData,
    policy: &ModerationPolicy,
) {
    if policy.decide(dto) != DisplayDecision::Show {
        clear_content(dto);
        return;
    }
    if let Some(embed) = &item.post.data.embed {
        match embed {
            Union::Refs(PostViewEmbedRefs::AppBskyEmbedRecordView(view)) => {
                dto.context = Some(ContextMarker::Quote);
                project_quote(dto, &view.data.record, policy);
            }
            Union::Refs(PostViewEmbedRefs::AppBskyEmbedRecordWithMediaView(view)) => {
                dto.context = Some(ContextMarker::Quote);
                project_quote(dto, &view.data.record.data.record, policy);
            }
            Union::Unknown(_) => {
                dto.context = Some(ContextMarker::Unsupported);
                dto.state = ContentState::Unknown;
                dto.text.clear();
                dto.facets.clear();
                dto.images.clear();
            }
            Union::Refs(PostViewEmbedRefs::AppBskyEmbedExternalView(_)) => {
                dto.context = Some(ContextMarker::External)
            }
            Union::Refs(PostViewEmbedRefs::AppBskyEmbedVideoView(_)) => {
                dto.context = Some(ContextMarker::Video)
            }
            _ => dto.context = Some(ContextMarker::Unsupported),
        }
    }
    if let Some(reply) = &item.reply {
        dto.context = Some(ContextMarker::Reply);
        let grandparent_hidden = reply
            .data
            .grandparent_author
            .as_ref()
            .is_some_and(|profile| {
                let profile = &profile.data;
                profile.viewer.as_ref().is_some_and(|viewer| {
                    viewer.data.blocked_by == Some(true)
                        || viewer.data.blocking.is_some()
                        || viewer.data.blocking_by_list.is_some()
                        || viewer.data.muted == Some(true)
                        || viewer.data.muted_by_list.is_some()
                })
            });
        let unavailable = grandparent_hidden
            || match &reply.data.parent {
                Union::Refs(atrium_api::app::bsky::feed::defs::ReplyRefParentRefs::PostView(
                    post,
                )) => {
                    author_state(
                        post.data.labels.as_deref(),
                        post.data.author.data.viewer.as_ref().map(|v| &v.data),
                        post.data.author.data.labels.as_deref(),
                    ) != Ok(ContentState::Clean)
                }
                Union::Refs(
                    atrium_api::app::bsky::feed::defs::ReplyRefParentRefs::BlockedPost(_)
                    | atrium_api::app::bsky::feed::defs::ReplyRefParentRefs::NotFoundPost(_),
                )
                | Union::Unknown(_) => true,
            }
            || match &reply.data.root {
                Union::Refs(atrium_api::app::bsky::feed::defs::ReplyRefRootRefs::PostView(
                    post,
                )) => {
                    author_state(
                        post.data.labels.as_deref(),
                        post.data.author.data.viewer.as_ref().map(|v| &v.data),
                        post.data.author.data.labels.as_deref(),
                    ) != Ok(ContentState::Clean)
                }
                Union::Refs(
                    atrium_api::app::bsky::feed::defs::ReplyRefRootRefs::BlockedPost(_)
                    | atrium_api::app::bsky::feed::defs::ReplyRefRootRefs::NotFoundPost(_),
                )
                | Union::Unknown(_) => true,
            };
        if unavailable {
            dto.state = ContentState::Unavailable;
            clear_content(dto);
            return;
        }
    }
    if let Some(Union::Refs(
        atrium_api::app::bsky::feed::defs::FeedViewPostReasonRefs::ReasonRepost(reason),
    )) = &item.reason
    {
        if dto.context.is_none() {
            dto.context = Some(ContextMarker::Repost);
        }
        let profile = &reason.data.by.data;
        if profile.viewer.as_ref().is_some_and(|viewer| {
            viewer.data.blocked_by == Some(true)
                || viewer.data.blocking.is_some()
                || viewer.data.blocking_by_list.is_some()
                || viewer.data.muted == Some(true)
                || viewer.data.muted_by_list.is_some()
        }) {
            dto.state = ContentState::Unknown;
            clear_content(dto);
        }
    }
}

fn project_quote(
    dto: &mut PostDto,
    quote: &Union<atrium_api::app::bsky::embed::record::ViewRecordRefs>,
    policy: &ModerationPolicy,
) {
    match quote {
        Union::Refs(atrium_api::app::bsky::embed::record::ViewRecordRefs::ViewRecord(record)) => {
            let profile = &record.data.author.data;
            let record_data = RecordData::try_from_unknown(record.data.value.clone()).ok();
            let record_state = record_data.as_ref().and_then(|data| data.labels.as_ref()).map(|labels| match labels {
                Union::Refs(atrium_api::app::bsky::feed::post::RecordLabelsRefs::ComAtprotoLabelDefsSelfLabels(values)) => {
                    if values.data.values.iter().any(|label| is_warning(&label.data.val)) {
                        ContentState::Warning("content warning".into())
                    } else if values.data.values.iter().any(|label| !label.data.val.starts_with('!') && !label.data.val.starts_with("sys:")) {
                        ContentState::Warning("labeled content".into())
                    } else {
                        ContentState::Clean
                    }
                }
                Union::Unknown(_) => ContentState::Unknown,
            }).unwrap_or(ContentState::Clean);
            if !matches!(
                author_state(
                    record.data.labels.as_deref(),
                    profile.viewer.as_ref().map(|v| &v.data),
                    profile.labels.as_deref()
                ),
                Ok(ContentState::Clean)
            ) || record_data.is_none()
                || policy.decide_state(&profile.did.to_string(), &record_state)
                    != DisplayDecision::Show
            {
                dto.quote = None;
                return;
            }
            let record_data = record_data.unwrap();
            if record_data.text.len() > MAX_FIELD_BYTES {
                dto.quote = None;
                return;
            }
            dto.quote = Some(Box::new(PostDto {
                uri: bounded(&record.data.uri),
                cid: bounded(&record.data.cid.as_ref().to_string()),
                author: AuthorDto {
                    did: bounded(&profile.did.to_string()),
                    handle: bounded(&profile.handle.to_string()),
                    display_name: bounded(profile.display_name.as_deref().unwrap_or_default()),
                },
                text: record_data.text,
                facets: Vec::new(),
                like_count: record.data.like_count.unwrap_or_default().max(0) as u64,
                repost_count: record.data.repost_count.unwrap_or_default().max(0) as u64,
                reply_count: record.data.reply_count.unwrap_or_default().max(0) as u64,
                state: record_state,
                quote: None,
                images: Vec::new(),
                context: None,
            }));
        }
        _ => dto.quote = None,
    }
}

fn clear_content(dto: &mut PostDto) {
    dto.text.clear();
    dto.facets.clear();
    dto.images.clear();
    dto.quote = None;
    dto.context = None;
}

fn placeholder(source: &PostDto, state: ContentState, marker: &str) -> PostDto {
    PostDto {
        uri: bounded(&source.uri),
        cid: bounded(&source.cid),
        author: AuthorDto {
            did: bounded(&source.author.did),
            handle: bounded(&source.author.handle),
            display_name: bounded(&source.author.display_name),
        },
        text: marker.into(),
        facets: Vec::new(),
        like_count: source.like_count,
        repost_count: source.repost_count,
        reply_count: source.reply_count,
        state,
        quote: None,
        images: Vec::new(),
        context: None,
    }
}

fn minimal_identity(post: &PostView) -> PostDto {
    let mut dto = PostDto {
        uri: "unavailable".into(),
        cid: "unavailable".into(),
        author: AuthorDto {
            did: "unavailable".into(),
            handle: "unavailable".into(),
            display_name: String::new(),
        },
        text: "post unavailable".into(),
        facets: Vec::new(),
        like_count: 0,
        repost_count: 0,
        reply_count: 0,
        state: ContentState::Unavailable,
        quote: None,
        images: Vec::new(),
        context: None,
    };
    if post.data.uri.len() <= MAX_FIELD_BYTES {
        dto.uri = post.data.uri.clone();
    }
    dto
}

fn bounded(value: &str) -> String {
    if value.len() <= MAX_FIELD_BYTES {
        value.to_owned()
    } else {
        "unavailable".into()
    }
}

fn is_warning(value: &str) -> bool {
    matches!(value, "porn" | "sexual" | "nudity" | "graphic-media")
}
fn author_state(
    post_labels: Option<&[atrium_api::com::atproto::label::defs::Label]>,
    viewer: Option<&atrium_api::app::bsky::actor::defs::ViewerStateData>,
    author_labels: Option<&[atrium_api::com::atproto::label::defs::Label]>,
) -> Result<ContentState, ()> {
    let Some(post_labels) = post_labels else {
        return Ok(ContentState::Unknown);
    };
    let Some(author_labels) = author_labels else {
        return Ok(ContentState::Unknown);
    };
    if let Some(viewer) = viewer {
        if viewer.blocked_by == Some(true)
            || viewer.blocking.is_some()
            || viewer.blocking_by_list.is_some()
        {
            return Ok(ContentState::Blocked);
        }
        if viewer.muted == Some(true) || viewer.muted_by_list.is_some() {
            return Ok(ContentState::Muted);
        }
    }
    if post_labels
        .iter()
        .chain(author_labels.iter())
        .any(|label| matches!(label.data.val.as_str(), "!hide" | "!no-unauthenticated"))
    {
        return Ok(ContentState::Blocked);
    }
    if post_labels
        .iter()
        .chain(author_labels.iter())
        .any(|label| is_warning(&label.data.val))
    {
        return Ok(ContentState::Warning("content warning".into()));
    }
    if post_labels
        .iter()
        .chain(author_labels.iter())
        .any(|label| !label.data.val.starts_with('!') && !label.data.val.starts_with("sys:"))
    {
        return Ok(ContentState::Warning("labeled content".into()));
    }
    Ok(ContentState::Clean)
}
fn post_image_views(
    embed: Option<&Union<PostViewEmbedRefs>>,
) -> Vec<&atrium_api::app::bsky::embed::images::ViewImage> {
    match embed {
        Some(Union::Refs(PostViewEmbedRefs::AppBskyEmbedImagesView(view))) => {
            view.data.images.iter().collect()
        }
        Some(Union::Refs(PostViewEmbedRefs::AppBskyEmbedRecordWithMediaView(view))) => {
            match &view.data.media {
                Union::Refs(
                    atrium_api::app::bsky::embed::record_with_media::ViewMediaRefs::AppBskyEmbedImagesView(
                        images,
                    ),
                ) => images.data.images.iter().collect(),
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}
fn image_metadata(image: &atrium_api::app::bsky::embed::images::ViewImage) -> ImageMetadata {
    ImageMetadata {
        alt: bounded(&image.data.alt),
        width: image
            .data
            .aspect_ratio
            .as_ref()
            .map(|a| a.data.width.get().min(u32::MAX as u64) as u32),
        height: image
            .data
            .aspect_ratio
            .as_ref()
            .map(|a| a.data.height.get().min(u32::MAX as u64) as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed_correlation_preflight;
    use crate::FeedSource;
    use atrium_api::app::bsky::actor::defs::ProfileViewBasicData;
    use atrium_api::app::bsky::feed::defs::PostViewData;
    use atrium_api::types::{
        string::{Datetime, Did, Handle},
        Unknown,
    };

    fn typed_post(
        labels: Option<Vec<atrium_api::com::atproto::label::defs::Label>>,
        viewer: Option<atrium_api::app::bsky::actor::defs::ViewerState>,
    ) -> PostView {
        let author = ProfileViewBasicData {
            associated: None,
            avatar: None,
            created_at: None,
            did: Did::new("did:plc:test".into()).unwrap(),
            display_name: Some("Test".into()),
            handle: Handle::new("test.example".into()).unwrap(),
            labels: Some(vec![]),
            pronouns: None,
            status: None,
            verification: None,
            viewer,
        };
        let record: Unknown = serde_json::from_value(
            serde_json::json!({"$type":"app.bsky.feed.post","text":"visible real post 🛰","createdAt":"2026-01-01T00:00:00Z"}),
        )
        .unwrap();
        PostViewData {
            author: author.into(),
            bookmark_count: None,
            cid: serde_json::from_str(
                "\"bafyreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"",
            )
            .unwrap(),
            embed: None,
            indexed_at: Datetime::now(),
            labels,
            like_count: Some(1),
            quote_count: None,
            record,
            reply_count: None,
            repost_count: None,
            threadgate: None,
            uri: "at://did:plc:test/app.bsky.feed.post/abc".into(),
            viewer: None,
        }
        .into()
    }
    fn correlation() -> crate::FeedCorrelation {
        crate::FeedCorrelation {
            schema: 1,
            auth_generation: 1,
            mount_generation: 1,
            request_id: 1,
            source: "following".into(),
            cursor: None,
        }
    }

    fn embedded_post(post: PostView, feed: serde_json::Value) -> FeedViewPost {
        let mut item = feed;
        item["post"] = serde_json::to_value(post).unwrap();
        serde_json::from_value::<atrium_api::app::bsky::feed::defs::FeedViewPostData>(item)
            .unwrap()
            .into()
    }

    fn avatar_sentinel_post(avatar: &str, media: &str, thumb: &str, fullsize: &str) -> PostView {
        let mut post = typed_post(Some(vec![]), None);
        post.data.author.data.avatar = Some(avatar.into());
        let mut value = serde_json::to_value(&post.data).unwrap();
        value["embed"] = serde_json::json!({
            "$type":"app.bsky.embed.recordWithMedia#view",
            "record":{"$type":"app.bsky.embed.record#view","record":{
                "$type":"app.bsky.embed.record#viewRecord",
                "uri":"at://did:plc:quoted/app.bsky.feed.post/quoted",
                "cid":"bafyreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "author":{"did":"did:plc:quoted","handle":"quoted.example","displayName":"Quoted","viewer":{},"labels":[]},
                "indexedAt":"2026-01-01T00:00:00Z",
                "labels":[],
                "value":{"$type":"app.bsky.feed.post","text":"QUOTE_BODY_SENTINEL","createdAt":"2026-01-01T00:00:00Z"}
            }},
            "media":{"$type":"app.bsky.embed.images#view","images":[
                {"thumb":thumb,"fullsize":fullsize,"alt":"first"},
                {"thumb":media,"fullsize":media,"alt":"second"}
            ]}
        });
        value["record"] =
            serde_json::json!({"text":"visible real post 🛰","createdAt":"2026-01-01T00:00:00Z"});
        post.data = serde_json::from_value(value).unwrap();
        post
    }

    fn guest_delivery_text(post: PostView) -> String {
        let item = embedded_post(post, serde_json::json!({}));
        let delivery = project_page(correlation(), vec![item], None).unwrap();
        serde_json::to_string(&delivery).unwrap()
    }
    #[test]
    fn typed_reply_and_repost_receive_fixed_context_markers() {
        let post = typed_post(Some(vec![]), None);
        for (feed, expected) in [
            (
                serde_json::json!({"reply":{"root":{"$type":"app.bsky.feed.defs#postView","author":{"did":"did:plc:root","handle":"root.example","labels":[],"viewer":{}},"cid":"bafyreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","indexedAt":"2026-01-01T00:00:00Z","labels":[],"record":{"text":"root","createdAt":"2026-01-01T00:00:00Z"},"uri":"at://did:plc:root/app.bsky.feed.post/root"},"parent":{"$type":"app.bsky.feed.defs#postView","author":{"did":"did:plc:parent","handle":"parent.example","labels":[],"viewer":{}},"cid":"bafyreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","indexedAt":"2026-01-01T00:00:00Z","labels":[],"record":{"text":"parent","createdAt":"2026-01-01T00:00:00Z"},"uri":"at://did:plc:parent/app.bsky.feed.post/parent"}}}),
                ContextMarker::Reply,
            ),
            (
                serde_json::json!({"reason":{"$type":"app.bsky.feed.defs#reasonRepost","by":{"did":"did:plc:other","handle":"other.example","labels":[],"viewer":{}},"indexedAt":"2026-01-01T00:00:00Z"}}),
                ContextMarker::Repost,
            ),
        ] {
            let mut feed_item = feed;
            feed_item["post"] = serde_json::to_value(&post).unwrap();
            let decoded = serde_json::from_value::<
                atrium_api::app::bsky::feed::defs::FeedViewPostData,
            >(feed_item)
            .unwrap();
            let delivery = project_page(correlation(), vec![decoded.into()], None).unwrap();
            let output = serde_json::to_value(delivery).unwrap();
            assert_eq!(
                output["posts"][0]["context"],
                serde_json::to_value(expected).unwrap()
            );
            assert!(output["posts"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("visible real post"));
        }
    }

    fn projected_avatar_relationship(viewer: Option<atrium_api::app::bsky::actor::defs::ViewerState>) -> (Relationship, bool) {
        let mut post = typed_post(Some(vec![]), viewer);
        let mut data = serde_json::to_value(&post.data).unwrap();
        data["author"]["avatar"] = serde_json::json!("https://example.com/avatar.png");
        post.data = serde_json::from_value(data).unwrap();
        let feed_item = serde_json::from_value::<atrium_api::app::bsky::feed::defs::FeedViewPostData>(
            serde_json::json!({"post": post}),
        )
        .unwrap();
        let projected = project_page_with_native(correlation(), vec![feed_item.into()], None, Some("did:plc:viewer".into())).unwrap();
        let descriptor = projected.native.descriptors.iter().find(|descriptor| matches!(descriptor.identity, AssetIdentity::Avatar { .. })).unwrap();
        (descriptor.relationship, descriptor.following_confirmed)
    }

    #[test]
    fn native_avatar_relationship_requires_confirmed_following() {
        let following = serde_json::from_value::<atrium_api::app::bsky::actor::defs::ViewerState>(
            serde_json::json!({"following":"at://did:plc:viewer/app.bsky.graph.follow/abc"}),
        ).unwrap();
        assert_eq!(projected_avatar_relationship(Some(following)), (Relationship::Followed, true));
        assert_eq!(projected_avatar_relationship(None), (Relationship::Unknown, false));

        let viewer_without_following = serde_json::from_value::<atrium_api::app::bsky::actor::defs::ViewerState>(
            serde_json::json!({"muted":false}),
        ).unwrap();
        assert_eq!(projected_avatar_relationship(Some(viewer_without_following)), (Relationship::Unknown, false));
    }

    #[test]
    fn record_with_media_projects_descriptors_and_dto_image_metadata_together() {
        let post = avatar_sentinel_post(
            "https://cdn.example/AVATAR_SENTINEL.png",
            "https://cdn.example/MEDIA_SENTINEL_1.png",
            "https://cdn.example/THUMB_ZERO_SENTINEL.png",
            "https://cdn.example/FULLSIZE_ZERO_SENTINEL.png",
        );
        let item = embedded_post(post, serde_json::json!({}));
        let projected = project_page_with_native(
            correlation(),
            vec![item],
            None,
            Some("did:plc:viewer".into()),
        )
        .unwrap();
        assert_eq!(projected.delivery.posts.len(), 1);
        let dto = &projected.delivery.posts[0];
        assert_eq!(dto.state, ContentState::Clean);
        assert_eq!(dto.images.len(), 2, "dto: {dto:?}");
        assert_eq!(dto.images[0].alt, "first");
        assert_eq!(dto.images[1].alt, "second");
        let mut indices = projected
            .native
            .descriptors
            .iter()
            .filter_map(|descriptor| match &descriptor.identity {
                AssetIdentity::PostImage { index, .. } => Some(*index),
                AssetIdentity::Avatar { .. } => None,
            })
            .collect::<Vec<_>>();
        indices.sort_unstable();
        assert_eq!(indices, vec![0, 1]);
        let mut revisions = projected
            .native
            .descriptors
            .iter()
            .filter_map(|descriptor| match &descriptor.identity {
                AssetIdentity::PostImage { index, thumbnail, fullsize, .. } => {
                    Some((*index, thumbnail.clone(), fullsize.clone()))
                }
                AssetIdentity::Avatar { .. } => None,
            })
            .collect::<Vec<_>>();
        revisions.sort_by_key(|(index, _, _)| *index);
        assert_eq!(
            revisions,
            vec![
                (
                    0,
                    "https://cdn.example/THUMB_ZERO_SENTINEL.png".into(),
                    "https://cdn.example/FULLSIZE_ZERO_SENTINEL.png".into(),
                ),
                (
                    1,
                    "https://cdn.example/MEDIA_SENTINEL_1.png".into(),
                    "https://cdn.example/MEDIA_SENTINEL_1.png".into(),
                ),
            ]
        );
        assert!(
            projected
                .native
                .descriptors
                .iter()
                .any(|descriptor| matches!(&descriptor.identity, AssetIdentity::Avatar { source_revision, .. } if source_revision == "https://cdn.example/AVATAR_SENTINEL.png"))
        );
        assert!(projected.validate_for("did:plc:viewer"));
    }

    #[test]
    fn hidden_or_blocked_posts_suppress_post_media_assets() {
        for (labels, viewer) in [
            (
                serde_json::json!([{"src":"did:plc:test","uri":"at://did:plc:test/app.bsky.feed.post/abc","val":"!hide","cts":"2026-01-01T00:00:00Z"}]),
                serde_json::json!({"muted":false}),
            ),
            (serde_json::json!([]), serde_json::json!({"blockedBy":true})),
            (serde_json::json!([]), serde_json::json!({"muted":true})),
        ] {
            let mut post = avatar_sentinel_post(
                "https://cdn.example/AVATAR_SENTINEL.png",
                "https://cdn.example/MEDIA_SENTINEL_1.png",
                "https://cdn.example/THUMB_ZERO_SENTINEL.png",
                "https://cdn.example/FULLSIZE_ZERO_SENTINEL.png",
            );
            let mut value = serde_json::to_value(&post.data).unwrap();
            value["labels"] = labels;
            value["author"]["viewer"] = viewer;
            post.data = serde_json::from_value(value).unwrap();
            let item = embedded_post(post, serde_json::json!({}));
            let projected = project_page_with_native(
                correlation(),
                vec![item],
                None,
                Some("did:plc:viewer".into()),
            )
            .unwrap();
            assert_ne!(projected.delivery.posts[0].state, ContentState::Clean);
            assert!(projected.delivery.posts[0].images.is_empty());
            assert!(
                projected.native.descriptors.is_empty(),
                "state {:?}",
                projected.delivery.posts[0].state
            );
            assert!(projected.validate_for("did:plc:viewer"));
        }
    }

    #[test]
    fn changed_avatar_source_url_produces_a_different_descriptor_identity() {
        let mut revisions = Vec::new();
        for avatar in [
            "https://cdn.example/AVATAR_SENTINEL_FIRST.png",
            "https://cdn.example/AVATAR_SENTINEL_SECOND.png",
        ] {
            let post = avatar_sentinel_post(
                avatar,
                "https://cdn.example/MEDIA_SENTINEL_1.png",
                "https://cdn.example/THUMB_ZERO_SENTINEL.png",
                "https://cdn.example/FULLSIZE_ZERO_SENTINEL.png",
            );
            let item = embedded_post(post, serde_json::json!({}));
            let projected = project_page_with_native(
                correlation(),
                vec![item],
                None,
                Some("did:plc:viewer".into()),
            )
            .unwrap();
            let identity = projected
                .native
                .descriptors
                .iter()
                .find_map(|descriptor| match &descriptor.identity {
                    AssetIdentity::Avatar { source_revision, author_did } => {
                        assert_eq!(author_did, "did:plc:test");
                        Some(source_revision.clone())
                    }
                    AssetIdentity::PostImage { .. } => None,
                })
                .unwrap();
            assert_eq!(identity, avatar);
            revisions.push(identity);
        }
        assert_ne!(revisions[0], revisions[1]);
    }

    #[test]
    fn serialized_guest_delivery_omits_native_asset_urls() {
        let post = avatar_sentinel_post(
            "https://cdn.example/AVATAR_SENTINEL.png",
            "https://cdn.example/MEDIA_SENTINEL_1.png",
            "https://cdn.example/THUMB_ZERO_SENTINEL.png",
            "https://cdn.example/FULLSIZE_ZERO_SENTINEL.png",
        );
        let serialized = guest_delivery_text(post);
        for sentinel in [
            "AVATAR_SENTINEL",
            "MEDIA_SENTINEL_1",
            "THUMB_ZERO_SENTINEL",
            "FULLSIZE_ZERO_SENTINEL",
        ] {
            assert!(
                !serialized.contains(sentinel),
                "{sentinel} leaked into guest delivery: {serialized}"
            );
        }
        assert!(serialized.contains("\"alt\":\"first\""));
        assert!(serialized.contains("\"alt\":\"second\""));
    }

    #[test]
    fn feed_preflight_accepts_matching_source_and_cursor() {
        let mut valid = correlation();
        valid.source = "following".into();
        assert!(feed_correlation_preflight(
            &valid,
            &FeedSource::Following,
            None,
            "did:plc:viewer",
            valid.request_id,
        ));
        valid.cursor = Some("cursor-1".into());
        assert!(feed_correlation_preflight(
            &valid,
            &FeedSource::Following,
            Some("cursor-1"),
            "did:plc:viewer",
            valid.request_id,
        ));
    }

    #[test]
    fn feed_preflight_rejects_wrong_source() {
        let mut wrong = correlation();
        wrong.source = "following".into();
        assert!(!feed_correlation_preflight(
            &wrong,
            &FeedSource::Feed("at://did:plc:x/app.bsky.feed.generator/f".into()),
            None,
            "did:plc:viewer",
            wrong.request_id,
        ));
    }

    #[test]
    fn feed_preflight_rejects_wrong_cursor() {
        let mut wrong = correlation();
        wrong.source = "following".into();
        wrong.cursor = Some("cursor-a".into());
        assert!(!feed_correlation_preflight(
            &wrong,
            &FeedSource::Following,
            Some("cursor-b"),
            "did:plc:viewer",
            wrong.request_id,
        ));
        wrong.cursor = None;
        assert!(!feed_correlation_preflight(
            &wrong,
            &FeedSource::Following,
            Some("cursor-b"),
            "did:plc:viewer",
            wrong.request_id,
        ));
    }

    #[test]
    fn feed_preflight_rejects_wrong_schema() {
        let mut wrong = correlation();
        wrong.source = "following".into();
        wrong.schema = SCHEMA_VERSION + 1;
        assert!(wrong.validate().is_err());
        assert!(!feed_correlation_preflight(
            &wrong,
            &FeedSource::Following,
            None,
            "did:plc:viewer",
            wrong.request_id,
        ));
    }

    #[test]
    fn feed_preflight_rejects_zero_auth_generation() {
        let mut wrong = correlation();
        wrong.source = "following".into();
        wrong.auth_generation = 0;
        assert!(!feed_correlation_preflight(
            &wrong,
            &FeedSource::Following,
            None,
            "did:plc:viewer",
            wrong.request_id,
        ));
    }

    #[test]
    fn feed_preflight_rejects_zero_request_id() {
        let mut wrong = correlation();
        wrong.source = "following".into();
        wrong.request_id = 0;
        assert!(!feed_correlation_preflight(
            &wrong,
            &FeedSource::Following,
            None,
            "did:plc:viewer",
            wrong.request_id,
        ));
    }

    #[test]
    fn feed_preflight_rejects_oversized_correlation_fields() {
        let mut oversized_source = correlation();
        oversized_source.source = "f".repeat(bsky_feed::MAX_FIELD_BYTES + 1);
        assert!(!feed_correlation_preflight(
            &oversized_source,
            &FeedSource::Following,
            None,
            "did:plc:viewer",
            oversized_source.request_id,
        ));
        let mut oversized_cursor = correlation();
        oversized_cursor.source = "following".into();
        oversized_cursor.cursor = Some("c".repeat(bsky_feed::MAX_FIELD_BYTES + 1));
        assert!(!feed_correlation_preflight(
            &oversized_cursor,
            &FeedSource::Following,
            oversized_cursor.cursor.as_deref(),
            "did:plc:viewer",
            oversized_cursor.request_id,
        ));
    }

    #[test]
    fn feed_preflight_rejects_absent_account_identity_when_required() {
        let mut valid = correlation();
        valid.source = "following".into();
        assert!(!feed_correlation_preflight(
            &valid,
            &FeedSource::Following,
            None,
            "",
            valid.request_id,
        ));
        assert!(feed_correlation_preflight(
            &valid,
            &FeedSource::Following,
            None,
            "",
            0,
        ));
    }

    #[test]
    fn clean_typed_post_is_delivered_and_missing_moderation_fails_closed() {
        let clean = typed_post(Some(vec![]), None);
        let delivery = project_page(
            correlation(),
            vec![FeedViewPost::from(
                serde_json::from_value::<atrium_api::app::bsky::feed::defs::FeedViewPostData>(
                    serde_json::json!({"post": clean}),
                )
                .unwrap(),
            )],
            None,
        )
        .unwrap();
        assert_eq!(delivery.posts[0].text, "visible real post 🛰");
        let unknown = typed_post(None, None);
        let post = project_post(&unknown, &ModerationPolicy::default());
        assert_eq!(post.state, ContentState::Unknown);
        assert!(post.text.is_empty());
        let absent_viewer_is_not_a_block = typed_post(Some(vec![]), None);
        assert_eq!(
            project_post(&absent_viewer_is_not_a_block, &ModerationPolicy::default()).state,
            ContentState::Clean
        );
    }
    fn project_serialized(post: PostView, feed: serde_json::Value) -> serde_json::Value {
        let mut item = feed;
        item["post"] = serde_json::to_value(post).unwrap();
        let item =
            serde_json::from_value::<atrium_api::app::bsky::feed::defs::FeedViewPostData>(item)
                .unwrap();
        let delivery = project_page(correlation(), vec![item.into()], None).unwrap();
        serde_json::to_value(delivery).unwrap()
    }

    fn quote_post(quote: serde_json::Value) -> PostView {
        let mut post = typed_post(Some(vec![]), None);
        let mut value = serde_json::to_value(&post.data).unwrap();
        value["embed"] = serde_json::json!({"$type":"app.bsky.embed.record#view","record":quote});
        value["record"] =
            serde_json::json!({"text":"visible real post 🛰","createdAt":"2026-01-01T00:00:00Z"});
        post.data = serde_json::from_value(value).unwrap();
        post
    }

    fn quote_fixture() -> serde_json::Value {
        let fixture = serde_json::json!({
            "$type":"app.bsky.embed.record#viewRecord",
            "uri":"at://did:plc:quoted/app.bsky.feed.post/quoted",
            "cid":"bafyreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "author":{"did":"did:plc:quoted","handle":"quoted.example","displayName":"Quoted","viewer":{},"labels":[]},
            "indexedAt":"2026-01-01T00:00:00Z",
            "labels":[],
            "value":{"$type":"app.bsky.feed.post","text":"QUOTE_BODY_SENTINEL","createdAt":"2026-01-01T00:00:00Z"},
            "embeds":[{"$type":"app.bsky.embed.images#view","images":[{"thumb":"https://example.com/QUOTE_ASSET_SENTINEL","fullsize":"https://example.com/QUOTE_ASSET_SENTINEL","alt":"asset"}]}]
        });
        let view_record: atrium_api::app::bsky::embed::record::ViewRecordData =
            serde_json::from_value(fixture.clone()).unwrap();
        let record = RecordData::try_from_unknown(view_record.value)
            .expect("fixture inner record must decode as app.bsky.feed.post");
        assert_eq!(record.text, "QUOTE_BODY_SENTINEL");
        fixture
    }

    #[test]
    fn hidden_outer_context_never_serializes_quote_body_or_assets() {
        for feed in [
            serde_json::json!({"reply":{"root":{},"parent":{}}}),
            serde_json::json!({"reason":{"$type":"app.bsky.feed.defs#reasonRepost","by":{"did":"did:plc:other","handle":"other.example","labels":[],"viewer":{"muted":true}},"indexedAt":"2026-01-01T00:00:00Z"}}),
        ] {
            let post = quote_post(quote_fixture());
            let output = project_serialized(post.clone(), serde_json::json!({})).to_string();
            assert!(
                output.contains("QUOTE_BODY_SENTINEL"),
                "clean quote must be present before hiding: {output}"
            );
            if feed.get("reply").is_some() {
                let mut feed = feed;
                feed["reply"] = serde_json::json!({"root":{"$type":"app.bsky.feed.defs#notFoundPost"},"parent":{"$type":"app.bsky.feed.defs#notFoundPost"}});
                let output = project_serialized(post, feed).to_string();
                assert!(!output.contains("QUOTE_BODY_SENTINEL"));
                assert!(!output.contains("QUOTE_ASSET_SENTINEL"));
            } else {
                let output = project_serialized(post, feed).to_string();
                assert!(!output.contains("QUOTE_BODY_SENTINEL"));
                assert!(!output.contains("QUOTE_ASSET_SENTINEL"));
            }
        }
    }

    #[test]
    fn quoted_record_moderation_is_applied_before_serialization() {
        for (labels, viewer) in [
            (
                serde_json::json!([{"src":"did:plc:quoted","uri":"at://did:plc:quoted/app.bsky.feed.post/quoted","val":"porn","cts":"2026-01-01T00:00:00Z"}]),
                serde_json::json!({}),
            ),
            (serde_json::json!([]), serde_json::json!({"blockedBy":true})),
            (serde_json::json!([]), serde_json::json!({"muted":true})),
        ] {
            let mut quote = quote_fixture();
            quote["author"]["viewer"] = viewer;
            quote["value"]["labels"] = serde_json::json!({"$type":"com.atproto.label.defs#selfLabels","values":[{"val":"porn"}]});
            quote["author"]["labels"] = labels;
            let output = project_serialized(quote_post(quote), serde_json::json!({})).to_string();
            assert!(!output.contains("QUOTE_BODY_SENTINEL"), "{output}");
            assert!(!output.contains("QUOTE_ASSET_SENTINEL"), "{output}");
        }
    }

    #[test]
    fn clean_quote_is_serialized_with_body_and_assets_absent_from_dto_asset_scope() {
        let post = quote_post(quote_fixture());
        let mut feed = serde_json::json!({});
        feed["post"] = serde_json::to_value(&post).unwrap();
        let decoded =
            serde_json::from_value::<atrium_api::app::bsky::feed::defs::FeedViewPostData>(feed)
                .unwrap();
        let mut dto = project_post(&post, &ModerationPolicy::default());
        apply_feed_context(&mut dto, &decoded, &ModerationPolicy::default());
        let output = serde_json::to_value(
            project_page(correlation(), vec![decoded.clone().into()], None).unwrap(),
        )
        .unwrap();
        assert!(
            dto.quote.is_some(),
            "decoded feed {:?}",
            decoded.post.data.embed
        );
        let quote = &output["posts"][0]["quote"];
        assert_eq!(quote["text"], "QUOTE_BODY_SENTINEL", "delivery: {output}");
        assert!(quote["images"].as_array().unwrap().is_empty());
        assert!(!output.to_string().contains("QUOTE_ASSET_SENTINEL"));
    }

    #[test]
    fn oversize_record_becomes_nonleaking_bounded_placeholder() {
        let mut post = typed_post(Some(vec![]), None);
        let mut value: serde_json::Value = serde_json::to_value(&post.data.record).unwrap();
        value["text"] = serde_json::Value::String("x".repeat(MAX_FIELD_BYTES + 1));
        post.data.record = serde_json::from_value(value).unwrap();
        let projected = project_post(&post, &ModerationPolicy::default());
        assert_eq!(projected.state, ContentState::Unavailable);
        assert!(projected.text.is_empty());
        assert!(projected.images.is_empty());
        let delivery = project_page(
            correlation(),
            vec![FeedViewPost::from(
                serde_json::from_value::<atrium_api::app::bsky::feed::defs::FeedViewPostData>(
                    serde_json::json!({"post": post}),
                )
                .unwrap(),
            )],
            None,
        )
        .unwrap();
        assert!(serde_json::to_vec(&delivery).unwrap().len() <= MAX_DELIVERY_BYTES);
    }

    #[test]
    fn native_projection_rejects_posts_exceeding_max_history() {
        let post = typed_post(Some(vec![]), None);
        let items: Vec<FeedViewPost> = (0..=bsky_feed::MAX_HISTORY)
            .map(|_| {
                embedded_post(post.clone(), serde_json::json!({}))
            })
            .collect();
        let result = project_page_with_native(
            correlation(),
            items,
            None,
            Some("did:plc:viewer".into()),
        );
        assert!(
            result.is_err(),
            "projection with {} posts should fail",
            bsky_feed::MAX_HISTORY + 1
        );
    }

    #[test]
    fn native_projection_allows_exact_max_history() {
        let post = typed_post(Some(vec![]), None);
        let items: Vec<FeedViewPost> = (0..bsky_feed::MAX_HISTORY)
            .map(|_| {
                embedded_post(post.clone(), serde_json::json!({}))
            })
            .collect();
        let result = project_page_with_native(
            correlation(),
            items,
            None,
            Some("did:plc:viewer".into()),
        );
        assert!(
            result.is_ok(),
            "projection with {} posts should succeed",
            bsky_feed::MAX_HISTORY
        );
    }

    #[test]
    fn avatar_revision_beyond_limit_is_rejected_at_admission() {
        let post = avatar_sentinel_post(
            &"x".repeat(MAX_REVISION_URL_BYTES + 1),
            "https://cdn.example/MEDIA.png",
            "https://cdn.example/THUMB.png",
            "https://cdn.example/FULL.png",
        );
        let item = embedded_post(post.clone(), serde_json::json!({}));
        let result = project_page_with_native(
            correlation(),
            vec![item],
            None,
            Some("did:plc:viewer".into()),
        );
        assert!(
            result.is_err(),
            "avatar revision exceeding {} bytes should be rejected",
            MAX_REVISION_URL_BYTES
        );
    }
}
