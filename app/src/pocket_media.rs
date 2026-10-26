//! App-side bridge between the bounded media lane and the renderer.
//!
//! Fetched bytes live in `bsky_worker::media_registry` and arrive here as
//! completions. This module is the only place that turns those bytes into
//! GPU textures and remembers which guest handle refers to which asset, so
//! the render loop can draw an avatar from cache, from disk, or from a
//! freshly downloaded copy without knowing the difference.

use std::time::{Instant, SystemTime};

use bsky_media::asset_cache::{AssetCache, AssetKind};
use bsky_render::{ImageHandleMap, TextureCache};
use bsky_worker::media_executor::{AssetIdentityKey, MediaCompletion};
use bsky_worker::media_registry::PocketMediaRegistry;

pub use bsky_worker::media_registry::PocketMediaRegistry as Registry;

/// How long a freshly submitted media fetch may take end to end.
const MEDIA_FETCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
/// Upper bound on new downloads admitted from one frame's visible set.
const VISIBLE_SUBMIT_LIMIT: usize = 4;

/// Stable texture-cache key for an account-scoped asset identity.
pub fn identity_key(identity: &AssetIdentityKey) -> String {
    format!(
        "asset:{}:{}:{}",
        identity.scope_did, identity.auth_generation, identity.identity_tag
    )
}

/// The identity tag that matches `AssetIdentityKey::from_asset_identity`.
pub fn identity_tag(identity: &bsky_feed::AssetIdentity) -> String {
    match identity {
        bsky_feed::AssetIdentity::Avatar {
            author_did,
            source_revision,
        } => format!("avatar:{author_did}:{source_revision}"),
        bsky_feed::AssetIdentity::PostImage {
            post_uri,
            cid,
            index,
            thumbnail,
            fullsize,
        } => format!("image:{post_uri}:{cid}:{index}:{thumbnail}:{fullsize}"),
    }
}

fn is_avatar(identity: &AssetIdentityKey) -> bool {
    identity.identity_tag.starts_with("avatar:")
}

fn identity_author(identity: &AssetIdentityKey) -> Option<&str> {
    identity
        .identity_tag
        .strip_prefix("avatar:")?
        .split(':')
        .next()
}

/// Insert bytes into the texture cache, using the RGBA path (and circular
/// mask) for avatars. Returns the cache key on success.
fn insert_texture(cache: &mut TextureCache, key: &str, bytes: &[u8], avatar: bool) -> bool {
    let result = if avatar {
        cache.insert_rgba(key.to_string(), bytes)
    } else {
        cache.insert(key.to_string(), bytes)
    };
    if result.is_err() {
        return false;
    }
    if avatar {
        if let Some(texture) = cache.get(key) {
            texture.apply_circular_mask();
        }
    }
    true
}

/// Drain the media lane, install completed bytes as textures (writing them
/// through to disk), and bind visible handles to their texture keys.
///
/// `followed` is answered from the installed descriptor for the author, which
/// carries the natively confirmed relationship; unknown authors count as not
/// followed.
pub fn service(
    registry: &PocketMediaRegistry,
    cache: &mut TextureCache,
    disk: &AssetCache,
    handles: &mut ImageHandleMap,
    visible: &[String],
) -> usize {
    let followed_for = |author: &str| -> bool {
        visible.iter().any(|handle| {
            registry.descriptor(handle).is_some_and(|d| {
                d.following_confirmed
                    && matches!(
                        &d.identity,
                        bsky_feed::AssetIdentity::Avatar { author_did, .. } if author_did == author
                    )
            })
        })
    };

    let mut installed = 0usize;
    for completion in registry.drain() {
        let avatar = is_avatar(&completion.identity);
        let key = identity_key(&completion.identity);
        if cache.contains(&key) {
            continue;
        }
        let Ok(bytes) = completion.bytes.as_ref() else {
            continue;
        };
        let followed_author = followed_for(identity_author(&completion.identity).unwrap_or(""));
        let _ = disk.put(
            &completion.identity,
            AssetKind::Avatar {
                followed: followed_author,
            },
            bytes,
        );
        if insert_texture(cache, &key, bytes, avatar) {
            installed += 1;
        }
    }

    handles.clear();
    let mut next_handle = 1u32;
    for handle in visible {
        let Some(descriptor) = registry.descriptor(handle) else {
            continue;
        };
        let key = format!(
            "asset:{}:{}:{}",
            descriptor.scope.account_did,
            descriptor.scope.auth_generation,
            identity_tag(&descriptor.identity)
        );
        if cache.contains(&key) && handles.insert(next_handle, key) {
            next_handle = next_handle.saturating_add(1);
        }
    }

    let deadline = Instant::now() + MEDIA_FETCH_BUDGET;
    registry.submit_visible(visible.iter().cloned(), deadline, VISIBLE_SUBMIT_LIMIT);
    installed
}

/// Prime the session texture cache from disk without a network fetch.
pub fn prime_from_disk(
    cache: &mut TextureCache,
    disk: &AssetCache,
    identity: &AssetIdentityKey,
    kind: AssetKind,
    avatar: bool,
) -> Option<String> {
    let key = identity_key(identity);
    if cache.contains(&key) {
        return Some(key);
    }
    let (bytes, _fresh) = disk.get(identity, kind, SystemTime::now()).ok()??;
    insert_texture(cache, &key, &bytes, avatar).then_some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_key_is_stable_and_account_scoped() {
        let a = AssetIdentityKey {
            scope_did: "did:plc:a".into(),
            auth_generation: 1,
            identity_tag: "avatar:did:plc:x:rev1".into(),
        };
        let b = AssetIdentityKey {
            scope_did: "did:plc:b".into(),
            auth_generation: 1,
            identity_tag: "avatar:did:plc:x:rev1".into(),
        };
        assert_ne!(identity_key(&a), identity_key(&b));
        assert_eq!(identity_key(&a), identity_key(&a.clone()));
    }

    #[test]
    fn identity_author_extracts_the_did() {
        let key = AssetIdentityKey {
            scope_did: "did:plc:me".into(),
            auth_generation: 1,
            identity_tag: "avatar:did:plc:author:rev9".into(),
        };
        assert_eq!(identity_author(&key), Some("did:plc:author"));
        let image = AssetIdentityKey {
            scope_did: "did:plc:me".into(),
            auth_generation: 1,
            identity_tag: "image:at://x/1:bafy:0:t:f".into(),
        };
        assert_eq!(identity_author(&image), None);
        assert!(!is_avatar(&image));
        assert!(is_avatar(&key));
    }

    #[test]
    fn identity_tag_matches_executor_format() {
        let avatar = bsky_feed::AssetIdentity::Avatar {
            author_did: "did:plc:author".into(),
            source_revision: "rev".into(),
        };
        let key = AssetIdentityKey::from_asset_identity("did:plc:me", 3, &avatar);
        assert_eq!(key.identity_tag, identity_tag(&avatar));
    }
}
