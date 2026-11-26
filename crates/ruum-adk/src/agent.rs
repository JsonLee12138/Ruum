//! Multi-turn agent execution, tool bridging, and response normalization.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use adk_agent::LlmAgentBuilder;
use adk_core::{
    AdkError, Content, ErrorCategory, ErrorComponent, Part, RunConfig,
    SessionId, StreamingMode, Tool, ToolContext, Toolset, UserId,
};
use adk_runner::Runner;
use adk_session::{CreateRequest, InMemorySessionService, SessionService};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;

use crate::model::{
    AdkClientError, AdkModelConfig, ConnectionProbeResult, RemoteModel,
    build_llm, list_models_with_http, map_adk_error, sanitize_error_message,
    test_connection_with_http,
};

const DEFAULT_MAX_ITERATIONS: u32 = 8;
const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(30);
const APP_NAME: &str = "ruum-adk";
const USER_ID: &str = "user";

/// Metadata describing an invocable tool exposed to an `LlmAgent`.
#[derive(Debug, Clone)]
pub struct AdkToolDef {
    /// Unique tool name presented to the model.
    pub name: String,
    /// Concise description of when and how to use the tool.
    pub description: String,
    /// JSON Schema describing the tool's input object.
    pub parameters_schema: Value,
}

/// Async executor backing an in-process [`AdkToolDef`].
#[async_trait]
pub trait AdkToolExecutor: Send + Sync {
    /// Executes the tool with JSON arguments and returns a JSON payload.
    async fn execute(&self, args: Value) -> Result<Value, String>;
}

struct BridgedTool {
    def: AdkToolDef,
    executor: Arc<dyn AdkToolExecutor>,
}

#[async_trait]
impl Tool for BridgedTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(self.def.parameters_schema.clone())
    }

    async fn execute(
        &self,
        _ctx: Arc<dyn ToolContext>,
        args: Value,
    ) -> adk_core::Result<Value> {
        self.executor.execute(args).await.map_err(AdkError::tool)
    }
}

/// Request parameters for executing a multi-turn agent loop.
pub struct AdkAgentRequest {
    /// System instruction guiding agent behavior.
    pub instruction: String,
    /// User prompt for the current turn.
    pub user_prompt: String,
    /// In-process tools exposed to the model.
    pub tools: Vec<(AdkToolDef, Arc<dyn AdkToolExecutor>)>,
    /// Optional external toolsets (for example from `mcp-stdio`).
    pub toolsets: Vec<Arc<dyn Toolset>>,
    /// Maximum LLM round-trips before aborting (default `8`).
    pub max_iterations: u32,
    /// Per-tool execution timeout (default `30s`).
    pub tool_timeout: Duration,
    /// Retries once without tools if the model rejects tools with HTTP 400.
    pub fallback_without_tools_on_unsupported: bool,
    /// Optional cooperative cancellation token aborting in-flight inference.
    pub cancel: Option<crate::cancel::CancelToken>,
}

impl AdkAgentRequest {
    /// Builds a request with safe defaults (`max_iterations = 8`,
    /// `tool_timeout = 30s`, `fallback_without_tools_on_unsupported = true`).
    pub fn new(
        instruction: impl Into<String>,
        user_prompt: impl Into<String>,
    ) -> Self {
        Self {
            instruction: instruction.into(),
            user_prompt: user_prompt.into(),
            tools: Vec::new(),
            toolsets: Vec::new(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            fallback_without_tools_on_unsupported: true,
            cancel: None,
        }
    }

    /// Attaches a cooperative cancellation token to this agent run.
    #[must_use]
    pub fn with_cancel(mut self, cancel: crate::cancel::CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

impl fmt::Debug for AdkAgentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tool_names: Vec<&str> = self
            .tools
            .iter()
            .map(|(def, _)| def.name.as_str())
            .collect();
        f.debug_struct("AdkAgentRequest")
            .field("instruction_len", &self.instruction.len())
            .field("user_prompt_len", &self.user_prompt.len())
            .field("tool_names", &tool_names)
            .field("toolsets_count", &self.toolsets.len())
            .field("max_iterations", &self.max_iterations)
            .field("tool_timeout", &self.tool_timeout)
            .field(
                "fallback_without_tools_on_unsupported",
                &self.fallback_without_tools_on_unsupported,
            )
            .field("has_cancel", &self.cancel.is_some())
            .finish()
    }
}

/// Normalized result of a completed agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdkAgentResponse {
    /// Final assistant text with thinking traces stripped.
    pub text: String,
    /// Captured thinking/reasoning content, if any was emitted.
    pub reasoning: Option<String>,
    /// Model identifier that produced the response.
    pub model: String,
    /// Total tool calls issued during the multi-turn loop.
    pub tool_calls_made: usize,
}

