//! Unified in-memory SSH session orchestration (agent + askpass + `ControlMaster`).
//!
//! Supports both private-key authentication (via an ephemeral [`SshAgentRegistry`]
//! listener on `SSH_AUTH_SOCK`) and password authentication (via a local
//! token-authenticated `askpass` IPC listener with `SSH_ASKPASS_REQUIRE=force`).
//!
//! Invariants:
//! - Never reads, writes, or modifies `~/.ssh/config` (`-F none`).
//! - Never writes private keys or passwords to disk (except the Windows
//!   ephemeral askpass token file inside the session directory, wiped on drop).
//! - Never modifies remote `~/.ssh/authorized_keys`.
//! - Uses `0o700` private session directories and `ControlMaster` only on Unix;
//!   applies `CREATE_NO_WINDOW` (`0x0800_0000`) to child processes on Windows.

use std::fmt::{self, Write as _};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read as _, Write};
#[cfg(windows)]
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use ssh_key::PrivateKey;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::credentials::{LazyCredentialStore, SshCredentialSecret};
use crate::ssh_agent::{LaunchTarget, SshAgentRegistry};
use crate::storage::{AuthMode, SshProfile};

/// Internal subcommand marker injected into `SSH_ASKPASS` child environments.
pub const ENV_ASKPASS_CMD: &str = "RUUM_ASKPASS_CMD";

/// Local IPC endpoint (Unix socket path or `127.0.0.1:<port>`) for askpass.
pub const ENV_ASKPASS_ENDPOINT: &str = "RUUM_ASKPASS_ENDPOINT";

/// Ephemeral 256-bit hexadecimal authentication token for askpass requests.
pub const ENV_ASKPASS_TOKEN: &str = "RUUM_ASKPASS_TOKEN";

/// Optional path to a file containing the askpass authentication token.
pub const ENV_ASKPASS_TOKEN_FILE: &str = "RUUM_ASKPASS_TOKEN_FILE";

/// Optional override path to the `ruum` binary invoked as `SSH_ASKPASS`.
pub const ENV_ASKPASS_BIN: &str = "RUUM_ASKPASS_BIN";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const MAX_ASKPASS_FRAME_BYTES: u64 = 4096;

/// Validated remote SSH endpoint derived from explicit host/port/user or a
/// saved [`SshProfile`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshEndpoint {
    pub target: LaunchTarget,
    pub label: String,
}

impl SshEndpoint {
    /// Validate `(host, port, username)` and construct an [`SshEndpoint`].
    pub fn new(host: &str, port: u16, username: &str) -> Result<Self, String> {
        let target = LaunchTarget::new(host, port, username)?;
        let label = target.destination();
        Ok(Self { target, label })
    }

    /// Validate `(host, port, username)` with a human-readable display label.
    pub fn with_label(
        host: &str,
        port: u16,
        username: &str,
        label: impl Into<String>,
    ) -> Result<Self, String> {
        let target = LaunchTarget::new(host, port, username)?;
        let label = label.into();
        let label = if label.trim().is_empty() {
            target.destination()
        } else {
            label
        };
        Ok(Self { target, label })
    }

    /// Construct an [`SshEndpoint`] from a persisted [`SshProfile`].
    pub fn from_profile(profile: &SshProfile) -> Result<Self, String> {
        Self::with_label(
            &profile.host,
            profile.port,
            &profile.username,
            profile.alias.clone(),
        )
    }

    #[must_use]
    pub const fn target(&self) -> &LaunchTarget {
        &self.target
    }

    #[must_use]
    pub fn host(&self) -> &str {
        self.target.host()
    }

    #[must_use]
    pub fn ssh_host(&self) -> &str {
        self.target.ssh_host()
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.target.port()
    }

