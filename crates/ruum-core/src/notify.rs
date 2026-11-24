//! Tri-platform core OS desktop notification adapters.
//!
//! Prefers delivering notifications over local IPC to an active `ruum-gui`
//! instance (`IpcRequest::ShowNotification`). When no GUI listener is active,
//! falls back to safe, non-blocking OS utilities (`osascript` on macOS,
//! `notify-send` on Linux, and hidden `powershell.exe` Toast on Windows with
//! `CREATE_NO_WINDOW`) without depending on GPUI or `unsafe`.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::ai_cli::{CommandRunner, DefaultCommandRunner};
use crate::confirm::{
    IpcRequest, IpcResponse, load_or_create_ipc_token, send_ipc_message,
};
use crate::watchdog_config::{config_dir, ipc_token_path};

/// Canonical desktop application / bundle / AUMID identifier.
pub const RUUM_APP_ID: &str = "io.github.tobi.ruum";

/// Canonical short display name used for desktop notifications.
pub const RUUM_APP_NAME: &str = "ruum";

const EMBEDDED_RUUM_ICO: &[u8] = include_bytes!("../../../assets/ruum.ico");
const EMBEDDED_RUUM_SVG: &[u8] = include_bytes!("../../../assets/ruum.svg");

const MACOS_LSREGISTER_PATH: &str = "/System/Library/Frameworks/\
    CoreServices.framework/Frameworks/LaunchServices.framework/Support/\
    lsregister";
const WINDOWS_AUMID_REG_KEY: &str =
    r"HKCU\Software\Classes\AppUserModelId\io.github.tobi.ruum";
const WINDOWS_ICON_BACKGROUND_COLOR: &str = "FF151821";
const ICON_STAMP_FILE: &str = "notification-icon.stamp";
const WINDOWS_ICON_PREFIX: &str = "ruum-icon-";

/// Notification tag for low disk space alerts.
pub const NOTIF_TAG_LOW_SPACE: &str = "ruum.low_disk_space";

/// Notification tag when background cleanup starts after confirmation.
pub const NOTIF_TAG_CLEANUP_STARTED: &str = "ruum.cleanup_started";

/// Notification tag for completed cleanup summaries.
pub const NOTIF_TAG_CLEANUP_DONE: &str = "ruum.cleanup_complete";

/// Notification tag for AI analysis completion / proposals.
pub const NOTIF_TAG_AI_ANALYSIS: &str = "ruum.ai_analysis";

/// Environment variable disabling fallback OS notification commands when `"1"`.
pub const ENV_DISABLE_OS_NOTIFICATIONS: &str = "RUUM_DISABLE_OS_NOTIFICATIONS";

const MACOS_OSASCRIPT_NOTIFY: &str = "display notification \
    (system attribute \"RUUM_NOTIF_BODY\") \
    with title (system attribute \"RUUM_NOTIF_TITLE\")";

const WINDOWS_TOAST_SCRIPT: &str = "$ErrorActionPreference = 'Stop'; \
    [Windows.UI.Notifications.ToastNotificationManager, \
    Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
    [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, \
    ContentType = WindowsRuntime] | Out-Null; \
    $t = [System.Security.SecurityElement]::Escape($env:RUUM_NOTIF_TITLE); \
    $b = [System.Security.SecurityElement]::Escape($env:RUUM_NOTIF_BODY); \
    $xml = New-Object Windows.Data.Xml.Dom.XmlDocument; \
    $xml.LoadXml(\"<toast><visual><binding template=`\"ToastGeneric`\">\
    <text>$t</text><text>$b</text></binding></visual></toast>\"); \
    $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
    [Windows.UI.Notifications.ToastNotificationManager]\
    ::CreateToastNotifier('io.github.tobi.ruum').Show($toast)";

/// Operating system family for desktop notification dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationOs {
    MacOs,
    Linux,
    Windows,
}

fn os_notifications_disabled() -> bool {
    std::env::var(ENV_DISABLE_OS_NOTIFICATIONS).is_ok_and(|v| v.trim() == "1")
}

/// Captured IPC and environment state used by notification adapters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationEnv {
    pub ipc_target: Option<PathBuf>,
    pub ipc_token: Option<String>,
    pub suppress_os_fallback: bool,
}

impl NotificationEnv {
    /// Capture current local IPC socket/port and authentication token.
    #[must_use]
    pub fn current() -> Self {
        let suppress_os_fallback = os_notifications_disabled();
        let Some((target, token)) = resolve_ipc_target_and_token() else {
            return Self {
                ipc_target: None,
                ipc_token: None,
                suppress_os_fallback,
            };
        };
        Self {
            ipc_target: Some(target),
            ipc_token: Some(token),
            suppress_os_fallback,
        }
    }

    /// Construct an environment with an explicit IPC target and token.
    #[must_use]
    pub const fn with_ipc(target: PathBuf, token: String) -> Self {
        Self {
            ipc_target: Some(target),
            ipc_token: Some(token),
            suppress_os_fallback: false,
        }
    }

    /// Construct an environment with IPC disabled (forces fallback delivery).
    #[must_use]
    pub const fn without_ipc() -> Self {
        Self {
            ipc_target: None,
            ipc_token: None,
            suppress_os_fallback: false,
        }
    }
}

/// Captured filesystem and environment inputs used when synchronizing
/// OS-level desktop notification icon registration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationIconEnv {
    pub app_data_dir: Option<PathBuf>,
    pub current_exe: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
}

impl NotificationIconEnv {
    /// Capture current process environment inputs for notification icon sync.
    #[must_use]
    pub fn current() -> Self {
        Self {
            app_data_dir: config_dir(),
            current_exe: std::env::current_exe().ok(),
            home: std::env::home_dir(),
            xdg_data_home: std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        }
    }
}

