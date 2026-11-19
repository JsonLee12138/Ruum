//! `cargo xtask bundle`: the macOS app, disk image, and installer package.
//!
//! Plain Apple tools and nothing to install: `sips` and `iconutil` draw the
//! icon from its SVG, `codesign` signs, `ditto` zips the way Finder does
//! (keeping the signature's extended attributes), `hdiutil` builds `.dmg`,
//! `pkgbuild` and `productbuild` build `.pkg`, and `notarytool` and `stapler`
//! notarize. Everything lands in `target/bundle/`.
//!
//! Signing is ad hoc unless an identity is given. An ad-hoc app runs on the
//! Mac that built it; a downloaded one needs a Developer ID signature and
//! notarization, or Gatekeeper stops it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const APP: &str = "ruum.app";
const GUI_EXECUTABLE: &str = "ruum-gui";
const CLI_EXECUTABLE: &str = "ruum";
const BUNDLE_ID: &str = "io.github.tobi.ruum";
/// What the binary is built for at least; `LSMinimumSystemVersion` in
/// `packaging/macos/Info.plist.in` says the same, and GPUI needs 10.15.7.
const DEPLOYMENT_TARGET: &str = "11.0";
const HDIUTIL_BACKOFFS_MS: [u64; 5] = [500, 1000, 2000, 4000, 8000];

const DISTRIBUTION_TEMPLATE: &str =
    include_str!("../../packaging/macos/Distribution.xml.in");
const POSTINSTALL_SCRIPT: &str =
    include_str!("../../packaging/macos/scripts/postinstall");

/// The iconset `iconutil` expects: each point size at 1x and 2x.
const ICON_SIZES: [(&str, u32); 10] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
];

/// What `bundle` was asked to do.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// A `Developer ID Application: …` identity from the keychain; ad hoc
    /// when absent.
    identity: Option<String>,
    /// An optional `Developer ID Installer: …` identity for `.pkg`.
    installer_identity: Option<String>,
    scanner_pack: Option<PathBuf>,
    require_scanner_pack: bool,
    notarize: bool,
    zip: bool,
    dmg: bool,
    pkg: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut explicit_format = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sign" => {
                let identity = args.next().ok_or(
                    "--sign needs an identity, e.g. \
                     \"Developer ID Application: Name (TEAMID)\"",
                )?;
                // An empty identity is what CI passes when a secret is
                // missing; codesign would fail on it without saying why.
                if identity.trim().is_empty() {
                    return Err("--sign got an empty identity".into());
                }
                options.identity = Some(identity.clone());
            }
            "--installer-sign" => {
                let identity = args
                    .next()
                    .ok_or("--installer-sign needs an identity argument")?;
                // Empty secret falls back to an unsigned .pkg cleanly.
                if !identity.trim().is_empty() {
                    options.installer_identity = Some(identity.clone());
                }
            }
            "--scanner-pack" => {
                let path = args
                    .next()
                    .filter(|p| !p.trim().is_empty())
                    .ok_or("--scanner-pack needs a file path")?;
                options.scanner_pack = Some(PathBuf::from(path));
            }
            "--require-scanner-pack" => options.require_scanner_pack = true,
            "--notarize" => options.notarize = true,
            "--zip" => {
                options.zip = true;
                explicit_format = true;
            }
            "--dmg" => {
                options.dmg = true;
                explicit_format = true;
            }
            "--pkg" => {
                options.pkg = true;
                explicit_format = true;
            }
            "--all" => {
                options.zip = true;
                options.dmg = true;
                options.pkg = true;
                explicit_format = true;
            }
            other => return Err(format!("unknown bundle option {other}")),
        }
    }
    if !explicit_format {
        options.zip = true;
    }
    if options.notarize && options.identity.is_none() {
        return Err("--notarize needs --sign: Apple notarizes Developer ID \
             signatures only"
            .into());
    }
    Ok(options)
}

