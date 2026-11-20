//! Unified headless `ruum` CLI binary (zero GPUI dependency).
//!
//! When invoked with zero arguments, probes graphical desktop availability and
//! a co-located `ruum-gui` binary via [`DesktopLaunchFactory`], launching the
//! GUI detached if reachable or falling back to CLI help.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use ruum_core::ai_cli::{DefaultCommandRunner, run_analysis_for_config};
use ruum_core::credentials::{
    LazyCredentialStore, SshCredentialSecret,
    delete_ssh_profile_with_credentials,
};
use ruum_core::desktop_launch::{DesktopLaunchEnv, DesktopLaunchFactory};
use ruum_core::notify::notify_gui_watchdog_update;
use ruum_core::payload::Archive;
use ruum_core::remote::{
    Operation, RemoteScan, Request, SessionRequest, read_request,
    serve_session, validate_host,
};
use ruum_core::scan::{ScanOptions, scan};
use ruum_core::size::human_bytes;
use ruum_core::space::{space_info, volume_root_for};
use ruum_core::ssh_session::{
    ENV_ASKPASS_CMD, SshAuthMaterial, SshEndpoint, SshSession,
    run_askpass_cli_from_env,
};
use ruum_core::storage::{
    AiState, AuthMode, DEFAULT_SSH_PORT, DEFAULT_SSH_SESSION_DURATION_MINUTES,
    SshProfile, StorageDb, UpdateChannel, WebhookEndpoint,
};
use ruum_core::tree::{Metric, Node};
use ruum_core::updater::{
    DEFAULT_GITHUB_REPO, UpdateArch, UpdateCheckRequest, UpdateEnv,
    UpdateFactory, check_for_updates, resolve_effective_public_key_for_url,
    resolve_update_base_url,
};
use ruum_core::watchdog::{
    ServiceStatus, install_service, run_default_loop_with_db,
    run_default_once_with_db, service_status, uninstall_service,
};
use ruum_core::watchdog_config::AiCliProvider;
use serde_json::json;

const HELP_TEXT: &str = "\
ruum — headless disk usage explorer, configuration CLI, and MCP server

usage: ruum                       launch ruum-gui if desktop is available, else print help
       ruum help                  show this help
       ruum scan [PATH] [OPTIONS] scan a local directory (or stream --stdio-v1)
       ruum space [PATH] [--json] check O(1) volume space and directory readability
       ruum ssh <TARGET> [PATH] [--check] [OPTIONS]
                                  scan a saved SSH profile (alias/id) or host
       ruum config <SUBCOMMAND>   manage ruum.db settings, SSH cards, watchdog, webhook, AI
       ruum watchdog [run | once | install | uninstall | status]
                                  run or manage the disk-space watchdog service
       ruum analyze [PATH]        run AI cleanup analysis using ai_config
       ruum run-script <SCRIPT> [--profile ID_OR_ALIAS] [--yes] [--pause]
                                  review and execute a generated cleanup script
       ruum mcp [--read-only]     run Model Context Protocol (MCP) server over stdio
       ruum update [check] [--channel stable|beta] [--json]
                                  check for updates and show upgrade instructions
       ruum extract-scanner ARCH OUTPUT
                                  extract embedded static Linux scanner (x86_64 | aarch64)

Run `ruum <subcommand> --help` for subcommand options.";

const SCAN_HELP: &str = "\
ruum scan — scan a local directory or serve --stdio-v1 wire protocol

usage: ruum scan [PATH] [OPTIONS]

options:
  -D, --disk               scan the volume containing ~
  -a, --apparent-size      measure apparent length instead of allocated blocks
  -l, --follow-links       follow symlinks
  -H, --no-hidden          skip dotfiles and dot-directories
  -x, --one-filesystem     stay on the root filesystem (default)
  -X, --cross-filesystems  include nested mounts
  -d, --depth N            display depth (1-6, default 3)
      --metric bytes|files rank entries by bytes (default) or file count
      --stdio-v1           speak remote scan stdio-v1 JSON protocol
      --json               emit JSON summary
  -h, --help               show this help";

const SSH_HELP: &str = "\
ruum ssh — inspect a saved SSH profile (alias or id) or raw SSH destination

usage: ruum ssh <ALIAS_OR_ID_OR_HOST> [PATH] [OPTIONS]

options:
      --check              print a single-line check summary
  -D, --disk               scan the remote root volume
  -a, --apparent-size      measure apparent length instead of allocated blocks
  -H, --no-hidden          skip dotfiles and dot-directories
      --metric bytes|files rank entries by bytes or file count
  -h, --help               show this help";

const CONFIG_HELP: &str = "\
ruum config — manage local SQLite configuration (ruum.db)

usage:
  ruum config app get [--json]
  ruum config app set [--power-preset P] [--default-depth 1..6]
                      [--apparent-size BOOL] [--include-hidden BOOL]
                      [--follow-system-appearance BOOL]
                      [--ssh-session-duration-minutes N] [--json]
  ruum config ssh list [--json]
  ruum config ssh add --alias A --host H [--port P] --username U
                      [--auth-mode password|private_key] [--default-path PATH]
                      [--password PW | --password-stdin]
                      [--private-key-file FILE | --private-key-stdin]
                      [--passphrase PASS] [--json]
  ruum config ssh edit <ALIAS_OR_ID> [--alias A] [--host H] [--port P]
                      [--username U] [--auth-mode password|private_key]
                      [--default-path PATH | --clear-default-path]
                      [--password PW | --password-stdin]
                      [--private-key-file FILE | --private-key-stdin]
                      [--passphrase PASS] [--json]
  ruum config ssh remove <ALIAS_OR_ID> [--json]
  ruum config watchdog get [--json]
  ruum config watchdog set [--enabled BOOL] [--background-service BOOL]
                           [--min-free-gb N] [--poll-interval-secs N]
                           [--cooldown-secs N] [--json]
  ruum config webhook list [--json]
  ruum config webhook add --name N --url U [--enabled BOOL]
                          [--headers-json JSON] [--timeout-secs N] [--json]
  ruum config webhook edit <ID_OR_NAME> [--name N] [--url U] [--enabled BOOL]
                           [--headers-json JSON] [--timeout-secs N] [--json]
  ruum config webhook remove <ID_OR_NAME> [--json]
  ruum config webhook get [--json]
  ruum config webhook set [--enabled BOOL] [--url URL]
                          [--headers-json JSON] [--timeout-secs N] [--json]
  ruum config ai get [--json]
  ruum config ai set [--auto-analyze BOOL] [--provider agy|pi|codex|custom]
                     [--custom-command CMD] [--extra-args ARGS]
                     [--model M | --clear-model] [--json]
  ruum config update get [--json]
  ruum config update set [--auto-check BOOL] [--channel stable|beta]
                         [--send-client-id BOOL] [--clear-skipped] [--json]";

const UPDATE_HELP: &str = "\
ruum update — check for ruum updates and print upgrade instructions

usage: ruum update [check] [OPTIONS]

options:
      --channel stable|beta  override configured release channel
      --json                 emit JSON output
  -h, --help                 show this help";

const WATCHDOG_HELP: &str = "\
ruum watchdog — disk space watchdog service and single-tick runner

usage: ruum watchdog [run | once | --once | install | uninstall | status]";

const MCP_HELP: &str = "\
ruum mcp — Model Context Protocol (MCP) server over stdio

usage: ruum mcp [--read-only]";

const ANALYZE_HELP: &str = "\
ruum analyze — trigger AI cleanup analysis using ai_config

usage: ruum analyze [PATH] [--json]";

const SPACE_HELP: &str = "\
ruum space — inspect volume free space and directory readability in O(1)

usage: ruum space [PATH] [--json]

options:
      --json               emit JSON output
  -h, --help               show this help";

const RUN_SCRIPT_HELP: &str = "\
ruum run-script — review and execute a deterministic cleanup script

usage: ruum run-script <SCRIPT_PATH> [--profile <ID_OR_ALIAS>] [--yes] [--pause]

