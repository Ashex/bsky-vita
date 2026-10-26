use std::collections::HashMap;
use std::time::Instant;

use bsky_feed::{
    AccountScope, AssetIdentity, NativeAssetDescriptor, MAX_AGGREGATE_DESCRIPTOR_BYTES,
};

use crate::media_executor::{
    fetch_url, Admission, AssetIdentityKey, MediaCompletion, MediaExecutor,
};

pub const MAX_DESCRIPTORS: usize = bsky_feed::MAX_HISTORY * 9;
const IMAGE_BYTE_QUOTA: u64 = bsky_net::MAX_IMAGE_BYTES as u64;

/// Bounded, account-scoped index of feed media descriptors.
pub struct PocketMediaRegistry {
    executor: MediaExecutor,
    scope: Option<AccountScope>,
    descriptors: HashMap<String, NativeAssetDescriptor>,
}

impl Default for PocketMediaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PocketMediaRegistry {
    pub fn new() -> Self {
        Self {
            executor: MediaExecutor::spawn_default(),
            scope: None,
            descriptors: HashMap::new(),
        }
    }

    /// Replace the bounded descriptor index for one validated account scope.
    pub fn install(&mut self, scope: AccountScope, descriptors: &[NativeAssetDescriptor]) -> bool {
        if descriptors.len() > MAX_DESCRIPTORS
            || descriptors.iter().map(descriptor_bytes).sum::<usize>()
                > MAX_AGGREGATE_DESCRIPTOR_BYTES
            || descriptors
                .iter()
                .any(|d| d.scope != scope || d.handle.len() > 256)
        {
            return false;
        }
        let mut indexed = HashMap::with_capacity(descriptors.len());
        for descriptor in descriptors {
            if indexed
                .insert(descriptor.handle.clone(), descriptor.clone())
                .is_some()
            {
                return false;
            }
        }
        if self.scope.as_ref().is_none_or(|current| current != &scope) {
            self.executor.set_generation(scope.auth_generation);
        }
        self.scope = Some(scope);
        self.descriptors = indexed;
        true
    }

    /// Submit only a descriptor already installed for the current scope.
    pub fn submit(&self, handle: &str, deadline: Instant) -> Option<Admission> {
        let descriptor = self.descriptors.get(handle)?;
        let scope = self.scope.as_ref()?;
        if descriptor.scope != *scope {
            return None;
        }
        let url = fetch_url(&descriptor.identity)?;
        let key = AssetIdentityKey::from_asset_identity(
            &scope.account_did,
            scope.auth_generation,
            &descriptor.identity,
        );
        Some(self.executor.submit(key, url, deadline, IMAGE_BYTE_QUOTA))
    }

    /// Drain completions belonging to the installed account and auth generation.
    pub fn drain(&self) -> Vec<MediaCompletion> {
        let mut accepted = Vec::new();
        while let Some(completion) = self.executor.try_recv() {
            if self
                .scope
                .as_ref()
                .is_some_and(|scope| completion_matches(scope, &completion.identity))
            {
                accepted.push(completion);
            }
        }
        accepted
    }

    /// Invalidate pending work and discard all descriptors before account teardown.
    pub fn invalidate(&mut self) {
        self.executor.invalidate();
        self.scope = None;
        self.descriptors.clear();
    }
}

fn completion_matches(scope: &AccountScope, identity: &AssetIdentityKey) -> bool {
    scope.account_did == identity.scope_did && scope.auth_generation == identity.auth_generation
}

fn descriptor_bytes(descriptor: &NativeAssetDescriptor) -> usize {
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
    descriptor
        .scope
        .account_did
        .len()
        .saturating_add(descriptor.handle.len())
        .saturating_add(identity_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsky_feed::Relationship;

    fn scope(did: &str, generation: u64) -> AccountScope {
        AccountScope {
            account_did: did.into(),
            auth_generation: generation,
        }
    }
    fn descriptor(scope: AccountScope, handle: &str, revision: &str) -> NativeAssetDescriptor {
        NativeAssetDescriptor {
            scope,
            identity: AssetIdentity::Avatar {
                author_did: "did:plc:author".into(),
                source_revision: revision.into(),
            },
            handle: handle.into(),
            relationship: Relationship::Unknown,
            following_confirmed: false,
        }
    }
    fn registry() -> PocketMediaRegistry {
        PocketMediaRegistry::new()
    }

    #[test]
    fn install_rejects_descriptor_count_boundary_plus_one() {
        let mut r = registry();
        let s = scope("did:plc:a", 1);
        let at_limit: Vec<_> = (0..MAX_DESCRIPTORS)
            .map(|i| descriptor(s.clone(), &format!("h{i}"), "x"))
            .collect();
        assert!(r.install(s.clone(), &at_limit));
        let mut over = at_limit;
        over.push(descriptor(s.clone(), "over", "x"));
        assert!(!r.install(s, &over));
    }

    #[test]
    fn install_rejects_aggregate_byte_boundary_plus_one() {
        let mut r = registry();
        let s = scope("", 1);
        assert!(r.install(
            s.clone(),
            &[descriptor(
                s.clone(),
                "",
                &"x".repeat(MAX_AGGREGATE_DESCRIPTOR_BYTES - 14)
            )]
        ));
        assert!(!r.install(
            s.clone(),
            &[descriptor(
                s.clone(),
                "",
                &"x".repeat(MAX_AGGREGATE_DESCRIPTOR_BYTES - 13)
            )]
        ));
    }

    #[test]
    fn install_rejects_oversized_sets_scope_mismatch_and_duplicate_handles() {
        let mut r = registry();
        let s = scope("did:plc:a", 1);
        let too_many: Vec<_> = (0..=MAX_DESCRIPTORS)
            .map(|i| descriptor(s.clone(), &format!("h{i}"), "x"))
            .collect();
        assert!(!r.install(s.clone(), &too_many));
        assert!(!r.install(s.clone(), &[descriptor(scope("did:plc:b", 1), "h", "x")]));
        assert!(!r.install(
            s.clone(),
            &[
                descriptor(s.clone(), "same", "x"),
                descriptor(s, "same", "y")
            ]
        ));
    }

    #[test]
    fn submit_unknown_handle_returns_none() {
        let mut r = registry();
        assert!(r.install(scope("did:plc:a", 1), &[]));
        assert!(r.submit("missing", Instant::now()).is_none());
    }

    #[test]
    fn drain_filters_stale_account_and_auth_generation() {
        let live = scope("did:plc:new", 8);
        let key = |did: &str, generation| AssetIdentityKey {
            scope_did: did.into(),
            auth_generation: generation,
            identity_tag: "test".into(),
        };
        assert!(!completion_matches(&live, &key("did:plc:old", 8)));
        assert!(!completion_matches(&live, &key("did:plc:new", 7)));
        assert!(completion_matches(&live, &key("did:plc:new", 8)));
    }

    #[test]
    fn account_change_clears_installed_descriptors() {
        let mut r = registry();
        let a = scope("did:plc:a", 1);
        assert!(r.install(a.clone(), &[descriptor(a, "h", "x")]));
        assert!(r.install(scope("did:plc:b", 2), &[]));
        assert!(r.submit("h", Instant::now()).is_none());
    }
}
