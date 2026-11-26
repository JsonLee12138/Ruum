//! Tri-platform application & CLI self-update engine (`#[cfg(feature = "service")]`).
//!
//! Encapsulates install-kind detection, Ed25519 manifest signature
//! verification, streaming SHA-256 asset downloads with GitHub fallback, and
//! desktop self-update execution behind [`UpdateAdapter`] and
//! [`UpdateFactory`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::ai_cli::CommandRunner;
use crate::cli_install::{CliInstallOs, resolve_cli_binary_for};
use crate::storage::UpdateChannel;
use crate::watchdog_service::{
    CommandServiceRunner, ServiceAdapterFactory, ServiceOs, ServiceStatus,
};

/// Default Cloudflare Worker update base URL provisioned in Slice 1.
pub const DEFAULT_UPDATE_BASE_URL: &str =
    "https://ruum-updates.jsonlee12138.com";

/// Default Ed25519 verifying public key (standard base64, 32 bytes).
pub const DEFAULT_UPDATE_PUBLIC_KEY: &str =
    "Jm8/Ndh4zGAobjvWnV/meTVCuhNOCzJbV6/QUVTP0ng=";

/// Default GitHub repository slug used for fallback release manifests.
pub const DEFAULT_GITHUB_REPO: &str = "JsonLee12138/Ruum";

/// Automatic background update check interval (24 hours).
pub const UPDATE_CHECK_INTERVAL_SECS: u64 = 86_400;

const DOWNLOAD_CHUNK_BYTES: usize = 64 * 1024;
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const REQUEST_TIMEOUT_SECS: u64 = 15;
const DOWNLOAD_TIMEOUT_SECS: u64 = 300;

static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

const MACOS_ADMIN_UPDATE_JXA: &str = "\
ObjC.import('Foundation');\
const env = $.NSProcessInfo.processInfo.environment;\
const src = ObjC.unwrap(env.objectForKey('RUUM_APP_SRC'));\
const dst = ObjC.unwrap(env.objectForKey('RUUM_APP_DST'));\
const bak = ObjC.unwrap(env.objectForKey('RUUM_APP_BAK'));\
if (!src || !dst || !bak) { throw new Error('missing RUUM_APP_* env'); }\
const q = (s) => \"'\" + String(s).replace(/'/g, \"'\\\\''\") + \"'\";\
const app = Application.currentApplication();\
app.includeStandardAdditions = true;\
const cmd = 'rm -rf ' + q(bak) + ' && if [ -e ' + q(dst) + ' ]; then mv ' + q(dst) + ' ' + q(bak) + '; fi && ' +\
'if ditto ' + q(src) + ' ' + q(dst) + '; then rm -rf ' + q(bak) + '; else ' +\
'rm -rf ' + q(dst) + '; if [ -e ' + q(bak) + ' ]; then mv ' + q(bak) + ' ' + q(dst) + '; fi; exit 1; fi';\
app.doShellScript(cmd, { administratorPrivileges: true });";

const WIN_MSI_WAIT_AND_INSTALL_PS: &str = "\
$oldPid = [int]$env:RUUM_OLD_PID;\
$msiPath = $env:RUUM_MSI_PATH;\
$guiExe = $env:RUUM_GUI_EXE;\
$deadline = (Get-Date).AddSeconds(30);\
while ($oldPid -gt 0 -and (Get-Date) -lt $deadline) {\
  if (-not (Get-Process -Id $oldPid -ErrorAction SilentlyContinue)) { break }\
  Start-Sleep -Milliseconds 250;\
}\
$proc = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/i', $msiPath, '/passive') -Wait -PassThru;\
if ($proc.ExitCode -eq 0 -and $guiExe -and (Test-Path -LiteralPath $guiExe)) {\
  Start-Process -FilePath $guiExe;\
}";

/// Operating system family for update detection and execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateOs {
    MacOs,
    Linux,
    Windows,
}

impl UpdateOs {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }
}

/// CPU architecture for update asset selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateArch {
    Aarch64,
    X86_64,
}

impl UpdateArch {
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_arch = "aarch64") {
            Self::Aarch64
        } else {
            Self::X86_64
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64",
            Self::X86_64 => "x86_64",
        }
    }
}

/// Detected installation form determining update strategy and asset key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    MacAppBundle,
    WindowsInno,
    WindowsMsi,
    WindowsPortable,
    LinuxAppImage,
    LinuxTarball,
    LinuxSystemPackage,
    CliStandalone,
    Development,
}

impl InstallKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MacAppBundle => "mac_app_bundle",
            Self::WindowsInno => "windows_inno",
            Self::WindowsMsi => "windows_msi",
            Self::WindowsPortable => "windows_portable",
            Self::LinuxAppImage => "linux_appimage",
            Self::LinuxTarball => "linux_tarball",
            Self::LinuxSystemPackage => "linux_system_package",
            Self::CliStandalone => "cli_standalone",
            Self::Development => "development",
        }
    }

    #[must_use]
    pub const fn pkg_param(self) -> &'static str {
        match self {
            Self::MacAppBundle => "app_zip",
            Self::WindowsInno => "inno",
            Self::WindowsMsi => "msi",
            Self::WindowsPortable => "portable_zip",
            Self::LinuxAppImage => "appimage",
            Self::LinuxTarball => "tarball",
            Self::LinuxSystemPackage => "system_pkg",
            Self::CliStandalone => "cli",
            Self::Development => "dev",
        }
    }

    /// Resolve the canonical manifest asset key for `(os, arch, self)`.
    #[must_use]
    pub fn asset_key(self, os: UpdateOs, arch: UpdateArch) -> Option<String> {
        let arch_s = arch.as_str();
        match (os, self) {
            (
                UpdateOs::MacOs,
                Self::MacAppBundle | Self::CliStandalone | Self::Development,
            ) => Some(format!("macos-{arch_s}-app_zip")),
            (UpdateOs::Windows, Self::WindowsInno) => {
                Some(format!("windows-{arch_s}-inno"))
            }
            (UpdateOs::Windows, Self::WindowsMsi) => {
                Some(format!("windows-{arch_s}-msi"))
            }
            (
                UpdateOs::Windows,
                Self::WindowsPortable | Self::CliStandalone | Self::Development,
            ) => Some(format!("windows-{arch_s}-portable_zip")),
            (UpdateOs::Linux, Self::LinuxAppImage) => {
                Some(format!("linux-{arch_s}-appimage"))
            }
            (
                UpdateOs::Linux,
                Self::LinuxTarball
                | Self::LinuxSystemPackage
                | Self::CliStandalone
                | Self::Development,
            ) => Some(format!("linux-{arch_s}-tarball")),
            _ => None,
        }
    }
}

/// Captured environment inputs for deterministic cross-OS install detection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateEnv {
    pub os: Option<UpdateOs>,
    pub arch: Option<UpdateArch>,
    pub exe_path: PathBuf,
    pub appimage_env: Option<PathBuf>,
    pub appdir_env: Option<PathBuf>,
    pub ld_library_path: Option<String>,
    pub inno_installed: Option<bool>,
    pub msi_installed: Option<bool>,
    pub is_cli_binary: bool,
    pub home: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub program_files: Option<PathBuf>,
    pub writable_override: Option<bool>,
    pub linux_os_release: Option<String>,
}

fn non_empty_path_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn exe_is_cli_binary(exe_path: &Path) -> bool {
    exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("ruum")
                || name.eq_ignore_ascii_case("ruum.exe")
        })
}

impl UpdateEnv {
    /// Capture the current process environment once for update operations.
    #[must_use]
    pub fn current() -> Self {
        let exe_path = std::env::current_exe().unwrap_or_default();
        let is_cli_binary = exe_is_cli_binary(&exe_path);
        let linux_os_release = if cfg!(target_os = "linux") {
            fs::read_to_string("/etc/os-release").ok()
        } else {
            None
        };
        Self {
            os: Some(UpdateFactory::current_os()),
            arch: Some(UpdateArch::current()),
            exe_path,
            appimage_env: non_empty_path_env("APPIMAGE"),
            appdir_env: non_empty_path_env("APPDIR"),
            ld_library_path: std::env::var("LD_LIBRARY_PATH")
                .ok()
                .filter(|s| !s.is_empty()),
            inno_installed: None,
            msi_installed: None,
            is_cli_binary,
            home: std::env::home_dir(),
            local_appdata: non_empty_path_env("LOCALAPPDATA"),
            program_files: non_empty_path_env("ProgramFiles"),
            writable_override: None,
            linux_os_release,
        }
    }
}

/// Asset entry inside [`Manifest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestAsset {
    pub filename: String,
    pub path: String,
    pub fallback_url: String,
    pub sha256: String,
    pub size: u64,
}

/// Signed release manifest (`schema_version = 1`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub version: String,
    pub tag: String,
    pub channel: String,
    pub published_at: String,
    pub release_url: String,
    pub notes: String,
    pub assets: BTreeMap<String, ManifestAsset>,
}

/// Result of applying (or instructing) an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    RestartRequired {
        message: String,
        spawned_relaunch: bool,
    },
    InstallerLaunched {
        message: String,
    },
    ManualActionRequired {
        command: String,
        url: Option<String>,
    },
}

trait DetachedSpawner: Send + Sync {
    fn spawn_detached(
        &self,
        program: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
        remove_envs: &[&str],
        hide_window: bool,
    ) -> io::Result<()>;
}

struct DefaultDetachedSpawner;

impl DetachedSpawner for DefaultDetachedSpawner {
    fn spawn_detached(
        &self,
        program: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
        remove_envs: &[&str],
        hide_window: bool,
    ) -> io::Result<()> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for key in remove_envs {
            cmd.env_remove(key);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        #[cfg(unix)]
        {
            let _ = hide_window;
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        }
        #[cfg(windows)]
        {
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let flags = if hide_window {
                CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
            } else {
                CREATE_NEW_PROCESS_GROUP
            };
            std::os::windows::process::CommandExt::creation_flags(
                &mut cmd, flags,
            );
        }
        let _child = cmd.spawn()?;
        Ok(())
    }
}

/// OS-specific update detection and application adapter.
pub trait UpdateAdapter: Send + Sync + fmt::Debug {
    fn os_kind(&self) -> UpdateOs;
    fn detect_install_kind(&self, env: &UpdateEnv) -> InstallKind;
    fn can_self_update(&self, env: &UpdateEnv, kind: InstallKind) -> bool;
    fn recommended_upgrade_command(
        &self,
        env: &UpdateEnv,
        kind: InstallKind,
        base_url: &str,
        manifest: Option<&Manifest>,
    ) -> String;
    fn apply_update(
        &self,
        env: &UpdateEnv,
        kind: InstallKind,
        downloaded_file: &Path,
        manifest: &Manifest,
        runner: &dyn CommandRunner,
    ) -> io::Result<ApplyOutcome>;
}

/// macOS update adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsUpdateAdapter;

impl MacOsUpdateAdapter {
    fn app_bundle_root(exe_path: &Path) -> Option<PathBuf> {
        let canonical = dunce::canonicalize(exe_path)
            .unwrap_or_else(|_| exe_path.to_path_buf());
        let segments = normalized_segments(&canonical);
        for (idx, seg) in segments.iter().enumerate() {
            if seg.eq_ignore_ascii_case("ruum.app")
                && segments
                    .get(idx + 1)
                    .is_some_and(|s| s.eq_ignore_ascii_case("Contents"))
                && segments
                    .get(idx + 2)
                    .is_some_and(|s| s.eq_ignore_ascii_case("MacOS"))
            {
                let mut cur = canonical.as_path();
                loop {
                    if cur
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case("ruum.app"))
                    {
                        return Some(cur.to_path_buf());
                    }
                    cur = cur.parent()?;
                }
            }
        }
        None
    }

    fn apply_with_spawner(
        self,
        env: &UpdateEnv,
        kind: InstallKind,
        downloaded_file: &Path,
        manifest: &Manifest,
        runner: &dyn CommandRunner,
        spawner: &dyn DetachedSpawner,
    ) -> io::Result<ApplyOutcome> {
        let base_url = resolve_update_base_url();
        if kind != InstallKind::MacAppBundle {
            return Ok(manual_action_outcome(
                &self, env, kind, &base_url, manifest,
            ));
        }

        let current_app =
            Self::app_bundle_root(&env.exe_path).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "cannot locate ruum.app ancestor for {}",
                        env.exe_path.display()
                    ),
                )
            })?;
        let app_parent = current_app.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "ruum.app has no parent directory",
            )
        })?;
        let writable = is_dir_writable(app_parent, env.writable_override);

        let stage_dir = if writable {
            tempfile::Builder::new()
                .prefix(".ruum-update-stage-")
                .tempdir_in(app_parent)?
        } else {
            tempfile::Builder::new()
                .prefix("ruum-update-stage-")
                .tempdir()?
        };

        let zip_str = downloaded_file.to_string_lossy();
        let stage_str = stage_dir.path().to_string_lossy();
        let extract_out = runner.run(
            Path::new("ditto"),
            &["-xk", zip_str.as_ref(), stage_str.as_ref()],
            &[],
        )?;
        if !extract_out.success() {
            return Err(io::Error::other(format!(
                "ditto extract failed: {}",
                extract_out.stderr.trim()
            )));
        }

        let new_app = locate_app_bundle(stage_dir.path()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "extracted archive does not contain ruum.app",
            )
        })?;
        validate_macos_app_bundle(&new_app)?;

        if current_app.join("Contents/_CodeSignature").exists() {
            let new_app_str = new_app.to_string_lossy();
            let cs_out = runner.run(
                Path::new("codesign"),
                &["--verify", "--deep", "--strict", new_app_str.as_ref()],
                &[],
            )?;
            if !cs_out.success() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "codesign verification failed: {}",
                        cs_out.stderr.trim()
                    ),
                ));
            }
        }

        let pid_path = default_watchdog_pid_path_for(
            UpdateOs::MacOs,
            env.home.as_deref(),
            None,
            None,
        );
        let was_running = stop_running_watchdog_with(
            UpdateOs::MacOs,
            pid_path.as_deref(),
            env.home.as_deref(),
            None,
            runner,
        );

        let backup_app = app_parent.join("ruum.app.bak");
        let replace_res = if writable {
            replace_macos_app_writable(
                &new_app,
                &current_app,
                &backup_app,
                runner,
            )
        } else {
            let new_str = new_app.to_string_lossy();
            let dst_str = current_app.to_string_lossy();
            let bak_str = backup_app.to_string_lossy();
            runner
                .run(
                    Path::new("osascript"),
                    &["-l", "JavaScript", "-e", MACOS_ADMIN_UPDATE_JXA],
                    &[
                        ("RUUM_APP_SRC", new_str.as_ref()),
                        ("RUUM_APP_DST", dst_str.as_ref()),
                        ("RUUM_APP_BAK", bak_str.as_ref()),
                    ],
                )
                .and_then(|out| {
                    if out.success() {
                        Ok(())
                    } else {
                        Err(io::Error::other(format!(
                            "osascript app update failed: {}",
                            out.stderr.trim()
                        )))
                    }
                })
        };

        if let Err(err) = replace_res {
            if was_running {
                let watchdog_bin = current_app.join("Contents/MacOS/ruum");
                restart_watchdog_with(
                    UpdateOs::MacOs,
                    &watchdog_bin,
                    env.home.as_deref(),
                    None,
                    runner,
                );
            }
            return Err(err);
        }

        if was_running {
            let watchdog_bin = current_app.join("Contents/MacOS/ruum");
            restart_watchdog_with(
                UpdateOs::MacOs,
                &watchdog_bin,
                env.home.as_deref(),
                None,
                runner,
            );
        }

        let app_str = current_app.to_string_lossy();
        let spawned_relaunch = spawner
            .spawn_detached(
                Path::new("open"),
                &["-n", app_str.as_ref()],
                &[],
                &[],
                false,
            )
            .is_ok();

        Ok(ApplyOutcome::RestartRequired {
            message: format!(
                "Updated ruum.app to v{}; restarting...",
                manifest.version
            ),
            spawned_relaunch,
        })
    }
}

