//! Out-of-band OS authentication and confirmation for permanent deletion.
//!
//! Permanent deletion (`delete_permanently`) challenges the human operator
//! through an OS-native dialog before removing anything. On macOS it invokes
//! `LocalAuthentication` (`LAContext` `LAPolicyDeviceOwnerAuthentication`) so
//! Touch ID is used whenever enabled, with password fallback. Prompts pass
//! user-supplied paths and reasons through environment variables rather than
//! interpolating them into scripts, and any cancellation, non-zero exit
//! status, or missing GUI session fails closed.

use std::fmt::Write as _;
use std::io;
use std::process::{Command, Stdio};

use crate::removal::Plan;
use crate::size::human_bytes;

const SAMPLE_LIMIT: usize = 5;

/// Default authentication reason shown in the OS prompt when no custom reason
/// is supplied by the caller.
pub const DEFAULT_AUTH_REASON: &str = "清理回收站";

const MACOS_SCRIPT: &str = "\
ObjC.import('Foundation');\
ObjC.import('LocalAuthentication');\
ObjC.import('stdlib');\
const env = $.NSProcessInfo.processInfo.environment;\
const reason = ObjC.unwrap(env.objectForKey('RUUM_AUTH_REASON')) || '清理回收站';\
const ctx = $.LAContext.alloc.init;\
if (!ctx.canEvaluatePolicyError(2, null)) { $.exit(1); }\
let done = false;\
let ok = false;\
ctx.evaluatePolicyLocalizedReasonReply(2, reason, function(success, _err) {\
    ok = Boolean(success);\
    done = true;\
});\
while (!done) {\
    $.NSRunLoop.currentRunLoop.runUntilDate(\
        $.NSDate.dateWithTimeIntervalSinceNow(0.05)\
    );\
}\
$.exit(ok ? 0 : 1);";

const LINUX_SCRIPT: &str = "\
if command -v zenity >/dev/null 2>&1 \
&& { [ -n \"${DISPLAY:-}\" ] || [ -n \"${WAYLAND_DISPLAY:-}\" ]; }; then \
exec zenity --question --default-cancel \
--title=\"$RUUM_AUTH_TITLE\" --text=\"$RUUM_AUTH_REASON\"; \
elif command -v kdialog >/dev/null 2>&1 \
&& { [ -n \"${DISPLAY:-}\" ] || [ -n \"${WAYLAND_DISPLAY:-}\" ]; }; then \
exec kdialog --title \"$RUUM_AUTH_TITLE\" \
--yesno \"$RUUM_AUTH_REASON\"; \
elif command -v pkexec >/dev/null 2>&1; then \
exec pkexec /bin/true; \
else exit 1; fi";

const WINDOWS_SCRIPT: &str = "\
Add-Type -AssemblyName System.Windows.Forms; \
$res = [System.Windows.Forms.MessageBox]::Show(\
$env:RUUM_AUTH_REASON, \
$env:RUUM_AUTH_TITLE, \
[System.Windows.Forms.MessageBoxButtons]::YesNo, \
[System.Windows.Forms.MessageBoxIcon]::Warning, \
[System.Windows.Forms.MessageBoxDefaultButton]::Button2); \
if ($res -eq [System.Windows.Forms.DialogResult]::Yes) \
{ exit 0 } else { exit 1 }";

/// Operating system family for out-of-band authentication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthOs {
    MacOs,
    Linux,
    Windows,
}

/// Domain-agnostic challenge payload presented to the operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthRequest {
    pub title: String,
    pub reason: String,
    pub items: Vec<String>,
}

impl AuthRequest {
    /// Summarize a permanent-deletion [`Plan`] with the default reason
    /// (`"清理回收站"`).
    #[must_use]
    pub fn for_permanent_delete(plan: &Plan) -> Self {
        Self::for_permanent_delete_with_reason(plan, None)
    }

    /// Summarize a permanent-deletion [`Plan`] with an optional caller-supplied
    /// reason (defaults to [`DEFAULT_AUTH_REASON`] = `"清理回收站"`).
    #[must_use]
    pub fn for_permanent_delete_with_reason(
        plan: &Plan,
        caller_reason: Option<&str>,
    ) -> Self {
        let purpose = caller_reason
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or(DEFAULT_AUTH_REASON);
        let items: Vec<String> = plan
            .targets
            .iter()
            .map(|target| target.path.display().to_string())
            .collect();
        let count = items.len();
        let noun = if count == 1 { "item" } else { "items" };
        let total = human_bytes(plan.bytes());
        let mut reason = format!(
            "{purpose} — Permanently delete {count} {noun} ({total}) under {}?",
            plan.root.display()
        );
        if !items.is_empty() {
            reason.push('\n');
            for item in items.iter().take(SAMPLE_LIMIT) {
                let _ = write!(reason, "\n- {item}");
            }
            if count > SAMPLE_LIMIT {
                let more = count - SAMPLE_LIMIT;
                let _ = write!(reason, "\n... and {more} more");
            }
        }
        Self {
            title: format!("Ruum — {purpose}"),
            reason,
            items,
        }
    }
}

