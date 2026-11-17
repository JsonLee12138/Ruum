//! Tri-platform self-CLI registration (`ruum` in `PATH`).
//!
//! Encapsulates OS-specific CLI installation, status probing, and removal
//! behind [`CliInstallAdapter`] and [`CliInstallFactory`]:
//! - macOS: symlink `<usr_local_bin>/ruum` (with JXA `administratorPrivileges`
//!   fallback on `PermissionDenied`) and detect `~/.local/bin/ruum`
//! - Linux: symlink `~/.local/bin/ruum` and check `PATH` membership
//! - Windows: copy `ruum.exe` into `%LOCALAPPDATA%\Programs\ruum\bin` and
//!   manage User `Path` via `[Environment]::SetEnvironmentVariable`

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ai_cli::CommandRunner;

const MACOS_ADMIN_INSTALL_JXA: &str = "\
ObjC.import('Foundation');\
const env = $.NSProcessInfo.processInfo.environment;\
const src = ObjC.unwrap(env.objectForKey('RUUM_CLI_SRC'));\
const dst = ObjC.unwrap(env.objectForKey('RUUM_CLI_DST'));\
const dir = ObjC.unwrap(env.objectForKey('RUUM_CLI_DIR'));\
if (!src || !dst || !dir) { throw new Error('missing RUUM_CLI_* env'); }\
const q = (s) => \"'\" + String(s).replace(/'/g, \"'\\\\''\") + \"'\";\
const app = Application.currentApplication();\
app.includeStandardAdditions = true;\
app.doShellScript(\
'mkdir -p ' + q(dir) + ' && ln -sf ' + q(src) + ' ' + q(dst),\
{ administratorPrivileges: true }\
);";

const MACOS_ADMIN_UNINSTALL_JXA: &str = "\
ObjC.import('Foundation');\
const env = $.NSProcessInfo.processInfo.environment;\
const dst = ObjC.unwrap(env.objectForKey('RUUM_CLI_DST'));\
if (!dst) { throw new Error('missing RUUM_CLI_DST env'); }\
const q = (s) => \"'\" + String(s).replace(/'/g, \"'\\\\''\") + \"'\";\
const app = Application.currentApplication();\
app.includeStandardAdditions = true;\
app.doShellScript('rm -f ' + q(dst), { administratorPrivileges: true });";

const WIN_GET_USER_PATH_PS: &str =
    "[Environment]::GetEnvironmentVariable('Path', 'User')";

const WIN_ADD_USER_PATH_PS: &str = "\
$dir = $env:RUUM_BIN_DIR;\
$norm = $dir.Replace('/', '\\').TrimEnd('\\');\
$cur = [Environment]::GetEnvironmentVariable('Path', 'User');\
$parts = @();\
if ($cur) { $parts = @($cur -split ';' | Where-Object { $_.Trim() -ne '' }); }\
$exists = $false;\
foreach ($p in $parts) {\
  if ($p.Trim().Trim('\"').Replace('/', '\\').TrimEnd('\\') -ieq $norm) {\
    $exists = $true; break;\
  }\
}\
if (-not $exists) {\
  $parts += $dir;\
  $next = $parts -join ';';\
  [Environment]::SetEnvironmentVariable('Path', $next, 'User');\
}";

const WIN_REMOVE_USER_PATH_PS: &str = "\
$norm = $env:RUUM_BIN_DIR.Replace('/', '\\').TrimEnd('\\');\
$cur = [Environment]::GetEnvironmentVariable('Path', 'User');\
if ($cur) {\
  $kept = @($cur -split ';' | Where-Object {\
    $t = $_.Trim().Trim('\"');\
    $t -ne '' -and ($t.Replace('/', '\\').TrimEnd('\\') -ine $norm)\
  });\
  $next = $kept -join ';';\
  [Environment]::SetEnvironmentVariable('Path', $next, 'User');\
}";

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Operating system family for self-CLI `PATH` registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliInstallOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment context needed to resolve CLI install locations and `PATH`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliInstallEnv {
    pub home: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub path_var: Option<String>,
    pub usr_local_bin: PathBuf,
}

impl Default for CliInstallEnv {
    fn default() -> Self {
        Self {
            home: None,
            local_appdata: None,
            path_var: None,
            usr_local_bin: PathBuf::from("/usr/local/bin"),
        }
    }
}

impl CliInstallEnv {
    /// Capture the current process environment once for CLI operations.
    #[must_use]
    pub fn current() -> Self {
        let home = std::env::home_dir();
        let local_appdata = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty());
        let path_var = std::env::var("PATH").ok().filter(|s| !s.is_empty());
        Self {
            home,
            local_appdata,
            path_var,
            usr_local_bin: PathBuf::from("/usr/local/bin"),
        }
    }
}

/// Installation status of the `ruum` CLI command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliInstallStatus {
    Installed { path: PathBuf, in_path: bool },
    NotInstalled,
}

