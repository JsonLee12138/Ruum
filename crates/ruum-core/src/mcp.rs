//! Headless Model Context Protocol (MCP) JSON-RPC 2.0 server over `stdio`.
//!
//! Exposes disk discovery, scanning, subtree inspection, reclaimable space
//! findings, Git checkout safety checks, and removal guard planning without
//! initializing GPUI or opening any window.

use std::fmt;
use std::fs;
use std::io::{self, BufRead, Read as _, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustc_hash::FxHashMap;
use serde_json::{Value, json};

use crate::auth::{AuthAdapter, AuthFactory, AuthRequest};
use crate::classify::{Category, Reclaim};
use crate::confirm::{
    CleanupConfirmAdapter, CleanupConfirmDecision, CleanupConfirmItem,
    CleanupConfirmRequest, GuiCleanupConfirm, GuiRemoteConnectConfirm,
    RemoteConnectConfirmAdapter, RemoteConnectConfirmDecision,
    RemoteConnectConfirmRequest,
};
use crate::credentials::LazyCredentialStore;
use crate::filter::{Keep, filter};
use crate::git::{self, GitState};
use crate::identity::HostIdentity;
use crate::insights::{Finding, worth_a_look};
use crate::remote::{
    Header, RemoteRoots, RemoteScan, Request, Roots, Snapshot, validate_host,
};
use crate::remote_trash::{
    execute_remote_trash, execute_remote_trash_with_session, plan_remote,
};
use crate::removal::{
    Blocked, Plan, RemovalEvent, RemovalMode, Target, TrashBackend,
    detect_trash_backend, normalize, plan_for_mode, spawn,
};
use crate::scan::{ScanOptions, scan};
use crate::size::human_bytes;
use crate::space::{SpaceInfo, space_info, volume_root_for, volumes};
use crate::ssh_session::{SshAuthMaterial, SshEndpoint, SshSession};
use crate::storage::{
    DEFAULT_SSH_PORT, DEFAULT_SSH_SESSION_DURATION_MINUTES, SshProfile,
    StorageDb,
};
use crate::tree::{Node, NodeKind};

/// Maximum UTF-8 bytes accepted for a single JSON-RPC request line (`1 MiB`).
pub const MAX_LINE: u64 = 1 << 20;

/// Negotiated MCP protocol version.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

const DEFAULT_MAX_ENTRIES: usize = 25;
const MAX_ENTRIES_LIMIT: usize = 100;
const DEFAULT_RECLAIMABLE_LIMIT: usize = 20;
const MAX_RECLAIMABLE_LIMIT: usize = 100;

/// Cached result of a local or remote directory scan.
#[derive(Clone, Debug)]
pub struct CachedScan {
    pub tree: Node,
    pub space: Option<SpaceInfo>,
    pub home: Option<PathBuf>,
    pub platform: String,
    pub identity: Option<HostIdentity>,
    pub options: ScanOptions,
    pub scan_count: usize,
}

impl CachedScan {
    /// Rebuild the remote scan [`Header`] needed by remote removal guards.
    #[must_use]
    pub fn to_remote_header(&self, root: &Path) -> Header {
        let mut header = Header::new(
            root.to_path_buf(),
            self.home.clone(),
            self.platform.clone(),
            self.identity.clone(),
        );
        header.space = self.space;
        header
    }
}

/// Transport abstraction for remote SSH operations executed by [`McpServer`].
pub trait RemoteExecutor: Send + Sync + fmt::Debug {
    fn list_volumes_with_session(
        &self,
        session: &SshSession,
    ) -> io::Result<Roots>;
    fn list_volumes_raw(&self, host: &str) -> io::Result<Roots>;
    fn scan_with_session(
        &self,
        session: &SshSession,
        request: &Request,
    ) -> io::Result<Snapshot>;
    fn scan_raw(&self, host: &str, request: &Request) -> io::Result<Snapshot>;
    fn trash_with_session(
        &self,
        session: &SshSession,
        targets: &[Target],
        header: &Header,
    ) -> io::Result<()>;
    fn trash_raw(
        &self,
        host: &str,
        targets: &[Target],
        header: &Header,
    ) -> io::Result<()>;
}

/// Production [`RemoteExecutor`] backed by `ssh` subprocesses and [`SshSession`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRemoteExecutor;

impl RemoteExecutor for SystemRemoteExecutor {
    fn list_volumes_with_session(
        &self,
        session: &SshSession,
    ) -> io::Result<Roots> {
        RemoteRoots::discover_with_session(session).and_then(RemoteRoots::wait)
    }

    fn list_volumes_raw(&self, host: &str) -> io::Result<Roots> {
        RemoteRoots::discover(host).and_then(RemoteRoots::wait)
    }

    fn scan_with_session(
        &self,
        session: &SshSession,
        request: &Request,
    ) -> io::Result<Snapshot> {
        RemoteScan::spawn_with_session(session, request)
            .and_then(RemoteScan::wait)
    }

    fn scan_raw(&self, host: &str, request: &Request) -> io::Result<Snapshot> {
        RemoteScan::spawn(host, request).and_then(RemoteScan::wait)
    }

    fn trash_with_session(
        &self,
        session: &SshSession,
        targets: &[Target],
        header: &Header,
    ) -> io::Result<()> {
        execute_remote_trash_with_session(session, targets, header)
    }

    fn trash_raw(
        &self,
        host: &str,
        targets: &[Target],
        header: &Header,
    ) -> io::Result<()> {
        execute_remote_trash(host, targets, header)
    }
}

/// Composite key identifying an authorized remote SSH endpoint within an MCP
/// session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RemoteLeaseKey {
    pub profile_or_host: String,
    pub host: String,
    pub port: u16,
    pub username: String,
}

#[derive(Debug)]
struct RemoteConnectLease {
    expires_at: Instant,
    session: Option<SshSession>,
}

impl RemoteConnectLease {
    fn is_valid_at(&self, now: Instant) -> bool {
        now < self.expires_at
            && self
                .session
                .as_ref()
                .is_none_or(|sess| !sess.is_closed() && !sess.is_expired())
    }
}

enum RemoteTarget {
    Profile {
        profile: Box<SshProfile>,
        endpoint: SshEndpoint,
        cache_key: String,
    },
    RawHost {
        raw_host: String,
        host: String,
        port: u16,
        username: String,
    },
}

impl RemoteTarget {
    fn cache_key(&self) -> &str {
        match self {
            Self::Profile { cache_key, .. } => cache_key,
            Self::RawHost { raw_host, .. } => raw_host,
        }
    }

    fn lease_key(&self) -> RemoteLeaseKey {
        match self {
            Self::Profile { profile, .. } => RemoteLeaseKey {
                profile_or_host: profile.id.clone(),
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
            },
            Self::RawHost {
                raw_host,
                host,
                port,
                username,
            } => RemoteLeaseKey {
                profile_or_host: raw_host.clone(),
                host: host.clone(),
                port: *port,
                username: username.clone(),
            },
        }
    }

    fn confirm_request(&self, tool_name: &str) -> RemoteConnectConfirmRequest {
        match self {
            Self::Profile { profile, .. } => RemoteConnectConfirmRequest {
                target_label: profile.alias.clone(),
                alias: Some(profile.alias.clone()),
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
                auth_mode: profile.auth_mode.as_str().to_owned(),
                tool_name: tool_name.to_owned(),
            },
            Self::RawHost {
                raw_host,
                host,
                port,
                username,
            } => RemoteConnectConfirmRequest {
                target_label: raw_host.clone(),
                alias: None,
                host: host.clone(),
                port: *port,
                username: username.clone(),
                auth_mode: "external_ssh".to_owned(),
                tool_name: tool_name.to_owned(),
            },
        }
    }

    fn display_label(&self) -> &str {
        match self {
            Self::Profile { profile, .. } => &profile.alias,
            Self::RawHost { raw_host, .. } => raw_host,
        }
    }

    fn profile_alias(&self) -> Option<&str> {
        match self {
            Self::Profile { profile, .. } => Some(&profile.alias),
            Self::RawHost { .. } => None,
        }
    }

    fn raw_ssh_host(&self) -> Option<&str> {
        match self {
            Self::Profile { .. } => None,
            Self::RawHost { raw_host, .. } => Some(raw_host),
        }
    }

    fn default_path(&self) -> Option<&str> {
        match self {
            Self::Profile { profile, .. } => profile.default_path.as_deref(),
            Self::RawHost { .. } => None,
        }
    }
}

/// Stateful MCP session server holding the scan cache and OS auth adapter.
pub struct McpServer {
    cache: FxHashMap<(Option<String>, PathBuf), CachedScan>,
    auth: Box<dyn AuthAdapter>,
    confirm: Box<dyn CleanupConfirmAdapter>,
    remote_confirm: Box<dyn RemoteConnectConfirmAdapter>,
    remote_executor: Box<dyn RemoteExecutor>,
    storage: Option<StorageDb>,
    credentials: Option<LazyCredentialStore>,
    remote_connect_leases: FxHashMap<RemoteLeaseKey, RemoteConnectLease>,
    lease_ttl_override: Option<Duration>,
    read_only: bool,
}

impl fmt::Debug for McpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpServer")
            .field("cached_roots", &self.cache.len())
            .field("auth_os", &self.auth.os_kind())
            .field("confirm", &self.confirm)
            .field("remote_confirm", &self.remote_confirm)
            .field("has_storage", &self.storage.is_some())
            .field("has_credentials", &self.credentials.is_some())
            .field("active_remote_leases", &self.remote_connect_leases.len())
            .field("read_only", &self.read_only)
            .finish_non_exhaustive()
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new(AuthFactory::create())
    }
}

impl McpServer {
    #[must_use]
    pub fn new(auth: Box<dyn AuthAdapter>) -> Self {
        Self::with_confirm(auth, Box::new(GuiCleanupConfirm::new()))
    }

    #[must_use]
    pub fn with_confirm(
        auth: Box<dyn AuthAdapter>,
        confirm: Box<dyn CleanupConfirmAdapter>,
    ) -> Self {
        Self {
            cache: FxHashMap::default(),
            auth,
            confirm,
            remote_confirm: Box::new(GuiRemoteConnectConfirm::new()),
            remote_executor: Box::new(SystemRemoteExecutor),
            storage: None,
            credentials: None,
            remote_connect_leases: FxHashMap::default(),
            lease_ttl_override: None,
            read_only: false,
        }
    }

    /// Attach default local `StorageDb` and `LazyCredentialStore` without
    /// touching the OS keychain.
    #[must_use]
    pub fn with_default_services(mut self) -> Self {
        self.storage = StorageDb::open_default().ok();
        self.credentials = LazyCredentialStore::open_default().ok();
        self
    }

    #[must_use]
    pub fn with_storage(mut self, storage: StorageDb) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn set_storage(&mut self, storage: Option<StorageDb>) {
        self.storage = storage;
    }

    #[must_use]
    pub fn with_credentials(
        mut self,
        credentials: LazyCredentialStore,
    ) -> Self {
        self.credentials = Some(credentials);
        self
    }

    pub fn set_credentials(
        &mut self,
        credentials: Option<LazyCredentialStore>,
    ) {
        self.credentials = credentials;
    }

    #[must_use]
    pub fn with_remote_confirm(
        mut self,
        remote_confirm: Box<dyn RemoteConnectConfirmAdapter>,
    ) -> Self {
        self.remote_confirm = remote_confirm;
        self
    }

    pub fn set_remote_confirm(
        &mut self,
        remote_confirm: Box<dyn RemoteConnectConfirmAdapter>,
    ) {
        self.remote_confirm = remote_confirm;
    }

    #[must_use]
    pub fn with_remote_executor(
        mut self,
        remote_executor: Box<dyn RemoteExecutor>,
    ) -> Self {
        self.remote_executor = remote_executor;
        self
    }

    #[must_use]
    pub const fn with_lease_ttl(mut self, ttl: Duration) -> Self {
        self.lease_ttl_override = Some(ttl);
        self
    }

    #[must_use]
    pub fn remote_lease_count(&self) -> usize {
        self.remote_connect_leases.len()
    }

    #[cfg(test)]
    pub(crate) fn expire_remote_leases_for_test(&mut self) {
        let past = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        for lease in self.remote_connect_leases.values_mut() {
            lease.expires_at = past;
        }
    }

    #[must_use]
    pub const fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub const fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn set_confirm(&mut self, confirm: Box<dyn CleanupConfirmAdapter>) {
        self.confirm = confirm;
    }

    #[must_use]
    pub const fn cache(
        &self,
    ) -> &FxHashMap<(Option<String>, PathBuf), CachedScan> {
        &self.cache
    }

    pub fn insert_cached(
        &mut self,
        ssh_host: Option<String>,
        root: PathBuf,
        entry: CachedScan,
    ) {
        self.cache.insert((ssh_host, root), entry);
    }

    /// Invalidate any cached entry for `ssh_host` whose root is an ancestor or
    /// descendant of `path` so subsequent calls never read stale sizes.
    pub fn invalidate_overlapping(
        &mut self,
        ssh_host: Option<&str>,
        path: &Path,
    ) {
        let norm = if ssh_host.is_some() {
            normalize_remote_path(path)
        } else {
            normalize(path)
        };
        self.cache.retain(|(host, root), _| {
            if host.as_deref() != ssh_host {
                return true;
            }
            let root_norm = if ssh_host.is_some() {
                normalize_remote_path(root)
            } else {
                normalize(root)
            };
            !(root_norm.starts_with(&norm) || norm.starts_with(&root_norm))
        });
    }

