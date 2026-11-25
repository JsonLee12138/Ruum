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
mod background;
pub use ruum_core::git;
mod i18n;
mod marks;
mod notifications;
mod palette;
mod power;
mod qr;
mod rail_pages;
mod state;
#[cfg(test)]
mod tests;
mod tray_gauge;
mod tray_menu;
mod treemap_view;
mod ui;
mod views;
mod watchdog_runtime;
mod widgets;
mod window_chrome;

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
    App, AppContext as _, AssetSource, FocusHandle, Global, IntoElement,
    Pixels, Render, SharedString, Size, WeakEntity, Window, WindowHandle,
    WindowOptions, px, size,
};
use ruum_core::confirm::{
    CleanupConfirmAdapter, CleanupConfirmDecision, CleanupConfirmRequest,
    RemoteConnectConfirmDecision, RemoteConnectConfirmRequest,
};
use ruum_core::docker_adapter::{DockerEnv, DockerFactory, DockerProbeStatus};
use ruum_core::export::{
    RemoteCleanupScriptSpec, build_remote_cleanup_script,
    write_cleanup_script_atomic,
};
use ruum_core::removal::Target;
use ruum_core::scan::ScanOptions;
use ruum_core::storage::StorageDb;
use state::{CleanupConfirmModal, RemoteConnectConfirmModal, Ruum, Screen};

/// Resolve the sibling `ruum` CLI binary so prompts and MCP commands point to
/// `ruum` rather than `ruum-gui`.
pub(crate) fn resolve_cli_exe() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| ruum_core::cli_install::resolve_cli_binary(&exe).ok())
        .unwrap_or_else(|| PathBuf::from("ruum"))
}

gpui_kit::assets::icon_assets!(
    ExtraIconAssets,
    [Container, Download, RefreshCw]
);

struct RuumAssets;

impl AssetSource for RuumAssets {
    fn load(
        &self,
        path: &str,
    ) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIconAssets.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut items = ExtraIconAssets.list(path)?;
        items.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(items)
    }
}

/// What the GUI command line asked for.
#[derive(Debug)]
pub(crate) struct GuiArgs {
    pub(crate) root: PathBuf,
    options: ScanOptions,
    depth: u32,
    power: Option<power::PowerEfficiency>,
    initial_screen: Option<Screen>,
    background: bool,
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
    if let Some(internal) =
        parse_internal_gui_command(std::env::args_os().skip(1))?
    {
        return match internal {
            InternalGuiCommand::ConfirmCleanup(dialog_args) => {
                run_confirm_dialog(dialog_args)
            }
            InternalGuiCommand::ConfirmRemoteConnect(dialog_args) => {
                run_remote_connect_confirm_dialog(dialog_args)
            }
            InternalGuiCommand::ShowNotification(notif_args) => {
                run_show_notification(notif_args);
                Ok(())
            }
        };
    }

    let db_opt = StorageDb::open_default().ok();
    let app_settings = db_opt
        .as_ref()
        .and_then(|db| db.load_app_settings().ok())
        .unwrap_or_default();
    let saved = power::PowerEfficiency::parse(&app_settings.power_preset)
        .unwrap_or_default();
    let mut args =
        parse_gui_args_with_power(std::env::args_os().skip(1), saved)?;
    args.depth = app_settings.default_depth.clamp(1, 6);
    args.options.apparent_size = app_settings.apparent_size;
    args.options.include_hidden = app_settings.include_hidden;

    // Single-instance guard: probe an active local GUI over IPC before
    // opening any window or detaching.
    if args.background {
        if ruum_core::notify::try_ipc_ping() {
            return Ok(());
        }
    } else {
        let wake_tag =
            args.initial_screen.map(screen_open_tag).unwrap_or_default();
        if ruum_core::notify::try_ipc_open_window(wake_tag) {
            return Ok(());
        }
    }

