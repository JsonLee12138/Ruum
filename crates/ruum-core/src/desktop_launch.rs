//! Tri-platform desktop GUI availability probe and detached launcher.
//!
//! When the unified `ruum` CLI is invoked with zero arguments, it uses
//! [`DesktopLaunchFactory`] to check whether a graphical desktop session is
//! reachable and a sibling/bundled `ruum-gui` binary exists. If both hold,
//! `ruum-gui` is launched detached and `ruum` exits cleanly; otherwise `ruum`
//! falls back to printing CLI help.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Operating system family for desktop GUI probing and launching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopLaunchOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment inputs captured for deterministic desktop launch detection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopLaunchEnv {
    pub current_exe: PathBuf,
    pub home: Option<PathBuf>,
    pub ruum_gui_bin: Option<PathBuf>,
    pub display: Option<String>,
    pub wayland_display: Option<String>,
    pub ssh_connection: Option<String>,
    pub path_var: Option<String>,
    pub system_applications_dir: Option<PathBuf>,
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn non_empty_path_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

impl DesktopLaunchEnv {
    /// Capture the current process environment once for desktop launch probing.
    #[must_use]
    pub fn current() -> Self {
        Self {
            current_exe: std::env::current_exe().unwrap_or_default(),
            home: std::env::home_dir(),
            ruum_gui_bin: non_empty_path_env("RUUM_GUI_BIN"),
            display: non_empty_env("DISPLAY"),
            wayland_display: non_empty_env("WAYLAND_DISPLAY"),
            ssh_connection: non_empty_env("SSH_CONNECTION"),
            path_var: non_empty_env("PATH"),
            system_applications_dir: Some(PathBuf::from("/Applications")),
        }
    }
}

/// Adapter contract for probing graphical reachability and launching `ruum-gui`.
pub trait DesktopLaunchAdapter: Send + Sync + std::fmt::Debug {
    fn os_kind(&self) -> DesktopLaunchOs;
    fn is_graphical_available(&self, env: &DesktopLaunchEnv) -> bool;
    fn resolve_gui_binary(&self, env: &DesktopLaunchEnv) -> Option<PathBuf>;

    fn launch_detached(
        &self,
        gui_bin: &Path,
        _env: &DesktopLaunchEnv,
    ) -> io::Result<()> {
        spawn_detached_gui(gui_bin)
    }

    fn launch_if_available(
        &self,
        env: &DesktopLaunchEnv,
    ) -> io::Result<Option<PathBuf>> {
        if !self.is_graphical_available(env) {
            return Ok(None);
        }
        let Some(gui_bin) = self.resolve_gui_binary(env) else {
            return Ok(None);
        };
        self.launch_detached(&gui_bin, env)?;
        Ok(Some(gui_bin))
    }
}

/// macOS desktop launcher (`ruum-gui` sibling or `ruum.app` bundle).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsDesktopLauncher;

impl DesktopLaunchAdapter for MacOsDesktopLauncher {
    fn os_kind(&self) -> DesktopLaunchOs {
        DesktopLaunchOs::MacOs
    }

    fn is_graphical_available(&self, env: &DesktopLaunchEnv) -> bool {
        !(env.ssh_connection.is_some() && env.display.is_none())
    }

    fn resolve_gui_binary(&self, env: &DesktopLaunchEnv) -> Option<PathBuf> {
        if let Some(override_bin) = &env.ruum_gui_bin {
            return override_bin.is_file().then(|| override_bin.clone());
        }
        let canonical = canonicalize_exe(&env.current_exe);
        if let Some(parent) = canonical.parent() {
            let sibling = parent.join("ruum-gui");
            if sibling.is_file() {
                return Some(sibling);
            }
            let local_bundle = parent.join("ruum.app/Contents/MacOS/ruum-gui");
            if local_bundle.is_file() {
                return Some(local_bundle);
            }
        }
        if let Some(home) = &env.home {
            let user_bundle =
                home.join("Applications/ruum.app/Contents/MacOS/ruum-gui");
            if user_bundle.is_file() {
                return Some(user_bundle);
            }
        }
        if let Some(apps_dir) = &env.system_applications_dir {
            let sys_bundle = apps_dir.join("ruum.app/Contents/MacOS/ruum-gui");
            if sys_bundle.is_file() {
                return Some(sys_bundle);
            }
        }
        find_in_path(env.path_var.as_deref(), ':', "ruum-gui")
    }
}

