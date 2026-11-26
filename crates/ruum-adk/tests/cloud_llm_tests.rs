use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ruum_adk::{
    ALLOWED_RUUM_TOOLS, AdkAgentRequest, AdkClientError, AdkDirectClient,
    AdkToolDef, AdkToolExecutor, CancelToken, CloudGatewayError,
    CloudStreamStage, CloudTokenProvider, MAX_CLOUD_COMPACTED_BYTES,
    MAX_CLOUD_MAX_TOKENS, MAX_CLOUD_MESSAGES, RuumCloudConfig, RuumTask,
    compact_chat_request_payload,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
    raw_len: usize,
}

#[derive(Debug, Clone)]
struct MockReply {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    chunks: Vec<(Duration, String)>,
    delay: Duration,
}

impl MockReply {
    fn json(status: u16, body: &Value) -> Self {
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            402 => "Payment Required",
            403 => "Forbidden",
            429 => "Too Many Requests",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Error",
        };
        Self {
            status,
            reason,
            content_type: "application/json",
            headers: Vec::new(),
            chunks: vec![(Duration::ZERO, body.to_string())],
            delay: Duration::ZERO,
        }
    }

    const fn sse(chunks: Vec<(Duration, String)>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type: "text/event-stream; charset=utf-8",
            headers: Vec::new(),
            chunks,
            delay: Duration::ZERO,
        }
    }

    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

struct MockServer {
    base_url: String,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    notify: Arc<Notify>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl MockServer {
    fn recorded(&self) -> Vec<RecordedRequest> {
        self.recorded.lock().unwrap().clone()
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        if let Some(handle) = self.task.take() {
            handle.abort();
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn read_http_request(
    stream: &mut tokio::net::TcpStream,
) -> Option<RecordedRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end;
    loop {
        let n = tokio::time::timeout(
            Duration::from_secs(5),
            stream.read(&mut chunk),
        )
        .await
        .ok()?
        .ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
    }

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.split("\r\n");
    let req_line = lines.next()?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();

    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_owned();
            if key == "content-length" {
                content_length = val.parse::<usize>().unwrap_or(0);
            }
            headers.push((key, val));
        }
    }

    while buf.len() < header_end + content_length {
        let n = tokio::time::timeout(
            Duration::from_secs(5),
            stream.read(&mut chunk),
        )
        .await
        .ok()?
        .ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }

    let body_bytes =
        &buf[header_end..(header_end + content_length).min(buf.len())];
    let body = serde_json::from_slice(body_bytes).unwrap_or_else(|_| json!({}));

    Some(RecordedRequest {
        method,
        path,
        headers,
        body,
        raw_len: body_bytes.len(),
    })
}

fn start_mock_server(replies: Vec<MockReply>) -> MockServer {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let addr = std_listener.local_addr().unwrap();
    let listener = TcpListener::from_std(std_listener).unwrap();
    let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let recorded_bg = Arc::clone(&recorded);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_bg = Arc::clone(&stop);
    let notify = Arc::new(Notify::new());
    let notify_bg = Arc::clone(&notify);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    let task = tokio::spawn(async move {
        while !stop_bg.load(Ordering::SeqCst) {
            let remaining =
                deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let accepted = tokio::select! {
                () = notify_bg.notified() => break,
                () = tokio::time::sleep(remaining) => break,
                res = listener.accept() => res,
            };
            let Ok((mut stream, _)) = accepted else {
                break;
            };
            let queue = Arc::clone(&queue);
            let recorded_bg = Arc::clone(&recorded_bg);
            tokio::spawn(async move {
                let Some(req) = read_http_request(&mut stream).await else {
                    return;
                };
                recorded_bg.lock().unwrap().push(req);
                let reply = queue.lock().unwrap().pop_front().unwrap_or_else(|| {
                    MockReply::json(
                        500,
                        &json!({"code": "MOCK_EXHAUSTED", "message": "no reply"}),
                    )
                });
                if !reply.delay.is_zero() {
                    tokio::time::sleep(reply.delay).await;
                }
                let mut header_block = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\n",
                    reply.status, reply.reason, reply.content_type,
                );
                if reply.content_type.starts_with("application/json") {
                    let body_len: usize =
                        reply.chunks.iter().map(|(_, s)| s.len()).sum();
                    let _ =
                        write!(header_block, "Content-Length: {body_len}\r\n");
                }
                header_block.push_str("Connection: close\r\n");
                for (k, v) in reply.headers {
                    let _ = write!(header_block, "{k}: {v}\r\n");
                }
                header_block.push_str("\r\n");
                let _ = stream.write_all(header_block.as_bytes()).await;
                let _ = stream.flush().await;
                for (chunk_delay, chunk_body) in reply.chunks {
                    if !chunk_delay.is_zero() {
                        tokio::time::sleep(chunk_delay).await;
                    }
                    if !chunk_body.is_empty() {
                        if stream
                            .write_all(chunk_body.as_bytes())
                            .await
                            .is_err()
                        {
                            break;
                        }
                        let _ = stream.flush().await;
                    }
                }
            });
        }
    });

