//! Tri-platform native terminal launcher and file-manager script revealer.
//!
//! Encapsulates OS differences behind [`TerminalOs`], [`TerminalEnv`],
//! [`TerminalAdapter`], and [`TerminalFactory`]. All three adapters compile
//! unconditionally on every host OS so unit tests can exercise macOS, Linux,
//! and Windows command construction deterministically.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::export::shell_quote;

/// Target operating system family for terminal launching and file reveal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment inputs captured for deterministic terminal discovery.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalEnv {
    pub current_exe: PathBuf,
    pub path_var: Option<OsString>,
    pub terminal_var: Option<String>,
    pub local_appdata: Option<PathBuf>,
}

impl TerminalEnv {
    /// Capture the current process environment once.
    #[must_use]
    pub fn current() -> Self {
        Self {
            current_exe: std::env::current_exe().unwrap_or_default(),
            path_var: std::env::var_os("PATH").filter(|v| !v.is_empty()),
            terminal_var: std::env::var("TERMINAL")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            local_appdata: std::env::var_os("LOCALAPPDATA")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        }
    }
}

/// Resolved command and arguments to spawn a visible native terminal or file
/// manager window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLaunchPlan {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub launcher_file: Option<PathBuf>,
}

impl TerminalLaunchPlan {
    /// Lossy UTF-8 view of `args` for diagnostics and unit tests.
    #[must_use]
    pub fn args_lossy(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
}

/// OS-specific terminal launcher and script-file revealer contract.
pub trait TerminalAdapter: Send + Sync + std::fmt::Debug {
    fn os_kind(&self) -> TerminalOs;

    /// Resolve the sibling `ruum` / `ruum.exe` CLI binary from
    /// `env.current_exe` (never returning `ruum-gui`).
    fn resolve_cli_binary(&self, env: &TerminalEnv) -> io::Result<PathBuf> {
        resolve_cli_binary_for(self.os_kind(), env)
    }

    /// Build the command plan to open a visible native terminal executing
    /// `<ruum_exe> run-script <script_path> [--profile <profile_id>] [--pause]`.
    fn build_run_script_plan(
        &self,
        env: &TerminalEnv,
        script_path: &Path,
        profile_id: Option<&str>,
        pause: bool,
    ) -> io::Result<TerminalLaunchPlan>;

    /// Build the command plan to highlight or open the folder containing
    /// `script_path` in the OS file manager.
    fn build_reveal_plan(
        &self,
        env: &TerminalEnv,
        script_path: &Path,
    ) -> io::Result<TerminalLaunchPlan>;

    /// Spawn a visible native terminal running `script_path`.
    fn launch_run_script(
        &self,
        env: &TerminalEnv,
        script_path: &Path,
        profile_id: Option<&str>,
        pause: bool,
    ) -> io::Result<TerminalLaunchPlan> {
        let plan =
            self.build_run_script_plan(env, script_path, profile_id, pause)?;
        spawn_terminal_plan(&plan, self.os_kind())?;
        Ok(plan)
    }

    /// Open the OS file manager focused on `script_path`.
    fn reveal_script(
        &self,
        env: &TerminalEnv,
        script_path: &Path,
    ) -> io::Result<TerminalLaunchPlan> {
        let plan = self.build_reveal_plan(env, script_path)?;
        spawn_reveal_plan(&plan)?;
        Ok(plan)
    }
}

/// macOS terminal launcher (`0o700` `.command` + `open -a Terminal`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsTerminalAdapter;

/// Linux terminal launcher (`xdg-terminal-exec` -> `$TERMINAL` -> emulators).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxTerminalAdapter;

const LINUX_TERMINALS: &[&str] = &[
    "ghostty",
    "kitty",
    "alacritty",
    "wezterm",
    "gnome-terminal",
    "konsole",
    "xfce4-terminal",
    "xterm",
];

/// Windows terminal launcher (`wt.exe` -> `cmd.exe /c start powershell.exe`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsTerminalAdapter;

/// Factory returning the [`TerminalAdapter`] for the host or requested OS.
#[derive(Clone, Copy, Debug, Default)]
pub struct TerminalFactory;

impl TerminalFactory {
    #[must_use]
    pub const fn current_os() -> TerminalOs {
        if cfg!(target_os = "macos") {
            TerminalOs::MacOs
        } else if cfg!(windows) {
            TerminalOs::Windows
        } else {
            TerminalOs::Linux
        }
    }

