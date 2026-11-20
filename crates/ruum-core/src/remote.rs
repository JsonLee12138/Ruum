//! Read-only, versioned SSH scan protocol.
//!
//! Paths travel as JSON on stdin,
//! never as shell arguments. Postorder records avoid JSON recursion limits
//! and stream without making a second serialized copy of the whole tree.

use std::io::{self, BufRead, BufReader, Read as _, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};

use serde::{Deserialize, Serialize};

use crate::remote_progress::{INTERVAL, Progress, Shared, Stage, stage};
use crate::scan::ScanOptions;
#[cfg(not(unix))]
use crate::scan::scan;
use crate::space::{SpaceInfo, space_info, volume_root_for};
use crate::tree::{Node, NodeKind, aggregate};
use std::time::Instant;

const VERSION: u32 = 1;
const MAX_LINE: u64 = 1 << 20;
const MAX_DIAGNOSTICS: usize = 16 << 10;

// Non-interactive shells often omit ~/.local/bin. Do not require users to
// edit the server's startup files after the documented user-local install.
// Wrapped in a single-line `/bin/sh -c '...'` with zero inner `'`, `\`, `!`,
// or newlines so any remote login shell delegates cleanly without touching
// `stdin`.
const REMOTE_COMMAND: &str = concat!(
    "/bin/sh -c '",
    "if command -v ruum-scan >/dev/null 2>&1; then ",
    "exec ruum-scan --stdio-v1; ",
    r#"elif [ -x "$HOME/.local/bin/ruum-scan" ]; then "#,
    r#"exec "$HOME/.local/bin/ruum-scan" --stdio-v1; "#,
    "else ",
    r#"cached=""; "#,
    r#"for f in "${XDG_CACHE_HOME:-$HOME/.cache}/ruum/ssh/"*/ruum-scan; "#,
    "do ",
    r#"if [ -x "$f" ] && [ -f "$f" ]; then "#,
    r#"if [ -z "$cached" ] || [ "$f" -nt "$cached" ]; then "#,
    r#"cached="$f"; "#,
    "fi; ",
    "fi; ",
    "done; ",
    r#"if [ -n "$cached" ]; then "#,
    r#"exec "$cached" --stdio-v1; "#,
    "fi; ",
    r#"echo "ruum-scan is not installed on this server." >&2; "#,
    r#"echo "On the server, run make install-scan "#,
    r#"from the ruum checkout." >&2; "#,
    "exit 127; ",
    "fi'",
);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub root: Option<PathBuf>,
    pub disk: bool,
    pub options: ScanOptions,
}

// Old helpers ignore this extra field. Old clients may still half-close
// stdin; only clients opting in promise to keep it open during the scan.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Operation {
    // No scan fields: legacy helpers reject discovery before any traversal,
    // instead of silently ignoring an unfamiliar operation and scanning ~.
    Discover {
        discover_version: u32,
    },
    // Required version field without `disk`/`options` so legacy helpers reject
    // `Space` cleanly before any traversal.
    Space {
        space_version: u32,
        root: Option<PathBuf>,
    },
    Scan(Request),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionRequest {
    #[serde(flatten)]
    pub operation: Operation,
    #[serde(default)]
    pub watch_disconnect: bool,
    // Opt-in keeps the postorder-only wire format for older clients.
    #[serde(default)]
    pub live_progress: bool,
    #[serde(default)]
    pub client_dedup: bool,
    #[serde(default)]
    pub live_tree: bool,
}

/// The target's root candidates, without walking or probing any of them.
#[derive(Debug, Serialize, Deserialize)]
pub struct Roots {
    version: u32,
    pub home: Option<PathBuf>,
    pub platform: String,
    pub identity: Option<crate::identity::HostIdentity>,
    pub volumes: Vec<crate::space::Volume>,
}

/// Reports whether `path` is absolute on the local host or on a remote POSIX
/// or Windows target.
pub fn is_remote_absolute(path: &Path) -> bool {
    if path.is_absolute() {
        return true;
    }
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    if bytes.starts_with(b"/") {
        return true;
    }
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

pub fn read_roots(input: &mut impl BufRead) -> io::Result<Roots> {
    let roots: Roots = receive(input)?;
    if roots.version != VERSION
        || roots
            .volumes
            .iter()
            .any(|volume| !is_remote_absolute(&volume.point))
        || roots
            .home
            .as_ref()
            .is_some_and(|home| !is_remote_absolute(home))
    {
        return Err(io::Error::other("incompatible remote root discovery"));
    }
    Ok(roots)
}

/// Single-line remote volume space report returned by [`Operation::Space`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceReport {
    version: u32,
    pub root: PathBuf,
    pub space: SpaceInfo,
    pub platform: String,
    #[serde(default)]
    pub identity: Option<crate::identity::HostIdentity>,
}

pub fn read_space_report(input: &mut impl BufRead) -> io::Result<SpaceReport> {
    let report: SpaceReport = receive(input)?;
    if report.version != VERSION || !is_remote_absolute(&report.root) {
        return Err(io::Error::other("incompatible remote space report"));
    }
    Ok(report)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    version: u32,
    pub root: PathBuf,
    pub home: Option<PathBuf>,
    pub disk_root: Option<PathBuf>,
    pub space: Option<SpaceInfo>,
    pub platform: String,
    #[serde(default)]
    pub identity: Option<crate::identity::HostIdentity>,
    #[serde(default)]
    pub docker: crate::docker_adapter::DockerProbeStatus,
}

impl Header {
    #[must_use]
    pub fn new(
        root: PathBuf,
        home: Option<PathBuf>,
        platform: impl Into<String>,
        identity: Option<crate::identity::HostIdentity>,
    ) -> Self {
        Self {
            version: VERSION,
            root,
            home,
            disk_root: None,
            space: None,
            platform: platform.into(),
            identity,
            docker: crate::docker_adapter::DockerProbeStatus::default(),
        }
    }
}

#[derive(Debug)]
pub struct Snapshot {
    pub header: Header,
    pub tree: Node,
    pub docker: crate::docker_adapter::DockerProbeStatus,
}

// Children are already on the receiver's stack; counts preserve empty dirs
// and deduplicated leaves without transmitting derived subtree totals.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Record(
    pub String,
    pub u8,
    pub u64,
    pub u64,
    pub i64,
    pub bool,
    pub usize,
);

#[derive(Deserialize)]
struct Control {
    directory: Option<PathBuf>,
    enter: Option<String>,
    entry: Option<Record>,
    link: Option<(u64, u64)>,
    docker: Option<crate::docker_adapter::DockerProbeStatus>,
}

fn json_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

pub(crate) fn send(
    out: &mut impl Write,
    value: &impl Serialize,
) -> io::Result<()> {
    serde_json::to_writer(&mut *out, value).map_err(json_error)?;
    out.write_all(b"\n")
}

fn receive<T: serde::de::DeserializeOwned>(
    input: &mut impl BufRead,
) -> io::Result<T> {
    let mut line = String::new();
    receive_line(input, &mut line)?;
    serde_json::from_str(&line).map_err(json_error)
}

fn receive_line(input: &mut impl BufRead, line: &mut String) -> io::Result<()> {
    line.clear();
    // A remote process is not trusted to allocate arbitrary amounts of RAM
    // with a single malformed record (or an accidental login banner).
    let length = input.take(MAX_LINE).read_line(line)?;
    if length == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "remote helper closed the connection without a complete scan",
        ));
    }
    if !line.ends_with('\n') {
        let reason = if length as u64 == MAX_LINE {
            "oversized remote record"
        } else {
            "truncated remote record"
        };
        return Err(io::Error::new(io::ErrorKind::InvalidData, reason));
    }
    Ok(())
}

/// Only read-only discovery and scans: no command execution or removal.
pub fn serve(input: &mut impl BufRead, out: &mut impl Write) -> io::Result<()> {
    serve_session(read_request(input)?, out)
}

/// Read one bounded request, including the opt-in connection lifetime.
pub fn read_request(input: &mut impl BufRead) -> io::Result<SessionRequest> {
    receive(input)
}

/// Dispatch without resolving any scan path until a scan was requested.
pub fn serve_session(
    session: SessionRequest,
    out: &mut impl Write,
) -> io::Result<()> {
    match session.operation {
        Operation::Scan(request) => serve_request_with_progress(
            request,
            out,
            session.live_progress,
            session.client_dedup,
            session.live_tree,
        ),
        Operation::Discover { discover_version } => {
            if discover_version != VERSION {
                return Err(io::Error::other(
                    "unsupported root discovery version",
                ));
            }
            send(
                out,
                &Roots {
                    version: VERSION,
                    home: std::env::home_dir(),
                    platform: std::env::consts::OS.into(),
                    identity: crate::identity::HostIdentity::current(),
                    volumes: crate::space::volume_roots()?,
                },
            )?;
            out.flush()
        }
        Operation::Space {
            space_version,
            root,
        } => {
            if space_version != VERSION {
                return Err(io::Error::other(
                    "unsupported space check version",
                ));
            }
            let home = std::env::home_dir();
            let resolved = match root.filter(|p| !p.as_os_str().is_empty()) {
                Some(path) => expand_remote_tilde_with(
                    path,
                    home.as_deref(),
                    cfg!(windows),
                )?,
                None => home.ok_or_else(|| {
                    io::Error::other("no remote home directory")
                })?,
            };
            let canonical = dunce::canonicalize(&resolved).unwrap_or(resolved);
            if !canonical.is_dir() {
                return Err(io::Error::other(
                    "space target is not a directory",
                ));
            }
            let space = space_info(&canonical)?;
            send(
                out,
                &SpaceReport {
                    version: VERSION,
                    root: canonical,
                    space,
                    platform: std::env::consts::OS.into(),
                    identity: crate::identity::HostIdentity::current(),
                },
            )?;
            out.flush()
        }
    }
}

/// Scan a decoded request. The standalone helper supervises its lifetime.
pub fn serve_request(request: Request, out: &mut impl Write) -> io::Result<()> {
    serve_request_with_progress(request, out, false, false, false)
}

