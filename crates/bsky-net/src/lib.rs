//! XRPC HTTP client for the Vita target.
//!
//! Implements [`atrium_xrpc::HttpClient`] over our sync `ureq` + `rustls` +
//! `ring` (vita-rust patch) + `webpki-roots` stack. The trait's `send_http`
//! returns `impl Future<Output = ...> + Send`; the body of our impl runs
//! synchronously inside an `async move` block, so the type signature is
//! satisfied without bringing in tokio or any other async runtime.
//!
//! Consumers drive the returned future with [`futures::executor::block_on`]
//! (or any other executor that can poll a single Send-bound future).

use std::error::Error;
use std::io::{Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

thread_local! {
    static REQUEST_DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    #[cfg(test)]
    static TEST_SEND_TIMEOUTS: std::cell::RefCell<Vec<Duration>> = const { std::cell::RefCell::new(Vec::new()) };
    #[cfg(test)]
    static TEST_BODY_BYTES: std::sync::atomic::AtomicUsize = const { std::sync::atomic::AtomicUsize::new(0) };
}

#[cfg(test)]
static TEST_RESOLVER_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static TEST_NETWORK_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
static TEST_DNS_LOOKUP: OnceLock<
    Mutex<Option<std::sync::Arc<dyn Fn(&str) -> std::io::Result<Vec<SocketAddr>> + Send + Sync>>>,
> = OnceLock::new();

/// Run work under one absolute XRPC request deadline on the current thread.
/// Nested scopes preserve the earlier deadline and restore the prior scope.
pub fn with_request_deadline<T>(duration: Duration, f: impl FnOnce() -> T) -> T {
    with_absolute_request_deadline(Instant::now() + duration, f)
}

/// Run work under a caller-admitted absolute request deadline.
pub fn with_absolute_request_deadline<T>(deadline: Instant, f: impl FnOnce() -> T) -> T {
    REQUEST_DEADLINE.with(|slot| {
        let previous = slot.get();
        let requested = deadline;
        slot.set(Some(previous.map_or(requested, |old| old.min(requested))));
        struct Restore<'a>(&'a std::cell::Cell<Option<Instant>>, Option<Instant>);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _restore = Restore(slot, previous);
        f()
    })
}

const DNS_THREADS: usize = 2;
const DNS_QUEUE: usize = 8;
const DNS_CACHE: usize = 64;

type DnsJob = (String, mpsc::SyncSender<Result<Vec<SocketAddr>, String>>);
struct DnsPool {
    sender: mpsc::SyncSender<DnsJob>,
    cache: Mutex<Vec<(String, Vec<SocketAddr>)>>,
}
static DNS_POOL: OnceLock<DnsPool> = OnceLock::new();

fn dns_pool() -> &'static DnsPool {
    DNS_POOL.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<DnsJob>(DNS_QUEUE);
        let receiver = std::sync::Arc::new(Mutex::new(receiver));
        for _ in 0..DNS_THREADS {
            let receiver = receiver.clone();
            std::thread::spawn(move || loop {
                let job = receiver.lock().unwrap().recv();
                let Ok((hostport, reply)) = job else { break };
                #[cfg(test)]
                let injected = {
                    TEST_RESOLVER_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    TEST_DNS_LOOKUP
                        .get_or_init(|| Mutex::new(None))
                        .lock()
                        .unwrap()
                        .clone()
                };
                #[cfg(test)]
                let result = if let Some(lookup) = injected {
                    lookup(&hostport)
                } else {
                    hostport
                        .to_socket_addrs()
                        .map(|addresses| addresses.collect::<Vec<_>>())
                };
                #[cfg(not(test))]
                let result = hostport
                    .to_socket_addrs()
                    .map(|addresses| addresses.collect::<Vec<_>>());
                let result = result.map_err(|error| error.to_string());
                let _ = reply.send(result);
            });
        }
        DnsPool {
            sender,
            cache: Mutex::new(Vec::new()),
        }
    })
}

const MAX_CONNECT_SLICE: Duration = Duration::from_secs(15);

fn check_connect_budget(
    deadline: Option<Instant>,
    connect_allowance: Option<Duration>,
) -> std::io::Result<()> {
    if let Some(deadline) = deadline {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "request deadline exceeded")
            })?;
        if let Some(connect_allowance) = connect_allowance {
            if remaining < connect_allowance {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "insufficient request time remaining for connection attempt",
                ));
            }
        }
    }
    Ok(())
}

