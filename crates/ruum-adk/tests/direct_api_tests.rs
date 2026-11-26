use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ruum_adk::{
    AdkAgentRequest, AdkClientError, AdkDirectClient, AdkModelConfig,
    AdkToolDef, AdkToolExecutor, AiProtocol, CancelToken, normalize_endpoints,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zeroize::Zeroizing;

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

#[derive(Debug, Clone)]
struct MockReply {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: String,
    delay: Duration,
}

impl MockReply {
    fn json(status: u16, body: &Value) -> Self {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            _ => "Error",
        };
        Self {
            status,
            reason,
            content_type: "application/json",
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }

    fn openai_sse(
        content: Option<&str>,
        reasoning: Option<&str>,
        tool_call: Option<(&str, &str, &Value)>,
    ) -> Self {
        let mut out = String::new();
        if let Some(r) = reasoning {
            let chunk = json!({
                "id": "chatcmpl-1",
                "object": "chat.completion.chunk",
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "reasoning_content": r},
                    "finish_reason": null
                }]
            });
            let _ = write!(out, "data: {chunk}\n\n");
        }
        if let Some(c) = content {
            let chunk = json!({
                "id": "chatcmpl-1",
                "object": "chat.completion.chunk",
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": c},
                    "finish_reason": null
                }]
            });
            let _ = write!(out, "data: {chunk}\n\n");
        }
        if let Some((id, name, args)) = tool_call {
            let chunk = json!({
                "id": "chatcmpl-1",
                "object": "chat.completion.chunk",
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "tool_calls": [{
                            "index": 0,
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": args.to_string()
                            }
                        }]
                    },
                    "finish_reason": null
                }]
            });
            let _ = write!(out, "data: {chunk}\n\n");
            let done_chunk = json!({
                "id": "chatcmpl-1",
                "object": "chat.completion.chunk",
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "tool_calls"
                }]
            });
            let _ = write!(out, "data: {done_chunk}\n\n");
        } else {
            let done_chunk = json!({
                "id": "chatcmpl-1",
                "object": "chat.completion.chunk",
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }]
            });
            let _ = write!(out, "data: {done_chunk}\n\n");
        }
        out.push_str("data: [DONE]\n\n");
        Self {
            status: 200,
            reason: "OK",
            content_type: "text/event-stream",
            body: out,
            delay: Duration::ZERO,
        }
    }

    fn anthropic_sse(
        thinking: Option<&str>,
        text: Option<&str>,
        tool_use: Option<(&str, &str, &Value)>,
    ) -> Self {
        let mut out = String::new();
        let start = json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "test-model",
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": 10, "output_tokens": 0}
            }
        });
        let _ = write!(out, "event: message_start\ndata: {start}\n\n");

        let mut idx = 0_usize;
        if let Some(t) = thinking {
            let bs = json!({
                "type": "content_block_start",
                "index": idx,
                "content_block": {
                    "type": "thinking",
                    "thinking": "",
                    "signature": ""
                }
            });
            let bd = json!({
                "type": "content_block_delta",
                "index": idx,
                "delta": {"type": "thinking_delta", "thinking": t}
            });
            let sd = json!({
                "type": "content_block_delta",
                "index": idx,
                "delta": {"type": "signature_delta", "signature": "sig_mock"}
            });
            let be = json!({"type": "content_block_stop", "index": idx});
            let _ = write!(
                out,
                "event: content_block_start\ndata: {bs}\n\n\
                 event: content_block_delta\ndata: {bd}\n\n\
                 event: content_block_delta\ndata: {sd}\n\n\
                 event: content_block_stop\ndata: {be}\n\n"
            );
            idx += 1;
        }
        if let Some(txt) = text {
            let bs = json!({
                "type": "content_block_start",
                "index": idx,
                "content_block": {"type": "text", "text": ""}
            });
            let bd = json!({
                "type": "content_block_delta",
                "index": idx,
                "delta": {"type": "text_delta", "text": txt}
            });
            let be = json!({"type": "content_block_stop", "index": idx});
            let _ = write!(
                out,
                "event: content_block_start\ndata: {bs}\n\n\
                 event: content_block_delta\ndata: {bd}\n\n\
                 event: content_block_stop\ndata: {be}\n\n"
            );
            idx += 1;
        }
        let stop_reason = if let Some((id, name, input)) = tool_use {
            let bs = json!({
                "type": "content_block_start",
                "index": idx,
                "content_block": {
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": {}
                }
            });
            let bd = json!({
                "type": "content_block_delta",
                "index": idx,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": input.to_string()
                }
            });
            let be = json!({"type": "content_block_stop", "index": idx});
            let _ = write!(
                out,
                "event: content_block_start\ndata: {bs}\n\n\
                 event: content_block_delta\ndata: {bd}\n\n\
                 event: content_block_stop\ndata: {be}\n\n"
            );
            "tool_use"
        } else {
            "end_turn"
        };

        let md = json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
            "usage": {"output_tokens": 5}
        });
        let _ = write!(out, "event: message_delta\ndata: {md}\n\n");
        out.push_str(
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );

        Self {
            status: 200,
            reason: "OK",
            content_type: "text/event-stream",
            body: out,
            delay: Duration::ZERO,
        }
    }
}

struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    fn recorded(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock requests").clone()
    }
}

async fn start_mock_server(replies: Vec<MockReply>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback mock server");
    let addr = listener.local_addr().expect("mock local_addr");
    let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
    let requests = Arc::new(Mutex::new(Vec::new()));

    let q_clone = Arc::clone(&queue);
    let r_clone = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let q = Arc::clone(&q_clone);
            let r = Arc::clone(&r_clone);
            tokio::spawn(async move {
                let Some(req) = read_http_request(&mut stream).await else {
                    return;
                };
                r.lock().expect("lock requests").push(req);
                let reply = q
                    .lock()
                    .expect("lock replies")
                    .pop_front()
                    .unwrap_or_else(|| {
                        MockReply::json(
                            500,
                            &json!({"error": {"message": "no mock reply left"}}),
                        )
                    });
                if !reply.delay.is_zero() {
                    tokio::time::sleep(reply.delay).await;
                }
                let raw = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.status,
                    reply.reason,
                    reply.content_type,
                    reply.body.len(),
                    reply.body
                );
                let _ = stream.write_all(raw.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    MockServer {
        base_url: format!("http://{addr}"),
        requests,
        _task: task,
    }
}

async fn read_http_request(
    stream: &mut tokio::net::TcpStream,
) -> Option<RecordedRequest> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0_u8; 2048];
    let header_end;
    loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            header_end = pos;
            break;
        }
    }

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let req_line = lines.next()?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            if key == "content-length" {
                content_length = val.parse().unwrap_or(0);
            }
            headers.push((key, val));
        }
    }

    let body_start = header_end + 4;
    while buf.len().saturating_sub(body_start) < content_length {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }

    let body_bytes =
        &buf[body_start..buf.len().min(body_start + content_length)];
    let body = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body_bytes).unwrap_or(Value::Null)
    };

    Some(RecordedRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

struct EchoSizeTool {
    calls: Arc<AtomicUsize>,
    bytes_to_return: u64,
}

#[async_trait]
impl AdkToolExecutor for EchoSizeTool {
    async fn execute(&self, args: Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        Ok(json!({
            "path": path,
            "bytes": self.bytes_to_return,
        }))
    }
}

fn test_config(
    protocol: AiProtocol,
    base_url: impl Into<String>,
) -> AdkModelConfig {
    AdkModelConfig::new(protocol, "test-api-key")
        .with_base_url(base_url)
        .with_model("test-model")
        .with_timeout(Duration::from_secs(5))
}

#[test]
fn normalize_endpoints_openai_variants() {
    let default_ep =
        normalize_endpoints(AiProtocol::OpenAi, None).expect("default openai");
    assert_eq!(default_ep.sdk_base_url, "https://api.openai.com/v1");
    assert_eq!(default_ep.models_url, "https://api.openai.com/v1/models");

    let mimo = normalize_endpoints(
        AiProtocol::OpenAi,
        Some("https://api.xiaomimimo.com/v1/"),
    )
    .expect("mimo openai");
    assert_eq!(mimo.sdk_base_url, "https://api.xiaomimimo.com/v1");
    assert_eq!(mimo.models_url, "https://api.xiaomimimo.com/v1/models");

    let deepseek = normalize_endpoints(
        AiProtocol::OpenAi,
        Some("https://api.deepseek.com"),
    )
    .expect("deepseek openai");
    assert_eq!(deepseek.sdk_base_url, "https://api.deepseek.com");
    assert_eq!(deepseek.models_url, "https://api.deepseek.com/models");

    let chat_suffix = normalize_endpoints(
        AiProtocol::OpenAi,
        Some("https://api.openai.com/v1/chat/completions/"),
    )
    .expect("strip chat/completions");
    assert_eq!(chat_suffix.sdk_base_url, "https://api.openai.com/v1");
    assert_eq!(chat_suffix.models_url, "https://api.openai.com/v1/models");

    let models_suffix = normalize_endpoints(
        AiProtocol::OpenAi,
        Some("https://api.openai.com/v1/models"),
    )
    .expect("strip /models");
    assert_eq!(models_suffix.sdk_base_url, "https://api.openai.com/v1");
}

#[test]
fn normalize_endpoints_anthropic_variants() {
    let default_ep = normalize_endpoints(AiProtocol::Anthropic, None)
        .expect("default anthropic");
    assert_eq!(default_ep.sdk_base_url, "https://api.anthropic.com");
    assert_eq!(default_ep.models_url, "https://api.anthropic.com/v1/models");
    assert!(default_ep.is_official_anthropic);

    let v1_suffix = normalize_endpoints(
        AiProtocol::Anthropic,
        Some("https://api.anthropic.com/v1"),
    )
    .expect("strip /v1");
    assert_eq!(v1_suffix.sdk_base_url, "https://api.anthropic.com");
    assert!(v1_suffix.is_official_anthropic);

    let msg_suffix = normalize_endpoints(
        AiProtocol::Anthropic,
        Some("https://api.anthropic.com/v1/messages"),
    )
    .expect("strip /v1/messages");
    assert_eq!(msg_suffix.sdk_base_url, "https://api.anthropic.com");

    let mimo = normalize_endpoints(
        AiProtocol::Anthropic,
        Some("https://api.xiaomimimo.com/anthropic/v1/messages/"),
    )
    .expect("mimo anthropic");
    assert_eq!(mimo.sdk_base_url, "https://api.xiaomimimo.com/anthropic");
    assert_eq!(
        mimo.models_url,
        "https://api.xiaomimimo.com/anthropic/v1/models"
    );
    assert!(!mimo.is_official_anthropic);

    let deepseek = normalize_endpoints(
        AiProtocol::Anthropic,
        Some("https://api.deepseek.com/anthropic/v1"),
    )
    .expect("deepseek anthropic");
    assert_eq!(deepseek.sdk_base_url, "https://api.deepseek.com/anthropic");
    assert!(!deepseek.is_official_anthropic);

    let bad_scheme =
        normalize_endpoints(AiProtocol::OpenAi, Some("ftp://example.com/v1"));
    assert!(matches!(bad_scheme, Err(AdkClientError::InvalidConfig(_))));
}

#[tokio::test]
async fn insecure_http_policy_allows_loopback_and_blocks_remote_without_opt_in()
{
    let loopback = normalize_endpoints(
        AiProtocol::OpenAi,
        Some("http://127.0.0.1:8080/v1"),
    )
    .expect("loopback v4");
    assert!(loopback.is_loopback);

    let v6_loopback = normalize_endpoints(
        AiProtocol::Anthropic,
        Some("http://[::1]:8080/anthropic"),
    )
    .expect("loopback v6");
    assert!(v6_loopback.is_loopback);

    let client = AdkDirectClient::new().expect("create client");
    for protocol in [AiProtocol::OpenAi, AiProtocol::Anthropic] {
        let cfg = AdkModelConfig {
            protocol,
            base_url: Some("http://10.20.30.40:8080/v1".to_string()),
            api_key: Zeroizing::new("test-api-key".to_string()),
            model: Some("test-model".to_string()),
            timeout: Duration::from_secs(2),
            max_tokens: None,
            allow_insecure_http: false,
        };
        let err = client
            .list_models(&cfg)
            .await
            .expect_err("remote http should fail without opt-in");
        assert!(matches!(err, AdkClientError::InsecureEndpoint(_)));
    }
}

#[tokio::test]
async fn debug_and_error_redaction_never_leaks_api_key() {
    let cfg = test_config(AiProtocol::OpenAi, "http://127.0.0.1:9999/v1");
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("test-api-key"));
    assert!(dbg.contains("[REDACTED]"));

    let server = start_mock_server(vec![MockReply::json(
        401,
        &json!({
            "error": {
                "message": "bad token test-api-key rejected"
            }
        }),
    )])
    .await;

    let client = AdkDirectClient::new().expect("create client");
    let err = client
        .test_connection(&test_config(
            AiProtocol::OpenAi,
            format!("{}/v1", server.base_url),
        ))
        .await
        .expect_err("401 should fail");
    assert!(matches!(err, AdkClientError::Unauthorized { .. }));
    assert!(!err.to_string().contains("test-api-key"));
    assert!(!format!("{err:?}").contains("test-api-key"));
}