options:
      --profile <ID_OR_ALIAS>  execute the embedded remote payload via saved SSH profile
  -y, --yes                    skip interactive [y/N] confirmation prompt
      --pause                  wait for Enter before exiting
  -h, --help                   show this help";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ruum: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    // OpenSSH invokes `$SSH_ASKPASS "<prompt>"` with the prompt in argv[1]
    // while passing `RUUM_ASKPASS_CMD=ssh-askpass` in the child environment.
    if std::env::var_os(ENV_ASKPASS_CMD).is_some_and(|v| v == "ssh-askpass") {
        return cmd_ssh_askpass(&[]);
    }

    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        let env = DesktopLaunchEnv::current();
        if DesktopLaunchFactory::create()
            .launch_if_available(&env)
            .context("launch ruum-gui")?
            .is_some()
        {
            return Ok(());
        }
        println!("{HELP_TEXT}");
        return Ok(());
    }

    let first = args[0].to_string_lossy();
    match first.as_ref() {
        "help" | "-h" | "--help" => {
            println!("{HELP_TEXT}");
            Ok(())
        }
        "scan" => cmd_scan(&args[1..]),
        "space" => cmd_space(&args[1..]),
        "ssh" => cmd_ssh(&args[1..]),
        "config" => cmd_config(&args[1..]),
        "watchdog" => cmd_watchdog(&args[1..]),
        "analyze" => cmd_analyze(&args[1..]),
        "run-script" => cmd_run_script(&args[1..]),
        "mcp" => cmd_mcp(&args[1..]),
        "update" => cmd_update(&args[1..]),
        "extract-scanner" => cmd_extract_scanner(&args[1..]),
        "ssh-askpass" => cmd_ssh_askpass(&args[1..]),
        other if other.starts_with('-') => {
            bail!("unknown option `{other}`; try `ruum --help`")
        }
        other => bail!("unknown command `{other}`; try `ruum --help`"),
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn os_to_string(arg: &OsString, label: &str) -> Result<String> {
    arg.clone()
        .into_string()
        .map_err(|_| anyhow::anyhow!("{label} must be valid UTF-8"))
}

fn next_str(
    iter: &mut std::slice::Iter<'_, OsString>,
    flag: &str,
) -> Result<String> {
    let val = iter
        .next()
        .with_context(|| format!("{flag} requires a value"))?;
    os_to_string(val, flag)
}

fn parse_bool(raw: &str, flag: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => bail!("{flag} expects a boolean (true/false), got `{raw}`"),
    }
}

fn parse_u64(raw: &str, flag: &str) -> Result<u64> {
    raw.trim()
        .parse::<u64>()
        .with_context(|| format!("{flag} expects an unsigned integer"))
}

fn parse_u32(raw: &str, flag: &str) -> Result<u32> {
    raw.trim()
        .parse::<u32>()
        .with_context(|| format!("{flag} expects an unsigned integer"))
}

fn parse_u16(raw: &str, flag: &str) -> Result<u16> {
    raw.trim()
        .parse::<u16>()
        .with_context(|| format!("{flag} expects a TCP port (1-65535)"))
}

fn resolve_local_root(path_arg: Option<&Path>, disk: bool) -> Result<PathBuf> {
    if disk && path_arg.is_some() {
        bail!("--disk and a PATH cannot be combined");
    }
    let home = std::env::home_dir();
    let candidate = if disk {
        home.as_deref()
            .and_then(volume_root_for)
            .unwrap_or_else(|| PathBuf::from("/"))
    } else if let Some(path) = path_arg {
        path.to_path_buf()
    } else {
        home.context("no path given and no home directory")?
    };

    let root = dunce::canonicalize(&candidate).unwrap_or(candidate);
    let meta = fs::metadata(&root)
        .with_context(|| format!("cannot read {}", root.display()))?;
    anyhow::ensure!(meta.is_dir(), "{} is not a directory", root.display());
    Ok(root)
}

fn print_node_children(node: &Node, depth_remaining: u32, indent: usize) {
    if depth_remaining == 0 {
        return;
    }
    for child in node.children.iter().take(15) {
        let pad = " ".repeat(indent);
        println!(
            "{pad}{}  {}  ({} files)",
            child.name,
            human_bytes(child.bytes),
            child.files
        );
        if depth_remaining > 1 && !child.children.is_empty() {
            print_node_children(child, depth_remaining - 1, indent + 2);
        }
    }
}

fn cmd_scan(args: &[OsString]) -> Result<()> {
    let mut root: Option<PathBuf> = None;
    let mut disk = false;
    let mut options = ScanOptions::default();
    let mut depth = 3_u32;
    let mut stdio_v1 = false;
    let mut json_out = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{SCAN_HELP}");
                return Ok(());
            }
            Some("-D" | "--disk") => disk = true,
            Some("-a" | "--apparent-size") => options.apparent_size = true,
            Some("-l" | "--follow-links") => options.follow_links = true,
            Some("-H" | "--no-hidden") => options.include_hidden = false,
            Some("-x" | "--one-filesystem") => options.one_filesystem = true,
            Some("-X" | "--cross-filesystems") => {
                options.one_filesystem = false;
            }
            Some("-d" | "--depth") => {
                let raw = next_str(&mut iter, "--depth")?;
                depth = parse_u32(&raw, "--depth")?;
                anyhow::ensure!(
                    (1..=6).contains(&depth),
                    "--depth must be 1 to 6"
                );
            }
            Some("--metric") => {
                let raw = next_str(&mut iter, "--metric")?;
                options.metric = match raw.as_str() {
                    "bytes" | "size" => Metric::Bytes,
                    "files" => Metric::Files,
                    other => {
                        bail!("unknown metric `{other}`; try bytes or files")
                    }
                };
            }
            Some("--stdio-v1") => stdio_v1 = true,
            Some("--json") => json_out = true,
            Some(other) if other.starts_with('-') => {
                bail!("unknown option `{other}` for `ruum scan`");
            }
            _ => {
                anyhow::ensure!(root.is_none(), "only one path can be scanned");
                root = Some(PathBuf::from(arg));
            }
        }
    }

    if stdio_v1 {
        if root.is_none() && !disk {
            let mut input = BufReader::new(io::stdin());
            let session = read_request(&mut input)?;
            let mut out = BufWriter::new(io::stdout().lock());
            serve_session(session, &mut out)?;
            out.flush()?;
            return Ok(());
        }
        let target_root = resolve_local_root(root.as_deref(), disk)?;
        let session = SessionRequest {
            operation: Operation::Scan(Request {
                root: Some(target_root),
                disk: false,
                options,
            }),
            watch_disconnect: false,
            live_progress: false,
            client_dedup: false,
            live_tree: false,
        };
        let mut out = BufWriter::new(io::stdout().lock());
        serve_session(session, &mut out)?;
        out.flush()?;
        return Ok(());
    }

    let target_root = resolve_local_root(root.as_deref(), disk)?;
    let tree = scan(&target_root, options)?;

    if json_out {
        let entries: Vec<_> = tree
            .children
            .iter()
            .take(25)
            .map(|c| {
                json!({
                    "name": &*c.name,
                    "bytes": c.bytes,
                    "files": c.files,
                    "dirs": c.dirs,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "root": target_root.display().to_string(),
                "bytes": tree.bytes,
                "human": human_bytes(tree.bytes),
                "files": tree.files,
                "dirs": tree.dirs,
                "entries": entries,
            }))?
        );
    } else {
        println!(
            "{} — {} bytes ({}), {} files, {} directories",
            target_root.display(),
            tree.bytes,
            human_bytes(tree.bytes),
            tree.files,
            tree.dirs
        );
        print_node_children(&tree, depth, 2);
    }
    Ok(())
}

fn expand_local_tilde(path_arg: Option<&Path>) -> Result<PathBuf> {
    let home = std::env::home_dir();
    let Some(path) = path_arg else {
        return home.context("no path given and no home directory");
    };
    let raw = path.to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "~" {
        return home.context("no home directory");
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let base = home.context("no home directory")?;
        return Ok(if rest.is_empty() {
            base
        } else {
            base.join(rest)
        });
    }
    #[cfg(windows)]
    if let Some(rest) = trimmed.strip_prefix("~\\") {
        let base = home.context("no home directory")?;
        return Ok(if rest.is_empty() {
            base
        } else {
            base.join(rest)
        });
    }
    Ok(path.to_path_buf())
}