fn resolve_host(hostport: &str) -> std::io::Result<Vec<SocketAddr>> {
    let deadline = REQUEST_DEADLINE.with(|slot| slot.get());
    let connect_allowance = deadline.and_then(|deadline| {
        deadline
            .checked_duration_since(Instant::now())
            .map(|remaining| (remaining / 2).min(MAX_CONNECT_SLICE))
    });
    check_connect_budget(deadline, connect_allowance)?;
    let pool = dns_pool();
    if let Some(addresses) = pool
        .cache
        .lock()
        .unwrap()
        .iter()
        .find(|(key, _)| key == hostport)
        .map(|(_, addresses)| addresses.clone())
    {
        check_connect_budget(deadline, connect_allowance)?;
        return Ok(addresses);
    }
    let (reply, result) = mpsc::sync_channel(1);
    pool.sender
        .try_send((hostport.to_owned(), reply))
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::WouldBlock, "DNS resolver saturated")
        })?;
    let value = match deadline {
        Some(deadline) => result.recv_timeout(
            deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "request deadline exceeded during DNS",
                    )
                })?,
        ),
        None => result
            .recv()
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
    }
    .map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("DNS resolution unavailable: {error}"),
        )
    })?;
    let addresses = value.map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error))?;
    check_connect_budget(deadline, connect_allowance)?;
    let mut cache = pool.cache.lock().unwrap();
    if cache.len() >= DNS_CACHE {
        cache.remove(0);
    }
    cache.push((hostport.to_owned(), addresses.clone()));
    Ok(addresses)
}

fn remaining_request_time() -> Result<Option<Duration>, std::io::Error> {
    REQUEST_DEADLINE.with(|slot| match slot.get() {
        None => Ok(None),
        Some(deadline) => deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .map(Some)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "request deadline exceeded")
            }),
    })
}

use atrium_xrpc::http::{Request, Response};
use atrium_xrpc::HttpClient;

/// Hard ceilings on response sizes, enforced by the capped readers below.
/// The Vita shares ~512 MB of RAM with the OS, so an unbounded body from a
/// hostile or buggy server is a real OOM vector — every read is capped.
/// These are coarse safety ceilings, not tight per-asset budgets: finer
/// per-type limits (avatar vs thumb vs full image) would require threading an
/// asset kind through the worker's `FetchImage` request and touching every UI
/// call site, which is deliberately out of scope here.
pub const MAX_XRPC_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: u64 = 24 * 1024 * 1024;
pub const MAX_VIDEO_BYTES: u64 = 120 * 1024 * 1024;

/// A blocking HTTP client suitable for the Vita target.
///
/// Wraps a `ureq::Agent` (which is internally `Arc`-shared, so cloning is
/// cheap and lets us hand a copy into the per-request `async move` block).
pub struct VitaHttpClient {
    agent: ureq::Agent,
}

