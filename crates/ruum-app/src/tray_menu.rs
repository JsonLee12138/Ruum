//! Cross-platform system tray / menu-bar icon and quick-action menu.
//!
//! Uses `tray-icon` (pure Rust `ksni` D-Bus backend on Linux, native status
//! items on macOS and Windows) with zero `unsafe` code and zero external icon
//! assets at runtime.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, Once};

use crate::i18n::{Locale, common};
use crate::tray_gauge::{
    ICON_SIZE_PX, TrayGaugeAdapter, TrayGaugeFactory, TrayVisualRender,
    TrayVisualSignature, calc_used_percent,
};
use anyhow::Context as _;
use ruum_core::size::human_bytes;
use ruum_core::space::SpaceInfo;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
#[cfg(target_os = "linux")]
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

const ID_STATUS: &str = "ruum.tray.status";
const ID_OPEN_RUUM: &str = "ruum.tray.open_ruum";
const ID_OPEN_REMOTE: &str = "ruum.tray.open_remote";
const ID_OPEN_WATCHDOG: &str = "ruum.tray.open_watchdog";
const ID_OPEN_SETTINGS: &str = "ruum.tray.open_settings";
const ID_CHECK_UPDATES: &str = "ruum.tray.check_updates";
const ID_RUN_AI: &str = "ruum.tray.run_ai";
const ID_REVIEW_PROPOSAL: &str = "ruum.tray.review_proposal";
const ID_CLEANUP_PROGRESS: &str = "ruum.tray.cleanup_progress";
const ID_QUIT: &str = "ruum.tray.quit";

static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
static MENU_HANDLER_ONCE: Once = Once::new();
#[cfg(target_os = "linux")]
static TRAY_HANDLER_ONCE: Once = Once::new();
static EVENT_SINK: Mutex<Option<TrayEventSink>> = Mutex::new(None);

type EventWaker = Arc<dyn Fn() + Send + Sync>;

struct TrayEventSink {
    instance_id: u64,
    tx: Sender<TrayAction>,
    waker: Option<EventWaker>,
}

/// User actions triggered from the system tray menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// Open or focus the main `Ruum` window.
    OpenRuum,
    /// Open the `Remote` SSH servers page in the main window.
    OpenRemote,
    /// Open the `Watchdog` page in the main window.
    OpenWatchdog,
    /// Open the `Settings` page in the main window.
    OpenSettings,
    /// Open update modal or check for updates in `Settings::Updates`.
    CheckForUpdates,
    /// Trigger an immediate AI cleanup analysis.
    RunAiAnalysisNow,
    /// Open the independent confirmation window for the latest proposal.
    ReviewLatestProposal,
    /// Open or focus the background cleanup progress window.
    ShowCleanupProgress,
    /// Explicitly quit the application.
    Quit,
}

/// Live background cleanup counters displayed in the tray menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayCleanupStatus {
    pub completed: usize,
    pub total: usize,
    pub cancelling: bool,
}

/// Active OS tray icon and menu handles.
pub struct TrayMenu {
    instance_id: u64,
    locale: Locale,
    tray: TrayIcon,
    gauge_adapter: Box<dyn TrayGaugeAdapter>,
    status_item: MenuItem,
    check_updates_item: MenuItem,
    review_proposal_item: MenuItem,
    cleanup_progress_item: MenuItem,
    last_visual: RefCell<TrayVisualSignature>,
    rx: Receiver<TrayAction>,
}

impl fmt::Debug for TrayMenu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrayMenu")
            .field("instance_id", &self.instance_id)
            .finish_non_exhaustive()
    }
}

impl Drop for TrayMenu {
    fn drop(&mut self) {
        if let Ok(mut guard) = EVENT_SINK.lock()
            && guard
                .as_ref()
                .is_some_and(|sink| sink.instance_id == self.instance_id)
        {
            *guard = None;
        }
    }
}