    MockServer {
        base_url: format!("http://{addr}"),
        recorded,
        stop,
        notify,
        task: Some(task),
    }
}

fn header_value<'a>(req: &'a RecordedRequest, name: &str) -> Option<&'a str> {
    let target = name.to_ascii_lowercase();
    req.headers
        .iter()
        .find(|(k, _)| k == &target)
        .map(|(_, v)| v.as_str())
}

struct StubTokenProvider {
    current: Mutex<String>,
    refreshed: String,
    refresh_calls: AtomicUsize,
    fail_refresh: Option<CloudGatewayError>,
}

impl StubTokenProvider {
    fn new(initial: &str, refreshed: &str) -> Self {
        Self {
            current: Mutex::new(initial.to_owned()),
            refreshed: refreshed.to_owned(),
            refresh_calls: AtomicUsize::new(0),
            fail_refresh: None,
        }
    }

    fn refresh_count(&self) -> usize {
        self.refresh_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CloudTokenProvider for StubTokenProvider {
    async fn current_token(&self) -> Result<String, CloudGatewayError> {
        Ok(self.current.lock().unwrap().clone())
    }

    async fn refresh_token(&self) -> Result<String, CloudGatewayError> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(ref err) = self.fail_refresh {
            return Err(err.clone());
        }
        let mut guard = self.current.lock().unwrap();
        (*guard).clone_from(&self.refreshed);
        Ok(guard.clone())
    }
}

struct EchoToolExecutor;

#[async_trait]
impl AdkToolExecutor for EchoToolExecutor {
    async fn execute(&self, args: Value) -> Result<Value, String> {
        Ok(json!({ "ok": true, "args": args }))
    }
}

fn tool_def(name: &str) -> (AdkToolDef, Arc<dyn AdkToolExecutor>) {
    (
        AdkToolDef {
            name: name.to_owned(),
            description: format!("Tool {name}"),
            parameters_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }),
        },
        Arc::new(EchoToolExecutor),
    )
}

