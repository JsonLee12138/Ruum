//! Lazy keychain-backed AEAD credential storage adapted from `VaultMesh`.
//!
//! Only a single 32-byte root secret is ever stored in the OS keychain
//! (`macOS` Keychain, `Windows` Credential Manager, `Linux` Secret Service).
//! Per-profile SSH secrets are stored locally in
//! `<config_dir>/ruum/device-credentials.enc`, encrypted with `AES-256-GCM`
//! using an `HMAC-SHA256` subkey and OS user-bound AAD.
//!
//! Unlike eager stores, [`LazyCredentialStore`] never touches the OS keychain
//! on construction, and both `get` and `delete` short-circuit with zero
//! keychain calls whenever `device-credentials.enc` does not exist yet.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::storage::StorageDb;

/// OS keychain service identifier for Ruum's root encryption secret.
pub const ROOT_KEYRING_SERVICE: &str = "ruum.desktop";

/// OS keychain account identifier for Ruum's root encryption secret.
pub const ROOT_KEYRING_ACCOUNT: &str = "root-secret-v1";

/// Encrypted credential bundle filename under Ruum's configuration directory.
pub const CREDENTIALS_FILE_NAME: &str = "device-credentials.enc";

/// Subdirectory name under the OS configuration/support root.
pub const APP_DATA_DIR_NAME: &str = "ruum";

/// Credential namespace used for saved SSH profile secrets.
pub const SSH_CREDENTIAL_SERVICE: &str = "ruum.ssh";

/// Credential namespace used for direct AI API secrets.
pub const AI_CREDENTIAL_SERVICE: &str = "ruum.ai";

/// Account identifier for the stored direct AI API key.
pub const AI_DIRECT_API_KEY_ACCOUNT: &str = "direct-api-key";

const MAGIC: &[u8; 4] = b"RMDC";
const VERSION_EMPTY_AAD: u8 = 1;
const VERSION_BOUND_AAD: u8 = 2;
const KDF_INFO: &[u8] = b"ruum:device-credentials:v1";
const HEADER_PREFIX_BYTES: usize = 4 + 1 + 12;

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Operating system family for root secret storage and directory resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootSecretOs {
    MacOs,
    Linux,
    Windows,
}

/// Captured OS environment inputs for root secret storage and path lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootSecretEnv {
    pub os: RootSecretOs,
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub username: Option<String>,
    pub uid: Option<u32>,
}

impl RootSecretEnv {
    /// Capture the current process environment without reading the keychain.
    #[must_use]
    pub fn current() -> Self {
        #[cfg(unix)]
        let uid = Some(rustix::process::geteuid().as_raw());
        #[cfg(not(unix))]
        let uid = None;
        Self {
            os: RootSecretFactory::current_os(),
            home: std::env::home_dir(),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from),
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
            username: std::env::var("USERNAME").ok(),
            uid,
        }
    }

    /// Resolve `<config_dir>/ruum` across macOS, Windows, and Linux.
    #[must_use]
    pub fn app_data_dir(&self) -> Option<PathBuf> {
        match self.os {
            RootSecretOs::MacOs => self.home.as_ref().map(|h| {
                h.join("Library")
                    .join("Application Support")
                    .join(APP_DATA_DIR_NAME)
            }),
            RootSecretOs::Windows => self
                .appdata
                .as_ref()
                .filter(|p| !p.as_os_str().is_empty())
                .cloned()
                .or_else(|| {
                    self.home
                        .as_ref()
                        .map(|h| h.join("AppData").join("Roaming"))
                })
                .map(|base| base.join(APP_DATA_DIR_NAME)),
            RootSecretOs::Linux => self
                .xdg_config_home
                .as_ref()
                .filter(|p| {
                    !p.as_os_str().is_empty()
                        && (p.is_absolute()
                            || p.to_string_lossy().starts_with('/'))
                })
                .cloned()
                .or_else(|| self.home.as_ref().map(|h| h.join(".config")))
                .map(|base| base.join(APP_DATA_DIR_NAME)),
        }
    }
}

/// Injectable root secret provider for production keychains and unit tests.
pub trait RootSecretStore: Send + Sync {
    fn get_or_create_root_secret(&self) -> Result<Zeroizing<[u8; 32]>, String>;
}

/// Platform-specific root secret adapter contract.
pub trait RootSecretAdapter: RootSecretStore {
    fn os(&self) -> RootSecretOs;
}

fn keyring_get_or_create_root_secret(
    backend_label: &str,
) -> Result<Zeroizing<[u8; 32]>, String> {
    let entry = keyring::Entry::new(ROOT_KEYRING_SERVICE, ROOT_KEYRING_ACCOUNT)
        .map_err(|e| format!("{backend_label} 创建钥匙串入口失败: {e}"))?;
    match entry.get_secret() {
        Ok(bytes) => Ok(normalize_root_secret(&bytes)),
        Err(keyring::Error::NoEntry) => {
            let mut secret = Zeroizing::new([0_u8; 32]);
            OsRng.fill_bytes(secret.as_mut());
            entry
                .set_secret(secret.as_ref())
                .map_err(|e| format!("{backend_label} 保存根密钥失败: {e}"))?;
            Ok(secret)
        }
        Err(e) => Err(format!("{backend_label} 钥匙串错误: {e}")),
    }
}

/// macOS Keychain root secret adapter (`apple-native`).
#[derive(Clone, Copy, Debug, Default)]
pub struct MacOsRootSecretAdapter;

impl RootSecretStore for MacOsRootSecretAdapter {
    fn get_or_create_root_secret(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        keyring_get_or_create_root_secret("macOS")
    }
}

impl RootSecretAdapter for MacOsRootSecretAdapter {
    fn os(&self) -> RootSecretOs {
        RootSecretOs::MacOs
    }
}

/// Linux Secret Service root secret adapter (`async-secret-service` + `zbus`).
#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxRootSecretAdapter;

impl RootSecretStore for LinuxRootSecretAdapter {
    fn get_or_create_root_secret(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        keyring_get_or_create_root_secret("Linux")
    }
}

impl RootSecretAdapter for LinuxRootSecretAdapter {
    fn os(&self) -> RootSecretOs {
        RootSecretOs::Linux
    }
}

/// Windows Credential Manager root secret adapter (`windows-native`).
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsRootSecretAdapter;

impl RootSecretStore for WindowsRootSecretAdapter {
    fn get_or_create_root_secret(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        keyring_get_or_create_root_secret("Windows")
    }
}

