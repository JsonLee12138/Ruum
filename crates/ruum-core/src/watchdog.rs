//! Low-overhead disk space watchdog and service registration.
//!
//! Evaluates volume free space using lightweight $O(1)$ [`crate::space::space_info`]
//! calls against the user's home directory. When the threshold is breached and
//! the cooldown period has elapsed, the watchdog emits a notification, invokes
//! the configured AI assistant to generate a proposal, requests confirmation
//! via [`crate::confirm::CleanupConfirmAdapter`], and executes approved
//! cleanups without unattended file deletion.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ai_cli::{self, CommandRunner};
use crate::confirm::{
    CleanupConfirmAdapter, CleanupConfirmDecision, CleanupConfirmItem,
    CleanupConfirmRequest,
};
use crate::i18n::{self, Locale};
use crate::removal::{RemovalEvent, RemovalMode, Target};
use crate::watchdog_config::{
    CleanupProposal, ProposalItem, WatchdogConfig, WatchdogState, load_config,
    load_state, save_state,
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

/// Notification tag for low disk space alerts.
pub const NOTIF_TAG_LOW_SPACE: &str = "ruum.low_disk_space";
/// Notification tag for completed cleanup summaries.
pub const NOTIF_TAG_CLEANUP_DONE: &str = "ruum.cleanup_complete";
/// Notification tag for AI analysis completion / proposals.
pub const NOTIF_TAG_AI_ANALYSIS: &str = "ruum.ai_analysis";

/// Abstract adapter for sending temporary OS notifications.
pub trait NotificationAdapter: Send + Sync {
    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()>;

    fn notify(&self, title: &str, body: &str) -> io::Result<()> {
        self.notify_tagged(NOTIF_TAG_LOW_SPACE, title, body)
    }
}

/// Default system notification adapter sending non-persistent alerts.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsNotificationAdapter;

impl NotificationAdapter for OsNotificationAdapter {
    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        if try_ipc_notification(tag, title, body) {
            return Ok(());
        }
        send_fallback_os_notification(title, body)
    }
}

fn resolve_ipc_target_and_token() -> Option<(PathBuf, String)> {
    let token_path = crate::watchdog_config::ipc_token_path()?;
    let token = crate::confirm::load_or_create_ipc_token(&token_path).ok()?;
    #[cfg(unix)]
    let target = crate::confirm::resolve_socket_path();
    #[cfg(windows)]
    let target = crate::confirm::ipc_port_path()?;
    Some((target, token))
}

fn send_ipc_ok(
    socket_or_port: &Path,
    request: &crate::confirm::IpcRequest,
) -> bool {
    matches!(
        crate::confirm::send_ipc_message(socket_or_port, request),
        Ok(crate::confirm::IpcResponse::Ok)
    )
}

/// Try delivering a tagged notification through an active GUI instance.
pub fn try_ipc_notification_with(
    socket_or_port: &Path,
    token: &str,
    tag: &str,
    title: &str,
    body: &str,
) -> bool {
    send_ipc_ok(
        socket_or_port,
        &crate::confirm::IpcRequest::ShowNotification {
            token: token.to_string(),
            tag: tag.to_string(),
            title: title.to_string(),
            body: body.to_string(),
        },
    )
}

/// Try delivering a tagged notification through the default local IPC channel.
#[must_use]
pub fn try_ipc_notification(tag: &str, title: &str, body: &str) -> bool {
    let Some((target, token)) = resolve_ipc_target_and_token() else {
        return false;
    };
    try_ipc_notification_with(&target, &token, tag, title, body)
}

/// Try opening/focusing an active GUI window via an explicit IPC socket/port.
pub fn try_ipc_open_window_with(
    socket_or_port: &Path,
    token: &str,
    tag: &str,
) -> bool {
    send_ipc_ok(
        socket_or_port,
        &crate::confirm::IpcRequest::OpenWindow {
            token: token.to_string(),
            tag: tag.to_string(),
        },
    )
}

