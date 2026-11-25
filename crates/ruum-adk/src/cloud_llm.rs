//! Dedicated `RuumCloud` LLM transport (`POST /v1/chat/completions`).
//!
//! Implements [`adk_core::Llm`] with per-request timeouts, fixed session IDs,
//! single-shot 401 JWT refresh + replay, 7-tool whitelist filtering, and
//! 56 KiB / 20-message request budget compaction.

use std::collections::HashSet;
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adk_core::{
    AdkError, Content, ErrorCategory, ErrorComponent, FinishReason, Llm,
    LlmRequest, LlmResponse, LlmResponseStream, Part, Result as AdkResult,
    SchemaAdapter, UsageMetadata,
};
use async_trait::async_trait;
use futures::stream;
use serde_json::{Map, Value, json};
use url::{Host, Url};

use crate::model::{AdkClientError, sanitize_error_message};

/// Maximum serialized request payload bytes for `/v1/chat/completions`.
pub const MAX_CLOUD_COMPACTED_BYTES: usize = 56 * 1024;

/// Maximum chat messages allowed by `ruum-server` (`ChatCompletionsBodySchema`).
pub const MAX_CLOUD_MESSAGES: usize = 20;

/// Maximum `max_tokens` accepted by `ruum-server`.
pub const MAX_CLOUD_MAX_TOKENS: u32 = 4096;

/// Maximum tool-calling iterations enforced for `RuumCloud`.
pub const MAX_CLOUD_ITERATIONS: u32 = 8;

/// TCP/TLS connect timeout for Ruum Cloud inference requests.
pub const DEFAULT_CLOUD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-request timeout for Ruum Cloud `/v1/chat/completions`.
pub const DEFAULT_CLOUD_INFERENCE_TIMEOUT: Duration = Duration::from_secs(150);

/// Strict read-only MCP tool whitelist permitted by `ruum-server`.
pub const ALLOWED_RUUM_TOOLS: [&str; 7] = [
    "list_volumes",
    "scan_directory",
    "inspect_subtree",
    "find_reclaimable",
    "check_git_safety",
    "plan_removal",
    "list_ssh_profiles",
];

/// Return whether `name` is one of the 7 allowed read-only Ruum tools.
pub fn is_allowed_ruum_tool(name: &str) -> bool {
    ALLOWED_RUUM_TOOLS.contains(&name)
}

/// Task classification header (`X-Ruum-Task`) sent to `ruum-server`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuumTask {
    #[default]
    LocalAnalysis,
    TreeAnalysis,
}

impl RuumTask {
    /// Canonical header value sent in `X-Ruum-Task`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalAnalysis => "local_analysis",
            Self::TreeAnalysis => "tree_analysis",
        }
    }
}

/// Structured gateway error captured from `/v1/chat/completions` or token
/// refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudGatewayError {
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
    pub trace_id: Option<String>,
    pub retry_after_secs: Option<u64>,
    pub is_timeout: bool,
}

impl fmt::Display for CloudGatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(ref tid) = self.trace_id {
            write!(f, " [Trace: {tid}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for CloudGatewayError {}

/// Async token provider supplying and refreshing short-lived Ruum Cloud JWTs.
#[async_trait]
pub trait CloudTokenProvider: Send + Sync {
    /// Return a currently valid Bearer JWT.
    async fn current_token(&self) -> Result<String, CloudGatewayError>;

    /// Refresh the Bearer JWT once after an HTTP `401` response.
    async fn refresh_token(&self) -> Result<String, CloudGatewayError>;
}

/// Configuration for [`RuumCloudLlm`].
#[derive(Clone)]
pub struct RuumCloudConfig {
    pub base_url: String,
    pub model: String,
    pub task: RuumTask,
    pub client_version: String,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub session_id: Option<String>,
    pub token_provider: Arc<dyn CloudTokenProvider>,
}

impl fmt::Debug for RuumCloudConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuumCloudConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("task", &self.task)
            .field("client_version", &self.client_version)
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl RuumCloudConfig {
    /// Create a new Ruum Cloud LLM config with conservative timeouts (`10s`
    /// connect, `150s` per-request).
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        task: RuumTask,
        token_provider: Arc<dyn CloudTokenProvider>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            task,
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            connect_timeout: DEFAULT_CLOUD_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_CLOUD_INFERENCE_TIMEOUT,
            session_id: None,
            token_provider,
        }
    }

