//! Release changelog extraction and version bump preparation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const NOTES_USAGE: &str = "\
usage: cargo xtask release-notes --version <V> [--changelog CHANGELOG.md] [--out <path>]
";

const PREP_USAGE: &str = "\
usage: cargo xtask release-prep <version> [--changelog CHANGELOG.md] [--dry-run]
";

const TAG_USAGE: &str = "\
usage: cargo xtask release-tag <version> [--remote <name>] [--push] [--skip-ci-check]
";

#[derive(Debug, PartialEq, Eq)]
pub struct ReleaseTagOptions {
    pub version: String,
    pub remote: String,
    pub push: bool,
    pub skip_ci_check: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhCiRun {
    pub database_id: u64,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    pub url: String,
    #[serde(default)]
    pub event: String,
}

/// Extracts the Markdown release notes body for `version` from `changelog`.
pub fn extract_release_notes(
    changelog: &str,
    version: &str,
) -> Result<String, String> {
    let target = normalize_version_tag(version)?;
    let mut collecting = false;
    let mut lines: Vec<&str> = Vec::new();

    for line in changelog.lines() {
        if let Some(heading_ver) = parse_section_heading_version(line) {
            if collecting {
                break;
            }
            if heading_ver == target {
                collecting = true;
            }
            continue;
        }
        if collecting {
            if is_link_reference_line(line) {
                break;
            }
            lines.push(line);
        }
    }

    if !collecting {
        return Err(format!("version `{target}` not found in changelog"));
    }

    let start = lines.iter().position(|l| !l.trim().is_empty());
    let end = lines.iter().rposition(|l| !l.trim().is_empty());
    let (Some(start), Some(end)) = (start, end) else {
        return Err(format!("release notes for `{target}` are empty"));
    };

    Ok(lines[start..=end].join("\n"))
}

/// Updates `[workspace.package] version = "..."` in `Cargo.toml` text.
pub fn update_workspace_version(
    cargo_toml: &str,
    new_version: &str,
) -> Result<String, String> {
    let parsed = semver::Version::parse(new_version.trim())
        .map_err(|err| format!("invalid SemVer `{new_version}`: {err}"))?;
    let normalized = parsed.to_string();

    let mut in_workspace_pkg = false;
    let mut updated = false;
    let mut out_lines: Vec<String> = Vec::new();

    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_workspace_pkg = trimmed == "[workspace.package]";
            out_lines.push(line.to_owned());
            continue;
        }
        if in_workspace_pkg
            && !updated
            && let Some((key, _)) = trimmed.split_once('=')
            && key.trim() == "version"
        {
            out_lines.push(format!("version = \"{normalized}\""));
            updated = true;
            continue;
        }
        out_lines.push(line.to_owned());
    }

    if !updated {
        return Err(
            "could not find `version = \"...\"` under `[workspace.package]`"
                .into(),
        );
    }

    let mut result = out_lines.join("\n");
    if cargo_toml.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

/// Inserts a new release section at the top of `CHANGELOG.md` text if missing.
pub fn prepend_changelog_draft(
    changelog: &str,
    version: &str,
    date_ymd: &str,
    body: &str,
) -> Result<String, String> {
    let target = normalize_version_tag(version)?;
    for line in changelog.lines() {
        if parse_section_heading_version(line) == Some(target) {
            return Err(format!(
                "version `{target}` already exists in changelog"
            ));
        }
    }

    let section = format!("## [{target}] - {date_ymd}\n\n{}\n", body.trim());

    let mut lines: Vec<&str> = changelog.lines().collect();
    let insert_idx = lines
        .iter()
        .position(|line| parse_section_heading_version(line).is_some())
        .unwrap_or(lines.len());

    let before = lines.drain(..insert_idx).collect::<Vec<_>>().join("\n");
    let after = lines.join("\n");

    let mut out = String::new();
    if before.trim().is_empty() {
        out.push_str("# Changelog\n\n");
    } else {
        out.push_str(before.trim_end());
        out.push_str("\n\n");
    }
    out.push_str(&section);
    if !after.trim().is_empty() {
        out.push('\n');
        out.push_str(after.trim_start_matches('\n'));
        out.push('\n');
    }
    Ok(out)
}

