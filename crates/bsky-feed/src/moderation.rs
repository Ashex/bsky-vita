use crate::{ContentState, PostDto};

#[derive(Clone, Debug, Default)]
pub struct ModerationPolicy {
    pub blocked: Vec<String>,
    pub muted: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayDecision {
    Show,
    Placeholder(ContentState),
}

impl ModerationPolicy {
    /// Fail closed for unavailable/unknown state. Quotes are evaluated as
    /// independent posts by callers; a hidden outer post exposes no children.
    pub fn decide(&self, post: &PostDto) -> DisplayDecision {
        self.decide_state(&post.author.did, &post.state)
    }

    pub fn decide_state(&self, author_did: &str, state: &ContentState) -> DisplayDecision {
        if self.blocked.iter().any(|did| did == author_did) {
            return DisplayDecision::Placeholder(ContentState::Blocked);
        }
        if self.muted.iter().any(|did| did == author_did) {
            return DisplayDecision::Placeholder(ContentState::Muted);
        }
        match state {
            ContentState::Clean => DisplayDecision::Show,
            ContentState::Warning(warning) => {
                DisplayDecision::Placeholder(ContentState::Warning(warning.clone()))
            }
            ContentState::Blocked => DisplayDecision::Placeholder(ContentState::Blocked),
            ContentState::Muted => DisplayDecision::Placeholder(ContentState::Muted),
            ContentState::Unavailable => DisplayDecision::Placeholder(ContentState::Unavailable),
            ContentState::Unknown => DisplayDecision::Placeholder(ContentState::Unknown),
        }
    }

    /// Only visible clean content yields metadata eligible for an image request.
    pub fn visible_image_metadata<'a>(&self, post: &'a PostDto) -> &'a [crate::ImageMetadata] {
        if self.decide(post) == DisplayDecision::Show {
            &post.images
        } else {
            &[]
        }
    }
}