    #[must_use]
    pub fn with_client_version(mut self, version: impl Into<String>) -> Self {
        self.client_version = version.into();
        self
    }

    #[must_use]
    pub const fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

fn validate_gateway_base_url(raw: &str) -> Result<String, AdkClientError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AdkClientError::InvalidConfig(
            "Ruum Cloud base URL must be non-empty".to_owned(),
        ));
    }
    let parsed = Url::parse(trimmed).map_err(|err| {
        AdkClientError::InvalidConfig(format!(
            "invalid Ruum Cloud base URL: {err}"
        ))
    })?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(AdkClientError::InvalidConfig(
            "Ruum Cloud base URL must not contain credentials, query, or \
             fragment"
                .to_owned(),
        ));
    }
    match parsed.scheme() {
        "https" => {
            if parsed.host().is_none() {
                return Err(AdkClientError::InvalidConfig(
                    "Ruum Cloud base URL must include a host".to_owned(),
                ));
            }
        }
        "http" => {
            let is_loopback = match parsed.host() {
                Some(Host::Ipv4(ip)) => ip.is_loopback(),
                Some(Host::Ipv6(ip)) => ip.is_loopback(),
                Some(Host::Domain(domain)) => {
                    domain.eq_ignore_ascii_case("localhost")
                }
                None => false,
            };
            if !is_loopback {
                return Err(AdkClientError::InsecureEndpoint(
                    trimmed.to_owned(),
                ));
            }
        }
        other => {
            return Err(AdkClientError::InvalidConfig(format!(
                "unsupported Ruum Cloud URL scheme '{other}'"
            )));
        }
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// Dedicated `adk_core::Llm` implementation for `POST /v1/chat/completions`.
pub struct RuumCloudLlm {
    http: reqwest::Client,
    completions_url: String,
    model: String,
    task: RuumTask,
    client_version: String,
    request_timeout: Duration,
    session_id: String,
    token_provider: Arc<dyn CloudTokenProvider>,
    last_error: Arc<Mutex<Option<CloudGatewayError>>>,
}

impl fmt::Debug for RuumCloudLlm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuumCloudLlm")
            .field("completions_url", &self.completions_url)
            .field("model", &self.model)
            .field("task", &self.task)
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl RuumCloudLlm {
    /// Construct a [`RuumCloudLlm`] bound to a fixed `session_id` for one run.
    pub fn new(
        config: RuumCloudConfig,
        session_id: impl Into<String>,
    ) -> Result<Self, AdkClientError> {
        adk_core::ensure_crypto_provider();
        let base = validate_gateway_base_url(&config.base_url)?;
        let model = config.model.trim().to_owned();
        if model.is_empty() {
            return Err(AdkClientError::InvalidConfig(
                "Ruum Cloud model name is required".to_owned(),
            ));
        }
        let session = session_id.into();
        if session.trim().is_empty() {
            return Err(AdkClientError::InvalidConfig(
                "Ruum Cloud session_id is required".to_owned(),
            ));
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(config.connect_timeout)
            .build()
            .map_err(|err| {
                AdkClientError::Model(format!(
                    "failed to build Ruum Cloud HTTP client: {err}"
                ))
            })?;

        Ok(Self {
            http,
            completions_url: format!("{base}/v1/chat/completions"),
            model,
            task: config.task,
            client_version: config.client_version,
            request_timeout: config.request_timeout,
            session_id: session,
            token_provider: config.token_provider,
            last_error: Arc::new(Mutex::new(None)),
        })
    }

    /// Return the fixed `X-Ruum-Session` UUID used across all turns.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Clear any recorded structured error prior to an agent run.
    pub fn clear_last_error(&self) {
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = None;
        }
    }

    /// Take the most recent structured [`CloudGatewayError`] captured by this
    /// client, if any.
    pub fn take_last_error(&self) -> Option<CloudGatewayError> {
        self.last_error.lock().ok().and_then(|mut g| g.take())
    }

    fn record_error(&self, err: CloudGatewayError) -> AdkError {
        let status = err.status;
        let msg = err.to_string();
        let category = if err.is_timeout {
            ErrorCategory::Timeout
        } else {
            match status {
                Some(401) => ErrorCategory::Unauthorized,
                Some(403) => ErrorCategory::Forbidden,
                Some(404) => ErrorCategory::NotFound,
                Some(429) => ErrorCategory::RateLimited,
                Some(400 | 422) => ErrorCategory::InvalidInput,
                Some(500..=599) | None => ErrorCategory::Unavailable,
                _ => ErrorCategory::Internal,
            }
        };
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = Some(err);
        }
        let mut adk_err = AdkError::new(
            ErrorComponent::Model,
            category,
            "model.ruum_cloud.gateway",
            msg,
        )
        .with_provider("ruum-cloud");
        if let Some(code) = status {
            adk_err = adk_err.with_upstream_status(code);
        }
        adk_err
    }

    async fn send_once(
        &self,
        token: &str,
        body_bytes: &[u8],
    ) -> Result<reqwest::Response, CloudGatewayError> {
        self.http
            .post(&self.completions_url)
            .timeout(self.request_timeout)
            .bearer_auth(token)
            .header("X-Ruum-Session", &self.session_id)
            .header("X-Ruum-Task", self.task.as_str())
            .header("X-Ruum-Client-Version", &self.client_version)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body_bytes.to_vec())
            .send()
            .await
            .map_err(|err| {
                let is_timeout = err.is_timeout();
                let msg = sanitize_error_message(&err.to_string(), token);
                CloudGatewayError {
                    status: err.status().map(|s| s.as_u16()),
                    code: is_timeout
                        .then(|| "COMMON_UPSTREAM_TIMEOUT".to_owned()),
                    message: if is_timeout {
                        format!("Ruum Cloud inference timed out: {msg}")
                    } else {
                        format!("Ruum Cloud network error: {msg}")
                    },
                    trace_id: None,
                    retry_after_secs: None,
                    is_timeout,
                }
            })
    }
}

