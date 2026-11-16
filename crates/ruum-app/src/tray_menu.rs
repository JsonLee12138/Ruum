//! Cross-platform system tray / menu-bar icon and quick-action menu.
//!
//! Uses `tray-icon` (pure Rust `ksni` D-Bus backend on Linux, native status
//! items on macOS and Windows) with zero `unsafe` code and zero external icon
//! assets at runtime.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, Once};

use crate::i18n::{Locale, common};
use anyhow::Context as _;
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
    locale: Locale,
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
    pub fn install(locale: Locale) -> anyhow::Result<Self> {
        let initial_status = format_status_label(None, false, locale);
        let status_item =
            MenuItem::with_id(ID_STATUS, initial_status, false, None);
        let open_item = MenuItem::with_id(
            ID_OPEN_RUUM,
            common::tray_open_ruum(locale),
            true,
            None,
        );
        // `muda` interprets a single `&` as a mnemonic marker on Windows/GTK
        // and strips it on macOS; `&&` renders a literal `&` everywhere.
        let sources_item = MenuItem::with_id(
            ID_OPEN_SOURCES,
            common::tray_sources_and_drives(locale),
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
        let check_disk_item = MenuItem::with_id(
            ID_CHECK_DISK,
            common::tray_check_disk_space(locale),
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
            locale,
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
        let label =
            format_status_label(free_bytes, watchdog_enabled, self.locale);
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
    locale: Locale,
) -> String {
    common::tray_status_label(free_bytes, watchdog_enabled, locale)
}

fn build_treemap_icon(size: u32) -> anyhow::Result<Icon> {
    let rgba = build_treemap_icon_rgba(size);
    Icon::from_rgba(rgba, size, size).context("construct tray icon from RGBA")
}

/// Generate a crisp monochrome RGBA `C1 (Machined Monolith)` icon in memory.
///
/// On macOS the icon uses black pixels with alpha so `AppKit`'s template mode
/// adapts automatically to Light and Dark menu bars.
#[must_use]
pub fn build_treemap_icon_rgba(size: u32) -> Vec<u8> {
    build_treemap_icon_rgba_with_rgb(
        size,
        monochrome_rgb(cfg!(target_os = "macos")),
    )
}

const fn monochrome_rgb(is_macos: bool) -> [u8; 3] {
    if is_macos { [0, 0, 0] } else { [238, 238, 242] }
}

fn build_treemap_icon_rgba_with_rgb(size: u32, rgb: [u8; 3]) -> Vec<u8> {
    let pixel_count = (size as usize) * (size as usize);
    let mut rgba = vec![0u8; pixel_count * 4];
    if size < 8 {
        return rgba;
    }

    let geometry = C1Geometry::for_size(size);
    for y in 0..size {
        for x in 0..size {
            let alpha = geometry.alpha_at(x, y);
            if alpha > 0 {
                let idx = ((y * size + x) as usize) * 4;
                rgba[idx] = rgb[0];
                rgba[idx + 1] = rgb[1];
                rgba[idx + 2] = rgb[2];
                rgba[idx + 3] = alpha;
            }
        }
    }

    rgba
}

struct C1Geometry {
    pad: u32,
    frame_end: u32,
    frame_inner_start: u32,
    frame_inner_end: u32,
    content_start: u32,
    content_end: u32,
    left_end: u32,
    right_start: u32,
    top_end: u32,
    bottom_start: u32,
    wall_inner_x: u32,
    wall_inner_y: u32,
    wall_inner_end: u32,
    core_start_x: u32,
    core_start_y: u32,
    core_end: u32,
}

impl C1Geometry {
    fn for_size(size: u32) -> Self {
        // Above 28 px the 32 px C1 layout scales with 2 px strokes and moats;
        // smaller sizes lock strokes and moats to 1 px so the double-square
        // core keeps an open moat.
        let (pad, frame_t, frame_moat, gap, wall_t, core_moat) = if size >= 28 {
            (
                (3 * size + 16) / 32,
                (2 * size + 16) / 32,
                (2 * size + 16) / 32,
                (2 * size + 16) / 32,
                (2 * size + 16) / 32,
                (size + 16) / 32,
            )
        } else {
            let pad = if size >= 14 { (size / 12).max(1) } else { 0 };
            let frame_moat = u32::from(size >= 10);
            (pad, 1, frame_moat, 1, 1, 1)
        };

        let frame_end = size.saturating_sub(pad);
        let frame_inner_start = pad + frame_t;
        let frame_inner_end = frame_end.saturating_sub(frame_t);
        let content_start = frame_inner_start + frame_moat;
        let content_end = frame_inner_end.saturating_sub(frame_moat);
        let content_span = content_end.saturating_sub(content_start);
        let avail = content_span.saturating_sub(gap);

        let right_size = if size >= 28 {
            (avail * 9 + 8) / 16
        } else {
            (avail * 5 + 4) / 9
        };
        let left_w = avail.saturating_sub(right_size);

        let left_end = content_start + left_w;
        let right_start = left_end + gap;
        let bottom_start = content_end.saturating_sub(right_size);
        let top_end = bottom_start.saturating_sub(gap);

        let wall_inner_x = right_start + wall_t;
        let wall_inner_y = bottom_start + wall_t;
        let wall_inner_end = content_end.saturating_sub(wall_t);
        let core_start_x = wall_inner_x + core_moat;
        let core_start_y = wall_inner_y + core_moat;
        let core_end = wall_inner_end.saturating_sub(core_moat);

        Self {
            pad,
            frame_end,
            frame_inner_start,
            frame_inner_end,
            content_start,
            content_end,
            left_end,
            right_start,
            top_end,
            bottom_start,
            wall_inner_x,
            wall_inner_y,
            wall_inner_end,
            core_start_x,
            core_start_y,
            core_end,
        }
    }

    const fn alpha_at(&self, x: u32, y: u32) -> u8 {
        let in_frame =
            in_rect(x, y, self.pad, self.pad, self.frame_end, self.frame_end)
                && !in_rect(
                    x,
                    y,
                    self.frame_inner_start,
                    self.frame_inner_start,
                    self.frame_inner_end,
                    self.frame_inner_end,
                );
        if in_frame {
            return 160;
        }

        let in_monolith = in_rect(
            x,
            y,
            self.content_start,
            self.content_start,
            self.left_end,
            self.content_end,
        );
        let in_vault_wall = in_rect(
            x,
            y,
            self.right_start,
            self.bottom_start,
            self.content_end,
            self.content_end,
        ) && !in_rect(
            x,
            y,
            self.wall_inner_x,
            self.wall_inner_y,
            self.wall_inner_end,
            self.wall_inner_end,
        );
        let in_vault_core = in_rect(
            x,
            y,
            self.core_start_x,
            self.core_start_y,
            self.core_end,
            self.core_end,
        );
        if in_monolith || in_vault_wall || in_vault_core {
            return 255;
        }

        let in_slab = in_rect(
            x,
            y,
            self.right_start,
            self.content_start,
            self.content_end,
            self.top_end,
        );
        if in_slab {
            return 195;
        }

        0
    }
}

const fn in_rect(x: u32, y: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
    x >= x0 && x < x1 && y >= y0 && y < y1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_formats_free_space_and_watchdog_state() {
        assert_eq!(
            format_status_label(None, false, Locale::En),
            "Free: \u{2014} \u{00b7} Watchdog Off"
        );
        assert_eq!(
            format_status_label(Some(1024 * 1024 * 1024), true, Locale::En),
            "Free: 1.0 GiB \u{00b7} Watchdog On"
        );
        for locale in [Locale::Zh, Locale::Ja, Locale::Ko] {
            let off = format_status_label(None, false, locale);
            let on =
                format_status_label(Some(1024 * 1024 * 1024), true, locale);
            assert!(off.contains('\u{2014}'));
            assert!(on.contains("1.0 GiB"));
            assert_ne!(off, on);
            assert_ne!(off, format_status_label(None, false, Locale::En));
        }
    }

    #[test]
    fn icon_rgba_renders_c1_machined_monolith_silhouette() {
        assert_eq!(monochrome_rgb(true), [0, 0, 0]);
        assert_eq!(monochrome_rgb(false), [238, 238, 242]);

        let expected_rgb = monochrome_rgb(cfg!(target_os = "macos"));
        let expected_16 = |x, y| {
            let in_frame =
                in_rect(x, y, 1, 1, 15, 15) && !in_rect(x, y, 2, 2, 14, 14);
            let in_monolith = in_rect(x, y, 3, 3, 7, 13);
            let in_slab = in_rect(x, y, 8, 3, 13, 7);
            let in_vault_wall =
                in_rect(x, y, 8, 8, 13, 13) && !in_rect(x, y, 9, 9, 12, 12);
            let in_vault_core = in_rect(x, y, 10, 10, 11, 11);

            if in_monolith || in_vault_wall || in_vault_core {
                255
            } else if in_slab {
                195
            } else if in_frame {
                160
            } else {
                0
            }
        };
        let expected_32 = |x, y| {
            let in_frame =
                in_rect(x, y, 3, 3, 29, 29) && !in_rect(x, y, 5, 5, 27, 27);
            let in_monolith = in_rect(x, y, 7, 7, 14, 25);
            let in_slab = in_rect(x, y, 16, 7, 25, 14);
            let in_vault_wall =
                in_rect(x, y, 16, 16, 25, 25) && !in_rect(x, y, 18, 18, 23, 23);
            let in_vault_core = in_rect(x, y, 19, 19, 22, 22);

            if in_monolith || in_vault_wall || in_vault_core {
                255
            } else if in_slab {
                195
            } else if in_frame {
                160
            } else {
                0
            }
        };

        assert_c1_grid(
            &build_treemap_icon_rgba(16),
            16,
            expected_rgb,
            expected_16,
        );
        assert_c1_grid(
            &build_treemap_icon_rgba(32),
            32,
            expected_rgb,
            expected_32,
        );
        assert_c1_grid(
            &build_treemap_icon_rgba_with_rgb(16, [0, 0, 0]),
            16,
            [0, 0, 0],
            expected_16,
        );
        assert_c1_grid(
            &build_treemap_icon_rgba_with_rgb(32, [238, 238, 242]),
            32,
            [238, 238, 242],
            expected_32,
        );

        for size in [8u32, 24u32, 48u32] {
            let rgba = build_treemap_icon_rgba(size);
            assert_eq!(rgba.len(), (size as usize) * (size as usize) * 4);
            assert!(rgba.chunks_exact(4).any(|px| px[3] == 160));
            assert!(rgba.chunks_exact(4).any(|px| px[3] == 195));
            assert!(rgba.chunks_exact(4).any(|px| px[3] == 255));
            assert!(rgba.chunks_exact(4).any(|px| px[3] == 0));
            assert!(Icon::from_rgba(rgba, size, size).is_ok());
        }

        let tiny = build_treemap_icon_rgba(7);
        assert!(tiny.iter().all(|&b| b == 0));
    }

    fn assert_c1_grid(
        rgba: &[u8],
        size: u32,
        expected_rgb: [u8; 3],
        expected_alpha_at: impl Fn(u32, u32) -> u8,
    ) {
        assert_eq!(rgba.len(), (size as usize) * (size as usize) * 4);

        for y in 0..size {
            for x in 0..size {
                let idx = ((y * size + x) as usize) * 4;
                let expected_alpha = expected_alpha_at(x, y);
                let expected_px = if expected_alpha == 0 {
                    [0, 0, 0, 0]
                } else {
                    [
                        expected_rgb[0],
                        expected_rgb[1],
                        expected_rgb[2],
                        expected_alpha,
                    ]
                };
                assert_eq!(
                    &rgba[idx..idx + 4],
                    &expected_px,
                    "pixel mismatch at ({x}, {y}) for size {size}"
                );
            }
        }

        assert!(Icon::from_rgba(rgba.to_vec(), size, size).is_ok());
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