impl TrayMenu {
    /// Install the system tray icon and menu on the GUI main thread.
    pub fn install(locale: Locale) -> anyhow::Result<Self> {
        let initial_status = format_status_label(None, locale);
        let status_item =
            MenuItem::with_id(ID_STATUS, initial_status, false, None);
        let open_item = MenuItem::with_id(
            ID_OPEN_RUUM,
            common::tray_open_ruum(locale),
            true,
            None,
        );
        let remote_item = MenuItem::with_id(
            ID_OPEN_REMOTE,
            common::tray_remote(locale),
            true,
            None,
        );
        let watchdog_item = MenuItem::with_id(
            ID_OPEN_WATCHDOG,
            common::tray_watchdog(locale),
            true,
            None,
        );
        let settings_item = MenuItem::with_id(
            ID_OPEN_SETTINGS,
            common::tray_settings(locale),
            true,
            None,
        );
        let check_updates_item = MenuItem::with_id(
            ID_CHECK_UPDATES,
            common::tray_check_updates(locale),
            true,
            None,
        );
        let run_ai_item = MenuItem::with_id(
            ID_RUN_AI,
            common::tray_run_ai_analysis(locale),
            true,
            None,
        );
        let review_proposal_item = MenuItem::with_id(
            ID_REVIEW_PROPOSAL,
            common::tray_review_latest_proposal(locale),
            false,
            None,
        );
        let cleanup_progress_item = MenuItem::with_id(
            ID_CLEANUP_PROGRESS,
            format_cleanup_progress_label(None, locale),
            false,
            None,
        );
        let quit_item = MenuItem::with_id(
            ID_QUIT,
            common::tray_quit_ruum(locale),
            true,
            None,
        );

        let sep1 = PredefinedMenuItem::separator();
        let sep2 = PredefinedMenuItem::separator();
        let sep3 = PredefinedMenuItem::separator();

        let menu = Menu::with_items(&[
            &status_item,
            &sep1,
            &open_item,
            &remote_item,
            &watchdog_item,
            &settings_item,
            &check_updates_item,
            &sep2,
            &run_ai_item,
            &review_proposal_item,
            &cleanup_progress_item,
            &sep3,
            &quit_item,
        ])
        .context("build tray context menu")?;

        let gauge_adapter = TrayGaugeFactory::create();
        let initial_env = gauge_adapter.initial_env();
        let (icon, icon_is_template) = match gauge_adapter
            .render_monochrome_c1(ICON_SIZE_PX, initial_env)
        {
            TrayVisualRender::MonochromeC1 {
                size,
                rgba,
                icon_is_template,
            } => (
                Icon::from_rgba(rgba, size, size)
                    .context("construct tray icon from RGBA")?,
                icon_is_template,
            ),
            TrayVisualRender::DiskGauge(gauge) => (
                Icon::from_rgba(gauge.rgba, gauge.width, gauge.height)
                    .context("construct tray icon from RGBA")?,
                gauge.icon_is_template,
            ),
        };
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("ruum");
        if icon_is_template {
            builder = builder.with_icon_as_template(true);
        }
        let tray = builder.build().context("create system tray icon")?;
        let initial_sig =
            gauge_adapter.visual_signature(None, false, initial_env);

        let (tx, rx) = mpsc::channel();
        let instance_id = NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        install_event_sink(instance_id, tx);
        ensure_global_handlers_registered();

        Ok(Self {
            instance_id,
            locale,
            tray,
            gauge_adapter,
            status_item,
            check_updates_item,
            review_proposal_item,
            cleanup_progress_item,
            last_visual: RefCell::new(initial_sig),
            rx,
        })
    }

    /// Refresh the disabled status line, tooltip, proposal availability, and
    /// (when enabled) the menu-bar/tray disk gauge.
    pub fn update_status(
        &self,
        space: Option<SpaceInfo>,
        has_proposal: bool,
        show_disk_space: bool,
    ) {
        let free_bytes = space.map(|s| s.available);
        let label = format_status_label(free_bytes, self.locale);
        self.status_item.set_text(&label);
        self.review_proposal_item.set_enabled(has_proposal);
        let tooltip = format_tray_tooltip(space, show_disk_space, self.locale);
        let _ = self.tray.set_tooltip(Some(tooltip));

        let env = self.gauge_adapter.probe_env(&self.tray);
        let next_sig =
            self.gauge_adapter
                .visual_signature(space, show_disk_space, env);
        let changed = *self.last_visual.borrow() != next_sig;
        if changed {
            let (visual_applied, next_title) = match self
                .gauge_adapter
                .build_visual(space, show_disk_space, env)
            {
                TrayVisualRender::MonochromeC1 {
                    size,
                    rgba,
                    icon_is_template,
                } => (
                    apply_tray_icon(
                        self.gauge_adapter.as_ref(),
                        &self.tray,
                        rgba,
                        size,
                        size,
                        icon_is_template,
                    ),
                    None,
                ),
                TrayVisualRender::DiskGauge(gauge) => (
                    apply_tray_icon(
                        self.gauge_adapter.as_ref(),
                        &self.tray,
                        gauge.rgba,
                        gauge.width,
                        gauge.height,
                        gauge.icon_is_template,
                    ),
                    gauge.title,
                ),
            };
            if visual_applied {
                self.tray.set_title(next_title.as_deref());
                *self.last_visual.borrow_mut() = next_sig;
            }
        }
    }

