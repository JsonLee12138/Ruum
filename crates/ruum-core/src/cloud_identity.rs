//! Tri-platform privacy-preserving cloud device identity, `PoW` solver, and
//! Ed25519 request signing.
//!
//! Strictly avoids MAC address collection and `unsafe` Win32 FFI. Uses a
//! testable [`CommandRunner`] (configured with `CREATE_NO_WINDOW` on Windows in
//! production) and mixes `SQLite` `cloud_state.install_salt` only into weak or
//! placeholder anchor sides.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest as _, Sha256};

use crate::ai_cli::{CommandRunner, TimedCommandRunner};
use crate::storage::StorageDb;

/// Domain separation prefix for hardware anchor SHA-256 hashing.
pub const HW_ANCHOR_DOMAIN_PREFIX: &str = "ruum-hw-anchor-v1:";

/// Domain separation prefix for OS anchor SHA-256 hashing.
pub const OS_ANCHOR_DOMAIN_PREFIX: &str = "ruum-os-anchor-v1:";

/// HTTP header carrying the registered Ruum device identifier.
pub const HEADER_DEVICE_ID: &str = "X-Ruum-Device-Id";

/// HTTP header carrying the Unix timestamp (seconds) of the signed request.
pub const HEADER_TS: &str = "X-Ruum-Ts";

/// HTTP header carrying the per-request random nonce.
pub const HEADER_NONCE: &str = "X-Ruum-Nonce";

/// HTTP header carrying the base64-encoded Ed25519 request signature.
pub const HEADER_SIG: &str = "X-Ruum-Sig";

/// Maximum SHA-256 `PoW` iterations before failing closed.
pub const MAX_POW_ITERATIONS: u64 = 16_777_216;

/// Maximum `PoW` leading-zero difficulty accepted by the client solver.
pub const MAX_POW_DIFFICULTY_BITS: u8 = 32;

/// Operating system family for device anchor probing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudIdentityOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment inputs and filesystem roots for device anchor probing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudIdentityEnv {
    pub os: CloudIdentityOs,
    pub linux_dmi_dir: PathBuf,
    pub linux_machine_id_path: PathBuf,
    pub linux_dbus_machine_id_path: PathBuf,
}

impl Default for CloudIdentityEnv {
    fn default() -> Self {
        Self::current()
    }
}

impl CloudIdentityEnv {
    /// Capture default OS anchor paths for the current platform without
    /// touching any keychain or network resource.
    #[must_use]
    pub fn current() -> Self {
        Self {
            os: CloudIdentityFactory::current_os(),
            linux_dmi_dir: PathBuf::from("/sys/class/dmi/id"),
            linux_machine_id_path: PathBuf::from("/etc/machine-id"),
            linux_dbus_machine_id_path: PathBuf::from(
                "/var/lib/dbus/machine-id",
            ),
        }
    }
}

/// Unhashed, scrubbed hardware and OS anchor signals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawDeviceAnchors {
    pub hw_parts: Vec<String>,
    pub hw_strong: bool,
    pub os_parts: Vec<String>,
    pub os_strong: bool,
}

impl RawDeviceAnchors {
    /// Return whether either anchor side requires `cloud_state.install_salt`.
    #[must_use]
    pub const fn needs_install_salt(&self) -> bool {
        !self.hw_strong || !self.os_strong
    }
}

/// Hashed 64-character lowercase hex device anchors ready for registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAnchors {
    pub hw_anchor_hash: String,
    pub os_anchor_hash: String,
    pub hw_used_install_salt: bool,
    pub os_used_install_salt: bool,
}

/// Platform-specific hardware/OS anchor probing contract.
pub trait CloudIdentityAdapter: Send + Sync {
    fn os(&self) -> CloudIdentityOs;

    fn collect_raw_anchors(
        &self,
        env: &CloudIdentityEnv,
        runner: &dyn CommandRunner,
    ) -> RawDeviceAnchors;
}

/// macOS device anchor adapter (`ioreg`, `diskutil`, `sysctl`).
#[derive(Clone, Copy, Debug, Default)]
pub struct MacOsCloudIdentityAdapter;

impl CloudIdentityAdapter for MacOsCloudIdentityAdapter {
    fn os(&self) -> CloudIdentityOs {
        CloudIdentityOs::MacOs
    }