fn cmd_space(args: &[OsString]) -> Result<()> {
    let mut path_arg: Option<PathBuf> = None;
    let mut json_out = false;
    for arg in args {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{SPACE_HELP}");
                return Ok(());
            }
            Some("--json") => json_out = true,
            Some(other) if other.starts_with('-') => {
                bail!("unknown option `{other}` for `ruum space`");
            }
            _ => {
                anyhow::ensure!(
                    path_arg.is_none(),
                    "ruum space accepts at most one PATH"
                );
                path_arg = Some(PathBuf::from(arg));
            }
        }
    }

    let expanded = expand_local_tilde(path_arg.as_deref())?;
    let root = dunce::canonicalize(&expanded).unwrap_or(expanded);
    let meta = fs::metadata(&root)
        .with_context(|| format!("cannot access {}", root.display()))?;
    anyhow::ensure!(meta.is_dir(), "{} is not a directory", root.display());

    let readable = fs::read_dir(&root).is_ok();
    let info = space_info(&root).with_context(|| {
        format!("cannot inspect space for {}", root.display())
    })?;
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "root": root.display().to_string(),
                "available": info.available,
                "free": info.free,
                "total": info.total,
                "readable": readable,
                "platform": std::env::consts::OS,
            }))?
        );
    } else {
        println!(
            "{} — {} available / {} total ({} free) [{}]",
            root.display(),
            human_bytes(info.available),
            human_bytes(info.total),
            human_bytes(info.free),
            std::env::consts::OS,
        );
    }
    Ok(())
}

fn normalize_ssh_path_opt(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("~")
        .to_owned()
}

fn cmd_ssh(args: &[OsString]) -> Result<()> {
    let Some(first) = args.first() else {
        bail!("ssh needs an alias, profile id, or host; try `ruum ssh --help`");
    };
    if first == "-h" || first == "--help" {
        println!("{SSH_HELP}");
        return Ok(());
    }

    let target = os_to_string(first, "SSH target")?;
    let mut path_arg: Option<PathBuf> = None;
    let mut disk = false;
    let mut check = false;
    let mut options = ScanOptions::default();

    let mut iter = args[1..].iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{SSH_HELP}");
                return Ok(());
            }
            Some("--check") => check = true,
            Some("-D" | "--disk") => disk = true,
            Some("-a" | "--apparent-size") => options.apparent_size = true,
            Some("-H" | "--no-hidden") => options.include_hidden = false,
            Some("--metric") => {
                let raw = next_str(&mut iter, "--metric")?;
                options.metric = match raw.as_str() {
                    "bytes" | "size" => Metric::Bytes,
                    "files" => Metric::Files,
                    other => {
                        bail!("unknown metric `{other}`; try bytes or files")
                    }
                };
            }
            Some(other) if other.starts_with('-') => {
                bail!("unknown option `{other}` for `ruum ssh`");
            }
            _ => {
                anyhow::ensure!(
                    path_arg.is_none(),
                    "only one remote path can be specified"
                );
                path_arg = Some(PathBuf::from(arg));
            }
        }
    }

    anyhow::ensure!(
        !(disk && path_arg.is_some()),
        "--disk and a PATH cannot be combined"
    );

    let db = StorageDb::open_default().ok();
    let profile = db
        .as_ref()
        .and_then(|d| d.get_ssh_profile(&target).ok().flatten());

    let (display_target, snapshot) = if let Some(prof) = profile {
        let endpoint =
            SshEndpoint::from_profile(&prof).map_err(anyhow::Error::msg)?;
        let duration = db
            .as_ref()
            .and_then(|d| d.load_app_settings().ok())
            .map_or(DEFAULT_SSH_SESSION_DURATION_MINUTES, |s| {
                s.ssh_session_duration_minutes
            });
        let creds =
            LazyCredentialStore::open_default().map_err(anyhow::Error::msg)?;
        let material = SshAuthMaterial::from_profile_and_store(&prof, &creds)
            .map_err(anyhow::Error::msg)?;
        let session = SshSession::start(endpoint, material, duration)
            .map_err(anyhow::Error::msg)?;
        let effective_root = if disk {
            None
        } else {
            let resolved = path_arg
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned());
            let raw = resolved
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map_or_else(
                    || normalize_ssh_path_opt(prof.default_path.as_deref()),
                    ToOwned::to_owned,
                );
            Some(PathBuf::from(raw))
        };
        let snap = RemoteScan::spawn_with_session(
            &session,
            &Request {
                root: effective_root,
                disk,
                options,
            },
        )?
        .wait()?;
        if let (Some(d), Some(space)) = (&db, snap.header.space) {
            let _ = d.update_ssh_profile_scan_summary(
                &prof.id,
                space.total,
                space.available,
                now_unix_secs(),
            );
        }
        (prof.alias, snap)
    } else {
        validate_host(&target)?;
        let effective_root = if disk {
            None
        } else {
            let resolved = path_arg
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned());
            Some(PathBuf::from(normalize_ssh_path_opt(resolved.as_deref())))
        };
        let snap = RemoteScan::spawn(
            &target,
            &Request {
                root: effective_root,
                disk,
                options,
            },
        )?
        .wait()?;
        (target, snap)
    };

    println!(
        "{display_target}:{} — {} bytes ({}), {} files, {} directories",
        snapshot.header.root.display(),
        snapshot.tree.bytes,
        human_bytes(snapshot.tree.bytes),
        snapshot.tree.files,
        snapshot.tree.dirs
    );
    if !check {
        print_node_children(&snapshot.tree, 2, 2);
    }
    Ok(())
}

fn cmd_config(args: &[OsString]) -> Result<()> {
    let Some(first) = args.first() else {
        println!("{CONFIG_HELP}");
        return Ok(());
    };
    let section = first.to_string_lossy();
    match section.as_ref() {
        "-h" | "--help" | "help" => {
            println!("{CONFIG_HELP}");
            Ok(())
        }
        "app" => cmd_config_app(&args[1..]),
        "ssh" => cmd_config_ssh(&args[1..]),
        "watchdog" => cmd_config_watchdog(&args[1..]),
        "webhook" => cmd_config_webhook(&args[1..]),
        "ai" => cmd_config_ai(&args[1..]),
        "update" => cmd_config_update(&args[1..]),
        other => {
            bail!("unknown config section `{other}`; try `ruum config --help`")
        }
    }
}

fn check_json_only(args: &[OsString], context: &str) -> Result<bool> {
    let mut json_out = false;
    for arg in args {
        match arg.to_str() {
            Some("--json") => json_out = true,
            Some("-h" | "--help") => {
                println!("{CONFIG_HELP}");
                std::process::exit(0);
            }
            Some(other) => bail!("unknown option `{other}` for `{context}`"),
            None => bail!("invalid UTF-8 option for `{context}`"),
        }
    }
    Ok(json_out)
}

fn cmd_config_app(args: &[OsString]) -> Result<()> {
    let Some(action) = args.first().map(|a| a.to_string_lossy()) else {
        bail!("usage: ruum config app <get|set>");
    };
    if matches!(action.as_ref(), "-h" | "--help" | "help") {
        println!("{CONFIG_HELP}");
        return Ok(());
    }
    let rest = &args[1..];
    let db = StorageDb::open_default()?;
    match action.as_ref() {
        "get" => {
            let json_out = check_json_only(rest, "ruum config app get")?;
            let settings = db.load_app_settings()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&settings)?);
            } else {
                println!("power_preset = {}", settings.power_preset);
                println!("default_depth = {}", settings.default_depth);
                println!("apparent_size = {}", settings.apparent_size);
                println!("include_hidden = {}", settings.include_hidden);
                println!(
                    "follow_system_appearance = {}",
                    settings.follow_system_appearance
                );
                println!(
                    "ssh_session_duration_minutes = {}",
                    settings.ssh_session_duration_minutes
                );
            }
            Ok(())
        }
        "set" => {
            let mut settings = db.load_app_settings()?;
            let mut json_out = false;
            let mut iter = rest.iter();
            while let Some(arg) = iter.next() {
                match arg.to_str() {
                    Some("--power-preset") => {
                        let raw = next_str(&mut iter, "--power-preset")?;
                        let lower = raw.trim().to_ascii_lowercase();
                        anyhow::ensure!(
                            matches!(
                                lower.as_str(),
                                "miser"
                                    | "balanced"
                                    | "aggressive"
                                    | "drain-my-battery"
                            ),
                            "unknown power preset `{raw}`"
                        );
                        settings.power_preset = lower;
                    }
                    Some("--default-depth") => {
                        let raw = next_str(&mut iter, "--default-depth")?;
                        let d = parse_u32(&raw, "--default-depth")?;
                        anyhow::ensure!(
                            (1..=6).contains(&d),
                            "--default-depth must be 1 to 6"
                        );
                        settings.default_depth = d;
                    }
                    Some("--apparent-size") => {
                        let raw = next_str(&mut iter, "--apparent-size")?;
                        settings.apparent_size =
                            parse_bool(&raw, "--apparent-size")?;
                    }
                    Some("--include-hidden") => {
                        let raw = next_str(&mut iter, "--include-hidden")?;
                        settings.include_hidden =
                            parse_bool(&raw, "--include-hidden")?;
                    }
                    Some("--follow-system-appearance") => {
                        let raw =
                            next_str(&mut iter, "--follow-system-appearance")?;
                        settings.follow_system_appearance =
                            parse_bool(&raw, "--follow-system-appearance")?;
                    }
                    Some("--ssh-session-duration-minutes") => {
                        let raw = next_str(
                            &mut iter,
                            "--ssh-session-duration-minutes",
                        )?;
                        let mins =
                            parse_u64(&raw, "--ssh-session-duration-minutes")?;
                        anyhow::ensure!(
                            mins >= 1,
                            "--ssh-session-duration-minutes must be >= 1"
                        );
                        settings.ssh_session_duration_minutes = mins;
                    }
                    Some("--json") => json_out = true,
                    Some(other) => {
                        bail!("unknown option `{other}` for `config app set`")
                    }
                    None => bail!("invalid UTF-8 option for `config app set`"),
                }
            }
            settings.updated_at_secs = now_unix_secs();
            db.save_app_settings(&settings)?;
            let saved = db.load_app_settings()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&saved)?);
            } else {
                println!("app_settings updated");
            }
            Ok(())
        }
        other => bail!("unknown `config app` action `{other}`"),
    }
}

