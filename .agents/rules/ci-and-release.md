---
trigger: always_on
---
# CI & Release Pipeline Rules (GitHub Actions & Packaging)

Hard-won guardrails from Ruum's tri-platform GitHub Actions (`CI` and `Release`) runs across `v0.1.0-beta.1` through `v0.2.0`.

## 1. Pre-Push & Pre-Tag Gate
- Ensure `Cargo.toml` workspace `version` matches the pushed release tag `vX.Y.Z` verbatim before creating or moving a tag.
- Verify `Cargo.lock` is in sync (`--locked`), run local gates (`cargo xtask ci` or `cargo xtask lint` + `cargo test --workspace`), and audit dependency features across targets (`cargo tree --offline --target all -e features`) so no unintended C `-sys` build dependency (e.g., `libdbus-sys`) slips into Linux CI/Release builds.
- Remember that `cargo xtask clippy` (`clippy --workspace --all-targets --all-features -- -D warnings`) aborts at the first failing compilation unit (`lib` before `tests/*.rs`); audit integration test `#[cfg(...)]` symmetry manually and via cross-target checks when fixing any lint.

## 2. macOS Packaging (`xtask/src/bundle.rs` & `.github/workflows/release.yml`)
- Guard `hdiutil create`, `.dmg` `codesign`, and `hdiutil verify` with exponential backoff retries (`HDIUTIL_BACKOFFS_MS` / `sleep` loop in `release.yml`) because Spotlight and `diskimagesiod` (`diskimages-helper`) on `macos-15` can hold transient exclusive locks (`Resource temporarily unavailable` / `EBUSY` / `EAGAIN`) for ~4–5s immediately after image creation.

## 3. Windows Packaging (`xtask/src/package.rs` & `.github/workflows/release.yml`)
- **WiX Toolset v5**: Pin `wix` (`dotnet tool install wix --version 5.0.2 --tool-path "$RUNNER_TEMP\wix-tool"`), set `DOTNET_ROLL_FORWARD: Major`, `DOTNET_CLI_TELEMETRY_OPTOUT: "1"`, and `DOTNET_NOLOGO: "1"`, verify `& $wixExe --version`, invoke via `$env:WIX_PATH`, and normalize SemVer strings into numeric `Major.Minor.Patch` (`<= 255.255.65535`) for MSI.
- **Inno Setup `>= 6.3`**: Never trust Chocolatey's shim binary (`C:\ProgramData\chocolatey\bin\ISCC.exe`), whose PE metadata reports `0.0.0.0`. Probe real installation paths, `ISCmplr.dll` PE versions, `Uninstall` registry keys, and `ISCC.exe /?` output, upgrading via `choco` or `winget` when below `6.3`.

## 4. Linux Packaging (`nfpm` & `AppImage`)
- Normalize SemVer prerelease `-` to `~` when feeding `nfpm` so RPM version ordering remains valid.
- Never `strip` `target/portable/ruum` or `target/portable/ruum-gui` a second time after `cargo xtask portable`: stripping destroys the appended `ruum-scan` static trailer (always verified afterward with `ruum extract-scanner`).
- When touching Linux AppImage bundling (`xtask/src/package.rs`), replace SVG `AppDir/.DirIcon` with a raster PNG (`appimagetool` `continuous` warns it will reject invalid `.DirIcon` in future releases) and avoid unpinned floating tools where deterministic builds matter.

## 5. GitHub Actions Runner Capacity & Fast Triage
- Keep workflow actions on Node 24+ majors (`actions/checkout@v5`, `actions/upload-artifact@v7`, `actions/download-artifact@v8`, `actions/attest@v4`) to avoid Node 20 deprecation warnings and upcoming breakage.
- Cancel superseded in-progress `CI` and `Release` runs across different refs manually (`gh run cancel <run_id>`) before pushing a fix or re-tagging (`release.yml` now auto-cancels same-tag runs via workflow-level `concurrency`, and `ci.yml` cancels stale PR runs / queues `main` runs while skipping version-only and doc pushes), preserving scarce `macos-15` hosted slots (15-minute acquisition timeout) and avoiding 30-minute `windows-2025` LTO backlog.
- While a slow matrix job (`windows-2025` `+crt-static` thin LTO ~27–30m; `macos-15` ~22–30m) is still running, fetch logs from already-completed sibling jobs immediately via `gh api --allow-escape-sequences repos/<owner>/<repo>/actions/jobs/<job_id>/logs` rather than `gh run view --log-failed` (which blocks until the entire run finishes).