#[async_trait]
impl Llm for RuumCloudLlm {
    fn name(&self) -> &str {
        &self.model
    }

    async fn generate_content(
        &self,
        req: LlmRequest,
        _stream: bool,
    ) -> AdkResult<LlmResponseStream> {
        let model_name = if req.model.trim().is_empty() {
            self.model.as_str()
        } else {
            req.model.trim()
        };
        let mut payload =
            build_cloud_request_body(model_name, &req, self.schema_adapter())?;
        compact_chat_request_payload(
            &mut payload,
            MAX_CLOUD_MESSAGES,
            MAX_CLOUD_COMPACTED_BYTES,
        )?;
        let body_bytes = serde_json::to_vec(&payload).map_err(|err| {
            AdkError::model(format!(
                "failed to serialize cloud chat request: {err}"
            ))
        })?;
        let declared_tools: HashSet<String> = req
            .tools
            .keys()
            .filter(|name| is_allowed_ruum_tool(name))
            .cloned()
            .collect();

        let token = match self.token_provider.current_token().await {
            Ok(t) => t,
            Err(err) => return Err(self.record_error(err)),
        };

        let mut resp = match self.send_once(&token, &body_bytes).await {
            Ok(r) => r,
            Err(err) => return Err(self.record_error(err)),
        };

        if resp.status().as_u16() == 401 {
            let first_err =
                extract_gateway_error_from_response(resp, &token).await;
            if first_err.code.as_deref() == Some("AUTH_DEVICE_REVOKED") {
                return Err(self.record_error(first_err));
            }
            let refreshed = match self.token_provider.refresh_token().await {
                Ok(t) => t,
                Err(err) => return Err(self.record_error(err)),
            };
            resp = match self.send_once(&refreshed, &body_bytes).await {
                Ok(r) => r,
                Err(err) => return Err(self.record_error(err)),
            };
            if !resp.status().is_success() {
                let err =
                    extract_gateway_error_from_response(resp, &refreshed).await;
                return Err(self.record_error(err));
            }
        } else if !resp.status().is_success() {
            let err = extract_gateway_error_from_response(resp, &token).await;
            return Err(self.record_error(err));
        }

        let raw_json: Value = resp.json().await.map_err(|err| {
            self.record_error(CloudGatewayError {
                status: None,
                code: Some("COMMON_UPSTREAM_PROTOCOL_ERROR".to_owned()),
                message: format!("invalid Ruum Cloud JSON response: {err}"),
                trace_id: None,
                retry_after_secs: None,
                is_timeout: false,
            })
        })?;

        let llm_resp = parse_cloud_chat_response(&raw_json, &declared_tools)
            .map_err(|msg| {
                self.record_error(CloudGatewayError {
                    status: None,
                    code: Some("COMMON_UPSTREAM_PROTOCOL_ERROR".to_owned()),
                    message: msg,
                    trace_id: None,
                    retry_after_secs: None,
                    is_timeout: false,
                })
            })?;

        let stream_out: Pin<
            Box<dyn futures::Stream<Item = AdkResult<LlmResponse>> + Send>,
        > = Box::pin(stream::iter(vec![Ok(llm_resp)]));
        Ok(stream_out)
    }
}