    #[must_use]
    pub fn create_for(os: TerminalOs) -> Box<dyn TerminalAdapter> {
        match os {
            TerminalOs::MacOs => Box::new(MacOsTerminalAdapter),
            TerminalOs::Linux => Box::new(LinuxTerminalAdapter),
            TerminalOs::Windows => Box::new(WindowsTerminalAdapter),
        }
    }

    #[must_use]
    pub fn create() -> Box<dyn TerminalAdapter> {
        Self::create_for(Self::current_os())
    }
}

impl TerminalAdapter for MacOsTerminalAdapter {
    fn os_kind(&self) -> TerminalOs {
        TerminalOs::MacOs
    }

    fn build_run_script_plan(
        &self,
        env: &TerminalEnv,
        script_path: &Path,
        profile_id: Option<&str>,
        pause: bool,
    ) -> io::Result<TerminalLaunchPlan> {
        let cli_exe = self.resolve_cli_binary(env)?;
        let launcher_path = macos_command_launcher_path(script_path);
        let content = build_macos_command_script(
            &cli_exe,
            script_path,
            profile_id,
            pause,
        );
        write_macos_command_file(&launcher_path, &content)?;
        Ok(TerminalLaunchPlan {
            program: PathBuf::from("open"),
            args: vec![
                OsString::from("-a"),
                OsString::from("Terminal"),
                launcher_path.as_os_str().to_owned(),
            ],
            launcher_file: Some(launcher_path),
        })
    }

    fn build_reveal_plan(
        &self,
        _env: &TerminalEnv,
        script_path: &Path,
    ) -> io::Result<TerminalLaunchPlan> {
        Ok(TerminalLaunchPlan {
            program: PathBuf::from("open"),
            args: vec![
                OsString::from("-R"),
                script_path.as_os_str().to_owned(),
            ],
            launcher_file: None,
        })
    }
}

impl TerminalAdapter for LinuxTerminalAdapter {
    fn os_kind(&self) -> TerminalOs {
        TerminalOs::Linux
    }

    fn build_run_script_plan(
        &self,
        env: &TerminalEnv,
        script_path: &Path,
        profile_id: Option<&str>,
        pause: bool,
    ) -> io::Result<TerminalLaunchPlan> {
        let cli_exe = self.resolve_cli_binary(env)?;
        let cli_args =
            run_script_cli_args(&cli_exe, script_path, profile_id, pause);

        if let Some(exec_bin) =
            find_in_path(env.path_var.as_deref(), "xdg-terminal-exec")
        {
            return Ok(TerminalLaunchPlan {
                program: exec_bin,
                args: cli_args,
                launcher_file: None,
            });
        }

        if let Some(term_raw) = env.terminal_var.as_deref()
            && let Some(term_bin) =
                resolve_terminal_command(env.path_var.as_deref(), term_raw)
        {
            let name = term_bin
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let prefix = linux_terminal_prefix(name);
            let mut args: Vec<OsString> =
                prefix.iter().map(OsString::from).collect();
            args.extend(cli_args);
            return Ok(TerminalLaunchPlan {
                program: term_bin,
                args,
                launcher_file: None,
            });
        }

        for candidate in LINUX_TERMINALS {
            if let Some(bin) = find_in_path(env.path_var.as_deref(), candidate)
            {
                let prefix = linux_terminal_prefix(candidate);
                let mut args: Vec<OsString> =
                    prefix.iter().map(OsString::from).collect();
                args.extend(cli_args);
                return Ok(TerminalLaunchPlan {
                    program: bin,
                    args,
                    launcher_file: None,
                });
            }
        }

        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no supported Linux terminal emulator found in PATH or $TERMINAL",
        ))
    }

    fn build_reveal_plan(
        &self,
        env: &TerminalEnv,
        script_path: &Path,
    ) -> io::Result<TerminalLaunchPlan> {
        let parent = script_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if let Some(xdg) = find_in_path(env.path_var.as_deref(), "xdg-open") {
            return Ok(TerminalLaunchPlan {
                program: xdg,
                args: vec![parent.as_os_str().to_owned()],
                launcher_file: None,
            });
        }
        if let Some(gio) = find_in_path(env.path_var.as_deref(), "gio") {
            return Ok(TerminalLaunchPlan {
                program: gio,
                args: vec![
                    OsString::from("open"),
                    parent.as_os_str().to_owned(),
                ],
                launcher_file: None,
            });
        }
        Ok(TerminalLaunchPlan {
            program: PathBuf::from("xdg-open"),
            args: vec![parent.as_os_str().to_owned()],
            launcher_file: None,
        })
    }
}

