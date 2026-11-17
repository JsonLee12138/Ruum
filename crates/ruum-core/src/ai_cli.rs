//! AI CLI assistant integration, prompt generation and proposal validation.
//!
//! Provides command building for AI CLI assistants (`Pi`, `Agy`, `Codex`, and
//! `Custom`), enriched `PATH` resolution, strict read-only prompt generation
//! pointing at `<current_exe> mcp --read-only`, and parsing of JSON proposals
//! verified through [`crate::removal::plan`] and [`crate::git::state`].

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::insights::{self, Finding};
use crate::removal::RemovalMode;
use crate::size::human_bytes;
use crate::storage::AiConfig;
use crate::tree::{Node, path_of};
use crate::watchdog_config::{
    AiCliProvider, CleanupProposal, ProposalItem, WatchdogConfig,
};

fn ai_config_from_legacy(config: &WatchdogConfig) -> AiConfig {
    AiConfig {
        auto_analyze_on_alert: config.ai_auto_analyze,
        provider: config.ai_provider,
        custom_command: config.custom_command.clone(),
        extra_args: config.extra_args.clone(),
        model: config.model.clone(),
        updated_at_secs: 0,
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

/// Run an external command with piped output, non-interactive defaults, and timeout.
fn run_command_with_timeout(
    program: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
) -> io::Result<CommandOutput> {
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

/// Generate the strict read-only diagnostic prompt including pre-scanned items.
#[must_use]
pub fn build_analysis_prompt_with_candidates(
    home_dir: &Path,
    current_exe: &Path,
    candidates: &[ReclaimCandidateSummary],
) -> String {
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
    };
    resolve_cli_executable(name)
        .ok_or_else(|| format!("executable `{name}` could not be found"))
}

/// Run a short non-destructive connection test against an [`AiConfig`].
pub fn test_connection_for_config(
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
    }
}

/// Discover selectable models for the configured AI CLI provider.
pub fn discover_models(
    config: &WatchdogConfig,
    runner: &dyn CommandRunner,
) -> Result<(Vec<DiscoveredModel>, Option<String>), String> {
    discover_models_for_config(&ai_config_from_legacy(config), runner)
}

/// Probe CLI connectivity and fetch available models for an [`AiConfig`].
pub fn test_connection_and_discover_for_config(
    config: &AiConfig,
    runner: &dyn CommandRunner,
) -> Result<ConnectionTestResult, String> {
    let version_message = test_connection_for_config(config, runner)?;
    match discover_models_for_config(config, runner) {
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
    if !obj.contains_key("items") && !obj.contains_key("summary") {
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
}

#[derive(serde::Deserialize)]
struct RawItem {
    path: String,
    mode: Option<String>,
    reason: Option<String>,
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
}