async fn extract_gateway_error_from_response(
    resp: reqwest::Response,
    token: &str,
) -> CloudGatewayError {
    let status = resp.status().as_u16();
    let header_trace = resp
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let retry_after_secs = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let raw_body = resp.text().await.unwrap_or_default();
    parse_gateway_error_parts(
        status,
        header_trace.as_deref(),
        retry_after_secs,
        &raw_body,
        token,
    )
}

fn parse_gateway_error_parts(
    status: u16,
    header_trace: Option<&str>,
    retry_after_secs: Option<u64>,
    raw_body: &str,
    secret_to_redact: &str,
) -> CloudGatewayError {
    let parsed = serde_json::from_str::<Value>(raw_body).ok();
    let err_obj = parsed.as_ref().and_then(|v| v.get("error"));

    let code = parsed
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(Value::as_str)
        .filter(|c| *c != "OK")
        .or_else(|| err_obj.and_then(|e| e.get("code")).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let trace_id = parsed
        .as_ref()
        .and_then(|v| v.get("traceId").or_else(|| v.get("trace_id")))
        .and_then(Value::as_str)
        .or_else(|| {
            err_obj
                .and_then(|e| e.get("traceId").or_else(|| e.get("trace_id")))
                .and_then(Value::as_str)
        })
        .or(header_trace)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let raw_msg = parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            err_obj
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
        })
        .filter(|s| !s.trim().is_empty())
        .map_or_else(
            || {
                if raw_body.trim().is_empty() {
                    format!("HTTP {status}")
                } else {
                    raw_body.to_owned()
                }
            },
            ToOwned::to_owned,
        );

    let message = sanitize_error_message(&raw_msg, secret_to_redact);
    let is_timeout = matches!(status, 408 | 504)
        || code.as_deref() == Some("COMMON_UPSTREAM_TIMEOUT");

    CloudGatewayError {
        status: Some(status),
        code,
        message,
        trace_id,
        retry_after_secs,
        is_timeout,
    }
}

