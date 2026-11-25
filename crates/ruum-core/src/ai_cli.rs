//! AI CLI assistant integration, prompt generation and proposal validation.
//!
//! Provides command building for AI CLI assistants (`Pi`, `Agy`, `Codex`, and
//! `Custom`), enriched `PATH` resolution, strict read-only prompt generation
//! pointing at `<current_exe> mcp --read-only`, and parsing of JSON proposals
//! verified through [`crate::removal::plan`] and [`crate::git::state`].

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::future::Future;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::classify::Category;
use crate::cloud_client::{
    CloudApiError, CloudClient, select_best_available_model,
};
use crate::credentials::LazyCredentialStore;
use crate::docker_adapter::{
    DockerCleanTarget, DockerProbeStatus, DockerUsageSummary,
    validate_targets_against_summary,
};
use crate::i18n::{Locale, format_cloud_error_message};
use crate::identity::HostIdentity;
use crate::insights::{self, Finding};
use crate::mcp::{McpServer, local_read_only_tool_definitions};
use crate::remote::Header;
use crate::removal::RemovalMode;
use crate::size::human_bytes;
use crate::storage::{AiConfig, StorageDb};
use crate::trash_adapter::TrashOs;
use crate::tree::{Node, NodeKind, path_of};
use crate::watchdog_config::{
    AiCliProvider, ApiProtocol, CleanupProposal, ProposalItem, WatchdogConfig,
};

/// Environment variable checked as a headless fallback for direct API keys.
pub const ENV_AI_API_KEY: &str = "RUUM_AI_API_KEY";

/// Sentinel model identifier sent to Ruum Cloud when Auto routing is active.
pub const CLOUD_AUTO_MODEL_ID: &str = "auto";

/// Returns `true` when `RUUM_AI_API_KEY` is present and non-empty.
#[must_use]
pub fn env_api_key_present() -> bool {
    std::env::var(ENV_AI_API_KEY).is_ok_and(|val| !val.trim().is_empty())
}

fn ai_config_from_legacy(config: &WatchdogConfig) -> AiConfig {
    AiConfig {
        auto_analyze_on_alert: config.ai_auto_analyze,
        provider: config.ai_provider,
        custom_command: config.custom_command.clone(),
        extra_args: config.extra_args.clone(),
        model: config.model.clone(),
        ..AiConfig::default()
    }
}

/// Default timeout for full AI analysis CLI runs (seconds).
pub const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 120;
/// Short timeout for quick CLI probes, model listing, and speed checks.
pub const SHORT_COMMAND_TIMEOUT_SECS: u64 = 30;

/// Result of running a subprocess command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    #[must_use]
    pub const fn success(&self) -> bool {
        self.status_code == 0
    }
}

/// Abstract runner for external CLI commands.
pub trait CommandRunner: Send + Sync {
    fn run(
        &self,
        program: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> io::Result<CommandOutput>;
}

fn is_windows_batch_script(program: &Path) -> bool {
    program
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")
        })
}

fn sanitize_windows_batch_arg(arg: &str) -> String {
    arg.replace("\r\n", " ").replace(['\r', '\n'], " ")
}

fn extract_npm_shim_script_path(
    content: &str,
    shim_dir: &Path,
) -> Option<PathBuf> {
    const PREFIXES: [&str; 4] = ["%~dp0\\", "%dp0%\\", "%~dp0/", "%dp0%/"];
    let lower = content.to_ascii_lowercase();
    let mut cursor = 0;

    while cursor < lower.len() {
        let rest = &lower[cursor..];
        let next = PREFIXES
            .iter()
            .filter_map(|prefix| rest.find(prefix).map(|pos| (pos, *prefix)))
            .min_by_key(|(pos, _)| *pos);
        let Some((rel_pos, prefix)) = next else {
            break;
        };
        let match_start = cursor + rel_pos;
        let path_start = match_start + prefix.len();
        cursor = path_start;

        let quoted =
            match_start > 0 && content.as_bytes()[match_start - 1] == b'"';
        let tail = &content[path_start..];
        let path_len = if quoted {
            let Some(end_quote) = tail.find('"') else {
                continue;
            };
            if tail[..end_quote].contains(['\r', '\n']) {
                continue;
            }
            end_quote
        } else {
            tail.find(|c: char| {
                c.is_ascii_whitespace() || matches!(c, '"' | '&' | '|' | ')')
            })
            .unwrap_or(tail.len())
        };

        let rel_path = &tail[..path_len];
        if rel_path.ends_with(['\\', '/']) {
            continue;
        }

        let mut candidate = shim_dir.to_path_buf();
        let mut has_segment = false;
        let mut rejected = false;
        for seg in rel_path.split(['\\', '/']) {
            if seg.is_empty() || seg == "." {
                continue;
            }
            if seg == ".." {
                rejected = true;
                break;
            }
            candidate.push(seg);
            has_segment = true;
        }
        let is_js = candidate
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("js")
                    || ext.eq_ignore_ascii_case("cjs")
                    || ext.eq_ignore_ascii_case("mjs")
            });
        if !rejected && has_segment && is_js && candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

fn unwrap_npm_cmd_shim_with(
    program: &Path,
    is_windows: bool,
    enriched_path: &str,
) -> Option<(PathBuf, PathBuf)> {
    const MAX_SHIM_BYTES: u64 = 32_768;

    if !is_windows || !is_windows_batch_script(program) {
        return None;
    }
    let meta = fs::metadata(program).ok()?;
    if meta.len() > MAX_SHIM_BYTES {
        return None;
    }
    let shim_dir = program.parent()?;
    let content = fs::read_to_string(program).ok()?;
    let js_path = extract_npm_shim_script_path(&content, shim_dir)?;
    let node_exe = resolve_with_windows_exts(Some(shim_dir), "node", true)
        .or_else(|| resolve_cli_executable_with("node", true, enriched_path))?;
    Some((node_exe, js_path))
}

/// Run an external command with piped output, non-interactive defaults, and timeout.
fn run_command_with_timeout(
    program: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
) -> io::Result<CommandOutput> {
    let (effective_program, effective_args): (PathBuf, Vec<String>) =
        if cfg!(windows) && is_windows_batch_script(program) {
            let path_env = envs
                .iter()
                .rev()
                .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
                .map_or_else(enriched_path_var, |(_, v)| (*v).to_string());
            if let Some((node_exe, js_script)) =
                unwrap_npm_cmd_shim_with(program, true, &path_env)
            {
                let mut node_args = Vec::with_capacity(args.len() + 1);
                node_args.push(js_script.to_string_lossy().into_owned());
                node_args.extend(args.iter().copied().map(str::to_owned));
                (node_exe, node_args)
            } else {
                (
                    program.to_path_buf(),
                    args.iter()
                        .copied()
                        .map(sanitize_windows_batch_arg)
                        .collect(),
                )
            }
        } else {
            (
                program.to_path_buf(),
                args.iter().copied().map(str::to_owned).collect(),
            )
        };

    let mut cmd = Command::new(&effective_program);
    cmd.args(&effective_args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // Discourage interactive TUI prompts or ANSI progress spinners.
    cmd.env("TERM", "dumb");
    cmd.env("NO_COLOR", "1");
    for (k, v) in envs {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn()?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    let mut backoff = Duration::from_millis(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            // Skip joining reader threads on timeout: grandchildren (e.g.
            // `sleep 3` spawned by `dash` via `sh -c`) can keep the pipe
            // write end open after the direct child is killed, blocking join.
            drop((stdout_handle, stderr_handle));
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "command `{}` timed out after {}s",
                    program.display(),
                    timeout.as_secs()
                ),
            ));
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_millis(25));
    };

    let stdout_bytes = stdout_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();

    Ok(CommandOutput {
        status_code: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
    })
}

/// Default system command runner using [`std::process::Command`] with timeout.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultCommandRunner;

impl CommandRunner for DefaultCommandRunner {
    fn run(
        &self,
        program: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> io::Result<CommandOutput> {
        run_command_with_timeout(
            program,
            args,
            envs,
            Duration::from_secs(DEFAULT_COMMAND_TIMEOUT_SECS),
        )
    }
}

/// System command runner with a configurable execution timeout.
#[derive(Clone, Copy, Debug)]
pub struct TimedCommandRunner {
    pub timeout: Duration,
}

impl TimedCommandRunner {
    #[must_use]
    pub const fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl CommandRunner for TimedCommandRunner {
    fn run(
        &self,
        program: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> io::Result<CommandOutput> {
        run_command_with_timeout(program, args, envs, self.timeout)
    }
}

fn candidate_bin_dirs_for(
    is_macos: bool,
    is_linux: bool,
    is_windows: bool,
    home: Option<PathBuf>,
    local_app: Option<PathBuf>,
    app_data: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
    }
    if is_macos {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    } else if is_linux {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
        dirs.push(PathBuf::from("/home/linuxbrew/.linuxbrew/bin"));
    } else if is_windows {
        if let Some(local) = local_app.filter(|p| !p.as_os_str().is_empty()) {
            dirs.push(local.join("Programs"));
        }
        if let Some(roaming) = app_data.filter(|p| !p.as_os_str().is_empty()) {
            dirs.push(roaming.join("npm"));
        }
    }
    dirs
}

/// Candidate directories checked when probing CLI assistants.
#[must_use]
pub fn candidate_bin_dirs() -> Vec<PathBuf> {
    candidate_bin_dirs_for(
        cfg!(target_os = "macos"),
        cfg!(target_os = "linux"),
        cfg!(windows),
        std::env::home_dir(),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        std::env::var_os("APPDATA").map(PathBuf::from),
    )
}

/// Build an enriched `PATH` environment variable string.
#[must_use]
pub fn enriched_path_var() -> String {
    let mut entries = Vec::new();
    let current_path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };

    for candidate in candidate_bin_dirs() {
        let cand_str = candidate.display().to_string();
        if candidate.is_dir() && !entries.contains(&cand_str) {
            entries.push(cand_str);
        }
    }
    for item in current_path.split(sep) {
        let s = item.trim();
        if !s.is_empty() && !entries.iter().any(|e| e == s) {
            entries.push(s.to_string());
        }
    }
    entries.join(&sep.to_string())
}

fn resolve_with_windows_exts(
    base_dir: Option<&Path>,
    name: &str,
    check_win_exts: bool,
) -> Option<PathBuf> {
    let exact = match base_dir {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    };
    if exact.is_file() {
        return Some(exact);
    }
    if check_win_exts {
        for ext in ["exe", "cmd", "bat"] {
            let suffixed = format!("{name}.{ext}");
            let cand = match base_dir {
                Some(dir) => dir.join(&suffixed),
                None => PathBuf::from(&suffixed),
            };
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn resolve_cli_executable_with(
    command_or_name: &str,
    is_windows: bool,
    enriched_path: &str,
) -> Option<PathBuf> {
    let trimmed = command_or_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    if path.is_absolute() || path.components().count() > 1 {
        return resolve_with_windows_exts(None, trimmed, is_windows);
    }

    let sep = if is_windows { ';' } else { ':' };
    for dir_str in enriched_path.split(sep) {
        if dir_str.trim().is_empty() {
            continue;
        }
        let dir = Path::new(dir_str);
        if let Some(found) =
            resolve_with_windows_exts(Some(dir), trimmed, is_windows)
        {
            return Some(found);
        }
    }
    None
}

/// Locate a command binary in the enriched `PATH` or as a direct path.
#[must_use]
pub fn resolve_cli_executable(command_or_name: &str) -> Option<PathBuf> {
    resolve_cli_executable_with(
        command_or_name,
        cfg!(windows),
        &enriched_path_var(),
    )
}

/// Concrete invocation arguments for an AI CLI assistant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliInvocation {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl CliInvocation {
    #[must_use]
    pub const fn new(program: PathBuf, args: Vec<String>) -> Self {
        Self { program, args }
    }
}

/// Compact pre-scanned reclaimable candidate embedded in the analysis prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclaimCandidateSummary {
    pub path: PathBuf,
    pub bytes: u64,
    pub category: &'static str,
    pub finding: String,
    pub git_summary: Option<String>,
}

fn format_finding_label(finding: &Finding) -> String {
    match finding {
        Finding::Reclaimable(reason) => reason.label().to_string(),
        Finding::Worktrees { count, oldest_days } => {
            format!("{count} worktrees (oldest {oldest_days}d)")
        }
        Finding::StaleExperiments { count } => {
            format!("{count} stale experiments")
        }
    }
}

fn collect_bounded_headless_candidates(
    home_dir: &Path,
    out: &mut Vec<ReclaimCandidateSummary>,
) {
    let fixed_dirs = [
        ("Library/Caches", "Cache", "macOS application cache"),
        (".cache", "Cache", "User cache directory"),
        (
            ".cargo/registry/cache",
            "Cache",
            "Cargo crate archive cache",
        ),
        (".npm", "Cache", "npm cache directory"),
        (".pnpm-store", "Cache", "pnpm store cache"),
        (".gradle/caches", "Cache", "Gradle dependency cache"),
        (
            "Library/Developer/Xcode/DerivedData",
            "Build",
            "Xcode derived build artifacts",
        ),
    ];

    let mut candidate_paths: Vec<(PathBuf, &'static str, &'static str)> =
        Vec::new();
    for (rel, cat, desc) in fixed_dirs {
        let p = home_dir.join(rel);
        if p.is_dir() {
            candidate_paths.push((p, cat, desc));
        }
    }

    // Bounded depth-3 walk for project build/dependency artifacts under home.
    let artifact_names = [
        "target",
        "node_modules",
        ".venv",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        "DerivedData",
    ];
    let mut stack: Vec<(PathBuf, usize)> = vec![(home_dir.to_path_buf(), 0)];
    let mut visited_dirs = 0_usize;
    while let Some((dir, depth)) = stack.pop() {
        if depth >= 3 || visited_dirs >= 4_000 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited_dirs += 1;
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_dir() || meta.is_symlink() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == ".git" || name == "Library" {
                continue;
            }
            if artifact_names.contains(&name) {
                candidate_paths.push((
                    path,
                    "Build",
                    "Build or dependency artifact directory",
                ));
                continue;
            }
            if !name.starts_with('.') {
                stack.push((path, depth + 1));
            }
        }
    }

    let mut targets = Vec::new();
    let mut meta_map: Vec<(PathBuf, &'static str, &'static str)> = Vec::new();
    for (path, cat, desc) in candidate_paths {
        let normalized = crate::removal::normalize(&path);
        if meta_map.iter().any(|(p, ..)| p == &normalized) {
            continue;
        }
        let mut visited = 0;
        let bytes = compute_dir_bytes(&normalized, 0, &mut visited);
        if bytes < 1024 {
            continue;
        }
        let hidden = normalized
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'));
        targets.push(crate::removal::Target {
            path: normalized.clone(),
            bytes,
            is_dir: true,
            hidden,
        });
        meta_map.push((normalized, cat, desc));
    }

    let safe = crate::removal::plan(&targets, home_dir);
    for target in safe.targets {
        if let Some((_, cat, desc)) =
            meta_map.iter().find(|(p, ..)| p == &target.path)
        {
            let git_summary =
                crate::git::state(&target.path).map(|s| s.summary());
            out.push(ReclaimCandidateSummary {
                path: target.path,
                bytes: target.bytes,
                category: cat,
                finding: (*desc).to_string(),
                git_summary,
            });
        }
    }
}

/// Collect top reclaimable candidates under `home_dir`, reusing an existing
/// treemap when already scanned in the GUI, or a bounded walk otherwise.
#[must_use]
pub fn collect_reclaimable_candidates(
    home_dir: &Path,
    existing_tree: Option<&Node>,
    now_secs: u64,
) -> Vec<ReclaimCandidateSummary> {
    let mut out = Vec::new();
    if let Some(tree) = existing_tree {
        let now_i64 = i64::try_from(now_secs).unwrap_or(0);
        for candidate in insights::worth_a_look(tree, now_i64, 25) {
            let Some(node) = tree.resolve(&candidate.crumbs) else {
                continue;
            };
            let path = path_of(home_dir, tree, &candidate.crumbs);
            let git_summary = crate::git::state(&path).map(|s| s.summary());
            out.push(ReclaimCandidateSummary {
                path,
                bytes: candidate.bytes,
                category: node.category.label(),
                finding: format_finding_label(&candidate.finding),
                git_summary,
            });
        }
    } else {
        collect_bounded_headless_candidates(home_dir, &mut out);
    }

    out.sort_by_key(|item| std::cmp::Reverse(item.bytes));
    out.truncate(20);
    out
}

fn format_candidates_block(candidates: &[ReclaimCandidateSummary]) -> String {
    use std::fmt::Write as _;

    let mut cand_block = String::new();
    if candidates.is_empty() {
        cand_block.push_str(
            "Pre-scanned reclaimable candidates: none detected.\n\
             If you cannot discover safe targets in read-only mode, return an \
             empty `items` array.\n",
        );
    } else {
        cand_block.push_str(
            "PRE-SCANNED RECLAIMABLE CANDIDATES (already verified inside home \
             directory):\n",
        );
        for (idx, item) in candidates.iter().enumerate() {
            let git_note = item
                .git_summary
                .as_deref()
                .map_or(String::new(), |g| format!(", git: {g}"));
            let _ = writeln!(
                cand_block,
                "{}. {} ({}, {}, {}{})",
                idx + 1,
                item.path.display(),
                human_bytes(item.bytes),
                item.category,
                item.finding,
                git_note
            );
        }
        cand_block.push('\n');
    }
    cand_block
}

/// Generate the strict read-only diagnostic prompt including pre-scanned items.
#[must_use]
pub fn build_analysis_prompt_with_candidates(
    home_dir: &Path,
    current_exe: &Path,
    candidates: &[ReclaimCandidateSummary],
) -> String {
    let cand_block = format_candidates_block(candidates);

    format!(
        "You are an automated disk cleanup advisor for Ruum.\n\
        The user's home directory is: {home}\n\
        You have access to Ruum's read-only Model Context Protocol (MCP) \
        server via:\n\
        `{exe} mcp --read-only`\n\n\
        CRITICAL SAFETY CONSTRAINTS:\n\
        - This is a STRICT READ-ONLY diagnostic session.\n\
        - You MUST NOT execute any destructive deletions, modifications, \
        or run arbitrary shell commands to remove files.\n\
        - Prefer analyzing the pre-scanned candidates below and returning the \
        JSON object directly in a single response without extra tool calls \
        unless inspection is strictly required.\n\
        - If calling MCP tools, use only read-only discovery tools: \
        scan_directory, find_reclaimable, and check_git_safety.\n\
        - All candidate paths MUST be located inside the home directory \
        ({home}).\n\
        - Target safely reclaimable items such as build artifacts \
        (target, node_modules, .cache, package caches) or old temp files.\n\
        - Check git safety for any repositories before suggesting removal.\n\n\
        {cand_block}\
        You MUST output ONLY a valid JSON object (either raw JSON or within \
        a single ```json markdown block) matching this schema:\n\
        {{\n  \
          \"summary\": \"Brief explanation of reclaimable space found\",\n  \
          \"items\": [\n    \
            {{\n      \
              \"path\": \"/absolute/path/to/reclaimable/target\",\n      \
              \"mode\": \"trash\" or \"permanent\",\n      \
              \"reason\": \"Why this item is safe to clean up\"\n    \
            }}\n  \
          ]\n\
        }}\n\
        Do not include conversational filler outside the JSON.",
        home = home_dir.display(),
        exe = current_exe.display(),
        cand_block = cand_block,
    )
}

fn build_direct_api_analysis_prompt(
    home_dir: &Path,
    candidates: &[ReclaimCandidateSummary],
) -> (String, String) {
    let cand_block = format_candidates_block(candidates);
    let instruction = format!(
        "You are an automated disk cleanup advisor for Ruum.\n\
         The user's target directory is: {home}\n\
         You have direct in-process read-only local inspection tools attached: \
         list_volumes, scan_directory, inspect_subtree, find_reclaimable, \
         check_git_safety, and plan_removal.\n\n\
         CRITICAL SAFETY CONSTRAINTS:\n\
         - This is a STRICT READ-ONLY diagnostic session.\n\
         - Never suggest deleting system paths, the home root, or paths \
         outside {home}.\n\
         - Call the attached read-only tools directly if you need to inspect \
         additional subdirectories, verify git safety, or validate removal \
         guards.\n\
         - Prefer analyzing the pre-scanned candidates when sufficient.\n\
         - Output ONLY a valid JSON object matching the requested schema.",
        home = home_dir.display(),
    );
    let user_prompt = format!(
        "Analyze reclaimable disk space under {home}.\n\n\
         {cand_block}\
         You MUST output ONLY a valid JSON object (either raw JSON or within \
         a single ```json markdown block) matching this schema:\n\
         {{\n  \
           \"summary\": \"Brief explanation of reclaimable space found\",\n  \
           \"items\": [\n    \
             {{\n      \
               \"path\": \"/absolute/path/to/reclaimable/target\",\n      \
               \"mode\": \"trash\" or \"permanent\",\n      \
               \"reason\": \"Why this item is safe to clean up\"\n    \
             }}\n  \
           ]\n\
         }}\n\
         Do not include conversational filler outside the JSON.",
        home = home_dir.display(),
    );
    (instruction, user_prompt)
}

/// Generate the strict read-only diagnostic prompt for the AI assistant.
#[must_use]
pub fn build_analysis_prompt(home_dir: &Path, current_exe: &Path) -> String {
    build_analysis_prompt_with_candidates(home_dir, current_exe, &[])
}

/// Build the concrete invocation for an [`AiConfig`].
pub fn build_invocation_for_config(
    config: &AiConfig,
    prompt: &str,
) -> Result<CliInvocation, String> {
    let has_custom = !config.custom_command.trim().is_empty();
    let (base_cmd, provider) = if has_custom {
        (config.custom_command.trim(), config.provider)
    } else {
        match config.provider {
            AiCliProvider::Pi => ("pi", AiCliProvider::Pi),
            AiCliProvider::Agy => ("agy", AiCliProvider::Agy),
            AiCliProvider::Codex => ("codex", AiCliProvider::Codex),
            AiCliProvider::Custom => {
                return Err(
                    "custom AI provider selected but no command specified"
                        .to_string(),
                );
            }
            AiCliProvider::Api => {
                return Err(
                    "direct API provider does not use an external CLI \
                     executable"
                        .to_string(),
                );
            }
            AiCliProvider::RuumCloud => {
                return Err(
                    "Ruum Cloud provider does not use an external CLI \
                     executable"
                        .to_string(),
                );
            }
        }
    };

    let program = resolve_cli_executable(base_cmd).ok_or_else(|| {
        format!(
            "executable `{base_cmd}` not found in PATH or standard bin \
             directories"
        )
    })?;

    let mut args = Vec::new();
    match provider {
        AiCliProvider::Agy => {
            args.push("-p".to_string());
            args.push(prompt.to_string());
            args.push("--disable-slash-commands".to_string());
            args.push("--settings".to_string());
            args.push(r#"{"permissions":{"allow":["mcp(*)"]}}"#.to_string());
            if let Some(ref m) = config.model {
                args.push("--model".to_string());
                args.push(m.clone());
            }
        }
        AiCliProvider::Pi => {
            args.push("-p".to_string());
            args.push(prompt.to_string());
            args.push("--no-session".to_string());
            if let Some(ref m) = config.model {
                args.push("--model".to_string());
                args.push(m.clone());
            }
        }
        AiCliProvider::Codex => {
            args.push("exec".to_string());
            args.push(prompt.to_string());
            args.push("--skip-git-repo-check".to_string());
            args.push("--ephemeral".to_string());
            if let Some(ref m) = config.model {
                args.push("--model".to_string());
                args.push(m.clone());
            }
        }
        AiCliProvider::Custom => {
            args.push(prompt.to_string());
            if let Some(ref m) = config.model {
                args.push("--model".to_string());
                args.push(m.clone());
            }
        }
        AiCliProvider::Api => {
            return Err("direct API provider does not build CLI invocations"
                .to_string());
        }
        AiCliProvider::RuumCloud => {
            return Err("Ruum Cloud provider does not build CLI invocations"
                .to_string());
        }
    }

    if !config.extra_args.trim().is_empty() {
        for token in config.extra_args.split_whitespace() {
            args.push(token.to_string());
        }
    }

    Ok(CliInvocation::new(program, args))
}

/// Build the concrete invocation for the configured AI provider.
pub fn build_invocation(
    config: &WatchdogConfig,
    prompt: &str,
) -> Result<CliInvocation, String> {
    build_invocation_for_config(&ai_config_from_legacy(config), prompt)
}

/// Probe whether an AI CLI executable can be located.
pub fn probe_cli(
    provider: AiCliProvider,
    custom_command: &str,
) -> Result<PathBuf, String> {
    let custom = custom_command.trim();
    if !custom.is_empty() {
        return resolve_cli_executable(custom).ok_or_else(|| {
            format!("executable `{custom}` could not be found")
        });
    }
    let name = match provider {
        AiCliProvider::Pi => "pi",
        AiCliProvider::Agy => "agy",
        AiCliProvider::Codex => "codex",
        AiCliProvider::Custom => {
            return Err("custom command is empty".to_string());
        }
        AiCliProvider::Api => {
            return Err(
                "direct API provider does not use an external CLI executable"
                    .to_string(),
            );
        }
        AiCliProvider::RuumCloud => {
            return Err(
                "Ruum Cloud provider does not use an external CLI executable"
                    .to_string(),
            );
        }
    };
    resolve_cli_executable(name)
        .ok_or_else(|| format!("executable `{name}` could not be found"))
}

/// Run a short non-destructive connection test against an [`AiConfig`].
pub fn test_connection_for_config(
    config: &AiConfig,
    runner: &dyn CommandRunner,
) -> Result<String, String> {
    test_connection_for_config_with_context(
        config,
        runner,
        &AiExecutionContext::default(),
    )
}

/// Run a short non-destructive connection test against an [`AiConfig`] with an
/// explicit [`AiExecutionContext`].
pub fn test_connection_for_config_with_context(
    config: &AiConfig,
    runner: &dyn CommandRunner,
    ctx: &AiExecutionContext,
) -> Result<String, String> {
    AiBackendFactory::create(config.provider)
        .test_connection(config, runner, ctx)
}

fn cli_test_connection(
    config: &AiConfig,
    runner: &dyn CommandRunner,
) -> Result<String, String> {
    let probe_path = probe_cli(config.provider, &config.custom_command)?;
    let path_env = enriched_path_var();
    let envs = [("PATH", path_env.as_str())];

    let test_args = ["--version"];
    let output = runner.run(&probe_path, &test_args, &envs).map_err(|e| {
        format!("failed to execute `{}`: {e}", probe_path.display())
    })?;

    if output.success() {
        let text = output.stdout.trim();
        let display = if text.is_empty() {
            output.stderr.trim()
        } else {
            text
        };
        Ok(format!(
            "CLI ready at {}: {}",
            probe_path.display(),
            display.lines().next().unwrap_or("ok")
        ))
    } else {
        Err(format!(
            "CLI at {} exited with code {}: {}",
            probe_path.display(),
            output.status_code,
            output.stderr.trim()
        ))
    }
}

/// Run a short non-destructive connection test against the CLI assistant.
pub fn test_connection(
    config: &WatchdogConfig,
    runner: &dyn CommandRunner,
) -> Result<String, String> {
    test_connection_for_config(&ai_config_from_legacy(config), runner)
}

/// An AI model discovered from a CLI assistant or its local configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredModel {
    /// Model identifier passed to `--model` (e.g. `gemini-4-hydrogen-high`).
    pub id: String,
    /// Human-readable display label.
    pub label: String,
    /// Optional upstream provider (populated for multi-provider CLIs like Pi).
    pub provider: Option<String>,
}

/// Combined result of probing CLI connectivity and available models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionTestResult {
    /// CLI readiness & version summary.
    pub version_message: String,
    /// Models discovered for selection.
    pub models: Vec<DiscoveredModel>,
    /// Optional note when model discovery fell back or is unsupported.
    pub model_note: Option<String>,
}

