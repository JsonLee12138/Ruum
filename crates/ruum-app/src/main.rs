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

    let depth = args.depth;
    let title_root = args.root.clone();
    let home_at_boot = std::env::home_dir();
    let native_look = if db_opt.is_some() {
        app_settings.follow_system_appearance
    } else {
        appearance::follows_system(home_at_boot.as_deref())
    };
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
            remote_connect_window: None,
            tray: None,
            ipc_rx,
            ipc_server,
            in_proc_confirm_tx: in_proc_tx,
            in_proc_confirm_rx: in_proc_rx,
            in_proc_notif_tx,
            in_proc_notif_rx,
            last_watchdog_poll: None,
            background_analysis_in_flight: false,
            service_sync_in_flight: false,
            watchdog_config: initial_watchdog_config.clone(),
            update_state: state::UpdateUiState::Idle,
            update_settings: initial_update_settings,
            update_install_kind: initial_install_kind,
            update_banner_dismissed_version: None,
            update_download_cancel: None,
            update_last_error: None,
            update_quitting: Arc::clone(&update_quitting),
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

        let window = open_main_window(cx, Some(title_root));
        with_runtime_mut(cx, |rt| rt.main_window = Some(window));
        if !cfg!(test) {
            spawn_startup_update_and_sync(
                window,
                initial_watchdog_config.background_service,
                initial_install_kind,
                update_quitting,
                cx,
            );
        }

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
                    refresh_tray_update_status(cx);
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
        self.handled = true;
        // Read before calling `modal.confirm()`, which consumes `responder`.
        let is_in_app = self.modal.responder.is_none()
            && self.modal.response_file.is_none();
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
        let root = self.modal.request.root.clone();
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
        let task = cx.background_executor().spawn(async move {
            let (mut total_bytes, mut total_count, mut pruned_paths) =
                if approved_items.is_empty() {
                    (0_u64, 0_usize, Vec::new())
                } else {
                    ruum_core::watchdog::execute_confirmed_cleanup_tracked(
                        &approved_items,
                        &root,
                    )
                };
            for item in &approved_items {
                if !item.path.exists() && !pruned_paths.contains(&item.path) {
                    pruned_paths.push(item.path.clone());
                }
            }
            if !pruned_paths.is_empty()
                && let Some(path) = db_path.as_deref()
                && let Ok(db) = StorageDb::open(path)
            {
                let _ = db.prune_ai_state_paths(&pruned_paths, now_unix_secs());
            }
            let mut docker_err: Option<String> = None;
            let fresh_docker = if selected_docker_targets.is_empty() {
                None
            } else {
                let adapter = DockerFactory::create();
                let env = DockerEnv::current();
                if let DockerProbeStatus::Available(summary) = docker_status {
                    let outcomes = adapter.run_clean_targets(
                        &env,
                        &summary,
                        &selected_docker_targets,
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
                } else {
                    docker_err =
                        Some("Docker summary unavailable for cleanup".into());
                }
                Some(adapter.probe(&env))
            };
            let fresh_space = ruum_core::space::space_info(&root).ok();
            (
                total_bytes,
                total_count,
                pruned_paths,
                fresh_space,
                fresh_docker,
                docker_err,
            )
        });
        cx.spawn(async move |_, cx| {
            let (
                bytes,
                count,
                pruned_paths,
                fresh_space,
                fresh_docker,
                docker_err,
            ) = task.await;
            let summary =
                i18n::messages::cleaned_items_summary(count, bytes, locale);
            cx.update(|cx| {
                if count > 0 {
                    post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_CLEANUP_DONE,
                        i18n::messages::cleanup_complete_notif_title(locale),
                        &summary,
                        cx,
                    );
                }
                refresh_tray_status(cx);
                update_open_main_window(cx, |app, cx| {
                    app.prune_local_ai_proposal(&pruned_paths);
                    if let Some(space) = fresh_space {
                        app.space = Some(space);
                    }
                    if let Some(docker) = fresh_docker {
                        app.apply_local_docker_status(docker);
                    }
                    app.reload_watchdog_state();
                    app.notice = Some(
                        if let Some(err) = docker_err
                            && count == 0
                        {
                            (err, gpui_omarchy::Status::Error)
                        } else {
                            (
                                i18n::messages::cleaned_items_summary(
                                    count, bytes, app.locale,
                                ),
                                gpui_omarchy::Status::Success,
                            )
                        },
                    );
                    cx.notify();
                });
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
    cx.activate(true);

    with_runtime_mut(cx, |rt| rt.confirm_window = Some(window));
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
    cx.activate(true);

    with_runtime_mut(cx, |rt| rt.remote_connect_window = Some(window));
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
    let watchdog_enabled = rt.watchdog_config.enabled;
    let home = std::env::home_dir();
    let free_bytes = home
        .as_deref()
        .and_then(|h| ruum_core::space::space_info(h).ok())
        .map(|s| s.available);
    let has_proposal = StorageDb::open_default()
        .ok()
        .and_then(|db| db.load_ai_state().ok())
        .and_then(|s| s.parse_proposal().ok().flatten())
        .is_some_and(|p| !p.items.is_empty());

    if let Some(rt) = cx.try_global::<AppRuntime>()
        && let Some(tray) = &rt.tray
    {
        tray.update_status(free_bytes, watchdog_enabled, has_proposal);
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
pub(crate) struct AppRuntime {
    launch: LaunchConfig,
    pub(crate) cred_store: Option<ruum_core::credentials::LazyCredentialStore>,
    pub(crate) main_window: Option<WindowHandle<Ruum>>,
    confirm_window: Option<WindowHandle<CleanupConfirmWindow>>,
    remote_connect_window: Option<WindowHandle<RemoteConnectConfirmWindow>>,
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
    pub(crate) service_sync_in_flight: bool,
    watchdog_config: ruum_watchdog::WatchdogConfig,
    pub(crate) update_state: state::UpdateUiState,
    pub(crate) update_settings: ruum_core::storage::UpdateSettings,
    pub(crate) update_install_kind: ruum_core::updater::InstallKind,
    pub(crate) update_banner_dismissed_version: Option<String>,
    pub(crate) update_download_cancel: Option<Arc<AtomicBool>>,
    pub(crate) update_last_error: Option<String>,
    pub(crate) update_quitting: Arc<AtomicBool>,
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
        remote_connect_window: None,
        tray: None,
        ipc_rx: None,
        ipc_server: None,
        in_proc_confirm_tx,
        in_proc_confirm_rx,
        in_proc_notif_tx,
        in_proc_notif_rx,
        last_watchdog_poll: None,
        background_analysis_in_flight: false,
        service_sync_in_flight: false,
        watchdog_config: ruum_watchdog::WatchdogConfig::default(),
        update_state: state::UpdateUiState::Idle,
        update_settings: ruum_core::storage::UpdateSettings::default(),
        update_install_kind: ruum_core::updater::InstallKind::Development,
        update_banner_dismissed_version: None,
        update_download_cancel: None,
        update_last_error: None,
        update_quitting: Arc::new(AtomicBool::new(false)),
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
        cx.activate(true);
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
    cx.activate(true);
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
    main_window: WindowHandle<Ruum>,
    watchdog_bg_service: bool,
    install_kind: ruum_core::updater::InstallKind,
    quitting: Arc<AtomicBool>,
    cx: &App,
) {
    if cfg!(test) {
        return;
    }
    let weak_ruum =
        main_window.entity(cx).ok().map(|entity| entity.downgrade());
    let cli_exe = resolve_cli_exe();
    let lang = state::locale_code(i18n::Locale::current());
    let (tx, rx) = channel();
    let signal = Arc::new(AsyncSignal::default());
    let thread_signal = Arc::clone(&signal);

    let _ = std::thread::Builder::new()
        .name("ruum-startup-update".to_owned())
        .spawn(move || {
            // [RISK-NOTE 7] Sync installed Windows CLI copy after updates and
            // keep background watchdog service running when enabled.
            let _ = ruum_core::updater::sync_installed_windows_cli_if_needed();
            if watchdog_bg_service {
                let runner = ruum_core::ai_cli::TimedCommandRunner::new(
                    Duration::from_secs(5),
                );
                if !matches!(
                    ruum_core::watchdog::service_status(&runner),
                    Ok(ruum_core::watchdog::ServiceStatus::Active)
                ) {
                    let _ =
                        ruum_core::watchdog::install_service(&runner, &cli_exe);
                }
            }

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
        post_app_notification(&tag, &title, &body, cx);
    }

    maybe_run_in_app_watchdog_tick(cx);
}

fn maybe_run_in_app_watchdog_tick(cx: &mut App) {
    let Some(rt) = cx.try_global::<AppRuntime>() else {
        return;
    };
    if rt.background_analysis_in_flight || rt.service_sync_in_flight {
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
        if matches!(
            ruum_core::watchdog::service_status(&runner),
            Ok(ruum_core::watchdog::ServiceStatus::Active)
        ) {
            return;
        }
        let Ok(db) = StorageDb::open_default() else {
            return;
        };
        let Some(home) = std::env::home_dir() else {
            return;
        };
        let now_secs = now_unix_secs();
        let adapter = InProcessConfirmAdapter { tx: confirm_tx };
        let notifier = InProcessNotificationAdapter { tx: notif_tx };
        let _ = ruum_core::watchdog::run_once_with_db(
            &db, &home, &exe, now_secs, &runner, &adapter, &notifier,
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
        if let Ok(db) = StorageDb::open_default()
            && let Ok(svc) = db.watchdog()
        {
            let mut st = svc.load_state().unwrap_or_default();
            st.last_checked_secs = Some(now_secs);
            let _ = svc.save_state(&st);
        }
        Some((info, now_secs))
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            if let Some((info, checked_at)) = result {
                update_open_main_window(cx, |app, cx| {
                    app.watchdog_home_space = Some(info);
                    app.reload_watchdog_state();
                    app.watchdog_space_checked_at = Some(checked_at);
                    let msg = i18n::messages::disk_check_available(
                        info.available,
                        app.locale,
                    );
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
    let exe = resolve_cli_exe();
    let shared_cred_store = cx
        .try_global::<AppRuntime>()
        .and_then(|rt| rt.cred_store.clone());
    with_runtime_mut(cx, |rt| rt.background_analysis_in_flight = true);

    let task = cx.background_executor().spawn(async move {
        let home = std::env::home_dir().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "cannot find home directory",
            )
        })?;
        let db = StorageDb::open_default().map_err(std::io::Error::other)?;
        let ai_cfg = db.load_ai_config().unwrap_or_default();
        let now_secs = now_unix_secs();
        let runner = ruum_core::ai_cli::DefaultCommandRunner;
        let mut ai_ctx = ruum_core::ai_cli::AiExecutionContext::default();
        if let Some(store) = shared_cred_store {
            ai_ctx = ai_ctx.with_credential_store(store);
        }
        let proposal =
            ruum_core::ai_cli::run_analysis_with_tree_for_config_with_context(
                &ai_cfg, &home, &exe, now_secs, None, &runner, &ai_ctx,
            )
            .map_err(std::io::Error::other)?;
        let ai_state =
            ruum_core::storage::AiState::from_proposal(&proposal, now_secs)
                .unwrap_or_default();
        let _ = db.save_ai_state(&ai_state);
        let docker = DockerFactory::create().probe(&DockerEnv::current());
        Ok::<_, std::io::Error>((home, proposal, docker))
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
                Ok((home, proposal, docker)) => {
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
                        let db_path = StorageDb::open_default()
                            .ok()
                            .map(|db| db.db_path().to_path_buf());
                        open_local_confirm_window(request, db_path, docker, cx);
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
    let text = |value: Option<std::ffi::OsString>, need: &str| {
        value
            .and_then(|value| value.into_string().ok())
            .with_context(|| need.to_owned())
    };

    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.to_str().unwrap_or_default() {
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
    }
}
