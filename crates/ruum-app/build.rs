#[cfg(windows)]
fn main() {
    use std::{env, fs, path::PathBuf, process::Command};

    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../assets/ruum.ico")
        .canonicalize()
        .expect("Windows icon asset is missing");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let rc = out.join("ruum.rc");
    let res = out.join("ruum.res");

    // rc.exe treats backslashes inside quoted paths as separators, not escapes.
    let icon_path = icon.to_string_lossy().replace('\\', "/");
    fs::write(&rc, format!("1 ICON \"{icon_path}\"\n"))
        .expect("could not write Windows icon resource script");

    // Cargo can run outside a Visual Studio prompt, where the SDK bin is not
    // on PATH even though the MSVC linker and Windows SDK are installed.
    let rc_exe = env::var_os("RUUM_RC").map_or_else(
        || {
            let sdk_bin = env::var_os("WindowsSdkDir")
                .map(PathBuf::from)
                .or_else(|| {
                    env::var_os("ProgramFiles(x86)")
                        .map(|root| PathBuf::from(root).join("Windows Kits/10"))
                })
                .expect("Windows SDK is required to embed the icon")
                .join("bin");
            let host = match env::consts::ARCH {
                "x86_64" => "x64",
                "aarch64" => "arm64",
                "x86" => "x86",
                other => panic!("unsupported Windows build host: {other}"),
            };
            let mut versions: Vec<_> = fs::read_dir(&sdk_bin)
                .expect("Windows SDK bin directory is missing")
                .filter_map(Result::ok)
                .map(|entry| entry.path().join(host).join("rc.exe"))
                .filter(|path| path.is_file())
                .collect();
            versions.sort();
            versions
                .pop()
                .expect("rc.exe is missing from the Windows SDK")
        },
        PathBuf::from,
    );
    let status = Command::new(rc_exe)
        .arg("/nologo")
        .arg(format!("/fo{}", res.display()))
        .arg(&rc)
        .status()
        .expect("could not start rc.exe to embed the Windows icon");
    assert!(
        status.success(),
        "rc.exe failed to compile the Windows icon"
    );

    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rustc-link-arg-bin=ruum-gui={}", res.display());

    // `link.exe /MANIFESTINPUT:` rejects `\\?\` verbatim prefixes, so strip
    // them from the canonicalized manifest path before emitting linker args.
    let manifest_canonical =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("../../packaging/windows/ruum.manifest")
            .canonicalize()
            .expect("Windows manifest asset is missing");
    let manifest_text = manifest_canonical.to_string_lossy();
    let manifest_plain = manifest_text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| manifest_text.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| manifest_text.into_owned());
    assert!(
        std::path::Path::new(&manifest_plain).is_file(),
        "Windows manifest asset is missing at {manifest_plain}"
    );
    println!("cargo:rerun-if-changed={manifest_plain}");
    println!("cargo:rustc-link-arg-bin=ruum-gui=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=ruum-gui=/MANIFESTINPUT:{manifest_plain}"
    );
}

#[cfg(not(windows))]
fn main() {}
