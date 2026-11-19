#![cfg(feature = "service")]
//! End-to-end tests against the compiled `ruum` binary in an isolated sandbox.
//!
//! Every test runs `env!("CARGO_BIN_EXE_ruum")` with sandboxed `HOME`,
//! `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, `TMPDIR`, `APPDATA`, and
//! `LOCALAPPDATA`, clears GUI display variables, and sets `SSH_CONNECTION=1`
//! so tests never connect to a host GUI IPC socket, touch real user config or
//! keychains, or dial real SSH hosts.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use ruum_core::confirm::{
    IpcListenerGuard, IpcRequest, IpcResponse, RemoteConnectConfirmDecision,
    load_or_create_ipc_token,
};
use ruum_core::ssh_session::{
    ENV_ASKPASS_CMD, ENV_ASKPASS_ENDPOINT, ENV_ASKPASS_TOKEN, SshAuthMaterial,
    SshEndpoint, SshSession,
};
use ruum_core::storage::StorageDb;
use serde_json::{Value, json};
use tempfile::TempDir;
use zeroize::Zeroizing;

const MAX_UNIX_SOCKET_BYTES: usize = 96;
const VALID_TEST_ROOT_SECRET_HEX: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SENTINEL_FORBID_KEYCHAIN: &str = "must-not-touch-keychain";

#[derive(Debug)]
struct Sandbox {
    root: TempDir,
    #[allow(
        dead_code,
        reason = "keeps optional short XDG_RUNTIME_DIR alive for sandbox \
                  lifetime"
    )]
    short_runtime: Option<TempDir>,
    home: PathBuf,
    xdg_config: PathBuf,
    xdg_runtime: PathBuf,
    tmp: PathBuf,
    appdata: PathBuf,
    localappdata: PathBuf,
    bin: PathBuf,
    scan_root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create sandbox root");
        let base = root.path();

        let home = base.join("home");
        let xdg_config = base.join("xdg_config");
        let tmp = base.join("tmp");
        let appdata = base.join("appdata");
        let localappdata = base.join("localappdata");
        let bin = base.join("bin");
        let scan_root = base.join("scan_root");
        let default_runtime = base.join("run");

        for dir in [
            &home,
            &xdg_config,
            &tmp,
            &appdata,
            &localappdata,
            &bin,
            &scan_root,
            &default_runtime,
        ] {
            fs::create_dir_all(dir).expect("create sandbox subdir");
        }

        // Keep `XDG_RUNTIME_DIR/ruum-ipc.sock` strictly below 96 bytes so
        // Unix domain socket resolution never falls back to `/tmp/ruum-<uid>.sock`.
        let candidate_sock = default_runtime.join("ruum-ipc.sock");
        let (short_runtime, xdg_runtime) =
            if candidate_sock.as_os_str().as_encoded_bytes().len()
                < MAX_UNIX_SOCKET_BYTES
            {
                (None, default_runtime)
            } else {
                let short = tempfile::Builder::new()
                    .prefix("rm")
                    .tempdir_in("/tmp")
                    .expect("create short XDG_RUNTIME_DIR under /tmp");
                let path = short.path().to_path_buf();
                assert!(
                    path.join("ruum-ipc.sock")
                        .as_os_str()
                        .as_encoded_bytes()
                        .len()
                        < MAX_UNIX_SOCKET_BYTES,
                    "short runtime socket path must fit sun_path"
                );
                (Some(short), path)
            };

        Self {
            root,
            short_runtime,
            home,
            xdg_config,
            xdg_runtime,
            tmp,
            appdata,
            localappdata,
            bin,
            scan_root,
        }
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ruum"));
        cmd.env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_RUNTIME_DIR", &self.xdg_runtime)
            .env("TMPDIR", &self.tmp)
            .env("TEMP", &self.tmp)
            .env("TMP", &self.tmp)
            .env("APPDATA", &self.appdata)
            .env("LOCALAPPDATA", &self.localappdata)
            .env("SSH_CONNECTION", "1")
            .env("TERM", "dumb")
            .env("NO_COLOR", "1")
            .env("RUUM_TEST_ROOT_SECRET_HEX", SENTINEL_FORBID_KEYCHAIN)
            .env("RUUM_DISABLE_OS_NOTIFICATIONS", "1")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("RUUM_GUI_BIN")
            .env_remove(ENV_ASKPASS_CMD)
            .env_remove(ENV_ASKPASS_ENDPOINT)
            .env_remove(ENV_ASKPASS_TOKEN)
            .env_remove("CMUX_SURFACE_ID")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        cmd
    }

    fn cmd_with_test_keychain(&self) -> Command {
        let mut cmd = self.cmd();
        cmd.env("RUUM_TEST_ROOT_SECRET_HEX", VALID_TEST_ROOT_SECRET_HEX);
        cmd
    }

    fn cmd_with_sandbox_path(&self) -> Command {
        let mut cmd = self.cmd();
        let mut paths = vec![self.bin.clone()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        let joined: OsString =
            std::env::join_paths(paths).expect("join PATH entries");
        cmd.env("PATH", joined);
        cmd
    }

    fn config_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home.join("Library/Application Support/ruum")
        } else if cfg!(windows) {
            self.appdata.join("ruum")
        } else {
            self.xdg_config.join("ruum")
        }
    }

    fn db_path(&self) -> PathBuf {
        self.config_dir().join("ruum.db")
    }

    #[cfg(unix)]
    fn ipc_endpoint(&self) -> PathBuf {
        self.xdg_runtime.join("ruum-ipc.sock")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd()
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("execute ruum binary")
    }

    fn run_with_keychain(&self, args: &[&str]) -> Output {
        self.cmd_with_test_keychain()
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("execute ruum binary with test keychain")
    }

    #[cfg(unix)]
    fn write_unix_script(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = self.bin.join(name);
        fs::write(&path, body).expect("write unix stub script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("chmod stub script");
        path
    }

    fn write_stub_ai_cli(&self, marker: &Path) -> PathBuf {
        #[cfg(unix)]
        {
            let body = format!(
                "#!/bin/sh\n\
                 printf 'invoked\\n' > \"{}\"\n\
                 printf '{{\"summary\":\"stub analysis ok\",\"items\":[]}}\\n'\n",
                marker.display()
            );
            self.write_unix_script("stub-ai", &body)
        }
        #[cfg(windows)]
        {
            let path = self.bin.join("stub-ai.cmd");
            let body = format!(
                "@echo off\r\n\
                 echo invoked> \"{}\"\r\n\
                 echo {{\"summary\":\"stub analysis ok\",\"items\":[]}}\r\n",
                marker.display()
            );
            fs::write(&path, body).expect("write windows stub ai script");
            path
        }
    }

    fn write_stub_gui_bin(&self, marker: &Path) -> PathBuf {
        #[cfg(unix)]
        {
            let body = format!(
                "#!/bin/sh\nprintf 'gui-launched\\n' > \"{}\"\n",
                marker.display()
            );
            self.write_unix_script("ruum-gui", &body)
        }
        #[cfg(windows)]
        {
            let path = self.bin.join("ruum-gui.cmd");
            let body = format!(
                "@echo off\r\necho gui-launched> \"{}\"\r\n",
                marker.display()
            );
            fs::write(&path, body).expect("write windows stub gui script");
            path
        }
    }
}

fn stdout_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn run_mcp_session_with_cmd(
    mut cmd: Command,
    extra_args: &[&str],
    messages: &[Value],
) -> (Output, Vec<Value>) {
    cmd.arg("mcp")
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn ruum mcp");
    {
        let mut stdin = child.stdin.take().expect("mcp stdin");
        for msg in messages {
            let line = serde_json::to_string(msg).expect("serialize json-rpc");
            writeln!(stdin, "{line}").expect("write json-rpc line");
        }
        stdin.flush().expect("flush mcp stdin");
    }

    let output = child.wait_with_output().expect("wait for ruum mcp");
    let raw_stdout = String::from_utf8(output.stdout.clone())
        .expect("mcp stdout must be valid utf-8");
    let responses: Vec<Value> = raw_stdout
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|err| {
                panic!(
                    "stdout must contain only valid JSON-RPC frames \
                     ({err}): {line:?}"
                )
            })
        })
        .collect();
    (output, responses)
}

fn run_mcp_session(
    sandbox: &Sandbox,
    extra_args: &[&str],
    messages: &[Value],
) -> (Output, Vec<Value>) {
    run_mcp_session_with_cmd(sandbox.cmd(), extra_args, messages)
}

fn parse_tool_result(resp: &Value) -> (bool, Value) {
    assert_eq!(resp["jsonrpc"], "2.0");
    assert!(
        resp.get("error").is_none(),
        "unexpected json-rpc protocol error: {resp}"
    );
    let result = &resp["result"];
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let text = result["content"][0]["text"]
        .as_str()
        .expect("tool content text");
    let payload = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
    (is_error, payload)
}

#[derive(Debug)]
struct CapturedHttpRequest {
    request_line: String,
    headers: Vec<String>,
    body: String,
}

