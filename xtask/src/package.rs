//! Cross-platform packaging staging, Homebrew cask rendering, and SHA-256
//! checksum generation.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufReader, Read as _};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest as _, Sha256};

const DESKTOP_TEMPLATE: &str = include_str!("../../packaging/ruum.desktop.in");
const NFPM_TEMPLATE: &str = include_str!("../../packaging/linux/nfpm.yaml.in");
const APPRUN_SCRIPT: &str = include_str!("../../packaging/linux/AppRun");
const APPDATA_XML: &str =
    include_str!("../../packaging/linux/io.github.tobi.ruum.appdata.xml");
const WXS_TEMPLATE: &str = include_str!("../../packaging/windows/ruum.wxs");
const ISS_TEMPLATE: &str = include_str!("../../packaging/windows/ruum.iss");
const HOMEBREW_TEMPLATE: &str =
    include_str!("../../packaging/homebrew/ruum.rb.in");

const USAGE: &str = "\
usage: cargo xtask package <subcommand> [options]

subcommands:
  linux-stage    --arch <x86_64|aarch64> --out <dir> [--binary <path>] [--gui-binary <path>] [--version <V>]
  windows-stage  --arch <x86_64|aarch64> --out <dir> [--exe <path>] [--gui-exe <path>] [--scanner-pack <path>] [--version <V>]
  homebrew-cask  --version <V> --arm64-sha256 <H1> --x64-sha256 <H2> --repo <owner/repo> --out <path> [--signed]
  checksums      --dir <dist_dir>
  manifest       --version <V> --tag <T> --channel <stable|beta> --repo <owner/repo> --dir <dist> --notes-file <path> --out <path> [--sign-key-env <ENV>] [--published-at <iso8601>]
";

/// Target release architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "x86_64" => Ok(Self::X86_64),
            "aarch64" => Ok(Self::Aarch64),
            other => Err(format!(
                "unsupported architecture `{other}` (expected x86_64 or aarch64)"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    pub const fn nfpm_arch(self) -> &'static str {
        match self {
            Self::X86_64 => "amd64",
            Self::Aarch64 => "arm64",
        }
    }

    pub const fn wix_arch(self) -> &'static str {
        match self {
            Self::X86_64 => "x64",
            Self::Aarch64 => "arm64",
        }
    }

    pub const fn inno_arch(self) -> &'static str {
        match self {
            Self::X86_64 => "x64compatible",
            Self::Aarch64 => "arm64",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxStageOptions {
    pub arch: Arch,
    pub out: PathBuf,
    pub binary: Option<PathBuf>,
    pub gui_binary: Option<PathBuf>,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsStageOptions {
    pub arch: Arch,
    pub out: PathBuf,
    pub exe: Option<PathBuf>,
    pub gui_exe: Option<PathBuf>,
    pub scanner_pack: Option<PathBuf>,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomebrewCaskOptions {
    pub version: String,
    pub arm64_sha256: String,
    pub x64_sha256: String,
    pub repo: String,
    pub out: PathBuf,
    pub signed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            other => Err(format!(
                "unsupported channel `{other}` (expected stable or beta)"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestOptions {
    pub version: String,
    pub tag: String,
    pub channel: Channel,
    pub repo: String,
    pub dir: PathBuf,
    pub notes_file: PathBuf,
    pub out: PathBuf,
    pub sign_key_env: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestAsset {
    pub filename: String,
    pub path: String,
    pub fallback_url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub version: String,
    pub tag: String,
    pub channel: String,
    pub published_at: String,
    pub release_url: String,
    pub notes: String,
    pub assets: BTreeMap<String, ManifestAsset>,
}

/// Signs raw manifest UTF-8 bytes with a base64-encoded 32-byte Ed25519 seed.
pub fn sign_manifest_bytes(
    bytes: &[u8],
    seed_b64: &str,
) -> Result<String, String> {
    let decoded = BASE64
        .decode(seed_b64.trim())
        .map_err(|err| format!("invalid base64 signing key: {err}"))?;
    let seed: [u8; 32] = decoded.try_into().map_err(|v: Vec<u8>| {
        format!(
            "Ed25519 signing seed must be 32 bytes, got {} bytes",
            v.len()
        )
    })?;
    let signing_key = SigningKey::from_bytes(&seed);
    let sig = signing_key.sign(bytes);
    Ok(BASE64.encode(sig.to_bytes()))
}

/// Verifies raw manifest UTF-8 bytes against a base64 signature and public key.
#[cfg(test)]
pub fn verify_manifest_bytes(
    bytes: &[u8],
    sig_b64: &str,
    pub_b64: &str,
) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
    let pub_decoded = BASE64
        .decode(pub_b64.trim())
        .map_err(|err| format!("invalid base64 public key: {err}"))?;
    let pub_bytes: [u8; 32] =
        pub_decoded.try_into().map_err(|v: Vec<u8>| {
            format!(
                "Ed25519 public key must be 32 bytes, got {} bytes",
                v.len()
            )
        })?;
    let verifying_key = VerifyingKey::from_bytes(&pub_bytes)
        .map_err(|err| format!("invalid Ed25519 public key: {err}"))?;

    let sig_decoded = BASE64
        .decode(sig_b64.trim())
        .map_err(|err| format!("invalid base64 signature: {err}"))?;
    let sig_bytes: [u8; 64] =
        sig_decoded.try_into().map_err(|v: Vec<u8>| {
            format!("Ed25519 signature must be 64 bytes, got {} bytes", v.len())
        })?;
    let signature = Signature::from_bytes(&sig_bytes);

    verifying_key
        .verify(bytes, &signature)
        .map_err(|err| format!("Ed25519 signature verification failed: {err}"))
}

/// Builds `manifest.json`, writes it to `opts.out`, and optionally signs it
/// using the environment variable specified in `opts.sign_key_env`.
pub fn build_and_write_manifest(
    opts: &ManifestOptions,
) -> Result<ReleaseManifest, String> {
    let env_seed = match &opts.sign_key_env {
        Some(name) => std::env::var(name)
            .ok()
            .map(|val| val.trim().to_owned())
            .filter(|val| !val.is_empty()),
        None => None,
    };
    build_manifest_with_signing_key(opts, env_seed.as_deref())
}

/// Builds and writes `manifest.json`, writing `<out>.sig` when `signing_seed`
/// is `Some`.
pub fn build_manifest_with_signing_key(
    opts: &ManifestOptions,
    signing_seed: Option<&str>,
) -> Result<ReleaseManifest, String> {
    let parsed_ver = semver::Version::parse(opts.version.trim())
        .map_err(|err| format!("invalid SemVer `{}`: {err}", opts.version))?;
    let version = parsed_ver.to_string();

    let tag = opts.tag.trim();
    if tag.is_empty()
        || tag.chars().any(char::is_whitespace)
        || tag.contains("..")
        || tag.contains('/')
        || tag.contains('\\')
    {
        return Err(format!("invalid release tag `{}`", opts.tag));
    }

    let repo = validate_repo(&opts.repo)?;
    let notes_raw = fs::read_to_string(&opts.notes_file).map_err(|err| {
        format!("cannot read {}: {err}", opts.notes_file.display())
    })?;
    let notes = notes_raw.trim().to_owned();

    let sums_path = opts.dir.join("SHA256SUMS");
    let recorded_sums = if sums_path.is_file() {
        parse_sha256sums(&sums_path)?
    } else {
        HashMap::new()
    };

    let mut assets = BTreeMap::new();
    for (key, filename) in known_asset_specs(&version) {
        let asset_path = opts.dir.join(&filename);
        if !asset_path.is_file() {
            continue;
        }
        let meta = fs::metadata(&asset_path).map_err(|err| {
            format!("cannot stat {}: {err}", asset_path.display())
        })?;
        let sha256 = sha256_file(&asset_path)?;
        if let Some(expected) = recorded_sums.get(&filename)
            && expected != &sha256
        {
            return Err(format!(
                "SHA256SUMS mismatch for {filename}: expected {expected}, computed {sha256}"
            ));
        }
        assets.insert(
            key.to_owned(),
            ManifestAsset {
                path: format!("releases/{tag}/{filename}"),
                fallback_url: format!(
                    "https://github.com/{repo}/releases/download/{tag}/{filename}"
                ),
                filename,
                sha256,
                size: meta.len(),
            },
        );
    }

    if assets.is_empty() {
        return Err(format!(
            "no known release assets for version `{version}` found in {}",
            opts.dir.display()
        ));
    }

    let published_at = opts
        .published_at
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || {
                crate::release::unix_secs_to_rfc3339(
                    crate::release::now_unix_secs(),
                )
            },
            ToOwned::to_owned,
        );

    let manifest = ReleaseManifest {
        schema_version: 1,
        version,
        tag: tag.to_owned(),
        channel: opts.channel.as_str().to_owned(),
        published_at,
        release_url: format!("https://github.com/{repo}/releases/tag/{tag}"),
        notes,
        assets,
    };

    let mut json = serde_json::to_string_pretty(&manifest)
        .map_err(|err| format!("failed to serialize manifest: {err}"))?;
    json.push('\n');

    if let Some(parent) = opts.out.parent()
        && !parent.as_os_str().is_empty()
    {
        create_dir_all(parent)?;
    }
    write_file(&opts.out, &json)?;

    if let Some(seed_b64) = signing_seed.filter(|s| !s.trim().is_empty()) {
        let sig_b64 = sign_manifest_bytes(json.as_bytes(), seed_b64)?;
        let mut sig_os = opts.out.as_os_str().to_os_string();
        sig_os.push(".sig");
        let sig_path = PathBuf::from(sig_os);
        write_file(&sig_path, &sig_b64)?;
    }

    Ok(manifest)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Subcommand {
    LinuxStage(LinuxStageOptions),
    WindowsStage(WindowsStageOptions),
    HomebrewCask(HomebrewCaskOptions),
    Checksums { dir: PathBuf },
    Manifest(ManifestOptions),
}

pub fn package(args: &[String]) -> Result<(), String> {
    if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        println!("{USAGE}");
        return Ok(());
    }
    match parse_args(args)? {
        Subcommand::LinuxStage(opts) => stage_linux(&opts),
        Subcommand::WindowsStage(opts) => stage_windows(&opts),
        Subcommand::HomebrewCask(opts) => {
            let rendered = render_homebrew_cask(&opts)?;
            if let Some(parent) = opts.out.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent).map_err(|err| {
                    format!("cannot create {}: {err}", parent.display())
                })?;
            }
            write_file(&opts.out, &rendered)?;
            println!("{}", opts.out.display());
            Ok(())
        }
        Subcommand::Checksums { dir } => {
            let entries = generate_checksums(&dir)?;
            for (name, hex) in entries {
                println!("{hex}  {name}");
            }
            Ok(())
        }
        Subcommand::Manifest(opts) => {
            build_and_write_manifest(&opts)?;
            println!("{}", opts.out.display());
            Ok(())
        }
    }
}

fn parse_args(args: &[String]) -> Result<Subcommand, String> {
    let Some((sub, rest)) = args.split_first() else {
        return Err(format!("missing package subcommand\n\n{USAGE}"));
    };
    match sub.as_str() {
        "linux-stage" => parse_linux_stage(rest).map(Subcommand::LinuxStage),
        "windows-stage" => {
            parse_windows_stage(rest).map(Subcommand::WindowsStage)
        }
        "homebrew-cask" => {
            parse_homebrew_cask(rest).map(Subcommand::HomebrewCask)
        }
        "checksums" => {
            parse_checksums(rest).map(|dir| Subcommand::Checksums { dir })
        }
        "manifest" => parse_manifest(rest).map(Subcommand::Manifest),
        other => {
            Err(format!("unknown package subcommand `{other}`\n\n{USAGE}"))
        }
    }
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

fn parse_linux_stage(args: &[String]) -> Result<LinuxStageOptions, String> {
    let mut arch = None;
    let mut out = None;
    let mut binary = None;
    let mut gui_binary = None;
    let mut version = env!("CARGO_PKG_VERSION").to_string();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--arch" => {
                arch = Some(Arch::parse(next_arg(&mut iter, "--arch")?)?);
            }
            "--out" => out = Some(PathBuf::from(next_arg(&mut iter, "--out")?)),
            "--binary" => {
                binary = Some(PathBuf::from(next_arg(&mut iter, "--binary")?));
            }
            "--gui-binary" => {
                gui_binary =
                    Some(PathBuf::from(next_arg(&mut iter, "--gui-binary")?));
            }
            "--version" => {
                version = next_arg(&mut iter, "--version")?.to_string();
            }
            other => {
                return Err(format!("unknown linux-stage option `{other}`"));
            }
        }
    }
    Ok(LinuxStageOptions {
        arch: arch.ok_or("--arch is required")?,
        out: out.ok_or("--out is required")?,
        binary,
        gui_binary,
        version,
    })
}

fn parse_windows_stage(args: &[String]) -> Result<WindowsStageOptions, String> {
    let mut arch = None;
    let mut out = None;
    let mut exe = None;
    let mut gui_exe = None;
    let mut scanner_pack = None;
    let mut version = env!("CARGO_PKG_VERSION").to_string();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--arch" => {
                arch = Some(Arch::parse(next_arg(&mut iter, "--arch")?)?);
            }
            "--out" => out = Some(PathBuf::from(next_arg(&mut iter, "--out")?)),
            "--exe" => exe = Some(PathBuf::from(next_arg(&mut iter, "--exe")?)),
            "--gui-exe" => {
                gui_exe =
                    Some(PathBuf::from(next_arg(&mut iter, "--gui-exe")?));
            }
            "--scanner-pack" => {
                scanner_pack =
                    Some(PathBuf::from(next_arg(&mut iter, "--scanner-pack")?));
            }
            "--version" => {
                version = next_arg(&mut iter, "--version")?.to_string();
            }
            other => {
                return Err(format!("unknown windows-stage option `{other}`"));
            }
        }
    }
    Ok(WindowsStageOptions {
        arch: arch.ok_or("--arch is required")?,
        out: out.ok_or("--out is required")?,
        exe,
        gui_exe,
        scanner_pack,
        version,
    })
}

