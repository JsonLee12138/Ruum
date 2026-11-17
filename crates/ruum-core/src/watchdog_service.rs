//! Bridge between `ruum-core`'s [`CommandRunner`] and `ruum-watchdog`'s
//! [`ServiceRunner`] plus backward-compatible re-exports.

use std::fmt;
use std::io;
use std::path::Path;

use crate::ai_cli::CommandRunner;

pub use ruum_watchdog::service::{
    LinuxServiceAdapter, MacOsServiceAdapter, ServiceAdapter,
    ServiceAdapterFactory, ServiceCommandOutput, ServiceEnv, ServiceOs,
    ServiceRunner, ServiceStatus, WindowsServiceAdapter,
    linux_systemd_service_path, linux_systemd_unit_content,
    macos_launchd_plist_content, macos_launchd_plist_path,
};

/// Adapter bridging `ruum-core`'s [`CommandRunner`] to `ruum-watchdog`'s
/// [`ServiceRunner`].
pub struct CommandServiceRunner<'a> {
    runner: &'a dyn CommandRunner,
}

impl<'a> CommandServiceRunner<'a> {
    /// Wrap a [`CommandRunner`] reference for use with `ruum-watchdog` service
    /// adapters.
    #[must_use]
    pub const fn new(runner: &'a dyn CommandRunner) -> Self {
        Self { runner }
    }
}

impl fmt::Debug for CommandServiceRunner<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandServiceRunner")
            .finish_non_exhaustive()
    }
}

impl ServiceRunner for CommandServiceRunner<'_> {
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
        let out = self.runner.run(program, args, envs)?;
        Ok(ServiceCommandOutput {
            status_code: out.status_code,
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}

/// Install the watchdog as an OS background service for the current OS.
pub fn install_service(
    runner: &dyn CommandRunner,
    exe_path: &Path,
) -> io::Result<()> {
    let cli_exe = crate::cli_install::resolve_cli_binary(exe_path)?;
    let bridge = CommandServiceRunner::new(runner);
    ruum_watchdog::service::install_service(&bridge, &cli_exe)
}

/// Uninstall the watchdog OS background service for the current OS.
pub fn uninstall_service(runner: &dyn CommandRunner) -> io::Result<()> {
    let bridge = CommandServiceRunner::new(runner);
    ruum_watchdog::service::uninstall_service(&bridge)
}

/// Query the current status of the OS background service for the current OS.
pub fn service_status(runner: &dyn CommandRunner) -> io::Result<ServiceStatus> {
    let bridge = CommandServiceRunner::new(runner);
    ruum_watchdog::service::service_status(&bridge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_cli::CommandOutput;
    use std::path::PathBuf;
    use std::sync::Mutex;

    type RecordedCall = (PathBuf, Vec<String>, Vec<(String, String)>);

    #[derive(Default)]
    struct RecordingCommandRunner {
        calls: Mutex<Vec<RecordedCall>>,
    }

    impl CommandRunner for RecordingCommandRunner {
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
            Ok(CommandOutput {
                status_code: 0,
                stdout: "4242\n".to_string(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn command_service_runner_bridges_args_and_env() {
        let cmd_runner = RecordingCommandRunner::default();
        let bridge = CommandServiceRunner::new(&cmd_runner);

        let out = bridge
            .run_with_env(
                Path::new("powershell.exe"),
                &["-Command", "Write-Output $env:RUUM_EXE"],
                &[("RUUM_EXE", r"C:\Ruum\ruum.exe")],
            )
            .unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "4242\n");

        let calls = cmd_runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("powershell.exe"));
        assert_eq!(
            calls[0].2,
            vec![("RUUM_EXE".to_string(), r"C:\Ruum\ruum.exe".to_string())]
        );
    }

    #[test]
    fn windows_adapter_via_bridge_preserves_ruum_exe_env() {
        let temp = tempfile::tempdir().unwrap();
        let env = ServiceEnv {
            home: Some(temp.path().to_path_buf()),
            xdg_config_home: None,
            appdata: Some(temp.path().join("AppData")),
        };
        let cmd_runner = RecordingCommandRunner::default();
        let bridge = CommandServiceRunner::new(&cmd_runner);
        let adapter = ServiceAdapterFactory::create_for(ServiceOs::Windows);
        adapter
            .install(&bridge, &env, Path::new(r"C:\Ruum\ruum.exe"))
            .unwrap();

        let calls = cmd_runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[1].2,
            vec![("RUUM_EXE".to_string(), r"C:\Ruum\ruum.exe".to_string())]
        );
    }
}
