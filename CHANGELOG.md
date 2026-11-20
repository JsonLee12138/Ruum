# Changelog

All notable changes to Ruum are documented in this file.

## [0.2.2] - 2026-11-20

### Fixes

- **core,watchdog:** Retry SQLite reads and writes on `SQLITE_BUSY` and promote `0.2.2-beta.1` fixes to stable.
- **core,remote:** Wrap SSH commands in single-line `/bin/sh -c` and unify `/bin/sh -s`.

## [0.2.2-beta.1] - 2026-11-19

### Fixes

- **core,watchdog:** Retry SQLite reads and writes on `SQLITE_BUSY`.
- **core,remote:** Wrap SSH commands in single-line `/bin/sh -c` and unify `/bin/sh -s`.

## [0.2.1] - 2026-11-19

### Features

- **app,core:** Add Docker usage views, client-side window chrome, and shared keychain cache.

### Fixes

- **app:** Wrap Explore top bar on narrow windows across all locales.
- **core,watchdog:** Harden Docker idle timeout and watchdog Windows file retries.
- **packaging,ci:** Bundle AppImage shared libraries, PNG `.DirIcon`, AppData metadata, and canonical naming.

### Improvements

- **readme:** Align localized AppImage filename examples across `en`/`zh`/`ja`/`ko` docs.

## [0.2.1-beta.1] - 2026-11-19

### Features

- **app,core:** Add Docker usage views, client-side window chrome, and shared keychain cache.

### Fixes

- **app:** Wrap Explore top bar on narrow windows across all locales.
- **core,watchdog:** Harden Docker idle timeout and watchdog Windows file retries.

### Improvements

- **rules:** Add CI/release and cross-platform engineering guardrails.

## [0.2.0] - 2026-11-18

### Features

- Add standalone `ruum` CLI, `ruum-watchdog` crate, SQLite storage, and multi-webhook notification support.

### Fixes

- **scan,core:** Guard scan deadline loops against spurious wakeups and resolve Windows clippy lints.
- **ci,bundle:** Fix Windows `cli_e2e` lints and retry transient macOS `hdiutil` locks.
- **core,release:** Use pure-Rust `secret-service` on Linux and fix Windows lint/E2E tests.

## [0.1.0] - 2026-11-17

### Features

- **packaging,ci:** Add tri-platform installers, Homebrew Cask template, and release workflow.
- **core,app:** Add tri-platform watchdog service, CLI installer, clickable native system notifications, and updated app icon/tray menu.

### Fixes

- **core,app:** Support safe tri-platform trash cleanup via `TrashAdapter` and resolve Linux/Windows clippy and timeout pipe hangs.
- **settings:** Isolate model list scroll from outer settings page.
- **release,xtask:** Pin WiX 5.0.2 with `DOTNET_ROLL_FORWARD`, harden Inno Setup >= 6.3 detection, and fix Linux `nfpm` staging.

### Improvements

- **docs:** Rewrite tutorial README and add localized `zh`/`ja`/`ko` guides.

## [0.1.1] - 2026-11-16

### Features

- **app,core:** Rename project to `ruum`, add headless `ruum mcp` stdio server, OS-native permanent deletion auth, watchdog daemon, navigation icon rail, settings pages, multi-language localization (`en`/`zh`/`ja`/`ko`), system tray menu, and 60% initial window sizing.
