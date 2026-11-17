//! Cohesive disk space watchdog service layer for `ruum`.
//!
//! Provides:
//! - [`WatchdogConfigService`] (`config` module): owns `watchdog_config` and
//!   `watchdog_state` inside `<config_dir>/ruum/ruum.db` (`WAL` +
//!   `busy_timeout = 5000`).
//! - [`service`] module: tri-platform OS background service lifecycle
//!   (`launchd`, `systemd --user`, Windows `HKCU\...\Run` + detached daemon).
//! - Event-driven $O(1)$ space monitoring (`evaluate_tick`,
//!   `evaluate_and_emit`, `run_once`, `run_loop`, [`WatchdogEngine`]) emitting
//!   [`WatchdogEvent::LowDiskSpace`] to an injected [`WatchdogEventSink`].

pub mod config;
pub mod service;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use config::{
    DEFAULT_COOLDOWN_SECS, DEFAULT_MIN_FREE_GB, DEFAULT_POLL_INTERVAL_SECS,
    MIN_COOLDOWN_SECS, MIN_FREE_GB_FLOOR, MIN_POLL_INTERVAL_SECS,
    WatchdogConfig, WatchdogConfigService, WatchdogState, config_dir,
    ruum_db_path, watchdog_pid_path,
};
pub use service::{
    LinuxServiceAdapter, MacOsServiceAdapter, ServiceAdapter,
    ServiceAdapterFactory, ServiceCommandOutput, ServiceEnv, ServiceOs,
    ServiceRunner, ServiceStatus, WindowsServiceAdapter, install_service,
    linux_systemd_service_path, linux_systemd_unit_content,
    macos_launchd_plist_content, macos_launchd_plist_path, service_status,
    uninstall_service,
};

const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;

/// $O(1)$ filesystem free-space measurement for a mount path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceSample {
    pub available_bytes: u64,
    pub total_bytes: u64,
}

/// Abstract $O(1)$ volume free-space probe.
pub trait SpaceProbe: Send + Sync {
    fn probe(&self, path: &Path) -> io::Result<SpaceSample>;
}

/// Result of evaluating disk space for a single polling tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickResult {
    /// Available space is above threshold, or watchdog is disabled.
    Ok {
        available_bytes: u64,
        total_bytes: u64,
        threshold_bytes: u64,
    },
    /// Available space is below threshold, but alert is in cooldown.
    InCooldown {
        available_bytes: u64,
        total_bytes: u64,
        threshold_bytes: u64,
        remaining_cooldown_secs: u64,
    },
    /// Available space is below threshold and cooldown has expired.
    Breached {
        available_bytes: u64,
        total_bytes: u64,
        threshold_bytes: u64,
    },
}

/// Domain events emitted by the watchdog service layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchdogEvent {
    LowDiskSpace {
        timestamp_secs: u64,
        mount_path: PathBuf,
        available_bytes: u64,
        total_bytes: u64,
        threshold_bytes: u64,
    },
}

/// Event consumer receiving watchdog alerts (notifications, webhooks, AI).
pub trait WatchdogEventSink: Send + Sync {
    fn on_event(&self, event: &WatchdogEvent) -> io::Result<()>;
}

/// Evaluate volume free space against threshold and cooldown in $O(1)$.
pub fn evaluate_tick(
    config: &WatchdogConfig,
    state: &WatchdogState,
    mount_path: &Path,
    now_secs: u64,
    probe: &dyn SpaceProbe,
) -> io::Result<TickResult> {
    if !config.enabled {
        return Ok(TickResult::Ok {
            available_bytes: 0,
            total_bytes: 0,
            threshold_bytes: 0,
        });
    }

    let norm = config.normalized();
    let sample = probe.probe(mount_path)?;
    let threshold_bytes = norm.min_free_gb.saturating_mul(BYTES_PER_GIB);

    if sample.available_bytes >= threshold_bytes {
        return Ok(TickResult::Ok {
            available_bytes: sample.available_bytes,
            total_bytes: sample.total_bytes,
            threshold_bytes,
        });
    }

    let same_threshold = state
        .last_triggered_threshold_gb
        .is_none_or(|prev| prev == norm.min_free_gb);
    if same_threshold && let Some(last_trig) = state.last_triggered_secs {
        let elapsed = now_secs.saturating_sub(last_trig);
        if elapsed < norm.cooldown_secs {
            let remaining = norm.cooldown_secs.saturating_sub(elapsed);
            return Ok(TickResult::InCooldown {
                available_bytes: sample.available_bytes,
                total_bytes: sample.total_bytes,
                threshold_bytes,
                remaining_cooldown_secs: remaining,
            });
        }
    }

    Ok(TickResult::Breached {
        available_bytes: sample.available_bytes,
        total_bytes: sample.total_bytes,
        threshold_bytes,
    })
}

