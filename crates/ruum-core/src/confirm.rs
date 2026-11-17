//! Local IPC and cleanup confirmation adapters.
//!
//! Provides the [`CleanupConfirmAdapter`] trait and [`GuiCleanupConfirm`]
//! implementation which coordinates confirmation between background services
//! (such as the watchdog or MCP server) and the Ruum graphical interface.
//!
//! If Ruum is running, confirmation is requested over a lightweight local IPC
//! channel. If not running and a graphical desktop is reachable, Ruum is
//! launched in confirmation mode (`--confirm-cleanup`). If no graphical
//! environment is reachable, the confirmation fails closed to prevent
//! unattended deletions.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use crate::removal::RemovalMode;
use crate::watchdog_config::ipc_token_path;

/// Maximum allowable byte length for a Unix domain socket path.
///
/// POSIX specifies `sun_path` as 104 or 108 bytes including the null
/// terminator. Keeping candidate paths strictly below 96 bytes ensures
/// ample headroom across all supported Unix platforms.
pub const MAX_SOCKET_PATH_BYTES: usize = 96;

/// Custom serde serializer/deserializer for foreign [`RemovalMode`].
pub mod removal_mode_serde {
    use crate::removal::RemovalMode;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(
        mode: &RemovalMode,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match mode {
            RemovalMode::Permanent => serializer.serialize_str("permanent"),
            RemovalMode::Trash => serializer.serialize_str("trash"),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<RemovalMode, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "permanent" => Ok(RemovalMode::Permanent),
            "trash" => Ok(RemovalMode::Trash),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["permanent", "trash"],
            )),
        }
    }
}

/// A single item proposed for user confirmation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupConfirmItem {
    pub path: PathBuf,
    pub bytes: u64,
    #[serde(with = "removal_mode_serde")]
    pub mode: RemovalMode,
    pub reason: Option<String>,
    pub git_warning: Option<String>,
}

/// A request presented to the user for interactive confirmation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupConfirmRequest {
    pub root: PathBuf,
    pub title: String,
    pub summary: Option<String>,
    pub items: Vec<CleanupConfirmItem>,
}

/// The user's decision from a cleanup confirmation prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CleanupConfirmDecision {
    Confirmed { selected_paths: Vec<PathBuf> },
    Cancelled,
}

/// Adapter interface for requesting cleanup confirmation from the user.
pub trait CleanupConfirmAdapter: Send + Sync + std::fmt::Debug {
    fn confirm(
        &self,
        request: &CleanupConfirmRequest,
    ) -> io::Result<CleanupConfirmDecision>;
}

/// A request presented to the user before opening an MCP remote SSH session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConnectConfirmRequest {
    pub target_label: String,
    pub alias: Option<String>,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_mode: String,
    pub tool_name: String,
}

/// The user's decision from a remote SSH connection confirmation prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RemoteConnectConfirmDecision {
    Approved,
    Denied,
}

/// Adapter interface for requesting explicit user approval before MCP connects
/// to a remote SSH server.
pub trait RemoteConnectConfirmAdapter: Send + Sync + std::fmt::Debug {
    fn confirm(
        &self,
        request: &RemoteConnectConfirmRequest,
    ) -> io::Result<RemoteConnectConfirmDecision>;
}

/// Protocol request frame sent to the IPC listener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    ConfirmCleanup {
        token: String,
        request: CleanupConfirmRequest,
    },
    ConfirmRemoteConnect {
        token: String,
        request: RemoteConnectConfirmRequest,
    },
    NotifyWatchdogUpdate {
        token: String,
    },
    ShowNotification {
        token: String,
        tag: String,
        title: String,
        body: String,
    },
    OpenWindow {
        token: String,
        tag: String,
    },
}

/// Protocol response frame returned by the IPC listener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    ConfirmDecision {
        decision: CleanupConfirmDecision,
    },
    RemoteConnectDecision {
        decision: RemoteConnectConfirmDecision,
    },
    Ok,
    Error {
        message: String,
    },
}

/// Generate a cryptographically secure 256-bit hexadecimal token.
#[must_use]
pub fn generate_ipc_token() -> String {
    #[cfg(unix)]
    {
        if let Ok(mut file) = fs::File::open("/dev/urandom") {
            use std::io::Read;
            let mut buf = [0u8; 32];
            if file.read_exact(&mut buf).is_ok() {
                let mut hex = String::with_capacity(64);
                for b in buf {
                    let _ = write!(hex, "{b:02x}");
                }
                return hex;
            }
        }
    }

    // Fallback: mix system clock, PID, and temporary file entropy.
    let mut hasher = Sha256::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    hasher.update(now.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    if let Ok(temp) = tempfile::Builder::new().tempfile() {
        hasher.update(temp.path().to_string_lossy().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Read or create the per-user random IPC authentication token.
///
/// On Unix, permissions are restricted to `0600` so other local users
/// cannot inspect the token and hijack the IPC channel.
pub fn load_or_create_ipc_token(path: &Path) -> io::Result<String> {
    if let Ok(content) = fs::read_to_string(path) {
        let trimmed = content.trim();
        if trimmed.len() >= 32 {
            return Ok(trimmed.to_string());
        }
    }

    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "missing token parent dir")
    })?;
    fs::create_dir_all(parent)?;

    let token = generate_ipc_token();

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        let mut file = opts.open(path)?;
        file.write_all(token.as_bytes())?;
        file.write_all(b"\n")?;
    }

    #[cfg(not(unix))]
    {
        let mut file = fs::File::create(path)?;
        file.write_all(token.as_bytes())?;
        file.write_all(b"\n")?;
    }

    Ok(token)
}