    // When the app executable is reached through a terminal launcher,
    // cmux sends SIGTERM to its foreground process group as AppKit takes
    // focus. Spawn once into a separate group before AppKit starts. Restrict
    // this to interactive cmux sessions so scripts retain normal foreground
    // lifetime; the marker prevents the child from spawning recursively.
    #[cfg(target_os = "macos")]
    if std::io::stdin().is_terminal()
        && std::env::var_os("CMUX_SURFACE_ID").is_some()
        && std::env::var_os("RUUM_CMUX_DETACHED").is_none()
    {
        Command::new(std::env::current_exe().context("find ruum-gui")?)
            .args(std::env::args_os().skip(1))
            .env("RUUM_CMUX_DETACHED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Zero makes the child the leader of a new process group.
            .process_group(0)
            .spawn()
            .context("start ruum-gui")?;
        return Ok(());
    }

    let start_background_flag = args.background;
    let depth = args.depth;
    let title_root = args.root.clone();
    let home_at_boot = std::env::home_dir();
    let native_look = if db_opt.is_some() {
        app_settings.follow_system_appearance
    } else {
        appearance::follows_system(home_at_boot.as_deref())
    };
    let close_to_background = app_settings.close_to_background;
    let tray_show_disk_space = app_settings.tray_show_disk_space;
    let launch = LaunchConfig {
        root: args.root,
        options: args.options.clone(),
        depth,
        remote: None,
        remote_disk: false,
        choose_root: false,
        power: args.power,
        native_look,
        initial_screen: args.initial_screen,
    };

    let app = gpui_kit::application().with_assets(RuumAssets);
    app.on_reopen(|cx| {
        enter_foreground_mode(cx);
        open_or_navigate_main_window(None, cx);
    });
    app.run(move |cx| {
        gpui_omarchy::init(cx);
        install_notification_handler(cx);
        app_menu::install_for(i18n::Locale::current(), cx);
        app_menu::set_close_to_background(cx, close_to_background);
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
        let initial_watchdog_config = StorageDb::open_default()
            .ok()
            .and_then(|db| db.watchdog().ok())
            .and_then(|svc| svc.load_config().ok())
            .unwrap_or_default();
        let initial_update_settings = StorageDb::open_default()
            .ok()
            .and_then(|db| db.load_update_settings().ok())
            .unwrap_or_default();
        let initial_install_kind = ruum_core::updater::UpdateFactory::create()
            .detect_install_kind(&ruum_core::updater::UpdateEnv::current());
        let update_quitting = Arc::new(AtomicBool::new(false));
        let shared_cred_store =
            ruum_core::credentials::LazyCredentialStore::open_default().ok();
        cx.set_global(AppRuntime {
            launch,
            cred_store: shared_cred_store.clone(),
            main_window: None,
            confirm_window: None,
            cleanup_progress_window: None,
            remote_connect_window: None,
            active_cleanup: None,
            tray: None,
            ipc_rx,
            ipc_server,
            in_proc_confirm_tx: in_proc_tx,
            in_proc_confirm_rx: in_proc_rx,
            in_proc_notif_tx,
            in_proc_notif_rx,
            last_watchdog_poll: None,
            tray_show_disk_space,
            last_tray_space_refresh: None,
            background_analysis_in_flight: false,
            watchdog_config: initial_watchdog_config,
            update_state: state::UpdateUiState::Idle,
            update_settings: initial_update_settings,
            update_install_kind: initial_install_kind,
            update_banner_dismissed_version: None,
            update_download_cancel: None,
            update_last_error: None,
            update_quitting: Arc::clone(&update_quitting),
            #[cfg(test)]
            test_cleanup_gate: None,
        });
        if !cfg!(test)
            && let Some(store) = shared_cred_store
        {
            let db_path = ruum_watchdog::ruum_db_path();
            cx.background_executor()
                .spawn(async move {
                    let _ = preload_credentials_if_needed(
                        db_path.as_deref(),
                        &store,
                    );
                })
                .detach();
        }

        cx.on_action(|_: &app_menu::OpenExplore, cx| {
            open_or_navigate_main_window(Some(Screen::Explore), cx);
        });
        cx.on_action(|_: &app_menu::OpenDocker, cx| {
            open_or_navigate_main_window(Some(Screen::Docker), cx);
        });
        cx.on_action(|_: &app_menu::OpenRemote, cx| {
            open_or_navigate_main_window(Some(Screen::Remote), cx);
        });
        cx.on_action(|_: &app_menu::OpenWatchdog, cx| {
            open_or_navigate_main_window(Some(Screen::Watchdog), cx);
        });
        cx.on_action(|_: &app_menu::OpenSettings, cx| {
            open_or_navigate_main_window(Some(Screen::Settings), cx);
        });
        cx.on_action(|_: &app_menu::InstallCliCommand, cx| {
            open_or_navigate_main_window(Some(Screen::Settings), cx);
            update_open_main_window(cx, |app, cx| {
                app.install_or_update_cli(cx);
            });
        });
        cx.on_action(|_: &app_menu::CheckForUpdates, cx| {
            trigger_check_for_updates_from_menu_or_tray(cx);
        });
        cx.on_action(|_: &app_menu::ShowMainWindow, cx| {
            open_or_navigate_main_window(None, cx);
        });

        let bg_adapter = background::BackgroundPresentationFactory::create();
        let start_in_background =
            start_background_flag || bg_adapter.launched_as_login_item();
        let initial_window = if start_in_background {
            bg_adapter.enter_background(cx);
            None
        } else {
            let window = open_main_window(cx, Some(title_root));
            with_runtime_mut(cx, |rt| rt.main_window = Some(window));
            bg_adapter.enter_foreground(cx);
            Some(window)
        };
        if !cfg!(test) {
            spawn_startup_update_and_sync(
                initial_window,
                initial_install_kind,
                update_quitting,
                cx,
            );
        }

        // Install tray icon on the next GUI loop turn once platform status
        // bar integration is initialized. Failure keeps tray inactive so
        // closing the main window (or failing in `--background` mode) exits
        // cleanly without leaving a zombie process.
        let tray_signal = Arc::new(AsyncSignal::default());
        let waker_signal = Arc::clone(&tray_signal);
        cx.defer(move |cx| {
            match tray_menu::TrayMenu::install(i18n::Locale::current()) {
                Ok(tray) => {
                    tray.set_event_waker(move || waker_signal.notify());
                    app_menu::set_tray_active(cx, true);
                    with_runtime_mut(cx, |rt| rt.tray = Some(tray));
                    refresh_tray_status(cx);
                    refresh_tray_update_status(cx);
                }
                Err(err) => {
                    eprintln!("ruum: tray icon unavailable: {err}");
                    app_menu::set_tray_active(cx, false);
                    if cx.windows().is_empty()
                        && app_menu::should_quit_on_all_windows_closed(cx)
                    {
                        app_menu::request_quit(cx);
                    }
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
    pub db_path: Option<PathBuf>,
    pub remote_session: Option<Arc<ruum_core::ssh_session::SshSession>>,
    pub remote_header: Option<ruum_core::remote::Header>,
    pub docker: DockerProbeStatus,
}

impl CleanupConfirmWindow {
    /// Close request from the client-side titlebar: must go through
    /// `cancel` so the waiting requester is answered; only a dead entity
    /// falls back to removing the window outright.
    pub fn close_from_chrome(
        entity: &WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if entity
            .update(cx, |this, cx| this.cancel(window, cx))
            .is_err()
        {
            window.remove_window();
        }
    }

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
            db_path: None,
            remote_session: None,
            remote_header: None,
            docker: DockerProbeStatus::default(),
        }
    }

    pub fn set_target_context(
        &mut self,
        db_path: Option<PathBuf>,
        remote_profile_id: Option<String>,
        remote_session: Option<Arc<ruum_core::ssh_session::SshSession>>,
        remote_header: Option<ruum_core::remote::Header>,
        docker: DockerProbeStatus,
    ) {
        self.db_path = db_path;
        self.modal.remote_profile_id = remote_profile_id;
        self.remote_session = remote_session;
        self.remote_header = remote_header;
        self.docker = docker;
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
        self.db_path = None;
        self.remote_session = None;
        self.remote_header = None;
        self.docker = DockerProbeStatus::default();
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
        // Read before calling `modal.confirm()`, which consumes `responder`.
        let is_in_app = self.modal.responder.is_none()
            && self.modal.response_file.is_none();
        if is_in_app
            && self.modal.remote_profile_id.is_none()
            && cx
                .try_global::<AppRuntime>()
                .is_some_and(|rt| rt.active_cleanup.is_some())
        {
            open_cleanup_progress_window(cx);
            return;
        }
        self.handled = true;
        let selected_bytes = self.modal.selected_bytes();
        let decision = self.modal.confirm();
        let confirm_only = self.confirm_only;
        let locale = self.locale;
        let mut sync_fallback_notif: Option<(String, String)> = None;
        if let CleanupConfirmDecision::Confirmed {
            selected_paths,
            selected_docker_targets,
        } = decision
            && (!selected_paths.is_empty()
                || !selected_docker_targets.is_empty())
        {
            if is_in_app
                && let Some(profile_id) = self.modal.remote_profile_id.clone()
            {
                self.confirm_remote_script(
                    profile_id,
                    &selected_paths,
                    selected_docker_targets,
                    selected_bytes,
                    cx,
                );
            } else {
                let selected_count =
                    selected_paths.len() + selected_docker_targets.len();
                let started_title =
                    i18n::messages::cleanup_started_notif_title(locale);
                let started_body = i18n::messages::cleanup_started_notif_body(
                    selected_count,
                    selected_bytes,
                    locale,
                );
                if confirm_only
                    && !cfg!(test)
                    && !notifications::has_native_notification_identity()
                {
                    sync_fallback_notif =
                        Some((started_title.to_owned(), started_body));
                } else {
                    post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_CLEANUP_STARTED,
                        started_title,
                        &started_body,
                        cx,
                    );
                }
                if is_in_app {
                    self.confirm_local_cleanup(
                        &selected_paths,
                        selected_docker_targets,
                        selected_count,
                        selected_bytes,
                        locale,
                        cx,
                    );
                }
            }
        }
        with_runtime_mut(cx, |rt| rt.confirm_window = None);
        window.remove_window();
        if let Some((title, body)) = sync_fallback_notif {
            let _ = ruum_core::watchdog::send_fallback_os_notification(
                &title, &body,
            );
        }
        if confirm_only {
            app_menu::request_quit(cx);
        }
    }

    fn confirm_remote_script(
        &self,
        profile_id: String,
        selected_paths: &[PathBuf],
        docker_targets: Vec<ruum_core::docker_adapter::DockerCleanTarget>,
        selected_bytes: u64,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) {
        let db = self
            .db_path
            .as_deref()
            .and_then(|p| StorageDb::open(p).ok())
            .or_else(|| StorageDb::open_default().ok());
        let profile = db
            .as_ref()
            .and_then(|d| d.list_ssh_profiles().ok())
            .and_then(|list| list.into_iter().find(|p| p.id == profile_id));
        let session_meta = self.remote_session.as_ref().map(|s| {
            let ep = s.endpoint();
            (
                ep.label().to_owned(),
                ep.host().to_owned(),
                ep.port(),
                ep.username().to_owned(),
            )
        });
        let (alias, host, port, username) = if let Some(p) = profile {
            (p.alias, p.host, p.port, p.username)
        } else if let Some((label, host, port, user)) = session_meta {
            (label, host, port, user)
        } else {
            update_open_main_window(cx, |app, cx| {
                app.push_toast(
                    i18n::messages::script_generate_failed(
                        "SSH profile not found in storage",
                        app.locale,
                    ),
                    gpui_omarchy::Status::Error,
                    cx,
                );
            });
            return;
        };

        let targets: Vec<Target> = self
            .modal
            .request
            .items
            .iter()
            .filter(|item| selected_paths.contains(&item.path))
            .map(|item| {
                let hidden = item
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'));
                Target {
                    path: item.path.clone(),
                    bytes: item.bytes,
                    is_dir: false,
                    hidden,
                }
            })
            .collect();
        let total_count = targets.len() + docker_targets.len();
        let header = self.remote_header.clone().unwrap_or_else(|| {
            ruum_core::remote::Header::new(
                self.modal.request.root.clone(),
                None,
                "linux",
                None,
            )
        });
        let spec = RemoteCleanupScriptSpec {
            profile_alias: alias.clone(),
            host,
            port,
            username,
            header,
            targets,
            docker_targets,
        };
        let script_content = match build_remote_cleanup_script(&spec) {
            Ok(content) => content,
            Err(err) => {
                update_open_main_window(cx, |app, cx| {
                    app.push_toast(
                        i18n::messages::script_generate_failed(
                            &err, app.locale,
                        ),
                        gpui_omarchy::Status::Error,
                        cx,
                    );
                });
                return;
            }
        };
        let Some(app_data_dir) = self
            .db_path
            .as_deref()
            .and_then(|p| p.parent().filter(|d| !d.as_os_str().is_empty()))
            .map(std::path::Path::to_path_buf)
            .or_else(|| {
                db.as_ref()
                    .and_then(|d| d.db_path().parent())
                    .map(std::path::Path::to_path_buf)
            })
            .or_else(ruum_watchdog::config::config_dir)
        else {
            update_open_main_window(cx, |app, cx| {
                app.push_toast(
                    i18n::messages::script_generate_failed(
                        "cannot resolve application data directory",
                        app.locale,
                    ),
                    gpui_omarchy::Status::Error,
                    cx,
                );
            });
            return;
        };
        let now_secs = now_unix_secs();
        let script_path = match write_cleanup_script_atomic(
            &app_data_dir,
            &alias,
            now_secs,
            &script_content,
        ) {
            Ok(path) => path,
            Err(err) => {
                let err_msg = err.to_string();
                update_open_main_window(cx, |app, cx| {
                    app.push_toast(
                        i18n::messages::script_generate_failed(
                            &err_msg, app.locale,
                        ),
                        gpui_omarchy::Status::Error,
                        cx,
                    );
                });
                return;
            }
        };
        if let Some(d) = db {
            let _ = d.set_ssh_profile_pending_script(
                &profile_id,
                &script_path,
                total_count as u64,
                selected_bytes,
                now_secs,
            );
        }
        update_open_main_window(cx, |app, cx| {
            app.present_generated_remote_script(
                profile_id,
                alias,
                script_path,
                total_count,
                selected_bytes,
                cx,
            );
        });
    }

    fn confirm_local_cleanup(
        &self,
        selected_paths: &[PathBuf],
        selected_docker_targets: Vec<
            ruum_core::docker_adapter::DockerCleanTarget,
        >,
        selected_count: usize,
        selected_bytes: u64,
        locale: i18n::Locale,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) {
        update_open_main_window(cx, |app, cx| {
            let msg = i18n::messages::cleanup_started_notif_body(
                selected_count,
                selected_bytes,
                app.locale,
            );
            app.push_toast(msg, gpui_omarchy::Status::Neutral, cx);
        });
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
        if approved_items.is_empty() && selected_docker_targets.is_empty() {
            return;
        }
        let task_root = self.modal.request.root.clone();
        let db_path = self.db_path.clone();
        let docker_status = self.docker.clone();
        let docker_estimates: Vec<(
            ruum_core::docker_adapter::DockerCleanTarget,
            u64,
        )> = self
            .modal
            .request
            .docker_items
            .iter()
            .map(|i| (i.target.clone(), i.reclaimable_bytes))
            .collect();
        let home_dir = std::env::home_dir();
        let initial_item = approved_items
            .first()
            .map(|i| marks::display_path(&i.path, home_dir.as_deref()))
            .or_else(|| {
                (!selected_docker_targets.is_empty()).then(|| {
                    i18n::messages::cleanup_docker_stage("cleanup", locale)
                        .to_owned()
                })
            });
        let docker_total = selected_docker_targets.len();
        let total_items = approved_items.len() + docker_total;
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(CleanupProgressSnapshot {
            total_items,
            total_bytes: selected_bytes,
            completed_items: 0,
            reclaimed_bytes: 0,
            current_item: initial_item,
            cancelling: false,
            completion: None,
        }));
        with_runtime_mut(cx, |rt| {
            rt.active_cleanup = Some(ActiveCleanupTask {
                cancel: Arc::clone(&cancel),
                progress: Arc::clone(&progress),
            });
        });
        let _ = open_cleanup_progress_window_with_locale(Some(locale), cx);
        refresh_tray_cleanup_status(cx);

        let cancel_bg = Arc::clone(&cancel);
        let progress_bg = Arc::clone(&progress);
        let progress_signal = Arc::new(AsyncSignal::default());
        let bg_signal = Arc::clone(&progress_signal);
        let done_flag = Arc::new(AtomicBool::new(false));
        let bg_done = Arc::clone(&done_flag);
        #[cfg(test)]
        let test_gate = cx
            .try_global::<AppRuntime>()
            .and_then(|rt| rt.test_cleanup_gate.as_ref().map(Arc::clone));

        let task = cx.background_executor().spawn(async move {
            use ruum_core::watchdog::CleanupProgressEvent;

            #[cfg(test)]
            if let Some(gate) = test_gate {
                gate.wait().await;
            }

            let mut total_bytes = 0_u64;
            let mut total_count = 0_usize;
            let mut pruned_paths = Vec::new();
            let mut cancelled = cancel_bg.load(Ordering::Acquire);
            let mut file_completed = 0_usize;

            if !approved_items.is_empty() {
                let (bytes, count, paths, files_cancelled) =
                    ruum_core::watchdog::execute_confirmed_cleanup_cancellable(
                        &approved_items,
                        &task_root,
                        &cancel_bg,
                        &mut |ev| {
                            if let Ok(mut snap) = progress_bg.lock() {
                                match ev {
                                    CleanupProgressEvent::Start {
                                        total_items: file_planned,
                                        ..
                                    } => {
                                        snap.total_items =
                                            file_planned + docker_total;
                                    }
                                    CleanupProgressEvent::ItemDone {
                                        path,
                                        bytes,
                                        outcome,
                                    } => {
                                        file_completed += 1;
                                        snap.completed_items = file_completed;
                                        if outcome.is_ok() {
                                            snap.reclaimed_bytes = snap
                                                .reclaimed_bytes
                                                .saturating_add(bytes);
                                        }
                                        snap.current_item =
                                            Some(marks::display_path(
                                                &path,
                                                home_dir.as_deref(),
                                            ));
                                    }
                                    CleanupProgressEvent::Finished {
                                        reclaimed_bytes,
                                        cancelled: fin_cancelled,
                                        ..
                                    } => {
                                        snap.reclaimed_bytes = reclaimed_bytes;
                                        if fin_cancelled {
                                            snap.cancelling = true;
                                        }
                                    }
                                }
                            }
                            bg_signal.notify();
                        },
                    );
                total_bytes = bytes;
                total_count = count;
                pruned_paths = paths;
                cancelled |= files_cancelled;
            }
            if !cancelled {
                for item in &approved_items {
                    if !item.path.exists() && !pruned_paths.contains(&item.path)
                    {
                        pruned_paths.push(item.path.clone());
                    }
                }
            }
            if !pruned_paths.is_empty()
                && let Some(path) = db_path.as_deref()
                && let Ok(db) = StorageDb::open(path)
            {
                let _ = db.prune_ai_state_paths(&pruned_paths, now_unix_secs());
            }
            let mut docker_err: Option<String> = None;
            let fresh_docker = if selected_docker_targets.is_empty()
                || cancelled
                || cancel_bg.load(Ordering::Acquire)
            {
                if !selected_docker_targets.is_empty() {
                    cancelled = true;
                }
                None
            } else {
                let adapter = DockerFactory::create();
                let env = DockerEnv::current();
                if let DockerProbeStatus::Available(summary) = docker_status {
                    if let Ok(mut snap) = progress_bg.lock() {
                        snap.current_item = Some(
                            i18n::messages::cleanup_docker_stage(
                                "cleanup", locale,
                            )
                            .to_owned(),
                        );
                    }
                    bg_signal.notify();
                    let outcomes = adapter
                        .run_clean_targets_with_env_progress_cancellable(
                            &env,
                            &summary,
                            &selected_docker_targets,
                            Some(&cancel_bg),
                            &mut |stage, done, _total| {
                                if let Ok(mut snap) = progress_bg.lock() {
                                    snap.completed_items =
                                        file_completed + done;
                                    snap.current_item = Some(
                                        i18n::messages::cleanup_docker_stage(
                                            stage, locale,
                                        )
                                        .to_owned(),
                                    );
                                }
                                bg_signal.notify();
                            },
                        );
                    for outcome in outcomes {
                        let est = docker_estimates
                            .iter()
                            .find(|(t, _)| *t == outcome.target)
                            .map_or(0, |(_, b)| *b);
                        match outcome.result {
                            Ok(()) => {
                                total_bytes = total_bytes.saturating_add(est);
                                total_count += 1;
                            }
                            Err(err) if docker_err.is_none() => {
                                docker_err = Some(err);
                            }
                            Err(_) => {}
                        }
                    }
                    if cancel_bg.load(Ordering::Acquire) {
                        cancelled = true;
                    }
                } else {
                    docker_err =
                        Some("Docker summary unavailable for cleanup".into());
                }
                Some(adapter.probe(&env))
            };
            let fresh_space = ruum_core::space::space_info(&task_root).ok();
            let home_space = std::env::home_dir().and_then(|h| {
                if h == task_root {
                    fresh_space
                } else {
                    ruum_core::space::space_info(&h).ok()
                }
            });
            bg_done.store(true, Ordering::Release);
            bg_signal.notify();
            CleanupTaskOutcome {
                root: task_root,
                locale,
                bytes: total_bytes,
                count: total_count,
                pruned_paths,
                fresh_space,
                home_space,
                fresh_docker,
                docker_err,
                cancelled,
            }
        });

        let obs_signal = Arc::clone(&progress_signal);
        let obs_done = Arc::clone(&done_flag);
        cx.spawn(async move |_, cx| {
            loop {
                if obs_done.load(Ordering::Acquire) {
                    break;
                }
                obs_signal.wait().await;
                if obs_done.load(Ordering::Acquire) {
                    break;
                }
                cx.update(|cx| {
                    refresh_cleanup_progress_ui(cx);
                });
            }
        })
        .detach();

        let fin_signal = Arc::clone(&progress_signal);
        let fin_done = Arc::clone(&done_flag);
        cx.spawn(async move |_, cx| {
            let outcome = task.await;
            fin_done.store(true, Ordering::Release);
            fin_signal.notify();
            cx.update(|cx| {
                finish_active_cleanup_task(outcome, cx);
            });
        })
        .detach();
    }
}

impl Render for CleanupConfirmWindow {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) -> impl IntoElement {
        let content = rail_pages::cleanup_confirm_view(
            &self.modal,
            &self.focus,
            self.locale,
            cx,
        );
        let entity = cx.entity().downgrade();
        window_chrome::wrap_window_chrome(
            i18n::messages::title_confirm_cleanup(self.locale),
            content,
            move |window, cx| {
                Self::close_from_chrome(&entity, window, cx);
            },
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

/// Open (or update) a local Watchdog/AI cleanup confirmation window with
/// `ruum.db` pruning wired on completion.
pub fn open_local_confirm_window(
    request: CleanupConfirmRequest,
    db_path: Option<PathBuf>,
    docker: DockerProbeStatus,
    cx: &mut App,
) -> Option<WindowHandle<CleanupConfirmWindow>> {
    let handle = open_confirm_window(request, None, None, cx)?;
    let _ = handle.update(cx, |win, _, _| {
        win.set_target_context(db_path, None, None, None, docker);
    });
    Some(handle)
}

/// Open (or update) a remote SSH AI cleanup confirmation window scoped to a
/// specific `SshSession`, `Header`, and storage DB (generates cleanup script
/// on confirm).
pub fn open_remote_confirm_window(
    request: CleanupConfirmRequest,
    remote_profile_id: String,
    session: Arc<ruum_core::ssh_session::SshSession>,
    header: ruum_core::remote::Header,
    db_path: Option<PathBuf>,
    cx: &mut App,
) -> Option<WindowHandle<CleanupConfirmWindow>> {
    let docker = header.docker.clone();
    let handle = open_confirm_window(request, None, None, cx)?;
    let _ = handle.update(cx, |win, _, _| {
        win.set_target_context(
            db_path,
            Some(remote_profile_id),
            Some(session),
            Some(header),
            docker,
        );
    });
    Some(handle)
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
            enter_foreground_mode(cx);
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
                window_bounds: Some(gpui_kit::WindowBounds::centered(
                    size(px(760.), px(560.)),
                    cx,
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
                window_chrome::observe_window_chrome(window);
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
    enter_foreground_mode(cx);

    with_runtime_mut(cx, |rt| rt.confirm_window = Some(window));
    Some(window)
}

/// How long the standalone progress window lingers after cleanup completes
/// before automatically closing.
pub(crate) const CLEANUP_PROGRESS_LINGER: Duration = Duration::from_secs(3);

/// Terminal state of an in-app cleanup run displayed while lingering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupProgressCompletion {
    Completed { count: usize, bytes: u64 },
    Cancelled { count: usize, bytes: u64 },
    Failed { message: String },
}

/// Snapshot of the currently executing in-app background cleanup task.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanupProgressSnapshot {
    pub total_items: usize,
    pub total_bytes: u64,
    pub completed_items: usize,
    pub reclaimed_bytes: u64,
    pub current_item: Option<String>,
    pub cancelling: bool,
    pub completion: Option<CleanupProgressCompletion>,
}

/// Standalone floating progress window for an active local cleanup job.
#[derive(Debug)]
pub struct CleanupProgressWindow {
    pub focus: FocusHandle,
    pub locale: i18n::Locale,
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<Mutex<CleanupProgressSnapshot>>,
    linger_epoch: u64,
}

struct CleanupTaskOutcome {
    root: PathBuf,
    locale: i18n::Locale,
    bytes: u64,
    count: usize,
    pruned_paths: Vec<PathBuf>,
    fresh_space: Option<ruum_core::space::SpaceInfo>,
    home_space: Option<ruum_core::space::SpaceInfo>,
    fresh_docker: Option<DockerProbeStatus>,
    docker_err: Option<String>,
    cancelled: bool,
}

fn finish_active_cleanup_task(outcome: CleanupTaskOutcome, cx: &mut App) {
    let quitting = app_menu::is_quitting(cx);
    let windows = cx.windows();
    let progress_win = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.cleanup_progress_window)
        .filter(|h| windows.contains(&(*h).into()));

    with_runtime_mut(cx, |rt| {
        rt.active_cleanup = None;
        if progress_win.is_none() || quitting {
            rt.cleanup_progress_window = None;
        }
    });
    if quitting {
        if let Some(handle) = progress_win {
            let _ = handle.update(cx, |_, window, _| {
                window.remove_window();
            });
        }
        return;
    }
    if let Some(handle) = progress_win {
        let completion = if outcome.cancelled {
            CleanupProgressCompletion::Cancelled {
                count: outcome.count,
                bytes: outcome.bytes,
            }
        } else if outcome.count == 0
            && let Some(ref err) = outcome.docker_err
        {
            CleanupProgressCompletion::Failed {
                message: err.clone(),
            }
        } else {
            CleanupProgressCompletion::Completed {
                count: outcome.count,
                bytes: outcome.bytes,
            }
        };
        let scheduled = handle.update(cx, |win, _, cx| {
            let total =
                win.progress.lock().map_or(outcome.count, |g| g.total_items);
            let final_completed = if outcome.cancelled {
                outcome.count
            } else {
                total.max(outcome.count)
            };
            let epoch =
                win.mark_finished(completion, final_completed, outcome.bytes);
            cx.notify();
            epoch
        });
        if let Ok(scheduled_epoch) = scheduled {
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(CLEANUP_PROGRESS_LINGER)
                    .await;
                cx.update(|cx| {
                    if app_menu::is_quitting(cx)
                        || !cx.windows().contains(&handle.into())
                    {
                        return;
                    }
                    let should_close = handle
                        .read_with(cx, |win, _| {
                            win.linger_epoch == scheduled_epoch
                        })
                        .unwrap_or(false);
                    if !should_close {
                        return;
                    }
                    with_runtime_mut(cx, |rt| {
                        if rt.cleanup_progress_window == Some(handle) {
                            rt.cleanup_progress_window = None;
                        }
                    });
                    let _ = handle.update(cx, |_, window, _| {
                        window.remove_window();
                    });
                });
            })
            .detach();
        } else {
            with_runtime_mut(cx, |rt| {
                if rt.cleanup_progress_window == Some(handle) {
                    rt.cleanup_progress_window = None;
                }
            });
        }
    }

    let locale = outcome.locale;
    if outcome.cancelled {
        let summary = i18n::messages::cleanup_cancelled_summary(
            outcome.count,
            outcome.bytes,
            locale,
        );
        post_app_notification(
            ruum_core::watchdog::NOTIF_TAG_CLEANUP_DONE,
            i18n::messages::cleanup_cancelled_notif_title(locale),
            &summary,
            cx,
        );
    } else if outcome.count > 0 {
        let summary = i18n::messages::cleaned_items_summary(
            outcome.count,
            outcome.bytes,
            locale,
        );
        post_app_notification(
            ruum_core::watchdog::NOTIF_TAG_CLEANUP_DONE,
            i18n::messages::cleanup_complete_notif_title(locale),
            &summary,
            cx,
        );
    }

    refresh_tray_status_with_space(outcome.home_space, cx);
    update_open_main_window(cx, |app, cx| {
        app.prune_local_ai_proposal(&outcome.pruned_paths);
        if let Some(space) = outcome.fresh_space {
            if app.active_host_tab == state::HostTabKey::Local
                && app.root_path == outcome.root
            {
                app.space = Some(space);
            } else if let Some(local_tab) = app
                .host_tabs
                .iter_mut()
                .find(|t| t.key == state::HostTabKey::Local)
                && let Some(snap) = &mut local_tab.session
            {
                snap.space = Some(space);
            }
        }
        if let Some(hs) = outcome.home_space {
            app.watchdog_home_space = Some(hs);
            let checked_at = now_unix_secs();
            app.watchdog_space_checked_at = Some(checked_at);
            app.watchdog_state.last_checked_secs = Some(checked_at);
        }
        if let Some(docker) = outcome.fresh_docker {
            app.apply_local_docker_status(docker);
        }
        app.reload_watchdog_state();
        app.notice = Some(if outcome.cancelled {
            (
                i18n::messages::cleanup_cancelled_summary(
                    outcome.count,
                    outcome.bytes,
                    app.locale,
                ),
                gpui_omarchy::Status::Neutral,
            )
        } else if let Some(err) = outcome.docker_err
            && outcome.count == 0
        {
            (err, gpui_omarchy::Status::Error)
        } else {
            (
                i18n::messages::cleaned_items_summary(
                    outcome.count,
                    outcome.bytes,
                    app.locale,
                ),
                gpui_omarchy::Status::Success,
            )
        });
        cx.notify();
    });
}

fn refresh_cleanup_progress_ui(cx: &mut App) {
    let windows = cx.windows();
    let progress_win = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.cleanup_progress_window)
        .filter(|h| windows.contains(&(*h).into()));
    if let Some(handle) = progress_win {
        let _ = handle.update(cx, |_, _, cx| cx.notify());
    }
    refresh_tray_cleanup_status(cx);
}

impl CleanupProgressWindow {
    pub fn new(
        cancel: Arc<AtomicBool>,
        progress: Arc<Mutex<CleanupProgressSnapshot>>,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            locale: i18n::Locale::current(),
            cancel,
            progress,
            linger_epoch: 0,
        }
    }