    /// Serve newline-delimited JSON-RPC 2.0 messages until `reader` hits EOF.
    pub fn serve(
        &mut self,
        reader: &mut impl BufRead,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let read_bytes =
                reader.by_ref().take(MAX_LINE + 1).read_line(&mut line)?;
            if read_bytes == 0 {
                break;
            }
            if read_bytes as u64 > MAX_LINE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "oversized JSON-RPC line exceeds 1 MiB limit",
                ));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(trimmed) {
                Ok(val) => val,
                Err(err) => {
                    write_error(
                        writer,
                        &Value::Null,
                        -32700,
                        &format!("Parse error: {err}"),
                    )?;
                    continue;
                }
            };
            self.handle_message(&message, writer)?;
        }
        Ok(())
    }

    fn handle_message(
        &mut self,
        message: &Value,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let Some(obj) = message.as_object() else {
            return write_error(
                writer,
                &Value::Null,
                -32600,
                "Invalid Request",
            );
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            return write_error(
                writer,
                &id.unwrap_or(Value::Null),
                -32600,
                "Invalid Request: missing method",
            );
        };
        // JSON-RPC notifications omit `id` and must not produce a response.
        let Some(request_id) = id else {
            return Ok(());
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => write_result(
                writer,
                &request_id,
                &json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "ruum",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            ),
            "ping" => write_result(writer, &request_id, &json!({})),
            "tools/list" => write_result(
                writer,
                &request_id,
                &json!({ "tools": tool_definitions(self.read_only) }),
            ),
            "tools/call" => self.handle_tool_call(&request_id, &params, writer),
            _ => write_error(
                writer,
                &request_id,
                -32601,
                &format!("Method not found: {method}"),
            ),
        }
    }

    fn handle_tool_call(
        &mut self,
        request_id: &Value,
        params: &Value,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return write_error(
                writer,
                request_id,
                -32602,
                "Invalid params: missing tool name",
            );
        };
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(val) if val.is_object() => val.clone(),
            Some(_) => {
                return write_error(
                    writer,
                    request_id,
                    -32602,
                    "Invalid params: arguments must be an object",
                );
            }
        };
        if !is_known_tool(name) {
            return write_error(
                writer,
                request_id,
                -32601,
                &format!("Unknown tool: {name}"),
            );
        }
        if self.read_only
            && matches!(name, "trash_paths" | "delete_permanently")
        {
            let body = json!({
                "content": [{
                    "type": "text",
                    "text": format!("Tool '{name}' is unavailable: server is running in read-only mode")
                }],
                "isError": true,
            });
            return write_result(writer, request_id, &body);
        }
        let outcome = self.dispatch_tool(name, &args);
        let body = match outcome {
            Ok(payload) => {
                let text = serde_json::to_string(&payload)
                    .unwrap_or_else(|err| err.to_string());
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false,
                })
            }
            Err(err_text) => json!({
                "content": [{ "type": "text", "text": err_text }],
                "isError": true,
            }),
        };
        write_result(writer, request_id, &body)
    }

    fn dispatch_tool(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Result<Value, String> {
        match name {
            "list_ssh_profiles" => self.tool_list_ssh_profiles(),
            "list_volumes" => self.tool_list_volumes(args),
            "scan_directory" => self.tool_scan_directory(args),
            "inspect_subtree" => self.tool_inspect_subtree(args),
            "find_reclaimable" => self.tool_find_reclaimable(args),
            "check_git_safety" => Self::tool_check_git_safety(args),
            "plan_removal" => self.tool_plan_removal(args),
            "trash_paths" => self.tool_trash_paths(args),
            "delete_permanently" => self.tool_delete_permanently(args),
            _ => Err(format!("Unsupported tool: {name}")),
        }
    }

    fn resolved_ssh_session_duration_minutes(&self) -> u64 {
        self.storage
            .as_ref()
            .and_then(|db| db.load_app_settings().ok())
            .map_or(DEFAULT_SSH_SESSION_DURATION_MINUTES, |s| {
                s.ssh_session_duration_minutes
            })
            .max(1)
    }

    fn resolve_optional_remote(
        &self,
        args: &Value,
    ) -> Result<Option<RemoteTarget>, String> {
        let profile_arg = optional_trimmed_str(args, "profile")?;
        let host_arg = optional_trimmed_str(args, "ssh_host")?;
        if profile_arg.is_some() && host_arg.is_some() {
            return Err(
                "Specify either 'profile' or 'ssh_host', not both.".to_owned()
            );
        }
        if let Some(profile_key) = profile_arg {
            let db = self.storage.as_ref().ok_or_else(|| {
                format!(
                    "SSH profile '{profile_key}' unavailable: profile \
                     database not initialized"
                )
            })?;
            let profile = db
                .get_ssh_profile(profile_key)
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    format!("SSH profile '{profile_key}' not found")
                })?;
            let endpoint = SshEndpoint::from_profile(&profile)?;
            let cache_key = format!("profile:{}", profile.id);
            return Ok(Some(RemoteTarget::Profile {
                profile: Box::new(profile),
                endpoint,
                cache_key,
            }));
        }
        if let Some(raw_host) = host_arg {
            let (host, port, username) = parse_raw_ssh_host(raw_host)?;
            return Ok(Some(RemoteTarget::RawHost {
                raw_host: raw_host.to_owned(),
                host,
                port,
                username,
            }));
        }
        Ok(None)
    }

    fn has_valid_lease(&self, lease_key: &RemoteLeaseKey) -> bool {
        let now = Instant::now();
        self.remote_connect_leases
            .get(lease_key)
            .is_some_and(|lease| lease.is_valid_at(now))
    }

    /// Enforce Fail-Closed user confirmation or valid session lease BEFORE
    /// reading any keychain secret or opening any SSH transport.
    fn ensure_remote_authorized(
        &mut self,
        target: &RemoteTarget,
        tool_name: &str,
    ) -> Result<RemoteLeaseKey, String> {
        let lease_key = target.lease_key();
        if self.has_valid_lease(&lease_key) {
            return Ok(lease_key);
        }
        self.remote_connect_leases.remove(&lease_key);

        let request = target.confirm_request(tool_name);
        match self.remote_confirm.confirm(&request) {
            Ok(RemoteConnectConfirmDecision::Approved) => {
                let duration_minutes =
                    self.resolved_ssh_session_duration_minutes();
                let ttl = self.lease_ttl_override.unwrap_or_else(|| {
                    Duration::from_secs(duration_minutes * 60)
                });
                self.remote_connect_leases.insert(
                    lease_key.clone(),
                    RemoteConnectLease {
                        expires_at: Instant::now() + ttl,
                        session: None,
                    },
                );
                Ok(lease_key)
            }
            Ok(RemoteConnectConfirmDecision::Denied) => Err(format!(
                "Remote SSH connection to '{}' denied by user",
                target.display_label()
            )),
            Err(err) => {
                Err(format!("Remote connection confirmation failed: {err}"))
            }
        }
    }

    /// Ensure an active [`SshSession`] is attached to an already-authorized
    /// lease for `profile`, loading credentials from [`LazyCredentialStore`]
    /// on demand only after confirmation.
    fn ensure_profile_session(
        &mut self,
        lease_key: &RemoteLeaseKey,
        profile: &SshProfile,
        endpoint: &SshEndpoint,
    ) -> Result<(), String> {
        let needs_session = self
            .remote_connect_leases
            .get(lease_key)
            .and_then(|lease| lease.session.as_ref())
            .is_none_or(|sess| sess.is_closed() || sess.is_expired());
        if !needs_session {
            return Ok(());
        }

        let creds = self.credentials.as_ref().ok_or_else(|| {
            format!(
                "Credential store unavailable for SSH profile '{}'",
                profile.alias
            )
        })?;
        let material = SshAuthMaterial::from_profile_and_store(profile, creds)?;
        let duration_minutes = self.resolved_ssh_session_duration_minutes();
        let session =
            SshSession::start(endpoint.clone(), material, duration_minutes)?;
        let lease = self
            .remote_connect_leases
            .get_mut(lease_key)
            .ok_or_else(|| "remote authorization lease missing".to_owned())?;
        lease.session = Some(session);
        Ok(())
    }

    fn tool_list_ssh_profiles(&self) -> Result<Value, String> {
        let profiles = match &self.storage {
            Some(db) => db.list_ssh_profiles().map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        let list: Vec<Value> = profiles
            .into_iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "alias": p.alias,
                    "host": p.host,
                    "port": p.port,
                    "username": p.username,
                    "auth_mode": p.auth_mode.as_str(),
                    "default_path": p.default_path,
                })
            })
            .collect();
        Ok(json!({ "profiles": list }))
    }

    fn tool_list_volumes(&mut self, args: &Value) -> Result<Value, String> {
        if let Some(target) = self.resolve_optional_remote(args)? {
            let lease_key =
                self.ensure_remote_authorized(&target, "list_volumes")?;
            let roots = match &target {
                RemoteTarget::Profile {
                    profile, endpoint, ..
                } => {
                    self.ensure_profile_session(&lease_key, profile, endpoint)?;
                    let session = self
                        .remote_connect_leases
                        .get(&lease_key)
                        .and_then(|l| l.session.as_ref())
                        .ok_or_else(|| "SSH session missing".to_owned())?;
                    self.remote_executor
                        .list_volumes_with_session(session)
                        .map_err(|err| err.to_string())?
                }
                RemoteTarget::RawHost { raw_host, .. } => self
                    .remote_executor
                    .list_volumes_raw(raw_host)
                    .map_err(|err| err.to_string())?,
            };
            let list: Vec<Value> =
                roots.volumes.iter().map(format_volume).collect();
            return Ok(json!({
                "profile": target.profile_alias(),
                "ssh_host": target.raw_ssh_host(),
                "platform": roots.platform,
                "home": roots.home.map(|p| p.display().to_string()),
                "identity": roots.identity.map(|id| json!({
                    "hostname": id.hostname,
                    "machine_id": id.machine_id,
                })),
                "volumes": list,
            }));
        }
        let list: Vec<Value> = volumes().iter().map(format_volume).collect();
        Ok(json!({
            "profile": Value::Null,
            "ssh_host": Value::Null,
            "platform": std::env::consts::OS,
            "home": std::env::home_dir().map(|p| p.display().to_string()),
            "volumes": list,
        }))
    }

    fn tool_scan_directory(&mut self, args: &Value) -> Result<Value, String> {
        let remote = self.resolve_optional_remote(args)?;
        let path_arg = args.get("path").and_then(Value::as_str);
        let disk = args.get("disk").and_then(Value::as_bool).unwrap_or(false);
        let apparent_size = args
            .get("apparent_size")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let include_hidden = args
            .get("include_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let rescan =
            args.get("rescan").and_then(Value::as_bool).unwrap_or(false);
        let max_entries = parse_limit(
            args.get("max_entries"),
            DEFAULT_MAX_ENTRIES,
            MAX_ENTRIES_LIMIT,
        );
        let options = ScanOptions {
            apparent_size,
            include_hidden,
            ..ScanOptions::default()
        };
        if let Some(ref target) = remote {
            let _ = self.ensure_remote_authorized(target, "scan_directory")?;
        }
        let (root_path, cached_hit) = self.ensure_scanned(
            remote.as_ref(),
            path_arg,
            disk,
            &options,
            rescan,
            "scan_directory",
        )?;
        let cache_host = remote.as_ref().map(|t| t.cache_key().to_owned());
        let key = (cache_host, root_path.clone());
        let entry = self
            .cache
            .get(&key)
            .ok_or_else(|| "scan cache missing entry".to_owned())?;
        Ok(format_scan_summary(
            remote.as_ref().and_then(RemoteTarget::profile_alias),
            remote.as_ref().and_then(RemoteTarget::raw_ssh_host),
            &root_path,
            entry,
            cached_hit,
            max_entries,
        ))
    }

    fn tool_inspect_subtree(&mut self, args: &Value) -> Result<Value, String> {
        let remote = self.resolve_optional_remote(args)?;
        let root_str = required_str(args, "root")?;
        let subpath = args.get("subpath").and_then(Value::as_str);
        let filter_query = args
            .get("filter")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let max_entries = parse_limit(
            args.get("max_entries"),
            DEFAULT_MAX_ENTRIES,
            MAX_ENTRIES_LIMIT,
        );
        let (root_path, _) = self.ensure_cached_or_default_scan(
            remote.as_ref(),
            root_str,
            "inspect_subtree",
        )?;
        let cache_host = remote.as_ref().map(|t| t.cache_key().to_owned());
        let key = (cache_host, root_path.clone());
        let entry = self
            .cache
            .get(&key)
            .ok_or_else(|| "scan cache missing entry".to_owned())?;
        let (crumbs, target_path, target_node) =
            resolve_subpath(&root_path, &entry.tree, subpath)?;
        if let Some(needle) = filter_query {
            let matches =
                filter(target_node, &crumbs, needle).unwrap_or_default();
            let mut matched_nodes: Vec<(PathBuf, &Node)> = matches
                .keep
                .iter()
                .filter_map(|(match_crumbs, keep)| {
                    (*keep == Keep::Whole).then(|| {
                        let node = entry.tree.resolve(match_crumbs)?;
                        let path = path_for_crumbs(
                            &root_path,
                            &entry.tree,
                            match_crumbs,
                        );
                        Some((path, node))
                    })?
                })
                .collect();
            matched_nodes.sort_by(|left, right| {
                right
                    .1
                    .bytes
                    .cmp(&left.1.bytes)
                    .then_with(|| left.0.cmp(&right.0))
            });
            let items: Vec<Value> = matched_nodes
                .into_iter()
                .take(max_entries)
                .map(|(path, node)| format_node_entry(&path, node))
                .collect();
            return Ok(json!({
                "root": root_path.display().to_string(),
                "path": target_path.display().to_string(),
                "filter": needle,
                "match_count": matches.count,
                "matched_bytes": matches.bytes,
                "matched_files": matches.files,
                "human_size": human_bytes(matches.bytes),
                "entries": items,
            }));
        }
        let children =
            format_top_children(&target_path, target_node, max_entries);
        Ok(json!({
            "root": root_path.display().to_string(),
            "path": target_path.display().to_string(),
            "name": &*target_node.name,
            "kind": kind_label(target_node.kind),
            "bytes": target_node.bytes,
            "files": target_node.files,
            "dirs": target_node.dirs,
            "category": target_node.category.label(),
            "reclaim": target_node.reclaim.map(Reclaim::label),
            "human_size": human_bytes(target_node.bytes),
            "entries": children,
        }))
    }

    fn tool_find_reclaimable(&mut self, args: &Value) -> Result<Value, String> {
        let remote = self.resolve_optional_remote(args)?;
        let root_str = required_str(args, "root")?;
        let limit = parse_limit(
            args.get("limit"),
            DEFAULT_RECLAIMABLE_LIMIT,
            MAX_RECLAIMABLE_LIMIT,
        );
        let (root_path, _) = self.ensure_cached_or_default_scan(
            remote.as_ref(),
            root_str,
            "find_reclaimable",
        )?;
        let cache_host = remote.as_ref().map(|t| t.cache_key().to_owned());
        let key = (cache_host, root_path.clone());
        let entry = self
            .cache
            .get(&key)
            .ok_or_else(|| "scan cache missing entry".to_owned())?;
        let now = chrono::Utc::now().timestamp();
        let candidates = worth_a_look(&entry.tree, now, limit);
        let mut total_reclaimable = 0_u64;
        let items: Vec<Value> = candidates
            .into_iter()
            .filter_map(|candidate| {
                let node = entry.tree.resolve(&candidate.crumbs)?;
                let path =
                    path_for_crumbs(&root_path, &entry.tree, &candidate.crumbs);
                total_reclaimable =
                    total_reclaimable.saturating_add(candidate.bytes);
                Some(json!({
                    "name": &*node.name,
                    "path": path.display().to_string(),
                    "bytes": candidate.bytes,
                    "human_size": human_bytes(candidate.bytes),
                    "category": node.category.label(),
                    "reclaim": node.reclaim.map(Reclaim::label),
                    "finding": format_finding(&candidate.finding),
                }))
            })
            .collect();
        Ok(json!({
            "root": root_path.display().to_string(),
            "total_reclaimable_bytes": total_reclaimable,
            "human_total_reclaimable": human_bytes(total_reclaimable),
            "candidates": items,
        }))
    }

    fn tool_check_git_safety(args: &Value) -> Result<Value, String> {
        if has_non_null_arg(args, "ssh_host")
            || has_non_null_arg(args, "profile")
        {
            return Err(
                "Git safety inspection is local-only; ssh_host and profile \
                 are not supported."
                    .to_owned(),
            );
        }
        let paths = required_string_array(args, "paths")?;
        let results: Vec<Value> = paths
            .into_iter()
            .map(|raw| {
                let path = Path::new(&raw);
                let checkout = git::is_checkout(path);
                let state = git::state(path);
                json!({
                    "path": raw,
                    "is_checkout": checkout,
                    "is_clean": state.as_ref().map(GitState::is_clean),
                    "changed": state.as_ref().and_then(|s| s.changed),
                    "stashes": state.as_ref().map(|s| s.stashes),
                    "unpushed": state.as_ref().and_then(|s| s.unpushed),
                    "summary": state.as_ref().map_or_else(
                        || "not a git checkout".to_owned(),
                        GitState::summary,
                    ),
                })
            })
            .collect();
        Ok(json!({ "results": results }))
    }

    fn tool_plan_removal(&self, args: &Value) -> Result<Value, String> {
        let remote = self.resolve_optional_remote(args)?;
        let root_str = required_str(args, "root")?;
        let raw_targets = required_string_array(args, "targets")?;
        if let Some(target) = remote {
            let root_path = normalize_remote_path(Path::new(root_str));
            let cache_host = target.cache_key().to_owned();
            let key = (Some(cache_host), root_path.clone());
            let cached = self.cache.get(&key).ok_or_else(|| {
                format!(
                    "No cached remote scan for {}:{}; run scan_directory \
                     first.",
                    target.display_label(),
                    root_path.display()
                )
            })?;
            let targets = build_targets(
                &raw_targets,
                &root_path,
                Some(&cached.tree),
                true,
            );
            let header = cached.to_remote_header(&root_path);
            let remote_plan = plan_remote(&targets, &header);
            return Ok(json!({
                "profile": target.profile_alias(),
                "ssh_host": target.raw_ssh_host(),
                "root": remote_plan.root.display().to_string(),
                "targets": format_targets(&remote_plan.targets),
                "covered": format_targets(&remote_plan.covered),
                "blocked": format_blocked(&remote_plan.blocked),
                "total_bytes": remote_plan.bytes(),
                "reclaim_bytes": remote_plan.reclaim(),
                "foreign_bytes": 0,
                "human_reclaim": human_bytes(remote_plan.reclaim()),
                "trash_available": true,
                "allowed_modes": ["trash"],
            }));
        }
        let root_path = resolve_local_path(root_str)?;
        let cached_tree =
            self.cache.get(&(None, root_path.clone())).map(|c| &c.tree);
        let targets =
            build_targets(&raw_targets, &root_path, cached_tree, false);
        let local_plan =
            plan_for_mode(&targets, &root_path, RemovalMode::Permanent);
        let backend = detect_trash_backend();
        let trash_available = !matches!(backend, TrashBackend::Unavailable);
        let has_trash_target = trash_available
            && !local_plan.targets.is_empty()
            && !plan_for_mode(
                &local_plan.targets,
                &root_path,
                RemovalMode::Trash,
            )
            .blocked
            .is_empty();
        let allowed_modes = if trash_available && !has_trash_target {
            vec!["trash", "permanent"]
        } else {
            vec!["permanent"]
        };
        Ok(json!({
            "root": local_plan.root.display().to_string(),
            "targets": format_targets(&local_plan.targets),
            "covered": format_targets(&local_plan.covered),
            "blocked": format_blocked(&local_plan.blocked),
            "total_bytes": local_plan.bytes(),
            "reclaim_bytes": local_plan.reclaim(),
            "foreign_bytes": local_plan.foreign(),
            "human_reclaim": human_bytes(local_plan.reclaim()),
            "trash_available": trash_available,
            "trash_backend": backend.label(),
            "allowed_modes": allowed_modes,
        }))
    }

    fn tool_trash_paths(&mut self, args: &Value) -> Result<Value, String> {
        let remote = self.resolve_optional_remote(args)?;
        let root_str = required_str(args, "root")?;
        let raw_targets = required_string_array(args, "targets")?;
        if let Some(target) = remote {
            let root_path = normalize_remote_path(Path::new(root_str));
            let cache_host = target.cache_key().to_owned();
            let key = (Some(cache_host.clone()), root_path.clone());
            let cached = self.cache.get(&key).ok_or_else(|| {
                format!(
                    "No cached remote scan for {}:{}; run scan_directory \
                     first.",
                    target.display_label(),
                    root_path.display()
                )
            })?;
            let targets = build_targets(
                &raw_targets,
                &root_path,
                Some(&cached.tree),
                true,
            );
            let header = cached.to_remote_header(&root_path);
            let remote_plan = plan_remote(&targets, &header);
            if remote_plan.targets.is_empty() || !remote_plan.blocked.is_empty()
            {
                return Err(format_plan_rejection(
                    "trash_paths",
                    &remote_plan.root,
                    &remote_plan.blocked,
                ));
            }

            // Require an active remote SSH authorization lease (or explicit
            // user confirmation) before proceeding to item confirmation / trash.
            let lease_key =
                self.ensure_remote_authorized(&target, "trash_paths")?;

            let confirm_items: Vec<CleanupConfirmItem> = remote_plan
                .targets
                .iter()
                .map(|t| CleanupConfirmItem {
                    path: t.path.clone(),
                    bytes: t.bytes,
                    mode: RemovalMode::Trash,
                    reason: None,
                    git_warning: None,
                })
                .collect();
            let req = CleanupConfirmRequest {
                root: remote_plan.root.clone(),
                title: format!("Trash items on {}", target.display_label()),
                summary: Some(format!(
                    "Moving {} items ({}) to Trash",
                    remote_plan.targets.len(),
                    human_bytes(remote_plan.reclaim())
                )),
                items: confirm_items,
            };
            let decision = self
                .confirm
                .confirm(&req)
                .map_err(|err| format!("Cleanup confirmation failed: {err}"))?;
            let selected_paths = match decision {
                CleanupConfirmDecision::Confirmed { selected_paths } => {
                    selected_paths
                }
                CleanupConfirmDecision::Cancelled => {
                    return Err("cleanup cancelled by user".to_owned());
                }
            };
            let approved_targets: Vec<Target> = remote_plan
                .targets
                .into_iter()
                .filter(|t| selected_paths.contains(&t.path))
                .collect();
            if approved_targets.is_empty() {
                return Err("cleanup cancelled by user".to_owned());
            }

            // Re-verify the authorization lease has not expired while the
            // user was reviewing items.
            if !self.has_valid_lease(&lease_key) {
                self.remote_connect_leases.remove(&lease_key);
                return Err(
                    "remote SSH authorization lease expired before deletion; \
                     re-authorize connection before trashing"
                        .to_owned(),
                );
            }

            match &target {
                RemoteTarget::Profile {
                    profile, endpoint, ..
                } => {
                    self.ensure_profile_session(&lease_key, profile, endpoint)?;
                    let session = self
                        .remote_connect_leases
                        .get(&lease_key)
                        .and_then(|l| l.session.as_ref())
                        .ok_or_else(|| "SSH session missing".to_owned())?;
                    self.remote_executor
                        .trash_with_session(session, &approved_targets, &header)
                        .map_err(|err| err.to_string())?;
                }
                RemoteTarget::RawHost { raw_host, .. } => {
                    self.remote_executor
                        .trash_raw(raw_host, &approved_targets, &header)
                        .map_err(|err| err.to_string())?;
                }
            }
            self.invalidate_overlapping(Some(&cache_host), &root_path);
            let reclaim: u64 = approved_targets.iter().map(|t| t.bytes).sum();
            let skipped_by_user: Vec<String> = targets
                .iter()
                .filter(|t| !selected_paths.contains(&t.path))
                .map(|t| t.path.display().to_string())
                .collect();
            return Ok(json!({
                "mode": "trash",
                "target": target.display_label(),
                "profile": target.profile_alias(),
                "ssh_host": target.raw_ssh_host(),
                "root": remote_plan.root.display().to_string(),
                "removed_count": approved_targets.len(),
                "reclaim_bytes": reclaim,
                "human_reclaim": human_bytes(reclaim),
                "skipped_count": skipped_by_user.len(),
                "skipped_by_user": skipped_by_user,
            }));
        }

        let root_path = resolve_local_path(root_str)?;
        let cached_tree =
            self.cache.get(&(None, root_path.clone())).map(|c| &c.tree);
        let targets =
            build_targets(&raw_targets, &root_path, cached_tree, false);
        let local_plan =
            plan_for_mode(&targets, &root_path, RemovalMode::Trash);
        if local_plan.targets.is_empty() || !local_plan.blocked.is_empty() {
            return Err(format_plan_rejection(
                "trash_paths",
                &local_plan.root,
                &local_plan.blocked,
            ));
        }
        let confirm_items: Vec<CleanupConfirmItem> = local_plan
            .targets
            .iter()
            .map(|t| {
                let git_warning = git::state(&t.path).and_then(|st| {
                    if st.is_clean() {
                        None
                    } else {
                        Some(st.summary())
                    }
                });
                CleanupConfirmItem {
                    path: t.path.clone(),
                    bytes: t.bytes,
                    mode: RemovalMode::Trash,
                    reason: None,
                    git_warning,
                }
            })
            .collect();
        let req = CleanupConfirmRequest {
            root: local_plan.root.clone(),
            title: "Move items to Trash".to_owned(),
            summary: Some(format!(
                "Moving {} items ({}) to Trash",
                local_plan.targets.len(),
                human_bytes(local_plan.reclaim())
            )),
            items: confirm_items,
        };
        let decision = self
            .confirm
            .confirm(&req)
            .map_err(|err| format!("Cleanup confirmation failed: {err}"))?;
        let selected_paths = match decision {
            CleanupConfirmDecision::Confirmed { selected_paths } => {
                selected_paths
            }
            CleanupConfirmDecision::Cancelled => {
                return Err("cleanup cancelled by user".to_owned());
            }
        };
        let approved_targets: Vec<Target> = local_plan
            .targets
            .into_iter()
            .filter(|t| selected_paths.contains(&t.path))
            .collect();
        if approved_targets.is_empty() {
            return Err("cleanup cancelled by user".to_owned());
        }

        let skipped_by_user: Vec<String> = targets
            .iter()
            .filter(|t| !selected_paths.contains(&t.path))
            .map(|t| t.path.display().to_string())
            .collect();

        let backend = detect_trash_backend();
        if matches!(backend, TrashBackend::Unavailable) {
            return Err(
                "trash backend is unavailable on this system".to_owned()
            );
        }
        let approved_plan =
            plan_for_mode(&approved_targets, &root_path, RemovalMode::Trash);
        let space_before = space_info(&root_path).ok();
        let summary = run_local_removal(approved_plan, RemovalMode::Trash);
        self.invalidate_overlapping(None, &root_path);
        let space_after = space_info(&root_path).ok();
        if !summary.errors.is_empty() || summary.failed_count > 0 {
            return Err(format_removal_failure(
                "trash",
                &root_path,
                &summary,
                space_before,
                space_after,
            ));
        }
        Ok(json!({
            "mode": "trash",
            "root": root_path.display().to_string(),
            "trash_backend": backend.label(),
            "removed_count": summary.removed_count,
            "removed_bytes": summary.removed_bytes,
            "human_removed": human_bytes(summary.removed_bytes),
            "skipped_count": skipped_by_user.len(),
            "skipped_by_user": skipped_by_user,
            "errors": summary.errors,
            "space_before": space_before.map(format_space),
            "space_after": space_after.map(format_space),
        }))
    }

    fn tool_delete_permanently(
        &mut self,
        args: &Value,
    ) -> Result<Value, String> {
        if has_non_null_arg(args, "ssh_host")
            || has_non_null_arg(args, "profile")
        {
            return Err(
                "Permanent deletion over SSH is forbidden; use trash_paths \
                 instead."
                    .to_owned(),
            );
        }
        let root_str = required_str(args, "root")?;
        let raw_targets = required_string_array(args, "targets")?;
        let caller_reason = args.get("reason").and_then(Value::as_str);
        let root_path = resolve_local_path(root_str)?;
        let cached_tree =
            self.cache.get(&(None, root_path.clone())).map(|c| &c.tree);
        let targets =
            build_targets(&raw_targets, &root_path, cached_tree, false);
        let local_plan =
            plan_for_mode(&targets, &root_path, RemovalMode::Permanent);
        if local_plan.targets.is_empty() || !local_plan.blocked.is_empty() {
            return Err(format_plan_rejection(
                "delete_permanently",
                &local_plan.root,
                &local_plan.blocked,
            ));
        }
        let confirm_items: Vec<CleanupConfirmItem> = local_plan
            .targets
            .iter()
            .map(|t| {
                let git_warning = git::state(&t.path).and_then(|st| {
                    if st.is_clean() {
                        None
                    } else {
                        Some(st.summary())
                    }
                });
                CleanupConfirmItem {
                    path: t.path.clone(),
                    bytes: t.bytes,
                    mode: RemovalMode::Permanent,
                    reason: caller_reason.map(str::to_owned),
                    git_warning,
                }
            })
            .collect();
        let req = CleanupConfirmRequest {
            root: local_plan.root.clone(),
            title: "Permanently delete items".to_owned(),
            summary: Some(format!(
                "Permanently deleting {} items ({})",
                local_plan.targets.len(),
                human_bytes(local_plan.reclaim())
            )),
            items: confirm_items,
        };
        let decision = self
            .confirm
            .confirm(&req)
            .map_err(|err| format!("Cleanup confirmation failed: {err}"))?;
        let selected_paths = match decision {
            CleanupConfirmDecision::Confirmed { selected_paths } => {
                selected_paths
            }
            CleanupConfirmDecision::Cancelled => {
                return Err("permanent deletion cancelled by user".to_owned());
            }
        };
        let approved_targets: Vec<Target> = local_plan
            .targets
            .into_iter()
            .filter(|t| selected_paths.contains(&t.path))
            .collect();
        if approved_targets.is_empty() {
            return Err("permanent deletion cancelled by user".to_owned());
        }

        let skipped_by_user: Vec<String> = targets
            .iter()
            .filter(|t| !selected_paths.contains(&t.path))
            .map(|t| t.path.display().to_string())
            .collect();

        let approved_plan = plan_for_mode(
            &approved_targets,
            &root_path,
            RemovalMode::Permanent,
        );
        let request = AuthRequest::for_permanent_delete_with_reason(
            &approved_plan,
            caller_reason,
        );
        match self.auth.authenticate(&request) {
            Ok(true) => {}
            Ok(false) => {
                return Err("permanent deletion cancelled by user".to_owned());
            }
            Err(err) => {
                return Err(format!("OS authentication failed closed: {err}"));
            }
        }
        let space_before = space_info(&root_path).ok();
        let summary = run_local_removal(approved_plan, RemovalMode::Permanent);
        self.invalidate_overlapping(None, &root_path);
        let space_after = space_info(&root_path).ok();
        if !summary.errors.is_empty() || summary.failed_count > 0 {
            return Err(format_removal_failure(
                "permanent",
                &root_path,
                &summary,
                space_before,
                space_after,
            ));
        }
        Ok(json!({
            "mode": "permanent",
            "root": root_path.display().to_string(),
            "removed_count": summary.removed_count,
            "removed_bytes": summary.removed_bytes,
            "human_removed": human_bytes(summary.removed_bytes),
            "skipped_count": skipped_by_user.len(),
            "skipped_by_user": skipped_by_user,
            "errors": summary.errors,
            "space_before": space_before.map(format_space),
            "space_after": space_after.map(format_space),
        }))
    }

    fn ensure_cached_or_default_scan(
        &mut self,
        remote: Option<&RemoteTarget>,
        root_str: &str,
        tool_name: &str,
    ) -> Result<(PathBuf, bool), String> {
        let candidate_root = if remote.is_some() {
            normalize_remote_path(Path::new(root_str))
        } else {
            resolve_local_path(root_str)?
        };
        let cache_host = remote.map(|t| t.cache_key().to_owned());
        let key = (cache_host, candidate_root.clone());
        if self.cache.contains_key(&key) {
            return Ok((candidate_root, true));
        }
        let default_options = ScanOptions::default();
        self.ensure_scanned(
            remote,
            Some(root_str),
            false,
            &default_options,
            false,
            tool_name,
        )
    }

    fn ensure_scanned(
        &mut self,
        remote: Option<&RemoteTarget>,
        path_arg: Option<&str>,
        disk: bool,
        options: &ScanOptions,
        rescan: bool,
        tool_name: &str,
    ) -> Result<(PathBuf, bool), String> {
        if let Some(target) = remote {
            let effective_path = path_arg
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| if disk { None } else { target.default_path() });
            let cache_host = target.cache_key().to_owned();
            if !rescan && let Some(raw) = effective_path {
                let norm = normalize_remote_path(Path::new(raw));
                let key = (Some(cache_host.clone()), norm.clone());
                if let Some(existing) = self.cache.get(&key)
                    && same_scan_options(&existing.options, options)
                {
                    return Ok((norm, true));
                }
            }

            let lease_key = self.ensure_remote_authorized(target, tool_name)?;
            let request = Request {
                root: effective_path.map(PathBuf::from),
                disk,
                options: options.clone(),
            };
            let snapshot = match target {
                RemoteTarget::Profile {
                    profile, endpoint, ..
                } => {
                    self.ensure_profile_session(&lease_key, profile, endpoint)?;
                    let session = self
                        .remote_connect_leases
                        .get(&lease_key)
                        .and_then(|l| l.session.as_ref())
                        .ok_or_else(|| "SSH session missing".to_owned())?;
                    self.remote_executor
                        .scan_with_session(session, &request)
                        .map_err(|err| err.to_string())?
                }
                RemoteTarget::RawHost { raw_host, .. } => self
                    .remote_executor
                    .scan_raw(raw_host, &request)
                    .map_err(|err| err.to_string())?,
            };

            if let RemoteTarget::Profile { profile, .. } = target
                && let Some(space) = snapshot.header.space
                && let Some(db) = &self.storage
            {
                let now_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let _ = db.update_ssh_profile_scan_summary(
                    &profile.id,
                    space.total,
                    space.available,
                    now_secs,
                );
            }

            let root_path = normalize_remote_path(&snapshot.header.root);
            let key = (Some(cache_host), root_path.clone());
            let prev_count = self.cache.get(&key).map_or(0, |c| c.scan_count);
            self.cache.insert(
                key,
                CachedScan {
                    tree: snapshot.tree,
                    space: snapshot.header.space,
                    home: snapshot.header.home,
                    platform: snapshot.header.platform,
                    identity: snapshot.header.identity,
                    options: options.clone(),
                    scan_count: prev_count + 1,
                },
            );
            return Ok((root_path, false));
        }

        let root_path = resolve_local_scan_root(path_arg, disk)?;
        let key = (None, root_path.clone());
        if !rescan
            && let Some(existing) = self.cache.get(&key)
            && same_scan_options(&existing.options, options)
        {
            return Ok((root_path, true));
        }
        let tree =
            scan(&root_path, options.clone()).map_err(|err| err.to_string())?;
        let prev_count = self.cache.get(&key).map_or(0, |c| c.scan_count);
        let entry = CachedScan {
            tree,
            space: space_info(&root_path).ok(),
            home: std::env::home_dir(),
            platform: std::env::consts::OS.to_owned(),
            identity: crate::identity::HostIdentity::current(),
            options: options.clone(),
            scan_count: prev_count + 1,
        };
        self.cache.insert(key, entry);
        Ok((root_path, false))
    }
}