/// Evaluate a tick against in-memory config/state, update cooldown timestamps
/// on breach, and emit [`WatchdogEvent::LowDiskSpace`] to `sink`.
pub fn evaluate_and_emit(
    config: &WatchdogConfig,
    state: &mut WatchdogState,
    mount_path: &Path,
    now_secs: u64,
    probe: &dyn SpaceProbe,
    sink: &dyn WatchdogEventSink,
) -> io::Result<TickResult> {
    state.last_checked_secs = Some(now_secs);
    let tick = evaluate_tick(config, state, mount_path, now_secs, probe)?;

    if let TickResult::Breached {
        available_bytes,
        total_bytes,
        threshold_bytes,
    } = tick
    {
        let norm = config.normalized();
        state.last_triggered_secs = Some(now_secs);
        state.last_triggered_threshold_gb = Some(norm.min_free_gb);
        let event = WatchdogEvent::LowDiskSpace {
            timestamp_secs: now_secs,
            mount_path: mount_path.to_path_buf(),
            available_bytes,
            total_bytes,
            threshold_bytes,
        };
        sink.on_event(&event)?;
    }

    Ok(tick)
}

/// Execute a single watchdog evaluation using [`WatchdogConfigService`] for
/// config and cooldown persistence.
///
/// Persists `last_triggered_secs` and `last_triggered_threshold_gb` before
/// invoking `sink` so a failing or slow downstream handler cannot cause alert
/// storms on subsequent ticks.
pub fn run_once(
    config_service: &WatchdogConfigService,
    mount_path: &Path,
    now_secs: u64,
    probe: &dyn SpaceProbe,
    sink: &dyn WatchdogEventSink,
) -> io::Result<TickResult> {
    let config = config_service.load_config()?;
    let mut state = config_service.load_state()?;

    state.last_checked_secs = Some(now_secs);
    let tick = evaluate_tick(&config, &state, mount_path, now_secs, probe)?;

    let TickResult::Breached {
        available_bytes,
        total_bytes,
        threshold_bytes,
    } = tick
    else {
        config_service.save_state(&state)?;
        return Ok(tick);
    };

    let norm = config.normalized();
    state.last_triggered_secs = Some(now_secs);
    state.last_triggered_threshold_gb = Some(norm.min_free_gb);
    config_service.save_state(&state)?;

    let event = WatchdogEvent::LowDiskSpace {
        timestamp_secs: now_secs,
        mount_path: mount_path.to_path_buf(),
        available_bytes,
        total_bytes,
        threshold_bytes,
    };
    sink.on_event(&event)?;

    Ok(tick)
}

struct PidFileGuard {
    path: PathBuf,
    pid: u32,
}

impl PidFileGuard {
    fn acquire(path: &Path) -> Self {
        let pid = std::process::id();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, format!("{pid}\n"));
        Self {
            path: path.to_path_buf(),
            pid,
        }
    }
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        if let Ok(content) = fs::read_to_string(&self.path)
            && content.trim().parse::<u32>().ok() == Some(self.pid)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Run the background polling loop until `cancel` becomes `true`.