/// Configuration for spawning an external Stdio MCP server (`mcp-stdio` feature).
#[cfg(feature = "mcp-stdio")]
#[derive(Debug, Clone)]
pub struct McpStdioServerConfig {
    /// Program name or path to launch.
    pub command: String,
    /// CLI arguments passed to the server.
    pub args: Vec<String>,
    /// Additional environment variables for the child process.
    pub env: Vec<(String, String)>,
}

/// Connects to an external Stdio MCP server and exposes it as an ADK `Toolset`.
#[cfg(feature = "mcp-stdio")]
pub async fn connect_mcp_stdio(
    cfg: &McpStdioServerConfig,
) -> Result<Arc<dyn Toolset>, AdkClientError> {
    use adk_tool::mcp::rmcp::{ServiceExt, transport::TokioChildProcess};
    use adk_tool::mcp::{
        AdkClientHandler, AutoDeclineElicitationHandler, McpToolset,
    };

    let mut cmd = tokio::process::Command::new(&cfg.command);
    cmd.args(&cfg.args);
    for (key, value) in &cfg.env {
        cmd.env(key, value);
    }
    #[cfg(windows)]
    {
        // Suppress transient console windows when spawned from GUI/service hosts.
        cmd.creation_flags(0x0800_0000);
    }

    let transport = TokioChildProcess::new(cmd).map_err(|err| {
        AdkClientError::Model(format!("failed to spawn MCP server: {err}"))
    })?;
    let client = AdkClientHandler::new(Arc::new(AutoDeclineElicitationHandler))
        .serve(transport)
        .await
        .map_err(|err| {
            AdkClientError::Model(format!(
                "failed to initialize MCP stdio client: {err}"
            ))
        })?;
    Ok(Arc::new(McpToolset::new(client)))
}

/// Async direct AI client backed by `adk-model`, `adk-agent`, and `adk-runner`.
#[derive(Debug, Clone)]
pub struct AdkDirectClient {
    http: reqwest::Client,
    probe_http: reqwest::Client,
    cloud_probe_http: reqwest::Client,
}

impl AdkDirectClient {
    /// Initializes a client and ensures the process-wide `rustls` provider is set.
    pub fn new() -> Result<Self, AdkClientError> {
        adk_core::ensure_crypto_provider();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(crate::cloud_llm::DEFAULT_CLOUD_CONNECT_TIMEOUT)
            .build()
            .map_err(|err| {
                AdkClientError::Model(format!(
                    "failed to build HTTP client: {err}"
                ))
            })?;
        let probe_http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(crate::cloud_llm::DEFAULT_CLOUD_CONNECT_TIMEOUT)
            .build()
            .map_err(|err| {
                AdkClientError::Model(format!(
                    "failed to build probe HTTP client: {err}"
                ))
            })?;
        let cloud_probe_http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(crate::cloud_llm::DEFAULT_CLOUD_CONNECT_TIMEOUT)
            .build()
            .map_err(|err| {
                AdkClientError::Model(format!(
                    "failed to build cloud probe HTTP client: {err}"
                ))
            })?;
        Ok(Self {
            http,
            probe_http,
            cloud_probe_http,
        })
    }

    /// Queries the endpoint's `/models` list.
    pub async fn list_models(
        &self,
        config: &AdkModelConfig,
    ) -> Result<Vec<RemoteModel>, AdkClientError> {
        adk_core::ensure_crypto_provider();
        list_models_with_http(&self.http, config).await
    }

    /// Probes connectivity via `/models`, falling back to `max_tokens: 1` on 404/405.
    pub async fn test_connection(
        &self,
        config: &AdkModelConfig,
    ) -> Result<ConnectionProbeResult, AdkClientError> {
        adk_core::ensure_crypto_provider();
        test_connection_with_http(&self.http, config).await
    }

