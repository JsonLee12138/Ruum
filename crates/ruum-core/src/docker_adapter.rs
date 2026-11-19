//! Cross-platform Docker inspection and controlled cleanup adapter.
//!
//! Zero `service`-feature or C `-sys` dependencies so `ruum-scan`
//! (`default-features = false`) can reuse the exact same detection logic.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};

/// Timeout for read-only Docker `system df` probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Default idle timeout for streaming Docker cleanup commands.
pub const CLEAN_IDLE_TIMEOUT: Duration = Duration::from_mins(3);
/// Hard upper cap on total runtime for any single Docker cleanup command.
pub const CLEAN_MAX_TIMEOUT: Duration = Duration::from_mins(15);

/// Return the idle execution timeout for a given [`DockerCleanTarget`].
#[must_use]
pub const fn timeout_for_target(target: &DockerCleanTarget) -> Duration {
    match target {
        DockerCleanTarget::BuildCache => Duration::from_mins(5),
        DockerCleanTarget::Image { .. }
        | DockerCleanTarget::Container { .. } => Duration::from_mins(2),
    }
}

/// Operating system family for Docker CLI discovery and process flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DockerOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment inputs needed to locate the `docker` CLI across platforms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerEnv {
    pub home: Option<PathBuf>,
    pub path: Option<OsString>,
    pub program_files: Option<PathBuf>,
}

impl DockerEnv {
    /// Capture the current process environment once per probe or cleanup run.
    #[must_use]
    pub fn current() -> Self {
        Self {
            home: std::env::home_dir(),
            path: std::env::var_os("PATH"),
            program_files: std::env::var_os("ProgramFiles")
                .or_else(|| std::env::var_os("PROGRAMFILES"))
                .map(PathBuf::from),
        }
    }
}

/// Summary metrics for one Docker resource category.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerCategoryUsage {
    #[serde(default)]
    pub total_count: u64,
    #[serde(default)]
    pub active_count: u64,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub reclaimable_bytes: u64,
}

/// Unused (`containers == 0`) Docker image candidate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerImageEntry {
    pub id: String,
    pub repository: String,
    pub tag: String,
    pub created_since: String,
    pub size_bytes: u64,
    pub unique_bytes: u64,
    pub dangling: bool,
}

/// Stopped (`exited` / `created` / `dead`) Docker container candidate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerContainerEntry {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub size_bytes: u64,
}

/// Structured summary and safe cleanup candidates returned by `docker system df`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerUsageSummary {
    #[serde(default)]
    pub images: DockerCategoryUsage,
    #[serde(default)]
    pub containers: DockerCategoryUsage,
    #[serde(default)]
    pub local_volumes: DockerCategoryUsage,
    #[serde(default)]
    pub build_cache: DockerCategoryUsage,
    #[serde(default)]
    pub build_cache_reclaimable_bytes: u64,
    #[serde(default)]
    pub build_cache_shared_bytes: u64,
    #[serde(default)]
    pub removable_images: Vec<DockerImageEntry>,
    #[serde(default)]
    pub stopped_containers: Vec<DockerContainerEntry>,
    #[serde(default)]
    pub detailed: bool,
}

impl DockerUsageSummary {
    /// Total reclaimable bytes excluding local volumes (volumes are read-only).
    #[must_use]
    pub fn safe_reclaimable_bytes(&self) -> u64 {
        if !self.detailed {
            return self.build_cache_reclaimable_bytes;
        }
        let images_bytes: u64 = self
            .removable_images
            .iter()
            .map(|img| img.unique_bytes)
            .sum();
        let containers_bytes: u64 =
            self.stopped_containers.iter().map(|c| c.size_bytes).sum();
        self.build_cache_reclaimable_bytes
            .saturating_add(images_bytes)
            .saturating_add(containers_bytes)
    }
}

/// Four-state result of probing Docker CLI / daemon accessibility.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DockerProbeStatus {
    #[default]
    NotInstalled,
    DaemonStopped,
    PermissionDenied {
        socket_hint: String,
    },
    Available(DockerUsageSummary),
}

/// Controlled item-level Docker cleanup target (volumes intentionally omitted).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DockerCleanTarget {
    BuildCache,
    Image {
        id: String,
        reference: Option<String>,
        dangling: bool,
    },
    Container {
        id: String,
        name: String,
    },
}

/// Outcome of executing one [`DockerCleanTarget`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockerCleanOutcome {
    pub target: DockerCleanTarget,
    pub result: Result<(), String>,
}

/// Captured output of a Docker CLI invocation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerCommandOutput {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Subprocess execution abstraction for deterministic unit testing.
pub trait DockerCommandRunner: Send + Sync {
    fn run(
        &self,
        program: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> io::Result<DockerCommandOutput>;
}

/// Production [`DockerCommandRunner`] with `stdin = null`, `CREATE_NO_WINDOW`,
/// and monotonic `Instant` timeout without blocking on `.join()` on timeout.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdDockerCommandRunner;

impl DockerCommandRunner for StdDockerCommandRunner {
    fn run(
        &self,
        program: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> io::Result<DockerCommandOutput> {
        run_docker_command(program, args, timeout)
    }
}

/// OS-specific Docker CLI discovery, read-only probing, and cleanup execution.
pub trait DockerAdapter: Send + Sync + std::fmt::Debug {
    fn os_kind(&self) -> DockerOs;
    fn candidate_binaries(&self, env: &DockerEnv) -> Vec<PathBuf>;

    fn resolve_binary(&self, env: &DockerEnv) -> Option<PathBuf> {
        self.candidate_binaries(env)
            .into_iter()
            .find(|path| path.is_file())
    }

    fn probe(&self, env: &DockerEnv) -> DockerProbeStatus {
        let Some(binary) = self.resolve_binary(env) else {
            return DockerProbeStatus::NotInstalled;
        };
        self.probe_binary_with(&binary, &StdDockerCommandRunner)
    }

    fn probe_binary_with(
        &self,
        binary: &Path,
        runner: &dyn DockerCommandRunner,
    ) -> DockerProbeStatus {
        probe_with_runner(self.os_kind(), binary, runner)
    }

    fn run_clean_targets(
        &self,
        env: &DockerEnv,
        summary: &DockerUsageSummary,
        targets: &[DockerCleanTarget],
    ) -> Vec<DockerCleanOutcome> {
        let Some(binary) = self.resolve_binary(env) else {
            return targets
                .iter()
                .cloned()
                .map(|target| DockerCleanOutcome {
                    target,
                    result: Err("docker CLI not installed".to_owned()),
                })
                .collect();
        };
        self.run_clean_targets_with(
            &binary,
            summary,
            targets,
            &StdDockerCommandRunner,
        )
    }

    fn run_clean_targets_with(
        &self,
        binary: &Path,
        summary: &DockerUsageSummary,
        targets: &[DockerCleanTarget],
        runner: &dyn DockerCommandRunner,
    ) -> Vec<DockerCleanOutcome> {
        execute_clean_targets(binary, summary, targets, runner)
    }

    fn run_clean_targets_with_progress(
        &self,
        binary: &Path,
        summary: &DockerUsageSummary,
        targets: &[DockerCleanTarget],
        runner: &dyn DockerCommandRunner,
        on_progress: &mut dyn FnMut(&str, usize, usize),
    ) -> Vec<DockerCleanOutcome> {
        execute_clean_targets_with_progress(
            binary,
            summary,
            targets,
            runner,
            on_progress,
        )
    }

    fn run_clean_targets_with_env_progress(
        &self,
        env: &DockerEnv,
        summary: &DockerUsageSummary,
        targets: &[DockerCleanTarget],
        on_progress: &mut dyn FnMut(&str, usize, usize),
    ) -> Vec<DockerCleanOutcome> {
        let Some(binary) = self.resolve_binary(env) else {
            return targets
                .iter()
                .cloned()
                .map(|target| DockerCleanOutcome {
                    target,
                    result: Err("docker CLI not installed".to_owned()),
                })
                .collect();
        };
        self.run_clean_targets_with_progress(
            &binary,
            summary,
            targets,
            &StdDockerCommandRunner,
            on_progress,
        )
    }
}

/// macOS Docker CLI adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsDockerAdapter;

/// Linux Docker CLI adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxDockerAdapter;

/// Windows Docker CLI adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsDockerAdapter;

/// Factory returning the [`DockerAdapter`] for the active or requested OS.
#[derive(Clone, Copy, Debug, Default)]
pub struct DockerFactory;

impl DockerFactory {
    #[must_use]
    pub const fn current_os() -> DockerOs {
        if cfg!(target_os = "macos") {
            DockerOs::MacOs
        } else if cfg!(windows) {
            DockerOs::Windows
        } else {
            DockerOs::Linux
        }
    }

