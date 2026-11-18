//! Headless helper: no GPUI dependency and no destructive command surface.
use std::io::{self, BufReader, BufWriter, Read as _};
use std::time::{Duration, Instant};

const SCAN_DEADLINE: Duration = Duration::from_mins(5);

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--stdio-v1"] {
        limit_resources()?;
        // A live client is not permission to monopolise the server forever,
        // including when a filesystem syscall cannot be cancelled.
        let deadline = Instant::now() + SCAN_DEADLINE;
        std::thread::Builder::new()
            .name("scan-deadline".into())
            .spawn(move || {
                wait_until_deadline(deadline, Instant::now, std::thread::sleep);
                eprintln!(
                    "Remote scan exceeded five minutes; scan a smaller path."
                );
                std::process::exit(1);
            })?;
        let mut input = BufReader::new(io::stdin());
        let session = ruum_core::remote::read_request(&mut input)?;
        if session.watch_disconnect {
            std::thread::Builder::new()
                .name("scan-connection".into())
                .spawn(move || {
                    let mut byte = [0];
                    loop {
                        match input.read(&mut byte) {
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                            Ok(0) => {
                                // Exit the process, not just a scan task: a
                                // blocked filesystem must not outlive SSH.
                                std::process::exit(0);
                            }
                            _ => {
                                eprintln!("Remote scan connection closed or sent unexpected input.");
                                std::process::exit(1);
                            }
                        }
                    }
                })?;
        }
        ruum_core::remote::serve_session(
            session,
            &mut BufWriter::new(io::stdout().lock()),
        )
    } else if args.is_empty() || args == ["--help"] || args == ["-h"] {
        println!(
            "ruum-scan — read-only SSH scan helper\n\
                  usage: ruum-scan --stdio-v1\n\
                  Used by: ruum ssh HOST [PATH]\n\
                  Reads a versioned JSON request from stdin and streams a \
                  snapshot to stdout. No deletion commands."
        );
        Ok(())
    } else {
        Err(io::Error::other("unknown option; try --help"))
    }
}

/// Kernel-enforced, not a sampled RSS check that can miss an allocation
/// burst. Keep this in the helper: never constrain the desktop process.
#[cfg(target_os = "linux")]
fn limit_resources() -> io::Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

    const MAX_ADDRESS_SPACE: u64 = 1 << 30;
    let inherited = getrlimit(Resource::As);
    let maximum = inherited.maximum.unwrap_or(u64::MAX).min(MAX_ADDRESS_SPACE);
    let current = inherited.current.unwrap_or(u64::MAX).min(maximum);
    setrlimit(
        Resource::As,
        Rlimit {
            current: Some(current),
            maximum: Some(maximum),
        },
    )?;
    // An allocation failure must not write a potentially huge core dump.
    setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps, reason = "same fallible API as Linux")]
const fn limit_resources() -> io::Result<()> {
    Ok(())
}

/// Wait until `deadline` is reached, looping across any early wakeups.
fn wait_until_deadline<N, W>(deadline: Instant, mut now: N, mut wait: W)
where
    N: FnMut() -> Instant,
    W: FnMut(Duration),
{
    loop {
        let current = now();
        if current >= deadline {
            break;
        }
        wait(deadline.saturating_duration_since(current));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn deadline_wait_ignores_spurious_early_wakeups() {
        let base = Instant::now();
        let deadline = base + SCAN_DEADLINE;
        let clock = Cell::new(base);
        let wake_offsets = [
            Duration::from_secs(1),
            Duration::from_mins(2),
            SCAN_DEADLINE,
        ];
        let mut step = 0usize;
        let mut waits = Vec::new();

        wait_until_deadline(
            deadline,
            || clock.get(),
            |duration| {
                waits.push(duration);
                clock.set(base + wake_offsets[step]);
                step += 1;
            },
        );

        assert!(clock.get() >= deadline);
        assert_eq!(step, wake_offsets.len());
        assert_eq!(
            waits,
            vec![
                Duration::from_mins(5),
                Duration::from_secs(299),
                Duration::from_mins(3),
            ]
        );

        let mut late_waits = Vec::new();
        wait_until_deadline(
            deadline,
            || deadline + Duration::from_secs(1),
            |duration| late_waits.push(duration),
        );
        assert!(late_waits.is_empty());
    }
}
