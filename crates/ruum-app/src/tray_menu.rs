//! Cross-platform system tray / menu-bar icon and quick-action menu.
//!
//! Uses `tray-icon` (pure Rust `ksni` D-Bus backend on Linux, native status
//! items on macOS and Windows) with zero `unsafe` code and zero external icon
//! assets at runtime.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, Once};

use anyhow::Context as _;
use ruum_core::size::human_bytes;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
#[cfg(target_os = "linux")]
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

const ICON_SIZE_PX: u32 = 32;

const ID_STATUS: &str = "ruum.tray.status";
const ID_OPEN_RUUM: &str = "ruum.tray.open_ruum";
const ID_OPEN_SOURCES: &str = "ruum.tray.open_sources";
const ID_OPEN_WATCHDOG: &str = "ruum.tray.open_watchdog";
const ID_OPEN_SETTINGS: &str = "ruum.tray.open_settings";
const ID_CHECK_DISK: &str = "ruum.tray.check_disk";
const ID_RUN_AI: &str = "ruum.tray.run_ai";
const ID_REVIEW_PROPOSAL: &str = "ruum.tray.review_proposal";
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
    /// Open the `Sources & Drives` page in the main window.
    OpenSources,
    /// Open the `Watchdog` page in the main window.
    OpenWatchdog,
    /// Open the `Settings` page in the main window.
    OpenSettings,
    /// Run an immediate O(1) disk space check.
    CheckDiskSpaceNow,
    /// Trigger an immediate AI cleanup analysis.
    RunAiAnalysisNow,
    /// Open the independent confirmation window for the latest proposal.
    ReviewLatestProposal,
    /// Explicitly quit the application.
    Quit,
}

/// Active OS tray icon and menu handles.
pub struct TrayMenu {
    instance_id: u64,
    tray: TrayIcon,
    status_item: MenuItem,
    review_proposal_item: MenuItem,
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
    pub fn install() -> anyhow::Result<Self> {
        let initial_status = format_status_label(None, false);
        let status_item =
            MenuItem::with_id(ID_STATUS, initial_status, false, None);
        let open_item =
            MenuItem::with_id(ID_OPEN_RUUM, "Open Ruum", true, None);
        // `muda` interprets a single `&` as a mnemonic marker on Windows/GTK
        // and strips it on macOS; `&&` renders a literal `&` everywhere.
        let sources_item = MenuItem::with_id(
            ID_OPEN_SOURCES,
            "Sources && Drives\u{2026}",
            true,
            None,
        );
        let watchdog_item =
            MenuItem::with_id(ID_OPEN_WATCHDOG, "Watchdog", true, None);
        let settings_item =
            MenuItem::with_id(ID_OPEN_SETTINGS, "Settings\u{2026}", true, None);
        let check_disk_item = MenuItem::with_id(
            ID_CHECK_DISK,
            "Check Disk Space Now",
            true,
            None,
        );
        let run_ai_item =
            MenuItem::with_id(ID_RUN_AI, "Run AI Analysis Now", true, None);
        let review_proposal_item = MenuItem::with_id(
            ID_REVIEW_PROPOSAL,
            "Review Latest Proposal\u{2026}",
            false,
            None,
        );
        let quit_item = MenuItem::with_id(ID_QUIT, "Quit Ruum", true, None);

        let sep1 = PredefinedMenuItem::separator();
        let sep2 = PredefinedMenuItem::separator();
        let sep3 = PredefinedMenuItem::separator();

        let menu = Menu::with_items(&[
            &status_item,
            &sep1,
            &open_item,
            &sources_item,
            &watchdog_item,
            &settings_item,
            &sep2,
            &check_disk_item,
            &run_ai_item,
            &review_proposal_item,
            &sep3,
            &quit_item,
        ])
        .context("build tray context menu")?;

        let icon = build_treemap_icon(ICON_SIZE_PX)?;
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("ruum");
        if cfg!(target_os = "macos") {
            builder = builder.with_icon_as_template(true);
        }
        let tray = builder.build().context("create system tray icon")?;

        let (tx, rx) = mpsc::channel();
        let instance_id = NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        install_event_sink(instance_id, tx);
        ensure_global_handlers_registered();

        Ok(Self {
            instance_id,
            tray,
            status_item,
            review_proposal_item,
            rx,
        })
    }

    /// Refresh the disabled status line, tooltip, and proposal availability.
    pub fn update_status(
        &self,
        free_bytes: Option<u64>,
        watchdog_enabled: bool,
        has_proposal: bool,
    ) {
        let label = format_status_label(free_bytes, watchdog_enabled);
        self.status_item.set_text(&label);
        self.review_proposal_item.set_enabled(has_proposal);
        let _ = self
            .tray
            .set_tooltip(Some(format!("ruum \u{2014} {label}")));
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
        ID_OPEN_SOURCES => Some(TrayAction::OpenSources),
        ID_OPEN_WATCHDOG => Some(TrayAction::OpenWatchdog),
        ID_OPEN_SETTINGS => Some(TrayAction::OpenSettings),
        ID_CHECK_DISK => Some(TrayAction::CheckDiskSpaceNow),
        ID_RUN_AI => Some(TrayAction::RunAiAnalysisNow),
        ID_REVIEW_PROPOSAL => Some(TrayAction::ReviewLatestProposal),
        ID_QUIT => Some(TrayAction::Quit),
        _ => None,
    }
}

