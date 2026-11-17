//! Tri-platform core OS desktop notification adapters.
//!
//! Prefers delivering notifications over local IPC to an active `ruum-gui`
//! instance (`IpcRequest::ShowNotification`). When no GUI listener is active,
//! falls back to safe, non-blocking OS utilities (`osascript` on macOS,
//! `notify-send` on Linux, and hidden `powershell.exe` Toast on Windows with
//! `CREATE_NO_WINDOW`) without depending on GPUI or `unsafe`.

use std::io;
use std::path::{Path, PathBuf};

use crate::ai_cli::{CommandRunner, DefaultCommandRunner};
use crate::confirm::{
    IpcRequest, IpcResponse, load_or_create_ipc_token, send_ipc_message,
};
use crate::watchdog_config::ipc_token_path;

/// Notification tag for low disk space alerts.
pub const NOTIF_TAG_LOW_SPACE: &str = "ruum.low_disk_space";

/// Notification tag when background cleanup starts after confirmation.
pub const NOTIF_TAG_CLEANUP_STARTED: &str = "ruum.cleanup_started";

/// Notification tag for completed cleanup summaries.
pub const NOTIF_TAG_CLEANUP_DONE: &str = "ruum.cleanup_complete";

/// Notification tag for AI analysis completion / proposals.
pub const NOTIF_TAG_AI_ANALYSIS: &str = "ruum.ai_analysis";

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

/// Captured IPC and environment state used by notification adapters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationEnv {
    pub ipc_target: Option<PathBuf>,
    pub ipc_token: Option<String>,
}

impl NotificationEnv {
    /// Capture current local IPC socket/port and authentication token.
    #[must_use]
    pub fn current() -> Self {
        let Some((target, token)) = resolve_ipc_target_and_token() else {
            return Self::default();
        };
        Self {
            ipc_target: Some(target),
            ipc_token: Some(token),
        }
    }

    /// Construct an environment with an explicit IPC target and token.
    #[must_use]
    pub const fn with_ipc(target: PathBuf, token: String) -> Self {
        Self {
            ipc_target: Some(target),
            ipc_token: Some(token),
        }
    }

    /// Construct an environment with IPC disabled (forces fallback delivery).
    #[must_use]
    pub const fn without_ipc() -> Self {
        Self {
            ipc_target: None,
            ipc_token: None,
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
        dispatch_osascript_notification(runner, title, body)
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
        dispatch_notify_send(runner, title, body)
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
        dispatch_powershell_toast(runner, tag, title, body)
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
    let runner = DefaultCommandRunner;
    let _ = NotificationFactory::create().notify_with_env(
        &NotificationEnv::without_ipc(),
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
}
