//! The menu bar and the application-wide shortcuts.
//!
//! GPUI adds neither on its own. On macOS an app without them has an empty
//! menu bar and no ⌘Q, so this is where ruum becomes a Mac app. GPUI
//! draws no menu bar on Linux or Windows, so there the same actions get the
//! shortcuts their users expect, with ctrl, and nothing else.

use crate::i18n::{Locale, common};
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
            /// Switch to the Docker view.
            OpenDocker,
            /// Switch to the Remote SSH servers view.
            OpenRemote,
            /// Switch to the Watchdog view.
            OpenWatchdog,
            /// Open the Settings page.
            OpenSettings,
            /// Install or update the `ruum` CLI command in PATH.
            InstallCliCommand,
            /// Check for application updates or open the update dialog.
            CheckForUpdates,
            /// Show or focus the main window.
            ShowMainWindow,
            /// Toggle fullscreen mode for the active main window.
            ToggleFullscreen,
        ]
    );
}

/// Application lifecycle flags governing tray-resident vs. quit-on-close
/// behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppLifecycle {
    /// Whether the OS system tray / menu-bar item is currently active.
    pub tray_active: bool,
    /// Whether closing the last window hides into the tray rather than
    /// terminating the process.
    pub close_to_background: bool,
    /// Whether the process was launched solely for `--confirm-cleanup`.
    pub confirm_only: bool,
    /// Whether an explicit quit was requested (`Quit` action or tray Quit).
    pub quitting: bool,
}

impl Default for AppLifecycle {
    fn default() -> Self {
        Self {
            tray_active: false,
            close_to_background: true,
            confirm_only: false,
            quitting: false,
        }
    }
}

impl Global for AppLifecycle {}

impl AppLifecycle {
    /// Return whether closing the last window should terminate the process.
    #[must_use]
    pub const fn should_quit_when_empty(self) -> bool {
        self.quitting
            || !self.tray_active
            || !self.close_to_background
            || self.confirm_only
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

/// Record whether closing all windows keeps Ruum running in the background.
pub fn set_close_to_background(cx: &mut App, close_to_background: bool) {
    update_lifecycle(cx, |lifecycle| {
        lifecycle.close_to_background = close_to_background;
    });
}

/// Record whether the process is running in standalone `--confirm-cleanup`
/// mode.
pub fn set_confirm_only(cx: &mut App, confirm_only: bool) {
    update_lifecycle(cx, |lifecycle| lifecycle.confirm_only = confirm_only);
}

/// Mark the application as explicitly quitting, cancel/clean up pending
/// confirm dialogs, release runtime handles, and exit GPUI's event loop.
pub fn request_quit(cx: &mut App) {
    if is_quitting(cx) {
        return;
    }
    update_lifecycle(cx, |lifecycle| lifecycle.quitting = true);
    crate::cancel_update_tasks_on_quit(cx);
    crate::cleanup_runtime_on_quit(cx);
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

/// Install the menus, shortcuts and tray-aware quit-on-close rule for `locale`.
pub fn install_for(locale: Locale, cx: &mut App) {
    if cx.try_global::<AppLifecycle>().is_none() {
        cx.set_global(AppLifecycle::default());
    }

    // Stay resident in the background when the tray icon is active and
    // `close_to_background` is enabled unless an explicit quit was requested
    // or we are in `--confirm-cleanup` mode.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            if should_quit_on_all_windows_closed(cx) {
                request_quit(cx);
            } else {
                crate::enter_background_mode(cx);
            }
        }
    })
    .detach();

    cx.on_action(|_: &Quit, cx| request_quit(cx));
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            if let Some(confirm) =
                window.downcast::<crate::CleanupConfirmWindow>()
            {
                let _ = confirm.update(cx, |this, window, cx| {
                    this.cancel(window, cx);
                });
            } else if let Some(remote) =
                window.downcast::<crate::RemoteConnectConfirmWindow>()
            {
                let _ = remote.update(cx, |this, window, cx| {
                    this.deny(window, cx);
                });
            } else if let Some(progress) =
                window.downcast::<crate::CleanupProgressWindow>()
            {
                let _ = progress.update(cx, |_, window, cx| {
                    crate::CleanupProgressWindow::continue_in_background(
                        window, cx,
                    );
                });
            } else {
                let _ =
                    window.update(cx, |_, window, _| window.remove_window());
            }
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
            KeyBinding::new("f11", ToggleFullscreen, None),
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
    cx.set_menus(build_menus(locale));
}

