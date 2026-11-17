//! Unified `SQLite` storage layer (`ruum.db`) for `ruum-core`.
//!
//! Owns `app_settings`, `ssh_profiles`, `webhook_config`, `ai_config`, and
//! `ai_state` inside `<config_dir>/ruum/ruum.db` using `WAL` journal mode and
//! `busy_timeout = 5000`. Watchdog tables (`watchdog_config` and
//! `watchdog_state`) remain strictly owned by
//! [`ruum_watchdog::WatchdogConfigService`], exposed via [`StorageDb::watchdog`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{
    Connection, ErrorCode, OptionalExtension as _, Row, TransactionBehavior,
    params,
};
use ruum_watchdog::WatchdogConfigService;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::watchdog_config::{AiCliProvider, CleanupProposal};

/// Default scan power preset.
pub const DEFAULT_POWER_PRESET: &str = "balanced";

/// Default treemap rendering depth.
pub const DEFAULT_DEPTH: u32 = 3;

/// Minimum treemap rendering depth.
pub const MIN_DEPTH: u32 = 1;

/// Maximum treemap rendering depth.
pub const MAX_DEPTH: u32 = 6;

/// Default SSH authorization lease duration in minutes.
pub const DEFAULT_SSH_SESSION_DURATION_MINUTES: u64 = 15;

/// Minimum SSH authorization lease duration in minutes.
pub const MIN_SSH_SESSION_DURATION_MINUTES: u64 = 1;

/// Default SSH TCP port.
pub const DEFAULT_SSH_PORT: u16 = 22;

/// Default webhook HTTP request timeout in seconds.
pub const DEFAULT_WEBHOOK_TIMEOUT_SECS: u64 = 10;

/// Minimum webhook HTTP request timeout in seconds.
pub const MIN_WEBHOOK_TIMEOUT_SECS: u64 = 1;

/// Canonical `id` used when migrating legacy single-row `webhook_config`.
pub const DEFAULT_WEBHOOK_ID: &str = "default";

/// Canonical `name` used when migrating legacy single-row `webhook_config`.
pub const DEFAULT_WEBHOOK_NAME: &str = "Default";

static NEXT_PROFILE_SEQ: AtomicU64 = AtomicU64::new(1);

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS app_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    power_preset TEXT NOT NULL DEFAULT 'balanced',
    default_depth INTEGER NOT NULL DEFAULT 3,
    apparent_size INTEGER NOT NULL DEFAULT 0,
    include_hidden INTEGER NOT NULL DEFAULT 1,
    follow_system_appearance INTEGER NOT NULL DEFAULT 1,
    ssh_session_duration_minutes INTEGER NOT NULL DEFAULT 15,
    updated_at_secs INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS ssh_profiles (
    id TEXT PRIMARY KEY,
    alias TEXT NOT NULL UNIQUE,
    host TEXT NOT NULL,
    port INTEGER NOT NULL DEFAULT 22,
    username TEXT NOT NULL,
    auth_mode TEXT NOT NULL CHECK (auth_mode IN ('password', 'private_key')),
    default_path TEXT,
    last_total_bytes INTEGER,
    last_available_bytes INTEGER,
    last_scanned_secs INTEGER,
    created_at_secs INTEGER NOT NULL,
    updated_at_secs INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS webhook_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    url TEXT NOT NULL DEFAULT '',
    headers_json TEXT NOT NULL DEFAULT '{}',
    timeout_secs INTEGER NOT NULL DEFAULT 10,
    updated_at_secs INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS webhook_endpoints (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    url TEXT NOT NULL,
    headers_json TEXT NOT NULL DEFAULT '{}',
    timeout_secs INTEGER NOT NULL DEFAULT 10,
    created_at_secs INTEGER NOT NULL,
    updated_at_secs INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    auto_analyze_on_alert INTEGER NOT NULL DEFAULT 1,
    provider TEXT NOT NULL DEFAULT 'agy',
    custom_command TEXT NOT NULL DEFAULT '',
    extra_args TEXT NOT NULL DEFAULT '',
    model TEXT,
    updated_at_secs INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS ai_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_proposal_json TEXT,
    updated_at_secs INTEGER NOT NULL DEFAULT 0
);
";

/// Application-wide user preferences persisted in `app_settings`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub power_preset: String,
    pub default_depth: u32,
    pub apparent_size: bool,
    pub include_hidden: bool,
    pub follow_system_appearance: bool,
    pub ssh_session_duration_minutes: u64,
    pub updated_at_secs: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            power_preset: DEFAULT_POWER_PRESET.to_owned(),
            default_depth: DEFAULT_DEPTH,
            apparent_size: false,
            include_hidden: true,
            follow_system_appearance: true,
            ssh_session_duration_minutes: DEFAULT_SSH_SESSION_DURATION_MINUTES,
            updated_at_secs: 0,
        }
    }
}

impl AppSettings {
    /// Normalize fields to valid bounds and defaults.
    #[must_use]
    pub fn normalized(&self) -> Self {
        let preset = self.power_preset.trim();
        Self {
            power_preset: if preset.is_empty() {
                DEFAULT_POWER_PRESET.to_owned()
            } else {
                preset.to_owned()
            },
            default_depth: self.default_depth.clamp(MIN_DEPTH, MAX_DEPTH),
            apparent_size: self.apparent_size,
            include_hidden: self.include_hidden,
            follow_system_appearance: self.follow_system_appearance,
            ssh_session_duration_minutes: self
                .ssh_session_duration_minutes
                .max(MIN_SSH_SESSION_DURATION_MINUTES),
            updated_at_secs: self.updated_at_secs,
        }
    }
}

/// Authentication mechanism for a saved SSH profile.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Password,
    #[default]
    PrivateKey,
}

impl AuthMode {
    /// Canonical `SQLite` representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PrivateKey => "private_key",
        }
    }

    /// Parse from canonical `SQLite` representation.
    pub fn parse(raw: &str) -> io::Result<Self> {
        match raw.trim() {
            "password" => Ok(Self::Password),
            "private_key" => Ok(Self::PrivateKey),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown ssh auth_mode: `{other}`"),
            )),
        }
    }
}

/// Saved remote SSH server metadata (secrets live exclusively in keychain).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshProfile {
    pub id: String,
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_mode: AuthMode,
    pub default_path: Option<String>,
    pub last_total_bytes: Option<u64>,
    pub last_available_bytes: Option<u64>,
    pub last_scanned_secs: Option<u64>,
    pub created_at_secs: u64,
    pub updated_at_secs: u64,
}

impl SshProfile {
    /// Construct a new SSH profile with an auto-assigned `id` on insert.
    #[must_use]
    pub fn new(
        alias: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        auth_mode: AuthMode,
    ) -> Self {
        Self {
            id: String::new(),
            alias: alias.into(),
            host: host.into(),
            port,
            username: username.into(),
            auth_mode,
            default_path: None,
            last_total_bytes: None,
            last_available_bytes: None,
            last_scanned_secs: None,
            created_at_secs: 0,
            updated_at_secs: 0,
        }
    }
}

/// Generic HTTP webhook alert configuration persisted in `webhook_config`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookConfig {
    pub enabled: bool,
    pub url: String,
    pub headers_json: String,
    pub timeout_secs: u64,
    pub updated_at_secs: u64,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            headers_json: "{}".to_owned(),
            timeout_secs: DEFAULT_WEBHOOK_TIMEOUT_SECS,
            updated_at_secs: 0,
        }
    }
}

fn validate_webhook_url(raw: &str, allow_empty: bool) -> io::Result<String> {
    let url = raw.trim().to_owned();
    if url.is_empty() {
        if allow_empty {
            return Ok(url);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "webhook url must not be empty",
        ));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "webhook url must not contain whitespace or control chars",
        ));
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "webhook url must start with http:// or https://",
        ));
    }
    Ok(url)
}

fn validate_webhook_name(raw: &str) -> io::Result<String> {
    let name = raw.trim().to_owned();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "webhook name must be non-empty and free of control chars",
        ));
    }
    Ok(name)
}

impl WebhookConfig {
    /// Validate and parse `headers_json` into a deterministic header map.
    pub fn parsed_headers(&self) -> io::Result<BTreeMap<String, String>> {
        parse_headers_json(&self.headers_json)
    }

    /// Validate fields and normalize bounds.
    pub fn validated(&self) -> io::Result<Self> {
        let _ = parse_headers_json(&self.headers_json)?;
        let url = validate_webhook_url(&self.url, true)?;
        Ok(Self {
            enabled: self.enabled,
            url,
            headers_json: self.headers_json.trim().to_owned(),
            timeout_secs: self.timeout_secs.max(MIN_WEBHOOK_TIMEOUT_SECS),
            updated_at_secs: self.updated_at_secs,
        })
    }
}