    pub fn replace_task(
        &mut self,
        cancel: Arc<AtomicBool>,
        progress: Arc<Mutex<CleanupProgressSnapshot>>,
    ) {
        if !Arc::ptr_eq(&self.progress, &progress) {
            self.linger_epoch = self.linger_epoch.wrapping_add(1);
        }
        self.cancel = cancel;
        self.progress = progress;
    }

    pub(crate) fn mark_finished(
        &mut self,
        completion: CleanupProgressCompletion,
        completed_items: usize,
        reclaimed_bytes: u64,
    ) -> u64 {
        if let Ok(mut snap) = self.progress.lock() {
            snap.completed_items = completed_items;
            snap.reclaimed_bytes = reclaimed_bytes;
            snap.cancelling = false;
            snap.current_item = None;
            snap.completion = Some(completion);
        }
        self.linger_epoch = self.linger_epoch.wrapping_add(1);
        self.linger_epoch
    }

    #[cfg(test)]
    pub const fn set_locale(&mut self, locale: i18n::Locale) {
        self.locale = locale;
    }

    #[must_use]
    pub fn snapshot(&self) -> CleanupProgressSnapshot {
        self.progress
            .lock()
            .map_or_else(|_| CleanupProgressSnapshot::default(), |g| g.clone())
    }