/// Latency & health outcome for a single model speed probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSpeedResult {
    /// Model identifier that was benchmarked (or `(default)`).
    pub model: String,
    /// Round-trip execution time in milliseconds.
    pub elapsed_ms: u64,
    /// Whether the probe command exited cleanly.
    pub ok: bool,
    /// Short response snippet or failure reason.
    pub detail: String,
}

/// Strip `\r` progress-rewrite prefixes and ANSI escape sequences from a line.
#[must_use]
fn clean_terminal_line(line: &str) -> String {
    let latest = line.rsplit('\r').next().unwrap_or("");
    let mut out = String::with_capacity(latest.len());
    let mut chars = latest.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for seq_ch in chars.by_ref() {
                        if ('@'..='~').contains(&seq_ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    for seq_ch in chars.by_ref() {
                        if seq_ch == '\x07' {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if !ch.is_control() || ch == '\t' {
            out.push(ch);
        }
    }
    out
}

fn is_plausible_model_id(token: &str) -> bool {
    !token.is_empty()
        && !token.ends_with(':')
        && !token.ends_with("...")
        && token.bytes().any(|b| b.is_ascii_alphanumeric())
        && token.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'-' | b'_' | b'.' | b'/' | b':')
        })
}

fn push_unique_model(
    out: &mut Vec<DiscoveredModel>,
    seen: &mut HashSet<String>,
    model: DiscoveredModel,
) {
    if seen.insert(model.id.clone()) {
        out.push(model);
    }
}

/// Parse `agy models` stdout into structured model choices.
#[must_use]
pub fn parse_agy_models(raw: &str) -> Vec<DiscoveredModel> {
    let mut models = Vec::new();
    let mut seen = HashSet::new();

    for line in raw.lines() {
        let cleaned = clean_terminal_line(line);
        let trimmed = cleaned.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('-')
            || trimmed.starts_with('=')
            || trimmed.starts_with('─')
            || trimmed.contains("Fetching")
            || trimmed.contains("Loading")
            || trimmed.ends_with("...")
        {
            continue;
        }

        let Some((id, rest)) = trimmed.split_once(char::is_whitespace) else {
            if is_plausible_model_id(trimmed)
                && !trimmed.eq_ignore_ascii_case("model")
            {
                push_unique_model(
                    &mut models,
                    &mut seen,
                    DiscoveredModel {
                        id: trimmed.to_string(),
                        label: trimmed.to_string(),
                        provider: None,
                    },
                );
            }
            continue;
        };

        let label = rest.trim();
        if id.eq_ignore_ascii_case("model") || id.eq_ignore_ascii_case("id") {
            continue;
        }
        if !is_plausible_model_id(id) {
            continue;
        }
        push_unique_model(
            &mut models,
            &mut seen,
            DiscoveredModel {
                id: id.to_string(),
                label: if label.is_empty() {
                    id.to_string()
                } else {
                    label.to_string()
                },
                provider: None,
            },
        );
    }

    models
}

/// Parse `pi --list-models` tabular stdout into `provider/model` entries.
#[must_use]
pub fn parse_pi_models(raw: &str) -> Vec<DiscoveredModel> {
    let mut models = Vec::new();
    let mut seen = HashSet::new();

    for line in raw.lines() {
        let cleaned = clean_terminal_line(line);
        let trimmed = cleaned.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('-')
            || trimmed.starts_with('=')
        {
            continue;
        }

        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() < 2 {
            continue;
        }
        let provider = cols[0];
        let model_name = cols[1];
        if provider.eq_ignore_ascii_case("provider")
            && model_name.eq_ignore_ascii_case("model")
        {
            continue;
        }
        if !is_plausible_model_id(provider)
            || !is_plausible_model_id(model_name)
        {
            continue;
        }

        let prefix = format!("{provider}/");
        let full_id = if model_name.starts_with(&prefix) {
            model_name.to_string()
        } else {
            format!("{provider}/{model_name}")
        };

        push_unique_model(
            &mut models,
            &mut seen,
            DiscoveredModel {
                id: full_id,
                label: format!("{model_name} ({provider})"),
                provider: Some(provider.to_string()),
            },
        );
    }

    models
}

/// Extract top-level `key = "value"` string fields from Codex `config.toml`.
fn parse_codex_config_toml(raw: &str) -> (Option<String>, Option<String>) {
    let mut in_root = true;
    let mut default_model = None;
    let mut catalog_path = None;

    for raw_line in raw.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_root = false;
            continue;
        }
        if !in_root {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'').trim();
        if val.is_empty() {
            continue;
        }
        if key == "model" {
            default_model = Some(val.to_string());
        } else if key == "model_catalog_json" {
            catalog_path = Some(val.to_string());
        }
    }

    (default_model, catalog_path)
}

/// Parse Codex `models_cache.json` or custom `model_catalog_json` payload.
#[must_use]
pub fn parse_codex_models_json(raw: &str) -> Vec<DiscoveredModel> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(entries) = val
        .get("models")
        .and_then(serde_json::Value::as_array)
        .or_else(|| val.as_array())
    else {
        return Vec::new();
    };

    let mut visible = Vec::new();
    let mut hidden = Vec::new();
    let mut seen_visible = HashSet::new();
    let mut seen_hidden = HashSet::new();

    for entry in entries {
        let Some(id) = entry
            .get("slug")
            .or_else(|| entry.get("id"))
            .or_else(|| entry.get("model"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };

        let display_name = entry
            .get("display_name")
            .or_else(|| entry.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(id);

        let is_hidden = entry
            .get("visibility")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|v| v.eq_ignore_ascii_case("hide"));

        let item = DiscoveredModel {
            id: id.to_string(),
            label: display_name.to_string(),
            provider: Some("codex".to_string()),
        };

        if is_hidden {
            push_unique_model(&mut hidden, &mut seen_hidden, item);
        } else {
            push_unique_model(&mut visible, &mut seen_visible, item);
        }
    }

    if visible.is_empty() { hidden } else { visible }
}

/// Read Codex models safely from `~/.codex` without launching the interactive TUI.
#[must_use]
pub fn discover_codex_models_in_dir(codex_dir: &Path) -> Vec<DiscoveredModel> {
    let config_raw =
        fs::read_to_string(codex_dir.join("config.toml")).unwrap_or_default();
    let (default_model, catalog_opt) = parse_codex_config_toml(&config_raw);

    let mut json_models = Vec::new();
    if let Some(catalog) = catalog_opt {
        let p = Path::new(&catalog);
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            codex_dir.join(p)
        };
        if let Ok(raw) = fs::read_to_string(resolved) {
            json_models.extend(parse_codex_models_json(&raw));
        }
    }
    if let Ok(raw) = fs::read_to_string(codex_dir.join("models_cache.json")) {
        json_models.extend(parse_codex_models_json(&raw));
    }

    let mut result = Vec::new();
    let mut seen = HashSet::new();

    if let Some(ref def_id) = default_model {
        let def_label =
            json_models.iter().find(|m| &m.id == def_id).map_or_else(
                || format!("{def_id} (configured)"),
                |m| m.label.clone(),
            );
        push_unique_model(
            &mut result,
            &mut seen,
            DiscoveredModel {
                id: def_id.clone(),
                label: def_label,
                provider: Some("codex".to_string()),
            },
        );
    }

    for model in json_models {
        push_unique_model(&mut result, &mut seen, model);
    }

    result
}

/// Discover selectable models for an [`AiConfig`].
pub fn discover_models_for_config(
    config: &AiConfig,
    runner: &dyn CommandRunner,
) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
    discover_models_for_config_with_context(
        config,
        runner,
        &AiExecutionContext::default(),
    )
}

/// Discover selectable models for an [`AiConfig`] with an explicit
/// [`AiExecutionContext`].
pub fn discover_models_for_config_with_context(
    config: &AiConfig,
    runner: &dyn CommandRunner,
    ctx: &AiExecutionContext,
) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
    AiBackendFactory::create(config.provider)
        .discover_models(config, runner, ctx)
}

fn cli_discover_models(
    config: &AiConfig,
    runner: &dyn CommandRunner,
) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
    let custom = config.custom_command.trim();
    match config.provider {
        AiCliProvider::Agy => {
            let program = if custom.is_empty() {
                probe_cli(AiCliProvider::Agy, "")?
            } else {
                resolve_cli_executable(custom).ok_or_else(|| {
                    format!("executable `{custom}` could not be found")
                })?
            };
            let path_env = enriched_path_var();
            let envs = [("PATH", path_env.as_str())];
            let out = runner
                .run(&program, &["models"], &envs)
                .map_err(|e| format!("failed to list agy models: {e}"))?;
            if !out.success() {
                return Err(format!(
                    "`agy models` exited with code {}: {}",
                    out.status_code,
                    out.stderr.trim()
                ));
            }
            let models = parse_agy_models(&out.stdout);
            let note = models
                .is_empty()
                .then(|| "No models parsed from `agy models`.".to_string());
            Ok((models, note))
        }
        AiCliProvider::Pi => {
            let program = if custom.is_empty() {
                probe_cli(AiCliProvider::Pi, "")?
            } else {
                resolve_cli_executable(custom).ok_or_else(|| {
                    format!("executable `{custom}` could not be found")
                })?
            };
            let path_env = enriched_path_var();
            let envs = [("PATH", path_env.as_str())];
            let out = runner
                .run(&program, &["--list-models", "--offline"], &envs)
                .or_else(|_| runner.run(&program, &["--list-models"], &envs))
                .map_err(|e| format!("failed to list pi models: {e}"))?;
            let out = if out.success() {
                out
            } else {
                runner
                    .run(&program, &["--list-models"], &envs)
                    .map_err(|e| format!("failed to list pi models: {e}"))?
            };
            if !out.success() {
                return Err(format!(
                    "`pi --list-models` exited with code {}: {}",
                    out.status_code,
                    out.stderr.trim()
                ));
            }
            let models = parse_pi_models(&out.stdout);
            let note = models.is_empty().then(|| {
                "No models parsed from `pi --list-models`.".to_string()
            });
            Ok((models, note))
        }
        AiCliProvider::Codex => {
            // Codex CLI has no non-interactive `models` subcommand (`codex
            // models` launches the TUI), so read local `~/.codex` config/cache.
            let models = std::env::home_dir()
                .map(|h| discover_codex_models_in_dir(&h.join(".codex")))
                .unwrap_or_default();
            let note = if models.is_empty() {
                Some(
                    "No models found in ~/.codex/config.toml or \
                     models_cache.json; enter a model ID manually if needed."
                        .to_string(),
                )
            } else {
                Some("Loaded from ~/.codex config & cache.".to_string())
            };
            Ok((models, note))
        }
        AiCliProvider::Custom => Ok((
            Vec::new(),
            Some(
                "Model discovery is unavailable for custom commands; enter a \
                 model override manually if needed."
                    .to_string(),
            ),
        )),
        AiCliProvider::Api => {
            Err("direct API provider must use DirectApiAiAdapter".to_string())
        }
        AiCliProvider::RuumCloud => {
            Err("Ruum Cloud provider must use RuumCloudAiAdapter".to_string())
        }
    }
}

/// Discover selectable models for the configured AI CLI provider.
pub fn discover_models(
    config: &WatchdogConfig,
    runner: &dyn CommandRunner,
) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
    discover_models_for_config(&ai_config_from_legacy(config), runner)
}

/// Probe connectivity and fetch available models for an [`AiConfig`].
pub fn test_connection_and_discover_for_config(
    config: &AiConfig,
    runner: &dyn CommandRunner,
) -> Result<ConnectionTestResult, String> {
    test_connection_and_discover_for_config_with_context(
        config,
        runner,
        &AiExecutionContext::default(),
    )
}

/// Probe connectivity and fetch available models for an [`AiConfig`] with an
/// explicit [`AiExecutionContext`].
pub fn test_connection_and_discover_for_config_with_context(
    config: &AiConfig,
    runner: &dyn CommandRunner,
    ctx: &AiExecutionContext,
) -> Result<ConnectionTestResult, String> {
    let adapter = AiBackendFactory::create(config.provider);
    let version_message = adapter.test_connection(config, runner, ctx)?;
    match adapter.discover_models(config, runner, ctx) {
        Ok((models, model_note)) => Ok(ConnectionTestResult {
            version_message,
            models,
            model_note,
        }),
        Err(err) => Ok(ConnectionTestResult {
            version_message,
            models: Vec::new(),
            model_note: Some(format!("Model discovery note: {err}")),
        }),
    }
}

/// Probe CLI connectivity and fetch available models in one step.
pub fn test_connection_and_discover(
    config: &WatchdogConfig,
    runner: &dyn CommandRunner,
) -> Result<ConnectionTestResult, String> {
    test_connection_and_discover_for_config(
        &ai_config_from_legacy(config),
        runner,
    )
}

fn first_line_snippet(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .map(clean_terminal_line)
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= max_chars {
        line
    } else {
        let prefix: String = line.chars().take(max_chars).collect();
        format!("{prefix}…")
    }
}

/// Run a lightweight round-trip latency test for a specific AI model using
/// [`AiConfig`].
pub fn benchmark_model_for_config(
    config: &AiConfig,
    model_override: Option<&str>,
    runner: &dyn CommandRunner,
) -> ModelSpeedResult {
    benchmark_model_for_config_with_context(
        config,
        model_override,
        runner,
        &AiExecutionContext::default(),
    )
}

/// Run a lightweight round-trip latency test for a specific AI model using
/// [`AiConfig`] and an explicit [`AiExecutionContext`].
pub fn benchmark_model_for_config_with_context(
    config: &AiConfig,
    model_override: Option<&str>,
    runner: &dyn CommandRunner,
    ctx: &AiExecutionContext,
) -> ModelSpeedResult {
    AiBackendFactory::create(config.provider).benchmark_model(
        config,
        model_override,
        runner,
        ctx,
    )
}

fn cli_benchmark_model(
    config: &AiConfig,
    model_override: Option<&str>,
    runner: &dyn CommandRunner,
) -> ModelSpeedResult {
    let mut cfg = config.clone();
    if let Some(override_id) = model_override {
        let trimmed = override_id.trim();
        cfg.model = (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    let model_name = cfg
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "(default)".to_string());

    let invocation =
        match build_invocation_for_config(&cfg, "Reply with only OK.") {
            Ok(inv) => inv,
            Err(err) => {
                return ModelSpeedResult {
                    model: model_name,
                    elapsed_ms: 0,
                    ok: false,
                    detail: err,
                };
            }
        };

    let path_env = enriched_path_var();
    let envs = [("PATH", path_env.as_str())];
    let arg_refs: Vec<&str> =
        invocation.args.iter().map(String::as_str).collect();

    let start = Instant::now();
    let run_res = runner.run(&invocation.program, &arg_refs, &envs);
    let elapsed_ms =
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    match run_res {
        Ok(out) if out.success() => {
            let reply = first_line_snippet(&out.stdout, 60);
            ModelSpeedResult {
                model: model_name,
                elapsed_ms,
                ok: true,
                detail: if reply.is_empty() {
                    "OK".to_string()
                } else {
                    reply
                },
            }
        }
        Ok(out) => {
            let err_snippet = first_line_snippet(&out.stderr, 80);
            let detail = if err_snippet.is_empty() {
                format!("exited with code {}", out.status_code)
            } else {
                format!("exit {}: {err_snippet}", out.status_code)
            };
            ModelSpeedResult {
                model: model_name,
                elapsed_ms,
                ok: false,
                detail,
            }
        }
        Err(err) => ModelSpeedResult {
            model: model_name,
            elapsed_ms,
            ok: false,
            detail: err.to_string(),
        },
    }
}

/// Run a lightweight round-trip latency test for a specific AI model.
pub fn benchmark_model(
    config: &WatchdogConfig,
    model_override: Option<&str>,
    runner: &dyn CommandRunner,
) -> ModelSpeedResult {
    benchmark_model_for_config(
        &ai_config_from_legacy(config),
        model_override,
        runner,
    )
}

/// Check whether a candidate JSON slice is an AI cleanup proposal object.
fn parse_raw_proposal_candidate(slice: &str) -> Option<RawProposal> {
    let val: serde_json::Value = serde_json::from_str(slice).ok()?;
    let obj = val.as_object()?;
    if !obj.contains_key("items")
        && !obj.contains_key("summary")
        && !obj.contains_key("docker_targets")
    {
        return None;
    }
    serde_json::from_value(val).ok()
}

/// Find balanced top-level `{ ... }` spans in `text`, ignoring quoted braces.
fn balanced_json_objects(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut depth = 0_usize;
    let mut start_idx = None;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' if depth > 0 => {
                in_string = true;
            }
            '{' => {
                if depth == 0 {
                    start_idx = Some(idx);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0
                    && let Some(start) = start_idx.take()
                {
                    spans.push(&text[start..=idx]);
                }
            }
            _ => {}
        }
    }
    spans
}

/// Extract JSON content from AI CLI text, handling Markdown code fences.
#[must_use]
pub fn extract_json(raw: &str) -> Option<&str> {
    let mut fence_pos = 0;
    while let Some(rel) = raw[fence_pos..].find("```json") {
        let content_start = fence_pos + rel + 7;
        if let Some(end_rel) = raw[content_start..].find("```") {
            let candidate = raw[content_start..content_start + end_rel].trim();
            if parse_raw_proposal_candidate(candidate).is_some() {
                return Some(candidate);
            }
            fence_pos = content_start + end_rel + 3;
        } else {
            break;
        }
    }
    if let Some(start) = raw.find("```json") {
        let rest = &raw[start + 7..];
        if let Some(end) = rest.find("```") {
            return Some(rest[..end].trim());
        }
    }
    if let Some(start) = raw.find("```") {
        let rest = &raw[start + 3..];
        if let Some(end) = rest.find("```") {
            let candidate = rest[..end].trim();
            if candidate.starts_with('{') {
                return Some(candidate);
            }
        }
    }
    for span in balanced_json_objects(raw).into_iter().rev() {
        if parse_raw_proposal_candidate(span).is_some() {
            return Some(span.trim());
        }
    }
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end >= start {
        Some(raw[start..=end].trim())
    } else {
        None
    }
}

/// Parse an AI output string into a [`RawProposal`], unwrapping `"response"`
/// envelopes when the CLI wraps its output in metadata JSON.
fn extract_proposal_from_output(raw: &str) -> Result<RawProposal, String> {
    // Try direct JSON or `"response"` envelopes from valid JSON objects first,
    // so escaped ````json blocks inside a JSON string aren't extracted raw.
    for span in balanced_json_objects(raw).into_iter().rev() {
        if let Some(proposal) = parse_raw_proposal_candidate(span) {
            return Ok(proposal);
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(span)
            && let Some(inner) =
                val.get("response").and_then(serde_json::Value::as_str)
            && let Ok(proposal) = extract_proposal_from_output(inner)
        {
            return Ok(proposal);
        }
    }

    if let Some(slice) = extract_json(raw)
        && let Some(proposal) = parse_raw_proposal_candidate(slice)
    {
        return Ok(proposal);
    }

    let snippet = first_line_snippet(raw, 140);
    if snippet.is_empty() {
        Err("no valid JSON object found in AI CLI output (empty stdout)"
            .to_string())
    } else {
        Err(format!(
            "no valid JSON object found in AI CLI output: {snippet}"
        ))
    }
}

/// Recursively sum bytes inside a directory up to a bounded depth and count.
fn compute_dir_bytes(dir: &Path, depth: usize, visited: &mut usize) -> u64 {
    if depth > 16 || *visited > 20_000 {
        return 0;
    }
    let mut total = 0_u64;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        *visited += 1;
        if let Ok(meta) = fs::symlink_metadata(entry.path()) {
            total = total.saturating_add(meta.len());
            if meta.is_dir() && !meta.is_symlink() {
                total = total.saturating_add(compute_dir_bytes(
                    &entry.path(),
                    depth + 1,
                    visited,
                ));
            }
        }
    }
    total
}

