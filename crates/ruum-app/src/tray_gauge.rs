//! Tri-platform system tray / menu-bar icon, surface-tone detection, and
//! disk-space gauge rendering.
//!
//! Encapsulates OS-specific tray visual behavior behind [`TrayGaugeAdapter`]
//! and [`TrayGaugeFactory`]:
//! - macOS: `AppKit` `effectiveAppearance` Dark/Light probing, monochrome
//!   template `C1` monolith, and Retina `48x36` rounded disk capsule with
//!   native menu-bar title
//! - Windows: `HKCU\...\Personalize\SystemUsesLightTheme` taskbar theme probing,
//!   tone-adaptive `C1` monolith, and transparent `32x32` dual-zone icon
//! - Linux: universal dual-contrast `32x32` dark badge (capacity + 3-tier bar)
//!   plus panel title for `ksni` `StatusNotifierItem` trays

use std::fmt;

use ruum_core::space::SpaceInfo;
use tray_icon::{Icon, TrayIcon};

/// Default square tray icon size in pixels.
pub const ICON_SIZE_PX: u32 = 32;

const COMPACT_UNITS: [&str; 7] = ["B", "K", "M", "G", "T", "P", "E"];
const MAC_GAUGE_WIDTH: u32 = 48;
const MAC_GAUGE_HEIGHT: u32 = 36;
const MAC_GAUGE_MAX_FILL_PX: u32 = 32;
const SQUARE_GAUGE_MAX_FILL_PX: u32 = 22;

/// Target operating system layout for menu-bar / tray disk-gauge rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayGaugeOs {
    MacOs,
    Linux,
    Windows,
}

impl TrayGaugeOs {
    /// Return the host operating system variant at compile time.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// Effective background brightness of the host menu bar / tray surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraySurfaceTone {
    Dark,
    #[cfg_attr(
        not(any(target_os = "macos", windows)),
        allow(
            dead_code,
            reason = "Light tone is produced dynamically on macOS/Windows \
                      and exercised in cross-OS tests"
        )
    )]
    Light,
}

/// Visual severity level derived from disk usage percentage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAlertLevel {
    Normal,
    Warning,
    Critical,
}

/// Host tray surface environment passed into [`TrayGaugeAdapter`] methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayGaugeEnv {
    pub surface_tone: TraySurfaceTone,
}

impl TrayGaugeEnv {
    /// Default dark tray environment.
    #[must_use]
    pub const fn dark() -> Self {
        Self::from_surface_tone(TraySurfaceTone::Dark)
    }

    /// Construct a tray environment with an explicit [`TraySurfaceTone`].
    #[must_use]
    pub const fn from_surface_tone(surface_tone: TraySurfaceTone) -> Self {
        Self { surface_tone }
    }
}

/// Rendered RGBA tray gauge icon and optional menu-bar title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayGaugeRender {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub title: Option<String>,
    pub icon_is_template: bool,
    pub fill_px: u32,
    pub max_fill_px: u32,
    pub used_percent: u8,
    pub alert_level: TrayAlertLevel,
}

/// Rendered tray icon state (either the default C1 monolith or disk gauge).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayVisualRender {
    MonochromeC1 {
        size: u32,
        rgba: Vec<u8>,
        icon_is_template: bool,
    },
    DiskGauge(TrayGaugeRender),
}

/// Lightweight comparable signature used to skip redundant `set_icon` calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayVisualSignature {
    MonochromeC1 {
        os: TrayGaugeOs,
        surface_tone: TraySurfaceTone,
    },
    DiskGauge {
        os: TrayGaugeOs,
        title: Option<String>,
        compact_label: Option<String>,
        fill_px: u32,
        alert_level: TrayAlertLevel,
        surface_tone: TraySurfaceTone,
    },
}

/// Classify an `AppKit` `NSAppearanceName` string into a light or dark menu bar
/// tone.
#[cfg(any(target_os = "macos", test))]
#[must_use]
pub fn surface_tone_for_appearance_name(name: &str) -> TraySurfaceTone {
    if name.contains("Dark") {
        TraySurfaceTone::Dark
    } else {
        TraySurfaceTone::Light
    }
}

/// Classify Windows `Personalize\SystemUsesLightTheme` (`Some(true)` -> light,
/// otherwise dark) into a [`TraySurfaceTone`].
#[cfg(any(windows, test))]
#[must_use]
pub const fn surface_tone_for_windows_light_theme(
    uses_light_theme: Option<bool>,
) -> TraySurfaceTone {
    match uses_light_theme {
        Some(true) => TraySurfaceTone::Light,
        Some(false) | None => TraySurfaceTone::Dark,
    }
}

#[cfg(windows)]
mod win_taskbar_theme {
    #![allow(
        unsafe_code,
        reason = "Win32 RegGetValueW query for HKCU \
                  Personalize\\SystemUsesLightTheme"
    )]

    use std::ffi::c_void;

    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW,
    };

    pub(super) fn system_uses_light_theme() -> Option<bool> {
        let subkey: Vec<u16> =
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
        let value: Vec<u16> = "SystemUsesLightTheme"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: `subkey` and `value` are NUL-terminated UTF-16 buffers that
        // outlive the call; `data` and `size` point to stack storage sized for
        // one `DWORD` matching `RRF_RT_REG_DWORD`.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut data).cast::<c_void>(),
                &raw mut size,
            )
        };
        (status == 0).then_some(data != 0)
    }
}

/// Adapter interface encapsulating OS-specific tray surface detection and icon
/// rendering.
pub trait TrayGaugeAdapter: Send + Sync + fmt::Debug {
    fn os_kind(&self) -> TrayGaugeOs;
    fn initial_env(&self) -> TrayGaugeEnv;
    fn probe_env(&self, tray: &TrayIcon) -> TrayGaugeEnv;
    fn effective_tone(&self, env: TrayGaugeEnv) -> TraySurfaceTone;
    fn monochrome_signature_tone(&self, env: TrayGaugeEnv) -> TraySurfaceTone {
        self.effective_tone(env)
    }
    fn max_fill_px(&self) -> u32;
    fn gauge_title(&self, space: SpaceInfo) -> Option<String>;
    fn compact_label(&self, space: SpaceInfo) -> Option<String>;
    fn render_monochrome_c1(
        &self,
        size: u32,
        env: TrayGaugeEnv,
    ) -> TrayVisualRender;
    fn render_disk_gauge(
        &self,
        space: SpaceInfo,
        env: TrayGaugeEnv,
    ) -> TrayGaugeRender;
    fn apply_icon(
        &self,
        tray: &TrayIcon,
        icon: Icon,
        is_template: bool,
    ) -> tray_icon::Result<()>;

