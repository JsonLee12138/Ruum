//! Low-overhead disk space watchdog and service registration.
//!
//! Evaluates volume free space using lightweight $O(1)$ [`crate::space::space_info`]
//! calls against the user's home directory. When the threshold is breached and
//! the cooldown period has elapsed, the watchdog emits a notification, invokes
//! the configured AI assistant to generate a proposal, requests confirmation
//! via [`crate::confirm::CleanupConfirmAdapter`], and executes approved
//! cleanups without unattended file deletion.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ruum_watchdog::{
    SpaceProbe, SpaceSample, WatchdogEvent, WatchdogEventSink,
};

use crate::ai_cli::{self, CommandRunner, DefaultCommandRunner};
use crate::confirm::{
    CleanupConfirmAdapter, CleanupConfirmDecision, CleanupConfirmItem,
    CleanupConfirmRequest, GuiCleanupConfirm,
};
use crate::i18n::{self, Locale};
use crate::notify::resolve_ipc_target_and_token;
pub use crate::notify::{
    NOTIF_TAG_AI_ANALYSIS, NOTIF_TAG_CLEANUP_DONE, NOTIF_TAG_CLEANUP_STARTED,
    NOTIF_TAG_LOW_SPACE, NotificationAdapter, NotificationEnv,
    NotificationFactory, NotificationOs, OsNotificationAdapter,
    notify_gui_watchdog_update, notify_gui_watchdog_update_with,
    send_fallback_os_notification, send_os_notification, try_ipc_notification,
    try_ipc_notification_with, try_ipc_open_window, try_ipc_open_window_with,
};
use crate::removal::{RemovalEvent, RemovalMode, Target};
use crate::storage::{AiState, StorageDb};
use crate::watchdog_config::{
    CleanupProposal, ProposalItem, WatchdogConfig, WatchdogState, load_config,
    load_state, save_state,
};
use crate::webhook::{
    LowSpaceWebhookPayload, send_low_space_webhook, send_low_space_webhooks,
};

/// Result of evaluating disk space for a single polling tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TickResult {
    /// Available space is above threshold, or watchdog is disabled.
    Ok {
        available_bytes: u64,
        threshold_bytes: u64,
    },
    /// Available space is below threshold, but notification is in cooldown.
    InCooldown {
        available_bytes: u64,
        threshold_bytes: u64,
        remaining_cooldown_secs: u64,
    },
    /// Available space is below threshold and cooldown has expired.
    Breached {
        available_bytes: u64,
        threshold_bytes: u64,
    },
}

/// [`SpaceProbe`] adapter backed by [`crate::space::space_info`].
#[derive(Clone, Copy, Debug, Default)]
pub struct CoreSpaceProbe;

impl SpaceProbe for CoreSpaceProbe {
    fn probe(&self, path: &Path) -> io::Result<SpaceSample> {
        let space = crate::space::space_info(path)?;
        Ok(SpaceSample {
            available_bytes: space.available,
            total_bytes: space.total,
        })
    }
}

/// Core event sink handling [`WatchdogEvent::LowDiskSpace`] alerts:
///
/// 1. Sends a core OS/IPC notification (`notify`).
/// 2. Delivers a generic JSON webhook if configured and enabled (`webhook`).
/// 3. Reads `ai_config` from `ruum.db`; if `auto_analyze_on_alert` is true,
///    runs `ai_cli`, persists `ai_state.last_proposal_json`, notifies GUI via
///    IPC, and executes user-confirmed items via [`CleanupConfirmAdapter`].
pub struct CoreWatchdogEventHandler<'a> {
    db: &'a StorageDb,
    current_exe: PathBuf,
    cmd_runner: &'a dyn CommandRunner,
    confirm_adapter: &'a dyn CleanupConfirmAdapter,
    notifier: &'a dyn NotificationAdapter,
    locale: Locale,
    ipc_notify_target: Option<(PathBuf, String)>,
}

impl fmt::Debug for CoreWatchdogEventHandler<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreWatchdogEventHandler")
            .field("db", &self.db)
            .field("current_exe", &self.current_exe)
            .field("locale", &self.locale)
            .field(
                "has_ipc_target",
                &self.ipc_notify_target.as_ref().map(|(p, _)| p.clone()),
            )
            .finish_non_exhaustive()
    }
}

impl<'a> CoreWatchdogEventHandler<'a> {
    /// Construct a new [`CoreWatchdogEventHandler`] wired to `db` and adapters.
    #[must_use]
    pub fn new(
        db: &'a StorageDb,
        current_exe: PathBuf,
        cmd_runner: &'a dyn CommandRunner,
        confirm_adapter: &'a dyn CleanupConfirmAdapter,
        notifier: &'a dyn NotificationAdapter,
    ) -> Self {
        Self {
            db,
            current_exe,
            cmd_runner,
            confirm_adapter,
            notifier,
            locale: Locale::current(),
            ipc_notify_target: resolve_ipc_target_and_token(),
        }
    }

    /// Override localization used for notification titles/bodies.
    #[must_use]
    pub const fn with_locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    /// Override (or disable with `None`) the GUI IPC notification endpoint.
    #[must_use]
    pub fn with_ipc_target(
        mut self,
        target: Option<(PathBuf, String)>,
    ) -> Self {
        self.ipc_notify_target = target;
        self
    }

    fn send_ipc_watchdog_update(&self) {
        if let Some((ref target, ref token)) = self.ipc_notify_target {
            let _ = notify_gui_watchdog_update_with(target, token);
        }
    }
}