    #[must_use]
    pub fn username(&self) -> &str {
        self.target.username()
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn destination(&self) -> String {
        self.target.destination()
    }

    /// Build a baseline `ssh` [`Command`] for this endpoint without reading
    /// `~/.ssh/config` (`-F none`).
    #[must_use]
    pub fn build_ssh_command(
        &self,
        extra_ssh_args: &[&str],
        remote_command: &str,
    ) -> Command {
        let mut command = Command::new("ssh");
        command.args([
            "-F",
            "none",
            "-T",
            "-p",
            &self.port().to_string(),
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
        ]);
        command.args(extra_ssh_args);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        command
            .arg("--")
            .arg(self.destination())
            .arg(remote_command);
        command
    }
}

/// Decrypted authentication material held purely in memory for one session.
#[derive(Clone)]
pub enum SshAuthMaterial {
    PrivateKey {
        private_key_pem: Zeroizing<String>,
        passphrase: Option<Zeroizing<String>>,
    },
    Password {
        password: Zeroizing<String>,
    },
}

impl fmt::Debug for SshAuthMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrivateKey { passphrase, .. } => f
                .debug_struct("SshAuthMaterial::PrivateKey")
                .field("private_key_pem", &"[REDACTED]")
                .field("has_passphrase", &passphrase.is_some())
                .finish(),
            Self::Password { .. } => f
                .debug_struct("SshAuthMaterial::Password")
                .field("password", &"[REDACTED]")
                .finish(),
        }
    }
}

impl SshAuthMaterial {
    /// Convert a [`SshCredentialSecret`] from [`LazyCredentialStore`].
    #[must_use]
    pub fn from_credential_secret(secret: &SshCredentialSecret) -> Self {
        match secret {
            SshCredentialSecret::Password { password } => Self::Password {
                password: Zeroizing::new(password.clone()),
            },
            SshCredentialSecret::PrivateKey {
                private_key_pem,
                passphrase,
            } => Self::PrivateKey {
                private_key_pem: Zeroizing::new(private_key_pem.clone()),
                passphrase: passphrase
                    .as_deref()
                    .map(|p| Zeroizing::new(p.to_owned())),
            },
        }
    }