    pub fn close_from_chrome(
        _entity: &WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) {
        Self::continue_in_background(window, cx);
    }

    pub fn continue_in_background(window: &mut Window, cx: &mut App) {
        with_runtime_mut(cx, |rt| rt.cleanup_progress_window = None);
        window.remove_window();
    }

    pub fn cancel_cleanup(&mut self, cx: &mut gpui_kit::Context<'_, Self>) {
        self.cancel.store(true, Ordering::Release);
        if let Ok(mut snap) = self.progress.lock() {
            snap.cancelling = true;
        }
        refresh_tray_cleanup_status(cx);
        cx.notify();
    }
}

impl Render for CleanupProgressWindow {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) -> impl IntoElement {
        let snap = self.snapshot();
        let content = rail_pages::cleanup_progress_view(
            &snap,
            &self.focus,
            self.locale,
            cx,
        );
        let entity = cx.entity().downgrade();
        window_chrome::wrap_window_chrome(
            i18n::messages::title_cleanup_progress(self.locale),
            content,
            move |window, cx| {
                Self::close_from_chrome(&entity, window, cx);
            },
        )
    }
}

/// Open or focus the compact background cleanup progress window when an active
/// cleanup task exists.
pub fn open_cleanup_progress_window(
    cx: &mut App,
) -> Option<WindowHandle<CleanupProgressWindow>> {
    open_cleanup_progress_window_with_locale(None, cx)
}