    /// Fast streaming TTFT probe against a direct OpenAI/Anthropic model.
    pub async fn probe_model(
        &self,
        config: &AdkModelConfig,
        cancel: Option<&crate::cancel::CancelToken>,
    ) -> Result<crate::model::ModelProbeResult, AdkClientError> {
        adk_core::ensure_crypto_provider();
        crate::model::probe_model_with_http(&self.probe_http, config, cancel)
            .await
    }

    /// Probes a Ruum Cloud virtual model via `POST /v1/models/probe`.
    pub async fn probe_cloud_model(
        &self,
        config: &crate::cloud_llm::RuumCloudConfig,
    ) -> Result<crate::model::ModelProbeResult, AdkClientError> {
        adk_core::ensure_crypto_provider();
        crate::cloud_llm::probe_cloud_model_with_http(
            &self.cloud_probe_http,
            config,
        )
        .await
    }

    /// Runs a multi-turn agent loop until a final response or safety limit.
    pub async fn run_agent(
        &self,
        config: &AdkModelConfig,
        request: AdkAgentRequest,
    ) -> Result<AdkAgentResponse, AdkClientError> {
        adk_core::ensure_crypto_provider();
        let cancel = request.cancel.clone();
        if cancel
            .as_ref()
            .is_some_and(crate::cancel::CancelToken::is_cancelled)
        {
            return Err(AdkClientError::Cancelled);
        }
        let (llm, _endpoints, model_name) =
            build_llm(config, None, None, false)?;
        let api_key = config.api_key.as_str();
        let has_tools =
            !request.tools.is_empty() || !request.toolsets.is_empty();

        let fut = async {
            match run_agent_once(
                Arc::clone(&llm),
                &model_name,
                api_key,
                &request,
                has_tools,
            )
            .await
            {
                Ok(resp) => Ok(resp),
                Err(attempt)
                    if has_tools
                        && request.fallback_without_tools_on_unsupported
                        && attempt.tool_calls_made == 0
                        && attempt.is_tools_unsupported =>
                {
                    run_agent_once(llm, &model_name, api_key, &request, false)
                        .await
                        .map_err(|err| err.error)
                }
                Err(attempt) => Err(attempt.error),
            }
        };

        let timed = async {
            tokio::time::timeout(config.timeout, fut)
                .await
                .map_err(|_| AdkClientError::Timeout(config.timeout))?
        };

        let res = crate::cancel::race_cancel(cancel.as_ref(), timed)
            .await
            .map_err(|()| AdkClientError::Cancelled)?;
        if res.is_err()
            && cancel
                .as_ref()
                .is_some_and(crate::cancel::CancelToken::is_cancelled)
        {
            return Err(AdkClientError::Cancelled);
        }
        res
    }

    /// Runs a multi-turn agent loop against Ruum Cloud (`POST /v1/chat/completions`)
    /// with per-request timeouts decoupled from an outer loop timeout, a single
    /// fixed `X-Ruum-Session` UUID, strict 7-tool whitelisting, and
    /// `max_iterations <= 8` (`fallback_without_tools_on_unsupported = false`).
    pub async fn run_cloud_agent(
        &self,
        config: &crate::cloud_llm::RuumCloudConfig,
        mut request: AdkAgentRequest,
    ) -> Result<AdkAgentResponse, AdkClientError> {
        adk_core::ensure_crypto_provider();
        let effective_cancel =
            request.cancel.clone().or_else(|| config.cancel.clone());
        if effective_cancel
            .as_ref()
            .is_some_and(crate::cancel::CancelToken::is_cancelled)
        {
            return Err(AdkClientError::Cancelled);
        }
        let session_id = config
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or_else(
                || uuid::Uuid::new_v4().to_string(),
                ToOwned::to_owned,
            );

        let mut cloud_config = config.clone();
        cloud_config.cancel = effective_cancel.clone();
        let cloud_llm = Arc::new(crate::cloud_llm::RuumCloudLlm::new(
            cloud_config,
            session_id,
        )?);
        cloud_llm.clear_last_error();

        request.max_iterations = request
            .max_iterations
            .clamp(1, crate::cloud_llm::MAX_CLOUD_ITERATIONS);
        request.fallback_without_tools_on_unsupported = false;
        request.tools.retain(|(def, _)| {
            crate::cloud_llm::is_allowed_ruum_tool(&def.name)
        });
        request.toolsets.clear();

        let has_tools = !request.tools.is_empty();
        let llm_trait: Arc<dyn adk_core::Llm> = cloud_llm.clone();
        let run_fut =
            run_agent_once(llm_trait, &config.model, "", &request, has_tools);
        match crate::cancel::race_cancel(effective_cancel.as_ref(), run_fut)
            .await
        {
            Err(()) => Err(AdkClientError::Cancelled),
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(attempt)) => {
                if effective_cancel
                    .as_ref()
                    .is_some_and(crate::cancel::CancelToken::is_cancelled)
                {
                    return Err(AdkClientError::Cancelled);
                }
                if let Some(gw) = cloud_llm.take_last_error() {
                    return Err(AdkClientError::CloudGateway {
                        status: gw.status,
                        code: gw.code,
                        message: gw.message,
                        trace_id: gw.trace_id,
                        is_timeout: gw.is_timeout,
                    });
                }
                Err(attempt.error)
            }
        }
    }
}

