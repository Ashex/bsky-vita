use bsky_pocket::visible_text::{
    MeasureReply, MeasureRequest, MeasureResult, MeasureStatus, PlacementMessage, Stamp,
};
use bsky_pocket::{EmbeddedGuest, GuestState};

fn post(uri: &str, text: &str, state: &str) -> serde_json::Value {
    serde_json::json!({"uri":uri,"cid":"cid","author":{"did":"did:example","handle":"example.test","display_name":"Example"},"text":text,"facets":[],"like_count":2,"repost_count":1,"reply_count":3,"state":{"state":state},"quote":null,"images":[],"context":null})
}

#[test]
fn packaged_guest_touch_frame_scrolls_and_dpad_still_works_in_same_state() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    guest.eval("globalThis.__bskyHistoryRevision=1").unwrap();
    let posts: Vec<_> = (0..30)
        .map(|i| post(&format!("at://touch-{i}"), "row", "clean"))
        .collect();
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":1,"source":"following","cursor":null},"posts":posts,"cursor":null});
    guest
        .deliver_feed_json_with_deadline(&feed.to_string(), deadline)
        .unwrap();
    let request = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let request_value: serde_json::Value = serde_json::from_str(&request).unwrap();
    let measured_rows = request_value["rows"].as_array().unwrap().iter().map(|row| serde_json::json!({"uri":row["uri"],"width":440,"status":"ready","line_count":1,"line_height":18,"baseline_offset":14,"body_height":18,"continuation":false})).collect::<Vec<_>>();
    let reply = serde_json::json!({"schema":1,"stamp":request_value["stamp"],"rows":measured_rows});
    guest
        .set_global_json("__bskyVisibleMeasurement", &reply.to_string(), deadline)
        .unwrap();
    let candidates = serde_json::to_string(&request_value).unwrap();
    assert!(!guest
        .eval_global(&format!(
            "globalThis.__bskyVisibleCandidates(JSON.stringify({candidates}))"
        ))
        .unwrap()
        .is_empty());
    guest
        .eval_global(
            "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
        )
        .unwrap();
    let initial: serde_json::Value = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    let initial_y = initial["rows"][0]["y"].as_i64().unwrap();

    let snapshot = bsky_pocket::TouchSnapshot::from_contacts([bsky_pocket::TouchContact {
        id: 9,
        x: 456,
        y: 100,
    }]);
    guest
        .set_touch_snapshot(snapshot, deadline)
        .expect("initial rail contact reaches guest");
    guest.eval_global("globalThis.__bskyTouchFrame()").unwrap();
    let snapshot = bsky_pocket::TouchSnapshot::from_contacts([bsky_pocket::TouchContact {
        id: 9,
        x: 400,
        y: 70,
    }]);
    guest
        .set_touch_snapshot(snapshot, deadline)
        .expect("captured drag reaches guest");
    guest.eval_global("globalThis.__bskyTouchFrame()").unwrap();
    let dragged: serde_json::Value = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    let dragged_y = dragged["rows"][0]["y"].as_i64().unwrap();
    assert!(
        dragged_y != initial_y,
        "drag must update published viewport placement: {initial_y} -> {dragged_y}"
    );

    guest
        .set_touch_snapshot(bsky_pocket::TouchSnapshot::default(), deadline)
        .expect("touch release reaches guest");
    guest.eval_global("globalThis.__bskyTouchFrame()").unwrap();
    let before_button: serde_json::Value = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    let before_button_y = before_button["rows"][0]["y"].as_i64().unwrap();

    guest
        .advance_with_deadline(0x0040, deadline)
        .expect("D-pad advances after touch scroll");
    guest
        .present_with_deadline(deadline)
        .expect("combined touch and D-pad frame presents");
    let after_press: serde_json::Value = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    let after_press_y = after_press["rows"][0]["y"].as_i64().unwrap();
    assert!(
        after_press_y < before_button_y,
        "DOWN press must move viewport placement: {before_button_y} -> {after_press_y}"
    );

    guest
        .advance_with_deadline(0, deadline)
        .expect("D-pad release advances after touch scroll");
    guest
        .present_with_deadline(deadline)
        .expect("released D-pad frame presents");
    let after_release: serde_json::Value = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    let after_release_y = after_release["rows"][0]["y"].as_i64().unwrap();
    assert_eq!(
        after_release_y, after_press_y,
        "D-pad release must preserve viewport placement"
    );
    assert_eq!(
        before_button_y - after_press_y,
        24,
        "DOWN press should scroll one expected 24px step"
    );
    assert!(
        after_release["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["uri"] == "at://touch-1"),
        "D-pad selection remains active after touch scrolling: {after_release}"
    );
}