#[derive(serde::Deserialize)]
struct RawProposal {
    summary: Option<String>,
    #[serde(default)]
    items: Vec<RawItem>,
    #[serde(default)]
    docker_targets: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct RawItem {
    path: String,
    mode: Option<String>,
    reason: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RawDockerTarget {
    BuildCache,
    Image {
        id: String,
        #[serde(default)]
        reference: Option<String>,
        #[serde(default)]
        dangling: bool,
    },
    Container {
        id: String,
        #[serde(default)]
        name: String,
    },
}

impl From<RawDockerTarget> for DockerCleanTarget {
    fn from(raw: RawDockerTarget) -> Self {
        match raw {
            RawDockerTarget::BuildCache => Self::BuildCache,
            RawDockerTarget::Image {
                id,
                reference,
                dangling,
            } => Self::Image {
                id,
                reference,
                dangling,
            },
            RawDockerTarget::Container { id, name } => {
                Self::Container { id, name }
            }
        }
    }
}

/// One high-usage or reclaimable candidate collected purely from an in-memory
/// [`Node`] tree without touching the host filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeCandidate {
    pub path: PathBuf,
    pub bytes: u64,
    pub is_dir: bool,
    pub category: &'static str,
    pub finding: String,
}

/// Combined memory-tree candidates and Docker snapshot summary for AI prompts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeCandidates {
    pub items: Vec<TreeCandidate>,
    pub docker: Option<DockerUsageSummary>,
}

const fn sep_for_os(os: TrashOs) -> char {
    match os {
        TrashOs::Windows => '\\',
        TrashOs::MacOs | TrashOs::Linux => '/',
    }
}

const fn platform_for_os(os: TrashOs) -> &'static str {
    match os {
        TrashOs::MacOs => "macos",
        TrashOs::Windows => "windows",
        TrashOs::Linux => "linux",
    }
}

fn infer_os_from_root(root_path: &Path) -> TrashOs {
    let raw = root_path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let bytes = stripped.as_bytes();
    if (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || raw.starts_with(r"\\")
    {
        TrashOs::Windows
    } else {
        TrashOs::Linux
    }
}

fn join_lexical_for_os(base: &str, seg: &str, os: TrashOs) -> String {
    let sep = sep_for_os(os);
    if base.ends_with(sep) {
        format!("{base}{seg}")
    } else {
        format!("{base}{sep}{seg}")
    }
}

fn is_lexical_absolute(raw: &str, os: TrashOs) -> bool {
    match os {
        TrashOs::Linux | TrashOs::MacOs => raw.starts_with('/'),
        TrashOs::Windows => {
            let stripped = raw.strip_prefix(r"\\?\").unwrap_or(raw);
            let bytes = stripped.as_bytes();
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/')
        }
    }
}

fn normalize_lexical_for_os(
    raw: &str,
    os: TrashOs,
) -> Result<String, &'static str> {
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return Err("empty or invalid control characters in path");
    }
    match os {
        TrashOs::Linux | TrashOs::MacOs => {
            if !raw.starts_with('/') {
                return Err("path is not absolute for Unix");
            }
            let mut parts: Vec<&str> = Vec::new();
            for seg in raw.split('/') {
                match seg {
                    "" | "." => {}
                    ".." => {
                        parts.pop();
                    }
                    other => parts.push(other),
                }
            }
            if parts.is_empty() {
                Ok("/".to_owned())
            } else {
                Ok(format!("/{}", parts.join("/")))
            }
        }
        TrashOs::Windows => {
            let stripped = raw.strip_prefix(r"\\?\").unwrap_or(raw);
            let bytes = stripped.as_bytes();
            if bytes.len() < 3
                || !bytes[0].is_ascii_alphabetic()
                || bytes[1] != b':'
                || !matches!(bytes[2], b'\\' | b'/')
            {
                return Err("path is not absolute for Windows");
            }
            let drive = bytes[0].to_ascii_uppercase() as char;
            let mut parts: Vec<&str> = Vec::new();
            for seg in stripped[3..].split(['\\', '/']) {
                match seg {
                    "" | "." => {}
                    ".." => {
                        parts.pop();
                    }
                    other => parts.push(other),
                }
            }
            if parts.is_empty() {
                Ok(format!("{drive}:\\"))
            } else {
                Ok(format!("{drive}:\\{}", parts.join("\\")))
            }
        }
    }
}

fn relative_segments_for_os<'a>(
    resolved_norm: &'a str,
    root_norm: &str,
    os: TrashOs,
) -> Option<Vec<&'a str>> {
    match os {
        TrashOs::Linux | TrashOs::MacOs => {
            if resolved_norm == root_norm {
                return Some(Vec::new());
            }
            if root_norm == "/" {
                let tail = resolved_norm.strip_prefix('/')?;
                return Some(
                    tail.split('/').filter(|s| !s.is_empty()).collect(),
                );
            }
            let prefix = format!("{root_norm}/");
            let rest = if os == TrashOs::MacOs {
                let rest = resolved_norm.get(root_norm.len()..)?;
                if !resolved_norm
                    .get(..root_norm.len())?
                    .eq_ignore_ascii_case(root_norm)
                    || !rest.starts_with('/')
                {
                    return None;
                }
                &rest[1..]
            } else {
                resolved_norm.strip_prefix(&prefix)?
            };
            Some(rest.split('/').filter(|s| !s.is_empty()).collect())
        }
        TrashOs::Windows => {
            if resolved_norm.eq_ignore_ascii_case(root_norm) {
                return Some(Vec::new());
            }
            if root_norm.len() == 3 && root_norm.ends_with(":\\") {
                if !resolved_norm
                    .get(..3)
                    .is_some_and(|p| p.eq_ignore_ascii_case(root_norm))
                {
                    return None;
                }
                let tail = &resolved_norm[3..];
                return Some(
                    tail.split('\\').filter(|s| !s.is_empty()).collect(),
                );
            }
            if !resolved_norm
                .get(..root_norm.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(root_norm))
            {
                return None;
            }
            let tail =
                resolved_norm.get(root_norm.len()..)?.strip_prefix('\\')?;
            Some(tail.split('\\').filter(|s| !s.is_empty()).collect())
        }
    }
}

fn find_child_for_os<'a>(
    parent: &'a Node,
    segment: &str,
    os: TrashOs,
) -> Option<&'a Node> {
    if let Some(exact) = parent.children.iter().find(|c| &*c.name == segment) {
        return Some(exact);
    }
    match os {
        TrashOs::Linux => None,
        TrashOs::MacOs | TrashOs::Windows => parent
            .children
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(segment)),
    }
}

fn resolve_node_in_tree<'a>(
    tree: &'a Node,
    root_norm: &str,
    segments: &[&str],
    os: TrashOs,
) -> Option<(String, &'a Node)> {
    if segments.is_empty() || tree.kind == NodeKind::Symlink {
        return None;
    }
    let mut current = tree;
    let mut canonical = root_norm.to_owned();
    for (idx, seg) in segments.iter().enumerate() {
        if !current.is_dir() {
            return None;
        }
        let child = find_child_for_os(current, seg, os)?;
        if child.kind == NodeKind::Symlink || child.kind == NodeKind::Other {
            return None;
        }
        canonical = join_lexical_for_os(&canonical, &child.name, os);
        if idx + 1 < segments.len() && !child.is_dir() {
            return None;
        }
        current = child;
    }
    Some((canonical, current))
}

fn path_from_crumbs_for_os<'a>(
    root_norm: &str,
    tree: &'a Node,
    crumbs: &[usize],
    os: TrashOs,
) -> Option<(String, &'a Node)> {
    let mut current = tree;
    let mut full = root_norm.to_owned();
    for &idx in crumbs {
        let child = current.children.get(idx)?;
        if child.kind == NodeKind::Symlink
            || child.kind == NodeKind::Other
            || child.category == Category::Container
        {
            return None;
        }
        full = join_lexical_for_os(&full, &child.name, os);
        current = child;
    }
    Some((full, current))
}

fn collect_bounded_tree_nodes(
    root_norm: &str,
    tree: &Node,
    os: TrashOs,
    out: &mut Vec<TreeCandidate>,
    seen: &mut HashSet<String>,
) {
    let mut stack: Vec<(String, &Node, usize)> =
        vec![(root_norm.to_owned(), tree, 0)];
    let mut visited = 0_usize;

    while let Some((path_str, node, depth)) = stack.pop() {
        if visited >= 4_000 || depth > 4 {
            continue;
        }
        for child in &node.children {
            visited += 1;
            if child.kind == NodeKind::Symlink
                || child.kind == NodeKind::Other
                || child.category == Category::Container
                || child.bytes == 0
            {
                continue;
            }
            let child_path = join_lexical_for_os(&path_str, &child.name, os);
            if let Some(reclaim) = child.reclaim {
                if seen.insert(child_path.clone()) {
                    out.push(TreeCandidate {
                        path: PathBuf::from(&child_path),
                        bytes: child.bytes,
                        is_dir: child.is_dir(),
                        category: child.category.label(),
                        finding: reclaim.label().to_owned(),
                    });
                }
                continue;
            }
            if depth <= 2 && seen.insert(child_path.clone()) {
                let kind_label = if child.is_dir() {
                    "Large directory"
                } else {
                    "Large file"
                };
                out.push(TreeCandidate {
                    path: PathBuf::from(&child_path),
                    bytes: child.bytes,
                    is_dir: child.is_dir(),
                    category: child.category.label(),
                    finding: kind_label.to_owned(),
                });
            }
            if child.is_dir() && depth < 4 {
                stack.push((child_path, child, depth + 1));
            }
        }
    }
}

/// Collect high-usage / reclaimable candidates and Docker usage summary purely
/// from an in-memory [`Node`] tree and [`DockerProbeStatus`] without touching
/// the host filesystem.
#[must_use]
pub fn collect_tree_candidates(
    root_path: &Path,
    tree: &Node,
    docker: &DockerProbeStatus,
) -> TreeCandidates {
    let os = infer_os_from_root(root_path);
    let root_raw = root_path.to_string_lossy();
    let root_norm = normalize_lexical_for_os(&root_raw, os)
        .unwrap_or_else(|_| root_raw.into_owned());

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let now_i64 = i64::try_from(now_secs).unwrap_or(0);

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    for candidate in insights::worth_a_look(tree, now_i64, 25) {
        let Some((path_str, node)) =
            path_from_crumbs_for_os(&root_norm, tree, &candidate.crumbs, os)
        else {
            continue;
        };
        if seen.insert(path_str.clone()) {
            items.push(TreeCandidate {
                path: PathBuf::from(path_str),
                bytes: candidate.bytes,
                is_dir: node.is_dir(),
                category: node.category.label(),
                finding: format_finding_label(&candidate.finding),
            });
        }
    }

    collect_bounded_tree_nodes(&root_norm, tree, os, &mut items, &mut seen);
    items.sort_by_key(|item| std::cmp::Reverse(item.bytes));
    items.truncate(25);

    let docker_summary = match docker {
        DockerProbeStatus::Available(summary) => Some(summary.clone()),
        DockerProbeStatus::NotInstalled
        | DockerProbeStatus::DaemonStopped
        | DockerProbeStatus::PermissionDenied { .. } => None,
    };

    TreeCandidates {
        items,
        docker: docker_summary,
    }
}

/// Build an offline/in-memory diagnostic prompt from [`TreeCandidates`] for the
/// target OS (no remote AI CLI or host filesystem probing required).
#[must_use]
pub fn build_tree_analysis_prompt(
    root_path: &Path,
    target_os: TrashOs,
    candidates: &TreeCandidates,
) -> String {
    use std::fmt::Write as _;

    let os_label = match target_os {
        TrashOs::MacOs => "macOS",
        TrashOs::Linux => "Linux",
        TrashOs::Windows => "Windows",
    };

    let mut cand_block = String::new();
    if candidates.items.is_empty() {
        cand_block.push_str(
            "Pre-scanned file/directory candidates: none detected.\n",
        );
    } else {
        cand_block.push_str(
            "PRE-SCANNED FILE/DIRECTORY CANDIDATES (verified in memory snapshot):\n",
        );
        for (idx, item) in candidates.items.iter().enumerate() {
            let kind = if item.is_dir { "dir" } else { "file" };
            let _ = writeln!(
                cand_block,
                "{}. {} ({}, {}, {}, {})",
                idx + 1,
                item.path.display(),
                human_bytes(item.bytes),
                kind,
                item.category,
                item.finding,
            );
        }
        cand_block.push('\n');
    }

    if let Some(summary) = &candidates.docker {
        cand_block.push_str("DOCKER RECLAIMABLE SNAPSHOT:\n");
        let _ = writeln!(
            cand_block,
            "- Build cache reclaimable: {}",
            human_bytes(summary.build_cache_reclaimable_bytes)
        );
        for img in &summary.removable_images {
            let label = if img.dangling {
                "<none>:<none>".to_owned()
            } else {
                format!("{}:{}", img.repository, img.tag)
            };
            let _ = writeln!(
                cand_block,
                "- Unused image: id={}, ref={}, unique={}, dangling={}",
                img.id,
                label,
                human_bytes(img.unique_bytes),
                img.dangling
            );
        }
        for ctr in &summary.stopped_containers {
            let _ = writeln!(
                cand_block,
                "- Stopped container: id={}, name={}, image={}, state={}, size={}",
                ctr.id,
                ctr.name,
                ctr.image,
                ctr.state,
                human_bytes(ctr.size_bytes)
            );
        }
        cand_block.push('\n');
    }

    format!(
        "You are an automated disk cleanup advisor for Ruum.\n\
         Target OS: {os_label}\n\
         Scanned root: {root}\n\n\
         CRITICAL SAFETY CONSTRAINTS:\n\
         - Analyze ONLY the pre-scanned in-memory candidates and Docker snapshot below.\n\
         - Do NOT execute any shell commands or delete anything.\n\
         - Never suggest system directories, home root, container backing storage, or Docker volumes.\n\
         - For Docker cleanup, reference ONLY exact IDs from the Docker snapshot above.\n\n\
         {cand_block}\
         You MUST output ONLY a valid JSON object matching this schema:\n\
         {{\n  \
           \"summary\": \"Brief explanation of reclaimable space found\",\n  \
           \"items\": [\n    \
             {{\n      \
               \"path\": \"/exact/path/from/candidates\",\n      \
               \"mode\": \"trash\" or \"permanent\",\n      \
               \"reason\": \"Why this item is safe to clean up\"\n    \
             }}\n  \
           ],\n  \
           \"docker_targets\": [\n    \
             {{\"kind\": \"build_cache\"}},\n    \
             {{\"kind\": \"image\", \"id\": \"sha256:...\"}},\n    \
             {{\"kind\": \"container\", \"id\": \"...\"}}\n  \
           ]\n\
         }}\n\
         Do not include conversational filler outside the JSON.",
        root = root_path.display(),
    )
}

/// Validate an AI proposal purely in memory against a scanned [`Node`] tree,
/// [`DockerProbeStatus`], and explicit target [`TrashOs`] without touching the
/// host filesystem.
pub fn validate_proposal_against_tree(
    raw_output: &str,
    root_path: &Path,
    home_path: Option<&Path>,
    tree: &Node,
    docker: &DockerProbeStatus,
    target_os: TrashOs,
    now_secs: u64,
) -> Result<CleanupProposal, String> {
    let parsed = extract_proposal_from_output(raw_output)?;

    let root_raw = root_path
        .to_str()
        .ok_or_else(|| "root path is not valid UTF-8".to_owned())?;
    let root_norm = normalize_lexical_for_os(root_raw, target_os)
        .map_err(|err| format!("invalid root path: {err}"))?;
    let home_norm = home_path
        .and_then(Path::to_str)
        .and_then(|s| normalize_lexical_for_os(s, target_os).ok());

    let mut candidate_targets = Vec::new();
    let mut item_meta: Vec<(String, RemovalMode, Option<String>)> = Vec::new();

    for item in parsed.items {
        let raw_item = item.path.trim();
        if raw_item.is_empty() || raw_item.chars().any(char::is_control) {
            continue;
        }
        let resolved_input = if is_lexical_absolute(raw_item, target_os) {
            raw_item.to_owned()
        } else {
            join_lexical_for_os(&root_norm, raw_item, target_os)
        };
        let Ok(resolved_norm) =
            normalize_lexical_for_os(&resolved_input, target_os)
        else {
            continue;
        };
        let Some(segments) =
            relative_segments_for_os(&resolved_norm, &root_norm, target_os)
        else {
            continue;
        };
        let Some((canonical_path, node)) =
            resolve_node_in_tree(tree, &root_norm, &segments, target_os)
        else {
            continue;
        };

        let mode = match item.mode.as_deref() {
            Some("permanent") => RemovalMode::Permanent,
            _ => RemovalMode::Trash,
        };
        let hidden = node.name.starts_with('.');
        candidate_targets.push(crate::removal::Target {
            path: PathBuf::from(&canonical_path),
            bytes: node.bytes,
            is_dir: node.is_dir(),
            hidden,
        });
        item_meta.push((canonical_path, mode, item.reason));
    }

    let synthetic_header = Header::new(
        PathBuf::from(&root_norm),
        home_norm.map(PathBuf::from),
        platform_for_os(target_os),
        Some(HostIdentity {
            hostname: "ruum-tree".to_owned(),
            machine_id: None,
        }),
    );
    let safe_plan = crate::removal::plan_remote_for_os(
        &candidate_targets,
        &synthetic_header,
        target_os,
    );

    let mut proposal_items = Vec::new();
    for target in safe_plan.targets {
        let target_str = target.path.to_string_lossy();
        let meta_opt = item_meta.iter().find(|(p, ..)| {
            if target_os == TrashOs::Linux {
                p == target_str.as_ref()
            } else {
                p.eq_ignore_ascii_case(target_str.as_ref())
            }
        });
        let mode = meta_opt.map_or(RemovalMode::Trash, |(_, m, _)| *m);
        let reason = meta_opt.and_then(|(.., r)| r.clone());
        proposal_items.push(ProposalItem::new(
            target.path,
            target.bytes,
            mode,
            reason,
            None,
        ));
    }

    let raw_docker_targets: Vec<DockerCleanTarget> = parsed
        .docker_targets
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RawDockerTarget>(v).ok())
        .map(DockerCleanTarget::from)
        .collect();
    let docker_targets = match docker {
        DockerProbeStatus::Available(summary) => {
            validate_targets_against_summary(&raw_docker_targets, summary)
        }
        DockerProbeStatus::NotInstalled
        | DockerProbeStatus::DaemonStopped
        | DockerProbeStatus::PermissionDenied { .. } => Vec::new(),
    };

    Ok(CleanupProposal {
        timestamp_secs: now_secs,
        summary: parsed.summary.unwrap_or_else(|| {
            format!(
                "AI proposed {} items for cleanup",
                proposal_items.len() + docker_targets.len()
            )
        }),
        items: proposal_items,
        docker_targets,
    })
}

/// Run AI analysis locally against an in-memory scanned [`Node`] tree and
/// [`DockerProbeStatus`] without requiring AI CLI installation on the remote
/// host or touching the host filesystem for validation.
#[allow(
    clippy::too_many_arguments,
    reason = "in-memory tree analysis needs explicit root, home, tree, docker, and target OS"
)]
pub fn run_tree_analysis_for_config(
    config: &AiConfig,
    root_path: &Path,
    home_path: Option<&Path>,
    tree: &Node,
    docker: &DockerProbeStatus,
    target_os: TrashOs,
    now_secs: u64,
    runner: &dyn CommandRunner,
) -> Result<CleanupProposal, String> {
    run_tree_analysis_for_config_with_context(
        config,
        root_path,
        home_path,
        tree,
        docker,
        target_os,
        now_secs,
        runner,
        &AiExecutionContext::default(),
    )
}

/// Run AI analysis locally against an in-memory scanned [`Node`] tree with an
/// explicit [`AiExecutionContext`].
#[allow(
    clippy::too_many_arguments,
    reason = "in-memory tree analysis needs explicit root, home, tree, docker, target OS, and execution context"
)]
pub fn run_tree_analysis_for_config_with_context(
    config: &AiConfig,
    root_path: &Path,
    home_path: Option<&Path>,
    tree: &Node,
    docker: &DockerProbeStatus,
    target_os: TrashOs,
    now_secs: u64,
    runner: &dyn CommandRunner,
    ctx: &AiExecutionContext,
) -> Result<CleanupProposal, String> {
    AiBackendFactory::create(config.provider).run_tree_analysis(
        config, root_path, home_path, tree, docker, target_os, now_secs,
        runner, ctx,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "in-memory CLI tree analysis needs explicit root, home, tree, docker, and target OS"
)]
fn cli_run_tree_analysis(
    config: &AiConfig,
    root_path: &Path,
    home_path: Option<&Path>,
    tree: &Node,
    docker: &DockerProbeStatus,
    target_os: TrashOs,
    now_secs: u64,
    runner: &dyn CommandRunner,
) -> Result<CleanupProposal, String> {
    let candidates = collect_tree_candidates(root_path, tree, docker);
    let prompt = build_tree_analysis_prompt(root_path, target_os, &candidates);
    let invocation = build_invocation_for_config(config, &prompt)?;
    let path_env = enriched_path_var();
    let envs = [("PATH", path_env.as_str())];

    let arg_refs: Vec<&str> =
        invocation.args.iter().map(String::as_str).collect();
    let output =
        runner
            .run(&invocation.program, &arg_refs, &envs)
            .map_err(|e| {
                format!(
                    "failed to execute `{}`: {e}",
                    invocation.program.display()
                )
            })?;

    if !output.success() {
        let stderr = output.stderr.trim();
        let detail = if stderr.is_empty() {
            first_line_snippet(&output.stdout, 140)
        } else {
            stderr.to_string()
        };
        return Err(format!(
            "AI CLI exited with code {}: {detail}",
            output.status_code,
        ));
    }

    let raw = if output.stdout.trim().is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    validate_proposal_against_tree(
        raw, root_path, home_path, tree, docker, target_os, now_secs,
    )
}

/// Parse an AI response, enforcing [`crate::removal::plan`] and Git checks.
pub fn parse_and_validate_proposal(
    raw_output: &str,
    home_dir: &Path,
    now_secs: u64,
) -> Result<CleanupProposal, String> {
    parse_and_validate_proposal_with(
        raw_output,
        home_dir,
        now_secs,
        crate::removal::is_trash_target,
    )
}