#[tokio::test]
async fn cloud_agent_enforces_headers_non_streaming_max_tokens_whitelist_and_401_replay()
 {
    let server = start_mock_server(vec![
        MockReply::json(
            401,
            &json!({
                "code": "AUTH_TOKEN_EXPIRED",
                "message": "jwt expired",
                "traceId": "tr-401-exp",
                "error": {
                    "code": "AUTH_TOKEN_EXPIRED",
                    "message": "jwt expired",
                    "type": "ruum_gateway_error",
                    "traceId": "tr-401-exp"
                }
            }),
        )
        .with_header("X-Trace-ID", "tr-401-exp"),
        MockReply::json(
            200,
            &json!({
                "id": "chatcmpl-1",
                "object": "chat.completion",
                "model": "ruum-fast",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "scan_directory",
                                "arguments": "{\"path\":\"/tmp\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        ),
        MockReply::json(
            200,
            &json!({
                "id": "chatcmpl-2",
                "object": "chat.completion",
                "model": "ruum-fast",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "{\"summary\":\"ok\",\"items\":[]}"
                    },
                    "finish_reason": "stop"
                }]
            }),
        ),
    ]);

    let provider = Arc::new(StubTokenProvider::new("jwt-expired", "jwt-fresh"));
    let cfg = RuumCloudConfig::new(
        &server.base_url,
        "ruum-fast",
        RuumTask::LocalAnalysis,
        provider.clone(),
    );

    let mut req = AdkAgentRequest::new("Local sys", "Analyze disk");
    req.max_iterations = 99;
    req.fallback_without_tools_on_unsupported = true;
    req.tools = vec![
        tool_def("scan_directory"),
        tool_def("trash_paths"),
        tool_def("delete_everything"),
        tool_def("check_git_safety"),
    ];

    let client = AdkDirectClient::new().unwrap();
    let resp = client.run_cloud_agent(&cfg, req).await.unwrap();
    assert_eq!(resp.text, "{\"summary\":\"ok\",\"items\":[]}");
    assert_eq!(resp.tool_calls_made, 1);
    assert_eq!(provider.refresh_count(), 1);

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[0].path, "/v1/chat/completions");

    // Same X-Ruum-Session reused across 401 replay AND subsequent tool turn.
    let s0 = header_value(&recorded[0], "x-ruum-session").unwrap();
    let s1 = header_value(&recorded[1], "x-ruum-session").unwrap();
    let s2 = header_value(&recorded[2], "x-ruum-session").unwrap();
    assert!(!s0.is_empty());
    assert_eq!(s0, s1);
    assert_eq!(s1, s2);

    assert_eq!(
        header_value(&recorded[0], "authorization"),
        Some("Bearer jwt-expired")
    );
    assert_eq!(
        header_value(&recorded[1], "authorization"),
        Some("Bearer jwt-fresh")
    );
    assert_eq!(
        header_value(&recorded[2], "authorization"),
        Some("Bearer jwt-fresh")
    );
    assert_eq!(
        header_value(&recorded[1], "x-ruum-task"),
        Some("local_analysis")
    );
    assert_eq!(
        header_value(&recorded[1], "x-ruum-client-version"),
        Some(env!("CARGO_PKG_VERSION"))
    );

    assert_eq!(recorded[1].body["stream"], json!(true));
    assert!(
        header_value(&recorded[1], "accept")
            .is_some_and(|v| v.contains("text/event-stream"))
    );
    if let Some(mt) = recorded[1].body.get("max_tokens").and_then(Value::as_u64)
    {
        assert!(mt <= u64::from(MAX_CLOUD_MAX_TOKENS));
    }

    let sent_tools: Vec<&str> = recorded[1].body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t.pointer("/function/name").and_then(Value::as_str))
        .collect();
    assert_eq!(sent_tools, vec!["check_git_safety", "scan_directory"]);
    for name in &sent_tools {
        assert!(ALLOWED_RUUM_TOOLS.contains(name));
    }
    assert!(recorded[1].raw_len <= MAX_CLOUD_COMPACTED_BYTES);
}

#[tokio::test]
async fn cloud_agent_preserves_trace_id_and_never_retries_403_revoked() {
    let server = start_mock_server(vec![
        MockReply::json(
            403,
            &json!({
                "code": "AUTH_DEVICE_REVOKED",
                "message": "device revoked",
                "traceId": "tr-revoked-1",
                "error": {
                    "code": "AUTH_DEVICE_REVOKED",
                    "message": "device revoked",
                    "type": "ruum_gateway_error",
                    "traceId": "tr-revoked-1"
                }
            }),
        )
        .with_header("X-Trace-ID", "tr-revoked-1"),
    ]);

    let provider = Arc::new(StubTokenProvider::new("jwt-valid", "jwt-new"));
    let cfg = RuumCloudConfig::new(
        &server.base_url,
        "ruum-pro",
        RuumTask::TreeAnalysis,
        provider.clone(),
    );
    assert_eq!(cfg.connect_timeout, Duration::from_mins(1));
    assert_eq!(cfg.request_timeout, Duration::from_mins(3));
    let client = AdkDirectClient::new().unwrap();
    let err = client
        .run_cloud_agent(&cfg, AdkAgentRequest::new("Sys", "Inspect tree"))
        .await
        .expect_err("403 must fail without retries");

    assert_eq!(provider.refresh_count(), 0);
    assert_eq!(server.recorded().len(), 1);
    assert_eq!(
        header_value(&server.recorded()[0], "x-ruum-task"),
        Some("tree_analysis")
    );
    match err {
        AdkClientError::CloudGateway {
            status,
            code,
            trace_id,
            is_timeout,
            ..
        } => {
            assert_eq!(status, Some(403));
            assert_eq!(code.as_deref(), Some("AUTH_DEVICE_REVOKED"));
            assert_eq!(trace_id.as_deref(), Some("tr-revoked-1"));
            assert!(!is_timeout);
        }
        other => panic!("expected CloudGateway error, got {other:?}"),
    }
}

