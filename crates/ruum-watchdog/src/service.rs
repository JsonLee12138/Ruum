//! Tri-platform watchdog background service registration and lifecycle.
//!
//! Encapsulates OS-specific service management behind [`ServiceAdapter`] and
//! [`ServiceAdapterFactory`]:
//! - macOS: `launchd` user agent plist + `launchctl`
//! - Linux: `systemd --user` unit + `systemctl`
//! - Windows: zero-UAC `HKCU\...\Run` autostart + detached hidden daemon +
//!   `watchdog.pid` tracked via `tasklist` / `taskkill`
//!
//! All three platforms register `<exe> watchdog run` as the background command.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MACOS_LABEL: &str = "com.ruum.watchdog";
const LINUX_UNIT_NAME: &str = "ruum-watchdog.service";
const WIN_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const WIN_RUN_VALUE: &str = "RuumWatchdog";

/// Operating system family for watchdog background service registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment context needed to resolve service definition and PID paths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceEnv {
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
}

impl ServiceEnv {
    /// Capture the current process environment once for service operations.
    #[must_use]
    pub fn current() -> Self {
        let home = std::env::home_dir();
        let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty() && p.is_absolute());
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty());
        Self {
            home,
            xdg_config_home,
            appdata,
        }
    }
}

/// Operational status of the OS background service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceStatus {
    Active,
    Inactive,
    NotInstalled,
    Unsupported,
}

/// Output captured from running an OS service management command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceCommandOutput {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ServiceCommandOutput {
    /// Return `true` when the command exited with status `0`.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.status_code == 0
    }
}

/// Abstract runner for executing OS service management subprocesses.
pub trait ServiceRunner: Send + Sync {
    fn run(
        &self,
        program: &Path,
        args: &[&str],
    ) -> io::Result<ServiceCommandOutput>;

    /// Execute `program` with additional environment variables.
    fn run_with_env(
        &self,
        program: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> io::Result<ServiceCommandOutput> {
        let _ = envs;
        self.run(program, args)
    }
}

/// Adapter interface encapsulating OS-specific background service lifecycle.
pub trait ServiceAdapter: Send + Sync + fmt::Debug {
    fn os_kind(&self) -> ServiceOs;
    fn install(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
        exe_path: &Path,
    ) -> io::Result<()>;
    fn uninstall(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
    ) -> io::Result<()>;
    fn status(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
    ) -> io::Result<ServiceStatus>;
}

/// macOS `launchd` user agent adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsServiceAdapter;

impl MacOsServiceAdapter {
    fn plist_path_in(env: &ServiceEnv) -> Option<PathBuf> {
        env.home
            .as_ref()
            .map(|h| h.join("Library/LaunchAgents/com.ruum.watchdog.plist"))
    }
}

impl ServiceAdapter for MacOsServiceAdapter {
    fn os_kind(&self) -> ServiceOs {
        ServiceOs::MacOs
    }

    fn install(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
        exe_path: &Path,
    ) -> io::Result<()> {
        let plist_path = Self::plist_path_in(env).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no LaunchAgents dir")
        })?;
        if let Some(parent) = plist_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&plist_path, macos_launchd_plist_content(exe_path))?;
        let path_str = plist_path.to_string_lossy();
        let _ =
            runner.run(Path::new("launchctl"), &["unload", "-w", &path_str]);
        let out =
            runner.run(Path::new("launchctl"), &["load", "-w", &path_str])?;
        if out.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "launchctl load failed: {}",
                out.stderr
            )))
        }
    }

    fn uninstall(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
    ) -> io::Result<()> {
        let plist_path = Self::plist_path_in(env).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no LaunchAgents dir")
        })?;
        let path_str = plist_path.to_string_lossy();
        let _ =
            runner.run(Path::new("launchctl"), &["unload", "-w", &path_str]);
        if plist_path.exists() {
            fs::remove_file(&plist_path)?;
        }
        Ok(())
    }

    fn status(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
    ) -> io::Result<ServiceStatus> {
        let Some(plist_path) = Self::plist_path_in(env) else {
            return Ok(ServiceStatus::NotInstalled);
        };
        let out = runner.run(Path::new("launchctl"), &["list", MACOS_LABEL])?;
        if out.success() {
            Ok(ServiceStatus::Active)
        } else if plist_path.exists() {
            Ok(ServiceStatus::Inactive)
        } else {
            Ok(ServiceStatus::NotInstalled)
        }
    }
}

