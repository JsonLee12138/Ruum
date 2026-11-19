//! Exercise the shipped executable, not just its protocol functions.
use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Command, Stdio};

use ruum_core::remote::{Request, read_snapshot};
use ruum_core::scan::ScanOptions;

#[test]
fn executable_returns_a_read_only_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data");
    std::fs::write(&file, b"hello").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ruum-scan"))
        .arg("--stdio-v1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let request = Request {
        root: Some(dir.path().into()),
        disk: false,
        options: ScanOptions {
            apparent_size: true,
            ..ScanOptions::default()
        },
    };
    let mut input = child.stdin.take().unwrap();
    serde_json::to_writer(&mut input, &request).unwrap();
    input.write_all(b"\n").unwrap();
    drop(input);
    let snapshot =
        read_snapshot(&mut BufReader::new(child.stdout.take().unwrap()))
            .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(snapshot.tree.bytes, 5);
    assert_eq!(snapshot.tree.files, 1);
    assert_eq!(snapshot.header.root, dir.path().canonicalize().unwrap());
    assert_eq!(std::fs::read(file).unwrap(), b"hello");
}

// Every failure path still kills and reaps the child.
struct Helper(std::process::Child);

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn supervised_helper() -> Helper {
    Helper(
        Command::new(env!("CARGO_BIN_EXE_ruum-scan"))
            .arg("--stdio-v1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

fn wait_bounded(child: &mut std::process::Child) {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "helper outlived its client"
        );
        std::thread::yield_now();
    }
}

#[test]
fn disconnect_stops_a_helper_even_when_its_output_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    // More records than a pipe can hold, without a large or expensive scan.
    for index in 0..2000 {
        std::fs::write(dir.path().join(format!("entry-{index:04}")), [])
            .unwrap();
    }
    let mut helper = supervised_helper();
    let mut input = helper.0.stdin.take().unwrap();
    let request = serde_json::json!({
        "root": dir.path(), "disk": false,
        "options": ScanOptions::default(), "watch_disconnect": true,
    });
    serde_json::to_writer(&mut input, &request).unwrap();
    input.write_all(b"\n").unwrap();
    let mut output = BufReader::new(helper.0.stdout.take().unwrap());
    let mut header = String::new();
    output.read_line(&mut header).unwrap();
    assert!(header.contains("\"version\""));
    // Leave stdout open and unread: only the stdin watchdog can stop it.
    drop(input);
    wait_bounded(&mut helper.0);
}

#[test]
fn a_live_supervised_client_receives_a_complete_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("data"), b"hello").unwrap();
    let mut helper = supervised_helper();
    let mut input = helper.0.stdin.take().unwrap();
    let request = serde_json::json!({
        "root": dir.path(), "disk": false,
        "options": ScanOptions::default(), "watch_disconnect": true,
    });
    serde_json::to_writer(&mut input, &request).unwrap();
    input.write_all(b"\n").unwrap();
    let snapshot =
        read_snapshot(&mut BufReader::new(helper.0.stdout.take().unwrap()))
            .unwrap();
    assert_eq!(snapshot.tree.files, 1);
    assert!(helper.0.wait().unwrap().success());
    drop(input);
}

const MAX_ADDRESS_SPACE: u64 = 1 << 30;

fn limit_pair<'a>(
    limits: &'a str,
    prefix: &str,
    soft_index: usize,
) -> Option<(&'a str, &'a str)> {
    let line = limits.lines().find(|line| line.starts_with(prefix))?;
    let mut fields = line.split_whitespace();
    Some((fields.nth(soft_index)?, fields.next()?))
}

fn limits_installed(limits: &str) -> bool {
    let within_cap = |value: &str| {
        value
            .parse::<u64>()
            .is_ok_and(|bytes| bytes <= MAX_ADDRESS_SPACE)
    };
    let address_capped = limit_pair(limits, "Max address space", 3)
        .is_some_and(|(soft, hard)| within_cap(soft) && within_cap(hard));
    let core_zeroed = limit_pair(limits, "Max core file size", 4)
        .is_some_and(|(soft, hard)| soft == "0" && hard == "0");
    address_capped && core_zeroed
}

