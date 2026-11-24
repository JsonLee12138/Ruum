//! Tri-platform system login autostart registration (`ruum-gui --background`).
//!
//! Encapsulates OS-specific login autostart behind [`AutostartAdapter`] and
//! [`AutostartFactory`]:
//! - macOS: `SMAppService::mainAppService()` via [`MacOsLoginItemBridge`] for
//!   signed `.app` bundles on macOS 13+, with automatic fallback to
//!   `~/Library/LaunchAgents/com.ruum.gui.plist` on unsigned/non-bundle or
//!   pre-Ventura systems
//! - Linux: `${XDG_CONFIG_HOME:-~/.config}/autostart/ruum.desktop`
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `Ruum`

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ai_cli::CommandRunner;

const MACOS_GUI_LABEL: &str = "com.ruum.gui";
const LINUX_DESKTOP_NAME: &str = "ruum.desktop";
const WIN_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const WIN_RUN_VALUE: &str = "Ruum";

/// Operating system family for login autostart management.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutostartOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment context needed to resolve autostart registration paths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutostartEnv {
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub macos_major_version: Option<u32>,
    pub is_macos_app_bundle: bool,
}

impl AutostartEnv {
    /// Capture the current process environment once for autostart operations.
    #[must_use]
    pub fn current() -> Self {
        let home = std::env::home_dir();
        let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty() && p.is_absolute());
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty());
        let is_macos_app_bundle = if cfg!(target_os = "macos") {
            std::env::current_exe()
                .ok()
                .is_some_and(|p| is_macos_app_bundle_exe(&p))
        } else {
            false
        };
        Self {
            home,
            xdg_config_home,
            appdata,
            macos_major_version: None,
            is_macos_app_bundle,
        }
    }
}

fn is_macos_app_bundle_exe(exe: &Path) -> bool {
    let normalized = exe.to_string_lossy().replace('\\', "/");
    normalized.contains(".app/Contents/MacOS/")
}

/// Current state of login autostart registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutostartStatus {
    Enabled,
    Disabled,
    RequiresApproval,
}

/// Injectable macOS `SMAppService` bridge implemented in `ruum-app` (or mocked
/// in unit tests).
pub trait MacOsLoginItemBridge: Send + Sync + fmt::Debug {
    fn register(&self) -> io::Result<AutostartStatus>;
    fn unregister(&self) -> io::Result<()>;
    fn status(&self) -> io::Result<AutostartStatus>;
}

/// Adapter interface encapsulating OS-specific login autostart lifecycle.
pub trait AutostartAdapter: Send + Sync + fmt::Debug {
    fn os_kind(&self) -> AutostartOs;
    fn enable(
        &self,
        runner: &dyn CommandRunner,
        env: &AutostartEnv,
        gui_exe: &Path,
    ) -> io::Result<AutostartStatus>;
    fn disable(
        &self,
        runner: &dyn CommandRunner,
        env: &AutostartEnv,
    ) -> io::Result<()>;
    fn status(
        &self,
        runner: &dyn CommandRunner,
        env: &AutostartEnv,
    ) -> io::Result<AutostartStatus>;
}

/// macOS login autostart adapter (`SMAppService` + `LaunchAgents` fallback).
#[derive(Clone, Debug, Default)]
pub struct MacOsAutostartAdapter {
    bridge: Option<Arc<dyn MacOsLoginItemBridge>>,
}

impl MacOsAutostartAdapter {
    /// Create a macOS adapter without an `SMAppService` bridge (always uses
    /// `LaunchAgents` plist fallback).
    #[must_use]
    pub const fn new() -> Self {
        Self { bridge: None }
    }

    /// Create a macOS adapter with an injected `SMAppService` bridge.
    #[must_use]
    pub fn with_bridge(bridge: Arc<dyn MacOsLoginItemBridge>) -> Self {
        Self {
            bridge: Some(bridge),
        }
    }

