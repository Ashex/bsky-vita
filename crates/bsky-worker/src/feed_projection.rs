use atrium_api::app::bsky::feed::defs::{FeedViewPost, PostView, PostViewEmbedRefs};
use atrium_api::app::bsky::feed::post::RecordData;
use atrium_api::app::bsky::richtext::facet::MainFeaturesItem;
use atrium_api::types::{TryFromUnknown, Union};
use bsky_feed::{
    AuthorDto, ContentState, ContextMarker, DisplayDecision, FacetRange,
    FeedCorrelation as PocketCorrelation, FeedDelivery, ImageMetadata, ModerationPolicy, PostDto,
    MAX_DELIVERY_BYTES, MAX_FIELD_BYTES, SCHEMA_VERSION,
};

pub(crate) fn project_page(
    correlation: crate::FeedCorrelation,
    posts: Vec<FeedViewPost>,
    cursor: Option<String>,
) -> Result<FeedDelivery, String> {
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
    let mut projected = Vec::with_capacity(posts.len());
    for item in &posts {
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
    }
    Ok(delivery)
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
                dto.images = data.embed.as_ref().map(project_images).unwrap_or_default();
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
fn project_images(embed: &Union<PostViewEmbedRefs>) -> Vec<ImageMetadata> {
    match embed {
        Union::Refs(PostViewEmbedRefs::AppBskyEmbedImagesView(view)) => {
            view.data.images.iter().map(image_metadata).collect()
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
}
