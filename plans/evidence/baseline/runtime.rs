use bsky_pocket::visible_text::{
    MeasureReply, MeasureRequest, MeasureResult, MeasureStatus, PlacementMessage, Stamp,
};
use bsky_pocket::{EmbeddedGuest, GuestState};

fn post(uri: &str, text: &str, state: &str) -> serde_json::Value {
    serde_json::json!({"uri":uri,"cid":"cid","author":{"did":"did:example","handle":"example.test","display_name":"Example"},"text":text,"facets":[],"like_count":2,"repost_count":1,"reply_count":3,"state":{"state":state},"quote":null,"images":[]})
}

#[test]
fn packaged_feed_uses_stamped_measure_place_handshake_and_bounded_host_overlay() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("runtime initializes");
    assert_eq!(guest.state(), GuestState::Active);
    let feed = serde_json::json!({"schema":1,"correlation":{"schema":1,"auth_generation":1,"mount_generation":1,"request_id":1,"source":"following","cursor":null},"posts":[post("at://one", "A real variable-width line Ω", "clean"),post("at://hidden", "secret text must not leak", "blocked")],"cursor":null});
    guest.deliver_feed_json(&feed.to_string()).unwrap();
    guest
        .eval("globalThis.nativeBatch={commands:[],image:null};")
        .unwrap();
    guest.eval("globalThis.__bskyVisibleCandidates=raw=>{const r=JSON.parse(raw);return JSON.stringify({schema:1,stamp:r.stamp,rows:r.rows.map(x=>({uri:x.uri,width:x.width,text:x.uri==='at://hidden'?'':'A real variable-width line Ω',status:x.uri==='at://hidden'?'placeholder':'ready'}))})};globalThis.__bskyVisibleMeasured=raw=>{const r=JSON.parse(raw);let y=40;const rows=r.rows.map(x=>{const body=x.status==='placeholder'?18:x.body_height;const row={uri:x.uri,x:12,y,body_height:body};y+=76+body-18+8;return row});globalThis.__bskyVisiblePlacement=JSON.stringify({schema:1,stamp:r.stamp,rows})};").unwrap();
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
    assert_eq!(candidates["rows"][1]["text"], "");
    let measured_width = "A real variable-width line Ω".chars().count() as u16 * 7;
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
    guest.eval("globalThis.nativeBatch={commands:[{type:'text',x:1,y:1,scale:1,color:4294967295,text:'guest',stamp:null,line:0}],image:null}").unwrap();
    let mut final_batch = guest.native_batch().unwrap();
    final_batch.commands.extend(overlay.commands.clone());
    final_batch.validate(480, 272).unwrap();
    assert_eq!(final_batch.commands.len(), 2);
    assert!(final_batch
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