fn spawn_mock_http_server()
-> (u16, thread::JoinHandle<Option<CapturedHttpRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock http");
    let port = listener.local_addr().expect("mock http addr").port();
    listener
        .set_nonblocking(true)
        .expect("set mock http nonblocking");

    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    thread::sleep(Duration::from_millis(15));
                }
                Err(_) => return None,
            }
        };

        stream
            .set_nonblocking(false)
            .expect("blocking mock http stream");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("set read timeout");

        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).ok()?;

        let mut headers = Vec::new();
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).ok()?;
            if n == 0 || line == "\r\n" || line == "\n" {
                break;
            }
            let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
            if let Some((name, val)) = trimmed.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = val.trim().parse().unwrap_or(0);
            }
            headers.push(trimmed);
        }

        let mut body_bytes = vec![0_u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body_bytes).ok()?;
        }
        let body = String::from_utf8_lossy(&body_bytes).into_owned();

        let inner = reader.into_inner();
        let _ =
            inner.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let _ = inner.flush();

        Some(CapturedHttpRequest {
            request_line: request_line.trim().to_owned(),
            headers,
            body,
        })
    });

    (port, handle)
}

#[test]
fn cli_zero_args_launches_stub_gui_and_falls_back_to_help_when_headless() {
    let sb = Sandbox::new();

    // 1. Headless session (SSH_CONNECTION=1, no DISPLAY/WAYLAND_DISPLAY) ->
    //    falls back to printing CLI help and exits 0.
    let headless = sb.run(&[]);
    assert!(
        headless.status.success(),
        "headless 0-arg failed: {}",
        stderr_str(&headless)
    );
    let help_text = stdout_str(&headless);
    assert!(help_text.contains("ruum scan"), "help: {help_text}");
    assert!(help_text.contains("ruum config"), "help: {help_text}");
    assert!(stderr_str(&headless).is_empty());

    // 2. Graphical display reachable, but ruum-gui binary missing ->
    //    falls back cleanly to printing CLI help.
    let missing_gui = sb.root.path().join("missing-ruum-gui");
    let no_gui_out = sb
        .cmd()
        .env_remove("SSH_CONNECTION")
        .env("DISPLAY", ":99")
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("RUUM_GUI_BIN", &missing_gui)
        .stdin(Stdio::null())
        .output()
        .expect("run ruum with missing gui");
    assert!(no_gui_out.status.success());
    assert!(stdout_str(&no_gui_out).contains("ruum watchdog"));

    // 3. Graphical display reachable + stub ruum-gui present ->
    //    launches detached stub and prints no CLI help.
    let marker = sb.root.path().join("gui-launched.marker");
    let stub_gui = sb.write_stub_gui_bin(&marker);
    let launched = sb
        .cmd()
        .env_remove("SSH_CONNECTION")
        .env("DISPLAY", ":99")
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("RUUM_GUI_BIN", &stub_gui)
        .stdin(Stdio::null())
        .output()
        .expect("run ruum with stub gui");
    assert!(
        launched.status.success(),
        "0-arg GUI launch failed: {}",
        stderr_str(&launched)
    );
    assert!(stdout_str(&launched).is_empty());

    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "detached stub ruum-gui did not write marker"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn cli_help_and_argument_validation() {
    let sb = Sandbox::new();

    for arg in ["help", "-h", "--help"] {
        let out = sb.run(&[arg]);
        assert!(out.status.success(), "{arg} failed: {}", stderr_str(&out));
        let text = stdout_str(&out);
        assert!(text.contains("ruum scan"), "missing scan usage: {text}");
        assert!(text.contains("ruum space"), "missing space usage: {text}");
        assert!(text.contains("ruum ssh"), "missing ssh usage: {text}");
        assert!(text.contains("ruum config"), "missing config usage: {text}");
        assert!(
            text.contains("ruum watchdog"),
            "missing watchdog usage: {text}"
        );
        assert!(
            text.contains("ruum analyze"),
            "missing analyze usage: {text}"
        );
        assert!(
            text.contains("ruum mcp [--read-only]"),
            "missing mcp usage: {text}"
        );
        assert!(
            text.contains("ruum extract-scanner ARCH OUTPUT"),
            "missing extract-scanner usage: {text}"
        );
        assert!(stderr_str(&out).is_empty());
    }

    let unknown_flag = sb.run(&["--definitely-unknown-flag"]);
    assert!(!unknown_flag.status.success());
    assert!(stderr_str(&unknown_flag).contains("unknown option"));

    let unknown_cmd = sb.run(&["definitely-unknown-cmd"]);
    assert!(!unknown_cmd.status.success());
    assert!(stderr_str(&unknown_cmd).contains("unknown command"));

    for bad_depth in ["0", "7", "abc"] {
        let out = sb.run(&["scan", "--depth", bad_depth, "."]);
        assert!(
            !out.status.success(),
            "scan --depth {bad_depth} should fail, stdout: {}",
            stdout_str(&out)
        );
    }

    let scan_str = sb.scan_root.to_string_lossy().into_owned();
    let conflict = sb.run(&["scan", "--disk", &scan_str]);
    assert!(!conflict.status.success());
    assert!(stderr_str(&conflict).contains("--disk"));

    let bad_metric = sb.run(&["scan", "--metric", "inodes", &scan_str]);
    assert!(!bad_metric.status.success());

    let home_str = sb.home.to_string_lossy().into_owned();
    let two_paths = sb.run(&["scan", &scan_str, &home_str]);
    assert!(!two_paths.status.success());

    let missing_path = sb.scan_root.join("does-not-exist-anywhere");
    let missing_str = missing_path.to_string_lossy().into_owned();
    let missing_out = sb.run(&["scan", &missing_str]);
    assert!(!missing_out.status.success());
}

#[test]
fn cli_scan_local_text_json_and_stdio_v1() {
    let sb = Sandbox::new();
    let visible = sb.scan_root.join("visible/data.bin");
    let hidden = sb.scan_root.join(".hidden/secret.bin");
    fs::create_dir_all(visible.parent().unwrap()).expect("mkdir visible");
    fs::create_dir_all(hidden.parent().unwrap()).expect("mkdir hidden");
    fs::write(&visible, vec![b'a'; 4096]).expect("write visible");
    fs::write(&hidden, vec![b'b'; 8192]).expect("write hidden");

    let root_str = sb.scan_root.to_string_lossy().into_owned();

    let text_out = sb.run(&["scan", &root_str, "-a"]);
    assert!(
        text_out.status.success(),
        "scan text failed: {}",
        stderr_str(&text_out)
    );
    let text = stdout_str(&text_out);
    assert!(text.contains("12288 bytes"), "unexpected summary: {text}");
    assert!(text.contains(".hidden"), "missing hidden dir: {text}");

    let json_out =
        sb.run(&["scan", &root_str, "-a", "-H", "-d", "2", "--json"]);
    assert!(
        json_out.status.success(),
        "scan json failed: {}",
        stderr_str(&json_out)
    );
    let parsed: Value =
        serde_json::from_str(&stdout_str(&json_out)).expect("scan json");
    assert_eq!(parsed["bytes"], 4096);
    assert_eq!(parsed["files"], 1);
    let entries = parsed["entries"].as_array().expect("entries array");
    assert!(!entries.iter().any(|e| e["name"] == ".hidden"));

    let stdio_out = sb.run(&["scan", &root_str, "--stdio-v1"]);
    assert!(
        stdio_out.status.success(),
        "scan --stdio-v1 failed: {}",
        stderr_str(&stdio_out)
    );
    let stdio_stdout = stdout_str(&stdio_out);
    let lines: Vec<&str> = stdio_stdout.lines().collect();
    assert!(
        lines.len() >= 3,
        "expected header + nodes + done: {lines:?}"
    );
    let header: Value =
        serde_json::from_str(lines[0]).expect("stdio-v1 header json");
    assert_eq!(header["version"], 1);
    assert_eq!(lines.last().copied(), Some("\"done\""));
}

#[test]
fn cli_config_full_sqlite_crud_and_zero_keychain_reads() {
    let sb = Sandbox::new();

    // App settings CRUD (zero keychain).
    let app_def = sb.run(&["config", "app", "get", "--json"]);
    assert!(app_def.status.success(), "{}", stderr_str(&app_def));
    let def_json: Value = serde_json::from_str(&stdout_str(&app_def)).unwrap();
    assert_eq!(def_json["power_preset"], "balanced");
    assert_eq!(def_json["default_depth"], 3);
    assert_eq!(def_json["ssh_session_duration_minutes"], 15);

    let app_set = sb.run(&[
        "config",
        "app",
        "set",
        "--power-preset",
        "aggressive",
        "--default-depth",
        "5",
        "--apparent-size",
        "true",
        "--include-hidden",
        "false",
        "--follow-system-appearance",
        "false",
        "--ssh-session-duration-minutes",
        "30",
        "--json",
    ]);
    assert!(app_set.status.success(), "{}", stderr_str(&app_set));
    let app_get = sb.run(&["config", "app", "get", "--json"]);
    assert!(app_get.status.success());
    let saved_app: Value = serde_json::from_str(&stdout_str(&app_get)).unwrap();
    assert_eq!(saved_app["power_preset"], "aggressive");
    assert_eq!(saved_app["default_depth"], 5);
    assert_eq!(saved_app["apparent_size"], true);
    assert_eq!(saved_app["include_hidden"], false);
    assert_eq!(saved_app["follow_system_appearance"], false);
    assert_eq!(saved_app["ssh_session_duration_minutes"], 30);

    // Watchdog config CRUD via WatchdogConfigService (zero keychain).
    let wd_set = sb.run(&[
        "config",
        "watchdog",
        "set",
        "--enabled",
        "true",
        "--background-service",
        "false",
        "--min-free-gb",
        "35",
        "--poll-interval-secs",
        "120",
        "--cooldown-secs",
        "600",
        "--json",
    ]);
    assert!(wd_set.status.success(), "{}", stderr_str(&wd_set));
    let wd_get = sb.run(&["config", "watchdog", "get", "--json"]);
    assert!(wd_get.status.success());
    let saved_wd: Value = serde_json::from_str(&stdout_str(&wd_get)).unwrap();
    assert_eq!(saved_wd["enabled"], true);
    assert_eq!(saved_wd["background_service"], false);
    assert_eq!(saved_wd["min_free_gb"], 35);
    assert_eq!(saved_wd["poll_interval_secs"], 120);
    assert_eq!(saved_wd["cooldown_secs"], 600);

    // Webhook config CRUD (redacts secret header values in CLI output).
    let wh_set = sb.run(&[
        "config",
        "webhook",
        "set",
        "--enabled",
        "true",
        "--url",
        "http://127.0.0.1:9999/hook",
        "--headers-json",
        "{\"X-Api-Key\":\"super-secret-header\"}",
        "--timeout-secs",
        "15",
        "--json",
    ]);
    assert!(wh_set.status.success(), "{}", stderr_str(&wh_set));
    assert!(!stdout_str(&wh_set).contains("super-secret-header"));
    let wh_get = sb.run(&["config", "webhook", "get", "--json"]);
    assert!(wh_get.status.success());
    assert!(!stdout_str(&wh_get).contains("super-secret-header"));
    let saved_wh: Value = serde_json::from_str(&stdout_str(&wh_get)).unwrap();
    assert_eq!(saved_wh["enabled"], true);
    assert_eq!(saved_wh["url"], "http://127.0.0.1:9999/hook");
    assert_eq!(saved_wh["headers_redacted"], true);
    assert_eq!(saved_wh["header_names"], json!(["X-Api-Key"]));
    assert_eq!(saved_wh["timeout_secs"], 15);

    // AI config CRUD.
    let ai_set = sb.run(&[
        "config",
        "ai",
        "set",
        "--auto-analyze",
        "false",
        "--provider",
        "custom",
        "--custom-command",
        "/tmp/my-ai",
        "--extra-args",
        "--fast",
        "--model",
        "gpt-5",
        "--json",
    ]);
    assert!(ai_set.status.success(), "{}", stderr_str(&ai_set));
    let ai_clear = sb.run(&["config", "ai", "set", "--clear-model", "--json"]);
    assert!(ai_clear.status.success());
    let saved_ai: Value = serde_json::from_str(&stdout_str(&ai_clear)).unwrap();
    assert_eq!(saved_ai["auto_analyze_on_alert"], false);
    assert_eq!(saved_ai["provider"], "custom");
    assert_eq!(saved_ai["custom_command"], "/tmp/my-ai");
    assert_eq!(saved_ai["extra_args"], "--fast");
    assert!(saved_ai["model"].is_null());

    // SSH profiles CRUD: list must work with zero keychain access even when
    // credentials exist on disk.
    let empty_list = sb.run(&["config", "ssh", "list", "--json"]);
    assert!(empty_list.status.success(), "{}", stderr_str(&empty_list));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout_str(&empty_list)).unwrap(),
        json!([])
    );

    let ssh_add = sb.run_with_keychain(&[
        "config",
        "ssh",
        "add",
        "--alias",
        "prod",
        "--host",
        "10.0.1.10",
        "--port",
        "2222",
        "--username",
        "deploy",
        "--auth-mode",
        "password",
        "--password",
        "card-pw-123",
        "--default-path",
        "/var/log",
        "--json",
    ]);
    assert!(ssh_add.status.success(), "{}", stderr_str(&ssh_add));
    assert!(!stdout_str(&ssh_add).contains("card-pw-123"));

    // Metadata listing and non-secret edit must NOT wake keychain!
    let list_after_add = sb.run(&["config", "ssh", "list", "--json"]);
    assert!(
        list_after_add.status.success(),
        "list must not touch keychain: {}",
        stderr_str(&list_after_add)
    );
    let listed: Value =
        serde_json::from_str(&stdout_str(&list_after_add)).unwrap();
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    assert_eq!(listed[0]["alias"], "prod");
    assert_eq!(listed[0]["default_path"], "/var/log");

    let ssh_edit = sb.run(&[
        "config",
        "ssh",
        "edit",
        "prod",
        "--alias",
        "prod-renamed",
        "--port",
        "2200",
        "--clear-default-path",
        "--json",
    ]);
    assert!(
        ssh_edit.status.success(),
        "metadata-only edit must not touch keychain: {}",
        stderr_str(&ssh_edit)
    );
    let edited: Value = serde_json::from_str(&stdout_str(&ssh_edit)).unwrap();
    assert_eq!(edited["alias"], "prod-renamed");
    assert_eq!(edited["port"], 2200);
    assert!(edited["default_path"].is_null());

    let ssh_rm = sb.run_with_keychain(&[
        "config",
        "ssh",
        "remove",
        "prod-renamed",
        "--json",
    ]);
    assert!(ssh_rm.status.success(), "{}", stderr_str(&ssh_rm));
    let final_list = sb.run(&["config", "ssh", "list", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout_str(&final_list)).unwrap(),
        json!([])
    );
}

