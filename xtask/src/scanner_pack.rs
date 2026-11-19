//! Cross-platform static Linux `ruum-scan` builder and `.pack` packager.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const MUSL_ARCHES: [&str; 2] = ["x86_64", "aarch64"];

const USAGE: &str = "\
usage: cargo xtask scanner-pack [--out <path>] [--strict]
";

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    out: Option<PathBuf>,
    strict: bool,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut opts = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--out" => {
                let path = iter
                    .next()
                    .filter(|v| !v.is_empty())
                    .ok_or("--out requires a non-empty path")?;
                opts.out = Some(PathBuf::from(path));
            }
            "--strict" => opts.strict = true,
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return Err(String::new());
            }
            other => {
                return Err(format!("unknown scanner-pack option `{other}`"));
            }
        }
    }
    Ok(opts)
}

pub const fn rust_lld_filename(windows: bool) -> &'static str {
    if windows { "rust-lld.exe" } else { "rust-lld" }
}

pub fn target_triple(arch: &str) -> String {
    format!("{arch}-unknown-linux-musl")
}

pub fn linker_env_key(triple: &str) -> String {
    format!(
        "CARGO_TARGET_{}_LINKER",
        triple.to_uppercase().replace('-', "_")
    )
}

pub fn default_pack_path(target_dir: &Path) -> PathBuf {
    target_dir
        .join("scanner")
        .join(ruum_core::payload::PACK_FILE_NAME)
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

pub fn target_dir_for(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| root.join("target"), PathBuf::from)
}

fn rustc_output(args: &[&str]) -> Result<String, String> {
    let output = Command::new("rustc")
        .args(args)
        .output()
        .map_err(|e| format!("could not run rustc: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

pub fn missing_musl_targets(sysroot: &Path) -> Vec<String> {
    MUSL_ARCHES
        .iter()
        .map(|arch| target_triple(arch))
        .filter(|triple| {
            !sysroot
                .join("lib/rustlib")
                .join(triple)
                .join("lib")
                .is_dir()
        })
        .collect()
}

pub fn find_rust_lld(sysroot: &Path, host: &str) -> Option<PathBuf> {
    let path = sysroot
        .join("lib/rustlib")
        .join(host)
        .join("bin")
        .join(rust_lld_filename(cfg!(windows)));
    path.is_file().then_some(path)
}

pub fn build_musl_scanners(
    root: &Path,
    target_dir: &Path,
    strict: bool,
) -> Result<Option<Vec<(String, PathBuf)>>, String> {
    let sysroot = PathBuf::from(rustc_output(&["--print", "sysroot"])?);
    let version = rustc_output(&["-vV"])?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc did not report a host triple")?;

    let missing = missing_musl_targets(&sysroot);
    if !missing.is_empty() {
        let msg = format!(
            "missing Linux musl targets ({}); install with: rustup target add {}",
            missing.join(", "),
            MUSL_ARCHES
                .iter()
                .map(|a| target_triple(a))
                .collect::<Vec<_>>()
                .join(" "),
        );
        if strict {
            return Err(msg);
        }
        eprintln!("xtask scanner-pack: skipping ({msg})");
        return Ok(None);
    }

    let Some(linker) = find_rust_lld(&sysroot, host) else {
        let msg = format!(
            "{} not found under rustc sysroot for `{host}`",
            rust_lld_filename(cfg!(windows)),
        );
        if strict {
            return Err(msg);
        }
        eprintln!("xtask scanner-pack: skipping ({msg})");
        return Ok(None);
    };

    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let mut helpers = Vec::with_capacity(MUSL_ARCHES.len());
    for arch in MUSL_ARCHES {
        let triple = target_triple(arch);
        let key = linker_env_key(&triple);
        let status = Command::new(cargo)
            .current_dir(root)
            .args([
                "build",
                "--release",
                "--locked",
                "-p",
                "ruum-scan",
                "--target",
                &triple,
            ])
            .arg("--target-dir")
            .arg(target_dir)
            .env(key, &linker)
            // Isolate host RUSTFLAGS (such as Windows `+crt-static`) from
            // Linux musl cross-compilation.
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("CARGO_BUILD_RUSTFLAGS")
            .status()
            .map_err(|e| format!("could not run cargo for `{triple}`: {e}"))?;
        if !status.success() {
            return Err(format!(
                "static helper build failed for `{triple}`; install targets \
                 with: rustup target add x86_64-unknown-linux-musl \
                 aarch64-unknown-linux-musl"
            ));
        }
        helpers.push((
            arch.to_owned(),
            target_dir.join(&triple).join("release/ruum-scan"),
        ));
    }
    Ok(Some(helpers))
}

pub fn ensure_pack(
    root: &Path,
    target_dir: &Path,
    out: Option<&Path>,
    strict: bool,
) -> Result<Option<PathBuf>, String> {
    let out_path =
        out.map_or_else(|| default_pack_path(target_dir), Path::to_path_buf);
    let Some(helpers) = build_musl_scanners(root, target_dir, strict)? else {
        return Ok(None);
    };
    ruum_core::payload::write_pack(&out_path, &helpers).map_err(|err| {
        format!("failed to write {}: {err}", out_path.display())
    })?;
    Ok(Some(out_path))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let opts = match parse_args(args) {
        Ok(opts) => opts,
        Err(msg) if msg.is_empty() => return Ok(()),
        Err(msg) => return Err(msg),
    };
    let root = workspace_root();
    let target_dir = target_dir_for(&root);
    if let Some(path) =
        ensure_pack(&root, &target_dir, opts.out.as_deref(), opts.strict)?
    {
        println!("{}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_rust_lld_and_musl_target_helpers() {
        assert_eq!(rust_lld_filename(false), "rust-lld");
        assert_eq!(rust_lld_filename(true), "rust-lld.exe");
        assert_eq!(target_triple("x86_64"), "x86_64-unknown-linux-musl");
        assert_eq!(
            linker_env_key("x86_64-unknown-linux-musl"),
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER"
        );
        assert_eq!(
            linker_env_key("aarch64-unknown-linux-musl"),
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER"
        );

        let dir = tempfile::tempdir().unwrap();
        let sysroot = dir.path();
        assert_eq!(missing_musl_targets(sysroot).len(), 2);

        std::fs::create_dir_all(
            sysroot.join("lib/rustlib/x86_64-unknown-linux-musl/lib"),
        )
        .unwrap();
        assert_eq!(
            missing_musl_targets(sysroot),
            vec!["aarch64-unknown-linux-musl".to_owned()]
        );

        std::fs::create_dir_all(
            sysroot.join("lib/rustlib/aarch64-unknown-linux-musl/lib"),
        )
        .unwrap();
        assert!(missing_musl_targets(sysroot).is_empty());

        let parsed = parse_args(&[
            "--out".into(),
            "dist/ruum-scan-linux.pack".into(),
            "--strict".into(),
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Options {
                out: Some(PathBuf::from("dist/ruum-scan-linux.pack")),
                strict: true,
            }
        );
    }
}