fn open_cleanup_progress_window_with_locale(
    locale_override: Option<i18n::Locale>,
    cx: &mut App,
) -> Option<WindowHandle<CleanupProgressWindow>> {
    let windows = cx.windows();
    let existing = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.cleanup_progress_window)
        .filter(|h| windows.contains(&(*h).into()));
    let active_task = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.active_cleanup.as_ref())
        .map(|task| (Arc::clone(&task.cancel), Arc::clone(&task.progress)));

    if let Some(handle) = existing {
        let updated = handle.update(cx, |win, window, cx| {
            if let Some((cancel, progress)) = active_task {
                win.replace_task(cancel, progress);
            }
            if let Some(loc) = locale_override {
                win.locale = loc;
            }
            let focus = win.focus.clone();
            window.focus(&focus, cx);
            window.activate_window();
            cx.notify();
        });
        if updated.is_ok() {
            enter_foreground_mode(cx);
            return Some(handle);
        }
    }

    let (cancel, progress, native_look) = {
        let rt = cx.try_global::<AppRuntime>()?;
        let task = rt.active_cleanup.as_ref()?;
        (
            Arc::clone(&task.cancel),
            Arc::clone(&task.progress),
            rt.launch.native_look,
        )
    };
    let initial_locale = locale_override.unwrap_or_else(i18n::Locale::current);

    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::centered(
                    size(px(540.), px(300.)),
                    cx,
                )),
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some(
                        i18n::messages::title_cleanup_progress(initial_locale)
                            .into(),
                    ),
                    ..Default::default()
                }),
                app_id: Some("ruum".to_owned()),
                window_min_size: Some(size(px(460.), px(280.))),
                ..Default::default()
            },
            move |window, cx| {
                if native_look {
                    appearance::follow(window);
                }
                window_chrome::observe_window_chrome(window);
                let entity = cx.new(|cx| {
                    let mut win =
                        CleanupProgressWindow::new(cancel, progress, cx);
                    win.locale = initial_locale;
                    win
                });
                window.on_window_should_close(cx, move |_, cx| {
                    with_runtime_mut(cx, |rt| {
                        rt.cleanup_progress_window = None;
                    });
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
    enter_foreground_mode(cx);

    with_runtime_mut(cx, |rt| rt.cleanup_progress_window = Some(window));
    Some(window)
}

/// Standalone window confirming an MCP tool's request to connect over SSH.
#[derive(Debug)]
pub struct RemoteConnectConfirmWindow {
    pub modal: RemoteConnectConfirmModal,
    pub focus: FocusHandle,
    pub confirm_only: bool,
    pub handled: bool,
    pub locale: i18n::Locale,
}

impl RemoteConnectConfirmWindow {
    /// Close request from the client-side titlebar: must go through
    /// `deny` so the waiting requester is answered; only a dead entity
    /// falls back to removing the window outright.
    pub fn close_from_chrome(
        entity: &WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if entity.update(cx, |this, cx| this.deny(window, cx)).is_err() {
            window.remove_window();
        }
    }

    pub fn new(
        request: RemoteConnectConfirmRequest,
        responder: Option<Sender<RemoteConnectConfirmDecision>>,
        response_file: Option<PathBuf>,
        confirm_only: bool,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) -> Self {
        Self {
            modal: RemoteConnectConfirmModal::new(
                request,
                responder,
                response_file,
            ),
            focus: cx.focus_handle(),
            confirm_only,
            handled: false,
            locale: i18n::Locale::current(),
        }
    }

    pub fn replace_request(
        &mut self,
        request: RemoteConnectConfirmRequest,
        responder: Option<Sender<RemoteConnectConfirmDecision>>,
        response_file: Option<PathBuf>,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) {
        self.ensure_denied();
        self.modal =
            RemoteConnectConfirmModal::new(request, responder, response_file);
        self.handled = false;
        cx.notify();
    }

    /// Fail closed: if no explicit approval happened yet, emit `Denied`.
    pub fn ensure_denied(&mut self) {
        if !self.handled {
            self.handled = true;
            self.modal.deny();
        }
    }

    pub fn deny(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) {
        self.ensure_denied();
        let confirm_only = self.confirm_only;
        window.remove_window();
        if confirm_only {
            app_menu::request_quit(cx);
        }
    }

    pub fn approve(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) {
        if self.handled {
            return;
        }
        self.handled = true;
        self.modal.approve();
        let confirm_only = self.confirm_only;
        window.remove_window();
        if confirm_only {
            app_menu::request_quit(cx);
        }
    }
}

impl Render for RemoteConnectConfirmWindow {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui_kit::Context<'_, Self>,
    ) -> impl IntoElement {
        let content = rail_pages::remote_connect_confirm_view(
            &self.modal,
            &self.focus,
            self.locale,
            cx,
        );
        let entity = cx.entity().downgrade();
        window_chrome::wrap_window_chrome(
            i18n::messages::title_confirm_remote_connect(self.locale),
            content,
            move |window, cx| {
                Self::close_from_chrome(&entity, window, cx);
            },
        )
    }
}

pub(crate) fn open_remote_connect_confirm_window_with_mode(
    request: RemoteConnectConfirmRequest,
    responder: Option<Sender<RemoteConnectConfirmDecision>>,
    response_file: Option<PathBuf>,
    confirm_only: bool,
    cx: &mut App,
) -> Option<WindowHandle<RemoteConnectConfirmWindow>> {
    let windows = cx.windows();
    let existing = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.remote_connect_window)
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
            enter_foreground_mode(cx);
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
                window_bounds: Some(gpui_kit::WindowBounds::centered(
                    size(px(560.), px(380.)),
                    cx,
                )),
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some(
                        i18n::messages::title_confirm_remote_connect(
                            i18n::Locale::current(),
                        )
                        .into(),
                    ),
                    ..Default::default()
                }),
                app_id: Some("ruum".to_owned()),
                window_min_size: Some(size(px(460.), px(320.))),
                ..Default::default()
            },
            move |window, cx| {
                if native_look {
                    appearance::follow(window);
                }
                window_chrome::observe_window_chrome(window);
                let entity = cx.new(|cx| {
                    RemoteConnectConfirmWindow::new(
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
                        win.ensure_denied();
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
    enter_foreground_mode(cx);

    with_runtime_mut(cx, |rt| rt.remote_connect_window = Some(window));
    Some(window)
}

pub(crate) fn with_runtime_mut(cx: &mut App, f: impl FnOnce(&mut AppRuntime)) {
    if cx.has_global::<AppRuntime>() {
        f(cx.global_mut::<AppRuntime>());
    }
}

/// Return whether a cleanup proposal has any actionable filesystem or Docker
/// items.
#[must_use]
pub(crate) const fn has_actionable_proposal(
    proposal: &ruum_core::watchdog_config::CleanupProposal,
) -> bool {
    !proposal.items.is_empty() || !proposal.docker_targets.is_empty()
}

const TRAY_SPACE_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// Return whether the periodic 30s tray disk-space refresh should fire.
#[must_use]
pub(crate) fn should_refresh_tray_disk_space(
    has_tray: bool,
    tray_show_disk_space: bool,
    last_refresh: Option<Instant>,
    now: Instant,
) -> bool {
    if !has_tray || !tray_show_disk_space {
        return false;
    }
    last_refresh.is_none_or(|prev| {
        now.saturating_duration_since(prev) >= TRAY_SPACE_REFRESH_INTERVAL
    })
}

/// Update cached `AppRuntime` fields derived from `AppSettings`.
pub(crate) fn sync_runtime_app_settings(
    settings: &ruum_core::storage::AppSettings,
    cx: &mut App,
) {
    with_runtime_mut(cx, |rt| {
        rt.tray_show_disk_space = settings.tray_show_disk_space;
    });
}

/// Refresh tray menu status lines if a tray icon is active.
pub fn refresh_tray_status(cx: &mut App) {
    refresh_tray_status_with_space(None, cx);
}

/// Refresh tray menu status and disk gauge, reusing `home_space_override` when
/// the caller has already measured the home volume.
pub(crate) fn refresh_tray_status_with_space(
    home_space_override: Option<ruum_core::space::SpaceInfo>,
    cx: &mut App,
) {
    let Some(rt) = cx.try_global::<AppRuntime>() else {
        return;
    };
    if rt.tray.is_none() {
        return;
    }
    let watchdog_enabled = rt.watchdog_config.enabled;
    let show_disk_space = rt.tray_show_disk_space;
    let space = home_space_override.or_else(|| {
        std::env::home_dir()
            .as_deref()
            .and_then(|h| ruum_core::space::space_info(h).ok())
    });
    let has_proposal = StorageDb::open_default()
        .ok()
        .and_then(|db| db.load_ai_state().ok())
        .and_then(|s| s.parse_proposal().ok().flatten())
        .is_some_and(|p| has_actionable_proposal(&p));

    let cleanup_status = current_tray_cleanup_status(cx);
    let now = Instant::now();
    if let Some(rt) = cx.try_global::<AppRuntime>()
        && let Some(tray) = &rt.tray
    {
        tray.update_status(
            space,
            watchdog_enabled,
            has_proposal,
            show_disk_space,
        );
        tray.set_cleanup_progress(cleanup_status);
    }
    with_runtime_mut(cx, |rt| {
        rt.last_tray_space_refresh = Some(now);
    });
}

pub(crate) fn refresh_tray_cleanup_status(cx: &App) {
    let cleanup_status = current_tray_cleanup_status(cx);
    if let Some(rt) = cx.try_global::<AppRuntime>()
        && let Some(tray) = &rt.tray
    {
        tray.set_cleanup_progress(cleanup_status);
    }
}

/// Transition the process into tray-resident background presentation mode.
pub(crate) fn enter_background_mode(cx: &mut App) {
    background::BackgroundPresentationFactory::create().enter_background(cx);
}

/// Transition the process back into foreground presentation mode.
pub(crate) fn enter_foreground_mode(cx: &mut App) {
    background::BackgroundPresentationFactory::create().enter_foreground(cx);
}

/// Cancel/deny pending confirmation windows, drop runtime IPC/tray handles,
/// and remove any legacy headless `ruum watchdog` OS service residue before
/// quitting so `WatchdogIpcServer::drop` never deadlocks.
pub(crate) fn cleanup_runtime_on_quit(cx: &mut App) {
    for win in cx.windows() {
        if let Some(confirm) = win.downcast::<CleanupConfirmWindow>() {
            let _ = confirm.update(cx, |this, window, _| {
                this.ensure_cancelled();
                window.remove_window();
            });
        } else if let Some(remote) =
            win.downcast::<RemoteConnectConfirmWindow>()
        {
            let _ = remote.update(cx, |this, window, _| {
                this.ensure_denied();
                window.remove_window();
            });
        } else if let Some(progress) = win.downcast::<CleanupProgressWindow>() {
            let _ = progress.update(cx, |this, window, _| {
                this.cancel.store(true, Ordering::Release);
                window.remove_window();
            });
        }
    }

    let (tray, ipc_server) = if cx.has_global::<AppRuntime>() {
        let rt = cx.global_mut::<AppRuntime>();
        if let Some(task) = rt.active_cleanup.take() {
            task.cancel.store(true, Ordering::Release);
            if let Ok(mut snap) = task.progress.lock() {
                snap.cancelling = true;
            }
        }
        rt.confirm_window = None;
        rt.cleanup_progress_window = None;
        rt.remote_connect_window = None;
        rt.ipc_rx = None;
        (rt.tray.take(), rt.ipc_server.take())
    } else {
        (None, None)
    };
    drop(tray);
    drop(ipc_server);

    if !cfg!(test) {
        let _ = ruum_core::watchdog::uninstall_legacy_service_if_present(
            Duration::from_secs(2),
        );
    }
}

/// Update cached runtime Watchdog config and optionally reset poll timer.
pub(crate) fn notify_watchdog_config_saved(
    new_cfg: &ruum_watchdog::WatchdogConfig,
    reset_poll: bool,
    cx: &mut App,
) {
    with_runtime_mut(cx, |rt| {
        rt.watchdog_config = new_cfg.normalized();
        if reset_poll {
            rt.last_watchdog_poll = None;
        }
    });
    refresh_tray_status(cx);
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
pub(crate) struct ActiveCleanupTask {
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) progress: Arc<Mutex<CleanupProgressSnapshot>>,
}

#[derive(Debug)]
pub(crate) struct AppRuntime {
    launch: LaunchConfig,
    pub(crate) cred_store: Option<ruum_core::credentials::LazyCredentialStore>,
    pub(crate) main_window: Option<WindowHandle<Ruum>>,
    confirm_window: Option<WindowHandle<CleanupConfirmWindow>>,
    cleanup_progress_window: Option<WindowHandle<CleanupProgressWindow>>,
    remote_connect_window: Option<WindowHandle<RemoteConnectConfirmWindow>>,
    pub(crate) active_cleanup: Option<ActiveCleanupTask>,
    tray: Option<tray_menu::TrayMenu>,
    ipc_rx: Option<Receiver<watchdog_runtime::WatchdogIpcEvent>>,
    ipc_server: Option<watchdog_runtime::WatchdogIpcServer>,
    in_proc_confirm_tx: Sender<ConfirmChannelPayload>,
    in_proc_confirm_rx: Receiver<ConfirmChannelPayload>,
    in_proc_notif_tx: Sender<NotificationChannelPayload>,
    in_proc_notif_rx: Receiver<NotificationChannelPayload>,
    last_watchdog_poll: Option<Instant>,
    pub(crate) tray_show_disk_space: bool,
    pub(crate) last_tray_space_refresh: Option<Instant>,
    background_analysis_in_flight: bool,
    watchdog_config: ruum_watchdog::WatchdogConfig,
    pub(crate) update_state: state::UpdateUiState,
    pub(crate) update_settings: ruum_core::storage::UpdateSettings,
    pub(crate) update_install_kind: ruum_core::updater::InstallKind,
    pub(crate) update_banner_dismissed_version: Option<String>,
    pub(crate) update_download_cancel: Option<Arc<AtomicBool>>,
    pub(crate) update_last_error: Option<String>,
    pub(crate) update_quitting: Arc<AtomicBool>,
    #[cfg(test)]
    pub(crate) test_cleanup_gate: Option<Arc<AsyncSignal>>,
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
        cred_store: None,
        main_window,
        confirm_window: None,
        cleanup_progress_window: None,
        remote_connect_window: None,
        active_cleanup: None,
        tray: None,
        ipc_rx: None,
        ipc_server: None,
        in_proc_confirm_tx,
        in_proc_confirm_rx,
        in_proc_notif_tx,
        in_proc_notif_rx,
        last_watchdog_poll: None,
        tray_show_disk_space: false,
        last_tray_space_refresh: None,
        background_analysis_in_flight: false,
        watchdog_config: ruum_watchdog::WatchdogConfig::default(),
        update_state: state::UpdateUiState::Idle,
        update_settings: ruum_core::storage::UpdateSettings::default(),
        update_install_kind: ruum_core::updater::InstallKind::Development,
        update_banner_dismissed_version: None,
        update_download_cancel: None,
        update_last_error: None,
        update_quitting: Arc::new(AtomicBool::new(false)),
        #[cfg(test)]
        test_cleanup_gate: None,
    });
}

#[derive(Debug, Default)]
pub(crate) struct AsyncSignal {
    pending: AtomicBool,
    waker: Mutex<Option<std::task::Waker>>,
}

impl AsyncSignal {
    pub(crate) fn notify(&self) {
        self.pending.store(true, Ordering::Release);
        if let Ok(mut guard) = self.waker.lock()
            && let Some(waker) = guard.take()
        {
            waker.wake();
        }
    }

    pub(crate) async fn wait(&self) {
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

/// Default main window size (`1440×900`, centered on the primary display).
pub(crate) const DEFAULT_MAIN_WINDOW_SIZE: Size<Pixels> =
    size(px(1440.), px(900.));

/// Minimum main window size (`900×600`).
pub(crate) const MIN_MAIN_WINDOW_SIZE: Size<Pixels> = size(px(900.), px(600.));

/// Pre-authorize the process-shared credential store once in the background
/// when an existing `ruum.db` has saved SSH profiles and
/// `device-credentials.enc` already exists on disk. Never creates an empty DB.
pub(crate) fn preload_credentials_if_needed(
    db_path: Option<&std::path::Path>,
    store: &ruum_core::credentials::LazyCredentialStore,
) -> Result<bool, String> {
    if store.is_loaded() || !store.has_encrypted_file() {
        return Ok(false);
    }
    let Some(path) = db_path.filter(|p| p.is_file()) else {
        return Ok(false);
    };
    let db = StorageDb::open(path).map_err(|err| err.to_string())?;
    let profiles = db.list_ssh_profiles().map_err(|err| err.to_string())?;
    if profiles.is_empty() {
        return Ok(false);
    }
    store.preload()
}

/// Compute initial main window dimensions (default 1440x900, clamped to the
/// primary display and at least 900x600).
#[must_use]
pub(crate) fn initial_main_window_size(
    screen_size: Option<Size<Pixels>>,
) -> Size<Pixels> {
    let Some(screen) = screen_size else {
        return DEFAULT_MAIN_WINDOW_SIZE;
    };
    let width = screen
        .width
        .as_f32()
        .min(DEFAULT_MAIN_WINDOW_SIZE.width.as_f32())
        .max(MIN_MAIN_WINDOW_SIZE.width.as_f32());
    let height = screen
        .height
        .as_f32()
        .min(DEFAULT_MAIN_WINDOW_SIZE.height.as_f32())
        .max(MIN_MAIN_WINDOW_SIZE.height.as_f32());
    size(px(width), px(height))
}

fn open_main_window(
    cx: &mut App,
    title_root_override: Option<PathBuf>,
) -> WindowHandle<Ruum> {
    let (
        launch,
        shared_cred_store,
        rt_update_state,
        rt_update_settings,
        rt_update_install_kind,
        rt_update_banner_dismissed,
        rt_update_cancel,
        rt_update_last_error,
    ) = {
        let rt = cx
            .try_global::<AppRuntime>()
            .expect("AppRuntime installed before opening main window");
        (
            rt.launch.clone(),
            rt.cred_store.clone(),
            rt.update_state.clone(),
            rt.update_settings.clone(),
            rt.update_install_kind,
            rt.update_banner_dismissed_version.clone(),
            rt.update_download_cancel.clone(),
            rt.update_last_error.clone(),
        )
    };
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
                window_min_size: Some(MIN_MAIN_WINDOW_SIZE),
                ..Default::default()
            },
            move |window, cx| {
                if native_look {
                    appearance::follow(window);
                }
                window_chrome::observe_window_chrome(window);
                cx.new(|cx| {
                    let mut app = if launch.choose_root {
                        Ruum::choose_with_cred_store(
                            launch.options,
                            launch.depth,
                            launch.remote,
                            shared_cred_store,
                            cx,
                        )
                    } else {
                        Ruum::open_with_cred_store(
                            launch.root,
                            launch.options,
                            launch.depth,
                            launch.remote,
                            launch.remote_disk,
                            shared_cred_store,
                            cx,
                        )
                    };
                    app.power_choice = launch.power;
                    app.update_state = rt_update_state;
                    app.update_settings = rt_update_settings;
                    app.update_install_kind = rt_update_install_kind;
                    app.update_banner_dismissed_version =
                        rt_update_banner_dismissed;
                    app.update_download_cancel = rt_update_cancel;
                    app.update_last_error = rt_update_last_error;
                    if let Some(screen) = launch.initial_screen {
                        app.apply_target_screen(screen, cx);
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
    if !cfg!(test) {
        cx.background_executor()
            .spawn(async {
                let _ = ruum_core::notify::sync_notification_icon();
            })
            .detach();
    }
}

const fn screen_open_tag(screen: Screen) -> &'static str {
    match screen {
        Screen::Explore | Screen::Review | Screen::Running | Screen::Done => {
            "ruum.screen.explore"
        }
        Screen::Docker => "ruum.screen.docker",
        Screen::Remote => "ruum.screen.remote",
        Screen::Watchdog => "ruum.screen.watchdog",
        Screen::Settings => "ruum.screen.settings",
    }
}

#[must_use]
pub(crate) fn target_screen_for_notification_tag(tag: &str) -> Option<Screen> {
    match tag {
        ruum_core::watchdog::NOTIF_TAG_LOW_SPACE
        | ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS
        | "ruum.screen.watchdog" => Some(Screen::Watchdog),
        "ruum.screen.explore" => Some(Screen::Explore),
        "ruum.screen.docker" => Some(Screen::Docker),
        "ruum.screen.remote" => Some(Screen::Remote),
        "ruum.screen.settings" => Some(Screen::Settings),
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

pub(crate) fn open_or_navigate_main_window(
    target: Option<Screen>,
    cx: &mut App,
) {
    let windows = cx.windows();
    let existing = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.main_window)
        .filter(|h| windows.contains(&(*h).into()));

    if let Some(handle) = existing {
        let _ = handle.update(cx, |app, window, cx| {
            if let Some(screen) = target {
                app.apply_target_screen(screen, cx);
            }
            let focus = app.focus.clone();
            window.focus(&focus, cx);
            window.activate_window();
        });
        enter_foreground_mode(cx);
        return;
    }

    let window = open_main_window(cx, None);
    if let Some(screen) = target {
        let _ = window.update(cx, |app, window, cx| {
            app.apply_target_screen(screen, cx);
            let focus = app.focus.clone();
            window.focus(&focus, cx);
            window.activate_window();
        });
    }
    with_runtime_mut(cx, |rt| rt.main_window = Some(window));
    enter_foreground_mode(cx);
}

pub(crate) fn update_open_main_window_checked<F>(cx: &mut App, f: F) -> bool
where
    F: FnOnce(&mut Ruum, &mut gpui_kit::Context<'_, Ruum>),
{
    let windows = cx.windows();
    let existing = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.main_window)
        .filter(|h| windows.contains(&(*h).into()))
        .or_else(|| windows.into_iter().find_map(|win| win.downcast::<Ruum>()));
    if let Some(handle) = existing {
        return handle.update(cx, |app, _, cx| f(app, cx)).is_ok();
    }
    false
}

fn update_open_main_window<F>(cx: &mut App, f: F)
where
    F: FnOnce(&mut Ruum, &mut gpui_kit::Context<'_, Ruum>),
{
    let _ = update_open_main_window_checked(cx, f);
}

pub(crate) fn sync_runtime_update_state(
    update_state: state::UpdateUiState,
    update_settings: ruum_core::storage::UpdateSettings,
    update_install_kind: ruum_core::updater::InstallKind,
    update_banner_dismissed_version: Option<String>,
    update_download_cancel: Option<Arc<AtomicBool>>,
    update_last_error: Option<String>,
    cx: &mut App,
) {
    with_runtime_mut(cx, |rt| {
        rt.update_state = update_state;
        rt.update_settings = update_settings;
        rt.update_install_kind = update_install_kind;
        rt.update_banner_dismissed_version = update_banner_dismissed_version;
        rt.update_download_cancel = update_download_cancel;
        rt.update_last_error = update_last_error;
    });
    refresh_tray_update_status(cx);
}

pub(crate) fn refresh_tray_update_status(cx: &App) {
    if let Some(rt) = cx.try_global::<AppRuntime>()
        && let Some(tray) = &rt.tray
    {
        let version = rt
            .update_state
            .available_version_if_unskipped(&rt.update_settings);
        tray.set_update_available(version);
    }
}

pub(crate) fn runtime_update_quitting(cx: &App) -> Option<Arc<AtomicBool>> {
    cx.try_global::<AppRuntime>()
        .map(|rt| Arc::clone(&rt.update_quitting))
}

pub(crate) fn cancel_update_tasks_on_quit(cx: &App) {
    if let Some(rt) = cx.try_global::<AppRuntime>() {
        rt.update_quitting.store(true, Ordering::Release);
        if let Some(cancel) = &rt.update_download_cancel {
            cancel.store(true, Ordering::Release);
        }
    }
}

fn trigger_check_for_updates_from_menu_or_tray(cx: &mut App) {
    open_or_navigate_main_window(None, cx);
    update_open_main_window(cx, |app, cx| {
        if app
            .update_state
            .should_open_modal_directly(&app.update_settings)
        {
            app.open_update_modal(cx);
        } else {
            app.navigate_rail(Screen::Settings, cx);
            app.set_settings_category(state::SettingsCategory::Updates, cx);
            app.check_for_updates_now(cx);
        }
    });
}

fn spawn_startup_update_and_sync(
    main_window: Option<WindowHandle<Ruum>>,
    install_kind: ruum_core::updater::InstallKind,
    quitting: Arc<AtomicBool>,
    cx: &App,
) {
    if cfg!(test) {
        return;
    }
    let weak_ruum = main_window
        .and_then(|win| win.entity(cx).ok().map(|entity| entity.downgrade()));
    let lang = state::locale_code(i18n::Locale::current());
    let (tx, rx) = channel();
    let signal = Arc::new(AsyncSignal::default());
    let thread_signal = Arc::clone(&signal);

    let _ = std::thread::Builder::new()
        .name("ruum-startup-update".to_owned())
        .spawn(move || {
            // [RISK-NOTE 7] Sync installed Windows CLI copy after updates and
            // retire any legacy headless `ruum watchdog` OS service residue.
            let _ = ruum_core::updater::sync_installed_windows_cli_if_needed();
            let _ = ruum_core::watchdog::uninstall_legacy_service_if_present(
                Duration::from_secs(2),
            );

            if ruum_core::updater::is_auto_check_disabled() {
                return;
            }

            for _ in 0..20 {
                if quitting.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if quitting.load(Ordering::Relaxed) {
                return;
            }

            let Ok(db) = StorageDb::open_default() else {
                return;
            };
            let settings = db.load_update_settings().unwrap_or_default();
            if !ruum_core::updater::should_auto_check(
                &settings,
                now_unix_secs(),
            ) {
                return;
            }
            let outcome = state::run_update_check_blocking(
                Some(db.db_path()),
                &settings,
                install_kind,
                lang,
            );
            if tx.send(outcome).is_ok() {
                thread_signal.notify();
            }
        });

    cx.spawn(async move |cx| {
        signal.wait().await;
        let Ok(outcome) = rx.try_recv() else {
            return;
        };
        cx.update(|cx| {
            if app_menu::is_quitting(cx) {
                return;
            }
            let fallback = outcome.clone();
            let applied_to_weak = weak_ruum.as_ref().is_some_and(|weak| {
                weak.update(cx, |app, cx| {
                    app.apply_update_check_outcome(outcome, cx);
                })
                .is_ok()
            });
            if !applied_to_weak {
                let updated_open =
                    update_open_main_window_checked(cx, |app, cx| {
                        app.apply_update_check_outcome(fallback.clone(), cx);
                    });
                if !updated_open {
                    state::apply_update_check_to_runtime(fallback, cx);
                }
            }
        });
    })
    .detach();
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
        handle_tray_action(action, cx);
    }
}

pub(crate) fn handle_tray_action(action: tray_menu::TrayAction, cx: &mut App) {
    match action {
        tray_menu::TrayAction::OpenRuum => {
            open_or_navigate_main_window(None, cx);
        }
        tray_menu::TrayAction::OpenRemote => {
            open_or_navigate_main_window(Some(Screen::Remote), cx);
        }
        tray_menu::TrayAction::OpenWatchdog => {
            open_or_navigate_main_window(Some(Screen::Watchdog), cx);
        }
        tray_menu::TrayAction::OpenSettings => {
            open_or_navigate_main_window(Some(Screen::Settings), cx);
        }
        tray_menu::TrayAction::CheckForUpdates => {
            trigger_check_for_updates_from_menu_or_tray(cx);
        }
        tray_menu::TrayAction::ToggleWatchdog => {
            toggle_watchdog_from_tray(cx);
        }
        tray_menu::TrayAction::RunAiAnalysisNow => {
            trigger_background_ai_analysis(cx);
        }
        tray_menu::TrayAction::ReviewLatestProposal => {
            review_latest_proposal_from_tray(cx);
        }
        tray_menu::TrayAction::ShowCleanupProgress => {
            open_cleanup_progress_window(cx);
        }
        tray_menu::TrayAction::Quit => {
            app_menu::request_quit(cx);
        }
    }
}

pub(crate) fn current_tray_cleanup_status(
    cx: &App,
) -> Option<tray_menu::TrayCleanupStatus> {
    let rt = cx.try_global::<AppRuntime>()?;
    let task = rt.active_cleanup.as_ref()?;
    let (completed, total, cancelling) = {
        let snap = task.progress.lock().ok()?;
        (snap.completed_items, snap.total_items, snap.cancelling)
    };
    Some(tray_menu::TrayCleanupStatus {
        completed,
        total,
        cancelling,
    })
}

fn dispatch_runtime_notification(
    tag: &str,
    title: &str,
    body: &str,
    cx: &mut App,
) {
    post_app_notification(tag, title, body, cx);
    if tag == ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS {
        update_open_main_window(cx, |app, cx| {
            app.reload_watchdog_state();
            app.push_toast(body.to_owned(), gpui_omarchy::Status::Error, cx);
        });
    }
}

pub(crate) fn poll_app_runtime(cx: &mut App) {
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
                let home_space = std::env::home_dir()
                    .as_deref()
                    .and_then(|h| ruum_core::space::space_info(h).ok());
                update_open_main_window(cx, |app, cx| {
                    app.reload_watchdog_state();
                    if let Some(hs) = home_space {
                        app.watchdog_home_space = Some(hs);
                    }
                    cx.notify();
                });
                refresh_tray_status_with_space(home_space, cx);
            }
            watchdog_runtime::WatchdogIpcEvent::ShowNotification {
                tag,
                title,
                body,
            } => {
                dispatch_runtime_notification(&tag, &title, &body, cx);
            }
            watchdog_runtime::WatchdogIpcEvent::OpenWindow { tag } => {
                activate_window_for_notification_tag(&tag, cx);
            }
            watchdog_runtime::WatchdogIpcEvent::ConfirmRemoteConnect {
                request,
                responder,
            } => {
                open_or_navigate_main_window(None, cx);
                update_open_main_window(cx, |app, cx| {
                    app.enqueue_remote_connect_confirm(
                        request,
                        Some(responder),
                        None,
                        cx,
                    );
                });
            }
        }
    }

    for (request, responder) in in_proc_requests {
        open_confirm_window(request, Some(responder), None, cx);
    }

    for (tag, title, body) in in_proc_notifs {
        dispatch_runtime_notification(&tag, &title, &body, cx);
    }

    maybe_run_in_app_watchdog_tick(cx);

    let should_refresh_space =
        cx.try_global::<AppRuntime>().is_some_and(|rt| {
            should_refresh_tray_disk_space(
                rt.tray.is_some(),
                rt.tray_show_disk_space,
                rt.last_tray_space_refresh,
                Instant::now(),
            )
        });
    if should_refresh_space {
        refresh_tray_status(cx);
    }
}

fn maybe_run_in_app_watchdog_tick(cx: &mut App) {
    let Some(rt) = cx.try_global::<AppRuntime>() else {
        return;
    };
    if rt.background_analysis_in_flight {
        return;
    }
    let cfg = &rt.watchdog_config;
    if !cfg.enabled {
        return;
    }
    let interval = Duration::from_secs(cfg.poll_interval_secs.max(1));
    if let Some(last) = rt.last_watchdog_poll
        && last.elapsed() < interval
    {
        return;
    }
    let confirm_tx = rt.in_proc_confirm_tx.clone();
    let notif_tx = rt.in_proc_notif_tx.clone();
    let exe = resolve_cli_exe();
    with_runtime_mut(cx, |rt| {
        rt.last_watchdog_poll = Some(Instant::now());
        rt.background_analysis_in_flight = true;
    });

    let task = cx.background_executor().spawn(async move {
        let runner = ruum_core::ai_cli::DefaultCommandRunner;
        if ruum_core::watchdog::has_legacy_service_residue(&runner)
            && matches!(
                ruum_core::watchdog::service_status(&runner),
                Ok(ruum_core::watchdog::ServiceStatus::Active)
            )
        {
            return None;
        }
        let Ok(db) = StorageDb::open_default() else {
            return None;
        };
        let home = std::env::home_dir()?;
        let now_secs = now_unix_secs();
        let adapter = InProcessConfirmAdapter { tx: confirm_tx };
        let notifier = InProcessNotificationAdapter { tx: notif_tx };
        let _ = ruum_core::watchdog::run_once_with_db(
            &db, &home, &exe, now_secs, &runner, &adapter, &notifier,
        );
        ruum_core::space::space_info(&home).ok()
    });

    cx.spawn(async move |cx| {
        let home_space = task.await;
        cx.update(|cx| {
            with_runtime_mut(cx, |rt| {
                rt.background_analysis_in_flight = false;
            });
            update_open_main_window(cx, |app, cx| {
                app.reload_watchdog_state();
                if let Some(hs) = home_space {
                    app.watchdog_home_space = Some(hs);
                }
                cx.notify();
            });
            refresh_tray_status_with_space(home_space, cx);
        });
    })
    .detach();
}

fn toggle_watchdog_from_tray(cx: &mut App) {
    toggle_watchdog_in_db(None, cx);
}

pub(crate) fn toggle_watchdog_in_db(
    db_path: Option<&std::path::Path>,
    cx: &mut App,
) {
    let db_opt = match db_path {
        Some(path) => StorageDb::open(path).ok(),
        None => StorageDb::open_default().ok(),
    };
    let prev_cfg = db_opt
        .as_ref()
        .and_then(|db| db.watchdog().ok())
        .and_then(|svc| svc.load_config().ok())
        .or_else(|| {
            cx.try_global::<AppRuntime>()
                .map(|rt| rt.watchdog_config.clone())
        })
        .unwrap_or_default();

    let prev_enabled = prev_cfg.enabled;
    let mut next_cfg = prev_cfg;
    next_cfg.enabled = !prev_enabled;
    next_cfg.updated_at_secs = now_unix_secs();
    let next_cfg = next_cfg.normalized();

    if let Some(ref db) = db_opt
        && let Ok(svc) = db.watchdog()
    {
        let _ = svc.save_config(&next_cfg);
        if !prev_enabled && next_cfg.enabled {
            let mut st = svc.load_state().unwrap_or_default();
            st.last_triggered_secs = None;
            st.last_triggered_threshold_gb = None;
            let _ = svc.save_state(&st);
        }
    }

    notify_watchdog_config_saved(&next_cfg, next_cfg.enabled, cx);
    update_open_main_window(cx, |app, cx| {
        app.watchdog_config = next_cfg.clone();
        app.watchdog_draft.enabled = next_cfg.enabled;
        app.reload_watchdog_state();
        cx.notify();
    });
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

    trigger_background_ai_analysis_with(
        None,
        None,
        i18n::Locale::current(),
        cx,
    );
}

pub(crate) fn trigger_background_ai_analysis_with(
    db_path: Option<PathBuf>,
    home_override: Option<PathBuf>,
    locale: i18n::Locale,
    cx: &mut App,
) {
    if let Some(rt) = cx.try_global::<AppRuntime>()
        && rt.background_analysis_in_flight
    {
        return;
    }
    let exe = resolve_cli_exe();
    let shared_cred_store = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.cred_store.clone());
    with_runtime_mut(cx, |rt| rt.background_analysis_in_flight = true);

    let task = cx.background_executor().spawn(async move {
        let home =
            home_override
                .or_else(std::env::home_dir)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "cannot find home directory",
                    )
                })?;
        let db = match db_path.as_deref() {
            Some(path) => {
                StorageDb::open(path).map_err(std::io::Error::other)?
            }
            None if !cfg!(test) => {
                StorageDb::open_default().map_err(std::io::Error::other)?
            }
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no test db_path configured",
                ));
            }
        };
        let ai_cfg = db.load_ai_config().unwrap_or_default();
        let now_secs = now_unix_secs();
        let runner = ruum_core::ai_cli::DefaultCommandRunner;
        let mut ai_ctx = ruum_core::ai_cli::AiExecutionContext::default()
            .with_db_path(db.db_path().to_path_buf());
        if let Some(store) = shared_cred_store {
            ai_ctx = ai_ctx.with_credential_store(store);
        }
        let proposal =
            match ruum_core::ai_cli::run_analysis_with_tree_for_config_with_context(
                &ai_cfg, &home, &exe, now_secs, None, &runner, &ai_ctx,
            ) {
                Ok(proposal) => proposal,
                Err(err) => {
                    let err_secs = now_unix_secs();
                    let summary = ruum_core::watchdog::format_ai_failure_summary(
                        &err, locale,
                    );
                    let _ = db.record_ai_error(&summary, err_secs);
                    return Err(std::io::Error::other(err));
                }
            };
        let done_secs = now_unix_secs();
        let ai_state =
            ruum_core::storage::AiState::from_proposal(&proposal, done_secs)
                .unwrap_or_default();
        let _ = db.save_ai_state(&ai_state);
        let resolved_db_path = db.db_path().to_path_buf();
        let docker = DockerFactory::create().probe(&DockerEnv::current());
        Ok::<_, std::io::Error>((home, proposal, docker, resolved_db_path))
    });

    cx.spawn(async move |cx| {
        let res = task.await;
        cx.update(|cx| {
            with_runtime_mut(cx, |rt| {
                rt.background_analysis_in_flight = false;
            });
            refresh_tray_status(cx);
            match res {
                Ok((home, proposal, docker, resolved_db_path)) => {
                    update_open_main_window(cx, |app, cx| {
                        app.reload_watchdog_state();
                        cx.notify();
                    });
                    let request = state::proposal_confirm_request_with_docker(
                        home, &proposal, &docker, locale,
                    );
                    if request.items.is_empty()
                        && request.docker_items.is_empty()
                    {
                        post_app_notification(
                            ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                            i18n::messages::ai_analysis_notif_title(locale),
                            i18n::messages::ai_analysis_no_reclaimable(locale),
                            cx,
                        );
                    } else {
                        open_local_confirm_window(
                            request,
                            Some(resolved_db_path),
                            docker,
                            cx,
                        );
                    }
                }
                Err(err) => {
                    let raw = err.to_string();
                    let kind = ruum_core::watchdog::classify_ai_failure(&raw);
                    let summary =
                        ruum_core::watchdog::format_ai_failure_summary(
                            &raw, locale,
                        );
                    post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                        i18n::messages::ai_analysis_failure_notif_title(
                            kind, locale,
                        ),
                        &summary,
                        cx,
                    );
                    update_open_main_window(cx, |app, cx| {
                        app.reload_watchdog_state();
                        app.push_toast(
                            summary.clone(),
                            gpui_omarchy::Status::Error,
                            cx,
                        );
                    });
                }
            }
        });
    })
    .detach();
}