fn parse_and_validate_proposal_with(
    raw_output: &str,
    home_dir: &Path,
    now_secs: u64,
    is_trash: impl Fn(&Path) -> bool,
) -> Result<CleanupProposal, String> {
    let parsed = extract_proposal_from_output(raw_output)?;

    let mut candidate_targets = Vec::new();
    let mut item_meta = Vec::new();

    for item in parsed.items {
        let raw_p = PathBuf::from(&item.path);
        let resolved = if raw_p.is_absolute() {
            raw_p
        } else {
            home_dir.join(raw_p)
        };
        let normalized = crate::removal::normalize(&resolved);

        let Ok(meta) = fs::symlink_metadata(&normalized) else {
            // Drop paths that do not exist on disk.
            continue;
        };

        let is_dir = meta.is_dir();
        let hidden = normalized
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'));
        let bytes = if is_dir {
            let mut visited = 0;
            compute_dir_bytes(&normalized, 0, &mut visited)
        } else {
            meta.len()
        };

        let mode = match item.mode.as_deref() {
            Some("permanent") => RemovalMode::Permanent,
            _ => RemovalMode::Trash,
        };

        candidate_targets.push(crate::removal::Target {
            path: normalized.clone(),
            bytes,
            is_dir,
            hidden,
        });

        item_meta.push((normalized, mode, item.reason));
    }

    // Safety gate: judge targets against scanned home root
    let safe_plan = crate::removal::plan(&candidate_targets, home_dir);

    let mut proposal_items = Vec::new();
    for target in safe_plan.targets {
        let meta_opt = item_meta.iter().find(|(p, ..)| p == &target.path);
        let requested_mode =
            meta_opt.map_or(RemovalMode::Trash, |(_, m, _)| *m);
        let mode = if is_trash(&target.path) {
            RemovalMode::Permanent
        } else {
            requested_mode
        };
        let reason = meta_opt.and_then(|(.., r)| r.clone());

        let git_warning = crate::git::state(&target.path).and_then(|st| {
            if st.is_clean() {
                None
            } else {
                Some(format!("Git checkout not clean: {}", st.summary()))
            }
        });

        proposal_items.push(ProposalItem::new(
            target.path,
            target.bytes,
            mode,
            reason,
            git_warning,
        ));
    }

    Ok(CleanupProposal {
        timestamp_secs: now_secs,
        summary: parsed.summary.unwrap_or_else(|| {
            format!("AI proposed {} items for cleanup", proposal_items.len())
        }),
        items: proposal_items,
        docker_targets: Vec::new(),
    })
}

/// Run an AI diagnostic analysis for an [`AiConfig`], optionally reusing a
/// scanned tree.
pub fn run_analysis_with_tree_for_config(
    config: &AiConfig,
    home_dir: &Path,
    current_exe: &Path,
    now_secs: u64,
    existing_tree: Option<&Node>,
    runner: &dyn CommandRunner,
) -> Result<CleanupProposal, String> {
    run_analysis_with_tree_for_config_with_context(
        config,
        home_dir,
        current_exe,
        now_secs,
        existing_tree,
        runner,
        &AiExecutionContext::default(),
    )
}

/// Run an AI diagnostic analysis for an [`AiConfig`] with an explicit
/// [`AiExecutionContext`].
pub fn run_analysis_with_tree_for_config_with_context(
    config: &AiConfig,
    home_dir: &Path,
    current_exe: &Path,
    now_secs: u64,
    existing_tree: Option<&Node>,
    runner: &dyn CommandRunner,
    ctx: &AiExecutionContext,
) -> Result<CleanupProposal, String> {
    AiBackendFactory::create(config.provider).run_analysis_with_tree(
        config,
        home_dir,
        current_exe,
        now_secs,
        existing_tree,
        runner,
        ctx,
    )
}

fn cli_run_analysis_with_tree(
    config: &AiConfig,
    home_dir: &Path,
    current_exe: &Path,
    now_secs: u64,
    existing_tree: Option<&Node>,
    runner: &dyn CommandRunner,
) -> Result<CleanupProposal, String> {
    let candidates =
        collect_reclaimable_candidates(home_dir, existing_tree, now_secs);
    let prompt = build_analysis_prompt_with_candidates(
        home_dir,
        current_exe,
        &candidates,
    );
    let invocation = build_invocation_for_config(config, &prompt)?;
    let path_env = enriched_path_var();
    let envs = [("PATH", path_env.as_str())];

    let arg_refs: Vec<&str> =
        invocation.args.iter().map(String::as_str).collect();
    let output =
        runner
            .run(&invocation.program, &arg_refs, &envs)
            .map_err(|e| {
                format!(
                    "failed to execute `{}`: {e}",
                    invocation.program.display()
                )
            })?;

    if !output.success() {
        let stderr = output.stderr.trim();
        let detail = if stderr.is_empty() {
            first_line_snippet(&output.stdout, 140)
        } else {
            stderr.to_string()
        };
        return Err(format!(
            "AI CLI exited with code {}: {detail}",
            output.status_code,
        ));
    }

    let raw = if output.stdout.trim().is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    parse_and_validate_proposal(raw, home_dir, now_secs)
}

/// Run an AI diagnostic analysis directly from an [`AiConfig`].
pub fn run_analysis_for_config(
    config: &AiConfig,
    home_dir: &Path,
    current_exe: &Path,
    now_secs: u64,
    runner: &dyn CommandRunner,
) -> Result<CleanupProposal, String> {
    run_analysis_with_tree_for_config(
        config,
        home_dir,
        current_exe,
        now_secs,
        None,
        runner,
    )
}

/// Run an AI diagnostic analysis, optionally reusing a scanned tree.
pub fn run_analysis_with_tree(
    config: &WatchdogConfig,
    home_dir: &Path,
    current_exe: &Path,
    now_secs: u64,
    existing_tree: Option<&Node>,
    runner: &dyn CommandRunner,
) -> Result<CleanupProposal, String> {
    run_analysis_with_tree_for_config(
        &ai_config_from_legacy(config),
        home_dir,
        current_exe,
        now_secs,
        existing_tree,
        runner,
    )
}

/// Run an AI diagnostic analysis using the injected command runner.
pub fn run_analysis(
    config: &WatchdogConfig,
    home_dir: &Path,
    current_exe: &Path,
    now_secs: u64,
    runner: &dyn CommandRunner,
) -> Result<CleanupProposal, String> {
    run_analysis_for_config(
        &ai_config_from_legacy(config),
        home_dir,
        current_exe,
        now_secs,
        runner,
    )
}

/// Discriminant of the AI execution backend backing a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiBackendKind {
    Cli,
    DirectApi,
    RuumCloud,
}

/// Execution-time credential and HTTP overrides for AI operations.
#[derive(Clone, Default)]
pub struct AiExecutionContext {
    pub api_key: Option<Zeroizing<String>>,
    pub credential_store: Option<LazyCredentialStore>,
    pub allow_insecure_http: bool,
    pub db_path: Option<PathBuf>,
    pub cloud_base_url: Option<String>,
}

impl AiExecutionContext {
    /// Override the direct API key (used by GUI drafts and offline tests).
    #[must_use]
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        let raw = key.into();
        let trimmed = raw.trim();
        self.api_key =
            (!trimmed.is_empty()).then(|| Zeroizing::new(trimmed.to_owned()));
        self
    }

    /// Inject a custom [`LazyCredentialStore`] instead of opening the default
    /// desktop store.
    #[must_use]
    pub fn with_credential_store(mut self, store: LazyCredentialStore) -> Self {
        self.credential_store = Some(store);
        self
    }

    /// Allow non-loopback `http://` endpoints when explicitly requested.
    #[must_use]
    pub const fn with_allow_insecure_http(mut self, allow: bool) -> Self {
        self.allow_insecure_http = allow;
        self
    }

    /// Inject an explicit `SQLite` database path for `RuumCloud` state access.
    #[must_use]
    pub fn with_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.db_path = Some(path.into());
        self
    }

    /// Override the Ruum Cloud gateway base URL (primarily for offline tests).
    #[must_use]
    pub fn with_cloud_base_url(mut self, url: impl Into<String>) -> Self {
        let raw = url.into();
        let trimmed = raw.trim();
        self.cloud_base_url = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        self
    }
}

impl fmt::Debug for AiExecutionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiExecutionContext")
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("has_credential_store", &self.credential_store.is_some())
            .field("allow_insecure_http", &self.allow_insecure_http)
            .field("db_path", &self.db_path)
            .field("cloud_base_url", &self.cloud_base_url)
            .finish()
    }
}

/// OS/backend-agnostic execution contract for AI probing and analysis.
pub trait AiExecutionAdapter: Send + Sync {
    fn kind(&self) -> AiBackendKind;

    fn test_connection(
        &self,
        config: &AiConfig,
        runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<String, String>;

    fn discover_models(
        &self,
        config: &AiConfig,
        runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<(Vec<DiscoveredModel>, Option<String>), String>;

    fn benchmark_model(
        &self,
        config: &AiConfig,
        model_override: Option<&str>,
        runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> ModelSpeedResult;

    #[allow(
        clippy::too_many_arguments,
        reason = "local analysis adapter requires root, exe path, timestamp, optional scanned tree, runner, and context"
    )]
    fn run_analysis_with_tree(
        &self,
        config: &AiConfig,
        home_dir: &Path,
        current_exe: &Path,
        now_secs: u64,
        existing_tree: Option<&Node>,
        runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<CleanupProposal, String>;

    #[allow(
        clippy::too_many_arguments,
        reason = "in-memory tree analysis adapter requires root, home, tree, docker, target OS, timestamp, runner, and context"
    )]
    fn run_tree_analysis(
        &self,
        config: &AiConfig,
        root_path: &Path,
        home_path: Option<&Path>,
        tree: &Node,
        docker: &DockerProbeStatus,
        target_os: TrashOs,
        now_secs: u64,
        runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<CleanupProposal, String>;
}

/// Adapter routing `agy`, `pi`, `codex`, and `custom` through external CLIs.
#[derive(Clone, Copy, Debug, Default)]
pub struct CliAiAdapter;

impl AiExecutionAdapter for CliAiAdapter {
    fn kind(&self) -> AiBackendKind {
        AiBackendKind::Cli
    }

    fn test_connection(
        &self,
        config: &AiConfig,
        runner: &dyn CommandRunner,
        _ctx: &AiExecutionContext,
    ) -> Result<String, String> {
        cli_test_connection(config, runner)
    }

    fn discover_models(
        &self,
        config: &AiConfig,
        runner: &dyn CommandRunner,
        _ctx: &AiExecutionContext,
    ) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
        cli_discover_models(config, runner)
    }

    fn benchmark_model(
        &self,
        config: &AiConfig,
        model_override: Option<&str>,
        runner: &dyn CommandRunner,
        _ctx: &AiExecutionContext,
    ) -> ModelSpeedResult {
        cli_benchmark_model(config, model_override, runner)
    }

    fn run_analysis_with_tree(
        &self,
        config: &AiConfig,
        home_dir: &Path,
        current_exe: &Path,
        now_secs: u64,
        existing_tree: Option<&Node>,
        runner: &dyn CommandRunner,
        _ctx: &AiExecutionContext,
    ) -> Result<CleanupProposal, String> {
        cli_run_analysis_with_tree(
            config,
            home_dir,
            current_exe,
            now_secs,
            existing_tree,
            runner,
        )
    }

    fn run_tree_analysis(
        &self,
        config: &AiConfig,
        root_path: &Path,
        home_path: Option<&Path>,
        tree: &Node,
        docker: &DockerProbeStatus,
        target_os: TrashOs,
        now_secs: u64,
        runner: &dyn CommandRunner,
        _ctx: &AiExecutionContext,
    ) -> Result<CleanupProposal, String> {
        cli_run_tree_analysis(
            config, root_path, home_path, tree, docker, target_os, now_secs,
            runner,
        )
    }
}

/// Adapter routing [`AiCliProvider::Api`] through `crates/ruum-adk` and an
/// in-process read-only [`McpServer`].
#[derive(Clone, Copy, Debug, Default)]
pub struct DirectApiAiAdapter;

/// Adapter routing [`AiCliProvider::RuumCloud`] through [`CloudClient`] and
/// [`ruum_adk::RuumCloudLlm`].
#[derive(Clone, Copy, Debug, Default)]
pub struct RuumCloudAiAdapter;

fn format_cloud_api_err(err: &CloudApiError) -> String {
    err.format_localized(Locale::current())
}

fn format_cloud_adk_err(err: &ruum_adk::AdkClientError) -> String {
    match err {
        ruum_adk::AdkClientError::CloudGateway {
            status,
            code,
            message,
            trace_id,
            is_timeout,
        } => format_cloud_error_message(
            code.as_deref(),
            *status,
            message,
            trace_id.as_deref(),
            *is_timeout,
            Locale::current(),
        ),
        other => other.to_string(),
    }
}

fn build_cloud_client_from_context(
    ctx: &AiExecutionContext,
) -> Result<CloudClient, String> {
    let db = match ctx.db_path {
        Some(ref path) => Arc::new(
            StorageDb::open(path)
                .map_err(|e| format!("failed to open Ruum database: {e}"))?,
        ),
        None => Arc::new(
            StorageDb::open_default()
                .map_err(|e| format!("failed to open Ruum database: {e}"))?,
        ),
    };
    let cred_store = if let Some(ref store) = ctx.credential_store {
        store.clone()
    } else if let Some(ref db_path) = ctx.db_path
        && let Some(dir) = db_path.parent()
        && !dir.as_os_str().is_empty()
    {
        LazyCredentialStore::platform(dir)
    } else {
        LazyCredentialStore::open_default()?
    };

    let mut client = CloudClient::new(db, cred_store);
    if let Some(ref base_url) = ctx.cloud_base_url {
        client = client
            .with_base_url(base_url)
            .map_err(|e| format_cloud_api_err(&e))?;
    }
    Ok(client)
}

fn resolve_cloud_preferred_model<'a>(
    model_override: Option<&'a str>,
    config: &'a AiConfig,
    cached_selected: Option<&'a str>,
) -> Option<&'a str> {
    model_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            config
                .model
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .or_else(|| cached_selected.map(str::trim).filter(|s| !s.is_empty()))
}

fn prepare_cloud_inference_model(
    client: &CloudClient,
    config: &AiConfig,
    model_override: Option<&str>,
) -> Result<(String, String), String> {
    let ent = client
        .fetch_entitlement(false)
        .map_err(|e| format_cloud_api_err(&e))?;
    if ent.decision.eq_ignore_ascii_case("DENY")
        || (!ent.is_paid && ent.trial_remaining == 0)
    {
        return Err(CloudApiError::new(
            Some(402),
            Some("ENTITLEMENT_PAYMENT_REQUIRED"),
            "Free trial exhausted or plan expired",
            None,
        )
        .format_localized(Locale::current()));
    }

    let models = client
        .fetch_virtual_models()
        .map_err(|e| format_cloud_api_err(&e))?;
    if !models.iter().any(|m| m.available) {
        return Err(CloudApiError::new(
            Some(403),
            Some("INFERENCE_MODEL_NOT_ALLOWED"),
            "No Ruum Cloud models are currently available for this account",
            None,
        )
        .format_localized(Locale::current()));
    }
    let cached_selected = client.cached_selected_model_id();
    let preferred = resolve_cloud_preferred_model(
        model_override,
        config,
        cached_selected.as_deref(),
    );
    let chosen_id = if let Some(pref) = preferred
        && models.iter().any(|m| m.id == pref && m.available)
    {
        pref.to_owned()
    } else {
        CLOUD_AUTO_MODEL_ID.to_owned()
    };
    let base_url = client
        .resolved_base_url()
        .map_err(|e| format_cloud_api_err(&e))?;
    Ok((base_url, chosen_id))
}

fn map_cloud_api_to_gateway_error(
    err: CloudApiError,
) -> ruum_adk::CloudGatewayError {
    ruum_adk::CloudGatewayError {
        status: err.http_status,
        code: err.code,
        message: err.message,
        trace_id: err.trace_id,
        retry_after_secs: err.retry_after_secs,
        is_timeout: err.is_timeout,
    }
}

struct CoreCloudTokenProvider {
    client: Arc<CloudClient>,
}

impl ruum_adk::CloudTokenProvider for CoreCloudTokenProvider {
    fn current_token<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<String, ruum_adk::CloudGatewayError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        let client = Arc::clone(&self.client);
        spawn_blocking_thread(
            "ruum-cloud-jwt",
            || {
                Err(ruum_adk::CloudGatewayError {
                    status: None,
                    code: Some("CLOUD_INTERNAL_ERROR".to_owned()),
                    message: "cloud token future mutex poisoned".to_owned(),
                    trace_id: None,
                    retry_after_secs: None,
                    is_timeout: false,
                })
            },
            |err| {
                Err(ruum_adk::CloudGatewayError {
                    status: None,
                    code: Some("CLOUD_INTERNAL_ERROR".to_owned()),
                    message: format!(
                        "failed to spawn cloud token worker: {err}"
                    ),
                    trace_id: None,
                    retry_after_secs: None,
                    is_timeout: false,
                })
            },
            move || {
                client
                    .ensure_valid_jwt()
                    .map_err(map_cloud_api_to_gateway_error)
            },
        )
    }

    fn refresh_token<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<String, ruum_adk::CloudGatewayError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        let client = Arc::clone(&self.client);
        spawn_blocking_thread(
            "ruum-cloud-refresh",
            || {
                Err(ruum_adk::CloudGatewayError {
                    status: None,
                    code: Some("CLOUD_INTERNAL_ERROR".to_owned()),
                    message: "cloud refresh future mutex poisoned".to_owned(),
                    trace_id: None,
                    retry_after_secs: None,
                    is_timeout: false,
                })
            },
            |err| {
                Err(ruum_adk::CloudGatewayError {
                    status: None,
                    code: Some("CLOUD_INTERNAL_ERROR".to_owned()),
                    message: format!(
                        "failed to spawn cloud refresh worker: {err}"
                    ),
                    trace_id: None,
                    retry_after_secs: None,
                    is_timeout: false,
                })
            },
            move || {
                let state = client
                    .refresh_access_token()
                    .map_err(map_cloud_api_to_gateway_error)?;
                state
                    .access_token
                    .ok_or_else(|| ruum_adk::CloudGatewayError {
                        status: None,
                        code: Some("AUTH_TOKEN_MISSING".to_owned()),
                        message: "cloud token refresh returned no access token"
                            .to_owned(),
                        trace_id: None,
                        retry_after_secs: None,
                        is_timeout: false,
                    })
            },
        )
    }
}

impl AiExecutionAdapter for RuumCloudAiAdapter {
    fn kind(&self) -> AiBackendKind {
        AiBackendKind::RuumCloud
    }

    fn test_connection(
        &self,
        _config: &AiConfig,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<String, String> {
        let client = build_cloud_client_from_context(ctx)?;
        let ent = client
            .fetch_entitlement(false)
            .map_err(|e| format_cloud_api_err(&e))?;
        let models = client
            .fetch_virtual_models()
            .map_err(|e| format_cloud_api_err(&e))?;
        let avail = models.iter().filter(|m| m.available).count();
        let status_label = if ent.is_paid {
            "paid plan active".to_owned()
        } else {
            format!("trial remaining: {}", ent.trial_remaining)
        };
        Ok(format!(
            "Ruum Cloud ready ({status_label}, {avail}/{} model(s) available)",
            models.len()
        ))
    }

    fn discover_models(
        &self,
        _config: &AiConfig,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
        let client = build_cloud_client_from_context(ctx)?;
        let models = client
            .fetch_virtual_models()
            .map_err(|e| format_cloud_api_err(&e))?;
        let any_available = models.iter().any(|m| m.available);
        let mapped = models
            .into_iter()
            .map(|m| {
                let title = m
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&m.id);
                let tier_tag = if m.tier.trim().is_empty() {
                    "cloud"
                } else {
                    m.tier.trim()
                };
                let label = if m.available {
                    format!("{title} ({tier_tag})")
                } else {
                    format!("{title} ({tier_tag}, locked)")
                };
                DiscoveredModel {
                    id: m.id,
                    label,
                    provider: Some("ruum-cloud".to_owned()),
                }
            })
            .collect();
        let note = (!any_available).then(|| {
            "No Ruum Cloud models are currently available; purchase a plan or \
             redeem a code to unlock."
                .to_owned()
        });
        Ok((mapped, note))
    }

