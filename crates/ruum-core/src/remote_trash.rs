//! Platform-aware, `HostIdentity`-guarded remote SSH trash planning and
//! execution.
//!
//! Remote cleanup never invokes `rm`: it validates targets against the scanned
//! machine's [`Header`], verifies `HostIdentity` and mount/symlink boundaries
//! on the target host, and moves items to the OS trash (`gio trash`,
//! `trash-put`, or same-device XDG Trash on Linux; Finder `delete POSIX file`
//! on macOS).

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::export::shell_quote;
use crate::identity::HostIdentity;
use crate::remote::{Header, validate_host};
use crate::removal::{Blocked, Target, percent_encode};

const REMOTE_SYSTEM_TREES: [&str; 24] = [
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/lib64",
    "/lib32",
    "/libx32",
    "/nix/store",
    "/gnu/store",
    "/proc",
    "/run",
    "/sbin",
    "/sys",
    "/usr",
    "/var/lib",
    "/efi",
    "/home/linuxbrew/.linuxbrew",
    "/System",
    "/Library",
    "/private",
    "/cores",
    "/opt/homebrew",
    "/Network",
];

const MACOS_DATA_PREFIX: &str = "/System/Volumes/Data/";

/// Outcome of planning a remote trash operation against a captured [`Header`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemotePlan {
    pub targets: Vec<Target>,
    pub covered: Vec<Target>,
    pub blocked: Vec<Blocked>,
    pub root: PathBuf,
    pub reclaim: u64,
}