fn read_stdin_trimmed(label: &str) -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .with_context(|| format!("read {label} from stdin"))?;
    let trimmed = buf.trim_end_matches(['\r', '\n']).to_owned();
    anyhow::ensure!(!trimmed.is_empty(), "{label} from stdin was empty");
    Ok(trimmed)
}

fn parse_auth_mode(raw: &str) -> Result<AuthMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "password" => Ok(AuthMode::Password),
        "private_key" | "private-key" | "key" => Ok(AuthMode::PrivateKey),
        other => bail!(
            "unknown auth mode `{other}`; expected password or private_key"
        ),
    }
}

struct SshSecretInput {
    password: Option<String>,
    password_stdin: bool,
    private_key_file: Option<PathBuf>,
    private_key_stdin: bool,
    passphrase: Option<String>,
}

impl SshSecretInput {
    fn resolve(
        self,
        auth_mode: AuthMode,
    ) -> Result<Option<SshCredentialSecret>> {
        let has_pw = self.password.is_some() || self.password_stdin;
        let has_key = self.private_key_file.is_some() || self.private_key_stdin;
        anyhow::ensure!(
            !(has_pw && has_key),
            "cannot combine password and private-key options"
        );
        if !has_pw && !has_key {
            return Ok(None);
        }
        match auth_mode {
            AuthMode::Password => {
                anyhow::ensure!(
                    !has_key,
                    "private key options cannot be used with auth_mode=password"
                );
                let pw = if let Some(pw) = self.password {
                    pw
                } else {
                    read_stdin_trimmed("SSH password")?
                };
                anyhow::ensure!(!pw.is_empty(), "SSH password cannot be empty");
                Ok(Some(SshCredentialSecret::Password { password: pw }))
            }
            AuthMode::PrivateKey => {
                anyhow::ensure!(
                    !has_pw,
                    "password options cannot be used with auth_mode=private_key"
                );
                let pem = if let Some(path) = self.private_key_file {
                    fs::read_to_string(&path).with_context(|| {
                        format!("read private key file {}", path.display())
                    })?
                } else {
                    read_stdin_trimmed("SSH private key")?
                };
                anyhow::ensure!(
                    !pem.trim().is_empty(),
                    "SSH private key cannot be empty"
                );
                Ok(Some(SshCredentialSecret::PrivateKey {
                    private_key_pem: pem,
                    passphrase: self.passphrase,
                }))
            }
        }
    }
}

fn cmd_config_ssh(args: &[OsString]) -> Result<()> {
    let Some(action) = args.first().map(|a| a.to_string_lossy()) else {
        bail!("usage: ruum config ssh <list|add|edit|remove>");
    };
    if matches!(action.as_ref(), "-h" | "--help" | "help") {
        println!("{CONFIG_HELP}");
        return Ok(());
    }
    let rest = &args[1..];
    let db = StorageDb::open_default()?;
    match action.as_ref() {
        "list" => {
            let json_out = check_json_only(rest, "ruum config ssh list")?;
            // Metadata only — zero keychain accesses.
            let profiles = db.list_ssh_profiles()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&profiles)?);
            } else if profiles.is_empty() {
                println!("no SSH profiles configured");
            } else {
                for p in profiles {
                    println!(
                        "{}  {}  {}@{}:{}  [{}]{}",
                        p.id,
                        p.alias,
                        p.username,
                        p.host,
                        p.port,
                        p.auth_mode.as_str(),
                        p.default_path
                            .as_deref()
                            .map_or(String::new(), |d| format!("  {d}"))
                    );
                }
            }
            Ok(())
        }
        "add" => cmd_config_ssh_add(&db, rest),
        "edit" => cmd_config_ssh_edit(&db, rest),
        "remove" | "rm" | "delete" => {
            let Some(key_os) = rest.first() else {
                bail!("usage: ruum config ssh remove <ALIAS_OR_ID> [--json]");
            };
            let key = os_to_string(key_os, "ALIAS_OR_ID")?;
            let json_out =
                check_json_only(&rest[1..], "ruum config ssh remove")?;
            let creds = LazyCredentialStore::open_default()
                .map_err(anyhow::Error::msg)?;
            let removed =
                delete_ssh_profile_with_credentials(&db, &creds, &key)?;
            anyhow::ensure!(removed, "SSH profile `{key}` not found");
            if json_out {
                println!("{}", json!({ "removed": true, "target": key }));
            } else {
                println!("removed SSH profile `{key}`");
            }
            Ok(())
        }
        other => bail!("unknown `config ssh` action `{other}`"),
    }
}

fn cmd_config_ssh_add(db: &StorageDb, args: &[OsString]) -> Result<()> {
    let mut alias = None;
    let mut host = None;
    let mut port = DEFAULT_SSH_PORT;
    let mut username = None;
    let mut auth_mode: Option<AuthMode> = None;
    let mut default_path = None;
    let mut json_out = false;
    let mut secrets = SshSecretInput {
        password: None,
        password_stdin: false,
        private_key_file: None,
        private_key_stdin: false,
        passphrase: None,
    };

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--alias") => alias = Some(next_str(&mut iter, "--alias")?),
            Some("--host") => host = Some(next_str(&mut iter, "--host")?),
            Some("--port") => {
                let raw = next_str(&mut iter, "--port")?;
                port = parse_u16(&raw, "--port")?;
            }
            Some("--username" | "--user") => {
                username = Some(next_str(&mut iter, "--username")?);
            }
            Some("--auth-mode") => {
                let raw = next_str(&mut iter, "--auth-mode")?;
                auth_mode = Some(parse_auth_mode(&raw)?);
            }
            Some("--default-path") => {
                default_path = Some(next_str(&mut iter, "--default-path")?);
            }
            Some("--password") => {
                secrets.password = Some(next_str(&mut iter, "--password")?);
            }
            Some("--password-stdin") => secrets.password_stdin = true,
            Some("--private-key-file") => {
                let raw = next_str(&mut iter, "--private-key-file")?;
                secrets.private_key_file = Some(PathBuf::from(raw));
            }
            Some("--private-key-stdin") => secrets.private_key_stdin = true,
            Some("--passphrase") => {
                secrets.passphrase = Some(next_str(&mut iter, "--passphrase")?);
            }
            Some("--json") => json_out = true,
            Some(other) => {
                bail!("unknown option `{other}` for `config ssh add`")
            }
            None => bail!("invalid UTF-8 option for `config ssh add`"),
        }
    }

    let alias = alias.context("--alias is required")?;
    let host = host.context("--host is required")?;
    let username = username.context("--username is required")?;
    let mode = match auth_mode {
        Some(m) => m,
        None if secrets.password.is_some() || secrets.password_stdin => {
            AuthMode::Password
        }
        None if secrets.private_key_file.is_some()
            || secrets.private_key_stdin =>
        {
            AuthMode::PrivateKey
        }
        None => bail!("--auth-mode is required (password or private_key)"),
    };

    let _ = SshEndpoint::with_label(&host, port, &username, &alias)
        .map_err(anyhow::Error::msg)?;
    let secret = secrets.resolve(mode)?;

    let mut profile = SshProfile::new(alias, host, port, username, mode);
    profile.default_path =
        Some(normalize_ssh_path_opt(default_path.as_deref()));
    let inserted = db.add_ssh_profile(&profile)?;

    if let Some(sec) = secret {
        let write_res = LazyCredentialStore::open_default()
            .and_then(|creds| creds.set_ssh_secret(&inserted.id, &sec));
        if let Err(err) = write_res {
            let _ = db.delete_ssh_profile(&inserted.id);
            return Err(anyhow::anyhow!("{err}"));
        }
    }

    if json_out {
        println!("{}", serde_json::to_string_pretty(&inserted)?);
    } else {
        println!("added SSH profile `{}` ({})", inserted.alias, inserted.id);
    }
    Ok(())
}