/// Converts UNIX epoch seconds to `(year, month, day)` in UTC.
pub const fn unix_secs_to_ymd(secs: u64) -> (i64, u32, u32) {
    let days = (secs / 86_400).cast_signed();
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m as u32, d as u32)
}

/// Converts UNIX epoch seconds to an RFC 3339 UTC timestamp string.
pub fn unix_secs_to_rfc3339(secs: u64) -> String {
    let (year, month, day) = unix_secs_to_ymd(secs);
    let day_secs = secs % 86_400;
    let hour = day_secs / 3_600;
    let minute = (day_secs % 3_600) / 60;
    let second = day_secs % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Returns current UNIX epoch seconds.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Entry point for `cargo xtask release-notes`.
pub fn release_notes(args: &[String]) -> Result<(), String> {
    if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        println!("{NOTES_USAGE}");
        return Ok(());
    }

    let mut version: Option<String> = None;
    let mut changelog = PathBuf::from("CHANGELOG.md");
    let mut out: Option<PathBuf> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--version" => {
                version = Some(next_arg(&mut iter, "--version")?.to_owned());
            }
            "--changelog" => {
                changelog = PathBuf::from(next_arg(&mut iter, "--changelog")?);
            }
            "--out" => out = Some(PathBuf::from(next_arg(&mut iter, "--out")?)),
            other => {
                return Err(format!("unknown release-notes option `{other}`"));
            }
        }
    }

    let version = version.ok_or("--version is required")?;
    let changelog_path = resolve_default_path(&changelog, "CHANGELOG.md");
    let raw = fs::read_to_string(&changelog_path).map_err(|err| {
        format!("cannot read {}: {err}", changelog_path.display())
    })?;
    let notes = extract_release_notes(&raw, &version)?;

    if let Some(out_path) = out {
        if let Some(parent) = out_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|err| {
                format!("cannot create {}: {err}", parent.display())
            })?;
        }
        fs::write(&out_path, format!("{notes}\n")).map_err(|err| {
            format!("cannot write {}: {err}", out_path.display())
        })?;
        println!("{}", out_path.display());
    } else {
        println!("{notes}");
    }
    Ok(())
}

/// Entry point for `cargo xtask release-prep`.
pub fn release_prep(args: &[String]) -> Result<(), String> {
    if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        println!("{PREP_USAGE}");
        return Ok(());
    }

    let mut version: Option<String> = None;
    let mut changelog = workspace_root().join("CHANGELOG.md");
    let mut cargo_toml = workspace_root().join("Cargo.toml");
    let mut dry_run = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--changelog" => {
                changelog = PathBuf::from(next_arg(&mut iter, "--changelog")?);
            }
            "--cargo-toml" => {
                cargo_toml =
                    PathBuf::from(next_arg(&mut iter, "--cargo-toml")?);
            }
            "--dry-run" => dry_run = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown release-prep option `{other}`"));
            }
            positional => {
                if version.is_some() {
                    return Err(format!(
                        "unexpected extra positional argument `{positional}`"
                    ));
                }
                version = Some(positional.to_owned());
            }
        }
    }

    let raw_version = version.ok_or("<version> is required")?;
    let parsed = semver::Version::parse(raw_version.trim())
        .map_err(|err| format!("invalid SemVer `{raw_version}`: {err}"))?;
    let version_str = parsed.to_string();

    let cargo_raw = fs::read_to_string(&cargo_toml).map_err(|err| {
        format!("cannot read {}: {err}", cargo_toml.display())
    })?;
    let updated_cargo = update_workspace_version(&cargo_raw, &version_str)?;

    let existing_changelog = if changelog.exists() {
        fs::read_to_string(&changelog).map_err(|err| {
            format!("cannot read {}: {err}", changelog.display())
        })?
    } else {
        "# Changelog\n\nAll notable changes to Ruum are documented in this file.\n"
            .to_owned()
    };

    let already_exists = existing_changelog.lines().any(|l| {
        parse_section_heading_version(l) == Some(version_str.as_str())
    });

    let (year, month, day) = unix_secs_to_ymd(now_unix_secs());
    let date_ymd = format!("{year:04}-{month:02}-{day:02}");

    if dry_run {
        println!(
            "dry-run: would set [workspace.package] version = \"{version_str}\" in {}",
            cargo_toml.display()
        );
        if already_exists {
            println!(
                "dry-run: {} already contains [{version_str}]",
                changelog.display()
            );
        } else {
            println!(
                "dry-run: would prepend [{version_str}] - {date_ymd} to {}",
                changelog.display()
            );
        }
        return Ok(());
    }

    fs::write(&cargo_toml, updated_cargo).map_err(|err| {
        format!("cannot write {}: {err}", cargo_toml.display())
    })?;

    if !already_exists {
        let used_cliff = try_git_cliff_prepend(&changelog, &version_str);
        if !used_cliff {
            let draft_body =
                format!("### Changes\n\n- Prepare release `{version_str}`.");
            let updated_changelog = prepend_changelog_draft(
                &existing_changelog,
                &version_str,
                &date_ymd,
                &draft_body,
            )?;
            fs::write(&changelog, updated_changelog).map_err(|err| {
                format!("cannot write {}: {err}", changelog.display())
            })?;
        }
    }

    println!("prepared release {version_str}");
    Ok(())
}