/// Fallback short socket path for the current user: `/tmp/ruum-<uid>.sock`.
#[cfg(unix)]
#[must_use]
pub fn fallback_socket_path() -> PathBuf {
    let uid = rustix::process::getuid().as_raw();
    PathBuf::from(format!("/tmp/ruum-{uid}.sock"))
}

/// Select a Unix socket path, enforcing `< 96` bytes on `sun_path`.
#[cfg(unix)]
#[must_use]
pub fn select_socket_path(
    xdg_runtime_dir: Option<&Path>,
    tmpdir: Option<&Path>,
    fallback: PathBuf,
) -> PathBuf {
    if let Some(xdg) = xdg_runtime_dir {
        let candidate = xdg.join("ruum-ipc.sock");
        if candidate.as_os_str().as_encoded_bytes().len()
            < MAX_SOCKET_PATH_BYTES
        {
            return candidate;
        }
    }
    if let Some(tmp) = tmpdir {
        let candidate = tmp.join("ruum-ipc.sock");
        if candidate.as_os_str().as_encoded_bytes().len()
            < MAX_SOCKET_PATH_BYTES
        {
            return candidate;
        }
    }
    fallback
}

/// Determine the active Unix domain socket path for ruum IPC.
#[cfg(unix)]
#[must_use]
pub fn resolve_socket_path() -> PathBuf {
    let xdg = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let tmp = std::env::var_os("TMPDIR").map(PathBuf::from);
    select_socket_path(xdg.as_deref(), tmp.as_deref(), fallback_socket_path())
}

/// Path to the port file used for local IPC on Windows.
#[cfg(windows)]
#[must_use]
pub fn ipc_port_path() -> Option<PathBuf> {
    crate::watchdog_config::config_dir().map(|d| d.join("ipc.port"))
}

/// Test whether a graphical display is reachable in the current session.
#[must_use]
pub fn is_desktop_reachable() -> bool {
    is_desktop_reachable_env(|k| std::env::var(k).ok())
}

/// Test whether a graphical display is reachable with injected environment.
pub fn is_desktop_reachable_env<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    if cfg!(target_os = "macos") {
        // macOS local user sessions reach WindowServer. If running inside
        // a remote SSH session with no forwarded DISPLAY, windows cannot open.
        if get_env("SSH_CONNECTION").is_some_and(|s| !s.is_empty())
            && get_env("DISPLAY").is_none_or(|s| s.is_empty())
        {
            return false;
        }
        true
    } else if cfg!(target_os = "linux") {
        get_env("WAYLAND_DISPLAY").is_some_and(|s| !s.is_empty())
            || get_env("DISPLAY").is_some_and(|s| !s.is_empty())
    } else if cfg!(windows) {
        get_env("SSH_CONNECTION").is_none_or(|s| s.is_empty())
    } else {
        false
    }
}

/// Read a line from a stream, parse request, verify token, and write response.
pub fn handle_ipc_stream<S, H>(
    mut stream: S,
    expected_token: &str,
    handler: H,
) -> io::Result<()>
where
    S: io::Read + io::Write,
    H: Fn(IpcRequest) -> io::Result<IpcResponse>,
{
    let mut line = String::new();
    {
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut line)?;
    }
    if line.trim().is_empty() {
        return Ok(());
    }

    let request: IpcRequest = match serde_json::from_str(&line) {
        Ok(req) => req,
        Err(e) => {
            let resp = IpcResponse::Error {
                message: format!("invalid IPC request JSON: {e}"),
            };
            let mut out = serde_json::to_string(&resp).map_err(|err| {
                io::Error::new(io::ErrorKind::InvalidData, err)
            })?;
            out.push('\n');
            let _ = stream.write_all(out.as_bytes());
            return Ok(());
        }
    };

    let token = match &request {
        IpcRequest::ConfirmCleanup { token, .. }
        | IpcRequest::ConfirmRemoteConnect { token, .. }
        | IpcRequest::NotifyWatchdogUpdate { token }
        | IpcRequest::ShowNotification { token, .. }
        | IpcRequest::OpenWindow { token, .. } => token.as_str(),
    };

    let response = if token == expected_token {
        match handler(request) {
            Ok(resp) => resp,
            Err(e) => IpcResponse::Error {
                message: format!("handler error: {e}"),
            },
        }
    } else {
        IpcResponse::Error {
            message: "unauthorized: IPC token mismatch".to_string(),
        }
    };

    let mut out = serde_json::to_string(&response)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    out.push('\n');
    stream.write_all(out.as_bytes())?;
    stream.flush()?;
    Ok(())
}

/// Guarded local IPC server listener that removes its socket/port on drop.
#[cfg(unix)]
#[derive(Debug)]
pub struct IpcListenerGuard {
    listener: UnixListener,
    path: PathBuf,
}