impl Default for VitaHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl VitaHttpClient {
    pub fn new() -> Self {
        // Inactivity timeouts, NOT an overall deadline. `timeout_read` /
        // `timeout_write` bound how long a single socket op may stall with
        // no progress; as long as bytes keep flowing the clock resets. A
        // previous overall `.timeout(45s)` killed large-but-progressing
        // downloads (a 20 MB video on slow wifi exceeds 45s even while
        // steadily transferring) — fatal for video blobs, which are read
        // whole into memory. With inactivity timeouts a slow steady
        // download finishes; only a genuinely stalled connection errors.
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(15))
            .timeout_read(std::time::Duration::from_secs(30))
            .timeout_write(std::time::Duration::from_secs(30))
            .build();
        Self { agent }
    }

    /// GET `url` into memory, capped at `max_bytes`. Synchronous (ureq
    /// blocks) and auth-free — used for image fetches against the CDN.
    /// Returns an error rather than OOMing if the server advertises (via
    /// Content-Length) or streams more than `max_bytes`.
    pub fn get_bytes_capped(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        let resp = match self.agent.get(url).call() {
            Ok(r) => r,
            // ureq classifies HTTP >= 400 as Err(Status); surface the code.
            Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code}")),
            Err(e) => return Err(format!("{e}")),
        };
        let total = content_length(&resp);
        read_capped(resp.into_reader(), max_bytes, total, |_, _| {})
    }

    /// GET `url` into memory, capped at `max_bytes`, with an absolute
    /// deadline that covers DNS, connection, headers, and the body read.
    /// Installs per-operation transport timeouts (`timeout_connect`,
    /// `timeout_read`, `timeout_write`, overall `.timeout()`) derived from
    /// the remaining time, and prevents implicit redirect-following so
    /// every redirect also observes the deadline. The scoped-deadline
    /// mechanism (see `with_absolute_request_deadline`) is occupied on the
    /// calling thread for the duration of this call; any pre-existing scope
    /// is restored on return. Returns a clear error when the deadline has
    /// already expired before any I/O happens.
    pub fn get_bytes_capped_with_deadline(
        &self,
        url: &str,
        max_bytes: u64,
        deadline: std::time::Instant,
    ) -> Result<Vec<u8>, String> {
        with_absolute_request_deadline(deadline, || {
            let remaining = remaining_request_time()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "deadline already expired".to_string())?;

            let scoped_agent = ureq::AgentBuilder::new()
                .resolver(resolve_host)
                .timeout_connect(remaining.min(MAX_CONNECT_SLICE))
                .timeout_read(remaining)
                .timeout_write(remaining)
                .redirects(0)
                .build();
            let mut req = scoped_agent.get(url).timeout(remaining);

            // Apply Host header for the resolver's benefit (same approach
            // the scoped agent in send_http relies on).
            if let Some(host) = url_host(url) {
                req = req.set("Host", &host);
            }

            let resp = match req.call() {
                Ok(r) => r,
                Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code}")),
                Err(e) => return Err(format!("{e}")),
            };

            remaining_request_time()
                .map_err(|e| e.to_string())?;

            let total = content_length(&resp);
            read_capped(resp.into_reader(), max_bytes, total, |_, _| {})
        })
    }

    /// Stream `url` to `dest`, capped at `max_bytes`, forwarding throttled
    /// progress via `on_progress(downloaded, total)`. The body is written
    /// straight to a `<dest>.part` sidecar in 64 KB chunks — it never sits
    /// whole in memory — and renamed to `dest` only after a complete,
    /// within-cap download. The partial file is removed on any error, so a
    /// half-written file is never mistaken for a complete cache entry.
    /// Synchronous and auth-free; used for video blobs (tens of MB).
    pub fn download_to_file_with_progress(
        &self,
        url: &str,
        dest: &std::path::Path,
        max_bytes: u64,
        on_progress: impl FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        let resp = match self.agent.get(url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code}")),
            Err(e) => return Err(format!("{e}")),
        };
        let total = content_length(&resp);
        if let Some(t) = total {
            if t > max_bytes {
                return Err(format!(
                    "body too large: Content-Length {t} > cap {max_bytes}"
                ));
            }
        }
        // Stream straight to disk via a `.part` sidecar, capped, then install.
        stream_to_file_capped(resp.into_reader(), dest, max_bytes, total, on_progress)
    }
}