fn cmd_config_ssh_edit(db: &StorageDb, args: &[OsString]) -> Result<()> {
    let Some(key_os) = args.first() else {
        bail!("usage: ruum config ssh edit <ALIAS_OR_ID> [OPTIONS]");
    };
    let key = os_to_string(key_os, "ALIAS_OR_ID")?;
    let mut profile = db
        .get_ssh_profile(&key)?
        .with_context(|| format!("SSH profile `{key}` not found"))?;
    let mut json_out = false;
    let mut secrets = SshSecretInput {
        password: None,
        password_stdin: false,
        private_key_file: None,
        private_key_stdin: false,
        passphrase: None,
    };

    let mut iter = args[1..].iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--alias") => profile.alias = next_str(&mut iter, "--alias")?,
            Some("--host") => profile.host = next_str(&mut iter, "--host")?,
            Some("--port") => {
                let raw = next_str(&mut iter, "--port")?;
                profile.port = parse_u16(&raw, "--port")?;
            }
            Some("--username" | "--user") => {
                profile.username = next_str(&mut iter, "--username")?;
            }
            Some("--auth-mode") => {
                let raw = next_str(&mut iter, "--auth-mode")?;
                profile.auth_mode = parse_auth_mode(&raw)?;
            }
            Some("--default-path") => {
                profile.default_path =
                    Some(next_str(&mut iter, "--default-path")?);
            }
            Some("--clear-default-path") => profile.default_path = None,
            Some("--password") => {
                secrets.password = Some(next_str(&mut iter, "--password")?);
            }
            Some("--password-stdin") => secrets.password_stdin = true,
            Some("--private-key-file") => {
                let raw = next_str(&mut iter, "--private-key-file")?;
                secrets.private_key_file = Some(PathBuf::from(raw));
            }
            Some("--private-key-stdin") => secrets.private_key_stdin = true,
            Some("--passphrase") => {
                secrets.passphrase = Some(next_str(&mut iter, "--passphrase")?);
            }
            Some("--json") => json_out = true,
            Some(other) => {
                bail!("unknown option `{other}` for `config ssh edit`")
            }
            None => bail!("invalid UTF-8 option for `config ssh edit`"),
        }
    }

    let _ = SshEndpoint::from_profile(&profile).map_err(anyhow::Error::msg)?;
    let secret = secrets.resolve(profile.auth_mode)?;
    profile.updated_at_secs = now_unix_secs();

    if let Some(sec) = secret {
        let creds =
            LazyCredentialStore::open_default().map_err(anyhow::Error::msg)?;
        creds
            .set_ssh_secret(&profile.id, &sec)
            .map_err(anyhow::Error::msg)?;
    }

    let updated = db.update_ssh_profile(&profile)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&updated)?);
    } else {
        println!("updated SSH profile `{}` ({})", updated.alias, updated.id);
    }
    Ok(())
}

fn cmd_config_watchdog(args: &[OsString]) -> Result<()> {
    let Some(action) = args.first().map(|a| a.to_string_lossy()) else {
        bail!("usage: ruum config watchdog <get|set>");
    };
    if matches!(action.as_ref(), "-h" | "--help" | "help") {
        println!("{CONFIG_HELP}");
        return Ok(());
    }
    let rest = &args[1..];
    let db = StorageDb::open_default()?;
    let wd = db.watchdog()?;
    match action.as_ref() {
        "get" => {
            let json_out = check_json_only(rest, "ruum config watchdog get")?;
            let cfg = wd.load_config()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&cfg)?);
            } else {
                println!("enabled = {}", cfg.enabled);
                println!("background_service = {}", cfg.background_service);
                println!("min_free_gb = {}", cfg.min_free_gb);
                println!("poll_interval_secs = {}", cfg.poll_interval_secs);
                println!("cooldown_secs = {}", cfg.cooldown_secs);
            }
            Ok(())
        }
        "set" => {
            let mut cfg = wd.load_config()?;
            let mut json_out = false;
            let mut iter = rest.iter();
            while let Some(arg) = iter.next() {
                match arg.to_str() {
                    Some("--enabled") => {
                        let raw = next_str(&mut iter, "--enabled")?;
                        cfg.enabled = parse_bool(&raw, "--enabled")?;
                    }
                    Some("--background-service") => {
                        let raw = next_str(&mut iter, "--background-service")?;
                        cfg.background_service =
                            parse_bool(&raw, "--background-service")?;
                    }
                    Some("--min-free-gb") => {
                        let raw = next_str(&mut iter, "--min-free-gb")?;
                        cfg.min_free_gb = parse_u64(&raw, "--min-free-gb")?;
                    }
                    Some("--poll-interval-secs") => {
                        let raw = next_str(&mut iter, "--poll-interval-secs")?;
                        cfg.poll_interval_secs =
                            parse_u64(&raw, "--poll-interval-secs")?;
                    }
                    Some("--cooldown-secs") => {
                        let raw = next_str(&mut iter, "--cooldown-secs")?;
                        cfg.cooldown_secs = parse_u64(&raw, "--cooldown-secs")?;
                    }
                    Some("--json") => json_out = true,
                    Some(other) => {
                        bail!(
                            "unknown option `{other}` for `config watchdog set`"
                        )
                    }
                    None => {
                        bail!("invalid UTF-8 option for `config watchdog set`")
                    }
                }
            }
            cfg.updated_at_secs = now_unix_secs();
            wd.save_config(&cfg)?;
            let saved = wd.load_config()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&saved)?);
            } else {
                println!("watchdog_config updated");
            }
            Ok(())
        }
        other => bail!("unknown `config watchdog` action `{other}`"),
    }
}

fn webhook_endpoint_json(ep: &WebhookEndpoint) -> serde_json::Value {
    let header_names: Vec<String> = ep
        .parsed_headers()
        .map(|m| m.into_keys().collect())
        .unwrap_or_default();
    json!({
        "id": ep.id,
        "name": ep.name,
        "enabled": ep.enabled,
        "url": ep.url,
        "header_names": header_names,
        "headers_redacted": true,
        "timeout_secs": ep.timeout_secs,
        "created_at_secs": ep.created_at_secs,
        "updated_at_secs": ep.updated_at_secs,
    })
}

fn cmd_config_webhook_add(db: &StorageDb, args: &[OsString]) -> Result<()> {
    let mut name = None;
    let mut url = None;
    let mut enabled = true;
    let mut headers_json = "{}".to_owned();
    let mut timeout_secs = 10_u64;
    let mut json_out = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--name") => name = Some(next_str(&mut iter, "--name")?),
            Some("--url") => url = Some(next_str(&mut iter, "--url")?),
            Some("--enabled") => {
                let raw = next_str(&mut iter, "--enabled")?;
                enabled = parse_bool(&raw, "--enabled")?;
            }
            Some("--headers-json") => {
                headers_json = next_str(&mut iter, "--headers-json")?;
            }
            Some("--timeout-secs") => {
                let raw = next_str(&mut iter, "--timeout-secs")?;
                timeout_secs = parse_u64(&raw, "--timeout-secs")?;
            }
            Some("--json") => json_out = true,
            Some(other) => {
                bail!("unknown option `{other}` for `config webhook add`")
            }
            None => bail!("invalid UTF-8 option for `config webhook add`"),
        }
    }

    let mut ep = WebhookEndpoint::new(
        name.context("--name is required")?,
        url.context("--url is required")?,
    );
    ep.enabled = enabled;
    ep.headers_json = headers_json;
    ep.timeout_secs = timeout_secs;
    let inserted = db.add_webhook(&ep)?;
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&webhook_endpoint_json(&inserted))?
        );
    } else {
        println!("added webhook `{}` ({})", inserted.name, inserted.id);
    }
    Ok(())
}