/// Named HTTP webhook endpoint persisted in `webhook_endpoints`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookEndpoint {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub url: String,
    pub headers_json: String,
    pub timeout_secs: u64,
    pub created_at_secs: u64,
    pub updated_at_secs: u64,
}

impl Default for WebhookEndpoint {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            url: String::new(),
            headers_json: "{}".to_owned(),
            timeout_secs: DEFAULT_WEBHOOK_TIMEOUT_SECS,
            created_at_secs: 0,
            updated_at_secs: 0,
        }
    }
}

impl WebhookEndpoint {
    /// Construct a new enabled webhook endpoint with an auto-assigned `id` on
    /// insert.
    #[must_use]
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            ..Self::default()
        }
    }

    /// Validate and parse `headers_json` into a deterministic header map.
    pub fn parsed_headers(&self) -> io::Result<BTreeMap<String, String>> {
        parse_headers_json(&self.headers_json)
    }

    /// Validate fields and normalize bounds.
    pub fn validated(&self) -> io::Result<Self> {
        let name = validate_webhook_name(&self.name)?;
        let _ = parse_headers_json(&self.headers_json)?;
        let url = validate_webhook_url(&self.url, false)?;
        Ok(Self {
            id: self.id.trim().to_owned(),
            name,
            enabled: self.enabled,
            url,
            headers_json: self.headers_json.trim().to_owned(),
            timeout_secs: self.timeout_secs.max(MIN_WEBHOOK_TIMEOUT_SECS),
            created_at_secs: self.created_at_secs,
            updated_at_secs: self.updated_at_secs,
        })
    }
}

/// Validate a JSON object string representing HTTP headers.
pub fn parse_headers_json(raw: &str) -> io::Result<BTreeMap<String, String>> {
    let parsed: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid webhook headers_json: {e}"),
            )
        })?;
    let obj = parsed.as_object().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "webhook headers_json must be a JSON object",
        )
    })?;
    let mut headers = BTreeMap::new();
    for (name, value) in obj {
        if !is_valid_header_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid HTTP header name in headers_json: `{name}`"),
            ));
        }
        let val_str = value.as_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("header `{name}` value must be a JSON string"),
            )
        })?;
        if val_str.chars().any(|c| c == '\r' || c == '\n' || c == '\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("header `{name}` value contains forbidden characters"),
            ));
        }
        headers.insert(name.clone(), val_str.to_owned());
    }
    Ok(headers)
}

fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Independent AI analysis configuration persisted in `ai_config`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub auto_analyze_on_alert: bool,
    pub provider: AiCliProvider,
    pub custom_command: String,
    pub extra_args: String,
    pub model: Option<String>,
    pub updated_at_secs: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            auto_analyze_on_alert: true,
            provider: AiCliProvider::Agy,
            custom_command: String::new(),
            extra_args: String::new(),
            model: None,
            updated_at_secs: 0,
        }
    }
}

impl AiConfig {
    /// Normalize optional model and string fields.
    #[must_use]
    pub fn normalized(&self) -> Self {
        let model = self
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        Self {
            auto_analyze_on_alert: self.auto_analyze_on_alert,
            provider: self.provider,
            custom_command: self.custom_command.trim().to_owned(),
            extra_args: self.extra_args.trim().to_owned(),
            model,
            updated_at_secs: self.updated_at_secs,
        }
    }
}

/// Persistent AI cleanup proposal state stored in `ai_state`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiState {
    pub last_proposal_json: Option<String>,
    pub updated_at_secs: u64,
}

impl AiState {
    /// Deserialize `last_proposal_json` into a [`CleanupProposal`], if present.
    pub fn parse_proposal(&self) -> io::Result<Option<CleanupProposal>> {
        let Some(raw) = self.last_proposal_json.as_deref() else {
            return Ok(None);
        };
        let proposal: CleanupProposal =
            serde_json::from_str(raw).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid last_proposal_json: {e}"),
                )
            })?;
        Ok(Some(proposal))
    }

    /// Construct an [`AiState`] from a structured [`CleanupProposal`].
    pub fn from_proposal(
        proposal: &CleanupProposal,
        updated_at_secs: u64,
    ) -> io::Result<Self> {
        let json = serde_json::to_string(proposal).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("failed to serialize cleanup proposal: {e}"),
            )
        })?;
        Ok(Self {
            last_proposal_json: Some(json),
            updated_at_secs,
        })
    }

    /// Remove confirmed/cleaned paths from `last_proposal_json`, clearing
    /// `last_proposal_json` to `None` when no items remain.
    ///
    /// Returns the remaining proposal (`None` when absent or empty).
    pub fn prune_confirmed_paths(
        &mut self,
        removed_paths: &[PathBuf],
        now_secs: u64,
    ) -> io::Result<Option<CleanupProposal>> {
        let Some(mut proposal) = self.parse_proposal()? else {
            return Ok(None);
        };
        let pruned = proposal.prune_paths(removed_paths);
        if proposal.items.is_empty() {
            if self.last_proposal_json.is_some() {
                self.last_proposal_json = None;
                self.updated_at_secs = now_secs;
            }
            return Ok(None);
        }
        if pruned > 0 {
            let json = serde_json::to_string(&proposal).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("failed to serialize cleanup proposal: {e}"),
                )
            })?;
            self.last_proposal_json = Some(json);
            self.updated_at_secs = now_secs;
        }
        Ok(Some(proposal))
    }
}

/// Unified `SQLite` storage handle for `ruum.db`.
pub struct StorageDb {
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl fmt::Debug for StorageDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageDb")
            .field("db_path", &self.db_path)
            .finish_non_exhaustive()
    }
}

fn sqlite_err(err: &rusqlite::Error) -> io::Error {
    if let rusqlite::Error::SqliteFailure(ffi_err, detail) = err
        && ffi_err.code == ErrorCode::ConstraintViolation
    {
        let msg = detail.as_deref().unwrap_or("sqlite constraint violation");
        if msg.contains("UNIQUE") {
            return io::Error::new(io::ErrorKind::AlreadyExists, msg);
        }
        return io::Error::new(io::ErrorKind::InvalidInput, msg);
    }
    io::Error::other(err.to_string())
}

fn u64_to_sqlite_i64(val: u64) -> i64 {
    i64::try_from(val).unwrap_or(i64::MAX)
}

fn sqlite_i64_to_u64(val: i64) -> u64 {
    u64::try_from(val).unwrap_or(0)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn generate_profile_id() -> String {
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = NEXT_PROFILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut hasher = Sha256::new();
    hasher.update(now_nanos.to_le_bytes());
    hasher.update(seq.to_le_bytes());
    hasher.update(pid.to_le_bytes());
    let d = hasher.finalize();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-{:02x}{:02x}-\
         {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        d[0],
        d[1],
        d[2],
        d[3],
        d[4],
        d[5],
        d[6] & 0x0f,
        d[7],
        (d[8] & 0x3f) | 0x80,
        d[9],
        d[10],
        d[11],
        d[12],
        d[13],
        d[14],
        d[15],
    )
}

struct NormalizedProfileFields {
    alias: String,
    host: String,
    port: u16,
    username: String,
    auth_mode: AuthMode,
    default_path: Option<String>,
}

fn normalize_profile_fields(
    profile: &SshProfile,
) -> io::Result<NormalizedProfileFields> {
    let alias = profile.alias.trim().to_owned();
    if alias.is_empty() || alias.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ssh profile alias must be non-empty and free of control chars",
        ));
    }
    let host = profile.host.trim().to_owned();
    if host.is_empty()
        || host.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ssh profile host must be non-empty without whitespace",
        ));
    }
    if profile.port == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ssh profile port must be between 1 and 65535",
        ));
    }
    let username = profile.username.trim().to_owned();
    if username.is_empty()
        || username
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ssh profile username must be non-empty without whitespace",
        ));
    }
    let default_path = profile
        .default_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    Ok(NormalizedProfileFields {
        alias,
        host,
        port: profile.port,
        username,
        auth_mode: profile.auth_mode,
        default_path,
    })
}