    /// Load and validate secret material for `profile` on demand.
    pub fn from_profile_and_store(
        profile: &SshProfile,
        creds: &LazyCredentialStore,
    ) -> Result<Self, String> {
        let Some(secret) = creds.get_ssh_secret(&profile.id)? else {
            return Err(format!(
                "未找到 SSH 卡片「{}」保存的凭据。",
                profile.alias
            ));
        };
        match (profile.auth_mode, &secret) {
            (AuthMode::Password, SshCredentialSecret::Password { .. })
            | (AuthMode::PrivateKey, SshCredentialSecret::PrivateKey { .. }) => {
                Ok(Self::from_credential_secret(&secret))
            }
            _ => Err(format!(
                "SSH 卡片「{}」的认证方式与已存凭据类型不匹配。",
                profile.alias
            )),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct AskpassRequest {
    token: String,
}

#[derive(Serialize, Deserialize)]
struct AskpassResponse {
    ok: bool,
    secret: Option<String>,
    error: Option<String>,
}

enum SessionAuthKind {
    PrivateKey {
        registry: Arc<Mutex<SshAgentRegistry>>,
        session_ref: String,
        socket_path: String,
        pubkey_path: PathBuf,
    },
    Password {
        endpoint: String,
        token: Zeroizing<String>,
        token_file: Option<PathBuf>,
        askpass_bin: PathBuf,
    },
}

/// Active, TTL-bounded in-memory SSH session configuring `ssh` subprocesses
/// without touching `~/.ssh/config` or remote `authorized_keys`.
pub struct SshSession {
    endpoint: SshEndpoint,
    auth: SessionAuthKind,
    session_dir: PathBuf,
    #[cfg(unix)]
    control_path: Option<PathBuf>,
    #[cfg(unix)]
    control_persist_secs: u64,
    cancelled: Arc<AtomicBool>,
    expires_at: Instant,
}

impl fmt::Debug for SshSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshSession")
            .field("endpoint", &self.endpoint)
            .field("session_dir", &self.session_dir)
            .field("is_closed", &self.is_closed())
            .field("is_expired", &self.is_expired())
            .finish_non_exhaustive()
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn generate_random_hex_token() -> Zeroizing<String> {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut hex = String::with_capacity(64);
    for b in bytes {
        let _ = write!(hex, "{b:02x}");
    }
    Zeroizing::new(hex)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn create_session_dir(short_id: &str) -> Result<PathBuf, String> {
    #[cfg(unix)]
    let dir = {
        let uid = rustix::process::geteuid().as_raw();
        PathBuf::from(format!("/tmp/ruum-ssh-{uid}/s-{short_id}"))
    };
    #[cfg(not(unix))]
    let dir = std::env::temp_dir().join(format!("ruum-ssh-s-{short_id}"));

    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create SSH session dir: {e}"))?;
    #[cfg(unix)]
    {
        if let Some(parent) = dir.parent() {
            let _ =
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Failed to secure SSH session dir: {e}"))?;
    }
    Ok(dir)
}

fn write_private_owner_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut opts = OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    let mut file = opts
        .open(path)
        .map_err(|e| format!("Failed to write session file: {e}"))?;
    #[cfg(unix)]
    {
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to chmod session file: {e}"))?;
    }
    file.write_all(content)
        .map_err(|e| format!("Failed to write session content: {e}"))?;
    file.flush()
        .map_err(|e| format!("Failed to flush session file: {e}"))?;
    Ok(())
}

fn resolve_askpass_binary() -> Result<PathBuf, String> {
    if let Some(override_bin) = std::env::var_os(ENV_ASKPASS_BIN)
        && !override_bin.is_empty()
    {
        return Ok(PathBuf::from(override_bin));
    }
    let current = std::env::current_exe()
        .map_err(|e| format!("Failed to resolve current executable: {e}"))?;
    let canonical = fs::canonicalize(&current).unwrap_or(current);
    let sibling_name = if cfg!(windows) { "ruum.exe" } else { "ruum" };
    if let Some(dir) = canonical.parent() {
        let sibling = dir.join(sibling_name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Ok(canonical)
}

fn handle_askpass_stream(
    mut stream: impl io::Read + io::Write,
    expected_token: &str,
    secret: &str,
    deadline: Instant,
    cancelled: &AtomicBool,
) {
    if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
        return;
    }
    let mut line = String::new();
    {
        let mut reader =
            BufReader::new((&mut stream).take(MAX_ASKPASS_FRAME_BYTES));
        if reader.read_line(&mut line).is_err() {
            return;
        }
    }
    let response = match serde_json::from_str::<AskpassRequest>(&line) {
        Ok(req)
            if !req.token.is_empty()
                && constant_time_eq(
                    req.token.as_bytes(),
                    expected_token.as_bytes(),
                ) =>
        {
            AskpassResponse {
                ok: true,
                secret: Some(secret.to_owned()),
                error: None,
            }
        }
        Ok(_) => AskpassResponse {
            ok: false,
            secret: None,
            error: Some("unauthorized askpass token".to_owned()),
        },
        Err(e) => AskpassResponse {
            ok: false,
            secret: None,
            error: Some(format!("invalid askpass request: {e}")),
        },
    };
    if let Ok(mut encoded) = serde_json::to_string(&response) {
        encoded.push('\n');
        let _ = stream.write_all(encoded.as_bytes());
        let _ = stream.flush();
    }
}

#[cfg(unix)]
fn run_unix_askpass_listener(
    listener: &UnixListener,
    socket_path: &Path,
    expected_token: &Zeroizing<String>,
    secret: &Zeroizing<String>,
    ttl: Duration,
    cancelled: &AtomicBool,
) {
    let deadline = Instant::now() + ttl;
    let _ = listener.set_nonblocking(true);
    while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                handle_askpass_stream(
                    stream,
                    expected_token.as_str(),
                    secret.as_str(),
                    deadline,
                    cancelled,
                );
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break,
        }
    }
    let _ = fs::remove_file(socket_path);
}

#[cfg(windows)]
fn run_windows_askpass_listener(
    listener: &TcpListener,
    expected_token: &Zeroizing<String>,
    secret: &Zeroizing<String>,
    ttl: Duration,
    cancelled: &AtomicBool,
) {
    let deadline = Instant::now() + ttl;
    let _ = listener.set_nonblocking(true);
    while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                handle_askpass_stream(
                    stream,
                    expected_token.as_str(),
                    secret.as_str(),
                    deadline,
                    cancelled,
                );
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break,
        }
    }
}

impl SshSession {
    /// Start an in-memory SSH session for `endpoint` using `auth`.
    pub fn start(
        endpoint: SshEndpoint,
        auth: SshAuthMaterial,
        duration_minutes: u64,
    ) -> Result<Self, String> {
        let clamped_minutes = duration_minutes.clamp(1, 60);
        let ttl = Duration::from_secs(clamped_minutes * 60);
        let expires_at = Instant::now() + ttl;
        let cancelled = Arc::new(AtomicBool::new(false));
        let short_id = Uuid::new_v4().simple().to_string();
        let short_prefix = &short_id[..8];
        let session_dir = create_session_dir(short_prefix)?;

        #[cfg(unix)]
        let control_path = Some(session_dir.join("cm.sock"));
        #[cfg(unix)]
        let control_persist_secs = (clamped_minutes * 60).min(900);

        let auth_kind = match auth {
            SshAuthMaterial::PrivateKey {
                private_key_pem,
                passphrase,
            } => {
                let mut key =
                    PrivateKey::from_openssh(private_key_pem.as_str())
                        .map_err(|e| format!("Invalid private key: {e}"))?;
                if let Some(pass) = &passphrase
                    && !pass.is_empty()
                {
                    key = key.decrypt(pass.as_bytes()).map_err(|e| {
                        format!("Failed to decrypt private key: {e}")
                    })?;
                }
                // Write ONLY the non-secret public key for `-o IdentitiesOnly=yes -i`.
                let pubkey_openssh = key
                    .public_key()
                    .to_openssh()
                    .map_err(|e| format!("Failed to encode public key: {e}"))?;
                let pubkey_path = session_dir.join("id.pub");
                write_private_owner_file(
                    &pubkey_path,
                    pubkey_openssh.as_bytes(),
                )?;

                let registry = Arc::new(Mutex::new(SshAgentRegistry::new()));
                let info = registry
                    .lock()
                    .map_err(|_| {
                        "SSH agent registry mutex poisoned".to_owned()
                    })?
                    .start_session(
                        &session_dir,
                        endpoint.label(),
                        endpoint.label(),
                        &private_key_pem,
                        passphrase.as_ref(),
                        clamped_minutes,
                    )?;
                SessionAuthKind::PrivateKey {
                    registry,
                    session_ref: info.session_ref,
                    socket_path: info.socket_path,
                    pubkey_path,
                }
            }
            SshAuthMaterial::Password { password } => {
                let token = generate_random_hex_token();
                let askpass_bin = resolve_askpass_binary()?;

                #[cfg(unix)]
                let (askpass_endpoint, token_file) = {
                    let socket_path = session_dir.join("ask.sock");
                    if socket_path.exists() {
                        let _ = fs::remove_file(&socket_path);
                    }
                    let listener =
                        UnixListener::bind(&socket_path).map_err(|e| {
                            format!("Failed to bind askpass socket: {e}")
                        })?;
                    fs::set_permissions(
                        &socket_path,
                        fs::Permissions::from_mode(0o600),
                    )
                    .map_err(|e| {
                        format!("Failed to chmod askpass socket: {e}")
                    })?;
                    let cancelled_clone = Arc::clone(&cancelled);
                    let token_clone = token.clone();
                    let secret_clone = password;
                    let socket_clone = socket_path.clone();
                    thread::Builder::new()
                        .name(format!("ssh-askpass-{short_prefix}"))
                        .spawn(move || {
                            run_unix_askpass_listener(
                                &listener,
                                &socket_clone,
                                &token_clone,
                                &secret_clone,
                                ttl,
                                &cancelled_clone,
                            );
                        })
                        .map_err(|e| {
                            format!("Failed to spawn askpass listener: {e}")
                        })?;
                    (socket_path.to_string_lossy().into_owned(), None)
                };

                #[cfg(windows)]
                let (askpass_endpoint, token_file) = {
                    let listener =
                        TcpListener::bind("127.0.0.1:0").map_err(|e| {
                            format!("Failed to bind askpass TCP listener: {e}")
                        })?;
                    let addr = listener.local_addr().map_err(|e| {
                        format!("Failed to query askpass listener addr: {e}")
                    })?;
                    let token_path = session_dir.join("askpass.token");
                    write_private_owner_file(&token_path, token.as_bytes())?;
                    let cancelled_clone = Arc::clone(&cancelled);
                    let token_clone = token.clone();
                    let secret_clone = password;
                    thread::Builder::new()
                        .name(format!("ssh-askpass-{short_prefix}"))
                        .spawn(move || {
                            run_windows_askpass_listener(
                                &listener,
                                &token_clone,
                                &secret_clone,
                                ttl,
                                &cancelled_clone,
                            );
                        })
                        .map_err(|e| {
                            format!("Failed to spawn askpass listener: {e}")
                        })?;
                    (addr.to_string(), Some(token_path))
                };

                SessionAuthKind::Password {
                    endpoint: askpass_endpoint,
                    token,
                    token_file,
                    askpass_bin,
                }
            }
        };

        Ok(Self {
            endpoint,
            auth: auth_kind,
            session_dir,
            #[cfg(unix)]
            control_path,
            #[cfg(unix)]
            control_persist_secs,
            cancelled,
            expires_at,
        })
    }