/// Abstract adapter for sending temporary OS notifications.
pub trait NotificationAdapter: Send + Sync {
    /// Target OS family handled by this adapter.
    fn os_kind(&self) -> NotificationOs {
        NotificationFactory::current_os()
    }

    /// Send a tagged notification via IPC first, falling back to OS utilities.
    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()>;

    /// Send a notification tagged with [`NOTIF_TAG_LOW_SPACE`].
    fn notify(&self, title: &str, body: &str) -> io::Result<()> {
        self.notify_tagged(NOTIF_TAG_LOW_SPACE, title, body)
    }

    /// Send a tagged notification using injected environment and command runner.
    fn notify_with_env(
        &self,
        _env: &NotificationEnv,
        _runner: &dyn CommandRunner,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        self.notify_tagged(tag, title, body)
    }

    /// Synchronize OS-level notification icon registration and clear stale
    /// caches if needed. Returns `Ok(true)` when registration or icon files
    /// were updated.
    fn sync_notification_icon_with_env(
        &self,
        _env: &NotificationIconEnv,
        _runner: &dyn CommandRunner,
    ) -> io::Result<bool> {
        Ok(false)
    }
}

/// macOS notification adapter (`ruum-gui` IPC -> `osascript`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsNotificationAdapter;

impl NotificationAdapter for MacOsNotificationAdapter {
    fn os_kind(&self) -> NotificationOs {
        NotificationOs::MacOs
    }

    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        self.notify_with_env(
            &NotificationEnv::current(),
            &DefaultCommandRunner,
            tag,
            title,
            body,
        )
    }

    fn notify_with_env(
        &self,
        env: &NotificationEnv,
        runner: &dyn CommandRunner,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        if try_env_ipc_notification(env, tag, title, body) {
            return Ok(());
        }
        if env.suppress_os_fallback {
            return Ok(());
        }
        dispatch_osascript_notification(runner, title, body)
    }

    fn sync_notification_icon_with_env(
        &self,
        env: &NotificationIconEnv,
        runner: &dyn CommandRunner,
    ) -> io::Result<bool> {
        sync_macos_notification_icon(env, runner)
    }
}

/// Linux notification adapter (`ruum-gui` IPC -> `notify-send`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxNotificationAdapter;

impl NotificationAdapter for LinuxNotificationAdapter {
    fn os_kind(&self) -> NotificationOs {
        NotificationOs::Linux
    }

    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        self.notify_with_env(
            &NotificationEnv::current(),
            &DefaultCommandRunner,
            tag,
            title,
            body,
        )
    }

    fn notify_with_env(
        &self,
        env: &NotificationEnv,
        runner: &dyn CommandRunner,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        if try_env_ipc_notification(env, tag, title, body) {
            return Ok(());
        }
        if env.suppress_os_fallback {
            return Ok(());
        }
        dispatch_notify_send(runner, title, body)
    }

    fn sync_notification_icon_with_env(
        &self,
        env: &NotificationIconEnv,
        runner: &dyn CommandRunner,
    ) -> io::Result<bool> {
        sync_linux_notification_icon(env, runner)
    }
}

/// Windows notification adapter (`ruum-gui` IPC -> hidden `powershell.exe`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsNotificationAdapter;

impl NotificationAdapter for WindowsNotificationAdapter {
    fn os_kind(&self) -> NotificationOs {
        NotificationOs::Windows
    }

    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        self.notify_with_env(
            &NotificationEnv::current(),
            &DefaultCommandRunner,
            tag,
            title,
            body,
        )
    }

    fn notify_with_env(
        &self,
        env: &NotificationEnv,
        runner: &dyn CommandRunner,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        if try_env_ipc_notification(env, tag, title, body) {
            return Ok(());
        }
        if env.suppress_os_fallback {
            return Ok(());
        }
        dispatch_powershell_toast(runner, tag, title, body)
    }

    fn sync_notification_icon_with_env(
        &self,
        env: &NotificationIconEnv,
        runner: &dyn CommandRunner,
    ) -> io::Result<bool> {
        sync_windows_notification_icon(env, runner)
    }
}

/// Factory creating the host or requested [`NotificationAdapter`].
#[derive(Clone, Copy, Debug, Default)]
pub struct NotificationFactory;

impl NotificationFactory {
    /// Detect the current target operating system family.
    #[must_use]
    pub const fn current_os() -> NotificationOs {
        if cfg!(target_os = "macos") {
            NotificationOs::MacOs
        } else if cfg!(windows) {
            NotificationOs::Windows
        } else {
            NotificationOs::Linux
        }
    }

    /// Construct a boxed [`NotificationAdapter`] for an explicit OS family.
    #[must_use]
    pub fn create_for(os: NotificationOs) -> Box<dyn NotificationAdapter> {
        match os {
            NotificationOs::MacOs => Box::new(MacOsNotificationAdapter),
            NotificationOs::Linux => Box::new(LinuxNotificationAdapter),
            NotificationOs::Windows => Box::new(WindowsNotificationAdapter),
        }
    }

    /// Construct a boxed [`NotificationAdapter`] for the host OS.
    #[must_use]
    pub fn create() -> Box<dyn NotificationAdapter> {
        Self::create_for(Self::current_os())
    }
}

/// Default system notification adapter delegating to [`NotificationFactory`].
#[derive(Clone, Copy, Debug, Default)]
pub struct OsNotificationAdapter;

impl NotificationAdapter for OsNotificationAdapter {
    fn os_kind(&self) -> NotificationOs {
        NotificationFactory::current_os()
    }

    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        NotificationFactory::create().notify_tagged(tag, title, body)
    }

    fn notify_with_env(
        &self,
        env: &NotificationEnv,
        runner: &dyn CommandRunner,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        NotificationFactory::create()
            .notify_with_env(env, runner, tag, title, body)
    }

    fn sync_notification_icon_with_env(
        &self,
        env: &NotificationIconEnv,
        runner: &dyn CommandRunner,
    ) -> io::Result<bool> {
        NotificationFactory::create()
            .sync_notification_icon_with_env(env, runner)
    }
}