    fn benchmark_model(
        &self,
        config: &AiConfig,
        model_override: Option<&str>,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> ModelSpeedResult {
        let requested_model = model_override
            .or(config.model.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("(auto)")
            .to_owned();
        let client = match build_cloud_client_from_context(ctx) {
            Ok(c) => Arc::new(c),
            Err(err) => {
                return ModelSpeedResult {
                    model: requested_model,
                    elapsed_ms: 0,
                    ok: false,
                    detail: err,
                };
            }
        };
        let ent = match client.fetch_entitlement(false) {
            Ok(e) => e,
            Err(err) => {
                return ModelSpeedResult {
                    model: requested_model,
                    elapsed_ms: 0,
                    ok: false,
                    detail: format_cloud_api_err(&err),
                };
            }
        };
        if !ent.is_paid {
            return ModelSpeedResult {
                model: requested_model,
                elapsed_ms: 0,
                ok: false,
                detail: "Model speed test is disabled during free trial to \
                         preserve trial quota."
                    .to_owned(),
            };
        }

        let models = match client.fetch_virtual_models() {
            Ok(m) => m,
            Err(err) => {
                return ModelSpeedResult {
                    model: requested_model,
                    elapsed_ms: 0,
                    ok: false,
                    detail: format_cloud_api_err(&err),
                };
            }
        };
        let cached_selected = client.cached_selected_model_id();
        let preferred = resolve_cloud_preferred_model(
            model_override,
            config,
            cached_selected.as_deref(),
        );
        let Some(chosen) = select_best_available_model(&models, preferred)
        else {
            return ModelSpeedResult {
                model: requested_model,
                elapsed_ms: 0,
                ok: false,
                detail: "No available Ruum Cloud models for benchmark."
                    .to_owned(),
            };
        };
        let base_url = match client.resolved_base_url() {
            Ok(u) => u,
            Err(err) => {
                return ModelSpeedResult {
                    model: chosen.id.clone(),
                    elapsed_ms: 0,
                    ok: false,
                    detail: format_cloud_api_err(&err),
                };
            }
        };
        let adk = match ruum_adk::BlockingAdkClient::new() {
            Ok(c) => c,
            Err(err) => {
                return ModelSpeedResult {
                    model: chosen.id.clone(),
                    elapsed_ms: 0,
                    ok: false,
                    detail: err.to_string(),
                };
            }
        };
        let token_provider: Arc<dyn ruum_adk::CloudTokenProvider> =
            Arc::new(CoreCloudTokenProvider { client });
        let cloud_cfg = ruum_adk::RuumCloudConfig::new(
            base_url,
            chosen.id.clone(),
            ruum_adk::RuumTask::LocalAnalysis,
            token_provider,
        )
        .with_client_version(env!("CARGO_PKG_VERSION"));
        let mut req = ruum_adk::AdkAgentRequest::new(
            "Reply concisely.",
            "Reply with only OK.",
        );
        req.max_iterations = 1;
        req.fallback_without_tools_on_unsupported = false;

        let start = Instant::now();
        let res = adk.run_cloud_agent(&cloud_cfg, req);
        let elapsed_ms =
            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        match res {
            Ok(resp) => {
                let reply = first_line_snippet(&resp.text, 60);
                ModelSpeedResult {
                    model: resp.model,
                    elapsed_ms,
                    ok: true,
                    detail: if reply.is_empty() {
                        "OK".to_owned()
                    } else {
                        reply
                    },
                }
            }
            Err(err) => ModelSpeedResult {
                model: chosen.id.clone(),
                elapsed_ms,
                ok: false,
                detail: format_cloud_adk_err(&err),
            },
        }
    }

    fn run_analysis_with_tree(
        &self,
        config: &AiConfig,
        home_dir: &Path,
        _current_exe: &Path,
        now_secs: u64,
        existing_tree: Option<&Node>,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<CleanupProposal, String> {
        let client = Arc::new(build_cloud_client_from_context(ctx)?);
        let (base_url, model_id) =
            prepare_cloud_inference_model(&client, config, None)?;
        let adk = ruum_adk::BlockingAdkClient::new()
            .map_err(|err| format_cloud_adk_err(&err))?;

        let candidates =
            collect_reclaimable_candidates(home_dir, existing_tree, now_secs);
        let (instruction, prompt) =
            build_direct_api_analysis_prompt(home_dir, &candidates);

        let mut mcp = McpServer::default().with_read_only(true);
        if let Some(tree) = existing_tree {
            mcp.seed_local_tree_cache(
                home_dir,
                tree.clone(),
                Some(home_dir.to_path_buf()),
            );
        }
        let shared_mcp = Arc::new(Mutex::new(mcp));

        let mut req = ruum_adk::AdkAgentRequest::new(instruction, prompt);
        req.tools = build_local_mcp_adk_tools(&shared_mcp);
        req.max_iterations = ruum_adk::MAX_CLOUD_ITERATIONS;
        req.fallback_without_tools_on_unsupported = false;

        let token_provider: Arc<dyn ruum_adk::CloudTokenProvider> =
            Arc::new(CoreCloudTokenProvider { client });
        let cloud_cfg = ruum_adk::RuumCloudConfig::new(
            base_url,
            model_id,
            ruum_adk::RuumTask::LocalAnalysis,
            token_provider,
        )
        .with_client_version(env!("CARGO_PKG_VERSION"));

        let resp = adk
            .run_cloud_agent(&cloud_cfg, req)
            .map_err(|err| format_cloud_adk_err(&err))?;
        parse_and_validate_proposal(&resp.text, home_dir, now_secs)
    }

    fn run_tree_analysis(
        &self,
        config: &AiConfig,
        root_path: &Path,
        home_path: Option<&Path>,
        tree: &Node,
        docker: &DockerProbeStatus,
        target_os: TrashOs,
        now_secs: u64,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<CleanupProposal, String> {
        let client = Arc::new(build_cloud_client_from_context(ctx)?);
        let (base_url, model_id) =
            prepare_cloud_inference_model(&client, config, None)?;
        let adk = ruum_adk::BlockingAdkClient::new()
            .map_err(|err| format_cloud_adk_err(&err))?;

        let candidates = collect_tree_candidates(root_path, tree, docker);
        let prompt =
            build_tree_analysis_prompt(root_path, target_os, &candidates);

        // Remote / in-memory analysis runs single-turn inference with zero
        // local disk or SSH tools mounted.
        let mut req = ruum_adk::AdkAgentRequest::new(
            "You are an automated disk cleanup advisor for Ruum. Analyze \
             only the provided in-memory snapshot and output strictly valid \
             JSON.",
            prompt,
        );
        req.max_iterations = 1;
        req.fallback_without_tools_on_unsupported = false;

        let token_provider: Arc<dyn ruum_adk::CloudTokenProvider> =
            Arc::new(CoreCloudTokenProvider { client });
        let cloud_cfg = ruum_adk::RuumCloudConfig::new(
            base_url,
            model_id,
            ruum_adk::RuumTask::TreeAnalysis,
            token_provider,
        )
        .with_client_version(env!("CARGO_PKG_VERSION"));

        let resp = adk
            .run_cloud_agent(&cloud_cfg, req)
            .map_err(|err| format_cloud_adk_err(&err))?;
        validate_proposal_against_tree(
            &resp.text, root_path, home_path, tree, docker, target_os, now_secs,
        )
    }
}

/// Factory selecting the [`AiExecutionAdapter`] for a configured provider.
#[derive(Clone, Copy, Debug, Default)]
pub struct AiBackendFactory;

impl AiBackendFactory {
    /// Return the backend kind corresponding to `provider`.
    #[must_use]
    pub const fn kind_for(provider: AiCliProvider) -> AiBackendKind {
        match provider {
            AiCliProvider::Pi
            | AiCliProvider::Agy
            | AiCliProvider::Codex
            | AiCliProvider::Custom => AiBackendKind::Cli,
            AiCliProvider::Api => AiBackendKind::DirectApi,
            AiCliProvider::RuumCloud => AiBackendKind::RuumCloud,
        }
    }

    /// Construct the [`AiExecutionAdapter`] for `provider`.
    #[must_use]
    pub fn create(provider: AiCliProvider) -> Box<dyn AiExecutionAdapter> {
        match Self::kind_for(provider) {
            AiBackendKind::Cli => Box::new(CliAiAdapter),
            AiBackendKind::DirectApi => Box::new(DirectApiAiAdapter),
            AiBackendKind::RuumCloud => Box::new(RuumCloudAiAdapter),
        }
    }
}

fn read_env_api_key() -> Option<Zeroizing<String>> {
    let val = std::env::var(ENV_AI_API_KEY).ok()?;
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(Zeroizing::new(trimmed.to_owned()))
}

fn resolve_api_key(
    ctx: &AiExecutionContext,
) -> Result<Zeroizing<String>, String> {
    if let Some(ref key) = ctx.api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return Ok(Zeroizing::new(trimmed.to_owned()));
        }
    }
    if let Some(ref store) = ctx.credential_store {
        if let Some(key) = store.get_ai_api_key()? {
            return Ok(key);
        }
        if let Some(env_key) = read_env_api_key() {
            return Ok(env_key);
        }
    } else {
        if let Some(env_key) = read_env_api_key() {
            return Ok(env_key);
        }
        let store = LazyCredentialStore::open_default()?;
        if let Some(key) = store.get_ai_api_key()? {
            return Ok(key);
        }
    }
    Err(
        "API key is not configured for direct API provider (set it in \
         Settings, via `ruum config ai set --api-key-stdin`, or via \
         RUUM_AI_API_KEY)"
            .to_owned(),
    )
}

const fn map_protocol(protocol: ApiProtocol) -> ruum_adk::AiProtocol {
    match protocol {
        ApiProtocol::OpenAi => ruum_adk::AiProtocol::OpenAi,
        ApiProtocol::Anthropic => ruum_adk::AiProtocol::Anthropic,
    }
}

fn build_adk_model_config(
    config: &AiConfig,
    ctx: &AiExecutionContext,
    model_override: Option<&str>,
    timeout: Duration,
    max_tokens: Option<u32>,
) -> Result<ruum_adk::AdkModelConfig, String> {
    let api_key = resolve_api_key(ctx)?;
    let mut cfg = ruum_adk::AdkModelConfig::new(
        map_protocol(config.api_protocol),
        api_key.as_str(),
    )
    .with_timeout(timeout)
    .with_allow_insecure_http(ctx.allow_insecure_http);
    let base_url = config.api_base_url.trim();
    if !base_url.is_empty() {
        cfg = cfg.with_base_url(base_url);
    }
    if let Some(model) = model_override
        .or(config.model.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg = cfg.with_model(model);
    }
    if let Some(limit) = max_tokens {
        cfg = cfg.with_max_tokens(limit);
    }
    Ok(cfg)
}

fn sanitize_adk_error(err: &ruum_adk::AdkClientError, api_key: &str) -> String {
    ruum_adk::sanitize_error_message(&err.to_string(), api_key)
}

struct BlockingSlot<T> {
    result: Option<T>,
    waker: Option<Waker>,
}

struct BlockingThreadFuture<T> {
    slot: Arc<Mutex<BlockingSlot<T>>>,
    on_poison: fn() -> T,
}

impl<T> Future for BlockingThreadFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Ok(mut guard) = self.slot.lock() else {
            return Poll::Ready((self.on_poison)());
        };
        if let Some(res) = guard.result.take() {
            return Poll::Ready(res);
        }
        guard.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

fn spawn_blocking_thread<T: Send + 'static>(
    thread_name: &str,
    on_poison: fn() -> T,
    on_spawn_err: fn(io::Error) -> T,
    work: impl FnOnce() -> T + Send + 'static,
) -> Pin<Box<dyn Future<Output = T> + Send>> {
    let slot = Arc::new(Mutex::new(BlockingSlot {
        result: None,
        waker: None,
    }));
    let worker_slot = Arc::clone(&slot);
    let spawn_res = std::thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            let outcome = work();
            if let Ok(mut guard) = worker_slot.lock() {
                guard.result = Some(outcome);
                let waker = guard.waker.take();
                drop(guard);
                if let Some(w) = waker {
                    w.wake();
                }
            }
        });
    if let Err(err) = spawn_res
        && let Ok(mut guard) = slot.lock()
    {
        guard.result = Some(on_spawn_err(err));
    }
    Box::pin(BlockingThreadFuture { slot, on_poison })
}

struct LocalMcpToolExecutor {
    tool_name: String,
    server: Arc<Mutex<McpServer>>,
}

impl ruum_adk::AdkToolExecutor for LocalMcpToolExecutor {
    fn execute<'life0, 'async_trait>(
        &'life0 self,
        args: serde_json::Value,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<serde_json::Value, String>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        let tool_name = self.tool_name.clone();
        let server = Arc::clone(&self.server);
        spawn_blocking_thread(
            "ruum-mcp-tool",
            || Err("local MCP tool result mutex poisoned".to_owned()),
            |err| Err(format!("failed to spawn local MCP worker: {err}")),
            move || {
                // Execute synchronous disk/git tools off the Tokio runtime so
                // concurrent or timed-out tool calls never stall async workers.
                match server.lock() {
                    Ok(mut guard) => {
                        guard.dispatch_local_read_only_tool(&tool_name, &args)
                    }
                    Err(_) => Err("local MCP server mutex poisoned".to_owned()),
                }
            },
        )
    }
}

fn build_local_mcp_adk_tools(
    server: &Arc<Mutex<McpServer>>,
) -> Vec<(ruum_adk::AdkToolDef, Arc<dyn ruum_adk::AdkToolExecutor>)> {
    local_read_only_tool_definitions()
        .into_iter()
        .filter_map(|schema| {
            let name = schema.get("name")?.as_str()?.to_owned();
            let description = schema
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let parameters_schema = schema
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
            let def = ruum_adk::AdkToolDef {
                name: name.clone(),
                description,
                parameters_schema,
            };
            let exec: Arc<dyn ruum_adk::AdkToolExecutor> =
                Arc::new(LocalMcpToolExecutor {
                    tool_name: name,
                    server: Arc::clone(server),
                });
            Some((def, exec))
        })
        .collect()
}

impl AiExecutionAdapter for DirectApiAiAdapter {
    fn kind(&self) -> AiBackendKind {
        AiBackendKind::DirectApi
    }