    fn visual_signature(
        &self,
        space: Option<SpaceInfo>,
        show_disk_space: bool,
        env: TrayGaugeEnv,
    ) -> TrayVisualSignature {
        if show_disk_space && let Some(sp) = space {
            return TrayVisualSignature::DiskGauge {
                os: self.os_kind(),
                title: self.gauge_title(sp),
                compact_label: self.compact_label(sp),
                fill_px: calc_fill_px(self.max_fill_px(), sp),
                alert_level: alert_level_for_used_fraction(sp.used_fraction()),
                surface_tone: self.effective_tone(env),
            };
        }
        TrayVisualSignature::MonochromeC1 {
            os: self.os_kind(),
            surface_tone: self.monochrome_signature_tone(env),
        }
    }

    fn build_visual(
        &self,
        space: Option<SpaceInfo>,
        show_disk_space: bool,
        env: TrayGaugeEnv,
    ) -> TrayVisualRender {
        if show_disk_space && let Some(sp) = space {
            return TrayVisualRender::DiskGauge(
                self.render_disk_gauge(sp, env),
            );
        }
        self.render_monochrome_c1(ICON_SIZE_PX, env)
    }
}

/// macOS menu-bar tray gauge adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsTrayGaugeAdapter;

impl TrayGaugeAdapter for MacOsTrayGaugeAdapter {
    fn os_kind(&self) -> TrayGaugeOs {
        TrayGaugeOs::MacOs
    }

    fn initial_env(&self) -> TrayGaugeEnv {
        TrayGaugeEnv::dark()
    }

    fn probe_env(&self, tray: &TrayIcon) -> TrayGaugeEnv {
        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::NSAppearanceCustomization as _;

            let Some(mtm) = objc2::MainThreadMarker::new() else {
                return TrayGaugeEnv::dark();
            };
            let Some(item) = tray.ns_status_item() else {
                return TrayGaugeEnv::dark();
            };
            let Some(button) = item.button(mtm) else {
                return TrayGaugeEnv::dark();
            };
            let name = button.effectiveAppearance().name().to_string();
            TrayGaugeEnv::from_surface_tone(surface_tone_for_appearance_name(
                &name,
            ))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = tray;
            TrayGaugeEnv::dark()
        }
    }

    fn effective_tone(&self, env: TrayGaugeEnv) -> TraySurfaceTone {
        env.surface_tone
    }

    fn monochrome_signature_tone(&self, _env: TrayGaugeEnv) -> TraySurfaceTone {
        // AppKit automatically re-tints template status icons on appearance
        // changes, so the monochrome C1 image stays invariant.
        TraySurfaceTone::Dark
    }

    fn max_fill_px(&self) -> u32 {
        MAC_GAUGE_MAX_FILL_PX
    }

    fn gauge_title(&self, space: SpaceInfo) -> Option<String> {
        Some(format!(" {}", format_compact_free_bytes(space.available)))
    }

    fn compact_label(&self, _space: SpaceInfo) -> Option<String> {
        None
    }

    fn render_monochrome_c1(
        &self,
        size: u32,
        _env: TrayGaugeEnv,
    ) -> TrayVisualRender {
        TrayVisualRender::MonochromeC1 {
            size,
            rgba: build_treemap_icon_rgba_with_rgb(size, [0, 0, 0]),
            icon_is_template: true,
        }
    }

    fn render_disk_gauge(
        &self,
        space: SpaceInfo,
        env: TrayGaugeEnv,
    ) -> TrayGaugeRender {
        let width = MAC_GAUGE_WIDTH;
        let height = MAC_GAUGE_HEIGHT;
        let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
        let fill_px = calc_fill_px(MAC_GAUGE_MAX_FILL_PX, space);
        let used_percent = calc_used_percent(space);
        let alert_level = alert_level_for_used_fraction(space.used_fraction());
        let palette = surface_palette(self.effective_tone(env), alert_level);

        draw_rounded_gauge_bar(
            &mut rgba,
            width,
            height,
            &RoundedGaugeSpec {
                outer: (3, 7, 45, 29, 6.0),
                inner: (6, 10, 42, 26, 3.0),
                fill: (8, 12, 40, 24),
            },
            fill_px,
            &palette,
        );

        TrayGaugeRender {
            width,
            height,
            rgba,
            title: self.gauge_title(space),
            icon_is_template: false,
            fill_px,
            max_fill_px: MAC_GAUGE_MAX_FILL_PX,
            used_percent,
            alert_level,
        }
    }

    fn apply_icon(
        &self,
        tray: &TrayIcon,
        icon: Icon,
        is_template: bool,
    ) -> tray_icon::Result<()> {
        #[cfg(target_os = "macos")]
        {
            tray.set_icon_with_as_template(Some(icon), is_template)?;
            // `tray-icon` mutates image size and template after
            // `button.setImage`, so re-applying template mode ensures AppKit
            // re-reads the resized frame.
            tray.set_icon_as_template(is_template);
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = is_template;
            tray.set_icon(Some(icon))
        }
    }
}

/// Windows taskbar tray gauge adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsTrayGaugeAdapter;

impl TrayGaugeAdapter for WindowsTrayGaugeAdapter {
    fn os_kind(&self) -> TrayGaugeOs {
        TrayGaugeOs::Windows
    }

    fn initial_env(&self) -> TrayGaugeEnv {
        #[cfg(windows)]
        {
            TrayGaugeEnv::from_surface_tone(
                surface_tone_for_windows_light_theme(
                    win_taskbar_theme::system_uses_light_theme(),
                ),
            )
        }
        #[cfg(not(windows))]
        {
            TrayGaugeEnv::dark()
        }
    }

    fn probe_env(&self, _tray: &TrayIcon) -> TrayGaugeEnv {
        self.initial_env()
    }

    fn effective_tone(&self, env: TrayGaugeEnv) -> TraySurfaceTone {
        env.surface_tone
    }

    fn max_fill_px(&self) -> u32 {
        SQUARE_GAUGE_MAX_FILL_PX
    }