    #[must_use]
    pub fn create_for(os: DockerOs) -> Box<dyn DockerAdapter> {
        match os {
            DockerOs::MacOs => Box::new(MacOsDockerAdapter),
            DockerOs::Linux => Box::new(LinuxDockerAdapter),
            DockerOs::Windows => Box::new(WindowsDockerAdapter),
        }
    }

    #[must_use]
    pub fn create() -> Box<dyn DockerAdapter> {
        Self::create_for(Self::current_os())
    }
}

impl DockerAdapter for MacOsDockerAdapter {
    fn os_kind(&self) -> DockerOs {
        DockerOs::MacOs
    }

    fn candidate_binaries(&self, env: &DockerEnv) -> Vec<PathBuf> {
        let mut out = path_candidates(env, &["docker"]);
        push_unique(&mut out, PathBuf::from("/usr/local/bin/docker"));
        push_unique(&mut out, PathBuf::from("/opt/homebrew/bin/docker"));
        if let Some(home) = &env.home {
            push_unique(&mut out, home.join(".orbstack/bin/docker"));
            push_unique(&mut out, home.join(".rd/bin/docker"));
        }
        out
    }
}

impl DockerAdapter for LinuxDockerAdapter {
    fn os_kind(&self) -> DockerOs {
        DockerOs::Linux
    }

    fn candidate_binaries(&self, env: &DockerEnv) -> Vec<PathBuf> {
        let mut out = path_candidates(env, &["docker"]);
        push_unique(&mut out, PathBuf::from("/usr/bin/docker"));
        push_unique(&mut out, PathBuf::from("/usr/local/bin/docker"));
        out
    }
}

impl DockerAdapter for WindowsDockerAdapter {
    fn os_kind(&self) -> DockerOs {
        DockerOs::Windows
    }

    fn candidate_binaries(&self, env: &DockerEnv) -> Vec<PathBuf> {
        let mut out = path_candidates(env, &["docker.exe", "docker"]);
        if let Some(pf) = &env.program_files {
            push_unique(
                &mut out,
                pf.join("Docker")
                    .join("Docker")
                    .join("resources")
                    .join("bin")
                    .join("docker.exe"),
            );
        }
        out
    }
}

fn path_candidates(env: &DockerEnv, names: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(path) = &env.path else {
        return out;
    };
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in names {
            push_unique(&mut out, dir.join(name));
        }
    }
    out
}

fn push_unique(out: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !out.contains(&candidate) {
        out.push(candidate);
    }
}

fn probe_with_runner(
    os: DockerOs,
    binary: &Path,
    runner: &dyn DockerCommandRunner,
) -> DockerProbeStatus {
    let verbose = runner.run(
        binary,
        &["system", "df", "-v", "--format", "{{json .}}"],
        PROBE_TIMEOUT,
    );
    match verbose {
        Err(err) if err.kind() == io::ErrorKind::TimedOut => {
            run_summary_df_probe(os, binary, runner)
        }
        Err(err) => classify_io_error(&err, os),
        Ok(out) if out.status_code == 0 => {
            if let Some(summary) = parse_verbose_df_json(&out.stdout) {
                return DockerProbeStatus::Available(summary);
            }
            if let Some(summary) = parse_summary_df_ndjson(&out.stdout) {
                return DockerProbeStatus::Available(summary);
            }
            DockerProbeStatus::DaemonStopped
        }
        Ok(out) => {
            if is_permission_denied(&out.stderr) {
                return DockerProbeStatus::PermissionDenied {
                    socket_hint: extract_socket_hint(&out.stderr, os),
                };
            }
            if is_daemon_stopped(&out.stderr)
                || is_runtime_or_resource_failure(&out.stderr, out.status_code)
            {
                return DockerProbeStatus::DaemonStopped;
            }
            // Older Docker CLIs reject `-v` with `--format '{{json .}}'`;
            // gracefully fall back to summary mode.
            run_summary_df_probe(os, binary, runner)
        }
    }
}

fn run_summary_df_probe(
    os: DockerOs,
    binary: &Path,
    runner: &dyn DockerCommandRunner,
) -> DockerProbeStatus {
    match runner.run(
        binary,
        &["system", "df", "--format", "{{json .}}"],
        PROBE_TIMEOUT,
    ) {
        Err(err) => classify_io_error(&err, os),
        Ok(fb) if fb.status_code == 0 => parse_summary_df_ndjson(&fb.stdout)
            .map_or(
                DockerProbeStatus::DaemonStopped,
                DockerProbeStatus::Available,
            ),
        Ok(fb) => {
            if is_permission_denied(&fb.stderr) {
                DockerProbeStatus::PermissionDenied {
                    socket_hint: extract_socket_hint(&fb.stderr, os),
                }
            } else {
                DockerProbeStatus::DaemonStopped
            }
        }
    }
}

fn classify_io_error(err: &io::Error, os: DockerOs) -> DockerProbeStatus {
    match err.kind() {
        io::ErrorKind::NotFound => DockerProbeStatus::NotInstalled,
        io::ErrorKind::PermissionDenied => {
            DockerProbeStatus::PermissionDenied {
                socket_hint: default_socket_hint(os).to_owned(),
            }
        }
        _ => DockerProbeStatus::DaemonStopped,
    }
}

const fn default_socket_hint(os: DockerOs) -> &'static str {
    match os {
        DockerOs::Windows => "//./pipe/docker_engine",
        DockerOs::MacOs | DockerOs::Linux => "/var/run/docker.sock",
    }
}

fn is_permission_denied(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("permission denied") || lower.contains("access is denied")
}

fn is_daemon_stopped(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("cannot connect to the docker daemon")
        || lower.contains("is the docker daemon running")
        || lower.contains("error during connect")
        || lower.contains("connection refused")
        || lower.contains("dockerdesktoplinuxengine")
        || lower.contains("the system cannot find the file specified")
        || lower.contains("no such file or directory")
}

fn is_runtime_or_resource_failure(stderr: &str, status_code: i32) -> bool {
    if status_code < 0 {
        return true;
    }
    let lower = stderr.to_ascii_lowercase();
    lower.contains("fatal error: out of memory")
        || lower.contains("runtime: out of memory")
        || lower.contains("cannot allocate memory")
        || lower.contains("failed to create new os thread")
        || lower.contains("pthread_create failed")
}

fn extract_socket_hint(stderr: &str, os: DockerOs) -> String {
    if let Some(idx) = stderr.find("unix://") {
        let rest = &stderr[idx + "unix://".len()..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || matches!(c, ':' | ',' | '\'' | '"')
            })
            .unwrap_or(rest.len());
        let path = &rest[..end];
        if !path.is_empty() {
            return path.to_owned();
        }
    }
    if let Some(idx) = stderr.find("dial unix ") {
        let rest = &stderr[idx + "dial unix ".len()..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ':')
            .unwrap_or(rest.len());
        let path = &rest[..end];
        if !path.is_empty() {
            return path.to_owned();
        }
    }
    if let Some(idx) = stderr.find("//./pipe/") {
        let rest = &stderr[idx..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ':' | '\'' | '"'))
            .unwrap_or(rest.len());
        let path = &rest[..end];
        if !path.is_empty() {
            return path.to_owned();
        }
    }
    default_socket_hint(os).to_owned()
}

/// Strictly validate a Docker image or container ID (`[0-9a-f]{12,64}` or
/// `sha256:[0-9a-f]{64}`).
#[must_use]
pub fn is_valid_docker_id(id: &str) -> bool {
    if let Some(hex) = id.strip_prefix("sha256:") {
        hex.len() == 64 && hex.bytes().all(is_lower_hex)
    } else {
        (12..=64).contains(&id.len()) && id.bytes().all(is_lower_hex)
    }
}

const fn is_lower_hex(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'a'..=b'f')
}