fn review_latest_proposal_from_tray(cx: &mut App) {
    let db = StorageDb::open_default().ok();
    let db_path = db.as_ref().map(|d| d.db_path().to_path_buf());
    let proposal = db
        .and_then(|db| db.load_ai_state().ok())
        .and_then(|s| s.parse_proposal().ok().flatten());
    let Some(prop) = proposal
        .filter(|p| !p.items.is_empty() || !p.docker_targets.is_empty())
    else {
        open_or_navigate_main_window(Some(Screen::Watchdog), cx);
        return;
    };
    // Fail closed when no user home directory exists rather than widening
    // cleanup root checks to `/`.
    let Some(home) = std::env::home_dir() else {
        open_or_navigate_main_window(Some(Screen::Watchdog), cx);
        return;
    };
    let docker = DockerFactory::create().probe(&DockerEnv::current());
    let request = state::proposal_confirm_request_with_docker(
        home,
        &prop,
        &docker,
        i18n::Locale::current(),
    );
    if request.items.is_empty() && request.docker_items.is_empty() {
        open_or_navigate_main_window(Some(Screen::Watchdog), cx);
        return;
    }
    open_local_confirm_window(request, db_path, docker, cx);
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Debug, PartialEq, Eq)]
struct ConfirmDialogArgs {
    req_path: PathBuf,
    resp_path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
struct ShowNotificationArgs {
    tag: String,
    title: String,
    body: String,
}

#[derive(Debug, PartialEq, Eq)]
enum InternalGuiCommand {
    ConfirmCleanup(ConfirmDialogArgs),
    ConfirmRemoteConnect(ConfirmDialogArgs),
    ShowNotification(ShowNotificationArgs),
}

fn parse_internal_gui_command<I, S>(
    args: I,
) -> Result<Option<InternalGuiCommand>>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let items: Vec<std::ffi::OsString> =
        args.into_iter().map(Into::into).collect();
    if items.is_empty() {
        return Ok(None);
    }

