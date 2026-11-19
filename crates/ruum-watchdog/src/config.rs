//! `SQLite`-backed watchdog configuration and cooldown state service.
//!
//! Owns `watchdog_config` and `watchdog_state` inside `<config_dir>/ruum/ruum.db`
//! using `WAL` journal mode and `busy_timeout = 5000` for safe concurrent
//! access across GUI, CLI, and background watchdog daemon processes.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};

/// Minimum allowable free-space threshold in gigabytes.
pub const MIN_FREE_GB_FLOOR: u64 = 1;

/// Default free-space threshold in gigabytes.
pub const DEFAULT_MIN_FREE_GB: u64 = 20;

/// Minimum allowable watchdog polling interval in seconds.
pub const MIN_POLL_INTERVAL_SECS: u64 = 30;

/// Default watchdog polling interval in seconds (5 minutes).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 300;

/// Minimum allowable alert cooldown in seconds (5 minutes).
pub const MIN_COOLDOWN_SECS: u64 = 300;

/// Default alert cooldown in seconds (6 hours).
pub const DEFAULT_COOLDOWN_SECS: u64 = 21_600;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS watchdog_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    background_service INTEGER NOT NULL DEFAULT 0,
    min_free_gb INTEGER NOT NULL DEFAULT 20,
    poll_interval_secs INTEGER NOT NULL DEFAULT 300,
    cooldown_secs INTEGER NOT NULL DEFAULT 21600,
    updated_at_secs INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS watchdog_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_checked_secs INTEGER,
    last_triggered_secs INTEGER,
    last_triggered_threshold_gb INTEGER
);
";

/// Watchdog background monitoring configuration (strictly decoupled from AI).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchdogConfig {
    pub enabled: bool,
    pub background_service: bool,
    pub min_free_gb: u64,
    pub poll_interval_secs: u64,
    pub cooldown_secs: u64,
    pub updated_at_secs: u64,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            background_service: false,
            min_free_gb: DEFAULT_MIN_FREE_GB,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            cooldown_secs: DEFAULT_COOLDOWN_SECS,
            updated_at_secs: 0,
        }
    }
}

impl WatchdogConfig {
    /// Normalize threshold, polling interval, and cooldown to safe minimums.
    #[must_use]
    pub const fn normalized(&self) -> Self {
        Self {
            enabled: self.enabled,
            background_service: self.background_service,
            min_free_gb: if self.min_free_gb < MIN_FREE_GB_FLOOR {
                MIN_FREE_GB_FLOOR
            } else {
                self.min_free_gb
            },
            poll_interval_secs: if self.poll_interval_secs
                < MIN_POLL_INTERVAL_SECS
            {
                MIN_POLL_INTERVAL_SECS
            } else {
                self.poll_interval_secs
            },
            cooldown_secs: if self.cooldown_secs < MIN_COOLDOWN_SECS {
                MIN_COOLDOWN_SECS
            } else {
                self.cooldown_secs
            },
            updated_at_secs: self.updated_at_secs,
        }
    }
}

/// Persistent runtime cooldown state of the watchdog (without AI proposal).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchdogState {
    pub last_checked_secs: Option<u64>,
    pub last_triggered_secs: Option<u64>,
    pub last_triggered_threshold_gb: Option<u64>,
}

fn config_dir_for(
    is_macos: bool,
    is_windows: bool,
    home: Option<PathBuf>,
    appdata: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
) -> Option<PathBuf> {
    if is_macos {
        home.map(|h| h.join("Library/Application Support/ruum"))
    } else if is_windows {
        appdata
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| home.map(|h| h.join("AppData").join("Roaming")))
            .map(|base| base.join("ruum"))
    } else {
        xdg_config_home
            .filter(|p| {
                !p.as_os_str().is_empty()
                    && (p.is_absolute() || p.to_string_lossy().starts_with('/'))
            })
            .or_else(|| home.map(|h| h.join(".config")))
            .map(|base| base.join("ruum"))
    }
}

/// Cross-platform base configuration directory for ruum: `<config_dir>/ruum`.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    config_dir_for(
        cfg!(target_os = "macos"),
        cfg!(windows),
        std::env::home_dir(),
        std::env::var_os("APPDATA").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
    )
}

/// Default path to the unified `SQLite` database (`<config_dir>/ruum/ruum.db`).
#[must_use]
pub fn ruum_db_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("ruum.db"))
}

