//! `ruum`: find what is eating a volume, mark it, and remove it.
//!
//! The window opens on a treemap of the scanned root — the home directory
//! unless another path is given — with a breadcrumb bar, a selection line, and a
//! live free-space meter. Marking is non-destructive until the review screen
//! is confirmed.

// A window, not a console program: on Windows, opening it from Explorer or
// the Start menu should not bring a console window along. `main` attaches to
// the console of a terminal it was started from, so `--help` and errors still
// reach one. Ignored elsewhere.
#![windows_subsystem = "windows"]

mod app_menu;
mod appearance;
pub use ruum_core::git;
mod i18n;
mod marks;
mod notifications;
mod palette;
mod power;
mod rail_pages;
mod state;
#[cfg(test)]
mod tests;
mod tray_menu;
mod treemap_view;
mod ui;
mod views;
mod watchdog_runtime;
mod widgets;

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, Sender, channel},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(target_os = "macos")]
use std::{
    io::IsTerminal as _,
    os::unix::process::CommandExt as _,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result};
use gpui_kit::{
    App, AppContext as _, FocusHandle, Global, IntoElement, Pixels, Render,
    Size, Window, WindowHandle, WindowOptions, px, size,
};
use ruum_core::confirm::{
    CleanupConfirmAdapter, CleanupConfirmDecision, CleanupConfirmRequest,
};
use ruum_core::scan::ScanOptions;
use state::{CleanupConfirmModal, Ruum, Screen};

/// What the command line asked for.
#[derive(Debug)]
struct Args {
    root: PathBuf,
    remote: Option<String>,
    remote_disk: bool,
    check: bool,
    choose_root: bool,
    options: ScanOptions,
    depth: u32,
    power: Option<power::PowerEfficiency>,
    initial_screen: Option<Screen>,
}

fn main() -> Result<()> {
    #[cfg(windows)]
    console::attach();
    let outcome = run();
    #[cfg(windows)]
    console::detach();
    outcome
}