#[tokio::test]
async fn openai_single_turn_strips_think_tags() {
    let server = start_mock_server(vec![MockReply::openai_sse(
        Some("<think>checking sizes</think>\nFinal clean answer."),
        Some("extra trace"),
        None,
    )])
    .await;

    let client = AdkDirectClient::new().expect("create client");
    let cfg =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server.base_url));
    let req = AdkAgentRequest::new("You are helpful.", "Summarize.");
    let resp = client.run_agent(&cfg, req).await.expect("run openai");

    assert_eq!(resp.text, "Final clean answer.");
    let reasoning = resp.reasoning.expect("reasoning captured");
    assert!(reasoning.contains("checking sizes"));
    assert_eq!(resp.tool_calls_made, 0);

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].path, "/v1/chat/completions");
    assert!(
        recorded[0].headers.iter().any(|(k, v)| {
            k == "authorization" && v == "Bearer test-api-key"
        })
    );
}

#[tokio::test]
async fn anthropic_single_turn_normalizes_url_and_disables_caching() {
    let server = start_mock_server(vec![MockReply::anthropic_sse(
        Some("pondering"),
        Some("<think>inline</think>Claude answer."),
        None,
    )])
    .await;

    let client = AdkDirectClient::new().expect("create client");
    // Intentionally pass /v1/messages to verify normalization strips it.
    let cfg = test_config(
        AiProtocol::Anthropic,
        format!("{}/v1/messages", server.base_url),
    );
    let req = AdkAgentRequest::new("System instruction", "Hello Claude");
    let resp = client.run_agent(&cfg, req).await.expect("run anthropic");

    assert_eq!(resp.text, "Claude answer.");
    let reasoning = resp.reasoning.expect("reasoning captured");
    assert!(reasoning.contains("pondering"));
    assert!(reasoning.contains("inline"));

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].path, "/v1/messages");
    assert!(
        recorded[0]
            .headers
            .iter()
            .any(|(k, v)| { k == "x-api-key" && v == "test-api-key" })
    );
    let body_str = recorded[0].body.to_string();
    assert!(!body_str.contains("cache_control"));
}