pub fn bundle(args: &[String]) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("the app bundle is built on macOS".into());
    }
    let options = parse(args)?;
    let root = workspace_root();
    let version = env!("CARGO_PKG_VERSION");
    let arch = std::env::consts::ARCH;
    let out = root.join("target/bundle");
    let app = out.join(APP);

    // The target directory is named rather than inherited: a
    // `CARGO_TARGET_DIR` or `build.target-dir` elsewhere would put the fresh
    // binary there and leave a stale one here to be bundled.
    let target = root.join("target");
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    tool(
        Command::new(cargo)
            .current_dir(&root)
            .args([
                "build",
                "--release",
                "--locked",
                "-p",
                "ruum-app",
                "-p",
                "ruum-core",
                "--bin",
                GUI_EXECUTABLE,
                "--bin",
                CLI_EXECUTABLE,
            ])
            .arg("--target-dir")
            .arg(&target)
            .env("MACOSX_DEPLOYMENT_TARGET", DEPLOYMENT_TARGET),
    )?;

    // Only ever the bundle this task made, never anything else in target/.
    if app.exists() {
        fs::remove_dir_all(&app)
            .map_err(|err| format!("cannot clear {}: {err}", app.display()))?;
    }
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    for dir in [&macos, &resources] {
        fs::create_dir_all(dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    }

    for exe in [GUI_EXECUTABLE, CLI_EXECUTABLE] {
        let binary = macos.join(exe);
        copy(&target.join("release").join(exe), &binary)?;
        // Local symbols only: the backtrace of a panic still names functions.
        tool(Command::new("strip").arg("-x").arg(&binary))?;
    }

    let plist = fs::read_to_string(root.join("packaging/macos/Info.plist.in"))
        .map_err(|err| format!("cannot read the Info.plist template: {err}"))?;
    write(
        &contents.join("Info.plist"),
        &plist.replace("@VERSION@", version),
    )?;
    tool(
        Command::new("plutil")
            .arg("-lint")
            .arg(contents.join("Info.plist")),
    )?;

    icon(
        &root.join("packaging/macos/AppIcon.svg"),
        &out,
        &resources.join("AppIcon.icns"),
    )?;

    if let Some(pack_src) = options.scanner_pack.as_deref() {
        if !pack_src.is_file() {
            return Err(format!(
                "scanner pack not found at {}",
                pack_src.display()
            ));
        }
        copy(
            pack_src,
            &resources.join(ruum_core::payload::PACK_FILE_NAME),
        )?;
    } else if let Some(pack_src) = crate::scanner_pack::ensure_pack(
        &root,
        &target,
        None,
        options.require_scanner_pack,
    )? {
        copy(
            &pack_src,
            &resources.join(ruum_core::payload::PACK_FILE_NAME),
        )?;
    }

    sign(&app, options.identity.as_deref())?;

    let mut produced = vec![app.clone()];
    let zip = out.join(format!("ruum-{version}-{arch}-macos.zip"));
    if options.zip {
        archive(&app, &zip)?;
        if options.notarize {
            notarize(&zip)?;
            // The ticket is stapled to the app, so zip it again: a zip itself
            // cannot carry one.
            tool(Command::new("xcrun").args(["stapler", "staple"]).arg(&app))?;
            archive(&app, &zip)?;
            tool(
                Command::new("spctl")
                    .args(["--assess", "--type", "execute", "-vv"])
                    .arg(&app),
            )?;
        }
        produced.push(zip);
    }

    if options.dmg {
        let dmg = out.join(format!("ruum-{version}-{arch}-macos.dmg"));
        build_dmg(&app, &dmg, &out, options.identity.as_deref())?;
        if options.notarize {
            notarize(&dmg)?;
            tool(Command::new("xcrun").args(["stapler", "staple"]).arg(&dmg))?;
        }
        produced.push(dmg);
    }

    if options.pkg {
        let pkg = out.join(format!("ruum-{version}-{arch}-macos.pkg"));
        build_pkg(
            &app,
            &pkg,
            &out,
            version,
            arch,
            options.installer_identity.as_deref(),
        )?;
        if should_notarize_pkg(&options) {
            notarize(&pkg)?;
            tool(Command::new("xcrun").args(["stapler", "staple"]).arg(&pkg))?;
        } else if options.notarize {
            eprintln!(
                "xtask bundle: skipping notarization for unsigned .pkg \
                 (pass --installer-sign to sign and notarize .pkg)"
            );
        }
        produced.push(pkg);
    }

    println!();
    for path in produced {
        println!("{}", path.display());
    }
    Ok(())
}

