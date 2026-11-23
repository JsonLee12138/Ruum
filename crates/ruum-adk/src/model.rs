//! Protocol configuration, endpoint normalization, and model discovery.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use adk_core::{AdkError, Content, ErrorCategory, Llm, LlmRequest};
use adk_model::anthropic::{AnthropicClient, AnthropicConfig};
use adk_model::retry::RetryConfig;
use adk_model::{OpenAICompatible, OpenAICompatibleConfig};
use futures::StreamExt;
use serde_json::Value;
use url::Url;
use zeroize::Zeroizing;

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_ANTHROPIC_MAX_TOKENS: u32 = 4096;
const ANTHROPIC_API_VERSION: &str = "2023-06-01";
const MAX_ERROR_SNIPPET_LEN: usize = 512;

/// Wire protocol spoken by a direct AI API endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiProtocol {
    /// `OpenAI` Chat Completions compatible protocol (`/v1/chat/completions`).
    #[default]
    OpenAi,
    /// Anthropic Messages protocol (`/v1/messages`).
    Anthropic,
}

impl AiProtocol {
    /// Default official base URL for this protocol.
    pub const fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => DEFAULT_OPENAI_BASE_URL,
            Self::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
        }
    }

    /// Canonical lowercase identifier stored in configuration.
    pub const fn id(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    /// Parses a protocol identifier (`"openai"` or `"anthropic"`).
    pub fn from_id(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }
}

impl fmt::Display for AiProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Configuration for connecting to a direct AI model endpoint.
#[derive(Clone)]
pub struct AdkModelConfig {
    /// Wire protocol (`OpenAi` or `Anthropic`).
    pub protocol: AiProtocol,
    /// Optional custom base URL; defaults to the protocol's official endpoint.
    pub base_url: Option<String>,
    /// Secret API key kept zeroized on drop and redacted from `Debug`.
    pub api_key: Zeroizing<String>,
    /// Target model identifier (required for inference, optional for listing).
    pub model: Option<String>,
    /// Overall request timeout for probes and agent runs.
    pub timeout: Duration,
    /// Optional max output tokens override (Anthropic defaults to 4096).
    pub max_tokens: Option<u32>,
    /// Permits plain `http://` on non-loopback hosts when `true`.
    pub allow_insecure_http: bool,
}

impl Default for AdkModelConfig {
    fn default() -> Self {
        Self {
            protocol: AiProtocol::OpenAi,
            base_url: None,
            api_key: Zeroizing::new(String::new()),
            model: None,
            timeout: Duration::from_mins(2),
            max_tokens: None,
            allow_insecure_http: false,
        }
    }
}

impl AdkModelConfig {
    /// Creates a new configuration with sensible defaults for `protocol`.
    pub fn new(protocol: AiProtocol, api_key: impl Into<String>) -> Self {
        Self {
            protocol,
            api_key: Zeroizing::new(api_key.into()),
            ..Self::default()
        }
    }

    /// Sets a custom base URL.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Sets the target model identifier.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Sets the overall operation timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the max output tokens limit.
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Allows plain `http://` on non-loopback hosts when set to `true`.
    #[must_use]
    pub const fn with_allow_insecure_http(mut self, allow: bool) -> Self {
        self.allow_insecure_http = allow;
        self
    }
}

impl fmt::Debug for AdkModelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdkModelConfig")
            .field("protocol", &self.protocol)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("max_tokens", &self.max_tokens)
            .field("allow_insecure_http", &self.allow_insecure_http)
            .finish()
    }
}

/// Normalized endpoints derived from a protocol and user-supplied base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedEndpoints {
    /// Protocol associated with these endpoints.
    pub protocol: AiProtocol,
    /// Base URL passed to the underlying `adk-model` client (no trailing `/`).
    pub sdk_base_url: String,
    /// Full URL for `GET` model discovery.
    pub models_url: String,
    /// Whether this targets the official `https://api.anthropic.com` endpoint.
    pub is_official_anthropic: bool,
    /// Whether the host resolves to a loopback address (`localhost`/`127.0.0.1`/`::1`).
    pub is_loopback: bool,
    /// Whether the scheme is `https`.
    pub is_https: bool,
}

/// A model entry discovered from a provider's `/models` endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteModel {
    /// Machine identifier passed in completion requests.
    pub id: String,
    /// Human-friendly display label (falls back to `id`).
    pub label: String,
}