struct AttemptFailure {
    error: AdkClientError,
    tool_calls_made: usize,
    is_tools_unsupported: bool,
}

async fn run_agent_once(
    llm: Arc<dyn adk_core::Llm>,
    model_name: &str,
    api_key: &str,
    request: &AdkAgentRequest,
    include_tools: bool,
) -> Result<AdkAgentResponse, AttemptFailure> {
    let max_iterations = if include_tools {
        request.max_iterations.max(1)
    } else {
        1
    };

    let mut builder = LlmAgentBuilder::new("ruum_adk_agent")
        .model(llm)
        .max_iterations(max_iterations)
        .tool_timeout(request.tool_timeout);

    if !request.instruction.trim().is_empty() {
        builder = builder.instruction(&request.instruction);
    }

    if include_tools {
        for (def, executor) in &request.tools {
            builder = builder.tool(Arc::new(BridgedTool {
                def: def.clone(),
                executor: Arc::clone(executor),
            }));
        }
        for toolset in &request.toolsets {
            builder = builder.toolset(Arc::clone(toolset));
        }
    }

    let agent = builder.build().map_err(|err| AttemptFailure {
        error: map_adk_error(&err, api_key),
        tool_calls_made: 0,
        is_tools_unsupported: false,
    })?;

    let session_service = Arc::new(InMemorySessionService::new());
    let session = session_service
        .create(CreateRequest {
            app_name: APP_NAME.to_string(),
            user_id: USER_ID.to_string(),
            session_id: None,
            state: HashMap::new(),
        })
        .await
        .map_err(|err| AttemptFailure {
            error: map_adk_error(&err, api_key),
            tool_calls_made: 0,
            is_tools_unsupported: false,
        })?;

    let session_id = session.id().to_string();
    let runner = Runner::builder()
        .app_name(APP_NAME)
        .agent(Arc::new(agent))
        .session_service(session_service)
        .run_config(RunConfig {
            streaming_mode: StreamingMode::None,
            ..RunConfig::default()
        })
        .build()
        .map_err(|err| AttemptFailure {
            error: map_adk_error(&err, api_key),
            tool_calls_made: 0,
            is_tools_unsupported: false,
        })?;

    let user_id = UserId::try_from(USER_ID).map_err(|err| AttemptFailure {
        error: AdkClientError::InvalidConfig(err.to_string()),
        tool_calls_made: 0,
        is_tools_unsupported: false,
    })?;
    let typed_session_id =
        SessionId::try_from(session_id).map_err(|err| AttemptFailure {
            error: AdkClientError::InvalidConfig(err.to_string()),
            tool_calls_made: 0,
            is_tools_unsupported: false,
        })?;

    let mut stream = runner
        .run(
            user_id,
            typed_session_id,
            Content::new("user").with_text(&request.user_prompt),
        )
        .await
        .map_err(|err| AttemptFailure {
            error: map_adk_error(&err, api_key),
            tool_calls_made: 0,
            is_tools_unsupported: is_tools_unsupported_adk_error(&err),
        })?;

    let mut tool_calls_made = 0_usize;
    let mut reasoning_parts = Vec::new();
    let mut final_text: Option<String> = None;

    while let Some(item) = stream.next().await {
        let event = match item {
            Ok(ev) => ev,
            Err(err) => {
                let mapped = if err.component == ErrorComponent::Agent
                    && err.message.contains("Max iterations")
                {
                    AdkClientError::MaxIterationsExceeded(max_iterations)
                } else {
                    map_adk_error(&err, api_key)
                };
                return Err(AttemptFailure {
                    error: mapped,
                    tool_calls_made,
                    is_tools_unsupported: is_tools_unsupported_adk_error(&err),
                });
            }
        };

        if let Some(err_msg) = &event.llm_response.error_message {
            let cleaned = sanitize_error_message(err_msg, api_key);
            return Err(AttemptFailure {
                error: AdkClientError::Model(cleaned),
                tool_calls_made,
                is_tools_unsupported: false,
            });
        }

        let turn_tool_calls = event.tool_calls().len();
        tool_calls_made += turn_tool_calls;

        if event.llm_response.partial {
            continue;
        }

        let Some(content) = &event.llm_response.content else {
            continue;
        };
        if !matches!(content.role.as_str(), "model" | "assistant") {
            continue;
        }

        let mut raw_turn_text = String::new();
        for part in &content.parts {
            match part {
                Part::Thinking { thinking, .. } => {
                    let trimmed = thinking.trim();
                    if !trimmed.is_empty() {
                        reasoning_parts.push(trimmed.to_string());
                    }
                }
                Part::Text { text } => {
                    raw_turn_text.push_str(text);
                }
                _ => {}
            }
        }

        let (turn_text, inline_think) =
            strip_thinking_from_text(&raw_turn_text);
        if let Some(think) = inline_think {
            reasoning_parts.push(think);
        }

        if turn_tool_calls == 0 && !turn_text.is_empty() {
            final_text = Some(turn_text);
        }
    }

    let text =
        final_text.filter(|t| !t.trim().is_empty()).ok_or_else(|| {
            let error = if tool_calls_made > 0 {
                AdkClientError::MaxIterationsExceeded(max_iterations)
            } else {
                AdkClientError::Model(
                    "model returned an empty text response".to_string(),
                )
            };
            AttemptFailure {
                error,
                tool_calls_made,
                is_tools_unsupported: false,
            }
        })?;

    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join("\n"))
    };

    Ok(AdkAgentResponse {
        text,
        reasoning,
        model: model_name.to_string(),
        tool_calls_made,
    })
}