    #[must_use]
    pub const fn endpoint(&self) -> &SshEndpoint {
        &self.endpoint
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }

    #[must_use]
    pub const fn agent_socket(&self) -> Option<&str> {
        match &self.auth {
            SessionAuthKind::PrivateKey { socket_path, .. } => {
                Some(socket_path.as_str())
            }
            SessionAuthKind::Password { .. } => None,
        }
    }

    #[must_use]
    pub const fn askpass_endpoint(&self) -> Option<&str> {
        match &self.auth {
            SessionAuthKind::Password { endpoint, .. } => {
                Some(endpoint.as_str())
            }
            SessionAuthKind::PrivateKey { .. } => None,
        }
    }

    #[cfg(unix)]
    #[must_use]
    pub fn control_path(&self) -> Option<&Path> {
        self.control_path.as_deref()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn askpass_token(&self) -> Option<&str> {
        match &self.auth {
            SessionAuthKind::Password { token, .. } => Some(token.as_str()),
            SessionAuthKind::PrivateKey { .. } => None,
        }
    }

    /// Build a configured `ssh` [`Command`] bound to this in-memory session.
    ///
    /// Never reads `~/.ssh/config` (`-F none`). Applies `SSH_AUTH_SOCK` +
    /// `-o IdentitiesOnly=yes` for private keys, or `SSH_ASKPASS` +
    /// `SSH_ASKPASS_REQUIRE=force` for passwords. On Unix, attaches a short-lived
    /// `0o700` `ControlMaster` socket; on Windows, applies `CREATE_NO_WINDOW`.
    pub fn build_ssh_command(
        &self,
        extra_ssh_args: &[&str],
        remote_command: &str,
    ) -> io::Result<Command> {
        if self.is_closed() || self.is_expired() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "SSH session has closed or expired",
            ));
        }

