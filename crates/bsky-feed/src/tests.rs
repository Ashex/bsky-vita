use super::*;
use crate::moderation::*;

fn correlation(mount: u64, request: u64) -> FeedCorrelation {
    FeedCorrelation {
        schema: SCHEMA_VERSION,
        auth_generation: 3,
        mount_generation: mount,
        request_id: request,
        source: "following".into(),
        cursor: None,
    }
}
fn post(uri: &str, state: ContentState) -> PostDto {
    PostDto {
        uri: uri.into(),
        cid: "cid".into(),
        author: AuthorDto {
            did: "did:plc:a".into(),
            handle: "a.test".into(),
            display_name: "A".into(),
        },
        text: "hello 🛰".into(),
        facets: vec![],
        like_count: 0,
        repost_count: 0,
        reply_count: 0,
        state,
        quote: None,
        images: vec![ImageMetadata {
            alt: "preview".into(),
            width: None,
            height: None,
        }],
        context: None,
    }
}

#[test]
fn context_marker_is_bounded_typed_and_legacy_missing_field_defaults() {
    let value = serde_json::json!({"state":"clean"});
    let old_post = serde_json::json!({"uri":"u","cid":"c","author":{"did":"d","handle":"h","display_name":""},"text":"","facets":[],"like_count":0,"repost_count":0,"reply_count":0,"state":value,"quote":null,"images":[]});
    let parsed: PostDto = serde_json::from_value(old_post).unwrap();
    assert_eq!(parsed.context, None);
    let marker = serde_json::to_value(ContextMarker::Unsupported).unwrap();
    assert_eq!(marker, "unsupported");
    assert!(serde_json::from_value::<ContextMarker>(serde_json::json!("arbitrary text")).is_err());
}

#[test]
fn content_state_wire_format_is_explicit_and_null_is_rejected() {
    let clean = serde_json::to_value(ContentState::Clean).unwrap();
    assert_eq!(clean, serde_json::json!({"state":"clean"}));
    assert_eq!(
        serde_json::from_value::<ContentState>(clean).unwrap(),
        ContentState::Clean
    );
    assert!(serde_json::from_value::<ContentState>(serde_json::Value::Null).is_err());
}