#[test]
fn cli_watchdog_help_invalid_and_once_disabled() {
    let sb = Sandbox::new();

    for flag in ["-h", "--help"] {
        let out = sb.run(&["watchdog", flag]);
        assert!(
            out.status.success(),
            "watchdog {flag}: {}",
            stderr_str(&out)
        );
        let text = stdout_str(&out);
        assert!(text.contains("ruum watchdog"), "unexpected help: {text}");
        assert!(text.contains("once"), "missing once: {text}");
    }

    let bad = sb.run(&["watchdog", "--bogus-flag"]);
    assert!(!bad.status.success());
    assert!(stderr_str(&bad).contains("unknown option"));

    // Without config, watchdog defaults to disabled and writes ruum.db state.
    let out = sb.run(&["watchdog", "once"]);
    assert!(
        out.status.success(),
        "watchdog once disabled failed: {}",
        stderr_str(&out)
    );

    let db = StorageDb::open(sb.db_path()).expect("open sandbox ruum.db");
    let state = db.watchdog().unwrap().load_state().expect("load wd state");
    assert!(state.last_checked_secs.is_some());
    assert_eq!(state.last_triggered_secs, None);
    assert!(!sb.config_dir().join("watchdog-state.json").exists());
}

#[test]
fn cli_watchdog_once_breached_webhook_and_conditional_ai() {
    let space = ruum_core::space::space_info(&std::env::temp_dir())
        .expect("space_info");
    let avail_gb = space.available / (1024 * 1024 * 1024);
    let min_free_gb = avail_gb.saturating_add(100).max(20);
    let min_free_str = min_free_gb.to_string();

    // Case 1: auto_analyze_on_alert = false -> delivers generic Webhook JSON
    //         and MUST NOT invoke AI CLI.
    {
        let sb = Sandbox::new();
        #[cfg(unix)]
        {
            sb.write_unix_script("osascript", "#!/bin/sh\nexit 0\n");
            sb.write_unix_script("notify-send", "#!/bin/sh\nexit 0\n");
        }
        let marker = sb.root.path().join("ai_disabled.marker");
        let stub_ai = sb.write_stub_ai_cli(&marker);
        let stub_ai_str = stub_ai.to_string_lossy().into_owned();

        let (port, srv_handle) = spawn_mock_http_server();
        let hook_url = format!("http://127.0.0.1:{port}/alert");

        assert!(
            sb.run(&[
                "config",
                "watchdog",
                "set",
                "--enabled",
                "true",
                "--min-free-gb",
                &min_free_str,
                "--cooldown-secs",
                "3600",
            ])
            .status
            .success()
        );
        assert!(
            sb.run(&[
                "config",
                "webhook",
                "set",
                "--enabled",
                "true",
                "--url",
                &hook_url,
                "--headers-json",
                "{\"X-Ruum-Token\":\"tok-123\"}",
            ])
            .status
            .success()
        );
        assert!(
            sb.run(&[
                "config",
                "ai",
                "set",
                "--auto-analyze",
                "false",
                "--provider",
                "custom",
                "--custom-command",
                &stub_ai_str,
            ])
            .status
            .success()
        );

        let out = sb
            .cmd_with_sandbox_path()
            .args(["watchdog", "once"])
            .stdin(Stdio::null())
            .output()
            .expect("run watchdog once");
        assert!(
            out.status.success(),
            "watchdog once failed: {}",
            stderr_str(&out)
        );
        assert!(
            !marker.exists(),
            "AI CLI must not run when auto_analyze_on_alert=false"
        );

        let captured = srv_handle
            .join()
            .expect("join mock http")
            .expect("webhook request should arrive");
        assert!(
            captured.request_line.starts_with("POST /alert"),
            "req: {}",
            captured.request_line
        );
        assert!(
            captured
                .headers
                .iter()
                .any(|h| h.eq_ignore_ascii_case("x-ruum-token: tok-123")),
            "missing custom header in {:?}",
            captured.headers
        );
        let payload: Value =
            serde_json::from_str(&captured.body).expect("webhook json body");
        assert_eq!(payload["event"], "ruum.disk.low_space");
        assert!(payload["timestamp"].is_string());
        assert!(payload["host"]["platform"].is_string());
        assert!(payload["mount"]["available_bytes"].is_u64());
        assert!(payload["mount"]["threshold_bytes"].is_u64());
    }

    // Case 2: auto_analyze_on_alert = true -> delivers Webhook AND invokes AI.
    {
        let sb = Sandbox::new();
        #[cfg(unix)]
        {
            sb.write_unix_script("osascript", "#!/bin/sh\nexit 0\n");
            sb.write_unix_script("notify-send", "#!/bin/sh\nexit 0\n");
        }
        let marker = sb.root.path().join("ai_enabled.marker");
        let stub_ai = sb.write_stub_ai_cli(&marker);
        let stub_ai_str = stub_ai.to_string_lossy().into_owned();

        let (port, srv_handle) = spawn_mock_http_server();
        let hook_url = format!("http://127.0.0.1:{port}/alert");

        assert!(
            sb.run(&[
                "config",
                "watchdog",
                "set",
                "--enabled",
                "true",
                "--min-free-gb",
                &min_free_str,
                "--cooldown-secs",
                "3600",
            ])
            .status
            .success()
        );
        assert!(
            sb.run(&[
                "config",
                "webhook",
                "set",
                "--enabled",
                "true",
                "--url",
                &hook_url,
            ])
            .status
            .success()
        );
        assert!(
            sb.run(&[
                "config",
                "ai",
                "set",
                "--auto-analyze",
                "true",
                "--provider",
                "custom",
                "--custom-command",
                &stub_ai_str,
            ])
            .status
            .success()
        );

        let out = sb
            .cmd_with_sandbox_path()
            .args(["watchdog", "--once"])
            .stdin(Stdio::null())
            .output()
            .expect("run watchdog --once");
        assert!(
            out.status.success(),
            "watchdog --once breached failed: {}",
            stderr_str(&out)
        );
        assert!(
            marker.exists(),
            "stub AI CLI must execute when auto_analyze_on_alert=true"
        );
        assert!(
            srv_handle.join().expect("join mock http").is_some(),
            "webhook request expected"
        );

        let db = StorageDb::open(sb.db_path()).expect("open db");
        let wd_state = db.watchdog().unwrap().load_state().unwrap();
        assert!(wd_state.last_triggered_secs.is_some());
        assert_eq!(wd_state.last_triggered_threshold_gb, Some(min_free_gb));

        let ai_state = db.load_ai_state().expect("load ai_state");
        let proposal = ai_state
            .parse_proposal()
            .expect("parse proposal")
            .expect("proposal stored");
        assert_eq!(proposal.summary, "stub analysis ok");
    }
}