impl RootSecretAdapter for WindowsRootSecretAdapter {
    fn os(&self) -> RootSecretOs {
        RootSecretOs::Windows
    }
}

/// Factory creating OS-specific root secret adapters.
#[derive(Clone, Copy, Debug, Default)]
pub struct RootSecretFactory;

impl RootSecretFactory {
    /// Detect the compile-time target OS.
    #[must_use]
    pub const fn current_os() -> RootSecretOs {
        if cfg!(target_os = "macos") {
            RootSecretOs::MacOs
        } else if cfg!(windows) {
            RootSecretOs::Windows
        } else {
            RootSecretOs::Linux
        }
    }

    /// Construct the stateless adapter for a specific OS.
    #[must_use]
    pub fn create_for(os: RootSecretOs) -> Box<dyn RootSecretAdapter> {
        match os {
            RootSecretOs::MacOs => Box::new(MacOsRootSecretAdapter),
            RootSecretOs::Linux => Box::new(LinuxRootSecretAdapter),
            RootSecretOs::Windows => Box::new(WindowsRootSecretAdapter),
        }
    }

    /// Construct the stateless adapter for the host OS.
    #[must_use]
    pub fn create() -> Box<dyn RootSecretAdapter> {
        Self::create_for(Self::current_os())
    }

    /// Construct a shared [`RootSecretStore`] handle for the host OS.
    #[must_use]
    pub fn create_store() -> Arc<dyn RootSecretStore> {
        if let Ok(hex_val) = std::env::var("RUUM_TEST_ROOT_SECRET_HEX")
            && !hex_val.trim().is_empty()
        {
            return Arc::new(EnvHexRootSecretStore {
                raw_hex: Zeroizing::new(hex_val),
            });
        }
        match Self::current_os() {
            RootSecretOs::MacOs => Arc::new(MacOsRootSecretAdapter),
            RootSecretOs::Linux => Arc::new(LinuxRootSecretAdapter),
            RootSecretOs::Windows => Arc::new(WindowsRootSecretAdapter),
        }
    }
}

struct EnvHexRootSecretStore {
    raw_hex: Zeroizing<String>,
}

impl RootSecretStore for EnvHexRootSecretStore {
    fn get_or_create_root_secret(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        let trimmed = self.raw_hex.trim();
        let bytes = trimmed.as_bytes();
        if bytes.len() != 64 {
            return Err("RUUM_TEST_ROOT_SECRET_HEX must be 64 hex characters"
                .to_owned());
        }
        let mut out = Zeroizing::new([0_u8; 32]);
        for (idx, chunk) in bytes.chunks_exact(2).enumerate() {
            let pair = std::str::from_utf8(chunk)
                .map_err(|_| "invalid UTF-8 in RUUM_TEST_ROOT_SECRET_HEX")?;
            out[idx] = u8::from_str_radix(pair, 16)
                .map_err(|_| "invalid hex in RUUM_TEST_ROOT_SECRET_HEX")?;
        }
        Ok(out)
    }
}

/// Decrypted SSH credential secret stored per profile UUID in
/// `device-credentials.enc`.
#[derive(
    Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop,
)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SshCredentialSecret {
    Password {
        password: String,
    },
    PrivateKey {
        private_key_pem: String,
        passphrase: Option<String>,
    },
}

