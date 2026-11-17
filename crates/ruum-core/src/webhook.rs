//! Generic synchronous JSON webhook delivery for low-disk-space alerts.
//!
//! Uses blocking `ureq` (`rustls` + `json`) strictly under the `service` Cargo
//! feature—never `tokio` or async runtimes. Never logs or embeds user-supplied
//! header values (such as `Authorization` tokens) in errors.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::identity::HostIdentity;
use crate::storage::{WebhookConfig, WebhookEndpoint};

/// Canonical webhook event name for low disk space alerts.
pub const WEBHOOK_EVENT_LOW_SPACE: &str = "ruum.disk.low_space";

/// Canonical webhook event name for user-initiated endpoint tests.
pub const WEBHOOK_EVENT_TEST: &str = "ruum.webhook.test";

/// Host metadata included in [`LowSpaceWebhookPayload`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookHostInfo {
    pub hostname: String,
    pub machine_id: Option<String>,
    pub platform: String,
}

/// Mount space metrics included in [`LowSpaceWebhookPayload`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookMountInfo {
    pub path: String,
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub threshold_bytes: u64,
}

/// Canonical JSON webhook payload emitted on `WatchdogEvent::LowDiskSpace`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LowSpaceWebhookPayload {
    pub event: String,
    pub timestamp: String,
    pub host: WebhookHostInfo,
    pub mount: WebhookMountInfo,
}

impl LowSpaceWebhookPayload {
    /// Format a UNIX epoch second timestamp as RFC3339 UTC (`...Z`).
    #[must_use]
    pub fn format_utc_rfc3339(timestamp_secs: u64) -> String {
        let secs = i64::try_from(timestamp_secs).unwrap_or(0);
        let dt = DateTime::from_timestamp(secs, 0)
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        dt.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// Build a payload from an explicit [`HostIdentity`] and platform string.
    #[must_use]
    pub fn from_event_with_identity(
        timestamp_secs: u64,
        mount_path: &Path,
        available_bytes: u64,
        total_bytes: u64,
        threshold_bytes: u64,
        identity: Option<&HostIdentity>,
        platform: impl Into<String>,
    ) -> Self {
        let (hostname, machine_id) = match identity {
            Some(id) => (id.hostname.clone(), id.machine_id.clone()),
            None => ("unknown".to_owned(), None),
        };
        Self {
            event: WEBHOOK_EVENT_LOW_SPACE.to_owned(),
            timestamp: Self::format_utc_rfc3339(timestamp_secs),
            host: WebhookHostInfo {
                hostname,
                machine_id,
                platform: platform.into(),
            },
            mount: WebhookMountInfo {
                path: mount_path.display().to_string(),
                available_bytes,
                total_bytes,
                threshold_bytes,
            },
        }
    }

    /// Build a payload capturing the local machine's [`HostIdentity`].
    #[must_use]
    pub fn from_low_space_event(
        timestamp_secs: u64,
        mount_path: &Path,
        available_bytes: u64,
        total_bytes: u64,
        threshold_bytes: u64,
    ) -> Self {
        let identity = HostIdentity::current();
        Self::from_event_with_identity(
            timestamp_secs,
            mount_path,
            available_bytes,
            total_bytes,
            threshold_bytes,
            identity.as_ref(),
            std::env::consts::OS,
        )
    }

    /// Build a sample webhook payload for verifying endpoint connectivity.
    #[must_use]
    pub fn test_event(timestamp_secs: u64) -> Self {
        let identity = HostIdentity::current();
        let (hostname, machine_id) = match identity {
            Some(id) => (id.hostname, id.machine_id),
            None => ("unknown".to_owned(), None),
        };
        Self {
            event: WEBHOOK_EVENT_TEST.to_owned(),
            timestamp: Self::format_utc_rfc3339(timestamp_secs),
            host: WebhookHostInfo {
                hostname,
                machine_id,
                platform: std::env::consts::OS.to_owned(),
            },
            mount: WebhookMountInfo {
                path: "/".to_owned(),
                available_bytes: 1_073_741_824,
                total_bytes: 107_374_182_400,
                threshold_bytes: 21_474_836_480,
            },
        }
    }
}

fn post_webhook_json(
    url: &str,
    headers: &BTreeMap<String, String>,
    timeout_secs: u64,
    payload: &LowSpaceWebhookPayload,
) -> io::Result<()> {
    let agent_config = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .user_agent(concat!("ruum/", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = Agent::new_with_config(agent_config);

    let mut request = agent.post(url);
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }

    match request.send_json(payload) {
        Ok(response) => {
            if response.status().is_success() {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "webhook returned HTTP status {}",
                    response.status()
                )))
            }
        }
        Err(ureq::Error::StatusCode(code)) => Err(io::Error::other(format!(
            "webhook returned HTTP status {code}"
        ))),
        Err(ureq::Error::Timeout(reason)) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("webhook request timed out: {reason:?}"),
        )),
        Err(err) => {
            Err(io::Error::other(format!("webhook request failed: {err}")))
        }
    }
}