/// Serve MCP requests over an arbitrary reader/writer pair.
pub fn serve(
    mut reader: impl BufRead,
    mut writer: impl Write,
    auth: Box<dyn AuthAdapter>,
) -> io::Result<()> {
    let mut server = McpServer::new(auth);
    server.serve(&mut reader, &mut writer)
}

/// Serve MCP requests over process `stdin` and `stdout` using the host OS
/// authentication adapter.
pub fn serve_stdio() -> io::Result<()> {
    serve_stdio_with_options(false)
}

/// Serve MCP requests in read-only mode, omitting and rejecting destructive tools.
pub fn serve_stdio_read_only() -> io::Result<()> {
    serve_stdio_with_options(true)
}

/// Serve MCP requests over `stdin`/`stdout` with configurable read-only flag.
pub fn serve_stdio_with_options(read_only: bool) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut server = McpServer::default()
        .with_default_services()
        .with_read_only(read_only);
    server.serve(&mut reader, &mut writer)
}

fn write_result(
    writer: &mut impl Write,
    id: &Value,
    result: &Value,
) -> io::Result<()> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    serde_json::to_writer(&mut *writer, &payload)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn write_error(
    writer: &mut impl Write,
    id: &Value,
    code: i64,
    message: &str,
) -> io::Result<()> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    });
    serde_json::to_writer(&mut *writer, &payload)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    writer.write_all(b"\n")?;
    writer.flush()
}