#[test]
fn cli_analyze_runs_ai_and_saves_ai_state() {
    let sb = Sandbox::new();
    let marker = sb.root.path().join("analyze.marker");
    let stub_ai = sb.write_stub_ai_cli(&marker);
    let stub_ai_str = stub_ai.to_string_lossy().into_owned();
    let root_str = sb.scan_root.to_string_lossy().into_owned();

    assert!(
        sb.run(&[
            "config",
            "ai",
            "set",
            "--provider",
            "custom",
            "--custom-command",
            &stub_ai_str,
        ])
        .status
        .success()
    );

    let out = sb.run(&["analyze", &root_str, "--json"]);
    assert!(
        out.status.success(),
        "ruum analyze failed: {}",
        stderr_str(&out)
    );
    assert!(marker.exists(), "stub AI must be invoked by ruum analyze");
    let proposal: Value =
        serde_json::from_str(&stdout_str(&out)).expect("analyze json");
    assert_eq!(proposal["summary"], "stub analysis ok");

    let db = StorageDb::open(sb.db_path()).expect("open db");
    let ai_state = db.load_ai_state().expect("load ai_state");
    assert!(
        ai_state
            .parse_proposal()
            .unwrap()
            .is_some_and(|p| p.summary == "stub analysis ok")
    );
}

#[test]
fn cli_ssh_askpass_token_verification() {
    let sb = Sandbox::new();
    let endpoint = SshEndpoint::new("127.0.0.1", 22, "tester").unwrap();
    let session = SshSession::start(
        endpoint,
        SshAuthMaterial::Password {
            password: Zeroizing::new("s3cr3t-askpass-pw".to_owned()),
        },
        5,
    )
    .expect("start password ssh session");

    let built = session.build_ssh_command(&[], "true").unwrap();
    let mut ep: Option<OsString> = None;
    let mut tok: Option<OsString> = None;
    for (k, v) in built.get_envs() {
        if k == ENV_ASKPASS_ENDPOINT {
            ep = v.map(ToOwned::to_owned);
        } else if k == ENV_ASKPASS_TOKEN {
            tok = v.map(ToOwned::to_owned);
        }
    }
    let ep = ep.expect("RUUM_ASKPASS_ENDPOINT env");
    let tok = tok.expect("RUUM_ASKPASS_TOKEN env");

    // 1. Valid token returns password on stdout.
    let ok = sb
        .cmd()
        .args(["ssh-askpass"])
        .env(ENV_ASKPASS_ENDPOINT, &ep)
        .env(ENV_ASKPASS_TOKEN, &tok)
        .stdin(Stdio::null())
        .output()
        .expect("run ssh-askpass");
    assert!(ok.status.success(), "{}", stderr_str(&ok));
    assert_eq!(
        stdout_str(&ok).trim_end_matches(['\r', '\n']),
        "s3cr3t-askpass-pw"
    );

    // 2. Wrong token fails closed with empty stdout.
    let bad_tok = sb
        .cmd()
        .args(["ssh-askpass"])
        .env(ENV_ASKPASS_ENDPOINT, &ep)
        .env(ENV_ASKPASS_TOKEN, "wrong-token-value")
        .stdin(Stdio::null())
        .output()
        .expect("run ssh-askpass wrong token");
    assert!(!bad_tok.status.success());
    assert!(stdout_str(&bad_tok).is_empty());

    // 3. Missing token fails closed.
    let no_tok = sb
        .cmd()
        .args(["ssh-askpass"])
        .env(ENV_ASKPASS_ENDPOINT, &ep)
        .stdin(Stdio::null())
        .output()
        .expect("run ssh-askpass missing token");
    assert!(!no_tok.status.success());
    assert!(stdout_str(&no_tok).is_empty());

    // 4. Extra positional argument rejected when invoked directly.
    let extra_arg = sb.run(&["ssh-askpass", "unexpected"]);
    assert!(!extra_arg.status.success());

    // 5. Real OpenSSH invocation: `$SSH_ASKPASS "<prompt>"` with
    //    `RUUM_ASKPASS_CMD=ssh-askpass` in the environment.
    let openssh_ok = sb
        .cmd()
        .arg("tester@127.0.0.1's password: ")
        .env(ENV_ASKPASS_CMD, "ssh-askpass")
        .env(ENV_ASKPASS_ENDPOINT, &ep)
        .env(ENV_ASKPASS_TOKEN, &tok)
        .stdin(Stdio::null())
        .output()
        .expect("run OpenSSH askpass mode");
    assert!(openssh_ok.status.success(), "{}", stderr_str(&openssh_ok));
    assert_eq!(
        stdout_str(&openssh_ok).trim_end_matches(['\r', '\n']),
        "s3cr3t-askpass-pw"
    );

    let openssh_bad_tok = sb
        .cmd()
        .arg("tester@127.0.0.1's password: ")
        .env(ENV_ASKPASS_CMD, "ssh-askpass")
        .env(ENV_ASKPASS_ENDPOINT, &ep)
        .env(ENV_ASKPASS_TOKEN, "wrong-token-value")
        .stdin(Stdio::null())
        .output()
        .expect("run OpenSSH askpass mode with wrong token");
    assert!(!openssh_bad_tok.status.success());
    assert!(stdout_str(&openssh_bad_tok).is_empty());
}