#[tokio::test]
async fn cloud_agent_request_timeout_maps_to_cloud_gateway_timeout() {
    let mut slow = MockReply::json(200, &json!({}));
    slow.delay = Duration::from_secs(2);
    let server = start_mock_server(vec![slow]);

    let provider = Arc::new(StubTokenProvider::new("jwt-valid", "jwt-new"));
    let cfg = RuumCloudConfig::new(
        &server.base_url,
        "ruum-fast",
        RuumTask::LocalAnalysis,
        provider,
    )
    .with_request_timeout(Duration::from_millis(120));

    let client = AdkDirectClient::new().unwrap();
    let err = client
        .run_cloud_agent(&cfg, AdkAgentRequest::new("Sys", "Hello"))
        .await
        .expect_err("slow request must time out");

    match err {
        AdkClientError::CloudGateway { is_timeout, .. } => {
            assert!(is_timeout);
        }
        other => panic!("expected timeout CloudGateway error, got {other:?}"),
    }
}

#[test]
fn compact_chat_request_payload_bounds_messages_and_bytes_without_orphaning_tools()
 {
    let big_tool_output = "x".repeat(16 * 1024);
    let mut messages = vec![
        json!({"role": "system", "content": "sys prompt"}),
        json!({"role": "user", "content": "initial prompt"}),
    ];
    for idx in 0..12 {
        let call_id = format!("call_{idx}");
        messages.push(json!({
            "role": "assistant",
            "tool_calls": [{
                "id": call_id,
                "type": "function",
                "function": {"name": "scan_directory", "arguments": "{}"}
            }]
        }));
        messages.push(json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": big_tool_output
        }));
    }

    let mut payload = json!({
        "model": "ruum-fast",
        "messages": messages,
        "stream": false
    });

    compact_chat_request_payload(
        &mut payload,
        MAX_CLOUD_MESSAGES,
        MAX_CLOUD_COMPACTED_BYTES,
    )
    .unwrap();

    let compacted = payload["messages"].as_array().unwrap();
    assert!(compacted.len() <= MAX_CLOUD_MESSAGES);
    assert!(
        compacted.iter().any(|m| m["role"] != "system"),
        "nonSystem >= 1 required"
    );
    assert_eq!(compacted[0]["role"], "system");
    assert_eq!(compacted[1]["role"], "user");
    assert_eq!(compacted[1]["content"], "initial prompt");
    // No orphaned tool message immediately after initial user prompt.
    if compacted.len() > 2 {
        assert_ne!(compacted[2]["role"], "tool");
    }

    let serialized = serde_json::to_vec(&payload).unwrap();
    assert!(serialized.len() <= MAX_CLOUD_COMPACTED_BYTES);
}

