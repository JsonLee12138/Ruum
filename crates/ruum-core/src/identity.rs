//! Identity captured on the scanned machine, never inferred from an alias.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostIdentity {
    pub hostname: String,
    pub machine_id: Option<String>,
}

impl HostIdentity {
    pub fn current() -> Option<Self> {
        let hostname = if cfg!(target_os = "linux") {
            std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?
        } else {
            String::from_utf8(
                std::process::Command::new("hostname").output().ok()?.stdout,
            )
            .ok()?
        };
        let hostname = hostname.trim().to_owned();
        if hostname.is_empty()
            || !hostname
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
        {
            return None;
        }
        let machine_id = std::fs::read_to_string("/etc/machine-id")
            .ok()
            .map(|id| id.trim().to_owned())
            .filter(|id| {
                id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
            });
        Some(Self {
            hostname,
            machine_id,
        })
    }
}