fn expand_remote_tilde_with(
    path: PathBuf,
    home: Option<&Path>,
    allow_backslash: bool,
) -> io::Result<PathBuf> {
    let raw = path.to_string_lossy();
    let trimmed = raw.trim();
    if trimmed == "~" {
        return home
            .map(Path::to_path_buf)
            .ok_or_else(|| io::Error::other("no remote home directory"));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let base =
            home.ok_or_else(|| io::Error::other("no remote home directory"))?;
        return Ok(if rest.is_empty() {
            base.to_path_buf()
        } else {
            base.join(rest)
        });
    }
    if allow_backslash && let Some(rest) = trimmed.strip_prefix("~\\") {
        let base =
            home.ok_or_else(|| io::Error::other("no remote home directory"))?;
        return Ok(if rest.is_empty() {
            base.to_path_buf()
        } else {
            base.join(rest)
        });
    }
    Ok(path)
}

fn serve_request_with_progress(
    request: Request,
    out: &mut impl Write,
    live_progress: bool,
    client_dedup: bool,
    live_tree: bool,
) -> io::Result<()> {
    let _ = (live_progress, client_dedup, live_tree); // Only Unix has the streaming walker.
    let home = std::env::home_dir();
    let root = match request.root.filter(|p| !p.as_os_str().is_empty()) {
        Some(path) => {
            expand_remote_tilde_with(path, home.as_deref(), cfg!(windows))?
        }
        None => home
            .clone()
            .ok_or_else(|| io::Error::other("no remote home directory"))?,
    };
    let root = if request.disk {
        volume_root_for(&root).unwrap_or_else(|| PathBuf::from("/"))
    } else {
        root
    }
    .canonicalize()?;
    if !root.is_dir() {
        return Err(io::Error::other("scan root is not a directory"));
    }
    send(
        out,
        &Header {
            version: VERSION,
            disk_root: volume_root_for(&root),
            space: space_info(&root).ok(),
            root: root.clone(),
            home,
            platform: std::env::consts::OS.into(),
            identity: crate::identity::HostIdentity::current(),
            docker: crate::docker_adapter::DockerProbeStatus::default(),
        },
    )?;
    out.flush()?;
    let docker_probe = std::thread::Builder::new()
        .name("ruum-docker-probe".into())
        .spawn(|| {
            let env = crate::docker_adapter::DockerEnv::current();
            crate::docker_adapter::DockerFactory::create().probe(&env)
        })
        .ok();
    #[cfg(unix)]
    crate::remote_walk::stream(
        &root,
        &request.options,
        out,
        live_progress,
        client_dedup,
        live_tree,
    )?;
    #[cfg(not(unix))]
    write_node(out, &scan(&root, request.options)?)?;
    let docker = docker_probe
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    send(out, &serde_json::json!({ "docker": docker }))?;
    // An explicit terminator distinguishes a complete scan from a severed SSH
    // connection after any otherwise valid prefix of the postorder stream.
    send(out, &"done")?;
    out.flush()
}

#[cfg(not(unix))]
fn write_node(out: &mut impl Write, node: &Node) -> io::Result<()> {
    for child in &node.children {
        write_node(out, child)?;
    }
    let kind = match node.kind {
        NodeKind::Directory => 0,
        NodeKind::File => 1,
        NodeKind::Symlink => 2,
        NodeKind::Other => 3,
    };
    send(
        out,
        &Record(
            if node.name.contains('/') {
                "root".into()
            } else {
                node.name.to_string()
            },
            kind,
            node.own_bytes(),
            node.own_files(),
            node.modified,
            node.read_error,
            node.children.len(),
        ),
    )
}

pub fn read_snapshot(input: &mut impl BufRead) -> io::Result<Snapshot> {
    read_snapshot_with_progress(input, &Shared::default())
}

fn read_snapshot_with_progress(
    input: &mut impl BufRead,
    shared: &Shared,
) -> io::Result<Snapshot> {
    let mut header: Header = receive(input)?;
    if header.version != VERSION || !is_remote_absolute(&header.root) {
        return Err(io::Error::other("incompatible remote helper protocol"));
    }
    let mut docker = header.docker.clone();
    let mut progress = Progress {
        stage: Stage::Indexing,
        directory: Some(header.root.clone()),
        updated: Some(Instant::now()),
        ..Progress::default()
    };
    *shared.lock().expect("remote progress lock") = progress.clone();
    let mut published = Instant::now();
    let mut stack = Vec::<Node>::new();
    let mut line = String::new();
    let seen = crate::tree::Seen::new();
    let mut preview = crate::scan_preview::Builder::default();
    let mut ancestors = Vec::<String>::new();
    let mut preview_published = Instant::now();
    loop {
        receive_line(input, &mut line)?;
        let text = line.trim_start();
        // The common path is a typed tuple, not a heap-allocated JSON DOM
        // followed by another deserialization. Reuse the line allocation.
        let record: Record = if text.starts_with('[') {
            serde_json::from_str(text).map_err(json_error)?
        } else if text.starts_with('{') {
            let control: Control =
                serde_json::from_str(text).map_err(json_error)?;
            if let Some(name) = control.enter {
                if !(name.is_empty() && ancestors.is_empty())
                    && (name.is_empty()
                        || name == "."
                        || name == ".."
                        || name.contains(['/', '\0'])
                        || (cfg!(windows) && name.contains(['\\', ':'])))
                {
                    return Err(io::Error::other("invalid preview directory"));
                }
                ancestors.push(name);
                continue;
            }
            if let Some(directory) = control.directory {
                progress.directory = Some(directory);
                progress.updated = Some(Instant::now());
                *shared.lock().expect("remote progress lock") =
                    progress.clone();
                published = Instant::now();
                continue;
            }
            if let Some(probe) = control.docker {
                header.docker = probe.clone();
                docker = probe;
                continue;
            }
            let mut entry = control.entry.ok_or_else(|| {
                io::Error::other("invalid remote control record")
            })?;
            let identity = control.link.ok_or_else(|| {
                io::Error::other("missing remote hardlink identity")
            })?;
            if entry.1 != 1 || entry.6 != 0 {
                return Err(io::Error::other("invalid remote hardlink entry"));
            }
            if entry.2 > 0 && !seen.insert(identity) {
                entry.2 = 0;
            }
            entry
        } else if serde_json::from_str::<String>(text).map_err(json_error)?
            == "done"
        {
            break;
        } else {
            return Err(io::Error::other("invalid remote stream marker"));
        };
        let Record(name, kind, bytes, files, modified, read_error, count) =
            record;
        // Names must remain single components when joined to marked paths.
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains(['/', '\0'])
            || (cfg!(windows) && name.contains(['\\', ':']))
        {
            return Err(io::Error::other(format!(
                "remote name cannot be represented safely: {name:?}"
            )));
        }
        if count > stack.len() {
            return Err(io::Error::other(format!(
                "incomplete remote directory {name:?}: \
                 expected {count} children, received {}",
                stack.len(),
            )));
        }
        let kind = match kind {
            0 => NodeKind::Directory,
            1 => NodeKind::File,
            2 => NodeKind::Symlink,
            3 => NodeKind::Other,
            _ => return Err(io::Error::other("invalid remote node kind")),
        };
        if !kind.is_dir() && count != 0 {
            return Err(io::Error::other("remote leaf has children"));
        }
        // The final root often owns every stacked node. Transfer that
        // allocation instead of briefly duplicating a wide tree's storage.
        let children = if count == stack.len() {
            std::mem::take(&mut stack)
        } else {
            stack.split_off(stack.len() - count)
        };
        let mut node = Node::entry(name, kind, bytes);
        node.files = files;
        node.modified = modified;
        node.read_error = read_error;
        node.children = children;
        stack.push(node);
        let mut preview_changed = false;
        if !ancestors.is_empty() {
            if kind.is_dir() {
                ancestors.pop();
            } else {
                preview.add(ancestors.get(1).map(String::as_str), bytes, files);
                if progress.preview.is_none() {
                    progress.preview = Some(Arc::new(preview.snapshot()));
                    preview_changed = true;
                }
            }
        }
        progress.dirs += u64::from(kind.is_dir());
        // Legacy helpers include direct totals in directory records too.
        // Count each leaf once, never charge its parent's totals again.
        if !kind.is_dir() {
            progress.files += files;
            progress.bytes += bytes;
        }
        progress.errors += u64::from(read_error);
        if published.elapsed() >= INTERVAL || preview_changed {
            if progress.preview.is_some()
                && preview_published.elapsed().as_secs() >= 1
            {
                progress.preview = Some(Arc::new(preview.snapshot()));
                preview_published = Instant::now();
            }
            progress.updated = Some(Instant::now());
            *shared.lock().expect("remote progress lock") = progress.clone();
            published = Instant::now();
        }
    }
    if progress.preview.is_some() {
        progress.preview = Some(Arc::new(preview.snapshot()));
    }
    progress.stage = Stage::Finalizing;
    progress.updated = Some(Instant::now());
    *shared.lock().expect("remote progress lock") = progress;
    if stack.len() != 1 {
        return Err(io::Error::other("incomplete remote tree"));
    }
    let mut tree = stack.pop().expect("one root checked above");
    aggregate(&mut tree, crate::tree::Metric::Bytes);
    crate::classify::classify(&mut tree);
    Ok(Snapshot {
        header,
        tree,
        docker,
    })
}

pub fn validate_host(host: &str) -> io::Result<()> {
    if host.is_empty()
        || host.starts_with('-')
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._-:[]".contains(c))
    {
        return Err(io::Error::other("invalid SSH destination"));
    }
    Ok(())
}

