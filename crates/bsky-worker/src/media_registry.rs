use std::collections::HashMap;
use std::time::{Duration, Instant};

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
    post_handles: HashMap<String, Vec<String>>,
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
            post_handles: HashMap::new(),
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

    /// Install only a coordinator-published page for the live account. Retain
    /// descriptors exactly while their clean posts remain in bounded history.
    pub fn install_accepted(
        &mut self,
        scope: AccountScope,
        projected: &bsky_feed::ProjectedFeed,
        effects: &[bsky_feed::FeedEffect],
    ) -> bool {
        let Some(history) = effects.iter().find_map(|effect| match effect {
            bsky_feed::FeedEffect::PublishDelivery(delivery)
                if delivery.correlation == projected.delivery.correlation =>
            {
                Some(delivery)
            }
            _ => None,
        }) else {
            return false;
        };
        if scope.auth_generation != projected.delivery.correlation.auth_generation
            || !projected.validate_for(&scope.account_did)
        {
            return false;
        }
        let mut posts = if self.scope.as_ref() == Some(&scope)
            && projected.delivery.correlation.cursor.is_some()
        {
            self.post_handles.clone()
        } else {
            HashMap::new()
        };
        posts.retain(|uri, _| {
            history
                .posts
                .iter()
                .any(|p| p.uri == *uri && p.state == bsky_feed::ContentState::Clean)
        });
        for (index, post) in projected.delivery.posts.iter().enumerate() {
            if !history.posts.iter().any(|p| p.uri == post.uri)
                || post.state != bsky_feed::ContentState::Clean
            {
                continue;
            }
            let prefix = format!(
                "asset:{}:{index}:",
                projected.delivery.correlation.request_id
            );
            posts.insert(
                post.uri.clone(),
                projected
                    .native
                    .descriptors
                    .iter()
                    .filter(|d| d.handle.starts_with(&prefix))
                    .map(|d| d.handle.clone())
                    .collect(),
            );
        }
        let mut descriptors: Vec<_> = self
            .descriptors
            .values()
            .filter(|d| posts.values().any(|handles| handles.contains(&d.handle)))
            .cloned()
            .collect();
        for descriptor in &projected.native.descriptors {
            if posts
                .values()
                .any(|handles| handles.contains(&descriptor.handle))
            {
                descriptors.retain(|d| d.handle != descriptor.handle);
                descriptors.push(descriptor.clone());
            }
        }
        if !self.install(scope, &descriptors) {
            return false;
        }
        self.post_handles = posts;
        true
    }

    /// Resolve post URIs in viewport order to bounded native asset slots.
    pub fn visible_handles(&self, uris: &[String], limit: usize) -> Vec<String> {
        uris.iter()
            .filter_map(|uri| self.post_handles.get(uri))
            .flatten()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn post_for_handle(&self, handle: &str) -> Option<&str> {
        self.post_handles
            .iter()
            .find(|(_, handles)| handles.iter().any(|h| h == handle))
            .map(|(uri, _)| uri.as_str())
    }

    /// Resolve a completion back to its structured, installed identity.
    pub fn descriptor_for_identity(
        &self,
        key: &AssetIdentityKey,
    ) -> Option<&NativeAssetDescriptor> {
        self.descriptors.values().find(|d| {
            AssetIdentityKey::from_asset_identity(
                &d.scope.account_did,
                d.scope.auth_generation,
                &d.identity,
            ) == *key
        })
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

    /// Look up the installed descriptor for a handle in the current scope.
    pub fn descriptor(&self, handle: &str) -> Option<&NativeAssetDescriptor> {
        let descriptor = self.descriptors.get(handle)?;
        (descriptor.scope == self.scope.clone()?).then_some(descriptor)
    }

    /// Submit every installed handle from `handles` that is not already in
    /// flight or cached, up to `limit` submissions. Returns the number of
    /// jobs that were newly accepted.
    pub fn submit_visible(
        &self,
        handles: impl IntoIterator<Item = String>,
        deadline: Instant,
        limit: usize,
    ) -> usize {
        let mut accepted = 0;
        for handle in handles {
            if accepted >= limit {
                break;
            }
            if matches!(self.submit(&handle, deadline), Some(Admission::Accepted)) {
                accepted += 1;
            }
        }
        accepted
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
        self.post_handles.clear();
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
    fn accepted_pages_map_visible_posts_retain_history_and_reject_stale_scope() {
        let mut r = registry();
        let s = scope("did:plc:a", 3);
        let mut coordinator = bsky_feed::FeedCoordinator::default();
        coordinator.auth_changed(3);
        coordinator.open_mount();
        let mut first_handle = String::new();
        for page in 0..2 {
            let command = if page == 0 {
                bsky_feed::FeedCommand::FirstPage
            } else {
                bsky_feed::FeedCommand::NextPage
            };
            let effects = coordinator.command(command);
            let corr = effects
                .iter()
                .find_map(|e| match e {
                    bsky_feed::FeedEffect::FetchPage(c) => Some(c.clone()),
                    _ => None,
                })
                .unwrap();
            let post: bsky_feed::PostDto = serde_json::from_value(serde_json::json!({
                "uri": format!("post{page}"), "cid":"cid", "author": {
                    "did":"did:plc:author", "handle":"author.test", "display_name":"Author"},
                "text":"hello", "facets":[], "like_count":0, "repost_count":0,
                "reply_count":0, "state":{"state":"clean"}, "quote":null, "images":[]
            }))
            .unwrap();
            let delivery = bsky_feed::FeedDelivery {
                schema: 1,
                correlation: corr.clone(),
                posts: vec![post],
                cursor: Some("next".into()),
            };
            let handle = format!("asset:{}:0:avatar", corr.request_id);
            if page == 0 {
                first_handle = handle.clone();
            }
            let projected = bsky_feed::ProjectedFeed {
                delivery: delivery.clone(),
                native: bsky_feed::NativeFeedEnvelope {
                    correlation: corr.clone(),
                    descriptors: vec![descriptor(s.clone(), &handle, "revision")],
                },
            };
            assert!(!r.install_accepted(s.clone(), &projected, &[]));
            let accepted = coordinator.complete(&corr, Ok(delivery));
            assert!(!r.install_accepted(scope("did:plc:other", 3), &projected, &accepted));
            assert!(!r.install_accepted(scope("did:plc:a", 4), &projected, &accepted));
            assert!(r.install_accepted(s.clone(), &projected, &accepted));
            assert_eq!(r.visible_handles(&[format!("post{page}")], 8), vec![handle]);
            assert_eq!(
                r.visible_handles(&["post0".into()], 8),
                vec![first_handle.clone()]
            );
            assert!(r.visible_handles(&["unknown".into()], 8).is_empty());
            assert!(r.visible_handles(&["post0".into()], 0).is_empty());
            let stale = coordinator.complete(&corr, Ok(projected.delivery.clone()));
            assert!(!r.install_accepted(s.clone(), &projected, &stale));
        }
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

    #[test]
    fn descriptor_lookup_requires_current_scope() {
        let mut r = registry();
        let a = scope("did:plc:a", 1);
        assert!(r.install(a, &[descriptor(scope("did:plc:a", 1), "h", "rev")]));
        assert_eq!(
            r.descriptor("h").map(|d| d.identity.clone()),
            Some(AssetIdentity::Avatar {
                author_did: "did:plc:author".into(),
                source_revision: "rev".into(),
            })
        );
        assert!(r.descriptor("missing").is_none());
        r.invalidate();
        assert!(r.descriptor("h").is_none());
    }

    #[test]
    fn submit_visible_counts_only_accepted_within_limit() {
        let mut r = registry();
        let s = scope("did:plc:a", 1);
        let descriptors: Vec<_> = (0..4)
            .map(|i| descriptor(s.clone(), &format!("h{i}"), "rev"))
            .collect();
        assert!(r.install(s, &descriptors));
        let deadline = Instant::now() + Duration::from_secs(10);
        let accepted = r.submit_visible(
            vec!["h0".into(), "missing".into(), "h1".into(), "h2".into()],
            deadline,
            2,
        );
        assert!(accepted <= 2, "limit bounds the number of new submissions");
        let zero = r.submit_visible(vec!["h3".into()], deadline, 0);
        assert_eq!(zero, 0);
        let expired = r.submit_visible(vec!["h3".into()], Instant::now(), 4);
        assert_eq!(expired, 0, "an expired deadline is never accepted");
    }
}