    let mut confirm_cleanup = false;
    let mut confirm_remote = false;
    let mut show_notif = false;
    let mut req_path: Option<PathBuf> = None;
    let mut resp_path: Option<PathBuf> = None;
    let mut tag: Option<String> = None;
    let mut title: Option<String> = None;
    let mut body: Option<String> = None;

    let mut iter = items.into_iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--confirm-cleanup") => {
                confirm_cleanup = true;
                req_path = iter.next().map(PathBuf::from);
            }
            Some("--confirm-remote-connect") => {
                confirm_remote = true;
                req_path = iter.next().map(PathBuf::from);
            }
            Some("--response") => {
                resp_path = iter.next().map(PathBuf::from);
            }
            Some("--show-notification") => {
                show_notif = true;
            }
            Some("--tag") if show_notif => {
                tag = iter.next().and_then(|v| v.into_string().ok());
            }
            Some("--title") if show_notif => {
                title = iter.next().and_then(|v| v.into_string().ok());
            }
            Some("--body") if show_notif => {
                body = iter.next().and_then(|v| v.into_string().ok());
            }
            _ => {}
        }
    }

    if confirm_cleanup && confirm_remote {
        anyhow::bail!(
            "cannot combine --confirm-cleanup and --confirm-remote-connect"
        );
    }
    if confirm_cleanup {
        let Some((req, resp)) = req_path.zip(resp_path) else {
            anyhow::bail!(
                "--confirm-cleanup requires both request and --response paths"
            );
        };
        return Ok(Some(InternalGuiCommand::ConfirmCleanup(
            ConfirmDialogArgs {
                req_path: req,
                resp_path: resp,
            },
        )));
    }
    if confirm_remote {
        let Some((req, resp)) = req_path.zip(resp_path) else {
            anyhow::bail!(
                "--confirm-remote-connect requires both request and \
                 --response paths"
            );
        };
        return Ok(Some(InternalGuiCommand::ConfirmRemoteConnect(
            ConfirmDialogArgs {
                req_path: req,
                resp_path: resp,
            },
        )));
    }
    if show_notif {
        let Some(title) = title.filter(|s| !s.is_empty()) else {
            anyhow::bail!("--show-notification requires --title");
        };
        return Ok(Some(InternalGuiCommand::ShowNotification(
            ShowNotificationArgs {
                tag: tag
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "ruum.notification".to_owned()),
                title,
                body: body.unwrap_or_default(),
            },
        )));
    }

    Ok(None)
}