fn parse_homebrew_cask(args: &[String]) -> Result<HomebrewCaskOptions, String> {
    let mut version = None;
    let mut arm64_sha256 = None;
    let mut x64_sha256 = None;
    let mut repo = None;
    let mut out = None;
    let mut signed = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--version" => {
                version = Some(next_arg(&mut iter, "--version")?.to_string());
            }
            "--arm64-sha256" => {
                arm64_sha256 =
                    Some(next_arg(&mut iter, "--arm64-sha256")?.to_string());
            }
            "--x64-sha256" => {
                x64_sha256 =
                    Some(next_arg(&mut iter, "--x64-sha256")?.to_string());
            }
            "--repo" => {
                repo = Some(next_arg(&mut iter, "--repo")?.to_string());
            }
            "--out" => out = Some(PathBuf::from(next_arg(&mut iter, "--out")?)),
            "--signed" => signed = true,
            other => {
                return Err(format!("unknown homebrew-cask option `{other}`"));
            }
        }
    }
    Ok(HomebrewCaskOptions {
        version: version.ok_or("--version is required")?,
        arm64_sha256: arm64_sha256.ok_or("--arm64-sha256 is required")?,
        x64_sha256: x64_sha256.ok_or("--x64-sha256 is required")?,
        repo: repo.ok_or("--repo is required")?,
        out: out.ok_or("--out is required")?,
        signed,
    })
}

fn parse_checksums(args: &[String]) -> Result<PathBuf, String> {
    let mut dir = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dir" => dir = Some(PathBuf::from(next_arg(&mut iter, "--dir")?)),
            other => return Err(format!("unknown checksums option `{other}`")),
        }
    }
    dir.ok_or_else(|| "--dir is required".into())
}

fn parse_manifest(args: &[String]) -> Result<ManifestOptions, String> {
    let mut version = None;
    let mut tag = None;
    let mut channel = None;
    let mut repo = None;
    let mut dir = None;
    let mut notes_file = None;
    let mut out = None;
    let mut sign_key_env = None;
    let mut published_at = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--version" => {
                version = Some(next_arg(&mut iter, "--version")?.to_owned());
            }
            "--tag" => tag = Some(next_arg(&mut iter, "--tag")?.to_owned()),
            "--channel" => {
                channel =
                    Some(Channel::parse(next_arg(&mut iter, "--channel")?)?);
            }
            "--repo" => repo = Some(next_arg(&mut iter, "--repo")?.to_owned()),
            "--dir" => dir = Some(PathBuf::from(next_arg(&mut iter, "--dir")?)),
            "--notes-file" => {
                notes_file =
                    Some(PathBuf::from(next_arg(&mut iter, "--notes-file")?));
            }
            "--out" => out = Some(PathBuf::from(next_arg(&mut iter, "--out")?)),
            "--sign-key-env" => {
                sign_key_env =
                    Some(next_arg(&mut iter, "--sign-key-env")?.to_owned());
            }
            "--published-at" => {
                published_at =
                    Some(next_arg(&mut iter, "--published-at")?.to_owned());
            }
            other => return Err(format!("unknown manifest option `{other}`")),
        }
    }
    Ok(ManifestOptions {
        version: version.ok_or("--version is required")?,
        tag: tag.ok_or("--tag is required")?,
        channel: channel.ok_or("--channel is required")?,
        repo: repo.ok_or("--repo is required")?,
        dir: dir.ok_or("--dir is required")?,
        notes_file: notes_file.ok_or("--notes-file is required")?,
        out: out.ok_or("--out is required")?,
        sign_key_env,
        published_at,
    })
}