fn row_to_ssh_profile(row: &Row<'_>) -> rusqlite::Result<SshProfile> {
    let id: String = row.get(0)?;
    let alias: String = row.get(1)?;
    let host: String = row.get(2)?;
    let port_raw: i64 = row.get(3)?;
    let username: String = row.get(4)?;
    let auth_mode_raw: String = row.get(5)?;
    let default_path: Option<String> = row.get(6)?;
    let last_total: Option<i64> = row.get(7)?;
    let last_avail: Option<i64> = row.get(8)?;
    let last_scanned: Option<i64> = row.get(9)?;
    let created_at: i64 = row.get(10)?;
    let updated_at: i64 = row.get(11)?;

    let port = u16::try_from(port_raw).unwrap_or(DEFAULT_SSH_PORT);
    let auth_mode = AuthMode::parse(&auth_mode_raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;

    Ok(SshProfile {
        id,
        alias,
        host,
        port,
        username,
        auth_mode,
        default_path,
        last_total_bytes: last_total.and_then(|v| u64::try_from(v).ok()),
        last_available_bytes: last_avail.and_then(|v| u64::try_from(v).ok()),
        last_scanned_secs: last_scanned.and_then(|v| u64::try_from(v).ok()),
        created_at_secs: sqlite_i64_to_u64(created_at),
        updated_at_secs: sqlite_i64_to_u64(updated_at),
    })
}

const SSH_PROFILE_SELECT: &str = "\
SELECT id, alias, host, port, username, auth_mode, default_path, \
       last_total_bytes, last_available_bytes, last_scanned_secs, \
       created_at_secs, updated_at_secs \
FROM ssh_profiles";

const WEBHOOK_ENDPOINT_SELECT: &str = "\
SELECT id, name, enabled, url, headers_json, timeout_secs, \
       created_at_secs, updated_at_secs \
FROM webhook_endpoints";

fn row_to_webhook_endpoint(row: &Row<'_>) -> rusqlite::Result<WebhookEndpoint> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let enabled: i64 = row.get(2)?;
    let url: String = row.get(3)?;
    let headers_json: String = row.get(4)?;
    let timeout_secs: i64 = row.get(5)?;
    let created_at_secs: i64 = row.get(6)?;
    let updated_at_secs: i64 = row.get(7)?;
    Ok(WebhookEndpoint {
        id,
        name,
        enabled: enabled != 0,
        url,
        headers_json,
        timeout_secs: sqlite_i64_to_u64(timeout_secs)
            .max(MIN_WEBHOOK_TIMEOUT_SECS),
        created_at_secs: sqlite_i64_to_u64(created_at_secs),
        updated_at_secs: sqlite_i64_to_u64(updated_at_secs),
    })
}

fn migrate_legacy_webhook_config(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO webhook_endpoints (
            id, name, enabled, url, headers_json,
            timeout_secs, created_at_secs, updated_at_secs
         )
         SELECT
            ?1,
            ?2,
            enabled,
            TRIM(url),
            headers_json,
            CASE WHEN timeout_secs < 1 THEN 1 ELSE timeout_secs END,
            updated_at_secs,
            updated_at_secs
         FROM webhook_config
         WHERE id = 1
           AND TRIM(url) <> ''
           AND NOT EXISTS (SELECT 1 FROM webhook_endpoints)",
        params![DEFAULT_WEBHOOK_ID, DEFAULT_WEBHOOK_NAME],
    )?;
    Ok(())
}

const fn is_sqlite_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(ffi, _)
            if matches!(
                ffi.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            )
    )
}

fn retry_on_busy<T>(
    mut op: impl FnMut() -> rusqlite::Result<T>,
) -> io::Result<T> {
    const MAX_ATTEMPTS: usize = 40;
    const BACKOFF: Duration = Duration::from_millis(25);
    let mut attempt = 0;
    loop {
        match op() {
            Ok(val) => return Ok(val),
            Err(err) if is_sqlite_busy(&err) && attempt + 1 < MAX_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(BACKOFF);
            }
            Err(err) => return Err(sqlite_err(&err)),
        }
    }
}