impl UpdateAdapter for MacOsUpdateAdapter {
    fn os_kind(&self) -> UpdateOs {
        UpdateOs::MacOs
    }

    fn detect_install_kind(&self, env: &UpdateEnv) -> InstallKind {
        let canonical = dunce::canonicalize(&env.exe_path)
            .unwrap_or_else(|_| env.exe_path.clone());
        if is_dev_build_path(&canonical) || is_dev_build_path(&env.exe_path) {
            return InstallKind::Development;
        }
        if Self::app_bundle_root(&canonical).is_some() {
            return InstallKind::MacAppBundle;
        }
        InstallKind::CliStandalone
    }

    fn can_self_update(&self, _env: &UpdateEnv, kind: InstallKind) -> bool {
        kind == InstallKind::MacAppBundle
    }

    fn recommended_upgrade_command(
        &self,
        _env: &UpdateEnv,
        kind: InstallKind,
        base_url: &str,
        manifest: Option<&Manifest>,
    ) -> String {
        recommended_upgrade_command_for(
            UpdateOs::MacOs,
            None,
            kind,
            base_url,
            manifest,
        )
    }

    fn apply_update(
        &self,
        env: &UpdateEnv,
        kind: InstallKind,
        downloaded_file: &Path,
        manifest: &Manifest,
        runner: &dyn CommandRunner,
    ) -> io::Result<ApplyOutcome> {
        self.apply_with_spawner(
            env,
            kind,
            downloaded_file,
            manifest,
            runner,
            &DefaultDetachedSpawner,
        )
    }
}

/// Linux update adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxUpdateAdapter;

impl LinuxUpdateAdapter {
    fn has_desktop_entry(env: &UpdateEnv) -> bool {
        if let Some(home) = &env.home
            && home
                .join(".local/share/applications/ruum.desktop")
                .is_file()
        {
            return true;
        }
        if let Some(bin_dir) = env.exe_path.parent()
            && let Some(prefix) = bin_dir.parent()
            && prefix.join("share/applications/ruum.desktop").is_file()
        {
            return true;
        }
        false
    }

    fn apply_with_spawner(
        self,
        env: &UpdateEnv,
        kind: InstallKind,
        downloaded_file: &Path,
        manifest: &Manifest,
        runner: &dyn CommandRunner,
        spawner: &dyn DetachedSpawner,
    ) -> io::Result<ApplyOutcome> {
        let base_url = resolve_update_base_url();
        match kind {
            InstallKind::LinuxAppImage => {
                let Some(appimage_path) = env.appimage_env.as_deref() else {
                    return Ok(manual_action_outcome(
                        &self, env, kind, &base_url, manifest,
                    ));
                };
                let Some(parent) = appimage_path.parent() else {
                    return Ok(manual_action_outcome(
                        &self, env, kind, &base_url, manifest,
                    ));
                };
                if !is_dir_writable(parent, env.writable_override) {
                    return Ok(manual_action_outcome(
                        &self, env, kind, &base_url, manifest,
                    ));
                }

                let pid_path = default_watchdog_pid_path_for(
                    UpdateOs::Linux,
                    env.home.as_deref(),
                    None,
                    None,
                );
                let was_running = stop_running_watchdog_with(
                    UpdateOs::Linux,
                    pid_path.as_deref(),
                    env.home.as_deref(),
                    None,
                    runner,
                );

                let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
                let tmp_path = parent.join(format!(
                    ".ruum.AppImage.new.{}.{seq}",
                    std::process::id()
                ));
                let replace_res = (|| -> io::Result<()> {
                    fs::copy(downloaded_file, &tmp_path)?;
                    set_executable_mode(&tmp_path)?;
                    fs::rename(&tmp_path, appimage_path)?;
                    Ok(())
                })();
                if let Err(err) = replace_res {
                    let _ = fs::remove_file(&tmp_path);
                    if was_running {
                        restart_watchdog_with(
                            UpdateOs::Linux,
                            appimage_path,
                            env.home.as_deref(),
                            None,
                            runner,
                        );
                    }
                    return Err(err);
                }

                if was_running {
                    restart_watchdog_with(
                        UpdateOs::Linux,
                        appimage_path,
                        env.home.as_deref(),
                        None,
                        runner,
                    );
                }

                let (relaunch_envs, remove_envs) = appimage_relaunch_env(env);
                let env_refs: Vec<(&str, &str)> = relaunch_envs
                    .iter()
                    .map(|(k, v)| (*k, v.as_str()))
                    .collect();
                let spawned_relaunch = spawner
                    .spawn_detached(
                        appimage_path,
                        &[],
                        &env_refs,
                        &remove_envs,
                        false,
                    )
                    .is_ok();
                Ok(ApplyOutcome::RestartRequired {
                    message: format!(
                        "Updated AppImage to v{}; restarting...",
                        manifest.version
                    ),
                    spawned_relaunch,
                })
            }
            InstallKind::LinuxTarball => {
                let Some(exe_dir) = env.exe_path.parent() else {
                    return Ok(manual_action_outcome(
                        &self, env, kind, &base_url, manifest,
                    ));
                };
                if !is_dir_writable(exe_dir, env.writable_override) {
                    return Ok(manual_action_outcome(
                        &self, env, kind, &base_url, manifest,
                    ));
                }

                let stage = tempfile::Builder::new()
                    .prefix("ruum-tar-stage-")
                    .tempdir()?;
                let tar_str = downloaded_file.to_string_lossy();
                let stage_str = stage.path().to_string_lossy();
                let tar_out = runner.run(
                    Path::new("tar"),
                    &["-xzf", tar_str.as_ref(), "-C", stage_str.as_ref()],
                    &[],
                )?;
                if !tar_out.success() {
                    return Err(io::Error::other(format!(
                        "tar extract failed: {}",
                        tar_out.stderr.trim()
                    )));
                }

                let src_dir =
                    locate_tarball_root(stage.path()).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "tarball does not contain `ruum` binary",
                        )
                    })?;

                let pid_path = default_watchdog_pid_path_for(
                    UpdateOs::Linux,
                    env.home.as_deref(),
                    None,
                    None,
                );
                let was_running = stop_running_watchdog_with(
                    UpdateOs::Linux,
                    pid_path.as_deref(),
                    env.home.as_deref(),
                    None,
                    runner,
                );

                // Replace binaries atomically via temp file + rename first so
                // running inodes are unlinked (preventing ETXTBSY and keeping
                // the embedded ruum-scan trailer intact).
                let update_res = (|| -> io::Result<()> {
                    let src_cli = src_dir.join("ruum");
                    let dst_cli = exe_dir.join("ruum");
                    if src_cli.is_file() {
                        atomic_install_file(&src_cli, &dst_cli, true)?;
                    }
                    let src_gui = src_dir.join("ruum-gui");
                    let dst_gui = exe_dir.join("ruum-gui");
                    if src_gui.is_file() {
                        atomic_install_file(&src_gui, &dst_gui, true)?;
                    }
                    let install_sh = src_dir.join("install.sh");
                    if install_sh.is_file() {
                        let prefix_dir = exe_dir.parent().unwrap_or(exe_dir);
                        let bin_str = exe_dir.to_string_lossy();
                        let prefix_str = prefix_dir.to_string_lossy();
                        let sh_str = install_sh.to_string_lossy();
                        let out = runner.run(
                            Path::new("sh"),
                            &[sh_str.as_ref()],
                            &[
                                ("BINDIR", bin_str.as_ref()),
                                ("PREFIX", prefix_str.as_ref()),
                            ],
                        )?;
                        if !out.success() {
                            return Err(io::Error::other(format!(
                                "tarball install.sh failed: {}",
                                out.stderr.trim()
                            )));
                        }
                    }
                    Ok(())
                })();

                if let Err(err) = update_res {
                    if was_running {
                        restart_watchdog_with(
                            UpdateOs::Linux,
                            &exe_dir.join("ruum"),
                            env.home.as_deref(),
                            None,
                            runner,
                        );
                    }
                    return Err(err);
                }

                if was_running {
                    restart_watchdog_with(
                        UpdateOs::Linux,
                        &exe_dir.join("ruum"),
                        env.home.as_deref(),
                        None,
                        runner,
                    );
                }

                let relaunch_target = if exe_dir.join("ruum-gui").is_file() {
                    exe_dir.join("ruum-gui")
                } else {
                    env.exe_path.clone()
                };
                let spawned_relaunch = spawner
                    .spawn_detached(&relaunch_target, &[], &[], &[], false)
                    .is_ok();

                Ok(ApplyOutcome::RestartRequired {
                    message: format!(
                        "Updated ruum to v{}; restarting...",
                        manifest.version
                    ),
                    spawned_relaunch,
                })
            }
            InstallKind::LinuxSystemPackage
            | InstallKind::CliStandalone
            | InstallKind::Development
            | InstallKind::MacAppBundle
            | InstallKind::WindowsInno
            | InstallKind::WindowsMsi
            | InstallKind::WindowsPortable => {
                Ok(manual_action_outcome(&self, env, kind, &base_url, manifest))
            }
        }
    }
}

enum LdLibraryPathAction {
    KeepUnchanged,
    Set(String),
    Remove,
}