fn resolve_native_look_from_db() -> bool {
    StorageDb::open_default()
        .ok()
        .and_then(|db| db.load_app_settings().ok())
        .map_or_else(
            || {
                let home = std::env::home_dir();
                appearance::follows_system(home.as_deref())
            },
            |s| s.follow_system_appearance,
        )
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
            install_notification_handler(cx);
            app_menu::install_for(i18n::Locale::current(), cx);
            app_menu::set_confirm_only(cx, true);
            if resolve_native_look_from_db() {
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

fn run_remote_connect_confirm_dialog(args: ConfirmDialogArgs) -> Result<()> {
    let req_content = std::fs::read_to_string(&args.req_path)
        .context("read remote connect confirm request file")?;
    let request: RemoteConnectConfirmRequest =
        serde_json::from_str(&req_content)
            .context("parse remote connect confirm request JSON")?;

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_omarchy::init(cx);
            app_menu::install_for(i18n::Locale::current(), cx);
            app_menu::set_confirm_only(cx, true);
            if resolve_native_look_from_db() {
                appearance::apply(cx.window_appearance(), cx);
            }
            let _ = open_remote_connect_confirm_window_with_mode(
                request,
                None,
                Some(args.resp_path),
                true,
                cx,
            );
        });
    Ok(())
}

fn run_show_notification(args: ShowNotificationArgs) {
    if notifications::has_native_notification_identity() {
        let tag = args.tag;
        let title = args.title;
        let body = args.body;
        gpui_kit::application()
            .with_assets(gpui_kit::assets::Assets)
            .run(move |cx| {
                install_notification_handler(cx);
                post_app_notification(&tag, &title, &body, cx);
                cx.quit();
            });
    } else {
        let _ = ruum_core::watchdog::send_fallback_os_notification(
            &args.title,
            &args.body,
        );
    }
}

/// Read GUI launch arguments, program name already skipped.
#[cfg(test)]
pub(crate) fn parse_gui_args(
    args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<GuiArgs> {
    parse_gui_args_with_power(args, power::PowerEfficiency::default())
}

fn parse_open_screen(value: &str) -> Result<Screen> {
    match value {
        "explore" => Ok(Screen::Explore),
        "docker" => Ok(Screen::Docker),
        "remote" | "sources" => Ok(Screen::Remote),
        "watchdog" => Ok(Screen::Watchdog),
        "settings" => Ok(Screen::Settings),
        other => anyhow::bail!(
            "unknown screen {other}; try explore, docker, remote, watchdog, \
             or settings"
        ),
    }
}

fn parse_gui_args_with_power(
    args: impl Iterator<Item = std::ffi::OsString>,
    preset: power::PowerEfficiency,
) -> Result<GuiArgs> {
    let mut root: Option<PathBuf> = None;
    let options = ScanOptions {
        threads: preset.policy(power::cpu_threads()),
        ..ScanOptions::default()
    };
    let mut initial_screen = None;
    let mut background = false;
    let text = |value: Option<std::ffi::OsString>, need: &str| {
        value
            .and_then(|value| value.into_string().ok())
            .with_context(|| need.to_owned())
    };

    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.to_str().unwrap_or_default() {
            "--background" => {
                background = true;
            }
            "--open-screen" => {
                let value = text(
                    args.next(),
                    "--open-screen needs a screen (explore, docker, remote, \
                     watchdog, settings)",
                )?;
                initial_screen = Some(parse_open_screen(&value)?);
            }
            // Launch Services added a process serial number when opening an
            // app from Finder until OS X 10.9, and some launchers still do.
            other if other.starts_with("-psn_") => {}
            other if other.starts_with('-') => {
                anyhow::bail!("unknown option {other} for ruum-gui");
            }
            _ => {
                anyhow::ensure!(root.is_none(), "only one path can be scanned");
                root = Some(PathBuf::from(arg));
            }
        }
    }

    let home = std::env::home_dir();
    let root = match root {
        Some(root) => root,
        None => home.context("no path given and no home directory")?,
    };
    let root = dunce::canonicalize(&root).unwrap_or(root);
    let metadata = std::fs::metadata(&root)
        .with_context(|| format!("cannot read {}", root.display()))?;
    anyhow::ensure!(metadata.is_dir(), "{} is not a directory", root.display());

    Ok(GuiArgs {
        root,
        options,
        depth: 3,
        power: Some(preset),
        initial_screen,
        background,
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
    fn default_gui_args_resolve_home_and_ignore_macos_psn() {
        let parsed = parse_gui_args(
            std::iter::once("-psn_0_12345").map(std::ffi::OsString::from),
        )
        .expect("defaults with psn");
        let expected_home = dunce::canonicalize(std::env::home_dir().unwrap())
            .unwrap_or_else(|_| std::env::home_dir().unwrap());
        assert_eq!(parsed.root, expected_home);
        assert_eq!(parsed.depth, 3);
        assert_eq!(parsed.initial_screen, None);
        assert!(!parsed.background);
    }

    #[test]
    fn background_flag_sets_background_launch_mode() {
        let parsed = parse_gui_args(
            std::iter::once("--background").map(std::ffi::OsString::from),
        )
        .expect("parses --background");
        assert!(parsed.background);
        assert_eq!(parsed.initial_screen, None);

        let with_screen = parse_gui_args(arguments(&[
            "--background",
            "--open-screen",
            "settings",
        ]))
        .expect("parses --background with screen");
        assert!(with_screen.background);
        assert_eq!(with_screen.initial_screen, Some(Screen::Settings));
    }

    #[test]
    fn gui_args_reject_business_cli_flags_and_multiple_paths() {
        for bad in [
            vec!["--unknown-flag"],
            vec!["--scan-threads", "4", "."],
            vec!["--power-efficiency", "miser", "."],
            vec![".", ".."],
        ] {
            assert!(
                parse_gui_args(bad.into_iter().map(std::ffi::OsString::from))
                    .is_err()
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gui_args_preserve_non_unicode_root() {
        use std::os::unix::ffi::OsStringExt as _;

        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'r', 0xff]));
        std::fs::create_dir(&root).expect("directory");
        let args =
            parse_gui_args(std::iter::once(root.clone().into_os_string()))
                .expect("native path");
        assert_eq!(args.root, dunce::canonicalize(root).expect("root"));
    }

    #[test]
    fn internal_gui_commands_parse_confirm_and_notification_flags() {
        assert_eq!(
            parse_internal_gui_command(Vec::<&str>::new()).unwrap(),
            None
        );
        assert_eq!(parse_internal_gui_command(["."]).unwrap(), None);
        assert!(
            parse_internal_gui_command(["--confirm-cleanup", "req.json"])
                .is_err()
        );
        assert!(
            parse_internal_gui_command([
                "--confirm-remote-connect",
                "req.json"
            ])
            .is_err()
        );
        assert!(parse_internal_gui_command(["--show-notification"]).is_err());

        assert_eq!(
            parse_internal_gui_command([
                "--confirm-cleanup",
                "req.json",
                "--response",
                "resp.json",
            ])
            .unwrap(),
            Some(InternalGuiCommand::ConfirmCleanup(ConfirmDialogArgs {
                req_path: PathBuf::from("req.json"),
                resp_path: PathBuf::from("resp.json"),
            }))
        );
        assert_eq!(
            parse_internal_gui_command([
                "--confirm-remote-connect",
                "rc_req.json",
                "--response",
                "rc_resp.json",
            ])
            .unwrap(),
            Some(InternalGuiCommand::ConfirmRemoteConnect(
                ConfirmDialogArgs {
                    req_path: PathBuf::from("rc_req.json"),
                    resp_path: PathBuf::from("rc_resp.json"),
                }
            ))
        );
        assert_eq!(
            parse_internal_gui_command([
                "--show-notification",
                "--tag",
                "ruum.low_disk_space",
                "--title",
                "Low Disk",
                "--body",
                "Only 1 GB left",
            ])
            .unwrap(),
            Some(InternalGuiCommand::ShowNotification(ShowNotificationArgs {
                tag: "ruum.low_disk_space".to_owned(),
                title: "Low Disk".to_owned(),
                body: "Only 1 GB left".to_owned(),
            }))
        );
    }

    #[test]
    fn open_screen_parses_valid_screens_and_rejects_invalid() {
        assert_eq!(
            parse_gui_args(arguments(&[])).unwrap().initial_screen,
            None
        );
        assert_eq!(
            parse_gui_args(arguments(&["--open-screen", "explore"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Explore)
        );
        assert_eq!(
            parse_gui_args(arguments(&["--open-screen", "docker"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Docker)
        );
        assert_eq!(
            parse_gui_args(arguments(&["--open-screen", "remote"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Remote)
        );
        assert_eq!(
            parse_gui_args(arguments(&["--open-screen", "sources"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Remote)
        );
        assert_eq!(
            parse_gui_args(arguments(&["--open-screen", "watchdog"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Watchdog)
        );
        assert_eq!(
            parse_gui_args(arguments(&["--open-screen", "settings"]))
                .unwrap()
                .initial_screen,
            Some(Screen::Settings)
        );
        assert!(
            parse_gui_args(arguments(&["--open-screen", "unknown"])).is_err()
        );
        assert!(parse_gui_args(arguments(&["--open-screen"])).is_err());
    }

    #[test]
    fn notification_tag_maps_to_expected_target_screen() {
        use ruum_core::watchdog::{
            NOTIF_TAG_AI_ANALYSIS, NOTIF_TAG_CLEANUP_DONE,
            NOTIF_TAG_CLEANUP_STARTED, NOTIF_TAG_LOW_SPACE,
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
            target_screen_for_notification_tag(screen_open_tag(
                Screen::Explore
            )),
            Some(Screen::Explore)
        );
        assert_eq!(
            target_screen_for_notification_tag(screen_open_tag(Screen::Docker)),
            Some(Screen::Docker)
        );
        assert_eq!(
            target_screen_for_notification_tag(screen_open_tag(Screen::Remote)),
            Some(Screen::Remote)
        );
        assert_eq!(
            target_screen_for_notification_tag(screen_open_tag(
                Screen::Watchdog
            )),
            Some(Screen::Watchdog)
        );
        assert_eq!(
            target_screen_for_notification_tag(screen_open_tag(
                Screen::Settings
            )),
            Some(Screen::Settings)
        );
        assert_eq!(
            target_screen_for_notification_tag(NOTIF_TAG_CLEANUP_STARTED),
            None
        );
        assert_eq!(
            target_screen_for_notification_tag(NOTIF_TAG_CLEANUP_DONE),
            None
        );
        assert_eq!(target_screen_for_notification_tag("ruum.other"), None);
    }

    #[test]
    fn ruum_assets_provides_container_and_default_icons() {
        use gpui_kit::assets::IconName;

        assert!(
            RuumAssets
                .load(IconName::Container.path().as_ref())
                .expect("container icon lookup")
                .is_some()
        );
        assert!(
            RuumAssets
                .load(IconName::Download.path().as_ref())
                .expect("download icon lookup")
                .is_some()
        );
        assert!(
            RuumAssets
                .load(IconName::RefreshCw.path().as_ref())
                .expect("refresh-cw icon lookup")
                .is_some()
        );
        assert!(
            RuumAssets
                .load(IconName::Folder.path().as_ref())
                .expect("default folder icon lookup")
                .is_some()
        );
        assert!(
            RuumAssets
                .load(IconName::Bot.path().as_ref())
                .expect("bot icon lookup")
                .is_some()
        );
    }
}