fn send_webhook_endpoint(
    endpoint: &WebhookEndpoint,
    payload: &LowSpaceWebhookPayload,
) -> io::Result<()> {
    let clean = endpoint.validated()?;
    let headers = clean.parsed_headers()?;
    post_webhook_json(&clean.url, &headers, clean.timeout_secs, payload)
}

fn endpoint_key(endpoint: &WebhookEndpoint) -> String {
    if endpoint.id.trim().is_empty() {
        endpoint.name.clone()
    } else {
        endpoint.id.clone()
    }
}

/// Deliver a low-space alert JSON `POST` if the webhook is enabled and
/// configured with a non-empty URL.
pub fn send_low_space_webhook(
    config: &WebhookConfig,
    payload: &LowSpaceWebhookPayload,
) -> io::Result<()> {
    if !config.enabled || config.url.trim().is_empty() {
        return Ok(());
    }

    let clean = config.validated()?;
    let headers = clean.parsed_headers()?;
    post_webhook_json(&clean.url, &headers, clean.timeout_secs, payload)
}

/// Deliver a low-space alert JSON `POST` concurrently to every enabled
/// [`WebhookEndpoint`], isolating per-endpoint failures so one error never
/// blocks other endpoints.
#[must_use]
pub fn send_low_space_webhooks(
    endpoints: &[WebhookEndpoint],
    payload: &LowSpaceWebhookPayload,
) -> Vec<(String, io::Result<()>)> {
    let active: Vec<&WebhookEndpoint> = endpoints
        .iter()
        .filter(|ep| ep.enabled && !ep.url.trim().is_empty())
        .collect();
    if active.is_empty() {
        return Vec::new();
    }
    if active.len() == 1 {
        let ep = active[0];
        return vec![(endpoint_key(ep), send_webhook_endpoint(ep, payload))];
    }
    std::thread::scope(|scope| {
        #[allow(
            clippy::needless_collect,
            reason = "spawn all webhook threads before joining so delivery \
                      runs concurrently"
        )]
        let handles: Vec<_> = active
            .into_iter()
            .map(|ep| {
                let key = endpoint_key(ep);
                (key, scope.spawn(move || send_webhook_endpoint(ep, payload)))
            })
            .collect();
        handles
            .into_iter()
            .map(|(key, handle)| {
                let res = handle.join().unwrap_or_else(|_| {
                    Err(io::Error::other("webhook delivery thread panicked"))
                });
                (key, res)
            })
            .collect()
    })
}