fn build_cloud_request_body(
    model: &str,
    req: &LlmRequest,
    schema_adapter: &dyn SchemaAdapter,
) -> Result<Value, AdkError> {
    let mut messages = Vec::new();
    for content in &req.contents {
        append_content_messages(content, &mut messages);
    }

    if !messages.iter().any(|m| m["role"] != "system") {
        return Err(AdkError::new(
            ErrorComponent::Model,
            ErrorCategory::InvalidInput,
            "model.ruum_cloud.empty_messages",
            "at least one non-system message is required",
        ));
    }

    let mut obj = Map::new();
    obj.insert("model".to_owned(), Value::String(model.to_owned()));
    obj.insert("messages".to_owned(), Value::Array(messages));
    obj.insert("stream".to_owned(), Value::Bool(false));

    let mut allowed_tools: Vec<(&String, &Value)> = req
        .tools
        .iter()
        .filter(|(name, _)| is_allowed_ruum_tool(name))
        .collect();
    allowed_tools.sort_unstable_by_key(|(name, _)| *name);

    if !allowed_tools.is_empty() {
        let tools_json: Vec<Value> = allowed_tools
            .into_iter()
            .map(|(name, decl)| {
                let mut fn_obj = Map::new();
                fn_obj.insert("name".to_owned(), Value::String(name.clone()));
                if let Some(desc) =
                    decl.get("description").and_then(Value::as_str)
                {
                    fn_obj.insert(
                        "description".to_owned(),
                        Value::String(desc.to_owned()),
                    );
                }
                let params = decl.get("parameters").cloned().map_or_else(
                    || schema_adapter.empty_schema(),
                    |s| schema_adapter.normalize_schema(s),
                );
                fn_obj.insert("parameters".to_owned(), params);
                json!({
                    "type": "function",
                    "function": Value::Object(fn_obj),
                })
            })
            .collect();
        obj.insert("tools".to_owned(), Value::Array(tools_json));
    }

    if let Some(cfg) = &req.config {
        if let Some(temp) = cfg.temperature {
            let clamped = temp.clamp(0.0, 2.0);
            obj.insert("temperature".to_owned(), json!(clamped));
        }
        if let Some(max_out) = cfg.max_output_tokens {
            let clamped = max_out
                .clamp(1, i32::try_from(MAX_CLOUD_MAX_TOKENS).unwrap_or(4096));
            obj.insert("max_tokens".to_owned(), json!(clamped));
        }
    }

    Ok(Value::Object(obj))
}

fn append_content_messages(content: &Content, out: &mut Vec<Value>) {
    match content.role.as_str() {
        "system" => {
            let text = collect_plain_text(&content.parts);
            out.push(json!({ "role": "system", "content": text }));
        }
        "model" | "assistant" => {
            let text = collect_plain_text(&content.parts);
            let tool_calls: Vec<Value> = content
                .parts
                .iter()
                .filter_map(|p| match p {
                    Part::FunctionCall { name, args, id, .. } => {
                        let cid = id
                            .clone()
                            .unwrap_or_else(|| format!("call_{name}"));
                        let args_str = serde_json::to_string(args)
                            .unwrap_or_else(|_| "{}".to_owned());
                        Some(json!({
                            "id": cid,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": args_str,
                            }
                        }))
                    }
                    _ => None,
                })
                .collect();

            let mut msg = Map::new();
            msg.insert(
                "role".to_owned(),
                Value::String("assistant".to_owned()),
            );
            if !text.is_empty() {
                msg.insert("content".to_owned(), Value::String(text));
            } else if tool_calls.is_empty() {
                msg.insert("content".to_owned(), Value::String(" ".to_owned()));
            }
            if !tool_calls.is_empty() {
                msg.insert("tool_calls".to_owned(), Value::Array(tool_calls));
            }
            out.push(Value::Object(msg));
        }
        "function" | "tool" => {
            let mut emitted = false;
            for part in &content.parts {
                if let Part::FunctionResponse {
                    function_response,
                    id,
                    ..
                } = part
                {
                    let serialized =
                        serde_json::to_string(&function_response.response)
                            .unwrap_or_else(|_| "{}".to_owned());
                    let call_id =
                        id.clone().unwrap_or_else(|| "unknown".to_owned());
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": serialized,
                    }));
                    emitted = true;
                }
            }
            if !emitted {
                let text = collect_plain_text(&content.parts);
                out.push(json!({ "role": "user", "content": text }));
            }
        }
        _ => {
            let text = collect_plain_text(&content.parts);
            out.push(json!({ "role": "user", "content": text }));
        }
    }
}