/// Adapter interface encapsulating OS-specific CLI registration lifecycle.
pub trait CliInstallAdapter: Send + Sync + std::fmt::Debug {
    fn os_kind(&self) -> CliInstallOs;
    fn status(
        &self,
        runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<CliInstallStatus>;
    fn install(
        &self,
        runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<PathBuf>;
    fn uninstall(
        &self,
        runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<()>;
}

/// macOS CLI installer (`<usr_local_bin>/ruum` + `~/.local/bin/ruum`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsCliInstaller;

impl MacOsCliInstaller {
    fn local_bin_in(env: &CliInstallEnv) -> Option<PathBuf> {
        env.home.as_ref().map(|h| h.join(".local").join("bin"))
    }
}

impl CliInstallAdapter for MacOsCliInstaller {
    fn os_kind(&self) -> CliInstallOs {
        CliInstallOs::MacOs
    }

    fn status(
        &self,
        _runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<CliInstallStatus> {
        let Ok(cli_exe) = resolve_cli_binary_for(self.os_kind(), current_exe)
        else {
            return Ok(CliInstallStatus::NotInstalled);
        };
        let usr_local_target = env.usr_local_bin.join("ruum");
        if path_matches_exe(&usr_local_target, &cli_exe) {
            return Ok(CliInstallStatus::Installed {
                path: usr_local_target,
                in_path: true,
            });
        }

        if let Some(local_bin) = Self::local_bin_in(env) {
            let local_target = local_bin.join("ruum");
            if path_matches_exe(&local_target, &cli_exe) {
                let in_path =
                    dir_in_path_unix(env.path_var.as_deref(), &local_bin);
                return Ok(CliInstallStatus::Installed {
                    path: local_target,
                    in_path,
                });
            }
        }

        Ok(CliInstallStatus::NotInstalled)
    }

    fn install(
        &self,
        runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<PathBuf> {
        let cli_exe = resolve_cli_binary_for(self.os_kind(), current_exe)?;
        let dst = env.usr_local_bin.join("ruum");
        match atomic_symlink(&cli_exe, &dst) {
            Ok(()) => Ok(dst),
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                let src_str = cli_exe.to_string_lossy();
                let dst_str = dst.to_string_lossy();
                let dir_str = env.usr_local_bin.to_string_lossy();
                let out = runner.run(
                    Path::new("osascript"),
                    &["-l", "JavaScript", "-e", MACOS_ADMIN_INSTALL_JXA],
                    &[
                        ("RUUM_CLI_SRC", src_str.as_ref()),
                        ("RUUM_CLI_DST", dst_str.as_ref()),
                        ("RUUM_CLI_DIR", dir_str.as_ref()),
                    ],
                )?;
                if out.success() {
                    Ok(dst)
                } else {
                    Err(io::Error::other(format!(
                        "osascript CLI install failed: {}",
                        out.stderr.trim()
                    )))
                }
            }
            Err(err) => Err(err),
        }
    }

    fn uninstall(
        &self,
        runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<()> {
        let cli_exe = resolve_cli_binary_for(self.os_kind(), current_exe)
            .unwrap_or_else(|_| current_exe.to_path_buf());
        let usr_dst = env.usr_local_bin.join("ruum");
        if fs::symlink_metadata(&usr_dst).is_ok() {
            match fs::remove_file(&usr_dst) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                    let dst_str = usr_dst.to_string_lossy();
                    let out = runner.run(
                        Path::new("osascript"),
                        &["-l", "JavaScript", "-e", MACOS_ADMIN_UNINSTALL_JXA],
                        &[("RUUM_CLI_DST", dst_str.as_ref())],
                    )?;
                    if !out.success() {
                        return Err(io::Error::other(format!(
                            "osascript CLI uninstall failed: {}",
                            out.stderr.trim()
                        )));
                    }
                }
                Err(err) => return Err(err),
            }
        }

        if let Some(local_bin) = Self::local_bin_in(env) {
            let local_dst = local_bin.join("ruum");
            if let Ok(meta) = fs::symlink_metadata(&local_dst)
                && meta.file_type().is_symlink()
                && path_matches_exe(&local_dst, &cli_exe)
            {
                fs::remove_file(&local_dst)?;
            }
        }

        Ok(())
    }
}

/// Linux CLI installer (`~/.local/bin/ruum` + system package detection).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxCliInstaller;

impl LinuxCliInstaller {
    fn local_bin_in(env: &CliInstallEnv) -> Option<PathBuf> {
        env.home.as_ref().map(|h| h.join(".local").join("bin"))
    }
}

impl CliInstallAdapter for LinuxCliInstaller {
    fn os_kind(&self) -> CliInstallOs {
        CliInstallOs::Linux
    }

    fn status(
        &self,
        _runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<CliInstallStatus> {
        let Ok(cli_exe) = resolve_cli_binary_for(self.os_kind(), current_exe)
        else {
            return Ok(CliInstallStatus::NotInstalled);
        };
        if cli_exe == Path::new("/usr/bin/ruum")
            || cli_exe == Path::new("/usr/local/bin/ruum")
        {
            return Ok(CliInstallStatus::Installed {
                path: cli_exe,
                in_path: true,
            });
        }

        if let Some(local_bin) = Self::local_bin_in(env) {
            let target = local_bin.join("ruum");
            if path_matches_exe(&target, &cli_exe) {
                let in_path =
                    dir_in_path_unix(env.path_var.as_deref(), &local_bin);
                return Ok(CliInstallStatus::Installed {
                    path: target,
                    in_path,
                });
            }
        }

        Ok(CliInstallStatus::NotInstalled)
    }

    fn install(
        &self,
        _runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<PathBuf> {
        let cli_exe = resolve_cli_binary_for(self.os_kind(), current_exe)?;
        let local_bin = Self::local_bin_in(env).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no home directory for ~/.local/bin",
            )
        })?;
        let target = local_bin.join("ruum");

        // Do not overwrite the running binary with a self-referential symlink.
        if is_same_non_symlink_binary(&target, &cli_exe) {
            return Ok(target);
        }

        atomic_symlink(&cli_exe, &target)?;
        Ok(target)
    }

    fn uninstall(
        &self,
        _runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<()> {
        let cli_exe = resolve_cli_binary_for(self.os_kind(), current_exe)
            .unwrap_or_else(|_| current_exe.to_path_buf());
        let local_bin = Self::local_bin_in(env).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no home directory for ~/.local/bin",
            )
        })?;
        let target = local_bin.join("ruum");

        match fs::symlink_metadata(&target) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
            Ok(meta) => {
                if !meta.file_type().is_symlink()
                    && paths_match_file_or_lexical(&target, &cli_exe)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "refusing to delete the running executable at {}",
                            target.display()
                        ),
                    ));
                }
                fs::remove_file(&target)
            }
        }
    }
}

/// Windows CLI installer (`%LOCALAPPDATA%\Programs\ruum\bin\ruum.exe` + User
/// `Path`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsCliInstaller;