impl WatchdogEventSink for CoreWatchdogEventHandler<'_> {
    fn on_event(&self, event: &WatchdogEvent) -> io::Result<()> {
        let WatchdogEvent::LowDiskSpace {
            timestamp_secs,
            mount_path,
            available_bytes,
            total_bytes,
            threshold_bytes,
        } = event;

        let min_gb = threshold_bytes / (1024 * 1024 * 1024);
        if let Err(err) = self.notifier.notify_tagged(
            NOTIF_TAG_LOW_SPACE,
            i18n::low_disk_space_title(self.locale),
            &i18n::low_disk_space_body(*available_bytes, min_gb, self.locale),
        ) {
            eprintln!("ruum watchdog: notification error: {err}");
        }

        let endpoints = self.db.list_webhooks()?;
        let webhook_payload = LowSpaceWebhookPayload::from_low_space_event(
            *timestamp_secs,
            mount_path,
            *available_bytes,
            *total_bytes,
            *threshold_bytes,
        );
        if endpoints.is_empty() {
            let webhook_cfg = self.db.load_webhook_config()?;
            if let Err(err) =
                send_low_space_webhook(&webhook_cfg, &webhook_payload)
            {
                eprintln!("ruum watchdog: webhook delivery error: {err}");
            }
        } else {
            for (endpoint_id, res) in
                send_low_space_webhooks(&endpoints, &webhook_payload)
            {
                if let Err(err) = res {
                    eprintln!(
                        "ruum watchdog: webhook `{endpoint_id}` delivery \
                         error: {err}"
                    );
                }
            }
        }

        let ai_cfg = self.db.load_ai_config()?;
        if !ai_cfg.auto_analyze_on_alert {
            self.send_ipc_watchdog_update();
            return Ok(());
        }

        let proposal = match ai_cli::run_analysis_for_config(
            &ai_cfg,
            mount_path,
            &self.current_exe,
            *timestamp_secs,
            self.cmd_runner,
        ) {
            Ok(prop) => {
                let ai_state = AiState::from_proposal(&prop, *timestamp_secs)?;
                self.db.save_ai_state(&ai_state)?;
                self.send_ipc_watchdog_update();
                Some(prop)
            }
            Err(err) => {
                eprintln!("ruum watchdog: AI analysis error: {err}");
                self.send_ipc_watchdog_update();
                None
            }
        };

        if let Some(prop) = proposal.as_ref().filter(|p| !p.items.is_empty()) {
            let confirm_items = prop
                .items
                .iter()
                .map(|i| CleanupConfirmItem {
                    path: i.path.clone(),
                    bytes: i.bytes,
                    mode: i.mode,
                    reason: i.reason.clone(),
                    git_warning: i.git_warning.clone(),
                })
                .collect();

            let request = CleanupConfirmRequest {
                root: mount_path.clone(),
                title: i18n::recommended_cleanup_title(self.locale).to_string(),
                summary: Some(prop.summary.clone()),
                items: confirm_items,
                docker_items: Vec::new(),
                docker_permission_hint: None,
            };

            if let Ok(CleanupConfirmDecision::Confirmed {
                selected_paths,
                ..
            }) = self.confirm_adapter.confirm(&request)
            {
                let approved_items: Vec<ProposalItem> = prop
                    .items
                    .iter()
                    .filter(|item| selected_paths.contains(&item.path))
                    .cloned()
                    .collect();

                if !approved_items.is_empty() {
                    let (reclaimed_bytes, removed_items, cleaned_paths) =
                        execute_confirmed_cleanup_tracked(
                            &approved_items,
                            mount_path,
                        );
                    if !cleaned_paths.is_empty() {
                        if let Err(err) = self.db.prune_ai_state_paths(
                            &cleaned_paths,
                            *timestamp_secs,
                        ) {
                            eprintln!(
                                "ruum watchdog: failed to prune ai_state: {err}"
                            );
                        }
                        self.send_ipc_watchdog_update();
                    }
                    if removed_items > 0 {
                        let _ = self.notifier.notify_tagged(
                            NOTIF_TAG_CLEANUP_DONE,
                            i18n::cleanup_complete_title(self.locale),
                            &i18n::cleanup_complete_body(
                                removed_items,
                                reclaimed_bytes,
                                self.locale,
                            ),
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

/// Run a single `SQLite`-backed watchdog evaluation tick via `ruum-watchdog`
/// and [`CoreWatchdogEventHandler`].
#[allow(
    clippy::too_many_arguments,
    reason = "explicit adapter injection for deterministic tests"
)]
pub fn run_once_with_db(
    db: &StorageDb,
    mount_path: &Path,
    current_exe: &Path,
    now_secs: u64,
    cmd_runner: &dyn CommandRunner,
    confirm_adapter: &dyn CleanupConfirmAdapter,
    notifier: &dyn NotificationAdapter,
) -> io::Result<ruum_watchdog::TickResult> {
    let wd_service = db.watchdog()?;
    let handler = CoreWatchdogEventHandler::new(
        db,
        current_exe.to_path_buf(),
        cmd_runner,
        confirm_adapter,
        notifier,
    );
    ruum_watchdog::run_once(
        &wd_service,
        mount_path,
        now_secs,
        &CoreSpaceProbe,
        &handler,
    )
}

/// Run the `SQLite`-backed watchdog polling loop via `ruum-watchdog` and
/// [`CoreWatchdogEventHandler`].
#[allow(
    clippy::too_many_arguments,
    reason = "explicit adapter injection for deterministic tests"
)]
pub fn run_loop_with_db(
    db: &StorageDb,
    mount_path: &Path,
    pid_path: Option<&Path>,
    current_exe: &Path,
    cancel: &AtomicBool,
    cmd_runner: &dyn CommandRunner,
    confirm_adapter: &dyn CleanupConfirmAdapter,
    notifier: &dyn NotificationAdapter,
) -> io::Result<()> {
    let wd_service = db.watchdog()?;
    let handler = CoreWatchdogEventHandler::new(
        db,
        current_exe.to_path_buf(),
        cmd_runner,
        confirm_adapter,
        notifier,
    );
    ruum_watchdog::run_loop(
        &wd_service,
        mount_path,
        pid_path,
        cancel,
        &CoreSpaceProbe,
        &handler,
    )
}

fn default_db_paths()
-> io::Result<(StorageDb, PathBuf, PathBuf, Option<PathBuf>)> {
    let db = StorageDb::open_default()?;
    let home_dir = std::env::home_dir().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no home directory")
    })?;
    let current_exe = std::env::current_exe()?;
    let pid_path = ruum_watchdog::watchdog_pid_path();
    Ok((db, home_dir, current_exe, pid_path))
}

/// Run a single evaluation tick against the default `ruum.db` and adapters.
pub fn run_default_once_with_db() -> io::Result<ruum_watchdog::TickResult> {
    let (db, home_dir, current_exe, _) = default_db_paths()?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let runner = DefaultCommandRunner;
    let confirm = GuiCleanupConfirm::new();
    let notifier = NotificationFactory::create();
    run_once_with_db(
        &db,
        &home_dir,
        &current_exe,
        now_secs,
        &runner,
        &confirm,
        notifier.as_ref(),
    )
}

/// Run the background polling loop against the default `ruum.db` and adapters.
pub fn run_default_loop_with_db(cancel: &AtomicBool) -> io::Result<()> {
    let (db, home_dir, current_exe, pid_path) = default_db_paths()?;
    let runner = DefaultCommandRunner;
    let confirm = GuiCleanupConfirm::new();
    let notifier = NotificationFactory::create();
    run_loop_with_db(
        &db,
        &home_dir,
        pid_path.as_deref(),
        &current_exe,
        cancel,
        &runner,
        &confirm,
        notifier.as_ref(),
    )
}

/// Evaluate volume free space against threshold and cooldown in $O(1)$.
pub fn evaluate_tick(
    config: &WatchdogConfig,
    state: &WatchdogState,
    home_dir: &Path,
    now_secs: u64,
) -> io::Result<TickResult> {
    if !config.enabled {
        return Ok(TickResult::Ok {
            available_bytes: 0,
            threshold_bytes: 0,
        });
    }

    let norm = config.normalized();
    let space = crate::space::space_info(home_dir)?;
    let threshold_bytes = norm
        .min_free_gb
        .saturating_mul(1024)
        .saturating_mul(1024)
        .saturating_mul(1024);

    if space.available >= threshold_bytes {
        return Ok(TickResult::Ok {
            available_bytes: space.available,
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
                available_bytes: space.available,
                threshold_bytes,
                remaining_cooldown_secs: remaining,
            });
        }
    }

    Ok(TickResult::Breached {
        available_bytes: space.available,
        threshold_bytes,
    })
}

pub use crate::watchdog_service::{
    ServiceStatus, install_service, linux_systemd_service_path,
    linux_systemd_unit_content, macos_launchd_plist_content,
    macos_launchd_plist_path, service_status, uninstall_service,
};

/// Summary outcome of a single watchdog check execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchdogRunOutcome {
    pub tick: TickResult,
    pub proposal: Option<CleanupProposal>,
    pub confirmed: bool,
    pub reclaimed_bytes: u64,
    pub removed_items: usize,
}

/// Execute synchronous removal of items confirmed by the user.
pub fn execute_confirmed_cleanup(
    items: &[ProposalItem],
    home_dir: &Path,
) -> (u64, usize) {
    let (bytes, count, _) = execute_confirmed_cleanup_tracked(items, home_dir);
    (bytes, count)
}

/// Execute synchronous removal of items confirmed by the user, returning
/// `(reclaimed_bytes, removed_count, succeeded_paths)`.
#[must_use]
pub fn execute_confirmed_cleanup_tracked(
    items: &[ProposalItem],
    home_dir: &Path,
) -> (u64, usize, Vec<PathBuf>) {
    execute_confirmed_cleanup_with(items, home_dir, &mut |path, err| {
        eprintln!("ruum watchdog: failed to remove {}: {err}", path.display());
    })
}