fn collect_plain_text(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn trim_leading_orphaned_tools(mut slice: &[Value]) -> &[Value] {
    while slice
        .first()
        .is_some_and(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
    {
        slice = &slice[1..];
    }
    slice
}

fn rebuild_messages(
    system: Option<&Value>,
    initial_user: &Value,
    tail: &[Value],
) -> Vec<Value> {
    let mut out =
        Vec::with_capacity(usize::from(system.is_some()) + 1 + tail.len());
    if let Some(sys) = system {
        out.push(sys.clone());
    }
    out.push(initial_user.clone());
    out.extend_from_slice(tail);
    out
}

fn truncate_str_to_max_bytes(text: &str, max_bytes: usize) -> String {
    const SUFFIX: &str = "... [truncated]";
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    if max_bytes <= SUFFIX.len() {
        let mut end = max_bytes;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        return text[..end].to_owned();
    }
    let mut end = max_bytes - SUFFIX.len();
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{SUFFIX}", &text[..end])
}

fn serialized_len(payload: &Value) -> Result<usize, AdkError> {
    serde_json::to_vec(payload).map(|v| v.len()).map_err(|err| {
        AdkError::model(format!("failed to size payload: {err}"))
    })
}

fn shrink_largest_message_content(
    payload: &mut Value,
    excess_bytes: usize,
) -> bool {
    let Some(messages) =
        payload.get_mut("messages").and_then(Value::as_array_mut)
    else {
        return false;
    };
    let mut best_idx = None;
    let mut best_len = 0_usize;
    for (idx, msg) in messages.iter().enumerate() {
        if let Some(s) = msg.get("content").and_then(Value::as_str)
            && s.len() > best_len
            && s.len() > 64
        {
            best_len = s.len();
            best_idx = Some(idx);
        }
    }
    let Some(idx) = best_idx else {
        return false;
    };
    let Some(content_val) = messages[idx].get_mut("content") else {
        return false;
    };
    let Some(s) = content_val.as_str() else {
        return false;
    };
    let target_len = s.len().saturating_sub(excess_bytes.max(128)).max(64);
    let truncated = truncate_str_to_max_bytes(s, target_len);
    if truncated.len() >= s.len() {
        return false;
    }
    *content_val = Value::String(truncated);
    true
}

/// Compact a serialized chat completions JSON payload in-place.
///
/// Keeps `messages` within `max_messages` and serialized JSON within
/// `max_bytes`, preserving any system message, the initial user prompt, and the
/// newest turns without orphaning tool responses.
pub fn compact_chat_request_payload(
    payload: &mut Value,
    max_messages: usize,
    max_bytes: usize,
) -> Result<(), AdkError> {
    let raw_msgs = payload
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
        AdkError::new(
            ErrorComponent::Model,
            ErrorCategory::InvalidInput,
            "model.ruum_cloud.missing_messages",
            "chat payload missing messages array",
        )
    })?;

    let first_system = raw_msgs
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
        .cloned();
    let non_system: Vec<Value> = raw_msgs
        .into_iter()
        .filter(|m| m.get("role").and_then(Value::as_str) != Some("system"))
        .collect();

    if non_system.is_empty() {
        return Err(AdkError::new(
            ErrorComponent::Model,
            ErrorCategory::InvalidInput,
            "model.ruum_cloud.empty_messages",
            "at least one non-system message is required",
        ));
    }

    let initial_user = &non_system[0];
    let rest = &non_system[1..];
    let prefix_count = usize::from(first_system.is_some()) + 1;
    let max_tail = max_messages.saturating_sub(prefix_count);

    let mut tail = if rest.len() > max_tail {
        &rest[rest.len() - max_tail..]
    } else {
        rest
    };
    tail = trim_leading_orphaned_tools(tail);

    payload["messages"] = Value::Array(rebuild_messages(
        first_system.as_ref(),
        initial_user,
        tail,
    ));

    // Drop oldest middle turns from `tail` while exceeding `max_bytes`.
    while serialized_len(payload)? > max_bytes && tail.len() > 2 {
        tail = trim_leading_orphaned_tools(&tail[1..]);
        payload["messages"] = Value::Array(rebuild_messages(
            first_system.as_ref(),
            initial_user,
            tail,
        ));
    }

    // Shrink oversized message contents (preserving the latest turn if possible).
    for _ in 0..16 {
        let len = serialized_len(payload)?;
        if len <= max_bytes {
            break;
        }
        if !shrink_largest_message_content(payload, len - max_bytes) {
            break;
        }
    }

    // Last resort: drop remaining tail turns if still over budget.
    if serialized_len(payload)? > max_bytes && !tail.is_empty() {
        payload["messages"] = Value::Array(rebuild_messages(
            first_system.as_ref(),
            initial_user,
            &[],
        ));
        for _ in 0..16 {
            let len = serialized_len(payload)?;
            if len <= max_bytes {
                break;
            }
            if !shrink_largest_message_content(payload, len - max_bytes) {
                break;
            }
        }
    }

    Ok(())
}