#[tokio::test]
async fn openai_multi_turn_tool_calling_loop() {
    let tool_args = json!({"path": "/var/log"});
    let server = start_mock_server(vec![
        MockReply::openai_sse(
            None,
            None,
            Some(("call_1", "inspect_size", &tool_args)),
        ),
        MockReply::openai_sse(
            Some("Found 4096 bytes in /var/log."),
            None,
            None,
        ),
    ])
    .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let tool = Arc::new(EchoSizeTool {
        calls: Arc::clone(&calls),
        bytes_to_return: 4096,
    });

    let mut req = AdkAgentRequest::new("Inspect disk.", "Check /var/log");
    req.tools.push((
        AdkToolDef {
            name: "inspect_size".to_string(),
            description: "Inspect directory size".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        },
        tool,
    ));

    let client = AdkDirectClient::new().expect("create client");
    let cfg =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server.base_url));
    let resp = client
        .run_agent(&cfg, req)
        .await
        .expect("multi-turn openai");

    assert_eq!(resp.text, "Found 4096 bytes in /var/log.");
    assert_eq!(resp.tool_calls_made, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].body["tools"].is_array());
    let second_msgs = recorded[1].body["messages"].to_string();
    assert!(second_msgs.contains("4096"));
}

#[tokio::test]
async fn anthropic_multi_turn_tool_calling_loop() {
    let tool_args = json!({"path": "/tmp/cache"});
    let server = start_mock_server(vec![
        MockReply::anthropic_sse(
            None,
            None,
            Some(("toolu_1", "inspect_size", &tool_args)),
        ),
        MockReply::anthropic_sse(
            None,
            Some("Anthropic found 8192 bytes."),
            None,
        ),
    ])
    .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let tool = Arc::new(EchoSizeTool {
        calls: Arc::clone(&calls),
        bytes_to_return: 8192,
    });

    let mut req = AdkAgentRequest::new("Inspect disk.", "Check /tmp/cache");
    req.tools.push((
        AdkToolDef {
            name: "inspect_size".to_string(),
            description: "Inspect directory size".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        },
        tool,
    ));

    let client = AdkDirectClient::new().expect("create client");
    let cfg = test_config(AiProtocol::Anthropic, &server.base_url);
    let resp = client
        .run_agent(&cfg, req)
        .await
        .expect("multi-turn anthropic");

    assert_eq!(resp.text, "Anthropic found 8192 bytes.");
    assert_eq!(resp.tool_calls_made, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].body["tools"].is_array());
    let second_msgs = recorded[1].body["messages"].to_string();
    assert!(second_msgs.contains("8192"));
}