impl WindowsCliInstaller {
    fn bin_dir_in(env: &CliInstallEnv) -> Option<PathBuf> {
        env.local_appdata
            .as_deref()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| {
                env.home.as_ref().map(|h| h.join("AppData").join("Local"))
            })
            .map(|base| base.join("Programs").join("ruum").join("bin"))
    }
}

impl CliInstallAdapter for WindowsCliInstaller {
    fn os_kind(&self) -> CliInstallOs {
        CliInstallOs::Windows
    }

    fn status(
        &self,
        runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        _current_exe: &Path,
    ) -> io::Result<CliInstallStatus> {
        let Some(bin_dir) = Self::bin_dir_in(env) else {
            return Ok(CliInstallStatus::NotInstalled);
        };
        let target_exe = bin_dir.join("ruum.exe");
        if !target_exe.is_file() {
            return Ok(CliInstallStatus::NotInstalled);
        }

        let queried_path = match runner.run(
            Path::new("powershell.exe"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                WIN_GET_USER_PATH_PS,
            ],
            &[],
        ) {
            Ok(out) if out.success() => Some(out.stdout),
            _ => env.path_var.clone(),
        };

        let in_path = dir_in_path_windows(queried_path.as_deref(), &bin_dir);
        Ok(CliInstallStatus::Installed {
            path: target_exe,
            in_path,
        })
    }

    fn install(
        &self,
        runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<PathBuf> {
        let cli_exe = resolve_cli_binary_for(self.os_kind(), current_exe)?;
        let bin_dir = Self::bin_dir_in(env).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no local appdata dir for Windows CLI install",
            )
        })?;
        let target_exe = bin_dir.join("ruum.exe");

        fs::create_dir_all(&bin_dir)?;
        if !paths_equal_windows(&cli_exe, &target_exe) {
            fs::copy(&cli_exe, &target_exe)?;
        }
        // Keep sibling `ruum-gui.exe` co-located so `ruum` launched via PATH
        // can still find and launch the desktop GUI.
        if let Some(src_dir) = cli_exe.parent() {
            let gui_src = src_dir.join("ruum-gui.exe");
            let gui_dst = bin_dir.join("ruum-gui.exe");
            if gui_src.is_file() && !paths_equal_windows(&gui_src, &gui_dst) {
                fs::copy(&gui_src, &gui_dst)?;
            }
        }

        let bin_dir_str = bin_dir.to_string_lossy();
        let out = runner.run(
            Path::new("powershell.exe"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                WIN_ADD_USER_PATH_PS,
            ],
            &[("RUUM_BIN_DIR", bin_dir_str.as_ref())],
        )?;
        if !out.success() {
            return Err(io::Error::other(format!(
                "powershell SetEnvironmentVariable failed: {}",
                out.stderr.trim()
            )));
        }

        Ok(target_exe)
    }

    fn uninstall(
        &self,
        runner: &dyn CommandRunner,
        env: &CliInstallEnv,
        current_exe: &Path,
    ) -> io::Result<()> {
        let cli_exe = resolve_cli_binary_for(self.os_kind(), current_exe)
            .unwrap_or_else(|_| current_exe.to_path_buf());
        let bin_dir = Self::bin_dir_in(env).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no local appdata dir for Windows CLI uninstall",
            )
        })?;
        let bin_dir_str = bin_dir.to_string_lossy();
        let out = runner.run(
            Path::new("powershell.exe"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                WIN_REMOVE_USER_PATH_PS,
            ],
            &[("RUUM_BIN_DIR", bin_dir_str.as_ref())],
        )?;
        if !out.success() {
            return Err(io::Error::other(format!(
                "powershell SetEnvironmentVariable failed: {}",
                out.stderr.trim()
            )));
        }

        let target_exe = bin_dir.join("ruum.exe");
        if !paths_equal_windows(&cli_exe, &target_exe) && target_exe.is_file() {
            let _ = fs::remove_file(&target_exe);
        }
        let gui_dst = bin_dir.join("ruum-gui.exe");
        if !paths_equal_windows(current_exe, &gui_dst) && gui_dst.is_file() {
            let _ = fs::remove_file(&gui_dst);
        }

        Ok(())
    }
}

/// Factory selecting the [`CliInstallAdapter`] for the host or a requested OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CliInstallFactory;

impl CliInstallFactory {
    /// Detect the current operating system family.
    #[must_use]
    pub const fn current_os() -> CliInstallOs {
        if cfg!(target_os = "macos") {
            CliInstallOs::MacOs
        } else if cfg!(windows) {
            CliInstallOs::Windows
        } else {
            CliInstallOs::Linux
        }
    }

    /// Instantiate the adapter for a specific target OS (supports cross-OS
    /// unit tests on any host).
    #[must_use]
    pub fn create_for(os: CliInstallOs) -> Box<dyn CliInstallAdapter> {
        match os {
            CliInstallOs::MacOs => Box::new(MacOsCliInstaller),
            CliInstallOs::Linux => Box::new(LinuxCliInstaller),
            CliInstallOs::Windows => Box::new(WindowsCliInstaller),
        }
    }

    /// Instantiate the adapter for the current host OS.
    #[must_use]
    pub fn create() -> Box<dyn CliInstallAdapter> {
        Self::create_for(Self::current_os())
    }
}

/// Resolve the `ruum` CLI executable from `current_exe`.
///
/// When invoked from `ruum-gui` (or `ruum-gui.exe`), resolves the co-located
/// sibling `ruum` (`ruum.exe`) after canonicalizing symlinks so `ruum-gui` is
/// never symlinked or copied under the `ruum` CLI name.
pub fn resolve_cli_binary_for(
    os: CliInstallOs,
    current_exe: &Path,
) -> io::Result<PathBuf> {
    let canonical = dunce::canonicalize(current_exe)
        .unwrap_or_else(|_| current_exe.to_path_buf());
    let is_gui = [current_exe, &canonical].iter().any(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("ruum-gui")
                    || name.eq_ignore_ascii_case("ruum-gui.exe")
            })
    });
    if !is_gui {
        return Ok(current_exe.to_path_buf());
    }

    let sibling_name = match os {
        CliInstallOs::Windows => "ruum.exe",
        CliInstallOs::MacOs | CliInstallOs::Linux => "ruum",
    };
    if let Some(parent) = canonical.parent() {
        let sibling = parent.join(sibling_name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "sibling `{sibling_name}` CLI binary not found next to {}",
            canonical.display()
        ),
    ))
}