impl fmt::Debug for SshCredentialSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password { .. } => f
                .debug_struct("SshCredentialSecret::Password")
                .field("password", &"[REDACTED]")
                .finish(),
            Self::PrivateKey { passphrase, .. } => f
                .debug_struct("SshCredentialSecret::PrivateKey")
                .field("private_key_pem", &"[REDACTED]")
                .field("has_passphrase", &passphrase.is_some())
                .finish(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct StoredPayload {
    version: u32,
    entries: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiskFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    header_prefix_len: usize,
    header_prefix: [u8; HEADER_PREFIX_BYTES],
}

fn read_disk_fingerprint(file_path: &Path) -> Option<DiskFingerprint> {
    let mut file = File::open(file_path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut header_prefix = [0_u8; HEADER_PREFIX_BYTES];
    let mut filled = 0;
    while filled < HEADER_PREFIX_BYTES {
        match file.read(&mut header_prefix[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(DiskFingerprint {
        len: meta.len(),
        modified: meta.modified().ok(),
        header_prefix_len: filled,
        header_prefix,
    })
}

struct LoadedState {
    root_secret: Zeroizing<[u8; 32]>,
    entries: HashMap<String, String>,
    fingerprint: Option<DiskFingerprint>,
}

struct LazyCredentialStoreInner {
    file_path: PathBuf,
    root_store: Option<Arc<dyn RootSecretStore>>,
    loaded: Option<LoadedState>,
    unavailable: Option<String>,
}

/// On-demand AEAD credential store backed by a single OS keychain root secret.
///
/// Construction performs zero keychain operations. Keychain access occurs only
/// when `set` is called, or when `get`/`delete` is called while
/// `device-credentials.enc` already exists on disk.
#[derive(Clone)]
pub struct LazyCredentialStore {
    inner: Arc<Mutex<LazyCredentialStoreInner>>,
}

impl fmt::Debug for LazyCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (file_path, is_loaded, unavailable) = {
            let guard = self.inner.lock().ok();
            (
                guard.as_ref().map(|g| g.file_path.clone()),
                guard.as_ref().is_some_and(|g| g.loaded.is_some()),
                guard.as_ref().and_then(|g| g.unavailable.clone()),
            )
        };
        f.debug_struct("LazyCredentialStore")
            .field("file_path", &file_path)
            .field("is_loaded", &is_loaded)
            .field("unavailable", &unavailable)
            .finish()
    }
}

fn normalize_root_secret(bytes: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut secret = Zeroizing::new([0_u8; 32]);
    if bytes.len() == 32 {
        secret.copy_from_slice(bytes);
    } else {
        let hash = Sha256::digest(bytes);
        secret.copy_from_slice(&hash);
    }
    secret
}

fn current_os_user_binding() -> String {
    #[cfg(unix)]
    {
        format!("uid:{}", rustix::process::geteuid().as_raw())
    }
    #[cfg(windows)]
    {
        std::env::var("USERNAME").unwrap_or_else(|_| "unknown".to_owned())
    }
    #[cfg(not(any(unix, windows)))]
    {
        "unknown".to_owned()
    }
}

fn credential_aad() -> Vec<u8> {
    let user = current_os_user_binding();
    format!("app={APP_DATA_DIR_NAME}\nuser={user}\nproto={VERSION_BOUND_AAD}")
        .into_bytes()
}

/// Resolve the default per-user Ruum data directory via [`RootSecretEnv`].
#[must_use]
pub fn default_desktop_app_data_dir() -> Option<PathBuf> {
    RootSecretEnv::current().app_data_dir()
}

fn derive_encryption_key(root_secret: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(root_secret)
        .expect("HMAC accepts 32-byte key");
    mac.update(KDF_INFO);
    Zeroizing::new(mac.finalize().into_bytes().into())
}

fn entry_key(service: &str, account: &str) -> String {
    format!("{service}::{account}")
}

struct LoadedCredentials {
    entries: HashMap<String, String>,
    reseal: bool,
}

fn save_encrypted_entries(
    file_path: &Path,
    root_secret: &[u8; 32],
    entries: &HashMap<String, String>,
) -> Result<(), String> {
    let payload = StoredPayload {
        version: 1,
        entries: entries.clone(),
    };
    let plaintext = Zeroizing::new(
        serde_json::to_vec(&payload)
            .map_err(|e| format!("序列化凭据存储失败: {e}"))?,
    );

    let key = derive_encryption_key(root_secret);
    let cipher = Aes256Gcm::new_from_slice(key.as_ref())
        .map_err(|e| format!("AES 密码初始化失败: {e}"))?;

    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let aad = credential_aad();
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext.as_slice(),
                aad: &aad,
            },
        )
        .map_err(|e| format!("凭据加密失败: {e}"))?;

    let mut file_bytes = Vec::with_capacity(
        MAGIC.len() + 1 + nonce_bytes.len() + ciphertext.len(),
    );
    file_bytes.extend_from_slice(MAGIC);
    file_bytes.push(VERSION_BOUND_AAD);
    file_bytes.extend_from_slice(&nonce_bytes);
    file_bytes.extend_from_slice(&ciphertext);

    if let Some(parent) = file_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|e| format!("创建凭据目录失败: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let base_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(CREDENTIALS_FILE_NAME);
    let tmp_path =
        file_path.with_file_name(format!("{base_name}.{pid}.{seq}.tmp"));
    let write_res = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
            .map_err(|e| format!("创建临时凭据文件失败: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
        file.write_all(&file_bytes)
            .map_err(|e| format!("写入临时凭据文件失败: {e}"))?;
        file.flush().map_err(|e| format!("刷新凭据文件失败: {e}"))?;
        drop(file);
        fs::rename(&tmp_path, file_path)
            .map_err(|e| format!("原子更新凭据文件失败: {e}"))
    })();
    if write_res.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_res
}

fn load_encrypted_entries(
    file_path: &Path,
    root_secret: &[u8; 32],
) -> Result<LoadedCredentials, String> {
    if !file_path.is_file() {
        return Ok(LoadedCredentials {
            entries: HashMap::new(),
            reseal: false,
        });
    }

    let file_bytes =
        fs::read(file_path).map_err(|e| format!("读取凭据文件失败: {e}"))?;
    if file_bytes.len() < MAGIC.len() + 1 + 12 {
        return Err("凭据文件损坏: 长度过短".to_owned());
    }

    if &file_bytes[..MAGIC.len()] != MAGIC {
        return Err("凭据文件标识错误".to_owned());
    }

    let version = file_bytes[MAGIC.len()];
    let nonce_start = MAGIC.len() + 1;
    let nonce_end = nonce_start + 12;
    let nonce = Nonce::from_slice(&file_bytes[nonce_start..nonce_end]);
    let ciphertext = &file_bytes[nonce_end..];
    let key = derive_encryption_key(root_secret);
    let cipher = Aes256Gcm::new_from_slice(key.as_ref())
        .map_err(|e| format!("AES 密码初始化失败: {e}"))?;
    let aad = credential_aad();
    let (plaintext, reseal) = match version {
        VERSION_BOUND_AAD => (
            Zeroizing::new(
                cipher
                    .decrypt(
                        nonce,
                        Payload {
                            msg: ciphertext,
                            aad: &aad,
                        },
                    )
                    .map_err(|e| format!("凭据解密失败: {e}"))?,
            ),
            false,
        ),
        VERSION_EMPTY_AAD => (
            Zeroizing::new(
                cipher
                    .decrypt(nonce, ciphertext)
                    .map_err(|e| format!("凭据解密失败: {e}"))?,
            ),
            true,
        ),
        _ => return Err(format!("不支持的凭据版本: {version}")),
    };

    let payload: StoredPayload = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("反序列化凭据内容失败: {e}"))?;

    Ok(LoadedCredentials {
        entries: payload.entries,
        reseal,
    })
}

impl LazyCredentialStore {
    fn sync_loaded_with_disk(
        inner: &mut LazyCredentialStoreInner,
    ) -> Result<(), String> {
        let disk_fp = read_disk_fingerprint(&inner.file_path);
        if inner
            .loaded
            .as_ref()
            .is_some_and(|s| s.fingerprint.is_some())
            && disk_fp.is_none()
        {
            inner.loaded = None;
            return Ok(());
        }
        let file_path = inner.file_path.clone();
        if let Some(state) = inner.loaded.as_mut()
            && disk_fp != state.fingerprint
        {
            let loaded =
                load_encrypted_entries(&file_path, &state.root_secret)?;
            if loaded.reseal {
                save_encrypted_entries(
                    &file_path,
                    &state.root_secret,
                    &loaded.entries,
                )?;
            }
            state.entries = loaded.entries;
            state.fingerprint = read_disk_fingerprint(&file_path);
        }
        Ok(())
    }

    fn load_into_inner(
        inner: &mut LazyCredentialStoreInner,
        root_secret: Zeroizing<[u8; 32]>,
    ) -> Result<(), String> {
        let loaded = load_encrypted_entries(&inner.file_path, &root_secret)?;
        if loaded.reseal {
            save_encrypted_entries(
                &inner.file_path,
                &root_secret,
                &loaded.entries,
            )?;
        }
        let fingerprint = read_disk_fingerprint(&inner.file_path);
        inner.loaded = Some(LoadedState {
            root_secret,
            entries: loaded.entries,
            fingerprint,
        });
        Ok(())
    }

    fn with_loaded_state<T>(
        &self,
        require_existing_file: bool,
        op: impl FnOnce(&Path, &mut LoadedState) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        let mut inner =
            self.inner.lock().map_err(|_| "凭据互斥锁中毒".to_owned())?;
        if let Some(reason) = &inner.unavailable {
            return Err(reason.clone());
        }
        Self::sync_loaded_with_disk(&mut inner)?;
        if inner.loaded.is_none() {
            if require_existing_file && !inner.file_path.is_file() {
                return Ok(None);
            }
            let root_store = inner
                .root_store
                .clone()
                .ok_or_else(|| "根钥匙串适配器未配置".to_owned())?;
            drop(inner);

            // Release the store mutex while calling the OS keychain so
            // background preload never blocks UI interactions, and keep
            // `unavailable` unset on transient errors or cancellations.
            let root_secret = root_store.get_or_create_root_secret()?;

            inner =
                self.inner.lock().map_err(|_| "凭据互斥锁中毒".to_owned())?;
            if let Some(reason) = &inner.unavailable {
                return Err(reason.clone());
            }
            Self::sync_loaded_with_disk(&mut inner)?;
            if inner.loaded.is_none() {
                if require_existing_file && !inner.file_path.is_file() {
                    return Ok(None);
                }
                Self::load_into_inner(&mut inner, root_secret)?;
            }
        }
        let file_path = inner.file_path.clone();
        let state = inner
            .loaded
            .as_mut()
            .ok_or_else(|| "凭据状态初始化失败".to_owned())?;
        let outcome = op(&file_path, state);
        drop(inner);
        outcome.map(Some)
    }

    /// Construct a lazy store using the host OS keychain adapter without
    /// touching the keychain during construction.
    #[must_use]
    pub fn platform(app_data_dir: &Path) -> Self {
        let file_path = app_data_dir.join(CREDENTIALS_FILE_NAME);
        Self::new(file_path, RootSecretFactory::create_store())
    }

    /// Open the default `<config_dir>/ruum/device-credentials.enc` lazy store.
    pub fn open_default() -> Result<Self, String> {
        let dir = default_desktop_app_data_dir()
            .ok_or_else(|| "无法解析 ruum 配置目录".to_owned())?;
        Ok(Self::platform(&dir))
    }

    /// Construct a lazy store with explicit file path and injectable root store.
    ///
    /// Performs zero keychain calls and zero disk reads on construction.
    #[must_use]
    pub fn new(
        file_path: PathBuf,
        root_store: Arc<dyn RootSecretStore>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LazyCredentialStoreInner {
                file_path,
                root_store: Some(root_store),
                loaded: None,
                unavailable: None,
            })),
        }
    }

    /// Construct a fail-closed store that rejects all reads and writes.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LazyCredentialStoreInner {
                file_path: PathBuf::new(),
                root_store: None,
                loaded: None,
                unavailable: Some(reason.into()),
            })),
        }
    }

    /// Return the unavailability reason if constructed via [`Self::unavailable`].
    #[must_use]
    pub fn unavailable_reason(&self) -> Option<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.unavailable.clone())
    }

    /// Path to the underlying `device-credentials.enc` file.
    #[must_use]
    pub fn file_path(&self) -> PathBuf {
        self.inner
            .lock()
            .map(|inner| inner.file_path.clone())
            .unwrap_or_default()
    }

    /// Return whether `device-credentials.enc` currently exists on disk without
    /// touching the OS keychain.
    #[must_use]
    pub fn has_encrypted_file(&self) -> bool {
        let Ok(inner) = self.inner.lock() else {
            return false;
        };
        if inner.unavailable.is_some() {
            return false;
        }
        let path = inner.file_path.clone();
        drop(inner);
        !path.as_os_str().is_empty() && path.is_file()
    }

    /// Return whether the root secret and encrypted entries are currently
    /// cached in memory.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let file_exists = inner.file_path.is_file();
        if inner
            .loaded
            .as_ref()
            .is_some_and(|s| s.fingerprint.is_some() && !file_exists)
        {
            inner.loaded = None;
        }
        inner.loaded.is_some()
    }

    /// Pre-authorize and cache the root secret in memory when
    /// `device-credentials.enc` exists and is not yet loaded.
    ///
    /// Returns `Ok(false)` with zero keychain calls when already loaded or when
    /// no encrypted file exists on disk.
    pub fn preload(&self) -> Result<bool, String> {
        let mut inner =
            self.inner.lock().map_err(|_| "凭据互斥锁中毒".to_owned())?;
        if let Some(reason) = &inner.unavailable {
            return Err(reason.clone());
        }
        Self::sync_loaded_with_disk(&mut inner)?;
        if inner.loaded.is_some() || !inner.file_path.is_file() {
            return Ok(false);
        }
        let root_store = inner
            .root_store
            .clone()
            .ok_or_else(|| "根钥匙串适配器未配置".to_owned())?;
        drop(inner);

        let root_secret = root_store.get_or_create_root_secret()?;

        let mut inner =
            self.inner.lock().map_err(|_| "凭据互斥锁中毒".to_owned())?;
        if let Some(reason) = &inner.unavailable {
            return Err(reason.clone());
        }
        Self::sync_loaded_with_disk(&mut inner)?;
        if inner.loaded.is_some() || !inner.file_path.is_file() {
            return Ok(false);
        }
        Self::load_into_inner(&mut inner, root_secret)?;
        drop(inner);
        Ok(true)
    }

    /// Read a secret by `(service, account)`.
    ///
    /// Short-circuits to `Ok(None)` with zero keychain calls when the store
    /// has not been loaded yet and `device-credentials.enc` does not exist.
    pub fn get(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        let key = entry_key(service, account);
        let encoded = self
            .with_loaded_state(true, |_, state| {
                Ok(state.entries.get(&key).cloned())
            })?
            .flatten();
        let Some(encoded) = encoded else {
            return Ok(None);
        };
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|e| format!("Base64 解码凭据失败: {e}"))?;
        Ok(Some(Zeroizing::new(bytes)))
    }

    /// Convenience helper returning an error on missing or unavailable secret.
    pub fn read_unified_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Zeroizing<Vec<u8>>, String> {
        self.get(service, account)?
            .ok_or_else(|| "凭据不存在".to_owned())
    }

    /// Store or overwrite a secret, lazily opening/creating the root secret.
    pub fn set(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), String> {
        let key = entry_key(service, account);
        let encoded = STANDARD.encode(secret);
        self.with_loaded_state(false, |file_path, state| {
            let mut next_entries = state.entries.clone();
            next_entries.insert(key, encoded);
            save_encrypted_entries(
                file_path,
                &state.root_secret,
                &next_entries,
            )?;
            state.entries = next_entries;
            state.fingerprint = read_disk_fingerprint(file_path);
            Ok(())
        })?
        .ok_or_else(|| "凭据状态初始化失败".to_owned())?;
        Ok(())
    }

    /// Delete a secret by `(service, account)`.
    ///
    /// Short-circuits to `Ok(())` with zero keychain calls when the store
    /// has not been loaded yet and `device-credentials.enc` does not exist.
    pub fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        let key = entry_key(service, account);
        let _ = self.with_loaded_state(true, |file_path, state| {
            if state.entries.contains_key(&key) {
                let mut next_entries = state.entries.clone();
                next_entries.remove(&key);
                save_encrypted_entries(
                    file_path,
                    &state.root_secret,
                    &next_entries,
                )?;
                state.entries = next_entries;
                state.fingerprint = read_disk_fingerprint(file_path);
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Return a namespaced view bound to `service`.
    #[must_use]
    pub fn scoped(&self, service: &'static str) -> ScopedCredentialStore {
        ScopedCredentialStore {
            store: self.clone(),
            service,
        }
    }

    /// Persist a structured SSH secret for `profile_id`.
    pub fn set_ssh_secret(
        &self,
        profile_id: &str,
        secret: &SshCredentialSecret,
    ) -> Result<(), String> {
        let encoded = Zeroizing::new(
            serde_json::to_vec(secret)
                .map_err(|e| format!("序列化 SSH 凭据失败: {e}"))?,
        );
        self.set(
            SSH_CREDENTIAL_SERVICE,
            profile_id.trim(),
            encoded.as_slice(),
        )
    }

    /// Load a structured SSH secret for `profile_id` on demand.
    pub fn get_ssh_secret(
        &self,
        profile_id: &str,
    ) -> Result<Option<SshCredentialSecret>, String> {
        let Some(raw) = self.get(SSH_CREDENTIAL_SERVICE, profile_id.trim())?
        else {
            return Ok(None);
        };
        let secret: SshCredentialSecret = serde_json::from_slice(&raw)
            .map_err(|e| format!("解析 SSH 凭据失败: {e}"))?;
        Ok(Some(secret))
    }

    /// Remove the stored SSH secret for `profile_id`.
    pub fn delete_ssh_secret(&self, profile_id: &str) -> Result<(), String> {
        self.delete(SSH_CREDENTIAL_SERVICE, profile_id.trim())
    }

    /// Store or overwrite the direct AI API key in `ruum.ai`.
    pub fn set_ai_api_key(&self, api_key: &str) -> Result<(), String> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            return Err("AI API key cannot be empty".to_owned());
        }
        let secret = Zeroizing::new(trimmed.as_bytes().to_vec());
        self.set(
            AI_CREDENTIAL_SERVICE,
            AI_DIRECT_API_KEY_ACCOUNT,
            secret.as_slice(),
        )
    }

    /// Load the direct AI API key on demand (zero keychain calls if absent).
    pub fn get_ai_api_key(&self) -> Result<Option<Zeroizing<String>>, String> {
        let Some(raw) =
            self.get(AI_CREDENTIAL_SERVICE, AI_DIRECT_API_KEY_ACCOUNT)?
        else {
            return Ok(None);
        };
        let text = Zeroizing::new(
            String::from_utf8(raw.to_vec())
                .map_err(|e| format!("解码 AI API 密钥失败: {e}"))?,
        );
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(Zeroizing::new(trimmed.to_owned())))
    }

    /// Remove the stored direct AI API key if present.
    pub fn delete_ai_api_key(&self) -> Result<(), String> {
        self.delete(AI_CREDENTIAL_SERVICE, AI_DIRECT_API_KEY_ACCOUNT)
    }

    /// Return whether a non-empty direct AI API key is stored.
    pub fn has_ai_api_key(&self) -> Result<bool, String> {
        Ok(self.get_ai_api_key()?.is_some())
    }
}