fn build_menus(locale: Locale) -> [Menu; 4] {
    [
        Menu::new("ruum").items([
            MenuItem::action(common::menu_settings(locale), OpenSettings),
            MenuItem::action(
                common::menu_install_cli(locale),
                InstallCliCommand,
            ),
            MenuItem::action(
                common::menu_check_updates(locale),
                CheckForUpdates,
            ),
            MenuItem::separator(),
            MenuItem::os_submenu(
                common::menu_services(locale),
                SystemMenuType::Services,
            ),
            MenuItem::separator(),
            MenuItem::action(common::menu_hide_app(locale), Hide),
            MenuItem::action(common::menu_hide_others(locale), HideOthers),
            MenuItem::action(common::menu_show_all(locale), ShowAll),
            MenuItem::separator(),
            MenuItem::action(common::menu_quit_app(locale), Quit),
        ]),
        Menu::new(common::menu_file(locale)).items([
            MenuItem::action(common::menu_open_folder(locale), OpenFolder),
            MenuItem::action(common::menu_show_in_finder(locale), ShowInFinder),
            MenuItem::separator(),
            MenuItem::action(common::menu_rescan(locale), Rescan),
            MenuItem::separator(),
            MenuItem::action(common::menu_close_window(locale), CloseWindow),
        ]),
        Menu::new(common::menu_view(locale)).items([
            MenuItem::action(common::menu_explore(locale), OpenExplore),
            MenuItem::action(common::menu_docker(locale), OpenDocker),
            MenuItem::action(common::menu_remote(locale), OpenRemote),
            MenuItem::action(common::menu_watchdog(locale), OpenWatchdog),
            MenuItem::action(common::menu_settings(locale), OpenSettings),
            MenuItem::separator(),
            MenuItem::action(
                common::menu_show_main_window(locale),
                ShowMainWindow,
            ),
        ]),
        Menu::new(common::menu_go(locale)).items([
            MenuItem::action(common::menu_back(locale), GoBack),
            MenuItem::action(common::menu_forward(locale), GoForward),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_quit_rules_cover_tray_confirm_and_quit_flags() {
        assert!(AppLifecycle::default().should_quit_when_empty());

        let tray_only = AppLifecycle {
            tray_active: true,
            close_to_background: true,
            confirm_only: false,
            quitting: false,
        };
        assert!(!tray_only.should_quit_when_empty());

        let tray_close_to_quit = AppLifecycle {
            tray_active: true,
            close_to_background: false,
            confirm_only: false,
            quitting: false,
        };
        assert!(tray_close_to_quit.should_quit_when_empty());

        let tray_with_explicit_quit = AppLifecycle {
            tray_active: true,
            close_to_background: true,
            confirm_only: false,
            quitting: true,
        };
        assert!(tray_with_explicit_quit.should_quit_when_empty());

        let tray_with_confirm_only = AppLifecycle {
            tray_active: true,
            close_to_background: true,
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

            set_close_to_background(cx, false);
            assert!(should_quit_on_all_windows_closed(cx));

            set_close_to_background(cx, true);
            assert!(!should_quit_on_all_windows_closed(cx));

            set_confirm_only(cx, true);
            assert!(should_quit_on_all_windows_closed(cx));

            set_confirm_only(cx, false);
            assert!(!should_quit_on_all_windows_closed(cx));
        });
    }

    #[gpui_kit::test]
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "gpui_kit::test passes &mut TestAppContext"
    )]
    fn install_for_all_locales_populates_localized_menus(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
                install_for(locale, cx);
                let menus = build_menus(locale);
                assert_eq!(menus.len(), 4);
                assert!(!common::menu_file(locale).is_empty());
                assert!(!common::menu_view(locale).is_empty());
                assert!(!common::menu_go(locale).is_empty());
                assert!(!common::menu_check_updates(locale).is_empty());
            }
            assert_ne!(
                common::menu_check_updates(Locale::Zh),
                common::menu_check_updates(Locale::En)
            );
            assert_ne!(
                common::menu_check_updates(Locale::Ja),
                common::menu_check_updates(Locale::En)
            );
            assert_ne!(
                common::menu_check_updates(Locale::Ko),
                common::menu_check_updates(Locale::En)
            );
            assert_ne!(
                common::menu_file(Locale::Zh),
                common::menu_file(Locale::En)
            );
            assert_ne!(
                common::menu_file(Locale::Ja),
                common::menu_file(Locale::En)
            );
            assert_ne!(
                common::menu_file(Locale::Ko),
                common::menu_file(Locale::En)
            );
        });
    }
}