///
/// Wakes up early within ~1s whenever the persisted [`WatchdogConfig`] in
/// `ruum.db` changes.
pub fn run_loop(
    config_service: &WatchdogConfigService,
    mount_path: &Path,
    pid_path: Option<&Path>,
    cancel: &AtomicBool,
    probe: &dyn SpaceProbe,
    sink: &dyn WatchdogEventSink,
) -> io::Result<()> {
    let _pid_guard = pid_path.map(PidFileGuard::acquire);

    while !cancel.load(Ordering::Relaxed) {
        let active_config = config_service.load_config().unwrap_or_default();
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let _ = run_once(config_service, mount_path, now_secs, probe, sink);

        let sleep_secs = active_config.normalized().poll_interval_secs;
        for _ in 0..sleep_secs {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(latest) = config_service.load_config()
                && latest != active_config
            {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    Ok(())
}

/// Stateful wrapper combining [`WatchdogConfigService`] with the polling engine.
#[derive(Debug)]
pub struct WatchdogEngine {
    config_service: WatchdogConfigService,
}

impl WatchdogEngine {
    /// Wrap an already-opened [`WatchdogConfigService`].
    #[must_use]
    pub const fn new(config_service: WatchdogConfigService) -> Self {
        Self { config_service }
    }

    /// Open `ruum.db` at `db_path` and construct a [`WatchdogEngine`].
    pub fn open(db_path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self::new(WatchdogConfigService::open(db_path)?))
    }

    /// Access the underlying [`WatchdogConfigService`].
    #[must_use]
    pub const fn config_service(&self) -> &WatchdogConfigService {
        &self.config_service
    }

    /// Run a single evaluation tick against `mount_path`.
    pub fn run_once(
        &self,
        mount_path: &Path,
        now_secs: u64,
        probe: &dyn SpaceProbe,
        sink: &dyn WatchdogEventSink,
    ) -> io::Result<TickResult> {
        run_once(&self.config_service, mount_path, now_secs, probe, sink)
    }

    /// Run the continuous polling loop against `mount_path`.
    pub fn run_loop(
        &self,
        mount_path: &Path,
        pid_path: Option<&Path>,
        cancel: &AtomicBool,
        probe: &dyn SpaceProbe,
        sink: &dyn WatchdogEventSink,
    ) -> io::Result<()> {
        run_loop(
            &self.config_service,
            mount_path,
            pid_path,
            cancel,
            probe,
            sink,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    struct FixedProbe {
        sample: SpaceSample,
        calls: AtomicUsize,
    }

    impl FixedProbe {
        const fn new(available_bytes: u64, total_bytes: u64) -> Self {
            Self {
                sample: SpaceSample {
                    available_bytes,
                    total_bytes,
                },
                calls: AtomicUsize::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    impl SpaceProbe for FixedProbe {
        fn probe(&self, _path: &Path) -> io::Result<SpaceSample> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.sample)
        }
    }

    #[derive(Default)]
    struct CapturingSink {
        events: Mutex<Vec<WatchdogEvent>>,
        fail: bool,
    }

    impl CapturingSink {
        fn events(&self) -> Vec<WatchdogEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    impl WatchdogEventSink for CapturingSink {
        fn on_event(&self, event: &WatchdogEvent) -> io::Result<()> {
            self.events.lock().unwrap().push(event.clone());
            if self.fail {
                Err(io::Error::other("sink error"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn evaluate_tick_disabled_and_threshold_cooldown_rules() {
        let mount = Path::new("/home/tester");
        let probe = FixedProbe::new(5 * BYTES_PER_GIB, 100 * BYTES_PER_GIB);

        // Disabled skips probing entirely.
        let disabled = WatchdogConfig {
            enabled: false,
            min_free_gb: 20,
            ..WatchdogConfig::default()
        };
        let state = WatchdogState::default();
        assert_eq!(
            evaluate_tick(&disabled, &state, mount, 1_000, &probe).unwrap(),
            TickResult::Ok {
                available_bytes: 0,
                total_bytes: 0,
                threshold_bytes: 0,
            }
        );
        assert_eq!(probe.call_count(), 0);

        // Enabled and above threshold -> Ok.
        let ok_cfg = WatchdogConfig {
            enabled: true,
            min_free_gb: 4,
            cooldown_secs: 600,
            ..WatchdogConfig::default()
        };
        assert_eq!(
            evaluate_tick(&ok_cfg, &state, mount, 1_000, &probe).unwrap(),
            TickResult::Ok {
                available_bytes: 5 * BYTES_PER_GIB,
                total_bytes: 100 * BYTES_PER_GIB,
                threshold_bytes: 4 * BYTES_PER_GIB,
            }
        );

        // Enabled and below threshold -> Breached.
        let low_cfg = WatchdogConfig {
            enabled: true,
            min_free_gb: 20,
            cooldown_secs: 600,
            ..WatchdogConfig::default()
        };
        assert_eq!(
            evaluate_tick(&low_cfg, &state, mount, 1_000, &probe).unwrap(),
            TickResult::Breached {
                available_bytes: 5 * BYTES_PER_GIB,
                total_bytes: 100 * BYTES_PER_GIB,
                threshold_bytes: 20 * BYTES_PER_GIB,
            }
        );

        // Within cooldown for same threshold -> InCooldown.
        let cd_state = WatchdogState {
            last_checked_secs: Some(1_000),
            last_triggered_secs: Some(1_000),
            last_triggered_threshold_gb: Some(20),
        };
        assert_eq!(
            evaluate_tick(&low_cfg, &cd_state, mount, 1_250, &probe).unwrap(),
            TickResult::InCooldown {
                available_bytes: 5 * BYTES_PER_GIB,
                total_bytes: 100 * BYTES_PER_GIB,
                threshold_bytes: 20 * BYTES_PER_GIB,
                remaining_cooldown_secs: 350,
            }
        );

        // Cooldown expired -> Breached.
        assert_eq!(
            evaluate_tick(&low_cfg, &cd_state, mount, 1_600, &probe).unwrap(),
            TickResult::Breached {
                available_bytes: 5 * BYTES_PER_GIB,
                total_bytes: 100 * BYTES_PER_GIB,
                threshold_bytes: 20 * BYTES_PER_GIB,
            }
        );

        // Threshold changed -> bypasses cooldown.
        let changed_cfg = WatchdogConfig {
            min_free_gb: 25,
            ..low_cfg
        };
        assert_eq!(
            evaluate_tick(&changed_cfg, &cd_state, mount, 1_250, &probe)
                .unwrap(),
            TickResult::Breached {
                available_bytes: 5 * BYTES_PER_GIB,
                total_bytes: 100 * BYTES_PER_GIB,
                threshold_bytes: 25 * BYTES_PER_GIB,
            }
        );
    }

    #[test]
    fn evaluate_and_emit_triggers_once_and_skips_in_cooldown() {
        let mount = Path::new("/data");
        let probe = FixedProbe::new(2 * BYTES_PER_GIB, 50 * BYTES_PER_GIB);
        let sink = CapturingSink::default();
        let cfg = WatchdogConfig {
            enabled: true,
            min_free_gb: 10,
            cooldown_secs: 300,
            ..WatchdogConfig::default()
        };
        let mut state = WatchdogState::default();

        let t1 =
            evaluate_and_emit(&cfg, &mut state, mount, 2_000, &probe, &sink)
                .unwrap();
        assert!(matches!(t1, TickResult::Breached { .. }));
        assert_eq!(state.last_checked_secs, Some(2_000));
        assert_eq!(state.last_triggered_secs, Some(2_000));
        assert_eq!(state.last_triggered_threshold_gb, Some(10));
        assert_eq!(
            sink.events(),
            vec![WatchdogEvent::LowDiskSpace {
                timestamp_secs: 2_000,
                mount_path: mount.to_path_buf(),
                available_bytes: 2 * BYTES_PER_GIB,
                total_bytes: 50 * BYTES_PER_GIB,
                threshold_bytes: 10 * BYTES_PER_GIB,
            }]
        );

        // Second tick 100s later is in cooldown -> no additional event.
        let t2 =
            evaluate_and_emit(&cfg, &mut state, mount, 2_100, &probe, &sink)
                .unwrap();
        assert!(matches!(
            t2,
            TickResult::InCooldown {
                remaining_cooldown_secs: 200,
                ..
            }
        ));
        assert_eq!(state.last_checked_secs, Some(2_100));
        assert_eq!(state.last_triggered_secs, Some(2_000));
        assert_eq!(sink.events().len(), 1);

        // After 300s cooldown expires -> emits again.
        let t3 =
            evaluate_and_emit(&cfg, &mut state, mount, 2_300, &probe, &sink)
                .unwrap();
        assert!(matches!(t3, TickResult::Breached { .. }));
        assert_eq!(state.last_triggered_secs, Some(2_300));
        assert_eq!(sink.events().len(), 2);
    }

    #[test]
    fn run_once_persists_cooldown_before_sink_and_respects_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("ruum.db");
        let engine = WatchdogEngine::open(&db_path).unwrap();
        let mount = temp.path();

        let probe = FixedProbe::new(3 * BYTES_PER_GIB, 64 * BYTES_PER_GIB);
        let sink = CapturingSink::default();

        // Disabled by default: records check timestamp only, emits 0 events.
        let disabled_tick =
            engine.run_once(mount, 1_000, &probe, &sink).unwrap();
        assert!(matches!(disabled_tick, TickResult::Ok { .. }));
        assert_eq!(probe.call_count(), 0);
        assert!(sink.events().is_empty());
        let st0 = engine.config_service().load_state().unwrap();
        assert_eq!(st0.last_checked_secs, Some(1_000));
        assert_eq!(st0.last_triggered_secs, None);

        // Enable watchdog and trigger breach with a failing sink.
        engine
            .config_service()
            .save_config(&WatchdogConfig {
                enabled: true,
                min_free_gb: 20,
                cooldown_secs: 600,
                ..WatchdogConfig::default()
            })
            .unwrap();
        let failing_sink = CapturingSink {
            events: Mutex::new(Vec::new()),
            fail: true,
        };
        let err = engine
            .run_once(mount, 2_000, &probe, &failing_sink)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert_eq!(failing_sink.events().len(), 1);

        // Cooldown must already be persisted in SQLite despite sink error!
        let st1 = engine.config_service().load_state().unwrap();
        assert_eq!(st1.last_checked_secs, Some(2_000));
        assert_eq!(st1.last_triggered_secs, Some(2_000));
        assert_eq!(st1.last_triggered_threshold_gb, Some(20));

        // Next tick within cooldown skips sink completely.
        let tick2 = engine.run_once(mount, 2_100, &probe, &sink).unwrap();
        assert!(matches!(tick2, TickResult::InCooldown { .. }));
        assert!(sink.events().is_empty());
    }

    #[test]
    fn run_loop_writes_cleans_pid_and_wakes_on_sqlite_config_update() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("ruum.db");
        let pid_path = temp.path().join("watchdog.pid");
        let svc = WatchdogConfigService::open(&db_path).unwrap();
        svc.save_config(&WatchdogConfig {
            enabled: false,
            min_free_gb: 20,
            poll_interval_secs: 60,
            ..WatchdogConfig::default()
        })
        .unwrap();

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let cancel_bg = std::sync::Arc::clone(&cancel);
        let db_bg = db_path;
        let pid_bg = pid_path.clone();
        let mount_bg = temp.path().to_path_buf();

        let handle = std::thread::spawn(move || {
            let svc_bg = WatchdogConfigService::open(&db_bg).unwrap();
            let probe =
                FixedProbe::new(100 * BYTES_PER_GIB, 200 * BYTES_PER_GIB);
            let sink = CapturingSink::default();
            run_loop(
                &svc_bg,
                &mount_bg,
                Some(&pid_bg),
                &cancel_bg,
                &probe,
                &sink,
            )
        });

        let start = std::time::Instant::now();
        while !pid_path.exists()
            || svc.load_state().unwrap().last_checked_secs.is_none()
        {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "run_loop did not write pid/state in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_to_string(&pid_path)
                .unwrap()
                .trim()
                .parse::<u32>()
                .ok(),
            Some(std::process::id())
        );
        let first_checked = svc.load_state().unwrap().last_checked_secs;

        std::thread::sleep(Duration::from_millis(1100));
        svc.save_config(&WatchdogConfig {
            enabled: false,
            min_free_gb: 99,
            poll_interval_secs: 60,
            updated_at_secs: 42,
            ..WatchdogConfig::default()
        })
        .unwrap();

        let wake_start = std::time::Instant::now();
        loop {
            let checked = svc.load_state().unwrap().last_checked_secs;
            if checked > first_checked {
                break;
            }
            assert!(
                wake_start.elapsed() < Duration::from_secs(3),
                "run_loop did not wake within <= 3s on SQLite config change"
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        cancel.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
        assert!(!pid_path.exists());
    }

    #[test]
    fn pid_file_guard_preserves_newer_pid_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let pid_path = temp.path().join("watchdog.pid");
        {
            let _guard = PidFileGuard::acquire(&pid_path);
            assert!(pid_path.exists());
            fs::write(&pid_path, "999999\n").unwrap();
        }
        assert_eq!(fs::read_to_string(&pid_path).unwrap().trim(), "999999");
    }
}
