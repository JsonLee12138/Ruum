//! Cross-platform packaging staging, Homebrew cask rendering, and SHA-256
//! checksum generation.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufReader, Read as _};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

const DESKTOP_TEMPLATE: &str = include_str!("../../packaging/ruum.desktop.in");
const NFPM_TEMPLATE: &str = include_str!("../../packaging/linux/nfpm.yaml.in");
const APPRUN_SCRIPT: &str = include_str!("../../packaging/linux/AppRun");
const WXS_TEMPLATE: &str = include_str!("../../packaging/windows/ruum.wxs");
const ISS_TEMPLATE: &str = include_str!("../../packaging/windows/ruum.iss");
const HOMEBREW_TEMPLATE: &str =
    include_str!("../../packaging/homebrew/ruum.rb.in");

const USAGE: &str = "\
usage: cargo xtask package <subcommand> [options]

subcommands:
  linux-stage    --arch <x86_64|aarch64> --out <dir> [--binary <path>] [--version <V>]
  windows-stage  --arch <x86_64|aarch64> --out <dir> [--exe <path>] [--version <V>]
  homebrew-cask  --version <V> --arm64-sha256 <H1> --x64-sha256 <H2> --repo <owner/repo> --out <path> [--signed]
  checksums      --dir <dist_dir>
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
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsStageOptions {
    pub arch: Arch,
    pub out: PathBuf,
    pub exe: Option<PathBuf>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Subcommand {
    LinuxStage(LinuxStageOptions),
    WindowsStage(WindowsStageOptions),
    HomebrewCask(HomebrewCaskOptions),
    Checksums { dir: PathBuf },
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
        version,
    })
}