fn proc_limits_fixture(
    core_soft: &str,
    core_hard: &str,
    as_soft: &str,
    as_hard: &str,
) -> String {
    format!(
        "Limit                     Soft Limit           Hard Limit           \
         Units\n\
         Max core file size        {core_soft:<20} {core_hard:<20} bytes\n\
         Max address space         {as_soft:<20} {as_hard:<20} bytes\n"
    )
}

#[test]
fn limits_wait_for_both_address_space_and_core_hard_limit() {
    let inherited =
        proc_limits_fixture("0", "unlimited", "unlimited", "unlimited");
    assert!(!limits_installed(&inherited));

    let mid_syscall =
        proc_limits_fixture("0", "unlimited", "1073741824", "1073741824");
    assert!(!limits_installed(&mid_syscall));

    let soft_only = proc_limits_fixture("0", "0", "1073741824", "unlimited");
    assert!(!limits_installed(&soft_only));

    let core_soft_nonzero =
        proc_limits_fixture("1", "0", "1073741824", "1073741824");
    assert!(!limits_installed(&core_soft_nonzero));

    let over_address_cap =
        proc_limits_fixture("0", "0", "1073741825", "1073741825");
    assert!(!limits_installed(&over_address_cap));

    let installed = proc_limits_fixture("0", "0", "1073741824", "1073741824");
    assert!(limits_installed(&installed));
}

#[cfg(target_os = "linux")]
#[test]
fn memory_and_core_limits_are_installed_before_reading_a_request() {
    let mut helper = supervised_helper();
    // No startup chatter on stderr: wait for the kernel limit itself
    // while the helper is still blocked waiting for its first request.
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let limits =
            std::fs::read_to_string(format!("/proc/{}/limits", helper.0.id()))
                .unwrap();
        if limits_installed(&limits) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{limits}");
        assert!(helper.0.try_wait().unwrap().is_none());
        std::thread::yield_now();
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "creates 100,000 local empty files to measure helper RSS"]
fn wide_stream_has_bounded_resident_memory() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..100_000 {
        std::fs::File::create(dir.path().join(format!("file-{index:06}")))
            .unwrap();
    }
    assert_bounded_stream_memory(dir.path());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "creates 70,000 hardlinks to inodes outside the scanned fixture"]
fn external_hardlinks_have_bounded_resident_memory() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    for index in 0..70_000 {
        let file = outside.join(format!("file-{index}"));
        std::fs::File::create(&file).unwrap();
        std::fs::hard_link(&file, root.join(format!("file-{index}"))).unwrap();
    }
    assert_bounded_stream_memory(&root);
}

#[cfg(target_os = "linux")]
fn assert_bounded_stream_memory(root: &std::path::Path) {
    let mut helper = supervised_helper();
    let mut input = helper.0.stdin.take().unwrap();
    serde_json::to_writer(
        &mut input,
        &serde_json::json!({
            "root": root, "disk": false,
            "options": ScanOptions::default(), "watch_disconnect": true,
            "live_progress": true, "client_dedup": true, "live_tree": true,
        }),
    )
    .unwrap();
    input.write_all(b"\n").unwrap();
    let mut output = BufReader::new(helper.0.stdout.take().unwrap());
    let mut line = String::new();
    for sample in 0..2 {
        for _ in 0..20_000 {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
        }
        // Backpressure keeps the helper alive here; samples do not retain
        // the received records or run any expensive remote filesystem.
        let status =
            std::fs::read_to_string(format!("/proc/{}/status", helper.0.id()))
                .unwrap();
        let rss = status
            .lines()
            .find(|line| line.starts_with("VmRSS:"))
            .unwrap();
        let kib = rss
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<u64>()
            .unwrap();
        eprintln!(
            "helper RSS after {} records: {kib} KiB",
            (sample + 1) * 20_000
        );
        assert!(kib < 16 * 1024, "{rss}");
    }
    std::io::copy(&mut output, &mut std::io::sink()).unwrap();
    assert!(helper.0.wait().unwrap().success());
    drop(input);
}

#[test]
fn executable_discovers_roots_without_scanning_a_path() {
    let mut helper = supervised_helper();
    let mut input = helper.0.stdin.take().unwrap();
    input.write_all(b"{\"discover_version\":1,\"watch_disconnect\":true,\"root\":\"/missing-ruum-discovery-root\"}\n").unwrap();
    let roots = ruum_core::remote::read_roots(&mut BufReader::new(
        helper.0.stdout.take().unwrap(),
    ))
    .unwrap();
    assert!(roots.volumes.iter().all(|volume| volume.space.is_none()));
    assert!(helper.0.wait().unwrap().success());
    drop(input);
}

