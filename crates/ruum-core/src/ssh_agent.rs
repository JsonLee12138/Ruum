//! In-memory OpenSSH agent (`SSH_AUTH_SOCK`) and strict SSH target validation
//! adapted from `VaultMesh`.
//!
//! Keys are parsed and decrypted strictly in memory (`Zeroizing<String>`) and
//! exposed only over a short-lived, owner-only Unix domain socket
//! (`/tmp/ruum-ssh-<uid>/<session>.sock`) or Windows Named Pipe
//! (`\\.\pipe\ruum.ssh-agent.<session>`). Neither `~/.ssh/config` nor remote
//! `~/.ssh/authorized_keys` is ever read or modified.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use interprocess::os::windows::named_pipe::{PipeListenerOptions, pipe_mode};
use rsa::pkcs1v15;
use rsa::sha2::{Sha256, Sha512};
use rsa::signature::{SignatureEncoding as _, Signer as _};
use rsa::traits::PublicKeyParts as _;
use serde::{Deserialize, Serialize};
use ssh_key::private::KeypairData;
use ssh_key::{Algorithm, Mpint, PrivateKey};
use url::Host;
use uuid::Uuid;
use zeroize::Zeroizing;

const SSH2_AGENT_FAILURE: u8 = 30;
const SSH2_AGENTC_REQUEST_IDENTITIES: u8 = 11;
const SSH2_AGENT_IDENTITIES_ANSWER: u8 = 12;
const SSH2_AGENTC_SIGN_REQUEST: u8 = 13;
const SSH2_AGENT_SIGN_RESPONSE: u8 = 14;

/// OpenSSH agent sign request flag requesting `rsa-sha2-256`.
const SSH_AGENT_RSA_SHA2_256: u32 = 0x02;

/// OpenSSH agent sign request flag requesting `rsa-sha2-512`.
const SSH_AGENT_RSA_SHA2_512: u32 = 0x04;

const MAX_AGENT_MESSAGE_BYTES: usize = 256 * 1024;
const AGENT_COMMENT: &[u8] = b"ruum-agent";

/// Strictly validated SSH host, port, and username target.
///
/// Rejects leading `-` option injection, shell metacharacters, whitespace,
/// control characters, empty fields, and invalid ports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchTarget {
    host: String,
    port: u16,
    username: String,
}

impl LaunchTarget {
    /// Validate and normalize an SSH `(host, port, username)` target.
    pub fn new(host: &str, port: u16, username: &str) -> Result<Self, String> {
        let host = host.trim();
        let username = username.trim();
        if port == 0
            || host.is_empty()
            || host.len() > 253
            || host.starts_with('-')
            || !host.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || b".-:[]".contains(&byte)
            })
            || username.is_empty()
            || username.len() > 255
            || username.starts_with('-')
            || !username.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || b"._-".contains(&byte)
            })
        {
            return Err("SSH 账号的主机、端口或用户名无效。".to_owned());
        }
        let host = Host::parse(host)
            .map_err(|_| "SSH 账号的主机、端口或用户名无效。".to_owned())?
            .to_string();
        Ok(Self {
            host,
            port,
            username: username.to_owned(),
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Host formatted for `ssh` CLI destination (strips `[...]` around IPv6).
    #[must_use]
    pub fn ssh_host(&self) -> &str {
        self.host
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or(&self.host)
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    /// `username@host` destination string suitable for `ssh` CLI arguments.
    #[must_use]
    pub fn destination(&self) -> String {
        format!("{}@{}", self.username, self.ssh_host())
    }

    /// Human-readable quoted `ssh` command preview.
    #[must_use]
    pub fn display_command(&self) -> String {
        format!(
            "ssh -p {} -- {}",
            self.port,
            crate::export::shell_quote(&self.destination())
        )
    }
}

/// Public metadata for an active in-memory SSH agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshAgentSessionInfo {
    pub session_ref: String,
    pub socket_path: String,
    pub export_command: String,
    pub expires_at: String,
    pub account_label: String,
    pub fingerprint: String,
}

/// Active session handle tracked by [`SshAgentRegistry`].
#[derive(Debug)]
pub struct ActiveAgentSession {
    pub session_ref: String,
    pub account_ref: String,
    pub account_label: String,
    pub socket_path: PathBuf,
    pub expires_at_epoch_ms: u64,
    pub cancelled: Arc<AtomicBool>,
    pub config_fragment: Option<PathBuf>,
}

/// Ephemeral key material cached in memory only for the session lifetime.
#[derive(Clone)]
pub struct EphemeralKeyMaterial {
    pub private_key: Zeroizing<String>,
    pub public_key: Zeroizing<String>,
    pub host_key_sha256: String,
}

impl fmt::Debug for EphemeralKeyMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EphemeralKeyMaterial")
            .field("private_key", &"[REDACTED]")
            .field("host_key_sha256", &self.host_key_sha256)
            .finish_non_exhaustive()
    }
}