/// Adapter interface encapsulating how an OS challenges the human operator.
pub trait AuthAdapter: Send + Sync {
    fn os_kind(&self) -> AuthOs;
    fn authenticate(&self, request: &AuthRequest) -> io::Result<bool>;
}

/// macOS native Touch ID / user password prompt via `LocalAuthentication`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsAuth;

impl MacOsAuth {
    #[must_use]
    pub fn build_command(request: &AuthRequest) -> Command {
        let mut command = Command::new("osascript");
        command.args(["-l", "JavaScript", "-e", MACOS_SCRIPT]);
        apply_auth_env(&mut command, request);
        command
    }
}

impl AuthAdapter for MacOsAuth {
    fn os_kind(&self) -> AuthOs {
        AuthOs::MacOs
    }

    fn authenticate(&self, request: &AuthRequest) -> io::Result<bool> {
        run_auth_command(Self::build_command(request))
    }
}

/// Linux graphical confirmation or polkit prompt (`zenity`/`kdialog`/`pkexec`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxAuth;

impl LinuxAuth {
    #[must_use]
    pub fn build_command(request: &AuthRequest) -> Command {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(LINUX_SCRIPT);
        apply_auth_env(&mut command, request);
        command
    }
}

impl AuthAdapter for LinuxAuth {
    fn os_kind(&self) -> AuthOs {
        AuthOs::Linux
    }

    fn authenticate(&self, request: &AuthRequest) -> io::Result<bool> {
        run_auth_command(Self::build_command(request))
    }
}

/// Windows native `WinForms` `MessageBox` confirmation prompt via `PowerShell`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsAuth;

impl WindowsAuth {
    #[must_use]
    pub fn build_command(request: &AuthRequest) -> Command {
        let mut command = Command::new("powershell.exe");
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(
            &mut command,
            0x0800_0000, // CREATE_NO_WINDOW
        );
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            WINDOWS_SCRIPT,
        ]);
        apply_auth_env(&mut command, request);
        command
    }
}

impl AuthAdapter for WindowsAuth {
    fn os_kind(&self) -> AuthOs {
        AuthOs::Windows
    }

    fn authenticate(&self, request: &AuthRequest) -> io::Result<bool> {
        run_auth_command(Self::build_command(request))
    }
}

/// Factory that directly constructs the appropriate [`AuthAdapter`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuthFactory;

impl AuthFactory {
    #[must_use]
    pub const fn current_os() -> AuthOs {
        if cfg!(target_os = "macos") {
            AuthOs::MacOs
        } else if cfg!(windows) {
            AuthOs::Windows
        } else {
            AuthOs::Linux
        }
    }

    #[must_use]
    pub fn create_for(os: AuthOs) -> Box<dyn AuthAdapter> {
        match os {
            AuthOs::MacOs => Box::new(MacOsAuth),
            AuthOs::Linux => Box::new(LinuxAuth),
            AuthOs::Windows => Box::new(WindowsAuth),
        }
    }

    #[must_use]
    pub fn create() -> Box<dyn AuthAdapter> {
        Self::create_for(Self::current_os())
    }
}

fn apply_auth_env(command: &mut Command, request: &AuthRequest) {
    command
        .env("RUUM_AUTH_TITLE", &request.title)
        .env("RUUM_AUTH_REASON", &request.reason)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
}

