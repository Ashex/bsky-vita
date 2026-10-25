#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionKind {
    AuthComplete,
    Other,
}

/// Whether an action produced before the worker-response drain may be applied.
///
/// Authentication failures are handled after that drain and must take
/// precedence over same-frame actions, including `AuthComplete`.
pub(crate) fn allowed(session_dead: bool, action: ActionKind) -> bool {
    match (session_dead, action) {
        (true, ActionKind::AuthComplete) => false,
        (true, ActionKind::Other) => false,
        (false, _) => true,
    }
}
