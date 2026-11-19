//! Static Linux helpers and an appended, verifiable executable trailer.
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
    let root = crate::scanner_pack::workspace_root();
    let target = crate::scanner_pack::target_dir_for(&root);
    let helpers =
        crate::scanner_pack::build_musl_scanners(&root, &target, true)?
            .ok_or_else(|| "missing static Linux scanner targets".to_owned())?;
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