    fn collect_raw_anchors(
        &self,
        _env: &CloudIdentityEnv,
        runner: &dyn CommandRunner,
    ) -> RawDeviceAnchors {
        let mut hw_parts = Vec::new();
        if let Ok(out) = runner.run(
            Path::new("ioreg"),
            &["-rd1", "-c", "IOPlatformExpertDevice"],
            &[],
        ) && out.success()
        {
            if let Some(uuid) =
                parse_ioreg_property(&out.stdout, "IOPlatformUUID")
                    .and_then(|v| scrub_anchor_signal(&v))
            {
                hw_parts.push(format!("uuid={uuid}"));
            }
            if let Some(serial) =
                parse_ioreg_property(&out.stdout, "IOPlatformSerialNumber")
                    .and_then(|v| scrub_anchor_signal(&v))
            {
                hw_parts.push(format!("serial={serial}"));
            }
        }
        let hw_strong = !hw_parts.is_empty();

        let mut os_parts = Vec::new();
        if let Ok(out) =
            runner.run(Path::new("diskutil"), &["info", "-plist", "/"], &[])
            && out.success()
            && let Some(vol_uuid) =
                parse_plist_string_value(&out.stdout, "VolumeUUID")
                    .and_then(|v| scrub_anchor_signal(&v))
        {
            os_parts.push(format!("vol_uuid={vol_uuid}"));
        }
        if os_parts.is_empty()
            && let Ok(out) =
                runner.run(Path::new("sysctl"), &["-n", "kern.uuid"], &[])
            && out.success()
            && let Some(kern_uuid) = scrub_anchor_signal(&out.stdout)
        {
            os_parts.push(format!("kern_uuid={kern_uuid}"));
        }
        let os_strong = !os_parts.is_empty();

        RawDeviceAnchors {
            hw_parts,
            hw_strong,
            os_parts,
            os_strong,
        }
    }
}

/// Linux device anchor adapter (`/sys/class/dmi/id/*` + `machine-id`).
#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxCloudIdentityAdapter;

impl CloudIdentityAdapter for LinuxCloudIdentityAdapter {
    fn os(&self) -> CloudIdentityOs {
        CloudIdentityOs::Linux
    }

    fn collect_raw_anchors(
        &self,
        env: &CloudIdentityEnv,
        _runner: &dyn CommandRunner,
    ) -> RawDeviceAnchors {
        let mut hw_parts = Vec::new();
        for (label, file_name) in [
            ("product_uuid", "product_uuid"),
            ("board_serial", "board_serial"),
            ("product_serial", "product_serial"),
        ] {
            if let Some(val) =
                read_scrubbed_file(&env.linux_dmi_dir.join(file_name))
            {
                hw_parts.push(format!("{label}={val}"));
            }
        }
        let hw_strong = !hw_parts.is_empty();
        if !hw_strong
            && let Some(name) =
                read_scrubbed_file(&env.linux_dmi_dir.join("product_name"))
        {
            // Non-root Linux hosts often lack permission on product_uuid;
            // keep `hw_strong = false` so `install_salt` still prevents collisions.
            hw_parts.push(format!("product_name={name}"));
        }

        let mut os_parts = Vec::new();
        if let Some(mid) = read_scrubbed_file(&env.linux_machine_id_path)
            .or_else(|| read_scrubbed_file(&env.linux_dbus_machine_id_path))
        {
            os_parts.push(format!("machine_id={mid}"));
        }
        let os_strong = !os_parts.is_empty();

        RawDeviceAnchors {
            hw_parts,
            hw_strong,
            os_parts,
            os_strong,
        }
    }
}

/// Windows device anchor adapter (`reg.exe` + `powershell.exe Get-CimInstance`).
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsCloudIdentityAdapter;

impl CloudIdentityAdapter for WindowsCloudIdentityAdapter {
    fn os(&self) -> CloudIdentityOs {
        CloudIdentityOs::Windows
    }

    fn collect_raw_anchors(
        &self,
        _env: &CloudIdentityEnv,
        runner: &dyn CommandRunner,
    ) -> RawDeviceAnchors {
        let mut hw_parts = Vec::new();
        if let Ok(out) = runner.run(
            Path::new("reg.exe"),
            &["query", r"HKLM\HARDWARE\DESCRIPTION\System\BIOS"],
            &[],
        ) && out.success()
        {
            for (tag, key) in [
                ("mfg", "SystemManufacturer"),
                ("product", "SystemProductName"),
                ("board", "BaseBoardProduct"),
            ] {
                if let Some(val) = parse_reg_query_value(&out.stdout, key)
                    .and_then(|v| scrub_anchor_signal(&v))
                {
                    hw_parts.push(format!("{tag}={val}"));
                }
            }
        }
        if let Ok(out) = runner.run(
            Path::new("powershell.exe"),
            &[
                "-NoProfile",
                "-Command",
                "(Get-CimInstance -ClassName Win32_ComputerSystemProduct).UUID",
            ],
            &[],
        ) && out.success()
            && let Some(uuid) = out.stdout.lines().find_map(scrub_anchor_signal)
        {
            hw_parts.push(format!("uuid={uuid}"));
        }
        let hw_strong = !hw_parts.is_empty();

        let mut os_parts = Vec::new();
        if let Ok(out) = runner.run(
            Path::new("reg.exe"),
            &[
                "query",
                r"HKLM\SOFTWARE\Microsoft\Cryptography",
                "/v",
                "MachineGuid",
            ],
            &[],
        ) && out.success()
            && let Some(guid) =
                parse_reg_query_value(&out.stdout, "MachineGuid")
                    .and_then(|v| scrub_anchor_signal(&v))
        {
            os_parts.push(format!("machine_guid={guid}"));
        }
        let os_strong = !os_parts.is_empty();

        RawDeviceAnchors {
            hw_parts,
            hw_strong,
            os_parts,
            os_strong,
        }
    }
}