/// Resolve the `ruum` CLI executable for the current host OS.
pub fn resolve_cli_binary(current_exe: &Path) -> io::Result<PathBuf> {
    resolve_cli_binary_for(CliInstallFactory::current_os(), current_exe)
}

/// Query the CLI installation status for the current host OS.
pub fn cli_status(
    runner: &dyn CommandRunner,
    current_exe: &Path,
) -> io::Result<CliInstallStatus> {
    CliInstallFactory::create().status(
        runner,
        &CliInstallEnv::current(),
        current_exe,
    )
}

/// Install or update the CLI command for the current host OS.
pub fn install_cli(
    runner: &dyn CommandRunner,
    current_exe: &Path,
) -> io::Result<PathBuf> {
    CliInstallFactory::create().install(
        runner,
        &CliInstallEnv::current(),
        current_exe,
    )
}

/// Uninstall the CLI command for the current host OS.
pub fn uninstall_cli(
    runner: &dyn CommandRunner,
    current_exe: &Path,
) -> io::Result<()> {
    CliInstallFactory::create().uninstall(
        runner,
        &CliInstallEnv::current(),
        current_exe,
    )
}

fn create_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(src, dst)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (src, dst);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symlinks are unsupported on this platform",
        ))
    }
}

fn atomic_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    let parent = dst.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "symlink destination has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;

    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp =
        parent.join(format!(".ruum-cli-link.{}.{seq}", std::process::id()));
    if let Err(err) = create_symlink(src, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = fs::rename(&tmp, dst) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

fn path_matches_exe(candidate: &Path, current_exe: &Path) -> bool {
    if fs::symlink_metadata(candidate).is_err() {
        return false;
    }
    if candidate == current_exe {
        return true;
    }
    if let Ok(link_target) = fs::read_link(candidate) {
        let resolved = if link_target.is_absolute() {
            link_target
        } else {
            candidate
                .parent()
                .map_or_else(|| link_target.clone(), |p| p.join(&link_target))
        };
        if resolved == current_exe {
            return true;
        }
        if let (Ok(c_res), Ok(c_exe)) =
            (fs::canonicalize(&resolved), fs::canonicalize(current_exe))
            && c_res == c_exe
        {
            return true;
        }
    }
    if let (Ok(c_cand), Ok(c_exe)) =
        (fs::canonicalize(candidate), fs::canonicalize(current_exe))
    {
        return c_cand == c_exe;
    }
    false
}

fn paths_match_file_or_lexical(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if let (Ok(ca), Ok(cb)) = (fs::canonicalize(a), fs::canonicalize(b)) {
        return ca == cb;
    }
    false
}

fn is_same_non_symlink_binary(target: &Path, current_exe: &Path) -> bool {
    fs::symlink_metadata(target)
        .is_ok_and(|meta| !meta.file_type().is_symlink())
        && paths_match_file_or_lexical(target, current_exe)
}

fn split_unix_path_entries(path_var: &str) -> Vec<String> {
    let parts: Vec<&str> = path_var.split(':').collect();
    let mut entries = Vec::new();
    let mut idx = 0;
    while idx < parts.len() {
        let cur = parts[idx].trim();
        // Keep `C:\...` Windows drive prefixes intact in cross-OS tests.
        if cur.len() == 1
            && cur.as_bytes()[0].is_ascii_alphabetic()
            && idx + 1 < parts.len()
            && parts[idx + 1].starts_with('\\')
        {
            entries.push(format!("{cur}:{}", parts[idx + 1].trim_end()));
            idx += 2;
        } else {
            if !cur.is_empty() {
                entries.push(cur.to_string());
            }
            idx += 1;
        }
    }
    entries
}

fn strip_trailing_slashes_unix(path: &Path) -> &str {
    let raw = path.to_str().unwrap_or_default();
    if raw == "/" {
        return "/";
    }
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\'
    {
        let mut end = trimmed.len();
        while end > 3 && bytes[end - 1] == b'\\' {
            end -= 1;
        }
        return &trimmed[..end];
    }
    trimmed
}

fn dir_in_path_unix(path_var: Option<&str>, dir: &Path) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    let target_norm = strip_trailing_slashes_unix(dir);
    let target_canon = fs::canonicalize(dir).ok();
    for entry in split_unix_path_entries(path_var) {
        if strip_trailing_slashes_unix(Path::new(&entry)) == target_norm {
            return true;
        }
        if let Some(ref tc) = target_canon
            && fs::canonicalize(&entry).ok().as_ref() == Some(tc)
        {
            return true;
        }
    }
    false
}

fn normalize_windows_path(raw: &str) -> String {
    let mut s = raw.trim().trim_matches('"').replace('\\', "/");
    if let Some(stripped) = s.strip_prefix("//?/") {
        s = stripped.to_string();
    }
    while s.len() > 1 && s.ends_with('/') {
        let bytes = s.as_bytes();
        // Preserve drive root like `C:/`.
        if bytes.len() == 3 && bytes[1] == b':' {
            break;
        }
        s.pop();
    }
    s
}

fn dir_in_path_windows(path_var: Option<&str>, dir: &Path) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    let target_norm = normalize_windows_path(&dir.to_string_lossy());
    for entry in path_var.split(';') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        if normalize_windows_path(trimmed).eq_ignore_ascii_case(&target_norm) {
            return true;
        }
    }
    false
}