#[test]
fn cli_ssh_check_rejects_dangerous_hosts_and_parses_stdio_v1() {
    let sb = Sandbox::new();

    let missing_host = sb.run(&["ssh"]);
    assert!(!missing_host.status.success());

    for bad_host in ["-bad", "host;id", "user@host name", "bad`cmd`"] {
        let out = sb.run(&["ssh", bad_host, "--check"]);
        assert!(
            !out.status.success(),
            "host {bad_host:?} should be rejected"
        );
        assert!(
            stderr_str(&out).contains("invalid SSH destination"),
            "unexpected stderr for {bad_host:?}: {}",
            stderr_str(&out)
        );
    }

    #[cfg(unix)]
    {
        sb.write_unix_script(
            "ssh",
            r#"#!/bin/sh
host=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--" ]; then
    host="$arg"
    break
  fi
  prev="$arg"
done
IFS= read -r _req || true
if [ "$host" = "failhost" ]; then
  echo "Permission denied (publickey)." >&2
  exit 255
fi
printf '{"version":1,"root":"/remote/project","home":"/home/dev","disk_root":"/","space":{"total":10000,"free":4000,"available":4000},"platform":"linux","identity":{"hostname":"dev-host","machine_id":"0123456789abcdef0123456789abcdef"}}\n'
printf '["big.bin",1,4096,1,1700000000,false,0]\n'
printf '["project",0,0,0,1700000000,false,1]\n'
printf '"done"\n'
"#,
        );

        let ok = sb
            .cmd_with_sandbox_path()
            .args(["ssh", "dev", "--check", "/remote/project"])
            .stdin(Stdio::null())
            .output()
            .expect("run ssh --check");
        assert!(
            ok.status.success(),
            "ssh --check failed: {}",
            stderr_str(&ok)
        );
        let summary = stdout_str(&ok);
        assert!(
            summary.contains("dev:/remote/project"),
            "summary: {summary}"
        );
        assert!(summary.contains("4096"), "bytes missing: {summary}");
        assert!(summary.contains('1'), "file count missing: {summary}");

        // Also test saved SQLite SSH profile lookup via `ruum ssh <alias>`.
        assert!(
            sb.run_with_keychain(&[
                "config",
                "ssh",
                "add",
                "--alias",
                "prod-box",
                "--host",
                "10.0.1.9",
                "--port",
                "2222",
                "--username",
                "deploy",
                "--auth-mode",
                "password",
                "--password",
                "pw-check",
                "--default-path",
                "/remote/project",
            ])
            .status
            .success()
        );
        let prof_ok = sb
            .cmd_with_sandbox_path()
            .env("RUUM_TEST_ROOT_SECRET_HEX", VALID_TEST_ROOT_SECRET_HEX)
            .args(["ssh", "prod-box", "--check"])
            .stdin(Stdio::null())
            .output()
            .expect("run ssh profile --check");
        assert!(
            prof_ok.status.success(),
            "ssh profile --check failed: {}",
            stderr_str(&prof_ok)
        );
        assert!(stdout_str(&prof_ok).contains("prod-box:/remote/project"));

        let fail = sb
            .cmd_with_sandbox_path()
            .args(["ssh", "failhost", "--check", "/remote/project"])
            .stdin(Stdio::null())
            .output()
            .expect("run ssh failhost --check");
        assert!(!fail.status.success());
        assert!(
            stderr_str(&fail).contains("Permission denied"),
            "expected ssh diagnostics: {}",
            stderr_str(&fail)
        );
    }
}

#[test]
fn cli_extract_scanner_validation_and_missing_trailer() {
    let sb = Sandbox::new();

    let missing_all = sb.run(&["extract-scanner"]);
    assert!(!missing_all.status.success());
    assert!(
        stderr_str(&missing_all)
            .contains("usage: ruum extract-scanner ARCH OUTPUT")
    );

    let missing_out = sb.run(&["extract-scanner", "x86_64"]);
    assert!(!missing_out.status.success());
    assert!(
        stderr_str(&missing_out)
            .contains("usage: ruum extract-scanner ARCH OUTPUT")
    );

    let out_file = sb.tmp.join("ruum-scan-extracted");
    let out_str = out_file.to_string_lossy().into_owned();
    let no_trailer = sb.run(&["extract-scanner", "x86_64", &out_str]);
    assert!(!no_trailer.status.success());
    assert!(
        stderr_str(&no_trailer).contains("no embedded scanners"),
        "unexpected stderr: {}",
        stderr_str(&no_trailer)
    );
    assert!(!out_file.exists());
}

#[test]
fn cli_extract_scanner_reads_ruum_scanner_pack() {
    let sb = Sandbox::new();
    let mut elf = vec![0_u8; 120];
    elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
    elf[18..20].copy_from_slice(&62_u16.to_le_bytes());
    elf[32..40].copy_from_slice(&64_u64.to_le_bytes());
    elf[54..56].copy_from_slice(&56_u16.to_le_bytes());
    elf[56..58].copy_from_slice(&1_u16.to_le_bytes());

    let helper_src = sb.tmp.join("ruum-scan-x64");
    fs::write(&helper_src, &elf).unwrap();
    let pack_path = sb.tmp.join(ruum_core::payload::PACK_FILE_NAME);
    ruum_core::payload::write_pack(
        &pack_path,
        &[("x86_64".into(), helper_src)],
    )
    .unwrap();

    let extracted = sb.tmp.join("extracted-scan");
    let mut cmd = sb.cmd();
    cmd.env(ruum_core::payload::PACK_ENV_VAR, &pack_path);
    cmd.args([
        "extract-scanner",
        "x86_64",
        extracted.to_str().expect("utf8"),
    ]);
    let out = cmd.output().expect("run extract-scanner");
    assert!(out.status.success(), "{}", stderr_str(&out));
    assert_eq!(fs::read(&extracted).unwrap(), elf);
}

#[test]
fn cli_mcp_help_and_invalid_options() {
    let sb = Sandbox::new();

    for flag in ["-h", "--help"] {
        let out = sb.run(&["mcp", flag]);
        assert!(out.status.success(), "mcp {flag}: {}", stderr_str(&out));
        let text = stdout_str(&out);
        assert!(text.contains("ruum mcp"), "unexpected help: {text}");
        assert!(text.contains("--read-only"), "missing flag: {text}");
    }

    let bad = sb.run(&["mcp", "--bogus-flag"]);
    assert!(!bad.status.success());
    assert!(stderr_str(&bad).contains("unknown option"));
}