    fn gauge_title(&self, _space: SpaceInfo) -> Option<String> {
        None
    }

    fn compact_label(&self, space: SpaceInfo) -> Option<String> {
        Some(format_square_compact_free_bytes(space.available))
    }

    fn render_monochrome_c1(
        &self,
        size: u32,
        env: TrayGaugeEnv,
    ) -> TrayVisualRender {
        let rgb = match self.effective_tone(env) {
            TraySurfaceTone::Dark => [248, 250, 252],
            TraySurfaceTone::Light => [24, 24, 28],
        };
        TrayVisualRender::MonochromeC1 {
            size,
            rgba: build_treemap_icon_rgba_with_rgb(size, rgb),
            icon_is_template: false,
        }
    }

    fn render_disk_gauge(
        &self,
        space: SpaceInfo,
        env: TrayGaugeEnv,
    ) -> TrayGaugeRender {
        let width = ICON_SIZE_PX;
        let height = ICON_SIZE_PX;
        let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
        let fill_px = calc_fill_px(SQUARE_GAUGE_MAX_FILL_PX, space);
        let used_percent = calc_used_percent(space);
        let alert_level = alert_level_for_used_fraction(space.used_fraction());
        let palette = surface_palette(self.effective_tone(env), alert_level);

        // Lower zone (y: 17..29): transparent-backed rounded bordered bar so
        // 16x16 downscaling in the Windows notification area stays crisp.
        draw_rounded_gauge_bar(
            &mut rgba,
            width,
            height,
            &RoundedGaugeSpec {
                outer: (2, 17, 30, 29, 3.0),
                inner: (4, 19, 28, 27, 1.5),
                fill: (5, 20, 27, 26),
            },
            fill_px,
            &palette,
        );

        // Upper zone (y: 3..13): 2x-scaled 3x5 pixel glyphs for compact free
        // space.
        let label = format_square_compact_free_bytes(space.available);
        draw_pixel_text_centered(
            &mut rgba,
            width,
            3,
            &label,
            palette.text_rgba,
        );

        TrayGaugeRender {
            width,
            height,
            rgba,
            title: None,
            icon_is_template: false,
            fill_px,
            max_fill_px: SQUARE_GAUGE_MAX_FILL_PX,
            used_percent,
            alert_level,
        }
    }

    fn apply_icon(
        &self,
        tray: &TrayIcon,
        icon: Icon,
        _is_template: bool,
    ) -> tray_icon::Result<()> {
        tray.set_icon(Some(icon))
    }
}

/// Linux `ksni` tray gauge adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxTrayGaugeAdapter;

impl TrayGaugeAdapter for LinuxTrayGaugeAdapter {
    fn os_kind(&self) -> TrayGaugeOs {
        TrayGaugeOs::Linux
    }

    fn initial_env(&self) -> TrayGaugeEnv {
        TrayGaugeEnv::dark()
    }

    fn probe_env(&self, _tray: &TrayIcon) -> TrayGaugeEnv {
        TrayGaugeEnv::dark()
    }

    fn effective_tone(&self, _env: TrayGaugeEnv) -> TraySurfaceTone {
        // Universal dark dual-contrast badge stays legible on both light and
        // dark Linux desktop panels.
        TraySurfaceTone::Dark
    }

    fn max_fill_px(&self) -> u32 {
        SQUARE_GAUGE_MAX_FILL_PX
    }

    fn gauge_title(&self, space: SpaceInfo) -> Option<String> {
        Some(format_compact_free_bytes(space.available))
    }

    fn compact_label(&self, space: SpaceInfo) -> Option<String> {
        Some(format_square_compact_free_bytes(space.available))
    }

    fn render_monochrome_c1(
        &self,
        size: u32,
        _env: TrayGaugeEnv,
    ) -> TrayVisualRender {
        TrayVisualRender::MonochromeC1 {
            size,
            rgba: build_treemap_icon_rgba_with_rgb(size, [238, 238, 242]),
            icon_is_template: false,
        }
    }

    fn render_disk_gauge(
        &self,
        space: SpaceInfo,
        _env: TrayGaugeEnv,
    ) -> TrayGaugeRender {
        let width = ICON_SIZE_PX;
        let height = ICON_SIZE_PX;
        let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
        let fill_px = calc_fill_px(SQUARE_GAUGE_MAX_FILL_PX, space);
        let used_percent = calc_used_percent(space);
        let alert_level = alert_level_for_used_fraction(space.used_fraction());
        let fill_color = linux_alert_fill_rgba(alert_level);

        // Dual-contrast dark rounded badge (outer 0..32 r=5.0, inner 1..31
        // r=4.0).
        for y in 0..height {
            for x in 0..width {
                let outer_cov = rounded_rect_coverage(x, y, 0, 0, 32, 32, 5.0);
                if outer_cov <= 0.0 {
                    continue;
                }
                let inner_cov = rounded_rect_coverage(x, y, 1, 1, 31, 31, 4.0);
                let rim_cov = (outer_cov - inner_cov).clamp(0.0, 1.0);
                if rim_cov > 0.0 {
                    let alpha = (rim_cov * 220.0).round() as u8;
                    set_px(&mut rgba, width, x, y, [120, 128, 144, alpha]);
                } else {
                    let alpha = (outer_cov * 236.0).round() as u8;
                    set_px(&mut rgba, width, x, y, [22, 24, 30, alpha]);
                }
            }
        }

        // Lower zone (y: 19..28): clean bordered 3-tier progress bar with
        // 1 px dark inner moat.
        for y in 19..28 {
            for x in 3..29 {
                if in_rect(x, y, 5, 21, 27, 26) {
                    let px = if x < 5 + fill_px {
                        fill_color
                    } else {
                        [44, 48, 58, 255]
                    };
                    set_px(&mut rgba, width, x, y, px);
                } else if in_rect(x, y, 4, 20, 28, 27) {
                    set_px(&mut rgba, width, x, y, [22, 24, 30, 255]);
                } else {
                    set_px(&mut rgba, width, x, y, [188, 196, 210, 235]);
                }
            }
        }

        // Upper zone (y: 4..14): 2x crisp white pixel text so Waybar / KDE /
        // GNOME display capacity without hovering.
        let label = format_square_compact_free_bytes(space.available);
        draw_pixel_text_centered(
            &mut rgba,
            width,
            4,
            &label,
            [248, 250, 252, 255],
        );

        TrayGaugeRender {
            width,
            height,
            rgba,
            title: self.gauge_title(space),
            icon_is_template: false,
            fill_px,
            max_fill_px: SQUARE_GAUGE_MAX_FILL_PX,
            used_percent,
            alert_level,
        }
    }

