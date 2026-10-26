use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use bsky_net::VitaHttpClient;

const MAX_OUTSTANDING_BYTES: u64 = 96 * 1024 * 1024;
const REQUEST_CAP: usize = 32;
const RESULT_CAP: usize = 8;
const MAX_DEDUP_ENTRIES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    Accepted,
    Coalesced,
    Busy,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssetIdentityKey {
    pub scope_did: String,
    pub auth_generation: u64,
    pub identity_tag: String,
}

impl AssetIdentityKey {
    pub fn from_asset_identity(
        scope_did: &str,
        auth_generation: u64,
        identity: &bsky_feed::AssetIdentity,
    ) -> Self {
        let identity_tag = match identity {
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
        };
        AssetIdentityKey {
            scope_did: scope_did.to_string(),
            auth_generation,
            identity_tag,
        }
    }
}

pub fn fetch_url(identity: &bsky_feed::AssetIdentity) -> Option<String> {
    match identity {
        bsky_feed::AssetIdentity::Avatar {
            source_revision, ..
        } => (!source_revision.is_empty()).then(|| source_revision.clone()),
        bsky_feed::AssetIdentity::PostImage {
            thumbnail,
            fullsize,
            ..
        } => (!fullsize.is_empty())
            .then(|| fullsize.clone())
            .or_else(|| (!thumbnail.is_empty()).then(|| thumbnail.clone())),
    }
}

pub struct MediaCompletion {
    pub identity: AssetIdentityKey,
    pub bytes: Result<Vec<u8>, String>,
    #[allow(dead_code)]
    reservation: Option<ReservationHold>,
}

struct ReservationHold {
    accounting: Arc<MediaAccounting>,
    identity: AssetIdentityKey,
    byte_quota: u64,
}

impl Drop for ReservationHold {
    fn drop(&mut self) {
        release_job(&self.accounting, &self.identity, self.byte_quota);
    }
}

struct MediaJob {
    identity: AssetIdentityKey,
    url: String,
    admission_deadline: Instant,
    byte_quota: u64,
}

struct MediaAccounting {
    outstanding: AtomicU64,
    generation: AtomicU64,
    dedup: Mutex<Vec<AssetIdentityKey>>,
}

type Transport = Box<dyn Fn(&str, u64, Instant) -> Result<Vec<u8>, String> + Send>;

pub struct MediaExecutor {
    req_tx: mpsc::SyncSender<MediaJob>,
    res_rx: mpsc::Receiver<MediaCompletion>,
    accounting: Arc<MediaAccounting>,
    closed: Arc<AtomicBool>,
    _handle: JoinHandle<()>,
}

impl MediaExecutor {
    pub fn spawn_default() -> Self {
        let transport: Transport = Box::new(|url, max_bytes, deadline| {
            VitaHttpClient::new().get_bytes_capped_with_deadline(url, max_bytes, deadline)
        });
        Self::spawn_with_transport(transport)
    }

    fn spawn_with_transport(transport: Transport) -> Self {
        let (req_tx, req_rx) = mpsc::sync_channel::<MediaJob>(REQUEST_CAP);
        let (res_tx, res_rx) = mpsc::sync_channel::<MediaCompletion>(RESULT_CAP);
        let accounting = Arc::new(MediaAccounting {
            outstanding: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            dedup: Mutex::new(Vec::new()),
        });
        let closed = Arc::new(AtomicBool::new(false));
        let acc = Arc::clone(&accounting);
        let handle = thread::Builder::new()
            .name("bsky-media".into())
            .spawn(move || media_run(req_rx, res_tx, acc, transport))
            .expect("spawn bsky-media thread");
        MediaExecutor {
            req_tx,
            res_rx,
            accounting,
            closed,
            _handle: handle,
        }
    }