#[test]
fn cli_mcp_stdio_end_to_end_and_fail_closed() {
    let sb = Sandbox::new();
    let src_dir = sb.scan_root.join("src");
    let nm_dir = sb.scan_root.join("node_modules/pkg");
    let scratch_dir = sb.scan_root.join("scratch");
    fs::create_dir_all(&src_dir).expect("create src");
    fs::create_dir_all(&nm_dir).expect("create node_modules");
    fs::create_dir_all(&scratch_dir).expect("create scratch");

    fs::write(src_dir.join("main.rs"), "fn main() {}\n").expect("write main");
    fs::write(sb.scan_root.join("package.json"), "{}\n")
        .expect("write package.json");
    let big_bundle = fs::File::create(nm_dir.join("bundle.bin"))
        .expect("create node_modules bundle");
    big_bundle
        .set_len(65 * 1024 * 1024)
        .expect("set sparse length above 64 MiB reclaimable floor");
    let temp_log = scratch_dir.join("temp.log");
    fs::write(&temp_log, vec![b'y'; 2048]).expect("write temp.log");

    let root_str = sb.scan_root.to_string_lossy().into_owned();
    let temp_log_str = temp_log.to_string_lossy().into_owned();

    let requests = vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "list_volumes", "arguments": {} }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "scan_directory",
                "arguments": { "path": root_str, "apparent_size": true }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "inspect_subtree",
                "arguments": { "root": root_str, "subpath": "src" }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "find_reclaimable",
                "arguments": { "root": root_str }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "plan_removal",
                "arguments": { "root": root_str, "targets": [temp_log_str] }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "trash_paths",
                "arguments": { "root": root_str, "targets": [temp_log_str] }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {
                "name": "delete_permanently",
                "arguments": { "root": root_str, "targets": [temp_log_str] }
            }
        }),
    ];

    let (out, resps) = run_mcp_session(&sb, &[], &requests);
    assert!(out.status.success(), "mcp failed: {}", stderr_str(&out));
    // Notification must not produce a response line.
    assert_eq!(resps.len(), requests.len() - 1);

    assert_eq!(resps[0]["id"], 1);
    assert_eq!(resps[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(resps[0]["result"]["serverInfo"]["name"], "ruum");

    assert_eq!(resps[1]["id"], 2);
    let tools = resps[1]["result"]["tools"].as_array().expect("tools array");
    let tool_names: Vec<&str> =
        tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "list_ssh_profiles",
        "list_volumes",
        "scan_directory",
        "inspect_subtree",
        "find_reclaimable",
        "check_git_safety",
        "plan_removal",
        "trash_paths",
        "delete_permanently",
    ] {
        assert!(
            tool_names.contains(&expected),
            "missing tool {expected} in {tool_names:?}"
        );
    }

    let (err, volumes) = parse_tool_result(&resps[2]);
    assert!(!err, "list_volumes failed: {volumes}");
    assert!(volumes["volumes"].is_array());

    let (err, scanned) = parse_tool_result(&resps[3]);
    assert!(!err, "scan_directory failed: {scanned}");
    assert!(scanned["totals"]["files"].as_u64().unwrap_or(0) >= 3);
    assert!(scanned["totals"]["bytes"].as_u64().unwrap_or(0) > 0);

    let (err, subtree) = parse_tool_result(&resps[4]);
    assert!(!err, "inspect_subtree failed: {subtree}");
    assert_eq!(subtree["name"], "src");
    let entries = subtree["entries"].as_array().expect("subtree entries");
    assert!(entries.iter().any(|e| e["name"] == "main.rs"));

    let (err, reclaimable) = parse_tool_result(&resps[5]);
    assert!(!err, "find_reclaimable failed: {reclaimable}");
    let candidates = reclaimable["candidates"]
        .as_array()
        .expect("reclaimable candidates");
    assert!(
        candidates.iter().any(|c| c["name"] == "node_modules"),
        "expected node_modules in candidates: {candidates:?}"
    );

    let (err, plan) = parse_tool_result(&resps[6]);
    assert!(!err, "plan_removal failed: {plan}");
    assert_eq!(plan["targets"].as_array().map(Vec::len), Some(1));
    assert_eq!(plan["blocked"].as_array().map(Vec::len), Some(0));

    let (err, trash_res) = parse_tool_result(&resps[7]);
    assert!(err, "trash_paths must fail closed without GUI: {trash_res}");
    assert!(
        trash_res
            .as_str()
            .unwrap_or_default()
            .contains("graphical desktop"),
        "unexpected trash error: {trash_res}"
    );

    let (err, perm_res) = parse_tool_result(&resps[8]);
    assert!(
        err,
        "delete_permanently must fail closed without GUI: {perm_res}"
    );
    assert!(
        perm_res
            .as_str()
            .unwrap_or_default()
            .contains("graphical desktop"),
        "unexpected delete_permanently error: {perm_res}"
    );

    assert!(temp_log.exists(), "temp.log must not be deleted");

    // Verify --read-only across all platforms.
    let ro_requests = vec![
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/list",
            "params": {}
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": {
                "name": "trash_paths",
                "arguments": { "root": root_str, "targets": [temp_log_str] }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "tools/call",
            "params": {
                "name": "delete_permanently",
                "arguments": { "root": root_str, "targets": [temp_log_str] }
            }
        }),
    ];
    let (ro_out, ro_resps) =
        run_mcp_session(&sb, &["--read-only"], &ro_requests);
    assert!(
        ro_out.status.success(),
        "mcp --read-only: {}",
        stderr_str(&ro_out)
    );
    assert_eq!(ro_resps.len(), 3);

    let ro_tools = ro_resps[0]["result"]["tools"]
        .as_array()
        .expect("ro tools array");
    assert!(!ro_tools.iter().any(|t| matches!(
        t["name"].as_str(),
        Some("trash_paths" | "delete_permanently")
    )));

    let (err, ro_trash) = parse_tool_result(&ro_resps[1]);
    assert!(err);
    assert!(ro_trash.as_str().unwrap_or_default().contains("read-only"));

    let (err, ro_del) = parse_tool_result(&ro_resps[2]);
    assert!(err);
    assert!(ro_del.as_str().unwrap_or_default().contains("read-only"));
    assert!(temp_log.exists(), "temp.log must remain untouched");
}

#[test]
fn cli_mcp_remote_profiles_fail_closed_and_ipc_approved_flow() {
    let sb = Sandbox::new();
    assert!(
        sb.run_with_keychain(&[
            "config",
            "ssh",
            "add",
            "--alias",
            "prod",
            "--host",
            "10.0.1.25",
            "--port",
            "2222",
            "--username",
            "deploy",
            "--auth-mode",
            "password",
            "--password",
            "mcp-secret-pw",
            "--default-path",
            "/remote/project",
        ])
        .status
        .success()
    );

    let ssh_marker = sb.root.path().join("ssh_invoked.marker");
    #[cfg(unix)]
    {
        let ssh_trash_marker = sb.root.path().join("ssh_trash.marker");
        let script = format!(
            "#!/bin/sh\n\
             printf 'invoked\\n' > \"{}\"\n\
             last=\"\"\n\
             for a in \"$@\"; do last=\"$a\"; done\n\
             if [ \"$last\" = \"sh -s\" ]; then\n\
               cat >/dev/null\n\
               printf 'trashed\\n' > \"{}\"\n\
               exit 0\n\
             fi\n\
             IFS= read -r _req || true\n\
             printf '{{\"version\":1,\"root\":\"/remote/project\",\"home\":\"/home/deploy\",\"disk_root\":\"/\",\"space\":{{\"total\":20000,\"free\":8000,\"available\":8000}},\"platform\":\"linux\",\"identity\":{{\"hostname\":\"prod-host\",\"machine_id\":\"0123456789abcdef0123456789abcdef\"}}}}\\n'\n\
             printf '[\"old.log\",1,4096,1,1700000000,false,0]\\n'\n\
             printf '[\"project\",0,0,0,1700000000,false,1]\\n'\n\
             printf '\"done\"\\n'\n",
            ssh_marker.display(),
            ssh_trash_marker.display()
        );
        sb.write_unix_script("ssh", &script);
    }

    // 1. Without GUI confirmation, list_ssh_profiles succeeds with zero
    //    keychain/ssh access, while remote connection calls Fail-Closed.
    let unapproved_reqs = vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "list_ssh_profiles", "arguments": {} }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "list_volumes",
                "arguments": { "profile": "prod" }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "scan_directory",
                "arguments": { "ssh_host": "dev", "path": "/remote/project" }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "trash_paths",
                "arguments": {
                    "profile": "prod",
                    "root": "/remote/project",
                    "targets": ["/remote/project/old.log"]
                }
            }
        }),
    ];

    let (un_out, un_resps) = run_mcp_session_with_cmd(
        sb.cmd_with_sandbox_path(),
        &[],
        &unapproved_reqs,
    );
    assert!(un_out.status.success(), "{}", stderr_str(&un_out));
    assert_eq!(un_resps.len(), 5);

    let (err, list_res) = parse_tool_result(&un_resps[1]);
    assert!(!err, "list_ssh_profiles failed: {list_res}");
    let profiles = list_res["profiles"].as_array().expect("profiles");
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0]["alias"], "prod");
    assert_eq!(profiles[0]["host"], "10.0.1.25");
    assert_eq!(profiles[0]["port"], 2222);
    assert_eq!(profiles[0]["username"], "deploy");
    assert_eq!(profiles[0]["auth_mode"], "password");
    assert_eq!(profiles[0]["default_path"], "/remote/project");
    assert!(profiles[0].get("password").is_none());
    assert!(profiles[0].get("secret").is_none());

    for idx in [2, 3, 4] {
        let (is_err, payload) = parse_tool_result(&un_resps[idx]);
        assert!(is_err, "call {idx} must fail closed: {payload}");
    }
    assert!(
        !ssh_marker.exists(),
        "ssh must never be invoked before explicit user approval"
    );

    // 2. With simulated GUI IPC approval, remote profile scan + trash succeed.
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let token_path = sb.config_dir().join("ipc.token");
        let ipc_token =
            load_or_create_ipc_token(&token_path).expect("create ipc.token");
        let ipc_endpoint = sb.ipc_endpoint();
        let listener =
            IpcListenerGuard::bind(&ipc_endpoint).expect("bind ipc sock");
        let srv_token = ipc_token;
        let stop_ipc = Arc::new(AtomicBool::new(false));
        let stop_ipc_srv = Arc::clone(&stop_ipc);
        let handled = Arc::new(AtomicUsize::new(0));
        let handled_srv = Arc::clone(&handled);

        let ipc_thread = thread::spawn(move || {
            // Expect 1 remote connect confirmation; remote trash_paths is disabled.
            while !stop_ipc_srv.load(Ordering::Acquire)
                && handled_srv.load(Ordering::Acquire) < 1
            {
                let count_ref = &handled_srv;
                let stop_ref = &stop_ipc_srv;
                listener
                    .accept_and_handle(&srv_token, |req| {
                        count_ref.fetch_add(1, Ordering::AcqRel);
                        match req {
                            IpcRequest::ConfirmRemoteConnect {
                                request,
                                ..
                            } => {
                                assert_eq!(request.host, "10.0.1.25");
                                assert_eq!(
                                    request.alias.as_deref(),
                                    Some("prod")
                                );
                                Ok(IpcResponse::RemoteConnectDecision {
                                    decision:
                                        RemoteConnectConfirmDecision::Approved,
                                })
                            }
                            other => {
                                panic!("unexpected IPC request: {other:?}")
                            }
                        }
                    })
                    .expect("handle ipc request");
                if stop_ref.load(Ordering::Acquire) {
                    break;
                }
            }
        });

        let mut cmd = sb.cmd_with_sandbox_path();
        cmd.env("RUUM_TEST_ROOT_SECRET_HEX", VALID_TEST_ROOT_SECRET_HEX);
        let approved_reqs = vec![
            json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "initialize",
                "params": {}
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": {
                    "name": "scan_directory",
                    "arguments": {
                        "profile": "prod",
                        "path": "/remote/project"
                    }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 12,
                "method": "tools/call",
                "params": {
                    "name": "trash_paths",
                    "arguments": {
                        "profile": "prod",
                        "root": "/remote/project",
                        "targets": ["/remote/project/old.log"]
                    }
                }
            }),
        ];

        let (ok_out, ok_resps) =
            run_mcp_session_with_cmd(cmd, &[], &approved_reqs);
        stop_ipc.store(true, Ordering::Release);
        let _ = UnixStream::connect(&ipc_endpoint);
        ipc_thread.join().expect("join ipc thread");

        assert!(ok_out.status.success(), "{}", stderr_str(&ok_out));
        assert_eq!(ok_resps.len(), 3, "responses: {ok_resps:?}");

        let (scan_err, scan_payload) = parse_tool_result(&ok_resps[1]);
        assert!(!scan_err, "approved profile scan failed: {scan_payload}");
        assert_eq!(scan_payload["profile"], "prod");
        assert_eq!(scan_payload["totals"]["bytes"], 4096);

        let (trash_err, trash_payload) = parse_tool_result(&ok_resps[2]);
        assert!(
            trash_err,
            "remote trash_paths must disable background SSH deletion: {trash_payload}"
        );
        assert!(
            trash_payload
                .as_str()
                .unwrap_or_default()
                .contains("disabled"),
            "unexpected trash_paths payload: {trash_payload}"
        );
        assert_eq!(
            handled.load(Ordering::Acquire),
            1,
            "expected ConfirmRemoteConnect IPC call only"
        );
        assert!(
            !sb.root.path().join("ssh_trash.marker").exists(),
            "remote background trash must never execute via ssh"
        );
    }
}