/// Linux `systemd --user` service adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxServiceAdapter;

impl LinuxServiceAdapter {
    fn unit_path_in(env: &ServiceEnv) -> Option<PathBuf> {
        if let Some(ref xdg) = env.xdg_config_home
            && xdg.is_absolute()
        {
            return Some(xdg.join("systemd/user").join(LINUX_UNIT_NAME));
        }
        env.home
            .as_ref()
            .map(|h| h.join(".config/systemd/user").join(LINUX_UNIT_NAME))
    }
}

impl ServiceAdapter for LinuxServiceAdapter {
    fn os_kind(&self) -> ServiceOs {
        ServiceOs::Linux
    }

    fn install(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
        exe_path: &Path,
    ) -> io::Result<()> {
        let unit_path = Self::unit_path_in(env).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no systemd user dir")
        })?;
        if let Some(parent) = unit_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&unit_path, linux_systemd_unit_content(exe_path))?;
        let _ =
            runner.run(Path::new("systemctl"), &["--user", "daemon-reload"]);
        let enable_out = runner.run(
            Path::new("systemctl"),
            &["--user", "enable", LINUX_UNIT_NAME],
        )?;
        if !enable_out.success() {
            return Err(io::Error::other(format!(
                "systemctl enable failed: {}",
                enable_out.stderr
            )));
        }
        let restart_out = runner.run(
            Path::new("systemctl"),
            &["--user", "restart", LINUX_UNIT_NAME],
        )?;
        if restart_out.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "systemctl restart failed: {}",
                restart_out.stderr
            )))
        }
    }

    fn uninstall(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
    ) -> io::Result<()> {
        let unit_path = Self::unit_path_in(env).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no systemd user dir")
        })?;
        let _ = runner
            .run(Path::new("systemctl"), &["--user", "stop", LINUX_UNIT_NAME]);
        let _ = runner.run(
            Path::new("systemctl"),
            &["--user", "disable", LINUX_UNIT_NAME],
        );
        if unit_path.exists() {
            fs::remove_file(&unit_path)?;
        }
        let _ =
            runner.run(Path::new("systemctl"), &["--user", "daemon-reload"]);
        Ok(())
    }

    fn status(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
    ) -> io::Result<ServiceStatus> {
        let Some(unit_path) = Self::unit_path_in(env) else {
            return Ok(ServiceStatus::NotInstalled);
        };
        let out = runner.run(
            Path::new("systemctl"),
            &["--user", "is-active", LINUX_UNIT_NAME],
        )?;
        if out.success() && out.stdout.trim() == "active" {
            Ok(ServiceStatus::Active)
        } else if unit_path.exists() {
            Ok(ServiceStatus::Inactive)
        } else {
            Ok(ServiceStatus::NotInstalled)
        }
    }
}

/// Windows zero-UAC `HKCU\...\Run` + detached hidden daemon adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsServiceAdapter;

impl WindowsServiceAdapter {
    fn pid_path_in(env: &ServiceEnv) -> Option<PathBuf> {
        env.appdata
            .as_deref()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| {
                env.home.as_ref().map(|h| h.join("AppData").join("Roaming"))
            })
            .map(|base| base.join("ruum").join("watchdog.pid"))
    }

    fn live_pid(runner: &dyn ServiceRunner, pid_path: &Path) -> Option<u32> {
        let content = fs::read_to_string(pid_path).ok()?;
        let pid = content.trim().parse::<u32>().ok()?;
        if pid == 0 {
            return None;
        }
        let filter = format!("PID eq {pid}");
        let out = runner
            .run(
                Path::new("tasklist"),
                &["/FI", &filter, "/FO", "CSV", "/NH"],
            )
            .ok()?;
        if out.success() && out.stdout.to_ascii_lowercase().contains("ruum") {
            Some(pid)
        } else {
            None
        }
    }
}

impl ServiceAdapter for WindowsServiceAdapter {
    fn os_kind(&self) -> ServiceOs {
        ServiceOs::Windows
    }

