//! Account-scoped disk cache for fetched media bytes. Cache freshness is
//! independent of availability: stale avatars remain readable offline.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bsky_worker::media_executor::AssetIdentityKey;

pub const AVATAR_QUOTA_BYTES: u64 = 64 * 1024 * 1024;
const FOLLOWED_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const OTHER_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const POST_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    Avatar { followed: bool },
    PostImage { persist: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheRead {
    /// Whether a caller should schedule a network refresh. This does not
    /// determine whether available bytes are returned.
    pub fresh: bool,
}

pub struct AssetCache {
    root: PathBuf,
    avatar_quota: u64,
}

impl AssetCache {
    /// Account DID is embedded in every disk key; switching accounts is a
    /// cache miss rather than serving bytes across account scopes.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            avatar_quota: AVATAR_QUOTA_BYTES,
        }
    }

    pub fn with_avatar_quota(root: impl Into<PathBuf>, quota: u64) -> Self {
        Self {
            root: root.into(),
            avatar_quota: quota,
        }
    }

    /// Read bytes if present and valid. Returns freshness separately; stale
    /// avatar data is intentionally retained for offline use.
    pub fn get(
        &self,
        key: &AssetIdentityKey,
        kind: AssetKind,
        now: SystemTime,
    ) -> io::Result<Option<(Vec<u8>, CacheRead)>> {
        if matches!(kind, AssetKind::PostImage { persist: false }) {
            return Ok(None);
        }
        let path = self.entry_path(key);
        let stored = match fs::read(&path) {
            Ok(bytes) if bytes.len() >= 8 => bytes,
            Ok(_) => {
                let _ = fs::remove_file(path);
                return Ok(None);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                let _ = fs::remove_file(path);
                return Ok(None);
            }
        };
        let expected = u64::from_le_bytes(stored[..8].try_into().unwrap()) as usize;
        if stored.len() != expected.saturating_add(8) {
            let _ = fs::remove_file(path);
            return Ok(None);
        }
        let bytes = stored[8..].to_vec();
        let modified = match fs::metadata(&path) {
            Ok(metadata) => metadata.modified().unwrap_or(UNIX_EPOCH),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                let _ = fs::remove_file(path);
                return Ok(None);
            }
        };
        let ttl = ttl_for(kind);
        let fresh = now.duration_since(modified).unwrap_or_default() < ttl;
        // Successful reads update the LRU timestamp. The file is already
        // fully read into owned bytes, so eviction cannot invalidate callers.
        if let Ok(file) = fs::File::open(&path) {
            let _ = file.set_times(fs::FileTimes::new().set_modified(SystemTime::now()));
        }
        Ok(Some((bytes, CacheRead { fresh })))
    }

    pub fn put(&self, key: &AssetIdentityKey, kind: AssetKind, bytes: &[u8]) -> io::Result<()> {
        if matches!(kind, AssetKind::PostImage { persist: false }) {
            return Ok(());
        }
        fs::create_dir_all(&self.root)?;
        let path = self.entry_path(key);
        let tmp = sidecar(&path);
        let result: io::Result<()> = (|| {
            let mut payload = Vec::with_capacity(bytes.len() + 8);
            payload.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            payload.extend_from_slice(bytes);
            fs::write(&tmp, payload)?;
            if path.exists() {
                fs::remove_file(&path)?;
            }
            fs::rename(&tmp, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        self.evict()?;
        Ok(())
    }

    fn entry_path(&self, key: &AssetIdentityKey) -> PathBuf {
        // Hex encoding prevents path traversal and safely incorporates all
        // account, generation, and source-revision identity components.
        let raw = format!(
            "{}\0{}\0{}",
            key.scope_did, key.auth_generation, key.identity_tag
        );
        let name: String = raw.bytes().map(|b| format!("{b:02x}")).collect();
        self.root.join(format!("{name}.media"))
    }

    fn evict(&self) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        let mut rows = Vec::new();
        let mut total = 0u64;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("media") {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(m) if m.is_file() => m,
                _ => continue,
            };
            let len = metadata.len();
            total = total.saturating_add(len);
            rows.push((metadata.modified().unwrap_or(UNIX_EPOCH), path, len));
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        for (_, path, len) in rows {
            if total <= self.avatar_quota {
                break;
            }
            // Only cache payloads are eligible; .tmp and unrelated files are
            // ignored, so active atomic writes are never evicted.
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
        Ok(())
    }
}

fn sidecar(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.tmp", path.display()))
}
fn ttl_for(kind: AssetKind) -> Duration {
    match kind {
        AssetKind::Avatar { followed: true } => FOLLOWED_TTL,
        AssetKind::Avatar { followed: false } => OTHER_TTL,
        AssetKind::PostImage { .. } => POST_TTL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn key(revision: &str) -> AssetIdentityKey {
        AssetIdentityKey {
            scope_did: "did:plc:account".into(),
            auth_generation: 1,
            identity_tag: format!("avatar:did:plc:author:{revision}"),
        }
    }
    fn cache(dir: &Path) -> AssetCache {
        AssetCache::with_avatar_quota(dir, 8)
    }

    #[test]
    fn write_then_read_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let c = AssetCache::new(d.path());
        let k = key("r1");
        c.put(
            &k,
            AssetKind::Avatar { followed: false },
            b"avatar-bytes-valid",
        )
        .unwrap();
        assert_eq!(
            c.get(&k, AssetKind::Avatar { followed: false }, SystemTime::now())
                .unwrap()
                .unwrap()
                .0,
            b"avatar-bytes-valid"
        );
    }

    #[test]
    fn ttl_expiry_is_stale_but_bytes_remain_readable() {
        let d = tempfile::tempdir().unwrap();
        let c = AssetCache::new(d.path());
        let k = key("r1");
        c.put(
            &k,
            AssetKind::Avatar { followed: false },
            b"avatar-bytes-valid",
        )
        .unwrap();
        let got = c
            .get(
                &k,
                AssetKind::Avatar { followed: false },
                SystemTime::now() + OTHER_TTL + Duration::from_secs(1),
            )
            .unwrap()
            .unwrap();
        assert!(!got.1.fresh);
        assert_eq!(got.0, b"avatar-bytes-valid");
    }

    #[test]
    fn followed_avatar_has_longer_freshness_ttl() {
        assert!(
            ttl_for(AssetKind::Avatar { followed: true })
                > ttl_for(AssetKind::Avatar { followed: false })
        );
    }

    #[test]
    fn eviction_removes_lru_and_observes_quota() {
        let d = tempfile::tempdir().unwrap();
        let c = AssetCache::with_avatar_quota(d.path(), 20);
        let a = key("a");
        let b = key("b");
        c.put(&a, AssetKind::Avatar { followed: false }, b"123456")
            .unwrap();
        sleep(Duration::from_millis(20));
        c.put(&b, AssetKind::Avatar { followed: false }, b"abcdef")
            .unwrap();
        let used: u64 = fs::read_dir(d.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        assert!(used <= 20);
        assert!(c
            .get(&a, AssetKind::Avatar { followed: false }, SystemTime::now())
            .unwrap()
            .is_none());
        assert!(c
            .get(&b, AssetKind::Avatar { followed: false }, SystemTime::now())
            .unwrap()
            .is_some());
    }

    #[test]
    fn stale_avatar_bytes_are_available_without_scheduling_fetch() {
        let d = tempfile::tempdir().unwrap();
        let c = AssetCache::new(d.path());
        let k = key("same");
        c.put(&k, AssetKind::Avatar { followed: false }, b"cached-image")
            .unwrap();
        let cached = c
            .get(
                &k,
                AssetKind::Avatar { followed: false },
                SystemTime::now() + OTHER_TTL,
            )
            .unwrap()
            .unwrap();
        assert!(!cached.1.fresh);
        assert_eq!(cached.0, b"cached-image");
    }

    #[test]
    fn changed_identity_is_a_miss() {
        let d = tempfile::tempdir().unwrap();
        let c = AssetCache::new(d.path());
        c.put(
            &key("old"),
            AssetKind::Avatar { followed: false },
            b"avatar-bytes-valid",
        )
        .unwrap();
        assert!(c
            .get(
                &key("new"),
                AssetKind::Avatar { followed: false },
                SystemTime::now()
            )
            .unwrap()
            .is_none());
    }

    #[test]
    fn corrupt_entry_is_cleaned_as_miss() {
        let d = tempfile::tempdir().unwrap();
        let c = AssetCache::new(d.path());
        let k = key("bad");
        fs::create_dir_all(d.path()).unwrap();
        fs::write(c.entry_path(&k), b"short").unwrap();
        assert!(c
            .get(&k, AssetKind::Avatar { followed: false }, SystemTime::now())
            .unwrap()
            .is_none());
        assert!(!c.entry_path(&k).exists());
    }

    #[test]
    fn temporary_sidecar_is_not_recovered_or_served() {
        let d = tempfile::tempdir().unwrap();
        let c = AssetCache::new(d.path());
        let k = key("tmp");
        fs::create_dir_all(d.path()).unwrap();
        fs::write(sidecar(&c.entry_path(&k)), b"avatar-bytes-valid").unwrap();
        assert!(c
            .get(&k, AssetKind::Avatar { followed: false }, SystemTime::now())
            .unwrap()
            .is_none());
    }

    #[test]
    fn unrelated_files_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let c = cache(d.path());
        fs::write(d.path().join("notes.txt"), b"unrelated").unwrap();
        let k = key("keep");
        c.put(&k, AssetKind::Avatar { followed: false }, b"123456")
            .unwrap();
        assert!(d.path().join("notes.txt").exists());
    }
}