#[tokio::test]
async fn max_iterations_circuit_breaker_stops_infinite_tool_loop() {
    let tool_args = json!({"path": "/var"});
    let tool_reply = MockReply::openai_sse(
        None,
        None,
        Some(("call_loop", "inspect_size", &tool_args)),
    );
    let server = start_mock_server(vec![
        tool_reply.clone(),
        tool_reply.clone(),
        tool_reply,
    ])
    .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let tool = Arc::new(EchoSizeTool {
        calls: Arc::clone(&calls),
        bytes_to_return: 1,
    });
    let mut req = AdkAgentRequest::new("Loop forever", "Go");
    req.max_iterations = 2;
    req.tools.push((
        AdkToolDef {
            name: "inspect_size".to_string(),
            description: "Inspect size".to_string(),
            parameters_schema: json!({"type": "object"}),
        },
        tool,
    ));

    let client = AdkDirectClient::new().expect("create client");
    let cfg =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server.base_url));
    let err = client
        .run_agent(&cfg, req)
        .await
        .expect_err("max_iterations must trigger");

    assert!(matches!(err, AdkClientError::MaxIterationsExceeded(2)));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn fallback_without_tools_on_http_400_and_never_masks_401() {
    let server_400 = start_mock_server(vec![
        MockReply::json(
            400,
            &json!({"error": {"message": "tools are not supported by model"}}),
        ),
        MockReply::openai_sse(Some("Fallback succeeded."), None, None),
    ])
    .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let tool = Arc::new(EchoSizeTool {
        calls: Arc::clone(&calls),
        bytes_to_return: 1,
    });
    let mut req = AdkAgentRequest::new("Sys", "Analyze");
    req.tools.push((
        AdkToolDef {
            name: "inspect_size".to_string(),
            description: "Inspect size".to_string(),
            parameters_schema: json!({"type": "object"}),
        },
        tool,
    ));

    let client = AdkDirectClient::new().expect("create client");
    let cfg =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server_400.base_url));
    let resp = client
        .run_agent(&cfg, req)
        .await
        .expect("400 tools unsupported should fall back");
    assert_eq!(resp.text, "Fallback succeeded.");
    assert_eq!(resp.tool_calls_made, 0);

    let recorded = server_400.recorded();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].body.get("tools").is_some());
    assert!(recorded[1].body.get("tools").is_none());

    // 401 must NEVER trigger fallback.
    let server_401 = start_mock_server(vec![MockReply::json(
        401,
        &json!({"error": {"message": "invalid api key"}}),
    )])
    .await;
    let mut req_401 = AdkAgentRequest::new("Sys", "Analyze");
    req_401.tools.push((
        AdkToolDef {
            name: "inspect_size".to_string(),
            description: "Inspect size".to_string(),
            parameters_schema: json!({"type": "object"}),
        },
        Arc::new(EchoSizeTool {
            calls: Arc::new(AtomicUsize::new(0)),
            bytes_to_return: 1,
        }),
    ));
    let cfg_401 =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server_401.base_url));
    let err = client
        .run_agent(&cfg_401, req_401)
        .await
        .expect_err("401 must fail immediately");
    assert!(matches!(err, AdkClientError::Unauthorized { .. }));
    assert_eq!(server_401.recorded().len(), 1);
}