fn read_diagnostics(mut input: impl io::Read) -> String {
    let mut tail = Vec::new();
    let mut chunk = [0_u8; 4096];
    while let Ok(length) = input.read(&mut chunk) {
        if length == 0 {
            break;
        }
        tail.extend_from_slice(&chunk[..length]);
        if tail.len() > MAX_DIAGNOSTICS {
            tail.drain(..tail.len() - MAX_DIAGNOSTICS);
        }
    }
    // Diagnostics are remote-controlled too; keep text, not terminal escapes.
    String::from_utf8_lossy(&tail)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
fn ssh_error(host: &str, details: &str, fallback: &io::Error) -> io::Error {
    ssh_error_with_context(host, details, fallback, true)
}

fn ssh_error_with_context(
    host: &str,
    details: &str,
    fallback: &io::Error,
    has_embedded_archive: bool,
) -> io::Error {
    let fallback_msg = fallback.to_string();
    let reason = if details.is_empty() {
        fallback_msg.clone()
    } else {
        details.to_owned()
    };
    let lower_reason = reason.to_ascii_lowercase();
    let missing_bin_sh = reason.contains("/bin/sh")
        && (lower_reason.contains("not found")
            || lower_reason.contains("no such file")
            || lower_reason.contains("not recognized"));
    let missing_ruum_scan = lower_reason.contains("ruum-scan")
        && (lower_reason.contains("not installed")
            || lower_reason.contains("not found")
            || lower_reason.contains("no such file"));
    let handshake_err = reason.contains("unexpected SSH deployment handshake")
        || reason.contains("incomplete SSH deployment handshake")
        || (!missing_ruum_scan
            && fallback_msg.contains("unexpected SSH deployment handshake"));
    let hint = if reason.contains("Permission denied")
        || reason.contains("Authentication failed")
    {
        format!(
            "Run ssh {host} in a terminal to verify your credentials. \
             When starting ruum from a terminal, SSH can ask for a \
             password there; GUI launches need an agent or SSH askpass."
        )
    } else if reason.contains("Host key verification failed") {
        format!("Run ssh {host} in a terminal and verify its host key first.")
    } else if missing_bin_sh {
        format!(
            "Remote scan on {host} requires Unix POSIX /bin/sh, which was \
             not found on the target host."
        )
    } else if handshake_err {
        format!(
            "Check remote startup files on {host} (such as \
             ~/.config/fish/config.fish, ~/.bashrc, or ~/.zshenv) for \
             non-interactive SSH stdout output."
        )
    } else if missing_ruum_scan {
        if has_embedded_archive {
            format!(
                "Install ruum-scan ON {host}, not on this computer. \
                 Run make install-scan from the checkout on the server."
            )
        } else {
            format!(
                "This local build does not embed the Linux ruum-scan scanner \
                 pack (for example, a build without Linux musl targets). \
                 Install ruum-scan ON {host}, not on this computer: run \
                 `make install-scan` from the ruum checkout on {host}."
            )
        }
    } else {
        "No incomplete scan is shown. Retry, or choose another root.".into()
    };
    io::Error::other(format!("SSH {host}: {reason}\n\n{hint}"))
}

/// Cancelling a rescan or closing a window kills SSH and closes the pipe.
///
/// The helper monitors stdin for EOF even before it has a tree to write;
/// keep the request pipe open until the entire snapshot has arrived.
#[derive(Debug)]
pub struct RemoteJob<T> {
    child: Arc<Mutex<Child>>,
    result: mpsc::Receiver<io::Result<T>>,
    progress: Shared,
}

pub type RemoteScan = RemoteJob<Snapshot>;
pub type RemoteRoots = RemoteJob<Roots>;

fn ssh_command(
    host: &str,
) -> io::Result<(Command, Option<crate::payload::Archive>)> {
    validate_host(host)?;
    let archive = crate::payload::Archive::current()?;
    let remote = if archive.is_some() {
        crate::deployment::COMMAND
    } else {
        REMOTE_COMMAND
    };
    let mut command = Command::new("ssh");
    command.args([
        "-T",
        "-C",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=2",
        "--",
        host,
        remote,
    ]);
    Ok((command, archive))
}

#[cfg(feature = "service")]
fn ssh_command_for_session(
    session: &crate::ssh_session::SshSession,
) -> io::Result<(Command, Option<crate::payload::Archive>)> {
    let archive = crate::payload::Archive::current()?;
    let remote = if archive.is_some() {
        crate::deployment::COMMAND
    } else {
        REMOTE_COMMAND
    };
    let command = session.build_ssh_command(&["-C"], remote)?;
    Ok((command, archive))
}

#[cfg(feature = "service")]
fn ssh_command_for_endpoint(
    endpoint: &crate::ssh_session::SshEndpoint,
) -> io::Result<(Command, Option<crate::payload::Archive>)> {
    let archive = crate::payload::Archive::current()?;
    let remote = if archive.is_some() {
        crate::deployment::COMMAND
    } else {
        REMOTE_COMMAND
    };
    let command = endpoint.build_ssh_command(&["-C"], remote);
    Ok((command, archive))
}

impl RemoteScan {
    pub fn spawn(host: &str, request: &Request) -> io::Result<Self> {
        let (command, archive) = ssh_command(host)?;
        Self::spawn_with_archive(command, host, request, archive)
    }

    #[cfg(feature = "service")]
    pub fn spawn_with_session(
        session: &crate::ssh_session::SshSession,
        request: &Request,
    ) -> io::Result<Self> {
        let (command, archive) = ssh_command_for_session(session)?;
        let label = session.endpoint().label();
        Self::spawn_with_archive(command, label, request, archive)
    }

    #[cfg(feature = "service")]
    pub fn spawn_with_endpoint(
        endpoint: &crate::ssh_session::SshEndpoint,
        request: &Request,
    ) -> io::Result<Self> {
        let (command, archive) = ssh_command_for_endpoint(endpoint)?;
        Self::spawn_with_archive(command, endpoint.label(), request, archive)
    }

    #[cfg(all(test, unix))]
    fn spawn_command(
        command: Command,
        host: &str,
        request: &Request,
    ) -> io::Result<Self> {
        Self::spawn_with_archive(command, host, request, None)
    }

    fn spawn_with_archive(
        command: Command,
        host: &str,
        request: &Request,
        archive: Option<crate::payload::Archive>,
    ) -> io::Result<Self> {
        let metric = request.options.metric;
        Self::start(
            command,
            host,
            Operation::Scan(request.clone()),
            archive,
            move |input, progress| {
                let mut snapshot =
                    read_snapshot_with_progress(input, progress)?;
                if metric != crate::tree::Metric::Bytes {
                    aggregate(&mut snapshot.tree, metric);
                }
                Ok(snapshot)
            },
        )
    }
}

impl RemoteRoots {
    pub fn discover(host: &str) -> io::Result<Self> {
        let (command, archive) = ssh_command(host)?;
        Self::start(
            command,
            host,
            Operation::Discover {
                discover_version: VERSION,
            },
            archive,
            |input, _| read_roots(input),
        )
    }

    #[cfg(feature = "service")]
    pub fn discover_with_session(
        session: &crate::ssh_session::SshSession,
    ) -> io::Result<Self> {
        let (command, archive) = ssh_command_for_session(session)?;
        Self::start(
            command,
            session.endpoint().label(),
            Operation::Discover {
                discover_version: VERSION,
            },
            archive,
            |input, _| read_roots(input),
        )
    }

    #[cfg(feature = "service")]
    pub fn discover_with_endpoint(
        endpoint: &crate::ssh_session::SshEndpoint,
    ) -> io::Result<Self> {
        let (command, archive) = ssh_command_for_endpoint(endpoint)?;
        Self::start(
            command,
            endpoint.label(),
            Operation::Discover {
                discover_version: VERSION,
            },
            archive,
            |input, _| read_roots(input),
        )
    }
}

impl<T: Send + 'static> RemoteJob<T> {
    fn start(
        mut command: Command,
        host: &str,
        operation: Operation,
        archive: Option<crate::payload::Archive>,
        decode: impl FnOnce(
            &mut BufReader<std::process::ChildStdout>,
            &Shared,
        ) -> io::Result<T>
        + Send
        + 'static,
    ) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = child.stderr.take().expect("piped stderr");
        // Drain concurrently so noisy login scripts cannot block discovery.
        let diagnostics = std::thread::spawn(move || read_diagnostics(stderr));
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let child = Arc::new(Mutex::new(child));
        let worker = child.clone();
        let host = host.to_owned();
        let (tx, result) = mpsc::channel();
        let progress = Shared::default();
        let live = progress.clone();
        let has_embedded_archive = archive.is_some();
        std::thread::spawn(move || {
            let mut input = BufReader::new(stdout);
            let response = (|| {
                if let Some(archive) = archive {
                    crate::deployment::deploy_with_progress(
                        &archive,
                        &mut input,
                        &mut stdin,
                        |value| stage(&live, value),
                    )?;
                }
                stage(&live, Stage::Starting);
                send(
                    &mut stdin,
                    &SessionRequest {
                        operation,
                        watch_disconnect: true,
                        live_progress: true,
                        client_dedup: true,
                        live_tree: true,
                    },
                )?;
                decode(&mut input, &live)
            })();
            drop(stdin);
            if response.is_err() {
                let _ = worker.lock().expect("SSH process lock").kill();
            }
            // Never hold the process lock across wait(): cancellation must
            // be able to acquire it even after a response was read.
            let status = loop {
                let status =
                    worker.lock().expect("SSH process lock").try_wait();
                match status {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) => {}
                    Err(error) => break Err(error),
                }
                std::thread::park_timeout(std::time::Duration::from_millis(10));
            };
            let details = diagnostics.join().unwrap_or_default();
            let outcome = match (response, status) {
                (Ok(response), Ok(status)) if status.success() => Ok(response),
                (Err(error), _) | (_, Err(error)) => {
                    Err(ssh_error_with_context(
                        &host,
                        &details,
                        &error,
                        has_embedded_archive,
                    ))
                }
                (_, Ok(status)) => Err(ssh_error_with_context(
                    &host,
                    &details,
                    &io::Error::other(format!("SSH exited with {status}")),
                    has_embedded_archive,
                )),
            };
            let _ = tx.send(outcome);
        });
        Ok(Self {
            child,
            result,
            progress,
        })
    }

    pub fn progress(&self) -> Progress {
        self.progress.lock().expect("remote progress lock").clone()
    }

    pub fn poll(&self) -> Option<io::Result<T>> {
        self.result.try_recv().ok()
    }

    pub fn wait(self) -> io::Result<T> {
        self.result.recv().unwrap_or_else(|_| {
            Err(io::Error::other("remote worker disconnected"))
        })
    }
}