#[cfg(unix)]
impl IpcListenerGuard {
    pub fn bind(path: &Path) -> io::Result<Self> {
        if path.exists() {
            if UnixStream::connect(path).is_err() {
                let _ = fs::remove_file(path);
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "ruum IPC socket already in use",
                ));
            }
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(path)?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub fn accept_and_handle<H>(
        &self,
        expected_token: &str,
        handler: H,
    ) -> io::Result<()>
    where
        H: Fn(IpcRequest) -> io::Result<IpcResponse>,
    {
        let (stream, _) = self.listener.accept()?;
        handle_ipc_stream(stream, expected_token, handler)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
impl Drop for IpcListenerGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
#[derive(Debug)]
pub struct IpcListenerGuard {
    listener: TcpListener,
    port_path: PathBuf,
}

#[cfg(windows)]
impl IpcListenerGuard {
    pub fn bind(port_path: &Path) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        if let Some(parent) = port_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(port_path, format!("{port}\n"))?;
        Ok(Self {
            listener,
            port_path: port_path.to_path_buf(),
        })
    }

    pub fn accept_and_handle<H>(
        &self,
        expected_token: &str,
        handler: H,
    ) -> io::Result<()>
    where
        H: Fn(IpcRequest) -> io::Result<IpcResponse>,
    {
        let (stream, _) = self.listener.accept()?;
        handle_ipc_stream(stream, expected_token, handler)
    }

    #[must_use]
    pub fn port_path(&self) -> &Path {
        &self.port_path
    }
}

#[cfg(windows)]
impl Drop for IpcListenerGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.port_path);
    }
}

/// Interactive IPC read timeout for cleanup confirmation prompts (10 minutes).
///
/// Users reviewing an AI cleanup proposal in a detached confirmation window may
/// take several minutes before confirming or cancelling.
pub const IPC_CONFIRM_READ_TIMEOUT: std::time::Duration =
    std::time::Duration::from_mins(10);

/// Short IPC read timeout for non-interactive status notifications.
pub const IPC_NOTIFY_READ_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);

/// Select the socket read timeout appropriate for an IPC request kind.
#[must_use]
pub const fn ipc_read_timeout(request: &IpcRequest) -> std::time::Duration {
    match request {
        IpcRequest::ConfirmCleanup { .. }
        | IpcRequest::ConfirmRemoteConnect { .. } => IPC_CONFIRM_READ_TIMEOUT,
        IpcRequest::NotifyWatchdogUpdate { .. }
        | IpcRequest::ShowNotification { .. }
        | IpcRequest::OpenWindow { .. } => IPC_NOTIFY_READ_TIMEOUT,
    }
}

/// IPC client helper to send a request frame and receive a response frame.
#[cfg(unix)]
pub fn send_ipc_message(
    socket_path: &Path,
    request: &IpcRequest,
) -> io::Result<IpcResponse> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(ipc_read_timeout(request)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;

    let mut line = serde_json::to_string(request)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;
    if response_line.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "empty IPC response",
        ));
    }

    serde_json::from_str(&response_line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// IPC client helper to send a request frame and receive a response frame.
#[cfg(windows)]
pub fn send_ipc_message(
    port_path: &Path,
    request: &IpcRequest,
) -> io::Result<IpcResponse> {
    let content = fs::read_to_string(port_path)?;
    let port: u16 = content
        .trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(ipc_read_timeout(request)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;

    let mut line = serde_json::to_string(request)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;
    if response_line.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "empty IPC response",
        ));
    }

    serde_json::from_str(&response_line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn resolve_confirm_gui_binary_with(
    env: &crate::desktop_launch::DesktopLaunchEnv,
) -> io::Result<PathBuf> {
    crate::desktop_launch::DesktopLaunchFactory::create()
        .resolve_gui_binary(env)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "ruum-gui executable not found for confirmation dialog",
            )
        })
}

fn spawn_json_dialog<Req, Resp>(
    flag: &str,
    request: &Req,
    fallback: Resp,
) -> io::Result<Resp>
where
    Req: Serialize,
    Resp: serde::de::DeserializeOwned,
{
    let gui_exe = resolve_confirm_gui_binary_with(
        &crate::desktop_launch::DesktopLaunchEnv::current(),
    )?;
    let temp_dir = tempfile::tempdir()?;
    let req_path = temp_dir.path().join("confirm_request.json");
    let resp_path = temp_dir.path().join("confirm_response.json");

    let req_json = serde_json::to_string_pretty(request)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&req_path, req_json)?;

    let mut cmd = Command::new(gui_exe);
    cmd.arg(flag)
        .arg(&req_path)
        .arg("--response")
        .arg(&resp_path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let status = cmd.status()?;

    if !status.success() || !resp_path.exists() {
        return Ok(fallback);
    }

    let resp_content = fs::read_to_string(&resp_path)?;
    if resp_content.trim().is_empty() {
        return Ok(fallback);
    }

    serde_json::from_str(&resp_content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Spawn `ruum --confirm-cleanup` as an interactive confirmation dialog.
fn spawn_dialog(
    request: &CleanupConfirmRequest,
) -> io::Result<CleanupConfirmDecision> {
    spawn_json_dialog(
        "--confirm-cleanup",
        request,
        CleanupConfirmDecision::Cancelled,
    )
}

/// Spawn `ruum --confirm-remote-connect` as an interactive confirmation dialog.
fn spawn_remote_connect_dialog(
    request: &RemoteConnectConfirmRequest,
) -> io::Result<RemoteConnectConfirmDecision> {
    spawn_json_dialog(
        "--confirm-remote-connect",
        request,
        RemoteConnectConfirmDecision::Denied,
    )
}

fn resolve_ipc_token(override_token: Option<&str>) -> io::Result<String> {
    if let Some(t) = override_token {
        return Ok(t.to_owned());
    }
    let token_path = ipc_token_path().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no ipc.token path")
    })?;
    load_or_create_ipc_token(&token_path)
}

