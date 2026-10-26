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
use bsky_worker::media_executor::AssetIdentityKey;
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

fn asset_kind(descriptor: &bsky_feed::NativeAssetDescriptor) -> AssetKind {
    match &descriptor.identity {
        bsky_feed::AssetIdentity::Avatar { .. } => AssetKind::Avatar {
            followed: descriptor.following_confirmed,
        },
        bsky_feed::AssetIdentity::PostImage { .. } => AssetKind::PostImage { persist: true },
    }
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
    let visible = registry.visible_handles(visible, bsky_pocket::MAX_NATIVE_IMAGE_HANDLES);

    let mut installed = 0usize;
    for completion in registry.drain() {
        let Some(descriptor) = registry.descriptor_for_identity(&completion.identity) else {
            continue;
        };
        let kind = asset_kind(descriptor);
        let avatar = matches!(kind, AssetKind::Avatar { .. });
        let key = identity_key(&completion.identity);
        let Ok(bytes) = completion.bytes.as_ref() else {
            continue;
        };
        let _ = disk.put(&completion.identity, kind, bytes);
        if insert_texture(cache, &key, bytes, avatar) {
            installed += 1;
        }
    }

    handles.clear();
    let mut missing = Vec::new();
    for (slot, handle) in visible.iter().enumerate() {
        let Some(descriptor) = registry.descriptor(handle) else {
            continue;
        };
        let identity = AssetIdentityKey::from_asset_identity(
            &descriptor.scope.account_did,
            descriptor.scope.auth_generation,
            &descriptor.identity,
        );
        let key = identity_key(&identity);
        let kind = asset_kind(descriptor);
        let fresh = prime_from_disk(
            cache,
            disk,
            &identity,
            kind,
            matches!(kind, AssetKind::Avatar { .. }),
        )
        .is_some();
        if cache.contains(&key) {
            handles.insert(slot as u32 + 1, key);
        }
        if !fresh {
            missing.push(handle.clone());
        }
    }

    let deadline = Instant::now() + MEDIA_FETCH_BUDGET;
    registry.submit_visible(missing, deadline, VISIBLE_SUBMIT_LIMIT);
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
    let present = cache.contains(&key);
    let fresh = restore_bytes(disk, identity, kind, |bytes| {
        present || insert_texture(cache, &key, bytes, avatar)
    })?;
    fresh.then_some(key)
}

fn restore_bytes(
    disk: &AssetCache,
    identity: &AssetIdentityKey,
    kind: AssetKind,
    mut upload: impl FnMut(&[u8]) -> bool,
) -> Option<bool> {
    let (bytes, state) = disk.get(identity, kind, SystemTime::now()).ok()??;
    upload(&bytes).then_some(state.fresh)
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
        let identity = bsky_feed::AssetIdentity::Avatar {
            author_did: "did:plc:author".into(),
            source_revision: "https://cdn.example/avatar:revision".into(),
        };
        let key = AssetIdentityKey::from_asset_identity("did:plc:me", 1, &identity);
        let descriptor = bsky_feed::NativeAssetDescriptor {
            scope: bsky_feed::AccountScope {
                account_did: "did:plc:me".into(),
                auth_generation: 1,
            },
            identity,
            handle: "h".into(),
            relationship: bsky_feed::Relationship::Followed,
            following_confirmed: true,
        };
        let mut registry = Registry::new();
        assert!(registry.install(descriptor.scope.clone(), &[descriptor]));
        let restored = registry.descriptor_for_identity(&key).unwrap();
        assert!(
            matches!(&restored.identity, bsky_feed::AssetIdentity::Avatar { author_did, .. }
            if author_did == "did:plc:author")
        );
        assert_eq!(asset_kind(restored), AssetKind::Avatar { followed: true });
        let mut image = restored.clone();
        image.identity = bsky_feed::AssetIdentity::PostImage {
            post_uri: "post".into(),
            cid: "cid".into(),
            index: 0,
            thumbnail: "t".into(),
            fullsize: "f".into(),
        };
        assert_eq!(asset_kind(&image), AssetKind::PostImage { persist: true });
    }

    #[test]
    fn restart_restores_fresh_bytes_before_network_and_failed_upload_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let identity = AssetIdentityKey::from_asset_identity(
            "did:plc:me",
            1,
            &bsky_feed::AssetIdentity::Avatar {
                author_did: "did:plc:author".into(),
                source_revision: "revision".into(),
            },
        );
        let kind = AssetKind::Avatar { followed: true };
        AssetCache::new(dir.path())
            .put(&identity, kind, b"cached-image")
            .unwrap();
        let restarted = AssetCache::new(dir.path());
        let mut uploads = 0;
        assert_eq!(
            restore_bytes(&restarted, &identity, kind, |bytes| {
                uploads += 1;
                assert_eq!(bytes, b"cached-image");
                true
            }),
            Some(true)
        );
        assert_eq!(uploads, 1);
        assert_eq!(restore_bytes(&restarted, &identity, kind, |_| false), None);
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