    /// Update the tray check-for-updates menu item label for an available
    /// release version (or reset to `Check for Updates…` when `None`).
    pub fn set_update_available(&self, version: Option<&str>) {
        let label = format_check_updates_label(version, self.locale);
        self.check_updates_item.set_text(&label);
    }

    /// Update the tray cleanup-progress menu item label and enabled state.
    pub fn set_cleanup_progress(&self, cleanup: Option<TrayCleanupStatus>) {
        let label = format_cleanup_progress_label(cleanup, self.locale);
        self.cleanup_progress_item.set_text(&label);
        self.cleanup_progress_item.set_enabled(cleanup.is_some());
    }

    /// Pop the next queued [`TrayAction`], if any.
    pub fn poll_action(&self) -> Option<TrayAction> {
        self.rx.try_recv().ok()
    }

    /// Register a callback invoked whenever a tray menu action arrives.
    pub fn set_event_waker<F>(&self, waker: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        if let Ok(mut guard) = EVENT_SINK.lock()
            && let Some(sink) = guard.as_mut()
            && sink.instance_id == self.instance_id
        {
            sink.waker = Some(Arc::new(waker));
        }
    }
}

fn install_event_sink(instance_id: u64, tx: Sender<TrayAction>) {
    if let Ok(mut guard) = EVENT_SINK.lock() {
        *guard = Some(TrayEventSink {
            instance_id,
            tx,
            waker: None,
        });
    }
}

fn emit_tray_action(action: TrayAction) {
    let waker = {
        let Ok(guard) = EVENT_SINK.lock() else {
            return;
        };
        let Some(sink) = guard.as_ref() else {
            return;
        };
        if sink.tx.send(action).is_err() {
            return;
        }
        sink.waker.clone()
    };
    if let Some(wake) = waker {
        wake();
    }
}

fn ensure_global_handlers_registered() {
    MENU_HANDLER_ONCE.call_once(|| {
        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            if let Some(action) = action_for_menu_id(event.id.as_ref()) {
                emit_tray_action(action);
            }
        }));
    });

    #[cfg(target_os = "linux")]
    TRAY_HANDLER_ONCE.call_once(|| {
        TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                emit_tray_action(TrayAction::OpenRuum);
            }
        }));
    });
}

/// Map a `muda` menu item identifier back to a [`TrayAction`].
#[must_use]
pub fn action_for_menu_id(id: &str) -> Option<TrayAction> {
    match id {
        ID_OPEN_RUUM => Some(TrayAction::OpenRuum),
        ID_OPEN_REMOTE => Some(TrayAction::OpenRemote),
        ID_OPEN_WATCHDOG => Some(TrayAction::OpenWatchdog),
        ID_OPEN_SETTINGS => Some(TrayAction::OpenSettings),
        ID_CHECK_UPDATES => Some(TrayAction::CheckForUpdates),
        ID_RUN_AI => Some(TrayAction::RunAiAnalysisNow),
        ID_REVIEW_PROPOSAL => Some(TrayAction::ReviewLatestProposal),
        ID_CLEANUP_PROGRESS => Some(TrayAction::ShowCleanupProgress),
        ID_QUIT => Some(TrayAction::Quit),
        _ => None,
    }
}

/// Format the read-only disk space and watchdog status line for the tray menu.
#[must_use]
pub fn format_status_label(free_bytes: Option<u64>, locale: Locale) -> String {
    common::tray_status_label(free_bytes, locale)
}

/// Format the localized tray menu item label for active or idle cleanup.
#[must_use]
pub fn format_cleanup_progress_label(
    cleanup: Option<TrayCleanupStatus>,
    locale: Locale,
) -> String {
    match cleanup {
        Some(status) => common::tray_cleanup_progress_active(
            status.completed,
            status.total,
            status.cancelling,
            locale,
        ),
        None => common::tray_cleanup_progress_idle(locale).to_owned(),
    }
}

/// Format the localized tray menu item label for software updates.
#[must_use]
pub fn format_check_updates_label(
    version: Option<&str>,
    locale: Locale,
) -> String {
    match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(ver) => common::tray_update_available(ver, locale),
        None => common::tray_check_updates(locale).to_owned(),
    }
}

