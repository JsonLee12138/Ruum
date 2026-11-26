//! N-way per-host scan coordinators providing a 60s snapshot cache and
//! single-flight coalescing across `Local` and individual `Ssh(profile_id)`
//! streams without blocking or mixing hosts.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;
use ruum_core::remote::Snapshot;
use ruum_core::removal::normalize;
use ruum_core::tree::Node;

/// Freshness TTL for reusing scanned directory snapshots before triggering a
/// new filesystem or SSH traversal.
pub const SCAN_CACHE_TTL: Duration = Duration::from_mins(1);

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(40);

/// Distinct scan & AI execution stream key per host.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[allow(
    dead_code,
    reason = "explicit stream identity shared across local and N SSH hosts"
)]
pub enum HostStreamId {
    Local,
    Ssh(String),
}

#[derive(Clone, Debug)]
struct CacheEntry<T> {
    value: T,
    captured_at: Instant,
}

#[derive(Debug)]
struct InFlightState<T> {
    active_waiters: usize,
    owned_by_external: bool,
    done: Option<Result<T, String>>,
    worker_cancel: Arc<AtomicBool>,
}

#[derive(Debug)]
struct InFlightScan<T> {
    state: Mutex<InFlightState<T>>,
    cv: Condvar,
}

impl<T> InFlightScan<T> {
    fn new(owned_by_external: bool) -> Self {
        Self {
            state: Mutex::new(InFlightState {
                active_waiters: 0,
                owned_by_external,
                done: None,
                worker_cancel: Arc::new(AtomicBool::new(false)),
            }),
            cv: Condvar::new(),
        }
    }
}

/// Completes or aborts an externally driven UI scan registered on
/// [`ScanCoordinator::begin_external_flight`] so coalesced callers wait on it
/// instead of spawning a second filesystem walk.
#[derive(Debug)]
pub struct ExternalFlightHandle<K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    inner: Arc<Mutex<ScanCoordinatorInner<K, V>>>,
    key: K,
    flight: Arc<InFlightScan<V>>,
    finished: bool,
}

impl<K, V> ExternalFlightHandle<K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn finish(mut self, outcome: Result<V, String>) {
        self.resolve(outcome);
        self.finished = true;
    }

    fn resolve(&self, outcome: Result<V, String>) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner
                .inflight
                .get(&self.key)
                .is_some_and(|cur| Arc::ptr_eq(cur, &self.flight))
            {
                inner.inflight.remove(&self.key);
            }
            if let Ok(ref val) = outcome {
                inner.cache.insert(
                    self.key.clone(),
                    CacheEntry {
                        value: val.clone(),
                        captured_at: Instant::now(),
                    },
                );
            }
        }
        if let Ok(mut st) = self.flight.state.lock()
            && st.done.is_none()
        {
            st.done = Some(outcome);
            self.flight.cv.notify_all();
        }
    }
}

impl<K, V> Drop for ExternalFlightHandle<K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if !self.finished {
            self.resolve(Err("external scan cancelled".to_owned()));
            self.finished = true;
        }
    }
}

struct WaiterGuard<'a, K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    coord: &'a ScanCoordinator<K, V>,
    key: &'a K,
    flight: &'a Arc<InFlightScan<V>>,
}

impl<K, V> Drop for WaiterGuard<'_, K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn drop(&mut self) {
        let should_clear = if let Ok(mut guard) = self.flight.state.lock() {
            guard.active_waiters = guard.active_waiters.saturating_sub(1);
            if !guard.owned_by_external
                && guard.active_waiters == 0
                && guard.done.is_none()
            {
                guard.worker_cancel.store(true, Ordering::Release);
                true
            } else {
                false
            }
        } else {
            false
        };
        if should_clear
            && let Ok(mut inner) = self.coord.inner.lock()
            && inner
                .inflight
                .get(self.key)
                .is_some_and(|cur| Arc::ptr_eq(cur, self.flight))
        {
            inner.inflight.remove(self.key);
        }
    }
}