fn run() -> Result<()> {
    if extract_scanner()? || run_mcp()? || run_watchdog()? {
        return Ok(());
    }
    if let Some(notify_args) =
        notifications::parse_notify_wait_args(std::env::args_os().skip(1))
    {
        notifications::run_notify_wait_command(&notify_args)?;
        return Ok(());
    }
    if let Some(dialog_args) =
        parse_confirm_dialog_args(std::env::args_os().skip(1))
    {
        return run_confirm_dialog(dialog_args);
    }
    let saved = power::settings_path()
        .map_or_else(
            || Ok(power::PowerEfficiency::default()),
            |path| power::load(&path),
        )
        .unwrap_or_else(|error| {
            eprintln!(
                "{}",
                i18n::messages::cannot_load_power(
                    &error.to_string(),
                    i18n::Locale::current(),
                )
            );
            power::PowerEfficiency::default()
        });
    let args = parse_args_with_power(std::env::args_os().skip(1), saved)?;

    if args.check {
        let host = args
            .remote
            .as_deref()
            .context("--check requires ruum ssh HOST")?;
        let scan = ruum_core::remote::RemoteScan::spawn(
            host,
            &ruum_core::remote::Request {
                root: (!args.root.as_os_str().is_empty())
                    .then(|| args.root.clone()),
                disk: args.remote_disk,
                options: args.options,
            },
        )?;
        loop {
            if let Some(result) = scan.poll() {
                let snapshot = result?;
                println!(
                    "{}",
                    i18n::messages::check_summary(
                        host,
                        &snapshot.header.root.display().to_string(),
                        snapshot.tree.bytes,
                        snapshot.tree.files,
                        snapshot.tree.dirs,
                        i18n::Locale::current(),
                    )
                );
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    // When the app executable is reached through the command-line symlink,
    // cmux sends SIGTERM to its foreground process group as AppKit takes
    // focus. Spawn once into a separate group before AppKit starts. Restrict
    // this to interactive cmux sessions so scripts retain normal foreground
    // lifetime; the marker prevents the child from spawning recursively.
    #[cfg(target_os = "macos")]
    if std::io::stdin().is_terminal()
        && std::env::var_os("CMUX_SURFACE_ID").is_some()
        && std::env::var_os("RUUM_CMUX_DETACHED").is_none()
    {
        Command::new(std::env::current_exe().context("find ruum")?)
            .args(std::env::args_os().skip(1))
            .env("RUUM_CMUX_DETACHED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Zero makes the child the leader of a new process group.
            .process_group(0)
            .spawn()
            .context("start ruum")?;
        return Ok(());
    }

    let depth = args.depth;
    let title_root = args.root.clone();
    let home_at_boot = std::env::home_dir();
    let native_look = appearance::follows_system(home_at_boot.as_deref());
    let launch = LaunchConfig {
        root: args.root,
        options: args.options.clone(),
        depth,
        remote: args.remote.clone(),
        remote_disk: args.remote_disk,
        choose_root: args.choose_root,
        power: args.power,
        native_look,
        initial_screen: args.initial_screen,
    };

    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    app.on_reopen(|cx| {
        open_or_navigate_main_window(None, cx);
    });
    app.run(move |cx| {
        gpui_omarchy::init(cx);
        install_notification_handler(cx);
        app_menu::install_for(i18n::Locale::current(), cx);
        if native_look {
            appearance::apply(cx.window_appearance(), cx);
        }

        let (ipc_server, ipc_rx) =
            match watchdog_runtime::WatchdogIpcServer::start() {
                Ok((srv, rx)) => (Some(srv), Some(rx)),
                Err(err) => {
                    eprintln!("ruum: watchdog IPC listener unavailable: {err}");
                    (None, None)
                }
            };
        let (in_proc_tx, in_proc_rx) = channel();
        let (in_proc_notif_tx, in_proc_notif_rx) = channel();
        cx.set_global(AppRuntime {
            launch,
            main_window: None,
            confirm_window: None,
            tray: None,
            ipc_rx,
            ipc_server,
            in_proc_confirm_tx: in_proc_tx,
            in_proc_confirm_rx: in_proc_rx,
            in_proc_notif_tx,
            in_proc_notif_rx,
            last_watchdog_poll: None,
            background_analysis_in_flight: false,
        });

        cx.on_action(|_: &app_menu::OpenExplore, cx| {
            open_or_navigate_main_window(Some(Screen::Explore), cx);
        });
        cx.on_action(|_: &app_menu::OpenSources, cx| {
            open_or_navigate_main_window(Some(Screen::Sources), cx);
        });
        cx.on_action(|_: &app_menu::OpenWatchdog, cx| {
            open_or_navigate_main_window(Some(Screen::Watchdog), cx);
        });
        cx.on_action(|_: &app_menu::OpenSettings, cx| {
            open_or_navigate_main_window(Some(Screen::Settings), cx);
        });
        cx.on_action(|_: &app_menu::ShowMainWindow, cx| {
            open_or_navigate_main_window(None, cx);
        });

        let window = open_main_window(cx, Some(title_root));
        with_runtime_mut(cx, |rt| rt.main_window = Some(window));

        // Install tray icon on the next GUI loop turn once platform status
        // bar integration is initialized. Failure keeps tray inactive so
        // closing the main window still quits normally.
        let tray_signal = Arc::new(AsyncSignal::default());
        let waker_signal = Arc::clone(&tray_signal);
        cx.defer(move |cx| {
            match tray_menu::TrayMenu::install(i18n::Locale::current()) {
                Ok(tray) => {
                    tray.set_event_waker(move || waker_signal.notify());
                    app_menu::set_tray_active(cx, true);
                    with_runtime_mut(cx, |rt| rt.tray = Some(tray));
                    refresh_tray_status(cx);
                }
                Err(err) => {
                    eprintln!("ruum: tray icon unavailable: {err}");
                    app_menu::set_tray_active(cx, false);
                }
            }
        });

        let tray_wait_signal = Arc::clone(&tray_signal);
        cx.spawn(async move |cx| {
            loop {
                tray_wait_signal.wait().await;
                let keep_going = cx.update(|cx| {
                    drain_tray_actions(cx);
                    !app_menu::is_quitting(cx)
                });
                if !keep_going {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let keep_going = cx.update(|cx| {
                    if app_menu::is_quitting(cx) {
                        return false;
                    }
                    poll_app_runtime(cx);
                    true
                });
                if !keep_going {
                    break;
                }
            }
        })
        .detach();

        cx.activate(true);
    });
    Ok(())
}

/// Standalone window presenting an AI or watchdog cleanup proposal.
#[derive(Debug)]
pub struct CleanupConfirmWindow {
    pub modal: CleanupConfirmModal,
    pub focus: FocusHandle,
    pub confirm_only: bool,
    pub handled: bool,
    pub locale: i18n::Locale,
}

impl CleanupConfirmWindow {
    pub fn new(
        request: CleanupConfirmRequest,
        responder: Option<Sender<CleanupConfirmDecision>>,
        response_file: Option<PathBuf>,
        confirm_only: bool,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) -> Self {
        Self {
            modal: CleanupConfirmModal::new(request, responder, response_file),
            focus: cx.focus_handle(),
            confirm_only,
            handled: false,
            locale: i18n::Locale::current(),
        }
    }

    #[cfg(test)]
    pub const fn set_locale(&mut self, locale: i18n::Locale) {
        self.locale = locale;
    }

    pub fn replace_request(
        &mut self,
        request: CleanupConfirmRequest,
        responder: Option<Sender<CleanupConfirmDecision>>,
        response_file: Option<PathBuf>,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) {
        self.ensure_cancelled();
        self.modal =
            CleanupConfirmModal::new(request, responder, response_file);
        self.handled = false;
        cx.notify();
    }

    pub fn ensure_cancelled(&mut self) {
        if !self.handled {
            self.handled = true;
            let _ = self.modal.cancel();
        }
    }

    pub fn cancel(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) {
        self.ensure_cancelled();
        let confirm_only = self.confirm_only;
        window.remove_window();
        if confirm_only {
            app_menu::request_quit(cx);
        }
    }

    pub fn confirm(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) {
        if self.handled || self.modal.selected_count() == 0 {
            return;
        }
        self.handled = true;
        // Read before calling `modal.confirm()`, which consumes `responder`.
        let is_in_app = self.modal.responder.is_none()
            && self.modal.response_file.is_none();
        let decision = self.modal.confirm();
        let confirm_only = self.confirm_only;
        if is_in_app
            && let CleanupConfirmDecision::Confirmed { selected_paths } =
                decision
        {
            let approved_items: Vec<ruum_core::watchdog_config::ProposalItem> =
                self.modal
                    .request
                    .items
                    .iter()
                    .filter(|i| selected_paths.contains(&i.path))
                    .map(|i| {
                        ruum_core::watchdog_config::ProposalItem::new(
                            i.path.clone(),
                            i.bytes,
                            i.mode,
                            i.reason.clone(),
                            i.git_warning.clone(),
                        )
                    })
                    .collect();
            if !approved_items.is_empty() {
                let root = self.modal.request.root.clone();
                let locale = self.locale;
                let task = cx.background_executor().spawn(async move {
                    ruum_core::watchdog::execute_confirmed_cleanup(
                        &approved_items,
                        &root,
                    )
                });
                cx.spawn(async move |_, cx| {
                    let (bytes, count) = task.await;
                    let summary = i18n::messages::cleaned_items_summary(
                        count, bytes, locale,
                    );
                    cx.update(|cx| {
                        if !cfg!(test) && count > 0 {
                            post_app_notification(
                                ruum_core::watchdog::NOTIF_TAG_CLEANUP_DONE,
                                i18n::messages::cleanup_complete_notif_title(
                                    locale,
                                ),
                                &summary,
                                cx,
                            );
                        }
                        refresh_tray_status(cx);
                        update_open_main_window(cx, |app, cx| {
                            app.reload_watchdog_state();
                            app.notice = Some((
                                i18n::messages::cleaned_items_summary(
                                    count, bytes, app.locale,
                                ),
                                gpui_omarchy::Status::Success,
                            ));
                            cx.notify();
                        });
                    });
                })
                .detach();
            }
        }
        window.remove_window();
        if confirm_only {
            app_menu::request_quit(cx);
        }
    }
}

impl Render for CleanupConfirmWindow {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) -> impl IntoElement {
        rail_pages::cleanup_confirm_view(
            &self.modal,
            &self.focus,
            self.locale,
            cx,
        )
    }
}

/// Open (or update and focus) the standalone foreground cleanup confirm window.
pub fn open_confirm_window(
    request: CleanupConfirmRequest,
    responder: Option<Sender<CleanupConfirmDecision>>,
    response_file: Option<PathBuf>,
    cx: &mut App,
) -> Option<WindowHandle<CleanupConfirmWindow>> {
    open_confirm_window_with_mode(request, responder, response_file, false, cx)
}

fn open_confirm_window_with_mode(
    request: CleanupConfirmRequest,
    responder: Option<Sender<CleanupConfirmDecision>>,
    response_file: Option<PathBuf>,
    confirm_only: bool,
    cx: &mut App,
) -> Option<WindowHandle<CleanupConfirmWindow>> {
    let windows = cx.windows();
    let existing = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.confirm_window)
        .filter(|h| windows.contains(&(*h).into()));

    let pending = std::rc::Rc::new(std::cell::RefCell::new(Some((
        request,
        responder,
        response_file,
    ))));

    if let Some(handle) = existing {
        let slot = std::rc::Rc::clone(&pending);
        let updated = handle.update(cx, move |win, window, cx| {
            if let Some((req, resp, file)) = slot.borrow_mut().take() {
                win.replace_request(req, resp, file, cx);
            }
            let focus = win.focus.clone();
            window.focus(&focus, cx);
            window.activate_window();
        });
        if updated.is_ok() {
            cx.activate(true);
            return Some(handle);
        }
    }

    let (request, responder, response_file) = pending.borrow_mut().take()?;

    let native_look = cx.try_global::<AppRuntime>().map_or_else(
        || {
            let home = std::env::home_dir();
            appearance::follows_system(home.as_deref())
        },
        |rt| rt.launch.native_look,
    );

    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(
                    gpui_kit::Bounds::new(
                        gpui_kit::point(px(200.), px(150.)),
                        size(px(760.), px(560.)),
                    ),
                )),
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some(
                        i18n::messages::title_confirm_cleanup(
                            i18n::Locale::current(),
                        )
                        .into(),
                    ),
                    ..Default::default()
                }),
                app_id: Some("ruum".to_owned()),
                window_min_size: Some(size(px(560.), px(380.))),
                ..Default::default()
            },
            move |window, cx| {
                if native_look {
                    appearance::follow(window);
                }
                let entity = cx.new(|cx| {
                    CleanupConfirmWindow::new(
                        request,
                        responder,
                        response_file,
                        confirm_only,
                        cx,
                    )
                });
                let close_entity = entity.downgrade();
                window.on_window_should_close(cx, move |_, cx| {
                    let _ = close_entity.update(cx, |win, _| {
                        win.ensure_cancelled();
                    });
                    if confirm_only {
                        app_menu::request_quit(cx);
                    }
                    true
                });
                entity
            },
        )
        .ok()?;

    let _ = window.update(cx, |win, window, cx| {
        let focus = win.focus.clone();
        window.focus(&focus, cx);
        window.activate_window();
    });
    cx.activate(true);

    with_runtime_mut(cx, |rt| rt.confirm_window = Some(window));
    Some(window)
}