fn cmd_config_webhook_edit(db: &StorageDb, args: &[OsString]) -> Result<()> {
    let Some(key_os) = args.first() else {
        bail!("usage: ruum config webhook edit <ID_OR_NAME> [OPTIONS]");
    };
    let key = os_to_string(key_os, "ID_OR_NAME")?;
    let mut ep = db
        .get_webhook(&key)?
        .with_context(|| format!("webhook `{key}` not found"))?;
    let mut json_out = false;

    let mut iter = args[1..].iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--name") => ep.name = next_str(&mut iter, "--name")?,
            Some("--url") => ep.url = next_str(&mut iter, "--url")?,
            Some("--enabled") => {
                let raw = next_str(&mut iter, "--enabled")?;
                ep.enabled = parse_bool(&raw, "--enabled")?;
            }
            Some("--headers-json") => {
                ep.headers_json = next_str(&mut iter, "--headers-json")?;
            }
            Some("--timeout-secs") => {
                let raw = next_str(&mut iter, "--timeout-secs")?;
                ep.timeout_secs = parse_u64(&raw, "--timeout-secs")?;
            }
            Some("--json") => json_out = true,
            Some(other) => {
                bail!("unknown option `{other}` for `config webhook edit`")
            }
            None => bail!("invalid UTF-8 option for `config webhook edit`"),
        }
    }

    ep.updated_at_secs = now_unix_secs();
    let updated = db.update_webhook(&ep)?;
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&webhook_endpoint_json(&updated))?
        );
    } else {
        println!("updated webhook `{}` ({})", updated.name, updated.id);
    }
    Ok(())
}

fn cmd_config_webhook(args: &[OsString]) -> Result<()> {
    let Some(action) = args.first().map(|a| a.to_string_lossy()) else {
        bail!("usage: ruum config webhook <list|add|edit|remove|get|set>");
    };
    if matches!(action.as_ref(), "-h" | "--help" | "help") {
        println!("{CONFIG_HELP}");
        return Ok(());
    }
    let rest = &args[1..];
    let db = StorageDb::open_default()?;
    match action.as_ref() {
        "list" => {
            let json_out = check_json_only(rest, "ruum config webhook list")?;
            let endpoints = db.list_webhooks()?;
            if json_out {
                let redacted: Vec<_> =
                    endpoints.iter().map(webhook_endpoint_json).collect();
                println!("{}", serde_json::to_string_pretty(&redacted)?);
            } else if endpoints.is_empty() {
                println!("no webhooks configured");
            } else {
                for ep in endpoints {
                    let header_names: Vec<String> = ep
                        .parsed_headers()
                        .map(|m| m.into_keys().collect())
                        .unwrap_or_default();
                    println!(
                        "{}  {}  enabled={}  {}  headers={header_names:?}  \
                         timeout={}s",
                        ep.id, ep.name, ep.enabled, ep.url, ep.timeout_secs
                    );
                }
            }
            Ok(())
        }
        "add" => cmd_config_webhook_add(&db, rest),
        "edit" => cmd_config_webhook_edit(&db, rest),
        "remove" | "rm" | "delete" => {
            let Some(key_os) = rest.first() else {
                bail!(
                    "usage: ruum config webhook remove <ID_OR_NAME> [--json]"
                );
            };
            let key = os_to_string(key_os, "ID_OR_NAME")?;
            let json_out =
                check_json_only(&rest[1..], "ruum config webhook remove")?;
            let ep = db
                .get_webhook(&key)?
                .with_context(|| format!("webhook `{key}` not found"))?;
            let removed = db.delete_webhook(&ep.id)?;
            anyhow::ensure!(removed, "webhook `{key}` not found");
            if json_out {
                println!(
                    "{}",
                    json!({
                        "removed": true,
                        "id": ep.id,
                        "name": ep.name,
                    })
                );
            } else {
                println!("removed webhook `{}` ({})", ep.name, ep.id);
            }
            Ok(())
        }
        "get" => {
            let json_out = check_json_only(rest, "ruum config webhook get")?;
            let cfg = db.load_webhook_config()?;
            let header_names: Vec<String> = cfg
                .parsed_headers()
                .map(|m| m.into_keys().collect())
                .unwrap_or_default();
            if json_out {
                // Redact header secret values in terminal output.
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "enabled": cfg.enabled,
                        "url": cfg.url,
                        "header_names": header_names,
                        "headers_redacted": true,
                        "timeout_secs": cfg.timeout_secs,
                        "updated_at_secs": cfg.updated_at_secs,
                    }))?
                );
            } else {
                println!("enabled = {}", cfg.enabled);
                println!("url = {}", cfg.url);
                println!("header_names = {header_names:?}");
                println!("timeout_secs = {}", cfg.timeout_secs);
            }
            Ok(())
        }
        "set" => {
            let mut cfg = db.load_webhook_config()?;
            let mut json_out = false;
            let mut iter = rest.iter();
            while let Some(arg) = iter.next() {
                match arg.to_str() {
                    Some("--enabled") => {
                        let raw = next_str(&mut iter, "--enabled")?;
                        cfg.enabled = parse_bool(&raw, "--enabled")?;
                    }
                    Some("--url") => cfg.url = next_str(&mut iter, "--url")?,
                    Some("--headers-json") => {
                        cfg.headers_json =
                            next_str(&mut iter, "--headers-json")?;
                    }
                    Some("--timeout-secs") => {
                        let raw = next_str(&mut iter, "--timeout-secs")?;
                        cfg.timeout_secs = parse_u64(&raw, "--timeout-secs")?;
                    }
                    Some("--json") => json_out = true,
                    Some(other) => {
                        bail!(
                            "unknown option `{other}` for `config webhook set`"
                        )
                    }
                    None => {
                        bail!("invalid UTF-8 option for `config webhook set`")
                    }
                }
            }
            cfg.updated_at_secs = now_unix_secs();
            db.save_webhook_config(&cfg)?;
            let saved = db.load_webhook_config()?;
            let header_names: Vec<String> = saved
                .parsed_headers()
                .map(|m| m.into_keys().collect())
                .unwrap_or_default();
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "enabled": saved.enabled,
                        "url": saved.url,
                        "header_names": header_names,
                        "headers_redacted": true,
                        "timeout_secs": saved.timeout_secs,
                        "updated_at_secs": saved.updated_at_secs,
                    }))?
                );
            } else {
                println!("webhook_config updated");
            }
            Ok(())
        }
        other => bail!("unknown `config webhook` action `{other}`"),
    }
}

fn cmd_config_ai(args: &[OsString]) -> Result<()> {
    let Some(action) = args.first().map(|a| a.to_string_lossy()) else {
        bail!("usage: ruum config ai <get|set>");
    };
    if matches!(action.as_ref(), "-h" | "--help" | "help") {
        println!("{CONFIG_HELP}");
        return Ok(());
    }
    let rest = &args[1..];
    let db = StorageDb::open_default()?;
    match action.as_ref() {
        "get" => {
            let json_out = check_json_only(rest, "ruum config ai get")?;
            let cfg = db.load_ai_config()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&cfg)?);
            } else {
                println!(
                    "auto_analyze_on_alert = {}",
                    cfg.auto_analyze_on_alert
                );
                println!("provider = {}", cfg.provider.id());
                println!("custom_command = {}", cfg.custom_command);
                println!("extra_args = {}", cfg.extra_args);
                println!("model = {}", cfg.model.as_deref().unwrap_or(""));
            }
            Ok(())
        }
        "set" => {
            let mut cfg = db.load_ai_config()?;
            let mut json_out = false;
            let mut iter = rest.iter();
            while let Some(arg) = iter.next() {
                match arg.to_str() {
                    Some("--auto-analyze" | "--auto-analyze-on-alert") => {
                        let raw = next_str(&mut iter, "--auto-analyze")?;
                        cfg.auto_analyze_on_alert =
                            parse_bool(&raw, "--auto-analyze")?;
                    }
                    Some("--provider") => {
                        let raw = next_str(&mut iter, "--provider")?;
                        cfg.provider = AiCliProvider::from_id(&raw)
                            .with_context(|| {
                                format!(
                                    "unknown AI provider `{raw}`; expected \
                                     agy, pi, codex, or custom"
                                )
                            })?;
                    }
                    Some("--custom-command") => {
                        cfg.custom_command =
                            next_str(&mut iter, "--custom-command")?;
                    }
                    Some("--extra-args") => {
                        cfg.extra_args = next_str(&mut iter, "--extra-args")?;
                    }
                    Some("--model") => {
                        cfg.model = Some(next_str(&mut iter, "--model")?);
                    }
                    Some("--clear-model") => cfg.model = None,
                    Some("--json") => json_out = true,
                    Some(other) => {
                        bail!("unknown option `{other}` for `config ai set`")
                    }
                    None => bail!("invalid UTF-8 option for `config ai set`"),
                }
            }
            cfg.updated_at_secs = now_unix_secs();
            db.save_ai_config(&cfg)?;
            let saved = db.load_ai_config()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&saved)?);
            } else {
                println!("ai_config updated");
            }
            Ok(())
        }
        other => bail!("unknown `config ai` action `{other}`"),
    }
}