/// Factory creating unconditionally compiled tri-platform anchor adapters.
#[derive(Clone, Copy, Debug, Default)]
pub struct CloudIdentityFactory;

impl CloudIdentityFactory {
    /// Detect the compile-time target OS.
    #[must_use]
    pub const fn current_os() -> CloudIdentityOs {
        if cfg!(target_os = "macos") {
            CloudIdentityOs::MacOs
        } else if cfg!(windows) {
            CloudIdentityOs::Windows
        } else {
            CloudIdentityOs::Linux
        }
    }

    /// Construct the stateless adapter for `os`.
    #[must_use]
    pub fn create_for(os: CloudIdentityOs) -> Box<dyn CloudIdentityAdapter> {
        match os {
            CloudIdentityOs::MacOs => Box::new(MacOsCloudIdentityAdapter),
            CloudIdentityOs::Linux => Box::new(LinuxCloudIdentityAdapter),
            CloudIdentityOs::Windows => Box::new(WindowsCloudIdentityAdapter),
        }
    }

    /// Construct the stateless adapter for the host OS.
    #[must_use]
    pub fn create() -> Box<dyn CloudIdentityAdapter> {
        Self::create_for(Self::current_os())
    }
}

fn read_scrubbed_file(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    scrub_anchor_signal(&raw)
}

fn parse_ioreg_property(stdout: &str, key: &str) -> Option<String> {
    let quoted_key = format!("\"{key}\"");
    for line in stdout.lines() {
        let trimmed = line.trim();
        let rest = if let Some(pos) = trimmed.find(&quoted_key) {
            &trimmed[pos + quoted_key.len()..]
        } else if let Some(pos) = trimmed.find(key) {
            &trimmed[pos + key.len()..]
        } else {
            continue;
        };
        let Some((_, rhs)) = rest.split_once('=') else {
            continue;
        };
        let rhs = rhs.trim();
        if let Some(after_open) = rhs.strip_prefix('"')
            && let Some((val, _)) = after_open.split_once('"')
        {
            return Some(val.to_owned());
        }
        return Some(rhs.to_owned());
    }
    None
}

fn parse_plist_string_value(plist_xml: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let (_, after_key) = plist_xml.split_once(&key_tag)?;
    let (_, after_open) = after_key.split_once("<string>")?;
    let (value, _) = after_open.split_once("</string>")?;
    Some(value.trim().to_owned())
}

fn parse_reg_query_value(stdout: &str, value_name: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(value_name) else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let after_name = rest.trim_start();
        if !after_name.starts_with("REG_") {
            continue;
        }
        let Some((_, val)) = after_name.split_once(char::is_whitespace) else {
            continue;
        };
        return Some(val.trim().to_owned());
    }
    None
}

/// Scrub empty values, common OEM placeholders, and all-0/all-F strings.
#[must_use]
pub fn scrub_anchor_signal(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "to be filled by o.e.m."
            | "default string"
            | "system product name"
            | "system manufacturer"
            | "not specified"
            | "unknown"
            | "none"
            | "n/a"
            | "invalid"
    ) {
        return None;
    }
    let compact: String = lower
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | ':'))
        .collect();
    if compact.is_empty()
        || compact.bytes().all(|b| b == b'0')
        || compact.bytes().all(|b| b == b'f')
    {
        return None;
    }
    Some(trimmed.to_owned())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

fn sha256_prefixed_hex(prefix: &str, payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(payload.as_bytes());
    hex_lower(&hasher.finalize())
}

fn format_side_payload(
    parts: &[String],
    strong: bool,
    salt: Option<&str>,
) -> String {
    let joined = parts.join("|");
    if strong {
        return joined;
    }
    let salt_val = salt.unwrap_or_default();
    if joined.is_empty() {
        format!("salt={salt_val}")
    } else {
        format!("{joined}|salt={salt_val}")
    }
}