        let mut command = Command::new("ssh");
        command.args([
            "-F",
            "none",
            "-T",
            "-p",
            &self.endpoint.port().to_string(),
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
            "-o",
            "IdentitiesOnly=yes",
        ]);
        command.args(extra_ssh_args);

        match &self.auth {
            SessionAuthKind::PrivateKey {
                socket_path,
                pubkey_path,
                ..
            } => {
                command.arg("-i").arg(pubkey_path);
                command.env("SSH_AUTH_SOCK", socket_path);
                command.env_remove("SSH_ASKPASS");
                command.env_remove("SSH_ASKPASS_REQUIRE");
            }
            SessionAuthKind::Password {
                endpoint,
                token,
                token_file,
                askpass_bin,
            } => {
                command.args(["-o", "PubkeyAuthentication=no"]);
                command.env("SSH_ASKPASS", askpass_bin);
                command.env("SSH_ASKPASS_REQUIRE", "force");
                command.env(ENV_ASKPASS_CMD, "ssh-askpass");
                command.env(ENV_ASKPASS_ENDPOINT, endpoint);
                command.env(ENV_ASKPASS_TOKEN, token.as_str());
                if let Some(path) = token_file {
                    command.env(ENV_ASKPASS_TOKEN_FILE, path);
                }
                command.env_remove("SSH_AUTH_SOCK");
            }
        }