/// Result returned by [`AdkDirectClient::test_connection`](crate::AdkDirectClient::test_connection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionProbeResult {
    /// Active protocol tested.
    pub protocol: AiProtocol,
    /// Normalized SDK base endpoint that was probed.
    pub endpoint: String,
    /// Number of models returned by `/models`, or `None` if fallback was used.
    pub model_count: Option<usize>,
    /// Human-readable probe summary.
    pub message: String,
}

/// Sanitized error returned by `ruum-adk` operations; never exposes `api_key`.
#[derive(Debug, thiserror::Error)]
pub enum AdkClientError {
    /// Invalid configuration or malformed URL.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// Non-loopback `http://` endpoint rejected when `allow_insecure_http` is false.
    #[error(
        "insecure HTTP endpoint rejected (set allow_insecure_http for \
         non-loopback http://): {0}"
    )]
    InsecureEndpoint(String),
    /// Upstream returned HTTP 401 or 403 authentication failure.
    #[error("authentication failed (HTTP {status}): {message}")]
    Unauthorized {
        /// HTTP status code (`401` or `403`).
        status: u16,
        /// Sanitized error message.
        message: String,
    },
    /// Upstream HTTP failure with optional status code.
    #[error("API request failed{}: {message}", format_status(*status))]
    Http {
        /// Optional HTTP status code.
        status: Option<u16>,
        /// Sanitized error message.
        message: String,
    },
    /// Upstream indicated tool/function calling is unsupported (e.g. HTTP 400).
    #[error("model does not support tools: {0}")]
    ToolsUnsupported(String),
    /// Operation exceeded its configured timeout.
    #[error("request timed out after {0:?}")]
    Timeout(Duration),
    /// Agent exceeded `max_iterations` without completing.
    #[error("agent exceeded max iterations ({0}) without a final response")]
    MaxIterationsExceeded(u32),
    /// Model or runtime failure.
    #[error("model error: {0}")]
    Model(String),
}

fn format_status(status: Option<u16>) -> String {
    status.map_or_else(String::new, |s| format!(" (HTTP {s})"))
}

/// Strips any occurrence of `api_key` and bounds error message length.
pub fn sanitize_error_message(raw: &str, api_key: &str) -> String {
    let mut cleaned = if api_key.is_empty() {
        raw.to_string()
    } else {
        raw.replace(api_key, "[REDACTED]")
    };
    let trimmed_key = api_key.trim();
    if !trimmed_key.is_empty() && trimmed_key != api_key {
        cleaned = cleaned.replace(trimmed_key, "[REDACTED]");
    }
    let single_line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.len() <= MAX_ERROR_SNIPPET_LEN {
        return single_line;
    }
    let mut end = MAX_ERROR_SNIPPET_LEN;
    while !single_line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &single_line[..end])
}

fn strip_suffix_ci<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    if value.len() < suffix.len() {
        return None;
    }
    let split = value.len() - suffix.len();
    if !value.is_char_boundary(split) {
        return None;
    }
    if value[split..].eq_ignore_ascii_case(suffix) {
        Some(&value[..split])
    } else {
        None
    }
}

