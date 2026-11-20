---
trigger: always_on
---
# Tri-Platform Generality & Factory + Adapter Rule (Windows / macOS / Linux)

All features, background services, CLI tools, IPC transports, notifications, filesystem paths, subprocess invocations, and UI workflows in Ruum MUST be designed and implemented for **Windows, macOS, and Linux** as first-class targets.

## 1. Tri-Platform Parity
- Never ship a feature that leaves one of Windows, macOS, or Linux as `Unsupported` or a stub without explicit user approval in `.artifacts/SPEC.md`.
- Every feature touching OS-level concepts (autostart/daemon lifecycle, trash/deletion, auth, notifications, IPC, path discovery, subprocess resolution) must cover all three platforms symmetrically.

## 2. Mandatory Factory + Adapter Pattern for OS-Specific Logic
- Whenever logic diverges by OS, **do not scatter `if cfg!(target_os = ...)` branches across domain/UI code**.
- Encapsulate platform differences in a dedicated `*_adapter` / `*_service` module mirroring `crates/ruum-core/src/auth.rs`, `crates/ruum-core/src/trash_adapter.rs`, and `crates/ruum-core/src/watchdog_service.rs`:
  1. `<Domain>Os` enum (`MacOs`, `Linux`, `Windows`).
  2. `<Domain>Env` struct capturing OS environment inputs (e.g., `home`, `xdg_config_home` / `xdg_data_home`, `appdata`, `uid`) with `::current()`.
  3. `pub trait <Domain>Adapter: Send + Sync` (plus `Debug` where useful) defining the OS-agnostic contract.
  4. Three concrete stateless structs: `MacOs<Domain>...`, `Linux<Domain>...`, `Windows<Domain>...`.
  5. `<Domain>Factory` (or `<Domain>AdapterFactory`) exposing `current_os() -> <Domain>Os`, `create_for(os: <Domain>Os) -> Box<dyn <Domain>Adapter>`, and `create() -> Box<dyn <Domain>Adapter>`.
- Keep all three adapters compiled unconditionally (avoid gating entire adapter structs behind `#[cfg(target_os = ...)]` except for strictly OS-only FFI syscalls) so cross-OS unit tests can exercise `create_for(Os::{MacOs, Linux, Windows})` on any developer or CI machine.

## 3. Platform Checklists
- **Paths & Home Resolution**: Resolve home via `std::env::home_dir()` (never assume `$HOME` is always set on Windows or inside macOS `.app` / `launchd` environments). Follow platform config roots: `~/Library/Application Support/ruum` (macOS), `%APPDATA%\ruum` with `home/AppData/Roaming/ruum` fallback (Windows), and `${XDG_CONFIG_HOME:-~/.config}/ruum` (Linux).
- **Windows Subprocesses from GUI/Background Binaries**: Always apply `CREATE_NO_WINDOW` (`0x0800_0000` via `std::os::windows::process::CommandExt::creation_flags` under `#[cfg(windows)]`) when spawning helper commands (`powershell.exe`, `reg.exe`, `tasklist.exe`, AI CLIs) so no transient console window flashes.
- **Executable Discovery**: On Windows, check `.exe`, `.cmd`, `.bat` extensions and include `%APPDATA%\npm` alongside standard binary dirs when resolving CLI tools.
- **Path Separators & Prefixes**: Handle both `/` and `\` and strip Windows `\\?\` extended prefixes where lexical path matching is performed cross-platform.
- **No Host `Path::join` for Target-OS Env Strings**: Never use host `std::path::Path::join` (which injects `std::path::MAIN_SEPARATOR`) to construct strings compared against target-OS environment variables or virtual paths (for example, building Linux `$APPDIR/usr/lib` to match `LD_LIBRARY_PATH` on a Windows host). Use an explicit target-OS string normalization helper (pure `/` for POSIX, `\` for Windows) and add unit tests with backslash, mixed-slash, and trailing-slash inputs so regressions fail on every host OS.

## 4. Cross-Platform Compile, Subprocess & Runtime Guardrails
- **`#[cfg]` Hygiene Under `-D warnings`**: Every `use` import, struct field, helper function, `let mut` binding, and `.clone()` used on only a subset of targets MUST carry matching `#[cfg(...)]` attributes across **both** library targets (`src/`) and integration tests (`tests/*.rs`). Avoid `let x = y.clone()` inside `#[cfg(not(unix))]` when `y` is not used again (`clippy::redundant_clone`). On macOS, run `cargo xtask cross-lint` (Windows MSVC cross-clippy via `cargo-xwin`) and whenever cross targets are installed locally run `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc` / `x86_64-unknown-linux-gnu` before pushing; before tagging a release, verify tri-platform CI green status via `cargo xtask release-tag`.
- **Pure-Rust System Clients Over C `-sys` Crates**: Prefer pure-Rust protocol implementations (`keyring` with `async-secret-service` + `async-io` + `crypto-rust` via `zbus`, `rustls`, `rusqlite` `bundled`) over C `pkg-config` wrappers (`libdbus-sys`, `openssl-sys`) so Linux builds require no extra `-dev` headers and portable binaries carry no extra host `.so` runtime dependencies. Audit dependency changes across all targets with `cargo tree --offline --target all -e features`.
- **Windows `.cmd`/`.bat` (`CVE-2024-24576` / `BatBadBut`) & Timeout Pipe Safety**: Rust `std::process::Command` rejects `\r` and `\n` in arguments passed to `.cmd`/`.bat` scripts (`ErrorKind::InvalidInput`). Unwrap npm `.cmd`/`.bat` shims (`%~dp0\*.js`) directly to `node.exe <script.js>` when passing multiline prompts, sanitize `\r`/`\n` (or pass payload via `stdin`) for generic batch scripts, and never `.join()` stdout/stderr reader threads unconditionally after `child.kill()` on timeout (grandchildren can hold pipe write ends open and deadlock).
- **Monotonic Deadline Loops (`park_timeout` Spurious Wakeups)**: Never use `std::thread::park_timeout` or `Condvar::wait_timeout` as a single-shot timer. Always loop until `Instant::now() >= deadline` using `std::thread::sleep(deadline.saturating_duration_since(now))`.
- **Headless E2E Sandbox Isolation**: Integration tests spawning `ruum` / `ruum-watchdog` MUST isolate `HOME`, `APPDATA`, `LOCALAPPDATA`, `XDG_CONFIG_HOME`, and `XDG_RUNTIME_DIR` (`< 96` bytes on Unix for `sun_path`), set `SSH_CONNECTION=1`, `RUUM_TEST_ROOT_SECRET_HEX`, and `RUUM_DISABLE_OS_NOTIFICATIONS=1` (preventing `powershell.exe` WinRT Toast cold-starts taking 3–5s on `windows-11-arm`), and size local mock HTTP/IPC server deadlines to `>= 30s`.