fn send_ipc_with_override(
    socket_override: Option<&Path>,
    request: &IpcRequest,
) -> Result<IpcResponse, IpcUnavailableOrError> {
    #[cfg(unix)]
    let send_res = {
        let target =
            socket_override.map_or_else(resolve_socket_path, Path::to_path_buf);
        send_ipc_message(&target, request)
    };

    #[cfg(windows)]
    let send_res = {
        let target = match socket_override {
            Some(p) => p.to_path_buf(),
            None => match ipc_port_path() {
                Some(p) => p,
                None => return Err(IpcUnavailableOrError::Unavailable),
            },
        };
        send_ipc_message(&target, request)
    };

    match send_res {
        Ok(resp) => Ok(resp),
        Err(err) if is_socket_unavailable(&err) => {
            Err(IpcUnavailableOrError::Unavailable)
        }
        Err(err) => Err(IpcUnavailableOrError::Error(err)),
    }
}

enum IpcUnavailableOrError {
    Unavailable,
    Error(io::Error),
}

/// Cleanup confirmation adapter that delegates to Ruum GUI.
///
/// Tries IPC first; if Ruum GUI is not currently running, spawns
/// `ruum --confirm-cleanup <req.json> --response <resp.json>`
/// if a desktop display is available, and fails closed otherwise.
#[derive(Default)]
pub struct GuiCleanupConfirm {
    socket_path: Option<PathBuf>,
    token: Option<String>,
}

impl std::fmt::Debug for GuiCleanupConfirm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuiCleanupConfirm")
            .field("socket_path", &self.socket_path)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl GuiCleanupConfirm {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            socket_path: None,
            token: None,
        }
    }

    #[must_use]
    pub const fn with_socket_and_token(
        socket_path: PathBuf,
        token: String,
    ) -> Self {
        Self {
            socket_path: Some(socket_path),
            token: Some(token),
        }
    }

    fn try_ipc(&self, request: &CleanupConfirmRequest) -> IpcOutcome {
        let Ok(token) = resolve_ipc_token(self.token.as_deref()) else {
            return IpcOutcome::Unavailable;
        };
        let frame = IpcRequest::ConfirmCleanup {
            token,
            request: request.clone(),
        };

        match send_ipc_with_override(self.socket_path.as_deref(), &frame) {
            Ok(IpcResponse::ConfirmDecision { decision }) => {
                IpcOutcome::Decision(decision)
            }
            Ok(IpcResponse::Error { message }) => IpcOutcome::Error(
                io::Error::new(io::ErrorKind::PermissionDenied, message),
            ),
            Ok(IpcResponse::Ok | IpcResponse::RemoteConnectDecision { .. }) => {
                IpcOutcome::Error(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected IPC response for cleanup confirm",
                ))
            }
            Err(IpcUnavailableOrError::Unavailable) => IpcOutcome::Unavailable,
            Err(IpcUnavailableOrError::Error(err)) => IpcOutcome::Error(err),
        }
    }

    /// Internal confirm implementation allowing an injected spawner.
    pub fn confirm_with_spawner<S>(
        &self,
        request: &CleanupConfirmRequest,
        spawner: S,
    ) -> io::Result<CleanupConfirmDecision>
    where
        S: FnOnce(&CleanupConfirmRequest) -> io::Result<CleanupConfirmDecision>,
    {
        self.confirm_with_spawner_and_reachability(
            request,
            spawner,
            is_desktop_reachable,
        )
    }

    /// Internal confirm allowing injected spawner and desktop reachability.
    pub fn confirm_with_spawner_and_reachability<S, R>(
        &self,
        request: &CleanupConfirmRequest,
        spawner: S,
        desktop_reachable: R,
    ) -> io::Result<CleanupConfirmDecision>
    where
        S: FnOnce(&CleanupConfirmRequest) -> io::Result<CleanupConfirmDecision>,
        R: FnOnce() -> bool,
    {
        // 1. Try talking to an active GUI window over local IPC.
        match self.try_ipc(request) {
            IpcOutcome::Decision(decision) => return Ok(decision),
            IpcOutcome::Error(err) => return Err(err),
            IpcOutcome::Unavailable => {}
        }

        // 2. Fail closed if no desktop display is reachable.
        if !desktop_reachable() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "cleanup confirmation requires an active graphical desktop",
            ));
        }

        // 3. Fall back to launching a standalone confirmation dialog.
        spawner(request)
    }
}

enum IpcOutcome {
    Decision(CleanupConfirmDecision),
    Unavailable,
    Error(io::Error),
}

fn is_socket_unavailable(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::AddrNotAvailable
            | io::ErrorKind::TimedOut
    )
}

impl CleanupConfirmAdapter for GuiCleanupConfirm {
    fn confirm(
        &self,
        request: &CleanupConfirmRequest,
    ) -> io::Result<CleanupConfirmDecision> {
        self.confirm_with_spawner(request, spawn_dialog)
    }
}

/// Remote SSH connection confirmation adapter that delegates to Ruum GUI.
///
/// Tries local IPC first (`IpcRequest::ConfirmRemoteConnect`); if Ruum GUI is
/// not running and a graphical desktop is reachable, spawns
/// `ruum-gui --confirm-remote-connect <req.json> --response <resp.json>`, and
/// fails closed otherwise.
#[derive(Default)]
pub struct GuiRemoteConnectConfirm {
    socket_path: Option<PathBuf>,
    token: Option<String>,
}

impl std::fmt::Debug for GuiRemoteConnectConfirm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuiRemoteConnectConfirm")
            .field("socket_path", &self.socket_path)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