#[tokio::test]
async fn cloud_agent_forwards_event_key_retries_429_skips_fup_and_honors_cancel()
 {
    let rate_limit_reply = MockReply::json(
        429,
        &json!({
            "ok": false,
            "code": "INFERENCE_RATE_LIMITED",
            "message": "queued",
            "traceId": "tr-rl-1"
        }),
    )
    .with_header("Retry-After", "0");
    let ok_reply = MockReply::json(
        200,
        &json!({
            "id": "chatcmpl-ok",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Recovered"},
                "finish_reason": "stop"
            }]
        }),
    );
    let server = start_mock_server(vec![rate_limit_reply, ok_reply]);

    let tp = Arc::new(StubTokenProvider::new("jwt-ok", "jwt-refreshed"));
    let cfg = RuumCloudConfig::new(
        &server.base_url,
        "ruum-fast",
        RuumTask::LocalAnalysis,
        tp.clone(),
    )
    .with_event_key("disk_cleanup_local");

    let client = AdkDirectClient::new().unwrap();
    let resp = client
        .run_cloud_agent(&cfg, AdkAgentRequest::new("sys", "scan"))
        .await
        .expect("429 INFERENCE_RATE_LIMITED should retry and succeed");
    assert_eq!(resp.text, "Recovered");

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 2);
    assert!(recorded.iter().all(|req| {
        req.headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("x-ruum-event-key")
                && v == "disk_cleanup_local"
        })
    }));

    // Non-retryable FUP 429 must fail immediately without retry.
    let fup_reply = MockReply::json(
        429,
        &json!({
            "ok": false,
            "code": "FUP_DAILY_CAP_EXCEEDED",
            "message": "daily cap hit"
        }),
    );
    let fup_server = start_mock_server(vec![fup_reply]);
    let fup_cfg = RuumCloudConfig::new(
        &fup_server.base_url,
        "ruum-fast",
        RuumTask::LocalAnalysis,
        tp.clone(),
    );
    let fup_err = client
        .run_cloud_agent(&fup_cfg, AdkAgentRequest::new("sys", "scan"))
        .await
        .expect_err("FUP 429 must not retry");
    assert!(matches!(
        fup_err,
        AdkClientError::CloudGateway {
            code: Some(ref c),
            ..
        } if c == "FUP_DAILY_CAP_EXCEEDED"
    ));
    assert_eq!(fup_server.recorded().len(), 1);

    // CancelToken must abort an in-flight slow request cleanly.
    let mut slow = MockReply::json(200, &json!({}));
    slow.delay = Duration::from_secs(3);
    let slow_server = start_mock_server(vec![slow]);
    let cancel = CancelToken::new();
    let cancel_trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_trigger.cancel();
    });
    let slow_cfg = RuumCloudConfig::new(
        &slow_server.base_url,
        "ruum-fast",
        RuumTask::LocalAnalysis,
        tp,
    )
    .with_request_timeout(Duration::from_secs(5));
    let cancel_err = client
        .run_cloud_agent(
            &slow_cfg,
            AdkAgentRequest::new("sys", "scan").with_cancel(cancel),
        )
        .await
        .expect_err("cancelled cloud run must abort");
    assert!(matches!(cancel_err, AdkClientError::Cancelled));
}

#[tokio::test]
async fn probe_cloud_model_calls_models_probe_and_maps_result() {
    let reply = MockReply::json(
        200,
        &json!({
            "ok": true,
            "model": "ruum-fast",
            "ttftMs": 42,
            "totalMs": 55,
            "budgetExhausted": false
        }),
    );
    let server = start_mock_server(vec![reply]);
    let tp = Arc::new(StubTokenProvider::new("jwt-probe", "jwt-probe-2"));
    let cfg = RuumCloudConfig::new(
        &server.base_url,
        "ruum-fast",
        RuumTask::LocalAnalysis,
        tp,
    );
    let client = AdkDirectClient::new().unwrap();
    let res = client
        .probe_cloud_model(&cfg)
        .await
        .expect("probe_cloud_model should succeed");
    assert_eq!(res.model, "ruum-fast");
    assert_eq!(res.ttft_ms, 42);
    assert_eq!(res.total_ms, 55);
    assert!(!res.budget_exhausted);

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[0].path, "/v1/models/probe");
    assert_eq!(recorded[0].body["model"], "ruum-fast");
    assert!(recorded[0].body.get("timeoutMs").is_none());

    let cancel = CancelToken::new();
    cancel.cancel();
    let cfg_cancelled = RuumCloudConfig::new(
        &server.base_url,
        "ruum-fast",
        RuumTask::LocalAnalysis,
        Arc::new(StubTokenProvider::new("jwt-probe-c", "jwt-probe-c2")),
    )
    .with_cancel(cancel);
    let err = client
        .probe_cloud_model(&cfg_cancelled)
        .await
        .expect_err("cancelled probe must return Cancelled");
    assert!(matches!(err, AdkClientError::Cancelled));
}