    fn plist_path_in(env: &AutostartEnv) -> Option<PathBuf> {
        env.home
            .as_ref()
            .map(|h| h.join("Library/LaunchAgents/com.ruum.gui.plist"))
    }

    const fn uses_sm_app_service(&self, env: &AutostartEnv) -> bool {
        env.is_macos_app_bundle
            && matches!(env.macos_major_version, Some(v) if v >= 13)
            && self.bridge.is_some()
    }

    fn install_launch_agent(
        runner: &dyn CommandRunner,
        env: &AutostartEnv,
        gui_exe: &Path,
    ) -> io::Result<()> {
        let plist_path = Self::plist_path_in(env).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no LaunchAgents dir")
        })?;
        if let Some(parent) = plist_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&plist_path, macos_gui_launch_agent_plist_content(gui_exe))?;
        let path_str = plist_path.to_string_lossy();
        let _ = runner.run(
            Path::new("launchctl"),
            &["unload", "-w", &path_str],
            &[],
        );
        let out = runner.run(
            Path::new("launchctl"),
            &["load", "-w", &path_str],
            &[],
        )?;
        if out.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "launchctl load failed: {}",
                out.stderr
            )))
        }
    }

    fn remove_launch_agent(
        runner: &dyn CommandRunner,
        env: &AutostartEnv,
    ) -> io::Result<()> {
        let Some(plist_path) = Self::plist_path_in(env) else {
            return Ok(());
        };
        if plist_path.exists() {
            let path_str = plist_path.to_string_lossy();
            let _ = runner.run(
                Path::new("launchctl"),
                &["unload", "-w", &path_str],
                &[],
            );
            match fs::remove_file(&plist_path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
}

impl AutostartAdapter for MacOsAutostartAdapter {
    fn os_kind(&self) -> AutostartOs {
        AutostartOs::MacOs
    }

    fn enable(
        &self,
        runner: &dyn CommandRunner,
        env: &AutostartEnv,
        gui_exe: &Path,
    ) -> io::Result<AutostartStatus> {
        if self.uses_sm_app_service(env)
            && let Some(bridge) = &self.bridge
        {
            if let Ok(status) = bridge.register() {
                Self::remove_launch_agent(runner, env)?;
                return Ok(status);
            }
            // Unsigned / ad-hoc app bundles return `kSMErrorInvalidSignature`;
            // fall back transparently to the user LaunchAgent plist.
            Self::install_launch_agent(runner, env, gui_exe)?;
            return Ok(AutostartStatus::Enabled);
        }
        Self::install_launch_agent(runner, env, gui_exe)?;
        Ok(AutostartStatus::Enabled)
    }

    fn disable(
        &self,
        runner: &dyn CommandRunner,
        env: &AutostartEnv,
    ) -> io::Result<()> {
        let bridge_res = if self.uses_sm_app_service(env)
            && let Some(bridge) = &self.bridge
        {
            bridge.unregister()
        } else {
            Ok(())
        };
        Self::remove_launch_agent(runner, env)?;
        bridge_res
    }

    fn status(
        &self,
        _runner: &dyn CommandRunner,
        env: &AutostartEnv,
    ) -> io::Result<AutostartStatus> {
        if self.uses_sm_app_service(env)
            && let Some(bridge) = &self.bridge
            && let Ok(status) = bridge.status()
            && status != AutostartStatus::Disabled
        {
            return Ok(status);
        }
        if Self::plist_path_in(env).is_some_and(|p| p.exists()) {
            Ok(AutostartStatus::Enabled)
        } else {
            Ok(AutostartStatus::Disabled)
        }
    }
}

/// Linux XDG autostart `.desktop` adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxAutostartAdapter;

impl LinuxAutostartAdapter {
    fn desktop_path_in(env: &AutostartEnv) -> Option<PathBuf> {
        if let Some(ref xdg) = env.xdg_config_home
            && xdg.is_absolute()
        {
            return Some(xdg.join("autostart").join(LINUX_DESKTOP_NAME));
        }
        env.home
            .as_ref()
            .map(|h| h.join(".config/autostart").join(LINUX_DESKTOP_NAME))
    }
}

impl AutostartAdapter for LinuxAutostartAdapter {
    fn os_kind(&self) -> AutostartOs {
        AutostartOs::Linux
    }

    fn enable(
        &self,
        _runner: &dyn CommandRunner,
        env: &AutostartEnv,
        gui_exe: &Path,
    ) -> io::Result<AutostartStatus> {
        let desktop_path = Self::desktop_path_in(env).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no XDG autostart dir")
        })?;
        if let Some(parent) = desktop_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&desktop_path, linux_autostart_desktop_content(gui_exe))?;
        Ok(AutostartStatus::Enabled)
    }

    fn disable(
        &self,
        _runner: &dyn CommandRunner,
        env: &AutostartEnv,
    ) -> io::Result<()> {
        let Some(desktop_path) = Self::desktop_path_in(env) else {
            return Ok(());
        };
        if desktop_path.exists() {
            match fs::remove_file(&desktop_path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    fn status(
        &self,
        _runner: &dyn CommandRunner,
        env: &AutostartEnv,
    ) -> io::Result<AutostartStatus> {
        if Self::desktop_path_in(env).is_some_and(|p| p.exists()) {
            Ok(AutostartStatus::Enabled)
        } else {
            Ok(AutostartStatus::Disabled)
        }
    }
}

/// Windows `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsAutostartAdapter;

impl AutostartAdapter for WindowsAutostartAdapter {
    fn os_kind(&self) -> AutostartOs {
        AutostartOs::Windows
    }

    fn enable(
        &self,
        runner: &dyn CommandRunner,
        _env: &AutostartEnv,
        gui_exe: &Path,
    ) -> io::Result<AutostartStatus> {
        let reg_data = format!("\"{}\" --background", gui_exe.display());
        let out = runner.run(
            Path::new("reg"),
            &[
                "add",
                WIN_RUN_KEY,
                "/v",
                WIN_RUN_VALUE,
                "/t",
                "REG_SZ",
                "/d",
                &reg_data,
                "/f",
            ],
            &[],
        )?;
        if out.success() {
            Ok(AutostartStatus::Enabled)
        } else {
            Err(io::Error::other(format!("reg add failed: {}", out.stderr)))
        }
    }

    fn disable(
        &self,
        runner: &dyn CommandRunner,
        _env: &AutostartEnv,
    ) -> io::Result<()> {
        let _ = runner.run(
            Path::new("reg"),
            &["delete", WIN_RUN_KEY, "/v", WIN_RUN_VALUE, "/f"],
            &[],
        );
        Ok(())
    }

    fn status(
        &self,
        runner: &dyn CommandRunner,
        _env: &AutostartEnv,
    ) -> io::Result<AutostartStatus> {
        let out = runner.run(
            Path::new("reg"),
            &["query", WIN_RUN_KEY, "/v", WIN_RUN_VALUE],
            &[],
        )?;
        if out.success() {
            Ok(AutostartStatus::Enabled)
        } else {
            Ok(AutostartStatus::Disabled)
        }
    }
}

/// Factory selecting the login [`AutostartAdapter`] for the host or a
/// requested target OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutostartFactory;

impl AutostartFactory {
    /// Detect the current operating system family.
    #[must_use]
    pub const fn current_os() -> AutostartOs {
        if cfg!(target_os = "macos") {
            AutostartOs::MacOs
        } else if cfg!(windows) {
            AutostartOs::Windows
        } else {
            AutostartOs::Linux
        }
    }

    /// Instantiate the adapter for a specific target OS without an injected
    /// macOS `SMAppService` bridge.
    #[must_use]
    pub fn create_for(os: AutostartOs) -> Box<dyn AutostartAdapter> {
        Self::create_for_with_macos_bridge(os, None)
    }

    /// Instantiate the adapter for a specific target OS with an optional
    /// macOS `SMAppService` bridge.
    #[must_use]
    pub fn create_for_with_macos_bridge(
        os: AutostartOs,
        bridge: Option<Arc<dyn MacOsLoginItemBridge>>,
    ) -> Box<dyn AutostartAdapter> {
        match os {
            AutostartOs::MacOs => Box::new(MacOsAutostartAdapter { bridge }),
            AutostartOs::Linux => Box::new(LinuxAutostartAdapter),
            AutostartOs::Windows => Box::new(WindowsAutostartAdapter),
        }
    }

    /// Instantiate the adapter for the current host OS without an injected
    /// macOS `SMAppService` bridge.
    #[must_use]
    pub fn create() -> Box<dyn AutostartAdapter> {
        Self::create_for(Self::current_os())
    }

    /// Instantiate the adapter for the current host OS with an optional
    /// macOS `SMAppService` bridge.
    #[must_use]
    pub fn create_with_macos_bridge(
        bridge: Option<Arc<dyn MacOsLoginItemBridge>>,
    ) -> Box<dyn AutostartAdapter> {
        Self::create_for_with_macos_bridge(Self::current_os(), bridge)
    }
}

fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
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

fn desktop_quote_exec(exe_path: &Path) -> String {
    let raw = exe_path.to_string_lossy();
    let needs_quote = raw.is_empty()
        || raw.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\''
                        | '\\'
                        | '>'
                        | '<'
                        | '~'
                        | '|'
                        | '&'
                        | ';'
                        | '$'
                        | '*'
                        | '?'
                        | '#'
                        | '('
                        | ')'
                        | '`'
                        | '%'
                )
        });
    if !needs_quote {
        return raw.into_owned();
    }
    let mut out = String::with_capacity(raw.len() + 4);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '`' => out.push_str("\\`"),
            '%' => out.push_str("%%"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Path to the macOS fallback `LaunchAgents` plist for `ruum-gui`.
#[must_use]
pub fn macos_gui_launch_agent_plist_path() -> Option<PathBuf> {
    MacOsAutostartAdapter::plist_path_in(&AutostartEnv::current())
}

/// Content template for the macOS `LaunchAgents` plist launching `ruum-gui`.
#[must_use]
pub fn macos_gui_launch_agent_plist_content(gui_exe: &Path) -> String {
    let escaped = xml_escape(&gui_exe.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
             <key>Label</key>\n\
             <string>{MACOS_GUI_LABEL}</string>\n\
             <key>ProgramArguments</key>\n\
             <array>\n\
                 <string>{escaped}</string>\n\
                 <string>--background</string>\n\
             </array>\n\
             <key>KeepAlive</key>\n\
             <false/>\n\
             <key>RunAtLoad</key>\n\
             <true/>\n\
         </dict>\n\
         </plist>\n",
    )
}

/// Path to the Linux XDG autostart `.desktop` entry for `ruum-gui`.
#[must_use]
pub fn linux_autostart_desktop_path() -> Option<PathBuf> {
    LinuxAutostartAdapter::desktop_path_in(&AutostartEnv::current())
}

/// Content template for the Linux XDG autostart `.desktop` entry.
#[must_use]
pub fn linux_autostart_desktop_content(gui_exe: &Path) -> String {
    let quoted = desktop_quote_exec(gui_exe);
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Ruum\n\
         Comment=Disk Usage Explorer and Watchdog\n\
         Exec={quoted} --background\n\
         Icon=ruum\n\
         Terminal=false\n\
         Categories=Utility;System;\n\
         X-GNOME-Autostart-enabled=true\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_cli::CommandOutput;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockRunner {
        calls: Mutex<Vec<(PathBuf, Vec<String>)>>,
        fail_on: Mutex<Option<String>>,
        query_success: Mutex<bool>,
    }

    impl MockRunner {
        fn with_query_success(success: bool) -> Self {
            Self {
                query_success: Mutex::new(success),
                ..Self::default()
            }
        }

        fn calls(&self) -> Vec<(PathBuf, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for MockRunner {
        fn run(
            &self,
            program: &Path,
            args: &[&str],
            _envs: &[(&str, &str)],
        ) -> io::Result<CommandOutput> {
            let arg_strings: Vec<String> =
                args.iter().map(|s| (*s).to_string()).collect();
            self.calls
                .lock()
                .unwrap()
                .push((program.to_path_buf(), arg_strings.clone()));
            if let Some(ref needle) = *self.fail_on.lock().unwrap()
                && arg_strings.iter().any(|a| a.contains(needle))
            {
                return Ok(CommandOutput {
                    status_code: 1,
                    stdout: String::new(),
                    stderr: "mock failure".to_owned(),
                });
            }
            if program == Path::new("reg")
                && args.first().copied() == Some("query")
            {
                let ok = *self.query_success.lock().unwrap();
                return Ok(CommandOutput {
                    status_code: i32::from(!ok),
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            Ok(CommandOutput {
                status_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    #[derive(Debug)]
    struct MockMacBridge {
        register_outcome: Mutex<io::Result<AutostartStatus>>,
        status_outcome: Mutex<io::Result<AutostartStatus>>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl MockMacBridge {
        fn ok_enabled() -> Self {
            Self {
                register_outcome: Mutex::new(Ok(AutostartStatus::Enabled)),
                status_outcome: Mutex::new(Ok(AutostartStatus::Enabled)),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn failing_signature() -> Self {
            Self {
                register_outcome: Mutex::new(Err(io::Error::other(
                    "SMAppService register failed (3): Invalid signature",
                ))),
                status_outcome: Mutex::new(Ok(AutostartStatus::Disabled)),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn requires_approval() -> Self {
            Self {
                register_outcome: Mutex::new(Ok(
                    AutostartStatus::RequiresApproval,
                )),
                status_outcome: Mutex::new(Ok(
                    AutostartStatus::RequiresApproval,
                )),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl MacOsLoginItemBridge for MockMacBridge {
        fn register(&self) -> io::Result<AutostartStatus> {
            self.calls.lock().unwrap().push("register");
            match &*self.register_outcome.lock().unwrap() {
                Ok(s) => Ok(*s),
                Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
            }
        }

        fn unregister(&self) -> io::Result<()> {
            self.calls.lock().unwrap().push("unregister");
            Ok(())
        }

        fn status(&self) -> io::Result<AutostartStatus> {
            self.calls.lock().unwrap().push("status");
            match &*self.status_outcome.lock().unwrap() {
                Ok(s) => Ok(*s),
                Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
            }
        }
    }

    #[test]
    fn macos_13_bundle_uses_sm_app_service_and_cleans_legacy_gui_plist() {
        let temp = tempfile::tempdir().unwrap();
        let env = AutostartEnv {
            home: Some(temp.path().to_path_buf()),
            macos_major_version: Some(14),
            is_macos_app_bundle: true,
            ..AutostartEnv::default()
        };
        let plist =
            MacOsAutostartAdapter::plist_path_in(&env).expect("plist path");
        fs::create_dir_all(plist.parent().unwrap()).unwrap();
        fs::write(&plist, "stale").unwrap();

        let bridge = Arc::new(MockMacBridge::ok_enabled());
        let adapter = AutostartFactory::create_for_with_macos_bridge(
            AutostartOs::MacOs,
            Some(bridge.clone()),
        );
        assert_eq!(adapter.os_kind(), AutostartOs::MacOs);

        let runner = MockRunner::default();
        let status = adapter
            .enable(
                &runner,
                &env,
                Path::new("/Applications/ruum.app/Contents/MacOS/ruum-gui"),
            )
            .unwrap();
        assert_eq!(status, AutostartStatus::Enabled);
        assert!(!plist.exists(), "stale LaunchAgent plist must be removed");
        assert_eq!(*bridge.calls.lock().unwrap(), vec!["register"]);
        assert_eq!(
            adapter.status(&runner, &env).unwrap(),
            AutostartStatus::Enabled
        );

        adapter.disable(&runner, &env).unwrap();
        assert_eq!(
            *bridge.calls.lock().unwrap(),
            vec!["register", "status", "unregister"]
        );
    }

    #[test]
    fn macos_13_bundle_falls_back_to_launch_agent_on_signature_error() {
        let temp = tempfile::tempdir().unwrap();
        let env = AutostartEnv {
            home: Some(temp.path().to_path_buf()),
            macos_major_version: Some(15),
            is_macos_app_bundle: true,
            ..AutostartEnv::default()
        };
        let bridge = Arc::new(MockMacBridge::failing_signature());
        let adapter = AutostartFactory::create_for_with_macos_bridge(
            AutostartOs::MacOs,
            Some(bridge),
        );
        let runner = MockRunner::default();
        let exe = Path::new("/Applications/ruum.app/Contents/MacOS/ruum-gui");

        let status = adapter.enable(&runner, &env, exe).unwrap();
        assert_eq!(status, AutostartStatus::Enabled);

        let plist =
            MacOsAutostartAdapter::plist_path_in(&env).expect("plist path");
        assert!(plist.exists(), "fallback LaunchAgent plist must be written");
        let content = fs::read_to_string(&plist).unwrap();
        assert!(content.contains("<string>com.ruum.gui</string>"));
        assert!(content.contains("<string>--background</string>"));
        assert!(content.contains("<key>KeepAlive</key>\n<false/>"));
        assert!(content.contains("<key>RunAtLoad</key>\n<true/>"));

        assert_eq!(
            adapter.status(&runner, &env).unwrap(),
            AutostartStatus::Enabled
        );

        adapter.disable(&runner, &env).unwrap();
        assert!(!plist.exists(), "disable must remove fallback plist");
    }

    #[test]
    fn macos_requires_approval_does_not_create_duplicate_plist() {
        let temp = tempfile::tempdir().unwrap();
        let env = AutostartEnv {
            home: Some(temp.path().to_path_buf()),
            macos_major_version: Some(14),
            is_macos_app_bundle: true,
            ..AutostartEnv::default()
        };
        let bridge = Arc::new(MockMacBridge::requires_approval());
        let adapter = AutostartFactory::create_for_with_macos_bridge(
            AutostartOs::MacOs,
            Some(bridge),
        );
        let runner = MockRunner::default();
        let status = adapter
            .enable(
                &runner,
                &env,
                Path::new("/Applications/ruum.app/Contents/MacOS/ruum-gui"),
            )
            .unwrap();
        assert_eq!(status, AutostartStatus::RequiresApproval);
        let plist =
            MacOsAutostartAdapter::plist_path_in(&env).expect("plist path");
        assert!(!plist.exists());
        assert_eq!(
            adapter.status(&runner, &env).unwrap(),
            AutostartStatus::RequiresApproval
        );
    }

    #[test]
    fn macos_non_bundle_or_pre_ventura_uses_launch_agent_plist() {
        let temp = tempfile::tempdir().unwrap();
        let dev_env = AutostartEnv {
            home: Some(temp.path().to_path_buf()),
            macos_major_version: Some(15),
            is_macos_app_bundle: false,
            ..AutostartEnv::default()
        };
        let pre_ventura_env = AutostartEnv {
            home: Some(temp.path().to_path_buf()),
            macos_major_version: Some(12),
            is_macos_app_bundle: true,
            ..AutostartEnv::default()
        };
        let bridge = Arc::new(MockMacBridge::ok_enabled());
        let adapter = AutostartFactory::create_for_with_macos_bridge(
            AutostartOs::MacOs,
            Some(bridge.clone()),
        );
        let runner = MockRunner::default();

        for env in [dev_env, pre_ventura_env] {
            let status = adapter
                .enable(&runner, &env, Path::new("/tmp/a&b/ruum-gui"))
                .unwrap();
            assert_eq!(status, AutostartStatus::Enabled);
            assert!(bridge.calls.lock().unwrap().is_empty());

            let plist = MacOsAutostartAdapter::plist_path_in(&env).unwrap();
            let content = fs::read_to_string(&plist).unwrap();
            assert!(content.contains("/tmp/a&amp;b/ruum-gui"));
            assert!(content.contains("--background"));
            assert_eq!(
                adapter.status(&runner, &env).unwrap(),
                AutostartStatus::Enabled
            );

            adapter.disable(&runner, &env).unwrap();
            assert!(!plist.exists());
            assert_eq!(
                adapter.status(&runner, &env).unwrap(),
                AutostartStatus::Disabled
            );
        }
    }

    #[test]
    fn linux_adapter_writes_escaped_xdg_desktop_and_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let xdg = temp.path().join("xdg");
        let env = AutostartEnv {
            home: Some(temp.path().to_path_buf()),
            xdg_config_home: Some(xdg.clone()),
            ..AutostartEnv::default()
        };
        let adapter = AutostartFactory::create_for(AutostartOs::Linux);
        assert_eq!(adapter.os_kind(), AutostartOs::Linux);

        let runner = MockRunner::default();
        assert_eq!(
            adapter.status(&runner, &env).unwrap(),
            AutostartStatus::Disabled
        );

        let status = adapter
            .enable(&runner, &env, Path::new("/opt/My Apps/ruum$100%/ruum-gui"))
            .unwrap();
        assert_eq!(status, AutostartStatus::Enabled);

        let desktop_path = xdg.join("autostart/ruum.desktop");
        assert!(desktop_path.exists());
        let content = fs::read_to_string(&desktop_path).unwrap();
        assert!(content.contains(
            "Exec=\"/opt/My Apps/ruum\\$100%%/ruum-gui\" --background"
        ));
        assert!(content.contains("X-GNOME-Autostart-enabled=true"));
        assert_eq!(
            adapter.status(&runner, &env).unwrap(),
            AutostartStatus::Enabled
        );

        adapter.disable(&runner, &env).unwrap();
        assert!(!desktop_path.exists());
        assert_eq!(
            adapter.status(&runner, &env).unwrap(),
            AutostartStatus::Disabled
        );
    }

    #[test]
    fn windows_adapter_manages_hkcu_run_value_ruum_with_background_flag() {
        let env = AutostartEnv::default();
        let adapter = AutostartFactory::create_for(AutostartOs::Windows);
        assert_eq!(adapter.os_kind(), AutostartOs::Windows);

        let add_runner = MockRunner::with_query_success(true);
        let status = adapter
            .enable(
                &add_runner,
                &env,
                Path::new(r"C:\Program Files\Ruum\ruum-gui.exe"),
            )
            .unwrap();
        assert_eq!(status, AutostartStatus::Enabled);
        let calls = add_runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("reg"));
        assert_eq!(
            calls[0].1,
            vec![
                "add",
                WIN_RUN_KEY,
                "/v",
                "Ruum",
                "/t",
                "REG_SZ",
                "/d",
                r#""C:\Program Files\Ruum\ruum-gui.exe" --background"#,
                "/f",
            ]
        );
        assert_eq!(
            adapter.status(&add_runner, &env).unwrap(),
            AutostartStatus::Enabled
        );

        let absent_runner = MockRunner::with_query_success(false);
        assert_eq!(
            adapter.status(&absent_runner, &env).unwrap(),
            AutostartStatus::Disabled
        );

        adapter.disable(&absent_runner, &env).unwrap();
        let del_calls = absent_runner.calls();
        assert_eq!(
            del_calls.last().unwrap().1,
            vec!["delete", WIN_RUN_KEY, "/v", "Ruum", "/f"]
        );
    }
}