/// `AppIcon.icns` from the SVG, through an iconset of every size.
fn icon(svg: &Path, scratch: &Path, icns: &Path) -> Result<(), String> {
    let iconset = scratch.join("AppIcon.iconset");
    if iconset.exists() {
        fs::remove_dir_all(&iconset).map_err(|err| {
            format!("cannot clear {}: {err}", iconset.display())
        })?;
    }
    fs::create_dir_all(&iconset)
        .map_err(|err| format!("cannot create {}: {err}", iconset.display()))?;
    for (name, pixels) in ICON_SIZES {
        let pixels = pixels.to_string();
        tool(
            Command::new("sips")
                .args(["-s", "format", "png", "-z", &pixels, &pixels])
                .arg(svg)
                .arg("--out")
                .arg(iconset.join(name))
                // sips names every file it writes; the errors still show.
                .stdout(Stdio::null()),
        )?;
    }
    tool(
        Command::new("iconutil")
            .args(["-c", "icns"])
            .arg(&iconset)
            .arg("-o")
            .arg(icns),
    )
}

/// Sign the bundle, then check the signature holds.
///
/// A real identity gets the hardened runtime and a secure timestamp, both of
/// which notarization requires. No entitlements: the app needs none of what
/// the hardened runtime withholds, and the Trash is reached through
/// `NSFileManager`, which asks for no permission.
fn sign(app: &Path, identity: Option<&str>) -> Result<(), String> {
    let cli_helper = app.join("Contents/MacOS").join(CLI_EXECUTABLE);
    for target in [cli_helper.as_path(), app] {
        let mut command = Command::new("codesign");
        command.arg("--force");
        match identity {
            Some(identity) => {
                command
                    .args(["--timestamp", "--options", "runtime", "--sign"])
                    .arg(identity);
            }
            None => {
                command.args(["--sign", "-"]);
            }
        }
        tool(command.arg(target))?;
    }
    tool(
        Command::new("codesign")
            .args(["--verify", "--strict", "--verbose=2"])
            .arg(app),
    )
}

fn archive(app: &Path, zip: &Path) -> Result<(), String> {
    if zip.exists() {
        fs::remove_file(zip).map_err(|err| {
            format!("cannot replace {}: {err}", zip.display())
        })?;
    }
    tool(
        Command::new("ditto")
            .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
            .arg(app)
            .arg(zip),
    )
}

/// Retry a macOS disk-image operation when Spotlight or `diskimagesiod`
/// (`diskimages-helper`) briefly holds an exclusive lock (`EBUSY`/`EAGAIN`).
fn retry_with_backoff<F, S>(
    backoffs_ms: &[u64],
    mut sleep_fn: S,
    mut op: F,
) -> Result<(), String>
where
    F: FnMut() -> Result<(), String>,
    S: FnMut(Duration),
{
    let mut last_err = None;
    for attempt in 0..=backoffs_ms.len() {
        match op() {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err);
                if let Some(&delay) = backoffs_ms.get(attempt) {
                    sleep_fn(Duration::from_millis(delay));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "operation failed".into()))
}

fn build_dmg(
    app: &Path,
    dmg: &Path,
    scratch: &Path,
    identity: Option<&str>,
) -> Result<(), String> {
    let stage = scratch.join("dmg-stage");
    remove_dir_if_exists(&stage)?;
    fs::create_dir_all(&stage)
        .map_err(|err| format!("cannot create {}: {err}", stage.display()))?;

    tool(Command::new("ditto").arg(app).arg(stage.join(APP)))?;
    create_applications_symlink(&stage.join("Applications"))?;

    if dmg.exists() {
        fs::remove_file(dmg).map_err(|err| {
            format!("cannot replace {}: {err}", dmg.display())
        })?;
    }

    let create_outcome =
        retry_with_backoff(&HDIUTIL_BACKOFFS_MS, std::thread::sleep, || {
            let outcome = tool(
                Command::new("hdiutil")
                    .args(["create", "-volname", "ruum", "-srcfolder"])
                    .arg(&stage)
                    .args(["-ov", "-format", "UDZO"])
                    .arg(dmg),
            );
            if outcome.is_err() {
                let _ = fs::remove_file(dmg);
            }
            outcome
        });
    remove_dir_if_exists(&stage)?;
    create_outcome?;

    if let Some(id) = identity {
        retry_with_backoff(&HDIUTIL_BACKOFFS_MS, std::thread::sleep, || {
            tool(
                Command::new("codesign")
                    .args(["--force", "--timestamp", "--sign"])
                    .arg(id)
                    .arg(dmg),
            )
        })?;
    }
    retry_with_backoff(&HDIUTIL_BACKOFFS_MS, std::thread::sleep, || {
        tool(Command::new("hdiutil").arg("verify").arg(dmg))
    })
}