const fn same_scan_options(left: &ScanOptions, right: &ScanOptions) -> bool {
    left.apparent_size == right.apparent_size
        && left.follow_links == right.follow_links
        && left.include_hidden == right.include_hidden
        && left.one_filesystem == right.one_filesystem
        && matches!(
            (left.max_depth, right.max_depth),
            (None, None) | (Some(_), Some(_))
        )
}

fn has_non_null_arg(args: &Value, field: &str) -> bool {
    match args.get(field) {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

fn optional_trimmed_str<'a>(
    args: &'a Value,
    field: &str,
) -> Result<Option<&'a str>, String> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            Ok((!trimmed.is_empty()).then_some(trimmed))
        }
        Some(_) => Err(format!("Invalid parameter '{field}': expected string")),
    }
}

fn parse_raw_ssh_host(raw: &str) -> Result<(String, u16, String), String> {
    validate_host(raw).map_err(|err| err.to_string())?;
    let (username, host_part) = match raw.split_once('@') {
        Some((user, rest)) => (user.to_owned(), rest),
        None => (String::new(), raw),
    };
    let (host, port) = if let Some(rest) = host_part.strip_prefix('[') {
        if let Some((bracketed, after)) = rest.split_once(']') {
            if let Some(port_str) = after.strip_prefix(':') {
                let port = port_str.parse::<u16>().map_err(|_| {
                    format!("invalid SSH port in destination '{raw}'")
                })?;
                (bracketed.to_owned(), port)
            } else if after.is_empty() {
                (bracketed.to_owned(), DEFAULT_SSH_PORT)
            } else {
                return Err(format!("invalid SSH destination '{raw}'"));
            }
        } else {
            return Err(format!("invalid SSH destination '{raw}'"));
        }
    } else if host_part.matches(':').count() == 1
        && let Some((h, port_str)) = host_part.split_once(':')
    {
        let port = port_str
            .parse::<u16>()
            .map_err(|_| format!("invalid SSH port in destination '{raw}'"))?;
        (h.to_owned(), port)
    } else {
        (host_part.to_owned(), DEFAULT_SSH_PORT)
    };
    if host.is_empty() || port == 0 {
        return Err(format!("invalid SSH destination '{raw}'"));
    }
    Ok((host, port, username))
}

fn required_str<'a>(args: &'a Value, field: &str) -> Result<&'a str, String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing or empty required parameter: {field}"))
}

fn required_string_array(
    args: &Value,
    field: &str,
) -> Result<Vec<String>, String> {
    let Some(arr) = args.get(field).and_then(Value::as_array) else {
        return Err(format!("Missing or invalid array parameter: {field}"));
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(text) = item.as_str() else {
            return Err(format!("Every entry in '{field}' must be a string"));
        };
        out.push(text.to_owned());
    }
    Ok(out)
}

fn parse_limit(
    raw: Option<&Value>,
    default_val: usize,
    max_val: usize,
) -> usize {
    raw.and_then(Value::as_u64)
        .map_or(default_val, |v| (v as usize).clamp(1, max_val))
}

fn resolve_local_path(raw: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(path)
    };
    Ok(normalize(&abs))
}

fn resolve_local_scan_root(
    path_arg: Option<&str>,
    disk: bool,
) -> Result<PathBuf, String> {
    let base = match path_arg.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => resolve_local_path(raw)?,
        None => std::env::home_dir()
            .ok_or_else(|| "Could not determine home directory".to_owned())?,
    };
    let root = if disk {
        volume_root_for(&base).unwrap_or(base)
    } else {
        base
    };
    let norm = normalize(&root);
    let meta = fs::metadata(&norm)
        .map_err(|err| format!("{}: {err}", norm.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", norm.display()));
    }
    Ok(norm)
}

fn resolve_subpath<'a>(
    root_path: &Path,
    root_node: &'a Node,
    subpath: Option<&str>,
) -> Result<(Vec<usize>, PathBuf, &'a Node), String> {
    let Some(raw) = subpath.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok((Vec::new(), root_path.to_path_buf(), root_node));
    };
    let sub = Path::new(raw);
    let rel = if sub.is_absolute() {
        let norm_sub = normalize(sub);
        norm_sub
            .strip_prefix(root_path)
            .map_err(|_| {
                format!(
                    "subpath {} is outside scanned root {}",
                    norm_sub.display(),
                    root_path.display()
                )
            })?
            .to_path_buf()
    } else {
        sub.to_path_buf()
    };
    let mut crumbs = Vec::new();
    let mut current_node = root_node;
    let mut current_path = root_path.to_path_buf();
    for component in rel.components() {
        match component {
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir => {
                return Err("subpath must not contain parent ('..') traversal"
                    .to_owned());
            }
            Component::Prefix(_) => {
                return Err("invalid path prefix in subpath".to_owned());
            }
            Component::Normal(os_name) => {
                let name = os_name.to_string_lossy();
                let Some((idx, child)) = current_node
                    .children
                    .iter()
                    .enumerate()
                    .find(|(_, c)| &*c.name == name.as_ref())
                else {
                    return Err(format!(
                        "entry '{}' not found under {}",
                        name,
                        current_path.display()
                    ));
                };
                crumbs.push(idx);
                current_path.push(&*child.name);
                current_node = child;
            }
        }
    }
    Ok((crumbs, current_path, current_node))
}

fn path_for_crumbs(
    root_path: &Path,
    root_node: &Node,
    crumbs: &[usize],
) -> PathBuf {
    let mut path = root_path.to_path_buf();
    let mut node = root_node;
    for &idx in crumbs {
        let Some(child) = node.children.get(idx) else {
            break;
        };
        path.push(&*child.name);
        node = child;
    }
    path
}

fn normalize_remote_path(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return normalize(path);
    };
    if !text.starts_with('/') {
        return normalize(path);
    }
    let mut parts: Vec<&str> = Vec::new();
    for segment in text.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        PathBuf::from("/")
    } else {
        PathBuf::from(format!("/{}", parts.join("/")))
    }
}

fn build_targets(
    raw_targets: &[String],
    root_path: &Path,
    cached_tree: Option<&Node>,
    remote: bool,
) -> Vec<Target> {
    raw_targets
        .iter()
        .map(|raw| {
            let path = PathBuf::from(raw);
            let norm = if remote {
                normalize_remote_path(&path)
            } else {
                normalize(&path)
            };
            if let Some(tree) = cached_tree
                && let Ok(rel) = norm.strip_prefix(root_path)
                && let Some(node) = find_relative_node(tree, rel)
            {
                return Target {
                    path,
                    bytes: node.bytes,
                    is_dir: node.is_dir(),
                    hidden: node.name.starts_with('.'),
                };
            }
            if !remote && let Ok(meta) = fs::symlink_metadata(&path) {
                let hidden = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                return Target {
                    path,
                    bytes: meta.len(),
                    is_dir: meta.is_dir(),
                    hidden,
                };
            }
            Target {
                path,
                bytes: 0,
                is_dir: false,
                hidden: false,
            }
        })
        .collect()
}

