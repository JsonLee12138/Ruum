//! Static Linux helpers and an appended, verifiable executable trailer.
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn build() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("portable packaging currently runs on Linux".into());
    }
    super::run(&[
        "build",
        "--release",
        "--locked",
        "-p",
        "ruum-app",
        "-p",
        "ruum-core",
        "--bin",
        "ruum-gui",
        "--bin",
        "ruum",
    ])?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| root.join("target"), PathBuf::from);
    let rustc = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("rustc")
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().into())
    };
    let sysroot = rustc(&["--print", "sysroot"])?;
    let version = rustc(&["-vV"])?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc did not report a host triple")?;
    let linker = Path::new(&sysroot)
        .join("lib/rustlib")
        .join(host)
        .join("bin/rust-lld");
    let mut helpers = Vec::new();
    for arch in ["x86_64", "aarch64"] {
        let triple = format!("{arch}-unknown-linux-musl");
        let key = format!(
            "CARGO_TARGET_{}_LINKER",
            triple.to_uppercase().replace('-', "_")
        );
        let status = Command::new("cargo")
            .args([
                "build",
                "--release",
                "--locked",
                "-p",
                "ruum-scan",
                "--target",
                &triple,
            ])
            .env(key, &linker)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("static helper build failed; install targets with: \
                 rustup target add x86_64-unknown-linux-musl \
                 aarch64-unknown-linux-musl"
                .into());
        }
        helpers
            .push((arch.into(), target.join(triple).join("release/ruum-scan")));
    }
    let portable_dir = target.join("portable");
    std::fs::create_dir_all(&portable_dir).map_err(|e| e.to_string())?;
    let output_cli = portable_dir.join("ruum");
    let output_gui = portable_dir.join("ruum-gui");
    std::fs::copy(target.join("release/ruum"), &output_cli)
        .map_err(|e| e.to_string())?;
    std::fs::copy(target.join("release/ruum-gui"), &output_gui)
        .map_err(|e| e.to_string())?;
    // strip rewrites ELF and discards overlays, so it must precede embedding.
    for bin in [&output_cli, &output_gui] {
        let status = Command::new("strip")
            .arg(bin)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("strip failed for {}", bin.display()));
        }
    }
    ruum_core::payload::append(&output_cli, &helpers)
        .map_err(|e| e.to_string())?;
    println!(
        "Portable binaries: {} and {}",
        output_cli.display(),
        output_gui.display()
    );
    Ok(())
}