#[test]
fn compact_chat_request_payload_preserves_language_requirement_tail() {
    let huge_body = "x".repeat(70 * 1024);
    let tail = "\nOUTPUT LANGUAGE REQUIREMENT:\nMUST reply in Chinese (zh-CN).";
    let mut payload = json!({
        "model": "ruum-fast",
        "messages": [
            {"role": "system", "content": "sys"},
            {"role": "user", "content": format!("{huge_body}{tail}")}
        ],
        "stream": false
    });
    compact_chat_request_payload(
        &mut payload,
        MAX_CLOUD_MESSAGES,
        MAX_CLOUD_COMPACTED_BYTES,
    )
    .expect("compaction succeeds");
    let content = payload["messages"][1]["content"].as_str().unwrap();
    assert!(content.ends_with(tail), "tail stripped: {content}");
    assert!(
        serde_json::to_vec(&payload).unwrap().len()
            <= MAX_CLOUD_COMPACTED_BYTES
    );
}

#[tokio::test]
async fn cloud_agent_consumes_sse_stages_tool_calls_and_survives_past_single_chunk_timeout()
 {
    let server = start_mock_server(vec![
        MockReply::sse(vec![
            (
                Duration::ZERO,
                ": ruum-stage {\"stage\":\"queued\"}\n\n".to_owned(),
            ),
            (
                Duration::from_millis(60),
                ": ruum-stage {\"stage\":\"connecting\"}\n\n".to_owned(),
            ),
            (
                Duration::from_millis(60),
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"plan\"}}]}\n\n"
                    .to_owned(),
            ),
            (
                Duration::from_millis(60),
                ": ruum-stage {\"stage\":\"generating\"}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_sse_1\",\"function\":{\"name\":\"scan_directory\",\"arguments\":\"{\\\"path\\\":\\\"/tmp\\\"}\"}}]}}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n"
                    .to_owned(),
            ),
        ]),
        MockReply::sse(vec![
            (
                Duration::ZERO,
                ": ruum-stage {\"stage\":\"thinking\"}\n\n".to_owned(),
            ),
            (
                Duration::from_millis(60),
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"{\\\"summary\\\":\"}}]}\n\n"
                    .to_owned(),
            ),
            (
                Duration::from_millis(60),
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\\\"sse ok\\\",\\\"items\\\":[]}\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
                    .to_owned(),
            ),
        ]),
    ]);

    let stages = Arc::new(Mutex::new(Vec::new()));
    let stages_cb = Arc::clone(&stages);
    let provider = Arc::new(StubTokenProvider::new("jwt-valid", "jwt-new"));
    let cfg = RuumCloudConfig::new(
        &server.base_url,
        "ruum-fast",
        RuumTask::LocalAnalysis,
        provider,
    )
    // Total SSE run exceeds 150ms across 3x60ms chunks while each read stays < 150ms.
    .with_request_timeout(Duration::from_millis(150))
    .with_on_stage(Arc::new(move |stage| {
        stages_cb.lock().unwrap().push(stage);
    }));

    let mut req = AdkAgentRequest::new("Sys", "Inspect");
    req.tools = vec![tool_def("scan_directory")];
    let client = AdkDirectClient::new().unwrap();
    let resp = client.run_cloud_agent(&cfg, req).await.unwrap();
    assert_eq!(resp.text, "{\"summary\":\"sse ok\",\"items\":[]}");
    assert_eq!(resp.tool_calls_made, 1);

    let observed = stages.lock().unwrap().clone();
    assert!(observed.contains(&CloudStreamStage::Queued));
    assert!(observed.contains(&CloudStreamStage::Connecting));
    assert!(observed.contains(&CloudStreamStage::Thinking));
    assert!(observed.contains(&CloudStreamStage::Generating));
}

