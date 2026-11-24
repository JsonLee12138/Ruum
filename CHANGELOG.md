# Changelog

All notable changes to Ruum are documented in this file.

## [0.2.4-beta.3] - 2026-11-24

### Changes

- Prepare release `0.2.4-beta.3`.

## [0.2.4-beta.2] - 2026-11-24

### Changes

- Prepare release `0.2.4-beta.2`.

## [0.2.4-beta.1] - 2026-11-23

### Features

- **adk,core:** Add reusable `ruum-adk` direct AI API crate (`OpenAI` & `Anthropic`/Claude protocols with pure `rustls`, custom `base_url` normalization, `/models` discovery, multi-turn `LlmAgent` tool loop, and sync `BlockingAdkClient`) alongside `AiBackendFactory` / `DirectApiAiAdapter` and an in-process 6-tool read-only local MCP bridge (`list_volumes`, `scan_directory`, `inspect_subtree`, `find_reclaimable`, `check_git_safety`, `plan_removal`).
- **app,cli,core:** Add `API` provider support in **Settings › AI** (`OpenAI` / `Claude` protocol selector, `Base URL`, masked `API Key` stored lazily in `ruum.ai` keychain, draft connection test / model listing / latency benchmark across `en`/`zh`/`ja`/`ko`) and extend `ruum config ai get/set` with `--provider api`, `--api-protocol`, `--api-base-url`, `--api-key`, `--api-key-stdin`, and redacted output.

### Fixes

- **xtask:** Extend macOS `hdiutil` retry backoff up to 31s to prevent transient `.dmg` creation locks during releases.

## [0.2.3] - 2026-11-20

### Features

- **app,core,cli:** Add non-intrusive update UX (rail entry, once-per-version toast, Changelog modal with on-demand download, tray/menu integration, and **Settings › Updates** with `Stable` / `Beta` channels across `en`/`zh`/`ja`/`ko`), Ed25519 + SHA256 verified updater, tri-platform desktop installers, and `ruum update [check]` / `ruum config update`.
- **release,infra:** Add Cloudflare Worker + private R2 edge distribution (`https://ruum-updates.jsonlee12138.com`), `install.sh` / `install.ps1` installers, signed manifests, and Linux AppImage `gh-releases-zsync` (`.AppImage.zsync`) support.

### Fixes

- **core,remote,updater:** Fall back to cached `~/.cache/ruum/ssh/*/ruum-scan` helpers when local builds omit the embedded Linux scanner pack, sync `ruum-scan-linux.pack` on Windows CLI updates, treat empty release channels (`204 No Content` / `manifest not found`) as up to date, localize update errors, and normalize AppImage `LD_LIBRARY_PATH` cleanup with pure `/` POSIX semantics across all hosts.

### Tooling & Docs

- **xtask,ci,docs:** Add `cargo xtask cross-lint` (`cargo-xwin` Windows MSVC cross-clippy) and `cargo xtask release-tag` (enforcing clean tree, `--locked`, lint, optional cross-lint, and passing `CI` before tagging), and sync `README` (`en`/`zh`/`ja`/`ko`) and `infra/update-worker/README.md`.

## [0.2.3-beta.3] - 2026-11-20

### Fixes

- **core,updater:** Normalize AppImage `LD_LIBRARY_PATH` and `$APPDIR/usr/lib` matching with pure `/` POSIX semantics instead of host `Path::join`, fixing Windows unit test failures and ensuring cross-host separator consistency.

### Tooling

- **xtask,ci:** Add `cargo xtask cross-lint` (`cargo-xwin` Windows MSVC cross-clippy over the full workspace) and `cargo xtask release-tag` (enforcing clean tree, `--locked`, lint, optional cross-lint, and passing GitHub Actions `CI` before tagging/pushing).

## [0.2.3-beta.2] - 2026-11-20

### Fixes

- **core,remote:** Fall back to cached `~/.cache/ruum/ssh/*/ruum-scan` helpers when local builds omit the embedded Linux scanner pack, surface clearer missing-scanner diagnostics, and sync `ruum-scan-linux.pack` alongside Windows CLI updates.
- **core,worker,app:** Treat empty release channels (`204 No Content` / `manifest not found`) as up to date instead of an error, and translate update errors into localized user-friendly messages across `en`/`zh`/`ja`/`ko`.
- **app:** Simplify **Settings › Updates** copy by removing internal protocol/crypto jargon and the technical privacy note card.

### Packaging & Docs

- **release:** Embed `gh-releases-zsync` update metadata in Linux AppImages (`appimagetool -u`) and publish `.AppImage.zsync` alongside `.AppImage` while keeping built-in in-app updates intact (with clean relaunch env handling).
- **docs:** Sync `README` guides (`en`/`zh`/`ja`/`ko`) and `infra/update-worker/README.md` for tri-platform scanner packs, AppImage `zsync`, and empty-channel behavior.

## [0.2.3-beta.1] - 2026-11-20

### Features

- **app:** Add non-intrusive three-tier update UX (left icon-rail update entry, once-per-version floating toast, structured Changelog modal with on-demand download & background hide, system tray / macOS menu integration, and **Settings › Updates** with `Stable` / `Beta` channels across `en`/`zh`/`ja`/`ko`).
- **core,cli:** Add Ed25519 + SHA256 verified update engine with tri-platform desktop installers (`macOS` `.app`, `Windows` Inno/MSI, `Linux` AppImage/tarball), GitHub Releases fallback, SQLite `update_settings`, and `ruum update [check]` / `ruum config update`.
- **release,infra:** Add Cloudflare Worker + private R2 edge distribution (`https://ruum-updates.jsonlee12138.com`) with one-line `install.sh [--cli-only]` / `install.ps1` installers and signed `manifest.json` generation in `release.yml`.

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
