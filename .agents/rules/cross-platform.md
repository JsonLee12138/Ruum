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