fn run_auth_command(mut command: Command) -> io::Result<bool> {
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::*;
    use crate::removal::Target;

    fn env_value(command: &Command, key: &str) -> Option<String> {
        command.get_envs().find_map(|(k, v)| {
            (k == OsStr::new(key))
                .then(|| v.map(|val| val.to_string_lossy().into_owned()))
                .flatten()
        })
    }

    fn command_args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn factory_builds_adapter_for_each_os() {
        let mac = AuthFactory::create_for(AuthOs::MacOs);
        assert_eq!(mac.os_kind(), AuthOs::MacOs);

        let linux = AuthFactory::create_for(AuthOs::Linux);
        assert_eq!(linux.os_kind(), AuthOs::Linux);

        let windows = AuthFactory::create_for(AuthOs::Windows);
        assert_eq!(windows.os_kind(), AuthOs::Windows);

        let host = AuthFactory::create();
        assert_eq!(host.os_kind(), AuthFactory::current_os());
    }

    #[test]
    fn permanent_delete_request_includes_reason_and_samples_paths() {
        let root = PathBuf::from("/tmp/work");
        let targets: Vec<Target> = (1..=7)
            .map(|index| Target {
                path: root.join(format!("dir-{index}")),
                bytes: 1024 * 1024,
                is_dir: true,
                hidden: false,
            })
            .collect();
        let mut plan = Plan::default();
        plan.root = root;
        plan.targets = targets;

        let request = AuthRequest::for_permanent_delete(&plan);
        assert_eq!(request.title, "Ruum — 清理回收站");
        assert!(request.reason.starts_with("清理回收站 — "));
        assert_eq!(request.items.len(), 7);
        assert!(request.reason.contains("/tmp/work"));
        assert!(request.reason.contains("7 items"));
        assert!(request.reason.contains("7.0 MiB"));
        assert!(request.reason.contains("/tmp/work/dir-1"));
        assert!(request.reason.contains("/tmp/work/dir-5"));
        assert!(!request.reason.contains("/tmp/work/dir-6"));
        assert!(request.reason.contains("2 more"));

        let custom = AuthRequest::for_permanent_delete_with_reason(
            &plan,
            Some("清理回收站"),
        );
        assert_eq!(custom.title, "Ruum — 清理回收站");
        assert!(custom.reason.starts_with("清理回收站 — "));
    }

    #[test]
    fn adapters_use_touch_id_jxa_and_isolate_prompts_in_env_vars() {
        let malicious = "pwn\"'$(touch /tmp/pwned)`$ENV:PATH`\n; rm -rf /";
        let request = AuthRequest {
            title: format!("Title {malicious}"),
            reason: format!("Reason {malicious}"),
            items: vec![malicious.to_owned()],
        };

        let mac_cmd = MacOsAuth::build_command(&request);
        assert_eq!(mac_cmd.get_program(), OsStr::new("osascript"));
        let mac_args = command_args(&mac_cmd);
        assert_eq!(mac_args.first().map(String::as_str), Some("-l"));
        assert_eq!(mac_args.get(1).map(String::as_str), Some("JavaScript"));
        assert!(
            mac_args
                .get(3)
                .is_some_and(|s| s.contains("LocalAuthentication")
                    && s.contains("evaluatePolicyLocalizedReasonReply(2"))
        );
        assert_eq!(
            env_value(&mac_cmd, "RUUM_AUTH_TITLE").as_deref(),
            Some(request.title.as_str())
        );
        assert_eq!(
            env_value(&mac_cmd, "RUUM_AUTH_REASON").as_deref(),
            Some(request.reason.as_str())
        );
        for arg in &mac_args {
            assert!(
                !arg.contains("pwn"),
                "macOS command arg interpolated untrusted input: {arg}"
            );
        }

        let linux_cmd = LinuxAuth::build_command(&request);
        assert_eq!(
            env_value(&linux_cmd, "RUUM_AUTH_TITLE").as_deref(),
            Some(request.title.as_str())
        );
        assert_eq!(
            env_value(&linux_cmd, "RUUM_AUTH_REASON").as_deref(),
            Some(request.reason.as_str())
        );
        for arg in command_args(&linux_cmd) {
            assert!(
                !arg.contains("pwn"),
                "Linux command arg interpolated untrusted input: {arg}"
            );
        }

        let win_cmd = WindowsAuth::build_command(&request);
        assert_eq!(win_cmd.get_program(), OsStr::new("powershell.exe"));
        assert_eq!(
            env_value(&win_cmd, "RUUM_AUTH_TITLE").as_deref(),
            Some(request.title.as_str())
        );
        assert_eq!(
            env_value(&win_cmd, "RUUM_AUTH_REASON").as_deref(),
            Some(request.reason.as_str())
        );
        for arg in command_args(&win_cmd) {
            assert!(
                !arg.contains("pwn"),
                "Windows command arg interpolated untrusted input: {arg}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_auth_command_fails_closed_on_nonzero_or_spawn_error() {
        let ok = run_auth_command(Command::new("true")).expect("true runs");
        assert!(ok, "zero exit code means authenticated");

        let denied =
            run_auth_command(Command::new("false")).expect("false runs");
        assert!(!denied, "non-zero exit code must fail closed");

        let missing =
            run_auth_command(Command::new("/nonexistent-ruum-auth-bin"));
        assert!(missing.is_err(), "unspawnable command must return Err");
    }
}