/// Namespaced view into a [`LazyCredentialStore`].
#[derive(Clone, Debug)]
pub struct ScopedCredentialStore {
    store: LazyCredentialStore,
    service: &'static str,
}

impl ScopedCredentialStore {
    pub fn get(
        &self,
        account: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        self.store.get(self.service, account)
    }

    pub fn set(&self, account: &str, secret: &[u8]) -> Result<(), String> {
        self.store.set(self.service, account, secret)
    }

    pub fn delete(&self, account: &str) -> Result<(), String> {
        self.store.delete(self.service, account)
    }
}

/// Delete an SSH profile card atomically by removing its keychain-encrypted
/// credentials before deleting its `SQLite` metadata row.
///
/// Removing credentials first guarantees no orphaned secret remains if the
/// credential store fails (e.g. locked keychain), keeping DB metadata intact
/// for retry. When no credential file exists yet, `delete_ssh_secret`
/// short-circuits with zero keychain calls.
pub fn delete_ssh_profile_with_credentials(
    db: &StorageDb,
    creds: &LazyCredentialStore,
    id_or_alias: &str,
) -> io::Result<bool> {
    let Some(profile) = db.get_ssh_profile(id_or_alias)? else {
        return Ok(false);
    };
    creds
        .delete_ssh_secret(&profile.id)
        .map_err(io::Error::other)?;
    db.delete_ssh_profile(&profile.id)
}