/// Core 60s cache + symmetric single-flight coalescer parameterized by key `K`
/// and cached value `V`.
#[derive(Debug)]
struct ScanCoordinatorInner<K, V> {
    cache: FxHashMap<K, CacheEntry<V>>,
    inflight: FxHashMap<K, Arc<InFlightScan<V>>>,
}

impl<K, V> Default for ScanCoordinatorInner<K, V> {
    fn default() -> Self {
        Self {
            cache: FxHashMap::default(),
            inflight: FxHashMap::default(),
        }
    }
}

#[derive(Debug)]
pub struct ScanCoordinator<K, V> {
    inner: Arc<Mutex<ScanCoordinatorInner<K, V>>>,
}

impl<K, V> Default for ScanCoordinator<K, V> {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ScanCoordinatorInner::default())),
        }
    }
}

impl<K, V> ScanCoordinator<K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn get_fresh_with_ttl(&self, key: &K, ttl: Duration) -> Option<V> {
        let mut guard = self.inner.lock().ok()?;
        if let Some(entry) = guard.cache.get(key) {
            if entry.captured_at.elapsed() <= ttl {
                return Some(entry.value.clone());
            }
            guard.cache.remove(key);
        }
        None
    }

    pub fn get_fresh(&self, key: &K) -> Option<V> {
        self.get_fresh_with_ttl(key, SCAN_CACHE_TTL)
    }

    pub fn seed(&self, key: K, value: V) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.cache.insert(
                key,
                CacheEntry {
                    value,
                    captured_at: Instant::now(),
                },
            );
        }
    }

    pub fn invalidate_all(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.cache.clear();
        }
    }

    /// Register an already-running external scan (such as an Explore UI walk)
    /// so concurrent AI analyses coalesce onto it rather than launching a second
    /// filesystem traversal.
    pub fn begin_external_flight(&self, key: K) -> ExternalFlightHandle<K, V> {
        let flight = Arc::new(InFlightScan::new(true));
        let prev = self.inner.lock().ok().and_then(|mut guard| {
            guard.inflight.insert(key.clone(), Arc::clone(&flight))
        });
        if let Some(old) = prev
            && let Ok(mut st) = old.state.lock()
            && st.done.is_none()
        {
            st.done = Some(Err("superseded".to_owned()));
            old.cv.notify_all();
        }
        ExternalFlightHandle {
            inner: Arc::clone(&self.inner),
            key,
            flight,
            finished: false,
        }
    }

    pub fn get_or_scan_with<F>(
        &self,
        key: &K,
        caller_cancel: &AtomicBool,
        ttl: Duration,
        scan_fn: F,
    ) -> Result<V, String>
    where
        F: FnOnce(Arc<AtomicBool>) -> Result<V, String> + Send + 'static,
    {
        if caller_cancel.load(Ordering::Acquire) {
            return Err(ruum_core::ai_cli::AI_CANCELLED_SENTINEL.to_owned());
        }
        let (flight, spawn_worker, worker_cancel) = {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| "scan coordinator lock poisoned".to_owned())?;
            if let Some(entry) = guard.cache.get(key) {
                if entry.captured_at.elapsed() <= ttl {
                    return Ok(entry.value.clone());
                }
                guard.cache.remove(key);
            }
            if let Some(existing) = guard.inflight.get(key) {
                let res = (Arc::clone(existing), false, None);
                drop(guard);
                res
            } else {
                let flight = Arc::new(InFlightScan::new(false));
                let worker_cancel = flight.state.lock().map_or_else(
                    |_| Arc::new(AtomicBool::new(false)),
                    |s| Arc::clone(&s.worker_cancel),
                );
                guard.inflight.insert(key.clone(), Arc::clone(&flight));
                drop(guard);
                (flight, true, Some(worker_cancel))
            }
        };

        if spawn_worker {
            let key_done = key.clone();
            let inner_ref = Arc::clone(&self.inner);
            let inner_flight = Arc::clone(&flight);
            let cancel_flag = worker_cancel.unwrap_or_default();
            let _ =
                std::thread::Builder::new()
                    .name("ruum-scan-coalesce".to_owned())
                    .spawn(move || {
                        let outcome = scan_fn(cancel_flag);
                        if let Ok(mut inner) = inner_ref.lock() {
                            if inner.inflight.get(&key_done).is_some_and(
                                |cur| Arc::ptr_eq(cur, &inner_flight),
                            ) {
                                inner.inflight.remove(&key_done);
                            }
                            if let Ok(ref val) = outcome {
                                inner.cache.insert(
                                    key_done,
                                    CacheEntry {
                                        value: val.clone(),
                                        captured_at: Instant::now(),
                                    },
                                );
                            }
                        }
                        if let Ok(mut st) = inner_flight.state.lock() {
                            st.done = Some(outcome);
                            inner_flight.cv.notify_all();
                        }
                    });
        }

        self.wait_for_flight(key, &flight, caller_cancel)
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "Condvar::wait_timeout consumes the MutexGuard atomically while waiting"
    )]
    fn wait_for_flight(
        &self,
        key: &K,
        flight: &Arc<InFlightScan<V>>,
        caller_cancel: &AtomicBool,
    ) -> Result<V, String> {
        {
            let mut state_guard = flight
                .state
                .lock()
                .map_err(|_| "scan flight lock poisoned".to_owned())?;
            state_guard.active_waiters =
                state_guard.active_waiters.saturating_add(1);
            drop(state_guard);
        }
        let _waiter = WaiterGuard {
            coord: self,
            key,
            flight,
        };

        loop {
            if caller_cancel.load(Ordering::Acquire) {
                return Err(ruum_core::ai_cli::AI_CANCELLED_SENTINEL.to_owned());
            }
            let outcome = {
                let guard = flight
                    .state
                    .lock()
                    .map_err(|_| "scan flight lock poisoned".to_owned())?;
                if let Some(ref done) = guard.done {
                    Some(done.clone())
                } else {
                    let (after_wait, _) = flight
                        .cv
                        .wait_timeout(guard, WAIT_POLL_INTERVAL)
                        .map_err(|_| "scan flight lock poisoned".to_owned())?;
                    after_wait.done.clone()
                }
            };
            if let Some(res) = outcome {
                return res;
            }
        }
    }
}