/// Parses CLI options for `cargo xtask release-tag`.
pub fn parse_release_tag_args(
    args: &[String],
) -> Result<Option<ReleaseTagOptions>, String> {
    let mut version: Option<String> = None;
    let mut remote = "origin".to_owned();
    let mut push = false;
    let mut skip_ci_check = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--remote" => {
                next_arg(&mut iter, "--remote")?.clone_into(&mut remote);
            }
            "--push" => push = true,
            "--skip-ci-check" => skip_ci_check = true,
            "-h" | "--help" | "help" => {
                println!("{TAG_USAGE}");
                return Ok(None);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown release-tag option `{other}`"));
            }
            positional => {
                if version.is_some() {
                    return Err(format!(
                        "unexpected extra positional argument `{positional}`"
                    ));
                }
                version = Some(positional.to_owned());
            }
        }
    }

    let raw_version = version.ok_or("<version> is required")?;
    let parsed = semver::Version::parse(raw_version.trim())
        .map_err(|err| format!("invalid SemVer `{raw_version}`: {err}"))?;
    Ok(Some(ReleaseTagOptions {
        version: parsed.to_string(),
        remote,
        push,
        skip_ci_check,
    }))
}

/// Reads `[workspace.package].version` from `Cargo.toml` text.
pub fn read_workspace_version(cargo_toml: &str) -> Result<String, String> {
    let mut in_workspace_pkg = false;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_workspace_pkg = trimmed == "[workspace.package]";
            continue;
        }
        if !in_workspace_pkg {
            continue;
        }
        let Some((key, rhs)) = trimmed.split_once('=') else {
            continue;
        };
        if key.trim() != "version" {
            continue;
        }
        let val = rhs.trim();
        if let Some(rest) = val.strip_prefix('"')
            && let Some((ver, _)) = rest.split_once('"')
        {
            return Ok(ver.to_owned());
        }
        if let Some(rest) = val.strip_prefix('\'')
            && let Some((ver, _)) = rest.split_once('\'')
        {
            return Ok(ver.to_owned());
        }
    }
    Err("could not find `version = \"...\"` under `[workspace.package]`".into())
}

/// Verifies that `[workspace.package].version` matches `expected_version`.
pub fn verify_workspace_version(
    cargo_toml: &str,
    expected_version: &str,
) -> Result<(), String> {
    let actual = read_workspace_version(cargo_toml)?;
    if actual != expected_version {
        return Err(format!(
            "Cargo.toml [workspace.package].version is `{actual}`, \
             expected `{expected_version}`"
        ));
    }
    Ok(())
}

fn is_release_only_path(raw: &str) -> bool {
    let normalized = raw.replace('\\', "/");
    let mut path = normalized.as_str();
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped;
    }
    if path.is_empty() {
        return false;
    }
    matches!(
        path,
        "Cargo.toml"
            | "Cargo.lock"
            | "cliff.toml"
            | "LICENSE"
            | ".gitignore"
            | ".editorconfig"
    ) || Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        || path.starts_with("docs/")
        || path.starts_with(".agents/")
        || path.starts_with(".artifacts/")
}

/// Returns `true` when every changed path is a release/doc/metadata file.
pub fn is_release_only_paths(paths: &[String]) -> bool {
    !paths.is_empty() && paths.iter().all(|p| is_release_only_path(p))
}