/// Registry managing short-lived in-memory OpenSSH agent listeners.
#[derive(Debug)]
pub struct SshAgentRegistry {
    sessions: HashMap<String, ActiveAgentSession>,
    ephemeral_keys: HashMap<String, EphemeralKeyMaterial>,
    device_id: String,
}

impl Default for SshAgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SshAgentRegistry {
    fn drop(&mut self) {
        self.close_all();
    }
}

pub type SharedSshAgentRegistry = Arc<Mutex<SshAgentRegistry>>;

impl SshAgentRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_device_id(Uuid::new_v4().simple().to_string())
    }

    #[must_use]
    pub fn new_with_device_id(device_id: String) -> Self {
        Self {
            sessions: HashMap::new(),
            ephemeral_keys: HashMap::new(),
            device_id,
        }
    }

    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    #[must_use]
    pub fn get_ephemeral_key(
        &self,
        account_ref: &str,
        host_key_sha256: &str,
    ) -> Option<Zeroizing<String>> {
        self.ephemeral_keys.get(account_ref).and_then(|mat| {
            if mat.host_key_sha256 == host_key_sha256 {
                Some(mat.private_key.clone())
            } else {
                None
            }
        })
    }

    pub fn store_ephemeral_key(
        &mut self,
        account_ref: &str,
        host_key_sha256: &str,
        public_key: Zeroizing<String>,
        private_key: Zeroizing<String>,
    ) {
        self.ephemeral_keys.insert(
            account_ref.to_owned(),
            EphemeralKeyMaterial {
                private_key,
                public_key,
                host_key_sha256: host_key_sha256.to_owned(),
            },
        );
    }

    pub fn clear_ephemeral_key(&mut self, account_ref: &str) {
        self.ephemeral_keys.remove(account_ref);
    }

    /// Decrypt `private_key_pem` in memory and spawn an authenticated
    /// short-lived SSH agent listener on Unix Socket or Windows Named Pipe.
    pub fn start_session(
        &mut self,
        #[cfg(windows)] base_dir: &Path,
        #[cfg(not(windows))] _base_dir: &Path,
        account_ref: &str,
        account_label: &str,
        private_key_pem: &Zeroizing<String>,
        passphrase: Option<&Zeroizing<String>>,
        duration_minutes: u64,
    ) -> Result<SshAgentSessionInfo, String> {
        let mut key = PrivateKey::from_openssh(private_key_pem.as_str())
            .map_err(|e| format!("Invalid private key: {e}"))?;
        if let Some(passphrase) = passphrase
            && !passphrase.is_empty()
        {
            key = key
                .decrypt(passphrase.as_bytes())
                .map_err(|e| format!("Failed to decrypt private key: {e}"))?;
        }

        let duration_minutes = duration_minutes.clamp(1, 60);
        let duration = Duration::from_secs(duration_minutes * 60);
        let now = SystemTime::now();
        let expires_system_time = now + duration;
        let expires_at_epoch_ms = expires_system_time
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;
        let expires_at =
            chrono::DateTime::<chrono::Utc>::from(expires_system_time)
                .to_rfc3339();

        let session_uuid = Uuid::new_v4();
        let session_ref = format!("sess_{}", session_uuid.simple());
        let public_key = key.public_key();
        let fingerprint =
            public_key.fingerprint(ssh_key::HashAlg::Sha256).to_string();

        #[cfg(unix)]
        let session_dir = {
            let uid = rustix::process::geteuid().as_raw();
            let dir = PathBuf::from(format!("/tmp/ruum-ssh-{uid}"));
            fs::create_dir_all(&dir).map_err(|e| {
                format!("Failed to create agent sessions directory: {e}")
            })?;
            let _ =
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
            dir
        };
        #[cfg(windows)]
        let _session_dir = {
            let dir = base_dir.join("agent-sessions");
            fs::create_dir_all(&dir).map_err(|e| {
                format!("Failed to create agent sessions directory: {e}")
            })?;
            dir
        };

        #[cfg(unix)]
        let socket_path = session_dir.join(format!("{session_ref}.sock"));
        #[cfg(windows)]
        let socket_path =
            PathBuf::from(format!(r"\\.\pipe\ruum.ssh-agent.{session_ref}"));

        let cancelled = Arc::new(AtomicBool::new(false));

        #[cfg(unix)]
        {
            if socket_path.exists() {
                let _ = fs::remove_file(&socket_path);
            }
            let listener = UnixListener::bind(&socket_path)
                .map_err(|e| format!("Failed to bind SSH agent socket: {e}"))?;
            let _ = fs::set_permissions(
                &socket_path,
                fs::Permissions::from_mode(0o600),
            );

            let cancelled_clone = Arc::clone(&cancelled);
            let socket_path_clone = socket_path.clone();
            let key_clone = key;

            thread::Builder::new()
                .name(format!("ssh-agent-{}", &session_ref[..8]))
                .spawn(move || {
                    run_unix_agent_listener(
                        &listener,
                        &socket_path_clone,
                        &key_clone,
                        duration,
                        &cancelled_clone,
                    );
                })
                .map_err(|e| format!("Failed to spawn agent thread: {e}"))?;
        }

        #[cfg(windows)]
        {
            let pipe_name = socket_path.to_string_lossy().to_string();
            let listener = PipeListenerOptions::new()
                .path(pipe_name.as_str())
                .nonblocking(true)
                .create_duplex::<pipe_mode::Bytes>()
                .map_err(|e| {
                    format!("Failed to bind SSH agent named pipe: {e}")
                })?;

            let cancelled_clone = Arc::clone(&cancelled);
            let key_clone = key;

            thread::Builder::new()
                .name(format!("ssh-agent-{}", &session_ref[..8]))
                .spawn(move || {
                    run_windows_agent_listener(
                        &listener,
                        &key_clone,
                        duration,
                        &cancelled_clone,
                    );
                })
                .map_err(|e| format!("Failed to spawn agent thread: {e}"))?;
        }

        let socket_str = socket_path.to_string_lossy().to_string();
        let export_command = format!(r#"export SSH_AUTH_SOCK="{socket_str}""#);

        let info = SshAgentSessionInfo {
            session_ref: session_ref.clone(),
            socket_path: socket_str,
            export_command,
            expires_at,
            account_label: account_label.to_owned(),
            fingerprint,
        };

        self.sessions.insert(
            session_ref.clone(),
            ActiveAgentSession {
                session_ref,
                account_ref: account_ref.to_owned(),
                account_label: account_label.to_owned(),
                socket_path,
                expires_at_epoch_ms,
                cancelled,
                config_fragment: None,
            },
        );

        Ok(info)
    }

    pub fn attach_config_fragment(
        &mut self,
        session_ref: &str,
        fragment_path: PathBuf,
    ) {
        if let Some(session) = self.sessions.get_mut(session_ref) {
            session.config_fragment = Some(fragment_path);
        }
    }

    pub fn close_session(&mut self, session_ref: &str) -> bool {
        if let Some(session) = self.sessions.remove(session_ref) {
            session.cancelled.store(true, Ordering::Release);
            #[cfg(unix)]
            {
                let _ = fs::remove_file(&session.socket_path);
            }
            if let Some(fragment) = &session.config_fragment {
                let _ = fs::remove_file(fragment);
            }
            true
        } else {
            false
        }
    }

    pub fn close_all(&mut self) {
        self.ephemeral_keys.clear();
        for (_, session) in self.sessions.drain() {
            session.cancelled.store(true, Ordering::Release);
            #[cfg(unix)]
            {
                let _ = fs::remove_file(&session.socket_path);
            }
            if let Some(fragment) = &session.config_fragment {
                let _ = fs::remove_file(fragment);
            }
        }
    }
}