fn dispatch_osascript_notification(
    runner: &dyn CommandRunner,
    title: &str,
    body: &str,
) -> io::Result<()> {
    let envs = [("RUUM_NOTIF_TITLE", title), ("RUUM_NOTIF_BODY", body)];
    let out = runner.run(
        Path::new("osascript"),
        &["-e", MACOS_OSASCRIPT_NOTIFY],
        &envs,
    )?;
    if out.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "osascript failed with exit code {}: {}",
            out.status_code,
            out.stderr.trim()
        )))
    }
}

fn dispatch_notify_send(
    runner: &dyn CommandRunner,
    title: &str,
    body: &str,
) -> io::Result<()> {
    let args = ["-a", "ruum", "-i", "ruum", "-t", "5000", "--", title, body];
    let out = runner.run(Path::new("notify-send"), &args, &[])?;
    if out.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "notify-send failed with exit code {}: {}",
            out.status_code,
            out.stderr.trim()
        )))
    }
}

fn dispatch_powershell_toast(
    runner: &dyn CommandRunner,
    tag: &str,
    title: &str,
    body: &str,
) -> io::Result<()> {
    let envs = [
        ("RUUM_NOTIF_TAG", tag),
        ("RUUM_NOTIF_TITLE", title),
        ("RUUM_NOTIF_BODY", body),
    ];
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
        WINDOWS_TOAST_SCRIPT,
    ];
    let out = runner.run(Path::new("powershell.exe"), &args, &envs)?;
    if out.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "powershell toast failed with exit code {}: {}",
            out.status_code,
            out.stderr.trim()
        )))
    }
}

fn try_env_ipc_notification(
    env: &NotificationEnv,
    tag: &str,
    title: &str,
    body: &str,
) -> bool {
    let (Some(target), Some(token)) =
        (env.ipc_target.as_deref(), env.ipc_token.as_deref())
    else {
        return false;
    };
    try_ipc_notification_with(target, token, tag, title, body)
}

pub(crate) fn resolve_ipc_target_and_token() -> Option<(PathBuf, String)> {
    let token_path = ipc_token_path()?;
    let token = load_or_create_ipc_token(&token_path).ok()?;
    #[cfg(unix)]
    let target = crate::confirm::resolve_socket_path();
    #[cfg(windows)]
    let target = crate::confirm::ipc_port_path()?;
    Some((target, token))
}