/// Path to `watchdog.pid` under `<config_dir>/ruum`.
#[must_use]
pub fn watchdog_pid_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("watchdog.pid"))
}

fn sqlite_err(err: &rusqlite::Error) -> io::Error {
    io::Error::other(err.to_string())
}

const fn is_sqlite_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(ffi, _)
            if matches!(
                ffi.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            )
    )
}

const BUSY_RETRY_TIMEOUT: Duration = Duration::from_secs(30);

const fn busy_backoff(attempt: u64) -> Duration {
    let jitter_ms = 15 + ((attempt.saturating_mul(17)) % 35);
    Duration::from_millis(jitter_ms)
}

fn retry_on_busy_with_timeout<T>(
    timeout: Duration,
    mut op: impl FnMut() -> rusqlite::Result<T>,
) -> io::Result<T> {
    let deadline = Instant::now() + timeout;
    let mut attempt = 0_u64;
    loop {
        match op() {
            Ok(val) => return Ok(val),
            Err(err) if is_sqlite_busy(&err) => {
                let now = Instant::now();
                if now >= deadline {
                    return Err(sqlite_err(&err));
                }
                let remaining = deadline.saturating_duration_since(now);
                let sleep_for = busy_backoff(attempt).min(remaining);
                attempt = attempt.saturating_add(1);
                std::thread::sleep(sleep_for);
            }
            Err(err) => return Err(sqlite_err(&err)),
        }
    }
}

fn retry_on_busy<T>(op: impl FnMut() -> rusqlite::Result<T>) -> io::Result<T> {
    retry_on_busy_with_timeout(BUSY_RETRY_TIMEOUT, op)
}

fn u64_to_sqlite_i64(val: u64) -> i64 {
    i64::try_from(val).unwrap_or(i64::MAX)
}

fn sqlite_i64_to_u64(val: i64) -> u64 {
    u64::try_from(val).unwrap_or(0)
}

/// Service owning `watchdog_config` and `watchdog_state` inside `ruum.db`.
pub struct WatchdogConfigService {
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl fmt::Debug for WatchdogConfigService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchdogConfigService")
            .field("db_path", &self.db_path)
            .finish_non_exhaustive()
    }
}