/// Format the read-only disk space and watchdog status line for the tray menu.
#[must_use]
pub fn format_status_label(
    free_bytes: Option<u64>,
    watchdog_enabled: bool,
) -> String {
    let free = free_bytes.map_or_else(|| "\u{2014}".to_owned(), human_bytes);
    let watchdog = if watchdog_enabled {
        "Watchdog On"
    } else {
        "Watchdog Off"
    };
    format!("Free: {free} \u{00b7} {watchdog}")
}

fn build_treemap_icon(size: u32) -> anyhow::Result<Icon> {
    let rgba = build_treemap_icon_rgba(size);
    Icon::from_rgba(rgba, size, size).context("construct tray icon from RGBA")
}

/// Generate a crisp monochrome RGBA treemap-grid icon in memory.
///
/// On macOS the icon uses black pixels with alpha so `AppKit`'s template mode
/// adapts automatically to Light and Dark menu bars.
#[must_use]
pub fn build_treemap_icon_rgba(size: u32) -> Vec<u8> {
    let pixel_count = (size as usize) * (size as usize);
    let mut rgba = vec![0u8; pixel_count * 4];
    if size < 8 {
        return rgba;
    }

    let pad = (size / 8).max(1);
    let gap = (size / 16).max(1);
    let inner = size - 2 * pad;
    let left_end = pad + inner / 2;
    let right_start = left_end + gap;
    let top_end = pad + inner / 2;
    let bottom_start = top_end + gap;
    let br_mid = right_start + (pad + inner - right_start) / 2;
    let br_right_start = br_mid + gap;
    let outer_end = pad + inner;

    let (r, g, b) = if cfg!(target_os = "macos") {
        (0u8, 0u8, 0u8)
    } else {
        (238u8, 238u8, 242u8)
    };

    for y in 0..size {
        for x in 0..size {
            if is_outer_corner(x, y, pad, outer_end) {
                continue;
            }
            let alpha = if in_rect(x, y, pad, pad, left_end, outer_end)
                || in_rect(x, y, right_start, pad, outer_end, top_end)
                || in_rect(x, y, right_start, bottom_start, br_mid, outer_end)
            {
                255
            } else if in_rect(
                x,
                y,
                br_right_start,
                bottom_start,
                outer_end,
                outer_end,
            ) {
                175
            } else {
                0
            };

            if alpha > 0 {
                let idx = ((y * size + x) as usize) * 4;
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = alpha;
            }
        }
    }

    rgba
}

const fn in_rect(x: u32, y: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
    x >= x0 && x < x1 && y >= y0 && y < y1
}

const fn is_outer_corner(x: u32, y: u32, min: u32, max: u32) -> bool {
    let last = max - 1;
    (x == min || x == last) && (y == min || y == last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_formats_free_space_and_watchdog_state() {
        assert_eq!(
            format_status_label(None, false),
            "Free: \u{2014} \u{00b7} Watchdog Off"
        );
        assert_eq!(
            format_status_label(Some(1024 * 1024 * 1024), true),
            "Free: 1.0 GiB \u{00b7} Watchdog On"
        );
    }

    #[test]
    fn icon_rgba_has_expected_dimensions_and_treemap_silhouette() {
        for size in [16u32, 32u32] {
            let rgba = build_treemap_icon_rgba(size);
            assert_eq!(rgba.len(), (size as usize) * (size as usize) * 4);

            let opaque_pixels =
                rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
            let transparent_pixels =
                rgba.chunks_exact(4).filter(|px| px[3] == 0).count();
            assert!(opaque_pixels > 0);
            assert!(transparent_pixels > 0);
            assert!(Icon::from_rgba(rgba, size, size).is_ok());
        }
    }

    #[test]
    fn menu_id_mapping_covers_all_interactive_items() {
        assert_eq!(
            action_for_menu_id(ID_OPEN_RUUM),
            Some(TrayAction::OpenRuum)
        );
        assert_eq!(
            action_for_menu_id(ID_OPEN_SOURCES),
            Some(TrayAction::OpenSources)
        );
        assert_eq!(
            action_for_menu_id(ID_OPEN_WATCHDOG),
            Some(TrayAction::OpenWatchdog)
        );
        assert_eq!(
            action_for_menu_id(ID_OPEN_SETTINGS),
            Some(TrayAction::OpenSettings)
        );
        assert_eq!(
            action_for_menu_id(ID_CHECK_DISK),
            Some(TrayAction::CheckDiskSpaceNow)
        );
        assert_eq!(
            action_for_menu_id(ID_RUN_AI),
            Some(TrayAction::RunAiAnalysisNow)
        );
        assert_eq!(
            action_for_menu_id(ID_REVIEW_PROPOSAL),
            Some(TrayAction::ReviewLatestProposal)
        );
        assert_eq!(action_for_menu_id(ID_QUIT), Some(TrayAction::Quit));
        assert_eq!(action_for_menu_id(ID_STATUS), None);
        assert_eq!(action_for_menu_id("unknown"), None);
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
}