fn find_relative_node<'a>(root: &'a Node, rel: &Path) -> Option<&'a Node> {
    let mut current = root;
    for component in rel.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                let name_str = name.to_str()?;
                current = current.child_named(name_str)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn format_scan_summary(
    profile: Option<&str>,
    ssh_host: Option<&str>,
    root_path: &Path,
    entry: &CachedScan,
    cached: bool,
    max_entries: usize,
) -> Value {
    let errors = count_read_errors(&entry.tree);
    let categories = summarize_categories(&entry.tree);
    let children = format_top_children(root_path, &entry.tree, max_entries);
    json!({
        "profile": profile,
        "ssh_host": ssh_host,
        "root": root_path.display().to_string(),
        "platform": entry.platform,
        "cached": cached,
        "scan_count": entry.scan_count,
        "space": entry.space.map(format_space),
        "totals": {
            "bytes": entry.tree.bytes,
            "files": entry.tree.files,
            "dirs": entry.tree.dirs,
            "errors": errors,
            "human_size": human_bytes(entry.tree.bytes),
        },
        "categories": categories,
        "entries": children,
    })
}

fn count_read_errors(node: &Node) -> u64 {
    let own = u64::from(node.read_error);
    node.children
        .iter()
        .fold(own, |acc, child| acc + count_read_errors(child))
}

fn summarize_categories(root: &Node) -> Vec<Value> {
    let mut map: FxHashMap<Category, (u64, u64)> = FxHashMap::default();
    accumulate_categories(root, &mut map);
    let mut list: Vec<(Category, u64, u64)> = map
        .into_iter()
        .filter(|(_, (bytes, files))| *bytes > 0 || *files > 0)
        .map(|(cat, (bytes, files))| (cat, bytes, files))
        .collect();
    list.sort_by_key(|right| std::cmp::Reverse(right.1));
    list.into_iter()
        .map(|(cat, bytes, files)| {
            json!({
                "category": cat.label(),
                "bytes": bytes,
                "files": files,
                "human_size": human_bytes(bytes),
            })
        })
        .collect()
}

fn accumulate_categories(
    node: &Node,
    totals: &mut FxHashMap<Category, (u64, u64)>,
) {
    if !node.is_dir() {
        let entry = totals.entry(node.category).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(node.bytes);
        entry.1 = entry.1.saturating_add(node.files);
        return;
    }
    for child in &node.children {
        accumulate_categories(child, totals);
    }
}

fn format_top_children(
    base_path: &Path,
    node: &Node,
    max_entries: usize,
) -> Vec<Value> {
    node.children
        .iter()
        .take(max_entries)
        .map(|child| {
            let path = base_path.join(&*child.name);
            format_node_entry(&path, child)
        })
        .collect()
}

fn format_node_entry(path: &Path, node: &Node) -> Value {
    json!({
        "name": &*node.name,
        "path": path.display().to_string(),
        "kind": kind_label(node.kind),
        "bytes": node.bytes,
        "files": node.files,
        "dirs": node.dirs,
        "category": node.category.label(),
        "reclaim": node.reclaim.map(Reclaim::label),
        "human_size": human_bytes(node.bytes),
    })
}

const fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Directory => "directory",
        NodeKind::File => "file",
        NodeKind::Symlink => "symlink",
        NodeKind::Other => "other",
    }
}

fn format_finding(finding: &Finding) -> Value {
    match finding {
        Finding::Reclaimable(reclaim) => json!({
            "kind": "reclaimable",
            "reason": reclaim.label(),
        }),
        Finding::Worktrees { count, oldest_days } => json!({
            "kind": "worktrees",
            "count": count,
            "oldest_days": oldest_days,
        }),
        Finding::StaleExperiments { count } => json!({
            "kind": "stale_experiments",
            "count": count,
        }),
    }
}

fn format_volume(volume: &crate::space::Volume) -> Value {
    json!({
        "point": volume.point.display().to_string(),
        "device": volume.device,
        "space": volume.space.map(format_space),
    })
}

fn format_space(space: SpaceInfo) -> Value {
    json!({
        "total": space.total,
        "free": space.free,
        "available": space.available,
        "used": space.used(),
        "human_total": human_bytes(space.total),
        "human_free": human_bytes(space.free),
        "human_available": human_bytes(space.available),
    })
}

fn format_targets(targets: &[Target]) -> Vec<Value> {
    targets
        .iter()
        .map(|target| {
            json!({
                "path": target.path.display().to_string(),
                "bytes": target.bytes,
                "human_size": human_bytes(target.bytes),
                "is_dir": target.is_dir,
            })
        })
        .collect()
}

fn format_blocked(blocked: &[Blocked]) -> Vec<Value> {
    blocked
        .iter()
        .map(|item| {
            json!({
                "path": item.path.display().to_string(),
                "reason": item.reason,
            })
        })
        .collect()
}

struct RemovalSummary {
    removed_count: u64,
    removed_bytes: u64,
    failed_count: usize,
    errors: Vec<Value>,
}