impl HttpClient for VitaHttpClient {
    fn send_http(
        &self,
        request: Request<Vec<u8>>,
    ) -> impl std::future::Future<
        Output = Result<Response<Vec<u8>>, Box<dyn Error + Send + Sync + 'static>>,
    > + Send {
        let agent = self.agent.clone();
        async move {
            let (parts, body) = request.into_parts();
            let url = parts.uri.to_string();
            let method = parts.method.as_str();

            let remaining = remaining_request_time()?;
            #[cfg(test)]
            if let Some(timeout) = remaining {
                TEST_SEND_TIMEOUTS.with(|timeouts| timeouts.borrow_mut().push(timeout));
            }
            let scoped_agent;
            let agent = if let Some(remaining) = remaining {
                scoped_agent = ureq::AgentBuilder::new()
                    .resolver(resolve_host)
                    .timeout_connect(remaining.min(MAX_CONNECT_SLICE))
                    .timeout_read(remaining)
                    .timeout_write(remaining)
                    .redirects(0)
                    .build();
                &scoped_agent
            } else {
                &agent
            };
            let mut req = agent.request(method, &url);
            if let Some(remaining) = remaining {
                req = req.timeout(remaining);
            }
            for (name, value) in parts.headers.iter() {
                req = req.set(name.as_str(), value.to_str()?);
            }

            // ureq classifies HTTP >=400 as `Err(Status)`, but we want to pass
            // those through as a normal `Response` — atrium handles 401 via
            // its session-refresh path and other 4xx/5xx bubble up to callers
            // as XRPC errors with the body intact.
            let resp = match if body.is_empty() {
                req.call()
            } else {
                req.send_bytes(&body)
            } {
                Ok(r) => r,
                Err(ureq::Error::Status(_, r)) => r,
                Err(e) => return Err(Box::new(e) as Box<dyn Error + Send + Sync + 'static>),
            };

            let status = resp.status();
            let mut builder = Response::builder().status(status);
            for name in resp.headers_names() {
                if let Some(value) = resp.header(&name) {
                    builder = builder.header(name, value);
                }
            }

            // Cap XRPC response bodies — a runaway or hostile response must
            // not be read unbounded into memory on a 512 MB device.
            let total = content_length(&resp);
            let body_bytes = read_capped(resp.into_reader(), MAX_XRPC_BYTES, total, |_, _| {})
                .map_err(Box::<dyn Error + Send + Sync + 'static>::from)?;
            remaining_request_time()?;
            Ok(builder.body(body_bytes)?)
        }
    }
}

/// `Content-Length` header as a `u64`, if present and parseable.
fn content_length(resp: &ureq::Response) -> Option<u64> {
    resp.header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
}

/// Extract the `Host` header value from a URL: `host:port` for non-default
/// ports, `host` otherwise. Returns `None` if the URL has no authority.
fn url_host(url: &str) -> Option<String> {
    let authority = url
        .split("://")
        .nth(1)?
        .split('/')
        .next()?;
    Some(authority.to_owned())
}

/// Read `reader` fully into a `Vec`, but never more than `max_bytes`.
/// Rejects up front if `total` (Content-Length) already exceeds the cap, and
/// aborts mid-stream if the body grows past it — so a missing or lying
/// Content-Length cannot lead to unbounded allocation. Forwards
/// `on_progress(downloaded, total)` as bytes arrive.
fn read_capped(
    mut reader: impl Read,
    max_bytes: u64,
    total: Option<u64>,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    if let Some(t) = total {
        if t > max_bytes {
            return Err(format!(
                "body too large: Content-Length {t} > cap {max_bytes}"
            ));
        }
    }
    let mut out: Vec<u8> = Vec::new();
    if let Some(t) = total {
        out.reserve(t.min(max_bytes) as usize);
    }
    let mut buf = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if REQUEST_DEADLINE.with(|slot| slot.get()).is_some()
                    && remaining_request_time()
                        .map_err(|error| error.to_string())?
                        .is_none()
                {
                    unreachable!("scoped deadline returns Some while active");
                }
                if out.len() as u64 + n as u64 > max_bytes {
                    return Err(format!("body exceeded cap of {max_bytes} bytes"));
                }
                out.extend_from_slice(&buf[..n]);
                #[cfg(test)]
                if REQUEST_DEADLINE.with(|slot| slot.get()).is_some() {
                    TEST_BODY_BYTES
                        .with(|bytes| bytes.fetch_add(n, std::sync::atomic::Ordering::Relaxed));
                }
                on_progress(out.len() as u64, total);
            }
            Err(e) => return Err(format!("read: {e}")),
        }
    }
    Ok(out)
}

