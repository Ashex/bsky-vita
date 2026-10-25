use bsky_pocket::visible_text::{
    MeasureReply, MeasureRequest, MeasureResult, MeasureStatus, PlacementMessage, Stamp,
};
use bsky_pocket::{EmbeddedGuest, GuestState};

fn post(uri: &str, text: &str, state: &str) -> serde_json::Value {
    serde_json::json!({"uri":uri,"cid":"cid","author":{"did":"did:example","handle":"example.test","display_name":"Example"},"text":text,"facets":[],"like_count":2,"repost_count":1,"reply_count":3,"state":{"state":state},"quote":null,"images":[],"context":null})
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
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
    assert_eq!(window["rows"][0]["uri"], "at://150");
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
    assert_eq!(window["rows"][0]["uri"], "at://152");
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
    assert_eq!(window["rows"][0]["uri"], "at://151");
    let selected = window["rows"][0]["uri"].as_str().unwrap();
    assert_eq!(selected, "at://151");
    assert!(!candidates.is_empty(), "guest should find requested rows");
    let raw_window = guest
        .eval_global("globalThis.__bskyVisibleWindow || '[]'")
        .unwrap();
    let window: serde_json::Value = serde_json::from_str(&raw_window).unwrap();
    assert_eq!(
        window["rows"],
        serde_json::json!([
            {"uri":"at://151","width":440},
            {"uri":"at://152","width":440},
            {"uri":"at://153","width":440}
        ])
    );
    // Appending/replacing retained history must preserve the selected URI.
    let appended: Vec<_> = (0..200)
        .map(|i| post(&format!("at://{i}"), "row", "clean"))
        .collect();
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":2,"source":"following","cursor":null},"posts":appended,"cursor":null});
    guest
        .deliver_feed_json_with_deadline(&feed.to_string(), deadline)
        .unwrap();
    let preserved = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let preserved: serde_json::Value = serde_json::from_str(&preserved).unwrap();
    assert_eq!(preserved["rows"][0]["uri"], "at://151");
    let replacement = vec![post("at://replacement", "new", "clean")];
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":3,"source":"following","cursor":null},"posts":replacement,"cursor":null});
    guest
        .deliver_feed_json_with_deadline(&feed.to_string(), deadline)
        .unwrap();
    let reset_raw = guest.eval_global("globalThis.__bskyVisibleWindow").unwrap();
    let reset: serde_json::Value = serde_json::from_str(&reset_raw).unwrap();
    assert_eq!(reset["rows"][0]["uri"], "at://replacement");
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
    guest
        .eval_global(
            "globalThis.__bskyVisibleMeasured(JSON.stringify(globalThis.__bskyVisibleMeasurement))",
        )
        .unwrap();
    let scripted_placement: serde_json::Value = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    // Derive expected placement from the guest's explicit card components:
    // body top = card top + vertical padding + header + header gap + context.
    let first_body_y = 40 + 8 + 22 + 4 + 12 + 2;
    let first_card_height = 8 + 22 + 4 + 12 + 2 + 18 + 4 + 12 + 8;
    let second_card_top = 40 + first_card_height + 8;
    let second_body_y = second_card_top + 8 + 22 + 4;
    let second_card_height = 8 + 22 + 4 + 36 + 4 + 12 + 8;
    let third_card_top = second_card_top + second_card_height + 8;
    let third_body_y = third_card_top + 8 + 22 + 4;
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
    let placement: PlacementMessage = serde_json::from_str(
        &guest
            .eval_global("globalThis.__bskyVisiblePlacement")
            .unwrap(),
    )
    .unwrap();
    placement.validate_for(&request).unwrap();
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
        y: (placement.rows[0].y + reply.rows[0].baseline_offset) as f32,
        scale: 0.75,
        color: u32::MAX,
        text: candidates["rows"][0]["text"].as_str().unwrap().into(),
        stamp: Some(request.stamp.clone()),
        line: 0,
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