/// Hash raw anchors into 64-char hex hashes, mixing `install_salt` only into
/// weak sides.
pub fn resolve_device_anchors_with_salt(
    raw: &RawDeviceAnchors,
    install_salt: Option<&str>,
) -> Result<DeviceAnchors, String> {
    let clean_salt = install_salt.map(str::trim).filter(|s| !s.is_empty());
    if raw.needs_install_salt() && clean_salt.is_none() {
        return Err(
            "weak device anchor side requires non-empty cloud_state.install_salt"
                .to_owned(),
        );
    }

    let raw_hw = format_side_payload(&raw.hw_parts, raw.hw_strong, clean_salt);
    let raw_os = format_side_payload(&raw.os_parts, raw.os_strong, clean_salt);

    Ok(DeviceAnchors {
        hw_anchor_hash: sha256_prefixed_hex(HW_ANCHOR_DOMAIN_PREFIX, &raw_hw),
        os_anchor_hash: sha256_prefixed_hex(OS_ANCHOR_DOMAIN_PREFIX, &raw_os),
        hw_used_install_salt: !raw.hw_strong,
        os_used_install_salt: !raw.os_strong,
    })
}

/// Collect and hash device anchors, lazily persisting `install_salt` in `db`
/// only if either anchor side is weak.
pub fn collect_device_anchors(
    db: &StorageDb,
    env: &CloudIdentityEnv,
    runner: &dyn CommandRunner,
) -> Result<DeviceAnchors, String> {
    let raw = CloudIdentityFactory::create_for(env.os)
        .collect_raw_anchors(env, runner);
    let salt = if raw.needs_install_salt() {
        Some(db.ensure_install_salt().map_err(|e| e.to_string())?)
    } else {
        None
    };
    resolve_device_anchors_with_salt(&raw, salt.as_deref())
}

/// Collect and hash device anchors for the current host OS using a bounded
/// subprocess runner.
pub fn collect_host_device_anchors(
    db: &StorageDb,
) -> Result<DeviceAnchors, String> {
    let env = CloudIdentityEnv::current();
    let runner = TimedCommandRunner::new(Duration::from_secs(5));
    collect_device_anchors(db, &env, &runner)
}

/// Encode an Ed25519 verifying key as standard base64.
#[must_use]
pub fn signing_key_public_b64(signing_key: &SigningKey) -> String {
    STANDARD.encode(signing_key.verifying_key().to_bytes())
}

/// Derive `dev_<32 lowercase hex>` from a base64-encoded 32-byte Ed25519
/// public key.
pub fn derive_device_id(pubkey_b64: &str) -> Result<String, String> {
    let decoded = STANDARD
        .decode(pubkey_b64.trim())
        .map_err(|e| format!("invalid base64 public key: {e}"))?;
    let raw_pk: [u8; 32] = decoded.try_into().map_err(|v: Vec<u8>| {
        format!("Ed25519 public key must be 32 bytes, got {}", v.len())
    })?;
    let digest = Sha256::digest(raw_pk);
    let hex = hex_lower(&digest);
    Ok(format!("dev_{}", &hex[..32]))
}

/// Count leading zero bits in a 32-byte SHA-256 digest.
#[must_use]
pub fn leading_zero_bits(digest: &[u8; 32]) -> u8 {
    let mut bits: u8 = 0;
    for &byte in digest {
        if byte == 0 {
            bits = bits.saturating_add(8);
        } else {
            let lz = u8::try_from(byte.leading_zeros()).unwrap_or(8);
            bits = bits.saturating_add(lz);
            break;
        }
    }
    bits
}