#[tokio::test]
async fn list_models_and_probe_with_404_fallback() {
    let server = start_mock_server(vec![
        MockReply::json(
            200,
            &json!({
                "data": [
                    {"id": "model-alpha", "display_name": "Alpha"},
                    {"id": "model-beta"}
                ]
            }),
        ),
        MockReply::json(
            200,
            &json!({
                "data": [{"id": "model-alpha"}]
            }),
        ),
    ])
    .await;

    let client = AdkDirectClient::new().expect("create client");
    let cfg =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server.base_url));
    let models = client.list_models(&cfg).await.expect("list models");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "model-alpha");
    assert_eq!(models[0].label, "Alpha");
    assert_eq!(models[1].id, "model-beta");

    let probe = client.test_connection(&cfg).await.expect("probe models");
    assert_eq!(probe.model_count, Some(1));

    // 404 on /models falls back to minimal inference probe when model is set.
    let fallback_server = start_mock_server(vec![
        MockReply::json(404, &json!({"error": {"message": "not found"}})),
        MockReply::json(
            200,
            &json!({
                "id": "msg_probe",
                "type": "message",
                "role": "assistant",
                "model": "test-model",
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }),
        ),
    ])
    .await;

    let anth_cfg =
        test_config(AiProtocol::Anthropic, &fallback_server.base_url);
    let probe = client
        .test_connection(&anth_cfg)
        .await
        .expect("404 should fall back to minimal probe");
    assert_eq!(probe.model_count, None);

    let recorded = fallback_server.recorded();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].method, "GET");
    assert_eq!(recorded[0].path, "/v1/models");
    assert_eq!(recorded[1].method, "POST");
    assert_eq!(recorded[1].path, "/v1/messages");
    assert_eq!(recorded[1].body["max_tokens"], 1);
}