fn validate_repo(raw: &str) -> Result<&str, String> {
    let repo = raw.trim();
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(format!("--repo `{repo}` must look like owner/repo"));
    };
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || repo.chars().any(char::is_whitespace)
    {
        return Err(format!("--repo `{repo}` must look like owner/repo"));
    }
    Ok(repo)
}

fn known_asset_specs(version: &str) -> [(&'static str, String); 20] {
    [
        (
            "macos-aarch64-app_zip",
            format!("ruum-{version}-aarch64-macos.zip"),
        ),
        (
            "macos-aarch64-dmg",
            format!("ruum-{version}-aarch64-macos.dmg"),
        ),
        (
            "macos-aarch64-pkg",
            format!("ruum-{version}-aarch64-macos.pkg"),
        ),
        (
            "macos-x86_64-app_zip",
            format!("ruum-{version}-x86_64-macos.zip"),
        ),
        (
            "macos-x86_64-dmg",
            format!("ruum-{version}-x86_64-macos.dmg"),
        ),
        (
            "macos-x86_64-pkg",
            format!("ruum-{version}-x86_64-macos.pkg"),
        ),
        (
            "windows-x86_64-inno",
            format!("ruum-{version}-x86_64-windows-setup.exe"),
        ),
        (
            "windows-x86_64-msi",
            format!("ruum-{version}-x86_64-windows.msi"),
        ),
        (
            "windows-x86_64-portable_zip",
            format!("ruum-{version}-x86_64-windows.zip"),
        ),
        (
            "windows-aarch64-inno",
            format!("ruum-{version}-aarch64-windows-setup.exe"),
        ),
        (
            "windows-aarch64-msi",
            format!("ruum-{version}-aarch64-windows.msi"),
        ),
        (
            "windows-aarch64-portable_zip",
            format!("ruum-{version}-aarch64-windows.zip"),
        ),
        (
            "linux-x86_64-appimage",
            format!("ruum-{version}-x86_64.AppImage"),
        ),
        (
            "linux-x86_64-tarball",
            format!("ruum-{version}-x86_64-linux.tar.gz"),
        ),
        (
            "linux-x86_64-deb",
            format!("ruum-{version}-x86_64-linux.deb"),
        ),
        (
            "linux-x86_64-rpm",
            format!("ruum-{version}-x86_64-linux.rpm"),
        ),
        (
            "linux-aarch64-appimage",
            format!("ruum-{version}-aarch64.AppImage"),
        ),
        (
            "linux-aarch64-tarball",
            format!("ruum-{version}-aarch64-linux.tar.gz"),
        ),
        (
            "linux-aarch64-deb",
            format!("ruum-{version}-aarch64-linux.deb"),
        ),
        (
            "linux-aarch64-rpm",
            format!("ruum-{version}-aarch64-linux.rpm"),
        ),
    ]
}

fn parse_sha256sums(path: &Path) -> Result<HashMap<String, String>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let mut map = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(hex) = parts.next() else {
            continue;
        };
        let Some(rest) = parts.next() else {
            continue;
        };
        let name = rest.trim_start();
        if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            map.insert(name.to_owned(), hex.to_ascii_lowercase());
        }
    }
    Ok(map)
}

/// Normalizes a `SemVer` version string into an MSI `Major.Minor.Patch` string.
///
/// Windows Installer requires numeric `Major.Minor.Build` with `Major <= 255`,
/// `Minor <= 255`, and `Build <= 65535` (no prerelease or build metadata).
pub fn normalize_msi_version(version: &str) -> Result<String, String> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return Err("MSI version cannot be empty".into());
    }
    let without_build = trimmed.split('+').next().unwrap_or(trimmed);
    let core = without_build.split('-').next().unwrap_or(without_build);
    let parts: Vec<&str> = core.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(format!(
            "cannot normalize `{version}` to MSI Major.Minor.Patch"
        ));
    }
    let parse_part =
        |raw: &str, max: u16, label: &str| -> Result<u16, String> {
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("invalid MSI {label} component `{raw}`"));
            }
            let val: u16 = raw.parse().map_err(|_| {
                format!("MSI {label} component `{raw}` overflows u16")
            })?;
            if val > max {
                return Err(format!(
                    "MSI {label} component `{val}` exceeds maximum {max}"
                ));
            }
            Ok(val)
        };
    let major = parse_part(parts[0], 255, "major")?;
    let minor = parse_part(parts[1], 255, "minor")?;
    let patch = if parts.len() == 3 {
        parse_part(parts[2], 65535, "patch")?
    } else {
        0
    };
    Ok(format!("{major}.{minor}.{patch}"))
}

/// Normalizes a `SemVer` string for `nfpm` so prerelease hyphens become `~`,
/// keeping RPM package versions valid and ordering correctly.
pub fn normalize_nfpm_version(version: &str) -> Result<String, String> {
    let trimmed = version.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return Err(format!("invalid package version `{version}`"));
    }
    Ok(trimmed.replace('-', "~"))
}

pub fn stage_linux(opts: &LinuxStageOptions) -> Result<(), String> {
    let root = workspace_root();
    let binary = opts
        .binary
        .clone()
        .unwrap_or_else(|| root.join("target/portable/ruum"));
    if !binary.is_file() {
        return Err(format!(
            "portable Linux binary not found at {}; run `cargo xtask portable` \
             first",
            binary.display()
        ));
    }
    let gui_binary = opts
        .gui_binary
        .clone()
        .unwrap_or_else(|| binary.with_file_name("ruum-gui"));
    if !gui_binary.is_file() {
        return Err(format!(
            "portable Linux GUI binary not found at {}; run `cargo xtask \
             portable` first",
            gui_binary.display()
        ));
    }
    let nfpm_version = normalize_nfpm_version(&opts.version)?;

    let appdir = opts.out.join("AppDir");
    let appdir_bin = appdir.join("usr/bin");
    fs::create_dir_all(&appdir_bin).map_err(|err| {
        format!("cannot create {}: {err}", appdir_bin.display())
    })?;

    let staged_bin = appdir_bin.join("ruum");
    copy_file(&binary, &staged_bin)?;
    set_executable(&staged_bin)?;

    let staged_gui_bin = appdir_bin.join("ruum-gui");
    copy_file(&gui_binary, &staged_gui_bin)?;
    set_executable(&staged_gui_bin)?;

    let desktop_system = DESKTOP_TEMPLATE
        .replace("@BINDIR@", "/usr/bin")
        .replace("@VERSION@", &opts.version);
    let desktop_path = opts.out.join("ruum.desktop");
    write_file(&desktop_path, &desktop_system)?;

    // AppImage desktop files rely on PATH inside AppRun rather than /usr/bin.
    let desktop_appdir = DESKTOP_TEMPLATE
        .replace("@BINDIR@/", "")
        .replace("@BINDIR@", "")
        .replace("@VERSION@", &opts.version);
    write_file(&appdir.join("ruum.desktop"), &desktop_appdir)?;
    let share_apps = appdir.join("usr/share/applications");
    create_dir_all(&share_apps)?;
    write_file(&share_apps.join("ruum.desktop"), &desktop_appdir)?;

    let png_src = root.join("assets/ruum.png");
    let svg_src = root.join("assets/ruum.svg");
    let staged_icon = appdir.join("ruum.svg");
    copy_file(&svg_src, &staged_icon)?;
    copy_file(&png_src, &appdir.join("ruum.png"))?;
    copy_file(&png_src, &appdir.join(".DirIcon"))?;

    let icon_256_dir = appdir.join("usr/share/icons/hicolor/256x256/apps");
    create_dir_all(&icon_256_dir)?;
    copy_file(&png_src, &icon_256_dir.join("ruum.png"))?;

    let icon_scalable_dir =
        appdir.join("usr/share/icons/hicolor/scalable/apps");
    create_dir_all(&icon_scalable_dir)?;
    copy_file(&svg_src, &icon_scalable_dir.join("ruum.svg"))?;

    let metainfo_dir = appdir.join("usr/share/metainfo");
    create_dir_all(&metainfo_dir)?;
    write_file(
        &metainfo_dir.join("io.github.tobi.ruum.appdata.xml"),
        APPDATA_XML,
    )?;

    let apprun_path = appdir.join("AppRun");
    write_file(&apprun_path, APPRUN_SCRIPT)?;
    set_executable(&apprun_path)?;

    let nfpm = NFPM_TEMPLATE
        .replace("@NFPM_ARCH@", opts.arch.nfpm_arch())
        .replace("@NFPM_VERSION@", &nfpm_version)
        .replace("@BIN_PATH@", &slash_path(&staged_bin))
        .replace("@GUI_BIN_PATH@", &slash_path(&staged_gui_bin))
        .replace("@DESKTOP_PATH@", &slash_path(&desktop_path))
        .replace("@ICON_PATH@", &slash_path(&staged_icon))
        .replace("@LICENSE_PATH@", &slash_path(&root.join("LICENSE")))
        .replace("@README_PATH@", &slash_path(&root.join("README.md")));
    let nfpm_path = opts.out.join("nfpm.yaml");
    write_file(&nfpm_path, &nfpm)?;

    println!("{}", nfpm_path.display());
    println!("{}", appdir.display());
    Ok(())
}