impl RemotePlan {
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.targets.iter().map(|target| target.bytes).sum()
    }

    #[must_use]
    pub const fn reclaim(&self) -> u64 {
        self.reclaim
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// Validate that a remote [`Header`] has a usable [`HostIdentity`] and root.
pub fn validate_header(header: &Header) -> io::Result<&HostIdentity> {
    let identity = validate_identity(header.identity.as_ref())
        .map_err(io::Error::other)?;
    normalize_unix(&header.root).map_err(io::Error::other)?;
    if let Some(home) = &header.home {
        normalize_unix(home).map_err(io::Error::other)?;
    }
    Ok(identity)
}

/// Plan remote trash targets against a captured scan [`Header`].
#[must_use]
pub fn plan_remote(targets: &[Target], header: &Header) -> RemotePlan {
    let macos = header.platform.eq_ignore_ascii_case("macos");
    let root_norm = normalize_unix(&header.root).ok();
    let root_path = root_norm
        .as_deref()
        .map_or_else(|| header.root.clone(), PathBuf::from);
    let mut plan = RemotePlan {
        root: root_path,
        ..RemotePlan::default()
    };
    if targets.is_empty() {
        return plan;
    }

    if let Err(reason) = validate_identity(header.identity.as_ref()) {
        for target in targets {
            plan.blocked.push(Blocked {
                path: target.path.clone(),
                reason: reason.to_owned(),
            });
        }
        return plan;
    }

    let Some(root_norm) = root_norm else {
        for target in targets {
            plan.blocked.push(Blocked {
                path: target.path.clone(),
                reason: "remote scan root is not a valid absolute path".into(),
            });
        }
        return plan;
    };
    let root_key = remote_key(&root_norm, macos);
    let home_key = header
        .home
        .as_deref()
        .and_then(|home| normalize_unix(home).ok())
        .map(|home| remote_key(&home, macos));

    let mut accepted: Vec<(String, Target)> = Vec::new();
    for target in targets {
        let norm = match normalize_unix(&target.path) {
            Ok(norm) => norm,
            Err(reason) => {
                plan.blocked.push(Blocked {
                    path: target.path.clone(),
                    reason: reason.to_owned(),
                });
                continue;
            }
        };
        let key = remote_key(&norm, macos);
        if let Some(reason) = refuse_remote(
            &norm,
            &key,
            &root_norm,
            &root_key,
            home_key.as_deref(),
            macos,
        ) {
            plan.blocked.push(Blocked {
                path: target.path.clone(),
                reason,
            });
            continue;
        }
        accepted.push((
            key,
            Target {
                path: PathBuf::from(norm),
                ..target.clone()
            },
        ));
    }

    accepted.sort_by(|left, right| left.1.path.cmp(&right.1.path));
    let mut outer: Vec<(String, Target)> = Vec::new();
    for (key, target) in accepted {
        if outer
            .iter()
            .any(|(candidate_key, _)| within_unix(&key, candidate_key))
        {
            plan.covered.push(target);
        } else {
            outer.push((key, target));
        }
    }
    plan.targets = outer.into_iter().map(|(_, target)| target).collect();
    plan.reclaim = plan.targets.iter().map(|target| target.bytes).sum();
    plan
}

/// Generate the platform-specific POSIX trash script for `targets`.
pub fn trash_script_for(
    targets: &[Target],
    header: &Header,
) -> io::Result<String> {
    let identity = validate_header(header)?;
    if targets.is_empty() {
        return Err(io::Error::other("no remote targets to trash"));
    }
    let plan = plan_remote(targets, header);
    if let Some(blocked) = plan.blocked.first() {
        return Err(io::Error::other(format!(
            "refusing remote trash for {}: {}",
            blocked.path.display(),
            blocked.reason
        )));
    }
    if plan.targets.is_empty() {
        return Err(io::Error::other("no valid remote targets to trash"));
    }

    let root = normalize_unix(&header.root).map_err(io::Error::other)?;
    let scan_home = match &header.home {
        Some(home) => Some(normalize_unix(home).map_err(io::Error::other)?),
        None => None,
    };
    let macos = header.platform.eq_ignore_ascii_case("macos");

    let mut script = String::from("set -eu\n");
    append_host_guard(&mut script, identity, macos);
    let _ = writeln!(script, "root={}", shell_quote(&root));
    if let Some(home) = &scan_home {
        let _ = writeln!(script, "scan_home={}", shell_quote(home));
    } else {
        script.push_str("scan_home=${HOME-}\n");
    }

    if macos {
        append_macos_trash_fn(&mut script);
        for target in &plan.targets {
            let path = target.path.to_str().ok_or_else(|| {
                io::Error::other("remote target path is not valid UTF-8")
            })?;
            let _ = writeln!(script, "trash_one {}", shell_quote(path));
        }
    } else {
        append_linux_trash_fn(&mut script);
        for target in &plan.targets {
            let path = target.path.to_str().ok_or_else(|| {
                io::Error::other("remote target path is not valid UTF-8")
            })?;
            let encoded = percent_encode(path);
            let _ = writeln!(
                script,
                "trash_one {} {}",
                shell_quote(path),
                shell_quote(&encoded)
            );
        }
    }
    Ok(script)
}

/// Build the SSH command that feeds the remote trash script to `sh -s`.
pub fn build_ssh_trash_command(host: &str) -> io::Result<Command> {
    validate_host(host)?;
    let mut command = Command::new("ssh");
    command.args([
        "-T",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=2",
        "--",
        host,
        "sh -s",
    ]);
    Ok(command)
}

/// Execute `trash_script_for` over SSH against `host`.
pub fn execute_remote_trash(
    host: &str,
    targets: &[Target],
    header: &Header,
) -> io::Result<()> {
    let command = build_ssh_trash_command(host)?;
    let script = trash_script_for(targets, header)?;
    run_trash_command(command, &script)
}

pub(crate) fn run_trash_command(
    mut command: Command,
    script: &str,
) -> io::Result<()> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let reason = if stderr.is_empty() {
        format!("remote trash exited with {}", output.status)
    } else {
        stderr
    };
    Err(io::Error::other(reason))
}

fn validate_identity(
    identity: Option<&HostIdentity>,
) -> Result<&HostIdentity, &'static str> {
    let Some(identity) = identity else {
        return Err("remote scan has no host identity");
    };
    if identity.hostname.is_empty()
        || !identity
            .hostname
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
    {
        return Err("remote scan has an invalid hostname");
    }
    if let Some(id) = &identity.machine_id
        && (id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err("remote scan has an invalid machine ID");
    }
    Ok(identity)
}

fn normalize_unix(path: &Path) -> Result<String, &'static str> {
    let Some(text) = path.to_str() else {
        return Err("path is not valid UTF-8");
    };
    if text.is_empty() || !text.starts_with('/') {
        return Err("relative path cannot be removed");
    }
    if text.chars().any(char::is_control) {
        return Err("path contains control characters");
    }
    let mut parts: Vec<&str> = Vec::new();
    for segment in text.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        Ok("/".to_owned())
    } else {
        Ok(format!("/{}", parts.join("/")))
    }
}