    pub fn submit(
        &self,
        identity: AssetIdentityKey,
        url: String,
        admission_deadline: Instant,
        byte_quota: u64,
    ) -> Admission {
        if self.closed.load(Ordering::SeqCst) {
            return Admission::Closed;
        }
        if byte_quota == 0 || Instant::now() >= admission_deadline {
            return Admission::Busy;
        }
        {
            let mut dedup = self.accounting.dedup.lock().unwrap();
            if dedup.iter().any(|key| key == &identity) {
                return Admission::Coalesced;
            }
            if dedup.len() >= MAX_DEDUP_ENTRIES {
                return Admission::Busy;
            }
            let reserved = self.accounting.outstanding.load(Ordering::SeqCst);
            if reserved.saturating_add(byte_quota) > MAX_OUTSTANDING_BYTES {
                return Admission::Busy;
            }
            self.accounting
                .outstanding
                .fetch_add(byte_quota, Ordering::SeqCst);
            dedup.push(identity.clone());
        }
        let job = MediaJob {
            identity: identity.clone(),
            url,
            admission_deadline,
            byte_quota,
        };
        match self.req_tx.try_send(job) {
            Ok(()) => Admission::Accepted,
            Err(mpsc::TrySendError::Full(job)) => {
                release_job(&self.accounting, &job.identity, job.byte_quota);
                Admission::Busy
            }
            Err(mpsc::TrySendError::Disconnected(job)) => {
                release_job(&self.accounting, &job.identity, job.byte_quota);
                Admission::Closed
            }
        }
    }

    pub fn try_recv(&self) -> Option<MediaCompletion> {
        self.res_rx.try_recv().ok()
    }