/// Verify that `sha256("{nonce}:{solution}")` has at least `difficulty_bits`
/// leading zero bits.
#[must_use]
pub fn verify_pow_solution(
    nonce: &str,
    solution: &str,
    difficulty_bits: u8,
) -> bool {
    if nonce.trim().is_empty() || solution.trim().is_empty() {
        return false;
    }
    let mut hasher = Sha256::new();
    hasher.update(nonce.as_bytes());
    hasher.update(b":");
    hasher.update(solution.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    leading_zero_bits(&digest) >= difficulty_bits
}

fn format_u64_decimal(mut n: u64, buf: &mut [u8; 20]) -> &[u8] {
    if n == 0 {
        buf[19] = b'0';
        return &buf[19..];
    }
    let mut idx = buf.len();
    while n > 0 {
        idx -= 1;
        buf[idx] = b'0' + u8::try_from(n % 10).unwrap_or(0);
        n /= 10;
    }
    &buf[idx..]
}

/// Search for a `PoW` suffix `pow_solution` meeting `difficulty_bits`.
pub fn solve_pow_challenge(
    nonce: &str,
    difficulty_bits: u8,
) -> Result<String, String> {
    let trimmed = nonce.trim();
    if trimmed.is_empty() {
        return Err("PoW nonce must not be empty".to_owned());
    }
    if difficulty_bits > MAX_POW_DIFFICULTY_BITS {
        return Err(format!(
            "PoW difficulty {difficulty_bits} exceeds cap of \
             {MAX_POW_DIFFICULTY_BITS} bits"
        ));
    }
    let mut base = Sha256::new();
    base.update(trimmed.as_bytes());
    base.update(b":");

    let mut decimal_buf = [0_u8; 20];
    for counter in 0_u64..MAX_POW_ITERATIONS {
        let candidate = format_u64_decimal(counter, &mut decimal_buf);
        let mut hasher = base.clone();
        hasher.update(candidate);
        let digest: [u8; 32] = hasher.finalize().into();
        if leading_zero_bits(&digest) >= difficulty_bits {
            return Ok(std::str::from_utf8(candidate)
                .unwrap_or("0")
                .to_owned());
        }
    }

    Err(format!(
        "PoW solver exceeded {MAX_POW_ITERATIONS} iterations for difficulty \
         {difficulty_bits}"
    ))
}

/// Signed request headers matching `ruum-server`'s Ed25519 guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudAuthHeaders {
    pub device_id: String,
    pub ts: String,
    pub nonce: String,
    pub sig: String,
}