pub(crate) fn with_runtime_mut(cx: &mut App, f: impl FnOnce(&mut AppRuntime)) {
    if cx.has_global::<AppRuntime>() {
        f(cx.global_mut::<AppRuntime>());
    }
}

/// Refresh tray menu status lines if a tray icon is active.
pub fn refresh_tray_status(cx: &mut App) {
    let Some(rt) = cx.try_global::<AppRuntime>() else {
        return;
    };
    if rt.tray.is_none() {
        return;
    }
    let home = std::env::home_dir();
    let free_bytes = home
        .as_deref()
        .and_then(|h| ruum_core::space::space_info(h).ok())
        .map(|s| s.available);
    let cfg = ruum_core::watchdog_config::watchdog_config_path()
        .and_then(|p| ruum_core::watchdog_config::load_config(&p).ok())
        .unwrap_or_default();
    let has_proposal = ruum_core::watchdog_config::watchdog_state_path()
        .and_then(|p| ruum_core::watchdog_config::load_state(&p).ok())
        .and_then(|s| s.last_proposal)
        .is_some_and(|p| !p.items.is_empty());

    if let Some(rt) = cx.try_global::<AppRuntime>()
        && let Some(tray) = &rt.tray
    {
        tray.update_status(free_bytes, cfg.enabled, has_proposal);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LaunchConfig {
    root: PathBuf,
    options: ScanOptions,
    depth: u32,
    remote: Option<String>,
    remote_disk: bool,
    choose_root: bool,
    power: Option<power::PowerEfficiency>,
    native_look: bool,
    initial_screen: Option<Screen>,
}

type ConfirmChannelPayload =
    (CleanupConfirmRequest, Sender<CleanupConfirmDecision>);
type NotificationChannelPayload = (String, String, String);

#[derive(Debug)]
pub(crate) struct AppRuntime {
    launch: LaunchConfig,
    pub(crate) main_window: Option<WindowHandle<Ruum>>,
    confirm_window: Option<WindowHandle<CleanupConfirmWindow>>,
    tray: Option<tray_menu::TrayMenu>,
    ipc_rx: Option<Receiver<watchdog_runtime::WatchdogIpcEvent>>,
    #[allow(
        dead_code,
        reason = "keeps background IPC listener alive across window closes"
    )]
    ipc_server: Option<watchdog_runtime::WatchdogIpcServer>,
    in_proc_confirm_tx: Sender<ConfirmChannelPayload>,
    in_proc_confirm_rx: Receiver<ConfirmChannelPayload>,
    in_proc_notif_tx: Sender<NotificationChannelPayload>,
    in_proc_notif_rx: Receiver<NotificationChannelPayload>,
    last_watchdog_poll: Option<Instant>,
    background_analysis_in_flight: bool,
}

impl Global for AppRuntime {}

#[cfg(test)]
pub(crate) fn install_test_runtime(
    root: PathBuf,
    choose_root: bool,
    main_window: Option<WindowHandle<Ruum>>,
    cx: &mut App,
) {
    let (in_proc_confirm_tx, in_proc_confirm_rx) = channel();
    let (in_proc_notif_tx, in_proc_notif_rx) = channel();
    cx.set_global(AppRuntime {
        launch: LaunchConfig {
            root,
            options: ScanOptions::default(),
            depth: 3,
            remote: None,
            remote_disk: false,
            choose_root,
            power: None,
            native_look: false,
            initial_screen: None,
        },
        main_window,
        confirm_window: None,
        tray: None,
        ipc_rx: None,
        ipc_server: None,
        in_proc_confirm_tx,
        in_proc_confirm_rx,
        in_proc_notif_tx,
        in_proc_notif_rx,
        last_watchdog_poll: None,
        background_analysis_in_flight: false,
    });
}

#[derive(Debug, Default)]
struct AsyncSignal {
    pending: AtomicBool,
    waker: Mutex<Option<std::task::Waker>>,
}

impl AsyncSignal {
    fn notify(&self) {
        self.pending.store(true, Ordering::Release);
        if let Ok(mut guard) = self.waker.lock()
            && let Some(waker) = guard.take()
        {
            waker.wake();
        }
    }

    async fn wait(&self) {
        std::future::poll_fn(|cx| {
            if self.pending.swap(false, Ordering::AcqRel) {
                std::task::Poll::Ready(())
            } else {
                if let Ok(mut guard) = self.waker.lock() {
                    *guard = Some(cx.waker().clone());
                }
                if self.pending.swap(false, Ordering::AcqRel) {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            }
        })
        .await;
    }
}

#[derive(Debug)]
struct InProcessConfirmAdapter {
    tx: Sender<ConfirmChannelPayload>,
}

impl CleanupConfirmAdapter for InProcessConfirmAdapter {
    fn confirm(
        &self,
        request: &CleanupConfirmRequest,
    ) -> std::io::Result<CleanupConfirmDecision> {
        let (resp_tx, resp_rx) = channel();
        self.tx.send((request.clone(), resp_tx)).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "UI confirmation channel closed",
            )
        })?;
        Ok(resp_rx.recv().unwrap_or(CleanupConfirmDecision::Cancelled))
    }
}

#[derive(Debug)]
struct InProcessNotificationAdapter {
    tx: Sender<NotificationChannelPayload>,
}

impl ruum_core::watchdog::NotificationAdapter for InProcessNotificationAdapter {
    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> std::io::Result<()> {
        self.tx
            .send((tag.to_owned(), title.to_owned(), body.to_owned()))
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "UI notification channel closed",
                )
            })
    }
}

/// Compute initial main window dimensions (60% of primary display, minimum
/// 800x600), falling back to a 1440x900 screen when no display is reported.
#[must_use]
pub(crate) fn initial_main_window_size(
    screen_size: Option<Size<Pixels>>,
) -> Size<Pixels> {
    let screen = screen_size.unwrap_or_else(|| size(px(1440.), px(900.)));
    let width = (screen.width.as_f32() * 0.6).round().max(800.0);
    let height = (screen.height.as_f32() * 0.6).round().max(600.0);
    size(px(width), px(height))
}