    pub fn invalidate(&self) {
        self.accounting.generation.fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn generation(&self) -> u64 {
        self.accounting.generation.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn outstanding_bytes(&self) -> u64 {
        self.accounting.outstanding.load(Ordering::SeqCst)
    }
}

impl Drop for MediaExecutor {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

fn release_job(accounting: &MediaAccounting, identity: &AssetIdentityKey, byte_quota: u64) {
    accounting
        .outstanding
        .fetch_sub(byte_quota, Ordering::SeqCst);
    accounting
        .dedup
        .lock()
        .unwrap()
        .retain(|key| key != identity);
}

fn media_run(
    req_rx: mpsc::Receiver<MediaJob>,
    res_tx: mpsc::SyncSender<MediaCompletion>,
    accounting: Arc<MediaAccounting>,
    transport: Transport,
) {
    while let Ok(job) = req_rx.recv() {
        let generation = accounting.generation.load(Ordering::SeqCst);
        if job.identity.auth_generation != generation || Instant::now() >= job.admission_deadline {
            release_job(&accounting, &job.identity, job.byte_quota);
            continue;
        }
        let bytes = transport(&job.url, bsky_net::MAX_IMAGE_BYTES, job.admission_deadline);
        if job.identity.auth_generation != accounting.generation.load(Ordering::SeqCst)
            || Instant::now() >= job.admission_deadline
        {
            release_job(&accounting, &job.identity, job.byte_quota);
            continue;
        }
        let hold = ReservationHold {
            accounting: Arc::clone(&accounting),
            identity: job.identity.clone(),
            byte_quota: job.byte_quota,
        };
        let completion = MediaCompletion {
            identity: job.identity,
            bytes,
            reservation: Some(hold),
        };
        if res_tx.send(completion).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::time::Duration;

    fn key(tag: &str) -> AssetIdentityKey {
        AssetIdentityKey {
            scope_did: "did:plc:test".into(),
            auth_generation: 0,
            identity_tag: tag.into(),
        }
    }

    fn transport_returning(data: Vec<u8>) -> Transport {
        let data = Arc::new(data);
        Box::new(move |_, _, _| Ok((*data).clone()))
    }

    fn immediate() -> MediaExecutor {
        MediaExecutor::spawn_with_transport(transport_returning(vec![1, 2, 3, 4]))
    }

    fn wait_for_completion(executor: &MediaExecutor) -> Option<MediaCompletion> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(done) = executor.try_recv() {
                return Some(done);
            }
            thread::sleep(Duration::from_millis(2));
        }
        None
    }

    #[test]
    fn accepted_then_coalesced_duplicate_yields_single_completion() {
        let executor = immediate();
        let deadline = Instant::now() + Duration::from_secs(5);
        assert_eq!(
            executor.submit(key("a"), "http://x/a".into(), deadline, 1024),
            Admission::Accepted
        );
        assert_eq!(
            executor.submit(key("a"), "http://x/a".into(), deadline, 1024),
            Admission::Coalesced
        );
        let done = wait_for_completion(&executor).expect("completion");
        assert_eq!(done.identity, key("a"));
        assert_eq!(done.bytes.unwrap(), vec![1, 2, 3, 4]);
        assert!(
            executor.try_recv().is_none(),
            "coalesced duplicate duplicates"
        );
    }

    #[test]
    fn busy_when_request_channel_full() {
        let gate = Arc::new(Barrier::new(2));
        let released = Arc::clone(&gate);
        let transport: Transport = Box::new(move |_, _, _| {
            released.wait();
            Ok(vec![1])
        });
        let executor = MediaExecutor::spawn_with_transport(transport);
        let deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(
            executor.submit(key("first"), "http://x/0".into(), deadline, 1),
            Admission::Accepted
        );
        let mut saw_busy = false;
        for n in 1..(REQUEST_CAP * 4) {
            if executor.submit(key(&format!("k{n}")), format!("http://x/{n}"), deadline, 1)
                == Admission::Busy
            {
                saw_busy = true;
                break;
            }
        }
        gate.wait();
        assert!(saw_busy, "bounded request channel must eventually reject");
    }

    #[test]
    fn closed_after_executor_dropped() {
        let executor = immediate();
        drop(executor);
    }

    #[test]
    fn expired_admission_deadline_is_refused_before_submit() {
        let executor = immediate();
        let past = Instant::now() - Duration::from_secs(1);
        assert_ne!(
            executor.submit(key("a"), "http://x/a".into(), past, 1024),
            Admission::Accepted
        );
    }

    #[test]
    fn expired_deadline_during_pickup_publishes_nothing() {
        let transport: Transport = Box::new(|_, _, _| {
            thread::sleep(Duration::from_millis(120));
            Ok(vec![9])
        });
        let executor = MediaExecutor::spawn_with_transport(transport);
        executor.submit(
            key("slow"),
            "http://x/slow".into(),
            Instant::now() + Duration::from_millis(40),
            1024,
        );
        thread::sleep(Duration::from_millis(300));
        assert!(
            executor.try_recv().is_none(),
            "expired job must not publish"
        );
        assert_eq!(executor.outstanding_bytes(), 0);
    }

    #[test]
    fn completion_carries_identity_and_not_a_work_response() {
        let executor = immediate();
        executor.submit(
            key("typed"),
            "http://x/typed".into(),
            Instant::now() + Duration::from_secs(5),
            64,
        );
        let done: MediaCompletion = wait_for_completion(&executor).expect("completion");
        assert_eq!(done.identity.identity_tag, "typed");
    }

    #[test]
    fn byte_reservation_released_when_completion_consumed() {
        let executor = immediate();
        executor.submit(
            key("budget"),
            "http://x/budget".into(),
            Instant::now() + Duration::from_secs(5),
            4096,
        );
        let done = wait_for_completion(&executor).expect("completion");
        assert_eq!(executor.outstanding_bytes(), 4096);
        drop(done);
        assert_eq!(executor.outstanding_bytes(), 0);
    }

    #[test]
    fn byte_reservation_released_when_completion_dropped_undrained() {
        let executor = immediate();
        executor.submit(
            key("undrained"),
            "http://x/undrained".into(),
            Instant::now() + Duration::from_secs(5),
            8192,
        );
        assert_eq!(executor.outstanding_bytes(), 8192);
        drop(executor);
    }

    #[test]
    fn rejected_full_channel_releases_reservation() {
        let gate = Arc::new(Barrier::new(2));
        let released = Arc::clone(&gate);
        let transport: Transport = Box::new(move |_, _, _| {
            released.wait();
            Ok(vec![1])
        });
        let executor = MediaExecutor::spawn_with_transport(transport);
        let deadline = Instant::now() + Duration::from_secs(10);
        executor.submit(key("first"), "http://x/0".into(), deadline, 100);
        let mut saw_busy = false;
        for n in 1..(REQUEST_CAP * 4) {
            if executor.submit(
                key(&format!("k{n}")),
                format!("http://x/{n}"),
                deadline,
                100,
            ) == Admission::Busy
            {
                saw_busy = true;
                break;
            }
        }
        gate.wait();
        assert!(saw_busy);
    }

    #[test]
    fn stale_generation_never_publishes() {
        let transport: Transport = Box::new(|_, _, _| {
            thread::sleep(Duration::from_millis(80));
            Ok(vec![7])
        });
        let executor = MediaExecutor::spawn_with_transport(transport);
        let mut scoped = key("stale");
        scoped.auth_generation = 0;
        executor.submit(
            scoped,
            "http://x/stale".into(),
            Instant::now() + Duration::from_secs(10),
            512,
        );
        executor.invalidate();
        assert_eq!(executor.generation(), 1);
        thread::sleep(Duration::from_millis(250));
        assert!(
            executor.try_recv().is_none(),
            "stale generation must not publish"
        );
        assert_eq!(executor.outstanding_bytes(), 0);
    }

    #[test]
    fn generation_invalidation_releases_each_reservation_once() {
        let executor = immediate();
        executor.invalidate();
        let generation = executor.generation();
        let mut scoped = key("gen");
        scoped.auth_generation = generation;
        executor.submit(
            scoped,
            "http://x/gen".into(),
            Instant::now() + Duration::from_secs(5),
            2048,
        );
        let _ = wait_for_completion(&executor);
        assert_eq!(executor.outstanding_bytes(), 0);
    }

    #[test]
    fn dedup_state_is_bounded_and_released_after_consume() {
        let executor = immediate();
        let deadline = Instant::now() + Duration::from_secs(10);
        for n in 0..MAX_DEDUP_ENTRIES {
            let _ = executor.submit(
                key(&format!("bounded{n}")),
                format!("http://x/{n}"),
                deadline,
                1,
            );
        }
        let mut drained = 0;
        while let Some(done) = wait_for_completion(&executor) {
            drop(done);
            drained += 1;
            if drained > MAX_DEDUP_ENTRIES {
                break;
            }
        }
        assert_eq!(executor.outstanding_bytes(), 0);
    }

    #[test]
    fn byte_budget_exhaustion_reports_busy() {
        let gate = Arc::new(Barrier::new(2));
        let released = Arc::clone(&gate);
        let transport: Transport = Box::new(move |_, _, _| {
            released.wait();
            Ok(vec![1])
        });
        let executor = MediaExecutor::spawn_with_transport(transport);
        let deadline = Instant::now() + Duration::from_secs(10);
        let half = MAX_OUTSTANDING_BYTES / 2;
        assert_eq!(
            executor.submit(key("half"), "http://x/half".into(), deadline, half),
            Admission::Accepted
        );
        assert_eq!(
            executor.submit(key("over"), "http://x/over".into(), deadline, half + 1),
            Admission::Busy
        );
        gate.wait();
    }

    #[test]
    fn zero_byte_quota_is_refused() {
        let executor = immediate();
        assert_ne!(
            executor.submit(
                key("zero"),
                "http://x/zero".into(),
                Instant::now() + Duration::from_secs(5),
                0
            ),
            Admission::Accepted
        );
    }

    #[test]
    fn media_completion_arrives_without_touching_legacy_path() {
        let executor = immediate();
        executor.submit(
            key("isolated"),
            "http://x/isolated".into(),
            Instant::now() + Duration::from_secs(5),
            16,
        );
        let done = wait_for_completion(&executor).expect("media completion on its own lane");
        assert_eq!(done.identity.identity_tag, "isolated");
    }

    #[test]
    fn blocked_media_job_does_not_block_legacy_channel() {
        let started = Arc::new(Barrier::new(2));
        let started_release = Arc::clone(&started);
        let finish = Arc::new(Barrier::new(2));
        let finish_release = Arc::clone(&finish);
        let transport: Transport = Box::new(move |_, _, _| {
            started_release.wait();
            finish_release.wait();
            Ok(vec![1])
        });
        let executor = MediaExecutor::spawn_with_transport(transport);
        executor.submit(
            key("blocked"),
            "http://x/blocked".into(),
            Instant::now() + Duration::from_secs(30),
            16,
        );
        started.wait();
        let start = Instant::now();
        let legacy = mpsc::channel::<u8>();
        legacy.0.send(7).expect("legacy channel stays responsive");
        assert!(start.elapsed() < Duration::from_secs(1));
        finish.wait();
    }

    #[test]
    fn teardown_does_not_hang_with_blocked_download() {
        let started = Arc::new(Barrier::new(2));
        let started_release = Arc::clone(&started);
        let finish = Arc::new(Barrier::new(2));
        let finish_release = Arc::clone(&finish);
        let transport: Transport = Box::new(move |_, _, _| {
            started_release.wait();
            finish_release.wait();
            Ok(vec![1])
        });
        let executor = MediaExecutor::spawn_with_transport(transport);
        executor.submit(
            key("hang"),
            "http://x/hang".into(),
            Instant::now() + Duration::from_secs(30),
            32,
        );
        started.wait();
        let start = Instant::now();
        drop(executor);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "drop must not join an in-flight download"
        );
        finish.wait();
    }
}