fn parse_windows_stage(args: &[String]) -> Result<WindowsStageOptions, String> {
    let mut arch = None;
    let mut out = None;
    let mut exe = None;
    let mut version = env!("CARGO_PKG_VERSION").to_string();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--arch" => {
                arch = Some(Arch::parse(next_arg(&mut iter, "--arch")?)?);
            }
            "--out" => out = Some(PathBuf::from(next_arg(&mut iter, "--out")?)),
            "--exe" => exe = Some(PathBuf::from(next_arg(&mut iter, "--exe")?)),
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
            "portable Linux binary not found at {}; run `cargo xtask portable` first",
            binary.display()
        ));
    }
    let nfpm_version = normalize_nfpm_version(&opts.version)?;

    let appdir = opts.out.join("AppDir");
    let appdir_bin = appdir.join("usr/bin");
    fs::create_dir_all(&appdir_bin).map_err(|err| {
        format!("cannot create {}: {err}", appdir_bin.display())
    })?;

    let staged_bin = appdir_bin.join("ruum");
    fs::copy(&binary, &staged_bin).map_err(|err| {
        format!(
            "cannot copy {} to {}: {err}",
            binary.display(),
            staged_bin.display()
        )
    })?;
    set_executable(&staged_bin)?;

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

    let svg_src = root.join("assets/ruum.svg");
    let staged_icon = appdir.join("ruum.svg");
    copy_file(&svg_src, &staged_icon)?;
    copy_file(&svg_src, &appdir.join(".DirIcon"))?;

    let apprun_path = appdir.join("AppRun");
    write_file(&apprun_path, APPRUN_SCRIPT)?;
    set_executable(&apprun_path)?;

    let nfpm = NFPM_TEMPLATE
        .replace("@NFPM_ARCH@", opts.arch.nfpm_arch())
        .replace("@NFPM_VERSION@", &nfpm_version)
        .replace("@BIN_PATH@", &slash_path(&staged_bin))
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
    let icon_path = root.join("assets/ruum.ico");
    let readme_path = root.join("README.md");
    let license_path = root.join("LICENSE");
    let notices_path = root.join("THIRD_PARTY_NOTICES.md");

    let wxs = WXS_TEMPLATE
        .replace("@MSI_VERSION@", &xml_escape(&msi_version))
        .replace("@EXE_PATH@", &xml_escape(&native_path(&exe_path)))
        .replace("@ICON_PATH@", &xml_escape(&native_path(&icon_path)))
        .replace("@README_PATH@", &xml_escape(&native_path(&readme_path)))
        .replace("@LICENSE_PATH@", &xml_escape(&native_path(&license_path)))
        .replace("@NOTICES_PATH@", &xml_escape(&native_path(&notices_path)));
    let wxs_path = opts.out.join("ruum.wxs");
    write_file(&wxs_path, &wxs)?;

    let iss = ISS_TEMPLATE
        .replace("@VERSION@", &opts.version)
        .replace("@ARCH@", opts.arch.as_str())
        .replace("@INNO_ARCH_ALLOWED@", opts.arch.inno_arch())
        .replace("@INNO_ARCH_INSTALL64@", opts.arch.inno_arch())
        .replace("@OUTPUT_DIR@", &native_path(&opts.out))
        .replace("@EXE_PATH@", &native_path(&exe_path))
        .replace("@ICON_PATH@", &native_path(&icon_path))
        .replace("@README_PATH@", &native_path(&readme_path))
        .replace("@LICENSE_PATH@", &native_path(&license_path))
        .replace("@NOTICES_PATH@", &native_path(&notices_path));
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
        fs::write(&bin, b"ELF_WITH_TRAILER").unwrap();
        let out = tmp.path().join("stage");
        stage_linux(&LinuxStageOptions {
            arch: Arch::Aarch64,
            out: out.clone(),
            binary: Some(bin),
            version: "0.1.1".into(),
        })
        .unwrap();

        let desktop = fs::read_to_string(out.join("ruum.desktop")).unwrap();
        assert!(desktop.contains("Exec=/usr/bin/ruum %f"));
        assert!(desktop.contains("Exec=/usr/bin/ruum --disk"));
        assert!(desktop.contains("X-AppVersion=0.1.1"));

        let appdir_desktop =
            fs::read_to_string(out.join("AppDir/ruum.desktop")).unwrap();
        assert!(appdir_desktop.contains("Exec=ruum %f"));
        assert!(appdir_desktop.contains("Exec=ruum --disk"));
        assert!(!appdir_desktop.contains("@BINDIR@"));

        let nfpm = fs::read_to_string(out.join("nfpm.yaml")).unwrap();
        assert!(nfpm.contains("arch: \"arm64\""));
        assert!(nfpm.contains("overrides:"));
        assert!(!nfpm.contains('@'));

        assert!(out.join("AppDir/usr/bin/ruum").is_file());
        assert!(out.join("AppDir/ruum.svg").is_file());
        assert!(out.join("AppDir/.DirIcon").is_file());
        assert!(out.join("AppDir/AppRun").is_file());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let bin_mode = fs::metadata(out.join("AppDir/usr/bin/ruum"))
                .unwrap()
                .permissions()
                .mode();
            let apprun_mode = fs::metadata(out.join("AppDir/AppRun"))
                .unwrap()
                .permissions()
                .mode();
            assert_ne!(bin_mode & 0o111, 0);
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
            version: "0.1.1-beta.1".into(),
        })
        .unwrap();
        let wxs_x64 = fs::read_to_string(out_x64.join("ruum.wxs")).unwrap();
        assert!(wxs_x64.contains("Version=\"0.1.1\""));
        assert!(wxs_x64.contains("Scope=\"perMachine\""));
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
        assert!(!iss_x64.contains('@'));

        let out_arm = tmp.path().join("arm64");
        stage_windows(&WindowsStageOptions {
            arch: Arch::Aarch64,
            out: out_arm.clone(),
            exe: None,
            version: "0.1.1".into(),
        })
        .unwrap();
        let iss_arm = fs::read_to_string(out_arm.join("ruum.iss")).unwrap();
        assert!(iss_arm.contains("ArchitecturesAllowed=arm64"));
        assert!(iss_arm.contains("ArchitecturesInstallIn64BitMode=arm64"));
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
            version: "0.1.1".into(),
        })
        .unwrap();
        let wxs = fs::read_to_string(tricky.join("ruum.wxs")).unwrap();
        assert!(wxs.contains("a&amp;b"));
    }
}