/// Linux desktop launcher (`WAYLAND_DISPLAY` / `DISPLAY` + sibling `ruum-gui`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxDesktopLauncher;

impl DesktopLaunchAdapter for LinuxDesktopLauncher {
    fn os_kind(&self) -> DesktopLaunchOs {
        DesktopLaunchOs::Linux
    }

    fn is_graphical_available(&self, env: &DesktopLaunchEnv) -> bool {
        env.wayland_display.is_some() || env.display.is_some()
    }

    fn resolve_gui_binary(&self, env: &DesktopLaunchEnv) -> Option<PathBuf> {
        if let Some(override_bin) = &env.ruum_gui_bin {
            return override_bin.is_file().then(|| override_bin.clone());
        }
        let canonical = canonicalize_exe(&env.current_exe);
        if let Some(parent) = canonical.parent() {
            let sibling = parent.join("ruum-gui");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
        find_in_path(env.path_var.as_deref(), ':', "ruum-gui")
    }
}

/// Windows desktop launcher (sibling `ruum-gui.exe` + `CREATE_NO_WINDOW`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsDesktopLauncher;

impl DesktopLaunchAdapter for WindowsDesktopLauncher {
    fn os_kind(&self) -> DesktopLaunchOs {
        DesktopLaunchOs::Windows
    }

    fn is_graphical_available(&self, env: &DesktopLaunchEnv) -> bool {
        env.ssh_connection.is_none()
    }

    fn resolve_gui_binary(&self, env: &DesktopLaunchEnv) -> Option<PathBuf> {
        if let Some(override_bin) = &env.ruum_gui_bin {
            return override_bin.is_file().then(|| override_bin.clone());
        }
        let canonical = canonicalize_exe(&env.current_exe);
        if let Some(parent) = canonical.parent() {
            let sibling = parent.join("ruum-gui.exe");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
        find_in_path(env.path_var.as_deref(), ';', "ruum-gui.exe")
    }
}

/// Factory selecting the [`DesktopLaunchAdapter`] for the host or given OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DesktopLaunchFactory;

impl DesktopLaunchFactory {
    /// Detect the current operating system family.
    #[must_use]
    pub const fn current_os() -> DesktopLaunchOs {
        if cfg!(target_os = "macos") {
            DesktopLaunchOs::MacOs
        } else if cfg!(windows) {
            DesktopLaunchOs::Windows
        } else {
            DesktopLaunchOs::Linux
        }
    }

    /// Construct the stateless launcher adapter for a specific OS.
    #[must_use]
    pub fn create_for(os: DesktopLaunchOs) -> Box<dyn DesktopLaunchAdapter> {
        match os {
            DesktopLaunchOs::MacOs => Box::new(MacOsDesktopLauncher),
            DesktopLaunchOs::Linux => Box::new(LinuxDesktopLauncher),
            DesktopLaunchOs::Windows => Box::new(WindowsDesktopLauncher),
        }
    }