enum RemoteConnectIpcOutcome {
    Decision(RemoteConnectConfirmDecision),
    Unavailable,
    Error(io::Error),
}

impl GuiRemoteConnectConfirm {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            socket_path: None,
            token: None,
        }
    }

    #[must_use]
    pub const fn with_socket_and_token(
        socket_path: PathBuf,
        token: String,
    ) -> Self {
        Self {
            socket_path: Some(socket_path),
            token: Some(token),
        }
    }

    fn try_ipc(
        &self,
        request: &RemoteConnectConfirmRequest,
    ) -> RemoteConnectIpcOutcome {
        let Ok(token) = resolve_ipc_token(self.token.as_deref()) else {
            return RemoteConnectIpcOutcome::Unavailable;
        };
        let frame = IpcRequest::ConfirmRemoteConnect {
            token,
            request: request.clone(),
        };

        match send_ipc_with_override(self.socket_path.as_deref(), &frame) {
            Ok(IpcResponse::RemoteConnectDecision { decision }) => {
                RemoteConnectIpcOutcome::Decision(decision)
            }
            Ok(IpcResponse::Error { message }) => {
                RemoteConnectIpcOutcome::Error(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    message,
                ))
            }
            Ok(IpcResponse::Ok | IpcResponse::ConfirmDecision { .. }) => {
                RemoteConnectIpcOutcome::Error(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected IPC response for remote connect confirm",
                ))
            }
            Err(IpcUnavailableOrError::Unavailable) => {
                RemoteConnectIpcOutcome::Unavailable
            }
            Err(IpcUnavailableOrError::Error(err)) => {
                RemoteConnectIpcOutcome::Error(err)
            }
        }
    }

    /// Internal confirm implementation allowing an injected spawner.
    pub fn confirm_with_spawner<S>(
        &self,
        request: &RemoteConnectConfirmRequest,
        spawner: S,
    ) -> io::Result<RemoteConnectConfirmDecision>
    where
        S: FnOnce(
            &RemoteConnectConfirmRequest,
        ) -> io::Result<RemoteConnectConfirmDecision>,
    {
        self.confirm_with_spawner_and_reachability(
            request,
            spawner,
            is_desktop_reachable,
        )
    }

    /// Internal confirm allowing injected spawner and desktop reachability.
    pub fn confirm_with_spawner_and_reachability<S, R>(
        &self,
        request: &RemoteConnectConfirmRequest,
        spawner: S,
        desktop_reachable: R,
    ) -> io::Result<RemoteConnectConfirmDecision>
    where
        S: FnOnce(
            &RemoteConnectConfirmRequest,
        ) -> io::Result<RemoteConnectConfirmDecision>,
        R: FnOnce() -> bool,
    {
        match self.try_ipc(request) {
            RemoteConnectIpcOutcome::Decision(decision) => return Ok(decision),
            RemoteConnectIpcOutcome::Error(err) => return Err(err),
            RemoteConnectIpcOutcome::Unavailable => {}
        }

        if !desktop_reachable() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "remote connection confirmation requires an active graphical \
                 desktop",
            ));
        }

        spawner(request)
    }
}