/// Send a test webhook JSON `POST` (`ruum.webhook.test`) to verify an
/// endpoint's URL, custom headers, and connectivity.
pub fn test_webhook_endpoint(endpoint: &WebhookEndpoint) -> io::Result<()> {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let payload = LowSpaceWebhookPayload::test_event(now_secs);
    send_webhook_endpoint(endpoint, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    fn sample_payload() -> LowSpaceWebhookPayload {
        let id = HostIdentity {
            hostname: "server-01".to_owned(),
            machine_id: Some("9f8c0123456789abcdef0123456789ab".to_owned()),
        };
        LowSpaceWebhookPayload::from_event_with_identity(
            1_794_895_200,
            Path::new("/home/ubuntu"),
            1_825_361_100,
            107_374_182_400,
            21_474_836_480,
            Some(&id),
            "linux",
        )
    }

    fn read_http_request(
        stream: &mut std::net::TcpStream,
    ) -> (String, BTreeMap<String, String>, Vec<u8>) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut chunk).expect("read tcp request");
            assert!(n > 0, "unexpected EOF before HTTP headers completed");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos;
            }
        };

        let header_str =
            String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let mut lines = header_str.lines();
        let request_line = lines.next().unwrap_or_default().to_owned();
        let mut headers = BTreeMap::new();
        let mut content_length = 0_usize;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().to_ascii_lowercase();
                let val = v.trim().to_owned();
                if key == "content-length" {
                    content_length = val.parse().unwrap_or(0);
                }
                headers.insert(key, val);
            }
        }

        let body_start = header_end + 4;
        while buf.len() < body_start + content_length {
            let n = stream.read(&mut chunk).expect("read http body");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let body = buf[body_start
            ..body_start + content_length.min(buf.len() - body_start)]
            .to_vec();
        (request_line, headers, body)
    }

    #[test]
    fn skips_when_disabled_or_url_empty() {
        let payload = sample_payload();

        let disabled = WebhookConfig {
            enabled: false,
            url: "http://127.0.0.1:1/unreachable".to_owned(),
            ..WebhookConfig::default()
        };
        send_low_space_webhook(&disabled, &payload).unwrap();

        let empty_url = WebhookConfig {
            enabled: true,
            url: "   ".to_owned(),
            ..WebhookConfig::default()
        };
        send_low_space_webhook(&empty_url, &payload).unwrap();
    }

    #[test]
    fn posts_canonical_json_and_custom_headers_to_http_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let req = read_http_request(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\n\r\nok",
                )
                .unwrap();
            stream.flush().unwrap();
            req
        });

        let cfg = WebhookConfig {
            enabled: true,
            url: format!("http://{addr}/hooks/disk"),
            headers_json:
                r#"{"Authorization":"Bearer tok-123","X-Env":"prod"}"#
                    .to_owned(),
            timeout_secs: 5,
            updated_at_secs: 1,
        };
        let payload = sample_payload();
        send_low_space_webhook(&cfg, &payload).unwrap();

        let (request_line, headers, body) = server.join().unwrap();
        assert!(
            request_line.starts_with("POST /hooks/disk "),
            "unexpected request line: {request_line}"
        );
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer tok-123")
        );
        assert_eq!(headers.get("x-env").map(String::as_str), Some("prod"));
        assert!(
            headers
                .get("content-type")
                .is_some_and(|v| v.starts_with("application/json")),
            "unexpected content-type: {:?}",
            headers.get("content-type")
        );

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["event"], WEBHOOK_EVENT_LOW_SPACE);
        assert_eq!(parsed["timestamp"], "2026-11-17T06:00:00Z");
        assert_eq!(parsed["host"]["hostname"], "server-01");
        assert_eq!(
            parsed["host"]["machine_id"],
            "9f8c0123456789abcdef0123456789ab"
        );
        assert_eq!(parsed["host"]["platform"], "linux");
        assert_eq!(parsed["mount"]["path"], "/home/ubuntu");
        assert_eq!(parsed["mount"]["available_bytes"], 1_825_361_100_u64);
        assert_eq!(parsed["mount"]["total_bytes"], 107_374_182_400_u64);
        assert_eq!(parsed["mount"]["threshold_bytes"], 21_474_836_480_u64);
    }

    #[test]
    fn non_2xx_response_returns_error_without_leaking_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_http_request(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 3\r\n\r\nbad",
                )
                .unwrap();
            stream.flush().unwrap();
        });

        let cfg = WebhookConfig {
            enabled: true,
            url: format!("http://{addr}/fail"),
            headers_json: r#"{"Authorization":"Bearer super-secret-value"}"#
                .to_owned(),
            timeout_secs: 5,
            updated_at_secs: 1,
        };
        let err = send_low_space_webhook(&cfg, &sample_payload()).unwrap_err();
        server.join().unwrap();

        let msg = err.to_string();
        assert!(msg.contains("502"), "expected status 502 in error: {msg}");
        assert!(
            !msg.contains("super-secret-value"),
            "error must never leak header secrets: {msg}"
        );
    }

    #[test]
    fn slow_server_triggers_timeout_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_http_request(&mut stream);
            std::thread::sleep(Duration::from_millis(1600));
        });

        let cfg = WebhookConfig {
            enabled: true,
            url: format!("http://{addr}/slow"),
            headers_json: "{}".to_owned(),
            timeout_secs: 1,
            updated_at_secs: 1,
        };
        let err = send_low_space_webhook(&cfg, &sample_payload()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        server.join().unwrap();
    }

    #[test]
    fn send_low_space_webhooks_fans_out_and_isolates_failures() {
        let ok_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ok_addr = ok_listener.local_addr().unwrap();
        let ok_server = std::thread::spawn(move || {
            let (mut stream, _) = ok_listener.accept().unwrap();
            let req = read_http_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
            stream.flush().unwrap();
            req
        });

        let fail_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let fail_addr = fail_listener.local_addr().unwrap();
        let fail_server = std::thread::spawn(move || {
            let (mut stream, _) = fail_listener.accept().unwrap();
            let _ = read_http_request(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 3\r\n\r\nerr",
                )
                .unwrap();
            stream.flush().unwrap();
        });

        let mut disabled_ep =
            WebhookEndpoint::new("disabled", "http://127.0.0.1:1/never");
        disabled_ep.id = "ep-disabled".to_owned();
        disabled_ep.enabled = false;

        let mut fail_ep =
            WebhookEndpoint::new("fail", format!("http://{fail_addr}/fail"));
        fail_ep.id = "ep-fail".to_owned();
        fail_ep.headers_json =
            r#"{"Authorization":"Bearer top-secret-fail-tok"}"#.to_owned();
        fail_ep.timeout_secs = 5;

        let mut ok_ep =
            WebhookEndpoint::new("ok", format!("http://{ok_addr}/ok"));
        ok_ep.id = "ep-ok".to_owned();
        ok_ep.headers_json = r#"{"X-Endpoint":"healthy"}"#.to_owned();
        ok_ep.timeout_secs = 5;

        let outcomes = send_low_space_webhooks(
            &[disabled_ep, fail_ep, ok_ep],
            &sample_payload(),
        );
        assert_eq!(outcomes.len(), 2);

        let fail_res = outcomes
            .iter()
            .find(|(id, _)| id == "ep-fail")
            .expect("fail outcome present");
        let fail_err = fail_res.1.as_ref().unwrap_err().to_string();
        assert!(fail_err.contains("500"), "{fail_err}");
        assert!(
            !fail_err.contains("top-secret-fail-tok"),
            "must not leak header secrets: {fail_err}"
        );

        let ok_res = outcomes
            .iter()
            .find(|(id, _)| id == "ep-ok")
            .expect("ok outcome present");
        assert!(ok_res.1.is_ok(), "healthy endpoint must still succeed");

        let (req_line, headers, body) = ok_server.join().unwrap();
        fail_server.join().unwrap();
        assert!(req_line.starts_with("POST /ok "));
        assert_eq!(
            headers.get("x-endpoint").map(String::as_str),
            Some("healthy")
        );
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["event"], WEBHOOK_EVENT_LOW_SPACE);
    }

    #[test]
    fn test_webhook_endpoint_posts_test_event_payload() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let req = read_http_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
            stream.flush().unwrap();
            req
        });

        let mut ep = WebhookEndpoint::new(
            "manual-test",
            format!("http://{addr}/hooks/test"),
        );
        ep.enabled = false; // Manual test still delivers even when disabled.
        ep.headers_json = r#"{"X-Test-Header":"ping"}"#.to_owned();
        ep.timeout_secs = 5;

        test_webhook_endpoint(&ep).unwrap();

        let (req_line, headers, body) = server.join().unwrap();
        assert!(req_line.starts_with("POST /hooks/test "));
        assert_eq!(
            headers.get("x-test-header").map(String::as_str),
            Some("ping")
        );
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["event"], WEBHOOK_EVENT_TEST);
        assert!(parsed["host"]["platform"].is_string());
        assert!(parsed["mount"]["total_bytes"].is_u64());
    }
}