/// Selects a passing CI workflow run from `gh run list --json ...` output.
pub fn select_ci_run_verdict(
    runs_json: &str,
    commit_sha: &str,
) -> Result<GhCiRun, String> {
    let runs: Vec<GhCiRun> =
        serde_json::from_str(runs_json).map_err(|err| {
            format!("could not parse `gh run list` JSON output: {err}")
        })?;
    if runs.is_empty() {
        return Err(format!(
            "no CI workflow runs found for commit `{commit_sha}`; push to \
             remote and wait for CI, or pass --skip-ci-check"
        ));
    }

    let is_completed_success = |run: &&GhCiRun| {
        run.status.eq_ignore_ascii_case("completed")
            && run
                .conclusion
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case("success"))
    };

    if let Some(preferred) = runs.iter().find(|r| {
        is_completed_success(r)
            && matches!(r.event.as_str(), "push" | "workflow_dispatch")
    }) {
        return Ok(preferred.clone());
    }

    if let Some(any_success) = runs.iter().find(is_completed_success) {
        return Ok(any_success.clone());
    }

    if let Some(active) = runs
        .iter()
        .find(|r| !r.status.eq_ignore_ascii_case("completed"))
    {
        return Err(format!(
            "CI run {} for commit `{commit_sha}` has status `{}` ({}); wait \
             for completion before tagging",
            active.database_id, active.status, active.url
        ));
    }

    let failed = &runs[0];
    let conclusion = failed.conclusion.as_deref().unwrap_or("unknown");
    Err(format!(
        "CI run {} for commit `{commit_sha}` did not succeed (status: `{}`, \
         conclusion: `{conclusion}`): {}",
        failed.database_id, failed.status, failed.url
    ))
}