#[cfg(unix)]
#[test]
fn docker_probe_degrades_gracefully_and_streams_available_usage_through_stdio()
{
    use std::os::unix::fs::PermissionsExt as _;

    use ruum_core::docker_adapter::DockerProbeStatus;

    let scan_dir = tempfile::tempdir().unwrap();
    std::fs::write(scan_dir.path().join("file.txt"), b"abc").unwrap();

    // 1. Simulated Go runtime OOM failure (as can happen under Linux 1 GiB
    //    RLIMIT_AS): helper must degrade gracefully and finish the scan.
    let oom_bin_dir = tempfile::tempdir().unwrap();
    let oom_docker = oom_bin_dir.path().join("docker");
    std::fs::write(
        &oom_docker,
        "#!/bin/sh\necho 'fatal error: out of memory' >&2\nexit 2\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &oom_docker,
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let prepend_path = |dir: &std::path::Path| {
        let mut entries = vec![dir.to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            entries.extend(std::env::split_paths(&existing));
        }
        std::env::join_paths(entries).unwrap()
    };

    let mut oom_child = Command::new(env!("CARGO_BIN_EXE_ruum-scan"))
        .arg("--stdio-v1")
        .env("PATH", prepend_path(oom_bin_dir.path()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let req = Request {
        root: Some(scan_dir.path().to_path_buf()),
        disk: false,
        options: ScanOptions {
            apparent_size: true,
            ..ScanOptions::default()
        },
    };
    let mut stdin = oom_child.stdin.take().unwrap();
    serde_json::to_writer(&mut stdin, &req).unwrap();
    stdin.write_all(b"\n").unwrap();
    drop(stdin);
    let oom_snap =
        read_snapshot(&mut BufReader::new(oom_child.stdout.take().unwrap()))
            .unwrap();
    assert!(oom_child.wait().unwrap().success());
    assert_eq!(oom_snap.tree.files, 1);
    assert_eq!(oom_snap.docker, DockerProbeStatus::DaemonStopped);
    assert_eq!(oom_snap.header.docker, DockerProbeStatus::DaemonStopped);

    // 2. Healthy docker CLI returning `-v` JSON populates `snapshot.docker`
    //    and `snapshot.header.docker` identically.
    let ok_bin_dir = tempfile::tempdir().unwrap();
    let ok_docker = ok_bin_dir.path().join("docker");
    std::fs::write(
        &ok_docker,
        r#"#!/bin/sh
cat <<'EOF'
{"Images":[{"Repository":"<none>","Tag":"<none>","ID":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","CreatedSince":"2d","Size":"50MB","UniqueSize":"50MB","Containers":"0"}],"Containers":[{"ID":"111122223333","Names":"stopped-job","Image":"alpine:latest","State":"exited","Status":"Exited (0)","Size":"10MB"}],"Volumes":[],"BuildCache":[{"ID":"bc1","InUse":"false","Size":"20MB"}]}
EOF
"#,
    )
    .unwrap();
    std::fs::set_permissions(
        &ok_docker,
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let mut ok_child = Command::new(env!("CARGO_BIN_EXE_ruum-scan"))
        .arg("--stdio-v1")
        .env("PATH", prepend_path(ok_bin_dir.path()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ok_stdin = ok_child.stdin.take().unwrap();
    serde_json::to_writer(&mut ok_stdin, &req).unwrap();
    ok_stdin.write_all(b"\n").unwrap();
    drop(ok_stdin);
    let ok_snap =
        read_snapshot(&mut BufReader::new(ok_child.stdout.take().unwrap()))
            .unwrap();
    assert!(ok_child.wait().unwrap().success());
    assert_eq!(ok_snap.docker, ok_snap.header.docker);
    let DockerProbeStatus::Available(summary) = ok_snap.docker else {
        panic!(
            "expected Available docker summary, got {:?}",
            ok_snap.docker
        );
    };
    assert_eq!(summary.removable_images.len(), 1);
    assert_eq!(summary.stopped_containers.len(), 1);
    assert_eq!(summary.build_cache_reclaimable_bytes, 20_000_000);
}