    fn install(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
        exe_path: &Path,
    ) -> io::Result<()> {
        let pid_path = Self::pid_path_in(env).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no appdata dir for watchdog pid",
            )
        })?;
        if let Some(parent) = pid_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let reg_data = format!("\"{}\" watchdog run", exe_path.display());
        let reg_out = runner.run(
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
        )?;
        if !reg_out.success() {
            return Err(io::Error::other(format!(
                "reg add failed: {}",
                reg_out.stderr
            )));
        }

        if let Some(old_pid) = Self::live_pid(runner, &pid_path) {
            let pid_str = old_pid.to_string();
            let _ =
                runner.run(Path::new("taskkill"), &["/PID", &pid_str, "/F"]);
            let _ = fs::remove_file(&pid_path);
        }

        let exe_str = exe_path.to_string_lossy();
        let ps_cmd = "$p = Start-Process -WindowStyle Hidden -PassThru \
                      -FilePath $env:RUUM_EXE \
                      -ArgumentList 'watchdog','run'; \
                      Write-Output $p.Id";
        let ps_out = runner.run_with_env(
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
        )?;
        if !ps_out.success() {
            return Err(io::Error::other(format!(
                "powershell Start-Process failed: {}",
                ps_out.stderr
            )));
        }

        let pid = ps_out
            .stdout
            .lines()
            .rev()
            .find_map(|line| line.trim().parse::<u32>().ok())
            .filter(|&id| id > 0)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "failed to parse watchdog PID from output: {:?}",
                    ps_out.stdout
                ))
            })?;
        fs::write(&pid_path, format!("{pid}\n"))?;
        Ok(())
    }

    fn uninstall(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
    ) -> io::Result<()> {
        // Ignore missing Run key/value errors so uninstall is idempotent.
        let _ = runner.run(
            Path::new("reg"),
            &["delete", WIN_RUN_KEY, "/v", WIN_RUN_VALUE, "/f"],
        );

        let pid_path = Self::pid_path_in(env);
        if let Some(ref path) = pid_path
            && let Some(pid) = Self::live_pid(runner, path)
        {
            let pid_str = pid.to_string();
            let _ =
                runner.run(Path::new("taskkill"), &["/PID", &pid_str, "/F"]);
            let _ = fs::remove_file(path);
            return Ok(());
        }

        if let Some(ref path) = pid_path {
            let _ = fs::remove_file(path);
        }
        // Fallback when PID file is missing or stale: stop only `ruum.exe`
        // processes launched with the `watchdog` subcommand.
        let ps_cmd = "Get-CimInstance Win32_Process \
                      -Filter \"Name = 'ruum.exe'\" \
                      | Where-Object { \
                          $_.CommandLine -match '(^|\\s)watchdog(\\s|$)' \
                      } \
                      | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }";
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
        );
        Ok(())
    }

    fn status(
        &self,
        runner: &dyn ServiceRunner,
        env: &ServiceEnv,
    ) -> io::Result<ServiceStatus> {
        if let Some(pid_path) = Self::pid_path_in(env)
            && Self::live_pid(runner, &pid_path).is_some()
        {
            return Ok(ServiceStatus::Active);
        }

        let reg_out = runner.run(
            Path::new("reg"),
            &["query", WIN_RUN_KEY, "/v", WIN_RUN_VALUE],
        )?;
        if reg_out.success() {
            Ok(ServiceStatus::Inactive)
        } else {
            Ok(ServiceStatus::NotInstalled)
        }
    }
}

/// Factory selecting the watchdog [`ServiceAdapter`] for the host or a
/// requested target OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServiceAdapterFactory;

impl ServiceAdapterFactory {
    /// Detect the current operating system family.
    #[must_use]
    pub const fn current_os() -> ServiceOs {
        if cfg!(target_os = "macos") {
            ServiceOs::MacOs
        } else if cfg!(windows) {
            ServiceOs::Windows
        } else {
            ServiceOs::Linux
        }
    }

    /// Instantiate the adapter for a specific target OS (supports cross-OS
    /// unit tests on any host).
    #[must_use]
    pub fn create_for(os: ServiceOs) -> Box<dyn ServiceAdapter> {
        match os {
            ServiceOs::MacOs => Box::new(MacOsServiceAdapter),
            ServiceOs::Linux => Box::new(LinuxServiceAdapter),
            ServiceOs::Windows => Box::new(WindowsServiceAdapter),
        }
    }