fn build_pkg(
    app: &Path,
    pkg: &Path,
    scratch: &Path,
    version: &str,
    arch: &str,
    installer_identity: Option<&str>,
) -> Result<(), String> {
    let pkg_root = scratch.join("pkg-root");
    let pkg_scripts = scratch.join("pkg-scripts");
    let component_plist = scratch.join("pkg-component.plist");
    let component_pkg = scratch.join("ruum-component.pkg");
    let dist_xml = scratch.join("Distribution.xml");

    remove_dir_if_exists(&pkg_root)?;
    remove_dir_if_exists(&pkg_scripts)?;
    for file in [&component_plist, &component_pkg, &dist_xml, pkg] {
        if file.exists() {
            fs::remove_file(file).map_err(|err| {
                format!("cannot remove {}: {err}", file.display())
            })?;
        }
    }

    fs::create_dir_all(&pkg_root).map_err(|err| {
        format!("cannot create {}: {err}", pkg_root.display())
    })?;
    fs::create_dir_all(&pkg_scripts).map_err(|err| {
        format!("cannot create {}: {err}", pkg_scripts.display())
    })?;

    tool(Command::new("ditto").arg(app).arg(pkg_root.join(APP)))?;
    let postinstall = pkg_scripts.join("postinstall");
    write(&postinstall, POSTINSTALL_SCRIPT)?;
    set_executable(&postinstall)?;

    write(&component_plist, component_plist_xml())?;
    tool(Command::new("plutil").arg("-lint").arg(&component_plist))?;

    tool(
        Command::new("pkgbuild")
            .arg("--root")
            .arg(&pkg_root)
            .arg("--component-plist")
            .arg(&component_plist)
            .args(["--identifier", &format!("{BUNDLE_ID}.pkg")])
            .args(["--version", version])
            .args(["--install-location", "/Applications"])
            .arg("--scripts")
            .arg(&pkg_scripts)
            .arg(&component_pkg),
    )?;

    write(&dist_xml, &render_distribution_xml(version, arch))?;
    let mut productbuild = Command::new("productbuild");
    productbuild
        .arg("--distribution")
        .arg(&dist_xml)
        .arg("--package-path")
        .arg(scratch);
    if let Some(id) = installer_identity {
        productbuild.args(["--sign", id]);
    }
    productbuild.arg(pkg);
    tool(&mut productbuild)?;

    remove_dir_if_exists(&pkg_root)?;
    remove_dir_if_exists(&pkg_scripts)?;
    for file in [&component_plist, &component_pkg, &dist_xml] {
        let _ = fs::remove_file(file);
    }
    Ok(())
}

const fn macos_host_arch(arch: &str) -> &'static str {
    if arch.eq_ignore_ascii_case("aarch64")
        || arch.eq_ignore_ascii_case("arm64")
    {
        "arm64"
    } else if arch.eq_ignore_ascii_case("x86_64") {
        "x86_64"
    } else {
        "arm64,x86_64"
    }
}

fn render_distribution_xml(version: &str, arch: &str) -> String {
    DISTRIBUTION_TEMPLATE
        .replace("@VERSION@", version)
        .replace("@HOST_ARCH@", macos_host_arch(arch))
}

const fn component_plist_xml() -> &'static str {
    concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
        "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n",
        "<array>\n",
        "\t<dict>\n",
        "\t\t<key>BundleHasStrictIdentifier</key>\n",
        "\t\t<true/>\n",
        "\t\t<key>BundleIsRelocatable</key>\n",
        "\t\t<false/>\n",
        "\t\t<key>BundleIsVersionChecked</key>\n",
        "\t\t<true/>\n",
        "\t\t<key>BundleOverwriteAction</key>\n",
        "\t\t<string>upgrade</string>\n",
        "\t\t<key>RootRelativeBundlePath</key>\n",
        "\t\t<string>ruum.app</string>\n",
        "\t</dict>\n",
        "</array>\n",
        "</plist>\n",
    )
}