impl CloudAuthHeaders {
    /// Return the four canonical HTTP header `(name, value)` pairs.
    #[must_use]
    pub const fn as_pairs(&self) -> [(&'static str, &str); 4] {
        [
            (HEADER_DEVICE_ID, self.device_id.as_str()),
            (HEADER_TS, self.ts.as_str()),
            (HEADER_NONCE, self.nonce.as_str()),
            (HEADER_SIG, self.sig.as_str()),
        ]
    }
}

/// Build `"{METHOD_UPPER}\n{PATH_NO_QUERY}\n{TS}\n{NONCE}\n{sha256_hex(body)}"`.
#[must_use]
pub fn build_canonical_request_message(
    method: &str,
    path: &str,
    ts_secs: u64,
    nonce: &str,
    body: &[u8],
) -> String {
    let method_upper = method.trim().to_ascii_uppercase();
    let path_no_query = path.split('?').next().unwrap_or(path);
    let body_hash = hex_lower(&Sha256::digest(body));
    format!("{method_upper}\n{path_no_query}\n{ts_secs}\n{nonce}\n{body_hash}")
}

/// Sign a cloud gateway request with the device Ed25519 signing key.
#[must_use]
pub fn sign_cloud_request(
    signing_key: &SigningKey,
    device_id: &str,
    method: &str,
    path: &str,
    ts_secs: u64,
    nonce: &str,
    body: &[u8],
) -> CloudAuthHeaders {
    let message =
        build_canonical_request_message(method, path, ts_secs, nonce, body);
    let sig = signing_key.sign(message.as_bytes());
    CloudAuthHeaders {
        device_id: device_id.to_owned(),
        ts: ts_secs.to_string(),
        nonce: nonce.to_owned(),
        sig: STANDARD.encode(sig.to_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use ed25519_dalek::{Signature, Verifier as _};
    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::ai_cli::CommandOutput;
    use crate::credentials::test_helpers::MemoryRootSecretStore;
    use crate::credentials::{CREDENTIALS_FILE_NAME, LazyCredentialStore};

    #[derive(Default)]
    struct MockRunner {
        responses: BTreeMap<(String, Vec<String>), io::Result<CommandOutput>>,
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl MockRunner {
        fn with_ok(
            mut self,
            program: &str,
            args: &[&str],
            stdout: &str,
        ) -> Self {
            self.responses.insert(
                (
                    program.to_owned(),
                    args.iter().map(|s| (*s).to_owned()).collect(),
                ),
                Ok(CommandOutput {
                    status_code: 0,
                    stdout: stdout.to_owned(),
                    stderr: String::new(),
                }),
            );
            self
        }

        fn with_fail(mut self, program: &str, args: &[&str]) -> Self {
            self.responses.insert(
                (
                    program.to_owned(),
                    args.iter().map(|s| (*s).to_owned()).collect(),
                ),
                Ok(CommandOutput {
                    status_code: 1,
                    stdout: String::new(),
                    stderr: "failed".to_owned(),
                }),
            );
            self
        }

        fn recorded_calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for MockRunner {
        fn run(
            &self,
            program: &Path,
            args: &[&str],
            _envs: &[(&str, &str)],
        ) -> io::Result<CommandOutput> {
            let key = (
                program.to_string_lossy().into_owned(),
                args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            );
            self.calls.lock().unwrap().push(key.clone());
            match self.responses.get(&key) {
                Some(Ok(out)) => Ok(out.clone()),
                Some(Err(err)) => {
                    Err(io::Error::new(err.kind(), err.to_string()))
                }
                None => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("unmocked command: {} {:?}", key.0, key.1),
                )),
            }
        }
    }

    fn is_hex64_lower(s: &str) -> bool {
        s.len() == 64
            && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    #[test]
    fn placeholder_scrubbing_rejects_oem_and_degenerate_values() {
        for bad in [
            "",
            "   ",
            "To Be Filled By O.E.M.",
            " to be filled by o.e.m. ",
            "Default string",
            "System Product Name",
            "System Manufacturer",
            "Not Specified",
            "Unknown",
            "None",
            "N/A",
            "Invalid",
            "00000000-0000-0000-0000-000000000000",
            "000000000000",
            "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
            "ffff-ffff",
        ] {
            assert_eq!(
                scrub_anchor_signal(bad),
                None,
                "expected placeholder `{bad}` to be scrubbed"
            );
        }
        assert_eq!(
            scrub_anchor_signal("  C02ABC123DEF  ").as_deref(),
            Some("C02ABC123DEF")
        );
    }

    #[test]
    fn macos_adapter_collects_strong_anchors_without_mac_or_salt() {
        let temp = tempfile::tempdir().unwrap();
        let db1 = StorageDb::open(temp.path().join("db1.db")).unwrap();
        let db2 = StorageDb::open(temp.path().join("db2.db")).unwrap();

        let ioreg_out = r#"
+-o Root  <class IORegistryEntry, id 0x100000100, retain 20>
    {
      "IOPlatformUUID" = "11111111-2222-3333-4444-555555555555"
      "IOPlatformSerialNumber" = "C02TESTSERIAL"
    }
"#;
        let diskutil_plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>VolumeUUID</key>
	<string>AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE</string>
</dict>
</plist>"#;
        let runner = MockRunner::default()
            .with_ok(
                "ioreg",
                &["-rd1", "-c", "IOPlatformExpertDevice"],
                ioreg_out,
            )
            .with_ok("diskutil", &["info", "-plist", "/"], diskutil_plist);

        let mut env = CloudIdentityEnv::current();
        env.os = CloudIdentityOs::MacOs;

        let a1 = collect_device_anchors(&db1, &env, &runner).unwrap();
        let a2 = collect_device_anchors(&db2, &env, &runner).unwrap();

        assert!(is_hex64_lower(&a1.hw_anchor_hash));
        assert!(is_hex64_lower(&a1.os_anchor_hash));
        assert_ne!(a1.hw_anchor_hash, a1.os_anchor_hash);
        assert!(!a1.hw_used_install_salt);
        assert!(!a1.os_used_install_salt);
        // Reinstall survival: strong anchors match across fresh databases.
        assert_eq!(a1.hw_anchor_hash, a2.hw_anchor_hash);
        assert_eq!(a1.os_anchor_hash, a2.os_anchor_hash);
        assert_eq!(db1.load_cloud_state().unwrap().install_salt, None);

        // Also verify sysctl fallback when diskutil fails.
        let runner_fallback = MockRunner::default()
            .with_ok(
                "ioreg",
                &["-rd1", "-c", "IOPlatformExpertDevice"],
                ioreg_out,
            )
            .with_fail("diskutil", &["info", "-plist", "/"])
            .with_ok(
                "sysctl",
                &["-n", "kern.uuid"],
                "99999999-8888-7777-6666-555555555555\n",
            );
        let fb = collect_device_anchors(&db1, &env, &runner_fallback).unwrap();
        assert!(!fb.os_used_install_salt);
        assert!(is_hex64_lower(&fb.os_anchor_hash));

        // No MAC-reading tools invoked.
        for (prog, args) in runner.recorded_calls() {
            assert!(!prog.contains("ifconfig"));
            assert!(!prog.contains("networksetup"));
            assert!(
                !args
                    .iter()
                    .any(|a| a.contains("ether") || a.contains("MAC"))
            );
        }
    }

    #[test]
    fn windows_adapter_uses_reg_and_powershell_and_salts_only_weak_side() {
        let temp = tempfile::tempdir().unwrap();
        let db1 = StorageDb::open(temp.path().join("win1.db")).unwrap();
        let db2 = StorageDb::open(temp.path().join("win2.db")).unwrap();

        let bios_placeholders = "\r\n\
            HKEY_LOCAL_MACHINE\\HARDWARE\\DESCRIPTION\\System\\BIOS\r\n\
            \x20   SystemManufacturer    REG_SZ    System manufacturer\r\n\
            \x20   SystemProductName     REG_SZ    To Be Filled By O.E.M.\r\n\
            \x20   BaseBoardProduct      REG_SZ    Default string\r\n";
        let machine_guid_ok = "\r\n\
            HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n\
            \x20   MachineGuid    REG_SZ    \
            9f8e7d6c-5b4a-3210-fedc-ba9876543210\r\n";

        let runner = MockRunner::default()
            .with_ok(
                "reg.exe",
                &["query", r"HKLM\HARDWARE\DESCRIPTION\System\BIOS"],
                bios_placeholders,
            )
            .with_ok(
                "powershell.exe",
                &[
                    "-NoProfile",
                    "-Command",
                    "(Get-CimInstance -ClassName \
                     Win32_ComputerSystemProduct).UUID",
                ],
                "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF\r\n",
            )
            .with_ok(
                "reg.exe",
                &[
                    "query",
                    r"HKLM\SOFTWARE\Microsoft\Cryptography",
                    "/v",
                    "MachineGuid",
                ],
                machine_guid_ok,
            );

        let mut env = CloudIdentityEnv::current();
        env.os = CloudIdentityOs::Windows;

        let a1 = collect_device_anchors(&db1, &env, &runner).unwrap();
        let a2 = collect_device_anchors(&db2, &env, &runner).unwrap();

        assert!(is_hex64_lower(&a1.hw_anchor_hash));
        assert!(is_hex64_lower(&a1.os_anchor_hash));
        assert_ne!(a1.hw_anchor_hash, a1.os_anchor_hash);
        // Hardware side had only OEM/all-F placeholders -> salted.
        assert!(a1.hw_used_install_salt);
        assert_ne!(a1.hw_anchor_hash, a2.hw_anchor_hash);
        // OS MachineGuid was strong -> NOT salted -> survives DB reset.
        assert!(!a1.os_used_install_salt);
        assert_eq!(a1.os_anchor_hash, a2.os_anchor_hash);
        // Same DB re-reads the exact same salted hash.
        let a1_again = collect_device_anchors(&db1, &env, &runner).unwrap();
        assert_eq!(a1, a1_again);

        for (prog, args) in runner.recorded_calls() {
            assert!(!prog.eq_ignore_ascii_case("wmic"));
            assert!(!prog.eq_ignore_ascii_case("getmac"));
            assert!(!args.iter().any(|a| a.contains("Win32_NetworkAdapter")));
        }
    }

    #[test]
    fn linux_adapter_handles_non_root_fallback_and_dbus_machine_id() {
        let temp = tempfile::tempdir().unwrap();
        let dmi_dir = temp.path().join("dmi");
        fs::create_dir_all(&dmi_dir).unwrap();
        // Simulate non-root: no product_uuid/board_serial/product_serial,
        // only product_name + /var/lib/dbus/machine-id.
        fs::write(dmi_dir.join("product_name"), "ThinkPad X1 Carbon\n")
            .unwrap();
        let etc_machine_id = temp.path().join("missing-etc-machine-id");
        let dbus_machine_id = temp.path().join("dbus-machine-id");
        fs::write(&dbus_machine_id, "0123456789abcdef0123456789abcdef\n")
            .unwrap();

        let env = CloudIdentityEnv {
            os: CloudIdentityOs::Linux,
            linux_dmi_dir: dmi_dir.clone(),
            linux_machine_id_path: etc_machine_id,
            linux_dbus_machine_id_path: dbus_machine_id,
        };
        let runner = MockRunner::default();
        let db1 = StorageDb::open(temp.path().join("lin1.db")).unwrap();
        let db2 = StorageDb::open(temp.path().join("lin2.db")).unwrap();

        let weak_hw = collect_device_anchors(&db1, &env, &runner).unwrap();
        let weak_hw_2 = collect_device_anchors(&db2, &env, &runner).unwrap();
        assert!(is_hex64_lower(&weak_hw.hw_anchor_hash));
        assert!(is_hex64_lower(&weak_hw.os_anchor_hash));
        assert!(weak_hw.hw_used_install_salt);
        assert!(!weak_hw.os_used_install_salt);
        assert_ne!(weak_hw.hw_anchor_hash, weak_hw_2.hw_anchor_hash);
        assert_eq!(weak_hw.os_anchor_hash, weak_hw_2.os_anchor_hash);

        // Now add strong product_uuid -> hw becomes strong and ignores salt.
        fs::write(
            dmi_dir.join("product_uuid"),
            "12345678-9abc-def0-1234-56789abcdef0\n",
        )
        .unwrap();
        let strong1 = collect_device_anchors(&db1, &env, &runner).unwrap();
        let strong2 = collect_device_anchors(&db2, &env, &runner).unwrap();
        assert!(!strong1.hw_used_install_salt);
        assert_eq!(strong1.hw_anchor_hash, strong2.hw_anchor_hash);
    }

    #[test]
    fn derive_device_id_matches_server_format() {
        let sk = SigningKey::from_bytes(&[9_u8; 32]);
        let pk_bytes = sk.verifying_key().to_bytes();
        let pk_b64 = STANDARD.encode(pk_bytes);

        let digest = Sha256::digest(pk_bytes);
        let hex = hex_lower(&digest);
        let expected = format!("dev_{}", &hex[..32]);

        assert_eq!(signing_key_public_b64(&sk), pk_b64);
        assert_eq!(derive_device_id(&pk_b64).unwrap(), expected);
        assert!(derive_device_id("not-valid-base64!!!").is_err());
        assert!(derive_device_id(&STANDARD.encode([1_u8; 16])).is_err());
    }

    #[test]
    fn pow_solver_finds_valid_solution_and_enforces_cap() {
        let sol0 = solve_pow_challenge("nonce-zero", 0).unwrap();
        assert!(verify_pow_solution("nonce-zero", &sol0, 0));

        let sol12 = solve_pow_challenge("test-pow-nonce", 12).unwrap();
        assert!(verify_pow_solution("test-pow-nonce", &sol12, 12));
        let digest: [u8; 32] =
            Sha256::digest(format!("test-pow-nonce:{sol12}").as_bytes()).into();
        assert!(leading_zero_bits(&digest) >= 12);

        assert!(solve_pow_challenge("", 8).is_err());
        assert!(solve_pow_challenge("nonce", 64).is_err());
    }

    #[test]
    fn sign_cloud_request_produces_verifiable_canonical_signature() {
        let sk = SigningKey::from_bytes(&[17_u8; 32]);
        let pk = sk.verifying_key();
        let device_id = derive_device_id(&signing_key_public_b64(&sk)).unwrap();
        let body = br#"{"skuId":"pro_month"}"#;

        let headers = sign_cloud_request(
            &sk,
            &device_id,
            "post",
            "/v1/billing/purchase?foo=bar",
            1_700_000_123,
            "nonce-abc",
            body,
        );

        assert_eq!(headers.device_id, device_id);
        assert_eq!(headers.ts, "1700000123");
        assert_eq!(headers.nonce, "nonce-abc");

        let body_hex = hex_lower(&Sha256::digest(body));
        let expected_msg = format!(
            "POST\n/v1/billing/purchase\n1700000123\nnonce-abc\n{body_hex}"
        );
        assert_eq!(
            build_canonical_request_message(
                "post",
                "/v1/billing/purchase?foo=bar",
                1_700_000_123,
                "nonce-abc",
                body,
            ),
            expected_msg
        );

        let sig_bytes = STANDARD.decode(&headers.sig).unwrap();
        let sig = Signature::from_slice(&sig_bytes).unwrap();
        assert!(pk.verify(expected_msg.as_bytes(), &sig).is_ok());

        let pairs = headers.as_pairs();
        assert_eq!(pairs[0], ("X-Ruum-Device-Id", headers.device_id.as_str()));
        assert_eq!(pairs[1], ("X-Ruum-Ts", headers.ts.as_str()));
        assert_eq!(pairs[2], ("X-Ruum-Nonce", headers.nonce.as_str()));
        assert_eq!(pairs[3], ("X-Ruum-Sig", headers.sig.as_str()));
    }

    #[test]
    fn startup_and_cloud_state_reads_trigger_zero_keychain_calls() {
        let temp = tempfile::tempdir().unwrap();
        let db = StorageDb::open(temp.path().join("ruum.db")).unwrap();
        let root_store = Arc::new(MemoryRootSecretStore::new([31_u8; 32]));
        let creds = LazyCredentialStore::new(
            temp.path().join(CREDENTIALS_FILE_NAME),
            root_store.clone(),
        );

        // Constructing LazyCredentialStore, loading cloud_state, and computing
        // anchors even with install_salt never touches the OS keychain.
        let state = db.load_cloud_state().unwrap();
        assert_eq!(state.device_id, None);
        let env = CloudIdentityEnv {
            os: CloudIdentityOs::Linux,
            linux_dmi_dir: temp.path().join("empty-dmi"),
            linux_machine_id_path: temp.path().join("missing-mid"),
            linux_dbus_machine_id_path: temp.path().join("missing-dbus"),
        };
        let _anchors =
            collect_device_anchors(&db, &env, &MockRunner::default()).unwrap();
        assert_eq!(root_store.call_count(), 0);
        assert!(db.load_cloud_state().unwrap().install_salt.is_some());

        // Only explicit lazy key creation touches the keychain once.
        let _sk = creds.load_or_create_cloud_signing_key().unwrap();
        assert_eq!(root_store.call_count(), 1);
    }
}
