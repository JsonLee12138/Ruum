//! Deploy only the matching embedded scanner, over the existing SSH pipe.

use crate::payload::Archive;
use std::io::{self, BufRead, Read as _, Write};

// The cache is content-addressed, private and atomically published. We never
// overwrite a system helper or trust an unchecked cached executable. The
// ready handshake keeps scan JSON out of the binary transfer's stdin.
// Wrapped in a single-line `/bin/sh -c '...'` with zero inner `'`, `\`, `!`,
// or newlines so any remote login shell (fish, csh/tcsh, nu, elvish, etc.)
// hands execution cleanly to `/bin/sh` without modifying `stdin`.
pub const COMMAND: &str = concat!(
    "/bin/sh -c '",
    "set -eu;",
    r#"printf "RUUM_BOOTSTRAP_V1 %s %s" "$(uname -s)" "$(uname -m)";"#,
    "echo;",
    r#"IFS=" " read -r digest size || exit 2;"#,
    r#"[ "${#digest}" -eq 64 ] || exit 2;"#,
    "s=$digest;",
    r#"while [ "${#s}" -gt 0 ]; do "#,
    "rest=${s#?};",
    r#"c=${s%"$rest"};"#,
    r#"case "$c" in 0|1|2|3|4|5|6|7|8|9|a|b|c|d|e|f) ;; *) exit 2;; esac;"#,
    "s=$rest;",
    "done;",
    r#"case "$size" in ""|0*) exit 2;; esac;"#,
    r#"[ "${#size}" -le 8 ] || exit 2;"#,
    "s=$size;",
    r#"while [ "${#s}" -gt 0 ]; do "#,
    "rest=${s#?};",
    r#"c=${s%"$rest"};"#,
    r#"case "$c" in 0|1|2|3|4|5|6|7|8|9) ;; *) exit 2;; esac;"#,
    "s=$rest;",
    "done;",
    r#"[ "$size" -gt 0 ] && [ "$size" -le 67108864 ] || exit 2;"#,
    "umask 077;",
    r#"dir="${XDG_CACHE_HOME:-$HOME/.cache}/ruum/ssh/$digest";"#,
    r#"mkdir -p -- "$dir";"#,
    r#"helper="$dir/ruum-scan";"#,
    r#"valid() { [ -x "$helper" ] || return 1;"#,
    r#"actual=$(sha256sum -- "$helper") || return 1;"#,
    r#"[ "${actual%% *}" = "$digest" ]; };"#,
    r#"temporary="";"#,
    r#"cleanup() { case "$temporary" in "") ;; "#,
    r#"*) rm -f -- "$temporary";; esac; };"#,
    "if valid; then :; else ",
    "trap cleanup EXIT HUP INT TERM;",
    r#"temporary=$(mktemp "$dir/.upload.XXXXXX");"#,
    "echo RUUM_UPLOAD_V1;",
    r#"head -c "$size" > "$temporary" || exit 3;"#,
    r#"[ "$(wc -c < "$temporary")" -eq "$size" ] || exit 3;"#,
    r#"actual=$(sha256sum -- "$temporary") || exit 3;"#,
    r#"[ "${actual%% *}" = "$digest" ] || exit 3;"#,
    r#"chmod -- 700 "$temporary";"#,
    r#"mv -f -- "$temporary" "$helper";"#,
    r#"temporary="";"#,
    "trap - EXIT HUP INT TERM;",
    "fi;",
    "echo RUUM_READY_V1;",
    r#"exec "$helper" --stdio-v1"#,
    "'",
);

fn control_line(input: &mut impl BufRead) -> io::Result<String> {
    let mut line = String::new();
    input.take(1024).read_line(&mut line)?;
    if !line.ends_with('\n') {
        return Err(io::Error::other("incomplete SSH deployment handshake"));
    }
    Ok(line.trim_end().into())
}