impl<T> Drop for RemoteJob<T> {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// Lightweight remote volume space check using [`Operation::Space`], with a
/// protocol-only fallback to depth-0 [`Operation::Scan`] for older helpers.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoteSpace;

impl RemoteSpace {
    pub fn check(host: &str, root: Option<&Path>) -> io::Result<SpaceReport> {
        Self::check_with_jobs(
            || {
                let (command, archive) = ssh_command(host)?;
                Self::spawn_space(command, host, root, archive)
            },
            || RemoteScan::spawn(host, &Self::fallback_request(root)),
        )
    }

    #[cfg(feature = "service")]
    pub fn check_with_session(
        session: &crate::ssh_session::SshSession,
        root: Option<&Path>,
    ) -> io::Result<SpaceReport> {
        Self::check_with_jobs(
            || {
                let (command, archive) = ssh_command_for_session(session)?;
                Self::spawn_space(
                    command,
                    session.endpoint().label(),
                    root,
                    archive,
                )
            },
            || {
                RemoteScan::spawn_with_session(
                    session,
                    &Self::fallback_request(root),
                )
            },
        )
    }

    #[cfg(feature = "service")]
    pub fn check_with_endpoint(
        endpoint: &crate::ssh_session::SshEndpoint,
        root: Option<&Path>,
    ) -> io::Result<SpaceReport> {
        Self::check_with_jobs(
            || {
                let (command, archive) = ssh_command_for_endpoint(endpoint)?;
                Self::spawn_space(command, endpoint.label(), root, archive)
            },
            || {
                RemoteScan::spawn_with_endpoint(
                    endpoint,
                    &Self::fallback_request(root),
                )
            },
        )
    }

    fn fallback_request(root: Option<&Path>) -> Request {
        Request {
            root: root.map(Path::to_path_buf),
            disk: false,
            options: ScanOptions {
                max_depth: Some(0),
                ..ScanOptions::default()
            },
        }
    }

    fn spawn_space(
        command: Command,
        host: &str,
        root: Option<&Path>,
        archive: Option<crate::payload::Archive>,
    ) -> io::Result<RemoteJob<SpaceReport>> {
        RemoteJob::start(
            command,
            host,
            Operation::Space {
                space_version: VERSION,
                root: root.map(Path::to_path_buf),
            },
            archive,
            |input, _| read_space_report(input),
        )
    }

    fn check_with_jobs(
        spawn_space: impl FnOnce() -> io::Result<RemoteJob<SpaceReport>>,
        spawn_fallback: impl FnOnce() -> io::Result<RemoteScan>,
    ) -> io::Result<SpaceReport> {
        match spawn_space()?.wait() {
            Ok(report) => Ok(report),
            Err(err) if is_unsupported_space_error(&err) => {
                // Drain the entire depth-0 stream through `read_snapshot`
                // until `"done"` so the remote helper never hits SIGPIPE.
                let snapshot = spawn_fallback()?.wait()?;
                let space = snapshot.header.space.ok_or_else(|| {
                    io::Error::other("remote scan did not report volume space")
                })?;
                Ok(SpaceReport {
                    version: VERSION,
                    root: snapshot.header.root,
                    space,
                    platform: snapshot.header.platform,
                    identity: snapshot.header.identity,
                })
            }
            Err(err) => Err(err),
        }
    }
}

fn is_unsupported_space_error(err: &io::Error) -> bool {
    let lower = err.to_string().to_ascii_lowercase();
    let missing_bin_sh = lower.contains("/bin/sh")
        && (lower.contains("not found")
            || lower.contains("no such file")
            || lower.contains("not recognized"));
    let transport_or_auth = lower.contains("permission denied")
        || lower.contains("authentication failed")
        || lower.contains("host key verification failed")
        || lower.contains("remote host identification has changed")
        || lower.contains("not installed")
        || lower.contains("command not found")
        || lower.contains("unexpected ssh deployment handshake")
        || lower.contains("incomplete ssh deployment handshake")
        || lower.contains("requires unix posix /bin/sh")
        || missing_bin_sh
        || lower.contains("connection refused")
        || lower.contains("connection timed out")
        || lower.contains("operation timed out")
        || lower.contains("could not resolve hostname")
        || lower.contains("no route to host")
        || lower.contains("connection reset")
        || lower.contains("kex_exchange_identification");
    if transport_or_auth {
        return false;
    }
    lower.contains("did not match any variant of untagged enum operation")
        || lower.contains("missing field")
        || lower.contains("unknown variant")
        || lower.contains("unsupported space check version")
        || (lower.contains("invaliddata") && lower.contains("operation"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            root: None,
            disk: false,
            options: ScanOptions::default(),
        }
    }

    #[cfg(unix)]
    fn wait<T: Send + 'static>(scan: &RemoteJob<T>) -> io::Result<T> {
        wait_for(scan, std::time::Duration::from_secs(10))
    }