fn open_main_window(
    cx: &mut App,
    title_root_override: Option<PathBuf>,
) -> WindowHandle<Ruum> {
    let launch = cx
        .try_global::<AppRuntime>()
        .expect("AppRuntime installed before opening main window")
        .launch
        .clone();
    let home = std::env::home_dir();
    let title_root = title_root_override.unwrap_or_else(|| launch.root.clone());
    let native_look = launch.native_look;
    let win_size =
        initial_main_window_size(cx.primary_display().map(|d| d.bounds().size));

    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::centered(
                    win_size, cx,
                )),
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some(
                        if launch.choose_root {
                            i18n::messages::title_choose_source(
                                i18n::Locale::current(),
                            )
                            .to_owned()
                        } else {
                            format!(
                                "ruum · {}",
                                marks::display_path(
                                    &title_root,
                                    home.as_deref(),
                                )
                            )
                        }
                        .into(),
                    ),
                    ..Default::default()
                }),
                // Wayland app id. Hyprland reports it as the window class,
                // and the desktop entry's StartupWMClass and the documented
                // window rule both match `ruum`.
                app_id: Some("ruum".to_owned()),
                window_min_size: Some(size(px(800.), px(600.))),
                ..Default::default()
            },
            move |window, cx| {
                if native_look {
                    appearance::follow(window);
                }
                cx.new(|cx| {
                    let mut app = if launch.choose_root {
                        Ruum::choose(
                            launch.options,
                            launch.depth,
                            launch.remote,
                            cx,
                        )
                    } else {
                        Ruum::open(
                            launch.root,
                            launch.options,
                            launch.depth,
                            launch.remote,
                            launch.remote_disk,
                            cx,
                        )
                    };
                    app.power_choice = launch.power;
                    if let Some(screen) = launch.initial_screen {
                        app.navigate_rail(screen, cx);
                        if screen == Screen::Watchdog {
                            app.reload_watchdog_state();
                        }
                    }
                    app
                })
            },
        )
        .expect("open the ruum window");

    // The treemap owns the keyboard from the first frame; there is no
    // text field to focus first.
    let _ = window.update(cx, |this, window, cx| {
        let focus = this.focus.clone();
        window.focus(&focus, cx);
        window.activate_window();
    });
    window
}

pub(crate) fn install_notification_handler(cx: &App) {
    cx.set_app_identity(
        notifications::RUUM_APP_ID,
        notifications::RUUM_APP_NAME,
    );
    cx.on_system_notification_response(|response, cx| {
        handle_notification_response(&response, cx);
    });
}

#[must_use]
pub(crate) fn target_screen_for_notification_tag(tag: &str) -> Option<Screen> {
    match tag {
        ruum_core::watchdog::NOTIF_TAG_LOW_SPACE
        | ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS => Some(Screen::Watchdog),
        _ => None,
    }
}

pub(crate) fn handle_notification_response(
    response: &gpui_kit::SystemNotificationResponse,
    cx: &mut App,
) {
    activate_window_for_notification_tag(&response.tag, cx);
}

fn activate_window_for_notification_tag(tag: &str, cx: &mut App) {
    open_or_navigate_main_window(target_screen_for_notification_tag(tag), cx);
    update_open_main_window(cx, |app, cx| {
        app.reload_watchdog_state();
        cx.notify();
    });
    refresh_tray_status(cx);
}

pub(crate) fn post_app_notification(
    tag: &str,
    title: &str,
    body: &str,
    cx: &App,
) {
    if cfg!(test) || notifications::has_native_notification_identity() {
        cx.show_system_notification(gpui_kit::SystemNotification {
            tag: tag.to_owned().into(),
            title: title.to_owned().into(),
            body: body.to_owned().into(),
            actions: Vec::new(),
        });
        return;
    }
    let title_owned = title.to_owned();
    let body_owned = body.to_owned();
    cx.background_executor()
        .spawn(async move {
            let _ = ruum_core::watchdog::send_fallback_os_notification(
                &title_owned,
                &body_owned,
            );
        })
        .detach();
}

fn open_or_navigate_main_window(target: Option<Screen>, cx: &mut App) {
    let windows = cx.windows();
    let existing = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.main_window)
        .filter(|h| windows.contains(&(*h).into()));

    if let Some(handle) = existing {
        let _ = handle.update(cx, |app, window, cx| {
            if let Some(screen) = target {
                app.navigate_rail(screen, cx);
                if screen == Screen::Watchdog {
                    app.reload_watchdog_state();
                }
            }
            let focus = app.focus.clone();
            window.focus(&focus, cx);
            window.activate_window();
        });
        cx.activate(true);
        return;
    }

    let window = open_main_window(cx, None);
    if let Some(screen) = target {
        let _ = window.update(cx, |app, window, cx| {
            app.navigate_rail(screen, cx);
            if screen == Screen::Watchdog {
                app.reload_watchdog_state();
            }
            let focus = app.focus.clone();
            window.focus(&focus, cx);
            window.activate_window();
        });
    }
    with_runtime_mut(cx, |rt| rt.main_window = Some(window));
    cx.activate(true);
}

fn update_open_main_window<F>(cx: &mut App, f: F)
where
    F: FnOnce(&mut Ruum, &mut gpui_kit::Context<'_, Ruum>),
{
    let windows = cx.windows();
    let existing = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.main_window)
        .filter(|h| windows.contains(&(*h).into()));
    if let Some(handle) = existing {
        let _ = handle.update(cx, |app, _, cx| f(app, cx));
    }
}

fn drain_tray_actions(cx: &mut App) {
    let mut actions = Vec::new();
    if let Some(rt) = cx.try_global::<AppRuntime>()
        && let Some(tray) = &rt.tray
    {
        while let Some(action) = tray.poll_action() {
            actions.push(action);
        }
    }
    for action in actions {
        match action {
            tray_menu::TrayAction::OpenRuum => {
                open_or_navigate_main_window(None, cx);
            }
            tray_menu::TrayAction::OpenSources => {
                open_or_navigate_main_window(Some(Screen::Sources), cx);
            }
            tray_menu::TrayAction::OpenWatchdog => {
                open_or_navigate_main_window(Some(Screen::Watchdog), cx);
            }
            tray_menu::TrayAction::OpenSettings => {
                open_or_navigate_main_window(Some(Screen::Settings), cx);
            }
            tray_menu::TrayAction::CheckDiskSpaceNow => {
                trigger_tray_space_check(cx);
            }
            tray_menu::TrayAction::RunAiAnalysisNow => {
                trigger_background_ai_analysis(cx);
            }
            tray_menu::TrayAction::ReviewLatestProposal => {
                review_latest_proposal_from_tray(cx);
            }
            tray_menu::TrayAction::Quit => {
                app_menu::request_quit(cx);
            }
        }
    }
}

fn poll_app_runtime(cx: &mut App) {
    drain_tray_actions(cx);

    let mut ipc_events = Vec::new();
    let mut in_proc_requests = Vec::new();
    let mut in_proc_notifs = Vec::new();
    if let Some(rt) = cx.try_global::<AppRuntime>() {
        if let Some(rx) = &rt.ipc_rx {
            while let Ok(ev) = rx.try_recv() {
                ipc_events.push(ev);
            }
        }
        while let Ok(req) = rt.in_proc_confirm_rx.try_recv() {
            in_proc_requests.push(req);
        }
        while let Ok(notif) = rt.in_proc_notif_rx.try_recv() {
            in_proc_notifs.push(notif);
        }
    }

    for event in ipc_events {
        match event {
            watchdog_runtime::WatchdogIpcEvent::Confirm {
                request,
                responder,
            } => {
                open_confirm_window(request, Some(responder), None, cx);
            }
            watchdog_runtime::WatchdogIpcEvent::WatchdogUpdated => {
                update_open_main_window(cx, |app, cx| {
                    app.reload_watchdog_state();
                    cx.notify();
                });
                refresh_tray_status(cx);
            }
            watchdog_runtime::WatchdogIpcEvent::ShowNotification {
                tag,
                title,
                body,
            } => {
                post_app_notification(&tag, &title, &body, cx);
            }
            watchdog_runtime::WatchdogIpcEvent::OpenWindow { tag } => {
                activate_window_for_notification_tag(&tag, cx);
            }
        }
    }

    for (request, responder) in in_proc_requests {
        open_confirm_window(request, Some(responder), None, cx);
    }

    for (tag, title, body) in in_proc_notifs {
        post_app_notification(&tag, &title, &body, cx);
    }

    maybe_run_in_app_watchdog_tick(cx);
}

