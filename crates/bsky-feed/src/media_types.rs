use crate::{FeedCorrelation, FeedDelivery};

pub const MAX_ACCOUNT_DID_BYTES: usize = 1024;
pub const MAX_POST_URI_BYTES: usize = 1024;
pub const MAX_CID_BYTES: usize = 1024;
pub const MAX_REVISION_URL_BYTES: usize = 8192;
pub const MAX_AGGREGATE_DESCRIPTOR_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AssetHandleKind {
    Avatar,
    Image(u32),
}

fn parse_asset_handle(handle: &str, request_id: u64) -> Option<(usize, AssetHandleKind)> {
    if handle.len() > 256 {
        return None;
    }
    let prefix = "asset:";
    let rest = handle.strip_prefix(prefix)?;
    let (rid_str, rest) = rest.split_once(':')?;
    if rid_str.parse::<u64>().ok()? != request_id {
        return None;
    }
    let (post_idx_str, kind_str) = rest.split_once(':')?;
    if post_idx_str.is_empty() || kind_str.is_empty() {
        return None;
    }
    if post_idx_str.as_bytes().iter().any(|b| !b.is_ascii_digit()) {
        return None;
    }
    let post_index: usize = post_idx_str.parse().ok()?;
    let expected_prefix = format!("asset:{}:{}:", request_id, post_index);
    if !handle.starts_with(&expected_prefix) {
        return None;
    }
    let kind = if kind_str == "avatar" {
        let expected_full = format!("asset:{}:{}:avatar", request_id, post_index);
        if handle != expected_full {
            return None;
        }
        AssetHandleKind::Avatar
    } else {
        let image_index: u32 = kind_str.parse().ok()?;
        let expected_full = format!("asset:{}:{}:{}", request_id, post_index, image_index);
        if handle != expected_full {
            return None;
        }
        AssetHandleKind::Image(image_index)
    };
    Some((post_index, kind))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountScope {
    pub account_did: String,
    pub auth_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AssetIdentity {
    Avatar {
        author_did: String,
        source_revision: String,
    },
    PostImage {
        post_uri: String,
        cid: String,
        index: u32,
        thumbnail: String,
        fullsize: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relationship {
    Unknown,
    Followed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeAssetDescriptor {
    pub scope: AccountScope,
    pub identity: AssetIdentity,
    pub handle: String,
    pub relationship: Relationship,
    pub following_confirmed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeFeedEnvelope {
    pub correlation: FeedCorrelation,
    pub descriptors: Vec<NativeAssetDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedFeed {
    pub delivery: FeedDelivery,
    pub native: NativeFeedEnvelope,
}

impl NativeFeedEnvelope {
    pub fn matches(&self, delivery: &FeedDelivery, expected_account_did: &str) -> bool {
        self.matches_for(delivery, expected_account_did)
    }

    pub fn matches_for(&self, delivery: &FeedDelivery, expected_account_did: &str) -> bool {
        if expected_account_did.is_empty()
            || expected_account_did.len() > MAX_ACCOUNT_DID_BYTES
            || self.correlation != delivery.correlation
            || self.correlation.schema != delivery.schema
        {
            return false;
        }
        if self.descriptors.is_empty() {
            return true;
        }
        if self.descriptors.len() > crate::MAX_HISTORY * 9
            || self.aggregate_bytes() > MAX_AGGREGATE_DESCRIPTOR_BYTES
        {
            return false;
        }
        let mut seen_handles = std::collections::HashSet::with_capacity(self.descriptors.len());
        for descriptor in &self.descriptors {
            if descriptor.scope.account_did != expected_account_did
                || descriptor.scope.auth_generation != self.correlation.auth_generation
            {
                return false;
            }
            if !seen_handles.insert(&descriptor.handle) {
                return false;
            }
            let (post_index, identity_kind) =
                match parse_asset_handle(&descriptor.handle, self.correlation.request_id) {
                    Some(parsed) => parsed,
                    None => return false,
                };
            let post = match delivery.posts.get(post_index) {
                Some(post) if post.state == crate::ContentState::Clean => post,
                _ => return false,
            };
            match &descriptor.identity {
                AssetIdentity::Avatar {
                    author_did,
                    source_revision,
                } => {
                    if identity_kind != AssetHandleKind::Avatar {
                        return false;
                    }
                    if author_did.is_empty()
                        || author_did.len() > MAX_ACCOUNT_DID_BYTES
                        || source_revision.is_empty()
                        || source_revision.len() > MAX_REVISION_URL_BYTES
                    {
                        return false;
                    }
                    if post.author.did != *author_did {
                        return false;
                    }
                }
                AssetIdentity::PostImage {
                    post_uri,
                    cid,
                    index,
                    thumbnail,
                    fullsize,
                } => {
                    if identity_kind != AssetHandleKind::Image(*index) {
                        return false;
                    }
                    if cid.is_empty()
                        || cid.len() > MAX_CID_BYTES
                        || thumbnail.is_empty()
                        || thumbnail.len() > MAX_REVISION_URL_BYTES
                        || fullsize.is_empty()
                        || fullsize.len() > MAX_REVISION_URL_BYTES
                        || post_uri.is_empty()
                        || post_uri.len() > MAX_POST_URI_BYTES
                    {
                        return false;
                    }
                    if post.uri != *post_uri || post.cid != *cid {
                        return false;
                    }
                    match post.images.get(*index as usize) {
                        Some(image) if image.alt.len() <= crate::MAX_FIELD_BYTES => {}
                        _ => return false,
                    }
                }
            }
        }
        true
    }

    fn aggregate_bytes(&self) -> usize {
        self.descriptors.iter().fold(0usize, |total, descriptor| {
            let identity_bytes = match &descriptor.identity {
                AssetIdentity::Avatar {
                    author_did,
                    source_revision,
                } => author_did.len().saturating_add(source_revision.len()),
                AssetIdentity::PostImage {
                    post_uri,
                    cid,
                    thumbnail,
                    fullsize,
                    ..
                } => post_uri
                    .len()
                    .saturating_add(cid.len())
                    .saturating_add(thumbnail.len())
                    .saturating_add(fullsize.len()),
            };
            total
                .saturating_add(descriptor.scope.account_did.len())
                .saturating_add(descriptor.handle.len())
                .saturating_add(identity_bytes)
        })
    }
}

impl ProjectedFeed {
    pub fn new(delivery: FeedDelivery, descriptors: Vec<NativeAssetDescriptor>) -> Self {
        Self {
            native: NativeFeedEnvelope {
                correlation: delivery.correlation.clone(),
                descriptors,
            },
            delivery,
        }
    }

    pub fn validate_for(&self, expected_account_did: &str) -> bool {
        self.delivery.schema == crate::SCHEMA_VERSION
            && serde_json::to_vec(&self.delivery)
                .is_ok_and(|bytes| bytes.len() <= crate::MAX_DELIVERY_BYTES)
            && self.delivery.posts.len() <= crate::MAX_HISTORY
            && self.delivery.correlation.validate().is_ok()
            && self
                .delivery
                .cursor
                .as_ref()
                .is_none_or(|c| c.len() <= crate::MAX_FIELD_BYTES)
            && self
                .delivery
                .posts
                .iter()
                .all(|post| post.validate().is_ok())
            && self
                .native
                .matches_for(&self.delivery, expected_account_did)
    }

    pub fn validate(&self) -> bool {
        self.native.descriptors.is_empty() && self.validate_for("")
    }
}