fn cmd_config_update(args: &[OsString]) -> Result<()> {
    let Some(action) = args.first().map(|a| a.to_string_lossy()) else {
        bail!("usage: ruum config update <get|set>");
    };
    if matches!(action.as_ref(), "-h" | "--help" | "help") {
        println!("{CONFIG_HELP}");
        return Ok(());
    }
    let rest = &args[1..];
    let db = StorageDb::open_default()?;
    match action.as_ref() {
        "get" => {
            let json_out = check_json_only(rest, "ruum config update get")?;
            let cfg = db.load_update_settings()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&cfg)?);
            } else {
                println!("auto_check = {}", cfg.auto_check);
                println!("channel = {}", cfg.channel);
                println!("send_client_id = {}", cfg.send_client_id);
                println!(
                    "skipped_version = {}",
                    cfg.skipped_version.as_deref().unwrap_or("")
                );
                println!("last_checked_at_secs = {}", cfg.last_checked_at_secs);
            }
            Ok(())
        }
        "set" => {
            let mut cfg = db.load_update_settings()?;
            let mut json_out = false;
            let mut iter = rest.iter();
            while let Some(arg) = iter.next() {
                match arg.to_str() {
                    Some("--auto-check") => {
                        let raw = next_str(&mut iter, "--auto-check")?;
                        cfg.auto_check = parse_bool(&raw, "--auto-check")?;
                    }
                    Some("--channel") => {
                        let raw = next_str(&mut iter, "--channel")?;
                        cfg.channel = raw.parse::<UpdateChannel>()?;
                    }
                    Some("--send-client-id") => {
                        let raw = next_str(&mut iter, "--send-client-id")?;
                        cfg.send_client_id =
                            parse_bool(&raw, "--send-client-id")?;
                    }
                    Some("--clear-skipped") => cfg.skipped_version = None,
                    Some("--json") => json_out = true,
                    Some(other) => {
                        bail!(
                            "unknown option `{other}` for `config update set`"
                        )
                    }
                    None => {
                        bail!("invalid UTF-8 option for `config update set`")
                    }
                }
            }
            cfg.updated_at_secs = now_unix_secs();
            db.save_update_settings(&cfg)?;
            let saved = db.load_update_settings()?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&saved)?);
            } else {
                println!("update_settings updated");
            }
            Ok(())
        }
        other => bail!("unknown `config update` action `{other}`"),
    }
}

fn normalized_cli_lang() -> String {
    let raw = sys_locale::get_locale().unwrap_or_else(|| "en".to_owned());
    let head = raw.split(['-', '_', '.']).next().unwrap_or("en").trim();
    if head.is_empty() {
        "en".to_owned()
    } else {
        head.to_ascii_lowercase()
    }
}

fn cmd_update(args: &[OsString]) -> Result<()> {
    let rest = match args.first().and_then(|a| a.to_str()) {
        Some("check") => &args[1..],
        Some("-h" | "--help" | "help") => {
            println!("{UPDATE_HELP}");
            return Ok(());
        }
        _ => args,
    };

    let mut channel_override: Option<UpdateChannel> = None;
    let mut json_out = false;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{UPDATE_HELP}");
                return Ok(());
            }
            Some("--channel") => {
                let raw = next_str(&mut iter, "--channel")?;
                channel_override = Some(raw.parse::<UpdateChannel>()?);
            }
            Some("--json") => json_out = true,
            Some(other) => {
                bail!(
                    "unknown option `{other}` for `ruum update`; try \
                     `ruum update --help`"
                );
            }
            None => bail!("invalid UTF-8 option for `ruum update`"),
        }
    }

    let db = StorageDb::open_default()?;
    let mut settings = db.load_update_settings()?;
    let channel = channel_override.unwrap_or(settings.channel);
    let client_id = if settings.send_client_id {
        Some(db.ensure_update_client_id()?)
    } else {
        None
    };

    let base_url = resolve_update_base_url();
    let pubkey_b64 = resolve_effective_public_key_for_url(&base_url);
    let env = UpdateEnv::current();
    let os = env.os.unwrap_or_else(UpdateFactory::current_os);
    let arch = env.arch.unwrap_or_else(UpdateArch::current);
    let install_kind = UpdateFactory::create_for(os).detect_install_kind(&env);
    let lang = normalized_cli_lang();

    let result = check_for_updates(&UpdateCheckRequest {
        channel,
        current_version: env!("CARGO_PKG_VERSION"),
        os,
        arch,
        install_kind,
        lang: &lang,
        entry: "cli",
        client_id: client_id.as_deref(),
        base_url: &base_url,
        public_key_b64: &pubkey_b64,
        github_repo: DEFAULT_GITHUB_REPO,
        github_fallback_base: None,
    })?;

    let now = now_unix_secs();
    settings = db.load_update_settings()?;
    settings.last_checked_at_secs = now;
    settings.updated_at_secs = now;
    db.save_update_settings(&settings)?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    if result.update_available {
        println!(
            "Update available: v{} -> v{} ({})",
            result.current_version, result.latest_version, result.channel
        );
        println!("Published: {}", result.manifest.published_at);
        if let Some(asset) = &result.selected_asset {
            println!(
                "Package: {} ({})",
                asset.filename,
                human_bytes(asset.size)
            );
        }
        if !result.manifest.notes.trim().is_empty() {
            println!("\n{}", result.manifest.notes.trim());
        }
        println!("\nUpgrade command:\n  {}", result.upgrade_command);
        println!("Release page: {}", result.manifest.release_url);
    } else {
        println!(
            "ruum v{} is up to date ({}, latest v{})",
            result.current_version, result.channel, result.latest_version
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchdogSubcommand {
    RunLoop,
    RunOnce,
    Install,
    Uninstall,
    Status,
    Help,
}

fn parse_watchdog_args(args: &[OsString]) -> Result<WatchdogSubcommand> {
    if args.is_empty() {
        return Ok(WatchdogSubcommand::RunLoop);
    }
    if args.len() == 1 {
        return match args[0].to_str() {
            Some("-h" | "--help" | "help") => Ok(WatchdogSubcommand::Help),
            Some("run") => Ok(WatchdogSubcommand::RunLoop),
            Some("once" | "--once") => Ok(WatchdogSubcommand::RunOnce),
            Some("install") => Ok(WatchdogSubcommand::Install),
            Some("uninstall") => Ok(WatchdogSubcommand::Uninstall),
            Some("status") => Ok(WatchdogSubcommand::Status),
            Some(other) => bail!(
                "unknown option `{other}` for `ruum watchdog`; try \
                 `ruum watchdog --help`"
            ),
            None => bail!("invalid UTF-8 option for `ruum watchdog`"),
        };
    }
    if args.len() == 2
        && args[0] == "run"
        && (args[1] == "-h" || args[1] == "--help")
    {
        return Ok(WatchdogSubcommand::Help);
    }
    bail!(
        "unknown option `{}` for `ruum watchdog`; try `ruum watchdog --help`",
        args.iter()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn cmd_watchdog(args: &[OsString]) -> Result<()> {
    match parse_watchdog_args(args)? {
        WatchdogSubcommand::Help => {
            println!("{WATCHDOG_HELP}");
            Ok(())
        }
        WatchdogSubcommand::RunOnce => {
            let _ = run_default_once_with_db()?;
            Ok(())
        }
        WatchdogSubcommand::RunLoop => {
            let cancel = AtomicBool::new(false);
            run_default_loop_with_db(&cancel)?;
            Ok(())
        }
        WatchdogSubcommand::Install => {
            let current_exe = std::env::current_exe()?;
            let exe = dunce::canonicalize(&current_exe).unwrap_or(current_exe);
            install_service(&DefaultCommandRunner, &exe)?;
            println!("watchdog service installed");
            Ok(())
        }
        WatchdogSubcommand::Uninstall => {
            uninstall_service(&DefaultCommandRunner)?;
            println!("watchdog service uninstalled");
            Ok(())
        }
        WatchdogSubcommand::Status => {
            let status = service_status(&DefaultCommandRunner)?;
            let label = match status {
                ServiceStatus::Active => "active",
                ServiceStatus::Inactive => "inactive",
                ServiceStatus::NotInstalled => "not_installed",
                ServiceStatus::Unsupported => "unsupported",
            };
            println!("{label}");
            Ok(())
        }
    }
}

fn cmd_analyze(args: &[OsString]) -> Result<()> {
    let mut path_arg: Option<PathBuf> = None;
    let mut json_out = false;
    for arg in args {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{ANALYZE_HELP}");
                return Ok(());
            }
            Some("--json") => json_out = true,
            Some(other) if other.starts_with('-') => {
                bail!("unknown option `{other}` for `ruum analyze`");
            }
            _ => {
                anyhow::ensure!(
                    path_arg.is_none(),
                    "ruum analyze accepts at most one PATH"
                );
                path_arg = Some(PathBuf::from(arg));
            }
        }
    }

    let root = resolve_local_root(path_arg.as_deref(), false)?;
    let db = StorageDb::open_default()?;
    let ai_cfg = db.load_ai_config()?;
    let current_exe = std::env::current_exe()?;
    let now = now_unix_secs();

    let proposal = run_analysis_for_config(
        &ai_cfg,
        &root,
        &current_exe,
        now,
        &DefaultCommandRunner,
    )
    .map_err(anyhow::Error::msg)?;

    let ai_state = AiState::from_proposal(&proposal, now)?;
    db.save_ai_state(&ai_state)?;
    notify_gui_watchdog_update();

    if json_out {
        println!("{}", serde_json::to_string_pretty(&proposal)?);
    } else {
        println!("{} ({} items)", proposal.summary, proposal.items.len());
        for item in &proposal.items {
            println!(
                "  {}  {}  [{}]",
                item.path.display(),
                human_bytes(item.bytes),
                item.mode.label()
            );
        }
    }
    Ok(())
}

fn cmd_mcp(args: &[OsString]) -> Result<()> {
    let mut read_only = false;
    for arg in args {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{MCP_HELP}");
                return Ok(());
            }
            Some("--read-only") => read_only = true,
            Some(other) => {
                bail!(
                    "unknown option `{other}` for `ruum mcp`; try `ruum mcp \
                     --help`"
                )
            }
            None => bail!("invalid UTF-8 option for `ruum mcp`"),
        }
    }
    ruum_core::mcp::serve_stdio_with_options(read_only)?;
    Ok(())
}