    fn test_connection(
        &self,
        config: &AiConfig,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<String, String> {
        let adk_cfg = build_adk_model_config(
            config,
            ctx,
            None,
            Duration::from_secs(SHORT_COMMAND_TIMEOUT_SECS),
            Some(16),
        )?;
        let key_ref = adk_cfg.api_key.as_str();
        let client = ruum_adk::BlockingAdkClient::new()
            .map_err(|err| sanitize_adk_error(&err, key_ref))?;
        let probe = client
            .test_connection(&adk_cfg)
            .map_err(|err| sanitize_adk_error(&err, key_ref))?;
        Ok(format!("{} ({})", probe.message, probe.endpoint))
    }

    fn discover_models(
        &self,
        config: &AiConfig,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
        let adk_cfg = build_adk_model_config(
            config,
            ctx,
            None,
            Duration::from_secs(SHORT_COMMAND_TIMEOUT_SECS),
            None,
        )?;
        let key_ref = adk_cfg.api_key.as_str();
        let client = ruum_adk::BlockingAdkClient::new()
            .map_err(|err| sanitize_adk_error(&err, key_ref))?;
        let models = client
            .list_models(&adk_cfg)
            .map_err(|err| sanitize_adk_error(&err, key_ref))?;
        let proto_id = config.api_protocol.id().to_owned();
        let mapped: Vec<DiscoveredModel> = models
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.id,
                label: m.label,
                provider: Some(proto_id.clone()),
            })
            .collect();
        let note = mapped.is_empty().then(|| {
            "No models returned by /models; enter a model ID manually if needed."
                .to_owned()
        });
        Ok((mapped, note))
    }

    fn benchmark_model(
        &self,
        config: &AiConfig,
        model_override: Option<&str>,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> ModelSpeedResult {
        let requested_model = model_override
            .or(config.model.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("(none)")
            .to_owned();
        let adk_cfg = match build_adk_model_config(
            config,
            ctx,
            model_override,
            Duration::from_secs(SHORT_COMMAND_TIMEOUT_SECS),
            Some(16),
        ) {
            Ok(cfg) => cfg,
            Err(err) => {
                return ModelSpeedResult {
                    model: requested_model,
                    elapsed_ms: 0,
                    ok: false,
                    detail: err,
                };
            }
        };
        let key_ref = adk_cfg.api_key.as_str();
        let client = match ruum_adk::BlockingAdkClient::new() {
            Ok(c) => c,
            Err(err) => {
                return ModelSpeedResult {
                    model: requested_model,
                    elapsed_ms: 0,
                    ok: false,
                    detail: sanitize_adk_error(&err, key_ref),
                };
            }
        };
        let mut req = ruum_adk::AdkAgentRequest::new(
            "Reply concisely.",
            "Reply with only OK.",
        );
        req.max_iterations = 1;
        req.fallback_without_tools_on_unsupported = false;

        let start = Instant::now();
        let res = client.run_agent(&adk_cfg, req);
        let elapsed_ms =
            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        match res {
            Ok(resp) => {
                let reply = first_line_snippet(&resp.text, 60);
                ModelSpeedResult {
                    model: resp.model,
                    elapsed_ms,
                    ok: true,
                    detail: if reply.is_empty() {
                        "OK".to_owned()
                    } else {
                        reply
                    },
                }
            }
            Err(err) => ModelSpeedResult {
                model: requested_model,
                elapsed_ms,
                ok: false,
                detail: sanitize_adk_error(&err, key_ref),
            },
        }
    }

    fn run_analysis_with_tree(
        &self,
        config: &AiConfig,
        home_dir: &Path,
        _current_exe: &Path,
        now_secs: u64,
        existing_tree: Option<&Node>,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<CleanupProposal, String> {
        let adk_cfg = build_adk_model_config(
            config,
            ctx,
            None,
            Duration::from_secs(DEFAULT_COMMAND_TIMEOUT_SECS),
            None,
        )?;
        let key_ref = adk_cfg.api_key.as_str();
        let client = ruum_adk::BlockingAdkClient::new()
            .map_err(|err| sanitize_adk_error(&err, key_ref))?;

        let candidates =
            collect_reclaimable_candidates(home_dir, existing_tree, now_secs);
        let (instruction, prompt) =
            build_direct_api_analysis_prompt(home_dir, &candidates);

        let mut mcp = McpServer::default().with_read_only(true);
        if let Some(tree) = existing_tree {
            mcp.seed_local_tree_cache(
                home_dir,
                tree.clone(),
                Some(home_dir.to_path_buf()),
            );
        }
        let shared_mcp = Arc::new(Mutex::new(mcp));

        let mut req = ruum_adk::AdkAgentRequest::new(instruction, prompt);
        req.tools = build_local_mcp_adk_tools(&shared_mcp);

        let resp = client
            .run_agent(&adk_cfg, req)
            .map_err(|err| sanitize_adk_error(&err, key_ref))?;
        parse_and_validate_proposal(&resp.text, home_dir, now_secs)
    }

    fn run_tree_analysis(
        &self,
        config: &AiConfig,
        root_path: &Path,
        home_path: Option<&Path>,
        tree: &Node,
        docker: &DockerProbeStatus,
        target_os: TrashOs,
        now_secs: u64,
        _runner: &dyn CommandRunner,
        ctx: &AiExecutionContext,
    ) -> Result<CleanupProposal, String> {
        let adk_cfg = build_adk_model_config(
            config,
            ctx,
            None,
            Duration::from_secs(DEFAULT_COMMAND_TIMEOUT_SECS),
            None,
        )?;
        let key_ref = adk_cfg.api_key.as_str();
        let client = ruum_adk::BlockingAdkClient::new()
            .map_err(|err| sanitize_adk_error(&err, key_ref))?;

        let candidates = collect_tree_candidates(root_path, tree, docker);
        let prompt =
            build_tree_analysis_prompt(root_path, target_os, &candidates);

        // Remote / in-memory analysis runs single-turn inference with zero
        // local disk or SSH tools mounted.
        let mut req = ruum_adk::AdkAgentRequest::new(
            "You are an automated disk cleanup advisor for Ruum. Analyze \
             only the provided in-memory snapshot and output strictly valid \
             JSON.",
            prompt,
        );
        req.max_iterations = 1;
        req.fallback_without_tools_on_unsupported = false;

        let resp = client
            .run_agent(&adk_cfg, req)
            .map_err(|err| sanitize_adk_error(&err, key_ref))?;
        validate_proposal_against_tree(
            &resp.text, root_path, home_path, tree, docker, target_os, now_secs,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockCommandRunner {
        calls: Mutex<Vec<(PathBuf, Vec<String>)>>,
        next_output: Mutex<Option<CommandOutput>>,
    }

    impl MockCommandRunner {
        fn with_output(output: CommandOutput) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                next_output: Mutex::new(Some(output)),
            }
        }
    }

    impl CommandRunner for MockCommandRunner {
        fn run(
            &self,
            program: &Path,
            args: &[&str],
            _envs: &[(&str, &str)],
        ) -> io::Result<CommandOutput> {
            self.calls.lock().unwrap().push((
                program.to_path_buf(),
                args.iter().map(|s| (*s).to_string()).collect(),
            ));
            let value = self.next_output.lock().unwrap().take();
            if let Some(out) = value {
                Ok(out)
            } else {
                Ok(CommandOutput {
                    status_code: 0,
                    stdout: "mock stdout".to_string(),
                    stderr: String::new(),
                })
            }
        }
    }

    #[test]
    fn enriched_path_lookup_and_executable_resolution() {
        let path = enriched_path_var();
        assert!(!path.is_empty());

        // Standard commands like 'sh' or 'cargo' should resolve if present
        let resolved = resolve_cli_executable("sh");
        if cfg!(unix) {
            assert!(resolved.is_some());
        }
    }

    #[test]
    fn windows_candidate_bin_dirs_and_extension_resolution() {
        let dirs = candidate_bin_dirs_for(
            false,
            false,
            true,
            Some(PathBuf::from(r"C:\Users\tester")),
            Some(PathBuf::from(r"C:\Users\tester\AppData\Local")),
            Some(PathBuf::from(r"C:\Users\tester\AppData\Roaming")),
        );
        assert!(dirs.contains(
            &PathBuf::from(r"C:\Users\tester\AppData\Local").join("Programs")
        ));
        assert!(dirs.contains(
            &PathBuf::from(r"C:\Users\tester\AppData\Roaming").join("npm")
        ));

        let temp = tempfile::tempdir().unwrap();
        let npm_dir = temp.path().join("npm");
        fs::create_dir_all(&npm_dir).unwrap();
        let cmd_path = npm_dir.join("codex.cmd");
        let bat_path = npm_dir.join("pi.bat");
        let exe_path = npm_dir.join("agy.exe");
        fs::write(&cmd_path, b"@echo off\n").unwrap();
        fs::write(&bat_path, b"@echo off\n").unwrap();
        fs::write(&exe_path, b"").unwrap();

        let path_str = npm_dir.to_string_lossy().to_string();
        assert_eq!(
            resolve_cli_executable_with("agy", true, &path_str),
            Some(exe_path)
        );
        assert_eq!(
            resolve_cli_executable_with("codex", true, &path_str),
            Some(cmd_path.clone())
        );
        assert_eq!(
            resolve_cli_executable_with("pi", true, &path_str),
            Some(bat_path)
        );
        // Non-Windows ignores .cmd/.bat/.exe fallback
        assert_eq!(
            resolve_cli_executable_with("codex", false, &path_str),
            None
        );
        // Direct path with extension fallback
        let direct_codex = npm_dir.join("codex");
        assert_eq!(
            resolve_cli_executable_with(
                &direct_codex.to_string_lossy(),
                true,
                ""
            ),
            Some(cmd_path)
        );
    }

    #[test]
    fn build_invocation_all_providers() {
        let temp = tempfile::tempdir().unwrap();
        let fake_bin = temp.path().join("my-cli");
        fs::write(&fake_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let custom_str = fake_bin.to_string_lossy().to_string();

        // 1. Agy
        let cfg_agy = WatchdogConfig {
            ai_provider: AiCliProvider::Agy,
            custom_command: custom_str.clone(),
            model: Some("claude-3-5".to_string()),
            extra_args: "--verbose".to_string(),
            ..WatchdogConfig::default()
        };
        let inv_agy = build_invocation(&cfg_agy, "analyze this").unwrap();
        assert_eq!(inv_agy.program, fake_bin);
        assert_eq!(
            inv_agy.args,
            vec![
                "-p",
                "analyze this",
                "--disable-slash-commands",
                "--settings",
                r#"{"permissions":{"allow":["mcp(*)"]}}"#,
                "--model",
                "claude-3-5",
                "--verbose"
            ]
        );

        // 2. Pi
        let cfg_pi = WatchdogConfig {
            ai_provider: AiCliProvider::Pi,
            custom_command: custom_str.clone(),
            model: None,
            extra_args: String::new(),
            ..WatchdogConfig::default()
        };
        let inv_pi = build_invocation(&cfg_pi, "clean it").unwrap();
        assert_eq!(inv_pi.program, fake_bin);
        assert_eq!(inv_pi.args, vec!["-p", "clean it", "--no-session"]);

        // 3. Codex
        let cfg_codex = WatchdogConfig {
            ai_provider: AiCliProvider::Codex,
            custom_command: custom_str.clone(),
            model: Some("o3-mini".to_string()),
            extra_args: String::new(),
            ..WatchdogConfig::default()
        };
        let inv_codex = build_invocation(&cfg_codex, "inspect").unwrap();
        assert_eq!(inv_codex.program, fake_bin);
        assert_eq!(
            inv_codex.args,
            vec![
                "exec",
                "inspect",
                "--skip-git-repo-check",
                "--ephemeral",
                "--model",
                "o3-mini"
            ]
        );

        // 4. Custom
        let cfg_custom = WatchdogConfig {
            ai_provider: AiCliProvider::Custom,
            custom_command: custom_str,
            model: None,
            extra_args: "--dry-run".to_string(),
            ..WatchdogConfig::default()
        };
        let inv_custom = build_invocation(&cfg_custom, "prompt").unwrap();
        assert_eq!(inv_custom.program, fake_bin);
        assert_eq!(inv_custom.args, vec!["prompt", "--dry-run"]);
    }

    #[test]
    fn analysis_prompt_contains_read_only_and_mcp_cmd() {
        let home = Path::new("/Users/tester");
        let exe = Path::new("/usr/local/bin/ruum");
        let prompt = build_analysis_prompt(home, exe);
        assert!(prompt.contains("/usr/local/bin/ruum mcp --read-only"));
        assert!(prompt.contains("STRICT READ-ONLY"));
        assert!(prompt.contains("scan_directory"));
        assert!(prompt.contains("find_reclaimable"));
        assert!(prompt.contains("check_git_safety"));
    }

    #[test]
    fn extract_json_variants() {
        // Raw JSON
        let raw = "{\"summary\": \"test\", \"items\": []}";
        assert_eq!(extract_json(raw), Some(raw));

        // Markdown code fence ```json ... ```
        let md = "Here is the plan:\n```json\n{\n  \"summary\": \"ok\"\n}\n```\nDone.";
        assert_eq!(extract_json(md), Some("{\n  \"summary\": \"ok\"\n}"));

        // Markdown code fence ``` ... ```
        let md_plain = "Result:\n```\n{\"summary\": \"plain\"}\n```";
        assert_eq!(extract_json(md_plain), Some("{\"summary\": \"plain\"}"));

        // Invalid output with no JSON
        assert_eq!(extract_json("No json here!"), None);
    }

    #[test]
    fn parse_and_validate_proposal_filters_forbidden_paths() {
        let temp_home = tempfile::tempdir().unwrap();
        let home_path = temp_home.path();

        // Create a safe target inside home
        let safe_dir = home_path.join("project").join("target");
        fs::create_dir_all(&safe_dir).unwrap();
        fs::write(safe_dir.join("build.o"), b"object code").unwrap();

        // Create an untracked git repo to verify git status annotation
        let git_dir = home_path.join("git_project");
        fs::create_dir_all(git_dir.join(".git")).unwrap();
        fs::write(git_dir.join("file.txt"), b"dirty content").unwrap();

        let json_input = serde_json::json!({
            "summary": "Clean build cache and system",
            "items": [
                {
                    "path": safe_dir.to_string_lossy(),
                    "mode": "trash",
                    "reason": "Cargo target directory",
                },
                {
                    "path": "/etc/shadow",
                    "mode": "permanent",
                    "reason": "Forbidden system file",
                },
                {
                    "path": git_dir.to_string_lossy(),
                    "mode": "trash",
                    "reason": "Git checkout candidate",
                },
            ],
        })
        .to_string();

        let proposal =
            parse_and_validate_proposal(&json_input, home_path, 1_700_000_000)
                .unwrap();

        assert_eq!(proposal.timestamp_secs, 1_700_000_000);
        assert_eq!(proposal.summary, "Clean build cache and system");

        // `/etc/shadow` must be filtered out by removal::plan!
        assert!(
            !proposal
                .items
                .iter()
                .any(|i| i.path == Path::new("/etc/shadow"))
        );

        // safe_dir must be retained
        let safe_item =
            proposal.items.iter().find(|i| i.path == safe_dir).unwrap();
        assert_eq!(safe_item.mode, RemovalMode::Trash);
        assert_eq!(safe_item.reason.as_deref(), Some("Cargo target directory"));
        assert!(safe_item.bytes > 0);
    }

    #[test]
    fn test_connection_success_and_error() {
        let temp = tempfile::tempdir().unwrap();
        let fake_bin = temp.path().join("fake-ai");
        fs::write(&fake_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let cfg = WatchdogConfig {
            ai_provider: AiCliProvider::Custom,
            custom_command: fake_bin.to_string_lossy().to_string(),
            ..WatchdogConfig::default()
        };

        // 1. Success
        let runner_ok = MockCommandRunner::with_output(CommandOutput {
            status_code: 0,
            stdout: "fake-ai v1.2.3".to_string(),
            stderr: String::new(),
        });
        let res = test_connection(&cfg, &runner_ok).unwrap();
        assert!(res.contains("fake-ai v1.2.3"));

        // 2. Failure exit code
        let runner_err = MockCommandRunner::with_output(CommandOutput {
            status_code: 1,
            stdout: String::new(),
            stderr: "permission denied".to_string(),
        });
        let err = test_connection(&cfg, &runner_err).unwrap_err();
        assert!(err.contains("permission denied"));
    }

    #[test]
    fn parse_agy_models_strips_spinners_and_ansi() {
        let raw = "\x1b[36m⠋ Fetching available models...\x1b[0m\r\
                   \x1b[2Kgemini-4-hydrogen-high  Gemini 4 Hydrogen (High)\n\
                   claude-sonnet-4-6  Claude Sonnet 4.6\n\
                   gemini-4-hydrogen-high  Gemini 4 Hydrogen (High)\n\
                   Loading...\n";
        let models = parse_agy_models(raw);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gemini-4-hydrogen-high");
        assert_eq!(models[0].label, "Gemini 4 Hydrogen (High)");
        assert_eq!(models[1].id, "claude-sonnet-4-6");
        assert_eq!(models[1].label, "Claude Sonnet 4.6");
    }

    #[test]
    fn parse_pi_models_table() {
        let raw = "provider      model             context  max-out  thinking  images\n\
                   openai-codex  gpt-6-sol         272K     128K     yes       yes\n\
                   anthropic     claude-opus-4-6   200K     64K      yes       yes\n";
        let models = parse_pi_models(raw);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "openai-codex/gpt-6-sol");
        assert_eq!(models[0].provider.as_deref(), Some("openai-codex"));
        assert_eq!(models[1].id, "anthropic/claude-opus-4-6");
    }

    #[test]
    fn discover_codex_models_reads_config_and_cache() {
        let temp = tempfile::tempdir().unwrap();
        let codex_dir = temp.path();
        fs::write(
            codex_dir.join("config.toml"),
            "model = \"gpt-6-sol\"\n[features]\nfoo = true\n",
        )
        .unwrap();
        fs::write(
            codex_dir.join("models_cache.json"),
            r#"{"models":[
              {"slug":"gpt-6-sol","display_name":"GPT 6 Sol","visibility":"list"},
              {"slug":"o4-mini","display_name":"o4 Mini","visibility":"list"},
              {"slug":"internal-hidden","display_name":"Hidden","visibility":"hide"}
            ]}"#,
        )
        .unwrap();

        let models = discover_codex_models_in_dir(codex_dir);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-6-sol", "o4-mini"]);
        assert_eq!(models[0].label, "GPT 6 Sol");
    }

    #[test]
    fn test_connection_and_discover_parses_models() {
        struct SeqRunner {
            calls: Mutex<Vec<Vec<String>>>,
        }
        impl CommandRunner for SeqRunner {
            fn run(
                &self,
                _program: &Path,
                args: &[&str],
                _envs: &[(&str, &str)],
            ) -> io::Result<CommandOutput> {
                self.calls
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|s| (*s).to_string()).collect());
                if args == ["--version"] {
                    Ok(CommandOutput {
                        status_code: 0,
                        stdout: "agy 1.5.8".to_string(),
                        stderr: String::new(),
                    })
                } else {
                    Ok(CommandOutput {
                        status_code: 0,
                        stdout: "gemini-4-hydrogen-high  Gemini 4\n"
                            .to_string(),
                        stderr: String::new(),
                    })
                }
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let fake_agy = temp.path().join("agy");
        fs::write(&fake_agy, b"#!/bin/sh\nexit 0\n").unwrap();

        let cfg = WatchdogConfig {
            ai_provider: AiCliProvider::Agy,
            custom_command: fake_agy.to_string_lossy().to_string(),
            ..WatchdogConfig::default()
        };
        let runner = SeqRunner {
            calls: Mutex::new(Vec::new()),
        };
        let res = test_connection_and_discover(&cfg, &runner).unwrap();
        assert!(res.version_message.contains("agy 1.5.8"));
        assert_eq!(res.models.len(), 1);
        assert_eq!(res.models[0].id, "gemini-4-hydrogen-high");
    }

    #[test]
    fn benchmark_model_records_outcome_and_override() {
        let temp = tempfile::tempdir().unwrap();
        let fake_bin = temp.path().join("fake-agy");
        fs::write(&fake_bin, b"#!/bin/sh\nexit 0\n").unwrap();

        let cfg = WatchdogConfig {
            ai_provider: AiCliProvider::Agy,
            custom_command: fake_bin.to_string_lossy().to_string(),
            ..WatchdogConfig::default()
        };
        let runner = MockCommandRunner::with_output(CommandOutput {
            status_code: 0,
            stdout: "OK\n".to_string(),
            stderr: String::new(),
        });

        let speed =
            benchmark_model(&cfg, Some("gemini-4-hydrogen-high"), &runner);
        assert!(speed.ok);
        assert_eq!(speed.model, "gemini-4-hydrogen-high");
        assert_eq!(speed.detail, "OK");
        let args = runner.calls.lock().unwrap()[0].1.clone();
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gemini-4-hydrogen-high".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn timed_command_runner_kills_hung_process() {
        let runner = TimedCommandRunner::new(Duration::from_millis(80));
        let start = Instant::now();
        let err = runner
            .run(Path::new("/bin/sh"), &["-c", "sleep 3"], &[])
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn parse_and_validate_proposal_unwraps_response_envelope_and_reports_raw() {
        let temp_home = tempfile::tempdir().unwrap();
        let cache_dir = temp_home.path().join("project").join("target");
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(cache_dir.join("out.bin"), b"12345").unwrap();

        let inner = serde_json::json!({
            "summary": "ok",
            "items": [{
                "path": cache_dir.to_string_lossy(),
                "mode": "trash",
                "reason": "build artifact",
            }],
        })
        .to_string();
        let wrapped = serde_json::json!({
            "status": "SUCCESS",
            "response": format!("```json\n{inner}\n```"),
        })
        .to_string();
        let prop = parse_and_validate_proposal(&wrapped, temp_home.path(), 42)
            .unwrap();
        assert_eq!(prop.items.len(), 1);
        assert_eq!(prop.items[0].path, cache_dir);

        // Plain error text without proposal JSON surfaces diagnostic snippet
        let err = parse_and_validate_proposal(
            "jetski: no output produced — mcp denied",
            temp_home.path(),
            42,
        )
        .unwrap_err();
        assert!(err.contains("jetski: no output produced"));

        // Non-proposal JSON object must not match as a proposal
        let unrelated = r#"{"conversation_id":"abc","status":"FAILED"}"#;
        let err2 = parse_and_validate_proposal(unrelated, temp_home.path(), 42)
            .unwrap_err();
        assert!(err2.contains("conversation_id"));
    }

    #[test]
    fn collect_reclaimable_candidates_and_prompt_include_discovered_dirs() {
        let temp_home = tempfile::tempdir().unwrap();
        let target_dir = temp_home.path().join("myproj").join("target");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("big.o"), vec![b'a'; 4096]).unwrap();

        let cands = collect_reclaimable_candidates(
            temp_home.path(),
            None,
            1_700_000_000,
        );
        assert!(cands.iter().any(|c| c.path == target_dir));

        let prompt = build_analysis_prompt_with_candidates(
            temp_home.path(),
            Path::new("/usr/local/bin/ruum"),
            &cands,
        );
        assert!(prompt.contains("PRE-SCANNED RECLAIMABLE CANDIDATES"));
        assert!(prompt.contains(&target_dir.display().to_string()));
    }

    #[test]
    fn parse_and_validate_proposal_forces_permanent_for_trash_targets() {
        use crate::trash_adapter::{TrashAdapterFactory, TrashEnv, TrashOs};

        let temp_home = tempfile::tempdir().unwrap();
        let trash_item = temp_home.path().join(".Trash").join("old.zip");
        let cache_dir = temp_home.path().join("project").join("target");
        fs::create_dir_all(trash_item.parent().unwrap()).unwrap();
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(&trash_item, b"stale").unwrap();
        fs::write(cache_dir.join("out.bin"), b"build").unwrap();

        let raw = serde_json::json!({
            "summary": "Clean trash and build cache",
            "items": [
                {
                    "path": trash_item.to_string_lossy(),
                    "mode": "trash",
                    "reason": "Old item inside Trash",
                },
                {
                    "path": cache_dir.to_string_lossy(),
                    "mode": "trash",
                    "reason": "Build artifact",
                },
            ],
        })
        .to_string();

        let mac = TrashAdapterFactory::create_for(TrashOs::MacOs);
        let env = TrashEnv {
            home: Some(temp_home.path().to_path_buf()),
            xdg_data_home: None,
            uid: Some(501),
        };
        let prop = parse_and_validate_proposal_with(
            &raw,
            temp_home.path(),
            1_700_000_000,
            |path| mac.classify(path, &env, &[]).is_trash(),
        )
        .unwrap();

        let trash_prop = prop
            .items
            .iter()
            .find(|item| item.path == trash_item)
            .expect("trash item kept");
        assert_eq!(trash_prop.mode, RemovalMode::Permanent);

        let cache_prop = prop
            .items
            .iter()
            .find(|item| item.path == cache_dir)
            .expect("cache item kept");
        assert_eq!(cache_prop.mode, RemovalMode::Trash);
    }

    #[test]
    fn ai_config_entrypoints_work_without_watchdog_config() {
        let temp_home = tempfile::tempdir().unwrap();
        let fake_bin = temp_home.path().join("fake-ai");
        fs::write(&fake_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        let target_dir = temp_home.path().join("target");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("out.bin"), b"12345").unwrap();

        let cfg = AiConfig {
            auto_analyze_on_alert: true,
            provider: AiCliProvider::Custom,
            custom_command: fake_bin.to_string_lossy().into_owned(),
            extra_args: "--strict".to_owned(),
            model: Some("model-x".to_owned()),
            updated_at_secs: 1,
            ..AiConfig::default()
        };

        let inv = build_invocation_for_config(&cfg, "check disk").unwrap();
        assert_eq!(inv.program, fake_bin);
        assert_eq!(
            inv.args,
            vec!["check disk", "--model", "model-x", "--strict"]
        );

        let conn_runner = MockCommandRunner::with_output(CommandOutput {
            status_code: 0,
            stdout: "fake-ai 2.0".to_owned(),
            stderr: String::new(),
        });
        assert!(
            test_connection_for_config(&cfg, &conn_runner)
                .unwrap()
                .contains("fake-ai 2.0")
        );

        let speed_runner = MockCommandRunner::with_output(CommandOutput {
            status_code: 0,
            stdout: "OK".to_owned(),
            stderr: String::new(),
        });
        let speed = benchmark_model_for_config(&cfg, None, &speed_runner);
        assert!(speed.ok);
        assert_eq!(speed.model, "model-x");

        let prop_json = serde_json::json!({
            "summary": "Found target",
            "items": [{
                "path": target_dir.to_string_lossy(),
                "mode": "trash",
                "reason": "build dir",
            }],
        })
        .to_string();
        let analysis_runner = MockCommandRunner::with_output(CommandOutput {
            status_code: 0,
            stdout: prop_json,
            stderr: String::new(),
        });
        let prop = run_analysis_for_config(
            &cfg,
            temp_home.path(),
            Path::new("/usr/local/bin/ruum"),
            1_700_000_000,
            &analysis_runner,
        )
        .unwrap();
        assert_eq!(prop.summary, "Found target");
        assert_eq!(prop.items.len(), 1);
        assert_eq!(prop.items[0].path, target_dir);
    }

    #[test]
    fn windows_batch_detection_arg_sanitization_and_npm_shim_unwrap() {
        assert!(is_windows_batch_script(Path::new("codex.cmd")));
        assert!(is_windows_batch_script(Path::new("CODEX.CMD")));
        assert!(is_windows_batch_script(Path::new("pi.bat")));
        assert!(is_windows_batch_script(Path::new("PI.BAT")));
        assert!(!is_windows_batch_script(Path::new("codex.exe")));
        assert!(!is_windows_batch_script(Path::new("codex")));
        assert!(!is_windows_batch_script(Path::new("codex.js")));

        let raw_prompt =
            "line1\r\nline2\n\"quoted\" %TEMP% <json>{\"a\":1}\rline3";
        let sanitized = sanitize_windows_batch_arg(raw_prompt);
        assert!(!sanitized.contains('\r'));
        assert!(!sanitized.contains('\n'));
        assert_eq!(
            sanitized,
            "line1 line2 \"quoted\" %TEMP% <json>{\"a\":1} line3"
        );

        let temp = tempfile::tempdir().unwrap();
        let shim_dir = temp.path().join("npm");
        let node_path_dir = temp.path().join("nodejs");
        fs::create_dir_all(&shim_dir).unwrap();
        fs::create_dir_all(&node_path_dir).unwrap();

        let codex_js = shim_dir.join("node_modules/@openai/codex/bin/codex.js");
        let pi_mjs = shim_dir.join("node_modules/pi/cli.mjs");
        let agy_cjs = shim_dir.join("node_modules/agy/entry.cjs");
        let escape_js = temp.path().join("escape.js");
        let helper_exe = shim_dir.join("helper.exe");
        for file in [&codex_js, &pi_mjs, &agy_cjs, &escape_js, &helper_exe] {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, b"// stub\n").unwrap();
        }

        let path_node = node_path_dir.join("node.exe");
        fs::write(&path_node, b"").unwrap();
        let path_str = node_path_dir.to_string_lossy().into_owned();

        let codex_cmd = shim_dir.join("codex.cmd");
        fs::write(
            &codex_cmd,
            "@IF EXIST \"%~dp0\\node.exe\" (\r\n\
               \"%~dp0\\node.exe\" \"%~dp0\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n\
             ) ELSE (\r\n\
               node \"%~dp0\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n\
             )\r\n",
        )
        .unwrap();

        assert_eq!(
            unwrap_npm_cmd_shim_with(&codex_cmd, false, &path_str),
            None
        );
        assert_eq!(
            unwrap_npm_cmd_shim_with(&codex_cmd, true, &path_str),
            Some((path_node, codex_js))
        );

        let local_node = shim_dir.join("node.exe");
        fs::write(&local_node, b"").unwrap();

        let pi_cmd = shim_dir.join("pi.cmd");
        fs::write(
            &pi_cmd,
            "@ECHO off\r\n\"%DP0%\\node_modules\\pi\\cli.mjs\" %*\r\n",
        )
        .unwrap();
        assert_eq!(
            unwrap_npm_cmd_shim_with(&pi_cmd, true, ""),
            Some((local_node.clone(), pi_mjs))
        );

        let agy_bat = shim_dir.join("agy.bat");
        fs::write(
            &agy_bat,
            "@ECHO off\r\n\"%dp0%\\node_modules\\agy\\entry.cjs\" %*\r\n",
        )
        .unwrap();
        assert_eq!(
            unwrap_npm_cmd_shim_with(&agy_bat, true, ""),
            Some((local_node, agy_cjs))
        );

        let escape_cmd = shim_dir.join("escape.cmd");
        fs::write(&escape_cmd, "\"%~dp0\\..\\escape.js\" %*\r\n").unwrap();
        assert_eq!(unwrap_npm_cmd_shim_with(&escape_cmd, true, ""), None);

        let missing_cmd = shim_dir.join("missing.cmd");
        fs::write(&missing_cmd, "\"%~dp0\\node_modules\\missing.js\" %*\r\n")
            .unwrap();
        assert_eq!(unwrap_npm_cmd_shim_with(&missing_cmd, true, ""), None);

        let non_js_cmd = shim_dir.join("non-js.cmd");
        fs::write(&non_js_cmd, "\"%~dp0\\helper.exe\" %*\r\n").unwrap();
        assert_eq!(unwrap_npm_cmd_shim_with(&non_js_cmd, true, ""), None);

        let huge_cmd = shim_dir.join("huge.cmd");
        let mut huge = String::from("\"%~dp0\\node_modules\\pi\\cli.mjs\"\r\n");
        huge.push_str(&"REM padding\r\n".repeat(4_000));
        fs::write(&huge_cmd, huge).unwrap();
        assert_eq!(unwrap_npm_cmd_shim_with(&huge_cmd, true, ""), None);

        assert_eq!(unwrap_npm_cmd_shim_with(&helper_exe, true, ""), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_real_cmd_executes_with_sanitized_multiline_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let cmd_path = temp.path().join("echo-prompt.cmd");
        fs::write(&cmd_path, "@echo off\r\necho %*\r\n").unwrap();

        let prompt =
            "line1\r\nline2\n\"quoted\" %TEMP% <json>{\"ok\":true}</json>";
        let output = DefaultCommandRunner
            .run(&cmd_path, &[prompt], &[("TEMP", "RUUM_SAFE_TEMP")])
            .unwrap();
        assert!(output.success(), "stderr: {}", output.stderr);
        assert!(output.stdout.contains("line1"), "stdout: {}", output.stdout);
        assert!(output.stdout.contains("line2"), "stdout: {}", output.stdout);
        assert!(
            output.stdout.contains("quoted"),
            "stdout: {}",
            output.stdout
        );
        assert!(
            output.stdout.contains("<json>"),
            "stdout: {}",
            output.stdout
        );
        assert!(
            output.stdout.contains("%TEMP%")
                || output.stdout.contains("RUUM_SAFE_TEMP"),
            "stdout: {}",
            output.stdout
        );
    }

    fn build_sample_docker_summary() -> crate::docker_adapter::DockerUsageSummary
    {
        use crate::docker_adapter::{
            DockerContainerEntry, DockerImageEntry, DockerUsageSummary,
        };
        DockerUsageSummary {
            build_cache_reclaimable_bytes: 512 * 1024 * 1024,
            removable_images: vec![
                DockerImageEntry {
                    id: "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_owned(),
                    repository: "old-app".to_owned(),
                    tag: "v1".to_owned(),
                    created_since: "2 weeks ago".to_owned(),
                    size_bytes: 300_000_000,
                    unique_bytes: 200_000_000,
                    dangling: false,
                },
                DockerImageEntry {
                    id: "sha256:2222222222222222222222222222222222222222222222222222222222222222".to_owned(),
                    repository: "<none>".to_owned(),
                    tag: "<none>".to_owned(),
                    created_since: "3 weeks ago".to_owned(),
                    size_bytes: 100_000_000,
                    unique_bytes: 100_000_000,
                    dangling: true,
                },
            ],
            stopped_containers: vec![DockerContainerEntry {
                id: "abcdef012345".to_owned(),
                name: "old-worker".to_owned(),
                image: "old-app:v1".to_owned(),
                state: "exited".to_owned(),
                status: "Exited (0)".to_owned(),
                size_bytes: 50_000_000,
            }],
            detailed: true,
            ..DockerUsageSummary::default()
        }
    }

    fn make_tree_dir(name: &str, children: Vec<Node>) -> Node {
        let mut dir = Node::directory(name);
        dir.children = children;
        dir
    }

    #[test]
    fn collect_tree_candidates_uses_memory_tree_and_docker_summary_without_fs()
    {
        let mut root = make_tree_dir(
            "deploy",
            vec![
                make_tree_dir(
                    ".cache",
                    vec![make_tree_dir(
                        "pip",
                        vec![Node::entry(
                            "wheel.whl",
                            NodeKind::File,
                            90 << 20,
                        )],
                    )],
                ),
                make_tree_dir(
                    ".local",
                    vec![make_tree_dir(
                        "share",
                        vec![make_tree_dir(
                            "docker",
                            vec![Node::entry(
                                "overlay",
                                NodeKind::File,
                                500 << 20,
                            )],
                        )],
                    )],
                ),
                Node::entry("sym_cache", NodeKind::Symlink, 200 << 20),
                Node::entry("huge.log", NodeKind::File, 40 << 20),
            ],
        );
        crate::tree::aggregate(&mut root, crate::tree::Metric::Bytes);
        crate::classify::classify(&mut root);

        let summary = build_sample_docker_summary();
        let docker = DockerProbeStatus::Available(summary.clone());
        let candidates =
            collect_tree_candidates(Path::new("/srv/deploy"), &root, &docker);

        assert_eq!(candidates.docker, Some(summary));
        assert!(
            candidates
                .items
                .iter()
                .any(|c| c.path == Path::new("/srv/deploy/.cache"))
        );
        assert!(
            candidates
                .items
                .iter()
                .any(|c| c.path == Path::new("/srv/deploy/huge.log"))
        );
        assert!(
            !candidates
                .items
                .iter()
                .any(|c| c.path.to_string_lossy().contains("sym_cache")),
            "symlinks must not be proposed"
        );
        assert!(
            !candidates
                .items
                .iter()
                .any(|c| c.path.to_string_lossy().contains("docker")),
            "container storage must not be proposed"
        );
    }

    #[test]
    fn validate_proposal_against_tree_enforces_tree_truth_symlinks_docker_and_cross_os_guards()
     {
        // 1. Linux remote tree validation (using paths that do not exist on host).
        let mut linux_root = make_tree_dir(
            "/",
            vec![
                make_tree_dir(
                    "etc",
                    vec![Node::entry("passwd", NodeKind::File, 1024)],
                ),
                make_tree_dir(
                    "home",
                    vec![make_tree_dir(
                        "deploy",
                        vec![
                            make_tree_dir(
                                ".cache",
                                vec![make_tree_dir(
                                    "pip",
                                    vec![Node::entry(
                                        "pkg.whl",
                                        NodeKind::File,
                                        123_456,
                                    )],
                                )],
                            ),
                            make_tree_dir(
                                ".local",
                                vec![make_tree_dir(
                                    "share",
                                    vec![make_tree_dir(
                                        "docker",
                                        vec![Node::entry(
                                            "layer",
                                            NodeKind::File,
                                            999_999,
                                        )],
                                    )],
                                )],
                            ),
                            Node::entry("link_dir", NodeKind::Symlink, 50_000),
                            Node::entry("stale.log", NodeKind::File, 77_777),
                        ],
                    )],
                ),
            ],
        );
        crate::tree::aggregate(&mut linux_root, crate::tree::Metric::Bytes);
        crate::classify::classify(&mut linux_root);

        let docker =
            DockerProbeStatus::Available(build_sample_docker_summary());
        let raw_json = r#"{
            "summary": "Safe + unsafe mix",
            "items": [
                {"path": "/home/deploy/.cache/pip", "mode": "permanent", "reason": "pip cache"},
                {"path": "home/deploy/stale.log", "mode": "trash", "reason": "relative under root"},
                {"path": "/home/deploy/link_dir", "mode": "permanent", "reason": "symlink must be dropped"},
                {"path": "/home/deploy/nonexistent", "mode": "permanent", "reason": "hallucinated"},
                {"path": "/home/deploy/../etc/passwd", "mode": "permanent", "reason": "system path"},
                {"path": "/home", "mode": "permanent", "reason": "home ancestor"},
                {"path": "/home/deploy/.local/share", "mode": "permanent", "reason": "container storage ancestor"},
                {"path": "/home/deploy/.local/share/docker", "mode": "permanent", "reason": "container storage"}
            ],
            "docker_targets": [
                {"kind": "build_cache"},
                {"kind": "image", "id": "sha256:1111111111111111111111111111111111111111111111111111111111111111", "reference": "--force-evil"},
                {"kind": "image", "id": "sha256:1111111111111111111111111111111111111111111111111111111111111111"},
                {"kind": "image", "id": "sha256:9999999999999999999999999999999999999999999999999999999999999999"},
                {"kind": "container", "id": "abcdef012345", "name": "--injected"},
                {"kind": "container", "id": "deadbeef0000", "name": "running-prod"},
                {"kind": "volume", "id": "vol1"}
            ]
        }"#;

        let proposal = validate_proposal_against_tree(
            raw_json,
            Path::new("/"),
            Some(Path::new("/home/deploy")),
            &linux_root,
            &docker,
            TrashOs::Linux,
            1_700_000_000,
        )
        .expect("valid proposal");

        assert_eq!(proposal.items.len(), 2);
        assert_eq!(
            proposal.items[0].path,
            PathBuf::from("/home/deploy/.cache/pip")
        );
        assert_eq!(proposal.items[0].bytes, 123_456);
        assert_eq!(proposal.items[0].mode, RemovalMode::Permanent);
        assert_eq!(
            proposal.items[1].path,
            PathBuf::from("/home/deploy/stale.log")
        );
        assert_eq!(proposal.items[1].bytes, 77_777);

        // Docker targets: deduplicated, snapshot-reconstructed, ordered Container -> Image -> BuildCache.
        assert_eq!(
            proposal.docker_targets,
            vec![
                DockerCleanTarget::Container {
                    id: "abcdef012345".to_owned(),
                    name: "old-worker".to_owned(),
                },
                DockerCleanTarget::Image {
                    id: "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_owned(),
                    reference: Some("old-app:v1".to_owned()),
                    dangling: false,
                },
                DockerCleanTarget::BuildCache,
            ]
        );

        // PermissionDenied docker drops all docker_targets.
        let denied_prop = validate_proposal_against_tree(
            raw_json,
            Path::new("/"),
            Some(Path::new("/home/deploy")),
            &linux_root,
            &DockerProbeStatus::PermissionDenied {
                socket_hint: "/var/run/docker.sock".to_owned(),
            },
            TrashOs::Linux,
            1_700_000_000,
        )
        .expect("valid proposal without docker");
        assert!(denied_prop.docker_targets.is_empty());

        // 2. macOS cross-OS validation: blocks container storage ancestors.
        let mut mac_root = make_tree_dir(
            "alice",
            vec![make_tree_dir(
                "Library",
                vec![
                    make_tree_dir(
                        "Caches",
                        vec![Node::entry("app.bin", NodeKind::File, 4096)],
                    ),
                    make_tree_dir(
                        "Containers",
                        vec![make_tree_dir(
                            "com.docker.docker",
                            vec![make_tree_dir(
                                "Data",
                                vec![make_tree_dir(
                                    "vms",
                                    vec![Node::entry(
                                        "Docker.raw",
                                        NodeKind::File,
                                        10_000_000,
                                    )],
                                )],
                            )],
                        )],
                    ),
                ],
            )],
        );
        crate::tree::aggregate(&mut mac_root, crate::tree::Metric::Bytes);
        let mac_json = r#"{
            "summary": "mac test",
            "items": [
                {"path": "/Users/alice/Library/Caches/app.bin"},
                {"path": "/Users/alice/Library/Containers/com.docker.docker"},
                {"path": "/Users/alice/Library"}
            ]
        }"#;
        let mac_prop = validate_proposal_against_tree(
            mac_json,
            Path::new("/Users/alice"),
            Some(Path::new("/Users/alice")),
            &mac_root,
            &DockerProbeStatus::NotInstalled,
            TrashOs::MacOs,
            123,
        )
        .expect("mac proposal");
        assert_eq!(mac_prop.items.len(), 1);
        assert_eq!(
            mac_prop.items[0].path,
            PathBuf::from("/Users/alice/Library/Caches/app.bin")
        );

        // 3. Windows pure lexical validation (even when tests run on Unix).
        let mut win_root = make_tree_dir(
            "alice",
            vec![make_tree_dir(
                "AppData",
                vec![make_tree_dir(
                    "Local",
                    vec![
                        make_tree_dir(
                            "Temp",
                            vec![Node::entry("junk.tmp", NodeKind::File, 8192)],
                        ),
                        make_tree_dir(
                            "Docker",
                            vec![make_tree_dir(
                                "wsl",
                                vec![make_tree_dir(
                                    "data",
                                    vec![Node::entry(
                                        "ext4.vhdx",
                                        NodeKind::File,
                                        9_999_999,
                                    )],
                                )],
                            )],
                        ),
                    ],
                )],
            )],
        );
        crate::tree::aggregate(&mut win_root, crate::tree::Metric::Bytes);
        let win_json = r#"{
            "summary": "win test",
            "items": [
                {"path": "C:\\Users\\alice\\AppData\\Local\\Temp\\junk.tmp"},
                {"path": "C:\\Users\\alice\\AppData\\Local\\Docker\\wsl"}
            ]
        }"#;
        let win_prop = validate_proposal_against_tree(
            win_json,
            Path::new(r"C:\Users\alice"),
            Some(Path::new(r"C:\Users\alice")),
            &win_root,
            &DockerProbeStatus::NotInstalled,
            TrashOs::Windows,
            123,
        )
        .expect("win proposal");
        assert_eq!(win_prop.items.len(), 1);
        assert_eq!(win_prop.items[0].bytes, 8192);
    }

    #[test]
    fn run_tree_analysis_for_config_analyzes_remote_snapshot_locally() {
        let temp = tempfile::tempdir().unwrap();
        let fake_cli = temp.path().join("fake-ai");
        fs::write(&fake_cli, b"#!/bin/sh\n").unwrap();

        let mut tree = make_tree_dir(
            "deploy",
            vec![make_tree_dir(
                ".cache",
                vec![Node::entry("blob.bin", NodeKind::File, 128 << 20)],
            )],
        );
        crate::tree::aggregate(&mut tree, crate::tree::Metric::Bytes);
        crate::classify::classify(&mut tree);

        let runner = MockCommandRunner::with_output(CommandOutput {
            status_code: 0,
            stdout: r#"{"summary":"Remote cache + build cache","items":[{"path":"/home/deploy/.cache","mode":"permanent","reason":"pip cache"}],"docker_targets":[{"kind":"build_cache"}]}"#.to_owned(),
            stderr: String::new(),
        });
        let cfg = AiConfig {
            provider: AiCliProvider::Custom,
            custom_command: fake_cli.to_string_lossy().into_owned(),
            ..AiConfig::default()
        };
        let docker =
            DockerProbeStatus::Available(build_sample_docker_summary());

        let prop = run_tree_analysis_for_config(
            &cfg,
            Path::new("/home/deploy"),
            Some(Path::new("/home/deploy")),
            &tree,
            &docker,
            TrashOs::Linux,
            1_700_000_555,
            &runner,
        )
        .expect("local tree analysis");

        assert_eq!(prop.items.len(), 1);
        assert_eq!(prop.items[0].path, PathBuf::from("/home/deploy/.cache"));
        assert_eq!(prop.items[0].bytes, 128 << 20);
        assert_eq!(prop.docker_targets, vec![DockerCleanTarget::BuildCache]);
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1[0].contains("/home/deploy/.cache"));
        drop(calls);
    }

    fn read_mock_http_request(
        stream: &mut std::net::TcpStream,
    ) -> (String, String) {
        use std::io::Read as _;

        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0, "unexpected EOF reading HTTP headers");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos;
            }
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let content_len: usize = headers
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        let total_len = header_end + 4 + content_len;
        while buf.len() < total_len {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let body = String::from_utf8_lossy(&buf[header_end + 4..total_len])
            .into_owned();
        (headers, body)
    }

    fn write_mock_http_response(
        stream: &mut std::net::TcpStream,
        status: &str,
        content_type: &str,
        body: &str,
    ) {
        use std::io::Write as _;

        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    fn write_mock_http_json(
        stream: &mut std::net::TcpStream,
        status: &str,
        body: &str,
    ) {
        write_mock_http_response(stream, status, "application/json", body);
    }

    fn openai_sse_body(
        content: Option<&str>,
        tool_call: Option<(&str, &str, &str)>,
    ) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        if let Some(text) = content {
            let chunk = serde_json::json!({
                "id": "chatcmpl-1",
                "object": "chat.completion.chunk",
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "content": text },
                    "finish_reason": null
                }]
            });
            let _ = write!(out, "data: {chunk}\n\n");
        }
        if let Some((id, name, args_json)) = tool_call {
            let chunk = serde_json::json!({
                "id": "chatcmpl-1",
                "object": "chat.completion.chunk",
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "tool_calls": [{
                            "index": 0,
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": args_json
                            }
                        }]
                    },
                    "finish_reason": null
                }]
            });
            let _ = write!(out, "data: {chunk}\n\n");
        }
        let finish = if tool_call.is_some() {
            "tool_calls"
        } else {
            "stop"
        };
        let done = serde_json::json!({
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "model": "test-model",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": finish
            }]
        });
        let _ = write!(out, "data: {done}\n\ndata: [DONE]\n\n");
        out
    }

    fn anthropic_sse_body(text: &str) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        let start = serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-mock",
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": { "input_tokens": 10, "output_tokens": 0 }
            }
        });
        let bs = serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        });
        let bd = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": text }
        });
        let be =
            serde_json::json!({ "type": "content_block_stop", "index": 0 });
        let md = serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn", "stop_sequence": null },
            "usage": { "output_tokens": 5 }
        });
        let _ = write!(
            out,
            "event: message_start\ndata: {start}\n\n\
             event: content_block_start\ndata: {bs}\n\n\
             event: content_block_delta\ndata: {bd}\n\n\
             event: content_block_stop\ndata: {be}\n\n\
             event: message_delta\ndata: {md}\n\n\
             event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
        );
        out
    }

    #[test]
    fn ai_backend_factory_routes_cli_and_direct_api_with_zero_keychain_on_cli()
    {
        use crate::credentials::CREDENTIALS_FILE_NAME;
        use crate::credentials::LazyCredentialStore;
        use crate::credentials::test_helpers::MemoryRootSecretStore;
        use std::sync::Arc;

        for cli in [
            AiCliProvider::Agy,
            AiCliProvider::Pi,
            AiCliProvider::Codex,
            AiCliProvider::Custom,
        ] {
            assert_eq!(AiBackendFactory::kind_for(cli), AiBackendKind::Cli);
            assert_eq!(
                AiBackendFactory::create(cli).kind(),
                AiBackendKind::Cli
            );
        }
        assert_eq!(
            AiBackendFactory::kind_for(AiCliProvider::Api),
            AiBackendKind::DirectApi
        );
        assert_eq!(
            AiBackendFactory::create(AiCliProvider::Api).kind(),
            AiBackendKind::DirectApi
        );

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join(CREDENTIALS_FILE_NAME);
        let root_store = Arc::new(MemoryRootSecretStore::new([7_u8; 32]));
        let cred_store =
            LazyCredentialStore::new(file_path, root_store.clone());
        let fake_cli = dir.path().join("fake-cli");
        fs::write(&fake_cli, b"").unwrap();
        let cfg = AiConfig {
            provider: AiCliProvider::Custom,
            custom_command: fake_cli.to_string_lossy().into_owned(),
            ..AiConfig::default()
        };
        let runner = MockCommandRunner::with_output(CommandOutput {
            status_code: 0,
            stdout: "1.0.0".to_owned(),
            stderr: String::new(),
        });
        let ctx =
            AiExecutionContext::default().with_credential_store(cred_store);
        let msg = test_connection_for_config_with_context(&cfg, &runner, &ctx)
            .expect("CLI test_connection");
        assert!(msg.contains("1.0.0"));
        assert_eq!(root_store.call_count(), 0);
    }

    #[test]
    fn direct_api_probe_discover_benchmark_and_error_redaction() {
        use crate::watchdog_config::ApiProtocol;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for idx in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let (headers, _body) = read_mock_http_request(&mut stream);
                let headers_lc = headers.to_ascii_lowercase();
                assert!(
                    headers_lc.contains("authorization: bearer test-api-key"),
                    "missing auth header in request #{idx}: {headers}"
                );
                match idx {
                    0 | 1 => {
                        write_mock_http_json(
                            &mut stream,
                            "200 OK",
                            r#"{"data":[{"id":"mimo-v2-flash"}]}"#,
                        );
                    }
                    2 => {
                        let sse = openai_sse_body(Some("OK"), None);
                        write_mock_http_response(
                            &mut stream,
                            "200 OK",
                            "text/event-stream",
                            &sse,
                        );
                    }
                    _ => {
                        write_mock_http_json(
                            &mut stream,
                            "401 Unauthorized",
                            r#"{"error":"bad key test-api-key"}"#,
                        );
                    }
                }
            }
        });

        let cfg = AiConfig {
            provider: AiCliProvider::Api,
            api_protocol: ApiProtocol::OpenAi,
            api_base_url: format!("http://{addr}/v1"),
            model: Some("mimo-v2-flash".to_owned()),
            ..AiConfig::default()
        };
        let ctx = AiExecutionContext::default().with_api_key("test-api-key");

        let probe_msg = test_connection_for_config_with_context(
            &cfg,
            &MockCommandRunner::default(),
            &ctx,
        )
        .expect("probe succeeds");
        assert!(probe_msg.contains("1 models"));

        let (models, note) = discover_models_for_config_with_context(
            &cfg,
            &MockCommandRunner::default(),
            &ctx,
        )
        .expect("discover succeeds");
        assert!(note.is_none());
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "mimo-v2-flash");

        let speed = benchmark_model_for_config_with_context(
            &cfg,
            None,
            &MockCommandRunner::default(),
            &ctx,
        );
        assert!(speed.ok, "benchmark failed: {}", speed.detail);
        assert_eq!(speed.detail, "OK");

        let err = test_connection_for_config_with_context(
            &cfg,
            &MockCommandRunner::default(),
            &ctx,
        )
        .unwrap_err();
        assert!(!err.contains("test-api-key"), "key leaked in error: {err}");
        assert!(err.contains("[REDACTED]"));

        server.join().unwrap();
    }

    #[test]
    fn direct_api_local_analysis_runs_read_only_mcp_and_reuses_seeded_tree() {
        use crate::credentials::{
            CREDENTIALS_FILE_NAME, LazyCredentialStore,
            test_helpers::MemoryRootSecretStore,
        };
        use crate::watchdog_config::ApiProtocol;
        use std::net::TcpListener;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let home = crate::removal::normalize(dir.path());
        let cache_dir = home.join(".cache");
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(cache_dir.join("blob.bin"), vec![7_u8; 4096]).unwrap();

        let mut tree = make_tree_dir(
            "home",
            vec![make_tree_dir(
                ".cache",
                vec![Node::entry("blob.bin", NodeKind::File, 128 << 20)],
            )],
        );
        crate::tree::aggregate(&mut tree, crate::tree::Metric::Bytes);
        crate::classify::classify(&mut tree);

        let cred_file = home.join(CREDENTIALS_FILE_NAME);
        let root_store = Arc::new(MemoryRootSecretStore::new([11_u8; 32]));
        let cred_store = LazyCredentialStore::new(cred_file, root_store);
        cred_store.set_ai_api_key("test-api-key").unwrap();

        let home_str = home.display().to_string();
        let cache_str = cache_dir.display().to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            // Turn 1: assert tool whitelist & prompt rules, then trigger tool call.
            let (mut s1, _) = listener.accept().unwrap();
            let (_h1, b1) = read_mock_http_request(&mut s1);
            assert!(
                !b1.contains("mcp --read-only"),
                "direct API prompt must not mention `mcp --read-only`"
            );
            let req1: serde_json::Value = serde_json::from_str(&b1).unwrap();
            let tools = req1["tools"].as_array().expect("tools array");
            let mut tool_names: Vec<&str> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str())
                .collect();
            tool_names.sort_unstable();
            assert_eq!(
                tool_names,
                vec![
                    "check_git_safety",
                    "find_reclaimable",
                    "inspect_subtree",
                    "list_volumes",
                    "plan_removal",
                    "scan_directory",
                ]
            );
            for t in tools {
                let props = &t["function"]["parameters"]["properties"];
                assert!(props.get("profile").is_none());
                assert!(props.get("ssh_host").is_none());
            }

            let call_args = serde_json::json!({ "root": home_str }).to_string();
            let turn1 = openai_sse_body(
                None,
                Some(("call_1", "find_reclaimable", &call_args)),
            );
            write_mock_http_response(
                &mut s1,
                "200 OK",
                "text/event-stream",
                &turn1,
            );

            // Turn 2: assert tool output arrived from seeded cache, then reply.
            let (mut s2, _) = listener.accept().unwrap();
            let (_h2, b2) = read_mock_http_request(&mut s2);
            assert!(
                b2.contains("134217728"),
                "seeded 128 MiB cache result must appear in turn 2: {b2}"
            );
            let final_json = serde_json::json!({
                "summary": "Reclaim cached artifacts",
                "items": [{
                    "path": cache_str,
                    "mode": "trash",
                    "reason": "cache dir"
                }]
            })
            .to_string();
            let turn2 = openai_sse_body(Some(&final_json), None);
            write_mock_http_response(
                &mut s2,
                "200 OK",
                "text/event-stream",
                &turn2,
            );
        });

        let cfg = AiConfig {
            provider: AiCliProvider::Api,
            api_protocol: ApiProtocol::OpenAi,
            api_base_url: format!("http://{addr}/v1"),
            model: Some("gpt-mock".to_owned()),
            ..AiConfig::default()
        };
        let ctx =
            AiExecutionContext::default().with_credential_store(cred_store);
        let proposal = run_analysis_with_tree_for_config_with_context(
            &cfg,
            &home,
            Path::new("/bin/ruum"),
            1_700_000_888,
            Some(&tree),
            &MockCommandRunner::default(),
            &ctx,
        )
        .expect("direct API local analysis");

        assert_eq!(proposal.items.len(), 1);
        assert_eq!(proposal.items[0].path, cache_dir);
        assert_eq!(proposal.items[0].bytes, 4096);
        assert_eq!(proposal.items[0].mode, RemovalMode::Trash);

        server.join().unwrap();
    }

    #[test]
    fn direct_api_tree_analysis_runs_anthropic_in_memory_without_tools() {
        use crate::watchdog_config::ApiProtocol;
        use std::net::TcpListener;

        let mut tree = make_tree_dir(
            "deploy",
            vec![make_tree_dir(
                ".cache",
                vec![Node::entry("blob.bin", NodeKind::File, 64 << 20)],
            )],
        );
        crate::tree::aggregate(&mut tree, crate::tree::Metric::Bytes);
        crate::classify::classify(&mut tree);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (headers, body) = read_mock_http_request(&mut stream);
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("x-api-key: test-api-key"),
                "missing x-api-key header: {headers}"
            );
            let req: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert!(
                req.get("tools")
                    .and_then(|t| t.as_array())
                    .is_none_or(Vec::is_empty),
                "remote tree analysis must not mount local/SSH tools: {body}"
            );
            let payload_text = r#"{"summary":"Remote cache + build cache","items":[{"path":"/home/deploy/.cache","mode":"permanent","reason":"pip cache"}],"docker_targets":[{"kind":"build_cache"}]}"#;
            let sse = anthropic_sse_body(payload_text);
            write_mock_http_response(
                &mut stream,
                "200 OK",
                "text/event-stream",
                &sse,
            );
        });

        let cfg = AiConfig {
            provider: AiCliProvider::Api,
            api_protocol: ApiProtocol::Anthropic,
            api_base_url: format!("http://{addr}/anthropic/v1"),
            model: Some("claude-mock".to_owned()),
            ..AiConfig::default()
        };
        let docker =
            DockerProbeStatus::Available(build_sample_docker_summary());
        let ctx = AiExecutionContext::default().with_api_key("test-api-key");

        let prop = run_tree_analysis_for_config_with_context(
            &cfg,
            Path::new("/home/deploy"),
            Some(Path::new("/home/deploy")),
            &tree,
            &docker,
            TrashOs::Linux,
            1_700_000_555,
            &MockCommandRunner::default(),
            &ctx,
        )
        .expect("direct API remote tree analysis");

        assert_eq!(prop.items.len(), 1);
        assert_eq!(prop.items[0].path, PathBuf::from("/home/deploy/.cache"));
        assert_eq!(prop.items[0].bytes, 64 << 20);
        assert_eq!(prop.docker_targets, vec![DockerCleanTarget::BuildCache]);

        server.join().unwrap();
    }

    #[derive(Clone, Debug)]
    struct RecordedCloudReq {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    }

    impl RecordedCloudReq {
        fn header(&self, name: &str) -> Option<&str> {
            let target = name.to_ascii_lowercase();
            self.headers
                .iter()
                .find(|(k, _)| k == &target)
                .map(|(_, v)| v.as_str())
        }
    }

    struct CloudStubServer {
        base_url: String,
        recorded: Arc<Mutex<Vec<RecordedCloudReq>>>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl CloudStubServer {
        fn stop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }

        fn recorded(&self) -> Vec<RecordedCloudReq> {
            self.recorded.lock().unwrap().clone()
        }
    }

    impl Drop for CloudStubServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    type StubReply = (u16, Vec<(&'static str, &'static str)>, String);

    fn spawn_cloud_stub(replies: Vec<StubReply>) -> CloudStubServer {
        use std::collections::VecDeque;
        use std::fmt::Write as _;
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let recorded_bg = Arc::clone(&recorded);
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let deadline = Instant::now() + Duration::from_secs(30);

        let handle = std::thread::spawn(move || {
            while !stop_bg.load(Ordering::SeqCst) && Instant::now() < deadline {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = Vec::new();
                let mut tmp = [0_u8; 4096];
                let mut hend = None;
                while let Ok(n) = stream.read(&mut tmp) {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) =
                        buf.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        hend = Some(pos + 4);
                        break;
                    }
                }
                let Some(header_end) = hend else { continue };
                let head = String::from_utf8_lossy(&buf[..header_end]);
                let mut lines = head.split("\r\n");
                let req_line = lines.next().unwrap_or("");
                let mut parts = req_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_owned();
                let path = parts.next().unwrap_or("").to_owned();
                let mut headers = Vec::new();
                let mut clen = 0_usize;
                for line in lines {
                    if line.is_empty() {
                        continue;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        let kl = k.trim().to_ascii_lowercase();
                        let val = v.trim().to_owned();
                        if kl == "content-length" {
                            clen = val.parse::<usize>().unwrap_or(0);
                        }
                        headers.push((kl, val));
                    }
                }
                while buf.len() < header_end + clen {
                    match stream.read(&mut tmp) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let raw_body =
                    &buf[header_end..(header_end + clen).min(buf.len())];
                let body_val = serde_json::from_slice(raw_body)
                    .unwrap_or(serde_json::Value::Null);
                recorded_bg.lock().unwrap().push(RecordedCloudReq {
                    method,
                    path,
                    headers,
                    body: body_val,
                });

                let (status, extra_headers, body) = queue
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| (500, Vec::new(), "{}".to_owned()));
                let mut resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in extra_headers {
                    let _ = write!(resp, "{k}: {v}\r\n");
                }
                resp.push_str("\r\n");
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        CloudStubServer {
            base_url: format!("http://{addr}"),
            recorded,
            stop,
            handle: Some(handle),
        }
    }

    fn seed_registered_cloud_ctx(
        dir: &Path,
        base_url: &str,
    ) -> (AiExecutionContext, Arc<crate::storage::StorageDb>) {
        use crate::credentials::CREDENTIALS_FILE_NAME;
        use crate::credentials::test_helpers::MemoryRootSecretStore;
        use crate::storage::{CloudState, StorageDb};

        let db = Arc::new(StorageDb::open(dir.join("ruum.db")).unwrap());
        let root = Arc::new(MemoryRootSecretStore::new([42_u8; 32]));
        let creds =
            LazyCredentialStore::new(dir.join(CREDENTIALS_FILE_NAME), root);
        let sk = creds.load_or_create_cloud_signing_key().unwrap();
        let dev_id = crate::cloud_identity::derive_device_id(
            &crate::cloud_identity::signing_key_public_b64(&sk),
        )
        .unwrap();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 900;
        db.save_cloud_state(&CloudState {
            gateway_base_url: base_url.to_owned(),
            device_id: Some(dev_id),
            subject_id: Some("sub_test".to_owned()),
            access_token: Some("jwt-test".to_owned()),
            token_expires_at_secs: exp,
            ..CloudState::default()
        })
        .unwrap();
        let ctx = AiExecutionContext::default()
            .with_credential_store(creds)
            .with_db_path(db.db_path().to_path_buf())
            .with_cloud_base_url(base_url);
        (ctx, db)
    }

    #[test]
    fn ruum_cloud_test_connection_never_calls_completions_and_benchmark_blocked_on_trial()
     {
        let temp = tempfile::tempdir().unwrap();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 900;
        let ent_json = serde_json::json!({
            "code": "OK",
            "message": "success",
            "data": {
                "subjectId": "sub_test",
                "deviceId": "dev_test",
                "isPaid": false,
                "source": "trial",
                "decision": "ALLOW",
                "trialRemaining": 3,
                "fupZone": "green",
                "syncedAtSecs": exp - 10
            },
            "traceId": "tr-ent-1"
        })
        .to_string();
        let models_json = serde_json::json!({
            "object": "list",
            "traceId": "tr-mod-1",
            "data": [{
                "id": "ruum-fast",
                "display_name": "Ruum Fast",
                "tier": "trial",
                "available": true
            }]
        })
        .to_string();

        let mut server = spawn_cloud_stub(vec![
            (200, vec![], ent_json.clone()),
            (200, vec![], models_json),
            (200, vec![], ent_json),
        ]);
        let (ctx, _db) =
            seed_registered_cloud_ctx(temp.path(), &server.base_url);
        let cfg = AiConfig {
            setup_completed: true,
            provider: AiCliProvider::RuumCloud,
            ..AiConfig::default()
        };

        let conn_res = test_connection_for_config_with_context(
            &cfg,
            &MockCommandRunner::default(),
            &ctx,
        )
        .expect("RuumCloud test_connection should succeed");
        assert!(conn_res.contains("Ruum Cloud"));

        let bench = benchmark_model_for_config_with_context(
            &cfg,
            None,
            &MockCommandRunner::default(),
            &ctx,
        );
        assert!(!bench.ok, "benchmark must be blocked on free trial");

        server.stop();
        let paths: Vec<String> =
            server.recorded().into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            vec![
                "/v1/entitlement/status",
                "/v1/models",
                "/v1/entitlement/status"
            ]
        );
        assert!(!paths.iter().any(|p| p.contains("completions")));
    }

    #[test]
    fn ruum_cloud_blocks_completions_when_zero_models_available_and_runs_when_available()
     {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("artifact.bin"), b"123456").unwrap();

        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 900;

        // Case 1: 0 available models -> blocked client-side before /v1/chat/completions
        let ent_ok = serde_json::json!({
            "code": "OK",
            "message": "success",
            "data": {
                "subjectId": "sub_test",
                "deviceId": "dev_test",
                "isPaid": false,
                "source": "trial",
                "decision": "ALLOW",
                "trialRemaining": 2,
                "fupZone": "green",
                "syncedAtSecs": exp - 10
            },
            "traceId": "tr-e1"
        })
        .to_string();
        let no_avail_models = serde_json::json!({
            "object": "list",
            "traceId": "tr-m0",
            "data": [{
                "id": "ruum-pro",
                "display_name": "Ruum Pro",
                "tier": "paid",
                "available": false
            }]
        })
        .to_string();
        let mut srv_0 = spawn_cloud_stub(vec![
            (200, vec![], ent_ok.clone()),
            (200, vec![], no_avail_models),
        ]);
        let (ctx_0, _db_0) =
            seed_registered_cloud_ctx(temp.path(), &srv_0.base_url);
        let cfg = AiConfig {
            setup_completed: true,
            provider: AiCliProvider::RuumCloud,
            ..AiConfig::default()
        };
        let err = run_analysis_with_tree_for_config_with_context(
            &cfg,
            temp.path(),
            Path::new("/bin/ruum"),
            1_700_000_999,
            None,
            &MockCommandRunner::default(),
            &ctx_0,
        )
        .unwrap_err();
        srv_0.stop();
        assert!(!err.is_empty());
        assert!(
            !srv_0
                .recorded()
                .iter()
                .any(|r| r.path.contains("completions")),
            "must block /v1/chat/completions when 0 models available"
        );

        // Case 2: available model -> succeeds via RuumCloudLlm
        let avail_models = serde_json::json!({
            "object": "list",
            "traceId": "tr-m1",
            "data": [{
                "id": "ruum-fast",
                "display_name": "Ruum Fast",
                "tier": "trial",
                "available": true
            }]
        })
        .to_string();
        let chat_ok = serde_json::json!({
            "id": "chatcmpl-cloud-1",
            "object": "chat.completion",
            "model": "ruum-fast",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": serde_json::json!({
                        "summary": "Cloud cleanup",
                        "items": [{
                            "path": target.to_string_lossy(),
                            "mode": "permanent",
                            "reason": "build artifact"
                        }]
                    }).to_string()
                },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let temp2 = tempfile::tempdir().unwrap();
        let mut srv_1 = spawn_cloud_stub(vec![
            (200, vec![], ent_ok),
            (200, vec![], avail_models),
            (200, vec![], chat_ok),
        ]);
        let (ctx_1, _db_1) =
            seed_registered_cloud_ctx(temp2.path(), &srv_1.base_url);
        let prop = run_analysis_with_tree_for_config_with_context(
            &cfg,
            temp.path(),
            Path::new("/bin/ruum"),
            1_700_001_000,
            None,
            &MockCommandRunner::default(),
            &ctx_1,
        )
        .expect("RuumCloud analysis should succeed");
        srv_1.stop();
        assert_eq!(prop.summary, "Cloud cleanup");
        assert_eq!(prop.items.len(), 1);
        assert_eq!(prop.items[0].path, crate::removal::normalize(&target));
        assert!(
            srv_1
                .recorded()
                .iter()
                .any(|r| r.path == "/v1/chat/completions")
        );
    }

    #[test]
    fn ruum_cloud_end_to_end_401_token_refresh_replay_and_tree_analysis() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("node_modules");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("bundle.js"), b"0123456789").unwrap();

        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 900;

        let ent_ok = serde_json::json!({
            "code": "OK",
            "message": "success",
            "data": {
                "subjectId": "sub_test",
                "deviceId": "dev_test",
                "isPaid": true,
                "source": "alipay",
                "decision": "ALLOW",
                "trialRemaining": 0,
                "validUntilSecs": exp + 86400,
                "fupZone": "green",
                "syncedAtSecs": exp - 10
            },
            "traceId": "tr-ent"
        })
        .to_string();
        let models_ok = serde_json::json!({
            "object": "list",
            "traceId": "tr-mod",
            "data": [{
                "id": "ruum-pro",
                "display_name": "Ruum Pro",
                "tier": "paid",
                "available": true
            }]
        })
        .to_string();
        let err_401 = serde_json::json!({
            "code": "AUTH_TOKEN_EXPIRED",
            "message": "jwt expired",
            "traceId": "tr-401-exp",
            "error": {
                "code": "AUTH_TOKEN_EXPIRED",
                "message": "jwt expired",
                "type": "ruum_gateway_error",
                "traceId": "tr-401-exp"
            }
        })
        .to_string();
        let token_refresh_ok = serde_json::json!({
            "code": "OK",
            "message": "success",
            "data": {
                "accessToken": "jwt-refreshed-2",
                "expiresAtSecs": exp + 900,
                "deviceId": "dev_test",
                "subjectId": "sub_test"
            },
            "traceId": "tr-tok-2"
        })
        .to_string();
        let chat_tool_turn = serde_json::json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion",
            "model": "ruum-pro",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_scan_1",
                        "type": "function",
                        "function": {
                            "name": "list_volumes",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string();
        let chat_final = serde_json::json!({
            "id": "chatcmpl-final",
            "object": "chat.completion",
            "model": "ruum-pro",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": serde_json::json!({
                        "summary": "Cloud local cleanup",
                        "items": [{
                            "path": target.to_string_lossy(),
                            "mode": "permanent",
                            "reason": "rebuildable dependencies"
                        }]
                    }).to_string()
                },
                "finish_reason": "stop"
            }]
        })
        .to_string();

        let mut srv = spawn_cloud_stub(vec![
            (200, vec![], ent_ok.clone()),
            (200, vec![], models_ok.clone()),
            (401, vec![("X-Trace-ID", "tr-401-exp")], err_401),
            (200, vec![], token_refresh_ok),
            (200, vec![], chat_tool_turn),
            (200, vec![], chat_final),
        ]);
        let (ctx, _db) = seed_registered_cloud_ctx(temp.path(), &srv.base_url);
        let cfg = AiConfig {
            setup_completed: true,
            provider: AiCliProvider::RuumCloud,
            ..AiConfig::default()
        };

        let prop = run_analysis_with_tree_for_config_with_context(
            &cfg,
            temp.path(),
            Path::new("/bin/ruum"),
            1_700_002_000,
            None,
            &MockCommandRunner::default(),
            &ctx,
        )
        .expect("401 refresh + replay should succeed");
        srv.stop();

        assert_eq!(prop.summary, "Cloud local cleanup");
        assert_eq!(prop.items.len(), 1);
        assert_eq!(prop.items[0].path, crate::removal::normalize(&target));

        let reqs = srv.recorded();
        assert_eq!(reqs.len(), 6);
        assert_eq!(reqs[2].method, "POST");
        assert_eq!(reqs[2].path, "/v1/chat/completions");
        assert_eq!(reqs[3].path, "/v1/auth/token");
        assert_eq!(
            reqs[3].header("x-ruum-client-version"),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert!(reqs[3].header("x-ruum-sig").is_some());

        let s2 = reqs[2].header("x-ruum-session").unwrap();
        let s4 = reqs[4].header("x-ruum-session").unwrap();
        let s5 = reqs[5].header("x-ruum-session").unwrap();
        assert_eq!(s2, s4);
        assert_eq!(s4, s5);
        assert_eq!(reqs[4].header("x-ruum-task"), Some("local_analysis"));
        assert_eq!(reqs[4].body["stream"], serde_json::json!(false));
        let sent_tools: Vec<&str> = reqs[4].body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| {
                t.pointer("/function/name").and_then(|v| v.as_str())
            })
            .collect();
        assert!(!sent_tools.is_empty());
        for t in sent_tools {
            assert!(ruum_adk::is_allowed_ruum_tool(t), "unexpected tool {t}");
        }

        // Also test run_tree_analysis via RuumCloudAiAdapter (tree_analysis, 0 tools).
        let tree_file = temp.path().join("cache.tmp");
        fs::write(&tree_file, b"abcd").unwrap();
        let mut tree_node = Node::directory(
            temp.path().file_name().unwrap().to_string_lossy().as_ref(),
        );
        let mut child = Node::entry("cache.tmp", NodeKind::File, 4);
        child.category = Category::Cache;
        tree_node.children.push(child);
        crate::tree::aggregate(&mut tree_node, crate::tree::Metric::Bytes);

        let tree_reply = serde_json::json!({
            "id": "chatcmpl-tree",
            "object": "chat.completion",
            "model": "ruum-pro",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": serde_json::json!({
                        "summary": "Remote tree cleanup",
                        "items": [{
                            "path": tree_file.to_string_lossy(),
                            "mode": "permanent",
                            "reason": "cache"
                        }]
                    }).to_string()
                },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let temp_tree = tempfile::tempdir().unwrap();
        let mut srv_tree = spawn_cloud_stub(vec![
            (200, vec![], ent_ok),
            (200, vec![], models_ok),
            (200, vec![], tree_reply),
        ]);
        let (ctx_tree, _db_tree) =
            seed_registered_cloud_ctx(temp_tree.path(), &srv_tree.base_url);
        let tree_prop = run_tree_analysis_for_config_with_context(
            &cfg,
            temp.path(),
            Some(temp.path()),
            &tree_node,
            &DockerProbeStatus::NotInstalled,
            crate::trash_adapter::TrashAdapterFactory::current_os(),
            1_700_002_100,
            &MockCommandRunner::default(),
            &ctx_tree,
        )
        .expect("RuumCloud tree analysis should succeed");
        srv_tree.stop();
        assert_eq!(tree_prop.summary, "Remote tree cleanup");
        assert_eq!(tree_prop.items.len(), 1);
        let tree_reqs = srv_tree.recorded();
        assert_eq!(tree_reqs[2].header("x-ruum-task"), Some("tree_analysis"));
        assert!(tree_reqs[2].body.get("tools").is_none());
        assert_eq!(tree_reqs[2].body["stream"], serde_json::json!(false));
    }

    #[test]
    fn ruum_cloud_preserves_trace_id_on_402_429_and_discovers_virtual_models() {
        let temp = tempfile::tempdir().unwrap();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 900;
        let ent_ok = serde_json::json!({
            "code": "OK",
            "message": "success",
            "data": {
                "subjectId": "sub_test",
                "deviceId": "dev_test",
                "isPaid": false,
                "source": "trial",
                "decision": "ALLOW",
                "trialRemaining": 1,
                "fupZone": "green",
                "syncedAtSecs": exp - 10
            },
            "traceId": "tr-e"
        })
        .to_string();
        let models_ok = serde_json::json!({
            "object": "list",
            "traceId": "tr-m",
            "data": [{
                "id": "ruum-fast",
                "display_name": "Ruum Fast",
                "tier": "trial",
                "available": true
            }]
        })
        .to_string();
        let err_402 = serde_json::json!({
            "code": "ENTITLEMENT_PAYMENT_REQUIRED",
            "message": "trial exhausted",
            "traceId": "tr-402-pay",
            "error": {
                "code": "ENTITLEMENT_PAYMENT_REQUIRED",
                "message": "trial exhausted",
                "type": "ruum_gateway_error",
                "traceId": "tr-402-pay"
            }
        })
        .to_string();
        let err_429 = serde_json::json!({
            "code": "INFERENCE_FUP_LIMIT_EXCEEDED",
            "message": "fup reached",
            "traceId": "tr-429-fup",
            "error": {
                "code": "INFERENCE_FUP_LIMIT_EXCEEDED",
                "message": "fup reached",
                "type": "ruum_gateway_error",
                "traceId": "tr-429-fup"
            }
        })
        .to_string();

        let mut srv = spawn_cloud_stub(vec![
            (200, vec![], models_ok.clone()),
            (200, vec![], ent_ok.clone()),
            (200, vec![], models_ok.clone()),
            (402, vec![("X-Trace-ID", "tr-402-pay")], err_402),
            (200, vec![], ent_ok),
            (200, vec![], models_ok),
            (429, vec![("X-Trace-ID", "tr-429-fup")], err_429),
        ]);
        let (ctx, _db) = seed_registered_cloud_ctx(temp.path(), &srv.base_url);
        let cfg = AiConfig {
            setup_completed: true,
            provider: AiCliProvider::RuumCloud,
            ..AiConfig::default()
        };

        let (models, note) = discover_models_for_config_with_context(
            &cfg,
            &MockCommandRunner::default(),
            &ctx,
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "ruum-fast");
        assert_eq!(models[0].provider.as_deref(), Some("ruum-cloud"));
        assert!(note.is_none());

        let err1 = run_analysis_with_tree_for_config_with_context(
            &cfg,
            temp.path(),
            Path::new("/bin/ruum"),
            1_700_003_000,
            None,
            &MockCommandRunner::default(),
            &ctx,
        )
        .unwrap_err();
        assert!(err1.contains("[Trace: tr-402-pay]"), "err1={err1}");

        let err2 = run_analysis_with_tree_for_config_with_context(
            &cfg,
            temp.path(),
            Path::new("/bin/ruum"),
            1_700_003_001,
            None,
            &MockCommandRunner::default(),
            &ctx,
        )
        .unwrap_err();
        assert!(err2.contains("[Trace: tr-429-fup]"), "err2={err2}");

        srv.stop();
    }

    #[test]
    fn prepare_cloud_inference_model_precedence_and_auto() {
        let temp = tempfile::tempdir().unwrap();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 900;
        let ent_ok = serde_json::json!({
            "code": "OK",
            "message": "success",
            "data": {
                "subjectId": "sub_test",
                "deviceId": "dev_test",
                "isPaid": true,
                "source": "alipay",
                "decision": "ALLOW",
                "trialRemaining": 0,
                "validUntilSecs": exp + 86400,
                "fupZone": "green",
                "syncedAtSecs": exp - 10
            },
            "traceId": "tr-ent-pref"
        })
        .to_string();
        let models_ok = serde_json::json!({
            "object": "list",
            "traceId": "tr-mod-pref",
            "data": [
                {
                    "id": "ruum-fast",
                    "display_name": "Ruum Fast",
                    "tier": "trial",
                    "available": true
                },
                {
                    "id": "ruum-pro",
                    "display_name": "Ruum Pro",
                    "tier": "paid",
                    "available": true
                },
                {
                    "id": "ruum-locked",
                    "display_name": "Ruum Locked",
                    "tier": "paid",
                    "available": false
                }
            ]
        })
        .to_string();

        let mut srv = spawn_cloud_stub(vec![
            (200, vec![], ent_ok.clone()),
            (200, vec![], models_ok.clone()),
            (200, vec![], ent_ok.clone()),
            (200, vec![], models_ok.clone()),
            (200, vec![], ent_ok.clone()),
            (200, vec![], models_ok.clone()),
            (200, vec![], ent_ok.clone()),
            (200, vec![], models_ok.clone()),
            (200, vec![], ent_ok.clone()),
            (200, vec![], models_ok.clone()),
            (200, vec![], ent_ok),
            (200, vec![], models_ok),
        ]);
        let (ctx, db) = seed_registered_cloud_ctx(temp.path(), &srv.base_url);
        let client = build_cloud_client_from_context(&ctx).unwrap();

        // 1. Auto (None everywhere) resolves to "auto" when models exist.
        let auto_cfg = AiConfig {
            setup_completed: true,
            provider: AiCliProvider::RuumCloud,
            model: None,
            ..AiConfig::default()
        };
        let (_, chosen_auto) =
            prepare_cloud_inference_model(&client, &auto_cfg, None).unwrap();
        assert_eq!(chosen_auto, CLOUD_AUTO_MODEL_ID);
        assert_eq!(chosen_auto, "auto");
        assert_eq!(db.load_cloud_state().unwrap().selected_model_id, None);

        // 2. Fallback to cloud_state.selected_model_id when config.model is None.
        let mut st = db.load_cloud_state().unwrap();
        st.selected_model_id = Some("ruum-pro".to_owned());
        db.save_cloud_state(&st).unwrap();
        let (_, chosen_cached) =
            prepare_cloud_inference_model(&client, &auto_cfg, None).unwrap();
        assert_eq!(chosen_cached, "ruum-pro");

        // 3. config.model takes precedence over cloud_state.selected_model_id.
        let explicit_cfg = AiConfig {
            model: Some("ruum-fast".to_owned()),
            ..auto_cfg
        };
        let (_, chosen_cfg) =
            prepare_cloud_inference_model(&client, &explicit_cfg, None)
                .unwrap();
        assert_eq!(chosen_cfg, "ruum-fast");

        // 4. model_override takes precedence over config.model and cloud_state.
        let (_, chosen_override) = prepare_cloud_inference_model(
            &client,
            &explicit_cfg,
            Some("ruum-pro"),
        )
        .unwrap();
        assert_eq!(chosen_override, "ruum-pro");

        // 5. Stale/unknown model preference falls back to "auto".
        let (_, chosen_stale) = prepare_cloud_inference_model(
            &client,
            &explicit_cfg,
            Some("stale-removed-model"),
        )
        .unwrap();
        assert_eq!(chosen_stale, CLOUD_AUTO_MODEL_ID);

        // 6. Unavailable model preference falls back to "auto".
        let (_, chosen_unavailable) = prepare_cloud_inference_model(
            &client,
            &explicit_cfg,
            Some("ruum-locked"),
        )
        .unwrap();
        assert_eq!(chosen_unavailable, CLOUD_AUTO_MODEL_ID);

        srv.stop();
    }
}