fn normalize_posix_path_str(raw: &str) -> String {
    let replaced = raw.replace('\\', "/");
    let trimmed = replaced.trim_end_matches('/');
    if trimmed.is_empty() && replaced.starts_with('/') {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

// Build `$APPDIR/usr/lib` using pure `/` POSIX semantics rather than host
// `Path::join` so Windows hosts never inject `\` when matching Linux
// `LD_LIBRARY_PATH` entries.
fn appdir_stale_lib_dir(appdir: &Path) -> Option<String> {
    let base = normalize_posix_path_str(&appdir.to_string_lossy());
    if base.is_empty() {
        None
    } else if base == "/" {
        Some("/usr/lib".to_owned())
    } else {
        Some(format!("{base}/usr/lib"))
    }
}

fn clean_appdir_ld_library_path(
    raw: &str,
    appdir: &Path,
) -> LdLibraryPathAction {
    let Some(stale_dir) = appdir_stale_lib_dir(appdir) else {
        return LdLibraryPathAction::KeepUnchanged;
    };
    let mut found_stale = false;
    let mut kept = Vec::new();
    for entry in raw.split(':') {
        if entry.is_empty() {
            continue;
        }
        if normalize_posix_path_str(entry) == stale_dir {
            found_stale = true;
        } else {
            kept.push(entry);
        }
    }
    if !found_stale {
        LdLibraryPathAction::KeepUnchanged
    } else if kept.is_empty() {
        LdLibraryPathAction::Remove
    } else {
        LdLibraryPathAction::Set(kept.join(":"))
    }
}

fn appimage_relaunch_env(
    env: &UpdateEnv,
) -> (Vec<(&'static str, String)>, Vec<&'static str>) {
    let mut set_envs = Vec::new();
    let mut remove_envs = vec!["APPDIR", "APPIMAGE", "ARGV0", "OWD"];
    if let (Some(appdir), Some(raw_ld)) =
        (env.appdir_env.as_deref(), env.ld_library_path.as_deref())
    {
        match clean_appdir_ld_library_path(raw_ld, appdir) {
            LdLibraryPathAction::KeepUnchanged => {}
            LdLibraryPathAction::Set(cleaned) => {
                set_envs.push(("LD_LIBRARY_PATH", cleaned));
            }
            LdLibraryPathAction::Remove => {
                remove_envs.push("LD_LIBRARY_PATH");
            }
        }
    }
    (set_envs, remove_envs)
}

impl UpdateAdapter for LinuxUpdateAdapter {
    fn os_kind(&self) -> UpdateOs {
        UpdateOs::Linux
    }

    fn detect_install_kind(&self, env: &UpdateEnv) -> InstallKind {
        if let Some(appimage) = &env.appimage_env
            && appimage.is_file()
        {
            return InstallKind::LinuxAppImage;
        }
        if is_dev_build_path(&env.exe_path) {
            return InstallKind::Development;
        }
        let segments = normalized_segments(&env.exe_path);
        if segments.len() >= 2
            && segments[0] == "usr"
            && (segments[1] == "bin" || segments[1] == "lib")
        {
            return InstallKind::LinuxSystemPackage;
        }
        if !env.is_cli_binary {
            return InstallKind::LinuxTarball;
        }
        if let Some(parent) = env.exe_path.parent()
            && parent.join("ruum-gui").is_file()
        {
            return InstallKind::LinuxTarball;
        }
        if Self::has_desktop_entry(env) {
            return InstallKind::LinuxTarball;
        }
        InstallKind::CliStandalone
    }

    fn can_self_update(&self, env: &UpdateEnv, kind: InstallKind) -> bool {
        match kind {
            InstallKind::LinuxAppImage => env
                .appimage_env
                .as_deref()
                .and_then(Path::parent)
                .is_some_and(|p| is_dir_writable(p, env.writable_override)),
            InstallKind::LinuxTarball => env
                .exe_path
                .parent()
                .is_some_and(|p| is_dir_writable(p, env.writable_override)),
            _ => false,
        }
    }

    fn recommended_upgrade_command(
        &self,
        env: &UpdateEnv,
        kind: InstallKind,
        base_url: &str,
        manifest: Option<&Manifest>,
    ) -> String {
        recommended_upgrade_command_for(
            UpdateOs::Linux,
            env.linux_os_release.as_deref(),
            kind,
            base_url,
            manifest,
        )
    }

    fn apply_update(
        &self,
        env: &UpdateEnv,
        kind: InstallKind,
        downloaded_file: &Path,
        manifest: &Manifest,
        runner: &dyn CommandRunner,
    ) -> io::Result<ApplyOutcome> {
        self.apply_with_spawner(
            env,
            kind,
            downloaded_file,
            manifest,
            runner,
            &DefaultDetachedSpawner,
        )
    }
}

/// Windows update adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsUpdateAdapter;

impl WindowsUpdateAdapter {
    fn cli_bin_dir(env: &UpdateEnv) -> Option<PathBuf> {
        env.local_appdata
            .as_deref()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| {
                env.home.as_ref().map(|h| h.join("AppData").join("Local"))
            })
            .map(|base| base.join("Programs").join("ruum").join("bin"))
    }

    fn is_cli_standalone_path(env: &UpdateEnv) -> bool {
        if !env.is_cli_binary {
            return false;
        }
        if let Some(bin_dir) = Self::cli_bin_dir(env)
            && let Some(exe_parent) = env.exe_path.parent()
            && windows_paths_equal(exe_parent, &bin_dir)
        {
            return true;
        }
        let norm = normalize_windows_str(&env.exe_path);
        norm.to_ascii_lowercase()
            .ends_with(r"\programs\ruum\bin\ruum.exe")
    }

    fn is_under_program_files(env: &UpdateEnv) -> bool {
        if let Some(pf) = &env.program_files {
            let pf_norm = normalize_windows_str(pf).to_ascii_lowercase();
            let exe_norm =
                normalize_windows_str(&env.exe_path).to_ascii_lowercase();
            let prefix = format!("{}\\ruum\\", pf_norm.trim_end_matches('\\'));
            if exe_norm.starts_with(&prefix) {
                return true;
            }
        }
        false
    }

    #[cfg(windows)]
    fn probe_inno_registry(exe_path: &Path) -> bool {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let subkey = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{E8F4B9A2-6D1C-4B3A-9E7F-2C5A1D8E4B60}_is1";
        let exe_dir = exe_path
            .parent()
            .map(normalize_windows_str)
            .unwrap_or_default();
        for hive in ["HKCU", "HKLM"] {
            let full = format!("{hive}\\{subkey}");
            let Ok(out) = Command::new("reg.exe")
                .args(["query", &full, "/v", "InstallLocation"])
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .output()
            else {
                continue;
            };
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if exe_dir.is_empty()
                    || text
                        .to_ascii_lowercase()
                        .contains(&exe_dir.to_ascii_lowercase())
                {
                    return true;
                }
            }
        }
        false
    }

    #[cfg(not(windows))]
    const fn probe_inno_registry(_exe_path: &Path) -> bool {
        false
    }

    #[cfg(windows)]
    fn probe_msi_registry() -> bool {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("reg.exe")
            .args([
                "query",
                r"HKLM\Software\JsonLee12138\ruum",
                "/v",
                "installed",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[cfg(not(windows))]
    const fn probe_msi_registry() -> bool {
        false
    }

    fn apply_with_spawner(
        self,
        env: &UpdateEnv,
        kind: InstallKind,
        downloaded_file: &Path,
        manifest: &Manifest,
        runner: &dyn CommandRunner,
        spawner: &dyn DetachedSpawner,
    ) -> io::Result<ApplyOutcome> {
        let base_url = resolve_update_base_url();
        match kind {
            InstallKind::WindowsInno => {
                let pid_path = default_watchdog_pid_path_for(
                    UpdateOs::Windows,
                    env.home.as_deref(),
                    None,
                    env.local_appdata.as_deref(),
                );
                let was_running = stop_running_watchdog_with(
                    UpdateOs::Windows,
                    pid_path.as_deref(),
                    env.home.as_deref(),
                    env.local_appdata.as_deref(),
                    runner,
                );
                if let Err(err) = spawner.spawn_detached(
                    downloaded_file,
                    &["/SILENT", "/CLOSEAPPLICATIONS", "/RESTARTAPPLICATIONS"],
                    &[],
                    &[],
                    false,
                ) {
                    if was_running {
                        let cli_exe = env.exe_path.parent().map_or_else(
                            || env.exe_path.clone(),
                            |p| p.join("ruum.exe"),
                        );
                        restart_watchdog_with(
                            UpdateOs::Windows,
                            &cli_exe,
                            env.home.as_deref(),
                            env.local_appdata.as_deref(),
                            runner,
                        );
                    }
                    return Err(err);
                }
                Ok(ApplyOutcome::InstallerLaunched {
                    message: format!(
                        "Launched Inno installer for v{}; please wait for restart.",
                        manifest.version
                    ),
                })
            }
            InstallKind::WindowsMsi => {
                let pid_path = default_watchdog_pid_path_for(
                    UpdateOs::Windows,
                    env.home.as_deref(),
                    None,
                    env.local_appdata.as_deref(),
                );
                let was_running = stop_running_watchdog_with(
                    UpdateOs::Windows,
                    pid_path.as_deref(),
                    env.home.as_deref(),
                    env.local_appdata.as_deref(),
                    runner,
                );
                let pid_str = std::process::id().to_string();
                let msi_str = downloaded_file.to_string_lossy();
                let gui_exe = env.exe_path.parent().map_or_else(
                    || env.exe_path.clone(),
                    |p| p.join("ruum-gui.exe"),
                );
                let gui_str = gui_exe.to_string_lossy();
                if let Err(err) = spawner.spawn_detached(
                    Path::new("powershell.exe"),
                    &[
                        "-NoProfile",
                        "-NonInteractive",
                        "-WindowStyle",
                        "Hidden",
                        "-Command",
                        WIN_MSI_WAIT_AND_INSTALL_PS,
                    ],
                    &[
                        ("RUUM_OLD_PID", pid_str.as_str()),
                        ("RUUM_MSI_PATH", msi_str.as_ref()),
                        ("RUUM_GUI_EXE", gui_str.as_ref()),
                    ],
                    &[],
                    true,
                ) {
                    if was_running {
                        let cli_exe = env.exe_path.parent().map_or_else(
                            || env.exe_path.clone(),
                            |p| p.join("ruum.exe"),
                        );
                        restart_watchdog_with(
                            UpdateOs::Windows,
                            &cli_exe,
                            env.home.as_deref(),
                            env.local_appdata.as_deref(),
                            runner,
                        );
                    }
                    return Err(err);
                }
                Ok(ApplyOutcome::InstallerLaunched {
                    message: format!(
                        "Launched MSI installer for v{}; restarting when complete.",
                        manifest.version
                    ),
                })
            }
            InstallKind::WindowsPortable
            | InstallKind::CliStandalone
            | InstallKind::Development
            | InstallKind::MacAppBundle
            | InstallKind::LinuxAppImage
            | InstallKind::LinuxTarball
            | InstallKind::LinuxSystemPackage => {
                Ok(manual_action_outcome(&self, env, kind, &base_url, manifest))
            }
        }
    }
}

impl UpdateAdapter for WindowsUpdateAdapter {
    fn os_kind(&self) -> UpdateOs {
        UpdateOs::Windows
    }

    fn detect_install_kind(&self, env: &UpdateEnv) -> InstallKind {
        if is_dev_build_path(&env.exe_path) {
            return InstallKind::Development;
        }
        if Self::is_cli_standalone_path(env) {
            return InstallKind::CliStandalone;
        }
        match env.inno_installed {
            Some(true) => return InstallKind::WindowsInno,
            None if Self::probe_inno_registry(&env.exe_path) => {
                return InstallKind::WindowsInno;
            }
            _ => {}
        }
        match env.msi_installed {
            Some(true) => return InstallKind::WindowsMsi,
            None if Self::probe_msi_registry()
                || Self::is_under_program_files(env) =>
            {
                return InstallKind::WindowsMsi;
            }
            _ => {}
        }
        InstallKind::WindowsPortable
    }

    fn can_self_update(&self, _env: &UpdateEnv, kind: InstallKind) -> bool {
        matches!(kind, InstallKind::WindowsInno | InstallKind::WindowsMsi)
    }

    fn recommended_upgrade_command(
        &self,
        _env: &UpdateEnv,
        kind: InstallKind,
        base_url: &str,
        manifest: Option<&Manifest>,
    ) -> String {
        recommended_upgrade_command_for(
            UpdateOs::Windows,
            None,
            kind,
            base_url,
            manifest,
        )
    }

    fn apply_update(
        &self,
        env: &UpdateEnv,
        kind: InstallKind,
        downloaded_file: &Path,
        manifest: &Manifest,
        runner: &dyn CommandRunner,
    ) -> io::Result<ApplyOutcome> {
        self.apply_with_spawner(
            env,
            kind,
            downloaded_file,
            manifest,
            runner,
            &DefaultDetachedSpawner,
        )
    }
}

/// Factory selecting the [`UpdateAdapter`] for the host or a requested OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateFactory;

impl UpdateFactory {
    /// Detect the current operating system family.
    #[must_use]
    pub const fn current_os() -> UpdateOs {
        if cfg!(target_os = "macos") {
            UpdateOs::MacOs
        } else if cfg!(windows) {
            UpdateOs::Windows
        } else {
            UpdateOs::Linux
        }
    }

    /// Instantiate the adapter for a specific target OS (unconditionally
    /// compiled for cross-OS unit tests).
    #[must_use]
    pub fn create_for(os: UpdateOs) -> Box<dyn UpdateAdapter> {
        match os {
            UpdateOs::MacOs => Box::new(MacOsUpdateAdapter),
            UpdateOs::Linux => Box::new(LinuxUpdateAdapter),
            UpdateOs::Windows => Box::new(WindowsUpdateAdapter),
        }
    }

    /// Instantiate the adapter for the current host OS.
    #[must_use]
    pub fn create() -> Box<dyn UpdateAdapter> {
        Self::create_for(Self::current_os())
    }
}

/// Parameters for checking updates against the update service / GitHub fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateCheckRequest<'a> {
    pub channel: UpdateChannel,
    pub current_version: &'a str,
    pub os: UpdateOs,
    pub arch: UpdateArch,
    pub install_kind: InstallKind,
    pub lang: &'a str,
    pub entry: &'a str,
    pub client_id: Option<&'a str>,
    pub base_url: &'a str,
    pub public_key_b64: &'a str,
    pub github_repo: &'a str,
    pub github_fallback_base: Option<&'a str>,
}

/// Structured result of an update check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub channel: UpdateChannel,
    pub install_kind: InstallKind,
    pub manifest: Option<Manifest>,
    pub selected_asset_key: Option<String>,
    pub selected_asset: Option<ManifestAsset>,
    pub upgrade_command: String,
    pub can_self_update: bool,
    pub used_fallback: bool,
}

/// Resolve the update service base URL with explicit runtime & compile-time
/// inputs (filtering empty/whitespace strings per `[RISK-NOTE 6]`).
#[must_use]
pub fn resolve_update_base_url_with(
    runtime: Option<&str>,
    compile_time: Option<&str>,
) -> String {
    let pick = |opt: Option<&str>| -> Option<String> {
        opt.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_owned())
            .filter(|s| !s.is_empty())
    };
    pick(runtime)
        .or_else(|| pick(compile_time))
        .unwrap_or_else(|| DEFAULT_UPDATE_BASE_URL.to_owned())
}

/// Resolve the update service base URL (`RUUM_UPDATE_BASE_URL` runtime >
/// compile-time > [`DEFAULT_UPDATE_BASE_URL`]).
#[must_use]
pub fn resolve_update_base_url() -> String {
    let runtime = std::env::var("RUUM_UPDATE_BASE_URL").ok();
    resolve_update_base_url_with(
        runtime.as_deref(),
        option_env!("RUUM_UPDATE_BASE_URL"),
    )
}

/// Resolve the Ed25519 verifying key with explicit compile-time input
/// (filtering empty/whitespace strings per `[RISK-NOTE 6]`).
#[must_use]
pub fn resolve_update_public_key_with(compile_time: Option<&str>) -> &str {
    compile_time
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_UPDATE_PUBLIC_KEY)
}

/// Resolve the compile-time or embedded Ed25519 verifying public key.
#[must_use]
pub fn resolve_update_public_key() -> &'static str {
    match option_env!("RUUM_UPDATE_PUBLIC_KEY") {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                DEFAULT_UPDATE_PUBLIC_KEY
            } else {
                trimmed
            }
        }
        None => DEFAULT_UPDATE_PUBLIC_KEY,
    }
}

/// Resolve the verifying key for `base_url`.
///
/// In release builds this always returns [`resolve_update_public_key`]. Only in
/// `debug_assertions` AND when `base_url` points to a local loopback test
/// server (`127.0.0.1` / `localhost`) does `RUUM_TEST_UPDATE_PUBLIC_KEY` apply
/// so E2E binary tests can sign mock manifests cleanly.
#[must_use]
pub fn resolve_effective_public_key_for_url(base_url: &str) -> String {
    #[cfg(debug_assertions)]
    if is_loopback_http_url(base_url)
        && let Ok(test_pk) = std::env::var("RUUM_TEST_UPDATE_PUBLIC_KEY")
    {
        let trimmed = test_pk.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    let _ = base_url;
    resolve_update_public_key().to_owned()
}

/// Pure helper evaluating whether automatic background update checks are
/// disabled.
#[must_use]
pub fn is_auto_check_disabled_with(
    env_val: Option<&str>,
    is_test: bool,
) -> bool {
    if is_test {
        return true;
    }
    env_val.is_some_and(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Return `true` when `RUUM_NO_UPDATE_CHECK` is truthy or running under
/// `cfg!(test)`.
#[must_use]
pub fn is_auto_check_disabled() -> bool {
    let env_val = std::env::var("RUUM_NO_UPDATE_CHECK").ok();
    is_auto_check_disabled_with(env_val.as_deref(), cfg!(test))
}

/// Return `true` when automatic background checking should run now.
#[must_use]
pub fn should_auto_check(
    settings: &crate::storage::UpdateSettings,
    now_secs: u64,
) -> bool {
    !is_auto_check_disabled()
        && settings.auto_check
        && now_secs.saturating_sub(settings.last_checked_at_secs)
            >= UPDATE_CHECK_INTERVAL_SECS
}

/// Verify Ed25519 signature over raw UTF-8 `manifest.json` bytes using
/// `verify_strict`, then deserialize and validate `schema_version == 1`.
pub fn verify_manifest_signature(
    raw_bytes: &[u8],
    sig_b64: &str,
    pubkey_b64: &str,
) -> io::Result<Manifest> {
    let pub_decoded = BASE64.decode(pubkey_b64.trim()).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid base64 public key: {e}"),
        )
    })?;
    let pub_bytes: [u8; 32] =
        pub_decoded.try_into().map_err(|v: Vec<u8>| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Ed25519 public key must be 32 bytes, got {} bytes",
                    v.len()
                ),
            )
        })?;
    let verifying_key = VerifyingKey::from_bytes(&pub_bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Ed25519 public key: {e}"),
        )
    })?;

    let sig_decoded = BASE64.decode(sig_b64.trim()).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid base64 signature: {e}"),
        )
    })?;
    let sig_bytes: [u8; 64] =
        sig_decoded.try_into().map_err(|v: Vec<u8>| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Ed25519 signature must be 64 bytes, got {} bytes",
                    v.len()
                ),
            )
        })?;
    let signature = Signature::from_bytes(&sig_bytes);

    verifying_key
        .verify_strict(raw_bytes, &signature)
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("manifest Ed25519 signature verification failed: {e}"),
            )
        })?;

    let manifest: Manifest =
        serde_json::from_slice(raw_bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid manifest JSON: {e}"),
            )
        })?;
    if manifest.schema_version != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported manifest schema_version {}",
                manifest.schema_version
            ),
        ));
    }
    let _ = parse_semver(&manifest.version)?;
    Ok(manifest)
}

fn parse_semver(raw: &str) -> io::Result<semver::Version> {
    let clean = raw.trim().trim_start_matches('v');
    semver::Version::parse(clean).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid SemVer `{raw}`: {e}"),
        )
    })
}

/// Compare two `SemVer` strings (`0.2.2-beta.1 < 0.2.2 < 0.2.3-beta.1`).
pub fn is_newer_version(
    current_version: &str,
    candidate_version: &str,
) -> io::Result<bool> {
    let cur = parse_semver(current_version)?;
    let cand = parse_semver(candidate_version)?;
    Ok(cand > cur)
}

fn is_loopback_http_url(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw.trim()) else {
        return false;
    };
    if parsed.scheme() != "http" {
        return false;
    }
    matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    )
}

/// Validate that a download/check URL uses `https://` (or loopback `http://`
/// for local tests) and contains no whitespace or control characters.
pub fn validate_update_url(raw: &str) -> io::Result<url::Url> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "update URL must not be empty or contain whitespace/control chars",
        ));
    }
    let parsed = url::Url::parse(trimmed).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid update URL `{trimmed}`: {e}"),
        )
    })?;
    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" if is_loopback_http_url(trimmed) => Ok(parsed),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("insecure or unsupported update URL scheme `{other}`"),
        )),
    }
}

/// Validate a manifest asset `path` (`releases/...` without `..` or control
/// characters).
pub fn validate_asset_path(raw: &str) -> io::Result<&str> {
    let path = raw.trim();
    if !path.starts_with("releases/")
        || path.ends_with('/')
        || path.contains("..")
        || path.contains("//")
        || path.contains('\\')
        || !path.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-')
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid manifest asset path `{raw}`"),
        ));
    }
    Ok(path)
}

/// Validate a saved asset filename against path traversal and control chars.
pub fn validate_safe_filename(raw: &str) -> io::Result<&str> {
    let name = raw.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe asset filename `{raw}`"),
        ));
    }
    Ok(name)
}

fn build_http_agent(timeout_secs: u64) -> ureq::Agent {
    let cfg = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .user_agent(concat!("ruum/", env!("CARGO_PKG_VERSION")))
        .build();
    ureq::Agent::new_with_config(cfg)
}

enum WorkerFetchResult {
    Verified(Manifest),
    NoRelease,
    FallbackEligible(io::Error),
    Fatal(io::Error),
}

#[derive(Deserialize)]
struct WorkerErrorBody {
    error: Option<String>,
}

fn is_no_release_404_body(body: &[u8]) -> bool {
    serde_json::from_slice::<WorkerErrorBody>(body)
        .ok()
        .and_then(|parsed| parsed.error)
        .is_some_and(|err| err.trim() == "manifest not found")
}