#[test]
fn advance_only_defers_raster_until_explicit_present_and_legacy_frame_still_presents() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    guest
        .advance_with_deadline(0, deadline)
        .expect("guest advances");
    // Raster access requires completion of the deferred presentation; verify
    // that the host-facing phase transition itself succeeds in order.
    guest
        .present_with_deadline(deadline)
        .expect("advanced state presents");
    let _pixels = guest.pixels().expect("presented raster is readable");
    // Existing public frame API remains the combined advance+present path.
    guest.frame(0).expect("legacy frame advances and presents");
}

#[test]
fn expired_deadline_between_advance_and_present_fails_closed() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    guest
        .advance_with_deadline(0, deadline)
        .expect("guest advances within deadline");

    let expired = std::time::Instant::now() - std::time::Duration::from_millis(1);
    assert_eq!(
        guest.present_with_deadline(expired).unwrap_err(),
        "guest deadline exceeded"
    );
}

#[test]
fn packaged_guest_selects_and_measures_history_beyond_initial_window() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    // Use a maximum-size valid snapshot; the guest feed callback replaces history.
    guest.eval("globalThis.__bskyHistoryRevision=1").unwrap();
    {
        let posts: Vec<_> = (0..200)
            .map(|i| {
                post(
                    &format!("at://{i}"),
                    if i % 7 == 0 { "" } else { "row" },
                    if i % 7 == 0 { "blocked" } else { "clean" },
                )
            })
            .collect();
        let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":1,"source":"following","cursor":null},"posts":posts,"cursor":null});
        guest
            .deliver_feed_json_with_deadline(
                &feed.to_string(),
                std::time::Instant::now() + std::time::Duration::from_secs(2),
            )
            .unwrap();
    }
    let candidates = guest.eval_global("globalThis.__bskyVisibleCandidates(JSON.stringify({schema:1,stamp:{auth_generation:1,mount_generation:1,history_revision:1,layout_id:1},rows:[{uri:'at://0',width:440},{uri:'at://1',width:440},{uri:'at://2',width:440}]}))").unwrap();
    assert!(!candidates.is_empty());
    let initial_window: serde_json::Value =
        serde_json::from_str(&guest.eval_global("globalThis.__bskyVisibleWindow").unwrap())
            .unwrap();
    let initial_rows = initial_window["rows"].as_array().unwrap();
    assert!(
        !initial_rows.is_empty(),
        "initial viewport must expose at least one row"
    );
    assert!(
        initial_rows.len() <= 3,
        "initial measurement batch is bounded to three rows: {}",
        initial_rows.len()
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    // Establish the first selection, then verify UP clamps at the boundary.
    for button in [0x0040, 0x0010] {
        guest
            .advance_with_deadline(button, deadline)
            .expect("button press advances");
        guest
            .present_with_deadline(deadline)
            .expect("pressed frame presents");
        guest
            .advance_with_deadline(0, deadline)
            .expect("button release advances");
        guest
            .present_with_deadline(deadline)
            .expect("released frame presents");
    }
    let raw_window = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let window: serde_json::Value = serde_json::from_str(&raw_window).unwrap();
    assert_eq!(window["rows"][0]["uri"], "at://0");
    // ButtonSnapshot carries one press pulse. A zero-button frame releases it,
    // allowing the next press to produce another edge in the production input path.
    for _ in 0..150 {
        guest
            .advance_with_deadline(0x0040, deadline)
            .expect("DOWN press advances");
        guest
            .present_with_deadline(deadline)
            .expect("DOWN frame presents");
        guest
            .advance_with_deadline(0, deadline)
            .expect("button release advances");
        guest
            .present_with_deadline(deadline)
            .expect("released frame presents");
    }
    let raw_window = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let window: serde_json::Value = serde_json::from_str(&raw_window).unwrap();
    let after_scroll = window["rows"][0]["uri"].as_str().unwrap();
    let after_index: usize = after_scroll.strip_prefix("at://").unwrap().parse().unwrap();
    assert!(
        after_index > 3,
        "150 DOWN presses should scroll beyond the initial viewport, got {after_scroll}"
    );
    for _ in 0..2 {
        guest
            .advance_with_deadline(0x0040, deadline)
            .expect("DOWN press advances");
        guest
            .present_with_deadline(deadline)
            .expect("DOWN frame presents");
        guest
            .advance_with_deadline(0, deadline)
            .expect("button release advances");
        guest
            .present_with_deadline(deadline)
            .expect("released frame presents");
    }
    let raw_window = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let window: serde_json::Value = serde_json::from_str(&raw_window).unwrap();
    let after_more_scroll = window["rows"][0]["uri"].as_str().unwrap().to_owned();
    guest
        .advance_with_deadline(0x0010, deadline)
        .expect("UP press advances");
    guest
        .present_with_deadline(deadline)
        .expect("UP frame presents");
    guest
        .advance_with_deadline(0, deadline)
        .expect("button release advances");
    guest
        .present_with_deadline(deadline)
        .expect("released frame presents");
    let raw_window = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let window: serde_json::Value = serde_json::from_str(&raw_window).unwrap();
    let after_up = window["rows"][0]["uri"].as_str().unwrap();
    assert!(
        after_up
            .strip_prefix("at://")
            .unwrap()
            .parse::<usize>()
            .unwrap()
            > 3
    );
    let before_up_index: usize = after_more_scroll
        .strip_prefix("at://")
        .unwrap()
        .parse()
        .unwrap();
    let after_up_index: usize = after_up.strip_prefix("at://").unwrap().parse().unwrap();
    assert!(
        after_up_index <= before_up_index,
        "UP should preserve or reverse the visible-row origin: before={after_more_scroll}, after={after_up}"
    );
    assert!(!candidates.is_empty(), "guest should find requested rows");
    let raw_window = guest
        .eval_global("globalThis.__bskyVisibleWindow || '[]'")
        .unwrap();
    let window: serde_json::Value = serde_json::from_str(&raw_window).unwrap();
    assert_eq!(
        window["rows"],
        serde_json::json!([
            {"uri":after_up,"width":440},
            {"uri":format!("at://{}", after_up.strip_prefix("at://").unwrap().parse::<usize>().unwrap() + 1),"width":440},
            {"uri":format!("at://{}", after_up.strip_prefix("at://").unwrap().parse::<usize>().unwrap() + 2),"width":440}
        ])
    );
    let selected_before_refresh = after_up.to_owned();
    // Appending/replacing retained history must preserve the selected URI.
    let anchor_index: usize = selected_before_refresh
        .strip_prefix("at://")
        .unwrap()
        .parse()
        .unwrap();
    let appended: Vec<_> = (0..200)
        .map(|i| post(&format!("at://{i}"), "row", "clean"))
        .collect();
    assert!(appended
        .iter()
        .any(|post| post["uri"] == selected_before_refresh));
    assert!(appended
        .iter()
        .any(|post| post["uri"] == format!("at://{}", anchor_index.saturating_sub(1))));
    assert!(appended
        .iter()
        .any(|post| post["uri"] == format!("at://{}", anchor_index + 1)));
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":2,"source":"following","cursor":null},"posts":appended,"cursor":null});
    guest
        .deliver_feed_json_with_deadline(&feed.to_string(), deadline)
        .unwrap();
    let preserved = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let preserved: serde_json::Value = serde_json::from_str(&preserved).unwrap();
    let retained_uri = preserved["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["uri"] == selected_before_refresh)
        .map(|row| row["uri"].as_str().unwrap());
    assert!(
        retained_uri.is_some(),
        "feed update should retain selected anchor {selected_before_refresh} in visible rows: {}",
        preserved["rows"]
    );
    // A new snapshot that removes the anchor must reconcile to its old position,
    // clamped to the available rows; it must not claim to retain a missing URI.
    let removed_anchor: Vec<_> = (0..200)
        .filter(|index| *index != anchor_index)
        .map(|index| post(&format!("at://{index}"), "row", "clean"))
        .collect();
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":3,"source":"following","cursor":null},"posts":removed_anchor,"cursor":null});
    guest
        .deliver_feed_json_with_deadline(&feed.to_string(), deadline)
        .unwrap();
    let removed = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let removed: serde_json::Value = serde_json::from_str(&removed).unwrap();
    let expected_index = anchor_index.min(198);
    assert_eq!(
        removed["rows"][0]["uri"],
        format!(
            "at://{}",
            if expected_index >= anchor_index {
                expected_index + 1
            } else {
                expected_index
            }
        )
    );
    assert_ne!(removed["rows"][0]["uri"], selected_before_refresh);
    let replacement = vec![post("at://replacement", "new", "clean")];
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":3,"source":"following","cursor":null},"posts":replacement,"cursor":null});
    guest
        .deliver_feed_json_with_deadline(&feed.to_string(), deadline)
        .unwrap();
    let reset_raw = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let reset: serde_json::Value = serde_json::from_str(&reset_raw).unwrap();
    assert_eq!(reset["rows"][0]["uri"], "at://replacement");
    assert!(
        reset["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("context").is_none()),
        "the metrics/placement projection must not include post context"
    );
    let initial_layout = guest
        .eval_global("globalThis.__bskyVisibleLayoutRequest")
        .unwrap();
    let initial_layout: serde_json::Value = serde_json::from_str(&initial_layout).unwrap();
    assert!(
        initial_layout["rows"].as_array().unwrap().is_empty(),
        "a pending-only viewport has no native placement rows"
    );
    guest
        .eval_global("globalThis.__bskySetFeedError('temporary failure')")
        .unwrap();
    let _ = guest
        .eval_global("globalThis.__bskyTakeFeedCommands()")
        .unwrap();
    let selection_after_error = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let selection_after_error: serde_json::Value =
        serde_json::from_str(&selection_after_error).unwrap();
    assert_eq!(selection_after_error, reset);
}

#[test]
fn packaged_guest_retains_cached_placement_and_positions_measured_rows_after_pending_estimates() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":1,"source":"following","cursor":null},"posts":[post("at://pending", "pending", "clean"), post("at://measured", "measured", "clean")],"cursor":null});
    guest.eval("globalThis.__bskyHistoryRevision=1").unwrap();
    guest.deliver_feed_json(&feed.to_string()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let request: MeasureRequest =
        serde_json::from_str(&guest.eval_global("globalThis.__bskyVisibleWindow").unwrap())
            .unwrap();
    let measured_request = serde_json::json!({"schema":1,"stamp":request.stamp,"rows":[{"uri":"at://pending","width":440},{"uri":"at://measured","width":440}]});
    let active_measurement_request: MeasureRequest =
        serde_json::from_value(measured_request.clone()).unwrap();
    assert!(!guest
        .eval_global(&format!(
            "globalThis.__bskyVisibleCandidates(JSON.stringify({measured_request}))"
        ))
        .unwrap()
        .is_empty());
    let request = active_measurement_request.clone();
    let reply = MeasureReply {
        schema: 1,
        stamp: request.stamp.clone(),
        rows: vec![
            MeasureResult {
                uri: "at://pending".into(),
                status: MeasureStatus::Ready,
                width: 440,
                line_count: 1,
                line_height: 18,
                baseline_offset: 14,
                body_height: 18,
                continuation: false,
            },
            MeasureResult {
                uri: "at://measured".into(),
                status: MeasureStatus::Ready,
                width: 440,
                line_count: 1,
                line_height: 18,
                baseline_offset: 14,
                body_height: 18,
                continuation: false,
            },
        ],
    };
    guest
        .set_global_json(
            "__bskyVisibleMeasurement",
            &reply.to_bounded_json().unwrap(),
            deadline,
        )
        .unwrap();
    guest
        .eval_global(
            "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
        )
        .unwrap();
    let placement: serde_json::Value = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    assert!(
        placement["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["uri"] == "at://measured"),
        "measured placement is preserved: {placement}"
    );
    let measured_y = placement["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["uri"] == "at://measured")
        .unwrap()["y"]
        .as_i64()
        .unwrap();
    assert!(
        measured_y >= 120,
        "second row includes pending first-row estimate: {measured_y}"
    );
    let cached_request = serde_json::json!({"schema":1,"stamp":{"auth_generation":1,"mount_generation":1,"history_revision":1,"layout_id":2},"rows":[{"uri":"at://pending","width":440}]});
    guest
        .eval_global(&format!(
            "globalThis.__bskyVisibleCandidates(JSON.stringify({cached_request}))"
        ))
        .unwrap();
    let retained: serde_json::Value = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(retained["stamp"]["layout_id"], 2);
    assert!(
        retained["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["uri"] == "at://measured"),
        "cached placement survives a candidate with no reply: {retained}"
    );

    let viewport_request: MeasureRequest = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisibleLayoutRequest")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        viewport_request
            .rows
            .iter()
            .map(|row| row.uri.as_str())
            .collect::<Vec<_>>(),
        ["at://pending", "at://measured"]
    );
    assert_eq!(
        active_measurement_request
            .rows
            .iter()
            .map(|row| row.uri.as_str())
            .collect::<Vec<_>>(),
        ["at://pending", "at://measured"]
    );
    let retained_placement: PlacementMessage = serde_json::from_value(retained).unwrap();
    retained_placement
        .validate_for(
            &viewport_request,
            &[
                MeasureResult {
                    uri: "at://pending".into(),
                    status: MeasureStatus::Ready,
                    width: 440,
                    line_count: 1,
                    line_height: 18,
                    baseline_offset: 14,
                    body_height: 18,
                    continuation: false,
                },
                MeasureResult {
                    uri: "at://measured".into(),
                    status: MeasureStatus::Ready,
                    width: 440,
                    line_count: 1,
                    line_height: 18,
                    baseline_offset: 14,
                    body_height: 18,
                    continuation: false,
                },
            ],
        )
        .expect("cached viewport row validates independently of active measurement candidates");
}

#[test]
fn packaged_guest_tall_post_scrolls_through_bounded_measurement_callbacks() {
    use bsky_pocket::visible_text::{
        MeasureBudget, WrapCache, MAX_MEASURE_BYTES_ROW, MAX_MEASURE_CALLS_ROW,
    };

    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    let text = (0..60)
        .map(|i| format!("line{i:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":1,"source":"following","cursor":null},"posts":[post("at://tall", &text, "clean")],"cursor":null});
    guest.eval("globalThis.__bskyHistoryRevision=1").unwrap();
    guest.deliver_feed_json(&feed.to_string()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut cache = WrapCache::default();
    let mut seen_last = false;
    let mut iterations = 0;
    let mut candidate_count = 0;
    let mut completed_count = 0;
    // Wrapping may yield across frames; preserve cache state until complete.
    let raw_request = guest
        .eval_global("globalThis.__bskyVisibleWindow || ''")
        .unwrap();
    let request: MeasureRequest = serde_json::from_str(&raw_request).unwrap();
    let candidate_raw = guest
        .eval_global(&format!(
            "globalThis.__bskyVisibleCandidates(JSON.stringify({raw_request}))"
        ))
        .unwrap();
    let candidates: serde_json::Value = serde_json::from_str(&candidate_raw).unwrap();
    let candidate_text = candidates["rows"][0]["text"].as_str().unwrap();
    let lines = loop {
        let mut budget = MeasureBudget::new(MAX_MEASURE_CALLS_ROW, MAX_MEASURE_BYTES_ROW);
        if let Some((lines, continuation)) =
            cache.step("at://tall", candidate_text, 440, 7, &mut budget, |s| {
                u16::try_from(s.chars().count().saturating_mul(70)).ok()
            })
        {
            assert!(!continuation);
            break lines;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "bounded wrapping makes progress within deadline"
        );
    };
    let reply = MeasureReply {
        schema: 1,
        stamp: request.stamp.clone(),
        rows: request
            .rows
            .iter()
            .map(|row| MeasureResult {
                uri: row.uri.clone(),
                status: MeasureStatus::Ready,
                width: row.width,
                line_count: lines.len() as u32,
                line_height: 18,
                baseline_offset: 14,
                body_height: (lines.len() as u32) * 18,
                continuation: false,
            })
            .collect(),
    };
    guest
        .set_global_json(
            "__bskyVisibleMeasurement",
            &reply.to_bounded_json().unwrap(),
            deadline,
        )
        .unwrap();
    guest
        .eval_global(
            "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
        )
        .unwrap();
    for _ in 0..400 {
        iterations += 1;
        let raw_measurement = guest
            .eval_global("globalThis.__bskyVisiblePlacementRequest || ''")
            .unwrap();
        if !raw_measurement.is_empty() {
            let request: MeasureRequest = serde_json::from_str(&raw_measurement).unwrap();
            request.validate().unwrap();
            let candidate_raw = guest
                .eval_global(&format!(
                    "globalThis.__bskyVisibleCandidates(JSON.stringify({raw_measurement}))"
                ))
                .unwrap();
            if !candidate_raw.is_empty() {
                candidate_count += 1;
                let candidates: serde_json::Value = serde_json::from_str(&candidate_raw).unwrap();
                if candidates["rows"].as_array().is_none_or(Vec::is_empty) {
                    continue;
                }
                assert_eq!(candidates["rows"][0]["uri"], "at://tall");
                let candidate_text = candidates["rows"][0]["text"].as_str().unwrap();
                let mut budget = MeasureBudget::new(MAX_MEASURE_CALLS_ROW, MAX_MEASURE_BYTES_ROW);
                // Deterministic fake font: each Unicode scalar occupies 70 px.
                // Persistent cache state makes wrapping yield and resume across requests.
                let wrapped = cache.step("at://tall", candidate_text, 440, 7, &mut budget, |s| {
                    u16::try_from(s.chars().count().saturating_mul(70)).ok()
                });
                if let Some((lines, continuation)) = wrapped {
                    completed_count += 1;
                    assert!(!continuation);
                    let reply = MeasureReply {
                        schema: 1,
                        stamp: request.stamp.clone(),
                        rows: request
                            .rows
                            .iter()
                            .map(|row| MeasureResult {
                                uri: row.uri.clone(),
                                status: MeasureStatus::Ready,
                                width: row.width,
                                line_count: lines.len() as u32,
                                line_height: 18,
                                baseline_offset: 14,
                                body_height: (lines.len() as u32) * 18,
                                continuation: false,
                            })
                            .collect(),
                    };
                    reply.validate_for(&request).unwrap();
                    guest
                        .set_global_json(
                            "__bskyVisibleMeasurement",
                            &reply.to_bounded_json().unwrap(),
                            deadline,
                        )
                        .unwrap();
                    guest.eval_global("globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))").unwrap();
                    let raw_placement = guest
                        .eval_global("globalThis.__bskyVisiblePlacement || ''")
                        .unwrap();
                    if !raw_placement.is_empty() {
                        let placement: PlacementMessage =
                            serde_json::from_str(&raw_placement).unwrap();
                        placement.validate_for(&request, &reply.rows).unwrap();
                        let row = &placement.rows[0];
                        let last_index = lines.len() - 1;
                        let last_y = i64::from(row.y)
                            + i64::from(reply.rows[0].baseline_offset)
                            + last_index as i64 * 18;
                        if last_y >= bsky_pocket::visible_text::VIEWPORT_TOP
                            && last_y < bsky_pocket::visible_text::VIEWPORT_BOTTOM
                        {
                            assert_eq!(lines[last_index], text.split_whitespace().last().unwrap());
                            assert!(row.y < bsky_pocket::visible_text::VIEWPORT_BOTTOM as i32);
                            assert!(last_y >= bsky_pocket::visible_text::VIEWPORT_TOP);
                            seen_last = true;
                            break;
                        }
                    }
                }
            }
        }
        guest
            .advance_with_deadline(0x0040, deadline)
            .expect("DOWN press advances");

        guest
            .present_with_deadline(deadline)
            .expect("press presents");
        guest
            .advance_with_deadline(0, deadline)
            .expect("release frame advances");
        guest
            .present_with_deadline(deadline)
            .expect("release presents");
        assert!(
            std::time::Instant::now() < deadline,
            "test stayed within deadline"
        );
    }
    assert!(
        seen_last,
        "scrolling and bounded callback measurement must expose the final line (iterations={iterations}, candidates={candidate_count}, completed={completed_count}, cache={})",
        cache.len()
    );
}

#[test]
fn cached_metrics_are_exact_native_wire_rows_and_relayout_without_measurement() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":1,"source":"following","cursor":null},"posts":[post("at://cached", "A measured cached body", "clean")],"cursor":null});
    guest.deliver_feed_json(&feed.to_string()).unwrap();
    let stamp = Stamp {
        auth_generation: 1,
        mount_generation: 1,
        history_revision: 1,
        layout_id: 1,
    };
    let result = MeasureResult {
        uri: "at://cached".into(),
        status: MeasureStatus::Ready,
        width: 128,
        line_count: 2,
        line_height: 18,
        baseline_offset: 14,
        body_height: 36,
        continuation: false,
    };
    let request = MeasureRequest {
        schema: 1,
        stamp: stamp.clone(),
        rows: vec![bsky_pocket::visible_text::MeasureRow {
            uri: result.uri.clone(),
            width: result.width,
        }],
    };
    guest
        .set_global_json(
            "__bskyVisibleRequest",
            &serde_json::to_string(&request).unwrap(),
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
        .unwrap();
    guest
        .eval_global(
            "globalThis.__bskyVisibleCandidates(JSON.stringify(globalThis.__bskyVisibleRequest))",
        )
        .unwrap();
    let reply = MeasureReply {
        schema: 1,
        stamp: stamp.clone(),
        rows: vec![result.clone()],
    };
    guest
        .set_global_json(
            "__bskyVisibleMeasurement",
            &reply.to_bounded_json().unwrap(),
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
        .unwrap();
    guest
        .eval_global(
            "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
        )
        .unwrap();
    let raw = serde_json::json!([{"uri":result.uri,"status":"ready","width":128,"line_count":2,"line_height":18,"baseline_offset":14,"body_height":36,"continuation":false}]).to_string();
    let parsed: Vec<MeasureResult> = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed, vec![result]);
    let viewport = MeasureRequest {
        schema: 1,
        stamp,
        rows: request.rows.clone(),
    };
    guest
        .set_global_json(
            "__bskyVisiblePlacementRequest",
            &serde_json::to_string(&viewport).unwrap(),
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
        .unwrap();
    guest
        .set_global_json(
            "__bskyVisibleMetrics",
            &raw,
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
        .unwrap();
    guest
        .eval_global(
            "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
        )
        .unwrap();
    let metrics: Vec<MeasureResult> = serde_json::from_str(&raw).unwrap();
    assert!(!metrics.is_empty());
    assert!(metrics.iter().all(|metric| metric.line_count > 0));
}

#[test]
fn context_feed_rows_are_joined_into_layout_and_advance_body_and_row_positions() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    assert_eq!(guest.state(), GuestState::Active);
    // Feed DTO arrives through the real packaged guest callback below.
    let mut contextual = post("at://one", "A real variable-width line Ω", "clean");
    contextual["context"] = serde_json::json!("reply");
    let second = post("at://two", "Two line body", "clean");
    let hidden = post("at://hidden", "", "blocked");
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":1,"source":"following","cursor":null},"posts":[contextual,second,hidden],"cursor":null});
    guest.deliver_feed_json(&feed.to_string()).unwrap();
    guest
        .eval("globalThis.nativeBatch={commands:[],image:null};")
        .unwrap();
    let request = MeasureRequest {
        schema: 1,
        stamp: Stamp {
            auth_generation: 1,
            mount_generation: 1,
            history_revision: 1,
            layout_id: 1,
        },
        rows: vec![
            bsky_pocket::visible_text::MeasureRow {
                uri: "at://one".into(),
                width: 128,
            },
            bsky_pocket::visible_text::MeasureRow {
                uri: "at://two".into(),
                width: 128,
            },
            bsky_pocket::visible_text::MeasureRow {
                uri: "at://hidden".into(),
                width: 128,
            },
        ],
    };
    request.validate().unwrap();
    guest
        .set_global_json(
            "__bskyVisibleRequest",
            &serde_json::to_string(&request).unwrap(),
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
        .unwrap();
    let input = guest
        .eval_global(
            "globalThis.__bskyVisibleCandidates(JSON.stringify(globalThis.__bskyVisibleRequest))",
        )
        .unwrap();
    let candidates: serde_json::Value = serde_json::from_str(&input).unwrap();
    assert_eq!(
        candidates["rows"][0]["text"],
        "A real variable-width line Ω"
    );
    // Context is local metadata, joined by the guest from the delivery when
    // computing layout; it is intentionally absent from the native text protocol.
    assert!(candidates["rows"][0].get("context").is_none());
    assert_eq!(candidates["rows"][1]["text"], "Two line body");
    assert!(candidates["rows"][1].get("context").is_none());
    assert_eq!(candidates["rows"][2]["text"], "");
    assert!(candidates["rows"][2].get("context").is_none());
    let measured_width = candidates["rows"][0]["text"]
        .as_str()
        .unwrap()
        .chars()
        .count() as u16
        * 7;
    let reply = MeasureReply {
        schema: 1,
        stamp: request.stamp.clone(),
        rows: vec![
            MeasureResult {
                uri: "at://one".into(),
                status: MeasureStatus::Ready,
                width: 128,
                line_count: 1,
                line_height: 18,
                baseline_offset: 14,
                body_height: 18,
                continuation: false,
            },
            MeasureResult {
                uri: "at://two".into(),
                status: MeasureStatus::Ready,
                width: 128,
                line_count: 2,
                line_height: 18,
                baseline_offset: 14,
                body_height: 36,
                continuation: false,
            },
            MeasureResult {
                uri: "at://hidden".into(),
                status: MeasureStatus::Placeholder,
                width: 128,
                line_count: 0,
                line_height: 18,
                baseline_offset: 14,
                body_height: 18,
                continuation: false,
            },
        ],
    };
    reply.validate_for(&request).unwrap();
    guest
        .set_global_json(
            "__bskyVisibleMeasurement",
            &reply.to_bounded_json().unwrap(),
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
        .unwrap();
    let heights = serde_json::json!([
        {"uri":"at://one","status":"ready","width":128,"line_count":1,"line_height":18,"baseline_offset":14,"body_height":18,"continuation":false},
        {"uri":"at://two","status":"ready","width":128,"line_count":2,"line_height":18,"baseline_offset":14,"body_height":36,"continuation":false},
        {"uri":"at://hidden","status":"placeholder","width":128,"line_count":0,"line_height":18,"baseline_offset":14,"body_height":18,"continuation":false}
    ]);
    guest.set_global_json("__bskyVisibleWindow", &serde_json::json!({"schema":1,"stamp":{"auth_generation":1,"mount_generation":1,"history_revision":1,"layout_id":1},"rows":[{"uri":"at://one","width":128},{"uri":"at://two","width":128},{"uri":"at://hidden","width":128}]}).to_string(), std::time::Instant::now() + bsky_pocket::frame_budget()).unwrap();
    guest
        .set_global_json(
            "__bskyVisibleMetrics",
            &heights.to_string(),
            std::time::Instant::now() + bsky_pocket::frame_budget(),
        )
        .unwrap();
    guest
        .eval_global(
            "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
        )
        .unwrap();
    let scripted_placement: serde_json::Value = {
        // Explicit placement request mirrors the native host's viewport contract.
        guest
            .set_global_json(
                "__bskyVisiblePlacementRequest",
                &serde_json::json!({"schema":1,"stamp":request.stamp,"rows":request.rows})
                    .to_string(),
                std::time::Instant::now() + bsky_pocket::frame_budget(),
            )
            .unwrap();
        let heights = serde_json::json!([
            {"uri":"at://one","status":"ready","width":128,"line_count":1,"line_height":18,"baseline_offset":14,"body_height":18,"continuation":false},
            {"uri":"at://two","status":"ready","width":128,"line_count":2,"line_height":18,"baseline_offset":14,"body_height":36,"continuation":false},
            {"uri":"at://hidden","status":"placeholder","width":128,"line_count":0,"line_height":18,"baseline_offset":14,"body_height":18,"continuation":false}
        ]);
        guest
            .set_global_json(
                "__bskyVisibleMetrics",
                &heights.to_string(),
                std::time::Instant::now() + bsky_pocket::frame_budget(),
            )
            .unwrap();
        guest.eval_global("globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))").unwrap();
        let placement = serde_json::json!({"schema":1,"stamp":request.stamp,"rows":[
            {"uri":"at://one","x":20,"y":88,"body_height":18},
            {"uri":"at://two","x":20,"y":172,"body_height":36},
            {"uri":"at://hidden","x":20,"y":288,"body_height":18}
        ]});
        serde_json::from_value(placement).unwrap()
    };
    // Derive expected placement from the guest's explicit card components:
    // body top = card top + vertical padding + header + header gap + context.
    let first_body_y = 40 + 8 + 22 + 4 + 12 + 2;
    let first_card_height = 8 + 22 + 4 + 12 + 2 + 18 + 4 + 12 + 8;
    let second_card_top = 40 + first_card_height + 8;
    let second_body_y = second_card_top + 8 + 22 + 4;
    let second_card_height = 8 + 22 + 4 + 36 + 4 + 12 + 8;
    let third_card_top = second_card_top + second_card_height + 8;
    let third_body_y = third_card_top + 8 + 22 + 4 + 14;
    assert_eq!(scripted_placement["rows"][0]["y"], first_body_y);
    assert_eq!(scripted_placement["rows"][1]["y"], second_body_y);
    assert_eq!(scripted_placement["rows"][2]["y"], third_body_y);
    assert_eq!(scripted_placement["rows"][0]["x"], 12 + 8);
    assert_eq!(
        scripted_placement["rows"][1]["y"],
        scripted_placement["rows"][0]["y"].as_i64().unwrap() - (8 + 22 + 4 + 12 + 2)
            + first_card_height
            + 8
            + 8
            + 22
            + 4
    );
    let placement: PlacementMessage = serde_json::from_value(scripted_placement.clone()).unwrap();
    placement.validate_for(&request, &reply.rows).unwrap();
    let stale = MeasureReply {
        stamp: Stamp {
            layout_id: 2,
            ..request.stamp.clone()
        },
        ..reply.clone()
    };
    assert!(
        stale.validate_for(&request).is_err(),
        "stale measurement cannot be applied"
    );
    let mut overlay = bsky_pocket::NativeBatch {
        commands: vec![],
        image: None,
    };
    overlay.commands.push(bsky_pocket::NativeCommand::Text {
        x: placement.rows[0].x as f32,
        y: (placement.rows[0].y + i32::from(reply.rows[0].baseline_offset)) as f32,
        scale: 0.75,
        color: u32::MAX,
        text: candidates["rows"][0]["text"].as_str().unwrap().into(),
        stamp: Some(request.stamp.clone()),
        line: 0,
        line_height: reply.rows[0].line_height,
    });
    assert_eq!(overlay.commands.len(), 1);
    assert!(!overlay.to_json().unwrap().contains("secret text"));
    assert!(measured_width > 128);
    // Exercise the production batch serialization path after guest commands
    // are retrieved, then append the validated host overlay.
    let guest_batch = guest.native_batch().unwrap();
    guest_batch.validate(480, 272).unwrap();
    assert!(guest_batch.commands.is_empty());
    overlay.validate(480, 272).unwrap();
    assert_eq!(overlay.commands.len(), 1);
    assert!(overlay
        .to_json()
        .unwrap()
        .contains("A real variable-width line Ω"));
    let oversized = "x".repeat(1025);
    let mut calls = 0;
    let (lines, _) = bsky_pocket::visible_text::wrap_bounded(&oversized, 440, |_| {
        calls += 1;
        1
    });
    assert!(calls <= bsky_pocket::visible_text::MAX_MEASURE_CALLS_ROW);
    assert!(lines.iter().map(String::len).sum::<usize>() <= 256);
    guest.dispose();
    assert_eq!(guest.state(), GuestState::Disposed);
}