const fn should_notarize_pkg(options: &Options) -> bool {
    options.notarize && options.pkg && options.installer_identity.is_some()
}

/// Submit `artifact` and wait for Apple's verdict.
///
/// Credentials come from the environment, never the command line, so they
/// stay out of shell history and CI logs: `NOTARY_PROFILE`, a keychain
/// profile saved with `xcrun notarytool store-credentials`, or an App Store
/// Connect API key as `NOTARY_KEY` (the `.p8` file), `NOTARY_KEY_ID` and
/// `NOTARY_ISSUER`.
fn notarize(artifact: &Path) -> Result<(), String> {
    let mut command = Command::new("xcrun");
    command
        .args(["notarytool", "submit"])
        .arg(artifact)
        .arg("--wait");
    if let Ok(profile) = std::env::var("NOTARY_PROFILE") {
        command.arg("--keychain-profile").arg(profile);
    } else {
        let var = |name: &str| {
            std::env::var(name).map_err(|_| {
                format!(
                    "--notarize needs NOTARY_PROFILE, or NOTARY_KEY, \
                     NOTARY_KEY_ID and NOTARY_ISSUER; {name} is not set"
                )
            })
        };
        command
            .arg("--key")
            .arg(var("NOTARY_KEY")?)
            .arg("--key-id")
            .arg(var("NOTARY_KEY_ID")?)
            .arg("--issuer")
            .arg(var("NOTARY_ISSUER")?);
    }
    tool(&mut command)
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn remove_dir_if_exists(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        fs::remove_dir_all(dir)
            .map_err(|err| format!("cannot clear {}: {err}", dir.display()))?;
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to).map(drop).map_err(|err| {
        format!("cannot copy {} to {}: {err}", from.display(), to.display())
    })
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents)
        .map_err(|err| format!("cannot write {}: {err}", path.display()))
}

#[cfg(unix)]
fn create_applications_symlink(link: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink("/Applications", link)
        .map_err(|err| format!("cannot link {}: {err}", link.display()))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps, reason = "matches unix signature")]
const fn create_applications_symlink(_link: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;

    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(path, perms)
        .map_err(|err| format!("cannot chmod 0755 {}: {err}", path.display()))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps, reason = "matches unix signature")]
const fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Run one tool, inheriting the terminal so its own report is seen.
fn tool(command: &mut Command) -> Result<(), String> {
    let display = format!("{command:?}");
    let status = command
        .stdin(Stdio::null())
        .status()
        .map_err(|err| format!("could not start {display}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{display} failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn signing_is_ad_hoc_unless_an_identity_is_given() {
        let options = parse(&[]).expect("no options");
        assert!(options.identity.is_none() && !options.notarize);
        assert!(options.zip && !options.dmg && !options.pkg);
        let options = parse(&args(&["--sign", "Developer ID Application: X"]))
            .expect("an identity");
        assert_eq!(
            options.identity.as_deref(),
            Some("Developer ID Application: X")
        );
    }

    #[test]
    fn notarizing_needs_a_real_signature() {
        let error = parse(&args(&["--notarize"])).expect_err("no identity");
        assert!(error.contains("--sign"), "{error}");
        assert!(
            parse(&args(&["--sign"])).is_err(),
            "an identity is required"
        );
        assert!(
            parse(&args(&["--sign", ""])).is_err(),
            "an empty identity is not one"
        );
    }

    #[test]
    fn parses_dmg_pkg_all_and_installer_sign_options() {
        let dmg_only = parse(&args(&["--dmg"])).expect("--dmg");
        assert!(!dmg_only.zip && dmg_only.dmg && !dmg_only.pkg);

        let pkg_only = parse(&args(&["--pkg"])).expect("--pkg");
        assert!(!pkg_only.zip && !pkg_only.dmg && pkg_only.pkg);

        let all = parse(&args(&[
            "--all",
            "--sign",
            "Developer ID Application: X",
            "--installer-sign",
            "",
            "--notarize",
        ]))
        .expect("--all with empty installer sign");
        assert!(all.zip && all.dmg && all.pkg);
        assert!(all.installer_identity.is_none());
        assert!(!should_notarize_pkg(&all));

        let signed_pkg = parse(&args(&[
            "--all",
            "--sign",
            "Developer ID Application: X",
            "--installer-sign",
            "Developer ID Installer: X",
            "--notarize",
        ]))
        .expect("--all with installer sign");
        assert_eq!(
            signed_pkg.installer_identity.as_deref(),
            Some("Developer ID Installer: X")
        );
        assert!(should_notarize_pkg(&signed_pkg));

        assert!(parse(&args(&["--installer-sign"])).is_err());
        assert!(parse(&args(&["--scanner-pack"])).is_err());

        let with_pack = parse(&args(&[
            "--scanner-pack",
            "target/scanner/ruum-scan-linux.pack",
            "--require-scanner-pack",
        ]))
        .expect("scanner pack flags");
        assert_eq!(
            with_pack.scanner_pack.as_deref(),
            Some(Path::new("target/scanner/ruum-scan-linux.pack"))
        );
        assert!(with_pack.require_scanner_pack);
    }

    #[test]
    fn the_plist_template_names_the_executable_and_takes_the_version() {
        let plist = include_str!("../../packaging/macos/Info.plist.in");
        assert!(plist.contains(&format!("<string>{GUI_EXECUTABLE}</string>")));
        assert!(plist.contains("@VERSION@"));
        assert!(
            plist.contains(&format!("<string>{DEPLOYMENT_TARGET}</string>"))
        );
        assert!(plist.contains(
            "<key>CFBundleIdentifier</key>\n\t\
             <string>io.github.tobi.ruum</string>"
        ));
        assert!(plist.contains(
            "<key>NSHumanReadableCopyright</key>\n\t\
             <string>Copyright (C) 2026 JsonLee12138. \
             Licensed under GPL-3.0-only.</string>"
        ));
    }

    #[test]
    fn renders_distribution_xml_and_non_relocatable_component_plist() {
        let xml = render_distribution_xml("0.1.1", "aarch64");
        assert!(xml.contains("enable_localSystem=\"true\""));
        assert!(xml.contains("hostArchitectures=\"arm64\""));
        assert!(xml.contains("version=\"0.1.1\""));
        assert!(!xml.contains('@'));

        let xml_intel = render_distribution_xml("0.1.1", "x86_64");
        assert!(xml_intel.contains("hostArchitectures=\"x86_64\""));

        let comp = component_plist_xml();
        assert!(comp.contains("<key>BundleIsRelocatable</key>\n\t\t<false/>"));
        assert!(comp.contains("<string>ruum.app</string>"));
        assert!(POSTINSTALL_SCRIPT.contains("/usr/local/bin/ruum"));
    }

    #[test]
    fn retries_transient_failures_with_configured_backoffs() {
        let mut sleeps: Vec<u64> = Vec::new();
        let mut attempts = 0_usize;
        let immediate = retry_with_backoff(
            &HDIUTIL_BACKOFFS_MS,
            |d| {
                sleeps.push(
                    u64::try_from(d.as_millis()).expect("backoff fits u64"),
                );
            },
            || {
                attempts += 1;
                Ok(())
            },
        );
        assert!(immediate.is_ok());
        assert_eq!(attempts, 1);
        assert!(sleeps.is_empty());

        sleeps.clear();
        attempts = 0;
        let third_ok = retry_with_backoff(
            &HDIUTIL_BACKOFFS_MS,
            |d| {
                sleeps.push(
                    u64::try_from(d.as_millis()).expect("backoff fits u64"),
                );
            },
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(format!("transient error #{attempts}"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(third_ok.is_ok());
        assert_eq!(attempts, 3);
        assert_eq!(sleeps, vec![500, 1000]);

        sleeps.clear();
        attempts = 0;
        let exhausted = retry_with_backoff(
            &HDIUTIL_BACKOFFS_MS,
            |d| {
                sleeps.push(
                    u64::try_from(d.as_millis()).expect("backoff fits u64"),
                );
            },
            || {
                attempts += 1;
                Err(format!("persistent error #{attempts}"))
            },
        );
        assert_eq!(
            exhausted.expect_err("exhausted retries must fail"),
            "persistent error #6"
        );
        assert_eq!(attempts, HDIUTIL_BACKOFFS_MS.len() + 1);
        assert_eq!(sleeps, HDIUTIL_BACKOFFS_MS);
    }
}