impl TerminalAdapter for WindowsTerminalAdapter {
    fn os_kind(&self) -> TerminalOs {
        TerminalOs::Windows
    }

    fn build_run_script_plan(
        &self,
        env: &TerminalEnv,
        script_path: &Path,
        profile_id: Option<&str>,
        pause: bool,
    ) -> io::Result<TerminalLaunchPlan> {
        let cli_exe = self.resolve_cli_binary(env)?;
        if let Some(wt_bin) = find_windows_terminal(env) {
            let mut args =
                vec![OsString::from("new-tab"), OsString::from("--")];
            args.extend(run_script_cli_args(
                &cli_exe,
                script_path,
                profile_id,
                pause,
            ));
            return Ok(TerminalLaunchPlan {
                program: wt_bin,
                args,
                launcher_file: None,
            });
        }

        let ps_cmd = build_powershell_run_command(
            &cli_exe,
            script_path,
            profile_id,
            pause,
        );
        Ok(TerminalLaunchPlan {
            program: PathBuf::from("cmd.exe"),
            args: vec![
                OsString::from("/c"),
                OsString::from("start"),
                OsString::from("Ruum Cleanup"),
                OsString::from("powershell.exe"),
                OsString::from("-NoProfile"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-Command"),
                OsString::from(ps_cmd),
            ],
            launcher_file: None,
        })
    }

    fn build_reveal_plan(
        &self,
        _env: &TerminalEnv,
        script_path: &Path,
    ) -> io::Result<TerminalLaunchPlan> {
        let mut select_arg = OsString::from("/select,");
        select_arg.push(script_path.as_os_str());
        Ok(TerminalLaunchPlan {
            program: PathBuf::from("explorer.exe"),
            args: vec![select_arg],
            launcher_file: None,
        })
    }
}

fn linux_terminal_prefix(name: &str) -> &'static [&'static str] {
    match name {
        "xdg-terminal-exec" => &[],
        "wezterm" => &["start", "--"],
        "gnome-terminal" | "kitty" => &["--"],
        "xfce4-terminal" => &["-x"],
        _ => &["-e"],
    }
}

fn run_script_cli_args(
    cli_exe: &Path,
    script_path: &Path,
    profile_id: Option<&str>,
    pause: bool,
) -> Vec<OsString> {
    let mut args = vec![
        cli_exe.as_os_str().to_owned(),
        OsString::from("run-script"),
        script_path.as_os_str().to_owned(),
    ];
    if let Some(pid) = profile_id.map(str::trim).filter(|s| !s.is_empty()) {
        args.push(OsString::from("--profile"));
        args.push(OsString::from(pid));
    }
    if pause {
        args.push(OsString::from("--pause"));
    }
    args
}

const fn cli_binary_name(os: TerminalOs) -> &'static str {
    match os {
        TerminalOs::MacOs | TerminalOs::Linux => "ruum",
        TerminalOs::Windows => "ruum.exe",
    }
}

fn resolve_cli_binary_for(
    os: TerminalOs,
    env: &TerminalEnv,
) -> io::Result<PathBuf> {
    let target_name = cli_binary_name(os);
    if !env.current_exe.as_os_str().is_empty() {
        let canonical = dunce::canonicalize(&env.current_exe)
            .unwrap_or_else(|_| env.current_exe.clone());
        if let Some(parent) = canonical.parent() {
            let sibling = parent.join(target_name);
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }
    if let Some(found) =
        find_in_path_for(os, env.path_var.as_deref(), target_name)
    {
        return Ok(found);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("sibling CLI executable `{target_name}` not found"),
    ))
}

fn macos_command_launcher_path(script_path: &Path) -> PathBuf {
    let dir = script_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(std::env::temp_dir, Path::to_path_buf);
    let stem = script_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("ruum-cleanup");
    dir.join(format!("{stem}.command"))
}