    fn apply_icon(
        &self,
        tray: &TrayIcon,
        icon: Icon,
        _is_template: bool,
    ) -> tray_icon::Result<()> {
        tray.set_icon(Some(icon))
    }
}

/// Factory selecting the [`TrayGaugeAdapter`] for the host or a requested
/// target OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrayGaugeFactory;

impl TrayGaugeFactory {
    /// Detect the current operating system family.
    #[must_use]
    pub const fn current_os() -> TrayGaugeOs {
        TrayGaugeOs::current()
    }

    /// Instantiate the adapter for a specific target OS.
    #[must_use]
    pub fn create_for(os: TrayGaugeOs) -> Box<dyn TrayGaugeAdapter> {
        match os {
            TrayGaugeOs::MacOs => Box::new(MacOsTrayGaugeAdapter),
            TrayGaugeOs::Linux => Box::new(LinuxTrayGaugeAdapter),
            TrayGaugeOs::Windows => Box::new(WindowsTrayGaugeAdapter),
        }
    }

    /// Instantiate the adapter for the current host OS.
    #[must_use]
    pub fn create() -> Box<dyn TrayGaugeAdapter> {
        Self::create_for(Self::current_os())
    }
}

/// Calculate integer used percentage clamped to `0..=100`.
#[must_use]
pub fn calc_used_percent(space: SpaceInfo) -> u8 {
    if space.total == 0 {
        return 0;
    }
    let pct =
        (f64::from(space.used_fraction().clamp(0.0, 1.0)) * 100.0).round();
    (pct as u32).min(100) as u8
}

/// Format remaining bytes into compact menu-bar text such as `"42.5G"` or
/// `"1.2T"`.
#[must_use]
pub fn format_compact_free_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1023.5 && unit + 1 < COMPACT_UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:.0}{}", value.round(), COMPACT_UNITS[unit])
    } else if value < 99.95 {
        format!("{value:.1}{}", COMPACT_UNITS[unit])
    } else {
        format!("{:.0}{}", value.round(), COMPACT_UNITS[unit])
    }
}

/// Format remaining bytes into `<= 4` characters for `32x32` square tray icons.
#[must_use]
pub fn format_square_compact_free_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 999.5 && unit + 1 < COMPACT_UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:.0}{}", value.round(), COMPACT_UNITS[unit])
    } else if value < 9.95 {
        format!("{value:.1}{}", COMPACT_UNITS[unit])
    } else {
        format!("{:.0}{}", value.round(), COMPACT_UNITS[unit])
    }
}

const fn alert_level_for_used_fraction(used_fraction: f32) -> TrayAlertLevel {
    if used_fraction >= 0.90 {
        TrayAlertLevel::Critical
    } else if used_fraction >= 0.75 {
        TrayAlertLevel::Warning
    } else {
        TrayAlertLevel::Normal
    }
}

fn calc_fill_px(max_width: u32, space: SpaceInfo) -> u32 {
    if space.total == 0 || space.used() == 0 {
        return 0;
    }
    if space.used() >= space.total {
        return max_width;
    }
    let frac = f64::from(space.used_fraction().clamp(0.0, 1.0));
    let raw = (frac * f64::from(max_width)).round() as u32;
    // Keep a visible sliver above 0% and a visible gap below 100%.
    raw.clamp(1, max_width.saturating_sub(1))
}

const fn linux_alert_fill_rgba(alert_level: TrayAlertLevel) -> [u8; 4] {
    match alert_level {
        TrayAlertLevel::Normal => [74, 222, 128, 255],
        TrayAlertLevel::Warning => [251, 191, 36, 255],
        TrayAlertLevel::Critical => [248, 113, 113, 255],
    }
}

struct SurfacePalette {
    frame_rgb: [u8; 3],
    frame_max_alpha: f32,
    text_rgba: [u8; 4],
    track_rgba: [u8; 4],
    fill_rgba: [u8; 4],
}

const fn surface_palette(
    surface_tone: TraySurfaceTone,
    alert_level: TrayAlertLevel,
) -> SurfacePalette {
    match surface_tone {
        TraySurfaceTone::Dark => SurfacePalette {
            frame_rgb: [248, 250, 252],
            frame_max_alpha: 215.0,
            text_rgba: [248, 250, 252, 255],
            track_rgba: [255, 255, 255, 48],
            fill_rgba: match alert_level {
                TrayAlertLevel::Normal => [48, 209, 88, 255],
                TrayAlertLevel::Warning => [251, 191, 36, 255],
                TrayAlertLevel::Critical => [255, 69, 58, 255],
            },
        },
        TraySurfaceTone::Light => SurfacePalette {
            frame_rgb: [24, 24, 28],
            frame_max_alpha: 205.0,
            text_rgba: [24, 24, 28, 255],
            track_rgba: [0, 0, 0, 38],
            fill_rgba: match alert_level {
                TrayAlertLevel::Normal => [34, 178, 76, 255],
                TrayAlertLevel::Warning => [217, 130, 12, 255],
                TrayAlertLevel::Critical => [229, 46, 38, 255],
            },
        },
    }
}

struct RoundedGaugeSpec {
    outer: (u32, u32, u32, u32, f32),
    inner: (u32, u32, u32, u32, f32),
    fill: (u32, u32, u32, u32),
}