/// Format the tray tooltip, embedding full free/total capacity when the disk
/// gauge is active (bounded under Windows' 128-UTF-16 tooltip limit).
#[must_use]
pub fn format_tray_tooltip(
    space: Option<SpaceInfo>,
    show_disk_space: bool,
    locale: Locale,
) -> String {
    if show_disk_space && let Some(sp) = space {
        let avail = human_bytes(sp.available);
        let total = human_bytes(sp.total);
        let used_pct = calc_used_percent(sp);
        let watchdog = watchdog_active_label(locale);
        return match locale {
            Locale::En => format!(
                "ruum \u{2014} Free: {avail} / {total} ({used_pct}% used) \u{00b7} {watchdog}"
            ),
            Locale::Zh => format!(
                "ruum \u{2014} \u{53ef}\u{7528}\u{ff1a}{avail} / {total}\u{ff08}\u{5df2}\u{7528} {used_pct}%\u{ff09}\u{00b7} {watchdog}"
            ),
            Locale::Ja => format!(
                "ruum \u{2014} \u{7a7a}\u{304d}: {avail} / {total}\u{ff08}\u{4f7f}\u{7528} {used_pct}%\u{ff09}\u{00b7} {watchdog}"
            ),
            Locale::Ko => format!(
                "ruum \u{2014} \u{c5ec}\u{c720}: {avail} / {total} ({used_pct}% \u{c0ac}\u{c6a9}) \u{00b7} {watchdog}"
            ),
        };
    }
    let label = format_status_label(space.map(|s| s.available), locale);
    format!("ruum \u{2014} {label}")
}

const fn watchdog_active_label(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Watchdog Active",
        Locale::Zh => "\u{7a7a}\u{95f4}\u{5b88}\u{62a4}\u{4e2d}",
        Locale::Ja => "\u{76e3}\u{8996}\u{4e2d}",
        Locale::Ko => "\u{c6cc}\u{ce58}\u{b3c5} \u{d65c}\u{c131}",
    }
}