    /// Construct the stateless launcher adapter for the current host OS.
    #[must_use]
    pub fn create() -> Box<dyn DesktopLaunchAdapter> {
        Self::create_for(Self::current_os())
    }
}

fn canonicalize_exe(exe: &Path) -> PathBuf {
    if exe.as_os_str().is_empty() {
        return PathBuf::new();
    }
    dunce::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf())
}

fn find_in_path(
    path_var: Option<&str>,
    sep: char,
    binary_name: &str,
) -> Option<PathBuf> {
    let raw = path_var?;
    for entry in raw.split(sep) {
        let trimmed = entry.trim().trim_matches('"');
        if trimmed.is_empty() {
            continue;
        }
        let candidate = Path::new(trimmed).join(binary_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn spawn_detached_gui(gui_bin: &Path) -> io::Result<()> {
    let mut cmd = Command::new(gui_bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    let _child = cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn factory_creates_all_three_adapters_unconditionally() {
        assert_eq!(
            DesktopLaunchFactory::create_for(DesktopLaunchOs::MacOs).os_kind(),
            DesktopLaunchOs::MacOs
        );
        assert_eq!(
            DesktopLaunchFactory::create_for(DesktopLaunchOs::Linux).os_kind(),
            DesktopLaunchOs::Linux
        );
        assert_eq!(
            DesktopLaunchFactory::create_for(DesktopLaunchOs::Windows)
                .os_kind(),
            DesktopLaunchOs::Windows
        );
        assert_eq!(
            DesktopLaunchFactory::create().os_kind(),
            DesktopLaunchFactory::current_os()
        );
    }

    #[test]
    fn linux_graphical_detection_and_sibling_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let cli_bin = dir.path().join("ruum");
        let gui_bin = dir.path().join("ruum-gui");
        fs::write(&cli_bin, b"cli").unwrap();
        fs::write(&gui_bin, b"gui").unwrap();

        let adapter = DesktopLaunchFactory::create_for(DesktopLaunchOs::Linux);

        let headless = DesktopLaunchEnv {
            current_exe: cli_bin,
            ..DesktopLaunchEnv::default()
        };
        assert!(!adapter.is_graphical_available(&headless));
        assert_eq!(
            adapter.resolve_gui_binary(&headless),
            Some(dunce::canonicalize(&gui_bin).unwrap())
        );

        let wayland = DesktopLaunchEnv {
            wayland_display: Some("wayland-0".to_owned()),
            ..headless.clone()
        };
        assert!(adapter.is_graphical_available(&wayland));

        let x11 = DesktopLaunchEnv {
            display: Some(":0".to_owned()),
            ..headless
        };
        assert!(adapter.is_graphical_available(&x11));

        // Explicit missing RUUM_GUI_BIN overrides sibling discovery -> None.
        let missing_override = DesktopLaunchEnv {
            ruum_gui_bin: Some(dir.path().join("does-not-exist")),
            ..x11
        };
        assert_eq!(adapter.resolve_gui_binary(&missing_override), None);
    }

    #[test]
    fn macos_graphical_detection_bundle_and_symlink_canonicalize() {
        let dir = tempfile::tempdir().unwrap();
        let macos_dir = dir.path().join("ruum.app/Contents/MacOS");
        let usr_bin = dir.path().join("usr_local_bin");
        fs::create_dir_all(&macos_dir).unwrap();
        fs::create_dir_all(&usr_bin).unwrap();

        let real_cli = macos_dir.join("ruum");
        let real_gui = macos_dir.join("ruum-gui");
        fs::write(&real_cli, b"cli").unwrap();
        fs::write(&real_gui, b"gui").unwrap();

        #[cfg(unix)]
        let linked_cli = {
            let link = usr_bin.join("ruum");
            std::os::unix::fs::symlink(&real_cli, &link).unwrap();
            link
        };
        #[cfg(not(unix))]
        let linked_cli = real_cli;

        let adapter = DesktopLaunchFactory::create_for(DesktopLaunchOs::MacOs);

        // Local session resolves sibling ruum-gui through symlink.
        let local = DesktopLaunchEnv {
            current_exe: linked_cli,
            ..DesktopLaunchEnv::default()
        };
        assert!(adapter.is_graphical_available(&local));
        assert_eq!(
            adapter.resolve_gui_binary(&local),
            Some(dunce::canonicalize(&real_gui).unwrap())
        );

        // SSH without DISPLAY is non-graphical on macOS.
        let ssh_headless = DesktopLaunchEnv {
            ssh_connection: Some("10.0.0.1 50000 10.0.0.2 22".to_owned()),
            ..local
        };
        assert!(!adapter.is_graphical_available(&ssh_headless));

        // SSH with DISPLAY is graphical on macOS.
        let ssh_display = DesktopLaunchEnv {
            display: Some(":10.0".to_owned()),
            ..ssh_headless
        };
        assert!(adapter.is_graphical_available(&ssh_display));

        // Also resolves bundled ruum.app inside HOME/Applications.
        let standalone_cli = dir.path().join("standalone/ruum");
        fs::create_dir_all(standalone_cli.parent().unwrap()).unwrap();
        fs::write(&standalone_cli, b"cli").unwrap();
        let home = dir.path().join("home");
        let home_gui =
            home.join("Applications/ruum.app/Contents/MacOS/ruum-gui");
        fs::create_dir_all(home_gui.parent().unwrap()).unwrap();
        fs::write(&home_gui, b"gui").unwrap();
        let home_bundle_env = DesktopLaunchEnv {
            current_exe: standalone_cli,
            home: Some(home),
            ..DesktopLaunchEnv::default()
        };
        assert_eq!(
            adapter.resolve_gui_binary(&home_bundle_env),
            Some(home_gui)
        );
    }

    #[test]
    fn windows_graphical_detection_and_sibling_exe_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let cli_exe = dir.path().join("ruum.exe");
        let gui_exe = dir.path().join("ruum-gui.exe");
        fs::write(&cli_exe, b"MZ-cli").unwrap();
        fs::write(&gui_exe, b"MZ-gui").unwrap();

        let adapter =
            DesktopLaunchFactory::create_for(DesktopLaunchOs::Windows);
        let env = DesktopLaunchEnv {
            current_exe: cli_exe.clone(),
            ..DesktopLaunchEnv::default()
        };
        assert!(adapter.is_graphical_available(&env));
        assert_eq!(
            adapter.resolve_gui_binary(&env),
            Some(dunce::canonicalize(&gui_exe).unwrap())
        );

        let ssh_env = DesktopLaunchEnv {
            ssh_connection: Some("1".to_owned()),
            ..env
        };
        assert!(!adapter.is_graphical_available(&ssh_env));

        let custom_gui = dir.path().join("custom-gui.exe");
        fs::write(&custom_gui, b"MZ-custom").unwrap();
        let override_env = DesktopLaunchEnv {
            current_exe: cli_exe,
            ruum_gui_bin: Some(custom_gui.clone()),
            ..DesktopLaunchEnv::default()
        };
        assert_eq!(adapter.resolve_gui_binary(&override_env), Some(custom_gui));
    }

    #[test]
    fn launch_if_available_returns_none_without_display_or_gui() {
        let dir = tempfile::tempdir().unwrap();
        let cli_bin = dir.path().join("ruum");
        fs::write(&cli_bin, b"cli").unwrap();

        let env = DesktopLaunchEnv {
            current_exe: cli_bin,
            ssh_connection: Some("1".to_owned()),
            ..DesktopLaunchEnv::default()
        };
        let adapter = DesktopLaunchFactory::create();
        assert_eq!(adapter.launch_if_available(&env).unwrap(), None);
    }

    #[test]
    fn launch_if_available_spawns_detached_gui_stub() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("gui-started.txt");

        #[cfg(unix)]
        let stub = {
            use std::os::unix::fs::PermissionsExt as _;
            let p = dir.path().join("ruum-gui");
            let script = format!(
                "#!/bin/sh\nprintf 'launched\\n' > \"{}\"\n",
                marker.display()
            );
            fs::write(&p, script).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        #[cfg(windows)]
        let stub = {
            let p = dir.path().join("ruum-gui.cmd");
            let script = format!(
                "@echo off\r\necho launched> \"{}\"\r\n",
                marker.display()
            );
            fs::write(&p, script).unwrap();
            p
        };

        let env = DesktopLaunchEnv {
            ruum_gui_bin: Some(stub.clone()),
            display: Some(":0".to_owned()),
            ..DesktopLaunchEnv::default()
        };
        let launched = DesktopLaunchFactory::create()
            .launch_if_available(&env)
            .unwrap();
        assert_eq!(launched, Some(stub));

        let start = Instant::now();
        while !marker.exists() {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "detached stub did not write marker in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
