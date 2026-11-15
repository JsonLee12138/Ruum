//! The menu bar and the application-wide shortcuts.
//!
//! GPUI adds neither on its own. On macOS an app without them has an empty
//! menu bar and no ⌘Q, so this is where ruum becomes a Mac app. GPUI
//! draws no menu bar on Linux or Windows, so there the same actions get the
//! shortcuts their users expect, with ctrl, and nothing else.

use gpui_kit::{App, Global, KeyBinding, Menu, MenuItem, SystemMenuType};

pub use menu_actions::*;

#[allow(
    clippy::derive_partial_eq_without_eq,
    reason = "GPUI's actions! macro writes the derives, not us"
)]
mod menu_actions {
    use gpui_kit::actions;

    actions!(
        ruum,
        [
            /// Quit the application.
            Quit,
            /// Hide the application's windows.
            Hide,
            /// Hide every other application.
            HideOthers,
            /// Show every application again.
            ShowAll,
            /// Close the focused window.
            CloseWindow,
            /// Scan the current root again.
            Rescan,
            /// Choose a directory to scan.
            OpenFolder,
            /// Reveal the tile a key acts on in Finder.
            ShowInFinder,
            /// Back through the directories visited.
            GoBack,
            /// Forward again, after going back.
            GoForward,
            /// Switch to the Explore view.
            OpenExplore,
            /// Switch to the Sources & Drives view.
            OpenSources,
            /// Switch to the Watchdog view.
            OpenWatchdog,
            /// Open the Settings page.
            OpenSettings,
            /// Show or focus the main window.
            ShowMainWindow,
        ]
    );
}

/// Application lifecycle flags governing tray-resident vs. quit-on-close
/// behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppLifecycle {
    /// Whether the OS system tray / menu-bar item is currently active.
    pub tray_active: bool,
    /// Whether the process was launched solely for `--confirm-cleanup`.
    pub confirm_only: bool,
    /// Whether an explicit quit was requested (`Quit` action or tray Quit).
    pub quitting: bool,
}

impl Global for AppLifecycle {}

impl AppLifecycle {
    /// Return whether closing the last window should terminate the process.
    #[must_use]
    pub const fn should_quit_when_empty(self) -> bool {
        self.quitting || !self.tray_active || self.confirm_only
    }
}

fn update_lifecycle(cx: &mut App, update: impl FnOnce(&mut AppLifecycle)) {
    let mut lifecycle =
        cx.try_global::<AppLifecycle>().copied().unwrap_or_default();
    update(&mut lifecycle);
    cx.set_global(lifecycle);
}

/// Record whether the system tray icon is active in the current session.
pub fn set_tray_active(cx: &mut App, active: bool) {
    update_lifecycle(cx, |lifecycle| lifecycle.tray_active = active);
}

/// Record whether the process is running in standalone `--confirm-cleanup`
/// mode.
pub fn set_confirm_only(cx: &mut App, confirm_only: bool) {
    update_lifecycle(cx, |lifecycle| lifecycle.confirm_only = confirm_only);
}

/// Mark the application as explicitly quitting and exit GPUI's event loop.
pub fn request_quit(cx: &mut App) {
    update_lifecycle(cx, |lifecycle| lifecycle.quitting = true);
    cx.quit();
}

/// Return whether an explicit quit has been requested.
#[must_use]
pub fn is_quitting(cx: &App) -> bool {
    cx.try_global::<AppLifecycle>()
        .is_some_and(|lifecycle| lifecycle.quitting)
}

/// Return whether closing all open windows should quit the application.
#[must_use]
pub fn should_quit_on_all_windows_closed(cx: &App) -> bool {
    cx.try_global::<AppLifecycle>()
        .is_none_or(|lifecycle| lifecycle.should_quit_when_empty())
}

/// Install the menus, the shortcuts and the tray-aware quit-on-close rule.
pub fn install(cx: &mut App) {
    if cx.try_global::<AppLifecycle>().is_none() {
        cx.set_global(AppLifecycle::default());
    }

    // Stay resident in the background when the tray icon is active unless an
    // explicit quit was requested or we are in `--confirm-cleanup` mode.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() && should_quit_on_all_windows_closed(cx) {
            cx.quit();
        }
    })
    .detach();

    cx.on_action(|_: &Quit, cx| request_quit(cx));
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    });
    // `Rescan`, `OpenFolder`, `ShowInFinder` and the history are handled by
    // the window, which owns the scan and the selection; see `views::root`.
    if !cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("ctrl-w", CloseWindow, None),
            KeyBinding::new("ctrl-o", OpenFolder, None),
            KeyBinding::new("ctrl-r", Rescan, None),
            KeyBinding::new("f5", Rescan, None),
            KeyBinding::new("ctrl-shift-r", ShowInFinder, None),
            KeyBinding::new("ctrl-,", OpenSettings, None),
        ]);
        return;
    }
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-r", Rescan, None),
        KeyBinding::new("cmd-o", OpenFolder, None),
        // ⌘R is taken by Rescan, as in a browser; Xcode uses ⌘⇧R-like
        // chords for its reveals, and `o` does the same without a modifier.
        KeyBinding::new("cmd-shift-r", ShowInFinder, None),
        // Finder's and Safari's back and forward; alt-arrows work too.
        KeyBinding::new("cmd-[", GoBack, None),
        KeyBinding::new("cmd-]", GoForward, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
    ]);
    cx.set_menus([
        Menu::new("ruum").items([
            MenuItem::action("Settings\u{2026}", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide ruum", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit ruum", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open Folder\u{2026}", OpenFolder),
            MenuItem::action("Show in Finder", ShowInFinder),
            MenuItem::separator(),
            MenuItem::action("Rescan", Rescan),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("View").items([
            MenuItem::action("Explore", OpenExplore),
            MenuItem::action("Sources & Drives", OpenSources),
            MenuItem::action("Watchdog", OpenWatchdog),
            MenuItem::action("Settings\u{2026}", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Show Main Window", ShowMainWindow),
        ]),
        Menu::new("Go").items([
            MenuItem::action("Back", GoBack),
            MenuItem::action("Forward", GoForward),
        ]),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_quit_rules_cover_tray_confirm_and_quit_flags() {
        assert!(AppLifecycle::default().should_quit_when_empty());

        let tray_only = AppLifecycle {
            tray_active: true,
            confirm_only: false,
            quitting: false,
        };
        assert!(!tray_only.should_quit_when_empty());

        let tray_with_explicit_quit = AppLifecycle {
            tray_active: true,
            confirm_only: false,
            quitting: true,
        };
        assert!(tray_with_explicit_quit.should_quit_when_empty());

        let tray_with_confirm_only = AppLifecycle {
            tray_active: true,
            confirm_only: true,
            quitting: false,
        };
        assert!(tray_with_confirm_only.should_quit_when_empty());
    }

    #[gpui_kit::test]
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "gpui_kit::test passes &mut TestAppContext"
    )]
    fn lifecycle_globals_update_and_query_cleanly(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            assert!(should_quit_on_all_windows_closed(cx));
            assert!(!is_quitting(cx));

            set_tray_active(cx, true);
            assert!(!should_quit_on_all_windows_closed(cx));

            set_confirm_only(cx, true);
            assert!(should_quit_on_all_windows_closed(cx));

            set_confirm_only(cx, false);
            assert!(!should_quit_on_all_windows_closed(cx));
        });
    }
}