fn draw_rounded_gauge_bar(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    spec: &RoundedGaugeSpec,
    fill_px: u32,
    palette: &SurfacePalette,
) {
    let (ox0, oy0, ox1, oy1, or) = spec.outer;
    let (ix0, iy0, ix1, iy1, ir) = spec.inner;
    let (fx0, fy0, fx1, fy1) = spec.fill;
    for y in 0..height {
        for x in 0..width {
            if in_rect(x, y, fx0, fy0, fx1, fy1) {
                let px = if x < fx0 + fill_px {
                    palette.fill_rgba
                } else {
                    palette.track_rgba
                };
                set_px(rgba, width, x, y, px);
                continue;
            }
            let outer_cov = rounded_rect_coverage(x, y, ox0, oy0, ox1, oy1, or);
            let inner_cov = rounded_rect_coverage(x, y, ix0, iy0, ix1, iy1, ir);
            let frame_cov = (outer_cov - inner_cov).clamp(0.0, 1.0);
            if frame_cov > 0.0 {
                let alpha = (frame_cov * palette.frame_max_alpha).round() as u8;
                if alpha > 0 {
                    set_px(
                        rgba,
                        width,
                        x,
                        y,
                        [
                            palette.frame_rgb[0],
                            palette.frame_rgb[1],
                            palette.frame_rgb[2],
                            alpha,
                        ],
                    );
                }
            }
        }
    }
}

fn draw_pixel_text_centered(
    rgba: &mut [u8],
    canvas_width: u32,
    y_top: u32,
    text: &str,
    color: [u8; 4],
) {
    const SCALE: u32 = 2;
    let char_count = text.chars().count() as u32;
    if char_count == 0 {
        return;
    }
    let glyphs_width: u32 =
        text.chars().map(|ch| glyph_bitmap(ch).0 * SCALE).sum();
    let wide_total = glyphs_width + char_count.saturating_sub(1) * 2;
    let gap = if wide_total <= 28 { 2 } else { 1 };
    let total_width = glyphs_width + char_count.saturating_sub(1) * gap;
    let mut cursor_x = canvas_width.saturating_sub(total_width) / 2;

    for ch in text.chars() {
        let (logical_w, rows) = glyph_bitmap(ch);
        for (row_idx, &bits) in rows.iter().enumerate() {
            for col in 0..logical_w {
                let shift = logical_w - 1 - col;
                if (bits >> shift) & 1 != 0 {
                    for dy in 0..SCALE {
                        for dx in 0..SCALE {
                            let px = cursor_x + col * SCALE + dx;
                            let py = y_top + (row_idx as u32) * SCALE + dy;
                            if px < canvas_width {
                                set_px(rgba, canvas_width, px, py, color);
                            }
                        }
                    }
                }
            }
        }
        cursor_x += logical_w * SCALE + gap;
    }
}

const fn glyph_bitmap(ch: char) -> (u32, [u8; 5]) {
    match ch {
        '0' => (3, [0b111, 0b101, 0b101, 0b101, 0b111]),
        '1' => (3, [0b010, 0b110, 0b010, 0b010, 0b111]),
        '2' => (3, [0b111, 0b001, 0b111, 0b100, 0b111]),
        '3' => (3, [0b111, 0b001, 0b111, 0b001, 0b111]),
        '4' => (3, [0b101, 0b101, 0b111, 0b001, 0b001]),
        '5' => (3, [0b111, 0b100, 0b111, 0b001, 0b111]),
        '6' => (3, [0b111, 0b100, 0b111, 0b101, 0b111]),
        '7' => (3, [0b111, 0b001, 0b001, 0b010, 0b010]),
        '8' => (3, [0b111, 0b101, 0b111, 0b101, 0b111]),
        '9' => (3, [0b111, 0b101, 0b111, 0b001, 0b111]),
        '.' => (1, [0b000, 0b000, 0b000, 0b000, 0b001]),
        'B' => (3, [0b110, 0b101, 0b110, 0b101, 0b110]),
        'K' => (3, [0b101, 0b101, 0b110, 0b101, 0b101]),
        'M' => (3, [0b101, 0b111, 0b111, 0b101, 0b101]),
        'G' => (3, [0b111, 0b100, 0b101, 0b101, 0b111]),
        'T' => (3, [0b111, 0b010, 0b010, 0b010, 0b010]),
        'P' => (3, [0b111, 0b101, 0b111, 0b100, 0b100]),
        'E' => (3, [0b111, 0b100, 0b111, 0b100, 0b111]),
        _ => (3, [0b000, 0b000, 0b111, 0b000, 0b000]),
    }
}

fn rounded_rect_coverage(
    x: u32,
    y: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    radius: f32,
) -> f32 {
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let left = x0 as f32 + radius;
    let right = x1 as f32 - radius;
    let top = y0 as f32 + radius;
    let bottom = y1 as f32 - radius;
    let cx = px.clamp(left, right);
    let cy = py.clamp(top, bottom);
    let dist = (px - cx).hypot(py - cy);
    (radius + 0.5 - dist).clamp(0.0, 1.0)
}