fn is_tools_unsupported_adk_error(err: &AdkError) -> bool {
    if matches!(
        err.category,
        ErrorCategory::Unauthorized
            | ErrorCategory::Forbidden
            | ErrorCategory::NotFound
            | ErrorCategory::RateLimited
            | ErrorCategory::Timeout
            | ErrorCategory::Unavailable
    ) {
        return false;
    }

    if err.category == ErrorCategory::Unsupported {
        return true;
    }

    // Only treat HTTP 400 (InvalidInput) as tool-unsupported fallback when
    // no tool calls have succeeded yet.
    err.details.upstream_status_code == Some(400)
}

/// Removes `<think>...</think>` reasoning blocks and returns `(clean, reasoning)`.
pub fn strip_thinking_from_text(input: &str) -> (String, Option<String>) {
    let mut clean = String::with_capacity(input.len());
    let mut reasoning = Vec::new();
    let mut rest = input;

    while let Some(start) = find_ascii_ci(rest, "<think>") {
        clean.push_str(&rest[..start]);
        let after_open = &rest[start + "<think>".len()..];
        if let Some(end) = find_ascii_ci(after_open, "</think>") {
            let inner = after_open[..end].trim();
            if !inner.is_empty() {
                reasoning.push(inner.to_string());
            }
            rest = &after_open[end + "</think>".len()..];
        } else {
            // Unclosed <think> block runs to the end of the message.
            let inner = after_open.trim();
            if !inner.is_empty() {
                reasoning.push(inner.to_string());
            }
            rest = "";
            break;
        }
    }

    clean.push_str(rest);
    let cleaned = clean.trim().to_string();
    let extracted = if reasoning.is_empty() {
        None
    } else {
        Some(reasoning.join("\n"))
    };
    (cleaned, extracted)
}

fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    let needle_bytes = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle_bytes.len())
        .enumerate()
        .find_map(|(idx, window)| {
            (haystack.is_char_boundary(idx)
                && window.eq_ignore_ascii_case(needle_bytes))
            .then_some(idx)
        })
}