fn cmd_extract_scanner(args: &[OsString]) -> Result<()> {
    anyhow::ensure!(args.len() == 2, "usage: ruum extract-scanner ARCH OUTPUT");
    let archive = Archive::current()?
        .context("no embedded scanners; build with cargo xtask portable")?;
    let arch = args[0].to_str().context("ARCH must be x86_64 or aarch64")?;
    let (_, bytes) = archive.load(arch)?;
    let out_path = Path::new(&args[1]);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_path)?;
    output.write_all(&bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        output.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    println!("Extracted Linux {arch} scanner to {}", out_path.display());
    Ok(())
}

fn cmd_ssh_askpass(args: &[OsString]) -> Result<()> {
    anyhow::ensure!(
        args.is_empty(),
        "ruum ssh-askpass accepts no positional arguments"
    );
    let mut out = io::stdout().lock();
    run_askpass_cli_from_env(&mut out)?;
    Ok(())
}

fn cmd_run_script(args: &[OsString]) -> Result<()> {
    let mut script_path: Option<PathBuf> = None;
    let mut profile_key: Option<String> = None;
    let mut auto_yes = false;
    let mut pause = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{RUN_SCRIPT_HELP}");
                return Ok(());
            }
            Some("--profile") => {
                profile_key = Some(next_str(&mut iter, "--profile")?);
            }
            Some("-y" | "--yes") => auto_yes = true,
            Some("--pause") => pause = true,
            Some(other) if other.starts_with('-') => {
                bail!("unknown option `{other}` for `ruum run-script`");
            }
            _ => {
                anyhow::ensure!(
                    script_path.is_none(),
                    "ruum run-script accepts a single SCRIPT_PATH"
                );
                script_path = Some(PathBuf::from(arg));
            }
        }
    }

    let script_path = script_path.context(
        "usage: ruum run-script <SCRIPT_PATH> [--profile <ID_OR_ALIAS>] [--yes] [--pause]",
    )?;

    let res =
        execute_run_script(&script_path, profile_key.as_deref(), auto_yes);
    if pause {
        if let Err(ref err) = res {
            eprintln!("ruum: {err:#}");
        }
        print!("\nPress Enter to close...");
        let _ = io::stdout().flush();
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
    }
    res
}

fn execute_run_script(
    script_path: &Path,
    profile_key: Option<&str>,
    auto_yes: bool,
) -> Result<()> {
    let script_content =
        fs::read_to_string(script_path).with_context(|| {
            format!("read cleanup script {}", script_path.display())
        })?;

    println!("Cleanup script: {}", script_path.display());
    for line in script_content.lines().take(24) {
        let trimmed = line.trim();
        if trimmed == "#!/bin/sh" || trimmed == "set -eu" {
            continue;
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            println!("  {}", comment.trim());
        } else if !trimmed.is_empty() {
            break;
        }
    }

    if !auto_yes {
        print!("Proceed with cleanup? [y/N] ");
        io::stdout().flush()?;
        let mut reply = String::new();
        let read_bytes = io::stdin().read_line(&mut reply)?;
        let normalized = reply.trim().to_ascii_lowercase();
        if read_bytes == 0 || !matches!(normalized.as_str(), "y" | "yes") {
            bail!("cleanup aborted by user");
        }
    }

    if let Some(key) = profile_key {
        let db = StorageDb::open_default()?;
        let profile = db
            .get_ssh_profile(key)?
            .with_context(|| format!("SSH profile `{key}` not found"))?;
        let duration = db
            .load_app_settings()
            .map_or(DEFAULT_SSH_SESSION_DURATION_MINUTES, |s| {
                s.ssh_session_duration_minutes
            });
        let creds =
            LazyCredentialStore::open_default().map_err(anyhow::Error::msg)?;
        let endpoint =
            SshEndpoint::from_profile(&profile).map_err(anyhow::Error::msg)?;
        let material =
            SshAuthMaterial::from_profile_and_store(&profile, &creds)
                .map_err(anyhow::Error::msg)?;
        let mut session = SshSession::start(endpoint, material, duration)
            .map_err(anyhow::Error::msg)?;

        let payload = ruum_core::export::extract_remote_payload(
            &script_content,
        )
        .context(
            "cleanup script does not contain a valid RUUM_REMOTE_PAYLOAD \
                 block",
        )?;

        let mut cmd = session
            .build_ssh_command(&[], "/bin/sh -s")
            .context("build ssh command for remote cleanup script")?;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let mut child = cmd
            .spawn()
            .context("spawn ssh process for remote cleanup script")?;

        let write_res = if let Some(mut child_stdin) = child.stdin.take() {
            let res = child_stdin
                .write_all(payload.as_bytes())
                .and_then(|()| child_stdin.flush());
            drop(child_stdin);
            res
        } else {
            Ok(())
        };

        let status = child
            .wait()
            .context("wait for remote ssh cleanup execution")?;
        session.close();
        write_res.context("write cleanup payload to remote ssh stdin")?;

        if !status.success() {
            bail!(
                "remote cleanup script exited with status {}",
                status.code().unwrap_or(1)
            );
        }
        let _ = db.clear_ssh_profile_pending_script(&profile.id);
        println!("Remote cleanup finished; cleared pending script status.");
        return Ok(());
    }

    let flag = if script_content.contains("--delete")
        && !script_content.contains("--yes")
    {
        "--delete"
    } else {
        "--yes"
    };
    let mut cmd = Command::new(if cfg!(unix) { "/bin/sh" } else { "sh" });
    cmd.arg(script_path)
        .arg(flag)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let status = cmd
        .status()
        .context("execute local cleanup script via sh")?;
    if !status.success() {
        bail!(
            "cleanup script exited with status {}",
            status.code().unwrap_or(1)
        );
    }
    Ok(())
}