        #[cfg(unix)]
        if let Some(control_path) = &self.control_path {
            command.args(["-o", "ControlMaster=auto"]);
            command
                .arg("-o")
                .arg(format!("ControlPath={}", control_path.display()));
            command
                .arg("-o")
                .arg(format!("ControlPersist={}", self.control_persist_secs));
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        command
            .arg("--")
            .arg(self.endpoint.destination())
            .arg(remote_command);
        Ok(command)
    }

    /// Close all active agent/askpass listeners and wipe ephemeral files.
    pub fn close(&mut self) {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        match &self.auth {
            SessionAuthKind::PrivateKey {
                registry,
                session_ref,
                pubkey_path,
                ..
            } => {
                if let Ok(mut guard) = registry.lock() {
                    guard.close_session(session_ref);
                }
                let _ = fs::remove_file(pubkey_path);
            }
            SessionAuthKind::Password { token_file, .. } => {
                if let Some(path) = token_file {
                    let _ = fs::remove_file(path);
                }
            }
        }
        #[cfg(unix)]
        if let Some(control_path) = &self.control_path {
            if control_path.exists() {
                let _ = Command::new("ssh")
                    .args(["-F", "none", "-o"])
                    .arg(format!("ControlPath={}", control_path.display()))
                    .args(["-O", "exit", "--"])
                    .arg(self.endpoint.destination())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = fs::remove_file(control_path);
        }
        let _ = fs::remove_dir_all(&self.session_dir);
    }
}

/// Query an active [`SshSession`] askpass IPC endpoint with `token`.
pub fn query_askpass_endpoint(
    endpoint: &str,
    token: &str,
) -> io::Result<Zeroizing<String>> {
    if endpoint.trim().is_empty() || token.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "missing askpass endpoint or token",
        ));
    }

    let req_json = serde_json::to_string(&AskpassRequest {
        token: token.to_owned(),
    })
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    #[cfg(unix)]
    let mut stream = UnixStream::connect(endpoint)?;
    #[cfg(windows)]
    let mut stream = TcpStream::connect(endpoint)?;

    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(req_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream.take(MAX_ASKPASS_FRAME_BYTES));
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "askpass server closed connection without response",
        ));
    }
    let resp: AskpassResponse = serde_json::from_str(&line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if !resp.ok {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            resp.error.unwrap_or_else(|| {
                "askpass authentication rejected".to_owned()
            }),
        ));
    }
    resp.secret
        .map(Zeroizing::new)
        .ok_or_else(|| io::Error::other("askpass response missing secret"))
}