#[test]
fn cli_space_reports_json_and_unreadable_directory_without_root_prompt() {
    let sb = Sandbox::new();

    let help = sb.run(&["space", "--help"]);
    assert!(help.status.success(), "{}", stderr_str(&help));
    assert!(stdout_str(&help).contains("ruum space"));

    let root_str = sb.scan_root.to_string_lossy().into_owned();
    let ok = sb.run(&["space", &root_str, "--json"]);
    assert!(
        ok.status.success(),
        "ruum space failed: {}",
        stderr_str(&ok)
    );
    let parsed: Value =
        serde_json::from_str(&stdout_str(&ok)).expect("space json");
    assert_eq!(parsed["readable"], true);
    assert!(parsed["total"].as_u64().unwrap_or(0) > 0);
    assert!(parsed["available"].is_u64());
    assert!(parsed["free"].is_u64());
    assert!(parsed["platform"].is_string());

    // `~` resolves against HOME cleanly.
    let tilde = sb.run(&["space", "~", "--json"]);
    assert!(
        tilde.status.success(),
        "ruum space ~ failed: {}",
        stderr_str(&tilde)
    );
    let tilde_json: Value =
        serde_json::from_str(&stdout_str(&tilde)).expect("space ~ json");
    assert_eq!(tilde_json["readable"], true);

    // Non-directory file fails cleanly.
    let file_path = sb.scan_root.join("plain.txt");
    fs::write(&file_path, b"hi").unwrap();
    let file_str = file_path.to_string_lossy().into_owned();
    assert!(!sb.run(&["space", &file_str]).status.success());

    // Unreadable directory still returns space stats with readable=false
    // and never tells the user root privileges are required.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let locked = sb.scan_root.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap();
        let locked_str = locked.to_string_lossy().into_owned();
        if fs::read_dir(&locked).is_err() {
            let space_locked = sb.run(&["space", &locked_str, "--json"]);
            assert!(
                space_locked.status.success(),
                "ruum space on unreadable dir must succeed: {}",
                stderr_str(&space_locked)
            );
            let locked_json: Value =
                serde_json::from_str(&stdout_str(&space_locked))
                    .expect("locked space json");
            assert_eq!(locked_json["readable"], false);
            assert!(locked_json["total"].as_u64().unwrap_or(0) > 0);
            assert!(
                !stderr_str(&space_locked)
                    .to_ascii_lowercase()
                    .contains("root privileges")
            );
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
            .unwrap();
    }
}

#[test]
fn cli_config_webhook_multi_endpoint_crud_and_redaction() {
    let sb = Sandbox::new();

    let initial = sb.run(&["config", "webhook", "list", "--json"]);
    assert!(initial.status.success(), "{}", stderr_str(&initial));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout_str(&initial)).unwrap(),
        json!([])
    );

    let add1 = sb.run(&[
        "config",
        "webhook",
        "add",
        "--name",
        "ops-1",
        "--url",
        "http://127.0.0.1:9991/h1",
        "--headers-json",
        "{\"Authorization\":\"Bearer secret-token-1\"}",
        "--timeout-secs",
        "12",
        "--json",
    ]);
    assert!(add1.status.success(), "{}", stderr_str(&add1));
    assert!(!stdout_str(&add1).contains("secret-token-1"));
    let ep1: Value = serde_json::from_str(&stdout_str(&add1)).unwrap();
    assert_eq!(ep1["name"], "ops-1");
    assert_eq!(ep1["enabled"], true);
    assert_eq!(ep1["headers_redacted"], true);
    assert_eq!(ep1["header_names"], json!(["Authorization"]));
    assert_eq!(ep1["timeout_secs"], 12);

    let add2 = sb.run(&[
        "config",
        "webhook",
        "add",
        "--name",
        "ops-2",
        "--url",
        "http://127.0.0.1:9992/h2",
        "--enabled",
        "false",
        "--json",
    ]);
    assert!(add2.status.success(), "{}", stderr_str(&add2));

    let listed = sb.run(&["config", "webhook", "list", "--json"]);
    assert!(listed.status.success());
    assert!(!stdout_str(&listed).contains("secret-token-1"));
    let list_json: Value = serde_json::from_str(&stdout_str(&listed)).unwrap();
    assert_eq!(list_json.as_array().map(Vec::len), Some(2));

    let edited = sb.run(&[
        "config",
        "webhook",
        "edit",
        "ops-2",
        "--enabled",
        "true",
        "--timeout-secs",
        "20",
        "--json",
    ]);
    assert!(edited.status.success(), "{}", stderr_str(&edited));
    let edit_json: Value = serde_json::from_str(&stdout_str(&edited)).unwrap();
    assert_eq!(edit_json["enabled"], true);
    assert_eq!(edit_json["timeout_secs"], 20);

    let removed = sb.run(&["config", "webhook", "remove", "ops-1", "--json"]);
    assert!(removed.status.success(), "{}", stderr_str(&removed));

    let after = sb.run(&["config", "webhook", "list", "--json"]);
    assert!(after.status.success());
    let after_json: Value = serde_json::from_str(&stdout_str(&after)).unwrap();
    assert_eq!(after_json.as_array().map(Vec::len), Some(1));
    assert_eq!(after_json[0]["name"], "ops-2");
}

#[test]
fn cli_watchdog_once_fans_out_to_multiple_webhooks() {
    let space = ruum_core::space::space_info(&std::env::temp_dir())
        .expect("space_info");
    let min_free_gb = (space.available / (1024 * 1024 * 1024))
        .saturating_add(100)
        .max(20);
    let min_free_str = min_free_gb.to_string();

    let sb = Sandbox::new();
    #[cfg(unix)]
    {
        sb.write_unix_script("osascript", "#!/bin/sh\nexit 0\n");
        sb.write_unix_script("notify-send", "#!/bin/sh\nexit 0\n");
    }

    let (port1, srv1) = spawn_mock_http_server();
    let (port2, srv2) = spawn_mock_http_server();
    let url1 = format!("http://127.0.0.1:{port1}/hook-one");
    let url2 = format!("http://127.0.0.1:{port2}/hook-two");

    assert!(
        sb.run(&[
            "config",
            "watchdog",
            "set",
            "--enabled",
            "true",
            "--min-free-gb",
            &min_free_str,
            "--cooldown-secs",
            "3600",
        ])
        .status
        .success()
    );
    assert!(
        sb.run(&["config", "ai", "set", "--auto-analyze", "false",])
            .status
            .success()
    );
    assert!(
        sb.run(&[
            "config",
            "webhook",
            "add",
            "--name",
            "hook-one",
            "--url",
            &url1,
            "--headers-json",
            "{\"X-Hook\":\"one\"}",
        ])
        .status
        .success()
    );
    assert!(
        sb.run(&[
            "config",
            "webhook",
            "add",
            "--name",
            "hook-two",
            "--url",
            &url2,
            "--headers-json",
            "{\"X-Hook\":\"two\"}",
        ])
        .status
        .success()
    );
    assert!(
        sb.run(&[
            "config",
            "webhook",
            "add",
            "--name",
            "hook-disabled",
            "--url",
            "http://127.0.0.1:1/never",
            "--enabled",
            "false",
        ])
        .status
        .success()
    );

    let out = sb
        .cmd_with_sandbox_path()
        .args(["watchdog", "once"])
        .stdin(Stdio::null())
        .output()
        .expect("run watchdog once");
    assert!(
        out.status.success(),
        "watchdog once multi-webhook failed: {}",
        stderr_str(&out)
    );

    let req1 = srv1.join().unwrap().expect("hook-one should receive POST");
    let req2 = srv2.join().unwrap().expect("hook-two should receive POST");
    assert!(req1.request_line.starts_with("POST /hook-one "));
    assert!(req2.request_line.starts_with("POST /hook-two "));
    assert!(
        req1.headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case("x-hook: one"))
    );
    assert!(
        req2.headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case("x-hook: two"))
    );
}

