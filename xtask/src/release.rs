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
}