/// Normalizes a user-supplied base URL into SDK and `/models` endpoints.
pub fn normalize_endpoints(
    protocol: AiProtocol,
    raw_base_url: Option<&str>,
) -> Result<NormalizedEndpoints, AdkClientError> {
    let candidate = raw_base_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| protocol.default_base_url());

    let mut parsed = Url::parse(candidate).map_err(|err| {
        AdkClientError::InvalidConfig(format!("invalid base URL: {err}"))
    })?;

    let is_https = match parsed.scheme() {
        "https" => true,
        "http" => false,
        other => {
            return Err(AdkClientError::InvalidConfig(format!(
                "unsupported URL scheme '{other}' (expected http or https)"
            )));
        }
    };

    if parsed.host().is_none() {
        return Err(AdkClientError::InvalidConfig(
            "base URL must include a host".to_string(),
        ));
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AdkClientError::InvalidConfig(
            "base URL must not embed credentials".to_string(),
        ));
    }

    let is_loopback = match parsed.host() {
        Some(url::Host::Domain(domain)) => {
            domain.eq_ignore_ascii_case("localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };

    parsed.set_query(None);
    parsed.set_fragment(None);

    let mut path = parsed.path().trim_end_matches('/').to_string();
    match protocol {
        AiProtocol::OpenAi => {
            for suffix in ["/chat/completions", "/models"] {
                if let Some(stripped) = strip_suffix_ci(&path, suffix) {
                    path = stripped.trim_end_matches('/').to_string();
                    break;
                }
            }
        }
        AiProtocol::Anthropic => {
            // `adk-anthropic` always appends `/v1/messages` or `/v1/models`,
            // so strip accidental endpoint or `/v1` suffixes to avoid `/v1/v1/...`.
            for _ in 0..2 {
                let mut changed = false;
                for suffix in [
                    "/v1/messages",
                    "/v1/models",
                    "/messages",
                    "/models",
                    "/v1",
                ] {
                    if let Some(stripped) = strip_suffix_ci(&path, suffix) {
                        path = stripped.trim_end_matches('/').to_string();
                        changed = true;
                        break;
                    }
                }
                if !changed {
                    break;
                }
            }
        }
    }

    parsed.set_path(&path);
    let sdk_base_url = parsed.as_str().trim_end_matches('/').to_string();
    let models_url = match protocol {
        AiProtocol::OpenAi => format!("{sdk_base_url}/models"),
        AiProtocol::Anthropic => format!("{sdk_base_url}/v1/models"),
    };

    let is_official_anthropic = protocol == AiProtocol::Anthropic
        && is_https
        && parsed
            .host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case("api.anthropic.com"))
        && path.is_empty();

    Ok(NormalizedEndpoints {
        protocol,
        sdk_base_url,
        models_url,
        is_official_anthropic,
        is_loopback,
        is_https,
    })
}

pub(crate) fn validate_endpoints(
    config: &AdkModelConfig,
) -> Result<NormalizedEndpoints, AdkClientError> {
    let endpoints =
        normalize_endpoints(config.protocol, config.base_url.as_deref())?;
    if !endpoints.is_https
        && !endpoints.is_loopback
        && !config.allow_insecure_http
    {
        return Err(AdkClientError::InsecureEndpoint(endpoints.sdk_base_url));
    }
    Ok(endpoints)
}

/// Builds an `adk-core` [`Llm`] client for the given configuration.
pub fn build_llm(
    config: &AdkModelConfig,
    model_override: Option<&str>,
    max_tokens_override: Option<u32>,
    disable_retries: bool,
) -> Result<(Arc<dyn Llm>, NormalizedEndpoints, String), AdkClientError> {
    adk_core::ensure_crypto_provider();
    let endpoints = validate_endpoints(config)?;
    let model = model_override
        .or(config.model.as_deref())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| {
            AdkClientError::InvalidConfig("model name is required".to_string())
        })?
        .to_string();

    let api_key = config.api_key.as_str();
    let llm: Arc<dyn Llm> = match config.protocol {
        AiProtocol::OpenAi => {
            let oai_cfg = OpenAICompatibleConfig::new(api_key, &model)
                .with_base_url(&endpoints.sdk_base_url)
                .with_provider_name("openai-compatible");
            let mut client = OpenAICompatible::new(oai_cfg)
                .map_err(|err| map_adk_error(&err, api_key))?;
            if disable_retries {
                client = client.with_retry_config(RetryConfig::disabled());
            }
            Arc::new(client)
        }
        AiProtocol::Anthropic => {
            let max_tokens = max_tokens_override
                .or(config.max_tokens)
                .unwrap_or(DEFAULT_ANTHROPIC_MAX_TOKENS);
            let mut anth_cfg = AnthropicConfig::new(api_key, &model)
                .with_base_url(&endpoints.sdk_base_url)
                .with_max_tokens(max_tokens)
                // Third-party `/anthropic` gateways often reject `cache_control`
                // with HTTP 400, so only enable prompt caching on official Anthropic.
                .with_prompt_caching(endpoints.is_official_anthropic);
            if !endpoints.is_https
                && !endpoints.is_loopback
                && config.allow_insecure_http
            {
                anth_cfg = anth_cfg.allow_insecure_http();
            }
            let mut client = AnthropicClient::new(anth_cfg)
                .map_err(|err| map_adk_error(&err, api_key))?;
            if disable_retries {
                client = client.with_retry_config(RetryConfig::disabled());
            }
            Arc::new(client)
        }
    };

    Ok((llm, endpoints, model))
}

pub(crate) fn map_adk_error(err: &AdkError, api_key: &str) -> AdkClientError {
    let msg = sanitize_error_message(&err.message, api_key);
    let status = err.details.upstream_status_code;

    if matches!(
        err.category,
        ErrorCategory::Unauthorized | ErrorCategory::Forbidden
    ) || matches!(status, Some(401 | 403))
    {
        return AdkClientError::Unauthorized {
            status: status.unwrap_or(401),
            message: msg,
        };
    }

    if err.category == ErrorCategory::Timeout || status == Some(408) {
        return AdkClientError::Model(format!("request timed out: {msg}"));
    }

    if let Some(code) = status {
        return AdkClientError::Http {
            status: Some(code),
            message: msg,
        };
    }

    AdkClientError::Model(msg)
}