fn remote_key(norm: &str, macos: bool) -> String {
    if !macos {
        return norm.to_owned();
    }
    let folded = match norm.strip_prefix(MACOS_DATA_PREFIX) {
        Some(rest) if !rest.is_empty() => format!("/{rest}"),
        _ => norm.to_owned(),
    };
    folded.to_ascii_lowercase()
}

fn within_unix(path: &str, base: &str) -> bool {
    if base == "/" {
        return path.starts_with('/');
    }
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn refuse_remote(
    norm: &str,
    key: &str,
    root_norm: &str,
    root_key: &str,
    home_key: Option<&str>,
    macos: bool,
) -> Option<String> {
    if norm == "/" || key == "/" {
        return Some("the filesystem root cannot be removed".into());
    }
    if key == root_key || norm == root_norm {
        return Some("the scanned root cannot be removed".into());
    }
    if let Some(home) = home_key {
        if key == home {
            return Some("the home directory cannot be removed".into());
        }
        if within_unix(home, key) {
            return Some("it contains the home directory".into());
        }
    }
    if !within_unix(key, root_key) && !within_unix(norm, root_norm) {
        return Some("outside the scanned root".into());
    }
    let in_home = home_key.is_some_and(|home| within_unix(key, home));
    if !in_home {
        for tree in REMOTE_SYSTEM_TREES {
            let tree_key = remote_key(tree, macos);
            if within_unix(key, &tree_key) {
                return Some(format!(
                    "part of the system under {tree}: \
                     remove it with the tool that installed it"
                ));
            }
        }
    }
    for tree in REMOTE_SYSTEM_TREES {
        let tree_key = remote_key(tree, macos);
        if tree_key != key && within_unix(&tree_key, key) {
            return Some(format!(
                "it contains {tree}, which is part of the system"
            ));
        }
    }
    None
}

fn append_host_guard(
    script: &mut String,
    identity: &HostIdentity,
    macos: bool,
) {
    if macos {
        script.push_str("actual_host=$(hostname) || exit 1\n");
    } else {
        script.push_str(
            "IFS= read -r actual_host < /proc/sys/kernel/hostname || {\n\
             \x20 echo 'Cannot read /proc/sys/kernel/hostname; \
             refusing remote trash.' >&2\n\
             \x20 exit 1\n\
             }\n",
        );
    }
    let _ = writeln!(
        script,
        "[ \"$actual_host\" = {} ] || {{ \
         echo 'Wrong hostname; refusing remote trash.' >&2; exit 1; }}",
        shell_quote(&identity.hostname)
    );
    if let Some(id) = &identity.machine_id {
        script.push_str(
            "IFS= read -r actual_id < /etc/machine-id || {\n\
             \x20 echo 'Cannot read /etc/machine-id; \
             refusing remote trash.' >&2\n\
             \x20 exit 1\n\
             }\n",
        );
        let _ = writeln!(
            script,
            "[ \"$actual_id\" = {} ] || {{ \
             echo 'Wrong machine ID; refusing remote trash.' >&2; exit 1; }}",
            shell_quote(id)
        );
    }
}

fn append_common_target_guards(script: &mut String, realpath_cmd: &str) {
    script.push_str(
        "  p=$1\n\
         \x20 case \"$root\" in /) prefix=/;; *) prefix=$root/;; esac\n\
         \x20 case \"$p\" in \"$prefix\"*) ;; *) \
         echo \"Outside scan: $p\" >&2; return 1;; esac\n\
         \x20 [ \"$p\" != \"$root\" ] || return 1\n\
         \x20 [ ! -L \"$p\" ] || { \
         echo \"Refusing symlink: $p\" >&2; return 1; }\n\
         \x20 [ -e \"$p\" ] || { \
         echo \"Missing path: $p\" >&2; return 1; }\n",
    );
    let _ = writeln!(script, "  real=$({realpath_cmd} -- \"$p\") || return 1");
    script.push_str(
        "  [ \"$real\" = \"$p\" ] || {\n\
         \x20   echo \"Changed or linked path: $p\" >&2; return 1;\n\
         \x20 }\n\
         \x20 if [ -n \"${HOME-}\" ]; then\n\
         \x20   [ \"$p\" != \"$HOME\" ] || return 1\n\
         \x20   case \"$HOME/\" in \"$p/\"*) \
         echo \"Contains home: $p\" >&2; return 1;; esac\n\
         \x20 fi\n\
         \x20 if [ -n \"${scan_home-}\" ]; then\n\
         \x20   [ \"$p\" != \"$scan_home\" ] || return 1\n\
         \x20   case \"$scan_home/\" in \"$p/\"*) \
         echo \"Contains home: $p\" >&2; return 1;; esac\n\
         \x20 fi\n\
         \x20 for system in /bin /boot /dev /etc /lib /lib64 /lib32 /libx32 \
         /nix/store /gnu/store /proc /run /sbin /sys /usr /var/lib /efi \
         /home/linuxbrew/.linuxbrew /System /Library /private /cores \
         /opt/homebrew /Network; do\n\
         \x20   case \"$p/\" in \"$system/\"*) \
         echo \"System path: $p\" >&2; return 1;; esac\n\
         \x20   case \"$system/\" in \"$p/\"*) \
         echo \"Contains system: $p\" >&2; return 1;; esac\n\
         \x20 done\n\
         \x20 root_dev=$(df -P -- \"$root\" | awk 'NR==2 {print $1}') \
         || return 1\n\
         \x20 target_dev=$(df -P -- \"$p\" | awk 'NR==2 {print $1}') \
         || return 1\n\
         \x20 [ -n \"$root_dev\" ] && [ \"$target_dev\" = \"$root_dev\" ] || {\n\
         \x20   echo \"Crosses filesystem: $p\" >&2; return 1;\n\
         \x20 }\n",
    );
}

fn append_linux_trash_fn(script: &mut String) {
    script.push_str(
        "command -v realpath >/dev/null\n\
         command -v findmnt >/dev/null\n\
         trash_one() {\n",
    );
    append_common_target_guards(script, "realpath -e");
    script.push_str(
        "  encoded=$2\n\
         \x20 mounts=$(findmnt -rn --raw -o TARGET) || return 1\n\
         \x20 printf '%s\\n' \"$mounts\" | while IFS= read -r mount; do\n\
         \x20   mount=$(env printf '%b' \"$mount\") || exit 1\n\
         \x20   case \"$mount/\" in \"$p/\"*) \
         echo \"Contains mount: $p\" >&2; exit 1;; esac\n\
         \x20 done\n\
         \x20 if command -v gio >/dev/null 2>&1 && gio trash -- \"$p\"; then\n\
         \x20   return 0\n\
         \x20 fi\n\
         \x20 if command -v trash-put >/dev/null 2>&1 \
         && trash-put -- \"$p\"; then\n\
         \x20   return 0\n\
         \x20 fi\n\
         \x20 xdg_trash=\"${XDG_DATA_HOME:-${HOME:?HOME not set}/.local/share}\
         /Trash\"\n\
         \x20 mkdir -p -- \"$xdg_trash/files\" \"$xdg_trash/info\" || return 1\n\
         \x20 trash_dev=$(df -P -- \"$xdg_trash/files\" \
         | awk 'NR==2 {print $1}') || return 1\n\
         \x20 [ -n \"$trash_dev\" ] && [ \"$trash_dev\" = \"$target_dev\" ] \
         || {\n\
         \x20   echo \"Different filesystem from XDG Trash: $p\" >&2\n\
         \x20   return 1\n\
         \x20 }\n\
         \x20 base=$(basename -- \"$p\") || return 1\n\
         \x20 name=$base\n\
         \x20 dest=\"$xdg_trash/files/$name\"\n\
         \x20 i=1\n\
         \x20 while [ -e \"$dest\" ] || [ -L \"$dest\" ] \
         || [ -e \"$xdg_trash/info/$name.trashinfo\" ]; do\n\
         \x20   name=\"$base.$i\"\n\
         \x20   dest=\"$xdg_trash/files/$name\"\n\
         \x20   i=$((i + 1))\n\
         \x20 done\n\
         \x20 del_date=$(date +%Y-%m-%dT%H:%M:%S) || return 1\n\
         \x20 printf '[Trash Info]\\nPath=%s\\nDeletionDate=%s\\n' \
         \"$encoded\" \"$del_date\" > \"$xdg_trash/info/$name.trashinfo\" \
         || return 1\n\
         \x20 if ! mv -- \"$p\" \"$dest\"; then\n\
         \x20   unlink \"$xdg_trash/info/$name.trashinfo\" 2>/dev/null || true\n\
         \x20   return 1\n\
         \x20 fi\n\
         }\n",
    );
}

fn append_macos_trash_fn(script: &mut String) {
    script.push_str(
        "command -v realpath >/dev/null\n\
         command -v osascript >/dev/null\n\
         trash_one() {\n",
    );
    append_common_target_guards(script, "realpath");
    script.push_str(
        "  mounts=$(/sbin/mount \
         | sed -n 's/^.* on \\(.*\\) ([^()]*)$/\\1/p') || return 1\n\
         \x20 printf '%s\\n' \"$mounts\" | while IFS= read -r mount; do\n\
         \x20   [ -n \"$mount\" ] || continue\n\
         \x20   case \"$mount/\" in \"$p/\"*) \
         echo \"Contains mount: $p\" >&2; exit 1;; esac\n\
         \x20 done\n\
         \x20 RUUM_TRASH_TARGET=\"$p\" osascript -e \
         'tell application \"Finder\" to delete POSIX file \
         (system attribute \"RUUM_TRASH_TARGET\")' >/dev/null\n\
         }\n",
    );
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::*;
    use crate::identity::HostIdentity;
    use crate::remote::Header;
    use crate::removal::Target;

    fn target(path: &str, bytes: u64, is_dir: bool) -> Target {
        Target {
            path: PathBuf::from(path),
            bytes,
            is_dir,
            hidden: false,
        }
    }

    fn linux_header(root: &str, home: Option<&str>) -> Header {
        Header::new(
            PathBuf::from(root),
            home.map(PathBuf::from),
            "linux",
            Some(HostIdentity {
                hostname: "prod-box-01".into(),
                machine_id: Some("0123456789abcdef0123456789abcdef".into()),
            }),
        )
    }

    fn macos_header(root: &str, home: Option<&str>) -> Header {
        Header::new(
            PathBuf::from(root),
            home.map(PathBuf::from),
            "macos",
            Some(HostIdentity {
                hostname: "mac-studio.local".into(),
                machine_id: None,
            }),
        )
    }

    #[test]
    fn plan_remote_blocks_missing_or_invalid_identity() {
        let mut header = linux_header("/home/me", Some("/home/me"));
        header.identity = None;
        let plan =
            plan_remote(&[target("/home/me/cache", 1024, true)], &header);
        assert!(plan.targets.is_empty());
        assert_eq!(plan.blocked.len(), 1);
        assert!(
            plan.blocked[0].reason.contains("identity"),
            "{}",
            plan.blocked[0].reason
        );
        assert!(
            trash_script_for(&[target("/home/me/cache", 1024, true)], &header)
                .is_err()
        );

        let mut bad_host = linux_header("/home/me", Some("/home/me"));
        bad_host.identity = Some(HostIdentity {
            hostname: "bad host; rm".into(),
            machine_id: None,
        });
        assert!(validate_header(&bad_host).is_err());

        let mut bad_id = linux_header("/home/me", Some("/home/me"));
        bad_id.identity = Some(HostIdentity {
            hostname: "valid-host".into(),
            machine_id: Some("short".into()),
        });
        assert!(validate_header(&bad_id).is_err());
    }

    #[test]
    fn plan_remote_blocks_outside_root_home_system_trees_and_control_chars() {
        let header = linux_header("/", Some("/home/me"));
        let targets = [
            target("/", 100, true),
            target("/home/me", 100, true),
            target("/home", 100, true),
            target("/usr/local", 100, true),
            target("/var", 100, true),
            target("/home/me/bad\nname", 100, false),
            target("/home/me/cache", 500, true),
            target("/home/me/cache/nested", 200, false),
        ];
        let plan = plan_remote(&targets, &header);
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].path, PathBuf::from("/home/me/cache"));
        assert_eq!(plan.reclaim(), 500);
        assert_eq!(plan.bytes(), 500);
        assert_eq!(plan.covered.len(), 1);
        assert_eq!(
            plan.covered[0].path,
            PathBuf::from("/home/me/cache/nested")
        );
        assert_eq!(plan.blocked.len(), 6);

        let sub_header = linux_header("/home/me/work", Some("/home/me"));
        let sub_plan = plan_remote(
            &[
                target("/home/me/work", 100, true),
                target("/home/me/other", 100, true),
                target("/home/me/work/../other", 100, true),
            ],
            &sub_header,
        );
        assert!(sub_plan.targets.is_empty());
        assert_eq!(sub_plan.blocked.len(), 3);
    }

    #[test]
    fn plan_remote_folds_macos_case_and_data_volume_prefix() {
        let header = macos_header("/Users/me", Some("/Users/me"));
        let plan = plan_remote(
            &[
                target("/Users/me", 100, true),
                target("/System/Volumes/Data/Users/me", 100, true),
                target("/System/Volumes/Data/Library/Caches", 100, true),
                target("/Users/me/Library/Caches/pip", 4096, true),
            ],
            &header,
        );
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(
            plan.targets[0].path,
            PathBuf::from("/Users/me/Library/Caches/pip")
        );
        assert_eq!(plan.blocked.len(), 3);
    }

    #[test]
    fn linux_trash_script_checks_identity_mounts_and_never_calls_rm() {
        let header = linux_header("/home/me", Some("/home/me"));
        let script = trash_script_for(
            &[target("/home/me/.cache/pip", 2048, true)],
            &header,
        )
        .expect("linux script");

        assert!(script.contains("/proc/sys/kernel/hostname"));
        assert!(script.contains("'prod-box-01'"));
        assert!(script.contains("/etc/machine-id"));
        assert!(script.contains("'0123456789abcdef0123456789abcdef'"));
        assert!(script.contains("[ ! -L \"$p\" ]"));
        assert!(script.contains("realpath -e -- \"$p\""));
        assert!(script.contains("findmnt -rn --raw -o TARGET"));
        assert!(script.contains("gio trash -- \"$p\""));
        assert!(script.contains("trash-put -- \"$p\""));
        assert!(script.contains("[Trash Info]"));
        assert!(!script.contains("rm -rf"), "must never contain rm -rf");
        assert!(!script.contains("rm -f"), "must never contain rm -f");
    }

    #[test]
    fn macos_trash_script_checks_hostname_device_and_uses_finder_trash() {
        let header = macos_header("/Users/me", Some("/Users/me"));
        let script = trash_script_for(
            &[target("/Users/me/Library/Caches/pip", 2048, true)],
            &header,
        )
        .expect("macos script");

        assert!(script.contains("actual_host=$(hostname)"));
        assert!(script.contains("'mac-studio.local'"));
        assert!(script.contains("[ ! -L \"$p\" ]"));
        assert!(script.contains("realpath -- \"$p\""));
        assert!(script.contains("df -P -- \"$root\""));
        assert!(script.contains("RUUM_TRASH_TARGET=\"$p\" osascript"));
        assert!(script.contains("delete POSIX file"));
        assert!(!script.contains("rm -rf"), "must never contain rm -rf");
        assert!(!script.contains("rm -f"), "must never contain rm -f");
    }

    #[test]
    fn ssh_trash_command_validates_host_and_uses_sh_stdin() {
        assert!(build_ssh_trash_command("-oProxyCommand=evil").is_err());
        assert!(build_ssh_trash_command("host; rm -rf /").is_err());

        let cmd = build_ssh_trash_command("user@dev-box").expect("valid host");
        assert_eq!(cmd.get_program(), OsStr::new("ssh"));
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert!(args.contains(&OsStr::new("-T")));
        assert!(args.contains(&OsStr::new("--")));
        assert!(args.contains(&OsStr::new("user@dev-box")));
        assert!(args.contains(&OsStr::new("sh -s")));
    }

    #[cfg(unix)]
    #[test]
    fn generated_scripts_pass_sh_syntax_check_and_refuse_wrong_host() {
        let dir = tempfile::tempdir().expect("tempdir");
        let keep = dir.path().join("keep.bin");
        std::fs::write(&keep, b"important").expect("write");

        let linux = linux_header(
            dir.path().to_str().expect("utf8"),
            dir.path().to_str(),
        );
        let mac = macos_header(
            dir.path().to_str().expect("utf8"),
            dir.path().to_str(),
        );
        let item = [target(keep.to_str().expect("utf8"), 9, false)];

        for header in [&linux, &mac] {
            let script = trash_script_for(&item, header).expect("script");
            let mut syntax = Command::new("sh");
            syntax.arg("-n").arg("-s");
            run_trash_command(syntax, &script)
                .expect("POSIX sh -n syntax valid");

            let mut exec = Command::new("sh");
            exec.arg("-s");
            let err = run_trash_command(exec, &script)
                .expect_err("wrong host must abort");
            assert!(
                err.to_string().contains("refusing remote trash"),
                "unexpected error: {err}"
            );
            assert!(keep.exists(), "file must remain untouched on wrong host");
        }
    }
}