#[test]
fn correlation_captures_mount_and_request_identity() {
    let old = correlation(1, 10);
    let new = correlation(2, 11);
    assert_ne!(old, new);
    assert!(old.validate().is_ok());
    let mut coordinator = FeedCoordinator::default();
    coordinator.auth_changed(3);
    coordinator.open_mount();
    let first = match coordinator.command(FeedCommand::FirstPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    assert!(first.validate().is_ok());
    assert!(coordinator.active().is_some());
    assert_ne!(old, new);
}

#[test]
fn latest_refresh_is_coalesced_and_wrong_completion_ignored() {
    let refresh = correlation(1, 2);
    let latest = correlation(2, 3);
    let mut state = FeedCoordinator::default();
    state.auth_changed(3);
    state.open_mount();
    let active = match state.command(FeedCommand::FirstPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    assert!(state.command(FeedCommand::Refresh).is_empty());
    assert!(state.command(FeedCommand::Refresh).is_empty());
    assert!(state.complete(&refresh, Err("stale".into())).is_empty());
    assert_eq!(state.active(), Some(&active));
    assert_ne!(latest.request_id, refresh.request_id);
}

#[test]
fn persistent_coordinator_keeps_one_request_and_unique_identity_across_mounts() {
    let mut coordinator = FeedCoordinator::default();
    coordinator.auth_changed(3);
    let mount1 = coordinator.open_mount();
    let first = match coordinator.command(FeedCommand::FirstPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    coordinator.close_mount(mount1);
    let mount2 = coordinator.open_mount();
    assert!(coordinator.command(FeedCommand::FirstPage).is_empty());
    assert_eq!(coordinator.active(), Some(&first));
    let wrong = correlation(mount1, first.request_id + 1);
    assert!(coordinator
        .complete(&wrong, Err("duplicate".into()))
        .is_empty());
    assert_eq!(coordinator.active(), Some(&first));
    let delivery = FeedDelivery {
        schema: SCHEMA_VERSION,
        correlation: first.clone(),
        posts: vec![post("ignored", ContentState::Clean)],
        cursor: None,
    };
    assert!(coordinator
        .complete(&first, Ok(delivery))
        .iter()
        .all(|e| !matches!(e, FeedEffect::PublishDelivery(_))));
    assert_eq!(coordinator.history().posts().len(), 0);
    assert_ne!(mount1, mount2);
}

#[test]
fn accepted_history_revision_advances_for_first_page_and_pagination_only() {
    let mut coordinator = FeedCoordinator::default();
    coordinator.auth_changed(3);
    coordinator.open_mount();
    let accepted = match coordinator.command(FeedCommand::FirstPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    let delivery = FeedDelivery {
        schema: SCHEMA_VERSION,
        correlation: accepted.clone(),
        posts: vec![post("visible", ContentState::Clean)],
        cursor: Some("cursor".into()),
    };
    assert!(coordinator
        .complete(&accepted, Ok(delivery))
        .iter()
        .any(|effect| { matches!(effect, FeedEffect::PublishDelivery(_)) }));
    assert_eq!(coordinator.history_revision(), 1);
    let next = match coordinator.command(FeedCommand::NextPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    let delivery = FeedDelivery {
        schema: SCHEMA_VERSION,
        correlation: next.clone(),
        posts: vec![post("next", ContentState::Clean)],
        cursor: None,
    };
    coordinator.complete(&next, Ok(delivery));
    assert_eq!(coordinator.history_revision(), 2);
    assert!(coordinator
        .complete(&next, Err("duplicate".into()))
        .is_empty());
    assert_eq!(coordinator.history_revision(), 2);
}

#[test]
fn completion_dispatches_only_latest_queued_refresh_after_success_or_error() {
    for success in [true, false] {
        let mut coordinator = FeedCoordinator::default();
        coordinator.auth_changed(3);
        coordinator.open_mount();
        let active = match coordinator.command(FeedCommand::FirstPage).remove(0) {
            FeedEffect::FetchPage(c) => c,
            _ => unreachable!(),
        };
        coordinator.command(FeedCommand::Refresh);
        coordinator.command(FeedCommand::Refresh);
        let result = if success {
            Ok(FeedDelivery {
                schema: SCHEMA_VERSION,
                correlation: active.clone(),
                posts: vec![post("first", ContentState::Clean)],
                cursor: Some("next".into()),
            })
        } else {
            Err("temporary failure".into())
        };
        let effects = coordinator.complete(&active, result);
        let fetches: Vec<_> = effects
            .iter()
            .filter_map(|effect| match effect {
                FeedEffect::FetchPage(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(fetches.len(), 1);
        assert_eq!(fetches[0].cursor, None);
        assert_eq!(coordinator.active(), Some(fetches[0]));
        let current = fetches[0].clone();
        assert!(coordinator
            .complete(&active, Err("duplicate".into()))
            .is_empty());
        assert_eq!(coordinator.active(), Some(&current));
    }
}

#[test]
fn auth_replacement_releases_old_flight_and_accepts_new_account_immediately() {
    let mut coordinator = FeedCoordinator::default();
    coordinator.auth_changed(1);
    let mount_a = coordinator.open_mount();
    let old = match coordinator.command(FeedCommand::FirstPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    coordinator.auth_changed(2);
    let mount_b = coordinator.open_mount();
    let new = match coordinator.command(FeedCommand::FirstPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    assert_ne!(mount_a, mount_b);
    assert_eq!(new.auth_generation, 2);
    assert_eq!(coordinator.active(), Some(&new));
    assert!(coordinator
        .complete(&old, Err("old auth failure".into()))
        .is_empty());
    assert_eq!(coordinator.active(), Some(&new));
}

#[test]
fn failed_page_retries_same_operation_and_next_page_cursor_without_losing_history() {
    let mut coordinator = FeedCoordinator::default();
    coordinator.auth_changed(3);
    coordinator.open_mount();
    let first = match coordinator.command(FeedCommand::FirstPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    coordinator.complete(&first, Err("first failed".into()));
    let retry = match coordinator.command(FeedCommand::Retry).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    assert_eq!(retry.cursor, first.cursor);
    coordinator.complete(
        &retry,
        Ok(FeedDelivery {
            schema: SCHEMA_VERSION,
            correlation: retry.clone(),
            posts: vec![post("one", ContentState::Clean)],
            cursor: Some("cursor-1".into()),
        }),
    );
    let next = match coordinator.command(FeedCommand::NextPage).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    coordinator.complete(&next, Err("next failed".into()));
    let retry_next = match coordinator.command(FeedCommand::Retry).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    assert_eq!(retry_next.cursor.as_deref(), Some("cursor-1"));
    coordinator.complete(
        &retry_next,
        Ok(FeedDelivery {
            schema: SCHEMA_VERSION,
            correlation: retry_next.clone(),
            posts: vec![post("two", ContentState::Clean)],
            cursor: Some("cursor-2".into()),
        }),
    );
    assert_eq!(
        coordinator
            .history()
            .posts()
            .iter()
            .map(|p| p.uri.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
}

#[test]
fn failed_refresh_retry_repeats_first_page_and_auth_or_mount_clears_retry() {
    let mut coordinator = FeedCoordinator::default();
    coordinator.auth_changed(3);
    let mount = coordinator.open_mount();
    let refresh = match coordinator.command(FeedCommand::Refresh).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    coordinator.complete(&refresh, Err("refresh failed".into()));
    let retry = match coordinator.command(FeedCommand::Retry).remove(0) {
        FeedEffect::FetchPage(c) => c,
        _ => unreachable!(),
    };
    assert_eq!(retry.cursor, None);
    coordinator.complete(&retry, Err("again".into()));
    coordinator.close_mount(mount);
    assert!(coordinator.command(FeedCommand::Retry).is_empty());
    coordinator.auth_changed(4);
    coordinator.open_mount();
    assert!(coordinator.command(FeedCommand::Retry).is_empty());
}

#[test]
fn feed_history_deduplicates_and_trims_to_cap() {
    let mut history = FeedHistory::default();
    assert_eq!(
        history
            .append([
                post("x", ContentState::Clean),
                post("x", ContentState::Clean)
            ])
            .unwrap(),
        1
    );
    let rows = (0..MAX_HISTORY + 2).map(|n| post(&n.to_string(), ContentState::Clean));
    history.append(rows).unwrap();
    assert_eq!(history.posts().len(), MAX_HISTORY);
}

#[test]
fn facet_ranges_use_utf8_bytes_and_command_whitelist_is_closed() {
    let text = "a🛰b";
    assert!(FacetRange {
        start: 1,
        end: 5,
        feature: "link".into()
    }
    .validate(text));
    assert!(!FacetRange {
        start: 2,
        end: 5,
        feature: "link".into()
    }
    .validate(text));
    assert!(FeedCommand::parse("anything").is_err());
    assert_eq!(FeedCommand::parse("refresh"), Ok(FeedCommand::Refresh));
    assert_eq!(FeedCommand::parse("next_page"), Ok(FeedCommand::NextPage));
    assert_eq!(FeedCommand::parse("retry"), Ok(FeedCommand::Retry));
    assert_eq!(FeedCommand::parse("first_page"), Ok(FeedCommand::FirstPage));
    assert_eq!(FeedCommand::parse("leave"), Ok(FeedCommand::Leave));
    assert!(FeedCommand::parse("fetch arbitrary").is_err());
}

#[test]
fn moderation_hides_content_and_prevents_asset_admission() {
    let policy = ModerationPolicy::default();
    let hidden = post("hidden", ContentState::Warning("sensitive".into()));
    assert!(matches!(
        policy.decide(&hidden),
        DisplayDecision::Placeholder(_)
    ));
    assert!(policy.visible_image_metadata(&hidden).is_empty());
    let clean = post("clean", ContentState::Clean);
    assert_eq!(policy.visible_image_metadata(&clean).len(), 1);
    let mut blocked = ModerationPolicy::default();
    blocked.blocked.push("did:plc:a".into());
    assert!(blocked.visible_image_metadata(&clean).is_empty());
}

#[test]
fn field_and_delivery_limits_are_explicit() {
    let mut item = post("x", ContentState::Clean);
    item.text = "x".repeat(MAX_FIELD_BYTES + 1);
    assert!(item.validate().is_err());
    assert!(FeedHistory::delivery_size(&[post("clean", ContentState::Clean)]).is_some());
}

#[test]
fn history_uses_serialized_json_size_across_pages_and_trims_before_overshoot() {
    let mut history = FeedHistory::default();
    let mut rows = Vec::new();
    for n in 0..150 {
        let mut item = post(&n.to_string(), ContentState::Clean);
        item.text = "\"\\\n🛰".repeat(2_500);
        rows.push(item);
    }
    let mut rows = rows;
    let second_page = rows.split_off(100);
    history.append(rows).unwrap();
    assert!(history.posts().len() <= MAX_HISTORY);
    history.append(second_page).unwrap();

    assert!(
        FeedHistory::delivery_size(&history.posts().iter().cloned().collect::<Vec<_>>()).unwrap()
            <= MAX_DELIVERY_BYTES
    );
}

#[test]
fn full_envelope_cap_accounts_for_escaped_text_cursor_and_two_pages() {
    let c = FeedCorrelation {
        schema: SCHEMA_VERSION,
        auth_generation: 3,
        mount_generation: 7,
        request_id: 11,
        source: "following/\"escaped\"".into(),
        cursor: None,
    };
    let long_cursor = "\\\"".repeat(20_000);
    let mut history = FeedHistory::default();
    let make_rows = |start: u64| {
        (start..start + 12).map(|n| {
            let mut row = post(&n.to_string(), ContentState::Clean);
            row.text = "\\\"\n🛰".repeat(5_000);
            row
        })
    };
    history
        .append_with_envelope(make_rows(0), &c, Some(&long_cursor))
        .unwrap();
    history
        .append_with_envelope(make_rows(12), &c, Some(&long_cursor))
        .unwrap();
    let rows: Vec<_> = history.posts().iter().cloned().collect();
    let actual = serde_json::to_vec(&FeedDelivery {
        schema: SCHEMA_VERSION,
        correlation: c.clone(),
        posts: rows.clone(),
        cursor: Some(long_cursor.clone()),
    })
    .unwrap();
    assert!(actual.len() <= MAX_DELIVERY_BYTES, "{} bytes", actual.len());
    assert!(rows.len() <= MAX_HISTORY);
    assert_eq!(rows.last().unwrap().uri, "23");
    assert!(rows.iter().all(|row| row.text == "\\\"\n🛰".repeat(5_000)));

    let actual_size = FeedHistory::delivery_size_for(&rows, &c, Some(&long_cursor)).unwrap();
    assert_eq!(actual.len(), actual_size);
    assert!(actual_size <= MAX_DELIVERY_BYTES);

    let mut too_long_cursor = long_cursor.clone();
    too_long_cursor.push_str(&"\\\"".repeat(MAX_DELIVERY_BYTES));
    let mut bounded = FeedHistory::default();
    assert!(bounded
        .append_with_envelope(
            [post("oversize", ContentState::Clean)],
            &c,
            Some(&too_long_cursor),
        )
        .is_err());
    assert!(bounded.posts().is_empty());
}

#[test]
fn history_caps_count_before_accepting_more_than_two_hundred_rows() {
    let mut history = FeedHistory::default();
    history
        .append((0..MAX_HISTORY).map(|n| post(&n.to_string(), ContentState::Clean)))
        .unwrap();
    history
        .append([post("latest", ContentState::Clean)])
        .unwrap();
    assert_eq!(history.posts().len(), MAX_HISTORY);
    assert_eq!(history.posts().back().unwrap().uri, "latest");
}