fn parse_cloud_chat_response(
    json: &Value,
    declared_tools: &HashSet<String>,
) -> Result<LlmResponse, String> {
    if let Some(err) = json.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("upstream error");
        return Err(msg.to_owned());
    }

    let choice = json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .ok_or_else(|| "Ruum Cloud response missing choices[0]".to_owned())?;

    let message = choice.get("message").ok_or_else(|| {
        "Ruum Cloud response missing choices[0].message".to_owned()
    })?;

    let mut parts = Vec::new();
    if let Some(reasoning) = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        parts.push(Part::Thinking {
            thinking: reasoning.to_owned(),
            signature: None,
        });
    }

    if let Some(text) = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        parts.push(Part::Text {
            text: text.to_owned(),
        });
    }

    if let Some(tool_calls) =
        message.get("tool_calls").and_then(Value::as_array)
    {
        for tc in tool_calls {
            let func = &tc["function"];
            let Some(name) = func.get("name").and_then(Value::as_str) else {
                continue;
            };
            if !declared_tools.is_empty() && !declared_tools.contains(name) {
                return Err(format!(
                    "model attempted undeclared tool '{name}'"
                ));
            }
            let args = match func.get("arguments") {
                Some(Value::String(raw)) => {
                    if raw.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str::<Value>(raw).map_err(|err| {
                            format!(
                                "invalid arguments for tool '{name}': {err}"
                            )
                        })?
                    }
                }
                Some(other @ Value::Object(_)) => other.clone(),
                None | Some(Value::Null) => json!({}),
                Some(other) => {
                    return Err(format!(
                        "unexpected arguments type for tool '{name}': {other}"
                    ));
                }
            };
            let id =
                tc.get("id").and_then(Value::as_str).map(ToOwned::to_owned);
            parts.push(Part::FunctionCall {
                name: name.to_owned(),
                args,
                id,
                thought_signature: None,
            });
        }
    }

    let content = Content {
        role: "model".to_owned(),
        parts,
    };

    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(|fr| match fr {
            "length" => FinishReason::MaxTokens,
            "content_filter" => FinishReason::Safety,
            _ => FinishReason::Stop,
        });

    let tool_call_turn = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .is_some_and(|r| matches!(r, "tool_calls" | "function_call"))
        || content.has_function_calls();

    let usage_metadata = json.get("usage").and_then(|u| {
        let obj = u.as_object()?;
        let i32_field = |k: &str| -> i32 {
            obj.get(k)
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok())
                .unwrap_or_default()
        };
        Some(UsageMetadata {
            prompt_token_count: i32_field("prompt_tokens"),
            candidates_token_count: i32_field("completion_tokens"),
            total_token_count: i32_field("total_tokens"),
            ..Default::default()
        })
    });

    Ok(LlmResponse {
        content: Some(content),
        usage_metadata,
        finish_reason,
        citation_metadata: None,
        partial: false,
        turn_complete: !tool_call_turn,
        interrupted: false,
        error_code: None,
        error_message: None,
        provider_metadata: None,
        interaction_id: None,
    })
}