impl WatchdogConfigService {
    /// Open (or create) `ruum.db` at `db_path`, configure `busy_timeout = 5000`
    /// before `journal_mode = WAL`, and ensure watchdog tables exist.
    pub fn open(db_path: impl AsRef<Path>) -> io::Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = retry_on_busy(|| Connection::open(&db_path))?;
        // Set busy_timeout before switching to WAL and retry transient
        // SQLITE_BUSY/SQLITE_LOCKED during initial DELETE -> WAL transition.
        retry_on_busy(|| {
            conn.execute_batch(
                "PRAGMA busy_timeout = 5000;\nPRAGMA journal_mode = WAL;",
            )?;
            conn.execute_batch(SCHEMA_SQL)
        })?;
        Ok(Self {
            db_path,
            conn: Mutex::new(conn),
        })
    }

    /// Open the default `<config_dir>/ruum/ruum.db` database.
    pub fn open_default() -> io::Result<Self> {
        let path = ruum_db_path().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "cannot resolve ruum config directory",
            )
        })?;
        Self::open(path)
    }

    /// Path to the underlying `SQLite` database file.
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn with_conn_retry<T>(
        &self,
        mut op: impl FnMut(&mut Connection) -> rusqlite::Result<T>,
    ) -> io::Result<T> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| io::Error::other("watchdog config mutex poisoned"))?;
        retry_on_busy(|| op(&mut conn))
    }

    /// Load watchdog configuration, falling back to defaults when no row
    /// exists yet.
    pub fn load_config(&self) -> io::Result<WatchdogConfig> {
        let row = self.with_conn_retry(|conn| {
            conn.query_row(
                "SELECT enabled, background_service, min_free_gb, \
                 poll_interval_secs, cooldown_secs, updated_at_secs \
                 FROM watchdog_config WHERE id = 1",
                [],
                |row| {
                    let enabled: i64 = row.get(0)?;
                    let background_service: i64 = row.get(1)?;
                    let min_free_gb: i64 = row.get(2)?;
                    let poll_interval_secs: i64 = row.get(3)?;
                    let cooldown_secs: i64 = row.get(4)?;
                    let updated_at_secs: i64 = row.get(5)?;
                    Ok(WatchdogConfig {
                        enabled: enabled != 0,
                        background_service: background_service != 0,
                        min_free_gb: sqlite_i64_to_u64(min_free_gb),
                        poll_interval_secs: sqlite_i64_to_u64(
                            poll_interval_secs,
                        ),
                        cooldown_secs: sqlite_i64_to_u64(cooldown_secs),
                        updated_at_secs: sqlite_i64_to_u64(updated_at_secs),
                    })
                },
            )
            .optional()
        })?;
        Ok(row.unwrap_or_default().normalized())
    }

    /// Atomically upsert normalized watchdog configuration into `ruum.db`.
    pub fn save_config(&self, config: &WatchdogConfig) -> io::Result<()> {
        let norm = config.normalized();
        self.with_conn_retry(|conn| {
            conn.execute(
                "INSERT INTO watchdog_config (
                    id, enabled, background_service, min_free_gb,
                    poll_interval_secs, cooldown_secs, updated_at_secs
                 ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    enabled = excluded.enabled,
                    background_service = excluded.background_service,
                    min_free_gb = excluded.min_free_gb,
                    poll_interval_secs = excluded.poll_interval_secs,
                    cooldown_secs = excluded.cooldown_secs,
                    updated_at_secs = excluded.updated_at_secs",
                params![
                    i64::from(norm.enabled),
                    i64::from(norm.background_service),
                    u64_to_sqlite_i64(norm.min_free_gb),
                    u64_to_sqlite_i64(norm.poll_interval_secs),
                    u64_to_sqlite_i64(norm.cooldown_secs),
                    u64_to_sqlite_i64(norm.updated_at_secs),
                ],
            )
            .map(|_| ())
        })
    }

    /// Load watchdog cooldown state, falling back to empty state when absent.
    pub fn load_state(&self) -> io::Result<WatchdogState> {
        let row = self.with_conn_retry(|conn| {
            conn.query_row(
                "SELECT last_checked_secs, last_triggered_secs, \
                 last_triggered_threshold_gb \
                 FROM watchdog_state WHERE id = 1",
                [],
                |row| {
                    let last_checked: Option<i64> = row.get(0)?;
                    let last_triggered: Option<i64> = row.get(1)?;
                    let last_threshold: Option<i64> = row.get(2)?;
                    Ok(WatchdogState {
                        last_checked_secs: last_checked
                            .and_then(|v| u64::try_from(v).ok()),
                        last_triggered_secs: last_triggered
                            .and_then(|v| u64::try_from(v).ok()),
                        last_triggered_threshold_gb: last_threshold
                            .and_then(|v| u64::try_from(v).ok()),
                    })
                },
            )
            .optional()
        })?;
        Ok(row.unwrap_or_default())
    }

    /// Atomically upsert watchdog cooldown state into `ruum.db`.
    pub fn save_state(&self, state: &WatchdogState) -> io::Result<()> {
        self.with_conn_retry(|conn| {
            conn.execute(
                "INSERT INTO watchdog_state (
                    id, last_checked_secs, last_triggered_secs,
                    last_triggered_threshold_gb
                 ) VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET
                    last_checked_secs = excluded.last_checked_secs,
                    last_triggered_secs = excluded.last_triggered_secs,
                    last_triggered_threshold_gb =
                        excluded.last_triggered_threshold_gb",
                params![
                    state.last_checked_secs.map(u64_to_sqlite_i64),
                    state.last_triggered_secs.map(u64_to_sqlite_i64),
                    state.last_triggered_threshold_gb.map(u64_to_sqlite_i64),
                ],
            )
            .map(|_| ())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn config_defaults_and_normalization() {
        let def = WatchdogConfig::default();
        assert!(!def.enabled);
        assert!(!def.background_service);
        assert_eq!(def.min_free_gb, DEFAULT_MIN_FREE_GB);
        assert_eq!(def.poll_interval_secs, DEFAULT_POLL_INTERVAL_SECS);
        assert_eq!(def.cooldown_secs, DEFAULT_COOLDOWN_SECS);
        assert_eq!(def.updated_at_secs, 0);

        let zeroed = WatchdogConfig {
            min_free_gb: 0,
            poll_interval_secs: 5,
            cooldown_secs: 0,
            ..def
        };
        let norm = zeroed.normalized();
        assert_eq!(norm.min_free_gb, MIN_FREE_GB_FLOOR);
        assert_eq!(norm.poll_interval_secs, MIN_POLL_INTERVAL_SECS);
        assert_eq!(norm.cooldown_secs, MIN_COOLDOWN_SECS);

        let low_cooldown = WatchdogConfig {
            cooldown_secs: 60,
            ..WatchdogConfig::default()
        };
        assert_eq!(low_cooldown.normalized().cooldown_secs, MIN_COOLDOWN_SECS);
    }

    #[test]
    fn sqlite_service_creates_wal_db_with_busy_timeout_and_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("nested/ruum.db");

        let svc = WatchdogConfigService::open(&db_path).unwrap();
        assert_eq!(svc.db_path(), db_path.as_path());
        assert!(db_path.is_file());

        let (journal_mode, busy_timeout): (String, i64) = {
            let conn = svc.conn.lock().unwrap();
            let mode: String = conn
                .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
                .unwrap();
            let timeout: i64 = conn
                .query_row("PRAGMA busy_timeout;", [], |r| r.get(0))
                .unwrap();
            drop(conn);
            (mode, timeout)
        };
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(busy_timeout, 5000);

        assert_eq!(svc.load_config().unwrap(), WatchdogConfig::default());
        assert_eq!(svc.load_state().unwrap(), WatchdogState::default());
    }

    #[test]
    fn sqlite_service_save_and_reload_normalizes_and_persists() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("ruum.db");

        let svc = WatchdogConfigService::open(&db_path).unwrap();
        let unnormalized = WatchdogConfig {
            enabled: true,
            background_service: true,
            min_free_gb: 0,
            poll_interval_secs: 5,
            cooldown_secs: 42,
            updated_at_secs: 1_700_000_001,
        };
        svc.save_config(&unnormalized).unwrap();

        let state = WatchdogState {
            last_checked_secs: Some(1_700_000_100),
            last_triggered_secs: Some(1_700_000_050),
            last_triggered_threshold_gb: Some(20),
        };
        svc.save_state(&state).unwrap();
        drop(svc);

        let reopened = WatchdogConfigService::open(&db_path).unwrap();
        let loaded_cfg = reopened.load_config().unwrap();
        assert_eq!(
            loaded_cfg,
            WatchdogConfig {
                enabled: true,
                background_service: true,
                min_free_gb: MIN_FREE_GB_FLOOR,
                poll_interval_secs: MIN_POLL_INTERVAL_SECS,
                cooldown_secs: MIN_COOLDOWN_SECS,
                updated_at_secs: 1_700_000_001,
            }
        );
        assert_eq!(reopened.load_state().unwrap(), state);

        // Overwriting state with cleared optionals updates row cleanly.
        let cleared = WatchdogState {
            last_checked_secs: Some(1_700_000_200),
            last_triggered_secs: None,
            last_triggered_threshold_gb: None,
        };
        reopened.save_state(&cleared).unwrap();
        assert_eq!(reopened.load_state().unwrap(), cleared);
    }

    #[test]
    fn sqlite_service_concurrent_open_and_reads_writes_succeed() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("concurrent/ruum.db");
        let thread_count = 4_usize;
        let barrier = Arc::new(Barrier::new(thread_count));

        let mut handles = Vec::with_capacity(thread_count);
        for idx in 0..thread_count {
            let path = db_path.clone();
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait();
                let svc = WatchdogConfigService::open(&path).unwrap();
                for iter in 0_u64..10 {
                    let ts = 1_700_000_000 + (idx as u64) * 100 + iter;
                    svc.save_config(&WatchdogConfig {
                        enabled: iter % 2 == 0,
                        background_service: true,
                        min_free_gb: 10 + iter,
                        poll_interval_secs: 60 + iter,
                        cooldown_secs: 600 + iter,
                        updated_at_secs: ts,
                    })
                    .unwrap();
                    svc.save_state(&WatchdogState {
                        last_checked_secs: Some(ts),
                        last_triggered_secs: Some(ts),
                        last_triggered_threshold_gb: Some(10 + iter),
                    })
                    .unwrap();
                    let cfg = svc.load_config().unwrap();
                    let st = svc.load_state().unwrap();
                    assert!(cfg.min_free_gb >= 10);
                    assert!(st.last_checked_secs.is_some());
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn config_dir_for_covers_all_platforms_and_fallbacks() {
        let home = Some(PathBuf::from("/users/tester"));
        assert_eq!(
            config_dir_for(true, false, home.clone(), None, None),
            Some(PathBuf::from(
                "/users/tester/Library/Application Support/ruum"
            ))
        );
        assert_eq!(
            config_dir_for(
                false,
                true,
                home.clone(),
                Some(PathBuf::from("C:/AppData/Roaming")),
                None
            ),
            Some(PathBuf::from("C:/AppData/Roaming/ruum"))
        );
        assert_eq!(
            config_dir_for(
                false,
                true,
                home.clone(),
                Some(PathBuf::new()),
                None
            ),
            Some(PathBuf::from("/users/tester/AppData/Roaming/ruum"))
        );
        assert_eq!(
            config_dir_for(
                false,
                false,
                home.clone(),
                None,
                Some(PathBuf::from("/custom/xdg"))
            ),
            Some(PathBuf::from("/custom/xdg/ruum"))
        );
        assert_eq!(
            config_dir_for(false, false, home, None, None),
            Some(PathBuf::from("/users/tester/.config/ruum"))
        );
    }

    fn with_contended_write_lock(
        db_path: &Path,
        hold: Duration,
        op: impl FnOnce(),
    ) {
        let lock_path = db_path.to_path_buf();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let lock_conn = Connection::open(&lock_path).unwrap();
            lock_conn
                .execute_batch("PRAGMA busy_timeout = 5000;")
                .unwrap();
            lock_conn.execute_batch("BEGIN IMMEDIATE;").unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(hold);
            lock_conn.execute_batch("COMMIT;").unwrap();
        });
        locked_rx.recv().unwrap();
        op();
        handle.join().unwrap();
    }

    #[test]
    fn retry_on_busy_recovers_from_transient_busy_and_locked() {
        let mut attempts = 0_usize;
        let value = retry_on_busy(|| {
            attempts += 1;
            if attempts <= 42 {
                let code = if attempts.is_multiple_of(2) {
                    rusqlite::ffi::SQLITE_LOCKED
                } else {
                    rusqlite::ffi::SQLITE_BUSY
                };
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(code),
                    Some("transient lock".to_owned()),
                ))
            } else {
                Ok(99_u32)
            }
        })
        .unwrap();
        assert_eq!(value, 99);
        assert_eq!(attempts, 43);
    }

    #[test]
    fn retry_on_busy_passes_through_non_busy_error_immediately() {
        let mut attempts = 0_usize;
        let err = retry_on_busy::<()>(|| {
            attempts += 1;
            Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some("constraint failed".to_owned()),
            ))
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(err.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn retry_on_busy_with_timeout_expires_after_deadline() {
        let started = Instant::now();
        let mut attempts = 0_usize;
        let err =
            retry_on_busy_with_timeout::<()>(Duration::from_millis(60), || {
                attempts += 1;
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    Some("always busy".to_owned()),
                ))
            })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert!(attempts >= 2);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn save_config_retries_on_sqlite_busy_with_zero_busy_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("ruum.db");
        let svc = WatchdogConfigService::open(&db_path).unwrap();
        {
            let conn = svc.conn.lock().unwrap();
            conn.execute_batch("PRAGMA busy_timeout = 0;").unwrap();
        }

        let cfg = WatchdogConfig {
            enabled: true,
            background_service: true,
            min_free_gb: 30,
            poll_interval_secs: 120,
            cooldown_secs: 900,
            updated_at_secs: 1_700_000_777,
        };
        with_contended_write_lock(&db_path, Duration::from_millis(120), || {
            svc.save_config(&cfg).unwrap();
        });
        assert_eq!(svc.load_config().unwrap(), cfg);
    }

    #[test]
    fn save_state_retries_on_sqlite_busy_with_zero_busy_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("ruum.db");
        let svc = WatchdogConfigService::open(&db_path).unwrap();
        {
            let conn = svc.conn.lock().unwrap();
            conn.execute_batch("PRAGMA busy_timeout = 0;").unwrap();
        }

        let state = WatchdogState {
            last_checked_secs: Some(1_700_000_888),
            last_triggered_secs: Some(1_700_000_800),
            last_triggered_threshold_gb: Some(25),
        };
        with_contended_write_lock(&db_path, Duration::from_millis(120), || {
            svc.save_state(&state).unwrap();
        });
        assert_eq!(svc.load_state().unwrap(), state);
    }
}