fn maybe_run_in_app_watchdog_tick(cx: &mut App) {
    let Some(rt) = cx.try_global::<AppRuntime>() else {
        return;
    };
    if rt.background_analysis_in_flight {
        return;
    }
    let cfg = ruum_core::watchdog_config::watchdog_config_path()
        .and_then(|p| ruum_core::watchdog_config::load_config(&p).ok())
        .unwrap_or_default()
        .normalized();
    if !cfg.enabled {
        return;
    }
    let interval = Duration::from_secs(cfg.poll_interval_secs);
    if let Some(last) = rt.last_watchdog_poll
        && last.elapsed() < interval
    {
        return;
    }
    let confirm_tx = rt.in_proc_confirm_tx.clone();
    let notif_tx = rt.in_proc_notif_tx.clone();
    with_runtime_mut(cx, |rt| {
        rt.last_watchdog_poll = Some(Instant::now());
        rt.background_analysis_in_flight = true;
    });

    let task = cx.background_executor().spawn(async move {
        let runner = ruum_core::ai_cli::DefaultCommandRunner;
        if matches!(
            ruum_core::watchdog::service_status(&runner),
            Ok(ruum_core::watchdog::ServiceStatus::Active)
        ) {
            return;
        }
        let Some(cfg_path) = ruum_core::watchdog_config::watchdog_config_path()
        else {
            return;
        };
        let Some(state_path) =
            ruum_core::watchdog_config::watchdog_state_path()
        else {
            return;
        };
        let Some(home) = std::env::home_dir() else {
            return;
        };
        let exe =
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ruum"));
        let now_secs = now_unix_secs();
        let adapter = InProcessConfirmAdapter { tx: confirm_tx };
        let notifier = InProcessNotificationAdapter { tx: notif_tx };
        let _ = ruum_core::watchdog::run_once(
            &cfg_path,
            &state_path,
            &home,
            &exe,
            now_secs,
            &runner,
            &adapter,
            &notifier,
        );
    });

    cx.spawn(async move |cx| {
        task.await;
        cx.update(|cx| {
            with_runtime_mut(cx, |rt| {
                rt.background_analysis_in_flight = false;
            });
            update_open_main_window(cx, |app, cx| {
                app.reload_watchdog_state();
                cx.notify();
            });
            refresh_tray_status(cx);
        });
    })
    .detach();
}

fn trigger_tray_space_check(cx: &App) {
    let task = cx.background_executor().spawn(async {
        let home = std::env::home_dir()?;
        let info = ruum_core::space::space_info(&home).ok()?;
        let now_secs = now_unix_secs();
        if let Some(path) = ruum_core::watchdog_config::watchdog_state_path() {
            let mut st = ruum_core::watchdog_config::load_state(&path)
                .unwrap_or_default();
            st.last_checked_secs = Some(now_secs);
            let _ = ruum_core::watchdog_config::save_state(&path, &st);
        }
        Some((info.available, now_secs))
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            if let Some((avail, checked_at)) = result {
                update_open_main_window(cx, |app, cx| {
                    app.reload_watchdog_state();
                    app.watchdog_space_checked_at = Some(checked_at);
                    let msg =
                        i18n::messages::disk_check_available(avail, app.locale);
                    app.push_toast(msg, gpui_omarchy::Status::Success, cx);
                });
            }
            refresh_tray_status(cx);
        });
    })
    .detach();
}

fn trigger_background_ai_analysis(cx: &mut App) {
    let windows = cx.windows();
    let main_open = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.main_window)
        .filter(|h| windows.contains(&(*h).into()));
    if let Some(handle) = main_open {
        let _ = handle.update(cx, |app, _, cx| {
            app.run_ai_analysis_now(cx);
        });
        return;
    }

    if let Some(rt) = cx.try_global::<AppRuntime>()
        && rt.background_analysis_in_flight
    {
        return;
    }
    with_runtime_mut(cx, |rt| rt.background_analysis_in_flight = true);

    let task = cx.background_executor().spawn(async {
        let home = std::env::home_dir().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "cannot find home directory",
            )
        })?;
        let cfg = ruum_core::watchdog_config::watchdog_config_path()
            .and_then(|p| ruum_core::watchdog_config::load_config(&p).ok())
            .unwrap_or_default();
        let exe =
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ruum"));
        let now_secs = now_unix_secs();
        let runner = ruum_core::ai_cli::DefaultCommandRunner;
        let proposal = ruum_core::ai_cli::run_analysis(
            &cfg, &home, &exe, now_secs, &runner,
        )
        .map_err(std::io::Error::other)?;
        if let Some(state_path) =
            ruum_core::watchdog_config::watchdog_state_path()
        {
            let mut st = ruum_core::watchdog_config::load_state(&state_path)
                .unwrap_or_default();
            st.last_proposal = Some(proposal.clone());
            st.last_triggered_secs = Some(now_secs);
            let _ = ruum_core::watchdog_config::save_state(&state_path, &st);
        }
        Ok::<_, std::io::Error>((home, proposal))
    });

    cx.spawn(async move |cx| {
        let res = task.await;
        cx.update(|cx| {
            with_runtime_mut(cx, |rt| {
                rt.background_analysis_in_flight = false;
            });
            update_open_main_window(cx, |app, cx| {
                app.reload_watchdog_state();
                cx.notify();
            });
            refresh_tray_status(cx);
            let locale = i18n::Locale::current();
            match res {
                Ok((home, proposal)) => {
                    if proposal.items.is_empty() {
                        post_app_notification(
                            ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                            i18n::messages::ai_analysis_notif_title(locale),
                            i18n::messages::ai_analysis_no_reclaimable(locale),
                            cx,
                        );
                    } else {
                        let request = state::proposal_confirm_request(
                            home, &proposal, locale,
                        );
                        open_confirm_window(request, None, None, cx);
                    }
                }
                Err(err) => {
                    post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                        i18n::messages::ai_analysis_failed_notif_title(locale),
                        &err.to_string(),
                        cx,
                    );
                }
            }
        });
    })
    .detach();
}