impl RemoteConnectConfirmAdapter for GuiRemoteConnectConfirm {
    fn confirm(
        &self,
        request: &RemoteConnectConfirmRequest,
    ) -> io::Result<RemoteConnectConfirmDecision> {
        self.confirm_with_spawner(request, spawn_remote_connect_dialog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_mode_serde_variants() {
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
        struct Wrapper {
            #[serde(with = "removal_mode_serde")]
            mode: RemovalMode,
        }

        let perm = Wrapper {
            mode: RemovalMode::Permanent,
        };
        let perm_json = serde_json::to_string(&perm).unwrap();
        assert_eq!(perm_json, "{\"mode\":\"permanent\"}");
        let perm_parsed: Wrapper = serde_json::from_str(&perm_json).unwrap();
        assert_eq!(perm_parsed.mode, RemovalMode::Permanent);

        let trash = Wrapper {
            mode: RemovalMode::Trash,
        };
        let trash_json = serde_json::to_string(&trash).unwrap();
        assert_eq!(trash_json, "{\"mode\":\"trash\"}");
        let trash_parsed: Wrapper = serde_json::from_str(&trash_json).unwrap();
        assert_eq!(trash_parsed.mode, RemovalMode::Trash);

        let err = serde_json::from_str::<Wrapper>("{\"mode\":\"invalid\"}")
            .unwrap_err();
        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn cleanup_confirm_request_and_decision_serde() {
        let req = CleanupConfirmRequest {
            root: PathBuf::from("/Users/test"),
            title: "Cleanup Test".to_string(),
            summary: Some("Found 1 item".to_string()),
            items: vec![CleanupConfirmItem {
                path: PathBuf::from("/Users/test/.cache"),
                bytes: 1024,
                mode: RemovalMode::Trash,
                reason: Some("old cache".to_string()),
                git_warning: None,
            }],
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: CleanupConfirmRequest =
            serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);

        let conf = CleanupConfirmDecision::Confirmed {
            selected_paths: vec![PathBuf::from("/Users/test/.cache")],
        };
        let conf_json = serde_json::to_string(&conf).unwrap();
        assert!(conf_json.contains("\"status\":\"confirmed\""));
        let parsed_conf: CleanupConfirmDecision =
            serde_json::from_str(&conf_json).unwrap();
        assert_eq!(parsed_conf, conf);

        let canc = CleanupConfirmDecision::Cancelled;
        let canc_json = serde_json::to_string(&canc).unwrap();
        assert!(canc_json.contains("\"status\":\"cancelled\""));
        let parsed_canc: CleanupConfirmDecision =
            serde_json::from_str(&canc_json).unwrap();
        assert_eq!(parsed_canc, canc);
    }

    #[test]
    fn ipc_token_generation_and_load() {
        let token = generate_ipc_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));

        let temp = tempfile::tempdir().unwrap();
        let token_path = temp.path().join("nested/ipc.token");

        let loaded = load_or_create_ipc_token(&token_path).unwrap();
        assert_eq!(loaded.len(), 64);

        // Permissions on Unix must be 0600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(&token_path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Second load reads existing token
        let reloaded = load_or_create_ipc_token(&token_path).unwrap();
        assert_eq!(reloaded, loaded);
    }

    #[test]
    #[cfg(unix)]
    fn socket_path_length_fallback() {
        let short_xdg = PathBuf::from("/run/user/1000");
        let fallback = PathBuf::from("/tmp/ruum-501.sock");
        let chosen =
            select_socket_path(Some(&short_xdg), None, fallback.clone());
        assert_eq!(chosen, short_xdg.join("ruum-ipc.sock"));

        // Oversized path (>= 96 bytes) triggers fallback
        let long_path = PathBuf::from(
            "/very/long/temporary/directory/path/that/exceeds/the/sun_path/\
             limit/for/local/domain/sockets/and/should/trigger/fallback",
        );
        let chosen_fallback = select_socket_path(
            Some(&long_path),
            Some(&long_path),
            fallback.clone(),
        );
        assert_eq!(chosen_fallback, fallback);
    }

    #[test]
    fn desktop_reachability_detection() {
        // macOS tests
        if cfg!(target_os = "macos") {
            let headless = is_desktop_reachable_env(|k| match k {
                "SSH_CONNECTION" => Some("127.0.0.1 1234 127.0.0.1 22".into()),
                _ => None,
            });
            assert!(!headless);

            let ssh_with_x11 = is_desktop_reachable_env(|k| match k {
                "SSH_CONNECTION" => Some("127.0.0.1 1234 127.0.0.1 22".into()),
                "DISPLAY" => Some(":0".into()),
                _ => None,
            });
            assert!(ssh_with_x11);

            let local = is_desktop_reachable_env(|_| None);
            assert!(local);
        }

        // Linux tests
        if cfg!(target_os = "linux") {
            let wayland = is_desktop_reachable_env(|k| match k {
                "WAYLAND_DISPLAY" => Some("wayland-0".into()),
                _ => None,
            });
            assert!(wayland);

            let x11 = is_desktop_reachable_env(|k| match k {
                "DISPLAY" => Some(":0".into()),
                _ => None,
            });
            assert!(x11);

            let none = is_desktop_reachable_env(|_| None);
            assert!(!none);
        }
    }

    #[test]
    fn ipc_round_trip_and_token_verification() {
        let temp = tempfile::tempdir().unwrap();
        let token = "test_token_12345678901234567890123456789012".to_string();

        #[cfg(unix)]
        let socket_or_port = temp.path().join("ruum-test.sock");
        #[cfg(windows)]
        let socket_or_port = temp.path().join("ipc.port");

        let listener = IpcListenerGuard::bind(&socket_or_port).unwrap();
        let server_token = token.clone();

        let server_thread = std::thread::spawn(move || {
            // First message: ConfirmCleanup with valid token
            listener
                .accept_and_handle(&server_token, |req| match req {
                    IpcRequest::ConfirmCleanup { request, .. } => {
                        let selected =
                            request.items.into_iter().map(|i| i.path).collect();
                        Ok(IpcResponse::ConfirmDecision {
                            decision: CleanupConfirmDecision::Confirmed {
                                selected_paths: selected,
                            },
                        })
                    }
                    IpcRequest::ConfirmRemoteConnect { .. } => {
                        Ok(IpcResponse::RemoteConnectDecision {
                            decision: RemoteConnectConfirmDecision::Denied,
                        })
                    }
                    IpcRequest::NotifyWatchdogUpdate { .. }
                    | IpcRequest::ShowNotification { .. }
                    | IpcRequest::OpenWindow { .. } => Ok(IpcResponse::Ok),
                })
                .unwrap();

            // Second message: wrong token
            listener
                .accept_and_handle(&server_token, |_| Ok(IpcResponse::Ok))
                .unwrap();
        });

        // 1. Send valid ConfirmCleanup
        let client = GuiCleanupConfirm::with_socket_and_token(
            socket_or_port.clone(),
            token,
        );
        let req = CleanupConfirmRequest {
            root: PathBuf::from("/test"),
            title: "Test".to_string(),
            summary: None,
            items: vec![CleanupConfirmItem {
                path: PathBuf::from("/test/file"),
                bytes: 100,
                mode: RemovalMode::Trash,
                reason: None,
                git_warning: None,
            }],
        };
        let decision = client.confirm(&req).unwrap();
        assert_eq!(
            decision,
            CleanupConfirmDecision::Confirmed {
                selected_paths: vec![PathBuf::from("/test/file")],
            }
        );

        // 2. Send invalid token
        let bad_client = GuiCleanupConfirm::with_socket_and_token(
            socket_or_port,
            "wrong_token_abcdefghijklmnopqrstuvwxyz".to_string(),
        );
        let err = bad_client.confirm_with_spawner(&req, |_| {
            panic!("spawner should not be reached");
        });
        // Token mismatch returns PermissionDenied error
        assert!(err.is_err());
        assert_eq!(err.unwrap_err().kind(), io::ErrorKind::PermissionDenied);

        server_thread.join().unwrap();
    }

    #[test]
    fn gui_cleanup_confirm_spawner_and_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let unused_sock = temp.path().join("nonexistent.sock");
        let client = GuiCleanupConfirm::with_socket_and_token(
            unused_sock,
            "dummy_token_123456789012345678901234".to_string(),
        );

        let req = CleanupConfirmRequest {
            root: PathBuf::from("/test"),
            title: "Fallback Test".to_string(),
            summary: None,
            items: vec![],
        };

        // When display is reachable, spawner is called
        if is_desktop_reachable() {
            let decision = client
                .confirm_with_spawner(&req, |_| {
                    Ok(CleanupConfirmDecision::Cancelled)
                })
                .unwrap();
            assert_eq!(decision, CleanupConfirmDecision::Cancelled);
        }
    }