pub fn stage_windows(opts: &WindowsStageOptions) -> Result<(), String> {
    let root = workspace_root();
    let msi_version = normalize_msi_version(&opts.version)?;
    fs::create_dir_all(&opts.out).map_err(|err| {
        format!("cannot create {}: {err}", opts.out.display())
    })?;

    let exe_path = opts
        .exe
        .clone()
        .unwrap_or_else(|| root.join("target/release/ruum.exe"));
    let gui_exe_path = opts
        .gui_exe
        .clone()
        .unwrap_or_else(|| exe_path.with_file_name("ruum-gui.exe"));
    let icon_path = root.join("assets/ruum.ico");
    let readme_path = root.join("README.md");
    let license_path = root.join("LICENSE");
    let notices_path = root.join("THIRD_PARTY_NOTICES.md");
    let resolved_pack = if let Some(explicit) = &opts.scanner_pack {
        if !explicit.is_file() {
            return Err(format!(
                "scanner pack not found at {}",
                explicit.display()
            ));
        }
        Some(explicit.clone())
    } else {
        let default_pack = crate::scanner_pack::default_pack_path(
            &crate::scanner_pack::target_dir_for(&root),
        );
        default_pack.is_file().then_some(default_pack)
    };
    let (wxs_pack_component, wxs_pack_ref, iss_pack_entry) = if let Some(
        pack_path,
    ) =
        &resolved_pack
    {
        let escaped = xml_escape(&native_path(pack_path));
        let comp = format!(
            "<Component Id=\"ScannerPackFile\" Directory=\"INSTALLFOLDER\" \
                 Guid=\"B7F4E102-1C94-4A6D-9B28-3D71E5A4C006\">\n      \
                 <File Id=\"ScannerPack\" Source=\"{escaped}\" KeyPath=\"yes\" />\n    \
                 </Component>"
        );
        let comp_ref = "<ComponentRef Id=\"ScannerPackFile\" />".to_owned();
        let iss_entry = format!(
            "Source: \"{}\"; DestDir: \"{{app}}\"; Flags: ignoreversion",
            native_path(pack_path)
        );
        (comp, comp_ref, iss_entry)
    } else {
        (String::new(), String::new(), String::new())
    };

    let wxs = WXS_TEMPLATE
        .replace("@MSI_VERSION@", &xml_escape(&msi_version))
        .replace("@EXE_PATH@", &xml_escape(&native_path(&exe_path)))
        .replace("@GUI_EXE_PATH@", &xml_escape(&native_path(&gui_exe_path)))
        .replace("@ICON_PATH@", &xml_escape(&native_path(&icon_path)))
        .replace("@README_PATH@", &xml_escape(&native_path(&readme_path)))
        .replace("@LICENSE_PATH@", &xml_escape(&native_path(&license_path)))
        .replace("@NOTICES_PATH@", &xml_escape(&native_path(&notices_path)))
        .replace("@SCANNER_PACK_COMPONENT@", &wxs_pack_component)
        .replace("@SCANNER_PACK_COMPONENT_REF@", &wxs_pack_ref);
    let wxs_path = opts.out.join("ruum.wxs");
    write_file(&wxs_path, &wxs)?;

    let iss = ISS_TEMPLATE
        .replace("@VERSION@", &opts.version)
        .replace("@ARCH@", opts.arch.as_str())
        .replace("@INNO_ARCH_ALLOWED@", opts.arch.inno_arch())
        .replace("@INNO_ARCH_INSTALL64@", opts.arch.inno_arch())
        .replace("@OUTPUT_DIR@", &native_path(&opts.out))
        .replace("@EXE_PATH@", &native_path(&exe_path))
        .replace("@GUI_EXE_PATH@", &native_path(&gui_exe_path))
        .replace("@ICON_PATH@", &native_path(&icon_path))
        .replace("@README_PATH@", &native_path(&readme_path))
        .replace("@LICENSE_PATH@", &native_path(&license_path))
        .replace("@NOTICES_PATH@", &native_path(&notices_path))
        .replace("@SCANNER_PACK_FILE_ENTRY@", &iss_pack_entry);
    let iss_path = opts.out.join("ruum.iss");
    write_file(&iss_path, &iss)?;

    println!("WIX_ARCH={}", opts.arch.wix_arch());
    println!("{}", wxs_path.display());
    println!("{}", iss_path.display());
    Ok(())
}

pub fn render_homebrew_cask(
    opts: &HomebrewCaskOptions,
) -> Result<String, String> {
    let version = opts.version.trim();
    if version.is_empty() || version.chars().any(char::is_whitespace) {
        return Err(format!("invalid cask version `{}`", opts.version));
    }
    let arm64 = validate_sha256(&opts.arm64_sha256, "--arm64-sha256")?;
    let x64 = validate_sha256(&opts.x64_sha256, "--x64-sha256")?;
    let repo = opts.repo.trim();
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(format!("--repo `{repo}` must look like owner/repo"));
    };
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || repo.chars().any(char::is_whitespace)
    {
        return Err(format!("--repo `{repo}` must look like owner/repo"));
    }

    let caveats = if opts.signed {
        ""
    } else {
        concat!(
            "\n  caveats <<~EOS\n",
            "    ruum is not notarized with an Apple Developer ID in this release.\n",
            "    If macOS blocks the app on first launch, remove the quarantine attribute:\n",
            "      xattr -dr com.apple.quarantine \"#{appdir}/ruum.app\"\n",
            "  EOS\n",
        )
    };

    Ok(HOMEBREW_TEMPLATE
        .replace("@VERSION@", version)
        .replace("@ARM64_SHA256@", &arm64)
        .replace("@X64_SHA256@", &x64)
        .replace("@REPO@", repo)
        .replace("@CAVEATS@", caveats))
}

pub fn generate_checksums(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let entries = fs::read_dir(dir)
        .map_err(|err| format!("cannot read {}: {err}", dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            format!("cannot read entry in {}: {err}", dir.display())
        })?;
        let path = entry.path();
        let meta = entry
            .metadata()
            .map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "SHA256SUMS"
            || name.starts_with("manifest.json")
            || name.ends_with(".sha256")
            || name.starts_with('.')
        {
            continue;
        }
        files.push((name, path));
    }
    if files.is_empty() {
        return Err(format!("no package artifacts found in {}", dir.display()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sums = String::new();
    let mut results = Vec::with_capacity(files.len());
    for (name, path) in files {
        let hex = sha256_file(&path)?;
        let line = format!("{hex}  {name}\n");
        write_file(&dir.join(format!("{name}.sha256")), &line)?;
        sums.push_str(&line);
        results.push((name, hex));
    }
    write_file(&dir.join("SHA256SUMS"), &sums)?;
    Ok(results)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let file = File::open(path)
        .map_err(|err| format!("cannot open {}: {err}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buf)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

fn validate_sha256(raw: &str, flag: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.len() != 64 || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{flag} must be a 64-character hex SHA-256"));
    }
    Ok(trimmed.to_ascii_lowercase())
}

fn xml_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn native_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn create_dir_all(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|err| format!("cannot create {}: {err}", path.display()))
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to).map(drop).map_err(|err| {
        format!("cannot copy {} to {}: {err}", from.display(), to.display())
    })
}

fn write_file(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents)
        .map_err(|err| format!("cannot write {}: {err}", path.display()))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;

    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(path, perms)
        .map_err(|err| format!("cannot chmod 0755 {}: {err}", path.display()))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps, reason = "matches unix signature")]
const fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn str_args(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parses_package_cli_subcommands() {
        let linux = parse_args(&str_args(&[
            "linux-stage",
            "--arch",
            "x86_64",
            "--out",
            "dist/stage",
        ]))
        .unwrap();
        assert!(matches!(
            linux,
            Subcommand::LinuxStage(LinuxStageOptions {
                arch: Arch::X86_64,
                ..
            })
        ));

        let win = parse_args(&str_args(&[
            "windows-stage",
            "--arch",
            "aarch64",
            "--out",
            "dist/win",
        ]))
        .unwrap();
        assert!(matches!(
            win,
            Subcommand::WindowsStage(WindowsStageOptions {
                arch: Arch::Aarch64,
                ..
            })
        ));

        let cask = parse_args(&str_args(&[
            "homebrew-cask",
            "--version",
            "0.1.1",
            "--arm64-sha256",
            &"a".repeat(64),
            "--x64-sha256",
            &"b".repeat(64),
            "--repo",
            "JsonLee12138/Ruum",
            "--out",
            "dist/ruum.rb",
            "--signed",
        ]))
        .unwrap();
        assert!(matches!(
            cask,
            Subcommand::HomebrewCask(HomebrewCaskOptions { signed: true, .. })
        ));

        let sums =
            parse_args(&str_args(&["checksums", "--dir", "dist"])).unwrap();
        assert!(matches!(sums, Subcommand::Checksums { .. }));

        let manifest = parse_args(&str_args(&[
            "manifest",
            "--version",
            "0.2.2",
            "--tag",
            "v0.2.2",
            "--channel",
            "stable",
            "--repo",
            "JsonLee12138/Ruum",
            "--dir",
            "dist",
            "--notes-file",
            "notes.md",
            "--out",
            "dist/manifest.json",
            "--sign-key-env",
            "RUUM_UPDATE_SIGNING_KEY",
            "--published-at",
            "2026-11-20T03:21:19Z",
        ]))
        .unwrap();
        assert!(matches!(
            manifest,
            Subcommand::Manifest(ManifestOptions {
                channel: Channel::Stable,
                sign_key_env: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn normalizes_semver_to_msi_version() {
        assert_eq!(normalize_msi_version("0.1.1").unwrap(), "0.1.1");
        assert_eq!(
            normalize_msi_version("1.2.3-rc.1+build.9").unwrap(),
            "1.2.3"
        );
        assert_eq!(normalize_msi_version("2.4").unwrap(), "2.4.0");
        assert!(normalize_msi_version("").is_err());
        assert!(normalize_msi_version("256.0.0").is_err());
        assert!(normalize_msi_version("0.1.2.3").is_err());
    }

    #[test]
    fn normalizes_nfpm_version_for_rpm_safety() {
        assert_eq!(normalize_nfpm_version("0.1.1").unwrap(), "0.1.1");
        assert_eq!(normalize_nfpm_version("0.1.1-rc.1").unwrap(), "0.1.1~rc.1");
        assert!(normalize_nfpm_version("").is_err());
    }

    #[test]
    fn stages_linux_nfpm_and_appdir_without_bindir_in_appdir_desktop() {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("ruum");
        let gui_bin = tmp.path().join("ruum-gui");
        fs::write(&bin, b"ELF_WITH_TRAILER").unwrap();
        fs::write(&gui_bin, b"ELF_GUI").unwrap();
        let out = tmp.path().join("stage");
        stage_linux(&LinuxStageOptions {
            arch: Arch::Aarch64,
            out: out.clone(),
            binary: Some(bin),
            gui_binary: None,
            version: "0.1.1".into(),
        })
        .unwrap();

        let desktop = fs::read_to_string(out.join("ruum.desktop")).unwrap();
        assert!(desktop.contains("Exec=/usr/bin/ruum-gui %f"));
        assert!(desktop.contains("Actions=remote;"));
        assert!(desktop.contains("[Desktop Action remote]"));
        assert!(
            desktop.contains("Exec=/usr/bin/ruum-gui --open-screen remote")
        );
        assert!(desktop.contains("X-AppVersion=0.1.1"));

        let appdir_desktop =
            fs::read_to_string(out.join("AppDir/ruum.desktop")).unwrap();
        assert!(appdir_desktop.contains("Exec=ruum-gui %f"));
        assert!(appdir_desktop.contains("Actions=remote;"));
        assert!(appdir_desktop.contains("[Desktop Action remote]"));
        assert!(appdir_desktop.contains("Exec=ruum-gui --open-screen remote"));
        assert!(!appdir_desktop.contains("@BINDIR@"));

        let nfpm = fs::read_to_string(out.join("nfpm.yaml")).unwrap();
        assert!(nfpm.contains("arch: \"arm64\""));
        assert!(nfpm.contains("overrides:"));
        assert!(nfpm.contains("/usr/bin/ruum-gui"));
        assert!(!nfpm.contains('@'));

        assert!(out.join("AppDir/usr/bin/ruum").is_file());
        assert!(out.join("AppDir/usr/bin/ruum-gui").is_file());
        assert!(out.join("AppDir/ruum.svg").is_file());
        assert!(out.join("AppDir/ruum.png").is_file());
        assert!(out.join("AppDir/.DirIcon").is_file());
        assert!(
            out.join("AppDir/usr/share/icons/hicolor/256x256/apps/ruum.png")
                .is_file()
        );
        assert!(
            out.join("AppDir/usr/share/icons/hicolor/scalable/apps/ruum.svg")
                .is_file()
        );
        assert!(out.join("AppDir/AppRun").is_file());

        let png_magic = b"\x89PNG\r\n\x1a\n";
        for icon_rel in [
            "AppDir/.DirIcon",
            "AppDir/ruum.png",
            "AppDir/usr/share/icons/hicolor/256x256/apps/ruum.png",
        ] {
            let bytes = fs::read(out.join(icon_rel)).unwrap();
            assert!(
                bytes.starts_with(png_magic),
                "{icon_rel} must be a valid PNG"
            );
        }

        let share_desktop = fs::read_to_string(
            out.join("AppDir/usr/share/applications/ruum.desktop"),
        )
        .unwrap();
        assert_eq!(share_desktop, appdir_desktop);

        let metainfo =
            fs::read_to_string(out.join(
                "AppDir/usr/share/metainfo/io.github.tobi.ruum.appdata.xml",
            ))
            .unwrap();
        assert!(metainfo.contains("<id>io.github.tobi.ruum</id>"));
        assert!(
            metainfo
                .contains("<project_license>GPL-3.0-only</project_license>")
        );
        assert!(metainfo.contains(
            "<launchable type=\"desktop-id\">ruum.desktop</launchable>"
        ));
        assert!(metainfo.contains("<binary>ruum-gui</binary>"));
        assert!(metainfo.contains("<binary>ruum</binary>"));
        assert!(!metainfo.contains('@'));

        let apprun = fs::read_to_string(out.join("AppDir/AppRun")).unwrap();
        assert!(apprun.contains(
            "export LD_LIBRARY_PATH=\"$APPDIR/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}\""
        ));
        assert!(apprun.contains("update|space|run-script"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let bin_mode = fs::metadata(out.join("AppDir/usr/bin/ruum"))
                .unwrap()
                .permissions()
                .mode();
            let gui_mode = fs::metadata(out.join("AppDir/usr/bin/ruum-gui"))
                .unwrap()
                .permissions()
                .mode();
            let apprun_mode = fs::metadata(out.join("AppDir/AppRun"))
                .unwrap()
                .permissions()
                .mode();
            assert_ne!(bin_mode & 0o111, 0);
            assert_ne!(gui_mode & 0o111, 0);
            assert_ne!(apprun_mode & 0o111, 0);
        }
    }

    #[test]
    fn stages_windows_wxs_and_iss_for_both_architectures() {
        let tmp = TempDir::new().unwrap();
        let out_x64 = tmp.path().join("x64");
        stage_windows(&WindowsStageOptions {
            arch: Arch::X86_64,
            out: out_x64.clone(),
            exe: Some(tmp.path().join("ruum.exe")),
            gui_exe: None,
            scanner_pack: None,
            version: "0.1.1-beta.1".into(),
        })
        .unwrap();
        let wxs_x64 = fs::read_to_string(out_x64.join("ruum.wxs")).unwrap();
        assert!(wxs_x64.contains("Version=\"0.1.1\""));
        assert!(wxs_x64.contains("Scope=\"perMachine\""));
        assert!(wxs_x64.contains("ruum-gui.exe"));
        assert!(wxs_x64.contains("Key=\"System.AppUserModel.ID\""));
        assert!(wxs_x64.contains("Value=\"io.github.tobi.ruum\""));
        assert!(!wxs_x64.contains("ScannerPackFile"));
        assert!(!wxs_x64.contains('@'));
        let iss_x64 = fs::read_to_string(out_x64.join("ruum.iss")).unwrap();
        assert!(
            iss_x64.contains("AppId={{E8F4B9A2-6D1C-4B3A-9E7F-2C5A1D8E4B60}")
        );
        assert!(!iss_x64.contains("}}"));
        assert!(iss_x64.contains("ArchitecturesAllowed=x64compatible"));
        assert!(iss_x64.contains(
            "OutputBaseFilename=ruum-0.1.1-beta.1-x86_64-windows-setup"
        ));
        assert!(iss_x64.contains("ruum-gui.exe"));
        assert!(iss_x64.contains("AppUserModelID: \"io.github.tobi.ruum\""));
        assert!(iss_x64.contains("CloseApplications=yes"));
        assert!(
            iss_x64.contains("CloseApplicationsFilter=ruum.exe,ruum-gui.exe")
        );
        assert!(iss_x64.contains("RestartApplications=yes"));
        assert!(!iss_x64.contains("ruum-scan-linux.pack"));
        assert!(!iss_x64.contains('@'));

        let pack_file = tmp.path().join("ruum-scan-linux.pack");
        fs::write(&pack_file, b"pack-bytes").unwrap();
        let out_arm = tmp.path().join("arm64");
        stage_windows(&WindowsStageOptions {
            arch: Arch::Aarch64,
            out: out_arm.clone(),
            exe: None,
            gui_exe: None,
            scanner_pack: Some(pack_file),
            version: "0.1.1".into(),
        })
        .unwrap();
        let wxs_arm = fs::read_to_string(out_arm.join("ruum.wxs")).unwrap();
        assert!(wxs_arm.contains("ScannerPackFile"));
        assert!(wxs_arm.contains("ruum-scan-linux.pack"));
        assert!(!wxs_arm.contains('@'));
        let iss_arm = fs::read_to_string(out_arm.join("ruum.iss")).unwrap();
        assert!(iss_arm.contains("ArchitecturesAllowed=arm64"));
        assert!(iss_arm.contains("ArchitecturesInstallIn64BitMode=arm64"));
        assert!(iss_arm.contains("ruum-scan-linux.pack"));
        assert!(!iss_arm.contains('@'));
    }

    #[test]
    fn renders_homebrew_cask_for_unsigned_and_signed_builds() {
        let h1 = "A".repeat(64);
        let h2 = "b".repeat(64);
        let unsigned = render_homebrew_cask(&HomebrewCaskOptions {
            version: "0.1.1".into(),
            arm64_sha256: h1,
            x64_sha256: h2.clone(),
            repo: "JsonLee12138/Ruum".into(),
            out: PathBuf::from("ruum.rb"),
            signed: false,
        })
        .unwrap();
        assert!(unsigned.contains(&"a".repeat(64)));
        assert!(unsigned.contains(&h2));
        assert!(unsigned.contains("auto_updates true"));
        assert!(unsigned.contains("com.apple.quarantine"));
        assert!(!unsigned.contains('@'));

        let signed = render_homebrew_cask(&HomebrewCaskOptions {
            version: "0.1.1".into(),
            arm64_sha256: "a".repeat(64),
            x64_sha256: h2,
            repo: "JsonLee12138/Ruum".into(),
            out: PathBuf::from("ruum.rb"),
            signed: true,
        })
        .unwrap();
        assert!(signed.contains("auto_updates true"));
        assert!(!signed.contains("com.apple.quarantine"));
        assert!(!signed.contains("caveats"));
    }

    #[test]
    fn generates_sorted_checksums_ignoring_existing_sha_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("z-last.zip"), b"hello").unwrap();
        fs::write(tmp.path().join("a-first.dmg"), b"world").unwrap();
        fs::write(tmp.path().join("stale.sha256"), b"ignore").unwrap();
        fs::write(tmp.path().join("SHA256SUMS"), b"ignore").unwrap();
        fs::write(tmp.path().join("manifest.json"), b"ignore").unwrap();
        fs::write(tmp.path().join("manifest.json.sig"), b"ignore").unwrap();

        let entries = generate_checksums(tmp.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "a-first.dmg");
        assert_eq!(entries[1].0, "z-last.zip");
        assert_eq!(
            entries[0].1,
            "486ea46224d1bb4fb680f34f7c9ad96a8f24ec88be73ea8e5a6c65260e9cb8a7"
        );
        assert_eq!(
            entries[1].1,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );

        let sums = fs::read_to_string(tmp.path().join("SHA256SUMS")).unwrap();
        assert_eq!(
            sums,
            format!(
                "{}  a-first.dmg\n{}  z-last.zip\n",
                entries[0].1, entries[1].1
            )
        );
        let sidecar =
            fs::read_to_string(tmp.path().join("a-first.dmg.sha256")).unwrap();
        assert_eq!(sidecar, format!("{}  a-first.dmg\n", entries[0].1));
    }

    #[test]
    fn rejects_invalid_inputs_and_escapes_xml_paths() {
        let tmp = TempDir::new().unwrap();
        assert!(generate_checksums(tmp.path()).is_err());
        assert!(
            stage_linux(&LinuxStageOptions {
                arch: Arch::X86_64,
                out: tmp.path().join("out"),
                binary: Some(tmp.path().join("missing-ruum")),
                gui_binary: None,
                version: "0.1.1".into(),
            })
            .is_err()
        );
        assert!(
            render_homebrew_cask(&HomebrewCaskOptions {
                version: "0.1.1".into(),
                arm64_sha256: "short".into(),
                x64_sha256: "b".repeat(64),
                repo: "JsonLee12138/Ruum".into(),
                out: PathBuf::from("ruum.rb"),
                signed: false,
            })
            .is_err()
        );
        assert!(
            render_homebrew_cask(&HomebrewCaskOptions {
                version: "0.1.1".into(),
                arm64_sha256: "a".repeat(64),
                x64_sha256: "b".repeat(64),
                repo: "invalid-repo".into(),
                out: PathBuf::from("ruum.rb"),
                signed: false,
            })
            .is_err()
        );

        let tricky = tmp.path().join("a&b");
        stage_windows(&WindowsStageOptions {
            arch: Arch::X86_64,
            out: tricky.clone(),
            exe: Some(tricky.join("ruum.exe")),
            gui_exe: None,
            scanner_pack: None,
            version: "0.1.1".into(),
        })
        .unwrap();
        let wxs = fs::read_to_string(tricky.join("ruum.wxs")).unwrap();
        assert!(wxs.contains("a&amp;b"));
    }

    #[test]
    fn windows_release_workflow_pins_wix5_and_roll_forward() {
        let workflow = include_str!("../../.github/workflows/release.yml");
        assert!(workflow.contains("DOTNET_ROLL_FORWARD: Major"));
        assert!(
            workflow.contains(
                "dotnet tool install wix --version 5.0.2 --tool-path"
            )
        );
        assert!(workflow.contains("WIX_PATH="));
        assert!(workflow.contains("& $env:WIX_PATH build"));
        assert!(!workflow.contains("dotnet tool install --global wix"));
        assert!(workflow.contains("cargo xtask scanner-pack --strict"));
        assert!(workflow.contains("ruum-scan-linux.pack"));
    }

    #[test]
    fn linux_release_workflow_and_readme_use_compliant_appimage_contract() {
        let workflow = include_str!("../../.github/workflows/release.yml");
        assert!(workflow.contains("bundle-appdir-libs.sh"));
        assert!(workflow.contains("appstreamcli validate-tree"));
        assert!(workflow.contains("xvfb-run"));
        assert!(workflow.contains("zsync"));
        assert!(workflow.contains("gh-releases-zsync|"));
        assert!(workflow.contains("-u \"$update_info\""));
        assert!(workflow.contains(".AppImage.zsync"));
        assert!(workflow.contains("readelf -p .upd_info"));
        assert!(workflow.contains(
            "dist/ruum-${{ needs.validate.outputs.version }}-${{ matrix.arch }}.AppImage"
        ));
        assert!(workflow.contains(concat!(
            "dist/ruum-${{ needs.validate.outputs.version }}-",
            "${{ matrix.arch }}.AppImage.zsync",
        )));
        assert!(!workflow.contains("-linux.AppImage"));

        let readme = include_str!("../../README.md");
        assert!(readme.contains("ruum-*-x86_64.AppImage"));
        assert!(!readme.contains("-linux.AppImage"));
    }

    #[cfg(unix)]
    #[test]
    fn bundle_appdir_libs_filters_excludelist_and_preserves_ruum_bytes() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::Command;

        let tmp = TempDir::new().unwrap();
        let root = workspace_root();
        let appdir = tmp.path().join("AppDir");
        let bin_dir = appdir.join("usr/bin");
        let fake_sys_lib = tmp.path().join("syslib");
        let mock_tools = tmp.path().join("tools");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&fake_sys_lib).unwrap();
        fs::create_dir_all(&mock_tools).unwrap();

        let ruum_bytes = b"RUUM_ELF_HEADER_PLUS_RUUM_SCAN_TRAILER_12345";
        fs::write(bin_dir.join("ruum"), ruum_bytes).unwrap();
        fs::write(bin_dir.join("ruum-gui"), b"RUUM_GUI_ELF").unwrap();

        for lib in [
            "libxkbcommon-x11.so.0",
            "libxkbcommon.so.0",
            "libxcb-shape.so.0",
            "libc.so.6",
            "libGL.so.1",
            "libwayland-client.so.0",
            "libfontconfig.so.1",
        ] {
            fs::write(fake_sys_lib.join(lib), format!("SO:{lib}")).unwrap();
        }

        let patchelf_log = tmp.path().join("patchelf.log");
        let ldd_script = format!(
            "#!/bin/sh\n\
             printf '\\tlinux-vdso.so.1 (0x00007fff)\\n'\n\
             printf '\\tlibxkbcommon-x11.so.0 => {sys}/libxkbcommon-x11.so.0 (0x1)\\n'\n\
             printf '\\tlibxkbcommon.so.0 => {sys}/libxkbcommon.so.0 (0x2)\\n'\n\
             printf '\\tlibxcb-shape.so.0 => {sys}/libxcb-shape.so.0 (0x3)\\n'\n\
             printf '\\tlibGL.so.1 => {sys}/libGL.so.1 (0x4)\\n'\n\
             printf '\\tlibwayland-client.so.0 => {sys}/libwayland-client.so.0 (0x5)\\n'\n\
             printf '\\tlibfontconfig.so.1 => {sys}/libfontconfig.so.1 (0x6)\\n'\n\
             printf '\\tlibc.so.6 => {sys}/libc.so.6 (0x7)\\n'\n",
            sys = fake_sys_lib.display()
        );
        let ldd_path = mock_tools.join("ldd");
        fs::write(&ldd_path, ldd_script).unwrap();
        fs::set_permissions(&ldd_path, fs::Permissions::from_mode(0o755))
            .unwrap();

        let patchelf_script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> {log}\n",
            log = patchelf_log.display()
        );
        let patchelf_path = mock_tools.join("patchelf");
        fs::write(&patchelf_path, patchelf_script).unwrap();
        fs::set_permissions(&patchelf_path, fs::Permissions::from_mode(0o755))
            .unwrap();

        let existing_path = std::env::var_os("PATH").unwrap_or_default();
        let combined_path = format!(
            "{}:{}",
            mock_tools.display(),
            existing_path.to_string_lossy()
        );
        let output = Command::new("bash")
            .arg(root.join("packaging/linux/bundle-appdir-libs.sh"))
            .arg(&appdir)
            .arg(root.join("packaging/linux/appimage-excludelist"))
            .env("PATH", combined_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "bundle-appdir-libs.sh failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        assert!(appdir.join("usr/lib/libxkbcommon-x11.so.0").is_file());
        assert!(appdir.join("usr/lib/libxkbcommon.so.0").is_file());
        assert!(appdir.join("usr/lib/libxcb-shape.so.0").is_file());
        assert!(!appdir.join("usr/lib/libc.so.6").exists());
        assert!(!appdir.join("usr/lib/libGL.so.1").exists());
        assert!(!appdir.join("usr/lib/libwayland-client.so.0").exists());
        assert!(!appdir.join("usr/lib/libfontconfig.so.1").exists());

        assert_eq!(fs::read(bin_dir.join("ruum")).unwrap(), ruum_bytes);

        let log_contents = fs::read_to_string(&patchelf_log).unwrap();
        assert!(log_contents.contains("--set-rpath $ORIGIN/../lib"));
        assert!(log_contents.contains("usr/bin/ruum-gui"));
        assert!(log_contents.contains("--set-rpath $ORIGIN "));
        assert!(
            !log_contents
                .lines()
                .any(|line| line.ends_with("usr/bin/ruum")),
            "patchelf must never touch usr/bin/ruum: {log_contents}"
        );
    }

    #[test]
    fn builds_manifest_and_verifies_ed25519_raw_byte_signature() {
        let tmp = TempDir::new().unwrap();
        let dist = tmp.path().join("dist");
        fs::create_dir_all(&dist).unwrap();

        let ver = "0.2.2-beta.1";
        let tag = "v0.2.2-beta.1";
        let sample_files = [
            (
                "macos-aarch64-app_zip",
                format!("ruum-{ver}-aarch64-macos.zip"),
            ),
            ("macos-aarch64-dmg", format!("ruum-{ver}-aarch64-macos.dmg")),
            ("macos-aarch64-pkg", format!("ruum-{ver}-aarch64-macos.pkg")),
            (
                "macos-x86_64-app_zip",
                format!("ruum-{ver}-x86_64-macos.zip"),
            ),
            ("macos-x86_64-dmg", format!("ruum-{ver}-x86_64-macos.dmg")),
            ("macos-x86_64-pkg", format!("ruum-{ver}-x86_64-macos.pkg")),
            (
                "windows-x86_64-inno",
                format!("ruum-{ver}-x86_64-windows-setup.exe"),
            ),
            (
                "windows-x86_64-msi",
                format!("ruum-{ver}-x86_64-windows.msi"),
            ),
            (
                "windows-x86_64-portable_zip",
                format!("ruum-{ver}-x86_64-windows.zip"),
            ),
            (
                "windows-aarch64-inno",
                format!("ruum-{ver}-aarch64-windows-setup.exe"),
            ),
            (
                "windows-aarch64-msi",
                format!("ruum-{ver}-aarch64-windows.msi"),
            ),
            (
                "windows-aarch64-portable_zip",
                format!("ruum-{ver}-aarch64-windows.zip"),
            ),
            (
                "linux-x86_64-appimage",
                format!("ruum-{ver}-x86_64.AppImage"),
            ),
            (
                "linux-x86_64-tarball",
                format!("ruum-{ver}-x86_64-linux.tar.gz"),
            ),
            ("linux-x86_64-deb", format!("ruum-{ver}-x86_64-linux.deb")),
            ("linux-x86_64-rpm", format!("ruum-{ver}-x86_64-linux.rpm")),
            (
                "linux-aarch64-appimage",
                format!("ruum-{ver}-aarch64.AppImage"),
            ),
            (
                "linux-aarch64-tarball",
                format!("ruum-{ver}-aarch64-linux.tar.gz"),
            ),
            ("linux-aarch64-deb", format!("ruum-{ver}-aarch64-linux.deb")),
            ("linux-aarch64-rpm", format!("ruum-{ver}-aarch64-linux.rpm")),
        ];
        for (key, name) in &sample_files {
            fs::write(dist.join(name), format!("payload:{key}")).unwrap();
        }
        generate_checksums(&dist).unwrap();

        let notes_path = tmp.path().join("notes.md");
        fs::write(&notes_path, "### Fixes\n\n- Retry SQLite busy errors.\n")
            .unwrap();
        let out_path = dist.join("manifest.json");

        let manifest = build_and_write_manifest(&ManifestOptions {
            version: ver.into(),
            tag: tag.into(),
            channel: Channel::Beta,
            repo: "JsonLee12138/Ruum".into(),
            dir: dist.clone(),
            notes_file: notes_path,
            out: out_path.clone(),
            sign_key_env: None,
            published_at: Some("2026-11-20T03:21:19Z".into()),
        })
        .unwrap();

        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.version, ver);
        assert_eq!(manifest.tag, tag);
        assert_eq!(manifest.channel, "beta");
        assert_eq!(manifest.published_at, "2026-11-20T03:21:19Z");
        assert_eq!(
            manifest.release_url,
            "https://github.com/JsonLee12138/Ruum/releases/tag/v0.2.2-beta.1"
        );
        assert_eq!(manifest.assets.len(), 20);
        let dmg = &manifest.assets["macos-aarch64-dmg"];
        assert_eq!(dmg.filename, format!("ruum-{ver}-aarch64-macos.dmg"));
        assert_eq!(dmg.path, format!("releases/{tag}/{}", dmg.filename));
        assert_eq!(
            dmg.fallback_url,
            format!(
                "https://github.com/JsonLee12138/Ruum/releases/download/{tag}/{}",
                dmg.filename
            )
        );
        assert_eq!(dmg.size, "payload:macos-aarch64-dmg".len() as u64);

        let raw_bytes = fs::read(&out_path).unwrap();
        let seed = [7_u8; 32];
        let seed_b64 = BASE64.encode(seed);
        let signing_key = SigningKey::from_bytes(&seed);
        let pub_b64 = BASE64.encode(signing_key.verifying_key().to_bytes());

        let sig_b64 = sign_manifest_bytes(&raw_bytes, &seed_b64).unwrap();
        verify_manifest_bytes(&raw_bytes, &sig_b64, &pub_b64).unwrap();

        let mut tampered = raw_bytes;
        tampered[0] = b'{';
        tampered.push(b' ');
        assert!(verify_manifest_bytes(&tampered, &sig_b64, &pub_b64).is_err());

        let signed_out = tmp.path().join("signed-manifest.json");
        let signed_opts = ManifestOptions {
            version: ver.into(),
            tag: tag.into(),
            channel: Channel::Beta,
            repo: "JsonLee12138/Ruum".into(),
            dir: dist.clone(),
            notes_file: tmp.path().join("notes.md"),
            out: signed_out.clone(),
            sign_key_env: None,
            published_at: Some("2026-11-20T03:21:19Z".into()),
        };
        build_manifest_with_signing_key(&signed_opts, Some(&seed_b64)).unwrap();
        let signed_raw = fs::read(&signed_out).unwrap();
        let sidecar_sig =
            fs::read_to_string(tmp.path().join("signed-manifest.json.sig"))
                .unwrap();
        verify_manifest_bytes(&signed_raw, &sidecar_sig, &pub_b64).unwrap();

        // Re-running checksums must ignore manifest.json* files.
        fs::write(dist.join("manifest.json.sig"), &sidecar_sig).unwrap();
        fs::write(dist.join("manifest.json.tmp"), b"temp").unwrap();
        let re_sums = generate_checksums(&dist).unwrap();
        assert_eq!(re_sums.len(), 20);

        // Mismatched SHA256SUMS must be rejected.
        fs::write(
            dist.join("SHA256SUMS"),
            format!("{}  ruum-{ver}-aarch64-macos.dmg\n", "0".repeat(64)),
        )
        .unwrap();
        assert!(build_and_write_manifest(&signed_opts).is_err());
    }

    #[test]
    fn release_workflow_integrates_update_manifest_and_r2_sync() {
        let workflow = include_str!("../../.github/workflows/release.yml");
        assert!(workflow.contains(
            "RUUM_UPDATE_BASE_URL: ${{ vars.RUUM_UPDATE_BASE_URL }}"
        ));
        assert!(workflow.contains(
            "RUUM_UPDATE_PUBLIC_KEY: ${{ vars.RUUM_UPDATE_PUBLIC_KEY }}"
        ));
        assert!(workflow.contains("cargo xtask release-notes"));
        assert!(workflow.contains("cargo xtask package manifest"));
        assert!(workflow.contains("RUUM_UPDATE_SIGNING_KEY"));
        assert!(workflow.contains("wrangler r2 object put"));
        assert!(workflow.contains("--remote"));
        assert!(workflow.contains("channels/$CHANNEL/manifest.json"));
    }

    fn workflow_job_section<'a>(
        workflow: &'a str,
        job_id: &str,
        next_job_id: Option<&str>,
    ) -> &'a str {
        let marker = format!("\n  {job_id}:\n");
        let (_, from_job) = workflow
            .split_once(&marker)
            .unwrap_or_else(|| panic!("missing job `{job_id}`"));
        match next_job_id {
            Some(next) => {
                let next_marker = format!("\n  {next}:\n");
                from_job
                    .split_once(&next_marker)
                    .map_or(from_job, |(section, _)| section)
            }
            None => from_job,
        }
    }

    #[test]
    fn ci_and_release_workflows_enforce_concurrency_timeouts_and_change_gating()
    {
        let ci = include_str!("../../.github/workflows/ci.yml");
        assert!(ci.contains(concat!(
            "paths-ignore: ",
            "[\"**.md\", \"docs/**\", \"cliff.toml\", \"LICENSE\"]"
        )));
        assert!(ci.contains("workflow_dispatch:"));
        assert!(ci.contains("include_macos_intel:"));
        assert!(ci.contains(
            "group: ci-${{ github.event.pull_request.number || github.ref }}"
        ));
        assert!(ci.contains(
            "cancel-in-progress: ${{ github.event_name == 'pull_request' }}"
        ));

        let changes = workflow_job_section(ci, "changes", Some("tooling-test"));
        assert!(changes.contains("fetch-depth: 0"));
        assert!(changes.contains("node scripts/ci-classify-changes.mjs"));
        assert!(changes.contains("timeout-minutes: 5"));
        for out in [
            "run_rust",
            "run_macos_bundle",
            "run_worker",
            "run_ci_scripts",
            "run_macos_intel",
        ] {
            assert!(
                changes.contains(&format!(
                    "{out}: ${{{{ steps.classify.outputs.{out} }}}}"
                )),
                "changes job missing output `{out}`"
            );
        }

        let tooling = workflow_job_section(ci, "tooling-test", Some("check"));
        assert!(tooling.contains("needs: changes"));
        assert!(tooling.contains(concat!(
            "needs.changes.outputs.run_worker == 'true' || ",
            "needs.changes.outputs.run_ci_scripts == 'true'"
        )));
        assert!(tooling.contains("timeout-minutes: 5"));
        assert!(
            tooling
                .contains("node --test scripts/ci-classify-changes.test.mjs")
        );
        assert!(
            tooling.contains(
                "node --test infra/update-worker/tests/worker.test.mjs"
            )
        );

        let check = workflow_job_section(ci, "check", Some("check-macos"));
        assert!(check.contains("needs: changes"));
        assert!(check.contains("if: needs.changes.outputs.run_rust == 'true'"));
        assert!(check.contains("timeout-minutes: 35"));

        let macos = workflow_job_section(ci, "check-macos", Some("windows"));
        assert!(macos.contains("needs: changes"));
        assert!(macos.contains("if: needs.changes.outputs.run_rust == 'true'"));
        assert!(macos.contains("timeout-minutes: 45"));
        assert!(
            macos.contains(
                "if: needs.changes.outputs.run_macos_bundle == 'true'"
            )
        );
        assert!(macos.contains("cargo xtask bundle --all"));

        let windows =
            workflow_job_section(ci, "windows", Some("other-architectures"));
        assert!(windows.contains("needs: changes"));
        assert!(
            windows.contains("if: needs.changes.outputs.run_rust == 'true'")
        );
        assert!(windows.contains("timeout-minutes: 45"));

        let other = workflow_job_section(
            ci,
            "other-architectures",
            Some("test-macos-intel"),
        );
        assert!(other.contains("needs: changes"));
        assert!(other.contains("if: needs.changes.outputs.run_rust == 'true'"));
        assert!(other.contains("timeout-minutes: 45"));
        assert!(other.contains("ubuntu-24.04-arm"));
        assert!(other.contains("windows-11-arm"));
        assert!(!other.contains("macos-15-intel"));

        let intel = workflow_job_section(ci, "test-macos-intel", None);
        assert!(intel.contains("needs: changes"));
        assert!(intel.contains(concat!(
            "if: needs.changes.outputs.run_rust == 'true' && ",
            "needs.changes.outputs.run_macos_intel == 'true'"
        )));
        assert!(intel.contains("runs-on: macos-15-intel"));
        assert!(intel.contains("timeout-minutes: 60"));
        assert!(intel.contains("cargo xtask test"));

        let release = include_str!("../../.github/workflows/release.yml");
        assert!(
            release.contains(
                "group: release-${{ inputs.tag || github.ref_name }}"
            )
        );
        assert!(release.contains("cancel-in-progress: true"));

        for (job, next, timeout) in [
            ("validate", Some("linux"), "timeout-minutes: 10"),
            ("linux", Some("macos"), "timeout-minutes: 45"),
            ("macos", Some("windows"), "timeout-minutes: 75"),
            ("windows", Some("publish"), "timeout-minutes: 60"),
            ("publish", None, "timeout-minutes: 20"),
        ] {
            let section = workflow_job_section(release, job, next);
            assert!(
                section.contains(timeout),
                "release job `{job}` missing `{timeout}`"
            );
        }
    }
}