fn fetch_worker_manifest(
    agent: &ureq::Agent,
    req: &UpdateCheckRequest<'_>,
) -> WorkerFetchResult {
    let base = req.base_url.trim().trim_end_matches('/');
    let mut check_url = match validate_update_url(&format!("{base}/v1/check")) {
        Ok(u) => u,
        Err(e) => return WorkerFetchResult::Fatal(e),
    };
    {
        let mut qp = check_url.query_pairs_mut();
        qp.append_pair("channel", req.channel.as_str());
        qp.append_pair("os", req.os.as_str());
        qp.append_pair("arch", req.arch.as_str());
        qp.append_pair("pkg", req.install_kind.pkg_param());
        qp.append_pair("ver", req.current_version);
        qp.append_pair("lang", req.lang);
        qp.append_pair("entry", req.entry);
    }

    let mut request = agent.get(check_url.as_str());
    if let Some(cid) = req.client_id.map(str::trim).filter(|s| !s.is_empty()) {
        request = request.header("X-Ruum-Client-Id", cid);
    }

    match request.config().http_status_as_error(false).build().call() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if status == 204 {
                return WorkerFetchResult::NoRelease;
            }
            if status == 404 {
                let body_bytes = resp
                    .into_body()
                    .with_config()
                    .limit(16 * 1024)
                    .read_to_vec()
                    .unwrap_or_default();
                if is_no_release_404_body(&body_bytes) {
                    return WorkerFetchResult::NoRelease;
                }
                return WorkerFetchResult::Fatal(io::Error::other(
                    "worker returned HTTP 404",
                ));
            }
            if (500..=599).contains(&status) {
                return WorkerFetchResult::FallbackEligible(io::Error::other(
                    format!("worker returned HTTP {status}"),
                ));
            }
            if !resp.status().is_success() {
                return WorkerFetchResult::Fatal(io::Error::other(format!(
                    "worker returned HTTP {status}"
                )));
            }
            let Some(sig) = resp
                .headers()
                .get("x-ruum-signature")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
            else {
                return WorkerFetchResult::Fatal(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "worker response missing X-Ruum-Signature header",
                ));
            };
            let raw_bytes = match resp
                .into_body()
                .with_config()
                .limit(MAX_MANIFEST_BYTES)
                .read_to_vec()
            {
                Ok(b) => b,
                Err(e) => {
                    return WorkerFetchResult::FallbackEligible(
                        io::Error::other(format!(
                            "failed reading worker response body: {e}"
                        )),
                    );
                }
            };
            match verify_manifest_signature(
                &raw_bytes,
                &sig,
                req.public_key_b64,
            ) {
                Ok(manifest) => WorkerFetchResult::Verified(manifest),
                Err(e) => WorkerFetchResult::Fatal(e),
            }
        }
        Err(ureq::Error::StatusCode(code)) => {
            let err = io::Error::other(format!("worker returned HTTP {code}"));
            if (500..=599).contains(&code) {
                WorkerFetchResult::FallbackEligible(err)
            } else {
                WorkerFetchResult::Fatal(err)
            }
        }
        Err(ureq::Error::Timeout(reason)) => {
            WorkerFetchResult::FallbackEligible(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("worker request timed out: {reason:?}"),
            ))
        }
        Err(e) => WorkerFetchResult::FallbackEligible(io::Error::other(
            format!("worker request failed: {e}"),
        )),
    }
}

fn fetch_github_fallback_manifest(
    agent: &ureq::Agent,
    req: &UpdateCheckRequest<'_>,
) -> io::Result<Manifest> {
    let gh_base = req
        .github_fallback_base
        .unwrap_or("https://github.com")
        .trim()
        .trim_end_matches('/');
    let manifest_url = validate_update_url(&format!(
        "{gh_base}/{}/releases/latest/download/manifest.json",
        req.github_repo.trim()
    ))?;
    let sig_url = validate_update_url(&format!(
        "{gh_base}/{}/releases/latest/download/manifest.json.sig",
        req.github_repo.trim()
    ))?;

    let manifest_resp =
        agent.get(manifest_url.as_str()).call().map_err(|e| {
            io::Error::other(format!(
                "GitHub fallback manifest fetch failed: {e}"
            ))
        })?;
    let raw_bytes = manifest_resp
        .into_body()
        .with_config()
        .limit(MAX_MANIFEST_BYTES)
        .read_to_vec()
        .map_err(|e| {
            io::Error::other(format!(
                "GitHub fallback manifest read failed: {e}"
            ))
        })?;

    let sig_resp = agent.get(sig_url.as_str()).call().map_err(|e| {
        io::Error::other(format!("GitHub fallback signature fetch failed: {e}"))
    })?;
    let sig_bytes = sig_resp
        .into_body()
        .with_config()
        .limit(4096)
        .read_to_vec()
        .map_err(|e| {
            io::Error::other(format!(
                "GitHub fallback signature read failed: {e}"
            ))
        })?;
    let sig_str = std::str::from_utf8(&sig_bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("GitHub signature is not UTF-8: {e}"),
        )
    })?;

    verify_manifest_signature(&raw_bytes, sig_str, req.public_key_b64)
}

/// Check for updates against the primary Worker, automatically falling back to
/// GitHub Releases on network/5xx failure and verifying the Ed25519 signature.
pub fn check_for_updates(
    req: &UpdateCheckRequest<'_>,
) -> io::Result<UpdateCheckResult> {
    let agent = build_http_agent(REQUEST_TIMEOUT_SECS);
    let env_for_cmd = UpdateEnv {
        os: Some(req.os),
        arch: Some(req.arch),
        ..UpdateEnv::current()
    };
    let adapter = UpdateFactory::create_for(req.os);
    let can_self_update =
        adapter.can_self_update(&env_for_cmd, req.install_kind);

    let (manifest, used_fallback) = match fetch_worker_manifest(&agent, req) {
        WorkerFetchResult::Verified(m) => (m, false),
        WorkerFetchResult::NoRelease => {
            let upgrade_command = adapter.recommended_upgrade_command(
                &env_for_cmd,
                req.install_kind,
                req.base_url,
                None,
            );
            return Ok(UpdateCheckResult {
                current_version: req.current_version.to_owned(),
                latest_version: req.current_version.to_owned(),
                update_available: false,
                channel: req.channel,
                install_kind: req.install_kind,
                manifest: None,
                selected_asset_key: None,
                selected_asset: None,
                upgrade_command,
                can_self_update,
                used_fallback: false,
            });
        }
        WorkerFetchResult::Fatal(e) => return Err(e),
        WorkerFetchResult::FallbackEligible(primary_err) => {
            if req.channel != UpdateChannel::Stable {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "primary update service unavailable for beta channel \
                         ({primary_err})"
                    ),
                ));
            }
            (fetch_github_fallback_manifest(&agent, req)?, true)
        }
    };

    let update_available =
        is_newer_version(req.current_version, &manifest.version)?;
    let selected_asset_key = req.install_kind.asset_key(req.os, req.arch);
    let selected_asset = selected_asset_key
        .as_ref()
        .and_then(|k| manifest.assets.get(k))
        .cloned();
    let upgrade_command = adapter.recommended_upgrade_command(
        &env_for_cmd,
        req.install_kind,
        req.base_url,
        Some(&manifest),
    );

    Ok(UpdateCheckResult {
        current_version: req.current_version.to_owned(),
        latest_version: manifest.version.clone(),
        update_available,
        channel: req.channel,
        install_kind: req.install_kind,
        manifest: Some(manifest),
        selected_asset_key,
        selected_asset,
        upgrade_command,
        can_self_update,
        used_fallback,
    })
}

fn stream_download_single<F>(
    agent: &ureq::Agent,
    url: &url::Url,
    part_path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    cancel: &AtomicBool,
    on_progress: &mut F,
) -> io::Result<()>
where
    F: FnMut(u64, u64),
{
    if cancel.load(Ordering::Relaxed) {
        let _ = fs::remove_file(part_path);
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "download cancelled",
        ));
    }

    let resp = agent.get(url.as_str()).call().map_err(|e| {
        io::Error::other(format!("download request failed: {e}"))
    })?;
    if !resp.status().is_success() {
        return Err(io::Error::other(format!(
            "download returned HTTP {}",
            resp.status().as_u16()
        )));
    }
    let header_len = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let total = if expected_size > 0 {
        expected_size
    } else {
        header_len.unwrap_or(0)
    };

    let _ = fs::remove_file(part_path);
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(part_path)?;

    let mut reader = resp.into_body().into_reader();
    let mut buf = vec![0_u8; DOWNLOAD_CHUNK_BYTES];
    let mut hasher = Sha256::new();
    let mut downloaded = 0_u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = fs::remove_file(part_path);
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "download cancelled",
            ));
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(out);
                let _ = fs::remove_file(part_path);
                return Err(e);
            }
        };
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = fs::remove_file(part_path);
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "download cancelled",
            ));
        }
        if let Err(e) = out.write_all(&buf[..n]) {
            drop(out);
            let _ = fs::remove_file(part_path);
            return Err(e);
        }
        hasher.update(&buf[..n]);
        downloaded = downloaded.saturating_add(n as u64);
        on_progress(downloaded, total);
    }

    out.flush()?;
    drop(out);

    if expected_size > 0 && downloaded != expected_size {
        let _ = fs::remove_file(part_path);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "downloaded size mismatch: expected {expected_size} bytes, \
                 got {downloaded} bytes"
            ),
        ));
    }

    let actual_sha256 = hex_lower(&hasher.finalize());
    if actual_sha256 != expected_sha256 {
        let _ = fs::remove_file(part_path);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "SHA-256 mismatch: expected {expected_sha256}, \
                 got {actual_sha256}"
            ),
        ));
    }

    Ok(())
}

/// Stream-download an asset in 64 KiB chunks into `dest_dir`.
///
/// Falls back to `asset.fallback_url` when the primary URL fails, verifies
/// `size` and `sha256`, and removes `.part` on failure or cancellation.
pub fn download_and_verify_asset<F>(
    base_url: &str,
    asset: &ManifestAsset,
    dest_dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: F,
) -> io::Result<PathBuf>
where
    F: FnMut(u64, u64),
{
    let safe_name = validate_safe_filename(&asset.filename)?;
    let safe_path = validate_asset_path(&asset.path)?;
    let expected_sha = asset.sha256.trim().to_ascii_lowercase();
    if expected_sha.len() != 64
        || !expected_sha.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "manifest asset sha256 must be 64 hex characters",
        ));
    }

    let primary_raw = format!(
        "{}/v1/download/{safe_path}",
        base_url.trim().trim_end_matches('/')
    );
    let primary_url = validate_update_url(&primary_raw)?;
    let fallback_url = validate_update_url(&asset.fallback_url)?;

    fs::create_dir_all(dest_dir)?;
    let final_path = dest_dir.join(safe_name);
    let part_path = dest_dir.join(format!("{safe_name}.part"));
    let agent = build_http_agent(DOWNLOAD_TIMEOUT_SECS);

    let primary_res = stream_download_single(
        &agent,
        &primary_url,
        &part_path,
        asset.size,
        &expected_sha,
        cancel,
        &mut on_progress,
    );

    if let Err(primary_err) = primary_res {
        if primary_err.kind() == io::ErrorKind::Interrupted
            || cancel.load(Ordering::Relaxed)
        {
            return Err(primary_err);
        }
        stream_download_single(
            &agent,
            &fallback_url,
            &part_path,
            asset.size,
            &expected_sha,
            cancel,
            &mut on_progress,
        )
        .map_err(|fb_err| {
            io::Error::new(
                fb_err.kind(),
                format!(
                    "primary download failed ({primary_err}); fallback failed \
                     ({fb_err})"
                ),
            )
        })?;
    }

    if final_path.exists() {
        let _ = fs::remove_file(&final_path);
    }
    fs::rename(&part_path, &final_path)?;
    Ok(final_path)
}

/// Format a one-line CLI / package-manager upgrade command for the given
/// [`InstallKind`] and release channel.
#[must_use]
pub fn recommended_upgrade_command_for(
    os: UpdateOs,
    linux_os_release: Option<&str>,
    kind: InstallKind,
    base_url: &str,
    manifest: Option<&Manifest>,
) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let is_beta =
        manifest.is_some_and(|m| m.channel.trim().eq_ignore_ascii_case("beta"));

    match kind {
        InstallKind::CliStandalone => match os {
            UpdateOs::Windows => {
                if is_beta {
                    format!(
                        "$env:RUUM_CHANNEL='beta'; irm {base}/install.ps1 | iex"
                    )
                } else {
                    format!("irm {base}/install.ps1 | iex")
                }
            }
            UpdateOs::MacOs | UpdateOs::Linux => {
                if is_beta {
                    format!(
                        "curl -fsSL {base}/install.sh | sh -s -- --cli-only \
                         --channel beta"
                    )
                } else {
                    format!(
                        "curl -fsSL {base}/install.sh | sh -s -- --cli-only"
                    )
                }
            }
        },
        InstallKind::WindowsInno
        | InstallKind::WindowsMsi
        | InstallKind::WindowsPortable => {
            if is_beta {
                format!(
                    "$env:RUUM_CHANNEL='beta'; irm {base}/install.ps1 | iex"
                )
            } else {
                format!("irm {base}/install.ps1 | iex")
            }
        }
        InstallKind::LinuxSystemPackage => {
            linux_system_pkg_upgrade_command(linux_os_release, base, is_beta)
        }
        InstallKind::MacAppBundle
        | InstallKind::LinuxAppImage
        | InstallKind::LinuxTarball => {
            if is_beta {
                format!(
                    "curl -fsSL {base}/install.sh | sh -s -- --channel beta"
                )
            } else {
                format!("curl -fsSL {base}/install.sh | sh")
            }
        }
        InstallKind::Development => {
            "git pull && cargo build --release".to_owned()
        }
    }
}

/// Format a one-line upgrade command using default host OS & distro detection.
#[must_use]
pub fn recommended_upgrade_command(
    kind: InstallKind,
    base_url: &str,
    manifest: Option<&Manifest>,
) -> String {
    let env = UpdateEnv::current();
    recommended_upgrade_command_for(
        UpdateFactory::current_os(),
        env.linux_os_release.as_deref(),
        kind,
        base_url,
        manifest,
    )
}

fn linux_system_pkg_upgrade_command(
    os_release: Option<&str>,
    base_url: &str,
    is_beta: bool,
) -> String {
    if let Some(raw) = os_release {
        let lower = raw.to_ascii_lowercase();
        if lower.contains("arch")
            || lower.contains("omarchy")
            || lower.contains("manjaro")
            || lower.contains("endeavouros")
        {
            return "yay -S ruum-bin".to_owned();
        }
        if lower.contains("debian") || lower.contains("ubuntu") {
            return "sudo apt update && sudo apt install --only-upgrade ruum"
                .to_owned();
        }
        if lower.contains("fedora")
            || lower.contains("rhel")
            || lower.contains("centos")
        {
            return "sudo dnf upgrade ruum".to_owned();
        }
    }
    if is_beta {
        format!("curl -fsSL {base_url}/install.sh | sh -s -- --channel beta")
    } else {
        format!("curl -fsSL {base_url}/install.sh | sh")
    }
}