fn run_local_removal(plan: Plan, mode: RemovalMode) -> RemovalSummary {
    let handle = spawn(plan, mode);
    let mut errors = Vec::new();
    loop {
        let mut progressed = false;
        while let Some(event) = handle.poll() {
            progressed = true;
            match event {
                RemovalEvent::Start { .. } => {}
                RemovalEvent::Item {
                    path,
                    bytes: _,
                    outcome,
                } => {
                    if let Err(reason) = outcome {
                        let spawn_failed = path.as_os_str().is_empty();
                        errors.push(json!({
                            "path": path.display().to_string(),
                            "error": reason,
                        }));
                        if spawn_failed {
                            return RemovalSummary {
                                removed_count: 0,
                                removed_bytes: 0,
                                failed_count: 1,
                                errors,
                            };
                        }
                    }
                }
                RemovalEvent::Done {
                    removed,
                    bytes,
                    failed,
                } => {
                    return RemovalSummary {
                        removed_count: removed,
                        removed_bytes: bytes,
                        failed_count: failed,
                        errors,
                    };
                }
            }
        }
        if !progressed {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

fn format_plan_rejection(
    action: &str,
    root: &Path,
    blocked: &[Blocked],
) -> String {
    let message = if blocked.is_empty() {
        format!(
            "{action} aborted: no valid targets under {}",
            root.display()
        )
    } else {
        format!(
            "{action} aborted: {} target(s) blocked by safety guards",
            blocked.len()
        )
    };
    serde_json::to_string(&json!({
        "error": message,
        "root": root.display().to_string(),
        "blocked": format_blocked(blocked),
    }))
    .unwrap_or(message)
}

fn format_removal_failure(
    mode: &str,
    root: &Path,
    summary: &RemovalSummary,
    space_before: Option<SpaceInfo>,
    space_after: Option<SpaceInfo>,
) -> String {
    serde_json::to_string(&json!({
        "mode": mode,
        "root": root.display().to_string(),
        "removed_count": summary.removed_count,
        "removed_bytes": summary.removed_bytes,
        "failed_count": summary.failed_count,
        "errors": summary.errors,
        "space_before": space_before.map(format_space),
        "space_after": space_after.map(format_space),
    }))
    .unwrap_or_else(|_| format!("{mode} removal encountered errors"))
}

fn is_known_tool(name: &str) -> bool {
    matches!(
        name,
        "list_ssh_profiles"
            | "list_volumes"
            | "scan_directory"
            | "inspect_subtree"
            | "find_reclaimable"
            | "check_git_safety"
            | "plan_removal"
            | "trash_paths"
            | "delete_permanently"
    )
}

fn tool_definitions(read_only: bool) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "list_ssh_profiles",
            "description": "List saved remote SSH server profiles (non-sensitive metadata only; never accesses credentials or opens connections).",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "list_volumes",
            "description": "List mounted volumes and free space on the local machine or a remote SSH host.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "profile": { "type": "string", "description": "Optional saved SSH profile alias or ID." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination (e.g. user@host)." }
                }
            }
        }),
        json!({
            "name": "scan_directory",
            "description": "Scan a directory tree (or its containing volume), returning totals, category breakdown, and largest entries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path to scan (defaults to home directory)." },
                    "disk": { "type": "boolean", "description": "Scan the whole containing volume instead of just path." },
                    "profile": { "type": "string", "description": "Optional saved SSH profile alias or ID." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." },
                    "apparent_size": { "type": "boolean", "description": "Measure apparent file length instead of allocated blocks." },
                    "include_hidden": { "type": "boolean", "description": "Include dotfiles and hidden directories (default true)." },
                    "max_entries": { "type": "integer", "description": "Maximum direct children to return (default 25, max 100)." },
                    "rescan": { "type": "boolean", "description": "Force a fresh scan instead of reusing the session cache." }
                }
            }
        }),
        json!({
            "name": "inspect_subtree",
            "description": "Inspect a subdirectory or search by substring filter within a scanned root.",
            "inputSchema": {
                "type": "object",
                "required": ["root"],
                "properties": {
                    "root": { "type": "string", "description": "Scanned root directory path." },
                    "subpath": { "type": "string", "description": "Optional relative or absolute path inside root." },
                    "filter": { "type": "string", "description": "Optional case-insensitive substring filter." },
                    "profile": { "type": "string", "description": "Optional saved SSH profile alias or ID." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." },
                    "max_entries": { "type": "integer", "description": "Maximum entries to return (default 25, max 100)." }
                }
            }
        }),
        json!({
            "name": "find_reclaimable",
            "description": "Find the largest reclaimable caches, build outputs, worktrees, and stale experiments under a scanned root.",
            "inputSchema": {
                "type": "object",
                "required": ["root"],
                "properties": {
                    "root": { "type": "string", "description": "Scanned root directory path." },
                    "profile": { "type": "string", "description": "Optional saved SSH profile alias or ID." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." },
                    "limit": { "type": "integer", "description": "Maximum candidates to return (default 20, max 100)." }
                }
            }
        }),
        json!({
            "name": "check_git_safety",
            "description": "Check whether local paths are Git checkouts with uncommitted changes, stashes, or unpushed commits.",
            "inputSchema": {
                "type": "object",
                "required": ["paths"],
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Local filesystem paths to inspect."
                    },
                    "profile": { "type": "string", "description": "Unsupported for git checks; rejected if set." },
                    "ssh_host": { "type": "string", "description": "Unsupported for git checks; rejected if set." }
                }
            }
        }),
        json!({
            "name": "plan_removal",
            "description": "Validate candidate paths against Ruum's removal safety guards without deleting anything.",
            "inputSchema": {
                "type": "object",
                "required": ["root", "targets"],
                "properties": {
                    "root": { "type": "string", "description": "Scanned root directory path." },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target paths under root to evaluate."
                    },
                    "profile": { "type": "string", "description": "Optional saved SSH profile alias or ID." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." }
                }
            }
        }),
    ];
    if !read_only {
        tools.push(json!({
            "name": "trash_paths",
            "description": "Move validated paths under a scanned root to the system Trash (local or guarded remote SSH).",
            "inputSchema": {
                "type": "object",
                "required": ["root", "targets"],
                "properties": {
                    "root": { "type": "string", "description": "Scanned root directory path." },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target paths under root to move to Trash."
                    },
                    "profile": { "type": "string", "description": "Optional saved SSH profile alias or ID." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." }
                }
            }
        }));
        tools.push(json!({
            "name": "delete_permanently",
            "description": "Permanently delete local paths under a scanned root after out-of-band OS native authentication.",
            "inputSchema": {
                "type": "object",
                "required": ["root", "targets"],
                "properties": {
                    "root": { "type": "string", "description": "Local scanned root directory path." },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target paths under root to permanently delete."
                    },
                    "reason": { "type": "string", "description": "Reason for permanent deletion shown in the OS authentication prompt (defaults to '清理回收站')." },
                    "profile": { "type": "string", "description": "Forbidden for permanent deletion." },
                    "ssh_host": { "type": "string", "description": "Forbidden for permanent deletion." }
                }
            }
        }));
    }
    tools
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::process::Command;

    use super::*;
    use crate::auth::AuthOs;
    use crate::classify::classify;
    use crate::tree::{Metric, aggregate};

    struct DenyAuth;

    impl AuthAdapter for DenyAuth {
        fn os_kind(&self) -> AuthOs {
            AuthOs::Linux
        }

        fn authenticate(&self, _request: &AuthRequest) -> io::Result<bool> {
            Ok(false)
        }
    }

    #[derive(Debug)]
    struct AutoConfirmAll;

    impl CleanupConfirmAdapter for AutoConfirmAll {
        fn confirm(
            &self,
            request: &CleanupConfirmRequest,
        ) -> io::Result<CleanupConfirmDecision> {
            Ok(CleanupConfirmDecision::Confirmed {
                selected_paths: request
                    .items
                    .iter()
                    .map(|i| i.path.clone())
                    .collect(),
            })
        }
    }

    #[derive(Debug)]
    struct MockDecisionConfirm {
        decision: CleanupConfirmDecision,
    }

    impl CleanupConfirmAdapter for MockDecisionConfirm {
        fn confirm(
            &self,
            _request: &CleanupConfirmRequest,
        ) -> io::Result<CleanupConfirmDecision> {
            Ok(self.decision.clone())
        }
    }

    fn run_session(
        server: &mut McpServer,
        requests: &[Value],
    ) -> io::Result<Vec<Value>> {
        let mut input = String::new();
        for req in requests {
            input.push_str(&serde_json::to_string(req).expect("serialize req"));
            input.push('\n');
        }
        let mut reader = Cursor::new(input.into_bytes());
        let mut output = Vec::new();
        server.serve(&mut reader, &mut output)?;
        let text = String::from_utf8(output).expect("utf8 output");
        Ok(text
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid json line"))
            .collect())
    }

    fn parse_tool_payload(response: &Value) -> (bool, Value) {
        let result = &response["result"];
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let text = result["content"][0]["text"]
            .as_str()
            .expect("tool content text");
        let parsed = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
        (is_error, parsed)
    }

    #[test]
    fn lifecycle_initialize_notifications_ping_tools_list_and_eof() {
        let mut server = McpServer::new(Box::new(DenyAuth));
        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": { "protocolVersion": "2024-11-05" }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "ping"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/list"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": { "name": "nonexistent_tool", "arguments": {} }
                }),
            ],
        )
        .expect("serve succeeds");

        // Notification produces no frame, so 4 responses for 5 messages.
        assert_eq!(responses.len(), 4);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "ruum");

        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"], json!({}));

        assert_eq!(responses[2]["id"], 3);
        let tools = responses[2]["result"]["tools"]
            .as_array()
            .expect("tools array");
        assert_eq!(tools.len(), 9);

        assert_eq!(responses[3]["id"], 4);
        assert_eq!(responses[3]["error"]["code"], -32601);
    }

    #[test]
    fn scan_cache_reuse_rescan_and_subtree_inspection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join(".cache/pip")).expect("mkdir .cache");
        fs::create_dir_all(root.join("src/app")).expect("mkdir src/app");
        fs::write(root.join(".cache/pip/wheel.bin"), vec![b'a'; 8192])
            .expect("write wheel");
        fs::write(root.join("src/app/main.rs"), vec![b'b'; 2048])
            .expect("write main.rs");

        let root_str = root.to_str().expect("utf8 root");
        let mut server = McpServer::new(Box::new(DenyAuth));

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": { "path": root_str, "apparent_size": true }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": { "path": root_str, "apparent_size": true }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": {
                            "path": root_str,
                            "apparent_size": true,
                            "rescan": true
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {
                        "name": "inspect_subtree",
                        "arguments": { "root": root_str, "subpath": "src/app" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 5,
                    "method": "tools/call",
                    "params": {
                        "name": "inspect_subtree",
                        "arguments": { "root": root_str, "filter": "wheel" }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, scan1) = parse_tool_payload(&responses[0]);
        assert!(!err1);
        assert_eq!(scan1["cached"], false);
        assert_eq!(scan1["scan_count"], 1);
        assert_eq!(scan1["totals"]["bytes"], 8192 + 2048);

        let (err2, scan2) = parse_tool_payload(&responses[1]);
        assert!(!err2);
        assert_eq!(scan2["cached"], true);
        assert_eq!(scan2["scan_count"], 1);

        let (err3, scan3) = parse_tool_payload(&responses[2]);
        assert!(!err3);
        assert_eq!(scan3["cached"], false);
        assert_eq!(scan3["scan_count"], 2);

        let (err4, sub) = parse_tool_payload(&responses[3]);
        assert!(!err4);
        assert_eq!(sub["entries"][0]["name"], "main.rs");
        assert_eq!(sub["entries"][0]["bytes"], 2048);

        let (err5, filtered) = parse_tool_payload(&responses[4]);
        assert!(!err5);
        assert_eq!(filtered["match_count"], 1);
        assert_eq!(filtered["entries"][0]["name"], "wheel.bin");
    }

    #[test]
    fn find_reclaimable_and_overlap_invalidation() {
        let mut root = Node::directory("root");
        let mut cache_dir = Node::directory(".cache");
        cache_dir.children.push(Node::entry(
            "blob.bin",
            NodeKind::File,
            128 * 1024 * 1024,
        ));
        root.children.push(cache_dir);
        aggregate(&mut root, Metric::Bytes);
        classify(&mut root);

        let mut server = McpServer::new(Box::new(DenyAuth));
        let root_path = normalize(&std::env::temp_dir().join("synth-work"));
        let root_str = root_path.to_str().expect("utf8").to_owned();
        let cache_str = root_path.join(".cache").display().to_string();
        server.insert_cached(
            None,
            root_path.clone(),
            CachedScan {
                tree: root,
                space: None,
                home: None,
                platform: "linux".to_owned(),
                identity: None,
                options: ScanOptions::default(),
                scan_count: 1,
            },
        );

        let responses = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "find_reclaimable",
                    "arguments": { "root": root_str }
                }
            })],
        )
        .expect("serve succeeds");

        let (is_err, payload) = parse_tool_payload(&responses[0]);
        assert!(!is_err);
        let candidates = payload["candidates"].as_array().expect("candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["path"], cache_str);
        assert_eq!(candidates[0]["finding"]["kind"], "reclaimable");

        server.invalidate_overlapping(None, &root_path.join(".cache"));
        assert!(server.cache().is_empty());
    }

    #[test]
    fn check_git_safety_detects_dirty_checkout_and_rejects_ssh_host() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = Command::new("git")
            .arg("init")
            .arg(dir.path())
            .output()
            .expect("git init");
        assert!(status.status.success());
        fs::write(dir.path().join("dirty.txt"), b"uncommitted").expect("write");

        let mut server = McpServer::new(Box::new(DenyAuth));
        let path_str = dir.path().to_str().expect("utf8");
        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "check_git_safety",
                        "arguments": { "paths": [path_str] }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "check_git_safety",
                        "arguments": {
                            "paths": [path_str],
                            "ssh_host": "user@remote"
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, res1) = parse_tool_payload(&responses[0]);
        assert!(!err1);
        assert_eq!(res1["results"][0]["is_checkout"], true);
        assert_eq!(res1["results"][0]["is_clean"], false);
        assert_eq!(res1["results"][0]["changed"], 1);

        let (err2, _res2) = parse_tool_payload(&responses[1]);
        assert!(err2, "ssh_host must be rejected for check_git_safety");
    }

    #[test]
    fn plan_removal_local_and_cached_remote_enforce_guards() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let cache_dir = root.join("cache");
        let nested = cache_dir.join("nested");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(nested.join("item.bin"), vec![0_u8; 4096]).expect("write");

        let mut server = McpServer::new(Box::new(DenyAuth));
        let root_str = root.to_str().expect("utf8");

        // Seed a remote scan entry for remote plan_removal testing.
        let mut remote_tree = Node::directory("me");
        remote_tree.children.push(Node::entry(
            "cache",
            NodeKind::Directory,
            2048,
        ));
        aggregate(&mut remote_tree, Metric::Bytes);
        server.insert_cached(
            Some("box".to_owned()),
            PathBuf::from("/home/me"),
            CachedScan {
                tree: remote_tree,
                space: None,
                home: Some(PathBuf::from("/home/me")),
                platform: "linux".to_owned(),
                identity: Some(HostIdentity {
                    hostname: "box".to_owned(),
                    machine_id: Some(
                        "0123456789abcdef0123456789abcdef".to_owned(),
                    ),
                }),
                options: ScanOptions::default(),
                scan_count: 1,
            },
        );

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "plan_removal",
                        "arguments": {
                            "root": root_str,
                            "targets": [
                                cache_dir.to_str().expect("utf8"),
                                nested.to_str().expect("utf8"),
                                root_str,
                                "/usr"
                            ]
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "plan_removal",
                        "arguments": {
                            "root": "/home/me",
                            "ssh_host": "box",
                            "targets": ["/home/me/cache", "/home/me", "/etc"]
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, local_plan) = parse_tool_payload(&responses[0]);
        assert!(!err1);
        assert_eq!(local_plan["targets"].as_array().expect("targets").len(), 1);
        assert_eq!(local_plan["covered"].as_array().expect("covered").len(), 1);
        assert_eq!(local_plan["blocked"].as_array().expect("blocked").len(), 2);

        let (err2, rem_plan) = parse_tool_payload(&responses[1]);
        assert!(!err2);
        assert_eq!(rem_plan["root"], "/home/me");
        assert_eq!(rem_plan["targets"].as_array().expect("targets").len(), 1);
        assert_eq!(rem_plan["targets"][0]["path"], "/home/me/cache");
        assert_eq!(rem_plan["blocked"].as_array().expect("blocked").len(), 2);
        assert_eq!(rem_plan["allowed_modes"], json!(["trash"]));
    }

    #[test]
    fn normalize_remote_path_keeps_posix_separators_and_invalidates() {
        assert_eq!(
            normalize_remote_path(Path::new("/home/me/./sub/../cache/"))
                .to_str(),
            Some("/home/me/cache")
        );
        assert_eq!(normalize_remote_path(Path::new("/..")).to_str(), Some("/"));
        assert_eq!(normalize_remote_path(Path::new("/")).to_str(), Some("/"));

        let mut remote_tree = Node::directory("me");
        let mut cache = Node::directory("cache");
        cache
            .children
            .push(Node::entry("item.bin", NodeKind::File, 2048));
        remote_tree.children.push(cache);
        aggregate(&mut remote_tree, Metric::Bytes);

        let mut server = McpServer::new(Box::new(DenyAuth));
        let cached = CachedScan {
            tree: remote_tree,
            space: None,
            home: Some(PathBuf::from("/home/me")),
            platform: "linux".to_owned(),
            identity: Some(HostIdentity {
                hostname: "box".to_owned(),
                machine_id: Some("0123456789abcdef0123456789abcdef".to_owned()),
            }),
            options: ScanOptions::default(),
            scan_count: 1,
        };
        server.insert_cached(
            Some("box".to_owned()),
            PathBuf::from("/home/me"),
            cached.clone(),
        );
        server.insert_cached(
            Some("box".to_owned()),
            PathBuf::from("/var/log"),
            cached,
        );

        let responses = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "plan_removal",
                    "arguments": {
                        "root": "/home/me/./",
                        "ssh_host": "box",
                        "targets": ["/home/me/./cache"]
                    }
                }
            })],
        )
        .expect("serve succeeds");
        let (is_err, payload) = parse_tool_payload(&responses[0]);
        assert!(!is_err, "unexpected error: {payload}");
        assert_eq!(payload["root"], "/home/me");
        assert_eq!(payload["targets"][0]["path"], "/home/me/cache");
        assert_eq!(payload["targets"][0]["bytes"], 2048);

        server
            .invalidate_overlapping(Some("box"), Path::new("/home/me/./cache"));
        assert!(!server.cache().contains_key(&(
            Some("box".to_owned()),
            PathBuf::from("/home/me")
        )));
        assert!(server.cache().contains_key(&(
            Some("box".to_owned()),
            PathBuf::from("/var/log")
        )));
    }

    struct CountingAuth {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        last_reason: std::sync::Arc<std::sync::Mutex<Option<String>>>,
        decision: Result<bool, &'static str>,
    }

    impl AuthAdapter for CountingAuth {
        fn os_kind(&self) -> AuthOs {
            AuthOs::Linux
        }

        fn authenticate(&self, request: &AuthRequest) -> io::Result<bool> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last_reason.lock().expect("lock reason") =
                Some(request.reason.clone());
            self.decision.map_err(io::Error::other)
        }
    }

    #[test]
    fn delete_permanently_rejects_ssh_and_blocked_without_calling_auth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let valid_target = root.join("build");
        fs::create_dir_all(&valid_target).expect("mkdir build");
        fs::write(valid_target.join("artifact.o"), b"bytes").expect("write");

        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut server = McpServer::new(Box::new(CountingAuth {
            calls: std::sync::Arc::clone(&calls),
            last_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
            decision: Ok(true),
        }));
        let root_str = root.to_str().expect("utf8");
        let valid_str = valid_target.to_str().expect("utf8");

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "delete_permanently",
                        "arguments": {
                            "root": root_str,
                            "targets": [valid_str],
                            "ssh_host": "user@remote"
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "delete_permanently",
                        "arguments": {
                            "root": root_str,
                            "targets": [valid_str, root_str]
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, msg1) = parse_tool_payload(&responses[0]);
        assert!(err1, "remote permanent delete must fail");
        assert!(
            msg1.as_str()
                .unwrap_or_default()
                .contains("Permanent deletion over SSH is forbidden")
        );

        let (err2, msg2) = parse_tool_payload(&responses[1]);
        assert!(err2, "batch with blocked target must abort");
        assert_eq!(msg2["blocked"].as_array().expect("blocked").len(), 1);
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "auth must not be invoked when ssh_host is set or targets are blocked"
        );
        assert!(
            valid_target.exists(),
            "sibling target in aborted batch must stay untouched"
        );
    }

    #[test]
    fn delete_permanently_fails_closed_on_deny_or_error_and_deletes_on_approve()
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let doomed = root.join("cache-dir");
        let keep = root.join("keep-dir");
        fs::create_dir_all(&doomed).expect("mkdir doomed");
        fs::create_dir_all(&keep).expect("mkdir keep");
        fs::write(doomed.join("temp.bin"), vec![b'x'; 1024]).expect("write");
        fs::write(keep.join(" precious.txt"), b"keep me").expect("write");

        let root_str = root.to_str().expect("utf8");
        let doomed_str = doomed.to_str().expect("utf8");

        // 1. User cancels OS auth prompt (Ok(false)).
        let deny_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let deny_reason = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut deny_server = McpServer::with_confirm(
            Box::new(CountingAuth {
                calls: std::sync::Arc::clone(&deny_calls),
                last_reason: std::sync::Arc::clone(&deny_reason),
                decision: Ok(false),
            }),
            Box::new(AutoConfirmAll),
        );
        let resp_deny = run_session(
            &mut deny_server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "delete_permanently",
                    "arguments": { "root": root_str, "targets": [doomed_str] }
                }
            })],
        )
        .expect("serve succeeds");
        let (is_err_deny, _) = parse_tool_payload(&resp_deny[0]);
        assert!(is_err_deny);
        assert_eq!(deny_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            deny_reason
                .lock()
                .expect("lock")
                .as_deref()
                .unwrap_or_default()
                .starts_with("清理回收站 — Permanently delete"),
            "default reason must start with 清理回收站"
        );
        assert!(doomed.exists(), "denied auth must leave target untouched");

        // 2. OS auth prompt errors (Err(...)).
        let err_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut err_server = McpServer::with_confirm(
            Box::new(CountingAuth {
                calls: std::sync::Arc::clone(&err_calls),
                last_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                decision: Err("no display server available"),
            }),
            Box::new(AutoConfirmAll),
        );
        let resp_err = run_session(
            &mut err_server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "delete_permanently",
                    "arguments": { "root": root_str, "targets": [doomed_str] }
                }
            })],
        )
        .expect("serve succeeds");
        let (is_err_auth, _) = parse_tool_payload(&resp_err[0]);
        assert!(is_err_auth);
        assert_eq!(err_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(doomed.exists(), "errored auth must fail closed");

        // 3. OS auth prompt approves (Ok(true)) with explicit caller reason.
        let ok_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ok_reason = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut ok_server = McpServer::with_confirm(
            Box::new(CountingAuth {
                calls: std::sync::Arc::clone(&ok_calls),
                last_reason: std::sync::Arc::clone(&ok_reason),
                decision: Ok(true),
            }),
            Box::new(AutoConfirmAll),
        );
        let resp_ok = run_session(
            &mut ok_server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": { "path": root_str }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "delete_permanently",
                        "arguments": {
                            "root": root_str,
                            "targets": [doomed_str],
                            "reason": "清理回收站"
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");
        let (is_err_scan, _) = parse_tool_payload(&resp_ok[0]);
        assert!(!is_err_scan);
        let (is_err_del, del_payload) = parse_tool_payload(&resp_ok[1]);
        assert!(
            !is_err_del,
            "approved deletion should succeed: {del_payload}"
        );
        assert_eq!(ok_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            ok_reason
                .lock()
                .expect("lock")
                .as_deref()
                .unwrap_or_default()
                .starts_with("清理回收站 — Permanently delete")
        );
        assert_eq!(del_payload["mode"], "permanent");
        assert_eq!(del_payload["removed_count"], 1);
        assert!(!doomed.exists(), "approved target must be removed");
        assert!(keep.exists(), "unmarked sibling must remain intact");
        assert!(
            ok_server.cache().is_empty(),
            "overlapping session cache entry must be invalidated after delete"
        );
    }

    #[test]
    fn trash_paths_aborts_when_blocked_or_remote_host_uncached() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let valid_item = root.join("logs");
        fs::create_dir_all(&valid_item).expect("mkdir logs");
        fs::write(valid_item.join("app.log"), b"log").expect("write log");

        let mut server = McpServer::new(Box::new(DenyAuth));
        let root_str = root.to_str().expect("utf8");
        let valid_str = valid_item.to_str().expect("utf8");

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "trash_paths",
                        "arguments": {
                            "root": root_str,
                            "targets": [valid_str, root_str]
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "trash_paths",
                        "arguments": {
                            "root": "/home/me",
                            "targets": ["/home/me/cache"],
                            "ssh_host": "uncached-box"
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, payload1) = parse_tool_payload(&responses[0]);
        assert!(err1, "trash_paths must abort when any target is blocked");
        assert_eq!(payload1["blocked"].as_array().expect("blocked").len(), 1);
        assert!(
            valid_item.exists(),
            "valid target must not be trashed when batch has a blocked target"
        );

        let (err2, payload2) = parse_tool_payload(&responses[1]);
        assert!(err2, "trash_paths on uncached remote host must fail");
        assert!(
            payload2
                .as_str()
                .unwrap_or_default()
                .contains("No cached remote scan")
        );
    }

    #[test]
    fn read_only_mode_hides_and_rejects_destructive_tools() {
        let mut server = McpServer::default().with_read_only(true);
        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/list"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "trash_paths",
                        "arguments": { "root": "/", "targets": ["/tmp"] }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "delete_permanently",
                        "arguments": { "root": "/", "targets": ["/tmp"] }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        // 1. tools/list must omit trash_paths and delete_permanently
        let tools = responses[0]["result"]["tools"]
            .as_array()
            .expect("tools array");
        assert_eq!(tools.len(), 7);
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            assert_ne!(name, "trash_paths");
            assert_ne!(name, "delete_permanently");
        }

        // 2. tools/call for trash_paths must report unavailable in read-only mode
        let (err_trash, payload_trash) = parse_tool_payload(&responses[1]);
        assert!(err_trash);
        assert!(
            payload_trash
                .as_str()
                .unwrap_or_default()
                .contains("read-only mode")
        );

        // 3. tools/call for delete_permanently must report unavailable in read-only mode
        let (err_del, payload_del) = parse_tool_payload(&responses[2]);
        assert!(err_del);
        assert!(
            payload_del
                .as_str()
                .unwrap_or_default()
                .contains("read-only mode")
        );
    }

    #[test]
    fn trash_paths_partial_selection_with_skipped_by_user() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let file1 = root.join("file1.log");
        let file2 = root.join("file2.log");
        fs::write(&file1, b"log1").expect("write file1");
        fs::write(&file2, b"log2").expect("write file2");

        let confirm = MockDecisionConfirm {
            decision: CleanupConfirmDecision::Confirmed {
                selected_paths: vec![file1.clone()],
            },
        };
        let mut server =
            McpServer::with_confirm(Box::new(DenyAuth), Box::new(confirm));
        let root_str = root.to_str().expect("utf8");
        let f1_str = file1.to_str().expect("utf8");
        let f2_str = file2.to_str().expect("utf8");

        let responses = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "trash_paths",
                    "arguments": {
                        "root": root_str,
                        "targets": [f1_str, f2_str]
                    }
                }
            })],
        )
        .expect("serve succeeds");

        let (err, payload) = parse_tool_payload(&responses[0]);
        assert!(!err);
        assert_eq!(payload["removed_count"], 1);
        assert_eq!(payload["skipped_count"], 1);
        let skipped = payload["skipped_by_user"].as_array().expect("skipped");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0], f2_str);

        // file1 trashed/removed, file2 untouched
        assert!(!file1.exists(), "selected target must be removed");
        assert!(file2.exists(), "unselected target must be untouched");
    }

    #[test]
    fn trash_paths_cancelled_by_user_leaves_all_intact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let file1 = root.join("file1.log");
        fs::write(&file1, b"log1").expect("write file1");

        let confirm = MockDecisionConfirm {
            decision: CleanupConfirmDecision::Cancelled,
        };
        let mut server =
            McpServer::with_confirm(Box::new(DenyAuth), Box::new(confirm));
        let root_str = root.to_str().expect("utf8");
        let f1_str = file1.to_str().expect("utf8");

        let responses = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "trash_paths",
                    "arguments": {
                        "root": root_str,
                        "targets": [f1_str]
                    }
                }
            })],
        )
        .expect("serve succeeds");

        let (err, payload) = parse_tool_payload(&responses[0]);
        assert!(err);
        assert!(
            payload
                .as_str()
                .unwrap_or_default()
                .contains("cleanup cancelled by user")
        );
        assert!(
            file1.exists(),
            "cancelled deletion must leave target intact"
        );
    }

    #[test]
    fn delete_permanently_whitelist_prevents_unauthorized_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let target_a = root.join("target_a.bin");
        let target_b = root.join("target_b.bin");
        fs::write(&target_a, b"a").expect("write a");
        fs::write(&target_b, b"b").expect("write b");

        // The mock confirm tries to sneak in target_b which was NOT in the caller request
        let confirm = MockDecisionConfirm {
            decision: CleanupConfirmDecision::Confirmed {
                selected_paths: vec![target_a.clone(), target_b.clone()],
            },
        };
        let mut server = McpServer::with_confirm(
            Box::new(CountingAuth {
                calls: std::sync::Arc::new(
                    std::sync::atomic::AtomicUsize::new(0),
                ),
                last_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                decision: Ok(true),
            }),
            Box::new(confirm),
        );

        let root_str = root.to_str().expect("utf8");
        let a_str = target_a.to_str().expect("utf8");

        let responses = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "delete_permanently",
                    "arguments": {
                        "root": root_str,
                        "targets": [a_str]
                    }
                }
            })],
        )
        .expect("serve succeeds");

        let (err, payload) = parse_tool_payload(&responses[0]);
        assert!(!err);
        assert_eq!(payload["removed_count"], 1);

        // target_a was authorized and deleted; target_b was NOT authorized and MUST stay
        assert!(!target_a.exists());
        assert!(
            target_b.exists(),
            "unrequested path injected by confirm must NOT be deleted"
        );
    }

    #[test]
    fn delete_permanently_cancelled_before_os_auth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let target = root.join("target.bin");
        fs::write(&target, b"target").expect("write");

        let auth_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let confirm = MockDecisionConfirm {
            decision: CleanupConfirmDecision::Cancelled,
        };
        let mut server = McpServer::with_confirm(
            Box::new(CountingAuth {
                calls: std::sync::Arc::clone(&auth_calls),
                last_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                decision: Ok(true),
            }),
            Box::new(confirm),
        );

        let root_str = root.to_str().expect("utf8");
        let target_str = target.to_str().expect("utf8");

        let responses = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "delete_permanently",
                    "arguments": {
                        "root": root_str,
                        "targets": [target_str]
                    }
                }
            })],
        )
        .expect("serve succeeds");

        let (err, payload) = parse_tool_payload(&responses[0]);
        assert!(err);
        assert!(
            payload
                .as_str()
                .unwrap_or_default()
                .contains("permanent deletion cancelled by user")
        );
        assert_eq!(
            auth_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "OS auth must not be invoked when user cancels at the confirm popup"
        );
        assert!(target.exists());
    }

    #[test]
    fn plan_removal_excludes_trash_mode_and_trash_paths_rejects_trash_targets()
    {
        use crate::trash_adapter::{ALREADY_IN_TRASH_REASON, TrashEnv};

        #[derive(Debug)]
        struct CountingConfirm(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl CleanupConfirmAdapter for CountingConfirm {
            fn confirm(
                &self,
                _request: &CleanupConfirmRequest,
            ) -> io::Result<CleanupConfirmDecision> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(CleanupConfirmDecision::Cancelled)
            }
        }

        let env = TrashEnv::current();
        let (root_path, trash_path) = if cfg!(target_os = "macos") {
            let Some(home) = env.home else {
                return;
            };
            (home.clone(), home.join(".Trash"))
        } else if cfg!(windows) {
            (
                PathBuf::from(r"C:\"),
                PathBuf::from(r"C:\$Recycle.Bin\S-1-5-21-0\$RTEST"),
            )
        } else {
            let Some(home) = env.home else {
                return;
            };
            let xdg = env
                .xdg_data_home
                .unwrap_or_else(|| home.join(".local/share"));
            let root = xdg.parent().map_or(home, Path::to_path_buf);
            (root, xdg.join("Trash"))
        };

        let confirm_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let mut server = McpServer::with_confirm(
            Box::new(CountingAuth {
                calls: std::sync::Arc::new(
                    std::sync::atomic::AtomicUsize::new(0),
                ),
                last_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
                decision: Ok(false),
            }),
            Box::new(CountingConfirm(std::sync::Arc::clone(&confirm_calls))),
        );

        let root_str = root_path.to_str().expect("utf8");
        let trash_str = trash_path.to_str().expect("utf8");
        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "plan_removal",
                        "arguments": {
                            "root": root_str,
                            "targets": [trash_str]
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "trash_paths",
                        "arguments": {
                            "root": root_str,
                            "targets": [trash_str]
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, payload1) = parse_tool_payload(&responses[0]);
        assert!(!err1, "plan_removal must succeed: {payload1}");
        assert_eq!(payload1["allowed_modes"], json!(["permanent"]));

        let (err2, payload2) = parse_tool_payload(&responses[1]);
        assert!(err2, "trash_paths must reject trash targets upfront");
        assert_eq!(
            payload2["blocked"][0]["reason"],
            json!(ALREADY_IN_TRASH_REASON),
            "{payload2}"
        );
        assert_eq!(
            confirm_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "confirm UI must not open for rejected trash targets"
        );
    }

    #[derive(Debug)]
    struct CountingRootStore {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        secret: [u8; 32],
    }

    impl crate::credentials::RootSecretStore for CountingRootStore {
        fn get_or_create_root_secret(
            &self,
        ) -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(zeroize::Zeroizing::new(self.secret))
        }
    }

    #[derive(Debug)]
    struct CountingRemoteConfirm {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        requests:
            std::sync::Arc<std::sync::Mutex<Vec<RemoteConnectConfirmRequest>>>,
        outcomes: std::sync::Mutex<
            std::collections::VecDeque<
                io::Result<RemoteConnectConfirmDecision>,
            >,
        >,
        fallback: RemoteConnectConfirmDecision,
    }

    impl CountingRemoteConfirm {
        fn always(decision: RemoteConnectConfirmDecision) -> Self {
            Self {
                calls: std::sync::Arc::new(
                    std::sync::atomic::AtomicUsize::new(0),
                ),
                requests: std::sync::Arc::new(
                    std::sync::Mutex::new(Vec::new()),
                ),
                outcomes: std::sync::Mutex::new(
                    std::collections::VecDeque::new(),
                ),
                fallback: decision,
            }
        }

        fn sequence(
            outcomes: Vec<io::Result<RemoteConnectConfirmDecision>>,
            fallback: RemoteConnectConfirmDecision,
        ) -> Self {
            Self {
                calls: std::sync::Arc::new(
                    std::sync::atomic::AtomicUsize::new(0),
                ),
                requests: std::sync::Arc::new(
                    std::sync::Mutex::new(Vec::new()),
                ),
                outcomes: std::sync::Mutex::new(outcomes.into()),
                fallback,
            }
        }
    }

    impl RemoteConnectConfirmAdapter for CountingRemoteConfirm {
        fn confirm(
            &self,
            request: &RemoteConnectConfirmRequest,
        ) -> io::Result<RemoteConnectConfirmDecision> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.requests.lock().unwrap().push(request.clone());
            let next = self.outcomes.lock().unwrap().pop_front();
            if let Some(next) = next {
                next
            } else {
                Ok(self.fallback)
            }
        }
    }

    #[derive(Debug, Default)]
    struct MockRemoteExecutor {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        ops: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        trashed_paths: std::sync::Arc<std::sync::Mutex<Vec<PathBuf>>>,
        session_ptrs: std::sync::Arc<std::sync::Mutex<Vec<usize>>>,
    }

    impl MockRemoteExecutor {
        fn sample_roots() -> Roots {
            let volumes = vec![crate::space::Volume {
                point: PathBuf::from("/"),
                device: Some("/dev/vda1".to_owned()),
                space: Some(SpaceInfo {
                    total: 100_000_000,
                    free: 20_000_000,
                    available: 15_000_000,
                }),
            }];
            serde_json::from_value(json!({
                "version": 1,
                "home": "/home/deploy",
                "platform": "linux",
                "identity": {
                    "hostname": "prod-web-01",
                    "machine_id": "0123456789abcdef0123456789abcdef",
                },
                "volumes": volumes,
            }))
            .expect("valid sample Roots")
        }

        fn sample_snapshot(root: &Path) -> Snapshot {
            let mut tree = Node::directory(
                root.file_name().and_then(|n| n.to_str()).unwrap_or("root"),
            );
            let mut old = Node::directory("old");
            old.children
                .push(Node::entry("app.log", NodeKind::File, 4096));
            tree.children.push(old);
            aggregate(&mut tree, Metric::Bytes);
            classify(&mut tree);

            let mut header = Header::new(
                root.to_path_buf(),
                Some(PathBuf::from("/home/deploy")),
                "linux",
                Some(HostIdentity {
                    hostname: "prod-web-01".to_owned(),
                    machine_id: Some(
                        "0123456789abcdef0123456789abcdef".to_owned(),
                    ),
                }),
            );
            header.space = Some(SpaceInfo {
                total: 100_000_000,
                free: 20_000_000,
                available: 15_000_000,
            });
            Snapshot { header, tree }
        }
    }

    impl RemoteExecutor for MockRemoteExecutor {
        fn list_volumes_with_session(
            &self,
            session: &SshSession,
        ) -> io::Result<Roots> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.ops
                .lock()
                .unwrap()
                .push("list_volumes_session".to_owned());
            self.session_ptrs
                .lock()
                .unwrap()
                .push(std::ptr::from_ref(session) as usize);
            Ok(Self::sample_roots())
        }

        fn list_volumes_raw(&self, host: &str) -> io::Result<Roots> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.ops
                .lock()
                .unwrap()
                .push(format!("list_volumes_raw:{host}"));
            Ok(Self::sample_roots())
        }

        fn scan_with_session(
            &self,
            session: &SshSession,
            request: &Request,
        ) -> io::Result<Snapshot> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.ops.lock().unwrap().push("scan_session".to_owned());
            self.session_ptrs
                .lock()
                .unwrap()
                .push(std::ptr::from_ref(session) as usize);
            let root = request
                .root
                .clone()
                .unwrap_or_else(|| PathBuf::from("/home/deploy"));
            Ok(Self::sample_snapshot(&root))
        }

        fn scan_raw(
            &self,
            host: &str,
            request: &Request,
        ) -> io::Result<Snapshot> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.ops.lock().unwrap().push(format!("scan_raw:{host}"));
            let root = request
                .root
                .clone()
                .unwrap_or_else(|| PathBuf::from("/home/deploy"));
            Ok(Self::sample_snapshot(&root))
        }

        fn trash_with_session(
            &self,
            session: &SshSession,
            targets: &[Target],
            _header: &Header,
        ) -> io::Result<()> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.ops.lock().unwrap().push("trash_session".to_owned());
            self.session_ptrs
                .lock()
                .unwrap()
                .push(std::ptr::from_ref(session) as usize);
            self.trashed_paths
                .lock()
                .unwrap()
                .extend(targets.iter().map(|t| t.path.clone()));
            Ok(())
        }

        fn trash_raw(
            &self,
            host: &str,
            targets: &[Target],
            _header: &Header,
        ) -> io::Result<()> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.ops.lock().unwrap().push(format!("trash_raw:{host}"));
            self.trashed_paths
                .lock()
                .unwrap()
                .extend(targets.iter().map(|t| t.path.clone()));
            Ok(())
        }
    }

    fn seed_profile_and_keychain(
        temp_dir: &Path,
    ) -> (StorageDb, SshProfile, PathBuf, [u8; 32]) {
        use crate::credentials::{
            CREDENTIALS_FILE_NAME, LazyCredentialStore, SshCredentialSecret,
        };
        use crate::storage::AuthMode;
        use rand_core::OsRng;

        let db = StorageDb::open(temp_dir.join("ruum.db")).unwrap();
        let mut prod = SshProfile::new(
            "prod-web",
            "10.0.1.25",
            2222,
            "deploy",
            AuthMode::PrivateKey,
        );
        prod.default_path = Some("/var/log".to_owned());
        let saved = db.add_ssh_profile(&prod).unwrap();

        let cred_path = temp_dir.join(CREDENTIALS_FILE_NAME);
        let root_secret = [0x42_u8; 32];
        let seed_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seed_store = LazyCredentialStore::new(
            cred_path.clone(),
            std::sync::Arc::new(CountingRootStore {
                calls: seed_calls,
                secret: root_secret,
            }),
        );
        let key = ssh_key::PrivateKey::random(
            &mut OsRng,
            ssh_key::Algorithm::Ed25519,
        )
        .unwrap();
        let pem = key.to_openssh(ssh_key::LineEnding::LF).unwrap();
        seed_store
            .set_ssh_secret(
                &saved.id,
                &SshCredentialSecret::PrivateKey {
                    private_key_pem: pem.to_string(),
                    passphrase: None,
                },
            )
            .unwrap();

        (db, saved, cred_path, root_secret)
    }

    #[test]
    fn list_ssh_profiles_returns_metadata_with_zero_secrets_keychain_or_confirm()
     {
        use crate::credentials::LazyCredentialStore;
        use crate::storage::AuthMode;

        let temp = tempfile::tempdir().unwrap();
        let (db, prod, cred_path, root_secret) =
            seed_profile_and_keychain(temp.path());
        let staging = SshProfile::new(
            "staging-pw",
            "staging.example.com",
            22,
            "ubuntu",
            AuthMode::Password,
        );
        db.add_ssh_profile(&staging).unwrap();

        let kc_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let creds = LazyCredentialStore::new(
            cred_path,
            std::sync::Arc::new(CountingRootStore {
                calls: std::sync::Arc::clone(&kc_calls),
                secret: root_secret,
            }),
        );
        let remote_confirm =
            CountingRemoteConfirm::always(RemoteConnectConfirmDecision::Denied);
        let confirm_calls = std::sync::Arc::clone(&remote_confirm.calls);
        let exec = MockRemoteExecutor::default();
        let exec_calls = std::sync::Arc::clone(&exec.calls);

        let mut server = McpServer::new(Box::new(DenyAuth))
            .with_storage(db)
            .with_credentials(creds)
            .with_remote_confirm(Box::new(remote_confirm))
            .with_remote_executor(Box::new(exec));

        let responses = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "list_ssh_profiles", "arguments": {} }
            })],
        )
        .unwrap();

        let (is_err, payload) = parse_tool_payload(&responses[0]);
        assert!(!is_err, "list_ssh_profiles failed: {payload}");
        let profiles = payload["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0]["id"], prod.id);
        assert_eq!(profiles[0]["alias"], "prod-web");
        assert_eq!(profiles[0]["host"], "10.0.1.25");
        assert_eq!(profiles[0]["port"], 2222);
        assert_eq!(profiles[0]["username"], "deploy");
        assert_eq!(profiles[0]["auth_mode"], "private_key");
        assert_eq!(profiles[0]["default_path"], "/var/log");
        assert!(profiles[0].get("password").is_none());
        assert!(profiles[0].get("private_key_pem").is_none());
        assert!(profiles[0].get("secret").is_none());

        assert_eq!(
            kc_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "list_ssh_profiles must never access keychain"
        );
        assert_eq!(
            confirm_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "list_ssh_profiles must never trigger remote connect confirmation"
        );
        assert_eq!(
            exec_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "list_ssh_profiles must never open SSH connections"
        );
    }

    #[test]
    fn profile_and_ssh_host_conflict_and_disallowed_tools_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let (db, _prod, cred_path, root_secret) =
            seed_profile_and_keychain(temp.path());
        let creds = crate::credentials::LazyCredentialStore::new(
            cred_path,
            std::sync::Arc::new(CountingRootStore {
                calls: std::sync::Arc::new(
                    std::sync::atomic::AtomicUsize::new(0),
                ),
                secret: root_secret,
            }),
        );
        let remote_confirm = CountingRemoteConfirm::always(
            RemoteConnectConfirmDecision::Approved,
        );
        let confirm_calls = std::sync::Arc::clone(&remote_confirm.calls);

        let mut server = McpServer::new(Box::new(DenyAuth))
            .with_storage(db)
            .with_credentials(creds)
            .with_remote_confirm(Box::new(remote_confirm));

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "list_volumes",
                        "arguments": {
                            "profile": "prod-web",
                            "ssh_host": "deploy@10.0.1.25"
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": {
                            "profile": "prod-web",
                            "ssh_host": "deploy@10.0.1.25"
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "check_git_safety",
                        "arguments": {
                            "paths": ["/tmp"],
                            "profile": "prod-web"
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {
                        "name": "delete_permanently",
                        "arguments": {
                            "root": "/var/log",
                            "targets": ["/var/log/old"],
                            "profile": "prod-web"
                        }
                    }
                }),
            ],
        )
        .unwrap();

        for (idx, resp) in responses.iter().enumerate() {
            let (is_err, payload) = parse_tool_payload(resp);
            assert!(is_err, "call #{idx} must fail: {payload}");
        }
        assert!(
            parse_tool_payload(&responses[0])
                .1
                .as_str()
                .unwrap_or_default()
                .contains("not both")
        );
        assert!(
            parse_tool_payload(&responses[3])
                .1
                .as_str()
                .unwrap_or_default()
                .contains("Permanent deletion over SSH is forbidden")
        );
        assert_eq!(
            confirm_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "invalid profile/ssh_host combinations must fail before confirm"
        );
    }

    #[test]
    fn remote_tools_fail_closed_on_deny_or_headless_with_zero_keychain_and_ssh()
    {
        let temp = tempfile::tempdir().unwrap();
        let (db, prod, cred_path, root_secret) =
            seed_profile_and_keychain(temp.path());
        let kc_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let creds = crate::credentials::LazyCredentialStore::new(
            cred_path,
            std::sync::Arc::new(CountingRootStore {
                calls: std::sync::Arc::clone(&kc_calls),
                secret: root_secret,
            }),
        );

        let remote_confirm = CountingRemoteConfirm::sequence(
            vec![
                Ok(RemoteConnectConfirmDecision::Denied),
                Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "no active graphical desktop",
                )),
                Ok(RemoteConnectConfirmDecision::Denied),
                Ok(RemoteConnectConfirmDecision::Denied),
                Ok(RemoteConnectConfirmDecision::Denied),
                Ok(RemoteConnectConfirmDecision::Denied),
            ],
            RemoteConnectConfirmDecision::Denied,
        );
        let confirm_calls = std::sync::Arc::clone(&remote_confirm.calls);
        let exec = MockRemoteExecutor::default();
        let exec_calls = std::sync::Arc::clone(&exec.calls);

        let mut server = McpServer::with_confirm(
            Box::new(DenyAuth),
            Box::new(AutoConfirmAll),
        )
        .with_storage(db)
        .with_credentials(creds)
        .with_remote_confirm(Box::new(remote_confirm))
        .with_remote_executor(Box::new(exec));

        // Pre-seed a cached remote scan for `prod.id` WITHOUT granting a lease
        // to prove `trash_paths` still enforces RemoteConnectConfirmAdapter!
        let snap = MockRemoteExecutor::sample_snapshot(Path::new("/var/log"));
        server.insert_cached(
            Some(format!("profile:{}", prod.id)),
            PathBuf::from("/var/log"),
            CachedScan {
                tree: snap.tree,
                space: snap.header.space,
                home: snap.header.home,
                platform: snap.header.platform,
                identity: snap.header.identity,
                options: ScanOptions::default(),
                scan_count: 1,
            },
        );

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "list_volumes",
                        "arguments": { "profile": "prod-web" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": { "profile": "prod-web" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "inspect_subtree",
                        "arguments": {
                            "profile": "prod-web",
                            "root": "/tmp/uncached"
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {
                        "name": "find_reclaimable",
                        "arguments": {
                            "profile": "prod-web",
                            "root": "/tmp/uncached-2"
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 5,
                    "method": "tools/call",
                    "params": {
                        "name": "list_volumes",
                        "arguments": { "ssh_host": "ubuntu@10.0.0.99" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 6,
                    "method": "tools/call",
                    "params": {
                        "name": "trash_paths",
                        "arguments": {
                            "profile": "prod-web",
                            "root": "/var/log",
                            "targets": ["/var/log/old"]
                        }
                    }
                }),
            ],
        )
        .unwrap();

        for (idx, resp) in responses.iter().enumerate() {
            let (is_err, payload) = parse_tool_payload(resp);
            assert!(is_err, "denied/headless call #{idx} must fail: {payload}");
        }
        assert_eq!(confirm_calls.load(std::sync::atomic::Ordering::SeqCst), 6);
        assert_eq!(
            kc_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "keychain must never be accessed when connection confirm is denied"
        );
        assert_eq!(
            exec_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "SSH transport must never execute when confirm is denied"
        );
        assert_eq!(server.remote_lease_count(), 0);
    }

    #[test]
    fn approved_profile_reuses_lease_and_session_and_reprompts_after_expiry() {
        let temp = tempfile::tempdir().unwrap();
        let (db, prod, cred_path, root_secret) =
            seed_profile_and_keychain(temp.path());
        let kc_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let creds = crate::credentials::LazyCredentialStore::new(
            cred_path,
            std::sync::Arc::new(CountingRootStore {
                calls: std::sync::Arc::clone(&kc_calls),
                secret: root_secret,
            }),
        );

        let remote_confirm = CountingRemoteConfirm::sequence(
            vec![
                Ok(RemoteConnectConfirmDecision::Approved),
                Ok(RemoteConnectConfirmDecision::Denied),
            ],
            RemoteConnectConfirmDecision::Denied,
        );
        let confirm_calls = std::sync::Arc::clone(&remote_confirm.calls);
        let confirm_reqs = std::sync::Arc::clone(&remote_confirm.requests);

        let exec = MockRemoteExecutor::default();
        let exec_calls = std::sync::Arc::clone(&exec.calls);
        let session_ptrs = std::sync::Arc::clone(&exec.session_ptrs);
        let trashed_paths = std::sync::Arc::clone(&exec.trashed_paths);

        let mut server = McpServer::with_confirm(
            Box::new(DenyAuth),
            Box::new(AutoConfirmAll),
        )
        .with_storage(db)
        .with_credentials(creds)
        .with_remote_confirm(Box::new(remote_confirm))
        .with_remote_executor(Box::new(exec))
        .with_lease_ttl(Duration::from_mins(10));

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "list_volumes",
                        "arguments": { "profile": "prod-web" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": { "profile": "prod-web" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "plan_removal",
                        "arguments": {
                            "profile": prod.id,
                            "root": "/var/log",
                            "targets": ["/var/log/old"]
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {
                        "name": "trash_paths",
                        "arguments": {
                            "profile": "prod-web",
                            "root": "/var/log",
                            "targets": ["/var/log/old"]
                        }
                    }
                }),
            ],
        )
        .unwrap();

        for (idx, resp) in responses.iter().enumerate() {
            let (is_err, payload) = parse_tool_payload(resp);
            assert!(!is_err, "approved call #{idx} failed: {payload}");
        }
        let scan_payload = parse_tool_payload(&responses[1]).1;
        assert_eq!(scan_payload["profile"], "prod-web");
        assert_eq!(scan_payload["root"], "/var/log");

        // Confirm prompted only once; keychain loaded only once; SshSession reused.
        assert_eq!(confirm_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(kc_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(exec_calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        let reqs = confirm_reqs.lock().unwrap().clone();
        assert_eq!(reqs[0].alias.as_deref(), Some("prod-web"));
        assert_eq!(reqs[0].host, "10.0.1.25");
        assert_eq!(reqs[0].port, 2222);
        assert_eq!(reqs[0].username, "deploy");
        assert_eq!(reqs[0].auth_mode, "private_key");
        assert_eq!(reqs[0].tool_name, "list_volumes");
        let ptrs = session_ptrs.lock().unwrap().clone();
        assert_eq!(ptrs.len(), 3);
        assert_eq!(ptrs[0], ptrs[1]);
        assert_eq!(ptrs[1], ptrs[2]);
        assert_eq!(
            *trashed_paths.lock().unwrap(),
            vec![PathBuf::from("/var/log/old")]
        );

        // Expire lease -> next call must re-prompt and fail closed when denied.
        server.expire_remote_leases_for_test();
        let after_expiry = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "list_volumes",
                    "arguments": { "profile": "prod-web" }
                }
            })],
        )
        .unwrap();
        let (is_err, err_payload) = parse_tool_payload(&after_expiry[0]);
        assert!(is_err, "expired lease denied on re-prompt must fail closed");
        assert!(
            err_payload
                .as_str()
                .unwrap_or_default()
                .contains("denied by user")
        );
        assert_eq!(confirm_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            exec_calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "expired and denied session must not run SSH transport"
        );
    }
}