#[test]
fn cli_ssh_defaults_and_preserves_tilde_default_path() {
    let sb = Sandbox::new();

    let added = sb.run_with_keychain(&[
        "config",
        "ssh",
        "add",
        "--alias",
        "tilde-box",
        "--host",
        "10.0.2.15",
        "--username",
        "ubuntu",
        "--auth-mode",
        "password",
        "--password",
        "pw-tilde",
        "--json",
    ]);
    assert!(added.status.success(), "{}", stderr_str(&added));
    let added_json: Value = serde_json::from_str(&stdout_str(&added)).unwrap();
    assert_eq!(added_json["default_path"], "~");

    let edited = sb.run(&[
        "config",
        "ssh",
        "edit",
        "tilde-box",
        "--default-path",
        "~/workspace",
        "--json",
    ]);
    assert!(edited.status.success(), "{}", stderr_str(&edited));
    let edited_json: Value =
        serde_json::from_str(&stdout_str(&edited)).unwrap();
    assert_eq!(edited_json["default_path"], "~/workspace");

    #[cfg(unix)]
    {
        let req_capture = sb.root.path().join("captured_ssh_req.json");
        let script = format!(
            "#!/bin/sh\n\
             IFS= read -r req || true\n\
             printf '%s\\n' \"$req\" > \"{}\"\n\
             printf '{{\"version\":1,\"root\":\"/home/ubuntu/workspace\",\"home\":\"/home/ubuntu\",\"disk_root\":\"/\",\"space\":{{\"total\":10000,\"free\":4000,\"available\":4000}},\"platform\":\"linux\",\"identity\":{{\"hostname\":\"ubuntu-box\",\"machine_id\":\"0123456789abcdef0123456789abcdef\"}}}}\\n'\n\
             printf '[\"workspace\",0,0,0,1700000000,false,0]\\n'\n\
             printf '\"done\"\\n'\n",
            req_capture.display()
        );
        sb.write_unix_script("ssh", &script);

        let check = sb
            .cmd_with_sandbox_path()
            .env("RUUM_TEST_ROOT_SECRET_HEX", VALID_TEST_ROOT_SECRET_HEX)
            .args(["ssh", "tilde-box", "--check"])
            .stdin(Stdio::null())
            .output()
            .expect("run ssh tilde-box --check");
        assert!(
            check.status.success(),
            "ssh tilde-box failed: {}",
            stderr_str(&check)
        );
        let captured: Value = serde_json::from_str(
            &fs::read_to_string(&req_capture).expect("read captured ssh req"),
        )
        .expect("parse captured ssh req");
        assert_eq!(captured["root"], "~/workspace");
    }
}

#[test]
fn cli_run_script_bridges_ssh_session_pipes_payload_and_clears_pending() {
    use ruum_core::export::{
        RemoteCleanupScriptSpec, build_remote_cleanup_script,
        write_cleanup_script_atomic,
    };
    use ruum_core::identity::HostIdentity;
    use ruum_core::remote::Header;
    use ruum_core::removal::Target;

    let sb = Sandbox::new();
    let added = sb.run_with_keychain(&[
        "config",
        "ssh",
        "add",
        "--alias",
        "ops-box",
        "--host",
        "10.0.3.20",
        "--username",
        "deploy",
        "--auth-mode",
        "password",
        "--password",
        "super-secret-pw",
        "--json",
    ]);
    assert!(added.status.success(), "{}", stderr_str(&added));
    let profile: Value = serde_json::from_str(&stdout_str(&added)).unwrap();
    let profile_id = profile["id"].as_str().unwrap().to_owned();

    let header = Header::new(
        PathBuf::from("/home/deploy"),
        Some(PathBuf::from("/home/deploy")),
        "linux",
        Some(HostIdentity {
            hostname: "ops-box".into(),
            machine_id: Some("0123456789abcdef0123456789abcdef".into()),
        }),
    );
    let script_text = build_remote_cleanup_script(&RemoteCleanupScriptSpec {
        profile_alias: "ops-box".into(),
        host: "10.0.3.20".into(),
        port: 22,
        username: "deploy".into(),
        header,
        targets: vec![Target {
            path: PathBuf::from("/home/deploy/.cache/pip"),
            bytes: 4096,
            is_dir: true,
            hidden: false,
        }],
        docker_targets: Vec::new(),
    })
    .expect("build remote cleanup script");
    assert!(!script_text.contains("super-secret-pw"));

    let script_path = write_cleanup_script_atomic(
        &sb.config_dir(),
        "ops-box",
        1_700_000_123,
        &script_text,
    )
    .expect("write cleanup script");

    let db = StorageDb::open(sb.db_path()).expect("open db");
    db.set_ssh_profile_pending_script(
        &profile_id,
        &script_path,
        1,
        4096,
        1_700_000_123,
    )
    .expect("set pending script");

    // 1. Declining [y/N] aborts without touching SSH and keeps pending script.
    let mut abort_child = sb
        .cmd_with_test_keychain()
        .args([
            "run-script",
            script_path.to_str().unwrap(),
            "--profile",
            &profile_id,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aborted run-script");
    abort_child.stdin.take().unwrap().write_all(b"n\n").unwrap();
    let abort_out = abort_child.wait_with_output().unwrap();
    assert!(!abort_out.status.success());
    let still_pending = db.get_ssh_profile_by_id(&profile_id).unwrap().unwrap();
    assert_eq!(
        still_pending.pending_script_path.as_deref(),
        Some(script_path.to_str().unwrap())
    );

    #[cfg(unix)]
    {
        // 2. Non-zero SSH exit fails and preserves pending script.
        let fail_capture = sb.root.path().join("fail_ssh.txt");
        let fail_ssh = format!(
            "#!/bin/sh\ncat > \"{}\"\nexit 42\n",
            fail_capture.display()
        );
        sb.write_unix_script("ssh", &fail_ssh);

        let fail_out = sb
            .cmd_with_sandbox_path()
            .env("RUUM_TEST_ROOT_SECRET_HEX", VALID_TEST_ROOT_SECRET_HEX)
            .args([
                "run-script",
                script_path.to_str().unwrap(),
                "--profile",
                "ops-box",
                "--yes",
            ])
            .stdin(Stdio::null())
            .output()
            .expect("run-script fail");
        assert!(!fail_out.status.success());
        let after_fail =
            db.get_ssh_profile_by_id(&profile_id).unwrap().unwrap();
        assert!(after_fail.pending_script_path.is_some());

        // 3. Confirming interactively with --pause pipes payload, streams
        //    output, waits for Enter, and clears pending_script_* on exit 0.
        let ok_capture = sb.root.path().join("ok_ssh_payload.sh");
        let ok_ssh = format!(
            "#!/bin/sh\n\
             cat > \"{}\"\n\
             printf 'REMOTE_CLEANUP_OK askpass=%s\\n' \"${{SSH_ASKPASS:-}}\"\n",
            ok_capture.display()
        );
        sb.write_unix_script("ssh", &ok_ssh);

        let mut ok_child = sb
            .cmd_with_sandbox_path()
            .env("RUUM_TEST_ROOT_SECRET_HEX", VALID_TEST_ROOT_SECRET_HEX)
            .args([
                "run-script",
                script_path.to_str().unwrap(),
                "--profile",
                &profile_id,
                "--pause",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn ok run-script");
        ok_child.stdin.take().unwrap().write_all(b"y\n\n").unwrap();
        let ok_out = ok_child.wait_with_output().unwrap();
        assert!(
            ok_out.status.success(),
            "run-script failed: {}",
            stderr_str(&ok_out)
        );
        let stdout_text = stdout_str(&ok_out);
        assert!(stdout_text.contains("Proceed with cleanup? [y/N]"));
        assert!(stdout_text.contains("REMOTE_CLEANUP_OK askpass="));
        assert!(stdout_text.contains("Press Enter to close..."));

        let captured_payload =
            fs::read_to_string(&ok_capture).expect("read piped payload");
        assert!(captured_payload.contains("/home/deploy/.cache/pip"));
        assert!(!captured_payload.contains("RUUM_REMOTE_PAYLOAD_BEGIN"));
        assert!(!captured_payload.contains("super-secret-pw"));

        let cleared = db.get_ssh_profile_by_id(&profile_id).unwrap().unwrap();
        assert_eq!(cleared.pending_script_path, None);
        assert_eq!(cleared.pending_script_items, None);
        assert_eq!(cleared.pending_reclaim_bytes, None);
        assert_eq!(cleared.pending_created_secs, None);
    }
}