/// Stop a running `ruum` watchdog process WITHOUT calling
/// `ServiceAdapter::uninstall` (preserves `HKCU\...\Run` / plist / systemd
/// enablement). Returns `true` if a running watchdog was stopped.
pub fn stop_running_watchdog_with(
    os: UpdateOs,
    pid_path: Option<&Path>,
    home: Option<&Path>,
    xdg_or_appdata: Option<&Path>,
    runner: &dyn CommandRunner,
) -> bool {
    let mut stopped = false;

    match os {
        UpdateOs::MacOs => {
            let svc = ServiceAdapterFactory::create_for(ServiceOs::MacOs);
            let bridge = CommandServiceRunner::new(runner);
            let s_env = ruum_watchdog::ServiceEnv {
                home: home.map(Path::to_path_buf),
                xdg_config_home: None,
                appdata: None,
            };
            let plist = home.map(|h| {
                h.join("Library/LaunchAgents/com.ruum.watchdog.plist")
            });
            if plist.as_ref().is_some_and(|p| p.is_file())
                && let Some(ref plist_path) = plist
            {
                if svc.status(&bridge, &s_env).ok()
                    == Some(ServiceStatus::Active)
                {
                    let plist_str = plist_path.to_string_lossy();
                    let _ = runner.run(
                        Path::new("launchctl"),
                        &["unload", plist_str.as_ref()],
                        &[],
                    );
                    stopped = true;
                }
            } else if home.is_some()
                && svc.status(&bridge, &s_env).ok()
                    == Some(ServiceStatus::Active)
            {
                let _ = svc.uninstall(&bridge, &s_env);
                stopped = true;
            }
            if stop_pid_file_unix(pid_path, runner) {
                stopped = true;
            }
        }
        UpdateOs::Linux => {
            let svc = ServiceAdapterFactory::create_for(ServiceOs::Linux);
            let bridge = CommandServiceRunner::new(runner);
            let s_env = ruum_watchdog::ServiceEnv {
                home: home.map(Path::to_path_buf),
                xdg_config_home: xdg_or_appdata.map(Path::to_path_buf),
                appdata: None,
            };
            if svc.status(&bridge, &s_env).ok() == Some(ServiceStatus::Active) {
                let _ = runner.run(
                    Path::new("systemctl"),
                    &["--user", "stop", "ruum-watchdog.service"],
                    &[],
                );
                stopped = true;
            }
            if stop_pid_file_unix(pid_path, runner) {
                stopped = true;
            }
        }
        UpdateOs::Windows => {
            if let Some(path) = pid_path
                && let Some(pid) = read_valid_pid(path)
            {
                let filter = format!("PID eq {pid}");
                let alive = runner
                    .run(
                        Path::new("tasklist"),
                        &["/FI", &filter, "/FO", "CSV", "/NH"],
                        &[],
                    )
                    .is_ok_and(|out| {
                        out.success()
                            && out.stdout.to_ascii_lowercase().contains("ruum")
                    });
                if alive {
                    let pid_str = pid.to_string();
                    let _ = runner.run(
                        Path::new("taskkill"),
                        &["/PID", &pid_str, "/T", "/F"],
                        &[],
                    );
                    stopped = true;
                }
                let _ = fs::remove_file(path);
            }
        }
    }

    stopped
}

/// Restart the watchdog process after an aborted/rolled-back update or after
/// syncing Windows CLI binaries.
pub fn restart_watchdog_with(
    os: UpdateOs,
    exe_path: &Path,
    home: Option<&Path>,
    _xdg_or_appdata: Option<&Path>,
    runner: &dyn CommandRunner,
) {
    match os {
        UpdateOs::MacOs => {
            if let Some(h) = home {
                let plist =
                    h.join("Library/LaunchAgents/com.ruum.watchdog.plist");
                if plist.is_file() {
                    let plist_str = plist.to_string_lossy();
                    let _ = runner.run(
                        Path::new("launchctl"),
                        &["load", plist_str.as_ref()],
                        &[],
                    );
                }
            }
        }
        UpdateOs::Linux => {
            let _ = runner.run(
                Path::new("systemctl"),
                &["--user", "start", "ruum-watchdog.service"],
                &[],
            );
        }
        UpdateOs::Windows => {
            let exe_str = exe_path.to_string_lossy();
            let ps_cmd = "Start-Process -WindowStyle Hidden \
                          -FilePath $env:RUUM_EXE \
                          -ArgumentList 'watchdog','run'";
            let _ = runner.run(
                Path::new("powershell.exe"),
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-WindowStyle",
                    "Hidden",
                    "-Command",
                    ps_cmd,
                ],
                &[("RUUM_EXE", exe_str.as_ref())],
            );
        }
    }
}

/// Synchronize `%LOCALAPPDATA%\Programs\ruum\bin\ruum.exe` (installed via
/// `cli_install`) when it exists and its SHA-256 differs from the running
/// installation's `ruum.exe`.
pub fn sync_windows_cli_with(
    env: &UpdateEnv,
    pid_path: Option<&Path>,
    runner: &dyn CommandRunner,
) -> io::Result<bool> {
    let Some(bin_dir) = WindowsUpdateAdapter::cli_bin_dir(env) else {
        return Ok(false);
    };
    let dst_cli = bin_dir.join("ruum.exe");
    if !dst_cli.is_file() {
        return Ok(false);
    }

    let Ok(src_cli) =
        resolve_cli_binary_for(CliInstallOs::Windows, &env.exe_path)
    else {
        return Ok(false);
    };
    if !src_cli.is_file() || windows_paths_equal(&src_cli, &dst_cli) {
        return Ok(false);
    }

    let src_hash = sha256_file(&src_cli)?;
    let dst_hash = sha256_file(&dst_cli)?;
    if src_hash == dst_hash {
        return Ok(false);
    }

    let was_running = stop_running_watchdog_with(
        UpdateOs::Windows,
        pid_path,
        env.home.as_deref(),
        env.local_appdata.as_deref(),
        runner,
    );

    atomic_install_file(&src_cli, &dst_cli, false)?;

    if let Some(src_dir) = src_cli.parent() {
        let src_gui = src_dir.join("ruum-gui.exe");
        let dst_gui = bin_dir.join("ruum-gui.exe");
        if src_gui.is_file()
            && (!dst_gui.is_file()
                || sha256_file(&src_gui).ok() != sha256_file(&dst_gui).ok())
        {
            let _ = atomic_install_file(&src_gui, &dst_gui, false);
        }
        let src_pack = src_dir.join(crate::payload::PACK_FILE_NAME);
        let dst_pack = bin_dir.join(crate::payload::PACK_FILE_NAME);
        if src_pack.is_file()
            && (!dst_pack.is_file()
                || sha256_file(&src_pack).ok() != sha256_file(&dst_pack).ok())
        {
            let _ = atomic_install_file(&src_pack, &dst_pack, false);
        }
    }

    if was_running {
        restart_watchdog_with(
            UpdateOs::Windows,
            &dst_cli,
            env.home.as_deref(),
            env.local_appdata.as_deref(),
            runner,
        );
    }

    Ok(true)
}

/// On Windows startup, sync `%LOCALAPPDATA%\Programs\ruum\bin\ruum.exe` if
/// present and out of date compared to the current binary.
pub fn sync_installed_windows_cli_if_needed() -> io::Result<bool> {
    if !cfg!(windows) {
        return Ok(false);
    }
    let env = UpdateEnv::current();
    let pid_path = ruum_watchdog::config::watchdog_pid_path();
    sync_windows_cli_with(
        &env,
        pid_path.as_deref(),
        &crate::ai_cli::DefaultCommandRunner,
    )
}

fn manual_action_outcome(
    adapter: &dyn UpdateAdapter,
    env: &UpdateEnv,
    kind: InstallKind,
    base_url: &str,
    manifest: &Manifest,
) -> ApplyOutcome {
    ApplyOutcome::ManualActionRequired {
        command: adapter.recommended_upgrade_command(
            env,
            kind,
            base_url,
            Some(manifest),
        ),
        url: Some(manifest.release_url.clone()),
    }
}

fn default_watchdog_pid_path_for(
    os: UpdateOs,
    home: Option<&Path>,
    xdg: Option<&Path>,
    appdata: Option<&Path>,
) -> Option<PathBuf> {
    match os {
        UpdateOs::MacOs => home
            .map(|h| h.join("Library/Application Support/ruum/watchdog.pid")),
        UpdateOs::Windows => appdata
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| home.map(|h| h.join("AppData").join("Roaming")))
            .map(|b| b.join("ruum").join("watchdog.pid")),
        UpdateOs::Linux => xdg
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| home.map(|h| h.join(".config")))
            .map(|b| b.join("ruum").join("watchdog.pid")),
    }
}

fn read_valid_pid(pid_path: &Path) -> Option<u32> {
    fs::read_to_string(pid_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&pid| pid > 0)
}

fn stop_pid_file_unix(
    pid_path: Option<&Path>,
    runner: &dyn CommandRunner,
) -> bool {
    let Some(path) = pid_path else {
        return false;
    };
    let Some(pid) = read_valid_pid(path) else {
        return false;
    };
    let pid_str = pid.to_string();
    let alive = runner
        .run(Path::new("kill"), &["-0", &pid_str], &[])
        .is_ok_and(|o| o.success());
    if !alive {
        let _ = fs::remove_file(path);
        return false;
    }
    let _ = runner.run(Path::new("kill"), &["-TERM", &pid_str], &[]);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let still_alive = runner
            .run(Path::new("kill"), &["-0", &pid_str], &[])
            .is_ok_and(|o| o.success());
        if !still_alive {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_file(path);
    true
}

fn normalized_segments(path: &Path) -> Vec<String> {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or_else(|| raw.as_ref());
    stripped
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn is_dev_build_path(path: &Path) -> bool {
    let segs = normalized_segments(path);
    segs.windows(2).any(|w| {
        w[0].eq_ignore_ascii_case("target")
            && (w[1].eq_ignore_ascii_case("debug")
                || w[1].eq_ignore_ascii_case("release"))
    })
}

fn normalize_windows_str(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or_else(|| raw.as_ref());
    stripped
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_owned()
}

fn windows_paths_equal(a: &Path, b: &Path) -> bool {
    normalize_windows_str(a).eq_ignore_ascii_case(&normalize_windows_str(b))
}

fn is_dir_writable(dir: &Path, override_val: Option<bool>) -> bool {
    if let Some(forced) = override_val {
        return forced;
    }
    if !dir.is_dir() {
        return false;
    }
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let probe =
        dir.join(format!(".ruum-write-probe.{}.{seq}", std::process::id()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(f) => {
            drop(f);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn locate_app_bundle(stage_dir: &Path) -> Option<PathBuf> {
    let direct = stage_dir.join("ruum.app");
    if direct.is_dir() {
        return Some(direct);
    }
    let entries = fs::read_dir(stage_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("ruum.app"))
            {
                return Some(p);
            }
            let nested = p.join("ruum.app");
            if nested.is_dir() {
                return Some(nested);
            }
        }
    }
    None
}

fn validate_macos_app_bundle(app_dir: &Path) -> io::Result<()> {
    if !app_dir.join("Contents/MacOS/ruum-gui").is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "macOS update bundle missing Contents/MacOS/ruum-gui",
        ));
    }
    if !app_dir.join("Contents/Info.plist").is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "macOS update bundle missing Contents/Info.plist",
        ));
    }
    Ok(())
}

fn replace_macos_app_writable(
    new_app: &Path,
    current_app: &Path,
    backup_app: &Path,
    runner: &dyn CommandRunner,
) -> io::Result<()> {
    if backup_app.exists() {
        fs::remove_dir_all(backup_app)?;
    }
    if current_app.exists() {
        fs::rename(current_app, backup_app)?;
    }
    let move_res = fs::rename(new_app, current_app).or_else(|_| {
        let new_str = new_app.to_string_lossy();
        let dst_str = current_app.to_string_lossy();
        let out = runner.run(
            Path::new("ditto"),
            &[new_str.as_ref(), dst_str.as_ref()],
            &[],
        )?;
        if out.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "ditto copy failed: {}",
                out.stderr.trim()
            )))
        }
    });
    if let Err(err) = move_res {
        if current_app.exists() {
            let _ = fs::remove_dir_all(current_app);
        }
        if backup_app.exists() {
            let _ = fs::rename(backup_app, current_app);
        }
        return Err(err);
    }
    if backup_app.exists() {
        let _ = fs::remove_dir_all(backup_app);
    }
    Ok(())
}