fn git_output(repo_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| {
            format!("could not run `git {}`: {err}", args.join(" "))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "`git {}` failed: {}",
            args.join(" "),
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_tag_exists(repo_root: &Path, tag_name: &str) -> Result<bool, String> {
    let ref_name = format!("refs/tags/{tag_name}");
    let status = Command::new("git")
        .current_dir(repo_root)
        .args(["rev-parse", "-q", "--verify", &ref_name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("could not run `git rev-parse`: {err}"))?;
    Ok(status.success())
}

fn resolve_ci_target_commit(repo_root: &Path) -> Result<String, String> {
    let list = git_output(
        repo_root,
        &["rev-list", "--first-parent", "-n", "11", "HEAD"],
    )?;
    for sha in list.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let changed = git_output(
            repo_root,
            &[
                "diff-tree",
                "--first-parent",
                "--no-commit-id",
                "--name-only",
                "-r",
                "--root",
                sha,
            ],
        )?;
        let paths: Vec<String> = changed
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !is_release_only_paths(&paths) {
            return Ok(sha.to_owned());
        }
    }
    Err(
        "could not find a code commit within 10 parents of HEAD to verify CI"
            .into(),
    )
}

fn check_gh_ci_for_commit(
    repo_root: &Path,
    commit_sha: &str,
) -> Result<GhCiRun, String> {
    let output = Command::new("gh")
        .current_dir(repo_root)
        .args([
            "run",
            "list",
            "--commit",
            commit_sha,
            "--workflow",
            "CI",
            "--limit",
            "10",
            "--json",
            "databaseId,status,conclusion,url,event",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| {
            format!(
                "could not run `gh run list` (install GitHub CLI or pass \
                 --skip-ci-check): {err}"
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`gh run list` failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    select_ci_run_verdict(&stdout, commit_sha)
}

fn run_cargo_check_locked(repo_root: &Path) -> Result<(), String> {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let args = [
        "check",
        "--locked",
        "--workspace",
        "--all-targets",
        "--all-features",
    ];
    let display = format!("{cargo} {}", args.join(" "));
    let status = Command::new(cargo)
        .current_dir(repo_root)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .map_err(|err| format!("could not start `{display}`: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{display}` failed"))
    }
}

/// Entry point for `cargo xtask release-tag`.
pub fn release_tag(args: &[String]) -> Result<(), String> {
    let Some(opts) = parse_release_tag_args(args)? else {
        return Ok(());
    };
    let repo_root = workspace_root();
    let tag_name = format!("v{}", opts.version);

    if git_tag_exists(&repo_root, &tag_name)? {
        return Err(format!("tag `{tag_name}` already exists locally"));
    }

    let porcelain = git_output(&repo_root, &["status", "--porcelain"])?;
    if !porcelain.trim().is_empty() {
        return Err(
            "working tree is not clean; commit or stash changes before tagging"
                .into(),
        );
    }

    let cargo_path = repo_root.join("Cargo.toml");
    let cargo_raw = fs::read_to_string(&cargo_path).map_err(|err| {
        format!("cannot read {}: {err}", cargo_path.display())
    })?;
    verify_workspace_version(&cargo_raw, &opts.version)?;

    let changelog_path = repo_root.join("CHANGELOG.md");
    let changelog_raw = fs::read_to_string(&changelog_path).map_err(|err| {
        format!("cannot read {}: {err}", changelog_path.display())
    })?;
    extract_release_notes(&changelog_raw, &opts.version)?;

    run_cargo_check_locked(&repo_root)?;
    crate::lint()?;
    if crate::cross_lint::is_available() {
        crate::cross_lint::run_target(crate::cross_lint::DEFAULT_TARGET)?;
    } else {
        eprintln!(
            "xtask release-tag: skipping cross-lint (cargo-xwin, LLVM, or \
             Windows target not available)"
        );
    }

    if !opts.skip_ci_check {
        let sha = resolve_ci_target_commit(&repo_root)?;
        let ci_run = check_gh_ci_for_commit(&repo_root, &sha)?;
        println!(
            "verified CI run {} ({}, {}) for commit {sha}",
            ci_run.database_id, ci_run.event, ci_run.url
        );
    }

    let branch = if opts.push {
        let raw =
            git_output(&repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let name = raw.trim().to_owned();
        if name.is_empty() || name == "HEAD" {
            return Err(
                "cannot push from detached HEAD; check out a branch first"
                    .into(),
            );
        }
        Some(name)
    } else {
        None
    };

    git_output(&repo_root, &["tag", &tag_name])?;
    println!("created tag {tag_name}");

    if let Some(branch_name) = branch {
        git_output(&repo_root, &["push", &opts.remote, &branch_name])?;
        let tag_ref = format!("refs/tags/{tag_name}");
        git_output(&repo_root, &["push", &opts.remote, &tag_ref])?;
        println!("pushed {branch_name} and {tag_name} to {}", opts.remote);
    }

    Ok(())
}

fn try_git_cliff_prepend(changelog: &Path, version: &str) -> bool {
    let repo_root = workspace_root();
    try_git_cliff_prepend_with("git-cliff", &repo_root, changelog, version)
}

fn try_git_cliff_prepend_with(
    bin: &str,
    repo_root: &Path,
    changelog_path: &Path,
    version: &str,
) -> bool {
    let tag = format!("v{version}");
    let mut cmd = Command::new(bin);
    cmd.current_dir(repo_root);
    let config_path = repo_root.join("cliff.toml");
    if config_path.is_file() {
        cmd.arg("--config").arg(config_path);
    }
    let output = cmd
        .arg("--unreleased")
        .arg("--tag")
        .arg(&tag)
        .arg("--prepend")
        .arg(changelog_path)
        .stdin(Stdio::null())
        .output();
    let Ok(out) = output else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    fs::read_to_string(changelog_path)
        .is_ok_and(|updated| extract_release_notes(&updated, version).is_ok())
}

fn normalize_version_tag(raw: &str) -> Result<&str, String> {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix('v').unwrap_or(trimmed);
    if stripped.is_empty() || stripped.chars().any(char::is_whitespace) {
        return Err(format!("invalid version `{raw}`"));
    }
    Ok(stripped)
}

fn parse_section_heading_version(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## ")?.trim_start();
    let token = if let Some(after_open) = rest.strip_prefix('[') {
        after_open.split(']').next()?
    } else {
        rest.split_whitespace().next()?
    };
    let trimmed = token.trim();
    let clean = trimmed.strip_prefix('v').unwrap_or(trimmed);
    if clean.is_empty() || clean.eq_ignore_ascii_case("unreleased") {
        None
    } else {
        Some(clean)
    }
}

fn is_link_reference_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('[') && trimmed.contains("]: ")
}

fn next_arg<'a>(
    iter: &mut std::slice::Iter<'a, String>,
    flag: &str,
) -> Result<&'a str, String> {
    iter.next()
        .map(String::as_str)
        .filter(|val| !val.is_empty())
        .ok_or_else(|| format!("{flag} requires a non-empty value"))
}

fn resolve_default_path(path: &Path, default_name: &str) -> PathBuf {
    if path == Path::new(default_name) && !path.exists() {
        workspace_root().join(default_name)
    } else {
        path.to_path_buf()
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const SAMPLE_CHANGELOG: &str = "\
# Changelog

All notable changes to Ruum are documented in this file.

## [0.2.2] - 2026-11-20

### Fixes

- **core,watchdog:** Retry SQLite reads and writes on `SQLITE_BUSY`.

## [0.2.2-beta.1] - 2026-11-19

### Fixes

- **core,remote:** Wrap SSH commands in single-line `/bin/sh -c`.

## [v0.2.1] - 2026-11-19

### Features

- **app,core:** Add Docker usage views.
";

    #[test]
    fn extracts_release_notes_for_stable_and_beta_versions() {
        let stable = extract_release_notes(SAMPLE_CHANGELOG, "0.2.2").unwrap();
        assert!(stable.contains("### Fixes"));
        assert!(stable.contains("SQLITE_BUSY"));
        assert!(!stable.contains("0.2.2-beta.1"));
        assert!(!stable.contains("## [0.2.2]"));

        let beta =
            extract_release_notes(SAMPLE_CHANGELOG, "v0.2.2-beta.1").unwrap();
        assert!(beta.contains("Wrap SSH commands"));
        assert!(!beta.contains("0.2.1"));

        let prefixed =
            extract_release_notes(SAMPLE_CHANGELOG, "0.2.1").unwrap();
        assert!(prefixed.contains("Docker usage views"));

        assert!(extract_release_notes(SAMPLE_CHANGELOG, "9.9.9").is_err());
    }

    #[test]
    fn updates_only_workspace_package_version_in_cargo_toml() {
        let sample = "\
[workspace]
members = [\"xtask\"]

[workspace.package]
version = \"0.2.2\"
edition = \"2024\"

[workspace.dependencies]
anyhow = \"1\"
";
        let updated = update_workspace_version(sample, "0.3.0-beta.1").unwrap();
        assert!(updated.contains("version = \"0.3.0-beta.1\""));
        assert!(!updated.contains("version = \"0.2.2\""));
        assert!(update_workspace_version(sample, "not-semver").is_err());
        assert!(update_workspace_version("[workspace]\n", "0.3.0").is_err());
    }

    #[test]
    fn prepends_changelog_draft_without_duplicating_existing_version() {
        let updated = prepend_changelog_draft(
            SAMPLE_CHANGELOG,
            "0.2.3",
            "2026-11-21",
            "### Features\n\n- Add self-update.",
        )
        .unwrap();
        let idx_new = updated.find("## [0.2.3] - 2026-11-21").unwrap();
        let idx_old = updated.find("## [0.2.2] - 2026-11-20").unwrap();
        assert!(idx_new < idx_old);

        let dup = prepend_changelog_draft(
            SAMPLE_CHANGELOG,
            "0.2.2",
            "2026-11-20",
            "- duplicate",
        );
        assert!(dup.is_err());
    }

    #[test]
    fn formats_unix_timestamp_as_utc_rfc3339() {
        assert_eq!(unix_secs_to_ymd(0), (1970, 1, 1));
        assert_eq!(unix_secs_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_secs_to_rfc3339(1_795_144_879), "2026-11-20T03:21:19Z");
    }

    #[test]
    fn release_notes_cli_writes_out_file_and_errors_on_missing_version() {
        let tmp = TempDir::new().unwrap();
        let changelog_path = tmp.path().join("CHANGELOG.md");
        let out_path = tmp.path().join("notes.md");
        fs::write(&changelog_path, SAMPLE_CHANGELOG).unwrap();

        let args = vec![
            "--version".into(),
            "0.2.2-beta.1".into(),
            "--changelog".into(),
            changelog_path.display().to_string(),
            "--out".into(),
            out_path.display().to_string(),
        ];
        release_notes(&args).unwrap();
        let written = fs::read_to_string(&out_path).unwrap();
        assert!(written.contains("Wrap SSH commands"));

        let missing_args = vec![
            "--version".into(),
            "0.9.9".into(),
            "--changelog".into(),
            changelog_path.display().to_string(),
        ];
        assert!(release_notes(&missing_args).is_err());
    }

    #[test]
    fn release_prep_respects_dry_run_and_updates_files() {
        let tmp = TempDir::new().unwrap();
        let cargo_path = tmp.path().join("Cargo.toml");
        let changelog_path = tmp.path().join("CHANGELOG.md");
        let orig_cargo = "[workspace.package]\nversion = \"0.2.2\"\n";
        fs::write(&cargo_path, orig_cargo).unwrap();
        fs::write(&changelog_path, SAMPLE_CHANGELOG).unwrap();

        let dry_args = vec![
            "0.2.3-beta.1".into(),
            "--cargo-toml".into(),
            cargo_path.display().to_string(),
            "--changelog".into(),
            changelog_path.display().to_string(),
            "--dry-run".into(),
        ];
        release_prep(&dry_args).unwrap();
        assert_eq!(fs::read_to_string(&cargo_path).unwrap(), orig_cargo);
        assert_eq!(
            fs::read_to_string(&changelog_path).unwrap(),
            SAMPLE_CHANGELOG
        );

        let real_args = vec![
            "0.2.3-beta.1".into(),
            "--cargo-toml".into(),
            cargo_path.display().to_string(),
            "--changelog".into(),
            changelog_path.display().to_string(),
        ];
        release_prep(&real_args).unwrap();
        assert!(
            fs::read_to_string(&cargo_path)
                .unwrap()
                .contains("version = \"0.2.3-beta.1\"")
        );
        let updated_log = fs::read_to_string(&changelog_path).unwrap();
        assert!(updated_log.contains("## [0.2.3-beta.1] - "));
    }

    #[cfg(unix)]
    #[test]
    fn git_cliff_prepend_falls_back_when_exit_zero_writes_no_section() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        let cliff_toml = repo_root.join("cliff.toml");
        fs::write(&cliff_toml, "# mock config\n").unwrap();

        let changelog_path = repo_root.join("CHANGELOG.md");
        fs::write(&changelog_path, SAMPLE_CHANGELOG).unwrap();

        let noop_bin = tmp.path().join("fake-git-cliff-noop");
        fs::write(&noop_bin, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&noop_bin, fs::Permissions::from_mode(0o755))
            .unwrap();

        assert!(!try_git_cliff_prepend_with(
            noop_bin.to_str().unwrap(),
            &repo_root,
            &changelog_path,
            "0.2.3",
        ));

        let fallback = prepend_changelog_draft(
            &fs::read_to_string(&changelog_path).unwrap(),
            "0.2.3",
            "2026-11-21",
            "### Changes\n\n- Prepare release `0.2.3`.",
        )
        .unwrap();
        assert!(extract_release_notes(&fallback, "0.2.3").is_ok());

        let args_log = tmp.path().join("cliff-args.txt");
        let writer_bin = tmp.path().join("fake-git-cliff-writer");
        let writer_script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" > {log}\n\
             cat > {cl} <<'EOF'\n\
             # Changelog\n\n\
             ## [0.2.4] - 2026-11-21\n\n\
             ### Features\n\n\
             - Generated by git-cliff.\n\
             EOF\n",
            log = args_log.display(),
            cl = changelog_path.display()
        );
        fs::write(&writer_bin, writer_script).unwrap();
        fs::set_permissions(&writer_bin, fs::Permissions::from_mode(0o755))
            .unwrap();

        assert!(try_git_cliff_prepend_with(
            writer_bin.to_str().unwrap(),
            &repo_root,
            &changelog_path,
            "0.2.4",
        ));
        let recorded_args = fs::read_to_string(&args_log).unwrap();
        assert!(recorded_args.contains("--config"));
        assert!(recorded_args.contains(&cliff_toml.display().to_string()));
    }

    #[test]
    fn parses_release_tag_cli_options() {
        assert_eq!(
            parse_release_tag_args(&["0.2.3-beta.3".into()]).unwrap(),
            Some(ReleaseTagOptions {
                version: "0.2.3-beta.3".into(),
                remote: "origin".into(),
                push: false,
                skip_ci_check: false,
            })
        );
        assert_eq!(
            parse_release_tag_args(&[
                "0.2.3".into(),
                "--remote".into(),
                "upstream".into(),
                "--push".into(),
                "--skip-ci-check".into(),
            ])
            .unwrap(),
            Some(ReleaseTagOptions {
                version: "0.2.3".into(),
                remote: "upstream".into(),
                push: true,
                skip_ci_check: true,
            })
        );
        assert_eq!(parse_release_tag_args(&["--help".into()]).unwrap(), None);
        assert!(parse_release_tag_args(&[]).is_err());
        assert!(parse_release_tag_args(&["not-semver".into()]).is_err());
        assert!(
            parse_release_tag_args(&["0.2.3".into(), "extra".into()]).is_err()
        );
        assert!(
            parse_release_tag_args(&["0.2.3".into(), "--bogus".into()])
                .is_err()
        );
    }

    #[test]
    fn verifies_workspace_package_version_only() {
        let sample = "\
[workspace]
members = [\"xtask\"]

[workspace.package]
version = \"0.2.3-beta.3\" # release version
edition = \"2024\"

[package]
version = \"9.9.9\"
";
        assert_eq!(read_workspace_version(sample).unwrap(), "0.2.3-beta.3");
        verify_workspace_version(sample, "0.2.3-beta.3").unwrap();
        assert!(verify_workspace_version(sample, "0.2.3").is_err());
        assert!(read_workspace_version("[workspace]\n").is_err());
    }

    #[test]
    fn classifies_release_only_paths_matching_ci_rules() {
        assert!(!is_release_only_paths(&[]));
        assert!(is_release_only_paths(&[
            "Cargo.toml".into(),
            "Cargo.lock".into(),
            "CHANGELOG.md".into(),
            "README.md".into(),
            "docs/guide.txt".into(),
            ".agents/rules/ci-and-release.md".into(),
            ".artifacts/PLAN.md".into(),
        ]));
        assert!(is_release_only_paths(&[
            "./CHANGELOG.md".into(),
            "docs\\notes.txt".into(),
        ]));
        assert!(!is_release_only_paths(&[
            "Cargo.toml".into(),
            "crates/ruum-core/src/updater.rs".into(),
        ]));
        assert!(!is_release_only_paths(&[
            "CHANGELOG.md".into(),
            "xtask/src/release.rs".into(),
        ]));
    }

    #[test]
    fn selects_ci_run_verdict_from_gh_json() {
        let sha = "abcdef1234567";

        let empty_err = select_ci_run_verdict("[]", sha).unwrap_err();
        assert!(empty_err.contains(sha));

        let in_progress = r#"[
            {
                "databaseId": 101,
                "status": "in_progress",
                "conclusion": null,
                "url": "https://github.com/org/repo/actions/runs/101",
                "event": "push"
            }
        ]"#;
        let ip_err = select_ci_run_verdict(in_progress, sha).unwrap_err();
        assert!(ip_err.contains("in_progress"));
        assert!(ip_err.contains("runs/101"));

        let failed = r#"[
            {
                "databaseId": 102,
                "status": "completed",
                "conclusion": "failure",
                "url": "https://github.com/org/repo/actions/runs/102",
                "event": "push"
            }
        ]"#;
        let fail_err = select_ci_run_verdict(failed, sha).unwrap_err();
        assert!(fail_err.contains("failure"));
        assert!(fail_err.contains("runs/102"));

        let mixed_success = r#"[
            {
                "databaseId": 103,
                "status": "completed",
                "conclusion": "success",
                "url": "https://github.com/org/repo/actions/runs/103",
                "event": "pull_request"
            },
            {
                "databaseId": 104,
                "status": "completed",
                "conclusion": "success",
                "url": "https://github.com/org/repo/actions/runs/104",
                "event": "push"
            }
        ]"#;
        let chosen = select_ci_run_verdict(mixed_success, sha).unwrap();
        assert_eq!(chosen.database_id, 104);
        assert_eq!(chosen.event, "push");

        let pr_only = r#"[
            {
                "databaseId": 105,
                "status": "completed",
                "conclusion": "success",
                "url": "https://github.com/org/repo/actions/runs/105",
                "event": "pull_request"
            }
        ]"#;
        let pr_chosen = select_ci_run_verdict(pr_only, sha).unwrap();
        assert_eq!(pr_chosen.database_id, 105);
    }
}
