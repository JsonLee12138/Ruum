//! Disposable-fixture profiling; no default path and no SSH or deployment.
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write as _};
use std::path::Path;
use std::time::Instant;

use ruum_core::remote::{read_snapshot, serve};
use ruum_core::scan::{ScanOptions, scan};

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map_or("", String::as_str);
    let path = Path::new(args.get(2).ok_or_else(|| {
        io::Error::other(
            "usage: profile_remote generate|decode|stream|scan PATH",
        )
    })?);
    if !path.starts_with(std::env::temp_dir()) {
        return Err(io::Error::other(
            "profiling requires a disposable temp path",
        ));
    }
    let started = Instant::now();
    match mode {
        "generate" => {
            let mut out = BufWriter::new(File::create_new(path)?);
            let count = args
                .get(3)
                .and_then(|s| s.parse().ok())
                .unwrap_or(500_000_u64);
            writeln!(
                out,
                "{}",
                serde_json::json!({
                    "version": 1, "root": "/tmp/profile-fixture",
                    "home": null, "disk_root": null, "space": null,
                    "platform": "linux", "identity": null,
                })
            )?;
            for index in 0..count {
                writeln!(out, "[\"file-{index}\",1,4096,1,0,false,0]")?;
            }
            writeln!(out, "[\"root\",0,0,0,0,false,{count}]\n\"done\"")?;
            out.flush()?;
        }
        "decode" => {
            let snapshot =
                read_snapshot(&mut BufReader::new(File::open(path)?))?;
            eprintln!(
                "files={} bytes={}",
                snapshot.tree.files, snapshot.tree.bytes
            );
        }
        "scan" => {
            let tree = scan(path, ScanOptions::default())?;
            eprintln!("files={} bytes={}", tree.files, tree.bytes);
        }
        "ssh" | "ssh-plain" => {
            use std::process::{Command, Stdio};
            let config = args.get(3).ok_or_else(|| {
                io::Error::other("missing fixture SSH config")
            })?;
            let helper = args
                .get(4)
                .ok_or_else(|| io::Error::other("missing helper path"))?;
            if !Path::new(config).starts_with(std::env::temp_dir()) {
                return Err(io::Error::other(
                    "only isolated fixture SSH configs",
                ));
            }
            let mut command = Command::new("ssh");
            command.args(["-F", config, "-T"]);
            if mode == "ssh" {
                command.arg("-C");
            }
            let mut child = command
                .args(["fixture", helper, "--stdio-v1"])
                .stdin(std::process::Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;
            let mut input = child.stdin.take().expect("piped stdin");
            writeln!(
                input,
                "{}",
                serde_json::json!({
                    "root": path, "disk": false, "options": ScanOptions::default(),
                    "live_progress": true, "client_dedup": true,
                })
            )?;
            drop(input);
            let snapshot = read_snapshot(&mut BufReader::new(
                child.stdout.take().expect("piped stdout"),
            ))?;
            if !child.wait()?.success() {
                return Err(io::Error::other("fixture SSH failed"));
            }
            eprintln!("files={}", snapshot.tree.files);
        }
        "stream" => {
            let request = serde_json::to_vec(&serde_json::json!({
                "root": path, "disk": false, "options": ScanOptions::default(),
                "live_progress": true, "client_dedup": true,
            }))?;
            let mut input = request;
            input.push(b'\n');
            let mut output = BufWriter::new(io::stdout().lock());
            serve(&mut input.as_slice(), &mut output)?;
        }
        _ => return Err(io::Error::other("unknown profile mode")),
    }
    eprintln!("{mode} elapsed={:.3}s", started.elapsed().as_secs_f64());
    Ok(())
}