/// Resolve the askpass secret using injected environment lookup (for testing).
pub fn resolve_askpass_with_env<F>(get_env: F) -> io::Result<Zeroizing<String>>
where
    F: Fn(&str) -> Option<String>,
{
    let endpoint = get_env(ENV_ASKPASS_ENDPOINT)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "missing RUUM_ASKPASS_ENDPOINT",
            )
        })?;

    let token = if let Some(tok) = get_env(ENV_ASKPASS_TOKEN)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        Zeroizing::new(tok)
    } else if let Some(token_file) = get_env(ENV_ASKPASS_TOKEN_FILE)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        let content = fs::read_to_string(token_file)?;
        let trimmed = content.trim().to_owned();
        if trimmed.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "empty RUUM_ASKPASS_TOKEN_FILE",
            ));
        }
        Zeroizing::new(trimmed)
    } else {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "missing askpass authentication token",
        ));
    };

    query_askpass_endpoint(&endpoint, token.as_str())
}

/// Resolve the askpass secret from the current process environment.
pub fn resolve_askpass_from_env() -> io::Result<Zeroizing<String>> {
    resolve_askpass_with_env(|key| std::env::var(key).ok())
}

/// Execute the `ruum ssh-askpass` protocol and write the secret to `out`.
pub fn run_askpass_cli_from_env(out: &mut impl Write) -> io::Result<()> {
    let secret = resolve_askpass_from_env()?;
    writeln!(out, "{}", secret.as_str())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;

    use ssh_key::{Algorithm, LineEnding};

    use super::*;

    fn command_args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn command_envs(cmd: &Command) -> HashMap<String, Option<OsString>> {
        cmd.get_envs()
            .map(|(k, v)| {
                (k.to_string_lossy().into_owned(), v.map(ToOwned::to_owned))
            })
            .collect()
    }

    #[test]
    fn private_key_session_configures_agent_socket_and_identities_only() {
        let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
        let pem = key.to_openssh(LineEnding::LF).unwrap();
        let endpoint = SshEndpoint::new("10.0.0.5", 2222, "deploy").unwrap();

        let mut session = SshSession::start(
            endpoint,
            SshAuthMaterial::PrivateKey {
                private_key_pem: Zeroizing::new(pem.to_string()),
                passphrase: None,
            },
            15,
        )
        .unwrap();

        let agent_sock =
            session.agent_socket().expect("agent socket").to_owned();
        assert!(session.askpass_endpoint().is_none());

        let cmd = session
            .build_ssh_command(&["-C"], "ruum-scan --stdio-v1")
            .unwrap();
        let args = command_args(&cmd);
        let envs = command_envs(&cmd);

        assert!(args.windows(2).any(|w| w == ["-F", "none"]));
        assert!(args.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(args.windows(2).any(|w| w == ["-o", "IdentitiesOnly=yes"]));
        assert!(args.contains(&"-i".to_owned()));
        assert!(args.contains(&"deploy@10.0.0.5".to_owned()));
        assert_eq!(
            envs.get("SSH_AUTH_SOCK").and_then(|v| v.as_ref()),
            Some(&OsString::from(&agent_sock))
        );

        #[cfg(unix)]
        {
            assert!(args.windows(2).any(|w| w == ["-o", "ControlMaster=auto"]));
            let cm = session.control_path().unwrap().to_path_buf();
            assert!(cm.to_string_lossy().len() < 96);
            assert!(Path::new(&agent_sock).exists());
        }

        let session_dir = session.session_dir.clone();
        session.close();
        assert!(session.build_ssh_command(&[], "true").is_err());
        assert!(!session_dir.exists());
    }

    #[test]
    fn password_session_serves_askpass_and_rejects_bad_or_missing_tokens() {
        let endpoint = SshEndpoint::new("db.internal", 22, "ops").unwrap();
        let mut session = SshSession::start(
            endpoint,
            SshAuthMaterial::Password {
                password: Zeroizing::new("top-secret-pw!".to_owned()),
            },
            15,
        )
        .unwrap();

        let ask_ep = session
            .askpass_endpoint()
            .expect("askpass endpoint")
            .to_owned();
        let token = session.askpass_token().expect("token").to_owned();

        let cmd = session
            .build_ssh_command(&[], "ruum-scan --stdio-v1")
            .unwrap();
        let args = command_args(&cmd);
        let envs = command_envs(&cmd);

        assert!(args.windows(2).any(|w| w == ["-F", "none"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["-o", "PubkeyAuthentication=no"])
        );
        assert_eq!(
            envs.get("SSH_ASKPASS_REQUIRE").and_then(|v| v.as_ref()),
            Some(&OsString::from("force"))
        );
        assert_eq!(
            envs.get(ENV_ASKPASS_CMD).and_then(|v| v.as_ref()),
            Some(&OsString::from("ssh-askpass"))
        );
        assert_eq!(
            envs.get(ENV_ASKPASS_ENDPOINT).and_then(|v| v.as_ref()),
            Some(&OsString::from(&ask_ep))
        );

        // Valid token succeeds (and works across multiple queries in session).
        let secret1 = query_askpass_endpoint(&ask_ep, &token).unwrap();
        assert_eq!(secret1.as_str(), "top-secret-pw!");
        let secret2 = resolve_askpass_with_env(|k| match k {
            ENV_ASKPASS_ENDPOINT => Some(ask_ep.clone()),
            ENV_ASKPASS_TOKEN => Some(token.clone()),
            _ => None,
        })
        .unwrap();
        assert_eq!(secret2.as_str(), "top-secret-pw!");

        // Wrong token fails closed.
        let bad_err =
            query_askpass_endpoint(&ask_ep, "wrong-token").unwrap_err();
        assert_eq!(bad_err.kind(), io::ErrorKind::PermissionDenied);

        // Missing env vars fail closed.
        assert!(resolve_askpass_with_env(|_| None).is_err());

        session.close();
        assert!(query_askpass_endpoint(&ask_ep, &token).is_err());
    }

    #[test]
    fn from_profile_and_store_validates_auth_mode_match() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::storage::StorageDb::open(dir.path().join("ruum.db"))
            .unwrap();
        let root_store = Arc::new(
            crate::credentials::test_helpers::MemoryRootSecretStore::new(
                [7_u8; 32],
            ),
        );
        let creds = LazyCredentialStore::new(
            dir.path().join(crate::credentials::CREDENTIALS_FILE_NAME),
            root_store,
        );

        let profile = db
            .add_ssh_profile(&SshProfile::new(
                "edge",
                "192.168.1.50",
                2200,
                "admin",
                AuthMode::Password,
            ))
            .unwrap();

        // Missing secret fails closed.
        assert!(
            SshAuthMaterial::from_profile_and_store(&profile, &creds).is_err()
        );

        // Mismatched secret variant fails closed.
        creds
            .set_ssh_secret(
                &profile.id,
                &SshCredentialSecret::PrivateKey {
                    private_key_pem: "unused".to_owned(),
                    passphrase: None,
                },
            )
            .unwrap();
        assert!(
            SshAuthMaterial::from_profile_and_store(&profile, &creds).is_err()
        );

        // Matching secret variant succeeds.
        creds
            .set_ssh_secret(
                &profile.id,
                &SshCredentialSecret::Password {
                    password: "pw".to_owned(),
                },
            )
            .unwrap();
        let mat =
            SshAuthMaterial::from_profile_and_store(&profile, &creds).unwrap();
        assert!(matches!(mat, SshAuthMaterial::Password { .. }));
    }
}