    fn wait_for<T: Send + 'static>(
        scan: &RemoteJob<T>,
        timeout: std::time::Duration,
    ) -> io::Result<T> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(result) = scan.poll() {
                return result;
            }
            assert!(std::time::Instant::now() < deadline, "SSH scan hung");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[cfg(unix)]
    fn failing_process(script: &str) -> io::Error {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        let handle =
            RemoteScan::spawn_command(command, "dev", &request()).unwrap();
        wait(&handle).unwrap_err()
    }

    #[cfg(unix)]
    #[test]
    fn remote_job_wait_returns_worker_outcome() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "read -r request; echo 'Permission denied' >&2; exit 255",
        ]);
        let handle =
            RemoteScan::spawn_command(command, "dev", &request()).unwrap();
        let err = handle.wait().unwrap_err().to_string();
        assert!(err.contains("Permission denied"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn ssh_authentication_diagnostics_reach_the_caller() {
        let error = failing_process(
            "read -r request; echo 'Permission denied (publickey,password).' \
             >&2; exit 255",
        )
        .to_string();
        assert!(error.contains("Permission denied"), "{error}");
        assert!(error.contains("verify your credentials"), "{error}");
        assert!(!error.contains("truncated"), "{error}");
        assert!(!error.contains("Install ruum-scan"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn missing_helper_diagnostics_name_the_server() {
        let error = failing_process(
            "read -r request; echo 'ruum-scan: command not found' \
             >&2; exit 127",
        )
        .to_string();
        assert!(error.contains("command not found"), "{error}");
        assert!(error.contains("ON dev, not on this computer"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn banner_and_truncated_output_are_reported_without_a_hang() {
        for output in ["hello", "hello\\n"] {
            let error =
                failing_process(&format!("read -r request; printf '{output}'"))
                    .to_string();
            assert!(error.contains("SSH dev:"), "{error}");
        }
    }

    #[test]
    fn discovery_is_not_a_legacy_scan_request() {
        let session = SessionRequest {
            operation: Operation::Discover {
                discover_version: VERSION,
            },
            watch_disconnect: true,
            live_progress: true,
            client_dedup: true,
            live_tree: true,
        };
        let bytes = serde_json::to_vec(&session).unwrap();
        assert!(serde_json::from_slice::<Request>(&bytes).is_err());
        let mut input = bytes;
        input.push(b'\n');
        let mut output = Vec::new();
        serve(&mut input.as_slice(), &mut output).unwrap();
        let roots = read_roots(&mut output.as_slice()).unwrap();
        assert!(roots.volumes.iter().all(|volume| volume.space.is_none()));
        assert!(!roots.platform.is_empty());
        assert!(roots.identity.is_some());
    }

    #[test]
    fn discovery_never_resolves_a_scan_path_and_rejects_unknown_versions() {
        let mut output = Vec::new();
        // No scan fields: an old helper fails to decode this, rather than
        // falling back to its default home-directory walk.
        serve(&mut b"{\"discover_version\":1,\"root\":\"/does-not-exist-ruum-test\"}\n".as_slice(),
            &mut output).unwrap();
        assert!(read_roots(&mut output.as_slice()).is_ok());
        assert!(
            serve(
                &mut b"{\"discover_version\":99}\n".as_slice(),
                &mut Vec::new()
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_discovery_records_are_not_accepted_as_roots() {
        for value in [
            serde_json::json!({"version":99,"home":null,"platform":"linux","identity":null,"volumes":[]}),
            serde_json::json!({"version":1,"home":null,"platform":"linux","identity":null,"volumes":[{"point":"relative","device":null,"space":null}]}),
        ] {
            let mut data = serde_json::to_vec(&value).unwrap();
            data.push(b'\n');
            assert!(read_roots(&mut data.as_slice()).is_err());
        }
    }

    #[test]
    fn diagnostics_are_bounded_and_controls_are_stripped() {
        let mut data = vec![b'x'; MAX_DIAGNOSTICS * 4];
        data.extend_from_slice(b"\x1b[31mPermission denied\r\n");
        let text = read_diagnostics(data.as_slice());
        assert!(text.len() <= MAX_DIAGNOSTICS);
        assert!(text.ends_with("Permission denied"));
        assert!(!text.contains(['\x1b', '\r']));
    }

    // Uses an explicitly supplied config, never the user's default SSH host.
    #[cfg(unix)]
    #[test]
    #[ignore = "requires RUUM_TEST_SSH_CONFIG with isolated sshd"]
    fn real_ssh_transport_returns_a_snapshot() {
        let config = std::env::var("RUUM_TEST_SSH_CONFIG").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep"), b"hello").unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let helper = std::env::var("RUUM_TEST_HELPER").unwrap();
        std::os::unix::fs::symlink(helper, bin.join("ruum-scan")).unwrap();
        // Force the documented ~/.local/bin fallback, not the ambient PATH.
        let remote = format!(
            "HOME={} PATH=/nonexistent; export HOME PATH;\n{REMOTE_COMMAND}",
            crate::export::shell_quote(home.path().to_str().unwrap()),
        );
        let mut command = Command::new("ssh");
        command.args(["-F", &config, "-T", "-C", "fixture", &remote]);
        let request = Request {
            root: Some(dir.path().into()),
            options: ScanOptions {
                apparent_size: true,
                ..ScanOptions::default()
            },
            ..request()
        };
        let handle =
            RemoteScan::spawn_command(command, "fixture", &request).unwrap();
        let snapshot = wait(&handle).unwrap();
        assert_eq!(snapshot.tree.bytes, 5);
        assert_eq!(snapshot.tree.files, 1);
        assert_eq!(std::fs::read(dir.path().join("keep")).unwrap(), b"hello");
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires RUUM_TEST_SSH_CONFIG with isolated sshd"]
    fn real_ssh_transport_reports_auth_and_install_errors() {
        let config = std::env::var("RUUM_TEST_SSH_CONFIG").unwrap();
        let mut denied = Command::new("ssh");
        denied.args([
            "-F",
            &config,
            "-T",
            "-o",
            "PubkeyAuthentication=no",
            "-o",
            "PreferredAuthentications=none",
            "fixture",
            REMOTE_COMMAND,
        ]);
        let scan =
            RemoteScan::spawn_command(denied, "fixture", &request()).unwrap();
        let error = wait(&scan).unwrap_err().to_string();
        assert!(error.contains("Permission denied"), "{error}");

        let home = tempfile::tempdir().unwrap();
        let remote = format!(
            "HOME={} PATH=/nonexistent; export HOME PATH;\n{REMOTE_COMMAND}",
            crate::export::shell_quote(home.path().to_str().unwrap()),
        );
        let mut missing = Command::new("ssh");
        missing.args(["-F", &config, "-T", "fixture", &remote]);
        let scan =
            RemoteScan::spawn_command(missing, "fixture", &request()).unwrap();
        let error = wait(&scan).unwrap_err().to_string();
        assert!(error.contains("not installed on this server"), "{error}");
        assert!(error.contains("ON fixture"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires isolated sshd and RUUM_TEST_ARCHIVE"]
    fn real_ssh_transport_bootstraps_and_checks_cached_payloads() {
        let config = std::env::var("RUUM_TEST_SSH_CONFIG").unwrap();
        let archive = crate::payload::Archive::open(
            PathBuf::from(std::env::var("RUUM_TEST_ARCHIVE").unwrap())
                .as_path(),
        )
        .unwrap()
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("keep"), b"hello").unwrap();
        // Exercise the shipped static helper beyond the removed 256-level
        // cutoff, through deployment and decoding, not just the walker.
        let mut deep = data.path().to_path_buf();
        for _ in 0..320 {
            deep.push("d");
            std::fs::create_dir(&deep).unwrap();
        }
        std::fs::write(deep.join("leaf"), b"world").unwrap();
        let cache = home.path().join("cache");
        let remote = format!(
            "HOME={} XDG_CACHE_HOME={}; export HOME XDG_CACHE_HOME;\n{}",
            crate::export::shell_quote(home.path().to_str().unwrap()),
            crate::export::shell_quote(cache.to_str().unwrap()),
            crate::deployment::COMMAND,
        );
        let arch = std::env::consts::ARCH;
        let (entry, bytes) = archive.load(arch).unwrap();
        let cached =
            cache.join("ruum/ssh").join(&entry.sha256).join("ruum-scan");
        let request = Request {
            root: Some(data.path().into()),
            options: ScanOptions {
                apparent_size: true,
                ..ScanOptions::default()
            },
            ..request()
        };
        for attempt in 0..3 {
            let mut command = Command::new("ssh");
            command.args(["-F", &config, "-T", "fixture", &remote]);
            let handle = RemoteScan::spawn_with_archive(
                command,
                "fixture",
                &request,
                Some(archive.clone()),
            )
            .unwrap();
            let snapshot = wait(&handle).unwrap();
            assert_eq!(snapshot.tree.bytes, 10);
            assert_eq!(snapshot.tree.files, 2);
            assert_eq!(handle.progress().files, 2);
            assert!(snapshot.header.identity.is_some());
            assert_eq!(std::fs::read(&cached).unwrap(), bytes);
            if attempt == 1 {
                // A damaged cache must be repaired, never executed.
                let damaged = cached.with_extension("damaged");
                std::fs::write(&damaged, b"broken").unwrap();
                std::fs::rename(damaged, &cached).unwrap();
            }
        }
        assert!(!home.path().join(".local/bin/ruum-scan").exists());
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires isolated sshd and RUUM_TEST_ARCHIVE"]
    fn real_ssh_transport_discovers_roots_without_a_scan() {
        let config = std::env::var("RUUM_TEST_SSH_CONFIG").unwrap();
        let archive = crate::payload::Archive::open(
            PathBuf::from(std::env::var("RUUM_TEST_ARCHIVE").unwrap())
                .as_path(),
        )
        .unwrap()
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let cache = home.path().join("cache");
        let remote = format!(
            "HOME={} XDG_CACHE_HOME={}; export HOME XDG_CACHE_HOME;\n{}",
            crate::export::shell_quote(home.path().to_str().unwrap()),
            crate::export::shell_quote(cache.to_str().unwrap()),
            crate::deployment::COMMAND,
        );
        let mut command = Command::new("ssh");
        command.args(["-F", &config, "-T", "fixture", &remote]);
        let job = RemoteRoots::start(
            command,
            "fixture",
            Operation::Discover {
                discover_version: VERSION,
            },
            Some(archive),
            |input, _| read_roots(input),
        )
        .unwrap();
        let roots = wait(&job).unwrap();
        assert_eq!(roots.home.as_deref(), Some(home.path()));
        assert!(roots.identity.is_some());
        assert!(
            roots
                .volumes
                .iter()
                .all(|volume| volume.point.is_absolute()
                    && volume.space.is_none())
        );
        // Only the checked cache was installed, not a per-root scan log,
        // helper in PATH, or a traversed home tree.
        assert!(!home.path().join(".local").exists());
    }

    /// Explicit opt-in only: never contact a real machine in the test gate.
    #[test]
    #[ignore = "requires RUUM_TEST_SSH_HOST and RUUM_TEST_SSH_ROOT"]
    fn live_ssh_scan_uses_the_production_transport() {
        let host = std::env::var("RUUM_TEST_SSH_HOST").unwrap();
        let root = std::env::var("RUUM_TEST_SSH_ROOT").unwrap();
        let scan = RemoteScan::spawn(
            &host,
            &Request {
                root: Some(PathBuf::from(&root)),
                ..request()
            },
        )
        .unwrap();
        let snapshot =
            wait_for(&scan, std::time::Duration::from_mins(3)).unwrap();
        assert!(snapshot.tree.is_dir());
        assert!(snapshot.header.root.is_absolute());
        println!(
            "SSH {}: {} — {} bytes, {} files, {} directories; space: {:?}",
            host,
            snapshot.header.root.display(),
            snapshot.tree.bytes,
            snapshot.tree.files,
            snapshot.tree.dirs,
            snapshot.header.space,
        );
    }

    #[test]
    fn rejects_ssh_options_and_shell_syntax() {
        for host in ["-oProxyCommand=oops", "dev; touch x", "dev\nother", ""] {
            assert!(validate_host(host).is_err());
        }
        for host in ["dev", "user@dev", "[::1]"] {
            assert!(validate_host(host).is_ok());
        }
    }

    #[cfg(feature = "service")]
    #[test]
    fn session_and_endpoint_commands_avoid_ssh_config_and_pass_session_env() {
        use crate::ssh_session::{SshAuthMaterial, SshEndpoint, SshSession};
        use zeroize::Zeroizing;

        let endpoint = SshEndpoint::new("10.1.2.3", 2222, "ops").unwrap();
        let (ep_cmd, _) = ssh_command_for_endpoint(&endpoint).unwrap();
        let ep_args: Vec<String> = ep_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(ep_args.windows(2).any(|w| w == ["-F", "none"]));
        assert!(ep_args.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(ep_args.contains(&"-C".to_owned()));
        assert!(ep_args.contains(&"ops@10.1.2.3".to_owned()));

        let mut session = SshSession::start(
            endpoint,
            SshAuthMaterial::Password {
                password: Zeroizing::new("pw".to_owned()),
            },
            15,
        )
        .unwrap();
        let (sess_cmd, _) = ssh_command_for_session(&session).unwrap();
        let sess_args: Vec<String> = sess_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(sess_args.windows(2).any(|w| w == ["-F", "none"]));
        assert!(sess_args.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(sess_args.contains(&"-C".to_owned()));
        session.close();
        assert!(ssh_command_for_session(&session).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unix_backslashes_in_names_roundtrip_without_becoming_separators() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(r"back\slash"), b"x").unwrap();
        let request = Request {
            root: Some(dir.path().into()),
            disk: false,
            options: ScanOptions {
                apparent_size: true,
                ..ScanOptions::default()
            },
        };
        let mut input = Vec::new();
        send(&mut input, &request).unwrap();
        let mut data = Vec::new();
        serve(&mut input.as_slice(), &mut data).unwrap();
        let snapshot = read_snapshot(&mut data.as_slice()).unwrap();
        assert_eq!(snapshot.tree.bytes, 1);
        assert!(snapshot.tree.child_named(r"back\slash").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn live_counters_are_available_before_the_tree_finishes() {
        use std::os::unix::net::UnixStream;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), b"hello").unwrap();
        let request = Request {
            root: Some(dir.path().into()),
            disk: false,
            options: ScanOptions {
                apparent_size: true,
                ..ScanOptions::default()
            },
        };
        let mut data = Vec::new();
        serve_request(request, &mut data).unwrap();
        let mut lines = data.split_inclusive(|byte| *byte == b'\n');
        let header = lines.next().unwrap();
        let leaf = lines.next().unwrap();
        let tail = lines.flatten().copied().collect::<Vec<_>>();
        let (mut output, input) = UnixStream::pair().unwrap();
        input
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let shared = Shared::default();
        let live = shared.clone();
        let worker = std::thread::spawn(move || {
            read_snapshot_with_progress(&mut BufReader::new(input), &live)
        });
        output.write_all(header).unwrap();
        send(&mut output, &serde_json::json!({"enter": ""})).unwrap();
        output.write_all(leaf).unwrap();
        send(&mut output, &serde_json::json!({"directory": dir.path()}))
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let progress = shared.lock().unwrap().clone();
            if progress.files == 1 {
                assert_eq!(progress.bytes, 5);
                assert_eq!(progress.stage, Stage::Indexing);
                let preview =
                    progress.preview.as_ref().expect("preview before done");
                assert_eq!(preview.entries[0].bytes, 5);
                assert_eq!(preview.entries[0].name, "Files in root");
                assert!(!worker.is_finished());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        output.write_all(&tail).unwrap();
        let snapshot = worker.join().unwrap().unwrap();
        assert_eq!(snapshot.tree.bytes, 5);
        assert_eq!(shared.lock().unwrap().stage, Stage::Finalizing);
    }

    #[test]
    fn desktop_dedup_counts_links_once_without_requiring_external_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("first"), b"hello").unwrap();
        std::fs::hard_link(root.join("first"), root.join("alias")).unwrap();
        std::fs::hard_link(root.join("first"), dir.path().join("outside"))
            .unwrap();
        std::fs::create_dir_all(root.join("nested/deeper")).unwrap();
        std::fs::write(root.join("nested/deeper/file"), b"world").unwrap();
        for dedup in [true, false] {
            let session = SessionRequest {
                operation: Operation::Scan(Request {
                    root: Some(root.clone()),
                    disk: false,
                    options: ScanOptions {
                        apparent_size: true,
                        dedup_hardlinks: dedup,
                        ..ScanOptions::default()
                    },
                }),
                live_progress: true,
                client_dedup: true,
                live_tree: true,
                watch_disconnect: false,
            };
            let mut data = Vec::new();
            serve_session(session, &mut data).unwrap();
            let shared = Shared::default();
            let snapshot =
                read_snapshot_with_progress(&mut data.as_slice(), &shared)
                    .unwrap();
            assert_eq!(snapshot.tree.bytes, if dedup { 10 } else { 15 });
            assert_eq!(snapshot.tree.files, 3);
            if cfg!(unix) {
                let preview = shared.lock().unwrap().preview.clone().unwrap();
                let nested = preview
                    .entries
                    .iter()
                    .find(|entry| entry.name == "nested")
                    .unwrap();
                assert_eq!(nested.bytes, 5);
                assert_eq!(nested.files, 1);
                assert_eq!(
                    preview.entries.iter().map(|e| e.bytes).sum::<u64>(),
                    snapshot.tree.bytes
                );
            }
        }
    }

    #[test]
    fn desktop_dedup_accepts_more_than_the_old_pending_link_limit() {
        let mut data = Vec::new();
        send(
            &mut data,
            &Header {
                version: VERSION,
                root: std::env::temp_dir().join("links-test"),
                home: None,
                disk_root: None,
                space: None,
                platform: std::env::consts::OS.into(),
                identity: None,
                docker: crate::docker_adapter::DockerProbeStatus::default(),
            },
        )
        .unwrap();
        let count = 65_537_u64;
        for index in 0..=count {
            let inode = if index == count { 42 } else { index };
            send(
                &mut data,
                &serde_json::json!({
                    "link": [7, inode],
                    "entry": [format!("file-{index}"), 1, 1, 1, 0, false, 0],
                }),
            )
            .unwrap();
        }
        send(
            &mut data,
            &Record(
                "root".into(),
                0,
                0,
                0,
                0,
                false,
                usize::try_from(count + 1).unwrap(),
            ),
        )
        .unwrap();
        send(&mut data, &"done").unwrap();
        let snapshot = read_snapshot(&mut data.as_slice()).unwrap();
        assert_eq!(snapshot.tree.files, count + 1);
        assert_eq!(snapshot.tree.bytes, count);
    }

    #[test]
    fn protocol_roundtrip_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a ' file"), b"hello").unwrap();
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        let request = Request {
            root: Some(dir.path().into()),
            disk: false,
            options: ScanOptions {
                apparent_size: true,
                ..ScanOptions::default()
            },
        };
        let mut data = Vec::new();
        let mut input = Vec::new();
        send(&mut input, &request).unwrap();
        serve(&mut input.as_slice(), &mut data).unwrap();
        assert!(!data.windows(11).any(|part| part == b"\"directory\""));
        let snapshot = read_snapshot(&mut data.as_slice()).unwrap();
        assert_eq!(snapshot.tree.bytes, 5);
        assert_eq!(snapshot.tree.files, 1);
        assert!(snapshot.tree.child_named("empty").unwrap().is_dir());
        data.truncate(data.len() - 7);
        assert!(read_snapshot(&mut data.as_slice()).is_err());
        assert!(dir.path().join("a ' file").exists());
    }

    #[test]
    fn recognizes_remote_absolute_paths_across_posix_and_windows() {
        for valid in ["/only-on-the-remote-host", "/", "C:\\Users", "D:/data"] {
            assert!(is_remote_absolute(Path::new(valid)), "{valid}");
        }
        for invalid in ["relative", "C:foo", ""] {
            assert!(!is_remote_absolute(Path::new(invalid)), "{invalid}");
        }
    }

    #[test]
    fn read_roots_and_snapshot_accept_cross_platform_absolute_paths() {
        let roots = Roots {
            version: VERSION,
            home: Some(PathBuf::from("/home/remote")),
            platform: "linux".into(),
            identity: None,
            volumes: vec![
                crate::space::Volume {
                    point: PathBuf::from("/"),
                    device: Some("/dev/sda1".into()),
                    space: None,
                },
                crate::space::Volume {
                    point: PathBuf::from("C:\\"),
                    device: Some("NTFS".into()),
                    space: None,
                },
            ],
        };
        let mut roots_wire = Vec::new();
        send(&mut roots_wire, &roots).unwrap();
        let decoded_roots = read_roots(&mut roots_wire.as_slice()).unwrap();
        assert_eq!(decoded_roots.volumes.len(), 2);

        for remote_root in ["/only-on-the-remote-host", "D:/data"] {
            let mut snap_wire = Vec::new();
            send(
                &mut snap_wire,
                &Header::new(PathBuf::from(remote_root), None, "linux", None),
            )
            .unwrap();
            send(&mut snap_wire, &Record("root".into(), 0, 0, 0, 0, false, 0))
                .unwrap();
            send(&mut snap_wire, &"done").unwrap();
            let snapshot = read_snapshot(&mut snap_wire.as_slice()).unwrap();
            assert_eq!(snapshot.header.root, PathBuf::from(remote_root));
        }
    }

    #[test]
    fn expand_remote_tilde_handles_home_subpaths_and_absolute_paths() {
        let home = Path::new("/home/remote");
        assert_eq!(
            expand_remote_tilde_with(PathBuf::from("~"), Some(home), false)
                .unwrap(),
            PathBuf::from("/home/remote")
        );
        assert_eq!(
            expand_remote_tilde_with(
                PathBuf::from("~/projects/ruum"),
                Some(home),
                false,
            )
            .unwrap(),
            PathBuf::from("/home/remote/projects/ruum")
        );
        assert_eq!(
            expand_remote_tilde_with(PathBuf::from("~/"), Some(home), false)
                .unwrap(),
            PathBuf::from("/home/remote")
        );
        assert_eq!(
            expand_remote_tilde_with(
                PathBuf::from("~\\Downloads"),
                Some(home),
                true,
            )
            .unwrap(),
            PathBuf::from("/home/remote/Downloads")
        );
        assert_eq!(
            expand_remote_tilde_with(
                PathBuf::from("/var/log"),
                Some(home),
                false,
            )
            .unwrap(),
            PathBuf::from("/var/log")
        );
        assert!(
            expand_remote_tilde_with(PathBuf::from("~"), None, false).is_err()
        );
    }

    #[test]
    fn serve_request_expands_tilde_against_home_dir() {
        let Some(real_home) = std::env::home_dir() else {
            return;
        };
        if !real_home.is_dir() {
            return;
        }
        // Only test `~` expansion on the Header without walking a large real
        // home directory: pass max_depth = 0!
        let request = Request {
            root: Some(PathBuf::from("~")),
            disk: false,
            options: ScanOptions {
                max_depth: Some(0),
                ..ScanOptions::default()
            },
        };
        let mut out = Vec::new();
        serve_request(request, &mut out).unwrap();
        let snapshot = read_snapshot(&mut out.as_slice()).unwrap();
        assert_eq!(
            snapshot.header.root,
            real_home.canonicalize().unwrap_or(real_home)
        );
    }

    #[test]
    fn space_operation_is_distinct_from_scan_request() {
        let session = SessionRequest {
            operation: Operation::Space {
                space_version: VERSION,
                root: None,
            },
            watch_disconnect: true,
            live_progress: true,
            client_dedup: true,
            live_tree: true,
        };
        let bytes = serde_json::to_vec(&session).unwrap();
        assert!(serde_json::from_slice::<Request>(&bytes).is_err());

        let tmp = tempfile::tempdir().unwrap();
        let dir_req = SessionRequest {
            operation: Operation::Space {
                space_version: VERSION,
                root: Some(tmp.path().to_path_buf()),
            },
            watch_disconnect: true,
            live_progress: true,
            client_dedup: true,
            live_tree: true,
        };
        let mut input = serde_json::to_vec(&dir_req).unwrap();
        input.push(b'\n');
        let mut output = Vec::new();
        serve(&mut input.as_slice(), &mut output).unwrap();
        let report = read_space_report(&mut output.as_slice()).unwrap();
        assert!(report.space.total > 0);
        assert_eq!(
            report.root,
            dunce::canonicalize(tmp.path())
                .unwrap_or_else(|_| tmp.path().into())
        );

        assert!(
            serve(
                &mut b"{\"space_version\":99,\"root\":null}\n".as_slice(),
                &mut Vec::new(),
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn serve_space_succeeds_on_unreadable_directory_without_root() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(
            &locked,
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        let req = SessionRequest {
            operation: Operation::Space {
                space_version: VERSION,
                root: Some(locked.clone()),
            },
            watch_disconnect: true,
            live_progress: false,
            client_dedup: false,
            live_tree: false,
        };
        let mut input = serde_json::to_vec(&req).unwrap();
        input.push(b'\n');
        let mut output = Vec::new();
        let res = serve(&mut input.as_slice(), &mut output);
        std::fs::set_permissions(
            &locked,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        res.expect("space check must succeed without read_dir permission");
        let report = read_space_report(&mut output.as_slice()).unwrap();
        assert!(report.space.total > 0);
    }

    #[cfg(unix)]
    #[test]
    fn remote_space_falls_back_only_on_protocol_rejection_and_drains_stream() {
        let dir = tempfile::tempdir().unwrap();
        let fallback_drained = dir.path().join("fallback_drained");
        let fallback_called = dir.path().join("fallback_called");

        // 1. Direct Operation::Space success never spawns fallback Scan.
        let ok_report = RemoteSpace::check_with_jobs(
            || {
                let mut cmd = Command::new("sh");
                cmd.args([
                    "-c",
                    "read -r _req; \
                     printf '{\"version\":1,\"root\":\"/home/dev\",\"space\":{\"total\":1000,\"free\":400,\"available\":350},\"platform\":\"linux\",\"identity\":null}\\n'",
                ]);
                RemoteSpace::spawn_space(cmd, "dev", None, None)
            },
            || {
                std::fs::write(&fallback_called, b"should-not-run").unwrap();
                unreachable!("fallback must not run when Space succeeds");
            },
        )
        .unwrap();
        assert_eq!(ok_report.space.available, 350);
        assert!(!fallback_called.exists());

        // 2. Old helper rejecting Space protocol triggers depth-0 Scan fallback
        //    and drains the entire stream to `"done"` (no SIGPIPE).
        let drained_path = fallback_drained.display().to_string();
        let fallback_report = RemoteSpace::check_with_jobs(
            || {
                let mut cmd = Command::new("sh");
                cmd.args([
                    "-c",
                    "read -r _req; \
                     echo 'Error: Custom { kind: InvalidData, error: Error(\"data did not match any variant of untagged enum Operation\", line: 1, column: 2) }' >&2; \
                     exit 1",
                ]);
                RemoteSpace::spawn_space(cmd, "dev", None, None)
            },
            || {
                let mut cmd = Command::new("sh");
                let script = format!(
                    "read -r req; \
                     case \"$req\" in *'\"max_depth\":0'*) ;; *) exit 2;; esac; \
                     printf '{{\"version\":1,\"root\":\"/home/dev\",\"home\":\"/home/dev\",\"disk_root\":\"/\",\"space\":{{\"total\":2000,\"free\":900,\"available\":800}},\"platform\":\"linux\",\"identity\":null}}\\n'; \
                     printf '[\"child\",1,128,1,0,false,0]\\n'; \
                     printf '[\"dev\",0,0,0,0,false,1]\\n'; \
                     printf '\"done\"\\n'; \
                     printf 'drained\\n' > '{drained_path}'"
                );
                cmd.args(["-c", &script]);
                RemoteScan::spawn_command(cmd, "dev", &RemoteSpace::fallback_request(None))
            },
        )
        .unwrap();
        assert_eq!(fallback_report.space.available, 800);
        assert_eq!(fallback_report.root, PathBuf::from("/home/dev"));
        assert!(fallback_drained.exists());

        // 3. SSH auth / host key / network / missing helper errors NEVER fall back.
        for err_stderr in [
            "Permission denied (publickey,password).",
            "Host key verification failed.",
            "ssh: connect to host 10.0.0.1 port 22: Connection refused",
            "ruum-scan: command not found",
            "/bin/sh: not found",
        ] {
            let err = RemoteSpace::check_with_jobs(
                || {
                    let mut cmd = Command::new("sh");
                    cmd.args([
                        "-c",
                        &format!(
                            "read -r _req; echo '{err_stderr}' >&2; exit 255"
                        ),
                    ]);
                    RemoteSpace::spawn_space(cmd, "dev", None, None)
                },
                || {
                    std::fs::write(&fallback_called, b"must-not-run").unwrap();
                    unreachable!(
                        "must never fall back on SSH/transport errors"
                    );
                },
            )
            .unwrap_err();
            assert!(!err.to_string().is_empty());
            assert!(!fallback_called.exists(), "fallback ran for {err_stderr}");
        }
    }

    #[test]
    fn docker_control_frame_updates_both_snapshot_and_header_with_legacy_compat()
     {
        use crate::docker_adapter::{
            DockerImageEntry, DockerProbeStatus, DockerUsageSummary,
        };

        // 1. Legacy helper omits `docker` from Header and sends no `{"docker": ...}` frame.
        let mut legacy_wire = Vec::new();
        legacy_wire.extend_from_slice(
            br#"{"version":1,"root":"/srv/app","home":"/home/dev","disk_root":"/","space":null,"platform":"linux","identity":null}"#,
        );
        legacy_wire.push(b'\n');
        send(
            &mut legacy_wire,
            &Record("root".into(), 0, 0, 0, 0, false, 0),
        )
        .unwrap();
        send(&mut legacy_wire, &"done").unwrap();
        let legacy_snap = read_snapshot(&mut legacy_wire.as_slice()).unwrap();
        assert_eq!(legacy_snap.docker, DockerProbeStatus::NotInstalled);
        assert_eq!(legacy_snap.header.docker, DockerProbeStatus::NotInstalled);

        // 2. Modern helper emits `{"docker": ...}` before `"done"`, updating
        //    both `snapshot.docker` and `snapshot.header.docker`.
        let summary = DockerUsageSummary {
            build_cache_reclaimable_bytes: 2048,
            removable_images: vec![DockerImageEntry {
                id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                repository: "<none>".into(),
                tag: "<none>".into(),
                created_since: "3d".into(),
                size_bytes: 4096,
                unique_bytes: 4096,
                dangling: true,
            }],
            detailed: true,
            ..DockerUsageSummary::default()
        };
        let expected = DockerProbeStatus::Available(summary);
        let mut modern_wire = Vec::new();
        send(
            &mut modern_wire,
            &Header::new(PathBuf::from("/srv/app"), None, "linux", None),
        )
        .unwrap();
        send(
            &mut modern_wire,
            &Record("root".into(), 0, 0, 0, 0, false, 0),
        )
        .unwrap();
        send(&mut modern_wire, &serde_json::json!({ "docker": expected }))
            .unwrap();
        send(&mut modern_wire, &"done").unwrap();
        let modern_snap = read_snapshot(&mut modern_wire.as_slice()).unwrap();
        assert_eq!(modern_snap.docker, expected);
        assert_eq!(modern_snap.header.docker, expected);
    }

    #[test]
    fn serve_request_emits_docker_control_frame_before_done() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), b"123").unwrap();
        let request = Request {
            root: Some(dir.path().to_path_buf()),
            disk: false,
            options: ScanOptions {
                apparent_size: true,
                ..ScanOptions::default()
            },
        };
        let mut out = Vec::new();
        serve_request(request, &mut out).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() >= 3);
        let docker_line = lines[lines.len() - 2];
        assert!(
            docker_line.contains("\"docker\""),
            "expected docker control frame before done, got: {docker_line}"
        );
        assert_eq!(lines[lines.len() - 1], "\"done\"");
        let snapshot = read_snapshot(&mut out.as_slice()).unwrap();
        assert_eq!(snapshot.header.docker, snapshot.docker);
    }

    #[test]
    fn remote_command_is_single_line_bin_sh_c_without_outer_shell_hazards() {
        assert!(
            REMOTE_COMMAND.starts_with("/bin/sh -c '"),
            "REMOTE_COMMAND must start with /bin/sh -c ': {REMOTE_COMMAND}"
        );
        assert!(
            REMOTE_COMMAND.ends_with('\''),
            "REMOTE_COMMAND must end with single quote: {REMOTE_COMMAND}"
        );
        assert!(
            !REMOTE_COMMAND.contains(['\n', '\r', '\0']),
            "REMOTE_COMMAND must be single-line without NUL"
        );
        let inner = REMOTE_COMMAND
            .strip_prefix("/bin/sh -c '")
            .and_then(|s| s.strip_suffix('\''))
            .expect("wrapped in /bin/sh -c '...'");
        assert!(!inner.is_empty());
        for forbidden in ['\'', '\\', '!', '\n', '\r', '\0'] {
            assert!(
                !inner.contains(forbidden),
                "forbidden char {forbidden:?} in REMOTE_COMMAND inner script"
            );
        }
    }

    #[cfg(unix)]
    fn find_shell_in_path(name: &str) -> Option<PathBuf> {
        let mut dirs = std::env::var_os("PATH")
            .map(|val| std::env::split_paths(&val).collect::<Vec<_>>())
            .unwrap_or_default();
        for extra in ["/opt/homebrew/bin", "/usr/local/bin"] {
            let p = PathBuf::from(extra);
            if !dirs.contains(&p) {
                dirs.push(p);
            }
        }
        dirs.into_iter()
            .map(|dir| dir.join(name))
            .find(|cand| cand.is_file())
    }

    #[cfg(unix)]
    fn available_unix_shells() -> Vec<String> {
        let candidates = [
            "/bin/sh",
            "/bin/dash",
            "bash",
            "zsh",
            "/bin/csh",
            "/bin/tcsh",
            "fish",
            "nu",
        ];
        let mut shells = Vec::new();
        for cand in candidates {
            let resolved = if cand.starts_with('/') {
                Path::new(cand).is_file().then(|| cand.to_owned())
            } else {
                find_shell_in_path(cand)
                    .map(|p| p.to_string_lossy().into_owned())
            };
            let Some(shell) = resolved else {
                continue;
            };
            if shells.contains(&shell) {
                continue;
            }
            let ok = Command::new(&shell)
                .args(["-c", "echo ok"])
                .output()
                .is_ok_and(|out| {
                    out.status.success()
                        && String::from_utf8_lossy(&out.stdout).trim() == "ok"
                });
            if ok {
                shells.push(shell);
            }
        }
        assert!(
            shells.iter().any(|s| s == "/bin/sh"),
            "expected /bin/sh in {shells:?}"
        );
        shells
    }

    #[cfg(unix)]
    #[test]
    fn remote_command_executes_across_available_shells() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let empty_home = dir.path().join("empty_home");
        let local_home = dir.path().join("local_home");
        let local_bin = local_home.join(".local/bin");
        let path_bin = dir.path().join("path_bin");
        std::fs::create_dir_all(&empty_home).unwrap();
        std::fs::create_dir_all(&local_bin).unwrap();
        std::fs::create_dir_all(&path_bin).unwrap();

        let local_helper = local_bin.join("ruum-scan");
        std::fs::write(
            &local_helper,
            "#!/bin/sh\nIFS= read -r req\nprintf 'LOCAL|%s|%s\\n' \"${1-}\" \"$req\"\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &local_helper,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let path_helper = path_bin.join("ruum-scan");
        std::fs::write(
            &path_helper,
            "#!/bin/sh\nIFS= read -r req\nprintf 'PATH|%s|%s\\n' \"${1-}\" \"$req\"\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &path_helper,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let mut path_dirs = vec![path_bin];
        if let Some(existing) = std::env::var_os("PATH") {
            path_dirs.extend(std::env::split_paths(&existing));
        }
        let path_with_helper = std::env::join_paths(path_dirs).unwrap();

        for shell in available_unix_shells() {
            // 1. Missing helper returns 127 and documented stderr hint.
            let missing = Command::new(&shell)
                .args(["-c", REMOTE_COMMAND])
                .env("HOME", &empty_home)
                .env("PATH", "/nonexistent")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert_eq!(
                missing.status.code(),
                Some(127),
                "shell={shell} stderr={}",
                String::from_utf8_lossy(&missing.stderr)
            );
            let missing_err = String::from_utf8_lossy(&missing.stderr);
            assert!(
                missing_err
                    .contains("ruum-scan is not installed on this server."),
                "shell={shell} stderr={missing_err}"
            );
            assert!(missing.stdout.is_empty(), "shell={shell}");

            // 2. Fallback to $HOME/.local/bin/ruum-scan with stdin passthrough.
            let mut local_child = Command::new(&shell)
                .args(["-c", REMOTE_COMMAND])
                .env("HOME", &local_home)
                .env("PATH", "/nonexistent")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let _ = local_child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"discover_version\":1}\n");
            let local_out = local_child.wait_with_output().unwrap();
            assert!(
                local_out.status.success(),
                "shell={shell} stderr={}",
                String::from_utf8_lossy(&local_out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&local_out.stdout),
                "LOCAL|--stdio-v1|{\"discover_version\":1}\n",
                "shell={shell}"
            );

            // 3. PATH helper takes priority over $HOME/.local/bin/ruum-scan.
            let mut path_child = Command::new(&shell)
                .args(["-c", REMOTE_COMMAND])
                .env("HOME", &local_home)
                .env("PATH", &path_with_helper)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let _ = path_child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"ok\":true}\n");
            let path_out = path_child.wait_with_output().unwrap();
            assert!(
                path_out.status.success(),
                "shell={shell} stderr={}",
                String::from_utf8_lossy(&path_out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&path_out.stdout),
                "PATH|--stdio-v1|{\"ok\":true}\n",
                "shell={shell}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn remote_command_falls_back_to_newest_executable_cached_scanner() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::time::{Duration, SystemTime};

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let default_cache = home.join(".cache/ruum/ssh");
        let old_dir = default_cache.join("1111");
        let new_dir = default_cache.join("2222");
        let nonexec_dir = default_cache.join("3333");
        let xdg_cache = dir.path().join("xdg_cache");
        let xdg_dir = xdg_cache.join("ruum/ssh/4444");
        for p in [&old_dir, &new_dir, &nonexec_dir, &xdg_dir] {
            std::fs::create_dir_all(p).unwrap();
        }

        let now = SystemTime::now();
        let old_helper = old_dir.join("ruum-scan");
        std::fs::write(
            &old_helper,
            "#!/bin/sh\nIFS= read -r req\nprintf 'CACHE_OLD|%s|%s\\n' \"${1-}\" \"$req\"\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &old_helper,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old_helper)
            .unwrap()
            .set_modified(now - Duration::from_mins(2))
            .unwrap();

        let new_helper = new_dir.join("ruum-scan");
        std::fs::write(
            &new_helper,
            "#!/bin/sh\nIFS= read -r req\nprintf 'CACHE_NEW|%s|%s\\n' \"${1-}\" \"$req\"\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &new_helper,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&new_helper)
            .unwrap()
            .set_modified(now - Duration::from_secs(30))
            .unwrap();

        let nonexec_helper = nonexec_dir.join("ruum-scan");
        std::fs::write(&nonexec_helper, "#!/bin/sh\nprintf 'NONEXEC\\n'\n")
            .unwrap();
        std::fs::set_permissions(
            &nonexec_helper,
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let xdg_helper = xdg_dir.join("ruum-scan");
        std::fs::write(
            &xdg_helper,
            "#!/bin/sh\nIFS= read -r req\nprintf 'CACHE_XDG|%s|%s\\n' \"${1-}\" \"$req\"\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &xdg_helper,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        for shell in available_unix_shells() {
            // 1. Uses newest executable under $HOME/.cache/ruum/ssh/*/ruum-scan.
            let mut child = Command::new(&shell)
                .args(["-c", REMOTE_COMMAND])
                .env("HOME", &home)
                .env_remove("XDG_CACHE_HOME")
                .env("PATH", "/nonexistent")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let _ = child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"discover_version\":1}\n");
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "shell={shell} stderr={}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "CACHE_NEW|--stdio-v1|{\"discover_version\":1}\n",
                "shell={shell}"
            );

            // 2. Respects XDG_CACHE_HOME when set.
            let mut xdg_child = Command::new(&shell)
                .args(["-c", REMOTE_COMMAND])
                .env("HOME", &home)
                .env("XDG_CACHE_HOME", &xdg_cache)
                .env("PATH", "/nonexistent")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let _ = xdg_child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"ok\":true}\n");
            let xdg_out = xdg_child.wait_with_output().unwrap();
            assert!(
                xdg_out.status.success(),
                "shell={shell} stderr={}",
                String::from_utf8_lossy(&xdg_out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&xdg_out.stdout),
                "CACHE_XDG|--stdio-v1|{\"ok\":true}\n",
                "shell={shell}"
            );
        }
    }

    #[test]
    fn ssh_error_classifies_handshake_pollution_and_missing_bin_sh() {
        for msg in [
            "unexpected SSH deployment handshake",
            "incomplete SSH deployment handshake",
        ] {
            let err = ssh_error("dev", "", &io::Error::other(msg)).to_string();
            assert!(err.contains("config.fish"), "{err}");
            assert!(err.contains(".bashrc"), "{err}");
            assert!(err.contains(".zshenv"), "{err}");
            assert!(err.contains("stdout"), "{err}");
            assert!(!err.contains("Install ruum-scan"), "{err}");
        }

        let noisy_warn = ssh_error(
            "dev",
            "Warning: Permanently added 'dev' to known hosts.",
            &io::Error::other("unexpected SSH deployment handshake"),
        )
        .to_string();
        assert!(noisy_warn.contains("config.fish"), "{noisy_warn}");
        assert!(noisy_warn.contains("stdout"), "{noisy_warn}");

        for stderr in [
            "/bin/sh: not found",
            "zsh: no such file or directory: /bin/sh",
            "'/bin/sh' is not recognized as an internal or external command",
        ] {
            let err = ssh_error(
                "dev",
                stderr,
                &io::Error::other("incomplete SSH deployment handshake"),
            )
            .to_string();
            assert!(err.contains("/bin/sh"), "{err}");
            assert!(err.contains("Unix POSIX /bin/sh"), "{err}");
            assert!(!err.contains("Install ruum-scan"), "{err}");
        }

        let missing_scan = ssh_error(
            "dev",
            "ruum-scan: command not found",
            &io::Error::other("SSH exited with 127"),
        )
        .to_string();
        assert!(
            missing_scan.contains("Install ruum-scan ON dev"),
            "{missing_scan}"
        );

        let auth = ssh_error(
            "dev",
            "Permission denied (publickey).",
            &io::Error::other("incomplete SSH deployment handshake"),
        )
        .to_string();
        assert!(auth.contains("verify your credentials"), "{auth}");
        assert!(!auth.contains("config.fish"), "{auth}");

        let no_pack_scan = ssh_error_with_context(
            "prod",
            "ruum-scan is not installed on this server.",
            &io::Error::other("SSH exited with 127"),
            false,
        )
        .to_string();
        assert!(no_pack_scan.contains("musl"), "{no_pack_scan}");
        assert!(no_pack_scan.contains("make install-scan"), "{no_pack_scan}");
        assert!(no_pack_scan.contains("prod"), "{no_pack_scan}");

        let missing_sha = ssh_error_with_context(
            "prod",
            "sh: 1: sha256sum: not found",
            &io::Error::other("SSH exited with 127"),
            true,
        )
        .to_string();
        assert!(
            missing_sha.contains("sha256sum: not found"),
            "{missing_sha}"
        );
        assert!(!missing_sha.contains("Install ruum-scan"), "{missing_sha}");
    }

    #[test]
    fn is_unsupported_space_error_rejects_handshake_and_missing_bin_sh() {
        let non_fallback_errors = [
            ssh_error(
                "dev",
                "",
                &io::Error::other("unexpected SSH deployment handshake"),
            ),
            ssh_error(
                "dev",
                "",
                &io::Error::other("incomplete SSH deployment handshake"),
            ),
            ssh_error(
                "dev",
                "/bin/sh: not found",
                &io::Error::other("incomplete SSH deployment handshake"),
            ),
            ssh_error(
                "dev",
                "No such file or directory: /bin/sh",
                &io::Error::other("SSH exited with 127"),
            ),
            ssh_error(
                "dev",
                "'/bin/sh' is not recognized",
                &io::Error::other("SSH exited with 1"),
            ),
            io::Error::other(
                "unexpected SSH deployment handshake (missing field)",
            ),
            io::Error::other("/bin/sh: not found (missing field)"),
        ];
        for err in &non_fallback_errors {
            assert!(
                !is_unsupported_space_error(err),
                "must not fall back for: {err}"
            );
        }

        assert!(is_unsupported_space_error(&io::Error::other(
            "data did not match any variant of untagged enum Operation"
        )));
        assert!(is_unsupported_space_error(&io::Error::other(
            "unsupported space check version"
        )));
    }
}