impl StorageDb {
    /// Open (or create) `ruum.db` at `path`, configure `busy_timeout = 5000`
    /// before `journal_mode = WAL`, and ensure owned tables exist.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&db_path).map_err(|e| sqlite_err(&e))?;
        // Set busy_timeout before switching to WAL and retry transient
        // SQLITE_BUSY/SQLITE_LOCKED during initial DELETE -> WAL transition.
        retry_on_busy(|| {
            conn.execute_batch(
                "PRAGMA busy_timeout = 5000;\nPRAGMA journal_mode = WAL;",
            )?;
            conn.execute_batch(SCHEMA_SQL)?;
            migrate_legacy_webhook_config(&conn)
        })?;
        Ok(Self {
            db_path,
            conn: Mutex::new(conn),
        })
    }

    /// Open the default `<config_dir>/ruum/ruum.db` database.
    pub fn open_default() -> io::Result<Self> {
        let path = ruum_watchdog::config::config_dir()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot resolve ruum config directory",
                )
            })?
            .join("ruum.db");
        Self::open(path)
    }

    /// Path to the underlying `SQLite` database file.
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Open a [`WatchdogConfigService`] backed by the exact same `ruum.db`.
    pub fn watchdog(&self) -> io::Result<WatchdogConfigService> {
        WatchdogConfigService::open(&self.db_path)
    }

    fn lock_conn(&self) -> io::Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| io::Error::other("storage db mutex poisoned"))
    }

    /// Load `app_settings`, returning defaults if no row has been saved yet.
    pub fn load_app_settings(&self) -> io::Result<AppSettings> {
        let row = {
            let conn = self.lock_conn()?;
            conn.query_row(
                "SELECT power_preset, default_depth, apparent_size, \
                 include_hidden, follow_system_appearance, \
                 ssh_session_duration_minutes, updated_at_secs \
                 FROM app_settings WHERE id = 1",
                [],
                |row| {
                    let power_preset: String = row.get(0)?;
                    let default_depth: i64 = row.get(1)?;
                    let apparent_size: i64 = row.get(2)?;
                    let include_hidden: i64 = row.get(3)?;
                    let follow_system_appearance: i64 = row.get(4)?;
                    let ssh_duration: i64 = row.get(5)?;
                    let updated_at_secs: i64 = row.get(6)?;
                    Ok(AppSettings {
                        power_preset,
                        default_depth: u32::try_from(default_depth)
                            .unwrap_or(DEFAULT_DEPTH),
                        apparent_size: apparent_size != 0,
                        include_hidden: include_hidden != 0,
                        follow_system_appearance: follow_system_appearance != 0,
                        ssh_session_duration_minutes: sqlite_i64_to_u64(
                            ssh_duration,
                        ),
                        updated_at_secs: sqlite_i64_to_u64(updated_at_secs),
                    })
                },
            )
            .optional()
            .map_err(|e| sqlite_err(&e))?
        };
        Ok(row.unwrap_or_default().normalized())
    }

    /// Persist normalized `app_settings`.
    pub fn save_app_settings(&self, settings: &AppSettings) -> io::Result<()> {
        let norm = settings.normalized();
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO app_settings (
                id, power_preset, default_depth, apparent_size,
                include_hidden, follow_system_appearance,
                ssh_session_duration_minutes, updated_at_secs
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                power_preset = excluded.power_preset,
                default_depth = excluded.default_depth,
                apparent_size = excluded.apparent_size,
                include_hidden = excluded.include_hidden,
                follow_system_appearance = excluded.follow_system_appearance,
                ssh_session_duration_minutes =
                    excluded.ssh_session_duration_minutes,
                updated_at_secs = excluded.updated_at_secs",
            params![
                norm.power_preset,
                i64::from(norm.default_depth),
                i64::from(norm.apparent_size),
                i64::from(norm.include_hidden),
                i64::from(norm.follow_system_appearance),
                u64_to_sqlite_i64(norm.ssh_session_duration_minutes),
                u64_to_sqlite_i64(norm.updated_at_secs),
            ],
        )
        .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(())
    }

    /// Insert a new SSH profile, generating a unique `id` when empty.
    pub fn add_ssh_profile(
        &self,
        profile: &SshProfile,
    ) -> io::Result<SshProfile> {
        let fields = normalize_profile_fields(profile)?;
        let id = {
            let trimmed = profile.id.trim();
            if trimmed.is_empty() {
                generate_profile_id()
            } else if trimmed
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "ssh profile id must not contain whitespace or control chars",
                ));
            } else {
                trimmed.to_owned()
            }
        };

        let now = now_unix_secs();
        let created_at = if profile.created_at_secs == 0 {
            now
        } else {
            profile.created_at_secs
        };
        let updated_at = if profile.updated_at_secs == 0 {
            created_at
        } else {
            profile.updated_at_secs
        };

        let inserted = SshProfile {
            id,
            alias: fields.alias,
            host: fields.host,
            port: fields.port,
            username: fields.username,
            auth_mode: fields.auth_mode,
            default_path: fields.default_path,
            last_total_bytes: profile.last_total_bytes,
            last_available_bytes: profile.last_available_bytes,
            last_scanned_secs: profile.last_scanned_secs,
            created_at_secs: created_at,
            updated_at_secs: updated_at,
        };

        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO ssh_profiles (
                id, alias, host, port, username, auth_mode, default_path,
                last_total_bytes, last_available_bytes, last_scanned_secs,
                created_at_secs, updated_at_secs
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                inserted.id,
                inserted.alias,
                inserted.host,
                i64::from(inserted.port),
                inserted.username,
                inserted.auth_mode.as_str(),
                inserted.default_path,
                inserted.last_total_bytes.map(u64_to_sqlite_i64),
                inserted.last_available_bytes.map(u64_to_sqlite_i64),
                inserted.last_scanned_secs.map(u64_to_sqlite_i64),
                u64_to_sqlite_i64(inserted.created_at_secs),
                u64_to_sqlite_i64(inserted.updated_at_secs),
            ],
        )
        .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(inserted)
    }

    /// Update an existing SSH profile's editable metadata by `id`.
    pub fn update_ssh_profile(
        &self,
        profile: &SshProfile,
    ) -> io::Result<SshProfile> {
        let id = profile.id.trim();
        if id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ssh profile id must not be empty when updating",
            ));
        }
        let fields = normalize_profile_fields(profile)?;
        let updated_at = if profile.updated_at_secs == 0 {
            now_unix_secs()
        } else {
            profile.updated_at_secs
        };

        let changed = {
            let conn = self.lock_conn()?;
            conn.execute(
                "UPDATE ssh_profiles SET
                    alias = ?2,
                    host = ?3,
                    port = ?4,
                    username = ?5,
                    auth_mode = ?6,
                    default_path = ?7,
                    updated_at_secs = ?8
                 WHERE id = ?1",
                params![
                    id,
                    fields.alias,
                    fields.host,
                    i64::from(fields.port),
                    fields.username,
                    fields.auth_mode.as_str(),
                    fields.default_path,
                    u64_to_sqlite_i64(updated_at),
                ],
            )
            .map_err(|e| sqlite_err(&e))?
        };
        if changed == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("ssh profile `{id}` not found"),
            ));
        }
        self.get_ssh_profile_by_id(id)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("ssh profile `{id}` vanished after update"),
            )
        })
    }

    /// Fetch a saved SSH profile by primary key `id`.
    pub fn get_ssh_profile_by_id(
        &self,
        id: &str,
    ) -> io::Result<Option<SshProfile>> {
        let sql = format!("{SSH_PROFILE_SELECT} WHERE id = ?1");
        let conn = self.lock_conn()?;
        let res = conn
            .query_row(&sql, [id.trim()], row_to_ssh_profile)
            .optional()
            .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(res)
    }

    /// Fetch a saved SSH profile by unique display `alias`.
    pub fn get_ssh_profile_by_alias(
        &self,
        alias: &str,
    ) -> io::Result<Option<SshProfile>> {
        let sql = format!("{SSH_PROFILE_SELECT} WHERE alias = ?1");
        let conn = self.lock_conn()?;
        let res = conn
            .query_row(&sql, [alias.trim()], row_to_ssh_profile)
            .optional()
            .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(res)
    }

    /// Fetch a saved SSH profile matching either `id` or `alias`.
    pub fn get_ssh_profile(
        &self,
        id_or_alias: &str,
    ) -> io::Result<Option<SshProfile>> {
        let key = id_or_alias.trim();
        if key.is_empty() {
            return Ok(None);
        }
        let sql =
            format!("{SSH_PROFILE_SELECT} WHERE id = ?1 OR alias = ?1 LIMIT 1");
        let conn = self.lock_conn()?;
        let res = conn
            .query_row(&sql, [key], row_to_ssh_profile)
            .optional()
            .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(res)
    }

    /// List all saved SSH profiles ordered deterministically by `alias`.
    pub fn list_ssh_profiles(&self) -> io::Result<Vec<SshProfile>> {
        let sql = format!(
            "{SSH_PROFILE_SELECT} \
             ORDER BY alias COLLATE NOCASE ASC, created_at_secs ASC, id ASC"
        );
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(&sql).map_err(|e| sqlite_err(&e))?;
        let rows = stmt
            .query_map([], row_to_ssh_profile)
            .map_err(|e| sqlite_err(&e))?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item.map_err(|e| sqlite_err(&e))?);
        }
        drop(stmt);
        drop(conn);
        Ok(out)
    }

    /// Delete a saved SSH profile by `id`, returning whether a row existed.
    pub fn delete_ssh_profile(&self, id: &str) -> io::Result<bool> {
        let conn = self.lock_conn()?;
        let changed = conn
            .execute("DELETE FROM ssh_profiles WHERE id = ?1", [id.trim()])
            .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(changed > 0)
    }

    /// Update the cached remote disk scan summary for an SSH profile.
    pub fn update_ssh_profile_scan_summary(
        &self,
        id: &str,
        total_bytes: u64,
        available_bytes: u64,
        scanned_secs: u64,
    ) -> io::Result<()> {
        let id = id.trim();
        let changed = {
            let conn = self.lock_conn()?;
            conn.execute(
                "UPDATE ssh_profiles SET
                    last_total_bytes = ?2,
                    last_available_bytes = ?3,
                    last_scanned_secs = ?4,
                    updated_at_secs = ?5
                 WHERE id = ?1",
                params![
                    id,
                    u64_to_sqlite_i64(total_bytes),
                    u64_to_sqlite_i64(available_bytes),
                    u64_to_sqlite_i64(scanned_secs),
                    u64_to_sqlite_i64(scanned_secs),
                ],
            )
            .map_err(|e| sqlite_err(&e))?
        };
        if changed == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("ssh profile `{id}` not found"),
            ));
        }
        Ok(())
    }

    /// List all configured webhook endpoints ordered deterministically by
    /// `name`.
    pub fn list_webhooks(&self) -> io::Result<Vec<WebhookEndpoint>> {
        let sql = format!(
            "{WEBHOOK_ENDPOINT_SELECT} \
             ORDER BY name COLLATE NOCASE ASC, created_at_secs ASC, id ASC"
        );
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(&sql).map_err(|e| sqlite_err(&e))?;
        let rows = stmt
            .query_map([], row_to_webhook_endpoint)
            .map_err(|e| sqlite_err(&e))?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item.map_err(|e| sqlite_err(&e))?);
        }
        drop(stmt);
        drop(conn);
        Ok(out)
    }

    /// Fetch a webhook endpoint matching either `id` or `name` (preferring
    /// exact `id` match).
    pub fn get_webhook(
        &self,
        id_or_name: &str,
    ) -> io::Result<Option<WebhookEndpoint>> {
        let key = id_or_name.trim();
        if key.is_empty() {
            return Ok(None);
        }
        let sql = format!(
            "{WEBHOOK_ENDPOINT_SELECT} \
             WHERE id = ?1 OR name = ?1 \
             ORDER BY CASE WHEN id = ?1 THEN 0 ELSE 1 END, \
                      created_at_secs ASC, id ASC \
             LIMIT 1"
        );
        let conn = self.lock_conn()?;
        let res = conn
            .query_row(&sql, [key], row_to_webhook_endpoint)
            .optional()
            .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(res)
    }

    /// Insert a new webhook endpoint, generating a unique `id` when empty.
    pub fn add_webhook(
        &self,
        endpoint: &WebhookEndpoint,
    ) -> io::Result<WebhookEndpoint> {
        let clean = endpoint.validated()?;
        let id = if clean.id.is_empty() {
            generate_profile_id()
        } else if clean
            .id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "webhook id must not contain whitespace or control chars",
            ));
        } else {
            clean.id
        };

        let now = now_unix_secs();
        let created_at = if clean.created_at_secs == 0 {
            now
        } else {
            clean.created_at_secs
        };
        let updated_at = if clean.updated_at_secs == 0 {
            created_at
        } else {
            clean.updated_at_secs
        };

        let inserted = WebhookEndpoint {
            id,
            name: clean.name,
            enabled: clean.enabled,
            url: clean.url,
            headers_json: clean.headers_json,
            timeout_secs: clean.timeout_secs,
            created_at_secs: created_at,
            updated_at_secs: updated_at,
        };

        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO webhook_endpoints (
                id, name, enabled, url, headers_json,
                timeout_secs, created_at_secs, updated_at_secs
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                inserted.id,
                inserted.name,
                i64::from(inserted.enabled),
                inserted.url,
                inserted.headers_json,
                u64_to_sqlite_i64(inserted.timeout_secs),
                u64_to_sqlite_i64(inserted.created_at_secs),
                u64_to_sqlite_i64(inserted.updated_at_secs),
            ],
        )
        .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(inserted)
    }

    /// Update an existing webhook endpoint by `id`.
    pub fn update_webhook(
        &self,
        endpoint: &WebhookEndpoint,
    ) -> io::Result<WebhookEndpoint> {
        let clean = endpoint.validated()?;
        if clean.id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "webhook id must not be empty when updating",
            ));
        }
        let updated_at = if clean.updated_at_secs == 0 {
            now_unix_secs()
        } else {
            clean.updated_at_secs
        };

        let changed = {
            let conn = self.lock_conn()?;
            conn.execute(
                "UPDATE webhook_endpoints SET
                    name = ?2,
                    enabled = ?3,
                    url = ?4,
                    headers_json = ?5,
                    timeout_secs = ?6,
                    updated_at_secs = ?7
                 WHERE id = ?1",
                params![
                    clean.id,
                    clean.name,
                    i64::from(clean.enabled),
                    clean.url,
                    clean.headers_json,
                    u64_to_sqlite_i64(clean.timeout_secs),
                    u64_to_sqlite_i64(updated_at),
                ],
            )
            .map_err(|e| sqlite_err(&e))?
        };
        if changed == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("webhook `{}` not found", clean.id),
            ));
        }
        self.get_webhook(&clean.id)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("webhook `{}` vanished after update", clean.id),
            )
        })
    }

    /// Delete a webhook endpoint by `id`, returning whether a row existed.
    pub fn delete_webhook(&self, id: &str) -> io::Result<bool> {
        let id = id.trim();
        if id.is_empty() {
            return Ok(false);
        }
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err(&e))?;
        let changed = tx
            .execute("DELETE FROM webhook_endpoints WHERE id = ?1", [id])
            .map_err(|e| sqlite_err(&e))?;
        if changed > 0 {
            let has_remaining: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM webhook_endpoints)",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| sqlite_err(&e))?;
            if !has_remaining {
                tx.execute("DELETE FROM webhook_config WHERE id = 1", [])
                    .map_err(|e| sqlite_err(&e))?;
            }
        }
        tx.commit().map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(changed > 0)
    }

    /// Load `webhook_config`, reading the primary endpoint in
    /// `webhook_endpoints` first and falling back to legacy `webhook_config`.
    pub fn load_webhook_config(&self) -> io::Result<WebhookConfig> {
        let conn = self.lock_conn()?;
        let endpoint_row = conn
            .query_row(
                "SELECT enabled, url, headers_json, timeout_secs, \
                 updated_at_secs FROM webhook_endpoints \
                 ORDER BY CASE WHEN id = ?1 THEN 0 ELSE 1 END, \
                          created_at_secs ASC, id ASC \
                 LIMIT 1",
                [DEFAULT_WEBHOOK_ID],
                |row| {
                    let enabled: i64 = row.get(0)?;
                    let url: String = row.get(1)?;
                    let headers_json: String = row.get(2)?;
                    let timeout_secs: i64 = row.get(3)?;
                    let updated_at_secs: i64 = row.get(4)?;
                    Ok(WebhookConfig {
                        enabled: enabled != 0,
                        url,
                        headers_json,
                        timeout_secs: sqlite_i64_to_u64(timeout_secs)
                            .max(MIN_WEBHOOK_TIMEOUT_SECS),
                        updated_at_secs: sqlite_i64_to_u64(updated_at_secs),
                    })
                },
            )
            .optional()
            .map_err(|e| sqlite_err(&e))?;
        if let Some(cfg) = endpoint_row {
            return Ok(cfg);
        }

        let row = conn
            .query_row(
                "SELECT enabled, url, headers_json, timeout_secs, \
                 updated_at_secs FROM webhook_config WHERE id = 1",
                [],
                |row| {
                    let enabled: i64 = row.get(0)?;
                    let url: String = row.get(1)?;
                    let headers_json: String = row.get(2)?;
                    let timeout_secs: i64 = row.get(3)?;
                    let updated_at_secs: i64 = row.get(4)?;
                    Ok(WebhookConfig {
                        enabled: enabled != 0,
                        url,
                        headers_json,
                        timeout_secs: sqlite_i64_to_u64(timeout_secs)
                            .max(MIN_WEBHOOK_TIMEOUT_SECS),
                        updated_at_secs: sqlite_i64_to_u64(updated_at_secs),
                    })
                },
            )
            .optional()
            .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(row.unwrap_or_default())
    }

    /// Validate and persist `webhook_config`, keeping the primary
    /// `webhook_endpoints` row synchronized for backward compatibility.
    pub fn save_webhook_config(
        &self,
        config: &WebhookConfig,
    ) -> io::Result<()> {
        let clean = config.validated()?;
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err(&e))?;
        tx.execute(
            "INSERT INTO webhook_config (
                id, enabled, url, headers_json, timeout_secs, updated_at_secs
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                enabled = excluded.enabled,
                url = excluded.url,
                headers_json = excluded.headers_json,
                timeout_secs = excluded.timeout_secs,
                updated_at_secs = excluded.updated_at_secs",
            params![
                i64::from(clean.enabled),
                clean.url,
                clean.headers_json,
                u64_to_sqlite_i64(clean.timeout_secs),
                u64_to_sqlite_i64(clean.updated_at_secs),
            ],
        )
        .map_err(|e| sqlite_err(&e))?;

        if clean.url.is_empty() {
            tx.execute(
                "DELETE FROM webhook_endpoints WHERE id = ?1",
                [DEFAULT_WEBHOOK_ID],
            )
            .map_err(|e| sqlite_err(&e))?;
        } else {
            let existing_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM webhook_endpoints \
                     ORDER BY CASE WHEN id = ?1 THEN 0 ELSE 1 END, \
                              created_at_secs ASC, id ASC \
                     LIMIT 1",
                    [DEFAULT_WEBHOOK_ID],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| sqlite_err(&e))?;
            if let Some(target_id) = existing_id {
                tx.execute(
                    "UPDATE webhook_endpoints SET
                        enabled = ?2,
                        url = ?3,
                        headers_json = ?4,
                        timeout_secs = ?5,
                        updated_at_secs = ?6
                     WHERE id = ?1",
                    params![
                        target_id,
                        i64::from(clean.enabled),
                        clean.url,
                        clean.headers_json,
                        u64_to_sqlite_i64(clean.timeout_secs),
                        u64_to_sqlite_i64(clean.updated_at_secs),
                    ],
                )
                .map_err(|e| sqlite_err(&e))?;
            } else {
                tx.execute(
                    "INSERT INTO webhook_endpoints (
                        id, name, enabled, url, headers_json,
                        timeout_secs, created_at_secs, updated_at_secs
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        DEFAULT_WEBHOOK_ID,
                        DEFAULT_WEBHOOK_NAME,
                        i64::from(clean.enabled),
                        clean.url,
                        clean.headers_json,
                        u64_to_sqlite_i64(clean.timeout_secs),
                        u64_to_sqlite_i64(clean.updated_at_secs),
                        u64_to_sqlite_i64(clean.updated_at_secs),
                    ],
                )
                .map_err(|e| sqlite_err(&e))?;
            }
        }
        tx.commit().map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(())
    }

    /// Load `ai_config`, returning defaults if no row has been saved yet.
    pub fn load_ai_config(&self) -> io::Result<AiConfig> {
        let row = {
            let conn = self.lock_conn()?;
            conn.query_row(
                "SELECT auto_analyze_on_alert, provider, custom_command, \
                 extra_args, model, updated_at_secs \
                 FROM ai_config WHERE id = 1",
                [],
                |row| {
                    let auto_analyze: i64 = row.get(0)?;
                    let provider_raw: String = row.get(1)?;
                    let custom_command: String = row.get(2)?;
                    let extra_args: String = row.get(3)?;
                    let model: Option<String> = row.get(4)?;
                    let updated_at_secs: i64 = row.get(5)?;
                    let provider = AiCliProvider::from_id(&provider_raw)
                        .ok_or_else(|| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    format!(
                                        "unknown ai provider: `{provider_raw}`"
                                    ),
                                )),
                            )
                        })?;
                    Ok(AiConfig {
                        auto_analyze_on_alert: auto_analyze != 0,
                        provider,
                        custom_command,
                        extra_args,
                        model,
                        updated_at_secs: sqlite_i64_to_u64(updated_at_secs),
                    })
                },
            )
            .optional()
            .map_err(|e| sqlite_err(&e))?
        };
        Ok(row.unwrap_or_default().normalized())
    }

    /// Persist normalized `ai_config`.
    pub fn save_ai_config(&self, config: &AiConfig) -> io::Result<()> {
        let norm = config.normalized();
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO ai_config (
                id, auto_analyze_on_alert, provider, custom_command,
                extra_args, model, updated_at_secs
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                auto_analyze_on_alert = excluded.auto_analyze_on_alert,
                provider = excluded.provider,
                custom_command = excluded.custom_command,
                extra_args = excluded.extra_args,
                model = excluded.model,
                updated_at_secs = excluded.updated_at_secs",
            params![
                i64::from(norm.auto_analyze_on_alert),
                norm.provider.id(),
                norm.custom_command,
                norm.extra_args,
                norm.model,
                u64_to_sqlite_i64(norm.updated_at_secs),
            ],
        )
        .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(())
    }

    /// Load `ai_state`, returning empty state if no row has been saved yet.
    pub fn load_ai_state(&self) -> io::Result<AiState> {
        let row = {
            let conn = self.lock_conn()?;
            conn.query_row(
                "SELECT last_proposal_json, updated_at_secs \
                 FROM ai_state WHERE id = 1",
                [],
                |row| {
                    let last_proposal_json: Option<String> = row.get(0)?;
                    let updated_at_secs: i64 = row.get(1)?;
                    Ok(AiState {
                        last_proposal_json,
                        updated_at_secs: sqlite_i64_to_u64(updated_at_secs),
                    })
                },
            )
            .optional()
            .map_err(|e| sqlite_err(&e))?
        };
        Ok(row.unwrap_or_default())
    }

    /// Persist `ai_state`, validating `last_proposal_json` when present.
    pub fn save_ai_state(&self, state: &AiState) -> io::Result<()> {
        let clean_json = match state.last_proposal_json.as_deref() {
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    let _: serde_json::Value = serde_json::from_str(trimmed)
                        .map_err(|e| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                format!("invalid ai_state proposal JSON: {e}"),
                            )
                        })?;
                    Some(trimmed.to_owned())
                }
            }
            None => None,
        };

        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO ai_state (id, last_proposal_json, updated_at_secs)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                last_proposal_json = excluded.last_proposal_json,
                updated_at_secs = excluded.updated_at_secs",
            params![clean_json, u64_to_sqlite_i64(state.updated_at_secs)],
        )
        .map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(())
    }

    /// Remove confirmed/cleaned paths from `ai_state.last_proposal_json`,
    /// clearing `last_proposal_json` to `None` when no items remain, persisting
    /// the updated state, and returning the remaining proposal.
    pub fn prune_ai_state_paths(
        &self,
        removed_paths: &[PathBuf],
        now_secs: u64,
    ) -> io::Result<Option<CleanupProposal>> {
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err(&e))?;
        let row = tx
            .query_row(
                "SELECT last_proposal_json, updated_at_secs \
                 FROM ai_state WHERE id = 1",
                [],
                |row| {
                    let last_proposal_json: Option<String> = row.get(0)?;
                    let updated_at_secs: i64 = row.get(1)?;
                    Ok(AiState {
                        last_proposal_json,
                        updated_at_secs: sqlite_i64_to_u64(updated_at_secs),
                    })
                },
            )
            .optional()
            .map_err(|e| sqlite_err(&e))?;

        let mut state = row.unwrap_or_default();
        let before_json = state.last_proposal_json.clone();
        let remaining = state.prune_confirmed_paths(removed_paths, now_secs)?;
        if state.last_proposal_json != before_json {
            tx.execute(
                "INSERT INTO ai_state (id, last_proposal_json, updated_at_secs)
                 VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET
                    last_proposal_json = excluded.last_proposal_json,
                    updated_at_secs = excluded.updated_at_secs",
                params![
                    state.last_proposal_json,
                    u64_to_sqlite_i64(state.updated_at_secs),
                ],
            )
            .map_err(|e| sqlite_err(&e))?;
        }
        tx.commit().map_err(|e| sqlite_err(&e))?;
        drop(conn);
        Ok(remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::removal::RemovalMode;
    use crate::watchdog_config::ProposalItem;
    use ruum_watchdog::{WatchdogConfig, WatchdogState};
    use std::sync::{Arc, Barrier};

    #[test]
    fn open_configures_wal_busy_timeout_and_owns_only_core_tables() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("nested/ruum.db");

        let db = StorageDb::open(&db_path).unwrap();
        assert_eq!(db.db_path(), db_path.as_path());
        assert!(db_path.is_file());

        let (journal_mode, busy_timeout, tables): (String, i64, Vec<String>) = {
            let conn = db.conn.lock().unwrap();
            let mode: String = conn
                .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
                .unwrap();
            let timeout: i64 = conn
                .query_row("PRAGMA busy_timeout;", [], |r| r.get(0))
                .unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master \
                     WHERE type = 'table' ORDER BY name",
                )
                .unwrap();
            let names = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            drop(stmt);
            drop(conn);
            (mode, timeout, names)
        };

        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(busy_timeout, 5000);
        assert_eq!(
            tables,
            vec![
                "ai_config",
                "ai_state",
                "app_settings",
                "ssh_profiles",
                "webhook_config",
                "webhook_endpoints",
            ]
        );

        // Calling watchdog() shares the exact same file and initializes
        // watchdog tables without colliding with StorageDb.
        let wd = db.watchdog().unwrap();
        assert_eq!(wd.db_path(), db.db_path());
        assert_eq!(wd.load_config().unwrap(), WatchdogConfig::default());
        assert_eq!(wd.load_state().unwrap(), WatchdogState::default());
    }

    #[test]
    fn app_settings_defaults_normalization_and_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();

        let defaults = db.load_app_settings().unwrap();
        assert_eq!(defaults, AppSettings::default());
        assert_eq!(defaults.power_preset, "balanced");
        assert_eq!(defaults.default_depth, 3);
        assert!(!defaults.apparent_size);
        assert!(defaults.include_hidden);
        assert!(defaults.follow_system_appearance);
        assert_eq!(defaults.ssh_session_duration_minutes, 15);
        assert_eq!(defaults.updated_at_secs, 0);

        let custom = AppSettings {
            power_preset: "  ".to_owned(),
            default_depth: 99,
            apparent_size: true,
            include_hidden: false,
            follow_system_appearance: false,
            ssh_session_duration_minutes: 0,
            updated_at_secs: 1_700_000_010,
        };
        db.save_app_settings(&custom).unwrap();

        let loaded = db.load_app_settings().unwrap();
        assert_eq!(
            loaded,
            AppSettings {
                power_preset: "balanced".to_owned(),
                default_depth: 6,
                apparent_size: true,
                include_hidden: false,
                follow_system_appearance: false,
                ssh_session_duration_minutes: 1,
                updated_at_secs: 1_700_000_010,
            }
        );

        let custom2 = AppSettings {
            power_preset: "aggressive".to_owned(),
            default_depth: 0,
            ssh_session_duration_minutes: 30,
            updated_at_secs: 1_700_000_020,
            ..loaded
        };
        db.save_app_settings(&custom2).unwrap();
        let loaded2 = db.load_app_settings().unwrap();
        assert_eq!(loaded2.power_preset, "aggressive");
        assert_eq!(loaded2.default_depth, 1);
        assert_eq!(loaded2.ssh_session_duration_minutes, 30);
    }

    #[test]
    fn ssh_profiles_crud_unique_alias_and_scan_summary() {
        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();

        assert!(db.list_ssh_profiles().unwrap().is_empty());

        let mut prod = SshProfile::new(
            "prod-web",
            "10.0.1.25",
            2222,
            "deploy",
            AuthMode::PrivateKey,
        );
        prod.default_path = Some("/var/log".to_owned());
        prod.created_at_secs = 1_700_000_000;
        prod.updated_at_secs = 1_700_000_000;
        let inserted_prod = db.add_ssh_profile(&prod).unwrap();
        assert!(!inserted_prod.id.is_empty());
        assert_eq!(inserted_prod.alias, "prod-web");
        assert_eq!(inserted_prod.port, 2222);
        assert_eq!(inserted_prod.auth_mode, AuthMode::PrivateKey);
        assert_eq!(inserted_prod.default_path.as_deref(), Some("/var/log"));

        let mut staging = SshProfile::new(
            "alpha-staging",
            "staging.example.com",
            22,
            "ubuntu",
            AuthMode::Password,
        );
        staging.id = "custom-id-staging".to_owned();
        let inserted_staging = db.add_ssh_profile(&staging).unwrap();
        assert_eq!(inserted_staging.id, "custom-id-staging");
        assert_eq!(inserted_staging.auth_mode, AuthMode::Password);
        assert!(inserted_staging.created_at_secs > 0);

        // Duplicate alias must fail with AlreadyExists.
        let dup = SshProfile::new(
            "prod-web",
            "10.0.1.99",
            22,
            "root",
            AuthMode::Password,
        );
        let err = db.add_ssh_profile(&dup).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        // Invalid inputs rejected.
        let bad_port = SshProfile::new(
            "bad-port",
            "example.com",
            0,
            "user",
            AuthMode::Password,
        );
        assert_eq!(
            db.add_ssh_profile(&bad_port).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let empty_alias = SshProfile::new(
            "   ",
            "example.com",
            22,
            "user",
            AuthMode::Password,
        );
        assert_eq!(
            db.add_ssh_profile(&empty_alias).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );

        // Lookup by id, alias, and unified get_ssh_profile.
        assert_eq!(
            db.get_ssh_profile_by_id(&inserted_prod.id).unwrap(),
            Some(inserted_prod.clone())
        );
        assert_eq!(
            db.get_ssh_profile_by_alias("prod-web").unwrap(),
            Some(inserted_prod.clone())
        );
        assert_eq!(
            db.get_ssh_profile("alpha-staging").unwrap(),
            Some(inserted_staging)
        );
        assert_eq!(db.get_ssh_profile("missing").unwrap(), None);

        // List is ordered by alias.
        let list = db.list_ssh_profiles().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].alias, "alpha-staging");
        assert_eq!(list[1].alias, "prod-web");

        // Update scan summary.
        db.update_ssh_profile_scan_summary(
            &inserted_prod.id,
            100_000_000,
            25_000_000,
            1_700_000_500,
        )
        .unwrap();
        let after_scan = db
            .get_ssh_profile_by_id(&inserted_prod.id)
            .unwrap()
            .unwrap();
        assert_eq!(after_scan.last_total_bytes, Some(100_000_000));
        assert_eq!(after_scan.last_available_bytes, Some(25_000_000));
        assert_eq!(after_scan.last_scanned_secs, Some(1_700_000_500));
        assert_eq!(after_scan.updated_at_secs, 1_700_000_500);

        // Updating profile metadata preserves scan summary and enforces alias
        // uniqueness.
        let mut edit = after_scan;
        edit.alias = "alpha-staging".to_owned();
        assert_eq!(
            db.update_ssh_profile(&edit).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );

        edit.alias = "prod-api".to_owned();
        edit.port = 2200;
        edit.auth_mode = AuthMode::Password;
        edit.default_path = Some("   ".to_owned());
        edit.updated_at_secs = 1_700_000_900;
        let updated = db.update_ssh_profile(&edit).unwrap();
        assert_eq!(updated.alias, "prod-api");
        assert_eq!(updated.port, 2200);
        assert_eq!(updated.auth_mode, AuthMode::Password);
        assert_eq!(updated.default_path, None);
        assert_eq!(updated.last_total_bytes, Some(100_000_000));
        assert_eq!(db.get_ssh_profile_by_alias("prod-web").unwrap(), None);

        // Update/scan-summary for nonexistent ID fails with NotFound.
        let mut missing = updated;
        missing.id = "does-not-exist".to_owned();
        assert_eq!(
            db.update_ssh_profile(&missing).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            db.update_ssh_profile_scan_summary("does-not-exist", 1, 1, 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );

        // Delete profile.
        assert!(db.delete_ssh_profile(&inserted_prod.id).unwrap());
        assert!(!db.delete_ssh_profile(&inserted_prod.id).unwrap());
        assert_eq!(db.list_ssh_profiles().unwrap().len(), 1);
    }

    #[test]
    fn webhook_config_crud_and_headers_json_validation() {
        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();

        let def = db.load_webhook_config().unwrap();
        assert_eq!(def, WebhookConfig::default());
        assert_eq!(def.parsed_headers().unwrap(), BTreeMap::new());

        let valid = WebhookConfig {
            enabled: true,
            url: "https://hooks.example.com/ruum".to_owned(),
            headers_json: r#"{"Authorization":"Bearer secret","X-Env":"prod"}"#
                .to_owned(),
            timeout_secs: 0,
            updated_at_secs: 1_700_000_123,
        };
        db.save_webhook_config(&valid).unwrap();
        let loaded = db.load_webhook_config().unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.url, "https://hooks.example.com/ruum");
        assert_eq!(loaded.timeout_secs, 1);
        let headers = loaded.parsed_headers().unwrap();
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer secret")
        );
        assert_eq!(headers.get("X-Env").map(String::as_str), Some("prod"));

        // Invalid headers_json & URL payloads must be rejected.
        for bad_headers in [
            "not-json",
            "[1, 2, 3]",
            r#"{"X-Count": 42}"#,
            r#"{"Bad Header": "v"}"#,
            r#"{"X-Inject": "ok\r\nEvil: 1"}"#,
            r#"{"": "v"}"#,
        ] {
            let bad = WebhookConfig {
                headers_json: bad_headers.to_owned(),
                ..valid.clone()
            };
            assert_eq!(
                db.save_webhook_config(&bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "expected InvalidInput for headers_json={bad_headers}"
            );
        }

        let bad_scheme = WebhookConfig {
            url: "ftp://example.com/hook".to_owned(),
            ..valid
        };
        assert_eq!(
            db.save_webhook_config(&bad_scheme).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn ai_config_and_ai_state_crud_and_provider_mapping() {
        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();

        let def_cfg = db.load_ai_config().unwrap();
        assert_eq!(def_cfg, AiConfig::default());
        assert!(def_cfg.auto_analyze_on_alert);
        assert_eq!(def_cfg.provider, AiCliProvider::Agy);

        for provider in AiCliProvider::ALL {
            let cfg = AiConfig {
                auto_analyze_on_alert: provider != AiCliProvider::Codex,
                provider,
                custom_command: "my-ai".to_owned(),
                extra_args: "--json".to_owned(),
                model: Some("gpt-5".to_owned()),
                updated_at_secs: 1_700_000_300,
            };
            db.save_ai_config(&cfg).unwrap();
            assert_eq!(db.load_ai_config().unwrap(), cfg);
        }

        let empty_model = AiConfig {
            model: Some("   ".to_owned()),
            ..AiConfig::default()
        };
        db.save_ai_config(&empty_model).unwrap();
        assert_eq!(db.load_ai_config().unwrap().model, None);

        // AiState defaults & proposal serialization roundtrip.
        assert_eq!(db.load_ai_state().unwrap(), AiState::default());

        let proposal = CleanupProposal {
            timestamp_secs: 1_700_000_400,
            summary: "Clear cargo target cache".to_owned(),
            items: vec![ProposalItem::new(
                PathBuf::from("/tmp/target"),
                1_024_000,
                RemovalMode::Trash,
                Some("build cache".to_owned()),
                None,
            )],
        };
        let state = AiState::from_proposal(&proposal, 1_700_000_401).unwrap();
        db.save_ai_state(&state).unwrap();
        let loaded_state = db.load_ai_state().unwrap();
        assert_eq!(loaded_state, state);
        assert_eq!(loaded_state.parse_proposal().unwrap(), Some(proposal));

        // Invalid JSON in ai_state must be rejected.
        let invalid_state = AiState {
            last_proposal_json: Some("{not-json".to_owned()),
            updated_at_secs: 1,
        };
        assert_eq!(
            db.save_ai_state(&invalid_state).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn sqlite_table_check_constraints_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();
        let conn = db.conn.lock().unwrap();

        for singleton_insert in [
            "INSERT INTO app_settings (id) VALUES (2)",
            "INSERT INTO webhook_config (id) VALUES (2)",
            "INSERT INTO ai_config (id) VALUES (2)",
            "INSERT INTO ai_state (id) VALUES (2)",
        ] {
            assert!(
                conn.execute(singleton_insert, []).is_err(),
                "singleton check should reject: {singleton_insert}"
            );
        }

        assert!(
            conn.execute(
                "INSERT INTO ssh_profiles \
                 (id, alias, host, port, username, auth_mode, \
                  created_at_secs, updated_at_secs) \
                 VALUES ('p1', 'srv', 'h', 22, 'u', 'bogus_mode', 1, 1)",
                [],
            )
            .is_err(),
            "auth_mode CHECK constraint must reject invalid value"
        );
        drop(conn);
    }

    #[test]
    fn concurrent_storage_and_watchdog_service_access_has_no_sqlite_busy() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("shared/ruum.db");
        let thread_count = 6_usize;
        let barrier = Arc::new(Barrier::new(thread_count));

        let mut handles = Vec::with_capacity(thread_count);
        for tid in 0..thread_count {
            let path = db_path.clone();
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait();
                let db = StorageDb::open(&path).unwrap();
                let wd = db.watchdog().unwrap();

                for iter in 0_u64..10 {
                    let ts = 1_700_000_000 + (tid as u64) * 100 + iter;
                    db.save_app_settings(&AppSettings {
                        power_preset: "balanced".to_owned(),
                        default_depth: 3,
                        apparent_size: iter % 2 == 0,
                        include_hidden: true,
                        follow_system_appearance: true,
                        ssh_session_duration_minutes: 15 + iter,
                        updated_at_secs: ts,
                    })
                    .unwrap();

                    let alias = format!("host-{tid}-{iter}");
                    let inserted = db
                        .add_ssh_profile(&SshProfile::new(
                            alias,
                            "127.0.0.1",
                            22,
                            "tester",
                            AuthMode::PrivateKey,
                        ))
                        .unwrap();
                    db.update_ssh_profile_scan_summary(
                        &inserted.id,
                        10_000 + iter,
                        2_000 + iter,
                        ts,
                    )
                    .unwrap();

                    db.save_webhook_config(&WebhookConfig {
                        enabled: true,
                        url: "https://example.invalid/hook".to_owned(),
                        headers_json: r#"{"X-Worker":"1"}"#.to_owned(),
                        timeout_secs: 5,
                        updated_at_secs: ts,
                    })
                    .unwrap();

                    db.save_ai_config(&AiConfig {
                        auto_analyze_on_alert: true,
                        provider: AiCliProvider::Agy,
                        custom_command: String::new(),
                        extra_args: String::new(),
                        model: None,
                        updated_at_secs: ts,
                    })
                    .unwrap();

                    db.save_ai_state(&AiState {
                        last_proposal_json: Some(format!(
                            r#"{{"iter":{iter}}}"#
                        )),
                        updated_at_secs: ts,
                    })
                    .unwrap();

                    wd.save_config(&WatchdogConfig {
                        enabled: true,
                        background_service: false,
                        min_free_gb: 15 + iter,
                        poll_interval_secs: 60,
                        cooldown_secs: 300,
                        updated_at_secs: ts,
                    })
                    .unwrap();
                    wd.save_state(&WatchdogState {
                        last_checked_secs: Some(ts),
                        last_triggered_secs: Some(ts),
                        last_triggered_threshold_gb: Some(15 + iter),
                    })
                    .unwrap();

                    assert!(
                        db.load_app_settings().unwrap().updated_at_secs > 0
                    );
                    assert!(!db.list_ssh_profiles().unwrap().is_empty());
                    assert!(wd.load_config().unwrap().min_free_gb >= 15);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn webhook_endpoints_crud_validation_and_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();

        assert!(db.list_webhooks().unwrap().is_empty());

        let mut slack =
            WebhookEndpoint::new("slack-ops", "https://hooks.example.com/1");
        slack.headers_json = r#"{"Authorization":"Bearer s1"}"#.to_owned();
        slack.timeout_secs = 0; // clamped to 1
        let added_slack = db.add_webhook(&slack).unwrap();
        assert!(!added_slack.id.is_empty());
        assert_eq!(added_slack.name, "slack-ops");
        assert!(added_slack.enabled);
        assert_eq!(added_slack.timeout_secs, 1);

        let mut pager =
            WebhookEndpoint::new("alpha-pager", "http://127.0.0.1:8080/hook");
        pager.id = "ep-pager".to_owned();
        pager.enabled = false;
        let added_pager = db.add_webhook(&pager).unwrap();
        assert_eq!(added_pager.id, "ep-pager");
        assert!(!added_pager.enabled);

        // Lookup by id and by name.
        assert_eq!(
            db.get_webhook(&added_slack.id).unwrap(),
            Some(added_slack.clone())
        );
        assert_eq!(
            db.get_webhook("slack-ops").unwrap(),
            Some(added_slack.clone())
        );
        assert_eq!(db.get_webhook("missing").unwrap(), None);

        // Deterministic list ordering by name.
        let list = db.list_webhooks().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "alpha-pager");
        assert_eq!(list[1].name, "slack-ops");

        // Update endpoint.
        let mut edit = added_pager;
        edit.enabled = true;
        edit.name = "zeta-pager".to_owned();
        edit.timeout_secs = 25;
        let updated = db.update_webhook(&edit).unwrap();
        assert!(updated.enabled);
        assert_eq!(updated.name, "zeta-pager");
        assert_eq!(updated.timeout_secs, 25);

        // Validation rejects empty name, empty url, or bad headers.
        assert_eq!(
            db.add_webhook(&WebhookEndpoint::new(" ", "https://a.example/h"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            db.add_webhook(&WebhookEndpoint::new("bad-url", ""))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        // Delete endpoints.
        assert!(db.delete_webhook(&added_slack.id).unwrap());
        assert!(!db.delete_webhook(&added_slack.id).unwrap());
        assert_eq!(db.list_webhooks().unwrap().len(), 1);
        assert!(db.delete_webhook("ep-pager").unwrap());
        assert!(db.list_webhooks().unwrap().is_empty());
    }

    #[test]
    fn legacy_webhook_config_migrates_once_into_webhook_endpoints() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("legacy.db");

        // Seed a legacy-only database with webhook_config row id=1.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE webhook_config (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    enabled INTEGER NOT NULL DEFAULT 0,
                    url TEXT NOT NULL DEFAULT '',
                    headers_json TEXT NOT NULL DEFAULT '{}',
                    timeout_secs INTEGER NOT NULL DEFAULT 10,
                    updated_at_secs INTEGER NOT NULL DEFAULT 0
                );
                INSERT INTO webhook_config
                    (id, enabled, url, headers_json, timeout_secs, updated_at_secs)
                VALUES
                    (1, 1, 'https://legacy.example/hook', '{\"X-Old\":\"1\"}', 12, 500);",
            )
            .unwrap();
        }

        let db = StorageDb::open(&db_path).unwrap();
        let endpoints = db.list_webhooks().unwrap();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].id, DEFAULT_WEBHOOK_ID);
        assert_eq!(endpoints[0].name, DEFAULT_WEBHOOK_NAME);
        assert!(endpoints[0].enabled);
        assert_eq!(endpoints[0].url, "https://legacy.example/hook");
        assert_eq!(endpoints[0].headers_json, r#"{"X-Old":"1"}"#);
        assert_eq!(endpoints[0].timeout_secs, 12);
        assert_eq!(
            db.load_webhook_config().unwrap().url,
            "https://legacy.example/hook"
        );

        // Adding a second endpoint and deleting the migrated default must NOT
        // re-migrate on reopen.
        let second = db
            .add_webhook(&WebhookEndpoint::new(
                "secondary",
                "https://second.example/hook",
            ))
            .unwrap();
        assert!(db.delete_webhook(DEFAULT_WEBHOOK_ID).unwrap());
        drop(db);

        let reopened = StorageDb::open(&db_path).unwrap();
        let after = reopened.list_webhooks().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, second.id);

        // Deleting the last endpoint clears legacy row so reopen stays empty.
        assert!(reopened.delete_webhook(&second.id).unwrap());
        drop(reopened);
        let reopened2 = StorageDb::open(&db_path).unwrap();
        assert!(reopened2.list_webhooks().unwrap().is_empty());
    }

    #[test]
    fn ai_state_prune_confirmed_paths_updates_and_clears_when_empty() {
        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();

        let p1 = PathBuf::from("/tmp/cache/one");
        let p2 = PathBuf::from("/tmp/cache/two");
        let proposal = CleanupProposal {
            timestamp_secs: 1_700_000_000,
            summary: "Two cache dirs".to_owned(),
            items: vec![
                ProposalItem::new(
                    p1.clone(),
                    100,
                    RemovalMode::Trash,
                    None,
                    None,
                ),
                ProposalItem::new(
                    p2.clone(),
                    200,
                    RemovalMode::Trash,
                    None,
                    None,
                ),
            ],
        };
        let state = AiState::from_proposal(&proposal, 1_700_000_000).unwrap();
        db.save_ai_state(&state).unwrap();

        let rem1 = db
            .prune_ai_state_paths(std::slice::from_ref(&p1), 1_700_000_100)
            .unwrap()
            .expect("one item should remain");
        assert_eq!(rem1.items.len(), 1);
        assert_eq!(rem1.items[0].path, p2);
        let reloaded = db.load_ai_state().unwrap();
        assert_eq!(reloaded.updated_at_secs, 1_700_000_100);
        assert_eq!(reloaded.parse_proposal().unwrap().unwrap().items.len(), 1);

        let rem2 = db
            .prune_ai_state_paths(std::slice::from_ref(&p2), 1_700_000_200)
            .unwrap();
        assert_eq!(rem2, None);
        let cleared = db.load_ai_state().unwrap();
        assert_eq!(cleared.last_proposal_json, None);
        assert_eq!(cleared.updated_at_secs, 1_700_000_200);
    }
}