#[cfg(unix)]
fn run_unix_agent_listener(
    listener: &UnixListener,
    socket_path: &Path,
    key: &PrivateKey,
    ttl: Duration,
    cancelled: &AtomicBool,
) {
    let deadline = Instant::now() + ttl;
    let _ = listener.set_nonblocking(true);

    while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                handle_agent_stream(&mut stream, key, deadline, cancelled);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }

    let _ = fs::remove_file(socket_path);
}

#[cfg(windows)]
fn run_windows_agent_listener(
    listener: &interprocess::os::windows::named_pipe::PipeListener<
        pipe_mode::Bytes,
        pipe_mode::Bytes,
    >,
    key: &PrivateKey,
    ttl: Duration,
    cancelled: &AtomicBool,
) {
    let deadline = Instant::now() + ttl;
    while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
        match listener.accept() {
            Ok(mut stream) => {
                let _ = stream.set_nonblocking(false);
                handle_agent_stream(&mut stream, key, deadline, cancelled);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
}

fn put_u32(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_be_bytes());
}

fn put_string(buf: &mut Vec<u8>, data: &[u8]) {
    put_u32(buf, data.len() as u32);
    buf.extend_from_slice(data);
}

fn send_agent_msg(
    stream: &mut impl Write,
    msg_type: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let len = (1 + payload.len()) as u32;
    let mut header = [0_u8; 5];
    header[0..4].copy_from_slice(&len.to_be_bytes());
    header[4] = msg_type;
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

/// Serve OpenSSH agent protocol requests on a connected byte stream until EOF,
/// cancellation, or session TTL expiry.
pub fn handle_agent_stream(
    stream: &mut (impl Read + Write),
    key: &PrivateKey,
    deadline: Instant,
    cancelled: &AtomicBool,
) {
    let Ok(pubkey_bytes) = key.public_key().to_bytes() else {
        return;
    };

    loop {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            let _ = send_agent_msg(stream, SSH2_AGENT_FAILURE, &[]);
            break;
        }

        let mut len_buf = [0_u8; 4];
        if stream.read_exact(&mut len_buf).is_err() {
            break;
        }
        let msg_len = u32::from_be_bytes(len_buf) as usize;
        if msg_len == 0 || msg_len > MAX_AGENT_MESSAGE_BYTES {
            let _ = send_agent_msg(stream, SSH2_AGENT_FAILURE, &[]);
            break;
        }

        let mut msg_buf = vec![0_u8; msg_len];
        if stream.read_exact(&mut msg_buf).is_err() {
            break;
        }

        let msg_type = msg_buf[0];
        let payload = &msg_buf[1..];

        match msg_type {
            SSH2_AGENTC_REQUEST_IDENTITIES => {
                let mut answer = Vec::new();
                put_u32(&mut answer, 1);
                put_string(&mut answer, &pubkey_bytes);
                let comment = if key.comment().is_empty() {
                    AGENT_COMMENT
                } else {
                    key.comment().as_bytes()
                };
                put_string(&mut answer, comment);
                if send_agent_msg(stream, SSH2_AGENT_IDENTITIES_ANSWER, &answer)
                    .is_err()
                {
                    break;
                }
            }
            SSH2_AGENTC_SIGN_REQUEST => {
                if Instant::now() >= deadline
                    || cancelled.load(Ordering::Acquire)
                {
                    let _ = send_agent_msg(stream, SSH2_AGENT_FAILURE, &[]);
                    break;
                }

                if let Some(signature_payload) =
                    process_sign_request(payload, key)
                {
                    if send_agent_msg(
                        stream,
                        SSH2_AGENT_SIGN_RESPONSE,
                        &signature_payload,
                    )
                    .is_err()
                    {
                        break;
                    }
                } else {
                    let _ = send_agent_msg(stream, SSH2_AGENT_FAILURE, &[]);
                }
            }
            _ => {
                let _ = send_agent_msg(stream, SSH2_AGENT_FAILURE, &[]);
            }
        }
    }
}

fn mpint_to_biguint(mpint: &Mpint) -> Option<rsa::BigUint> {
    mpint.as_positive_bytes().map(rsa::BigUint::from_bytes_be)
}

fn rsa_private_key_from_ssh(
    key: &ssh_key::private::RsaKeypair,
) -> Option<rsa::RsaPrivateKey> {
    // Work around `ssh_key 0.6.7` `TryFrom<&RsaKeypair> for RsaPrivateKey`,
    // which duplicates `p` instead of passing `(p, q)` to `from_components`.
    let modulus = mpint_to_biguint(&key.public.n)?;
    let public_exp = mpint_to_biguint(&key.public.e)?;
    let private_exp = mpint_to_biguint(&key.private.d)?;
    let prime_p = mpint_to_biguint(&key.private.p)?;
    let prime_q = mpint_to_biguint(&key.private.q)?;
    let priv_key = rsa::RsaPrivateKey::from_components(
        modulus,
        public_exp,
        private_exp,
        vec![prime_p, prime_q],
    )
    .ok()?;
    if priv_key.size().saturating_mul(8) >= 2048 {
        Some(priv_key)
    } else {
        None
    }
}

fn sign_with_key_and_flags(
    key: &PrivateKey,
    data: &[u8],
    flags: u32,
) -> Option<(&'static str, Vec<u8>)> {
    match key.key_data() {
        KeypairData::Ed25519(_) => {
            let sig: ssh_key::Signature = key.try_sign(data).ok()?;
            Some(("ssh-ed25519", sig.as_bytes().to_vec()))
        }
        KeypairData::Ecdsa(_) => {
            let sig: ssh_key::Signature = key.try_sign(data).ok()?;
            let alg_name = match key.algorithm() {
                Algorithm::Ecdsa { curve } => match curve {
                    ssh_key::EcdsaCurve::NistP256 => "ecdsa-sha2-nistp256",
                    ssh_key::EcdsaCurve::NistP384 => "ecdsa-sha2-nistp384",
                    ssh_key::EcdsaCurve::NistP521 => "ecdsa-sha2-nistp521",
                },
                _ => return None,
            };
            Some((alg_name, sig.as_bytes().to_vec()))
        }
        KeypairData::Rsa(rsa_kp) => {
            let rsa_priv = rsa_private_key_from_ssh(rsa_kp)?;
            if flags & SSH_AGENT_RSA_SHA2_512 != 0 {
                let signing_key = pkcs1v15::SigningKey::<Sha512>::new(rsa_priv);
                let sig = signing_key.try_sign(data).ok()?;
                Some(("rsa-sha2-512", sig.to_vec()))
            } else if flags & SSH_AGENT_RSA_SHA2_256 != 0 {
                let signing_key = pkcs1v15::SigningKey::<Sha256>::new(rsa_priv);
                let sig = signing_key.try_sign(data).ok()?;
                Some(("rsa-sha2-256", sig.to_vec()))
            } else {
                let signing_key = pkcs1v15::SigningKey::<Sha512>::new(rsa_priv);
                let sig = signing_key.try_sign(data).ok()?;
                Some(("ssh-rsa", sig.to_vec()))
            }
        }
        _ => None,
    }
}

fn process_sign_request(payload: &[u8], key: &PrivateKey) -> Option<Vec<u8>> {
    if payload.len() < 4 {
        return None;
    }
    let key_len = u32::from_be_bytes(payload[0..4].try_into().ok()?) as usize;
    let offset1 = 4_usize.checked_add(key_len)?;
    if payload.len() < offset1.checked_add(4)? {
        return None;
    }
    let data_len =
        u32::from_be_bytes(payload[offset1..offset1 + 4].try_into().ok()?)
            as usize;
    let offset2 = offset1 + 4;
    let offset3 = offset2.checked_add(data_len)?;
    if payload.len() < offset3 {
        return None;
    }
    let data = &payload[offset2..offset3];

    let flags = if payload.len() >= offset3.checked_add(4)? {
        u32::from_be_bytes(payload[offset3..offset3 + 4].try_into().ok()?)
    } else {
        0
    };

    let (alg_name, sig_bytes) = sign_with_key_and_flags(key, data, flags)?;

    let mut sig_blob = Vec::new();
    put_string(&mut sig_blob, alg_name.as_bytes());
    put_string(&mut sig_blob, &sig_bytes);

    let mut response_payload = Vec::new();
    put_string(&mut response_payload, &sig_blob);
    Some(response_payload)
}

#[cfg(test)]
mod tests {
    use rsa::RsaPrivateKey;
    use rsa::signature::Verifier;
    use ssh_key::{EcdsaCurve, HashAlg, Signature};

    use super::*;

    fn parse_sign_response(output: &[u8]) -> (String, Vec<u8>) {
        assert!(output.len() > 5);
        assert_eq!(output[4], SSH2_AGENT_SIGN_RESPONSE);
        let blob_len =
            u32::from_be_bytes(output[5..9].try_into().unwrap()) as usize;
        let blob = &output[9..9 + blob_len];
        let alg_len =
            u32::from_be_bytes(blob[0..4].try_into().unwrap()) as usize;
        let alg = std::str::from_utf8(&blob[4..4 + alg_len])
            .unwrap()
            .to_owned();
        let sig_offset = 4 + alg_len;
        let sig_len = u32::from_be_bytes(
            blob[sig_offset..sig_offset + 4].try_into().unwrap(),
        ) as usize;
        let sig_bytes = blob[sig_offset + 4..sig_offset + 4 + sig_len].to_vec();
        (alg, sig_bytes)
    }

    #[test]
    fn test_agent_protocol_request_identities_and_sign() {
        struct MockStream<'a> {
            reader: std::io::Cursor<Vec<u8>>,
            writer: &'a mut Vec<u8>,
        }
        impl Read for MockStream<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.reader.read(buf)
            }
        }
        impl Write for MockStream<'_> {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.writer.write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.writer.flush()
            }
        }

        let key = PrivateKey::random(&mut rand_core::OsRng, Algorithm::Ed25519)
            .expect("key");
        let pubkey_bytes = key.public_key().to_bytes().expect("pubkey");

        // 1. Test REQUEST_IDENTITIES.
        let mut client_req = Vec::new();
        put_u32(&mut client_req, 1);
        client_req.push(SSH2_AGENTC_REQUEST_IDENTITIES);

        // 2. Test SIGN_REQUEST.
        let data_to_sign = b"authentication-challenge-bytes-123456";
        let mut sign_req = Vec::new();
        let mut sign_payload = Vec::new();
        sign_payload.push(SSH2_AGENTC_SIGN_REQUEST);
        put_string(&mut sign_payload, &pubkey_bytes);
        put_string(&mut sign_payload, data_to_sign);
        put_u32(&mut sign_payload, 0);

        put_u32(&mut sign_req, sign_payload.len() as u32);
        sign_req.extend_from_slice(&sign_payload);

        let mut input = client_req;
        input.extend_from_slice(&sign_req);

        let mut output = Vec::new();
        let cursor = std::io::Cursor::new(input);
        let cancelled = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_mins(1);

        let mut mock = MockStream {
            reader: cursor,
            writer: &mut output,
        };

        handle_agent_stream(&mut mock, &key, deadline, &cancelled);

        assert!(output.len() > 5);
        let resp1_len =
            u32::from_be_bytes(output[0..4].try_into().unwrap()) as usize;
        assert_eq!(output[4], SSH2_AGENT_IDENTITIES_ANSWER);

        let offset2 = 4 + resp1_len;
        let (alg, sig_bytes) = parse_sign_response(&output[offset2..]);
        assert_eq!(alg, "ssh-ed25519");
        let sig = Signature::new(Algorithm::Ed25519, sig_bytes).unwrap();
        assert!(Verifier::verify(key.public_key(), data_to_sign, &sig).is_ok());
    }

    #[test]
    fn test_agent_protocol_ecdsa_algorithms_and_rsa_sha2_flags() {
        let challenge = b"openssh-agent-sign-challenge";

        // ECDSA P-256 and P-384 preserve their curve algorithm names.
        for (curve, expected_alg) in [
            (EcdsaCurve::NistP256, "ecdsa-sha2-nistp256"),
            (EcdsaCurve::NistP384, "ecdsa-sha2-nistp384"),
        ] {
            let key = PrivateKey::random(
                &mut rand_core::OsRng,
                Algorithm::Ecdsa { curve },
            )
            .expect("ecdsa key");
            let pub_bytes = key.public_key().to_bytes().unwrap();
            let mut payload = Vec::new();
            put_string(&mut payload, &pub_bytes);
            put_string(&mut payload, challenge);
            put_u32(&mut payload, 0);

            let resp = process_sign_request(&payload, &key).unwrap();
            let mut framed = Vec::new();
            put_u32(&mut framed, (1 + resp.len()) as u32);
            framed.push(SSH2_AGENT_SIGN_RESPONSE);
            framed.extend_from_slice(&resp);

            let (alg, sig_bytes) = parse_sign_response(&framed);
            assert_eq!(alg, expected_alg);
            let sig =
                Signature::new(Algorithm::Ecdsa { curve }, sig_bytes).unwrap();
            assert!(
                Verifier::verify(key.public_key(), challenge, &sig).is_ok()
            );
        }

        // RSA SHA-2 flags (0x02 = rsa-sha2-256, 0x04 = rsa-sha2-512).
        let rsa_priv =
            RsaPrivateKey::new(&mut rand_core::OsRng, 2048).expect("rsa key");
        let rsa_kp = ssh_key::private::RsaKeypair::try_from(rsa_priv).unwrap();
        let rsa_key = PrivateKey::from(rsa_kp);
        let pub_bytes = rsa_key.public_key().to_bytes().unwrap();

        for (flags, expected_alg, hash) in [
            (SSH_AGENT_RSA_SHA2_256, "rsa-sha2-256", HashAlg::Sha256),
            (SSH_AGENT_RSA_SHA2_512, "rsa-sha2-512", HashAlg::Sha512),
        ] {
            let mut payload = Vec::new();
            put_string(&mut payload, &pub_bytes);
            put_string(&mut payload, challenge);
            put_u32(&mut payload, flags);

            let resp = process_sign_request(&payload, &rsa_key).unwrap();
            let mut framed = Vec::new();
            put_u32(&mut framed, (1 + resp.len()) as u32);
            framed.push(SSH2_AGENT_SIGN_RESPONSE);
            framed.extend_from_slice(&resp);

            let (alg, sig_bytes) = parse_sign_response(&framed);
            assert_eq!(alg, expected_alg);
            let sig =
                Signature::new(Algorithm::Rsa { hash: Some(hash) }, sig_bytes)
                    .unwrap();
            assert!(
                Verifier::verify(rsa_key.public_key(), challenge, &sig).is_ok()
            );
        }
    }

    #[test]
    fn test_start_and_close_session_socket_path_length_within_limits() {
        let mut registry = SshAgentRegistry::new();
        let key = PrivateKey::random(&mut rand_core::OsRng, Algorithm::Ed25519)
            .unwrap();
        let private_key_pem = key.to_openssh(ssh_key::LineEnding::LF).unwrap();
        let temp_dir =
            std::env::temp_dir().join(format!("ruum-test-{}", Uuid::new_v4()));
        let session_info = registry
            .start_session(
                &temp_dir,
                "account-123",
                "Test Account",
                &private_key_pem,
                None,
                15,
            )
            .expect("session must start successfully");

        #[cfg(windows)]
        {
            assert!(
                temp_dir.join("agent-sessions").is_dir(),
                "Windows SSH agent sessions directory must exist"
            );
            assert!(
                session_info
                    .socket_path
                    .starts_with(r"\\.\pipe\ruum.ssh-agent.")
            );
            let _ = std::fs::remove_dir_all(&temp_dir);
        }

        #[cfg(unix)]
        {
            // On Darwin/macOS, sockaddr_un.sun_path is strictly 104 bytes.
            assert!(
                session_info.socket_path.len() < 104,
                "Socket path must be under 104 bytes for macOS AF_UNIX: {}",
                session_info.socket_path
            );
            assert!(Path::new(&session_info.socket_path).exists());

            // Connect to the real socket and send REQUEST_IDENTITIES.
            let mut stream = std::os::unix::net::UnixStream::connect(
                &session_info.socket_path,
            )
            .unwrap();
            stream
                .write_all(&[0, 0, 0, 1, SSH2_AGENTC_REQUEST_IDENTITIES])
                .unwrap();
            let mut len_buf = [0_u8; 4];
            stream.read_exact(&mut len_buf).unwrap();
            let resp_len = u32::from_be_bytes(len_buf) as usize;
            let mut body = vec![0_u8; resp_len];
            stream.read_exact(&mut body).unwrap();
            assert_eq!(body[0], SSH2_AGENT_IDENTITIES_ANSWER);
        }

        let closed = registry.close_session(&session_info.session_ref);
        assert!(closed);
        #[cfg(unix)]
        {
            assert!(!Path::new(&session_info.socket_path).exists());
        }
    }

    #[test]
    fn test_ephemeral_key_cache_operations_and_close_all_clears() {
        let mut registry =
            SshAgentRegistry::new_with_device_id("dev-12345678".to_owned());
        assert_eq!(registry.device_id(), "dev-12345678");

        let acc_ref = "acc-001";
        let fp = "SHA256:fingerprint1";
        let alt_fp = "SHA256:fingerprint2";

        assert!(registry.get_ephemeral_key(acc_ref, fp).is_none());

        let priv_key = Zeroizing::new("fake_priv_key".to_owned());
        let pub_key = Zeroizing::new("fake_pub_key".to_owned());
        registry.store_ephemeral_key(acc_ref, fp, pub_key, priv_key);

        let hit = registry.get_ephemeral_key(acc_ref, fp);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().as_str(), "fake_priv_key");
        assert!(registry.get_ephemeral_key(acc_ref, alt_fp).is_none());

        registry.close_all();
        assert!(registry.get_ephemeral_key(acc_ref, fp).is_none());
    }

    #[test]
    fn launch_target_validates_and_rejects_shell_injection() {
        let target =
            LaunchTarget::new("10.0.0.2", 2222, "deploy-user").expect("target");
        assert_eq!(
            target.display_command(),
            "ssh -p 2222 -- 'deploy-user@10.0.0.2'"
        );
        assert_eq!(target.destination(), "deploy-user@10.0.0.2");

        let v6 = LaunchTarget::new("[::1]", 22, "ubuntu").expect("ipv6 target");
        assert_eq!(v6.ssh_host(), "::1");
        assert_eq!(v6.destination(), "ubuntu@::1");

        for bad_host in [
            "",
            "-oProxyCommand=pwn",
            "example.com;touch /tmp/pwned",
            "host name",
            "host\nline2",
            "user@host",
        ] {
            assert!(LaunchTarget::new(bad_host, 22, "user").is_err());
        }
        assert!(LaunchTarget::new("example.com", 22, "user;whoami").is_err());
        assert!(LaunchTarget::new("example.com", 22, "").is_err());
        assert!(LaunchTarget::new("example.com", 0, "user").is_err());
    }
}