pub(crate) async fn list_models_with_http(
    http: &reqwest::Client,
    config: &AdkModelConfig,
) -> Result<Vec<RemoteModel>, AdkClientError> {
    let endpoints = validate_endpoints(config)?;
    let api_key = config.api_key.as_str();

    let mut req = http.get(&endpoints.models_url).timeout(config.timeout);
    match config.protocol {
        AiProtocol::OpenAi => {
            if !api_key.is_empty() {
                req = req.bearer_auth(api_key);
            }
        }
        AiProtocol::Anthropic => {
            req = req
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_API_VERSION);
        }
    }

    let resp = req.send().await.map_err(|err| {
        if err.is_timeout() {
            AdkClientError::Timeout(config.timeout)
        } else {
            AdkClientError::Http {
                status: err.status().map(|s| s.as_u16()),
                message: sanitize_error_message(&err.to_string(), api_key),
            }
        }
    })?;

    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        let msg = sanitize_error_message(&body, api_key);
        if matches!(code, 401 | 403) {
            return Err(AdkClientError::Unauthorized {
                status: code,
                message: msg,
            });
        }
        return Err(AdkClientError::Http {
            status: Some(code),
            message: msg,
        });
    }

    let payload: Value =
        resp.json().await.map_err(|err| AdkClientError::Http {
            status: None,
            message: sanitize_error_message(
                &format!("invalid models JSON: {err}"),
                api_key,
            ),
        })?;

    Ok(parse_models_payload(&payload))
}

fn parse_models_payload(payload: &Value) -> Vec<RemoteModel> {
    let items = payload
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| payload.get("models").and_then(Value::as_array))
        .or_else(|| payload.as_array());

    let Some(items) = items else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let id = item
            .get("id")
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(id) = id else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let label = item
            .get("display_name")
            .or_else(|| item.get("label"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(id)
            .to_string();
        out.push(RemoteModel {
            id: id.to_string(),
            label,
        });
    }
    out
}

pub(crate) async fn test_connection_with_http(
    http: &reqwest::Client,
    config: &AdkModelConfig,
) -> Result<ConnectionProbeResult, AdkClientError> {
    let endpoints = validate_endpoints(config)?;
    match list_models_with_http(http, config).await {
        Ok(models) => {
            let count = models.len();
            Ok(ConnectionProbeResult {
                protocol: config.protocol,
                endpoint: endpoints.sdk_base_url,
                model_count: Some(count),
                message: format!("Connected ({count} models available)"),
            })
        }
        Err(AdkClientError::Http {
            status: Some(404 | 405),
            ..
        }) if config
            .model
            .as_deref()
            .is_some_and(|m| !m.trim().is_empty()) =>
        {
            probe_minimal_completion(config).await
        }
        Err(err) => Err(err),
    }
}

async fn probe_minimal_completion(
    config: &AdkModelConfig,
) -> Result<ConnectionProbeResult, AdkClientError> {
    let (llm, endpoints, model) = build_llm(config, None, Some(1), true)?;
    let mut req =
        LlmRequest::new(&model, vec![Content::new("user").with_text("ping")]);
    req.config = Some(adk_core::GenerateContentConfig {
        max_output_tokens: Some(1),
        ..Default::default()
    });

    let api_key = config.api_key.as_str();
    let probe_fut = async {
        let mut stream = llm
            .generate_content(req, false)
            .await
            .map_err(|err| map_adk_error(&err, api_key))?;
        while let Some(item) = stream.next().await {
            match item {
                Ok(resp) => {
                    if let Some(msg) = resp.error_message {
                        return Err(AdkClientError::Model(
                            sanitize_error_message(&msg, api_key),
                        ));
                    }
                }
                Err(err) => return Err(map_adk_error(&err, api_key)),
            }
        }
        Ok::<(), AdkClientError>(())
    };

    tokio::time::timeout(config.timeout, probe_fut)
        .await
        .map_err(|_| AdkClientError::Timeout(config.timeout))??;

    Ok(ConnectionProbeResult {
        protocol: config.protocol,
        endpoint: endpoints.sdk_base_url,
        model_count: None,
        message: format!("Connected via minimal completion ({model})"),
    })
}