fn execute_confirmed_cleanup_with(
    items: &[ProposalItem],
    home_dir: &Path,
    log_error: &mut dyn FnMut(&Path, &str),
) -> (u64, usize, Vec<PathBuf>) {
    let mut total_bytes = 0_u64;
    let mut total_count = 0_usize;
    let mut cleaned_paths = Vec::new();

    for mode in [RemovalMode::Trash, RemovalMode::Permanent] {
        let mode_items: Vec<&ProposalItem> =
            items.iter().filter(|i| i.mode == mode).collect();
        if mode_items.is_empty() {
            continue;
        }

        let targets: Vec<Target> = mode_items
            .iter()
            .map(|item| {
                let is_dir =
                    fs::symlink_metadata(&item.path).is_ok_and(|m| m.is_dir());
                let hidden = item
                    .path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                Target {
                    path: item.path.clone(),
                    bytes: item.bytes,
                    is_dir,
                    hidden,
                }
            })
            .collect();

        let plan = crate::removal::plan_for_mode(&targets, home_dir, mode);
        if plan.targets.is_empty() {
            continue;
        }

        let handle = crate::removal::spawn(plan, mode);
        let mut done = false;
        while !done {
            let mut progressed = false;
            while let Some(event) = handle.poll() {
                progressed = true;
                match event {
                    RemovalEvent::Done { removed, bytes, .. } => {
                        total_count += removed as usize;
                        total_bytes += bytes;
                        done = true;
                        break;
                    }
                    RemovalEvent::Item {
                        path,
                        outcome: Err(err),
                        ..
                    } => log_error(&path, &err),
                    RemovalEvent::Item {
                        path,
                        outcome: Ok(()),
                        ..
                    } => cleaned_paths.push(path),
                    RemovalEvent::Start { .. } => {}
                }
            }
            if !progressed && !done {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    (total_bytes, total_count, cleaned_paths)
}

/// Run a single watchdog evaluation, AI analysis, confirmation and cleanup.
#[allow(
    clippy::too_many_arguments,
    reason = "run_once takes explicit dependency-injected adapters for deterministic testing"
)]
pub fn run_once(
    config_path: &Path,
    state_path: &Path,
    home_dir: &Path,
    current_exe: &Path,
    now_secs: u64,
    cmd_runner: &dyn CommandRunner,
    confirm_adapter: &dyn CleanupConfirmAdapter,
    notifier: &dyn NotificationAdapter,
) -> io::Result<WatchdogRunOutcome> {
    run_once_in(
        config_path,
        state_path,
        home_dir,
        current_exe,
        now_secs,
        cmd_runner,
        confirm_adapter,
        notifier,
        Locale::current(),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "run_once_in isolates locale injection for deterministic tests"
)]
fn run_once_in(
    config_path: &Path,
    state_path: &Path,
    home_dir: &Path,
    current_exe: &Path,
    now_secs: u64,
    cmd_runner: &dyn CommandRunner,
    confirm_adapter: &dyn CleanupConfirmAdapter,
    notifier: &dyn NotificationAdapter,
    locale: Locale,
) -> io::Result<WatchdogRunOutcome> {
    let config = load_config(config_path)?;
    let mut state = load_state(state_path)?;

    state.last_checked_secs = Some(now_secs);
    let tick = evaluate_tick(&config, &state, home_dir, now_secs)?;

    let TickResult::Breached {
        available_bytes,
        threshold_bytes,
    } = tick
    else {
        save_state(state_path, &state)?;
        return Ok(WatchdogRunOutcome {
            tick,
            proposal: None,
            confirmed: false,
            reclaimed_bytes: 0,
            removed_items: 0,
        });
    };

    // 1. Notify user of low disk space
    let norm = config.normalized();
    let min_gb = threshold_bytes / (1024 * 1024 * 1024);
    let _ = notifier.notify_tagged(
        NOTIF_TAG_LOW_SPACE,
        i18n::low_disk_space_title(locale),
        &i18n::low_disk_space_body(available_bytes, min_gb, locale),
    );
    // Persist cooldown and threshold immediately so disabled or failing AI CLI
    // runs cannot cause notification or CLI storms on subsequent ticks.
    state.last_triggered_secs = Some(now_secs);
    state.last_triggered_threshold_gb = Some(norm.min_free_gb);
    save_state(state_path, &state)?;

    // 2. Invoke AI CLI for analysis if enabled
    let mut proposal = None;
    if config.ai_auto_analyze {
        match ai_cli::run_analysis(
            &config,
            home_dir,
            current_exe,
            now_secs,
            cmd_runner,
        ) {
            Ok(prop) => {
                state.last_proposal = Some(prop.clone());
                proposal = Some(prop);
                save_state(state_path, &state)?;
                notify_gui_watchdog_update();
            }
            Err(err) => {
                eprintln!("ruum watchdog: AI analysis error: {err}");
            }
        }
    }

    // 3. Trigger confirmation prompt if there are candidate items
    let mut confirmed = false;
    let mut reclaimed_bytes = 0_u64;
    let mut removed_items = 0_usize;

    if let Some(prop) = proposal.as_ref().filter(|p| !p.items.is_empty()) {
        let confirm_items = prop
            .items
            .iter()
            .map(|i| CleanupConfirmItem {
                path: i.path.clone(),
                bytes: i.bytes,
                mode: i.mode,
                reason: i.reason.clone(),
                git_warning: i.git_warning.clone(),
            })
            .collect();

        let request = CleanupConfirmRequest {
            root: home_dir.to_path_buf(),
            title: i18n::recommended_cleanup_title(locale).to_string(),
            summary: Some(prop.summary.clone()),
            items: confirm_items,
            docker_items: Vec::new(),
            docker_permission_hint: None,
        };

        if let Ok(CleanupConfirmDecision::Confirmed {
            selected_paths, ..
        }) = confirm_adapter.confirm(&request)
        {
            confirmed = true;
            // Whitelist intersection
            let approved_items: Vec<ProposalItem> = prop
                .items
                .iter()
                .filter(|item| selected_paths.contains(&item.path))
                .cloned()
                .collect();

            if !approved_items.is_empty() {
                let (bytes, count) =
                    execute_confirmed_cleanup(&approved_items, home_dir);
                reclaimed_bytes = bytes;
                removed_items = count;

                if removed_items > 0 {
                    let _ = notifier.notify_tagged(
                        NOTIF_TAG_CLEANUP_DONE,
                        i18n::cleanup_complete_title(locale),
                        &i18n::cleanup_complete_body(
                            removed_items,
                            reclaimed_bytes,
                            locale,
                        ),
                    );
                }
            }
        }
    }

    save_state(state_path, &state)?;
    notify_gui_watchdog_update();

    Ok(WatchdogRunOutcome {
        tick,
        proposal,
        confirmed,
        reclaimed_bytes,
        removed_items,
    })
}

struct PidFileGuard {
    path: PathBuf,
    pid: u32,
}

impl PidFileGuard {
    fn acquire(state_path: &Path) -> Self {
        let path = state_path.with_file_name("watchdog.pid");
        let pid = std::process::id();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, format!("{pid}\n"));
        Self { path, pid }
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

fn file_stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    Some((meta.len(), modified))
}

/// Run the background polling loop until interrupted.
#[allow(
    clippy::too_many_arguments,
    reason = "run_loop takes explicit dependency-injected adapters for deterministic testing"
)]
pub fn run_loop(
    config_path: &Path,
    state_path: &Path,
    home_dir: &Path,
    current_exe: &Path,
    cancel: &AtomicBool,
    runner: &dyn CommandRunner,
    confirm_adapter: &dyn CleanupConfirmAdapter,
    notifier: &dyn NotificationAdapter,
) -> io::Result<()> {
    let _pid_guard = PidFileGuard::acquire(state_path);
    while !cancel.load(Ordering::Relaxed) {
        let config_stamp = file_stamp(config_path);
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let _ = run_once(
            config_path,
            state_path,
            home_dir,
            current_exe,
            now_secs,
            runner,
            confirm_adapter,
            notifier,
        );

        let sleep_secs = load_config(config_path)
            .unwrap_or_default()
            .normalized()
            .poll_interval_secs;
        let sleep_deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(sleep_secs);
        loop {
            if cancel.load(Ordering::Relaxed)
                || file_stamp(config_path) != config_stamp
            {
                break;
            }
            let now = std::time::Instant::now();
            if now >= sleep_deadline {
                break;
            }
            let remaining = sleep_deadline.saturating_duration_since(now);
            std::thread::sleep(
                std::time::Duration::from_millis(200).min(remaining),
            );
        }
    }
    Ok(())
}

