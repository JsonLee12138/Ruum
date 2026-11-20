//! Windows cross-compilation `clippy` check via `cargo-xwin`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const DEFAULT_TARGET: &str = "x86_64-pc-windows-msvc";
pub const HOMEBREW_LLVM_BIN_DIRS: [&str; 2] =
    ["/opt/homebrew/opt/llvm/bin", "/usr/local/opt/llvm/bin"];

const USAGE: &str = "\
usage: cargo xtask cross-lint [--target <triple>]
";

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub target: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            target: DEFAULT_TARGET.to_owned(),
        }
    }
}

/// Parses CLI flags for `cargo xtask cross-lint`.
pub fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    let mut opts = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => {
                let val = iter
                    .next()
                    .map(String::as_str)
                    .filter(|v| !v.is_empty())
                    .ok_or("--target requires a non-empty triple")?;
                val.clone_into(&mut opts.target);
            }
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown cross-lint option `{other}`"));
            }
            positional => {
                return Err(format!(
                    "unexpected positional argument `{positional}`"
                ));
            }
        }
    }
    Ok(Some(opts))
}

/// Returns `true` when `target` appears in `rustup target list --installed`.
pub fn target_installed(rustup_output: &str, target: &str) -> bool {
    rustup_output.lines().any(|line| line.trim() == target)
}