/// Normalize local directory path key for 60s scan cache lookups.
pub fn normalize_local_cache_key(root: &Path) -> PathBuf {
    normalize(root)
}

/// Normalize a remote SSH root path into a stable cache key per profile.
pub fn normalize_remote_cache_key(root: Option<&Path>) -> String {
    let raw = root
        .and_then(|p| p.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("~");
    if raw == "/" {
        "/".to_owned()
    } else {
        raw.trim_end_matches('/').to_owned()
    }
}

/// Local filesystem 60s cache + single-flight coalescer (`HostStreamId::Local`).
pub type LocalScanCoordinator = ScanCoordinator<PathBuf, Arc<Node>>;

/// Single SSH profile's isolated 60s cache + single-flight coalescer.
pub type RemoteHostScanCoordinator = ScanCoordinator<String, Arc<Snapshot>>;

/// N-way isolated remote coordinator registry: every `profile_id` (`"srv-1"`,
/// `"srv-2"`, ...) gets its own independent lock, cache, and single-flight map.
#[derive(Debug, Default)]
pub struct RemoteScanCoordinators {
    by_profile: Mutex<FxHashMap<String, Arc<RemoteHostScanCoordinator>>>,
}

impl RemoteScanCoordinators {
    pub fn for_profile(
        &self,
        profile_id: &str,
    ) -> Arc<RemoteHostScanCoordinator> {
        let mut guard = self
            .by_profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            guard.entry(profile_id.to_owned()).or_insert_with(|| {
                Arc::new(RemoteHostScanCoordinator::default())
            }),
        )
    }

    pub fn invalidate_profile(&self, profile_id: &str) {
        if let Ok(guard) = self.by_profile.lock()
            && let Some(coord) = guard.get(profile_id)
        {
            coord.invalidate_all();
        }
    }
}