fn default_watchdog_paths() -> io::Result<(PathBuf, PathBuf, PathBuf, PathBuf)>
{
    let config_path = crate::watchdog_config::watchdog_config_path()
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no config path")
        })?;
    let state_path =
        crate::watchdog_config::watchdog_state_path().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no state path")
        })?;
    let home_dir = std::env::home_dir().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no home directory")
    })?;
    let current_exe = std::env::current_exe()?;
    Ok((config_path, state_path, home_dir, current_exe))
}

/// Run a single watchdog evaluation with default paths and a custom notifier.
pub fn run_default_once_with_notifier(
    notifier: &dyn NotificationAdapter,
) -> io::Result<WatchdogRunOutcome> {
    let (config_path, state_path, home_dir, current_exe) =
        default_watchdog_paths()?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let runner = crate::ai_cli::DefaultCommandRunner;
    let confirm = crate::confirm::GuiCleanupConfirm::new();
    run_once(
        &config_path,
        &state_path,
        &home_dir,
        &current_exe,
        now_secs,
        &runner,
        &confirm,
        notifier,
    )
}

/// Run the watchdog polling loop with default paths and a custom notifier.
pub fn run_default_loop_with_notifier(
    cancel: &AtomicBool,
    notifier: &dyn NotificationAdapter,
) -> io::Result<()> {
    let (config_path, state_path, home_dir, current_exe) =
        default_watchdog_paths()?;
    let runner = crate::ai_cli::DefaultCommandRunner;
    let confirm = crate::confirm::GuiCleanupConfirm::new();
    run_loop(
        &config_path,
        &state_path,
        &home_dir,
        &current_exe,
        cancel,
        &runner,
        &confirm,
        notifier,
    )
}

/// Convenience entrypoint for running a single watchdog evaluation with default
/// configuration paths and system adapters.
pub fn run_default_once() -> io::Result<WatchdogRunOutcome> {
    run_default_once_with_notifier(&OsNotificationAdapter)
}

