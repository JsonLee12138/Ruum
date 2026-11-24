//! Check and packaging entry points for ruum.
//!
//! Each gate is reachable on its own (`cargo xtask fmt`) and through the
//! aggregate CI runs (`cargo xtask lint`), mirroring how omatrack exposes every
//! linter both as a build target and as a labelled test. `lint` never fixes
//! anything: it reports the diff and fails, so a red local run is the same
//! signal CI gives.

mod bundle;
mod cross_lint;
mod package;
mod portable;
mod release;
mod scanner_pack;

use std::process::{Command, ExitCode, Stdio};

const USAGE: &str = "\
usage: cargo xtask <task>

tasks:
  lint      fmt --check, then clippy over the workspace with -D warnings
  cross-lint [--target <triple>]
            Windows MSVC clippy cross-check via cargo-xwin (default: x86_64-pc-windows-msvc)
  fmt       rustfmt --check over the workspace
  fmt-fix   rustfmt in place
  clippy    clippy --workspace --all-targets -- -D warnings
  test      cargo test --workspace
  ci        lint, then test
  portable  Linux: desktop executable with static x86_64/ARM64 scanners
  scanner-pack [--out <path>] [--strict]
            cross-compile static Linux scanners into ruum-scan-linux.pack
  bundle    macOS: target/bundle/ruum.app and its zip
              --target <triple>          cross-compile (aarch64-apple-darwin | x86_64-apple-darwin)
              --scanner-pack <path>      embed prebuilt ruum-scan-linux.pack
              --require-scanner-pack     fail if Linux musl targets are missing
              --sign IDENTITY            sign app with Developer ID (default: ad hoc)
              --installer-sign IDENTITY  sign .pkg with Developer ID Installer
              --dmg                      build .dmg disk image
              --pkg                      build .pkg installer package
              --all                      build .zip, .dmg, and .pkg
              --notarize                 notarize and staple; needs NOTARY_PROFILE, or
                                         NOTARY_KEY, NOTARY_KEY_ID and NOTARY_ISSUER
  package   cross-platform staging, Homebrew cask rendering, and checksums
              linux-stage --arch <x86_64|aarch64> --out <dir>
              windows-stage --arch <x86_64|aarch64> --out <dir>
              homebrew-cask --version <V> --arm64-sha256 <H1> --x64-sha256 <H2> --repo <owner/repo> --out <path> [--signed]
              checksums --dir <dist_dir>
              manifest --version <V> --tag <T> --channel <stable|beta> --repo <owner/repo> --dir <dist> --notes-file <path> --out <path> [--sign-key-env <ENV>] [--published-at <iso8601>]
  release-notes --version <V> [--changelog CHANGELOG.md] [--out <path>]
  release-prep  <version> [--changelog CHANGELOG.md] [--dry-run]
  release-tag   <version> [--remote <name>] [--push] [--skip-ci-check]
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let task = args.first().map_or("lint", String::as_str);

    let outcome = match task {
        "lint" => lint(),
        "cross-lint" => cross_lint::run(&args[1..]),
        "fmt" => fmt(false),
        "fmt-fix" => fmt(true),
        "clippy" => clippy(),
        "test" => test(),
        "ci" => lint().and_then(|()| test()),
        "bundle" => bundle::bundle(&args[1..]),
        "package" => package::package(&args[1..]),
        "portable" => portable::build(),
        "release-notes" => release::release_notes(&args[1..]),
        "release-prep" => release::release_prep(&args[1..]),
        "release-tag" => release::release_tag(&args[1..]),
        "scanner-pack" => scanner_pack::run(&args[1..]),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("unknown task: {other}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask {task}: {message}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn lint() -> Result<(), String> {
    fmt(false).and_then(|()| clippy())
}

fn fmt(fix: bool) -> Result<(), String> {
    let mut args = vec!["fmt", "--all"];
    args.push(if fix { "" } else { "--check" });
    let args: Vec<&str> = args.into_iter().filter(|a| !a.is_empty()).collect();
    run(&args)
}

fn clippy() -> Result<(), String> {
    run(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ])?;
    run(&["check", "-p", "ruum-core", "--no-default-features"])
}

fn test() -> Result<(), String> {
    run(&["test", "--workspace"])
}

fn run(args: &[&str]) -> Result<(), String> {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let display = format!("{cargo} {}", args.join(" "));
    let status = Command::new(cargo)
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