fn paths_equal_windows(a: &Path, b: &Path) -> bool {
    if normalize_windows_path(&a.to_string_lossy())
        .eq_ignore_ascii_case(&normalize_windows_path(&b.to_string_lossy()))
    {
        return true;
    }
    if let (Ok(ca), Ok(cb)) = (fs::canonicalize(a), fs::canonicalize(b)) {
        return ca == cb;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_cli::CommandOutput;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    type RecordedCall = (PathBuf, Vec<String>, Vec<(String, String)>);

    #[derive(Default)]
    struct ScriptedRunner {
        calls: Mutex<Vec<RecordedCall>>,
        responses: Mutex<VecDeque<io::Result<CommandOutput>>>,
        default_output: Mutex<Option<CommandOutput>>,
    }

    impl ScriptedRunner {
        fn with_default(out: CommandOutput) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(VecDeque::new()),
                default_output: Mutex::new(Some(out)),
            }
        }

        fn with_results(seq: Vec<io::Result<CommandOutput>>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(VecDeque::from(seq)),
                default_output: Mutex::new(None),
            }
        }

        fn calls(&self) -> Vec<RecordedCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for ScriptedRunner {
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
            let next = self.responses.lock().unwrap().pop_front();
            if let Some(res) = next {
                return res;
            }
            let def = self.default_output.lock().unwrap().clone();
            if let Some(out) = def {
                return Ok(out);
            }
            Ok(CommandOutput {
                status_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn ok_out(stdout: &str) -> CommandOutput {
        CommandOutput {
            status_code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn err_out(code: i32, stderr: &str) -> CommandOutput {
        CommandOutput {
            status_code: code,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn env_default_sets_usr_local_bin() {
        let env = CliInstallEnv::default();
        assert_eq!(env.usr_local_bin, PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn factory_creates_all_three_os_adapters() {
        assert_eq!(
            CliInstallFactory::create_for(CliInstallOs::MacOs).os_kind(),
            CliInstallOs::MacOs
        );
        assert_eq!(
            CliInstallFactory::create_for(CliInstallOs::Linux).os_kind(),
            CliInstallOs::Linux
        );
        assert_eq!(
            CliInstallFactory::create_for(CliInstallOs::Windows).os_kind(),
            CliInstallOs::Windows
        );
        assert_eq!(
            CliInstallFactory::create().os_kind(),
            CliInstallFactory::current_os()
        );
    }

    #[test]
    fn macos_cli_installer_direct_install_status_and_uninstall() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let usr_local_bin = temp.path().join("usr_local_bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&usr_local_bin).unwrap();

        let exe_dir = temp.path().join("Ruum.app/Contents/MacOS");
        fs::create_dir_all(&exe_dir).unwrap();
        let current_exe = exe_dir.join("ruum");
        fs::write(&current_exe, b"bin").unwrap();

        let env = CliInstallEnv {
            home: Some(home),
            local_appdata: None,
            path_var: Some("/usr/bin:/bin".to_string()),
            usr_local_bin: usr_local_bin.clone(),
        };
        let adapter = CliInstallFactory::create_for(CliInstallOs::MacOs);
        let runner = ScriptedRunner::with_default(ok_out(""));

        assert_eq!(
            adapter.status(&runner, &env, &current_exe).unwrap(),
            CliInstallStatus::NotInstalled
        );

        let installed_path =
            adapter.install(&runner, &env, &current_exe).unwrap();
        let expected_dst = usr_local_bin.join("ruum");
        assert_eq!(installed_path, expected_dst);
        assert!(runner.calls().is_empty(), "writable dir needs no osascript");

        assert_eq!(
            adapter.status(&runner, &env, &current_exe).unwrap(),
            CliInstallStatus::Installed {
                path: expected_dst.clone(),
                in_path: true,
            }
        );

        // Symlink pointing to a different binary reports NotInstalled.
        let other_exe = temp.path().join("other-ruum");
        fs::write(&other_exe, b"other").unwrap();
        assert_eq!(
            adapter.status(&runner, &env, &other_exe).unwrap(),
            CliInstallStatus::NotInstalled
        );

        // Re-installing atomically updates symlink to the new target.
        let updated = adapter.install(&runner, &env, &other_exe).unwrap();
        assert_eq!(updated, expected_dst);
        assert_eq!(
            adapter.status(&runner, &env, &other_exe).unwrap(),
            CliInstallStatus::Installed {
                path: expected_dst.clone(),
                in_path: true,
            }
        );

        adapter.uninstall(&runner, &env, &other_exe).unwrap();
        assert!(fs::symlink_metadata(&expected_dst).is_err());
        assert_eq!(
            adapter.status(&runner, &env, &other_exe).unwrap(),
            CliInstallStatus::NotInstalled
        );
    }

    #[test]
    fn macos_cli_installer_detects_local_bin_and_path_membership() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let local_bin = home.join(".local").join("bin");
        let usr_local_bin = temp.path().join("usr_local_bin");
        fs::create_dir_all(&local_bin).unwrap();
        fs::create_dir_all(&usr_local_bin).unwrap();

        let current_exe = temp.path().join("ruum-bin");
        fs::write(&current_exe, b"bin").unwrap();
        let other_exe = temp.path().join("other-bin");
        fs::write(&other_exe, b"other").unwrap();
        let local_link = local_bin.join("ruum");
        create_symlink(&current_exe, &local_link).unwrap();

        let adapter = CliInstallFactory::create_for(CliInstallOs::MacOs);
        let runner = ScriptedRunner::with_default(ok_out(""));

        let env_missing_path = CliInstallEnv {
            home: Some(home.clone()),
            local_appdata: None,
            path_var: Some("/usr/local/bin:/usr/bin".to_string()),
            usr_local_bin: usr_local_bin.clone(),
        };
        assert_eq!(
            adapter
                .status(&runner, &env_missing_path, &current_exe)
                .unwrap(),
            CliInstallStatus::Installed {
                path: local_link.clone(),
                in_path: false,
            }
        );

        let env_in_path = CliInstallEnv {
            home: Some(home),
            local_appdata: None,
            path_var: Some(format!("/usr/bin:{}/:/bin", local_bin.display())),
            usr_local_bin,
        };
        assert_eq!(
            adapter.status(&runner, &env_in_path, &current_exe).unwrap(),
            CliInstallStatus::Installed {
                path: local_link.clone(),
                in_path: true,
            }
        );

        // Uninstall with a different current_exe leaves local_link intact.
        adapter
            .uninstall(&runner, &env_in_path, &other_exe)
            .unwrap();
        assert!(fs::symlink_metadata(&local_link).is_ok());

        // Uninstall with matching current_exe removes local_link.
        adapter
            .uninstall(&runner, &env_in_path, &current_exe)
            .unwrap();
        assert!(fs::symlink_metadata(&local_link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn macos_cli_installer_permission_denied_uses_env_jxa() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let readonly_dir = temp.path().join("readonly_bin");
        fs::create_dir_all(&readonly_dir).unwrap();
        let malicious_exe = temp.path().join("Ruum 'quoted' $(pwn)");
        fs::write(&malicious_exe, b"bin").unwrap();

        let mut perms = fs::metadata(&readonly_dir).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(&readonly_dir, perms).unwrap();

        let env = CliInstallEnv {
            home: Some(temp.path().join("home")),
            local_appdata: None,
            path_var: None,
            usr_local_bin: readonly_dir.clone(),
        };
        let adapter = CliInstallFactory::create_for(CliInstallOs::MacOs);
        let runner = ScriptedRunner::with_default(ok_out(""));

        let res = adapter.install(&runner, &env, &malicious_exe);

        // Restore write permission so tempdir cleans up cleanly.
        let mut restore = fs::metadata(&readonly_dir).unwrap().permissions();
        restore.set_mode(0o755);
        let _ = fs::set_permissions(&readonly_dir, restore);

        let dst = res.unwrap();
        assert_eq!(dst, readonly_dir.join("ruum"));

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("osascript"));
        assert_eq!(calls[0].1[0], "-l");
        assert_eq!(calls[0].1[1], "JavaScript");
        assert_eq!(calls[0].1[2], "-e");
        let script = &calls[0].1[3];
        assert!(script.contains("NSProcessInfo"));
        assert!(script.contains("administratorPrivileges"));
        assert!(
            !script.contains("pwn"),
            "JXA script must not interpolate path: {script}"
        );
        assert!(calls[0].2.iter().any(|(k, v)| {
            k == "RUUM_CLI_SRC" && v == &malicious_exe.to_string_lossy()
        }));
        assert!(calls[0].2.iter().any(|(k, v)| {
            k == "RUUM_CLI_DST" && v == &dst.to_string_lossy()
        }));
        assert!(calls[0].2.iter().any(|(k, v)| {
            k == "RUUM_CLI_DIR" && v == &readonly_dir.to_string_lossy()
        }));

        // Failed osascript propagates an error.
        let mut perms_fail = fs::metadata(&readonly_dir).unwrap().permissions();
        perms_fail.set_mode(0o555);
        fs::set_permissions(&readonly_dir, perms_fail).unwrap();
        let runner_fail = ScriptedRunner::with_default(err_out(1, "cancelled"));
        let err_res = adapter.install(&runner_fail, &env, &malicious_exe);
        let mut restore_fail =
            fs::metadata(&readonly_dir).unwrap().permissions();
        restore_fail.set_mode(0o755);
        let _ = fs::set_permissions(&readonly_dir, restore_fail);
        assert!(err_res.is_err());

        // Also test osascript fallback on uninstall when directory is read-only.
        let dst_file = readonly_dir.join("ruum");
        fs::write(&dst_file, b"link").unwrap();
        let mut perms2 = fs::metadata(&readonly_dir).unwrap().permissions();
        perms2.set_mode(0o555);
        fs::set_permissions(&readonly_dir, perms2).unwrap();

        let un_runner = ScriptedRunner::with_default(ok_out(""));
        let un_res = adapter.uninstall(&un_runner, &env, &malicious_exe);

        let mut restore2 = fs::metadata(&readonly_dir).unwrap().permissions();
        restore2.set_mode(0o755);
        let _ = fs::set_permissions(&readonly_dir, restore2);

        un_res.unwrap();
        let un_calls = un_runner.calls();
        assert_eq!(un_calls.len(), 1);
        assert_eq!(un_calls[0].0, PathBuf::from("osascript"));
        assert!(un_calls[0].2.iter().any(|(k, v)| {
            k == "RUUM_CLI_DST" && v == &dst_file.to_string_lossy()
        }));
    }

    #[test]
    fn linux_cli_installer_lifecycle_and_self_binary_guard() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let local_bin = home.join(".local").join("bin");
        let current_exe = temp.path().join("opt").join("ruum").join("ruum");
        fs::create_dir_all(current_exe.parent().unwrap()).unwrap();
        fs::write(&current_exe, b"bin").unwrap();

        let adapter = CliInstallFactory::create_for(CliInstallOs::Linux);
        let runner = ScriptedRunner::with_default(ok_out(""));

        // System-wide binary paths report Installed + in_path: true.
        let env = CliInstallEnv {
            home: Some(home),
            local_appdata: None,
            path_var: Some("/usr/bin".to_string()),
            usr_local_bin: PathBuf::from("/usr/local/bin"),
        };
        assert_eq!(
            adapter
                .status(&runner, &env, Path::new("/usr/bin/ruum"))
                .unwrap(),
            CliInstallStatus::Installed {
                path: PathBuf::from("/usr/bin/ruum"),
                in_path: true,
            }
        );
        assert_eq!(
            adapter
                .status(&runner, &env, Path::new("/usr/local/bin/ruum"))
                .unwrap(),
            CliInstallStatus::Installed {
                path: PathBuf::from("/usr/local/bin/ruum"),
                in_path: true,
            }
        );

        // NotInstalled initially for portable binary.
        assert_eq!(
            adapter.status(&runner, &env, &current_exe).unwrap(),
            CliInstallStatus::NotInstalled
        );

        // Install creates ~/.local/bin/ruum symlink.
        let target = adapter.install(&runner, &env, &current_exe).unwrap();
        assert_eq!(target, local_bin.join("ruum"));

        // Status detects missing ~/.local/bin in PATH, then present in PATH.
        assert_eq!(
            adapter.status(&runner, &env, &current_exe).unwrap(),
            CliInstallStatus::Installed {
                path: target.clone(),
                in_path: false,
            }
        );
        let env_in_path = CliInstallEnv {
            path_var: Some(format!("{}:/usr/bin", local_bin.display())),
            ..env.clone()
        };
        assert_eq!(
            adapter.status(&runner, &env_in_path, &current_exe).unwrap(),
            CliInstallStatus::Installed {
                path: target.clone(),
                in_path: true,
            }
        );

        // Uninstall removes symlink and is idempotent when already gone.
        adapter
            .uninstall(&runner, &env_in_path, &current_exe)
            .unwrap();
        assert!(fs::symlink_metadata(&target).is_err());
        adapter
            .uninstall(&runner, &env_in_path, &current_exe)
            .unwrap();

        // Self-binary guard: if ~/.local/bin/ruum is the running binary itself
        // (non-symlink), install is a no-op and uninstall refuses to delete it.
        fs::create_dir_all(&local_bin).unwrap();
        fs::write(&target, b"real-self-bin").unwrap();
        assert_eq!(adapter.install(&runner, &env, &target).unwrap(), target);
        assert!(target.is_file());
        let err = adapter.uninstall(&runner, &env, &target).unwrap_err();
        assert!(
            err.to_string().contains("refusing")
                || err.to_string().contains("running"),
            "unexpected error: {err}"
        );
        assert!(target.is_file());

        // Missing home directory returns NotFound error on install/uninstall.
        let no_home = CliInstallEnv::default();
        assert_eq!(
            adapter
                .install(&runner, &no_home, &current_exe)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn windows_cli_installer_lifecycle_and_path_matching() {
        let temp = tempfile::tempdir().unwrap();
        let local_appdata = temp.path().join("LocalAppData");
        let src_dir = temp.path().join("Portable");
        fs::create_dir_all(&src_dir).unwrap();
        let current_exe = src_dir.join("ruum.exe");
        fs::write(&current_exe, b"MZ-ruum").unwrap();

        let env = CliInstallEnv {
            home: Some(temp.path().join("home")),
            local_appdata: Some(local_appdata.clone()),
            path_var: None,
            usr_local_bin: PathBuf::from("/usr/local/bin"),
        };
        let adapter = CliInstallFactory::create_for(CliInstallOs::Windows);
        let expected_bin_dir =
            local_appdata.join("Programs").join("ruum").join("bin");
        let expected_exe = expected_bin_dir.join("ruum.exe");

        // 1. NotInstalled before install.
        let r_missing = ScriptedRunner::with_default(ok_out(""));
        assert_eq!(
            adapter.status(&r_missing, &env, &current_exe).unwrap(),
            CliInstallStatus::NotInstalled
        );

        // 2. Install copies binary and sets User Path via RUUM_BIN_DIR env.
        let r_install = ScriptedRunner::with_default(ok_out(""));
        let dst = adapter.install(&r_install, &env, &current_exe).unwrap();
        assert_eq!(dst, expected_exe);
        assert_eq!(fs::read(&expected_exe).unwrap(), b"MZ-ruum");
        let inst_calls = r_install.calls();
        assert_eq!(inst_calls.len(), 1);
        assert_eq!(inst_calls[0].0, PathBuf::from("powershell.exe"));
        assert!(inst_calls[0].2.iter().any(|(k, v)| {
            k == "RUUM_BIN_DIR" && v == &expected_bin_dir.to_string_lossy()
        }));
        assert!(inst_calls[0].1.iter().any(|a| {
            a.contains("SetEnvironmentVariable") && a.contains("User")
        }));

        // 3. Status detects case-insensitive, slash-normalized, extended-prefix
        //    User Path entry.
        let win_slash_path = expected_bin_dir
            .to_string_lossy()
            .replace('/', "\\")
            .to_uppercase();
        let r_in_path = ScriptedRunner::with_default(ok_out(&format!(
            "C:\\Windows\\System32;\\\\?\\{win_slash_path}\\\r\n"
        )));
        assert_eq!(
            adapter.status(&r_in_path, &env, &current_exe).unwrap(),
            CliInstallStatus::Installed {
                path: expected_exe.clone(),
                in_path: true,
            }
        );

        // 4. Status falls back to env.path_var if powershell fails.
        let r_ps_fail =
            ScriptedRunner::with_results(vec![Ok(err_out(1, "err"))]);
        let env_fallback = CliInstallEnv {
            path_var: Some(format!("C:\\Tools;{}", expected_bin_dir.display())),
            ..env.clone()
        };
        assert_eq!(
            adapter
                .status(&r_ps_fail, &env_fallback, &current_exe)
                .unwrap(),
            CliInstallStatus::Installed {
                path: expected_exe.clone(),
                in_path: true,
            }
        );

        // 5. Status reports in_path: false when absent from User Path.
        let r_not_in_path =
            ScriptedRunner::with_default(ok_out("C:\\Windows\\System32\n"));
        assert_eq!(
            adapter.status(&r_not_in_path, &env, &current_exe).unwrap(),
            CliInstallStatus::Installed {
                path: expected_exe.clone(),
                in_path: false,
            }
        );

        // 6. Install and uninstall skip self-copy / self-delete when running
        //    from target_exe itself.
        let r_self = ScriptedRunner::with_default(ok_out(""));
        adapter.install(&r_self, &env, &expected_exe).unwrap();
        assert!(expected_exe.is_file());
        adapter.uninstall(&r_self, &env, &expected_exe).unwrap();
        assert!(expected_exe.is_file());

        // 7. Uninstall removes User Path entry via powershell and deletes exe
        //    when current_exe != target_exe.
        let r_un = ScriptedRunner::with_default(ok_out(""));
        adapter.uninstall(&r_un, &env, &current_exe).unwrap();
        assert!(!expected_exe.exists());
        let un_calls = r_un.calls();
        assert_eq!(un_calls.len(), 1);
        assert_eq!(un_calls[0].0, PathBuf::from("powershell.exe"));
        assert!(un_calls[0].2.iter().any(|(k, v)| {
            k == "RUUM_BIN_DIR" && v == &expected_bin_dir.to_string_lossy()
        }));

        // 8. Fallback to <home>/AppData/Local when local_appdata is None.
        let env_home_fallback = CliInstallEnv {
            home: Some(temp.path().join("win_home")),
            local_appdata: None,
            path_var: None,
            usr_local_bin: PathBuf::from("/usr/local/bin"),
        };
        let r_fb = ScriptedRunner::with_default(ok_out(""));
        let fb_dst = adapter
            .install(&r_fb, &env_home_fallback, &current_exe)
            .unwrap();
        assert_eq!(
            fb_dst,
            temp.path()
                .join("win_home/AppData/Local/Programs/ruum/bin/ruum.exe")
        );
        assert!(fb_dst.is_file());

        // 9. PowerShell failure propagates error on install & uninstall.
        let r_err = ScriptedRunner::with_default(err_out(1, "access denied"));
        assert!(adapter.install(&r_err, &env, &current_exe).is_err());
        assert!(adapter.uninstall(&r_err, &env, &current_exe).is_err());
    }

    #[test]
    fn dir_in_path_unix_preserves_windows_drive_prefix_entries() {
        let win_dir = Path::new(
            "C:\\Users\\RUNNER~1\\AppData\\Local\\Temp\\home\\.local\\bin",
        );
        assert!(dir_in_path_unix(
            Some(
                "/usr/bin:C:\\Users\\RUNNER~1\\AppData\\Local\\Temp\\home\\\
                 .local\\bin/:/bin"
            ),
            win_dir,
        ));
        assert!(dir_in_path_unix(
            Some(
                "C:\\Users\\RUNNER~1\\AppData\\Local\\Temp\\home\\.local\\\
                 bin:/usr/bin"
            ),
            win_dir,
        ));
        assert!(dir_in_path_unix(
            Some(
                "/usr/bin:C:\\Users\\RUNNER~1\\AppData\\Local\\Temp\\home\\\
                 .local\\bin\\:/bin"
            ),
            win_dir,
        ));
        assert!(!dir_in_path_unix(Some("/usr/bin:/bin"), win_dir));
        assert!(dir_in_path_unix(Some("c:/usr/bin"), Path::new("/usr/bin")));
        assert!(dir_in_path_unix(Some("C:\\\\:/usr/bin"), Path::new("C:\\")));
    }

    #[test]
    fn gui_executable_resolves_sibling_cli_across_all_platforms() {
        let temp = tempfile::tempdir().unwrap();
        let app_dir = temp.path().join("bin_dir");
        fs::create_dir_all(&app_dir).unwrap();

        let gui_unix = app_dir.join("ruum-gui");
        fs::write(&gui_unix, b"gui").unwrap();

        // Missing sibling `ruum` fails closed instead of symlinking `ruum-gui`.
        for os in [CliInstallOs::MacOs, CliInstallOs::Linux] {
            let err = resolve_cli_binary_for(os, &gui_unix).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
        }

        let cli_unix = app_dir.join("ruum");
        fs::write(&cli_unix, b"cli").unwrap();

        for os in [CliInstallOs::MacOs, CliInstallOs::Linux] {
            let resolved = resolve_cli_binary_for(os, &gui_unix).unwrap();
            assert_eq!(
                fs::canonicalize(&resolved).unwrap(),
                fs::canonicalize(&cli_unix).unwrap()
            );
        }

        // Linux install called with `ruum-gui` links `ruum`, never `ruum-gui`.
        let home = temp.path().join("linux_home");
        fs::create_dir_all(&home).unwrap();
        let linux_env = CliInstallEnv {
            home: Some(home),
            local_appdata: None,
            path_var: None,
            usr_local_bin: temp.path().join("usr_local"),
        };
        let linux_adapter = CliInstallFactory::create_for(CliInstallOs::Linux);
        let runner = ScriptedRunner::with_default(ok_out(""));
        let installed = linux_adapter
            .install(&runner, &linux_env, &gui_unix)
            .unwrap();
        let target_target = fs::read_link(&installed).unwrap();
        assert_eq!(
            fs::canonicalize(target_target).unwrap(),
            fs::canonicalize(&cli_unix).unwrap()
        );
        assert!(matches!(
            linux_adapter
                .status(&runner, &linux_env, &gui_unix)
                .unwrap(),
            CliInstallStatus::Installed { .. }
        ));

        // Windows install from `ruum-gui.exe` copies `ruum.exe` and keeps
        // sibling `ruum-gui.exe` co-located in `%LOCALAPPDATA%\Programs\ruum\bin`.
        let win_src = temp.path().join("win_src");
        let local_appdata = temp.path().join("win_local");
        fs::create_dir_all(&win_src).unwrap();
        let gui_win = win_src.join("ruum-gui.exe");
        let cli_win = win_src.join("ruum.exe");
        fs::write(&gui_win, b"MZ-gui").unwrap();
        fs::write(&cli_win, b"MZ-cli").unwrap();

        let win_env = CliInstallEnv {
            home: Some(temp.path().join("win_home")),
            local_appdata: Some(local_appdata.clone()),
            path_var: None,
            usr_local_bin: PathBuf::from("/usr/local/bin"),
        };
        let win_adapter = CliInstallFactory::create_for(CliInstallOs::Windows);
        let win_dst = win_adapter.install(&runner, &win_env, &gui_win).unwrap();
        let expected_dir = local_appdata.join("Programs/ruum/bin");
        assert_eq!(win_dst, expected_dir.join("ruum.exe"));
        assert_eq!(fs::read(&win_dst).unwrap(), b"MZ-cli");
        assert_eq!(
            fs::read(expected_dir.join("ruum-gui.exe")).unwrap(),
            b"MZ-gui"
        );
    }
}