fn build_macos_command_script(
    cli_exe: &Path,
    script_path: &Path,
    profile_id: Option<&str>,
    pause: bool,
) -> String {
    let mut parts = vec![
        shell_quote(&cli_exe.to_string_lossy()),
        "run-script".to_owned(),
        shell_quote(&script_path.to_string_lossy()),
    ];
    if let Some(pid) = profile_id.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push("--profile".to_owned());
        parts.push(shell_quote(pid));
    }
    if pause {
        parts.push("--pause".to_owned());
    }
    format!("#!/bin/sh\nexec {}\n", parts.join(" "))
}

fn write_macos_command_file(path: &Path, content: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(content.as_bytes())?;
    temp.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

fn ps_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn build_powershell_run_command(
    cli_exe: &Path,
    script_path: &Path,
    profile_id: Option<&str>,
    pause: bool,
) -> String {
    let mut tokens = vec![
        "&".to_owned(),
        ps_single_quote(&cli_exe.to_string_lossy()),
        "'run-script'".to_owned(),
        ps_single_quote(&script_path.to_string_lossy()),
    ];
    if let Some(pid) = profile_id.map(str::trim).filter(|s| !s.is_empty()) {
        tokens.push("'--profile'".to_owned());
        tokens.push(ps_single_quote(pid));
    }
    if pause {
        tokens.push("'--pause'".to_owned());
    }
    tokens.join(" ")
}

fn find_windows_terminal(env: &TerminalEnv) -> Option<PathBuf> {
    if let Some(found) =
        find_in_path_for(TerminalOs::Windows, env.path_var.as_deref(), "wt.exe")
    {
        return Some(found);
    }
    if let Some(local_appdata) = &env.local_appdata {
        let alias = local_appdata
            .join("Microsoft")
            .join("WindowsApps")
            .join("wt.exe");
        if alias.is_file() {
            return Some(alias);
        }
    }
    None
}

fn resolve_terminal_command(
    path_var: Option<&std::ffi::OsStr>,
    raw: &str,
) -> Option<PathBuf> {
    let token = raw.split_whitespace().next()?;
    let p = Path::new(token);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    find_in_path(path_var, token)
}

fn find_in_path(
    path_var: Option<&std::ffi::OsStr>,
    binary_name: &str,
) -> Option<PathBuf> {
    find_in_path_for(TerminalFactory::current_os(), path_var, binary_name)
}

fn find_in_path_for(
    os: TerminalOs,
    path_var: Option<&std::ffi::OsStr>,
    binary_name: &str,
) -> Option<PathBuf> {
    let raw = path_var?;
    let dirs: Vec<PathBuf> = if os == TerminalOs::Windows && !cfg!(windows) {
        raw.to_string_lossy()
            .split(';')
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    } else if os != TerminalOs::Windows && cfg!(windows) {
        raw.to_string_lossy()
            .split(':')
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    } else {
        std::env::split_paths(raw).collect()
    };

    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(binary_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn spawn_terminal_plan(
    plan: &TerminalLaunchPlan,
    _os: TerminalOs,
) -> io::Result<()> {
    let mut cmd = Command::new(&plan.program);
    cmd.args(&plan.args)
        .stdin(Stdio::null())
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
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if plan
            .program
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("cmd.exe"))
        {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
    }
    let _child = cmd.spawn()?;
    Ok(())
}

fn spawn_reveal_plan(plan: &TerminalLaunchPlan) -> io::Result<()> {
    let mut cmd = Command::new(&plan.program);
    cmd.args(&plan.args)
        .stdin(Stdio::null())
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
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _child = cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_file(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"bin").unwrap();
    }

    #[test]
    fn factory_creates_all_three_adapters_unconditionally() {
        assert_eq!(
            TerminalFactory::create_for(TerminalOs::MacOs).os_kind(),
            TerminalOs::MacOs
        );
        assert_eq!(
            TerminalFactory::create_for(TerminalOs::Linux).os_kind(),
            TerminalOs::Linux
        );
        assert_eq!(
            TerminalFactory::create_for(TerminalOs::Windows).os_kind(),
            TerminalOs::Windows
        );
        assert_eq!(
            TerminalFactory::create().os_kind(),
            TerminalFactory::current_os()
        );
    }

    #[test]
    fn resolves_sibling_cli_and_never_falls_back_to_gui() {
        let dir = tempfile::tempdir().unwrap();
        let gui_exe = dir.path().join("ruum-gui");
        touch_file(&gui_exe);

        let adapter = TerminalFactory::create_for(TerminalOs::Linux);
        let env_missing = TerminalEnv {
            current_exe: gui_exe.clone(),
            ..TerminalEnv::default()
        };
        let err = adapter.resolve_cli_binary(&env_missing).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        let cli_exe = dir.path().join("ruum");
        touch_file(&cli_exe);
        let env_present = TerminalEnv {
            current_exe: gui_exe,
            ..TerminalEnv::default()
        };
        assert_eq!(
            adapter.resolve_cli_binary(&env_present).unwrap(),
            dunce::canonicalize(&cli_exe).unwrap()
        );

        #[cfg(unix)]
        {
            let link_dir = tempfile::tempdir().unwrap();
            let linked_gui = link_dir.path().join("ruum-gui-link");
            std::os::unix::fs::symlink(
                dir.path().join("ruum-gui"),
                &linked_gui,
            )
            .unwrap();
            let env_symlink = TerminalEnv {
                current_exe: linked_gui,
                ..TerminalEnv::default()
            };
            assert_eq!(
                adapter.resolve_cli_binary(&env_symlink).unwrap(),
                dunce::canonicalize(&cli_exe).unwrap()
            );
        }
    }

    #[test]
    fn macos_builds_0700_command_launcher_and_open_terminal_plan() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("MacOS");
        let gui_exe = bin_dir.join("ruum-gui");
        let cli_exe = bin_dir.join("ruum");
        touch_file(&gui_exe);
        touch_file(&cli_exe);

        let script = dir.path().join("cleanup-scripts/prod-100.sh");
        touch_file(&script);

        let env = TerminalEnv {
            current_exe: gui_exe,
            ..TerminalEnv::default()
        };
        let adapter = TerminalFactory::create_for(TerminalOs::MacOs);
        let plan = adapter
            .build_run_script_plan(&env, &script, Some("srv-1"), true)
            .unwrap();

        assert_eq!(plan.program, PathBuf::from("open"));
        let launcher = plan.launcher_file.clone().expect("launcher file");
        assert_eq!(
            launcher.extension().and_then(|s| s.to_str()),
            Some("command")
        );
        assert_eq!(launcher.parent(), script.parent());
        assert_eq!(
            plan.args_lossy(),
            vec![
                "-a".to_owned(),
                "Terminal".to_owned(),
                launcher.to_string_lossy().into_owned(),
            ]
        );

        let body = fs::read_to_string(&launcher).unwrap();
        assert!(body.starts_with("#!/bin/sh\nexec "));
        assert!(body.contains("run-script"));
        assert!(body.contains("--profile 'srv-1'"));
        assert!(body.contains("--pause"));
        assert!(!body.contains("ruum-gui"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode =
                fs::metadata(&launcher).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }

        let reveal = adapter.build_reveal_plan(&env, &script).unwrap();
        assert_eq!(reveal.program, PathBuf::from("open"));
        assert_eq!(
            reveal.args_lossy(),
            vec!["-R".to_owned(), script.to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn linux_probe_order_xdg_terminal_env_and_fallback_emulators() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        let gui_exe = bin_dir.join("ruum-gui");
        let cli_exe = bin_dir.join("ruum");
        touch_file(&gui_exe);
        touch_file(&cli_exe);

        let script = dir.path().join("cleanup.sh");
        touch_file(&script);
        let adapter = TerminalFactory::create_for(TerminalOs::Linux);

        // 1. xdg-terminal-exec wins over $TERMINAL and alacritty.
        let path_xdg = dir.path().join("path_xdg");
        let xdg_bin = path_xdg.join("xdg-terminal-exec");
        let alacritty_bin = path_xdg.join("alacritty");
        let custom_term = path_xdg.join("kitty");
        touch_file(&xdg_bin);
        touch_file(&alacritty_bin);
        touch_file(&custom_term);
        let env_xdg = TerminalEnv {
            current_exe: gui_exe.clone(),
            path_var: Some(path_xdg.into_os_string()),
            terminal_var: Some("kitty".to_owned()),
            ..TerminalEnv::default()
        };
        let plan_xdg = adapter
            .build_run_script_plan(&env_xdg, &script, Some("prod"), true)
            .unwrap();
        assert_eq!(plan_xdg.program, xdg_bin);
        assert_eq!(
            plan_xdg.args_lossy(),
            vec![
                dunce::canonicalize(&cli_exe)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                "run-script".to_owned(),
                script.to_string_lossy().into_owned(),
                "--profile".to_owned(),
                "prod".to_owned(),
                "--pause".to_owned(),
            ]
        );

        // 2. Without xdg-terminal-exec, $TERMINAL takes priority.
        let path_term = dir.path().join("path_term");
        let wezterm_bin = path_term.join("wezterm");
        let xterm_bin = path_term.join("xterm");
        touch_file(&wezterm_bin);
        touch_file(&xterm_bin);
        let env_term = TerminalEnv {
            current_exe: gui_exe.clone(),
            path_var: Some(path_term.into_os_string()),
            terminal_var: Some("wezterm".to_owned()),
            ..TerminalEnv::default()
        };
        let plan_term = adapter
            .build_run_script_plan(&env_term, &script, None, true)
            .unwrap();
        assert_eq!(plan_term.program, wezterm_bin);
        assert_eq!(plan_term.args_lossy()[..2], ["start", "--"]);

        // 3. Fallback chain selects first available emulator (`ghostty` before `xterm`).
        let path_fb = dir.path().join("path_fb");
        let xterm_only = path_fb.join("xterm");
        let ghostty_bin = path_fb.join("ghostty");
        touch_file(&xterm_only);
        touch_file(&ghostty_bin);
        let env_fb = TerminalEnv {
            current_exe: gui_exe.clone(),
            path_var: Some(path_fb.into_os_string()),
            ..TerminalEnv::default()
        };
        let plan_fb = adapter
            .build_run_script_plan(&env_fb, &script, None, false)
            .unwrap();
        assert_eq!(plan_fb.program, ghostty_bin);
        assert_eq!(plan_fb.args_lossy()[0], "-e");

        // 4. Error when no terminal emulator exists.
        let empty_env = TerminalEnv {
            current_exe: gui_exe,
            path_var: Some(dir.path().join("empty").into_os_string()),
            ..TerminalEnv::default()
        };
        assert_eq!(
            adapter
                .build_run_script_plan(&empty_env, &script, None, true)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn windows_prefers_wt_exe_and_falls_back_to_cmd_start_powershell() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("win");
        let gui_exe = bin_dir.join("ruum-gui.exe");
        let cli_exe = bin_dir.join("ruum.exe");
        touch_file(&gui_exe);
        touch_file(&cli_exe);

        let script = dir.path().join("cleanup.sh");
        touch_file(&script);
        let adapter = TerminalFactory::create_for(TerminalOs::Windows);

        // 1. wt.exe present in PATH.
        let wt_dir = dir.path().join("wt");
        let wt_exe = wt_dir.join("wt.exe");
        touch_file(&wt_exe);
        let env_wt = TerminalEnv {
            current_exe: gui_exe.clone(),
            path_var: Some(wt_dir.into_os_string()),
            ..TerminalEnv::default()
        };
        let plan_wt = adapter
            .build_run_script_plan(&env_wt, &script, Some("srv'1"), true)
            .unwrap();
        assert_eq!(plan_wt.program, wt_exe);
        assert_eq!(plan_wt.args_lossy()[..2], ["new-tab", "--"]);
        assert!(plan_wt.args_lossy()[2].ends_with("ruum.exe"));

        // 2. Fallback to cmd.exe /c start "Ruum Cleanup" powershell.exe ...
        let env_fb = TerminalEnv {
            current_exe: gui_exe,
            path_var: Some(dir.path().join("missing").into_os_string()),
            ..TerminalEnv::default()
        };
        let plan_fb = adapter
            .build_run_script_plan(&env_fb, &script, Some("srv'1"), true)
            .unwrap();
        assert_eq!(plan_fb.program, PathBuf::from("cmd.exe"));
        let args = plan_fb.args_lossy();
        assert_eq!(
            &args[..8],
            [
                "/c",
                "start",
                "Ruum Cleanup",
                "powershell.exe",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
            ]
        );
        assert!(args[8].contains("'run-script'"));
        assert!(args[8].contains("'srv''1'"));
        assert!(args[8].contains("'--pause'"));

        // 3. Reveal plan uses explorer.exe /select,<path>.
        let reveal = adapter.build_reveal_plan(&env_fb, &script).unwrap();
        assert_eq!(reveal.program, PathBuf::from("explorer.exe"));
        assert_eq!(
            reveal.args_lossy(),
            vec![format!("/select,{}", script.display())]
        );
    }
}