    /// Instantiate the adapter for the current host OS.
    #[must_use]
    pub fn create() -> Box<dyn ServiceAdapter> {
        Self::create_for(Self::current_os())
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

fn systemd_quote_exec(exe_path: &Path) -> String {
    let raw = exe_path.to_string_lossy();
    let needs_quote = raw.is_empty()
        || raw.chars().any(|c| {
            c.is_whitespace()
                || matches!(c, '"' | '\\' | '\'' | '$' | '%' | ';' | '&' | '|')
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
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Path to the macOS `launchd` plist for the user watchdog service.
#[must_use]
pub fn macos_launchd_plist_path() -> Option<PathBuf> {
    MacOsServiceAdapter::plist_path_in(&ServiceEnv::current())
}

/// Content template for macOS `launchd` plist.
#[must_use]
pub fn macos_launchd_plist_content(exe_path: &Path) -> String {
    let escaped = xml_escape(&exe_path.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
        <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
        \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
        <plist version=\"1.0\">\n\
        <dict>\n\
            <key>Label</key>\n\
            <string>{MACOS_LABEL}</string>\n\
            <key>ProgramArguments</key>\n\
            <array>\n\
                <string>{escaped}</string>\n\
                <string>watchdog</string>\n\
                <string>run</string>\n\
            </array>\n\
            <key>KeepAlive</key>\n\
            <true/>\n\
            <key>RunAtLoad</key>\n\
            <true/>\n\
        </dict>\n\
        </plist>\n",
    )
}

/// Path to the Linux `systemd --user` service unit.
#[must_use]
pub fn linux_systemd_service_path() -> Option<PathBuf> {
    LinuxServiceAdapter::unit_path_in(&ServiceEnv::current())
}

/// Content template for Linux `systemd --user` unit.
#[must_use]
pub fn linux_systemd_unit_content(exe_path: &Path) -> String {
    let quoted = systemd_quote_exec(exe_path);
    format!(
        "[Unit]\n\
        Description=Ruum Disk Space Watchdog\n\
        After=default.target\n\n\
        [Service]\n\
        Type=simple\n\
        ExecStart={quoted} watchdog run\n\
        Restart=on-failure\n\
        RestartSec=30\n\n\
        [Install]\n\
        WantedBy=default.target\n",
    )
}

/// Install the watchdog as an OS background service for the current OS.
pub fn install_service(
    runner: &dyn ServiceRunner,
    exe_path: &Path,
) -> io::Result<()> {
    ServiceAdapterFactory::create().install(
        runner,
        &ServiceEnv::current(),
        exe_path,
    )
}

/// Uninstall the watchdog OS background service for the current OS.
pub fn uninstall_service(runner: &dyn ServiceRunner) -> io::Result<()> {
    ServiceAdapterFactory::create().uninstall(runner, &ServiceEnv::current())
}

/// Query the current status of the OS background service for the current OS.
pub fn service_status(runner: &dyn ServiceRunner) -> io::Result<ServiceStatus> {
    ServiceAdapterFactory::create().status(runner, &ServiceEnv::current())
}

/// Fast check whether the legacy headless `ruum watchdog` OS service has any
/// on-disk or registry residue (`plist` / `systemd` unit / Windows `watchdog.pid`
/// or `HKCU\...\Run` `RuumWatchdog`).
#[must_use]
pub fn has_service_residue_in(
    os: ServiceOs,
    env: &ServiceEnv,
    runner: &dyn ServiceRunner,
) -> bool {
    match os {
        ServiceOs::MacOs => {
            MacOsServiceAdapter::plist_path_in(env).is_some_and(|p| p.exists())
        }
        ServiceOs::Linux => {
            LinuxServiceAdapter::unit_path_in(env).is_some_and(|p| p.exists())
        }
        ServiceOs::Windows => {
            if WindowsServiceAdapter::pid_path_in(env)
                .is_some_and(|p| p.exists())
            {
                return true;
            }
            runner
                .run(
                    Path::new("reg"),
                    &["query", WIN_RUN_KEY, "/v", WIN_RUN_VALUE],
                )
                .is_ok_and(|out| out.success())
        }
    }
}

/// Fast check for legacy headless `ruum watchdog` OS service residue on the
/// current OS.
#[must_use]
pub fn has_service_residue(runner: &dyn ServiceRunner) -> bool {
    has_service_residue_in(
        ServiceAdapterFactory::current_os(),
        &ServiceEnv::current(),
        runner,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    type RecordedCall = (PathBuf, Vec<String>, Vec<(String, String)>);

    #[derive(Default)]
    struct ScriptedRunner {
        calls: Mutex<Vec<RecordedCall>>,
        responses: Mutex<VecDeque<ServiceCommandOutput>>,
        default_output: Mutex<Option<ServiceCommandOutput>>,
    }

    impl ScriptedRunner {
        fn with_default(out: ServiceCommandOutput) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(VecDeque::new()),
                default_output: Mutex::new(Some(out)),
            }
        }

        fn with_sequence(seq: Vec<ServiceCommandOutput>) -> Self {
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

    impl ServiceRunner for ScriptedRunner {
        fn run(
            &self,
            program: &Path,
            args: &[&str],
        ) -> io::Result<ServiceCommandOutput> {
            self.run_with_env(program, args, &[])
        }

        fn run_with_env(
            &self,
            program: &Path,
            args: &[&str],
            envs: &[(&str, &str)],
        ) -> io::Result<ServiceCommandOutput> {
            self.calls.lock().unwrap().push((
                program.to_path_buf(),
                args.iter().map(|s| (*s).to_string()).collect(),
                envs.iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ));
            let next = self.responses.lock().unwrap().pop_front();
            if let Some(out) = next {
                return Ok(out);
            }
            let def = self.default_output.lock().unwrap().clone();
            if let Some(out) = def {
                return Ok(out);
            }
            Ok(ServiceCommandOutput {
                status_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn ok_out(stdout: &str) -> ServiceCommandOutput {
        ServiceCommandOutput {
            status_code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn err_out(code: i32, stderr: &str) -> ServiceCommandOutput {
        ServiceCommandOutput {
            status_code: code,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn templates_escape_xml_and_quote_systemd_exec_with_watchdog_run() {
        let special = Path::new("/opt/R&D <app>/'v1'/\"bin\"/ruum");
        let plist = macos_launchd_plist_content(special);
        assert!(plist.contains(
            "<string>/opt/R&amp;D &lt;app&gt;/&apos;v1&apos;/&quot;bin&quot;/ruum</string>"
        ));
        assert!(plist.contains("<string>watchdog</string>"));
        assert!(plist.contains("<string>run</string>"));

        let spaced = Path::new("/home/my user/100%/ruum");
        let unit = linux_systemd_unit_content(spaced);
        assert!(
            unit.contains(
                "ExecStart=\"/home/my user/100%%/ruum\" watchdog run"
            )
        );
    }

    #[test]
    fn macos_service_adapter_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let env = ServiceEnv {
            home: Some(temp.path().to_path_buf()),
            xdg_config_home: None,
            appdata: None,
        };
        let adapter = ServiceAdapterFactory::create_for(ServiceOs::MacOs);
        assert_eq!(adapter.os_kind(), ServiceOs::MacOs);

        let runner_missing = ScriptedRunner::with_default(err_out(1, "nope"));
        assert_eq!(
            adapter.status(&runner_missing, &env).unwrap(),
            ServiceStatus::NotInstalled
        );

        let runner_install = ScriptedRunner::with_default(ok_out(""));
        adapter
            .install(&runner_install, &env, Path::new("/Applications/ruum"))
            .unwrap();
        let plist = temp
            .path()
            .join("Library/LaunchAgents/com.ruum.watchdog.plist");
        assert!(plist.is_file());
        let plist_text = fs::read_to_string(&plist).unwrap();
        assert!(plist_text.contains("<string>watchdog</string>"));
        assert!(plist_text.contains("<string>run</string>"));

        let calls = runner_install.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, PathBuf::from("launchctl"));
        assert_eq!(calls[0].1[0], "unload");
        assert_eq!(calls[1].1[0], "load");

        let runner_active = ScriptedRunner::with_default(ok_out(""));
        assert_eq!(
            adapter.status(&runner_active, &env).unwrap(),
            ServiceStatus::Active
        );
        let runner_inactive = ScriptedRunner::with_default(err_out(1, ""));
        assert_eq!(
            adapter.status(&runner_inactive, &env).unwrap(),
            ServiceStatus::Inactive
        );

        let runner_un = ScriptedRunner::with_default(ok_out(""));
        adapter.uninstall(&runner_un, &env).unwrap();
        assert!(!plist.exists());
    }

    #[test]
    fn linux_service_adapter_lifecycle_uses_restart() {
        let temp = tempfile::tempdir().unwrap();
        let xdg = temp.path().join("xdg");
        let env = ServiceEnv {
            home: Some(temp.path().to_path_buf()),
            xdg_config_home: Some(xdg.clone()),
            appdata: None,
        };
        let adapter = ServiceAdapterFactory::create_for(ServiceOs::Linux);
        assert_eq!(adapter.os_kind(), ServiceOs::Linux);

        let runner_missing = ScriptedRunner::with_default(err_out(3, ""));
        assert_eq!(
            adapter.status(&runner_missing, &env).unwrap(),
            ServiceStatus::NotInstalled
        );

        let runner_install = ScriptedRunner::with_default(ok_out(""));
        adapter
            .install(&runner_install, &env, Path::new("/usr/local/bin/ruum"))
            .unwrap();
        let unit = xdg.join("systemd/user/ruum-watchdog.service");
        assert!(unit.is_file());
        let unit_text = fs::read_to_string(&unit).unwrap();
        assert!(
            unit_text.contains("ExecStart=/usr/local/bin/ruum watchdog run")
        );

        let calls = runner_install.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].1, vec!["--user", "daemon-reload"]);
        assert_eq!(
            calls[1].1,
            vec!["--user", "enable", "ruum-watchdog.service"]
        );
        assert_eq!(
            calls[2].1,
            vec!["--user", "restart", "ruum-watchdog.service"]
        );

        let runner_active = ScriptedRunner::with_default(ok_out("active\n"));
        assert_eq!(
            adapter.status(&runner_active, &env).unwrap(),
            ServiceStatus::Active
        );
        let runner_inactive =
            ScriptedRunner::with_default(ok_out("inactive\n"));
        assert_eq!(
            adapter.status(&runner_inactive, &env).unwrap(),
            ServiceStatus::Inactive
        );

        let runner_un = ScriptedRunner::with_default(ok_out(""));
        adapter.uninstall(&runner_un, &env).unwrap();
        assert!(!unit.exists());
        assert_eq!(
            runner_un.calls().last().unwrap().1,
            vec!["--user", "daemon-reload"]
        );
    }

    #[test]
    fn windows_service_adapter_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let appdata = temp.path().join("Roaming");
        let env = ServiceEnv {
            home: Some(temp.path().to_path_buf()),
            xdg_config_home: None,
            appdata: Some(appdata.clone()),
        };
        let adapter = ServiceAdapterFactory::create_for(ServiceOs::Windows);
        assert_eq!(adapter.os_kind(), ServiceOs::Windows);

        let pid_path = appdata.join("ruum/watchdog.pid");

        let runner_not_inst =
            ScriptedRunner::with_default(err_out(1, "not found"));
        assert_eq!(
            adapter.status(&runner_not_inst, &env).unwrap(),
            ServiceStatus::NotInstalled
        );

        let runner_inst = ScriptedRunner::with_sequence(vec![
            ok_out("The operation completed successfully."),
            ok_out("4242\n"),
        ]);
        let exe = Path::new(r"C:\Program Files\Ruum\ruum.exe");
        adapter.install(&runner_inst, &env, exe).unwrap();
        assert_eq!(fs::read_to_string(&pid_path).unwrap().trim(), "4242");

        let inst_calls = runner_inst.calls();
        assert_eq!(inst_calls[0].0, PathBuf::from("reg"));
        assert_eq!(inst_calls[0].1[0], "add");
        let expected_reg = format!("\"{}\" watchdog run", exe.display());
        assert!(inst_calls[0].1.iter().any(|arg| arg == &expected_reg));
        assert_eq!(inst_calls[1].0, PathBuf::from("powershell.exe"));
        assert!(
            inst_calls[1]
                .1
                .iter()
                .any(|arg| arg.contains("'watchdog','run'"))
        );
        assert!(
            inst_calls[1]
                .2
                .iter()
                .any(|(k, v)| k == "RUUM_EXE" && v == &exe.to_string_lossy())
        );

        let runner_active = ScriptedRunner::with_default(ok_out(
            "\"ruum.exe\",\"4242\",\"Console\",\"1\",\"12,000 K\"\n",
        ));
        assert_eq!(
            adapter.status(&runner_active, &env).unwrap(),
            ServiceStatus::Active
        );

        let runner_stale = ScriptedRunner::with_sequence(vec![
            ok_out("\"svchost.exe\",\"4242\",\"Services\",\"0\",\"8,000 K\"\n"),
            ok_out("RuumWatchdog    REG_SZ    \"C:\\ruum.exe\" watchdog run\n"),
        ]);
        assert_eq!(
            adapter.status(&runner_stale, &env).unwrap(),
            ServiceStatus::Inactive
        );

        let runner_reinst = ScriptedRunner::with_sequence(vec![
            ok_out("reg ok"),
            ok_out("\"Ruum.exe\",\"4242\",\"Console\",\"1\",\"12,000 K\"\n"),
            ok_out("SUCCESS: terminated PID 4242"),
            ok_out("9898\n"),
        ]);
        adapter.install(&runner_reinst, &env, exe).unwrap();
        assert_eq!(fs::read_to_string(&pid_path).unwrap().trim(), "9898");
        assert_eq!(runner_reinst.calls()[2].0, PathBuf::from("taskkill"));

        let runner_un_pid = ScriptedRunner::with_sequence(vec![
            err_out(1, "missing reg entry ignored"),
            ok_out("\"ruum.exe\",\"9898\",\"Console\",\"1\",\"12,000 K\"\n"),
            ok_out("killed"),
        ]);
        adapter.uninstall(&runner_un_pid, &env).unwrap();
        assert!(!pid_path.exists());
        assert_eq!(runner_un_pid.calls()[2].0, PathBuf::from("taskkill"));

        let runner_un_fallback = ScriptedRunner::with_default(ok_out(""));
        adapter.uninstall(&runner_un_fallback, &env).unwrap();
        let fb_calls = runner_un_fallback.calls();
        assert_eq!(fb_calls[1].0, PathBuf::from("powershell.exe"));
        assert!(fb_calls[1].1.iter().any(|a| a.contains("Win32_Process")));
    }

    #[test]
    fn residue_check_detects_macos_linux_and_windows_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let env = ServiceEnv {
            home: Some(temp.path().to_path_buf()),
            xdg_config_home: Some(temp.path().join("xdg")),
            appdata: Some(temp.path().join("AppData")),
        };
        let runner_none = ScriptedRunner::with_default(err_out(1, "absent"));

        assert!(!has_service_residue_in(
            ServiceOs::MacOs,
            &env,
            &runner_none
        ));
        assert!(runner_none.calls().is_empty());
        let plist = MacOsServiceAdapter::plist_path_in(&env).unwrap();
        fs::create_dir_all(plist.parent().unwrap()).unwrap();
        fs::write(&plist, "plist").unwrap();
        assert!(has_service_residue_in(ServiceOs::MacOs, &env, &runner_none));

        assert!(!has_service_residue_in(
            ServiceOs::Linux,
            &env,
            &runner_none
        ));
        let unit = LinuxServiceAdapter::unit_path_in(&env).unwrap();
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        fs::write(&unit, "unit").unwrap();
        assert!(has_service_residue_in(ServiceOs::Linux, &env, &runner_none));

        assert!(!has_service_residue_in(
            ServiceOs::Windows,
            &env,
            &runner_none
        ));
        let runner_reg = ScriptedRunner::with_default(ok_out("RuumWatchdog"));
        assert!(has_service_residue_in(
            ServiceOs::Windows,
            &env,
            &runner_reg
        ));

        let pid_path = WindowsServiceAdapter::pid_path_in(&env).unwrap();
        fs::create_dir_all(pid_path.parent().unwrap()).unwrap();
        fs::write(&pid_path, "1234\n").unwrap();
        assert!(has_service_residue_in(
            ServiceOs::Windows,
            &env,
            &runner_none
        ));
    }
}