#[tokio::test]
async fn timeout_enforced_without_hanging() {
    let mut slow = MockReply::json(200, &json!({"data": []}));
    slow.delay = Duration::from_secs(3);
    let server = start_mock_server(vec![slow]).await;

    let client = AdkDirectClient::new().expect("create client");
    let cfg =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server.base_url))
            .with_timeout(Duration::from_millis(120));

    let err = client
        .list_models(&cfg)
        .await
        .expect_err("slow server must time out");
    assert!(matches!(err, AdkClientError::Timeout(_)));
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_client_works_outside_and_inside_tokio_runtime() {
    use ruum_adk::BlockingAdkClient;

    let blocking = BlockingAdkClient::new().expect("create blocking client");

    // Start mock server on a dedicated runtime so we can test outside & inside.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test rt");

    let (base_url, _guard) = rt.block_on(async {
        let srv = start_mock_server(vec![
            MockReply::openai_sse(Some("Outside OK"), None, None),
            MockReply::openai_sse(Some("Inside OK"), None, None),
        ])
        .await;
        (srv.base_url.clone(), srv)
    });

    let cfg = test_config(AiProtocol::OpenAi, format!("{base_url}/v1"));

    // 1. Outside any Tokio runtime.
    let resp_out = blocking
        .run_agent(&cfg, AdkAgentRequest::new("Sys", "Hi"))
        .expect("blocking outside runtime");
    assert_eq!(resp_out.text, "Outside OK");

    // 2. Inside an active Tokio runtime (nested runtime guard).
    let resp_in = rt.block_on(async {
        blocking.run_agent(&cfg, AdkAgentRequest::new("Sys", "Hi"))
    });
    assert_eq!(resp_in.expect("blocking inside runtime").text, "Inside OK");
}

#[tokio::test]
async fn run_agent_cancel_token_aborts_in_flight_request() {
    let mut slow = MockReply::openai_sse(Some("Too late"), None, None);
    slow.delay = Duration::from_secs(3);
    let server = start_mock_server(vec![slow]).await;

    let client = AdkDirectClient::new().expect("create client");
    let cfg =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server.base_url))
            .with_timeout(Duration::from_secs(5));
    let cancel = CancelToken::new();
    let cancel_trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_trigger.cancel();
    });

    let err = client
        .run_agent(&cfg, AdkAgentRequest::new("Sys", "Hi").with_cancel(cancel))
        .await
        .expect_err("cancelled run must abort");
    assert!(matches!(err, AdkClientError::Cancelled), "got {err:?}");
}

#[tokio::test]
async fn probe_model_supports_openai_anthropic_json_fallback_thinking_budget_and_cancel()
 {
    let openai_sse = MockReply::openai_sse(Some("pong"), None, None);
    let anthropic_sse = MockReply::anthropic_sse(Some("thinking"), None, None);
    let json_fallback = MockReply::json(
        200,
        &json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "pong"},
                "finish_reason": "stop"
            }]
        }),
    );
    let budget_exhausted = MockReply {
        status: 200,
        reason: "OK",
        content_type: "text/event-stream",
        body: format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({
                "id": "chatcmpl-budget",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "length"
                }]
            })
        ),
        delay: Duration::ZERO,
    };

    let server = start_mock_server(vec![
        openai_sse,
        anthropic_sse,
        json_fallback,
        budget_exhausted,
    ])
    .await;

    let client = AdkDirectClient::new().expect("create client");
    let openai_cfg =
        test_config(AiProtocol::OpenAi, format!("{}/v1", server.base_url))
            .with_timeout(Duration::from_secs(5));
    let r1 = client
        .probe_model(&openai_cfg, None)
        .await
        .expect("openai sse probe");
    assert!(!r1.budget_exhausted);

    let anth_cfg = test_config(AiProtocol::Anthropic, &server.base_url)
        .with_timeout(Duration::from_secs(5));
    let r2 = client
        .probe_model(&anth_cfg, None)
        .await
        .expect("anthropic thinking sse probe");
    assert!(!r2.budget_exhausted);

    let r3 = client
        .probe_model(&openai_cfg, None)
        .await
        .expect("json fallback probe");
    assert!(!r3.budget_exhausted);

    let r4 = client
        .probe_model(&openai_cfg, None)
        .await
        .expect("thinking budget exhausted probe");
    assert!(r4.budget_exhausted);

    let cancel = CancelToken::new();
    cancel.cancel();
    let aborted = client
        .probe_model(&openai_cfg, Some(&cancel))
        .await
        .expect_err("cancelled probe must return Cancelled");
    assert!(matches!(aborted, AdkClientError::Cancelled));

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 4);
    assert_eq!(recorded[0].body["stream"], true);
    assert_eq!(recorded[0].body["max_tokens"], 64);
    assert_eq!(recorded[1].body["stream"], true);
    assert_eq!(recorded[1].body["max_tokens"], 64);
}