    #[test]
    fn gui_cleanup_confirm_fails_closed_when_display_unreachable() {
        let temp = tempfile::tempdir().unwrap();
        let unused_sock = temp.path().join("nonexistent.sock");
        let client = GuiCleanupConfirm::with_socket_and_token(
            unused_sock,
            "dummy_token_123456789012345678901234".to_string(),
        );

        let req = CleanupConfirmRequest {
            root: PathBuf::from("/test"),
            title: "Headless Test".to_string(),
            summary: None,
            items: vec![],
        };

        // Force desktop_reachable to return false
        let res = client.confirm_with_spawner_and_reachability(
            &req,
            |_| panic!("spawner must not be called when display unreachable"),
            || false,
        );
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::NotConnected);
    }

    #[test]
    fn ipc_read_timeout_distinguishes_interactive_confirm_from_notify() {
        let confirm_req = IpcRequest::ConfirmCleanup {
            token: "tok".to_string(),
            request: CleanupConfirmRequest {
                root: PathBuf::from("/tmp"),
                title: "Confirm".to_string(),
                summary: None,
                items: Vec::new(),
            },
        };
        let notify_req = IpcRequest::NotifyWatchdogUpdate {
            token: "tok".to_string(),
        };

        assert_eq!(ipc_read_timeout(&confirm_req), IPC_CONFIRM_READ_TIMEOUT);
        assert_eq!(
            IPC_CONFIRM_READ_TIMEOUT,
            std::time::Duration::from_mins(10)
        );
        assert_eq!(ipc_read_timeout(&notify_req), IPC_NOTIFY_READ_TIMEOUT);
        assert_eq!(IPC_NOTIFY_READ_TIMEOUT, std::time::Duration::from_secs(10));
    }

    #[test]
    fn ipc_show_notification_and_open_window_serde_and_timeout() {
        let show_req = IpcRequest::ShowNotification {
            token: "tok".to_string(),
            tag: "ruum.low_disk_space".to_string(),
            title: "Low Disk Space".to_string(),
            body: "Free space is low".to_string(),
        };
        let show_json = serde_json::to_string(&show_req).unwrap();
        let show_parsed: IpcRequest = serde_json::from_str(&show_json).unwrap();
        assert_eq!(show_parsed, show_req);
        assert_eq!(ipc_read_timeout(&show_req), IPC_NOTIFY_READ_TIMEOUT);

        let open_req = IpcRequest::OpenWindow {
            token: "tok".to_string(),
            tag: "ruum.ai_analysis".to_string(),
        };
        let open_json = serde_json::to_string(&open_req).unwrap();
        let open_parsed: IpcRequest = serde_json::from_str(&open_json).unwrap();
        assert_eq!(open_parsed, open_req);
        assert_eq!(ipc_read_timeout(&open_req), IPC_NOTIFY_READ_TIMEOUT);
    }

    #[test]
    fn ipc_show_notification_and_open_window_auth_verification() {
        let temp = tempfile::tempdir().unwrap();
        let token = "valid_token_1234567890123456789012345678".to_string();

        #[cfg(unix)]
        let socket_or_port = temp.path().join("notif-auth.sock");
        #[cfg(windows)]
        let socket_or_port = temp.path().join("notif-auth.port");

        let listener = IpcListenerGuard::bind(&socket_or_port).unwrap();
        let server_token = token.clone();
        let server_thread = std::thread::spawn(move || {
            for _ in 0..3 {
                listener
                    .accept_and_handle(&server_token, |req| match req {
                        IpcRequest::ShowNotification { tag, .. } => {
                            assert_eq!(tag, "ruum.low_disk_space");
                            Ok(IpcResponse::Ok)
                        }
                        IpcRequest::OpenWindow { tag, .. } => {
                            assert_eq!(tag, "ruum.ai_analysis");
                            Ok(IpcResponse::Ok)
                        }
                        other => panic!("unexpected IPC request: {other:?}"),
                    })
                    .unwrap();
            }
        });

        let show_ok = send_ipc_message(
            &socket_or_port,
            &IpcRequest::ShowNotification {
                token: token.clone(),
                tag: "ruum.low_disk_space".to_string(),
                title: "Title".to_string(),
                body: "Body".to_string(),
            },
        )
        .unwrap();
        assert_eq!(show_ok, IpcResponse::Ok);

        let open_ok = send_ipc_message(
            &socket_or_port,
            &IpcRequest::OpenWindow {
                token,
                tag: "ruum.ai_analysis".to_string(),
            },
        )
        .unwrap();
        assert_eq!(open_ok, IpcResponse::Ok);

        let bad_auth = send_ipc_message(
            &socket_or_port,
            &IpcRequest::OpenWindow {
                token: "wrong_token_1234567890123456789012345678".to_string(),
                tag: "ruum.ai_analysis".to_string(),
            },
        )
        .unwrap();
        assert!(matches!(bad_auth, IpcResponse::Error { .. }));

        server_thread.join().unwrap();
    }

    #[test]
    fn remote_connect_confirm_serde_and_timeout() {
        let req = RemoteConnectConfirmRequest {
            target_label: "prod-web".to_owned(),
            alias: Some("prod-web".to_owned()),
            host: "10.0.1.25".to_owned(),
            port: 2222,
            username: "deploy".to_owned(),
            auth_mode: "private_key".to_owned(),
            tool_name: "scan_directory".to_owned(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: RemoteConnectConfirmRequest =
            serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);

        for decision in [
            RemoteConnectConfirmDecision::Approved,
            RemoteConnectConfirmDecision::Denied,
        ] {
            let d_json = serde_json::to_string(&decision).unwrap();
            let d_parsed: RemoteConnectConfirmDecision =
                serde_json::from_str(&d_json).unwrap();
            assert_eq!(d_parsed, decision);
        }

        let ipc_req = IpcRequest::ConfirmRemoteConnect {
            token: "tok".to_owned(),
            request: req,
        };
        assert_eq!(ipc_read_timeout(&ipc_req), IPC_CONFIRM_READ_TIMEOUT);
    }

    #[test]
    fn gui_remote_connect_confirm_ipc_spawner_and_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let token = "remote_connect_token_12345678901234567890".to_owned();

        #[cfg(unix)]
        let socket_or_port = temp.path().join("remote-confirm.sock");
        #[cfg(windows)]
        let socket_or_port = temp.path().join("remote-confirm.port");

        let req = RemoteConnectConfirmRequest {
            target_label: "prod".to_owned(),
            alias: Some("prod".to_owned()),
            host: "10.0.0.5".to_owned(),
            port: 22,
            username: "ubuntu".to_owned(),
            auth_mode: "password".to_owned(),
            tool_name: "list_volumes".to_owned(),
        };

        // 1. Live IPC server returns Approved.
        let listener = IpcListenerGuard::bind(&socket_or_port).unwrap();
        let server_token = token.clone();
        let handle = std::thread::spawn(move || {
            listener
                .accept_and_handle(&server_token, |frame| match frame {
                    IpcRequest::ConfirmRemoteConnect { request, .. } => {
                        assert_eq!(request.host, "10.0.0.5");
                        Ok(IpcResponse::RemoteConnectDecision {
                            decision: RemoteConnectConfirmDecision::Approved,
                        })
                    }
                    _ => panic!("unexpected IPC request"),
                })
                .unwrap();
        });

        let client = GuiRemoteConnectConfirm::with_socket_and_token(
            socket_or_port,
            token.clone(),
        );
        let decision = client
            .confirm_with_spawner(&req, |_| {
                panic!("spawner must not run when IPC answers");
            })
            .unwrap();
        assert_eq!(decision, RemoteConnectConfirmDecision::Approved);
        handle.join().unwrap();

        // 2. Unreachable socket + headless desktop -> Fail-Closed (NotConnected).
        let missing_sock = temp.path().join("missing.sock");
        let headless_client =
            GuiRemoteConnectConfirm::with_socket_and_token(missing_sock, token);
        let err = headless_client
            .confirm_with_spawner_and_reachability(
                &req,
                |_| panic!("spawner must not run when display unreachable"),
                || false,
            )
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotConnected);

        // 3. Unreachable socket + reachable desktop -> delegates to spawner.
        let denied = headless_client
            .confirm_with_spawner_and_reachability(
                &req,
                |_| Ok(RemoteConnectConfirmDecision::Denied),
                || true,
            )
            .unwrap();
        assert_eq!(denied, RemoteConnectConfirmDecision::Denied);
    }

    #[test]
    fn resolve_confirm_gui_binary_prefers_override_and_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let cli_name = if cfg!(windows) { "ruum.exe" } else { "ruum" };
        let gui_name = if cfg!(windows) {
            "ruum-gui.exe"
        } else {
            "ruum-gui"
        };
        let cli_bin = dir.path().join(cli_name);
        let gui_bin = dir.path().join(gui_name);
        let custom_gui = dir.path().join("custom-gui");
        fs::write(&cli_bin, b"cli").unwrap();
        fs::write(&gui_bin, b"gui").unwrap();
        fs::write(&custom_gui, b"custom").unwrap();

        let sibling_env = crate::desktop_launch::DesktopLaunchEnv {
            current_exe: cli_bin.clone(),
            ..Default::default()
        };
        assert_eq!(
            resolve_confirm_gui_binary_with(&sibling_env).unwrap(),
            dunce::canonicalize(&gui_bin).unwrap()
        );

        let override_env = crate::desktop_launch::DesktopLaunchEnv {
            current_exe: cli_bin,
            ruum_gui_bin: Some(custom_gui.clone()),
            ..Default::default()
        };
        assert_eq!(
            resolve_confirm_gui_binary_with(&override_env).unwrap(),
            custom_gui
        );

        let missing_env = crate::desktop_launch::DesktopLaunchEnv {
            ruum_gui_bin: Some(dir.path().join("nonexistent-gui")),
            ..Default::default()
        };
        assert_eq!(
            resolve_confirm_gui_binary_with(&missing_env)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