fn set_px(rgba: &mut [u8], width: u32, x: u32, y: u32, px: [u8; 4]) {
    let idx = ((y * width + x) as usize) * 4;
    if let Some(slot) = rgba.get_mut(idx..idx + 4) {
        slot.copy_from_slice(&px);
    }
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
    fn factory_creates_all_three_adapters_matching_os_kind() {
        for (os, expected) in [
            (TrayGaugeOs::MacOs, TrayGaugeOs::MacOs),
            (TrayGaugeOs::Linux, TrayGaugeOs::Linux),
            (TrayGaugeOs::Windows, TrayGaugeOs::Windows),
        ] {
            let adapter = TrayGaugeFactory::create_for(os);
            assert_eq!(adapter.os_kind(), expected);
        }
        assert_eq!(TrayGaugeFactory::current_os(), TrayGaugeOs::current());
        assert_eq!(
            TrayGaugeFactory::create().os_kind(),
            TrayGaugeOs::current()
        );
    }

    #[test]
    fn icon_rgba_renders_c1_machined_monolith_silhouette_and_adapts_windows_tone()
     {
        type AlphaFn<'a> = &'a dyn Fn(u32, u32) -> u8;

        let expected_16 = |x: u32, y: u32| -> u8 {
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
        let expected_32 = |x: u32, y: u32| -> u8 {
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

        let mac = TrayGaugeFactory::create_for(TrayGaugeOs::MacOs);
        let linux = TrayGaugeFactory::create_for(TrayGaugeOs::Linux);
        let win = TrayGaugeFactory::create_for(TrayGaugeOs::Windows);
        let dark_env = TrayGaugeEnv::dark();
        let light_env = TrayGaugeEnv::from_surface_tone(TraySurfaceTone::Light);
        let expected_checks: [(u32, AlphaFn<'_>); 2] =
            [(16, &expected_16), (32, &expected_32)];

        for (adapter, env, expected_rgb, expected_template) in [
            (&mac, dark_env, [0, 0, 0], true),
            (&mac, light_env, [0, 0, 0], true),
            (&linux, dark_env, [238, 238, 242], false),
            (&linux, light_env, [238, 238, 242], false),
            (&win, dark_env, [248, 250, 252], false),
            (&win, light_env, [24, 24, 28], false),
        ] {
            for (sz, expected_fn) in expected_checks {
                match adapter.render_monochrome_c1(sz, env) {
                    TrayVisualRender::MonochromeC1 {
                        size,
                        rgba,
                        icon_is_template,
                    } => {
                        assert_eq!(size, sz);
                        assert_eq!(icon_is_template, expected_template);
                        assert_c1_grid(&rgba, size, expected_rgb, expected_fn);
                    }
                    TrayVisualRender::DiskGauge(_) => {
                        panic!("expected MonochromeC1 render")
                    }
                }
            }
        }

        for size in [8u32, 24u32, 48u32] {
            let TrayVisualRender::MonochromeC1 { rgba, .. } =
                linux.render_monochrome_c1(size, dark_env)
            else {
                panic!("expected MonochromeC1");
            };
            assert_eq!(rgba.len(), (size as usize) * (size as usize) * 4);
            assert!(rgba.chunks_exact(4).any(|px| px[3] == 160));
            assert!(rgba.chunks_exact(4).any(|px| px[3] == 195));
            assert!(rgba.chunks_exact(4).any(|px| px[3] == 255));
            assert!(rgba.chunks_exact(4).any(|px| px[3] == 0));
            assert!(Icon::from_rgba(rgba, size, size).is_ok());
        }

        let TrayVisualRender::MonochromeC1 { rgba: tiny, .. } =
            win.render_monochrome_c1(7, dark_env)
        else {
            panic!("expected MonochromeC1");
        };
        assert!(tiny.iter().all(|&b| b == 0));
    }

    #[test]
    fn compact_free_bytes_formats_g_and_t_cleanly() {
        const GIB: u64 = 1024 * 1024 * 1024;
        const TIB: u64 = 1024 * GIB;

        assert_eq!(format_compact_free_bytes(0), "0B");
        assert_eq!(format_compact_free_bytes(512 * 1024 * 1024), "512M");
        assert_eq!(format_compact_free_bytes(42 * GIB + GIB / 2), "42.5G");
        assert_eq!(format_compact_free_bytes(100 * GIB), "100G");
        assert_eq!(format_compact_free_bytes(TIB + (TIB / 5)), "1.2T");

        assert_eq!(format_square_compact_free_bytes(0), "0B");
        assert_eq!(format_square_compact_free_bytes(9 * GIB), "9.0G");
        assert_eq!(format_square_compact_free_bytes(42 * GIB + GIB / 2), "43G");
        assert_eq!(format_square_compact_free_bytes(512 * GIB), "512G");
        assert_eq!(format_square_compact_free_bytes(1000 * GIB), "1.0T");
        assert_eq!(format_square_compact_free_bytes(TIB + (TIB / 5)), "1.2T");
        assert!(format_square_compact_free_bytes(u64::MAX).len() <= 4);
    }

    #[test]
    fn alert_level_thresholds_match_75_and_90_percent_boundaries() {
        assert_eq!(alert_level_for_used_fraction(0.0), TrayAlertLevel::Normal);
        assert_eq!(
            alert_level_for_used_fraction(0.749),
            TrayAlertLevel::Normal
        );
        assert_eq!(
            alert_level_for_used_fraction(0.75),
            TrayAlertLevel::Warning
        );
        assert_eq!(
            alert_level_for_used_fraction(0.899),
            TrayAlertLevel::Warning
        );
        assert_eq!(
            alert_level_for_used_fraction(0.90),
            TrayAlertLevel::Critical
        );
        assert_eq!(
            alert_level_for_used_fraction(1.0),
            TrayAlertLevel::Critical
        );
    }

    #[test]
    fn tray_disk_gauge_renders_tri_platform_fill_proportions_and_valid_icons() {
        const GIB: u64 = 1024 * 1024 * 1024;
        const TIB: u64 = 1024 * GIB;
        let total = 1000 * GIB;
        let avail_42_5g = 42 * GIB + GIB / 2;

        let cases = [
            (0u64, 0u8),
            (500 * GIB, 50u8),
            (750 * GIB, 75u8),
            (900 * GIB, 90u8),
            (990 * GIB, 99u8),
            (1000 * GIB, 100u8),
        ];

        let mac_adapter = TrayGaugeFactory::create_for(TrayGaugeOs::MacOs);
        let linux_adapter = TrayGaugeFactory::create_for(TrayGaugeOs::Linux);
        let win_adapter = TrayGaugeFactory::create_for(TrayGaugeOs::Windows);
        let dark_env = TrayGaugeEnv::dark();
        let light_env = TrayGaugeEnv::from_surface_tone(TraySurfaceTone::Light);

        let mut mac_fills = Vec::new();
        let mut linux_fills = Vec::new();
        let mut win_fills = Vec::new();

        let px_at = |g: &TrayGaugeRender, x: u32, y: u32| -> [u8; 4] {
            let idx = ((y * g.width + x) as usize) * 4;
            [
                g.rgba[idx],
                g.rgba[idx + 1],
                g.rgba[idx + 2],
                g.rgba[idx + 3],
            ]
        };

        for (used, expected_pct) in cases {
            let space = SpaceInfo {
                total,
                free: total - used,
                available: avail_42_5g,
            };

            // macOS Retina colored capsule (48x36)
            let mac = mac_adapter.render_disk_gauge(space, dark_env);
            assert_eq!(mac.width, 48);
            assert_eq!(mac.height, 36);
            assert_eq!(mac.title.as_deref(), Some(" 42.5G"));
            assert!(!mac.icon_is_template);
            assert_eq!(mac.used_percent, expected_pct);
            assert_eq!(mac.max_fill_px, 32);
            assert!(
                Icon::from_rgba(mac.rgba.clone(), mac.width, mac.height)
                    .is_ok()
            );
            let mac_filled = (8..40)
                .filter(|&x| px_at(&mac, x, 18) != [255, 255, 255, 48])
                .count() as u32;
            assert_eq!(mac_filled, mac.fill_px);
            mac_fills.push(mac_filled);

            // Linux dual-contrast dark badge (32x32) + title
            let lin = linux_adapter.render_disk_gauge(space, dark_env);
            assert_eq!(lin.width, 32);
            assert_eq!(lin.height, 32);
            assert_eq!(lin.title.as_deref(), Some("42.5G"));
            assert!(!lin.icon_is_template);
            assert_eq!(lin.used_percent, expected_pct);
            assert_eq!(lin.max_fill_px, 22);
            assert!(
                Icon::from_rgba(lin.rgba.clone(), lin.width, lin.height)
                    .is_ok()
            );
            assert_eq!(px_at(&lin, 15, 0), [120, 128, 144, 220]);
            assert_eq!(px_at(&lin, 15, 1), [22, 24, 30, 236]);
            let lin_glyph_px = (4..14)
                .flat_map(|y| (2..30).map(move |x| (x, y)))
                .filter(|&(x, y)| px_at(&lin, x, y) == [248, 250, 252, 255])
                .count();
            assert!(
                lin_glyph_px > 20,
                "expected crisp upper text glyphs on Linux"
            );
            let lin_filled = (5..27)
                .filter(|&x| px_at(&lin, x, 23) != [44, 48, 58, 255])
                .count() as u32;
            assert_eq!(lin_filled, lin.fill_px);
            linux_fills.push(lin_filled);

            // Windows 32x32 transparent dual-zone icon (text top + bar bottom)
            let win = win_adapter.render_disk_gauge(space, dark_env);
            assert_eq!(win.width, 32);
            assert_eq!(win.height, 32);
            assert_eq!(win.title, None);
            assert!(!win.icon_is_template);
            assert_eq!(win.used_percent, expected_pct);
            assert_eq!(win.max_fill_px, 22);
            assert!(
                Icon::from_rgba(win.rgba.clone(), win.width, win.height)
                    .is_ok()
            );
            for (cx, cy) in [(0, 0), (31, 0), (0, 31), (31, 31)] {
                assert_eq!(px_at(&win, cx, cy)[3], 0);
            }
            let win_glyph_px = (3..13)
                .flat_map(|y| (1..31).map(move |x| (x, y)))
                .filter(|&(x, y)| px_at(&win, x, y) == [248, 250, 252, 255])
                .count();
            assert!(
                win_glyph_px > 20,
                "expected crisp upper text glyphs on Windows"
            );
            let win_filled = (5..27)
                .filter(|&x| px_at(&win, x, 23) != [255, 255, 255, 48])
                .count() as u32;
            assert_eq!(win_filled, win.fill_px);
            win_fills.push(win_filled);
        }

        assert_eq!(mac_fills, vec![0, 16, 24, 29, 31, 32]);
        assert_eq!(linux_fills, vec![0, 11, 17, 20, 21, 22]);
        assert_eq!(win_fills, vec![0, 11, 17, 20, 21, 22]);

        // Verify 3-tier Green / Amber / Red colors on macOS Dark/Light,
        // Windows Dark/Light, and Linux.
        let space_for = |used: u64| SpaceInfo {
            total,
            free: total - used,
            available: avail_42_5g,
        };

        let mac_dark_colors = [500 * GIB, 750 * GIB, 900 * GIB]
            .map(|u| mac_adapter.render_disk_gauge(space_for(u), dark_env))
            .map(|g| px_at(&g, 9, 18));
        assert_eq!(
            mac_dark_colors,
            [[48, 209, 88, 255], [251, 191, 36, 255], [255, 69, 58, 255]]
        );

        let mac_light_colors = [500 * GIB, 750 * GIB, 900 * GIB]
            .map(|u| mac_adapter.render_disk_gauge(space_for(u), light_env))
            .map(|g| px_at(&g, 9, 18));
        assert_eq!(
            mac_light_colors,
            [[34, 178, 76, 255], [217, 130, 12, 255], [229, 46, 38, 255]]
        );

        let win_dark_colors = [500 * GIB, 750 * GIB, 900 * GIB]
            .map(|u| win_adapter.render_disk_gauge(space_for(u), dark_env))
            .map(|g| px_at(&g, 6, 23));
        assert_eq!(
            win_dark_colors,
            [[48, 209, 88, 255], [251, 191, 36, 255], [255, 69, 58, 255]]
        );

        let win_light_colors = [500 * GIB, 750 * GIB, 900 * GIB]
            .map(|u| win_adapter.render_disk_gauge(space_for(u), light_env))
            .map(|g| px_at(&g, 6, 23));
        assert_eq!(
            win_light_colors,
            [[34, 178, 76, 255], [217, 130, 12, 255], [229, 46, 38, 255]]
        );

        let win_light_50 =
            win_adapter.render_disk_gauge(space_for(500 * GIB), light_env);
        assert_eq!(px_at(&win_light_50, 3, 23), [24, 24, 28, 205]);
        assert_eq!(px_at(&win_light_50, 25, 23), [0, 0, 0, 38]);
        let win_light_glyphs = (3..13)
            .flat_map(|y| (1..31).map(move |x| (x, y)))
            .filter(|&(x, y)| px_at(&win_light_50, x, y) == [24, 24, 28, 255])
            .count();
        assert!(win_light_glyphs > 20);

        let linux_colors = [500 * GIB, 750 * GIB, 900 * GIB]
            .map(|u| linux_adapter.render_disk_gauge(space_for(u), dark_env))
            .map(|g| px_at(&g, 6, 23));
        assert_eq!(
            linux_colors,
            [
                [74, 222, 128, 255],
                [251, 191, 36, 255],
                [248, 113, 113, 255]
            ]
        );

        // Verify terabyte title formatting across adapters.
        let tb_space = SpaceInfo {
            total: 4 * TIB,
            free: TIB + TIB / 5,
            available: TIB + TIB / 5,
        };
        assert_eq!(
            mac_adapter
                .render_disk_gauge(tb_space, dark_env)
                .title
                .as_deref(),
            Some(" 1.2T")
        );
        assert_eq!(
            linux_adapter
                .render_disk_gauge(tb_space, dark_env)
                .title
                .as_deref(),
            Some("1.2T")
        );
        assert_eq!(
            win_adapter.render_disk_gauge(tb_space, dark_env).title,
            None
        );
    }

    #[test]
    fn surface_tone_classifiers_and_signatures_cover_all_three_adapters() {
        const GIB: u64 = 1024 * 1024 * 1024;

        assert_eq!(
            surface_tone_for_appearance_name("NSAppearanceNameDarkAqua"),
            TraySurfaceTone::Dark
        );
        assert_eq!(
            surface_tone_for_appearance_name("NSAppearanceNameVibrantDark"),
            TraySurfaceTone::Dark
        );
        assert_eq!(
            surface_tone_for_appearance_name(
                "NSAppearanceNameAccessibilityHighContrastDarkAqua"
            ),
            TraySurfaceTone::Dark
        );
        assert_eq!(
            surface_tone_for_appearance_name("NSAppearanceNameAqua"),
            TraySurfaceTone::Light
        );
        assert_eq!(
            surface_tone_for_appearance_name("NSAppearanceNameVibrantLight"),
            TraySurfaceTone::Light
        );

        assert_eq!(
            surface_tone_for_windows_light_theme(Some(true)),
            TraySurfaceTone::Light
        );
        assert_eq!(
            surface_tone_for_windows_light_theme(Some(false)),
            TraySurfaceTone::Dark
        );
        assert_eq!(
            surface_tone_for_windows_light_theme(None),
            TraySurfaceTone::Dark
        );

        let space = SpaceInfo {
            total: 1000 * GIB,
            free: 500 * GIB,
            available: 500 * GIB,
        };
        let dark_env = TrayGaugeEnv::dark();
        let light_env = TrayGaugeEnv::from_surface_tone(TraySurfaceTone::Light);

        let mac = TrayGaugeFactory::create_for(TrayGaugeOs::MacOs);
        let linux = TrayGaugeFactory::create_for(TrayGaugeOs::Linux);
        let win = TrayGaugeFactory::create_for(TrayGaugeOs::Windows);

        let mac_dark = mac.render_disk_gauge(space, dark_env);
        let mac_light = mac.render_disk_gauge(space, light_env);
        let border_idx = ((18 * 48 + 4) as usize) * 4;
        assert_eq!(
            &mac_dark.rgba[border_idx..border_idx + 4],
            &[248, 250, 252, 215]
        );
        assert_eq!(
            &mac_light.rgba[border_idx..border_idx + 4],
            &[24, 24, 28, 205]
        );
        let track_idx = ((18 * 48 + 38) as usize) * 4;
        assert_eq!(
            &mac_dark.rgba[track_idx..track_idx + 4],
            &[255, 255, 255, 48]
        );
        assert_eq!(&mac_light.rgba[track_idx..track_idx + 4], &[0, 0, 0, 38]);

        // macOS distinguishes Dark/Light for DiskGauge, normalizes MonochromeC1
        assert_ne!(
            mac.visual_signature(Some(space), true, dark_env),
            mac.visual_signature(Some(space), true, light_env)
        );
        assert_eq!(
            mac.visual_signature(Some(space), false, dark_env),
            mac.visual_signature(Some(space), false, light_env)
        );
        assert_ne!(
            mac.build_visual(Some(space), true, dark_env),
            mac.build_visual(Some(space), true, light_env)
        );

        // Windows adapts BOTH MonochromeC1 and DiskGauge to Dark/Light
        assert_ne!(
            win.visual_signature(Some(space), true, dark_env),
            win.visual_signature(Some(space), true, light_env)
        );
        assert_ne!(
            win.visual_signature(Some(space), false, dark_env),
            win.visual_signature(Some(space), false, light_env)
        );
        assert_ne!(
            win.build_visual(Some(space), false, dark_env),
            win.build_visual(Some(space), false, light_env)
        );
        assert_ne!(
            win.build_visual(Some(space), true, dark_env),
            win.build_visual(Some(space), true, light_env)
        );

        // Linux normalizes tone to Dark for both modes
        assert_eq!(
            linux.visual_signature(Some(space), true, dark_env),
            linux.visual_signature(Some(space), true, light_env)
        );
        assert_eq!(
            linux.visual_signature(Some(space), false, dark_env),
            linux.visual_signature(Some(space), false, light_env)
        );

        assert_eq!(mac.compact_label(space), None);
        assert_eq!(linux.compact_label(space).as_deref(), Some("500G"));
        assert_eq!(win.compact_label(space).as_deref(), Some("500G"));
        assert_eq!(mac.gauge_title(space).as_deref(), Some(" 500G"));
        assert_eq!(linux.gauge_title(space).as_deref(), Some("500G"));
        assert_eq!(win.gauge_title(space), None);
    }

    #[test]
    fn tray_visual_switch_off_restores_c1_icon_and_clears_title() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let space = SpaceInfo {
            total: 500 * GIB,
            free: 42 * GIB,
            available: 42 * GIB,
        };

        for os in [TrayGaugeOs::MacOs, TrayGaugeOs::Linux, TrayGaugeOs::Windows]
        {
            let adapter = TrayGaugeFactory::create_for(os);
            for env in [
                TrayGaugeEnv::dark(),
                TrayGaugeEnv::from_surface_tone(TraySurfaceTone::Light),
            ] {
                let on = adapter.build_visual(Some(space), true, env);
                assert!(matches!(on, TrayVisualRender::DiskGauge(_)));

                let off = adapter.build_visual(Some(space), false, env);
                assert_eq!(
                    off,
                    adapter.render_monochrome_c1(ICON_SIZE_PX, env)
                );
                match off {
                    TrayVisualRender::MonochromeC1 {
                        size,
                        icon_is_template,
                        ..
                    } => {
                        assert_eq!(size, ICON_SIZE_PX);
                        assert_eq!(icon_is_template, os == TrayGaugeOs::MacOs);
                    }
                    TrayVisualRender::DiskGauge(_) => {
                        panic!("expected MonochromeC1 fallback when disabled")
                    }
                }

                let sig_off = adapter.visual_signature(Some(space), false, env);
                assert_eq!(
                    sig_off,
                    TrayVisualSignature::MonochromeC1 {
                        os,
                        surface_tone: adapter.monochrome_signature_tone(env),
                    }
                );
                let sig_on = adapter.visual_signature(Some(space), true, env);
                assert_eq!(
                    sig_on,
                    adapter.visual_signature(Some(space), true, env)
                );
                assert_ne!(sig_on, sig_off);

                let sig_none = adapter.visual_signature(None, true, env);
                assert_eq!(sig_none, sig_off);
            }
        }
    }
}