/// Validate that a Docker reference or container name cannot inject options or
/// control characters.
#[must_use]
pub fn is_safe_docker_arg(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn entry_image_reference(entry: &DockerImageEntry) -> Option<String> {
    (!entry.dangling
        && !entry.repository.is_empty()
        && entry.repository != "<none>"
        && !entry.tag.is_empty()
        && entry.tag != "<none>")
        .then(|| format!("{}:{}", entry.repository, entry.tag))
        .filter(|r| is_safe_docker_arg(r))
}

/// Validate and sort requested Docker cleanup targets against a live snapshot
/// whitelist (`Container` -> `Image` -> `BuildCache`).
#[must_use]
pub fn validate_targets_against_summary(
    targets: &[DockerCleanTarget],
    summary: &DockerUsageSummary,
) -> Vec<DockerCleanTarget> {
    let mut seen_containers = HashSet::new();
    let mut seen_images: HashSet<(String, Option<String>)> = HashSet::new();
    let mut containers = Vec::new();
    let mut images = Vec::new();
    let mut include_build_cache = false;

    for target in targets {
        match target {
            DockerCleanTarget::Container { id, .. } => {
                if !is_valid_docker_id(id)
                    || !seen_containers.insert(id.clone())
                {
                    continue;
                }
                if let Some(entry) = summary
                    .stopped_containers
                    .iter()
                    .find(|c| c.id == *id && is_valid_docker_id(&c.id))
                {
                    let name = if is_safe_docker_arg(&entry.name) {
                        entry.name.clone()
                    } else {
                        entry.id.clone()
                    };
                    containers.push(DockerCleanTarget::Container {
                        id: entry.id.clone(),
                        name,
                    });
                }
            }
            DockerCleanTarget::Image {
                id,
                reference,
                dangling,
            } => {
                if !is_valid_docker_id(id) {
                    continue;
                }
                let matched = if !*dangling
                    && let Some(req_ref) = reference.as_deref()
                    && is_safe_docker_arg(req_ref)
                {
                    summary.removable_images.iter().find(|img| {
                        img.id == *id
                            && is_valid_docker_id(&img.id)
                            && !img.dangling
                            && entry_image_reference(img).as_deref()
                                == Some(req_ref)
                    })
                } else {
                    summary.removable_images.iter().find(|img| {
                        img.id == *id && is_valid_docker_id(&img.id)
                    })
                };
                if let Some(entry) = matched {
                    let resolved_ref = entry_image_reference(entry);
                    if seen_images
                        .insert((entry.id.clone(), resolved_ref.clone()))
                    {
                        images.push(DockerCleanTarget::Image {
                            id: entry.id.clone(),
                            reference: resolved_ref,
                            dangling: entry.dangling,
                        });
                    }
                }
            }
            DockerCleanTarget::BuildCache => {
                if summary.build_cache_reclaimable_bytes > 0
                    || summary.build_cache_shared_bytes > 0
                    || summary.build_cache.reclaimable_bytes > 0
                {
                    include_build_cache = true;
                }
            }
        }
    }

    let mut ordered = Vec::with_capacity(
        containers.len() + images.len() + usize::from(include_build_cache),
    );
    ordered.extend(containers);
    ordered.extend(images);
    if include_build_cache {
        ordered.push(DockerCleanTarget::BuildCache);
    }
    ordered
}

/// Build safe `docker` CLI arguments for a validated [`DockerCleanTarget`].
pub fn command_args_for_target(
    target: &DockerCleanTarget,
) -> Result<Vec<String>, String> {
    match target {
        DockerCleanTarget::BuildCache => Ok(vec![
            "builder".to_owned(),
            "prune".to_owned(),
            "-f".to_owned(),
        ]),
        DockerCleanTarget::Image {
            id,
            reference,
            dangling,
        } => {
            if !is_valid_docker_id(id) {
                return Err(format!("invalid docker image id: {id}"));
            }
            let arg = if !dangling
                && let Some(ref_str) = reference.as_deref()
                && is_safe_docker_arg(ref_str)
            {
                ref_str.to_owned()
            } else {
                id.clone()
            };
            Ok(vec![
                "image".to_owned(),
                "rm".to_owned(),
                "--".to_owned(),
                arg,
            ])
        }
        DockerCleanTarget::Container { id, .. } => {
            if !is_valid_docker_id(id) {
                return Err(format!("invalid docker container id: {id}"));
            }
            Ok(vec![
                "container".to_owned(),
                "rm".to_owned(),
                "--".to_owned(),
                id.clone(),
            ])
        }
    }
}

fn image_cli_arg(target: &DockerCleanTarget) -> Result<String, String> {
    let DockerCleanTarget::Image {
        id,
        reference,
        dangling,
    } = target
    else {
        return Err("expected image target".to_owned());
    };
    if !is_valid_docker_id(id) {
        return Err(format!("invalid docker image id: {id}"));
    }
    if !dangling
        && let Some(ref_str) = reference.as_deref()
        && is_safe_docker_arg(ref_str)
    {
        Ok(ref_str.to_owned())
    } else {
        Ok(id.clone())
    }
}

fn container_cli_arg(target: &DockerCleanTarget) -> Result<String, String> {
    let DockerCleanTarget::Container { id, .. } = target else {
        return Err("expected container target".to_owned());
    };
    if !is_valid_docker_id(id) {
        return Err(format!("invalid docker container id: {id}"));
    }
    Ok(id.clone())
}

fn run_checked_docker_args(
    binary: &Path,
    args: &[&str],
    timeout: Duration,
    runner: &dyn DockerCommandRunner,
) -> Result<(), String> {
    match runner.run(binary, args, timeout) {
        Ok(out) if out.status_code == 0 => Ok(()),
        Ok(out) => {
            let msg = out.stderr.trim();
            if msg.is_empty() {
                Err(format!("docker exited with status {}", out.status_code))
            } else {
                Err(msg.to_owned())
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn execute_single_target(
    binary: &Path,
    target: &DockerCleanTarget,
    runner: &dyn DockerCommandRunner,
) -> Result<(), String> {
    let args = command_args_for_target(target)?;
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_checked_docker_args(
        binary,
        &arg_refs,
        timeout_for_target(target),
        runner,
    )
}

fn execute_clean_targets(
    binary: &Path,
    summary: &DockerUsageSummary,
    targets: &[DockerCleanTarget],
    runner: &dyn DockerCommandRunner,
) -> Vec<DockerCleanOutcome> {
    execute_clean_targets_with_progress(
        binary,
        summary,
        targets,
        runner,
        &mut |_, _, _| {},
    )
}

fn execute_clean_targets_with_progress(
    binary: &Path,
    summary: &DockerUsageSummary,
    targets: &[DockerCleanTarget],
    runner: &dyn DockerCommandRunner,
    on_progress: &mut dyn FnMut(&str, usize, usize),
) -> Vec<DockerCleanOutcome> {
    let validated = validate_targets_against_summary(targets, summary);
    let total = validated.len();
    let mut containers = Vec::new();
    let mut images = Vec::new();
    let mut has_build_cache = false;

    for target in validated {
        match target {
            DockerCleanTarget::Container { .. } => containers.push(target),
            DockerCleanTarget::Image { .. } => images.push(target),
            DockerCleanTarget::BuildCache => has_build_cache = true,
        }
    }

    let mut outcomes = Vec::with_capacity(total);

    if !containers.is_empty() {
        on_progress("containers", outcomes.len(), total);
        execute_container_phase(binary, containers, runner, &mut outcomes);
        on_progress("containers", outcomes.len(), total);
    }

    if !images.is_empty() {
        on_progress("images", outcomes.len(), total);
        execute_image_phase(binary, images, runner, &mut outcomes);
        on_progress("images", outcomes.len(), total);
    }

    if has_build_cache {
        on_progress("build_cache", outcomes.len(), total);
        let target = DockerCleanTarget::BuildCache;
        let result = execute_single_target(binary, &target, runner);
        outcomes.push(DockerCleanOutcome { target, result });
        on_progress("build_cache", outcomes.len(), total);
    }

    outcomes
}

fn execute_container_phase(
    binary: &Path,
    containers: Vec<DockerCleanTarget>,
    runner: &dyn DockerCommandRunner,
    outcomes: &mut Vec<DockerCleanOutcome>,
) {
    if containers.len() > 1 {
        let batch_ids: Result<Vec<String>, String> =
            containers.iter().map(container_cli_arg).collect();
        if let Ok(ids) = batch_ids {
            let mut args = vec!["container", "rm", "--"];
            args.extend(ids.iter().map(String::as_str));
            if run_checked_docker_args(
                binary,
                &args,
                timeout_for_target(&containers[0]),
                runner,
            )
            .is_ok()
            {
                for target in containers {
                    outcomes.push(DockerCleanOutcome {
                        target,
                        result: Ok(()),
                    });
                }
                return;
            }
        }
    }

    for target in containers {
        let result = execute_single_target(binary, &target, runner);
        outcomes.push(DockerCleanOutcome { target, result });
    }
}

fn execute_image_phase(
    binary: &Path,
    images: Vec<DockerCleanTarget>,
    runner: &dyn DockerCommandRunner,
    outcomes: &mut Vec<DockerCleanOutcome>,
) {
    if images.len() > 1 {
        let batch_refs: Result<Vec<String>, String> =
            images.iter().map(image_cli_arg).collect();
        if let Ok(refs) = batch_refs {
            let mut args = vec!["image", "rm", "--"];
            args.extend(refs.iter().map(String::as_str));
            if run_checked_docker_args(
                binary,
                &args,
                timeout_for_target(&images[0]),
                runner,
            )
            .is_ok()
            {
                for target in images {
                    outcomes.push(DockerCleanOutcome {
                        target,
                        result: Ok(()),
                    });
                }
                return;
            }
        }
    }

    let mut results = Vec::with_capacity(images.len());
    let mut any_succeeded = false;
    let mut failed_indices = Vec::new();
    for (idx, target) in images.iter().enumerate() {
        let res = execute_single_target(binary, target, runner);
        if res.is_ok() {
            any_succeeded = true;
        } else {
            failed_indices.push(idx);
        }
        results.push(res);
    }

    if any_succeeded {
        for idx in failed_indices {
            results[idx] = execute_single_target(binary, &images[idx], runner);
        }
    }

    for (target, result) in images.into_iter().zip(results) {
        outcomes.push(DockerCleanOutcome { target, result });
    }
}

// ── `docker system df` JSON parsing ─────────────────────────────────────────

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrU64 {
    U64(u64),
    Str(String),
}

impl StringOrU64 {
    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(v) => Some(*v),
            Self::Str(s) => s.trim().parse::<u64>().ok(),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrBool {
    Bool(bool),
    Str(String),
}

impl StringOrBool {
    fn as_bool(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Str(s) => s.trim().eq_ignore_ascii_case("true"),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawVerboseDf {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    images: Vec<RawImageEntry>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    containers: Vec<RawContainerEntry>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    volumes: Vec<RawVolumeEntry>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    build_cache: Vec<RawBuildCacheEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawImageEntry {
    #[serde(default)]
    repository: String,
    #[serde(default)]
    tag: String,
    #[serde(default, rename = "ID")]
    id: String,
    #[serde(default)]
    created_since: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    unique_size: Option<String>,
    #[serde(default)]
    containers: Option<StringOrU64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawContainerEntry {
    #[serde(default, rename = "ID")]
    id: String,
    #[serde(default, alias = "Name")]
    names: String,
    #[serde(default)]
    image: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    size: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawVolumeEntry {
    #[serde(default)]
    links: Option<StringOrU64>,
    #[serde(default)]
    size: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawBuildCacheEntry {
    #[serde(default)]
    in_use: Option<StringOrBool>,
    #[serde(default)]
    shared: Option<StringOrBool>,
    #[serde(default)]
    size: String,
    #[serde(default)]
    reclaimable: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawSummaryLine {
    #[serde(default, rename = "Type")]
    kind: String,
    #[serde(default)]
    total_count: Option<StringOrU64>,
    #[serde(default)]
    active: Option<StringOrU64>,
    #[serde(default)]
    size: String,
    #[serde(default)]
    reclaimable: String,
}

fn parse_verbose_df_json(stdout: &str) -> Option<DockerUsageSummary> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw: RawVerboseDf = serde_json::from_str(trimmed).ok()?;

    let mut summary = DockerUsageSummary {
        detailed: true,
        ..DockerUsageSummary::default()
    };

    summary.images.total_count = raw.images.len() as u64;
    for img in raw.images {
        let size_bytes = parse_docker_size(&img.size).unwrap_or(0);
        let unique_bytes = img
            .unique_size
            .as_deref()
            .and_then(parse_docker_size)
            .unwrap_or(size_bytes);
        let refs = img.containers.as_ref().and_then(StringOrU64::as_u64);
        summary.images.size_bytes =
            summary.images.size_bytes.saturating_add(unique_bytes);
        if refs.is_some_and(|count| count > 0) {
            summary.images.active_count += 1;
        } else if refs == Some(0) && is_valid_docker_id(img.id.trim()) {
            let dangling =
                img.repository.trim() == "<none>" && img.tag.trim() == "<none>";
            summary.images.reclaimable_bytes = summary
                .images
                .reclaimable_bytes
                .saturating_add(unique_bytes);
            summary.removable_images.push(DockerImageEntry {
                id: img.id.trim().to_owned(),
                repository: img.repository.trim().to_owned(),
                tag: img.tag.trim().to_owned(),
                created_since: img.created_since.trim().to_owned(),
                size_bytes,
                unique_bytes,
                dangling,
            });
        }
    }

    summary.containers.total_count = raw.containers.len() as u64;
    for ctr in raw.containers {
        let size_bytes = parse_docker_size(&ctr.size).unwrap_or(0);
        summary.containers.size_bytes =
            summary.containers.size_bytes.saturating_add(size_bytes);
        let state = normalize_container_state(&ctr.state, &ctr.status);
        if matches!(state.as_str(), "exited" | "created" | "dead") {
            summary.containers.reclaimable_bytes = summary
                .containers
                .reclaimable_bytes
                .saturating_add(size_bytes);
            if is_valid_docker_id(ctr.id.trim()) {
                let clean_name =
                    ctr.names.trim().trim_start_matches('/').to_owned();
                summary.stopped_containers.push(DockerContainerEntry {
                    id: ctr.id.trim().to_owned(),
                    name: clean_name,
                    image: ctr.image.trim().to_owned(),
                    state,
                    status: ctr.status.trim().to_owned(),
                    size_bytes,
                });
            }
        } else {
            summary.containers.active_count += 1;
        }
    }

    summary.local_volumes.total_count = raw.volumes.len() as u64;
    for vol in raw.volumes {
        let size_bytes = parse_docker_size(&vol.size).unwrap_or(0);
        let links = vol
            .links
            .as_ref()
            .and_then(StringOrU64::as_u64)
            .unwrap_or(1);
        summary.local_volumes.size_bytes =
            summary.local_volumes.size_bytes.saturating_add(size_bytes);
        if links > 0 {
            summary.local_volumes.active_count += 1;
        } else {
            summary.local_volumes.reclaimable_bytes = summary
                .local_volumes
                .reclaimable_bytes
                .saturating_add(size_bytes);
        }
    }

    summary.build_cache.total_count = raw.build_cache.len() as u64;
    for bc in raw.build_cache {
        let in_use = bc.in_use.as_ref().is_some_and(StringOrBool::as_bool);
        let shared = bc.shared.as_ref().is_some_and(StringOrBool::as_bool);
        let size_bytes = parse_docker_size(&bc.size).unwrap_or(0);
        summary.build_cache.size_bytes =
            summary.build_cache.size_bytes.saturating_add(size_bytes);
        if in_use {
            summary.build_cache.active_count += 1;
        } else if shared {
            summary.build_cache_shared_bytes =
                summary.build_cache_shared_bytes.saturating_add(size_bytes);
        } else {
            let reclaim = bc
                .reclaimable
                .as_deref()
                .and_then(parse_docker_size)
                .unwrap_or(size_bytes);
            summary.build_cache.reclaimable_bytes = summary
                .build_cache
                .reclaimable_bytes
                .saturating_add(reclaim);
            summary.build_cache_reclaimable_bytes = summary
                .build_cache_reclaimable_bytes
                .saturating_add(reclaim);
        }
    }

    Some(summary)
}

fn normalize_container_state(state: &str, status: &str) -> String {
    let trimmed = state.trim();
    if !trimmed.is_empty() {
        return trimmed.to_ascii_lowercase();
    }
    let status_lower = status.trim().to_ascii_lowercase();
    if status_lower.contains("(paused)") || status_lower == "paused" {
        "paused".to_owned()
    } else if status_lower.starts_with("up ") || status_lower == "up" {
        "running".to_owned()
    } else if status_lower.starts_with("restarting") {
        "restarting".to_owned()
    } else if status_lower.starts_with("exited") {
        "exited".to_owned()
    } else if status_lower.starts_with("created") {
        "created".to_owned()
    } else if status_lower.starts_with("dead") {
        "dead".to_owned()
    } else {
        "unknown".to_owned()
    }
}

fn parse_summary_df_ndjson(stdout: &str) -> Option<DockerUsageSummary> {
    let mut summary = DockerUsageSummary {
        detailed: false,
        ..DockerUsageSummary::default()
    };
    let mut matched = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<RawSummaryLine>(trimmed) else {
            continue;
        };
        let cat = DockerCategoryUsage {
            total_count: row
                .total_count
                .as_ref()
                .and_then(StringOrU64::as_u64)
                .unwrap_or(0),
            active_count: row
                .active
                .as_ref()
                .and_then(StringOrU64::as_u64)
                .unwrap_or(0),
            size_bytes: parse_docker_size(&row.size).unwrap_or(0),
            reclaimable_bytes: parse_docker_size(&row.reclaimable).unwrap_or(0),
        };
        match row.kind.trim().to_ascii_lowercase().as_str() {
            "images" => {
                summary.images = cat;
                matched = true;
            }
            "containers" => {
                summary.containers = cat;
                matched = true;
            }
            "local volumes" => {
                summary.local_volumes = cat;
                matched = true;
            }
            "build cache" => {
                summary.build_cache_reclaimable_bytes = cat.reclaimable_bytes;
                summary.build_cache = cat;
                matched = true;
            }
            _ => {}
        }
    }
    matched.then_some(summary)
}

/// Parse a Docker human size string (e.g. `"1.2GB (50%)"`, `"1.5GiB"`,
/// `"12kB (virtual 80MB)"`, `"0B"`) into bytes.
#[must_use]
pub fn parse_docker_size(input: &str) -> Option<u64> {
    let head = input.split('(').next()?.trim();
    if head.is_empty() || head == "-" || head.eq_ignore_ascii_case("n/a") {
        return None;
    }
    if head == "0" {
        return Some(0);
    }
    let unit_idx = head.find(|c: char| c.is_ascii_alphabetic())?;
    let num: f64 = head[..unit_idx].trim().parse().ok()?;
    if !num.is_finite() || num < 0.0 {
        return None;
    }
    let unit = head[unit_idx..].trim().to_ascii_lowercase();
    let mult = match unit.as_str() {
        "b" => 1.0,
        "kb" => 1_000.0,
        "mb" => 1_000_000.0,
        "gb" => 1_000_000_000.0,
        "tb" => 1_000_000_000_000.0,
        "pb" => 1_000_000_000_000_000.0,
        "kib" => 1024.0,
        "mib" => 1_048_576.0,
        "gib" => 1_073_741_824.0,
        "tib" => 1_099_511_627_776.0,
        "pib" => 1_125_899_906_842_624.0,
        _ => return None,
    };
    Some((num * mult).round() as u64)
}

fn elapsed_nanos_u64(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn spawn_activity_reader(
    pipe: Option<impl io::Read + Send + 'static>,
    start: Instant,
    last_activity_ns: Arc<AtomicU64>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let Some(mut reader) = pipe else {
            return buf;
        };
        let mut chunk = [0_u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    last_activity_ns
                        .store(elapsed_nanos_u64(start), Ordering::Release);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        buf
    })
}

fn run_docker_command(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> io::Result<DockerCommandOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn()?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let start = Instant::now();
    let last_activity_ns = Arc::new(AtomicU64::new(0));
    let stdout_handle = spawn_activity_reader(
        stdout_pipe,
        start,
        Arc::clone(&last_activity_ns),
    );
    let stderr_handle = spawn_activity_reader(
        stderr_pipe,
        start,
        Arc::clone(&last_activity_ns),
    );

    let max_runtime = CLEAN_MAX_TIMEOUT.max(timeout);
    let mut backoff = Duration::from_millis(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let total_elapsed = start.elapsed();
        let total_ns =
            u64::try_from(total_elapsed.as_nanos()).unwrap_or(u64::MAX);
        let last_ns = last_activity_ns.load(Ordering::Acquire);
        let idle_elapsed =
            Duration::from_nanos(total_ns.saturating_sub(last_ns));
        if idle_elapsed >= timeout || total_elapsed >= max_runtime {
            let _ = child.kill();
            let _ = child.wait();
            // Do not join reader threads on timeout: child processes holding
            // the pipe write end open would block indefinitely.
            drop((stdout_handle, stderr_handle));
            let msg = if total_elapsed >= max_runtime {
                format!(
                    "docker command exceeded max runtime {}s",
                    max_runtime.as_secs()
                )
            } else {
                format!(
                    "docker command idle timed out after {}s",
                    timeout.as_secs()
                )
            };
            return Err(io::Error::new(io::ErrorKind::TimedOut, msg));
        }
        let remaining_idle = timeout.saturating_sub(idle_elapsed);
        let remaining_total = max_runtime.saturating_sub(total_elapsed);
        std::thread::sleep(backoff.min(remaining_idle).min(remaining_total));
        backoff = (backoff * 2).min(Duration::from_millis(25));
    };

    let stdout_bytes = stdout_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();

    Ok(DockerCommandOutput {
        status_code: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    type RecordedCall = (PathBuf, Vec<String>);
    type MockResponder = Box<
        dyn Fn(&Path, &[&str]) -> io::Result<DockerCommandOutput> + Send + Sync,
    >;

    struct MockRunner {
        calls: Arc<Mutex<Vec<RecordedCall>>>,
        responder: MockResponder,
    }

    impl MockRunner {
        fn new<F>(f: F) -> Self
        where
            F: Fn(&Path, &[&str]) -> io::Result<DockerCommandOutput>
                + Send
                + Sync
                + 'static,
        {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                responder: Box::new(f),
            }
        }

        fn calls(&self) -> Vec<RecordedCall> {
            self.calls.lock().expect("calls lock").clone()
        }
    }

    impl DockerCommandRunner for MockRunner {
        fn run(
            &self,
            program: &Path,
            args: &[&str],
            _timeout: Duration,
        ) -> io::Result<DockerCommandOutput> {
            self.calls.lock().expect("calls lock").push((
                program.to_path_buf(),
                args.iter().map(|s| (*s).to_owned()).collect(),
            ));
            (self.responder)(program, args)
        }
    }

    #[test]
    fn candidate_binaries_cover_macos_linux_and_windows_paths() {
        let mac_env = DockerEnv {
            home: Some(PathBuf::from("/Users/alice")),
            path: Some(OsString::from("/usr/bin")),
            program_files: None,
        };
        let mac_bins = DockerFactory::create_for(DockerOs::MacOs)
            .candidate_binaries(&mac_env);
        assert!(mac_bins.contains(&PathBuf::from("/usr/local/bin/docker")));
        assert!(mac_bins.contains(&PathBuf::from("/opt/homebrew/bin/docker")));
        assert!(
            mac_bins
                .contains(&PathBuf::from("/Users/alice/.orbstack/bin/docker"))
        );
        assert!(
            mac_bins.contains(&PathBuf::from("/Users/alice/.rd/bin/docker"))
        );

        let linux_env = DockerEnv {
            home: Some(PathBuf::from("/home/alice")),
            path: Some(OsString::from("/home/alice/bin")),
            program_files: None,
        };
        let linux_bins = DockerFactory::create_for(DockerOs::Linux)
            .candidate_binaries(&linux_env);
        assert_eq!(linux_bins[0], PathBuf::from("/home/alice/bin/docker"));
        assert!(linux_bins.contains(&PathBuf::from("/usr/bin/docker")));
        assert!(linux_bins.contains(&PathBuf::from("/usr/local/bin/docker")));

        let win_env = DockerEnv {
            home: Some(PathBuf::from(r"C:\Users\alice")),
            path: Some(OsString::from(r"C:\Tools")),
            program_files: Some(PathBuf::from(r"C:\Program Files")),
        };
        let win_bins = DockerFactory::create_for(DockerOs::Windows)
            .candidate_binaries(&win_env);
        assert!(win_bins.iter().any(|p| p.ends_with("docker.exe")));
        assert!(
            win_bins.contains(
                &PathBuf::from(r"C:\Program Files")
                    .join("Docker")
                    .join("Docker")
                    .join("resources")
                    .join("bin")
                    .join("docker.exe")
            )
        );
    }

    #[test]
    fn verbose_json_parses_unused_images_stopped_containers_and_excludes_active_and_volumes()
     {
        let verbose_json = r#"{
            "Images": [
                {
                    "Repository": "<none>",
                    "Tag": "<none>",
                    "ID": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "CreatedSince": "3 weeks ago",
                    "Size": "120MB",
                    "SharedSize": "20MB",
                    "UniqueSize": "100MB",
                    "Containers": "0"
                },
                {
                    "Repository": "nginx",
                    "Tag": "alpine",
                    "ID": "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
                    "CreatedSince": "2 days ago",
                    "Size": "45MB",
                    "SharedSize": "0B",
                    "UniqueSize": "45MB",
                    "Containers": "1"
                },
                {
                    "Repository": "ubuntu",
                    "Tag": "24.04",
                    "ID": "abcdef123456",
                    "CreatedSince": "1 month ago",
                    "Size": "80MB",
                    "SharedSize": "10MB",
                    "UniqueSize": "70MB",
                    "Containers": 0
                }
            ],
            "Containers": [
                {
                    "ID": "111122223333",
                    "Names": "old-job",
                    "Image": "ubuntu:24.04",
                    "State": "exited",
                    "Status": "Exited (0) 4 days ago",
                    "Size": "12kB (virtual 80MB)"
                },
                {
                    "ID": "444455556666",
                    "Names": "live-web",
                    "Image": "nginx:alpine",
                    "State": "running",
                    "Status": "Up 2 hours",
                    "Size": "4MB"
                },
                {
                    "ID": "777788889999",
                    "Names": "paused-worker",
                    "Image": "nginx:alpine",
                    "State": "paused",
                    "Status": "Up 1 hour (Paused)",
                    "Size": "2MB"
                },
                {
                    "ID": "aaaabbbbcccc",
                    "Names": "created-task",
                    "Image": "alpine:latest",
                    "State": "created",
                    "Status": "Created",
                    "Size": "0B"
                }
            ],
            "Volumes": [
                {"Name": "pgdata", "Links": "0", "Size": "2.5GB"}
            ],
            "BuildCache": [
                {"ID": "bc1", "InUse": "false", "Size": "1.5GiB"},
                {"ID": "bc2", "InUse": true, "Size": "500MB"}
            ]
        }"#;

        let runner = MockRunner::new(move |_, _| {
            Ok(DockerCommandOutput {
                status_code: 0,
                stdout: verbose_json.to_owned(),
                stderr: String::new(),
            })
        });
        let adapter = DockerFactory::create_for(DockerOs::Linux);
        let status =
            adapter.probe_binary_with(Path::new("/usr/bin/docker"), &runner);
        let DockerProbeStatus::Available(summary) = status else {
            panic!("expected Available, got {status:?}");
        };
        assert!(summary.detailed);
        assert_eq!(summary.removable_images.len(), 2);
        assert!(summary.removable_images[0].dangling);
        assert_eq!(summary.removable_images[0].unique_bytes, 100_000_000);
        assert!(!summary.removable_images[1].dangling);
        assert_eq!(summary.removable_images[1].repository, "ubuntu");
        assert!(
            summary
                .removable_images
                .iter()
                .all(|img| img.repository != "nginx")
        );

        assert_eq!(summary.stopped_containers.len(), 2);
        assert_eq!(summary.stopped_containers[0].id, "111122223333");
        assert_eq!(summary.stopped_containers[0].size_bytes, 12_000);
        assert_eq!(summary.stopped_containers[1].id, "aaaabbbbcccc");
        assert_eq!(summary.local_volumes.total_count, 1);
        assert_eq!(summary.local_volumes.size_bytes, 2_500_000_000);
        assert_eq!(
            summary.build_cache_reclaimable_bytes,
            1_610_612_736 // 1.5 GiB
        );
    }

    #[test]
    fn falls_back_to_summary_mode_when_verbose_json_is_unsupported() {
        let runner = MockRunner::new(|_, args| {
            if args.contains(&"-v") {
                Ok(DockerCommandOutput {
                    status_code: 125,
                    stdout: String::new(),
                    stderr: "unknown shorthand flag: 'v'".to_owned(),
                })
            } else {
                Ok(DockerCommandOutput {
                    status_code: 0,
                    stdout: concat!(
                        r#"{"Active":"1","Reclaimable":"500MB (50%)","Size":"1GB","TotalCount":"2","Type":"Images"}"#,
                        "\n",
                        r#"{"Active":"0","Reclaimable":"250MB","Size":"250MB","TotalCount":"3","Type":"Build Cache"}"#,
                        "\n"
                    )
                    .to_owned(),
                    stderr: String::new(),
                })
            }
        });
        let adapter = DockerFactory::create_for(DockerOs::Linux);
        let status =
            adapter.probe_binary_with(Path::new("/usr/bin/docker"), &runner);
        let DockerProbeStatus::Available(summary) = status else {
            panic!("expected Available, got {status:?}");
        };
        assert!(!summary.detailed);
        assert!(summary.removable_images.is_empty());
        assert!(summary.stopped_containers.is_empty());
        assert_eq!(summary.images.total_count, 2);
        assert_eq!(summary.build_cache_reclaimable_bytes, 250_000_000);
        assert_eq!(runner.calls().len(), 2);
    }

    #[test]
    fn classifies_permission_denied_daemon_stopped_and_go_rlimit_failure() {
        let perm_runner = MockRunner::new(|_, _| {
            Ok(DockerCommandOutput {
                status_code: 1,
                stdout: String::new(),
                stderr: "Got permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock: Get ...: dial unix /var/run/docker.sock: connect: permission denied".to_owned(),
            })
        });
        let adapter = DockerFactory::create_for(DockerOs::Linux);
        assert_eq!(
            adapter
                .probe_binary_with(Path::new("/usr/bin/docker"), &perm_runner),
            DockerProbeStatus::PermissionDenied {
                socket_hint: "/var/run/docker.sock".to_owned(),
            }
        );

        let stopped_runner = MockRunner::new(|_, _| {
            Ok(DockerCommandOutput {
                status_code: 1,
                stdout: String::new(),
                stderr: "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?".to_owned(),
            })
        });
        assert_eq!(
            adapter.probe_binary_with(
                Path::new("/usr/bin/docker"),
                &stopped_runner
            ),
            DockerProbeStatus::DaemonStopped
        );

        // Simulated Go runtime failure under RLIMIT_AS = 1 GiB must degrade
        // without retrying fallback or panicking.
        let oom_runner = MockRunner::new(|_, _| {
            Ok(DockerCommandOutput {
                status_code: 2,
                stdout: String::new(),
                stderr: "fatal error: out of memory".to_owned(),
            })
        });
        assert_eq!(
            adapter
                .probe_binary_with(Path::new("/usr/bin/docker"), &oom_runner),
            DockerProbeStatus::DaemonStopped
        );
        assert_eq!(oom_runner.calls().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_child_without_blocking_on_background_descendant_pipe() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("slow-docker");
        std::fs::write(&script, "#!/bin/sh\nsleep 3 &\nsleep 3\n").unwrap();
        std::fs::set_permissions(
            &script,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let started = Instant::now();
        let err = run_docker_command(
            &script,
            &["system", "df"],
            Duration::from_millis(80),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_millis(1200),
            "timed out command blocked on pipe join: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn id_validation_whitelist_and_safe_execution_order() {
        assert!(is_valid_docker_id("abcdef012345"));
        assert!(is_valid_docker_id(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_valid_docker_id("-rf"));
        assert!(!is_valid_docker_id("abcdef01234")); // too short
        assert!(!is_valid_docker_id("abcdef012345; rm -rf /"));
        assert!(!is_safe_docker_arg("--force"));
        assert!(!is_safe_docker_arg("repo:tag\nrm"));
        assert!(!is_safe_docker_arg("repo:tag extra"));
        assert!(is_safe_docker_arg("ubuntu:24.04"));

        let summary = DockerUsageSummary {
            build_cache_reclaimable_bytes: 512,
            removable_images: vec![DockerImageEntry {
                id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
                repository: "ubuntu".to_owned(),
                tag: "24.04".to_owned(),
                created_since: "1d".to_owned(),
                size_bytes: 100,
                unique_bytes: 100,
                dangling: false,
            }],
            stopped_containers: vec![DockerContainerEntry {
                id: "111122223333".to_owned(),
                name: "old-job".to_owned(),
                image: "ubuntu:24.04".to_owned(),
                state: "exited".to_owned(),
                status: "Exited (0)".to_owned(),
                size_bytes: 50,
            }],
            detailed: true,
            ..DockerUsageSummary::default()
        };

        let requested = vec![
            DockerCleanTarget::BuildCache,
            DockerCleanTarget::Image {
                id: "999999999999".to_owned(), // not in snapshot
                reference: Some("nginx:latest".to_owned()),
                dangling: false,
            },
            DockerCleanTarget::Image {
                id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
                reference: Some("-f".to_owned()), // tampered reference must be replaced from snapshot
                dangling: false,
            },
            DockerCleanTarget::Container {
                id: "111122223333".to_owned(),
                name: "old-job".to_owned(),
            },
            DockerCleanTarget::BuildCache, // duplicate
        ];

        let runner = MockRunner::new(|_, _| {
            Ok(DockerCommandOutput {
                status_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        });
        let adapter = DockerFactory::create_for(DockerOs::Linux);
        let outcomes = adapter.run_clean_targets_with(
            Path::new("/usr/bin/docker"),
            &summary,
            &requested,
            &runner,
        );
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes.iter().all(|o| o.result.is_ok()));

        let calls = runner.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].1, vec!["container", "rm", "--", "111122223333"]);
        assert_eq!(calls[1].1, vec!["image", "rm", "--", "ubuntu:24.04"]);
        assert_eq!(calls[2].1, vec!["builder", "prune", "-f"]);
    }

    #[test]
    fn json_serialization_matches_spec_tag_and_handles_null_arrays() {
        let status = DockerProbeStatus::PermissionDenied {
            socket_hint: "/var/run/docker.sock".to_owned(),
        };
        let encoded = serde_json::to_string(&status).unwrap();
        assert_eq!(
            encoded,
            r#"{"status":"permission_denied","socket_hint":"/var/run/docker.sock"}"#
        );
        let decoded: DockerProbeStatus =
            serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, status);

        let verbose_nulls = r#"{"Images":null,"Containers":null,"Volumes":null,"BuildCache":null}"#;
        let parsed = parse_verbose_df_json(verbose_nulls).unwrap();
        assert!(parsed.detailed);
        assert!(parsed.removable_images.is_empty());
        assert!(parsed.stopped_containers.is_empty());
        assert_eq!(parsed.build_cache_reclaimable_bytes, 0);
    }

    #[test]
    fn timeout_constants_and_tiered_timeouts_are_passed_to_runner() {
        type RecordedTimeoutCalls = Arc<Mutex<Vec<(Vec<String>, Duration)>>>;
        struct TimeoutRecordingRunner {
            recorded: RecordedTimeoutCalls,
        }
        impl DockerCommandRunner for TimeoutRecordingRunner {
            fn run(
                &self,
                _program: &Path,
                args: &[&str],
                timeout: Duration,
            ) -> io::Result<DockerCommandOutput> {
                self.recorded.lock().expect("lock").push((
                    args.iter().map(|s| (*s).to_owned()).collect(),
                    timeout,
                ));
                Ok(DockerCommandOutput {
                    status_code: 0,
                    stdout: r#"{"Images":[],"Containers":[],"Volumes":[],"BuildCache":[]}"#.to_owned(),
                    stderr: String::new(),
                })
            }
        }

        assert_eq!(PROBE_TIMEOUT, Duration::from_secs(15));
        assert_eq!(CLEAN_IDLE_TIMEOUT, Duration::from_mins(3));
        assert_eq!(CLEAN_MAX_TIMEOUT, Duration::from_mins(15));
        assert_eq!(
            timeout_for_target(&DockerCleanTarget::BuildCache),
            Duration::from_mins(5)
        );
        assert_eq!(
            timeout_for_target(&DockerCleanTarget::Image {
                id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
                reference: Some("alpine:3.20".to_owned()),
                dangling: false,
            }),
            Duration::from_mins(2)
        );
        assert_eq!(
            timeout_for_target(&DockerCleanTarget::Container {
                id: "111122223333".to_owned(),
                name: "old-job".to_owned(),
            }),
            Duration::from_mins(2)
        );

        let recorded = Arc::new(Mutex::new(Vec::new()));
        let runner = TimeoutRecordingRunner {
            recorded: Arc::clone(&recorded),
        };
        let adapter = DockerFactory::create_for(DockerOs::Linux);
        let _ =
            adapter.probe_binary_with(Path::new("/usr/bin/docker"), &runner);
        let summary = DockerUsageSummary {
            build_cache_reclaimable_bytes: 128,
            removable_images: vec![DockerImageEntry {
                id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
                repository: "alpine".to_owned(),
                tag: "3.20".to_owned(),
                created_since: "1d".to_owned(),
                size_bytes: 10,
                unique_bytes: 10,
                dangling: false,
            }],
            stopped_containers: vec![DockerContainerEntry {
                id: "111122223333".to_owned(),
                name: "old-job".to_owned(),
                image: "alpine:3.20".to_owned(),
                state: "exited".to_owned(),
                status: "Exited (0)".to_owned(),
                size_bytes: 5,
            }],
            detailed: true,
            ..DockerUsageSummary::default()
        };
        let targets = vec![
            DockerCleanTarget::Container {
                id: "111122223333".to_owned(),
                name: "old-job".to_owned(),
            },
            DockerCleanTarget::Image {
                id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
                reference: Some("alpine:3.20".to_owned()),
                dangling: false,
            },
            DockerCleanTarget::BuildCache,
        ];
        let outcomes = adapter.run_clean_targets_with(
            Path::new("/usr/bin/docker"),
            &summary,
            &targets,
            &runner,
        );
        assert_eq!(outcomes.len(), 3);
        let calls = recorded.lock().expect("lock").clone();
        assert_eq!(calls[0].1, Duration::from_secs(15));
        assert_eq!(calls[1].1, Duration::from_mins(2));
        assert_eq!(calls[2].1, Duration::from_mins(2));
        assert_eq!(calls[3].1, Duration::from_mins(5));
    }

    #[test]
    fn multi_tag_same_id_images_are_preserved_and_batched_in_order_with_progress()
     {
        let shared_id = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let summary = DockerUsageSummary {
            build_cache_reclaimable_bytes: 1024,
            removable_images: vec![
                DockerImageEntry {
                    id: shared_id.to_owned(),
                    repository: "ghcr.io/demo/app".to_owned(),
                    tag: "v1".to_owned(),
                    created_since: "1d".to_owned(),
                    size_bytes: 200,
                    unique_bytes: 200,
                    dangling: false,
                },
                DockerImageEntry {
                    id: shared_id.to_owned(),
                    repository: "ghcr.io/demo/app".to_owned(),
                    tag: "v2".to_owned(),
                    created_since: "1d".to_owned(),
                    size_bytes: 200,
                    unique_bytes: 200,
                    dangling: false,
                },
            ],
            stopped_containers: vec![
                DockerContainerEntry {
                    id: "111122223333".to_owned(),
                    name: "job-a".to_owned(),
                    image: "ghcr.io/demo/app:v1".to_owned(),
                    state: "exited".to_owned(),
                    status: "Exited (0)".to_owned(),
                    size_bytes: 20,
                },
                DockerContainerEntry {
                    id: "444455556666".to_owned(),
                    name: "job-b".to_owned(),
                    image: "ghcr.io/demo/app:v2".to_owned(),
                    state: "exited".to_owned(),
                    status: "Exited (0)".to_owned(),
                    size_bytes: 30,
                },
            ],
            detailed: true,
            ..DockerUsageSummary::default()
        };

        let requested = vec![
            DockerCleanTarget::BuildCache,
            DockerCleanTarget::Image {
                id: shared_id.to_owned(),
                reference: Some("ghcr.io/demo/app:v2".to_owned()),
                dangling: false,
            },
            DockerCleanTarget::Image {
                id: shared_id.to_owned(),
                reference: Some("ghcr.io/demo/app:v1".to_owned()),
                dangling: false,
            },
            DockerCleanTarget::Image {
                id: shared_id.to_owned(),
                reference: Some("ghcr.io/demo/app:v1".to_owned()),
                dangling: false,
            },
            DockerCleanTarget::Container {
                id: "444455556666".to_owned(),
                name: "job-b".to_owned(),
            },
            DockerCleanTarget::Container {
                id: "111122223333".to_owned(),
                name: "job-a".to_owned(),
            },
        ];

        let runner = MockRunner::new(|_, _| {
            Ok(DockerCommandOutput {
                status_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        });
        let adapter = DockerFactory::create_for(DockerOs::Linux);
        let mut progress = Vec::new();
        let outcomes = adapter.run_clean_targets_with_progress(
            Path::new("/usr/bin/docker"),
            &summary,
            &requested,
            &runner,
            &mut |stage, done, total| {
                progress.push((stage.to_owned(), done, total));
            },
        );
        assert_eq!(outcomes.len(), 5);
        assert!(outcomes.iter().all(|o| o.result.is_ok()));

        let calls = runner.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls[0].1,
            vec!["container", "rm", "--", "444455556666", "111122223333"]
        );
        assert_eq!(
            calls[1].1,
            vec![
                "image",
                "rm",
                "--",
                "ghcr.io/demo/app:v2",
                "ghcr.io/demo/app:v1"
            ]
        );
        assert_eq!(calls[2].1, vec!["builder", "prune", "-f"]);

        assert!(!progress.is_empty());
        assert_eq!(progress.first().unwrap().0, "containers");
        assert_eq!(progress.last().unwrap(), &("build_cache".to_owned(), 5, 5));
        assert!(progress.windows(2).all(|w| w[0].1 <= w[1].1));
        assert!(
            progress
                .iter()
                .all(|(_, done, total)| *done <= *total && *total == 5)
        );
    }

    #[test]
    fn batch_failure_falls_back_per_item_and_retries_dependent_images() {
        let img_parent = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
        let img_child = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
        let summary = DockerUsageSummary {
            build_cache_reclaimable_bytes: 512,
            removable_images: vec![
                DockerImageEntry {
                    id: img_parent.to_owned(),
                    repository: "demo/base".to_owned(),
                    tag: "v1".to_owned(),
                    created_since: "2d".to_owned(),
                    size_bytes: 100,
                    unique_bytes: 100,
                    dangling: false,
                },
                DockerImageEntry {
                    id: img_child.to_owned(),
                    repository: "demo/app".to_owned(),
                    tag: "v2".to_owned(),
                    created_since: "1d".to_owned(),
                    size_bytes: 100,
                    unique_bytes: 100,
                    dangling: false,
                },
            ],
            stopped_containers: vec![
                DockerContainerEntry {
                    id: "111122223333".to_owned(),
                    name: "c1".to_owned(),
                    image: "demo/app:v2".to_owned(),
                    state: "exited".to_owned(),
                    status: "Exited (0)".to_owned(),
                    size_bytes: 10,
                },
                DockerContainerEntry {
                    id: "444455556666".to_owned(),
                    name: "c2".to_owned(),
                    image: "demo/app:v2".to_owned(),
                    state: "exited".to_owned(),
                    status: "Exited (0)".to_owned(),
                    size_bytes: 10,
                },
            ],
            detailed: true,
            ..DockerUsageSummary::default()
        };

        let child_removed = Arc::new(Mutex::new(false));
        let child_removed_cb = Arc::clone(&child_removed);
        let runner = MockRunner::new(move |_, args| match args {
            ["container", "rm", "--", "111122223333", "444455556666"] => {
                Ok(DockerCommandOutput {
                    status_code: 1,
                    stdout: String::new(),
                    stderr: "batch container rm failed".to_owned(),
                })
            }
            ["container", "rm", "--", "111122223333"] => {
                Ok(DockerCommandOutput {
                    status_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
            ["container", "rm", "--", "444455556666"] => {
                Ok(DockerCommandOutput {
                    status_code: 1,
                    stdout: String::new(),
                    stderr: "container still in use".to_owned(),
                })
            }
            ["image", "rm", "--", "demo/base:v1", "demo/app:v2"] => {
                Ok(DockerCommandOutput {
                    status_code: 1,
                    stdout: String::new(),
                    stderr: "batch image rm conflict".to_owned(),
                })
            }
            ["image", "rm", "--", "demo/app:v2"] => {
                *child_removed_cb.lock().expect("lock") = true;
                Ok(DockerCommandOutput {
                    status_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
            ["image", "rm", "--", "demo/base:v1"] => {
                if *child_removed_cb.lock().expect("lock") {
                    Ok(DockerCommandOutput {
                        status_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                } else {
                    Ok(DockerCommandOutput {
                        status_code: 1,
                        stdout: String::new(),
                        stderr: "image has dependent child images".to_owned(),
                    })
                }
            }
            ["builder", "prune", "-f"] => Ok(DockerCommandOutput {
                status_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }),
            _ => unreachable!("unexpected args: {args:?}"),
        });

        let requested = vec![
            DockerCleanTarget::Container {
                id: "111122223333".to_owned(),
                name: "c1".to_owned(),
            },
            DockerCleanTarget::Container {
                id: "444455556666".to_owned(),
                name: "c2".to_owned(),
            },
            DockerCleanTarget::Image {
                id: img_parent.to_owned(),
                reference: Some("demo/base:v1".to_owned()),
                dangling: false,
            },
            DockerCleanTarget::Image {
                id: img_child.to_owned(),
                reference: Some("demo/app:v2".to_owned()),
                dangling: false,
            },
            DockerCleanTarget::BuildCache,
        ];
        let adapter = DockerFactory::create_for(DockerOs::Linux);
        let outcomes = adapter.run_clean_targets_with(
            Path::new("/usr/bin/docker"),
            &summary,
            &requested,
            &runner,
        );
        assert_eq!(outcomes.len(), 5);
        assert!(outcomes[0].result.is_ok());
        assert_eq!(
            outcomes[1].result.as_ref().map_err(String::as_str),
            Err("container still in use")
        );
        assert!(outcomes[2].result.is_ok());
        assert!(outcomes[3].result.is_ok());
        assert!(outcomes[4].result.is_ok());

        let calls = runner.calls();
        assert_eq!(calls.len(), 8);
        assert_eq!(calls[6].1, vec!["image", "rm", "--", "demo/base:v1"]);
        assert_eq!(calls[7].1, vec!["builder", "prune", "-f"]);
    }

    #[test]
    fn verbose_df_splits_shared_build_cache_and_allows_build_cache_cleanup() {
        let json = r#"{
            "Images": [],
            "Containers": [],
            "Volumes": [],
            "BuildCache": [
                {"ID": "bc1", "InUse": "false", "Shared": "true", "Size": "2GB", "Reclaimable": "0B"},
                {"ID": "bc2", "InUse": false, "Shared": false, "Size": "500MB"},
                {"ID": "bc3", "InUse": true, "Shared": true, "Size": "1GB"}
            ]
        }"#;
        let summary = parse_verbose_df_json(json).expect("parsed verbose df");
        assert_eq!(summary.build_cache.total_count, 3);
        assert_eq!(summary.build_cache.active_count, 1);
        assert_eq!(summary.build_cache.size_bytes, 3_500_000_000);
        assert_eq!(summary.build_cache_shared_bytes, 2_000_000_000);
        assert_eq!(summary.build_cache_reclaimable_bytes, 500_000_000);
        assert_eq!(summary.build_cache.reclaimable_bytes, 500_000_000);

        let shared_only = DockerUsageSummary {
            build_cache_reclaimable_bytes: 0,
            build_cache_shared_bytes: 2_000_000_000,
            detailed: true,
            ..DockerUsageSummary::default()
        };
        let validated = validate_targets_against_summary(
            &[DockerCleanTarget::BuildCache],
            &shared_only,
        );
        assert_eq!(validated, vec![DockerCleanTarget::BuildCache]);
    }

    #[test]
    fn probe_falls_back_to_summary_mode_when_verbose_df_times_out() {
        let runner = MockRunner::new(|_, args| {
            if args.contains(&"-v") {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "docker command timed out after 15s",
                ))
            } else {
                Ok(DockerCommandOutput {
                    status_code: 0,
                    stdout: concat!(
                        r#"{"Active":"1","Reclaimable":"500MB (50%)","Size":"1GB","TotalCount":"2","Type":"Images"}"#,
                        "\n",
                        r#"{"Active":"0","Reclaimable":"300MB","Size":"300MB","TotalCount":"4","Type":"Build Cache"}"#,
                        "\n"
                    )
                    .to_owned(),
                    stderr: String::new(),
                })
            }
        });
        let adapter = DockerFactory::create_for(DockerOs::Linux);
        let status =
            adapter.probe_binary_with(Path::new("/usr/bin/docker"), &runner);
        let DockerProbeStatus::Available(summary) = status else {
            panic!("expected Available on timeout fallback, got {status:?}");
        };
        assert!(!summary.detailed);
        assert_eq!(summary.images.total_count, 2);
        assert_eq!(summary.build_cache_reclaimable_bytes, 300_000_000);
        assert_eq!(runner.calls().len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn streaming_output_resets_idle_timeout_while_command_stays_active() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("streaming-docker");
        std::fs::write(
            &script,
            "#!/bin/sh\ni=0\nwhile [ $i -lt 5 ]; do\n  echo \"tick $i\"\n  i=$((i + 1))\n  sleep 0.045\ndone\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &script,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let out = run_docker_command(
            &script,
            &["builder", "prune", "-f"],
            Duration::from_millis(140),
        )
        .expect("streaming output should extend idle timeout");
        assert_eq!(out.status_code, 0);
        assert!(out.stdout.contains("tick 4"));
    }
}
