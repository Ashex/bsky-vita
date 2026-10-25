#[path = "../src/action_dispatch.rs"]
mod action_dispatch;

#[test]
fn auth_failure_suppresses_same_frame_auth_complete_dispatch() {
    let auth_complete_action_was_produced = true;
    let session_dead = true;

    // This is the production gate used before matching and dispatching every
    // ScreenAction variant, including AuthComplete.
    assert!(auth_complete_action_was_produced);
    assert!(!action_dispatch::allowed(
        session_dead,
        action_dispatch::ActionKind::AuthComplete,
    ));
}

#[test]
fn healthy_session_allows_same_frame_action_dispatch() {
    assert!(action_dispatch::allowed(
        false,
        action_dispatch::ActionKind::Other
    ));
}