fn review_latest_proposal_from_tray(cx: &mut App) {
    let proposal = ruum_core::watchdog_config::watchdog_state_path()
        .and_then(|p| ruum_core::watchdog_config::load_state(&p).ok())
        .and_then(|s| s.last_proposal);
    let Some(prop) = proposal.filter(|p| !p.items.is_empty()) else {
        open_or_navigate_main_window(Some(Screen::Watchdog), cx);
        return;
    };
    // Fail closed when no user home directory exists rather than widening
    // cleanup root checks to `/`.
    let Some(home) = std::env::home_dir() else {
        open_or_navigate_main_window(Some(Screen::Watchdog), cx);
        return;
    };
    let request =
        state::proposal_confirm_request(home, &prop, i18n::Locale::current());
    open_confirm_window(request, None, None, cx);
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Extraction works without a display or GPUI initialization.
fn extract_scanner() -> Result<bool> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_none_or(|arg| arg != "extract-scanner") {
        return Ok(false);
    }
    anyhow::ensure!(args.len() == 3, "usage: ruum extract-scanner ARCH OUTPUT");
    let archive = ruum_core::payload::Archive::current()?
        .context("no embedded scanners; build with cargo xtask portable")?;
    let arch = args[1].to_str().context("ARCH must be x86_64 or aarch64")?;
    let (_, bytes) = archive.load(arch)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[2])?;
    std::io::Write::write_all(&mut output, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        output.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    println!(
        "{}",
        i18n::messages::extracted_scanner(
            arch,
            &PathBuf::from(&args[2]).display().to_string(),
            i18n::Locale::current(),
        )
    );
    Ok(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum McpCommand {
    NotMcp,
    Serve { read_only: bool },
    Help,
}

fn parse_mcp_command<I, S>(args: I) -> Result<McpCommand>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let mut iter = args.into_iter().map(Into::into);
    let Some(first) = iter.next() else {
        return Ok(McpCommand::NotMcp);
    };
    if first != "mcp" {
        return Ok(McpCommand::NotMcp);
    }
    let mut read_only = false;
    for arg in iter {
        if arg == "-h" || arg == "--help" {
            return Ok(McpCommand::Help);
        } else if arg == "--read-only" {
            read_only = true;
        } else {
            anyhow::bail!(
                "unknown option for `ruum mcp`; try `ruum mcp --help`"
            );
        }
    }
    Ok(McpCommand::Serve { read_only })
}

/// Serve the Model Context Protocol over stdio without initializing GPUI.
fn run_mcp() -> Result<bool> {
    match parse_mcp_command(std::env::args_os().skip(1))? {
        McpCommand::NotMcp => Ok(false),
        McpCommand::Help => {
            println!("{}", i18n::messages::mcp_help(i18n::Locale::current()));
            Ok(true)
        }
        McpCommand::Serve { read_only } => {
            if read_only {
                ruum_core::mcp::serve_stdio_read_only()?;
            } else {
                ruum_core::mcp::serve_stdio()?;
            }
            Ok(true)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchdogCommand {
    NotWatchdog,
    RunLoop,
    RunOnce,
    Help,
}

fn parse_watchdog_command<I, S>(args: I) -> Result<WatchdogCommand>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let mut iter = args.into_iter().map(Into::into);
    let Some(first) = iter.next() else {
        return Ok(WatchdogCommand::NotWatchdog);
    };
    if first != "watchdog" {
        return Ok(WatchdogCommand::NotWatchdog);
    }
    let mut once = false;
    for arg in iter {
        if arg == "-h" || arg == "--help" {
            return Ok(WatchdogCommand::Help);
        } else if arg == "--once" {
            once = true;
        } else {
            anyhow::bail!(
                "unknown option for `ruum watchdog`; try `ruum watchdog --help`"
            );
        }
    }
    if once {
        Ok(WatchdogCommand::RunOnce)
    } else {
        Ok(WatchdogCommand::RunLoop)
    }
}

/// Run volume monitoring and AI cleanup poller without initializing GPUI.
fn run_watchdog() -> Result<bool> {
    match parse_watchdog_command(std::env::args_os().skip(1))? {
        WatchdogCommand::NotWatchdog => Ok(false),
        WatchdogCommand::Help => {
            println!(
                "{}",
                i18n::messages::watchdog_help(i18n::Locale::current())
            );
            Ok(true)
        }
        WatchdogCommand::RunOnce => {
            let _ = ruum_core::watchdog::run_default_once_with_notifier(
                &notifications::DesktopNotificationAdapter {
                    detached_once: true,
                },
            )?;
            Ok(true)
        }
        WatchdogCommand::RunLoop => {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            ruum_core::watchdog::run_default_loop_with_notifier(
                &cancel,
                &notifications::DesktopNotificationAdapter {
                    detached_once: false,
                },
            )?;
            Ok(true)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ConfirmDialogArgs {
    req_path: PathBuf,
    resp_path: PathBuf,
}

fn parse_confirm_dialog_args<I, S>(args: I) -> Option<ConfirmDialogArgs>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let mut iter = args.into_iter().map(Into::into);
    let mut req_path = None;
    let mut resp_path = None;
    while let Some(arg) = iter.next() {
        if arg == "--confirm-cleanup" {
            req_path = iter.next().map(PathBuf::from);
        } else if arg == "--response" {
            resp_path = iter.next().map(PathBuf::from);
        }
    }
    match (req_path, resp_path) {
        (Some(r), Some(s)) => Some(ConfirmDialogArgs {
            req_path: r,
            resp_path: s,
        }),
        _ => None,
    }
}

fn run_confirm_dialog(args: ConfirmDialogArgs) -> Result<()> {
    let req_content = std::fs::read_to_string(&args.req_path)
        .context("read confirm request file")?;
    let request: CleanupConfirmRequest = serde_json::from_str(&req_content)
        .context("parse confirm request JSON")?;

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_omarchy::init(cx);
            app_menu::install_for(i18n::Locale::current(), cx);
            app_menu::set_confirm_only(cx, true);
            let home = std::env::home_dir();
            let native_look = appearance::follows_system(home.as_deref());
            if native_look {
                appearance::apply(cx.window_appearance(), cx);
            }
            let _ = open_confirm_window_with_mode(
                request,
                None,
                Some(args.resp_path),
                true,
                cx,
            );
        });
    Ok(())
}

/// Read the command line, program name already skipped.
#[cfg(test)]
fn parse_args(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Args> {
    parse_args_with_power(args, power::PowerEfficiency::default())
}

fn parse_open_screen(value: &str) -> Result<Screen> {
    match value {
        "explore" => Ok(Screen::Explore),
        "sources" => Ok(Screen::Sources),
        "watchdog" => Ok(Screen::Watchdog),
        "settings" => Ok(Screen::Settings),
        other => anyhow::bail!(
            "unknown screen {other}; try explore, sources, watchdog, or settings"
        ),
    }
}

fn parse_args_with_power(
    args: impl Iterator<Item = std::ffi::OsString>,
    preset: power::PowerEfficiency,
) -> Result<Args> {
    let mut root: Option<PathBuf> = None;
    let mut options = ScanOptions {
        threads: preset.policy(power::cpu_threads()),
        ..ScanOptions::default()
    };
    let mut power = Some(preset);
    let mut depth = 3_u32;
    let mut disk = false;
    let mut initial_screen = None;
    // `std::env::args` panics on a name that is not Unicode, and a path is
    // any name: a restart as administrator hands the root back exactly as
    // it was, so the caller passes `args_os`.
    let text = |value: Option<std::ffi::OsString>, need: &str| {
        value
            .and_then(|value| value.into_string().ok())
            .with_context(|| need.to_owned())
    };
    let mut check = false;
    let mut choose_root = false;
    let mut args = args.peekable();
    let remote = if args.peek().is_some_and(|arg| arg == "ssh") {
        args.next();
        let host = text(args.next(), "ssh needs a host, e.g. ruum ssh dev")?;
        ruum_core::remote::validate_host(&host)?;
        Some(host)
    } else {
        None
    };

    while let Some(arg) = args.next() {
        match arg.to_str().unwrap_or_default() {
            "-h" | "--help" => {
                println!("{}", i18n::messages::usage(i18n::Locale::current()));
                std::process::exit(0);
            }
            "-a" | "--apparent-size" => options.apparent_size = true,
            "-l" | "--follow-links" => options.follow_links = true,
            "-H" | "--no-hidden" => options.include_hidden = false,
            // Staying on one volume is the default; the flag is kept so
            // old invocations still work.
            "-x" | "--one-filesystem" => options.one_filesystem = true,
            "-X" | "--cross-filesystems" => options.one_filesystem = false,
            "-D" | "--disk" => disk = true,
            "--check" => check = true,
            "--choose" => choose_root = true,
            "--open-screen" => {
                let value = text(
                    args.next(),
                    "--open-screen needs a screen (explore, sources, watchdog, settings)",
                )?;
                initial_screen = Some(parse_open_screen(&value)?);
            }
            "-d" | "--depth" => {
                let value = text(args.next(), "--depth needs a number")?;
                depth = value.parse().context("--depth needs a number")?;
                anyhow::ensure!(
                    (1..=6).contains(&depth),
                    "--depth must be 1 to 6"
                );
            }
            "--power-efficiency" => {
                let value =
                    text(args.next(), "--power-efficiency needs a preset")?;
                let preset = power::PowerEfficiency::parse(&value)
                    .context("unknown Power Efficiency preset")?;
                options.threads = preset.policy(power::cpu_threads());
                power = Some(preset);
            }
            "--scan-threads" => {
                power = None;
                let value = text(args.next(), "--scan-threads needs a number")?;
                options.threads.max_threads = value
                    .parse()
                    .context("--scan-threads needs a positive integer")?;
                anyhow::ensure!(
                    options.threads.max_threads > 0,
                    "--scan-threads must be positive"
                );
            }
            "--fixed-threads" => {
                options.threads.adaptive = false;
                power = None;
            }
            "--adaptive-threads" => {
                options.threads.adaptive = true;
                options.threads.system_cpu_limit = Some(0.80);
                power = None;
            }
            "--thread-throughput-percent" => {
                let value = text(
                    args.next(),
                    "--thread-throughput-percent needs a number",
                )?;
                let percent: u8 = value
                    .parse()
                    .context("throughput percent must be 1 to 100")?;
                anyhow::ensure!(
                    (1..=100).contains(&percent),
                    "throughput percent must be 1 to 100"
                );
                options.threads.retained_throughput =
                    f64::from(percent) / 100.0;
            }
            "--thread-system-cpu-percent" => {
                let value = text(
                    args.next(),
                    "--thread-system-cpu-percent needs a number",
                )?;
                let percent: u8 =
                    value.parse().context("CPU percent must be 0 to 100")?;
                anyhow::ensure!(percent <= 100, "CPU percent must be 0 to 100");
                options.threads.system_cpu_limit =
                    (percent > 0).then(|| f64::from(percent) / 100.0);
            }
            "--metric" => {
                let value = text(args.next(), "--metric needs a value")?;
                options.metric = match value.as_str() {
                    "files" => ruum_core::tree::Metric::Files,
                    "bytes" | "size" => ruum_core::tree::Metric::Bytes,
                    other => anyhow::bail!(
                        "unknown metric {other}; try bytes or files"
                    ),
                };
            }
            // Launch Services added a process serial number when opening an
            // app from Finder until OS X 10.9, and some launchers still do.
            other if other.starts_with("-psn_") => {}
            other if other.starts_with('-') => {
                let usage = i18n::messages::usage(i18n::Locale::current());
                anyhow::bail!("unknown option {other}\n\n{usage}");
            }
            _ => {
                anyhow::ensure!(root.is_none(), "only one path can be scanned");
                root = Some(PathBuf::from(arg));
            }
        }
    }

    anyhow::ensure!(
        !(disk && root.is_some()),
        "--disk and a PATH cannot be combined"
    );
    let choose_root = if remote.is_some() {
        choose_root || (root.is_none() && !disk && !check)
    } else {
        choose_root && root.is_none() && !disk && !check
    };
    if remote.is_some() {
        return Ok(Args {
            root: root.unwrap_or_default(),
            remote,
            remote_disk: disk,
            check,
            choose_root,
            power,
            options,
            depth,
            initial_screen,
        });
    }
    let home = std::env::home_dir();
    let root = match root {
        _ if disk => home
            .as_deref()
            .and_then(ruum_core::space::volume_root_for)
            .unwrap_or_else(|| PathBuf::from("/")),
        Some(root) => root,
        None => home.context("no path given and no home directory")?,
    };
    // Store the depth as the initial view setting rather than a scan option: it
    // is a display choice the run-time `[` and `]` keys also change.
    // Canonical, so a later widening recognises this tree in the wider walk;
    // through dunce, so Windows gets `C:\Users\…` rather than the `\\?\C:\…`
    // form nothing else is written in.
    let root = if choose_root {
        root
    } else {
        let root = dunce::canonicalize(&root).unwrap_or(root);
        let metadata = std::fs::metadata(&root)
            .with_context(|| format!("cannot read {}", root.display()))?;
        anyhow::ensure!(
            metadata.is_dir(),
            "{} is not a directory",
            root.display()
        );
        root
    };

    Ok(Args {
        root,
        remote: None,
        remote_disk: false,
        check,
        choose_root,
        options,
        depth: depth.clamp(1, 6),
        power,
        initial_screen,
    })
}

/// The console of the terminal ruum was started from, if any: a
/// windowed program on Windows gets none of its own.
#[cfg(windows)]
mod console {
    #![allow(
        unsafe_code,
        reason = "two Win32 calls that take no pointers to get wrong"
    )]

    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, FreeConsole,
    };

    /// Borrow the parent's console so printed text reaches it. Does
    /// nothing when started from Explorer, which has none.
    pub fn attach() {
        // SAFETY: takes a process id by value, and failure only means
        // there was no console to attach to.
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }

    /// Let go of it again, so the shell redraws its prompt.
    pub fn detach() {
        // SAFETY: no arguments; a process without a console is left as is.
        unsafe {
            FreeConsole();
        }
    }
}

#[cfg(test)]
mod argument_tests {
    use super::*;

    fn arguments(extra: &[&str]) -> impl Iterator<Item = std::ffi::OsString> {
        extra
            .iter()
            .copied()
            .chain(std::iter::once("."))
            .map(std::ffi::OsString::from)
    }

    #[test]
    fn adaptive_budget_and_fixed_override_are_explicit() {
        let args = parse_args(arguments(&[])).expect("defaults");
        assert_eq!(
            args.options.threads.max_threads,
            4.min(power::cpu_threads())
        );
        assert!(!args.options.threads.adaptive);
        let args = parse_args(arguments(&[
            "--scan-threads",
            "4",
            "--fixed-threads",
            "--thread-throughput-percent",
            "85",
            "--thread-system-cpu-percent",
            "0",
        ]))
        .expect("custom");
        assert_eq!(args.options.threads.max_threads, 4);
        assert!(!args.options.threads.adaptive);
        assert!(
            (args.options.threads.retained_throughput - 0.85).abs()
                < f64::EPSILON
        );
        assert_eq!(args.options.threads.system_cpu_limit, None);
    }

    #[test]
    fn default_scans_home_and_choose_requires_selection() {
        let parse = |args: &[&str]| {
            parse_args(args.iter().map(std::ffi::OsString::from)).unwrap()
        };
        assert!(!parse(&[]).choose_root);
        assert!(parse(&["--choose"]).choose_root);
        assert!(parse(&["ssh", "dev"]).choose_root);
        assert!(!parse(&["."]).choose_root);
        assert!(!parse(&["--disk"]).choose_root);
        assert!(!parse(&["ssh", "dev", "/only-on-server"]).choose_root);
        assert!(!parse(&["ssh", "dev", "--check"]).choose_root);
    }

    #[test]
    fn ssh_keeps_worker_options_without_inspecting_the_remote_root() {
        let args = parse_args(
            [
                "ssh",
                "dev",
                "--check",
                "--scan-threads",
                "4",
                "/only-on-the-remote-machine",
            ]
            .into_iter()
            .map(std::ffi::OsString::from),
        )
        .expect("remote arguments");
        assert_eq!(args.remote.as_deref(), Some("dev"));
        assert!(args.check);
        assert_eq!(args.options.threads.max_threads, 4);
        assert_eq!(args.root, PathBuf::from("/only-on-the-remote-machine"));
        assert!(args.power.is_none());

        let args = parse_args(
            ["ssh", "dev", "--disk"]
                .into_iter()
                .map(std::ffi::OsString::from),
        )
        .expect("remote volume");
        assert!(args.remote_disk);
        assert!(args.root.as_os_str().is_empty());
        assert!(args.power.is_some());
    }

    #[test]
    fn saved_power_is_loaded_before_cli_overrides() {
        use power::PowerEfficiency as Power;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("power-efficiency");
        power::save(&path, Power::Miser).expect("save");
        let saved = power::load(&path).expect("load");
        let args = parse_args_with_power(arguments(&[]), saved).expect("saved");
        assert_eq!(args.power, Some(Power::Miser));
        assert_eq!(
            args.options.threads.max_threads,
            2.min(power::cpu_threads())
        );
        let args = parse_args_with_power(
            arguments(&["--power-efficiency", "drain-my-battery"]),
            saved,
        )
        .expect("override");
        assert_eq!(args.power, Some(Power::DrainMyBattery));
        assert_eq!(args.options.threads.max_threads, power::cpu_threads());
        let args = parse_args_with_power(
            arguments(&["--scan-threads", "8", "--adaptive-threads"]),
            saved,
        )
        .expect("experiment");
        assert!(args.power.is_none());
        assert!(args.options.threads.adaptive);
        assert_eq!(args.options.threads.max_threads, 8);
        assert_eq!(power::load(&path).expect("unchanged"), saved);
        assert!(
            parse_args(arguments(&["--power-efficiency", "invalid"])).is_err()
        );
    }

    // APFS rejects these filename bytes; Linux filesystems permit them.
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_options_preserve_a_non_unicode_root() {
        use std::os::unix::ffi::OsStringExt as _;

        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'r', 0xff]));
        std::fs::create_dir(&root).expect("directory");
        let args = parse_args(
            [
                std::ffi::OsString::from("--scan-threads"),
                std::ffi::OsString::from("4"),
                root.clone().into_os_string(),
            ]
            .into_iter(),
        )
        .expect("native path");
        assert_eq!(args.root, dunce::canonicalize(root).expect("root"));
        assert_eq!(args.options.threads.max_threads, 4);
    }

    #[test]
    fn invalid_worker_budgets_are_rejected() {
        for extra in [
            vec!["--scan-threads", "0"],
            vec!["--scan-threads", "-1"],
            vec!["--thread-throughput-percent", "0"],
            vec!["--thread-throughput-percent", "101"],
            vec!["--thread-system-cpu-percent", "101"],
        ] {
            assert!(parse_args(arguments(&extra)).is_err());
        }
    }

    #[test]
    fn mcp_subcommand_dispatches_and_rejects_unknown_flags() {
        assert_eq!(
            parse_mcp_command(Vec::<&str>::new()).expect("empty"),
            McpCommand::NotMcp
        );
        assert_eq!(
            parse_mcp_command(["ssh", "dev"]).expect("ssh"),
            McpCommand::NotMcp
        );
        assert_eq!(
            parse_mcp_command(["mcp"]).expect("mcp serve"),
            McpCommand::Serve { read_only: false }
        );
        assert_eq!(
            parse_mcp_command(["mcp", "--read-only"]).expect("mcp read-only"),
            McpCommand::Serve { read_only: true }
        );
        assert_eq!(
            parse_mcp_command(["mcp", "--help"]).expect("mcp --help"),
            McpCommand::Help
        );
        assert_eq!(
            parse_mcp_command(["mcp", "-h"]).expect("mcp -h"),
            McpCommand::Help
        );
        assert!(parse_mcp_command(["mcp", "--unknown"]).is_err());
        for locale in i18n::ALL_LOCALES {
            assert!(i18n::messages::usage(locale).contains("ruum mcp"));
        }
    }

    #[test]
    fn watchdog_subcommand_dispatches_and_rejects_unknown_flags() {
        assert_eq!(
            parse_watchdog_command(Vec::<&str>::new()).expect("empty"),
            WatchdogCommand::NotWatchdog
        );
        assert_eq!(
            parse_watchdog_command(["watchdog"]).expect("watchdog loop"),
            WatchdogCommand::RunLoop
        );
        assert_eq!(
            parse_watchdog_command(["watchdog", "--once"])
                .expect("watchdog once"),
            WatchdogCommand::RunOnce
        );
        assert_eq!(
            parse_watchdog_command(["watchdog", "--help"])
                .expect("watchdog help"),
            WatchdogCommand::Help
        );
        assert_eq!(
            parse_watchdog_command(["watchdog", "-h"]).expect("watchdog -h"),
            WatchdogCommand::Help
        );
        assert!(parse_watchdog_command(["watchdog", "--invalid"]).is_err());
        for locale in i18n::ALL_LOCALES {
            assert!(i18n::messages::usage(locale).contains("ruum watchdog"));
        }
    }

    #[test]
    fn confirm_cleanup_arguments_parse_and_require_both_paths() {
        assert_eq!(parse_confirm_dialog_args(Vec::<&str>::new()), None);
        assert_eq!(
            parse_confirm_dialog_args(["--confirm-cleanup", "req.json"]),
            None
        );
        assert_eq!(
            parse_confirm_dialog_args(["--response", "resp.json"]),
            None
        );
        assert_eq!(
            parse_confirm_dialog_args([
                "--confirm-cleanup",
                "req.json",
                "--response",
                "resp.json",
            ]),
            Some(ConfirmDialogArgs {
                req_path: PathBuf::from("req.json"),
                resp_path: PathBuf::from("resp.json"),
            })
        );
        for locale in i18n::ALL_LOCALES {
            assert!(
                i18n::messages::usage(locale).contains("--confirm-cleanup")
            );
        }
    }

    #[test]
    fn open_screen_parses_valid_screens_and_rejects_invalid() {
        assert_eq!(parse_args(arguments(&[])).unwrap().initial_screen, None);
        assert_eq!(
            parse_args(arguments(&["--open-screen", "explore"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Explore)
        );
        assert_eq!(
            parse_args(arguments(&["--open-screen", "sources"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Sources)
        );
        assert_eq!(
            parse_args(arguments(&["--open-screen", "watchdog"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Watchdog)
        );
        assert_eq!(
            parse_args(arguments(&["--open-screen", "settings"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Settings)
        );
        assert!(parse_args(arguments(&["--open-screen", "unknown"])).is_err());
        assert!(parse_args(arguments(&["--open-screen"])).is_err());
    }

    #[test]
    fn notification_tag_maps_to_expected_target_screen() {
        use ruum_core::watchdog::{
            NOTIF_TAG_AI_ANALYSIS, NOTIF_TAG_CLEANUP_DONE, NOTIF_TAG_LOW_SPACE,
        };

        assert_eq!(
            target_screen_for_notification_tag(NOTIF_TAG_LOW_SPACE),
            Some(Screen::Watchdog)
        );
        assert_eq!(
            target_screen_for_notification_tag(NOTIF_TAG_AI_ANALYSIS),
            Some(Screen::Watchdog)
        );
        assert_eq!(
            target_screen_for_notification_tag(NOTIF_TAG_CLEANUP_DONE),
            None
        );
        assert_eq!(target_screen_for_notification_tag("ruum.other"), None);
    }
}