fn locate_tarball_root(stage_dir: &Path) -> Option<PathBuf> {
    if stage_dir.join("ruum").is_file() {
        return Some(stage_dir.to_path_buf());
    }
    let entries = fs::read_dir(stage_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && p.join("ruum").is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg_attr(
    not(unix),
    allow(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        reason = "Unix-only chmod; non-Unix keeps uniform Result signature"
    )
)]
fn set_executable_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn atomic_install_file(
    src: &Path,
    dst: &Path,
    executable: bool,
) -> io::Result<()> {
    let parent = dst.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "no parent directory")
    })?;
    fs::create_dir_all(parent)?;
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".ruum-upd.{}.{seq}", std::process::id()));
    let res = (|| -> io::Result<()> {
        fs::copy(src, &tmp)?;
        if executable {
            set_executable_mode(&tmp)?;
        }
        if dst.exists() {
            let _ = fs::remove_file(dst);
        }
        fs::rename(&tmp, dst)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_cli::CommandOutput;
    use ed25519_dalek::{Signer as _, SigningKey};
    use std::collections::VecDeque;
    use std::io::{BufRead as _, BufReader};
    use std::net::TcpListener;
    use std::sync::Mutex;

    type RecordedCall = (PathBuf, Vec<String>, Vec<(String, String)>);

    #[derive(Default)]
    struct MockRunner {
        calls: Mutex<Vec<RecordedCall>>,
        handlers: Mutex<VecDeque<CommandOutput>>,
        fail_program: Option<String>,
        create_app_on_ditto: bool,
        omit_info_plist_on_ditto: bool,
        tarball_with_install_sh: bool,
    }

    impl MockRunner {
        fn calls(&self) -> Vec<RecordedCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for MockRunner {
        fn run(
            &self,
            program: &Path,
            args: &[&str],
            envs: &[(&str, &str)],
        ) -> io::Result<CommandOutput> {
            self.calls.lock().unwrap().push((
                program.to_path_buf(),
                args.iter().map(|s| (*s).to_string()).collect(),
                envs.iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ));

            if let Some(fail) = &self.fail_program
                && program.to_string_lossy() == fail.as_str()
            {
                return Ok(CommandOutput {
                    status_code: 1,
                    stdout: String::new(),
                    stderr: format!("mock {fail} failed"),
                });
            }

            if self.create_app_on_ditto
                && program == Path::new("ditto")
                && args.first().copied() == Some("-xk")
                && let Some(dest) = args.get(2)
            {
                let app = Path::new(dest).join("ruum.app");
                fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
                fs::write(app.join("Contents/MacOS/ruum-gui"), b"new-gui")
                    .unwrap();
                fs::write(app.join("Contents/MacOS/ruum"), b"new-cli").unwrap();
                if !self.omit_info_plist_on_ditto {
                    fs::write(app.join("Contents/Info.plist"), b"<plist/>")
                        .unwrap();
                }
            }

            if self.tarball_with_install_sh
                && program == Path::new("tar")
                && let Some(idx) = args.iter().position(|a| *a == "-C")
                && let Some(dest) = args.get(idx + 1)
            {
                let root = Path::new(dest).join("ruum-0.3.0-x86_64-linux");
                fs::create_dir_all(&root).unwrap();
                fs::write(root.join("ruum"), b"new-linux-cli").unwrap();
                fs::write(root.join("ruum-gui"), b"new-linux-gui").unwrap();
                fs::write(root.join("install.sh"), b"#!/bin/sh\nexit 0\n")
                    .unwrap();
            }

            let next = self.handlers.lock().unwrap().pop_front();
            if let Some(out) = next {
                return Ok(out);
            }

            Ok(CommandOutput {
                status_code: 0,
                stdout: "active\n".to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[derive(Clone, Debug)]
    struct SpawnCall {
        program: PathBuf,
        args: Vec<String>,
        envs: Vec<(String, String)>,
        remove_envs: Vec<String>,
        hide_window: bool,
    }
    type ScriptedHttpResponse = (String, Vec<(String, String)>, Vec<u8>);

    #[derive(Default)]
    struct MockSpawner {
        calls: Mutex<Vec<SpawnCall>>,
        fail_spawn: bool,
    }

    impl MockSpawner {
        fn calls(&self) -> Vec<SpawnCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl DetachedSpawner for MockSpawner {
        fn spawn_detached(
            &self,
            program: &Path,
            args: &[&str],
            envs: &[(&str, &str)],
            remove_envs: &[&str],
            hide_window: bool,
        ) -> io::Result<()> {
            self.calls.lock().unwrap().push(SpawnCall {
                program: program.to_path_buf(),
                args: args.iter().map(|s| (*s).to_string()).collect(),
                envs: envs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
                remove_envs: remove_envs
                    .iter()
                    .map(|k| (*k).to_string())
                    .collect(),
                hide_window,
            });
            if self.fail_spawn {
                return Err(io::Error::other("mock spawn failed"));
            }
            Ok(())
        }
    }

    fn test_keypair() -> (SigningKey, String) {
        let seed = [7_u8; 32];
        let sk = SigningKey::from_bytes(&seed);
        let pk_b64 = BASE64.encode(sk.verifying_key().to_bytes());
        (sk, pk_b64)
    }

    fn sign_bytes(sk: &SigningKey, bytes: &[u8]) -> String {
        BASE64.encode(sk.sign(bytes).to_bytes())
    }

    fn sample_manifest(version: &str, channel: &str) -> Manifest {
        let mut assets = BTreeMap::new();
        assets.insert(
            "macos-aarch64-app_zip".to_owned(),
            ManifestAsset {
                filename: format!("ruum-{version}-aarch64-macos.zip"),
                path: format!(
                    "releases/v{version}/ruum-{version}-aarch64-macos.zip"
                ),
                fallback_url: format!(
                    "https://github.com/JsonLee12138/Ruum/releases/download/v{version}/ruum-{version}-aarch64-macos.zip"
                ),
                sha256: "a".repeat(64),
                size: 1024,
            },
        );
        Manifest {
            schema_version: 1,
            version: version.to_owned(),
            tag: format!("v{version}"),
            channel: channel.to_owned(),
            published_at: "2026-11-20T00:00:00Z".to_owned(),
            release_url: format!(
                "https://github.com/JsonLee12138/Ruum/releases/tag/v{version}"
            ),
            notes: "### Features\n- Unified updater".to_owned(),
            assets,
        }
    }

    fn spawn_scripted_http(
        responses: Vec<ScriptedHttpResponse>,
    ) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        use std::fmt::Write as _;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut reqs = Vec::new();
            for (status_line, headers, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut reader = BufReader::new(&mut stream);
                let mut first_line = String::new();
                let _ = reader.read_line(&mut first_line);
                let mut captured = first_line.trim().to_owned();
                loop {
                    let mut line = String::new();
                    let n = reader.read_line(&mut line).unwrap_or(0);
                    if n == 0 || line == "\r\n" || line == "\n" {
                        break;
                    }
                    captured.push('\n');
                    captured.push_str(line.trim_end_matches(['\r', '\n']));
                }
                reqs.push(captured);

                let mut resp_head = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    let _ = write!(resp_head, "{k}: {v}\r\n");
                }
                resp_head.push_str("Connection: close\r\n\r\n");
                let inner = reader.into_inner();
                inner.write_all(resp_head.as_bytes()).unwrap();
                inner.write_all(&body).unwrap();
                inner.flush().unwrap();
            }
            reqs
        });
        (port, handle)
    }

    #[test]
    fn env_resolution_precedence_and_empty_string_filtering() {
        assert_eq!(
            resolve_update_base_url_with(
                Some("  https://custom.example/  "),
                Some("https://compile.example/"),
            ),
            "https://custom.example"
        );
        assert_eq!(
            resolve_update_base_url_with(
                Some("   "),
                Some("https://compile.example/"),
            ),
            "https://compile.example"
        );
        assert_eq!(
            resolve_update_base_url_with(Some(""), Some("")),
            DEFAULT_UPDATE_BASE_URL
        );
        assert_eq!(
            resolve_update_public_key_with(Some("  ")),
            DEFAULT_UPDATE_PUBLIC_KEY
        );
        assert_eq!(resolve_update_public_key_with(Some("abc==")), "abc==");
        assert!(is_auto_check_disabled_with(Some("1"), false));
        assert!(is_auto_check_disabled_with(Some("TRUE"), false));
        assert!(!is_auto_check_disabled_with(Some("0"), false));
        assert!(is_auto_check_disabled_with(Some("0"), true));
        assert!(is_auto_check_disabled());
    }

    #[test]
    fn factory_creates_all_three_adapters_unconditionally() {
        assert_eq!(
            UpdateFactory::create_for(UpdateOs::MacOs).os_kind(),
            UpdateOs::MacOs
        );
        assert_eq!(
            UpdateFactory::create_for(UpdateOs::Linux).os_kind(),
            UpdateOs::Linux
        );
        assert_eq!(
            UpdateFactory::create_for(UpdateOs::Windows).os_kind(),
            UpdateOs::Windows
        );
    }

    #[test]
    fn macos_detects_all_app_bundles_and_cli_standalone_and_development() {
        let adapter = UpdateFactory::create_for(UpdateOs::MacOs);

        for bundle_exe in [
            "/Applications/ruum.app/Contents/MacOS/ruum-gui",
            "/Users/alice/Applications/ruum.app/Contents/MacOS/ruum-gui",
            "/opt/homebrew/Caskroom/ruum/0.2.2/ruum.app/Contents/MacOS/ruum-gui",
        ] {
            let env = UpdateEnv {
                os: Some(UpdateOs::MacOs),
                exe_path: PathBuf::from(bundle_exe),
                ..UpdateEnv::default()
            };
            assert_eq!(
                adapter.detect_install_kind(&env),
                InstallKind::MacAppBundle,
                "failed for {bundle_exe}"
            );
            assert!(adapter.can_self_update(&env, InstallKind::MacAppBundle));
        }

        // Symlinked CLI pointing into ruum.app must also resolve to MacAppBundle.
        #[cfg(unix)]
        {
            let tmp = tempfile::tempdir().unwrap();
            let real_cli = tmp.path().join("ruum.app/Contents/MacOS/ruum");
            fs::create_dir_all(real_cli.parent().unwrap()).unwrap();
            fs::write(&real_cli, b"cli").unwrap();
            let link = tmp.path().join("ruum");
            std::os::unix::fs::symlink(&real_cli, &link).unwrap();
            let env = UpdateEnv {
                os: Some(UpdateOs::MacOs),
                exe_path: link,
                is_cli_binary: true,
                ..UpdateEnv::default()
            };
            assert_eq!(
                adapter.detect_install_kind(&env),
                InstallKind::MacAppBundle
            );
        }

        let standalone = UpdateEnv {
            os: Some(UpdateOs::MacOs),
            exe_path: PathBuf::from("/Users/alice/.local/bin/ruum"),
            is_cli_binary: true,
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&standalone),
            InstallKind::CliStandalone
        );
        assert!(
            !adapter.can_self_update(&standalone, InstallKind::CliStandalone)
        );

        let dev = UpdateEnv {
            os: Some(UpdateOs::MacOs),
            exe_path: PathBuf::from("/Users/alice/Ruum/target/release/ruum"),
            is_cli_binary: true,
            ..UpdateEnv::default()
        };
        assert_eq!(adapter.detect_install_kind(&dev), InstallKind::Development);
    }

    #[test]
    fn windows_detects_inno_msi_portable_cli_standalone_and_development() {
        let adapter = UpdateFactory::create_for(UpdateOs::Windows);

        let inno = UpdateEnv {
            os: Some(UpdateOs::Windows),
            exe_path: PathBuf::from(
                r"C:\Users\alice\AppData\Local\Programs\ruum\ruum-gui.exe",
            ),
            inno_installed: Some(true),
            msi_installed: Some(false),
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&inno),
            InstallKind::WindowsInno
        );
        assert!(adapter.can_self_update(&inno, InstallKind::WindowsInno));

        let msi = UpdateEnv {
            os: Some(UpdateOs::Windows),
            exe_path: PathBuf::from(r"C:\Program Files\ruum\ruum-gui.exe"),
            inno_installed: Some(false),
            msi_installed: Some(true),
            program_files: Some(PathBuf::from(r"C:\Program Files")),
            ..UpdateEnv::default()
        };
        assert_eq!(adapter.detect_install_kind(&msi), InstallKind::WindowsMsi);
        assert!(adapter.can_self_update(&msi, InstallKind::WindowsMsi));

        let cli = UpdateEnv {
            os: Some(UpdateOs::Windows),
            exe_path: PathBuf::from(
                r"C:\Users\alice\AppData\Local\Programs\ruum\bin\ruum.exe",
            ),
            local_appdata: Some(PathBuf::from(r"C:\Users\alice\AppData\Local")),
            is_cli_binary: true,
            inno_installed: Some(false),
            msi_installed: Some(false),
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&cli),
            InstallKind::CliStandalone
        );

        let portable = UpdateEnv {
            os: Some(UpdateOs::Windows),
            exe_path: PathBuf::from(r"D:\Portable\ruum\ruum-gui.exe"),
            inno_installed: Some(false),
            msi_installed: Some(false),
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&portable),
            InstallKind::WindowsPortable
        );
        assert!(
            !adapter.can_self_update(&portable, InstallKind::WindowsPortable)
        );

        let dev = UpdateEnv {
            os: Some(UpdateOs::Windows),
            exe_path: PathBuf::from(r"C:\src\Ruum\target\debug\ruum.exe"),
            ..UpdateEnv::default()
        };
        assert_eq!(adapter.detect_install_kind(&dev), InstallKind::Development);
    }

    #[test]
    fn linux_detects_appimage_system_pkg_tarball_cli_standalone_and_dev() {
        let tmp = tempfile::tempdir().unwrap();
        let adapter = UpdateFactory::create_for(UpdateOs::Linux);

        // $APPIMAGE overrides /tmp/.mount_* current_exe().
        let appimage_file = tmp.path().join("Ruum-0.2.2-x86_64.AppImage");
        fs::write(&appimage_file, b"appimage").unwrap();
        let appimage_env = UpdateEnv {
            os: Some(UpdateOs::Linux),
            exe_path: PathBuf::from("/tmp/.mount_ruum123/usr/bin/ruum-gui"),
            appimage_env: Some(appimage_file),
            writable_override: Some(true),
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&appimage_env),
            InstallKind::LinuxAppImage
        );
        assert!(
            adapter.can_self_update(&appimage_env, InstallKind::LinuxAppImage)
        );

        for sys_exe in ["/usr/bin/ruum", "/usr/lib/ruum/ruum-gui"] {
            let sys_env = UpdateEnv {
                os: Some(UpdateOs::Linux),
                exe_path: PathBuf::from(sys_exe),
                is_cli_binary: sys_exe.ends_with("ruum"),
                ..UpdateEnv::default()
            };
            assert_eq!(
                adapter.detect_install_kind(&sys_env),
                InstallKind::LinuxSystemPackage
            );
            assert!(
                !adapter
                    .can_self_update(&sys_env, InstallKind::LinuxSystemPackage)
            );
        }

        // User dir with sibling ruum-gui -> LinuxTarball.
        let tarball_bin = tmp.path().join("tarball/bin");
        fs::create_dir_all(&tarball_bin).unwrap();
        fs::write(tarball_bin.join("ruum"), b"cli").unwrap();
        fs::write(tarball_bin.join("ruum-gui"), b"gui").unwrap();
        let tarball_env = UpdateEnv {
            os: Some(UpdateOs::Linux),
            exe_path: tarball_bin.join("ruum"),
            is_cli_binary: true,
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&tarball_env),
            InstallKind::LinuxTarball
        );

        // User dir with .desktop -> LinuxTarball.
        let desktop_home = tmp.path().join("home-desktop");
        let desktop_file =
            desktop_home.join(".local/share/applications/ruum.desktop");
        fs::create_dir_all(desktop_file.parent().unwrap()).unwrap();
        fs::write(&desktop_file, b"[Desktop Entry]\n").unwrap();
        let desktop_cli = desktop_home.join(".local/bin/ruum");
        fs::create_dir_all(desktop_cli.parent().unwrap()).unwrap();
        fs::write(&desktop_cli, b"cli").unwrap();
        let desktop_env = UpdateEnv {
            os: Some(UpdateOs::Linux),
            exe_path: desktop_cli,
            home: Some(desktop_home),
            is_cli_binary: true,
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&desktop_env),
            InstallKind::LinuxTarball
        );

        // Headless CLI alone -> CliStandalone.
        let headless_home = tmp.path().join("home-headless");
        let headless_cli = headless_home.join(".local/bin/ruum");
        fs::create_dir_all(headless_cli.parent().unwrap()).unwrap();
        fs::write(&headless_cli, b"cli").unwrap();
        let headless_env = UpdateEnv {
            os: Some(UpdateOs::Linux),
            exe_path: headless_cli,
            home: Some(headless_home),
            is_cli_binary: true,
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&headless_env),
            InstallKind::CliStandalone
        );

        let dev_env = UpdateEnv {
            os: Some(UpdateOs::Linux),
            exe_path: PathBuf::from("/home/dev/ruum/target/debug/ruum"),
            is_cli_binary: true,
            ..UpdateEnv::default()
        };
        assert_eq!(
            adapter.detect_install_kind(&dev_env),
            InstallKind::Development
        );
    }

    #[test]
    fn verify_manifest_signature_rejects_tampered_and_reformatted_bytes() {
        let (sk, pk_b64) = test_keypair();
        let manifest = sample_manifest("0.3.0", "stable");
        let raw = serde_json::to_vec_pretty(&manifest).unwrap();
        let sig = sign_bytes(&sk, &raw);

        let verified = verify_manifest_signature(&raw, &sig, &pk_b64).unwrap();
        assert_eq!(verified, manifest);

        // Compact JSON re-serialization has different bytes -> signature fails.
        let compact = serde_json::to_vec(&manifest).unwrap();
        assert!(verify_manifest_signature(&compact, &sig, &pk_b64).is_err());

        // Tampered byte -> signature fails.
        let mut tampered = raw.clone();
        tampered[10] ^= 0xff;
        assert!(verify_manifest_signature(&tampered, &sig, &pk_b64).is_err());

        // Wrong public key -> fails.
        let wrong_pk = BASE64.encode([9_u8; 32]);
        assert!(verify_manifest_signature(&raw, &sig, &wrong_pk).is_err());

        // Wrong schema_version -> rejected even if validly signed.
        let mut bad_schema = manifest;
        bad_schema.schema_version = 2;
        let bad_raw = serde_json::to_vec(&bad_schema).unwrap();
        let bad_sig = sign_bytes(&sk, &bad_raw);
        assert!(
            verify_manifest_signature(&bad_raw, &bad_sig, &pk_b64).is_err()
        );
    }

    #[test]
    fn semver_comparison_handles_prereleases_correctly() {
        assert!(is_newer_version("0.2.2-beta.1", "0.2.2").unwrap());
        assert!(is_newer_version("0.2.2", "0.2.3-beta.1").unwrap());
        assert!(is_newer_version("0.2.2-beta.1", "0.2.2-beta.2").unwrap());
        assert!(!is_newer_version("0.2.2", "0.2.2").unwrap());
        assert!(!is_newer_version("0.2.3", "0.2.2").unwrap());
        assert!(!is_newer_version("0.2.2", "0.2.2-beta.1").unwrap());
    }

    #[test]
    fn check_for_updates_worker_success_client_id_and_github_fallback_on_5xx() {
        let (sk, pk_b64) = test_keypair();
        let manifest = sample_manifest("0.3.0", "stable");
        let raw = serde_json::to_vec_pretty(&manifest).unwrap();
        let sig = sign_bytes(&sk, &raw);

        // 1. Primary worker succeeds and receives X-Ruum-Client-Id when set.
        let (port, srv) = spawn_scripted_http(vec![(
            "200 OK".to_owned(),
            vec![
                ("Content-Type".to_owned(), "application/json".to_owned()),
                ("X-Ruum-Signature".to_owned(), sig.clone()),
            ],
            raw.clone(),
        )]);
        let base_url = format!("http://127.0.0.1:{port}");
        let res = check_for_updates(&UpdateCheckRequest {
            channel: UpdateChannel::Stable,
            current_version: "0.2.2",
            os: UpdateOs::MacOs,
            arch: UpdateArch::Aarch64,
            install_kind: InstallKind::MacAppBundle,
            lang: "en",
            entry: "gui",
            client_id: Some("cid-xyz"),
            base_url: &base_url,
            public_key_b64: &pk_b64,
            github_repo: DEFAULT_GITHUB_REPO,
            github_fallback_base: None,
        })
        .unwrap();
        let reqs = srv.join().unwrap();
        assert!(reqs[0].contains("GET /v1/check?channel=stable"));
        assert!(
            reqs[0]
                .to_ascii_lowercase()
                .contains("x-ruum-client-id: cid-xyz")
        );
        assert!(res.update_available);
        assert!(res.manifest.is_some());
        assert_eq!(res.latest_version, "0.3.0");
        assert!(!res.used_fallback);
        assert_eq!(
            res.selected_asset_key.as_deref(),
            Some("macos-aarch64-app_zip")
        );

        // 2. Primary worker returns 503 -> falls back to GitHub manifest + .sig
        //    with same Ed25519 key (and omits X-Ruum-Client-Id when None).
        let (w_port, w_srv) = spawn_scripted_http(vec![(
            "503 Service Unavailable".to_owned(),
            vec![],
            b"down".to_vec(),
        )]);
        let (gh_port, gh_srv) = spawn_scripted_http(vec![
            (
                "200 OK".to_owned(),
                vec![(
                    "Content-Type".to_owned(),
                    "application/json".to_owned(),
                )],
                raw,
            ),
            (
                "200 OK".to_owned(),
                vec![("Content-Type".to_owned(), "text/plain".to_owned())],
                sig.into_bytes(),
            ),
        ]);
        let w_base = format!("http://127.0.0.1:{w_port}");
        let gh_base = format!("http://127.0.0.1:{gh_port}");
        let fb_res = check_for_updates(&UpdateCheckRequest {
            channel: UpdateChannel::Stable,
            current_version: "0.2.2",
            os: UpdateOs::MacOs,
            arch: UpdateArch::Aarch64,
            install_kind: InstallKind::CliStandalone,
            lang: "zh",
            entry: "cli",
            client_id: None,
            base_url: &w_base,
            public_key_b64: &pk_b64,
            github_repo: DEFAULT_GITHUB_REPO,
            github_fallback_base: Some(&gh_base),
        })
        .unwrap();
        let w_reqs = w_srv.join().unwrap();
        let gh_reqs = gh_srv.join().unwrap();
        assert!(!w_reqs[0].to_ascii_lowercase().contains("x-ruum-client-id"));
        assert!(gh_reqs[0].contains("/releases/latest/download/manifest.json"));
        assert!(
            gh_reqs[1].contains("/releases/latest/download/manifest.json.sig")
        );
        assert!(fb_res.used_fallback);
        assert!(fb_res.manifest.is_some());
        assert!(fb_res.update_available);
        assert!(fb_res.upgrade_command.contains("--cli-only"));
    }

    #[test]
    fn check_for_updates_treats_204_and_legacy_404_as_no_release() {
        let (_sk, pk_b64) = test_keypair();

        // 1. 204 No Content -> Ok(up-to-date, manifest = None) for both channels.
        let (p204, srv204) = spawn_scripted_http(vec![
            ("204 No Content".to_owned(), vec![], Vec::new()),
            (
                "404 Not Found".to_owned(),
                vec![(
                    "Content-Type".to_owned(),
                    "application/json".to_owned(),
                )],
                br#"{"error":"manifest not found"}"#.to_vec(),
            ),
            (
                "404 Not Found".to_owned(),
                vec![("Content-Type".to_owned(), "text/html".to_owned())],
                b"<html>Not Found</html>".to_vec(),
            ),
            (
                "502 Bad Gateway".to_owned(),
                vec![(
                    "Content-Type".to_owned(),
                    "application/json".to_owned(),
                )],
                br#"{"error":"incomplete manifest publication"}"#.to_vec(),
            ),
        ]);
        let base = format!("http://127.0.0.1:{p204}");

        let res_204 = check_for_updates(&UpdateCheckRequest {
            channel: UpdateChannel::Stable,
            current_version: "0.2.2",
            os: UpdateOs::MacOs,
            arch: UpdateArch::Aarch64,
            install_kind: InstallKind::MacAppBundle,
            lang: "en",
            entry: "gui",
            client_id: None,
            base_url: &base,
            public_key_b64: &pk_b64,
            github_repo: DEFAULT_GITHUB_REPO,
            github_fallback_base: None,
        })
        .unwrap();
        assert!(!res_204.update_available);
        assert_eq!(res_204.latest_version, "0.2.2");
        assert!(res_204.manifest.is_none());
        assert!(res_204.selected_asset.is_none());
        assert!(res_204.selected_asset_key.is_none());
        assert!(!res_204.used_fallback);

        // 2. Legacy Worker 404 {"error":"manifest not found"} -> NoRelease.
        let res_legacy_404 = check_for_updates(&UpdateCheckRequest {
            channel: UpdateChannel::Beta,
            current_version: "0.2.2",
            os: UpdateOs::Linux,
            arch: UpdateArch::X86_64,
            install_kind: InstallKind::LinuxAppImage,
            lang: "en",
            entry: "cli",
            client_id: None,
            base_url: &base,
            public_key_b64: &pk_b64,
            github_repo: DEFAULT_GITHUB_REPO,
            github_fallback_base: None,
        })
        .unwrap();
        assert!(!res_legacy_404.update_available);
        assert!(res_legacy_404.manifest.is_none());

        // 3. Genuine HTML 404 (wrong URL / reverse proxy) must still error.
        let err_404 = check_for_updates(&UpdateCheckRequest {
            channel: UpdateChannel::Stable,
            current_version: "0.2.2",
            os: UpdateOs::Linux,
            arch: UpdateArch::X86_64,
            install_kind: InstallKind::CliStandalone,
            lang: "en",
            entry: "cli",
            client_id: None,
            base_url: &base,
            public_key_b64: &pk_b64,
            github_repo: DEFAULT_GITHUB_REPO,
            github_fallback_base: None,
        })
        .unwrap_err();
        assert!(err_404.to_string().contains("404"), "{err_404}");

        // 4. Incomplete publication 502 on beta fails instead of faking up-to-date.
        let err_502 = check_for_updates(&UpdateCheckRequest {
            channel: UpdateChannel::Beta,
            current_version: "0.2.2",
            os: UpdateOs::Linux,
            arch: UpdateArch::X86_64,
            install_kind: InstallKind::CliStandalone,
            lang: "en",
            entry: "cli",
            client_id: None,
            base_url: &base,
            public_key_b64: &pk_b64,
            github_repo: DEFAULT_GITHUB_REPO,
            github_fallback_base: None,
        })
        .unwrap_err();
        assert!(err_502.to_string().contains("502"), "{err_502}");

        srv204.join().unwrap();
    }

    #[test]
    fn download_and_verify_asset_streaming_sha256_fallback_cancel_and_guards() {
        let tmp = tempfile::tempdir().unwrap();
        let payload = vec![0x5a_u8; 160 * 1024]; // > 2 x 64 KiB chunks
        let expected_sha = hex_lower(&Sha256::digest(&payload));

        // 1. Primary 500 -> fallback succeeds with progress callbacks.
        let (p_port, p_srv) = spawn_scripted_http(vec![(
            "500 Internal Server Error".to_owned(),
            vec![],
            b"err".to_vec(),
        )]);
        let (fb_port, fb_srv) = spawn_scripted_http(vec![(
            "200 OK".to_owned(),
            vec![],
            payload.clone(),
        )]);

        let asset = ManifestAsset {
            filename: "ruum-0.3.0.zip".to_owned(),
            path: "releases/v0.3.0/ruum-0.3.0.zip".to_owned(),
            fallback_url: format!(
                "http://127.0.0.1:{fb_port}/fallback/ruum-0.3.0.zip"
            ),
            sha256: expected_sha,
            size: payload.len() as u64,
        };
        let cancel = AtomicBool::new(false);
        let mut progress_steps = Vec::new();
        let out = download_and_verify_asset(
            &format!("http://127.0.0.1:{p_port}"),
            &asset,
            tmp.path(),
            &cancel,
            |dl, total| progress_steps.push((dl, total)),
        )
        .unwrap();
        p_srv.join().unwrap();
        fb_srv.join().unwrap();
        assert_eq!(out, tmp.path().join("ruum-0.3.0.zip"));
        assert_eq!(fs::read(&out).unwrap(), payload);
        assert!(!tmp.path().join("ruum-0.3.0.zip.part").exists());
        assert!(progress_steps.len() >= 2);
        assert_eq!(
            progress_steps.last().copied(),
            Some((payload.len() as u64, payload.len() as u64))
        );

        // 2. SHA-256 mismatch deletes .part and fails.
        let (bad_port, bad_srv) = spawn_scripted_http(vec![(
            "200 OK".to_owned(),
            vec![],
            payload.clone(),
        )]);
        let mut bad_asset = asset.clone();
        bad_asset.filename = "bad-sha.zip".to_owned();
        bad_asset.sha256 = "f".repeat(64);
        bad_asset.fallback_url =
            format!("http://127.0.0.1:{bad_port}/fallback/bad-sha.zip");
        let (bad_p_port, bad_p_srv) =
            spawn_scripted_http(vec![("200 OK".to_owned(), vec![], payload)]);
        let err = download_and_verify_asset(
            &format!("http://127.0.0.1:{bad_p_port}"),
            &bad_asset,
            tmp.path(),
            &cancel,
            |_, _| {},
        )
        .unwrap_err();
        bad_p_srv.join().unwrap();
        drop(bad_srv);
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(!tmp.path().join("bad-sha.zip.part").exists());
        assert!(!tmp.path().join("bad-sha.zip").exists());

        // 3. Cancellation interrupts and cleans .part.
        let cancel_flag = AtomicBool::new(true);
        let cancel_err = download_and_verify_asset(
            "https://updates.example.com",
            &asset,
            tmp.path(),
            &cancel_flag,
            |_, _| {},
        )
        .unwrap_err();
        assert_eq!(cancel_err.kind(), io::ErrorKind::Interrupted);

        // 4. URL scheme & traversal guards reject unsafe paths/URLs.
        assert!(validate_update_url("http://evil.example.com/a.zip").is_err());
        assert!(validate_update_url("ftp://127.0.0.1/a.zip").is_err());
        assert!(validate_update_url("http://127.0.0.1:8080/a.zip").is_ok());
        assert!(validate_update_url("https://example.com/a.zip").is_ok());
        assert!(validate_asset_path("releases/../secret").is_err());
        assert!(validate_asset_path("other/v1/file.zip").is_err());
        assert!(validate_safe_filename("../evil.zip").is_err());
        assert!(validate_safe_filename("sub/file.zip").is_err());
    }

    #[test]
    fn apply_update_macos_bundle_writable_jxa_and_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let app_dir = tmp.path().join("Applications/ruum.app");
        fs::create_dir_all(app_dir.join("Contents/MacOS")).unwrap();
        fs::write(app_dir.join("Contents/MacOS/ruum-gui"), b"old-gui").unwrap();
        fs::write(app_dir.join("Contents/MacOS/ruum"), b"old-cli").unwrap();
        fs::write(app_dir.join("Contents/Info.plist"), b"<plist/>").unwrap();
        let zip_path = tmp.path().join("ruum-0.3.0.zip");
        fs::write(&zip_path, b"zip-bytes").unwrap();

        let manifest = sample_manifest("0.3.0", "stable");
        let adapter = MacOsUpdateAdapter;

        // 1. Writable bundle: replaces atomically, stops watchdog (without
        //    calling uninstall), restarts watchdog after replacement, cleans
        //    .bak, and relaunches via `open -n`.
        let home_dir = tmp.path().join("home");
        let plist_path =
            home_dir.join("Library/LaunchAgents/com.ruum.watchdog.plist");
        fs::create_dir_all(plist_path.parent().unwrap()).unwrap();
        fs::write(&plist_path, b"<plist/>").unwrap();

        let runner = MockRunner {
            create_app_on_ditto: true,
            ..MockRunner::default()
        };
        let spawner = MockSpawner::default();
        let env = UpdateEnv {
            os: Some(UpdateOs::MacOs),
            exe_path: app_dir.join("Contents/MacOS/ruum-gui"),
            home: Some(home_dir),
            writable_override: Some(true),
            ..UpdateEnv::default()
        };
        let outcome = adapter
            .apply_with_spawner(
                &env,
                InstallKind::MacAppBundle,
                &zip_path,
                &manifest,
                &runner,
                &spawner,
            )
            .unwrap();
        assert!(matches!(
            outcome,
            ApplyOutcome::RestartRequired {
                spawned_relaunch: true,
                ..
            }
        ));
        assert_eq!(
            fs::read(app_dir.join("Contents/MacOS/ruum-gui")).unwrap(),
            b"new-gui"
        );
        assert!(!tmp.path().join("Applications/ruum.app.bak").exists());
        assert_eq!(spawner.calls()[0].program, PathBuf::from("open"));

        let wd_calls = runner.calls();
        let unload_idx = wd_calls
            .iter()
            .position(|c| {
                c.0 == Path::new("launchctl")
                    && c.1.first().map(String::as_str) == Some("unload")
            })
            .expect("launchctl unload must run");
        let load_idx = wd_calls
            .iter()
            .position(|c| {
                c.0 == Path::new("launchctl")
                    && c.1.first().map(String::as_str) == Some("load")
            })
            .expect("launchctl load must restart watchdog on success");
        assert!(unload_idx < load_idx);
        assert!(
            !wd_calls.iter().any(|c| c.0 == Path::new("launchctl")
                && c.1.iter().any(|a| a == "-w"))
        );
        assert!(plist_path.is_file());

        // 2. Non-writable bundle: invokes osascript JXA with RUUM_APP_* envs.
        let ro_runner = MockRunner {
            create_app_on_ditto: true,
            ..MockRunner::default()
        };
        let ro_spawner = MockSpawner::default();
        let ro_env = UpdateEnv {
            writable_override: Some(false),
            ..env.clone()
        };
        adapter
            .apply_with_spawner(
                &ro_env,
                InstallKind::MacAppBundle,
                &zip_path,
                &manifest,
                &ro_runner,
                &ro_spawner,
            )
            .unwrap();
        let calls = ro_runner.calls();
        let osa = calls
            .iter()
            .find(|c| c.0 == Path::new("osascript"))
            .expect("osascript invoked");
        assert!(osa.2.iter().any(|(k, _)| k == "RUUM_APP_SRC"));
        assert!(osa.2.iter().any(|(k, _)| k == "RUUM_APP_DST"));
        assert!(osa.2.iter().any(|(k, _)| k == "RUUM_APP_BAK"));

        // 3. Invalid archive missing Info.plist aborts without replacing app.
        let bad_runner = MockRunner {
            create_app_on_ditto: true,
            omit_info_plist_on_ditto: true,
            ..MockRunner::default()
        };
        let bad_spawner = MockSpawner::default();
        let err = adapter
            .apply_with_spawner(
                &env,
                InstallKind::MacAppBundle,
                &zip_path,
                &manifest,
                &bad_runner,
                &bad_spawner,
            )
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(bad_spawner.calls().is_empty());
    }

    #[test]
    fn apply_update_windows_inno_msi_and_sync_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let app_dir = tmp.path().join("install");
        let cli_bin_dir = tmp.path().join("localappdata/Programs/ruum/bin");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&cli_bin_dir).unwrap();
        fs::write(app_dir.join("ruum.exe"), b"new-cli-bytes").unwrap();
        fs::write(app_dir.join("ruum-gui.exe"), b"new-gui-bytes").unwrap();
        fs::write(
            app_dir.join(crate::payload::PACK_FILE_NAME),
            b"new-pack-bytes",
        )
        .unwrap();
        fs::write(cli_bin_dir.join("ruum.exe"), b"old-cli-bytes").unwrap();
        fs::write(
            cli_bin_dir.join(crate::payload::PACK_FILE_NAME),
            b"old-pack-bytes",
        )
        .unwrap();

        let pid_path = tmp.path().join("watchdog.pid");
        fs::write(&pid_path, "4242\n").unwrap();

        let ok_cmd = || CommandOutput {
            status_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        let runner = MockRunner {
            handlers: Mutex::new(VecDeque::from(vec![
                CommandOutput {
                    status_code: 0,
                    stdout: "\"ruum.exe\",\"4242\"\n".to_owned(),
                    stderr: String::new(),
                },
                ok_cmd(),
                ok_cmd(),
            ])),
            ..MockRunner::default()
        };

        let env = UpdateEnv {
            os: Some(UpdateOs::Windows),
            exe_path: app_dir.join("ruum-gui.exe"),
            home: Some(tmp.path().join("home")),
            local_appdata: Some(tmp.path().join("localappdata")),
            inno_installed: Some(true),
            ..UpdateEnv::default()
        };

        // Sync detects mismatch, stops watchdog via taskkill (never reg delete),
        // copies updated ruum.exe + ruum-gui.exe, and restarts watchdog.
        assert!(sync_windows_cli_with(&env, Some(&pid_path), &runner).unwrap());
        assert_eq!(
            fs::read(cli_bin_dir.join("ruum.exe")).unwrap(),
            b"new-cli-bytes"
        );
        assert_eq!(
            fs::read(cli_bin_dir.join("ruum-gui.exe")).unwrap(),
            b"new-gui-bytes"
        );
        assert_eq!(
            fs::read(cli_bin_dir.join(crate::payload::PACK_FILE_NAME)).unwrap(),
            b"new-pack-bytes"
        );
        let calls = runner.calls();
        assert!(calls.iter().any(|c| c.0 == Path::new("taskkill")));
        assert!(!calls.iter().any(|c| c.0 == Path::new("reg")));
        assert!(calls.iter().any(|c| c.0 == Path::new("powershell.exe")));

        // Second sync is a no-op because SHA-256 matches.
        assert!(
            !sync_windows_cli_with(&env, Some(&pid_path), &runner).unwrap()
        );

        // Inno & MSI launch checks.
        let adapter = WindowsUpdateAdapter;
        let manifest = sample_manifest("0.3.0", "stable");
        let setup_exe = tmp.path().join("ruum-0.3.0-x86_64-windows-setup.exe");
        let msi_file = tmp.path().join("ruum-0.3.0-x86_64-windows.msi");
        let spawner = MockSpawner::default();

        let inno_res = adapter
            .apply_with_spawner(
                &env,
                InstallKind::WindowsInno,
                &setup_exe,
                &manifest,
                &MockRunner::default(),
                &spawner,
            )
            .unwrap();
        assert!(matches!(inno_res, ApplyOutcome::InstallerLaunched { .. }));

        let msi_res = adapter
            .apply_with_spawner(
                &env,
                InstallKind::WindowsMsi,
                &msi_file,
                &manifest,
                &MockRunner::default(),
                &spawner,
            )
            .unwrap();
        assert!(matches!(msi_res, ApplyOutcome::InstallerLaunched { .. }));

        let spawned = spawner.calls();
        assert_eq!(spawned[0].program, setup_exe);
        assert_eq!(
            spawned[0].args,
            vec!["/SILENT", "/CLOSEAPPLICATIONS", "/RESTARTAPPLICATIONS"]
        );
        assert_eq!(spawned[1].program, PathBuf::from("powershell.exe"));
        assert!(spawned[1].hide_window); // CREATE_NO_WINDOW hidden waiter
        assert!(spawned[1].envs.iter().any(|(k, _)| k == "RUUM_OLD_PID"));
        assert!(spawned[1].envs.iter().any(|(k, _)| k == "RUUM_MSI_PATH"));

        // Inno spawn failure restarts watchdog if it was stopped.
        let appdata_dir = tmp.path().join("localappdata");
        let win_pid_path = appdata_dir.join("ruum/watchdog.pid");
        fs::create_dir_all(win_pid_path.parent().unwrap()).unwrap();
        fs::write(&win_pid_path, "5151\n").unwrap();
        let fail_runner = MockRunner {
            handlers: Mutex::new(VecDeque::from(vec![
                CommandOutput {
                    status_code: 0,
                    stdout: "\"ruum.exe\",\"5151\"\n".to_owned(),
                    stderr: String::new(),
                },
                ok_cmd(),
                ok_cmd(),
            ])),
            ..MockRunner::default()
        };
        let fail_spawner = MockSpawner {
            fail_spawn: true,
            ..MockSpawner::default()
        };
        let fail_env = UpdateEnv {
            local_appdata: Some(appdata_dir),
            ..env
        };
        assert!(
            adapter
                .apply_with_spawner(
                    &fail_env,
                    InstallKind::WindowsInno,
                    &setup_exe,
                    &manifest,
                    &fail_runner,
                    &fail_spawner,
                )
                .is_err()
        );
        let f_calls = fail_runner.calls();
        assert!(f_calls.iter().any(|c| c.0 == Path::new("taskkill")));
        assert!(f_calls.iter().any(|c| c.0 == Path::new("powershell.exe")));
    }

    #[test]
    fn apply_update_linux_appimage_tarball_and_system_package_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let adapter = LinuxUpdateAdapter;
        let manifest = sample_manifest("0.3.0", "stable");

        let linux_home = tmp.path().join("linux-home");
        let unit_path =
            linux_home.join(".config/systemd/user/ruum-watchdog.service");
        fs::create_dir_all(unit_path.parent().unwrap()).unwrap();
        fs::write(&unit_path, b"[Unit]\n").unwrap();

        // 1. Writable AppImage atomic rename + stop/restart watchdog + relaunch.
        let appimage_dst = tmp.path().join("Ruum.AppImage");
        let appimage_dl = tmp.path().join("dl.AppImage");
        fs::write(&appimage_dst, b"old-appimage").unwrap();
        fs::write(&appimage_dl, b"new-appimage").unwrap();
        let appimage_env = UpdateEnv {
            os: Some(UpdateOs::Linux),
            exe_path: PathBuf::from("/tmp/.mount_abc/usr/bin/ruum-gui"),
            appimage_env: Some(appimage_dst.clone()),
            appdir_env: Some(PathBuf::from("/tmp/.mount_abc")),
            ld_library_path: Some(
                "/tmp/.mount_abc/usr/lib:/opt/custom/lib".to_owned(),
            ),
            home: Some(linux_home.clone()),
            writable_override: Some(true),
            ..UpdateEnv::default()
        };
        let appimage_runner = MockRunner::default();
        let spawner = MockSpawner::default();
        let res = adapter
            .apply_with_spawner(
                &appimage_env,
                InstallKind::LinuxAppImage,
                &appimage_dl,
                &manifest,
                &appimage_runner,
                &spawner,
            )
            .unwrap();
        assert!(matches!(res, ApplyOutcome::RestartRequired { .. }));
        assert_eq!(fs::read(&appimage_dst).unwrap(), b"new-appimage");
        let ai_spawn = &spawner.calls()[0];
        assert_eq!(ai_spawn.program, appimage_dst);
        for var in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
            assert!(
                ai_spawn.remove_envs.iter().any(|k| k == var),
                "missing remove_env {var}"
            );
        }
        assert_eq!(
            ai_spawn.envs,
            vec![("LD_LIBRARY_PATH".to_owned(), "/opt/custom/lib".to_owned())]
        );

        // When LD_LIBRARY_PATH only held $APPDIR/usr/lib, remove it outright.
        fs::write(&appimage_dl, b"new-appimage-2").unwrap();
        let solo_ld_env = UpdateEnv {
            ld_library_path: Some("/tmp/.mount_abc/usr/lib".to_owned()),
            ..appimage_env.clone()
        };
        let solo_spawner = MockSpawner::default();
        adapter
            .apply_with_spawner(
                &solo_ld_env,
                InstallKind::LinuxAppImage,
                &appimage_dl,
                &manifest,
                &MockRunner::default(),
                &solo_spawner,
            )
            .unwrap();
        let solo_spawn = &solo_spawner.calls()[0];
        assert!(
            solo_spawn
                .remove_envs
                .iter()
                .any(|k| k == "LD_LIBRARY_PATH")
        );
        assert!(solo_spawn.envs.is_empty());

        // Windows-style backslash/mixed-slash APPDIR still strips POSIX
        // $APPDIR/usr/lib via pure `/` normalization on any host OS.
        fs::write(&appimage_dl, b"new-appimage-3").unwrap();
        let mixed_ld_env = UpdateEnv {
            appdir_env: Some(PathBuf::from(r"\tmp\.mount_abc")),
            ld_library_path: Some(
                "/tmp/.mount_abc/usr/lib/:/opt/custom/lib".to_owned(),
            ),
            ..appimage_env
        };
        let mixed_spawner = MockSpawner::default();
        adapter
            .apply_with_spawner(
                &mixed_ld_env,
                InstallKind::LinuxAppImage,
                &appimage_dl,
                &manifest,
                &MockRunner::default(),
                &mixed_spawner,
            )
            .unwrap();
        let mixed_spawn = &mixed_spawner.calls()[0];
        assert_eq!(
            mixed_spawn.envs,
            vec![("LD_LIBRARY_PATH".to_owned(), "/opt/custom/lib".to_owned())]
        );

        let ai_calls = appimage_runner.calls();
        let ai_stop = ai_calls
            .iter()
            .position(|c| {
                c.0 == Path::new("systemctl")
                    && c.1.get(1).map(String::as_str) == Some("stop")
            })
            .expect("systemctl --user stop must run for AppImage");
        let ai_start = ai_calls
            .iter()
            .position(|c| {
                c.0 == Path::new("systemctl")
                    && c.1.get(1).map(String::as_str) == Some("start")
            })
            .expect("systemctl --user start must restart watchdog");
        assert!(ai_stop < ai_start);
        assert!(!ai_calls.iter().any(|c| c.1.iter().any(|a| a == "disable")));

        // 2. Writable LinuxTarball replaces binaries, runs install.sh, and
        //    stops/restarts watchdog.
        let bin_dir = tmp.path().join("local/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("ruum"), b"old-cli").unwrap();
        fs::write(bin_dir.join("ruum-gui"), b"old-gui").unwrap();
        let tarball_dl = tmp.path().join("ruum-0.3.0-x86_64-linux.tar.gz");
        fs::write(&tarball_dl, b"tarball").unwrap();
        let tar_runner = MockRunner {
            tarball_with_install_sh: true,
            ..MockRunner::default()
        };
        let tar_spawner = MockSpawner::default();
        let tar_env = UpdateEnv {
            os: Some(UpdateOs::Linux),
            exe_path: bin_dir.join("ruum-gui"),
            home: Some(linux_home),
            writable_override: Some(true),
            ..UpdateEnv::default()
        };
        let tar_res = adapter
            .apply_with_spawner(
                &tar_env,
                InstallKind::LinuxTarball,
                &tarball_dl,
                &manifest,
                &tar_runner,
                &tar_spawner,
            )
            .unwrap();
        assert!(matches!(tar_res, ApplyOutcome::RestartRequired { .. }));
        assert_eq!(fs::read(bin_dir.join("ruum")).unwrap(), b"new-linux-cli");
        assert_eq!(
            fs::read(bin_dir.join("ruum-gui")).unwrap(),
            b"new-linux-gui"
        );
        let t_calls = tar_runner.calls();
        assert!(t_calls.iter().any(|c| c.0 == Path::new("sh")));
        let t_stop = t_calls
            .iter()
            .position(|c| {
                c.0 == Path::new("systemctl")
                    && c.1.get(1).map(String::as_str) == Some("stop")
            })
            .expect("systemctl --user stop must run for Tarball");
        let t_start = t_calls
            .iter()
            .position(|c| {
                c.0 == Path::new("systemctl")
                    && c.1.get(1).map(String::as_str) == Some("start")
            })
            .expect("systemctl --user start must restart watchdog");
        assert!(t_stop < t_start);
        assert!(!t_calls.iter().any(|c| c.1.iter().any(|a| a == "disable")));
        assert!(unit_path.is_file());

        // 3. Read-only LinuxSystemPackage never uses sudo & recommends distro cmd.
        let arch_env = UpdateEnv {
            os: Some(UpdateOs::Linux),
            exe_path: PathBuf::from("/usr/bin/ruum-gui"),
            linux_os_release: Some("ID=omarchy\nID_LIKE=arch\n".to_owned()),
            ..UpdateEnv::default()
        };
        let sys_runner = MockRunner::default();
        let sys_spawner = MockSpawner::default();
        let sys_res = adapter
            .apply_with_spawner(
                &arch_env,
                InstallKind::LinuxSystemPackage,
                &tarball_dl,
                &manifest,
                &sys_runner,
                &sys_spawner,
            )
            .unwrap();
        assert_eq!(
            sys_res,
            ApplyOutcome::ManualActionRequired {
                command: "yay -S ruum-bin".to_owned(),
                url: Some(manifest.release_url.clone()),
            }
        );
        assert!(sys_runner.calls().is_empty());

        let deb_env = UpdateEnv {
            linux_os_release: Some("ID=ubuntu\nID_LIKE=debian\n".to_owned()),
            ..arch_env
        };
        assert_eq!(
            adapter.recommended_upgrade_command(
                &deb_env,
                InstallKind::LinuxSystemPackage,
                DEFAULT_UPDATE_BASE_URL,
                Some(&manifest),
            ),
            "sudo apt update && sudo apt install --only-upgrade ruum"
        );
    }

    #[test]
    fn appimage_relaunch_env_normalizes_mixed_and_trailing_slashes() {
        let base_expected_remove = vec!["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

        for appdir in [
            "/tmp/.mount_abc",
            "/tmp/.mount_abc/",
            "/tmp/.mount_abc//",
            r"\tmp\.mount_abc",
            r"/tmp/.mount_abc\",
            r"\tmp\.mount_abc\/",
        ] {
            let with_custom = UpdateEnv {
                appdir_env: Some(PathBuf::from(appdir)),
                ld_library_path: Some(
                    "/tmp/.mount_abc/usr/lib:/opt/custom/lib".to_owned(),
                ),
                ..UpdateEnv::default()
            };
            let (set_envs, remove_envs) = appimage_relaunch_env(&with_custom);
            assert_eq!(
                set_envs,
                vec![("LD_LIBRARY_PATH", "/opt/custom/lib".to_owned())],
                "appdir={appdir}"
            );
            assert_eq!(remove_envs, base_expected_remove, "appdir={appdir}");

            let solo_trailing = UpdateEnv {
                appdir_env: Some(PathBuf::from(appdir)),
                ld_library_path: Some("/tmp/.mount_abc/usr/lib/".to_owned()),
                ..UpdateEnv::default()
            };
            let (set_solo, remove_solo) = appimage_relaunch_env(&solo_trailing);
            assert!(set_solo.is_empty(), "appdir={appdir}");
            assert_eq!(
                remove_solo,
                vec!["APPDIR", "APPIMAGE", "ARGV0", "OWD", "LD_LIBRARY_PATH"],
                "appdir={appdir}"
            );
        }

        // Unrelated LD_LIBRARY_PATH stays untouched.
        let unrelated = UpdateEnv {
            appdir_env: Some(PathBuf::from(r"/tmp/.mount_abc\")),
            ld_library_path: Some("/opt/custom/lib:/usr/local/lib".to_owned()),
            ..UpdateEnv::default()
        };
        let (set_unrel, remove_unrel) = appimage_relaunch_env(&unrelated);
        assert!(set_unrel.is_empty());
        assert_eq!(remove_unrel, base_expected_remove);
    }
}