/// Convenience entrypoint for running the watchdog polling loop with default
/// configuration paths and system adapters.
pub fn run_default_loop(cancel: &AtomicBool) -> io::Result<()> {
    run_default_loop_with_notifier(cancel, &OsNotificationAdapter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockNotification {
        notifications: Mutex<Vec<(String, String)>>,
        tagged: Mutex<Vec<(String, String, String)>>,
    }

    impl NotificationAdapter for MockNotification {
        fn notify_tagged(
            &self,
            tag: &str,
            title: &str,
            body: &str,
        ) -> io::Result<()> {
            self.notifications
                .lock()
                .unwrap()
                .push((title.to_string(), body.to_string()));
            self.tagged.lock().unwrap().push((
                tag.to_string(),
                title.to_string(),
                body.to_string(),
            ));
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockRunner {
        runs: Mutex<Vec<(PathBuf, Vec<String>)>>,
    }

    impl CommandRunner for MockRunner {
        fn run(
            &self,
            program: &Path,
            args: &[&str],
            _envs: &[(&str, &str)],
        ) -> io::Result<ai_cli::CommandOutput> {
            self.runs.lock().unwrap().push((
                program.to_path_buf(),
                args.iter().map(|s| (*s).to_string()).collect(),
            ));
            Ok(ai_cli::CommandOutput {
                status_code: 0,
                stdout: "active\n".to_string(),
                stderr: String::new(),
            })
        }
    }

    #[derive(Debug)]
    struct MockConfirm {
        decision: CleanupConfirmDecision,
    }

    impl CleanupConfirmAdapter for MockConfirm {
        fn confirm(
            &self,
            _request: &CleanupConfirmRequest,
        ) -> io::Result<CleanupConfirmDecision> {
            Ok(self.decision.clone())
        }
    }

    struct AiMock(String);

    impl CommandRunner for AiMock {
        fn run(
            &self,
            _program: &Path,
            _args: &[&str],
            _envs: &[(&str, &str)],
        ) -> io::Result<ai_cli::CommandOutput> {
            Ok(ai_cli::CommandOutput {
                status_code: 0,
                stdout: self.0.clone(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn evaluate_tick_disabled_and_thresholds() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();

        // 1. Watchdog disabled
        let cfg_disabled = WatchdogConfig {
            enabled: false,
            min_free_gb: 10,
            ..WatchdogConfig::default()
        };
        let state = WatchdogState::default();
        let res = evaluate_tick(&cfg_disabled, &state, home, 1000).unwrap();
        assert!(matches!(res, TickResult::Ok { .. }));

        // 2. High threshold triggers breached when free space is lower
        let cfg_enabled = WatchdogConfig {
            enabled: true,
            min_free_gb: 1_000_000, // 1 Petabyte (unlikely free)
            cooldown_secs: 3600,
            ..WatchdogConfig::default()
        };
        let res = evaluate_tick(&cfg_enabled, &state, home, 1000).unwrap();
        assert!(matches!(res, TickResult::Breached { .. }));

        // 3. Cooldown gating
        let state_triggered = WatchdogState {
            last_triggered_secs: Some(1000),
            ..WatchdogState::default()
        };
        // 500 seconds later (within 3600s cooldown)
        let res_cd =
            evaluate_tick(&cfg_enabled, &state_triggered, home, 1500).unwrap();
        assert!(matches!(
            res_cd,
            TickResult::InCooldown {
                remaining_cooldown_secs: 3100,
                ..
            }
        ));

        // 4. After cooldown passes (4000 seconds later)
        let res_after =
            evaluate_tick(&cfg_enabled, &state_triggered, home, 5000).unwrap();
        assert!(matches!(res_after, TickResult::Breached { .. }));
    }

    #[test]
    fn evaluate_tick_bypasses_cooldown_when_threshold_changes() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();

        let state = WatchdogState {
            last_triggered_secs: Some(1000),
            last_triggered_threshold_gb: Some(20),
            ..WatchdogState::default()
        };

        // Same threshold (1_000_000 vs 20 -> let's test matching 1_000_000)
        let state_same = WatchdogState {
            last_triggered_secs: Some(1000),
            last_triggered_threshold_gb: Some(1_000_000),
            ..WatchdogState::default()
        };
        let cfg = WatchdogConfig {
            enabled: true,
            min_free_gb: 1_000_000,
            cooldown_secs: 3600,
            ..WatchdogConfig::default()
        };
        let res_same = evaluate_tick(&cfg, &state_same, home, 1500).unwrap();
        assert!(matches!(res_same, TickResult::InCooldown { .. }));

        // Different prior threshold (20 GB != 1_000_000 GB) bypasses cooldown
        let res_changed = evaluate_tick(&cfg, &state, home, 1500).unwrap();
        assert!(matches!(res_changed, TickResult::Breached { .. }));
    }

    #[test]
    fn service_templates_generation() {
        let exe = Path::new("/usr/local/bin/ruum");

        let plist = macos_launchd_plist_content(exe);
        assert!(plist.contains("<string>com.ruum.watchdog</string>"));
        assert!(plist.contains("<string>/usr/local/bin/ruum</string>"));
        assert!(plist.contains("<string>watchdog</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains("<key>KeepAlive</key>"));

        let unit = linux_systemd_unit_content(exe);
        assert!(unit.contains("Description=Ruum Disk Space Watchdog"));
        assert!(unit.contains("ExecStart=/usr/local/bin/ruum watchdog run"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn run_once_breached_with_mock_components() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();

        let cfg_path = home.join("watchdog.json");
        let state_path = home.join("watchdog-state.json");

        // Target to clean
        let target_dir = home.join("cache").join("artifacts");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("temp.dat"), b"12345678").unwrap();

        let fake_ai_json = serde_json::json!({
            "summary": "Reclaim temp artifacts",
            "items": [{
                "path": target_dir.to_string_lossy(),
                "mode": "trash",
                "reason": "Test artifact",
            }],
        })
        .to_string();

        let runner = MockRunner::default();
        // Return fake AI proposal JSON on command
        *runner.runs.lock().unwrap() = Vec::new();

        let ai_runner = AiMock(fake_ai_json);

        // Create a fake executable to satisfy invocation
        let fake_exe = home.join("ruum");
        fs::write(&fake_exe, b"").unwrap();

        let cfg = WatchdogConfig {
            enabled: true,
            min_free_gb: 1_000_000, // force breach
            ai_auto_analyze: true,
            ai_provider: crate::watchdog_config::AiCliProvider::Custom,
            custom_command: fake_exe.to_string_lossy().to_string(),
            ..WatchdogConfig::default()
        };
        crate::watchdog_config::save_config(&cfg_path, &cfg).unwrap();

        let notifier = MockNotification::default();
        let confirm = MockConfirm {
            decision: CleanupConfirmDecision::Confirmed {
                selected_paths: vec![target_dir],
                selected_docker_targets: Vec::new(),
            },
        };

        let outcome = run_once(
            &cfg_path,
            &state_path,
            home,
            &fake_exe,
            1_700_000_000,
            &ai_runner,
            &confirm,
            &notifier,
        )
        .unwrap();

        assert!(matches!(outcome.tick, TickResult::Breached { .. }));
        assert!(outcome.confirmed);
        assert!(outcome.proposal.is_some());

        // Notifications sent: warning and completion
        let locale = Locale::current();
        let notes = notifier.notifications.lock().unwrap().clone();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].0, i18n::low_disk_space_title(locale));
        assert_eq!(notes[1].0, i18n::cleanup_complete_title(locale));

        let tagged = notifier.tagged.lock().unwrap().clone();
        assert_eq!(tagged.len(), 2);
        assert_eq!(tagged[0].0, NOTIF_TAG_LOW_SPACE);
        assert_eq!(tagged[1].0, NOTIF_TAG_CLEANUP_DONE);

        // State updated
        let reloaded_state = load_state(&state_path).unwrap();
        assert_eq!(reloaded_state.last_triggered_secs, Some(1_700_000_000));
        assert!(reloaded_state.last_proposal.is_some());
    }

    #[test]
    fn run_once_cancelled_by_user_performs_no_removal() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();

        let cfg_path = home.join("watchdog.json");
        let state_path = home.join("watchdog-state.json");

        let target_file = home.join("keep_me.txt");
        fs::write(&target_file, b"important").unwrap();

        let fake_ai_json = serde_json::json!({
            "summary": "Should be cancelled",
            "items": [{
                "path": target_file.to_string_lossy(),
                "mode": "trash",
                "reason": "Test item",
            }],
        })
        .to_string();

        let fake_exe = home.join("ruum");
        fs::write(&fake_exe, b"").unwrap();

        let cfg = WatchdogConfig {
            enabled: true,
            min_free_gb: 1_000_000,
            ai_auto_analyze: true,
            ai_provider: crate::watchdog_config::AiCliProvider::Custom,
            custom_command: fake_exe.to_string_lossy().to_string(),
            ..WatchdogConfig::default()
        };
        crate::watchdog_config::save_config(&cfg_path, &cfg).unwrap();

        let notifier = MockNotification::default();
        let confirm = MockConfirm {
            decision: CleanupConfirmDecision::Cancelled,
        };

        let outcome = run_once(
            &cfg_path,
            &state_path,
            home,
            &fake_exe,
            1_700_000_000,
            &AiMock(fake_ai_json),
            &confirm,
            &notifier,
        )
        .unwrap();

        assert!(!outcome.confirmed);
        assert_eq!(outcome.removed_items, 0);
        assert_eq!(outcome.reclaimed_bytes, 0);
        assert!(target_file.exists());

        // Only initial warning notification, no completion notification
        let locale = Locale::current();
        let notes = notifier.notifications.lock().unwrap().clone();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, i18n::low_disk_space_title(locale));
    }

    #[test]
    fn run_once_in_localizes_notifications_and_confirm_title() {
        #[derive(Debug, Default)]
        struct CapturingConfirm {
            title: Mutex<Option<String>>,
        }
        impl CleanupConfirmAdapter for CapturingConfirm {
            fn confirm(
                &self,
                request: &CleanupConfirmRequest,
            ) -> io::Result<CleanupConfirmDecision> {
                *self.title.lock().unwrap() = Some(request.title.clone());
                Ok(CleanupConfirmDecision::Cancelled)
            }
        }

        for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path();
            let cfg_path = home.join("watchdog.json");
            let state_path = home.join("watchdog-state.json");
            let target_file = home.join("cache.bin");
            fs::write(&target_file, b"abcd").unwrap();

            let fake_ai_json = serde_json::json!({
                "summary": "Test summary",
                "items": [{
                    "path": target_file.to_string_lossy(),
                    "mode": "trash",
                    "reason": "Test item",
                }],
            })
            .to_string();

            let fake_exe = home.join("ruum");
            fs::write(&fake_exe, b"").unwrap();
            let cfg = WatchdogConfig {
                enabled: true,
                min_free_gb: 1_000_000,
                ai_auto_analyze: true,
                ai_provider: crate::watchdog_config::AiCliProvider::Custom,
                custom_command: fake_exe.to_string_lossy().to_string(),
                ..WatchdogConfig::default()
            };
            crate::watchdog_config::save_config(&cfg_path, &cfg).unwrap();

            let notifier = MockNotification::default();
            let confirm = CapturingConfirm::default();
            run_once_in(
                &cfg_path,
                &state_path,
                home,
                &fake_exe,
                1_700_000_000,
                &AiMock(fake_ai_json),
                &confirm,
                &notifier,
                locale,
            )
            .unwrap();

            let notes = notifier.notifications.lock().unwrap().clone();
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].0, i18n::low_disk_space_title(locale));
            assert_eq!(
                confirm.title.lock().unwrap().as_deref(),
                Some(i18n::recommended_cleanup_title(locale))
            );
        }
    }

    #[test]
    fn run_once_in_cooldown_skips_analysis_and_notifications() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();

        let cfg_path = home.join("watchdog.json");
        let state_path = home.join("watchdog-state.json");

        let cfg = WatchdogConfig {
            enabled: true,
            min_free_gb: 1_000_000, // force threshold breach
            cooldown_secs: 3600,
            ..WatchdogConfig::default()
        };
        crate::watchdog_config::save_config(&cfg_path, &cfg).unwrap();

        // State previously triggered 100 seconds ago
        let prev_state = WatchdogState {
            last_checked_secs: Some(1_700_000_000),
            last_triggered_secs: Some(1_700_000_000),
            last_triggered_threshold_gb: None,
            last_proposal: None,
        };
        save_state(&state_path, &prev_state).unwrap();

        let notifier = MockNotification::default();
        let confirm = MockConfirm {
            decision: CleanupConfirmDecision::Cancelled,
        };
        let runner = MockRunner::default();

        let outcome = run_once(
            &cfg_path,
            &state_path,
            home,
            Path::new("/bin/ruum"),
            1_700_000_100, // within cooldown
            &runner,
            &confirm,
            &notifier,
        )
        .unwrap();

        assert!(matches!(outcome.tick, TickResult::InCooldown { .. }));
        assert!(notifier.notifications.lock().unwrap().is_empty());
        assert!(runner.runs.lock().unwrap().is_empty());
    }

    #[test]
    fn run_once_records_cooldown_without_ai_or_on_ai_error() {
        struct FailingRunner;
        impl CommandRunner for FailingRunner {
            fn run(
                &self,
                _program: &Path,
                _args: &[&str],
                _envs: &[(&str, &str)],
            ) -> io::Result<ai_cli::CommandOutput> {
                Ok(ai_cli::CommandOutput {
                    status_code: 1,
                    stdout: String::new(),
                    stderr: "cli crashed".to_string(),
                })
            }
        }

        // Case 1: ai_auto_analyze = false still records cooldown & threshold
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let cfg_path = home.join("watchdog.json");
        let state_path = home.join("watchdog-state.json");
        let cfg_no_ai = WatchdogConfig {
            enabled: true,
            min_free_gb: 1_000_000,
            cooldown_secs: 3600,
            ai_auto_analyze: false,
            ..WatchdogConfig::default()
        };
        crate::watchdog_config::save_config(&cfg_path, &cfg_no_ai).unwrap();

        let notifier = MockNotification::default();
        let confirm = MockConfirm {
            decision: CleanupConfirmDecision::Cancelled,
        };
        let runner = MockRunner::default();

        let first = run_once(
            &cfg_path,
            &state_path,
            home,
            Path::new("/bin/ruum"),
            1_700_000_000,
            &runner,
            &confirm,
            &notifier,
        )
        .unwrap();
        assert!(matches!(first.tick, TickResult::Breached { .. }));
        assert_eq!(notifier.notifications.lock().unwrap().len(), 1);

        let saved = load_state(&state_path).unwrap();
        assert_eq!(saved.last_triggered_secs, Some(1_700_000_000));
        assert_eq!(saved.last_triggered_threshold_gb, Some(1_000_000));

        // Second tick 60s later must be InCooldown (no notification storm)
        let second = run_once(
            &cfg_path,
            &state_path,
            home,
            Path::new("/bin/ruum"),
            1_700_000_060,
            &runner,
            &confirm,
            &notifier,
        )
        .unwrap();
        assert!(matches!(second.tick, TickResult::InCooldown { .. }));
        assert_eq!(notifier.notifications.lock().unwrap().len(), 1);

        // Case 2: ai_auto_analyze = true with failing AI CLI still records cooldown
        let temp2 = tempfile::tempdir().unwrap();
        let home2 = temp2.path();
        let cfg_path2 = home2.join("watchdog.json");
        let state_path2 = home2.join("watchdog-state.json");
        let fake_exe = home2.join("ruum");
        fs::write(&fake_exe, b"").unwrap();
        let cfg_ai_fail = WatchdogConfig {
            enabled: true,
            min_free_gb: 1_000_000,
            cooldown_secs: 3600,
            ai_auto_analyze: true,
            ai_provider: crate::watchdog_config::AiCliProvider::Custom,
            custom_command: fake_exe.to_string_lossy().to_string(),
            ..WatchdogConfig::default()
        };
        crate::watchdog_config::save_config(&cfg_path2, &cfg_ai_fail).unwrap();
        let notifier2 = MockNotification::default();
        let _ = run_once(
            &cfg_path2,
            &state_path2,
            home2,
            &fake_exe,
            1_700_000_000,
            &FailingRunner,
            &confirm,
            &notifier2,
        )
        .unwrap();
        let saved2 = load_state(&state_path2).unwrap();
        assert_eq!(saved2.last_triggered_secs, Some(1_700_000_000));
        assert_eq!(saved2.last_triggered_threshold_gb, Some(1_000_000));
    }

    #[test]
    fn run_loop_writes_and_cleans_pid_file_and_wakes_on_config_change() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let cfg_path = home.join("watchdog.json");
        let state_path = home.join("watchdog-state.json");
        let pid_path = home.join("watchdog.pid");

        let cfg_initial = WatchdogConfig {
            enabled: false,
            poll_interval_secs: 60,
            ..WatchdogConfig::default()
        };
        crate::watchdog_config::save_config(&cfg_path, &cfg_initial).unwrap();

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let cancel_bg = std::sync::Arc::clone(&cancel);

        let cfg_bg = cfg_path.clone();
        let state_bg = state_path.clone();
        let home_bg = home.to_path_buf();
        let handle = std::thread::spawn(move || {
            let runner = MockRunner::default();
            let confirm = MockConfirm {
                decision: CleanupConfirmDecision::Cancelled,
            };
            let notifier = MockNotification::default();
            run_loop(
                &cfg_bg,
                &state_bg,
                &home_bg,
                Path::new("/bin/ruum"),
                &cancel_bg,
                &runner,
                &confirm,
                &notifier,
            )
        });

        // Wait for initial tick and PID file creation
        let start = std::time::Instant::now();
        while !pid_path.exists() || !state_path.exists() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(3),
                "run_loop did not create pid/state files in time"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let pid_str = fs::read_to_string(&pid_path).unwrap();
        assert_eq!(
            pid_str.trim().parse::<u32>().ok(),
            Some(std::process::id())
        );
        let first_checked = load_state(&state_path).unwrap().last_checked_secs;
        assert!(first_checked.is_some());

        // Sleep 1.1s so `now_secs` advances, then modify config to wake loop
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let cfg_updated = WatchdogConfig {
            enabled: false,
            min_free_gb: 99,
            poll_interval_secs: 60,
            ..WatchdogConfig::default()
        };
        crate::watchdog_config::save_config(&cfg_path, &cfg_updated).unwrap();

        // Loop must wake up well below the 60s poll interval
        let wake_start = std::time::Instant::now();
        loop {
            let checked = load_state(&state_path).unwrap().last_checked_secs;
            if checked > first_checked {
                break;
            }
            assert!(
                wake_start.elapsed() < std::time::Duration::from_secs(10),
                "run_loop did not wake within <= 10s on watchdog.json change"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        cancel.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
        assert!(
            !pid_path.exists(),
            "watchdog.pid should be removed on clean exit"
        );
    }

    #[test]
    fn service_status_and_lifecycle_mock() {
        let runner = MockRunner::default();
        let status = service_status(&runner).unwrap();
        if cfg!(windows) {
            // Mock runner returns status 0 for `reg query`, so without a live
            // PID file Windows reports Inactive (never Unsupported).
            assert_eq!(status, ServiceStatus::Inactive);
        } else {
            // Mock runner returns "active", so service_status reports Active
            assert_eq!(status, ServiceStatus::Active);
        }
    }

    #[test]
    fn pid_file_guard_preserves_newer_pid_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("watchdog-state.json");
        let pid_path = temp.path().join("watchdog.pid");
        {
            let _guard = PidFileGuard::acquire(&state_path);
            assert!(pid_path.exists());
            fs::write(&pid_path, "999999\n").unwrap();
        }
        assert_eq!(fs::read_to_string(&pid_path).unwrap().trim(), "999999");
    }

    #[test]
    fn notification_adapter_notify_defaults_to_low_space_tag() {
        let notifier = MockNotification::default();
        notifier.notify("Title", "Body").unwrap();
        let tagged = notifier.tagged.lock().unwrap().clone();
        assert_eq!(
            tagged,
            vec![(
                NOTIF_TAG_LOW_SPACE.to_string(),
                "Title".to_string(),
                "Body".to_string(),
            )]
        );
    }

    #[test]
    fn try_ipc_notification_and_open_window_roundtrip_and_token_check() {
        use crate::confirm::{IpcListenerGuard, IpcRequest, IpcResponse};

        let temp = tempfile::tempdir().unwrap();
        let token = "watchdog_ipc_token_12345678901234567890123".to_string();

        #[cfg(unix)]
        let socket_or_port = temp.path().join("watchdog-ipc.sock");
        #[cfg(windows)]
        let socket_or_port = temp.path().join("watchdog-ipc.port");

        // Missing socket returns false without error.
        assert!(!try_ipc_notification_with(
            &socket_or_port,
            &token,
            NOTIF_TAG_LOW_SPACE,
            "Title",
            "Body",
        ));
        assert!(!try_ipc_open_window_with(
            &socket_or_port,
            &token,
            NOTIF_TAG_AI_ANALYSIS,
        ));

        let listener = IpcListenerGuard::bind(&socket_or_port).unwrap();
        let server_token = token.clone();
        let received = std::sync::Arc::new(Mutex::new(Vec::new()));
        let received_srv = std::sync::Arc::clone(&received);

        let server_thread = std::thread::spawn(move || {
            for _ in 0..4 {
                let rec = std::sync::Arc::clone(&received_srv);
                listener
                    .accept_and_handle(&server_token, move |req| {
                        rec.lock().unwrap().push(req);
                        Ok(IpcResponse::Ok)
                    })
                    .unwrap();
            }
        });

        assert!(try_ipc_notification_with(
            &socket_or_port,
            &token,
            NOTIF_TAG_LOW_SPACE,
            "Low Space",
            "Only 1 GB free",
        ));
        assert!(try_ipc_open_window_with(
            &socket_or_port,
            &token,
            NOTIF_TAG_AI_ANALYSIS,
        ));

        // Invalid token must return false and not invoke handler.
        assert!(!try_ipc_notification_with(
            &socket_or_port,
            "wrong_token_12345678901234567890123456",
            NOTIF_TAG_LOW_SPACE,
            "Low Space",
            "Body",
        ));
        assert!(!try_ipc_open_window_with(
            &socket_or_port,
            "wrong_token_12345678901234567890123456",
            NOTIF_TAG_AI_ANALYSIS,
        ));

        server_thread.join().unwrap();
        let reqs = received.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[0],
            IpcRequest::ShowNotification {
                token: token.clone(),
                tag: NOTIF_TAG_LOW_SPACE.to_string(),
                title: "Low Space".to_string(),
                body: "Only 1 GB free".to_string(),
            }
        );
        assert_eq!(
            reqs[1],
            IpcRequest::OpenWindow {
                token,
                tag: NOTIF_TAG_AI_ANALYSIS.to_string(),
            }
        );
    }

    #[test]
    fn execute_confirmed_cleanup_respects_mode_and_reports_item_errors() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();

        // Outside home -> blocked by plan_for_mode in Trash mode
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("keep.bin");
        fs::write(&outside_file, b"keep").unwrap();

        let blocked_items = vec![ProposalItem::new(
            outside_file.clone(),
            4,
            RemovalMode::Trash,
            None,
            None,
        )];
        let mut logged = Vec::new();
        let (bytes, count, cleaned) = execute_confirmed_cleanup_with(
            &blocked_items,
            home,
            &mut |path, err| {
                logged.push((path.to_path_buf(), err.to_owned()));
            },
        );
        assert_eq!((bytes, count), (0, 0));
        assert!(cleaned.is_empty());
        assert!(outside_file.exists());
        assert!(logged.is_empty());

        // Trigger a worker-level RemovalEvent::Item { outcome: Err(..) }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let locked_dir = home.join("locked");
            let locked_file = locked_dir.join("stuck.bin");
            fs::create_dir_all(&locked_dir).unwrap();
            fs::write(&locked_file, b"stuck").unwrap();
            fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o555))
                .unwrap();

            let perm_items = vec![ProposalItem::new(
                locked_file.clone(),
                5,
                RemovalMode::Permanent,
                None,
                None,
            )];
            let (perm_bytes, perm_count, perm_cleaned) =
                execute_confirmed_cleanup_with(
                    &perm_items,
                    home,
                    &mut |path, err| {
                        logged.push((path.to_path_buf(), err.to_owned()));
                    },
                );

            fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o755))
                .unwrap();

            assert_eq!((perm_bytes, perm_count), (0, 0));
            assert!(perm_cleaned.is_empty());
            assert!(locked_file.exists());
            assert_eq!(logged.len(), 1);
            assert_eq!(logged[0].0, locked_file);
            assert!(!logged[0].1.is_empty());
        }
    }

    #[test]
    fn core_space_probe_reads_live_volume_metrics() {
        let temp = tempfile::tempdir().unwrap();
        let sample = CoreSpaceProbe.probe(temp.path()).unwrap();
        assert!(sample.total_bytes > 0);
        assert!(sample.available_bytes <= sample.total_bytes);
    }

    #[test]
    fn core_event_handler_skips_ai_when_disabled_and_posts_webhook() {
        use crate::storage::{AiConfig, WebhookConfig};
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 4096];
            let header_end = loop {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0, "unexpected EOF reading webhook headers");
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    break pos;
                }
            };
            let headers = String::from_utf8_lossy(&buf[..header_end]);
            let content_len: usize = headers
                .lines()
                .find_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            let total_len = header_end + 4 + content_len;
            while buf.len() < total_len {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let raw = String::from_utf8_lossy(&buf).into_owned();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
            raw
        });

        db.save_webhook_config(&WebhookConfig {
            enabled: true,
            url: format!("http://{addr}/alert"),
            headers_json: r#"{"X-Test":"yes"}"#.to_owned(),
            timeout_secs: 5,
            updated_at_secs: 1,
        })
        .unwrap();
        db.save_ai_config(&AiConfig {
            auto_analyze_on_alert: false,
            ..AiConfig::default()
        })
        .unwrap();

        let ai_runner = MockRunner::default();
        let confirm = MockConfirm {
            decision: CleanupConfirmDecision::Cancelled,
        };
        let notifier = MockNotification::default();
        let handler = CoreWatchdogEventHandler::new(
            &db,
            PathBuf::from("/bin/ruum"),
            &ai_runner,
            &confirm,
            &notifier,
        )
        .with_locale(Locale::En)
        .with_ipc_target(None);

        let event = WatchdogEvent::LowDiskSpace {
            timestamp_secs: 1_794_895_200,
            mount_path: temp.path().to_path_buf(),
            available_bytes: 1_000_000_000,
            total_bytes: 100_000_000_000,
            threshold_bytes: 20_000_000_000,
        };
        handler.on_event(&event).unwrap();

        let raw_http = server.join().unwrap();
        assert!(raw_http.contains("ruum.disk.low_space"));
        assert!(raw_http.contains("2026-11-17T06:00:00Z"));
        assert_eq!(notifier.tagged.lock().unwrap().len(), 1);
        assert_eq!(notifier.tagged.lock().unwrap()[0].0, NOTIF_TAG_LOW_SPACE);
        assert!(
            ai_runner.runs.lock().unwrap().is_empty(),
            "AI CLI must never run when auto_analyze_on_alert is false"
        );
        assert_eq!(db.load_ai_state().unwrap().last_proposal_json, None);
    }

    #[test]
    fn core_event_handler_runs_ai_saves_state_and_honors_confirm_cancel() {
        use crate::storage::AiConfig;
        use crate::watchdog_config::AiCliProvider;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let db = StorageDb::open(home.join("ruum.db")).unwrap();

        let keep_file = home.join("keep.dat");
        fs::write(&keep_file, b"keep-me").unwrap();
        let fake_ai = home.join("fake-ai");
        fs::write(&fake_ai, b"").unwrap();

        db.save_ai_config(&AiConfig {
            auto_analyze_on_alert: true,
            provider: AiCliProvider::Custom,
            custom_command: fake_ai.to_string_lossy().into_owned(),
            ..AiConfig::default()
        })
        .unwrap();

        let ai_json = serde_json::json!({
            "summary": "Proposed 1 file",
            "items": [{
                "path": keep_file.to_string_lossy(),
                "mode": "trash",
                "reason": "test file",
            }],
        })
        .to_string();
        let ai_runner = AiMock(ai_json);
        let confirm = MockConfirm {
            decision: CleanupConfirmDecision::Cancelled,
        };
        let notifier = MockNotification::default();
        let handler = CoreWatchdogEventHandler::new(
            &db, fake_ai, &ai_runner, &confirm, &notifier,
        )
        .with_locale(Locale::En)
        .with_ipc_target(None);

        let event = WatchdogEvent::LowDiskSpace {
            timestamp_secs: 1_700_000_000,
            mount_path: home.to_path_buf(),
            available_bytes: 512,
            total_bytes: 1024,
            threshold_bytes: 1024 * 1024 * 1024,
        };
        handler.on_event(&event).unwrap();

        assert!(
            keep_file.exists(),
            "cancelled confirmation must not remove file"
        );
        let ai_state = db.load_ai_state().unwrap();
        assert_eq!(ai_state.updated_at_secs, 1_700_000_000);
        let parsed =
            ai_state.parse_proposal().unwrap().expect("proposal saved");
        assert_eq!(parsed.summary, "Proposed 1 file");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(notifier.tagged.lock().unwrap().len(), 1);
    }

    #[test]
    fn run_once_with_db_evaluates_sqlite_config_and_executes_confirmed_items() {
        use crate::storage::AiConfig;
        use crate::watchdog_config::AiCliProvider;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let db = StorageDb::open(home.join("ruum.db")).unwrap();

        let target_dir = home.join("cache").join("artifacts");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("tmp.dat"), b"123456").unwrap();

        let fake_ai = home.join("fake-ai");
        fs::write(&fake_ai, b"").unwrap();

        db.watchdog()
            .unwrap()
            .save_config(&ruum_watchdog::WatchdogConfig {
                enabled: true,
                min_free_gb: 1_000_000,
                ..ruum_watchdog::WatchdogConfig::default()
            })
            .unwrap();
        db.save_ai_config(&AiConfig {
            auto_analyze_on_alert: true,
            provider: AiCliProvider::Custom,
            custom_command: fake_ai.to_string_lossy().into_owned(),
            ..AiConfig::default()
        })
        .unwrap();

        let ai_json = serde_json::json!({
            "summary": "Clean test cache",
            "items": [{
                "path": target_dir.to_string_lossy(),
                "mode": "permanent",
                "reason": "test cache",
            }],
        })
        .to_string();
        let ai_runner = AiMock(ai_json);
        let confirm = MockConfirm {
            decision: CleanupConfirmDecision::Confirmed {
                selected_paths: vec![target_dir.clone()],
                selected_docker_targets: Vec::new(),
            },
        };
        let notifier = MockNotification::default();

        let tick = run_once_with_db(
            &db,
            home,
            &fake_ai,
            1_700_000_500,
            &ai_runner,
            &confirm,
            &notifier,
        )
        .unwrap();

        assert!(matches!(tick, ruum_watchdog::TickResult::Breached { .. }));
        assert!(!target_dir.exists(), "confirmed item should be removed");
        assert_eq!(notifier.tagged.lock().unwrap().len(), 2);
        assert_eq!(notifier.tagged.lock().unwrap()[0].0, NOTIF_TAG_LOW_SPACE);
        assert_eq!(
            notifier.tagged.lock().unwrap()[1].0,
            NOTIF_TAG_CLEANUP_DONE
        );
        let wd_state = db.watchdog().unwrap().load_state().unwrap();
        assert_eq!(wd_state.last_triggered_secs, Some(1_700_000_500));
        assert_eq!(
            db.load_ai_state().unwrap().last_proposal_json,
            None,
            "fully cleaned proposal must be cleared from ai_state"
        );
    }

    #[test]
    fn core_event_handler_delivers_multiple_webhooks_and_prunes_partial_cleanup()
     {
        use crate::storage::{AiConfig, WebhookEndpoint};
        use crate::watchdog_config::AiCliProvider;
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let db = StorageDb::open(home.join("ruum.db")).unwrap();

        let item_clean = home.join("clean_me.dat");
        let item_keep = home.join("keep_me.dat");
        fs::write(&item_clean, b"12345").unwrap();
        fs::write(&item_keep, b"67890").unwrap();
        let fake_ai = home.join("fake-ai");
        fs::write(&fake_ai, b"").unwrap();

        let ok_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ok_addr = ok_listener.local_addr().unwrap();
        let ok_server = std::thread::spawn(move || {
            let (mut stream, _) = ok_listener.accept().unwrap();
            let _ = stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut buf = [0_u8; 2048];
            let _ = stream.read(&mut buf);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
        });

        let fail_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let fail_addr = fail_listener.local_addr().unwrap();
        let fail_server = std::thread::spawn(move || {
            let (mut stream, _) = fail_listener.accept().unwrap();
            let _ = stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut buf = [0_u8; 2048];
            let _ = stream.read(&mut buf);
            stream
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 3\r\n\r\nerr",
                )
                .unwrap();
        });

        db.add_webhook(&WebhookEndpoint::new(
            "failing-hook",
            format!("http://{fail_addr}/fail"),
        ))
        .unwrap();
        db.add_webhook(&WebhookEndpoint::new(
            "healthy-hook",
            format!("http://{ok_addr}/ok"),
        ))
        .unwrap();
        db.save_ai_config(&AiConfig {
            auto_analyze_on_alert: true,
            provider: AiCliProvider::Custom,
            custom_command: fake_ai.to_string_lossy().into_owned(),
            ..AiConfig::default()
        })
        .unwrap();

        let ai_json = serde_json::json!({
            "summary": "Partial cleanup proposal",
            "items": [
                {
                    "path": item_clean.to_string_lossy(),
                    "mode": "permanent",
                    "reason": "clean",
                },
                {
                    "path": item_keep.to_string_lossy(),
                    "mode": "permanent",
                    "reason": "keep",
                }
            ],
        })
        .to_string();
        let ai_runner = AiMock(ai_json);
        let confirm = MockConfirm {
            decision: CleanupConfirmDecision::Confirmed {
                selected_paths: vec![item_clean.clone()],
                selected_docker_targets: Vec::new(),
            },
        };
        let notifier = MockNotification::default();
        let handler = CoreWatchdogEventHandler::new(
            &db, fake_ai, &ai_runner, &confirm, &notifier,
        )
        .with_locale(Locale::En)
        .with_ipc_target(None);

        handler
            .on_event(&WatchdogEvent::LowDiskSpace {
                timestamp_secs: 1_794_895_200,
                mount_path: home.to_path_buf(),
                available_bytes: 512,
                total_bytes: 1024,
                threshold_bytes: 1024 * 1024 * 1024,
            })
            .unwrap();

        ok_server.join().unwrap();
        fail_server.join().unwrap();
        assert!(!item_clean.exists());
        assert!(item_keep.exists());

        let remaining = db
            .load_ai_state()
            .unwrap()
            .parse_proposal()
            .unwrap()
            .expect("unconfirmed item remains in ai_state");
        assert_eq!(remaining.items.len(), 1);
        assert_eq!(remaining.items[0].path, item_keep);
    }
}