#[tokio::test]
async fn cloud_agent_sse_rejects_premature_eof_midstream_error_idle_stall_and_never_retries_504()
 {
    // 1. Premature EOF (no [DONE] and no finish_reason) fails cleanly without retry.
    let eof_server = start_mock_server(vec![MockReply::sse(vec![(
        Duration::ZERO,
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"half\"}}]}\n\n"
            .to_owned(),
    )])]);
    let provider = Arc::new(StubTokenProvider::new("jwt-valid", "jwt-new"));
    let eof_cfg = RuumCloudConfig::new(
        &eof_server.base_url,
        "ruum-fast",
        RuumTask::TreeAnalysis,
        provider.clone(),
    );
    let client = AdkDirectClient::new().unwrap();
    let eof_err = client
        .run_cloud_agent(&eof_cfg, AdkAgentRequest::new("Sys", "Hello"))
        .await
        .expect_err("premature EOF must error");
    assert_eq!(eof_server.recorded().len(), 1);
    assert!(matches!(eof_err, AdkClientError::CloudGateway { .. }));

    // 2. Mid-stream data: {"error": ...} fails immediately without retry.
    let mid_err_server = start_mock_server(vec![MockReply::sse(vec![(
        Duration::ZERO,
        ": ruum-stage {\"stage\":\"connecting\"}\n\ndata: {\"error\":{\"code\":\"COMMON_UPSTREAM_TIMEOUT\",\"status\":504,\"message\":\"upstream stalled\"}}\n\n"
            .to_owned(),
    )])]);
    let mid_cfg = RuumCloudConfig::new(
        &mid_err_server.base_url,
        "ruum-fast",
        RuumTask::TreeAnalysis,
        provider.clone(),
    );
    let mid_err = client
        .run_cloud_agent(&mid_cfg, AdkAgentRequest::new("Sys", "Hello"))
        .await
        .expect_err("mid-stream SSE error must fail");
    assert_eq!(mid_err_server.recorded().len(), 1);
    assert!(matches!(
        mid_err,
        AdkClientError::CloudGateway {
            is_timeout: true,
            ..
        }
    ));

    // 3. Socket-silence fallback aborts stalled SSE read.
    let stall_server = start_mock_server(vec![MockReply::sse(vec![
        (
            Duration::ZERO,
            ": ruum-stage {\"stage\":\"connecting\"}\n\n".to_owned(),
        ),
        (Duration::from_millis(400), "data: [DONE]\n\n".to_owned()),
    ])]);
    let stall_cfg = RuumCloudConfig::new(
        &stall_server.base_url,
        "ruum-fast",
        RuumTask::TreeAnalysis,
        provider.clone(),
    )
    .with_request_timeout(Duration::from_millis(90));
    let stall_err = client
        .run_cloud_agent(&stall_cfg, AdkAgentRequest::new("Sys", "Hello"))
        .await
        .expect_err("stalled SSE read must time out");
    assert_eq!(stall_server.recorded().len(), 1);
    assert!(matches!(
        stall_err,
        AdkClientError::CloudGateway {
            is_timeout: true,
            ..
        }
    ));

    // 4. HTTP 504 never retries.
    let t504_server = start_mock_server(vec![MockReply::json(
        504,
        &json!({
            "code": "COMMON_UPSTREAM_TIMEOUT",
            "message": "timeout"
        }),
    )]);
    let t504_cfg = RuumCloudConfig::new(
        &t504_server.base_url,
        "ruum-fast",
        RuumTask::TreeAnalysis,
        provider,
    );
    let t504_err = client
        .run_cloud_agent(&t504_cfg, AdkAgentRequest::new("Sys", "Hello"))
        .await
        .expect_err("HTTP 504 must fail without retry");
    assert_eq!(t504_server.recorded().len(), 1);
    assert!(matches!(
        t504_err,
        AdkClientError::CloudGateway {
            status: Some(504),
            is_timeout: true,
            ..
        }
    ));
}