fn tool_filename(name: &str, is_windows: bool) -> String {
    let has_exe = Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
    if is_windows && !has_exe {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn has_tool_in_path(
    path_env: Option<&OsStr>,
    tool: &str,
    is_windows: bool,
) -> bool {
    let Some(raw) = path_env else {
        return false;
    };
    let file = tool_filename(tool, is_windows);
    std::env::split_paths(raw).any(|dir| dir.join(&file).is_file())
}

/// Returns `true` when both `clang-cl` and `llvm-lib` exist in `path_env`.
pub fn tools_discoverable(path_env: Option<&OsStr>, is_windows: bool) -> bool {
    has_tool_in_path(path_env, "clang-cl", is_windows)
        && has_tool_in_path(path_env, "llvm-lib", is_windows)
}

/// Prepends existing LLVM candidate directories when `clang-cl` or `llvm-lib`
/// are not already discoverable on `current_path`.
pub fn augmented_path_with(
    current_path: Option<&OsStr>,
    candidate_dirs: &[&Path],
    is_windows: bool,
) -> Option<OsString> {
    if tools_discoverable(current_path, is_windows) {
        return None;
    }
    let current_dirs: Vec<PathBuf> = current_path
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    let mut combined: Vec<PathBuf> = candidate_dirs
        .iter()
        .copied()
        .filter(|dir| {
            dir.is_dir() && !current_dirs.iter().any(|cur| cur == dir)
        })
        .map(Path::to_path_buf)
        .collect();
    if combined.is_empty() {
        return None;
    }
    combined.extend(current_dirs);
    std::env::join_paths(combined).ok()
}

/// Returns the effective `PATH` including Homebrew LLVM fallback if needed.
pub fn effective_path(current_path: Option<&OsStr>) -> Option<OsString> {
    let candidates: Vec<&Path> =
        HOMEBREW_LLVM_BIN_DIRS.iter().map(Path::new).collect();
    augmented_path_with(current_path, &candidates, cfg!(windows))
        .or_else(|| current_path.map(OsStr::to_os_string))
}

fn has_cargo_xwin() -> bool {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    Command::new(cargo)
        .args(["xwin", "--version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn rustup_installed_targets() -> Result<String, String> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("could not run `rustup`: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "`rustup target list --installed` failed: {}",
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Returns `true` when `cargo-xwin`, LLVM tools, and `target` are available.
pub fn prerequisites_available(target: &str) -> bool {
    if !has_cargo_xwin() {
        return false;
    }
    let current_path = std::env::var_os("PATH");
    let eff = effective_path(current_path.as_deref());
    if !tools_discoverable(eff.as_deref(), cfg!(windows)) {
        return false;
    }
    rustup_installed_targets().is_ok_and(|out| target_installed(&out, target))
}

/// Returns `true` when default Windows cross-lint prerequisites are available.
pub fn is_available() -> bool {
    prerequisites_available(DEFAULT_TARGET)
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Runs `cargo xwin clippy` across the workspace for `target`.
pub fn run_target(target: &str) -> Result<(), String> {
    if !has_cargo_xwin() {
        return Err(
            "cargo-xwin not found; install with: cargo install cargo-xwin"
                .into(),
        );
    }

    let current_path = std::env::var_os("PATH");
    let candidates: Vec<&Path> =
        HOMEBREW_LLVM_BIN_DIRS.iter().map(Path::new).collect();
    let augmented = augmented_path_with(
        current_path.as_deref(),
        &candidates,
        cfg!(windows),
    );
    let effective = augmented.as_deref().or(current_path.as_deref());
    if !tools_discoverable(effective, cfg!(windows)) {
        return Err(
            "LLVM tools (`clang-cl`, `llvm-lib`) not found on PATH or in \
             Homebrew LLVM (/opt/homebrew/opt/llvm/bin, \
             /usr/local/opt/llvm/bin); install on macOS with: brew install llvm"
                .into(),
        );
    }

    let installed = rustup_installed_targets()?;
    if !target_installed(&installed, target) {
        return Err(format!(
            "rustup target `{target}` is not installed; install with: \
             rustup target add {target}"
        ));
    }

    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let args = [
        "xwin",
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--target",
        target,
        "--",
        "-D",
        "warnings",
    ];
    let display = format!("{cargo} {}", args.join(" "));
    let mut cmd = Command::new(cargo);
    cmd.current_dir(workspace_root())
        .args(args)
        .stdin(Stdio::null());
    if let Some(path_val) = augmented {
        cmd.env("PATH", path_val);
    }
    let status = cmd
        .status()
        .map_err(|err| format!("could not start `{display}`: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{display}` failed"))
    }
}

/// Entry point for `cargo xtask cross-lint`.
pub fn run(args: &[String]) -> Result<(), String> {
    let Some(opts) = parse_args(args)? else {
        return Ok(());
    };
    run_target(&opts.target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_default_custom_target_and_help_flags() {
        assert_eq!(
            parse_args(&[]).unwrap(),
            Some(Options {
                target: DEFAULT_TARGET.to_owned(),
            })
        );
        assert_eq!(
            parse_args(&["--target".into(), "aarch64-pc-windows-msvc".into()])
                .unwrap(),
            Some(Options {
                target: "aarch64-pc-windows-msvc".to_owned(),
            })
        );
        assert_eq!(parse_args(&["--help".into()]).unwrap(), None);
        assert!(parse_args(&["--target".into()]).is_err());
        assert!(parse_args(&["--unknown".into()]).is_err());
        assert!(parse_args(&["positional".into()]).is_err());
    }

    #[test]
    fn detects_installed_rustup_target_lines() {
        let output = "\
aarch64-apple-darwin
x86_64-unknown-linux-musl
x86_64-pc-windows-msvc
";
        assert!(target_installed(output, "x86_64-pc-windows-msvc"));
        assert!(!target_installed(output, "aarch64-pc-windows-msvc"));
        assert!(!target_installed("", "x86_64-pc-windows-msvc"));
    }

    #[test]
    fn augments_path_only_when_llvm_tools_are_missing() {
        let tmp = TempDir::new().unwrap();
        let existing_path_dir = tmp.path().join("bin");
        let llvm_dir = tmp.path().join("llvm/bin");
        let missing_dir = tmp.path().join("does-not-exist");
        fs::create_dir_all(&existing_path_dir).unwrap();
        fs::create_dir_all(&llvm_dir).unwrap();

        let base_path = std::env::join_paths([&existing_path_dir]).unwrap();
        let augmented = augmented_path_with(
            Some(base_path.as_os_str()),
            &[llvm_dir.as_path(), missing_dir.as_path()],
            false,
        )
        .expect("should prepend existing candidate dir");
        let parts: Vec<PathBuf> = std::env::split_paths(&augmented).collect();
        assert_eq!(parts, vec![llvm_dir.clone(), existing_path_dir.clone()]);

        // Once both clang-cl and llvm-lib exist on PATH, no augmentation happens.
        fs::write(existing_path_dir.join("clang-cl"), "").unwrap();
        fs::write(existing_path_dir.join("llvm-lib"), "").unwrap();
        assert!(tools_discoverable(Some(base_path.as_os_str()), false));
        assert_eq!(
            augmented_path_with(
                Some(base_path.as_os_str()),
                &[llvm_dir.as_path()],
                false,
            ),
            None
        );

        // Windows checks .exe suffixes too.
        assert!(!tools_discoverable(Some(base_path.as_os_str()), true));
        fs::write(existing_path_dir.join("clang-cl.exe"), "").unwrap();
        fs::write(existing_path_dir.join("llvm-lib.exe"), "").unwrap();
        assert!(tools_discoverable(Some(base_path.as_os_str()), true));
    }
}