pub fn deploy(
    archive: &Archive,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<()> {
    deploy_with_progress(archive, input, output, |_| {})
}

pub(crate) fn deploy_with_progress(
    archive: &Archive,
    input: &mut impl BufRead,
    output: &mut impl Write,
    mut progress: impl FnMut(crate::remote_progress::Stage),
) -> io::Result<()> {
    let hello = control_line(input)?;
    progress(crate::remote_progress::Stage::CheckingHelper);
    let parts: Vec<_> = hello.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "RUUM_BOOTSTRAP_V1" {
        return Err(io::Error::other("unexpected SSH deployment handshake"));
    }
    if parts[1] != "Linux" {
        return Err(io::Error::other(format!(
            "embedded scanners support Linux, not {}",
            parts[1],
        )));
    }
    let arch = match parts[2] {
        "x86_64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        other => {
            return Err(io::Error::other(format!(
                "unsupported remote CPU: {other}",
            )));
        }
    };
    let (entry, bytes) = archive.load(arch)?;
    writeln!(output, "{} {}", entry.sha256, entry.length)?;
    output.flush()?;
    let mut state = control_line(input)?;
    if state == "RUUM_UPLOAD_V1" {
        progress(crate::remote_progress::Stage::UploadingHelper);
        output.write_all(&bytes)?;
        output.flush()?;
        state = control_line(input)?;
    }
    if state != "RUUM_READY_V1" {
        return Err(io::Error::other("remote scanner deployment failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::io::BufReader;
    #[cfg(unix)]
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::process::{Child, Command, Stdio};

    #[cfg(unix)]
    fn find_in_path(name: &str) -> Option<PathBuf> {
        let mut dirs = std::env::var_os("PATH")
            .map(|val| std::env::split_paths(&val).collect::<Vec<_>>())
            .unwrap_or_default();
        for extra in ["/opt/homebrew/bin", "/usr/local/bin"] {
            let p = PathBuf::from(extra);
            if !dirs.contains(&p) {
                dirs.push(p);
            }
        }
        dirs.into_iter()
            .map(|dir| dir.join(name))
            .find(|cand| cand.is_file())
    }

    #[cfg(unix)]
    fn available_shells() -> Vec<String> {
        let candidates = [
            "/bin/sh",
            "/bin/dash",
            "bash",
            "zsh",
            "/bin/csh",
            "/bin/tcsh",
            "fish",
            "nu",
        ];
        let mut shells = Vec::new();
        for cand in candidates {
            let resolved = if cand.starts_with('/') {
                Path::new(cand).is_file().then(|| cand.to_owned())
            } else {
                find_in_path(cand).map(|p| p.to_string_lossy().into_owned())
            };
            let Some(shell) = resolved else {
                continue;
            };
            if shells.contains(&shell) {
                continue;
            }
            let ok = Command::new(&shell)
                .args(["-c", "echo ok"])
                .output()
                .is_ok_and(|out| {
                    out.status.success()
                        && String::from_utf8_lossy(&out.stdout).trim() == "ok"
                });
            if ok {
                shells.push(shell);
            }
        }
        assert!(
            shells.iter().any(|s| s == "/bin/sh"),
            "expected /bin/sh to be available: {shells:?}"
        );
        shells
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::write(path, body).expect("write executable script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod 755");
    }

    #[cfg(unix)]
    fn install_sha256sum_shim(bin_dir: &Path) {
        let shim = if let Some(shasum) = find_in_path("shasum") {
            let quoted = crate::export::shell_quote(
                shasum.to_str().expect("utf8 shasum path"),
            );
            format!(
                "#!/bin/sh\n\
                 if [ \"${{1-}}\" = \"--\" ]; then shift; fi\n\
                 exec {quoted} -a 256 \"$@\"\n"
            )
        } else {
            let real = find_in_path("sha256sum")
                .expect("expected shasum or sha256sum on Unix test host");
            let quoted = crate::export::shell_quote(
                real.to_str().expect("utf8 sha256sum path"),
            );
            format!(
                "#!/bin/sh\n\
                 if [ \"${{1-}}\" = \"--\" ]; then shift; fi\n\
                 exec {quoted} -- \"$@\"\n"
            )
        };
        write_executable(&bin_dir.join("sha256sum"), &shim);
    }

    #[cfg(unix)]
    fn build_test_path(bin_dir: &Path) -> std::ffi::OsString {
        let mut paths = vec![bin_dir.to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        std::env::join_paths(paths).expect("join PATH")
    }

    #[cfg(unix)]
    fn spawn_command_in_shell(
        shell: &str,
        home: &Path,
        cache: &Path,
        path_env: &std::ffi::OsStr,
        lc_all: Option<&str>,
    ) -> Child {
        let mut cmd = Command::new(shell);
        cmd.args(["-c", COMMAND])
            .env("HOME", home)
            .env("XDG_CACHE_HOME", cache)
            .env("PATH", path_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(loc) = lc_all {
            cmd.env("LC_ALL", loc);
        }
        cmd.spawn().expect("spawn shell with deployment COMMAND")
    }

    #[cfg(unix)]
    fn read_boot_line(
        stdout: &mut BufReader<std::process::ChildStdout>,
        shell: &str,
        context: &str,
    ) {
        let mut boot = String::new();
        let n = stdout.read_line(&mut boot).expect("read bootstrap line");
        assert!(
            n > 0 && boot.starts_with("RUUM_BOOTSTRAP_V1 "),
            "shell={shell} context={context} unexpected bootstrap={boot:?}"
        );
    }

    #[test]
    fn command_is_single_line_bin_sh_c_without_outer_shell_hazards() {
        assert!(
            COMMAND.starts_with("/bin/sh -c '"),
            "deployment COMMAND must start with /bin/sh -c ': {COMMAND}"
        );
        assert!(
            COMMAND.ends_with('\''),
            "deployment COMMAND must end with single quote: {COMMAND}"
        );
        assert!(
            !COMMAND.contains(['\n', '\r', '\0']),
            "deployment COMMAND must be single-line without NUL"
        );
        let inner = COMMAND
            .strip_prefix("/bin/sh -c '")
            .and_then(|s| s.strip_suffix('\''))
            .expect("wrapped in /bin/sh -c '...'");
        assert!(!inner.is_empty());
        for forbidden in ['\'', '\\', '!', '\n', '\r', '\0'] {
            assert!(
                !inner.contains(forbidden),
                "forbidden char {forbidden:?} in deployment inner script"
            );
        }
        for flag_end in [
            "mkdir -p --",
            "sha256sum --",
            "chmod -- 700",
            "mv -f --",
            "rm -f --",
        ] {
            assert!(
                inner.contains(flag_end),
                "missing `--` option terminator for `{flag_end}`"
            );
        }
    }

    #[test]
    fn deploy_with_progress_rejects_leading_stdout_noise_strictly() {
        let dir = tempfile::tempdir().unwrap();
        let pack_path = dir.path().join("test.pack");
        let helper = dir.path().join("helper");
        let mut elf = vec![0; 120];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[18..20].copy_from_slice(&62_u16.to_le_bytes());
        elf[32..40].copy_from_slice(&64_u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56_u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1_u16.to_le_bytes());
        std::fs::write(&helper, &elf).unwrap();
        crate::payload::write_pack(&pack_path, &[("x86_64".into(), helper)])
            .unwrap();
        let archive = Archive::open(&pack_path).unwrap().unwrap();

        let noisy = b"Welcome to fish\nRUUM_BOOTSTRAP_V1 Linux x86_64\n";
        let mut out = Vec::new();
        let err =
            deploy_with_progress(&archive, &mut &noisy[..], &mut out, |_| {})
                .unwrap_err();
        assert!(
            err.to_string()
                .contains("unexpected SSH deployment handshake")
        );
        assert!(out.is_empty());

        let truncated = b"RUUM_BOOTSTRAP_V1 Linux x86_64";
        let mut out2 = Vec::new();
        let err2 = deploy_with_progress(
            &archive,
            &mut &truncated[..],
            &mut out2,
            |_| {},
        )
        .unwrap_err();
        assert!(
            err2.to_string()
                .contains("incomplete SSH deployment handshake")
        );
    }

    #[cfg(unix)]
    #[test]
    fn deployment_command_cross_shell_upload_cache_hit_and_repair() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        install_sha256sum_shim(&bin_dir);
        let path_env = build_test_path(&bin_dir);

        let payload =
            b"#!/bin/sh\nIFS= read -r req\nprintf '%s|%s\\n' \"${1-}\" \"$req\"\n";
        let digest = crate::payload::digest(payload);
        let size = payload.len();

        for (idx, shell) in available_shells().into_iter().enumerate() {
            let root = dir.path().join(format!("shell_{idx}"));
            let home = root.join("home");
            let cache = root.join("cache");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&cache).unwrap();

            // 1. Cache miss: upload helper, chmod 0700, exec --stdio-v1.
            let mut child =
                spawn_command_in_shell(&shell, &home, &cache, &path_env, None);
            let mut stdin = child.stdin.take().unwrap();
            let mut stdout = BufReader::new(child.stdout.take().unwrap());
            let mut boot = String::new();
            assert!(
                stdout.read_line(&mut boot).unwrap() > 0,
                "shell={shell} closed stdout before bootstrap"
            );
            assert!(
                boot.starts_with("RUUM_BOOTSTRAP_V1 "),
                "shell={shell} bad boot: {boot:?}"
            );
            writeln!(stdin, "{digest} {size}").unwrap();
            stdin.flush().unwrap();

            let mut upload = String::new();
            assert!(stdout.read_line(&mut upload).unwrap() > 0);
            assert_eq!(upload, "RUUM_UPLOAD_V1\n", "shell={shell}");
            stdin.write_all(payload).unwrap();
            stdin.flush().unwrap();

            let mut ready = String::new();
            assert!(stdout.read_line(&mut ready).unwrap() > 0);
            assert_eq!(ready, "RUUM_READY_V1\n", "shell={shell}");
            stdin.write_all(b"{\"miss\":1}\n").unwrap();
            drop(stdin);

            let mut helper_out = String::new();
            stdout.read_line(&mut helper_out).unwrap();
            assert_eq!(helper_out, "--stdio-v1|{\"miss\":1}\n");
            let status = child.wait().unwrap();
            assert!(status.success(), "shell={shell} status={status}");

            let cached = cache.join("ruum/ssh").join(&digest).join("ruum-scan");
            assert_eq!(std::fs::read(&cached).unwrap(), payload);
            let mode = std::fs::metadata(&cached).unwrap().permissions().mode()
                & 0o777;
            assert_eq!(mode, 0o700, "shell={shell}");
            let leftover: Vec<_> = std::fs::read_dir(cached.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| {
                    e.file_name().to_string_lossy().starts_with(".upload.")
                })
                .collect();
            assert!(leftover.is_empty(), "shell={shell} leftover={leftover:?}");

            // 2. Cache hit: skips RUUM_UPLOAD_V1 and executes cached helper.
            let mut hit_child =
                spawn_command_in_shell(&shell, &home, &cache, &path_env, None);
            let mut hit_in = hit_child.stdin.take().unwrap();
            let mut hit_out = BufReader::new(hit_child.stdout.take().unwrap());
            let mut hit_boot = String::new();
            hit_out.read_line(&mut hit_boot).unwrap();
            assert!(hit_boot.starts_with("RUUM_BOOTSTRAP_V1 "));
            writeln!(hit_in, "{digest} {size}").unwrap();
            hit_in.flush().unwrap();
            let mut hit_ready = String::new();
            hit_out.read_line(&mut hit_ready).unwrap();
            assert_eq!(hit_ready, "RUUM_READY_V1\n", "shell={shell}");
            hit_in.write_all(b"{\"hit\":2}\n").unwrap();
            drop(hit_in);
            let mut hit_resp = String::new();
            hit_out.read_line(&mut hit_resp).unwrap();
            assert_eq!(hit_resp, "--stdio-v1|{\"hit\":2}\n");
            assert!(hit_child.wait().unwrap().success());

            // 3. Corrupted cache: repaired before execution.
            std::fs::write(&cached, b"corrupted").unwrap();
            let mut fix_child =
                spawn_command_in_shell(&shell, &home, &cache, &path_env, None);
            let mut fix_in = fix_child.stdin.take().unwrap();
            let mut fix_out = BufReader::new(fix_child.stdout.take().unwrap());
            let mut fix_boot = String::new();
            fix_out.read_line(&mut fix_boot).unwrap();
            writeln!(fix_in, "{digest} {size}").unwrap();
            fix_in.flush().unwrap();
            let mut fix_up = String::new();
            fix_out.read_line(&mut fix_up).unwrap();
            assert_eq!(fix_up, "RUUM_UPLOAD_V1\n", "shell={shell}");
            fix_in.write_all(payload).unwrap();
            fix_in.flush().unwrap();
            let mut fix_ready = String::new();
            fix_out.read_line(&mut fix_ready).unwrap();
            assert_eq!(fix_ready, "RUUM_READY_V1\n", "shell={shell}");
            fix_in.write_all(b"{\"repair\":3}\n").unwrap();
            drop(fix_in);
            let mut fix_resp = String::new();
            fix_out.read_line(&mut fix_resp).unwrap();
            assert_eq!(fix_resp, "--stdio-v1|{\"repair\":3}\n");
            assert!(fix_child.wait().unwrap().success());
            assert_eq!(std::fs::read(&cached).unwrap(), payload);
        }
    }

    #[cfg(unix)]
    #[test]
    fn deployment_command_fail_closed_rejection_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        install_sha256sum_shim(&bin_dir);
        let path_env = build_test_path(&bin_dir);

        let valid_64 = "a".repeat(64);
        let traversal_64 = format!("{}abcd", "../".repeat(20));
        assert_eq!(traversal_64.len(), 64);
        let upper_64 = "A".repeat(64);
        let mixed_64 = format!("{}F", "0".repeat(63));
        let short_63 = "a".repeat(63);
        let long_65 = "a".repeat(65);

        let bad_header_cases = [
            ("traversal_64", format!("{traversal_64} 10\n")),
            ("upper_64", format!("{upper_64} 10\n")),
            ("mixed_upper_64", format!("{mixed_64} 10\n")),
            ("short_63", format!("{short_63} 10\n")),
            ("long_65", format!("{long_65} 10\n")),
            ("empty_size", format!("{valid_64}\n")),
            ("leading_zero_size", format!("{valid_64} 0123\n")),
            ("zero_size", format!("{valid_64} 0\n")),
            ("over_max_size", format!("{valid_64} 67108865\n")),
            ("trailing_junk_size", format!("{valid_64} 10 evil\n")),
            ("negative_size", format!("{valid_64} -1\n")),
            ("overflow_size", format!("{valid_64} 999999999\n")),
        ];

        for shell in available_shells() {
            for (label, line) in &bad_header_cases {
                let case_dir = tempfile::tempdir_in(dir.path()).unwrap();
                let home = case_dir.path().join("home");
                let cache = case_dir.path().join("cache");
                std::fs::create_dir_all(&home).unwrap();
                std::fs::create_dir_all(&cache).unwrap();

                let mut child = spawn_command_in_shell(
                    &shell,
                    &home,
                    &cache,
                    &path_env,
                    Some("en_US.UTF-8"),
                );
                let mut stdin = child.stdin.take().unwrap();
                let mut stdout = BufReader::new(child.stdout.take().unwrap());
                read_boot_line(&mut stdout, &shell, label);
                let _ = stdin.write_all(line.as_bytes());
                drop(stdin);
                let out = child.wait_with_output().unwrap();
                assert_eq!(
                    out.status.code(),
                    Some(2),
                    "shell={shell} case={label} stderr={}",
                    String::from_utf8_lossy(&out.stderr)
                );
                assert!(
                    !cache.join("ruum/ssh").exists(),
                    "shell={shell} case={label} must not touch cache dir"
                );
            }

            // SHA-256 mismatch and truncated upload must exit 3 and clean up
            // any `.upload.*` temp files via the trap handler.
            for (label, declared_len, upload_bytes, hex_fill) in [
                ("sha256_mismatch", 5_usize, &b"hello"[..], '0'),
                ("truncated_upload", 10_usize, &b"short"[..], '1'),
            ] {
                let case_dir = tempfile::tempdir_in(dir.path()).unwrap();
                let home = case_dir.path().join("home");
                let cache = case_dir.path().join("cache");
                std::fs::create_dir_all(&home).unwrap();
                std::fs::create_dir_all(&cache).unwrap();

                let digest =
                    std::iter::repeat_n(hex_fill, 64).collect::<String>();
                let mut child = spawn_command_in_shell(
                    &shell, &home, &cache, &path_env, None,
                );
                let mut stdin = child.stdin.take().unwrap();
                let mut stdout = BufReader::new(child.stdout.take().unwrap());
                read_boot_line(&mut stdout, &shell, label);
                writeln!(stdin, "{digest} {declared_len}").unwrap();
                stdin.flush().unwrap();

                let mut upload = String::new();
                stdout.read_line(&mut upload).unwrap();
                assert_eq!(upload, "RUUM_UPLOAD_V1\n", "shell={shell} {label}");
                let _ = stdin.write_all(upload_bytes);
                drop(stdin);

                let out = child.wait_with_output().unwrap();
                assert_eq!(
                    out.status.code(),
                    Some(3),
                    "shell={shell} case={label} stderr={}",
                    String::from_utf8_lossy(&out.stderr)
                );
                let target_dir = cache.join("ruum/ssh").join(&digest);
                let entries: Vec<_> = std::fs::read_dir(&target_dir)
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                assert!(
                    entries.is_empty(),
                    "shell={shell} case={label} leaked files: {entries:?}"
                );
            }
        }
    }
}