#[cfg(test)]
pub mod test_helpers {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use uuid::Uuid;

    use super::*;

    #[derive(Debug)]
    pub struct MemoryRootSecretStore {
        pub secret: [u8; 32],
        pub calls: AtomicUsize,
    }

    impl MemoryRootSecretStore {
        #[must_use]
        pub const fn new(secret: [u8; 32]) -> Self {
            Self {
                secret,
                calls: AtomicUsize::new(0),
            }
        }

        #[must_use]
        pub fn call_count(&self) -> usize {
            self.calls.load(Ordering::Acquire)
        }
    }

    impl RootSecretStore for MemoryRootSecretStore {
        fn get_or_create_root_secret(
            &self,
        ) -> Result<Zeroizing<[u8; 32]>, String> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(Zeroizing::new(self.secret))
        }
    }

    #[derive(Debug)]
    pub struct TestDir(pub PathBuf);
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub fn temp_store()
    -> (LazyCredentialStore, Arc<MemoryRootSecretStore>, TestDir) {
        let temp_dir = std::env::temp_dir()
            .join(format!("ruum-cred-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).expect("create test dir");
        let file_path = temp_dir.join(CREDENTIALS_FILE_NAME);
        let root_store = Arc::new(MemoryRootSecretStore::new([42_u8; 32]));
        let store = LazyCredentialStore::new(file_path, root_store.clone());
        (store, root_store, TestDir(temp_dir))
    }
}

#[cfg(test)]
mod tests {
    use aes_gcm::Aes256Gcm;
    use aes_gcm::Nonce;
    use aes_gcm::aead::{Aead, KeyInit};

    use crate::storage::{AuthMode, SshProfile};

    use super::test_helpers::*;
    use super::*;

    #[test]
    fn unavailable_store_fails_closed_without_panicking() {
        let store = LazyCredentialStore::unavailable("根钥匙串不可用 (-128)");
        assert_eq!(
            store.unavailable_reason().as_deref(),
            Some("根钥匙串不可用 (-128)")
        );
        assert!(store.get("svc", "acc").is_err());
        assert!(store.set("svc", "acc", b"x").is_err());
        assert!(store.delete("svc", "acc").is_err());
        assert!(store.scoped("svc").get("acc").is_err());
        assert!(store.read_unified_secret("svc", "acc").is_err());
    }

    #[test]
    fn lazy_store_never_touches_keychain_on_init_or_missing_file_reads() {
        let (store, root_store, temp_dir) = temp_store();
        let file_path = temp_dir.0.join(CREDENTIALS_FILE_NAME);
        assert!(!file_path.exists());
        assert_eq!(root_store.call_count(), 0);

        // Missing file: get and delete short-circuit with zero keychain calls.
        assert_eq!(store.get("ruum.ssh", "missing").unwrap(), None);
        assert_eq!(root_store.call_count(), 0);
        store.delete("ruum.ssh", "missing").unwrap();
        assert_eq!(root_store.call_count(), 0);
        assert!(!file_path.exists());

        // First set wakes the root secret once and seals the bundle.
        store.set("ruum.ssh", "prof-1", b"hunter2").unwrap();
        assert_eq!(root_store.call_count(), 1);
        assert!(file_path.is_file());

        // Subsequent operations reuse the cached root secret without extra calls.
        assert_eq!(
            store.get("ruum.ssh", "prof-1").unwrap().unwrap().as_slice(),
            b"hunter2"
        );
        store.delete("ruum.ssh", "prof-1").unwrap();
        assert_eq!(root_store.call_count(), 1);

        // A new LazyCredentialStore instance opens keychain lazily only once
        // an existing encrypted file is read.
        let root_store2 = Arc::new(MemoryRootSecretStore::new([42_u8; 32]));
        let reloaded = LazyCredentialStore::new(file_path, root_store2.clone());
        assert_eq!(root_store2.call_count(), 0);
        assert_eq!(reloaded.get("ruum.ssh", "prof-1").unwrap(), None);
        assert_eq!(root_store2.call_count(), 1);
    }

    #[test]
    fn test_device_credential_store_crud_and_persistence() {
        let (store, _, temp_dir) = temp_store();
        let file_path = temp_dir.0.join(CREDENTIALS_FILE_NAME);

        assert_eq!(store.get("svc1", "acc1").unwrap(), None);

        store.set("svc1", "acc1", b"secret_value_1").unwrap();
        assert_eq!(
            store.get("svc1", "acc1").unwrap().unwrap().as_slice(),
            b"secret_value_1"
        );
        assert!(file_path.is_file());

        // Reload store from disk.
        let root_store = Arc::new(MemoryRootSecretStore::new([42_u8; 32]));
        let reloaded = LazyCredentialStore::new(file_path.clone(), root_store);
        assert_eq!(
            reloaded.get("svc1", "acc1").unwrap().unwrap().as_slice(),
            b"secret_value_1"
        );

        // Delete.
        reloaded.delete("svc1", "acc1").unwrap();
        assert_eq!(reloaded.get("svc1", "acc1").unwrap(), None);

        // Reload again, should be gone.
        let root_store2 = Arc::new(MemoryRootSecretStore::new([42_u8; 32]));
        let reloaded2 = LazyCredentialStore::new(file_path, root_store2);
        assert_eq!(reloaded2.get("svc1", "acc1").unwrap(), None);
    }

    #[test]
    fn leftover_legacy_migration_marker_does_not_affect_reads_or_writes() {
        let (store, _, temp_dir) = temp_store();
        let file_path = temp_dir.0.join(CREDENTIALS_FILE_NAME);
        store
            .set("ruum.desktop.migration", "legacy-services-v1", b"1")
            .unwrap();
        store
            .set("ruum.desktop.pin", "desktop-pin-1", b"pin")
            .unwrap();

        let reloaded = LazyCredentialStore::new(
            file_path,
            Arc::new(MemoryRootSecretStore::new([42_u8; 32])),
        );
        assert_eq!(
            reloaded
                .get("ruum.desktop.pin", "desktop-pin-1")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"pin"
        );
        assert!(
            reloaded
                .read_unified_secret(
                    "ruum.desktop.browser-pairing",
                    "native-host-hmac-v1"
                )
                .is_err()
        );
        reloaded
            .set(
                "ruum.desktop.browser-pairing",
                "native-host-hmac-v1",
                b"new",
            )
            .unwrap();
        reloaded
            .delete("ruum.desktop.pin", "desktop-pin-1")
            .unwrap();
        assert!(
            reloaded
                .get("ruum.desktop.pin", "desktop-pin-1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn empty_aad_file_is_resealed_with_bound_aad() {
        let (store, _, temp_dir) = temp_store();
        store.set("svc", "acc", b"sealed").unwrap();
        let file_path = temp_dir.0.join(CREDENTIALS_FILE_NAME);
        let bytes = fs::read(&file_path).unwrap();
        assert_eq!(bytes[MAGIC.len()], VERSION_BOUND_AAD);

        let root = [42_u8; 32];
        let key = derive_encryption_key(&root);
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).unwrap();
        let nonce_bytes = [7_u8; 12];
        let payload = serde_json::to_vec(&StoredPayload {
            version: 1,
            entries: HashMap::from([(
                entry_key("ruum.ssh", "legacy-account"),
                STANDARD.encode(b"legacy-file"),
            )]),
        })
        .unwrap();
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), payload.as_slice())
            .unwrap();
        let mut legacy_file = Vec::new();
        legacy_file.extend_from_slice(MAGIC);
        legacy_file.push(VERSION_EMPTY_AAD);
        legacy_file.extend_from_slice(&nonce_bytes);
        legacy_file.extend_from_slice(&ciphertext);
        fs::write(&file_path, &legacy_file).unwrap();

        let reloaded = LazyCredentialStore::new(
            file_path.clone(),
            Arc::new(MemoryRootSecretStore::new(root)),
        );
        assert_eq!(
            reloaded
                .get("ruum.ssh", "legacy-account")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"legacy-file"
        );
        assert_eq!(
            fs::read(&file_path).unwrap()[MAGIC.len()],
            VERSION_BOUND_AAD
        );
    }

    #[test]
    fn test_scoped_credential_store() {
        let (store, _, _) = temp_store();
        let scoped = store.scoped("ruum.ssh");

        scoped.set("user_credential", b"key_data").unwrap();
        assert_eq!(
            scoped.get("user_credential").unwrap().unwrap().as_slice(),
            b"key_data"
        );
        assert_eq!(
            store
                .get("ruum.ssh", "user_credential")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"key_data"
        );

        scoped.delete("user_credential").unwrap();
        assert_eq!(scoped.get("user_credential").unwrap(), None);
    }

    #[test]
    fn root_secret_factory_covers_all_three_platforms_and_paths() {
        for os in [
            RootSecretOs::MacOs,
            RootSecretOs::Linux,
            RootSecretOs::Windows,
        ] {
            let adapter = RootSecretFactory::create_for(os);
            assert_eq!(adapter.os(), os);
        }

        let mac_env = RootSecretEnv {
            os: RootSecretOs::MacOs,
            home: Some(PathBuf::from("/Users/example")),
            xdg_config_home: None,
            appdata: None,
            username: None,
            uid: Some(501),
        };
        assert_eq!(
            mac_env.app_data_dir(),
            Some(PathBuf::from(
                "/Users/example/Library/Application Support/ruum"
            ))
        );

        let win_env = RootSecretEnv {
            os: RootSecretOs::Windows,
            home: Some(PathBuf::from("C:/Users/example")),
            xdg_config_home: None,
            appdata: Some(PathBuf::from("C:/Users/example/AppData/Roaming")),
            username: Some("example".to_owned()),
            uid: None,
        };
        assert_eq!(
            win_env.app_data_dir(),
            Some(PathBuf::from("C:/Users/example/AppData/Roaming/ruum"))
        );

        let linux_env = RootSecretEnv {
            os: RootSecretOs::Linux,
            home: Some(PathBuf::from("/home/example")),
            xdg_config_home: Some(PathBuf::from("/home/example/.xdg")),
            appdata: None,
            username: None,
            uid: Some(1000),
        };
        assert_eq!(
            linux_env.app_data_dir(),
            Some(PathBuf::from("/home/example/.xdg/ruum"))
        );
    }

    #[test]
    fn ssh_secret_roundtrip_and_atomic_delete_with_storage_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = StorageDb::open(dir.path().join("ruum.db")).unwrap();
        let root_store = Arc::new(MemoryRootSecretStore::new([9_u8; 32]));
        let creds = LazyCredentialStore::new(
            dir.path().join(CREDENTIALS_FILE_NAME),
            root_store,
        );

        let profile = db
            .add_ssh_profile(&SshProfile::new(
                "prod-web",
                "10.0.0.10",
                22,
                "ubuntu",
                AuthMode::Password,
            ))
            .unwrap();

        let secret = SshCredentialSecret::Password {
            password: "s3cr3t-password".to_owned(),
        };
        creds.set_ssh_secret(&profile.id, &secret).unwrap();
        assert_eq!(creds.get_ssh_secret(&profile.id).unwrap(), Some(secret));

        // Simulate unavailable/locked credential store on delete: DB row stays.
        let locked_creds = LazyCredentialStore::unavailable("keychain locked");
        assert!(
            delete_ssh_profile_with_credentials(&db, &locked_creds, "prod-web")
                .is_err()
        );
        assert!(db.get_ssh_profile("prod-web").unwrap().is_some());

        // With working credential store: both secret and DB row are removed.
        assert!(
            delete_ssh_profile_with_credentials(&db, &creds, "prod-web")
                .unwrap()
        );
        assert!(db.get_ssh_profile("prod-web").unwrap().is_none());
        assert_eq!(creds.get_ssh_secret(&profile.id).unwrap(), None);

        // Deleting a profile when no encrypted file exists short-circuits.
        let profile2 = db
            .add_ssh_profile(&SshProfile::new(
                "no-secret",
                "10.0.0.11",
                22,
                "ubuntu",
                AuthMode::PrivateKey,
            ))
            .unwrap();
        let fresh_root = Arc::new(MemoryRootSecretStore::new([1_u8; 32]));
        let fresh_creds = LazyCredentialStore::new(
            dir.path().join("never-created.enc"),
            fresh_root.clone(),
        );
        assert!(
            delete_ssh_profile_with_credentials(&db, &fresh_creds, "no-secret")
                .unwrap()
        );
        assert_eq!(fresh_root.call_count(), 0);
        assert!(db.get_ssh_profile_by_id(&profile2.id).unwrap().is_none());
    }

    #[test]
    fn preload_short_circuits_without_file_and_repeats_without_extra_calls() {
        let (store, root_store, temp_dir) = temp_store();
        let file_path = temp_dir.0.join(CREDENTIALS_FILE_NAME);

        assert!(!store.has_encrypted_file());
        assert!(!store.is_loaded());
        assert_eq!(store.preload(), Ok(false));
        assert_eq!(root_store.call_count(), 0);
        assert!(!store.is_loaded());

        // Write encrypted file from a separate store (simulating an earlier
        // session or CLI process).
        let writer = LazyCredentialStore::new(
            file_path,
            Arc::new(MemoryRootSecretStore::new([42_u8; 32])),
        );
        writer.set("ruum.ssh", "prof-1", b"hunter2").unwrap();

        assert!(store.has_encrypted_file());
        assert!(!store.is_loaded());
        assert_eq!(store.preload(), Ok(true));
        assert_eq!(root_store.call_count(), 1);
        assert!(store.is_loaded());

        // Subsequent preload and reads reuse the cached root secret (0 extra).
        assert_eq!(store.preload(), Ok(false));
        assert_eq!(
            store.get("ruum.ssh", "prof-1").unwrap().unwrap().as_slice(),
            b"hunter2"
        );
        assert_eq!(root_store.call_count(), 1);
    }

    #[test]
    fn transient_keychain_error_allows_retry_without_locking_store() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        struct FlakyRootSecretStore {
            fail_next: AtomicBool,
            calls: AtomicUsize,
            secret: [u8; 32],
        }

        impl RootSecretStore for FlakyRootSecretStore {
            fn get_or_create_root_secret(
                &self,
            ) -> Result<Zeroizing<[u8; 32]>, String> {
                self.calls.fetch_add(1, Ordering::AcqRel);
                if self.fail_next.swap(false, Ordering::AcqRel) {
                    Err("User canceled the operation (-128)".to_owned())
                } else {
                    Ok(Zeroizing::new(self.secret))
                }
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join(CREDENTIALS_FILE_NAME);
        let seeder = LazyCredentialStore::new(
            file_path.clone(),
            Arc::new(MemoryRootSecretStore::new([77_u8; 32])),
        );
        seeder.set("ruum.ssh", "prof-cancel", b"pw-ok").unwrap();

        let flaky = Arc::new(FlakyRootSecretStore {
            fail_next: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            secret: [77_u8; 32],
        });
        let store = LazyCredentialStore::new(file_path, flaky.clone());

        assert!(store.preload().is_err());
        assert_eq!(store.unavailable_reason(), None);
        assert!(!store.is_loaded());
        assert_eq!(flaky.calls.load(Ordering::Acquire), 1);

        // Retry succeeds without rebuilding the store.
        assert_eq!(store.preload(), Ok(true));
        assert!(store.is_loaded());
        assert_eq!(flaky.calls.load(Ordering::Acquire), 2);
        assert_eq!(
            store
                .get("ruum.ssh", "prof-cancel")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"pw-ok"
        );
        assert_eq!(flaky.calls.load(Ordering::Acquire), 2);
    }

    #[test]
    fn external_file_update_and_delete_refresh_cache_without_keychain() {
        let (store, root_store, temp_dir) = temp_store();
        let file_path = temp_dir.0.join(CREDENTIALS_FILE_NAME);

        store.set("ruum.ssh", "prof-ext", b"secret-1").unwrap();
        assert_eq!(root_store.call_count(), 1);
        assert!(store.is_loaded());

        // External process (same length within same timestamp quantum) updates
        // the encrypted file with a fresh AES-GCM nonce.
        let cli_store = LazyCredentialStore::new(
            file_path.clone(),
            Arc::new(MemoryRootSecretStore::new([42_u8; 32])),
        );
        cli_store.set("ruum.ssh", "prof-ext", b"secret-2").unwrap();

        assert_eq!(
            store
                .get("ruum.ssh", "prof-ext")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"secret-2"
        );
        assert_eq!(root_store.call_count(), 1);

        // External process deletes the encrypted file -> in-memory cache clears.
        fs::remove_file(&file_path).unwrap();
        assert_eq!(store.get("ruum.ssh", "prof-ext").unwrap(), None);
        assert!(!store.is_loaded());
        assert!(!store.has_encrypted_file());
        assert_eq!(store.preload(), Ok(false));
        assert_eq!(root_store.call_count(), 1);
    }

    #[test]
    fn preload_releases_mutex_while_waiting_on_root_secret() {
        use std::sync::mpsc;
        use std::time::Duration;

        struct BlockingRootSecretStore {
            secret: [u8; 32],
            entered_tx: Mutex<Option<mpsc::Sender<()>>>,
            release_rx: Mutex<Option<mpsc::Receiver<()>>>,
        }

        impl RootSecretStore for BlockingRootSecretStore {
            fn get_or_create_root_secret(
                &self,
            ) -> Result<Zeroizing<[u8; 32]>, String> {
                let rx = self.release_rx.lock().unwrap().take();
                if let Some(rx) = rx {
                    let tx = self.entered_tx.lock().unwrap().take();
                    if let Some(tx) = tx {
                        let _ = tx.send(());
                    }
                    rx.recv_timeout(Duration::from_secs(5)).map_err(|err| {
                        format!("timed out waiting for test release: {err}")
                    })?;
                }
                Ok(Zeroizing::new(self.secret))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join(CREDENTIALS_FILE_NAME);
        let seeder = LazyCredentialStore::new(
            file_path.clone(),
            Arc::new(MemoryRootSecretStore::new([55_u8; 32])),
        );
        seeder.set("ruum.ssh", "prof-a", b"alpha").unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = Arc::new(BlockingRootSecretStore {
            secret: [55_u8; 32],
            entered_tx: Mutex::new(Some(entered_tx)),
            release_rx: Mutex::new(Some(release_rx)),
        });
        let store = LazyCredentialStore::new(file_path, blocker);
        let bg_store = store.clone();

        let handle = std::thread::spawn(move || bg_store.preload());
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("preload entered root_store");

        // While Thread 1 is blocked inside OS keychain, main/UI thread MUST NOT
        // block on `has_encrypted_file`, `is_loaded`, or `set`.
        assert!(store.has_encrypted_file());
        assert!(!store.is_loaded());
        store.set("ruum.ssh", "prof-b", b"beta").unwrap();
        assert!(store.is_loaded());

        release_tx.send(()).expect("release preload");
        assert_eq!(handle.join().unwrap(), Ok(false));

        // Both pre-existing and concurrent entries remain intact.
        assert_eq!(
            store.get("ruum.ssh", "prof-a").unwrap().unwrap().as_slice(),
            b"alpha"
        );
        assert_eq!(
            store.get("ruum.ssh", "prof-b").unwrap().unwrap().as_slice(),
            b"beta"
        );
    }

    #[test]
    fn ai_api_key_crud_and_lazy_keychain_short_circuit() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join(CREDENTIALS_FILE_NAME);
        let root_store = Arc::new(MemoryRootSecretStore::new([99_u8; 32]));
        let store = LazyCredentialStore::new(file_path, root_store.clone());

        assert_eq!(AI_CREDENTIAL_SERVICE, "ruum.ai");
        assert_eq!(AI_DIRECT_API_KEY_ACCOUNT, "direct-api-key");

        // Missing file short-circuits with zero keychain calls.
        assert!(!store.has_ai_api_key().unwrap());
        assert!(store.get_ai_api_key().unwrap().is_none());
        store.delete_ai_api_key().unwrap();
        assert_eq!(root_store.call_count(), 0);

        // Empty/whitespace API keys are rejected.
        assert!(store.set_ai_api_key("   ").is_err());
        assert_eq!(root_store.call_count(), 0);

        // Save, read, overwrite, delete.
        store.set_ai_api_key("  test-api-key  ").unwrap();
        assert_eq!(root_store.call_count(), 1);
        assert!(store.has_ai_api_key().unwrap());
        assert_eq!(
            store
                .get_ai_api_key()
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("test-api-key")
        );

        store.delete_ai_api_key().unwrap();
        assert!(!store.has_ai_api_key().unwrap());
        assert!(store.get_ai_api_key().unwrap().is_none());
    }
}