pub(crate) fn send_ipc_ok(socket_or_port: &Path, request: &IpcRequest) -> bool {
    matches!(
        send_ipc_message(socket_or_port, request),
        Ok(IpcResponse::Ok)
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
        &IpcRequest::ShowNotification {
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
        &IpcRequest::OpenWindow {
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

/// Probe whether an active GUI instance is already listening on the given IPC
/// socket/port without opening or focusing any window.
pub fn try_ipc_ping_with(socket_or_port: &Path, token: &str) -> bool {
    send_ipc_ok(
        socket_or_port,
        &IpcRequest::Ping {
            token: token.to_string(),
        },
    )
}

/// Probe whether an active GUI instance is already listening on default IPC
/// without opening or focusing any window.
#[must_use]
pub fn try_ipc_ping() -> bool {
    let Some((target, token)) = resolve_ipc_target_and_token() else {
        return false;
    };
    try_ipc_ping_with(&target, &token)
}

/// Notify an active GUI instance over an explicit IPC socket/port of a watchdog
/// state change.
pub fn notify_gui_watchdog_update_with(
    socket_or_port: &Path,
    token: &str,
) -> bool {
    send_ipc_ok(
        socket_or_port,
        &IpcRequest::NotifyWatchdogUpdate {
            token: token.to_string(),
        },
    )
}

/// Notify an active GUI instance over default local IPC of a watchdog update.
pub fn notify_gui_watchdog_update() {
    let Some((target, token)) = resolve_ipc_target_and_token() else {
        return;
    };
    let _ = notify_gui_watchdog_update_with(&target, &token);
}

/// Send a fallback OS desktop notification directly via platform CLI utilities
/// without touching IPC (best-effort; ignores missing utilities on headless
/// servers).
#[allow(
    clippy::unnecessary_wraps,
    reason = "kept fallible signature for adapter callers"
)]
pub fn send_fallback_os_notification(
    title: &str,
    body: &str,
) -> io::Result<()> {
    let mut env = NotificationEnv::without_ipc();
    env.suppress_os_fallback = os_notifications_disabled();
    let runner = DefaultCommandRunner;
    let _ = NotificationFactory::create().notify_with_env(
        &env,
        &runner,
        NOTIF_TAG_LOW_SPACE,
        title,
        body,
    );
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

/// Synchronize the host OS desktop notification icon registration and clear
/// stale icon caches when the packaged icon changes.
pub fn sync_notification_icon() -> io::Result<bool> {
    NotificationFactory::create().sync_notification_icon_with_env(
        &NotificationIconEnv::current(),
        &DefaultCommandRunner,
    )
}

fn sha256_prefix16(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(16);
    for byte in &digest[..8] {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn stamp_matches(stamp_path: &Path, expected: &str) -> bool {
    fs::read_to_string(stamp_path)
        .ok()
        .is_some_and(|existing| existing == expected)
}

fn write_stamp(app_data_dir: &Path, stamp: &str) -> io::Result<()> {
    fs::create_dir_all(app_data_dir)?;
    fs::write(app_data_dir.join(ICON_STAMP_FILE), stamp)
}

fn resolve_macos_app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name().and_then(|n| n.to_str()) != Some("MacOS") {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name().and_then(|n| n.to_str()) != Some("Contents") {
        return None;
    }
    let bundle = contents_dir.parent()?;
    (bundle.extension().and_then(|e| e.to_str()) == Some("app"))
        .then(|| bundle.to_path_buf())
}

fn sync_macos_notification_icon(
    env: &NotificationIconEnv,
    runner: &dyn CommandRunner,
) -> io::Result<bool> {
    let Some(exe) = env.current_exe.as_deref() else {
        return Ok(false);
    };
    let Some(bundle) = resolve_macos_app_bundle(exe) else {
        return Ok(false);
    };
    let icns_path = bundle.join("Contents/Resources/AppIcon.icns");
    if !icns_path.is_file() {
        return Ok(false);
    }

    let icns_bytes = fs::read(&icns_path)?;
    let hash16 = sha256_prefix16(&icns_bytes);
    let stamp = format!("macos:{hash16}:{}", bundle.display());
    if env
        .app_data_dir
        .as_deref()
        .is_some_and(|dir| stamp_matches(&dir.join(ICON_STAMP_FILE), &stamp))
    {
        return Ok(false);
    }

    let bundle_str = bundle.to_string_lossy();
    let out = runner.run(
        Path::new(MACOS_LSREGISTER_PATH),
        &["-f", bundle_str.as_ref()],
        &[],
    )?;
    if !out.success() {
        return Err(io::Error::other(format!(
            "lsregister failed with exit code {}: {}",
            out.status_code,
            out.stderr.trim()
        )));
    }

    if let Some(dir) = env.app_data_dir.as_deref() {
        write_stamp(dir, &stamp)?;
    }
    Ok(true)
}

fn cleanup_stale_windows_icons(
    app_data_dir: &Path,
    current_filename: &str,
) -> io::Result<()> {
    for entry in fs::read_dir(app_data_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name.starts_with(WINDOWS_ICON_PREFIX)
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ico"))
            && name != current_filename
        {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}

fn sync_windows_notification_icon(
    env: &NotificationIconEnv,
    runner: &dyn CommandRunner,
) -> io::Result<bool> {
    let Some(app_data_dir) = env.app_data_dir.as_deref() else {
        return Ok(false);
    };

    let hash16 = sha256_prefix16(EMBEDDED_RUUM_ICO);
    let icon_filename = format!("{WINDOWS_ICON_PREFIX}{hash16}.ico");
    let icon_path = app_data_dir.join(&icon_filename);
    let stamp = format!("windows:{hash16}:{}", icon_path.display());

    if stamp_matches(&app_data_dir.join(ICON_STAMP_FILE), &stamp)
        && icon_path.is_file()
    {
        return Ok(false);
    }

    fs::create_dir_all(app_data_dir)?;
    // Use a content-hashed filename so Windows Shell/Toast IconUri caches
    // invalidate immediately when the icon bytes change.
    fs::write(&icon_path, EMBEDDED_RUUM_ICO)?;
    cleanup_stale_windows_icons(app_data_dir, &icon_filename)?;

    let icon_uri = icon_path.to_string_lossy();
    let entries = [
        ("DisplayName", RUUM_APP_NAME),
        ("IconUri", icon_uri.as_ref()),
        ("IconBackgroundColor", WINDOWS_ICON_BACKGROUND_COLOR),
    ];
    for (value_name, value_data) in entries {
        let out = runner.run(
            Path::new("reg.exe"),
            &[
                "add",
                WINDOWS_AUMID_REG_KEY,
                "/v",
                value_name,
                "/t",
                "REG_SZ",
                "/d",
                value_data,
                "/f",
            ],
            &[],
        )?;
        if !out.success() {
            return Err(io::Error::other(format!(
                "reg.exe add {value_name} failed with exit code {}: {}",
                out.status_code,
                out.stderr.trim()
            )));
        }
    }

    write_stamp(app_data_dir, &stamp)?;
    Ok(true)
}

fn resolve_linux_hicolor_dir(env: &NotificationIconEnv) -> Option<PathBuf> {
    let data_home = env
        .xdg_data_home
        .clone()
        .filter(|dir| {
            !dir.as_os_str().is_empty()
                && (dir.is_absolute() || dir.to_string_lossy().starts_with('/'))
        })
        .or_else(|| env.home.as_ref().map(|h| h.join(".local/share")))?;
    Some(data_home.join("icons/hicolor"))
}

fn sync_linux_notification_icon(
    env: &NotificationIconEnv,
    runner: &dyn CommandRunner,
) -> io::Result<bool> {
    let Some(hicolor_dir) = resolve_linux_hicolor_dir(env) else {
        return Ok(false);
    };
    let icon_path = hicolor_dir.join("scalable/apps/ruum.svg");
    // Only refresh an existing user-level icon; do not write into ~/.local on
    // system-package installs where /usr/share/icons/hicolor owns the icon.
    if !icon_path.is_file() {
        return Ok(false);
    }

    let hash16 = sha256_prefix16(EMBEDDED_RUUM_SVG);
    let stamp = format!("linux:{hash16}:{}", icon_path.display());
    let existing_bytes = fs::read(&icon_path)?;
    let bytes_match = existing_bytes == EMBEDDED_RUUM_SVG;
    let stamp_ok = env
        .app_data_dir
        .as_deref()
        .is_none_or(|dir| stamp_matches(&dir.join(ICON_STAMP_FILE), &stamp));
    if bytes_match && stamp_ok {
        return Ok(false);
    }

    if !bytes_match {
        fs::write(&icon_path, EMBEDDED_RUUM_SVG)?;
    }

    let hicolor_str = hicolor_dir.to_string_lossy();
    match runner.run(
        Path::new("gtk-update-icon-cache"),
        &["-f", "-t", hicolor_str.as_ref()],
        &[],
    ) {
        Ok(_out) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    if let Some(dir) = env.app_data_dir.as_deref() {
        write_stamp(dir, &stamp)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_cli::CommandOutput;
    use crate::confirm::IpcListenerGuard;
    use std::sync::{Arc, Mutex};

    type RecordedRun = (PathBuf, Vec<String>, Vec<(String, String)>);

    #[derive(Default)]
    struct RecordingRunner {
        calls: Mutex<Vec<RecordedRun>>,
        status_code: i32,
        fail_io: bool,
    }

    impl CommandRunner for RecordingRunner {
        fn run(
            &self,
            program: &Path,
            args: &[&str],
            envs: &[(&str, &str)],
        ) -> io::Result<CommandOutput> {
            self.calls.lock().unwrap().push((
                program.to_path_buf(),
                args.iter().map(|s| (*s).to_string()).collect(),
                envs.iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ));
            if self.fail_io {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "binary missing",
                ));
            }
            Ok(CommandOutput {
                status_code: self.status_code,
                stdout: String::new(),
                stderr: if self.status_code == 0 {
                    String::new()
                } else {
                    "mock failure".to_string()
                },
            })
        }
    }

    #[test]
    fn factory_creates_all_three_adapters_with_correct_os_kind() {
        let mac = NotificationFactory::create_for(NotificationOs::MacOs);
        assert_eq!(mac.os_kind(), NotificationOs::MacOs);

        let linux = NotificationFactory::create_for(NotificationOs::Linux);
        assert_eq!(linux.os_kind(), NotificationOs::Linux);

        let win = NotificationFactory::create_for(NotificationOs::Windows);
        assert_eq!(win.os_kind(), NotificationOs::Windows);

        let host = NotificationFactory::create();
        assert_eq!(host.os_kind(), NotificationFactory::current_os());
    }

    #[test]
    fn all_adapters_prefer_ipc_and_skip_os_command_when_gui_online() {
        let temp = tempfile::tempdir().unwrap();
        let token = "notify_ipc_token_12345678901234567890123456".to_string();

        #[cfg(unix)]
        let socket_or_port = temp.path().join("notify.sock");
        #[cfg(windows)]
        let socket_or_port = temp.path().join("notify.port");

        let listener = IpcListenerGuard::bind(&socket_or_port).unwrap();
        let server_token = token.clone();
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_srv = Arc::clone(&received);

        let handle = std::thread::spawn(move || {
            for _ in 0..3 {
                let rec = Arc::clone(&received_srv);
                listener
                    .accept_and_handle(&server_token, move |req| {
                        rec.lock().unwrap().push(req);
                        Ok(IpcResponse::Ok)
                    })
                    .unwrap();
            }
        });

        let env = NotificationEnv::with_ipc(socket_or_port, token);
        for os in [
            NotificationOs::MacOs,
            NotificationOs::Linux,
            NotificationOs::Windows,
        ] {
            let adapter = NotificationFactory::create_for(os);
            let runner = RecordingRunner::default();
            adapter
                .notify_with_env(
                    &env,
                    &runner,
                    NOTIF_TAG_LOW_SPACE,
                    "Low Disk",
                    "Only 2 GB left",
                )
                .unwrap();
            assert!(
                runner.calls.lock().unwrap().is_empty(),
                "fallback CLI must not run when IPC succeeded for {os:?}"
            );
        }

        handle.join().unwrap();
        let reqs = received.lock().unwrap().clone();
        assert_eq!(reqs.len(), 3);
        for req in reqs {
            assert!(matches!(
                req,
                IpcRequest::ShowNotification {
                    tag,
                    title,
                    body,
                    ..
                } if tag == NOTIF_TAG_LOW_SPACE
                    && title == "Low Disk"
                    && body == "Only 2 GB left"
            ));
        }
    }

    #[test]
    fn macos_fallback_uses_env_attributes_without_script_interpolation() {
        let adapter = NotificationFactory::create_for(NotificationOs::MacOs);
        let runner = RecordingRunner::default();
        let tricky_title = "Disk \"Alert\" \\ $(whoami)";
        let tricky_body = "Path: /tmp/'test'\nFree: 1 GB";

        adapter
            .notify_with_env(
                &NotificationEnv::without_ipc(),
                &runner,
                NOTIF_TAG_LOW_SPACE,
                tricky_title,
                tricky_body,
            )
            .unwrap();

        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("osascript"));
        assert_eq!(calls[0].1[0], "-e");
        assert!(!calls[0].1[1].contains("whoami"));
        assert_eq!(
            calls[0].2,
            vec![
                ("RUUM_NOTIF_TITLE".to_string(), tricky_title.to_string()),
                ("RUUM_NOTIF_BODY".to_string(), tricky_body.to_string()),
            ]
        );
    }

    #[test]
    fn linux_fallback_uses_notify_send_argv_with_separator() {
        let adapter = NotificationFactory::create_for(NotificationOs::Linux);
        let runner = RecordingRunner::default();

        adapter
            .notify_with_env(
                &NotificationEnv::without_ipc(),
                &runner,
                NOTIF_TAG_AI_ANALYSIS,
                "--not-a-flag",
                "Cleanup ready",
            )
            .unwrap();

        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("notify-send"));
        assert_eq!(
            calls[0].1,
            vec![
                "-a",
                "ruum",
                "-i",
                "ruum",
                "-t",
                "5000",
                "--",
                "--not-a-flag",
                "Cleanup ready",
            ]
        );
    }

    #[test]
    fn windows_fallback_uses_hidden_powershell_and_env_vars() {
        let adapter = NotificationFactory::create_for(NotificationOs::Windows);
        let runner = RecordingRunner::default();
        let title = "Low <Space> & \"Alert\"";
        let body = "Only 1.5 GB free";

        adapter
            .notify_with_env(
                &NotificationEnv::without_ipc(),
                &runner,
                NOTIF_TAG_CLEANUP_DONE,
                title,
                body,
            )
            .unwrap();

        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("powershell.exe"));
        assert_eq!(
            calls[0].1[..5],
            [
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
            ]
        );
        assert!(!calls[0].1[5].contains(title));
        assert!(!calls[0].1[5].contains(body));
        assert!(calls[0].1[5].contains("SecurityElement"));
        assert_eq!(
            calls[0].2,
            vec![
                (
                    "RUUM_NOTIF_TAG".to_string(),
                    NOTIF_TAG_CLEANUP_DONE.to_string()
                ),
                ("RUUM_NOTIF_TITLE".to_string(), title.to_string()),
                ("RUUM_NOTIF_BODY".to_string(), body.to_string()),
            ]
        );
    }

    #[test]
    fn fallback_propagates_runner_errors() {
        let adapter = NotificationFactory::create_for(NotificationOs::Linux);
        let fail_status = RecordingRunner {
            status_code: 1,
            ..RecordingRunner::default()
        };
        assert!(
            adapter
                .notify_with_env(
                    &NotificationEnv::without_ipc(),
                    &fail_status,
                    NOTIF_TAG_LOW_SPACE,
                    "t",
                    "b",
                )
                .is_err()
        );

        let fail_io = RecordingRunner {
            fail_io: true,
            ..RecordingRunner::default()
        };
        assert!(
            adapter
                .notify_with_env(
                    &NotificationEnv::without_ipc(),
                    &fail_io,
                    NOTIF_TAG_LOW_SPACE,
                    "t",
                    "b",
                )
                .is_err()
        );
    }

    #[test]
    fn suppress_os_fallback_skips_os_commands_while_preserving_ipc() {
        assert!(!NotificationEnv::without_ipc().suppress_os_fallback);

        let suppressed_no_ipc = NotificationEnv {
            suppress_os_fallback: true,
            ..NotificationEnv::without_ipc()
        };
        for os in [
            NotificationOs::MacOs,
            NotificationOs::Linux,
            NotificationOs::Windows,
        ] {
            let adapter = NotificationFactory::create_for(os);
            let runner = RecordingRunner {
                fail_io: true,
                ..RecordingRunner::default()
            };
            adapter
                .notify_with_env(
                    &suppressed_no_ipc,
                    &runner,
                    NOTIF_TAG_LOW_SPACE,
                    "Low Disk",
                    "Only 2 GB left",
                )
                .unwrap();
            assert!(
                runner.calls.lock().unwrap().is_empty(),
                "OS fallback command must be skipped when suppressed for {os:?}"
            );
        }

        let temp = tempfile::tempdir().unwrap();
        let token = "notify_suppressed_token_1234567890123456".to_string();

        #[cfg(unix)]
        let socket_or_port = temp.path().join("notify.sock");
        #[cfg(windows)]
        let socket_or_port = temp.path().join("notify.port");

        let listener = IpcListenerGuard::bind(&socket_or_port).unwrap();
        let server_token = token.clone();
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_srv = Arc::clone(&received);

        let handle = std::thread::spawn(move || {
            for _ in 0..3 {
                let rec = Arc::clone(&received_srv);
                listener
                    .accept_and_handle(&server_token, move |req| {
                        rec.lock().unwrap().push(req);
                        Ok(IpcResponse::Ok)
                    })
                    .unwrap();
            }
        });

        let mut env = NotificationEnv::with_ipc(socket_or_port, token);
        assert!(!env.suppress_os_fallback);
        env.suppress_os_fallback = true;

        for os in [
            NotificationOs::MacOs,
            NotificationOs::Linux,
            NotificationOs::Windows,
        ] {
            let adapter = NotificationFactory::create_for(os);
            let runner = RecordingRunner {
                fail_io: true,
                ..RecordingRunner::default()
            };
            adapter
                .notify_with_env(
                    &env,
                    &runner,
                    NOTIF_TAG_LOW_SPACE,
                    "Low Disk",
                    "Only 2 GB left",
                )
                .unwrap();
            assert!(
                runner.calls.lock().unwrap().is_empty(),
                "OS command must not run when IPC succeeded for {os:?}"
            );
        }

        handle.join().unwrap();
        assert_eq!(received.lock().unwrap().len(), 3);
    }

    #[test]
    fn constants_and_embedded_assets_match_canonical_identity() {
        assert_eq!(RUUM_APP_ID, "io.github.tobi.ruum");
        assert_eq!(RUUM_APP_NAME, "ruum");
        assert!(!EMBEDDED_RUUM_ICO.is_empty());
        assert!(!EMBEDDED_RUUM_SVG.is_empty());
        let svg = std::str::from_utf8(EMBEDDED_RUUM_SVG).expect("utf8 svg");
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn macos_sync_registers_app_bundle_and_tracks_icns_hash() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("app-data");
        let adapter = NotificationFactory::create_for(NotificationOs::MacOs);

        // Non-.app executable path is a no-op.
        let bare_exe = temp.path().join("bin/ruum-gui");
        std::fs::create_dir_all(bare_exe.parent().unwrap()).unwrap();
        std::fs::write(&bare_exe, b"bin").unwrap();
        let env_bare = NotificationIconEnv {
            app_data_dir: Some(data_dir.clone()),
            current_exe: Some(bare_exe),
            home: None,
            xdg_data_home: None,
        };
        let runner_bare = RecordingRunner::default();
        assert!(
            !adapter
                .sync_notification_icon_with_env(&env_bare, &runner_bare)
                .unwrap()
        );
        assert!(runner_bare.calls.lock().unwrap().is_empty());

        // .app bundle without AppIcon.icns is a no-op.
        let bundle = temp.path().join("Applications/ruum.app");
        let macos_dir = bundle.join("Contents/MacOS");
        let resources_dir = bundle.join("Contents/Resources");
        std::fs::create_dir_all(&macos_dir).unwrap();
        std::fs::create_dir_all(&resources_dir).unwrap();
        let bundle_exe = macos_dir.join("ruum-gui");
        std::fs::write(&bundle_exe, b"bin").unwrap();
        let env_bundle = NotificationIconEnv {
            app_data_dir: Some(data_dir.clone()),
            current_exe: Some(bundle_exe),
            home: None,
            xdg_data_home: None,
        };
        let runner_missing_icns = RecordingRunner::default();
        assert!(
            !adapter
                .sync_notification_icon_with_env(
                    &env_bundle,
                    &runner_missing_icns
                )
                .unwrap()
        );
        assert!(runner_missing_icns.calls.lock().unwrap().is_empty());

        // First sync with AppIcon.icns runs lsregister -f and writes stamp.
        let icns_path = resources_dir.join("AppIcon.icns");
        std::fs::write(&icns_path, b"icns-v1").unwrap();
        let runner_first = RecordingRunner::default();
        assert!(
            adapter
                .sync_notification_icon_with_env(&env_bundle, &runner_first)
                .unwrap()
        );
        let calls_first = runner_first.calls.lock().unwrap().clone();
        assert_eq!(calls_first.len(), 1);
        assert_eq!(
            calls_first[0].0,
            PathBuf::from(
                "/System/Library/Frameworks/CoreServices.framework/\
                 Frameworks/LaunchServices.framework/Support/lsregister"
            )
        );
        assert_eq!(
            calls_first[0].1,
            vec!["-f".to_string(), bundle.display().to_string()]
        );
        let stamp_path = data_dir.join("notification-icon.stamp");
        let stamp_v1 = std::fs::read_to_string(&stamp_path).unwrap();
        assert!(stamp_v1.starts_with("macos:"));

        // Second sync with unchanged icon skips lsregister.
        let runner_second = RecordingRunner::default();
        assert!(
            !adapter
                .sync_notification_icon_with_env(&env_bundle, &runner_second)
                .unwrap()
        );
        assert!(runner_second.calls.lock().unwrap().is_empty());

        // Failing lsregister propagates error and preserves previous stamp.
        std::fs::write(&icns_path, b"icns-v2").unwrap();
        let runner_fail = RecordingRunner {
            status_code: 1,
            ..RecordingRunner::default()
        };
        assert!(
            adapter
                .sync_notification_icon_with_env(&env_bundle, &runner_fail)
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&stamp_path).unwrap(), stamp_v1);

        // Succeeding sync after icon update re-runs lsregister and updates stamp.
        let runner_updated = RecordingRunner::default();
        assert!(
            adapter
                .sync_notification_icon_with_env(&env_bundle, &runner_updated)
                .unwrap()
        );
        assert_eq!(runner_updated.calls.lock().unwrap().len(), 1);
        let stamp_v2 = std::fs::read_to_string(&stamp_path).unwrap();
        assert_ne!(stamp_v2, stamp_v1);
    }

    #[test]
    fn windows_sync_writes_hashed_ico_cleans_stale_and_updates_registry() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = NotificationFactory::create_for(NotificationOs::Windows);

        // Missing app_data_dir is a no-op.
        let runner_none = RecordingRunner::default();
        assert!(
            !adapter
                .sync_notification_icon_with_env(
                    &NotificationIconEnv::default(),
                    &runner_none,
                )
                .unwrap()
        );
        assert!(runner_none.calls.lock().unwrap().is_empty());

        let data_dir = temp.path().join("appdata/ruum");
        std::fs::create_dir_all(&data_dir).unwrap();
        let stale_ico = data_dir.join("ruum-icon-0000000000000000.ico");
        let keep_file = data_dir.join("keep.txt");
        std::fs::write(&stale_ico, b"old-ico").unwrap();
        std::fs::write(&keep_file, b"keep").unwrap();

        let env = NotificationIconEnv {
            app_data_dir: Some(data_dir.clone()),
            current_exe: None,
            home: None,
            xdg_data_home: None,
        };
        let runner_first = RecordingRunner::default();
        assert!(
            adapter
                .sync_notification_icon_with_env(&env, &runner_first)
                .unwrap()
        );

        assert!(!stale_ico.exists(), "stale ruum-icon-*.ico must be removed");
        assert!(keep_file.is_file(), "unrelated files must remain untouched");

        let hash16 = sha256_prefix16(EMBEDDED_RUUM_ICO);
        let expected_ico = data_dir.join(format!("ruum-icon-{hash16}.ico"));
        assert_eq!(std::fs::read(&expected_ico).unwrap(), EMBEDDED_RUUM_ICO);

        let calls = runner_first.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);
        let expected_key =
            r"HKCU\Software\Classes\AppUserModelId\io.github.tobi.ruum";
        let expected_uri = expected_ico.display().to_string();
        assert_eq!(calls[0].0, PathBuf::from("reg.exe"));
        assert_eq!(
            calls[0].1,
            vec![
                "add",
                expected_key,
                "/v",
                "DisplayName",
                "/t",
                "REG_SZ",
                "/d",
                "ruum",
                "/f",
            ]
        );
        assert_eq!(calls[1].0, PathBuf::from("reg.exe"));
        assert_eq!(
            calls[1].1,
            vec![
                "add",
                expected_key,
                "/v",
                "IconUri",
                "/t",
                "REG_SZ",
                "/d",
                expected_uri.as_str(),
                "/f",
            ]
        );
        assert_eq!(calls[2].0, PathBuf::from("reg.exe"));
        assert_eq!(
            calls[2].1,
            vec![
                "add",
                expected_key,
                "/v",
                "IconBackgroundColor",
                "/t",
                "REG_SZ",
                "/d",
                "FF151821",
                "/f",
            ]
        );

        let stamp_path = data_dir.join("notification-icon.stamp");
        assert_eq!(
            std::fs::read_to_string(&stamp_path).unwrap(),
            format!("windows:{hash16}:{expected_uri}")
        );

        // Second sync skips when both stamp and hashed .ico are present.
        let runner_second = RecordingRunner::default();
        assert!(
            !adapter
                .sync_notification_icon_with_env(&env, &runner_second)
                .unwrap()
        );
        assert!(runner_second.calls.lock().unwrap().is_empty());

        // Deleting the .ico forces re-sync even when stamp is present.
        std::fs::remove_file(&expected_ico).unwrap();
        let runner_resync = RecordingRunner::default();
        assert!(
            adapter
                .sync_notification_icon_with_env(&env, &runner_resync)
                .unwrap()
        );
        assert_eq!(runner_resync.calls.lock().unwrap().len(), 3);
        assert_eq!(std::fs::read(&expected_ico).unwrap(), EMBEDDED_RUUM_ICO);
    }

    #[test]
    fn linux_sync_updates_stale_user_svg_and_tolerates_missing_cache_tool() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = NotificationFactory::create_for(NotificationOs::Linux);
        let data_dir = temp.path().join("config/ruum");
        let xdg_data = temp.path().join("xdg-data");
        let home = temp.path().join("home");
        let hicolor = xdg_data.join("icons/hicolor");
        let icon_path = hicolor.join("scalable/apps/ruum.svg");

        let env = NotificationIconEnv {
            app_data_dir: Some(data_dir.clone()),
            current_exe: None,
            home: Some(home.clone()),
            xdg_data_home: Some(xdg_data),
        };

        // Missing user icon is a no-op and must not pollute XDG_DATA_HOME/home.
        let runner_missing = RecordingRunner::default();
        assert!(
            !adapter
                .sync_notification_icon_with_env(&env, &runner_missing)
                .unwrap()
        );
        assert!(!icon_path.exists());
        assert!(runner_missing.calls.lock().unwrap().is_empty());

        // Stale user SVG is replaced and gtk-update-icon-cache is invoked.
        std::fs::create_dir_all(icon_path.parent().unwrap()).unwrap();
        std::fs::write(&icon_path, b"<svg>stale</svg>").unwrap();
        let runner_update = RecordingRunner::default();
        assert!(
            adapter
                .sync_notification_icon_with_env(&env, &runner_update)
                .unwrap()
        );
        assert_eq!(std::fs::read(&icon_path).unwrap(), EMBEDDED_RUUM_SVG);
        let calls = runner_update.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("gtk-update-icon-cache"));
        assert_eq!(
            calls[0].1,
            vec!["-f", "-t", hicolor.to_string_lossy().as_ref()]
        );
        let stamp_path = data_dir.join("notification-icon.stamp");
        assert!(
            std::fs::read_to_string(&stamp_path)
                .unwrap()
                .starts_with("linux:")
        );

        // Second sync skips when SVG and stamp match.
        let runner_second = RecordingRunner::default();
        assert!(
            !adapter
                .sync_notification_icon_with_env(&env, &runner_second)
                .unwrap()
        );
        assert!(runner_second.calls.lock().unwrap().is_empty());

        // ErrorKind::NotFound from missing gtk-update-icon-cache is tolerated.
        std::fs::write(&icon_path, b"<svg>stale-again</svg>").unwrap();
        let runner_not_found = RecordingRunner {
            fail_io: true,
            ..RecordingRunner::default()
        };
        assert!(
            adapter
                .sync_notification_icon_with_env(&env, &runner_not_found)
                .unwrap()
        );
        assert_eq!(std::fs::read(&icon_path).unwrap(), EMBEDDED_RUUM_SVG);

        // Non-zero exit from gtk-update-icon-cache is tolerated cleanly.
        std::fs::write(&icon_path, b"<svg>stale-status</svg>").unwrap();
        let runner_nonzero = RecordingRunner {
            status_code: 1,
            ..RecordingRunner::default()
        };
        assert!(
            adapter
                .sync_notification_icon_with_env(&env, &runner_nonzero)
                .unwrap()
        );
        assert_eq!(std::fs::read(&icon_path).unwrap(), EMBEDDED_RUUM_SVG);

        // Empty XDG_DATA_HOME falls back to ~/.local/share.
        let home_hicolor = home.join(".local/share/icons/hicolor");
        let home_icon = home_hicolor.join("scalable/apps/ruum.svg");
        std::fs::create_dir_all(home_icon.parent().unwrap()).unwrap();
        std::fs::write(&home_icon, b"<svg>home-stale</svg>").unwrap();
        let env_fallback = NotificationIconEnv {
            app_data_dir: Some(data_dir),
            current_exe: None,
            home: Some(home),
            xdg_data_home: Some(PathBuf::new()),
        };
        let runner_home = RecordingRunner::default();
        assert!(
            adapter
                .sync_notification_icon_with_env(&env_fallback, &runner_home)
                .unwrap()
        );
        assert_eq!(std::fs::read(&home_icon).unwrap(), EMBEDDED_RUUM_SVG);
    }

    #[test]
    fn try_ipc_ping_with_verifies_token_and_missing_endpoint() {
        let temp = tempfile::tempdir().unwrap();
        let token = "ping_ipc_token_1234567890123456789012345".to_string();

        #[cfg(unix)]
        let socket_or_port = temp.path().join("ping.sock");
        #[cfg(windows)]
        let socket_or_port = temp.path().join("ping.port");

        assert!(!try_ipc_ping_with(&socket_or_port, &token));

        let listener = IpcListenerGuard::bind(&socket_or_port).unwrap();
        let server_token = token.clone();
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                listener
                    .accept_and_handle(&server_token, |req| {
                        assert!(matches!(req, IpcRequest::Ping { .. }));
                        Ok(IpcResponse::Ok)
                    })
                    .unwrap();
            }
        });

        assert!(try_ipc_ping_with(&socket_or_port, &token));
        assert!(!try_ipc_ping_with(&socket_or_port, "wrong_token"));
        handle.join().unwrap();
    }
}