fn apply_tray_icon(
    adapter: &dyn TrayGaugeAdapter,
    tray: &TrayIcon,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    is_template: bool,
) -> bool {
    let icon = match Icon::from_rgba(rgba, width, height) {
        Ok(icon) => icon,
        Err(err) => {
            eprintln!("ruum: failed to build tray icon: {err}");
            return false;
        }
    };
    if let Err(err) = adapter.apply_icon(tray, icon, is_template) {
        eprintln!("ruum: failed to set tray icon: {err}");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_formats_free_space_and_watchdog_state() {
        assert_eq!(
            format_status_label(None, Locale::En),
            "Free: \u{2014} \u{00b7} Watchdog Active"
        );
        assert_eq!(
            format_status_label(Some(1024 * 1024 * 1024), Locale::En),
            "Free: 1.0 GiB \u{00b7} Watchdog Active"
        );
        for locale in [Locale::Zh, Locale::Ja, Locale::Ko] {
            let empty = format_status_label(None, locale);
            let filled = format_status_label(Some(1024 * 1024 * 1024), locale);
            assert!(empty.contains('\u{2014}'));
            assert!(filled.contains("1.0 GiB"));
            assert_ne!(empty, filled);
            assert_ne!(empty, format_status_label(None, Locale::En));
        }
    }

    #[test]
    fn menu_id_mapping_covers_all_interactive_items() {
        assert_eq!(
            action_for_menu_id(ID_OPEN_RUUM),
            Some(TrayAction::OpenRuum)
        );
        assert_eq!(
            action_for_menu_id(ID_OPEN_REMOTE),
            Some(TrayAction::OpenRemote)
        );
        assert_eq!(
            action_for_menu_id(ID_OPEN_WATCHDOG),
            Some(TrayAction::OpenWatchdog)
        );
        assert_eq!(
            action_for_menu_id(ID_OPEN_SETTINGS),
            Some(TrayAction::OpenSettings)
        );
        assert_eq!(action_for_menu_id("ruum.tray.check_disk"), None);
        assert_eq!(
            action_for_menu_id(ID_RUN_AI),
            Some(TrayAction::RunAiAnalysisNow)
        );
        assert_eq!(
            action_for_menu_id(ID_REVIEW_PROPOSAL),
            Some(TrayAction::ReviewLatestProposal)
        );
        assert_eq!(
            action_for_menu_id(ID_CLEANUP_PROGRESS),
            Some(TrayAction::ShowCleanupProgress)
        );
        assert_eq!(
            action_for_menu_id(ID_CHECK_UPDATES),
            Some(TrayAction::CheckForUpdates)
        );
        assert_eq!(action_for_menu_id("ruum.tray.toggle_watchdog"), None);
        assert_eq!(action_for_menu_id(ID_QUIT), Some(TrayAction::Quit));
        assert_eq!(action_for_menu_id(ID_STATUS), None);
        assert_eq!(action_for_menu_id("unknown"), None);
    }

    #[test]
    fn check_updates_label_formats_available_version_across_locales() {
        assert_eq!(
            format_check_updates_label(None, Locale::En),
            "Check for Updates\u{2026}"
        );
        assert_eq!(
            format_check_updates_label(Some("0.3.0"), Locale::En),
            "Update Available (v0.3.0)\u{2026}"
        );
        assert_eq!(
            format_check_updates_label(Some("v0.3.0"), Locale::En),
            "Update Available (v0.3.0)\u{2026}"
        );
        for locale in [Locale::Zh, Locale::Ja, Locale::Ko] {
            let idle = format_check_updates_label(None, locale);
            let avail = format_check_updates_label(Some("0.3.0"), locale);
            assert!(avail.contains("v0.3.0"));
            assert_ne!(idle, avail);
            assert_ne!(idle, format_check_updates_label(None, Locale::En));
            assert_ne!(
                avail,
                format_check_updates_label(Some("0.3.0"), Locale::En)
            );
        }
    }

    #[test]
    fn cleanup_progress_label_reflects_idle_active_and_cancelling_across_locales()
     {
        let active = TrayCleanupStatus {
            completed: 3,
            total: 12,
            cancelling: false,
        };
        let cancelling = TrayCleanupStatus {
            completed: 3,
            total: 12,
            cancelling: true,
        };
        for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
            let idle_label = format_cleanup_progress_label(None, locale);
            let active_label =
                format_cleanup_progress_label(Some(active), locale);
            let stop_label =
                format_cleanup_progress_label(Some(cancelling), locale);
            assert!(!idle_label.is_empty());
            assert!(active_label.contains("(3/12)"));
            assert!(stop_label.contains("(3/12)"));
            assert_ne!(active_label, stop_label);
            assert_ne!(idle_label, active_label);
            if locale != Locale::En {
                assert_ne!(
                    idle_label,
                    format_cleanup_progress_label(None, Locale::En)
                );
                assert_ne!(
                    active_label,
                    format_cleanup_progress_label(Some(active), Locale::En)
                );
            }
        }
    }

    #[test]
    fn event_sink_queues_action_and_invokes_waker() {
        use std::sync::atomic::AtomicBool;

        let (tx, rx) = mpsc::channel();
        let instance_id = NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        install_event_sink(instance_id, tx);

        let woken = Arc::new(AtomicBool::new(false));
        let woken_flag = Arc::clone(&woken);
        if let Ok(mut guard) = EVENT_SINK.lock()
            && let Some(sink) = guard.as_mut()
        {
            sink.waker = Some(Arc::new(move || {
                woken_flag.store(true, Ordering::SeqCst);
            }));
        }

        emit_tray_action(TrayAction::OpenWatchdog);
        assert_eq!(rx.try_recv().ok(), Some(TrayAction::OpenWatchdog));
        assert!(woken.load(Ordering::SeqCst));

        if let Ok(mut guard) = EVENT_SINK.lock()
            && guard
                .as_ref()
                .is_some_and(|sink| sink.instance_id == instance_id)
        {
            *guard = None;
        }
    }

    #[test]
    fn tray_tooltip_includes_full_capacity_when_gauge_enabled_within_win32_limit()
     {
        use ruum_core::space::SpaceInfo;

        const GIB: u64 = 1024 * 1024 * 1024;
        let space = SpaceInfo {
            total: 500 * GIB,
            free: 42 * GIB,
            available: 42 * GIB,
        };

        for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
            let tip_on = format_tray_tooltip(Some(space), true, locale);
            assert!(tip_on.contains("42 GiB"));
            assert!(tip_on.contains("500 GiB"));
            assert!(tip_on.contains("92%"));
            assert!(tip_on.encode_utf16().count() < 128);

            let tip_off = format_tray_tooltip(Some(space), false, locale);
            assert!(tip_off.contains("42 GiB"));
            assert!(tip_off.starts_with("ruum \u{2014} "));
        }
    }
}