/// Try opening/focusing an active GUI window through the default local IPC.
#[must_use]
pub fn try_ipc_open_window(tag: &str) -> bool {
    let Some((target, token)) = resolve_ipc_target_and_token() else {
        return false;
    };
    try_ipc_open_window_with(&target, &token, tag)
}

/// Send a fallback OS desktop notification directly via CLI utilities
/// (`osascript` on macOS, `notify-send` on Linux) without touching IPC.
pub fn send_fallback_os_notification(
    title: &str,
    body: &str,
) -> io::Result<()> {
    if cfg!(target_os = "macos") {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            body.replace('\"', "\\\""),
            title.replace('\"', "\\\""),
        );
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    } else if cfg!(target_os = "linux") {
        let _ = Command::new("notify-send")
            .arg("-t")
            .arg("5000")
            .arg(title)
            .arg(body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

/// Send a temporary OS desktop notification, preferring an active GUI via IPC
/// and falling back to platform CLI utilities.
pub fn send_os_notification(title: &str, body: &str) -> io::Result<()> {
    if try_ipc_notification(NOTIF_TAG_LOW_SPACE, title, body) {
        return Ok(());
    }
    send_fallback_os_notification(title, body)
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

/// Notify an active GUI instance over IPC of a watchdog state change.
pub fn notify_gui_watchdog_update() {
    let Some((target, token)) = resolve_ipc_target_and_token() else {
        return;
    };
    let _ = send_ipc_ok(
        &target,
        &crate::confirm::IpcRequest::NotifyWatchdogUpdate { token },
    );
}

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
    execute_confirmed_cleanup_with(items, home_dir, &mut |path, err| {
        eprintln!("ruum watchdog: failed to remove {}: {err}", path.display());
    })
}

fn execute_confirmed_cleanup_with(
    items: &[ProposalItem],
    home_dir: &Path,
    log_error: &mut dyn FnMut(&Path, &str),
) -> (u64, usize) {
    let mut total_bytes = 0_u64;
    let mut total_count = 0_usize;

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
                        outcome: Ok(()), ..
                    }
                    | RemovalEvent::Start { .. } => {}
                }
            }
            if !progressed && !done {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    (total_bytes, total_count)
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
        };

        if let Ok(CleanupConfirmDecision::Confirmed { selected_paths }) =
            confirm_adapter.confirm(&request)
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
        for _ in 0..sleep_secs {
            if cancel.load(Ordering::Relaxed)
                || file_stamp(config_path) != config_stamp
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
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
        assert!(plist.contains("<key>KeepAlive</key>"));

        let unit = linux_systemd_unit_content(exe);
        assert!(unit.contains("Description=Ruum Disk Space Watchdog"));
        assert!(unit.contains("ExecStart=/usr/local/bin/ruum watchdog"));
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

        // Loop must wake up within <= 2s (well below the 60s poll interval)
        let wake_start = std::time::Instant::now();
        loop {
            let checked = load_state(&state_path).unwrap().last_checked_secs;
            if checked > first_checked {
                break;
            }
            assert!(
                wake_start.elapsed() < std::time::Duration::from_secs(3),
                "run_loop did not wake within <= 2s on watchdog.json change"
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
        let (bytes, count) = execute_confirmed_cleanup_with(
            &blocked_items,
            home,
            &mut |path, err| {
                logged.push((path.to_path_buf(), err.to_owned()));
            },
        );
        assert_eq!((bytes, count), (0, 0));
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
            let (perm_bytes, perm_count) = execute_confirmed_cleanup_with(
                &perm_items,
                home,
                &mut |path, err| {
                    logged.push((path.to_path_buf(), err.to_owned()));
                },
            );

            fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o755))
                .unwrap();

            assert_eq!((perm_bytes, perm_count), (0, 0));
            assert!(locked_file.exists());
            assert_eq!(logged.len(), 1);
            assert_eq!(logged[0].0, locked_file);
            assert!(!logged[0].1.is_empty());
        }
    }
}