/// Stream `reader` to `dest` via a `<dest>.part` sidecar, capped at
/// `max_bytes`, forwarding `on_progress(downloaded, total)`. On success the
/// sidecar is renamed to `dest`, removing any existing `dest` first — Vita's
/// filesystem doesn't reliably rename over an existing file (mirrors
/// `bsky_auth`'s `FileSessionStore::write_to_disk`), and this also clears a
/// stale zero-byte cache file the caller's cache-hit check skipped. The `.part`
/// is removed on any error so a torn download never lingers.
fn stream_to_file_capped(
    mut reader: impl Read,
    dest: &std::path::Path,
    max_bytes: u64,
    total: Option<u64>,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    // `<dest>.part` (append, don't replace the extension — keeps the `.mp4` so
    // the sidecar reads as `<cid>.mp4.part`).
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = std::path::PathBuf::from(part);

    let mut stream = || -> Result<(), String> {
        let mut file =
            std::fs::File::create(&part).map_err(|e| format!("create {}: {e}", part.display()))?;
        let mut buf = [0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => return Err(format!("read: {e}")),
            };
            written += n as u64;
            if written > max_bytes {
                return Err(format!("body exceeded cap of {max_bytes} bytes"));
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("write {}: {e}", part.display()))?;
            on_progress(written, total);
        }
        file.flush()
            .map_err(|e| format!("flush {}: {e}", part.display()))
    };

    match stream() {
        Ok(()) => {
            // Vita won't reliably rename over an existing file — remove a stale
            // destination (e.g. a zero-byte `<cid>.mp4` from an earlier failed
            // write) before installing the freshly downloaded `.part`.
            if dest.exists() {
                if let Err(e) = std::fs::remove_file(dest) {
                    let _ = std::fs::remove_file(&part);
                    return Err(format!("remove stale {}: {e}", dest.display()));
                }
            }
            std::fs::rename(&part, dest).map_err(|e| {
                let _ = std::fs::remove_file(&part);
                format!("rename {} -> {}: {e}", part.display(), dest.display())
            })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::future::Future;
    use std::sync::Arc;

    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    #[test]
    fn request_deadline_caps_slow_trickle_response() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::net::TcpListener;
        use std::thread;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n")
                .unwrap();
            for _ in 0..20 {
                thread::sleep(Duration::from_millis(25));
                if stream.write_all(b"x").is_err() {
                    break;
                }
            }
        });
        let client = VitaHttpClient::new();
        let request = Request::builder()
            .method("GET")
            .uri(format!("http://{address}/"))
            .body(Vec::new())
            .unwrap();
        TEST_BODY_BYTES.with(|bytes| bytes.store(0, std::sync::atomic::Ordering::Relaxed));
        let started = Instant::now();
        let result = with_request_deadline(Duration::from_millis(100), || {
            let mut future = Box::pin(client.send_http(request));
            let waker = std::task::Waker::from(Arc::new(NoopWake));
            let mut context = std::task::Context::from_waker(&waker);
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(result) => result,
                std::task::Poll::Pending => panic!("blocking HTTP future unexpectedly pending"),
            }
        });
        assert!(
            result.is_err(),
            "slow trickle must exceed absolute deadline"
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        server.join().unwrap();
    }

    #[test]
    fn repeated_sends_receive_decreasing_remaining_timeouts() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::net::TcpListener;
        TEST_SEND_TIMEOUTS.with(|timeouts| timeouts.borrow_mut().clear());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                let _ = stream.read(&mut request);
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
            }
        });
        let client = VitaHttpClient::new();
        with_request_deadline(Duration::from_secs(1), || {
            for _ in 0..2 {
                let request = Request::builder()
                    .method("GET")
                    .uri(format!("http://{address}/"))
                    .body(Vec::new())
                    .unwrap();
                let mut future = Box::pin(client.send_http(request));
                let waker = std::task::Waker::from(Arc::new(NoopWake));
                let mut context = std::task::Context::from_waker(&waker);
                assert!(matches!(
                    future.as_mut().poll(&mut context),
                    std::task::Poll::Ready(Ok(_))
                ));
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        server.join().unwrap();
        TEST_SEND_TIMEOUTS.with(|timeouts| {
            let values = timeouts.borrow();
            assert_eq!(values.len(), 2);
            assert!(values[1] < values[0]);
        });
    }

    #[test]
    fn scoped_redirect_is_returned_as_original_response_without_following() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /other\r\nContent-Length: 8\r\nConnection: close\r\n\r\nredirect").unwrap();
            let mut extra = [0; 1];
            assert_eq!(
                stream.read(&mut extra).unwrap_or(0),
                0,
                "redirect must not be followed"
            );
        });
        let request = Request::builder()
            .method("GET")
            .uri(format!("http://{address}/start"))
            .body(Vec::new())
            .unwrap();
        let client = VitaHttpClient::new();
        let response = with_request_deadline(Duration::from_secs(3), || {
            let mut future = Box::pin(client.send_http(request));
            let waker = std::task::Waker::from(Arc::new(NoopWake));
            let mut context = std::task::Context::from_waker(&waker);
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(result) => result.unwrap(),
                std::task::Poll::Pending => panic!("blocking HTTP future unexpectedly pending"),
            }
        });
        assert_eq!(response.status(), 302);
        assert_eq!(response.headers().get("location").unwrap(), "/other");
        assert_eq!(response.body(), b"redirect");
        server.join().unwrap();
    }

    #[test]
    fn expired_request_is_rejected_before_send() {
        with_request_deadline(Duration::from_millis(1), || {
            std::thread::sleep(Duration::from_millis(5));
            assert_eq!(
                remaining_request_time().unwrap_err().kind(),
                std::io::ErrorKind::TimedOut
            );
        });
    }

    #[test]
    fn request_deadline_scope_restores_and_narrows_nested_deadline() {
        assert!(remaining_request_time().unwrap().is_none());
        with_request_deadline(Duration::from_secs(5), || {
            let before = remaining_request_time().unwrap().unwrap();
            assert!(before <= Duration::from_secs(5));
            with_request_deadline(Duration::from_millis(20), || {
                assert!(remaining_request_time().unwrap().unwrap() <= Duration::from_millis(20));
            });
            assert!(remaining_request_time().unwrap().unwrap() <= before);
        });
        assert!(remaining_request_time().unwrap().is_none());
    }

    struct TestDnsReset;

    impl Drop for TestDnsReset {
        fn drop(&mut self) {
            clear_test_dns();
        }
    }

    fn install_test_dns(
        lookup: Arc<dyn Fn(&str) -> std::io::Result<Vec<SocketAddr>> + Send + Sync>,
    ) -> TestDnsReset {
        *TEST_DNS_LOOKUP
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(lookup);
        TestDnsReset
    }

    fn clear_test_dns() {
        *TEST_DNS_LOOKUP
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    #[test]
    fn blocked_dns_deadline_returns_timeout_without_spawning_replacement() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (result_tx, result_rx) = mpsc::channel();
        TEST_RESOLVER_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        let _dns_reset = install_test_dns(Arc::new(move |hostport| {
            if hostport.starts_with("blocked.test:") {
                let _ = started_tx.send(());
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2));
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "test DNS lookup released",
                ));
            }
            Ok(vec!["127.0.0.1:80".parse().unwrap()])
        }));
        let caller = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(100);
            let result =
                with_absolute_request_deadline(deadline, || resolve_host("blocked.test:80"));
            result_tx
                .send(result.map_err(|error| error.kind()))
                .unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(std::io::ErrorKind::TimedOut)
        );
        let pool = dns_pool();
        let mut accepted = 0;
        for index in 0..DNS_QUEUE {
            let (reply, _result) = mpsc::sync_channel(1);
            match pool
                .sender
                .try_send((format!("queued-{index}.test:80"), reply))
            {
                Ok(()) => accepted += 1,
                Err(mpsc::TrySendError::Full(_)) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => panic!("DNS pool disconnected"),
            }
        }
        let second = std::thread::spawn(|| {
            let deadline = Instant::now() + Duration::from_millis(100);
            with_absolute_request_deadline(deadline, || resolve_host("blocked.test:81"))
                .unwrap_err()
                .kind()
        });
        let second_error = second.join().unwrap();
        assert!(matches!(
            second_error,
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        assert!(
            TEST_RESOLVER_CALLS.load(std::sync::atomic::Ordering::SeqCst)
                <= DNS_QUEUE + DNS_THREADS
        );
        assert!(accepted <= DNS_QUEUE);
        let second = std::thread::spawn(|| {
            with_absolute_request_deadline(Instant::now() + Duration::from_millis(50), || {
                resolve_host("blocked.test:81")
            })
            .unwrap_err()
            .kind()
        });
        assert!(matches!(
            second.join().unwrap(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        assert!(
            TEST_RESOLVER_CALLS.load(std::sync::atomic::Ordering::SeqCst)
                <= DNS_QUEUE + DNS_THREADS
        );
        let _ = release_tx.send(());
        caller.join().unwrap();
    }

    #[test]
    fn expired_dns_deadline_rejects_cached_and_uncached_hosts_before_admission() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lookup_calls = calls.clone();
        let _dns_reset = install_test_dns(Arc::new(move |hostport| {
            if hostport.starts_with("expired-") {
                lookup_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(vec!["127.0.0.1:80".parse().unwrap()])
        }));
        let address: SocketAddr = "127.0.0.1:80".parse().unwrap();
        let pool = dns_pool();
        let cached_host = "expired-cached.test:80";
        let uncached_host = "expired-uncached.test:80";
        pool.cache
            .lock()
            .unwrap()
            .push((cached_host.into(), vec![address]));
        let expired = Instant::now() + Duration::from_millis(1);
        for hostport in [cached_host, uncached_host] {
            let error = with_absolute_request_deadline(expired, || {
                std::thread::sleep(Duration::from_millis(5));
                resolve_host(hostport)
            })
            .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        pool.cache
            .lock()
            .unwrap()
            .retain(|(key, _)| key != cached_host);
    }

    #[test]
    fn http_401_response_body_is_preserved() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 18\r\nConnection: close\r\n\r\ninvalid token body").unwrap();
        });
        let request = Request::builder()
            .method("GET")
            .uri(format!("http://{address}/"))
            .body(Vec::new())
            .unwrap();
        let client = VitaHttpClient::new();
        let response = with_request_deadline(Duration::from_secs(3), || {
            let mut future = Box::pin(client.send_http(request));
            let waker = std::task::Waker::from(Arc::new(NoopWake));
            let mut context = std::task::Context::from_waker(&waker);
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(result) => result.unwrap(),
                std::task::Poll::Pending => panic!("blocking HTTP future unexpectedly pending"),
            }
        });
        assert_eq!(response.status(), 401);
        assert_eq!(response.body(), b"invalid token body");
        server.join().unwrap();
    }

    #[test]
    fn read_capped_rejects_oversized_content_length() {
        // Server advertises 10 bytes, cap is 4 → fast-reject before reading.
        let r = read_capped(std::io::Cursor::new(vec![0u8; 10]), 4, Some(10), |_, _| {});
        assert!(r.is_err());
    }

    #[test]
    fn read_capped_rejects_stream_over_cap_without_content_length() {
        // No Content-Length, but the stream itself exceeds the cap → abort
        // mid-read rather than allocate unbounded.
        let r = read_capped(std::io::Cursor::new(vec![0u8; 100]), 16, None, |_, _| {});
        assert!(r.is_err());
    }

    #[test]
    fn read_capped_accepts_within_cap() {
        let data = vec![7u8; 50];
        let out = read_capped(std::io::Cursor::new(data.clone()), 64, Some(50), |_, _| {}).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn stream_to_file_overwrites_stale_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("vid.mp4");
        // A stale zero-byte cache file (the kind the cache-hit check skips).
        std::fs::write(&dest, b"").unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 0);

        let data = vec![9u8; 2048];
        stream_to_file_capped(
            std::io::Cursor::new(data.clone()),
            &dest,
            1 << 20,
            Some(data.len() as u64),
            |_, _| {},
        )
        .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), data, "stale file replaced");
        let mut part = dest.clone().into_os_string();
        part.push(".part");
        assert!(!std::path::Path::new(&part).exists(), ".part cleaned up");
    }

    #[test]
    fn stream_to_file_over_cap_leaves_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("vid.mp4");
        let r = stream_to_file_capped(
            std::io::Cursor::new(vec![0u8; 100]),
            &dest,
            16,
            None,
            |_, _| {},
        );
        assert!(r.is_err(), "over-cap stream rejected");
        assert!(!dest.exists(), "no final file on cap failure");
        let mut part = dest.into_os_string();
        part.push(".part");
        assert!(
            !std::path::Path::new(&part).exists(),
            "no .part left behind"
        );
    }

    #[test]
    fn client_constructs() {
        let _ = VitaHttpClient::new();
    }

    #[test]
    fn client_is_send_sync() {
        // Confirms the resulting future is Send-bound, which atrium requires.
        fn assert_send<T: Send>(_: T) {}
        let client = VitaHttpClient::new();
        let req = Request::builder()
            .method("GET")
            .uri("https://example.invalid/")
            .body(Vec::new())
            .unwrap();
        let fut = client.send_http(req);
        assert_send(fut);
    }

    // ── get_bytes_capped_with_deadline tests ──────────────────────────

    #[test]
    fn deadline_capped_get_expired_before_start_returns_error() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Expired deadline → error before any I/O (no server needed).
        let client = VitaHttpClient::new();
        let past = Instant::now() - Duration::from_secs(10);
        let result = client.get_bytes_capped_with_deadline(
            "http://127.0.0.1:1/",
            1024,
            past,
        );
        assert!(result.is_err(), "expired deadline must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("deadline") || err.contains("expired"),
            "error must indicate deadline expiry: {err}"
        );
    }

    #[test]
    fn deadline_capped_get_respects_content_length_cap() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n").unwrap();
            let body = vec![b'x'; 100];
            stream.write_all(&body).unwrap();
        });
        let client = VitaHttpClient::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        let result =
            client.get_bytes_capped_with_deadline(&format!("http://{address}/"), 4, deadline);
        assert!(result.is_err(), "Content-Length > cap must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("Content-Length"),
            "error must mention Content-Length: {err}"
        );
        server.join().unwrap();
    }

    #[test]
    fn deadline_capped_get_stream_over_cap_body() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            // No Content-Length → chunked or connection-close body.
            stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").unwrap();
            let body = vec![b'x'; 200];
            stream.write_all(&body).unwrap();
        });
        let client = VitaHttpClient::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        let result =
            client.get_bytes_capped_with_deadline(&format!("http://{address}/"), 16, deadline);
        assert!(result.is_err(), "streaming body > cap must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("exceeded cap"),
            "error must mention cap: {err}"
        );
        server.join().unwrap();
    }

    #[test]
    fn deadline_capped_get_restores_prior_scope_and_does_not_leak() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = match listener.accept() {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let mut request = [0; 1024];
                let _ = stream.read(&mut request);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbody").unwrap();
            }
        });
        let client = VitaHttpClient::new();
        // Outer scope: 5s deadline.
        with_request_deadline(Duration::from_secs(5), || {
            let outer_start = remaining_request_time().unwrap().unwrap();
            // Inner call: uses its own 3s deadline.
            let inner_deadline = Instant::now() + Duration::from_secs(3);
            let result = client.get_bytes_capped_with_deadline(
                &format!("http://{address}/"),
                1024,
                inner_deadline,
            );
            assert!(result.is_ok(), "inner call must succeed");
            // After inner call returns, outer scope must still be active
            // and must not have been shortened.
            let after = remaining_request_time().unwrap().unwrap();
            assert!(
                after > Duration::from_secs(3),
                "outer deadline must not be shortened by inner call"
            );
            assert!(
                after <= outer_start,
                "outer deadline must not grow"
            );
        });
        // After outer scope exits, no deadline should remain.
        assert!(
            remaining_request_time().unwrap().is_none(),
            "deadline must be cleared after outer scope"
        );
        // Now call unscoped get_bytes_capped to confirm the deadline
        // from the inner call is not leaked — it should use the plain
        // agent (no deadline) and succeed.
        let result = client.get_bytes_capped(&format!("http://{address}/"), 1024);
        assert!(result.is_ok(), "unscoped call after scoped must succeed");
        server.join().unwrap();
    }

    #[test]
    fn deadline_capped_get_body_read_timeout_reported_as_error() {
        let _guard = TEST_NETWORK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            // Send headers, then stall. The transport timeout and/or
            // the scoped deadline will fire before we send the body.
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\nConnection: close\r\n\r\n").unwrap();
            // Stall: the client will time out during the body read.
            std::thread::sleep(Duration::from_secs(2));
            // The client has already given up by now.
        });
        let client = VitaHttpClient::new();
        let deadline = Instant::now() + Duration::from_millis(200);
        let result =
            client.get_bytes_capped_with_deadline(&format!("http://{address}/"), 10_000, deadline);
        assert!(result.is_err(), "body read must time out");
        let err = result.unwrap_err();
        assert!(
            err.contains("ead") || err.contains("deadline") || err.contains("timeout") || err.contains("TimedOut"),
            "error must indicate timeout: {err}"
        );
        server.join().unwrap();
    }
}
