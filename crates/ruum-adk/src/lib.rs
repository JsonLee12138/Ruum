//! Standalone reusable direct AI client facade built on `adk-*`.

pub mod agent;
#[cfg(feature = "blocking")]
pub mod blocking;
pub mod cancel;
pub mod cloud_llm;
pub mod model;

pub use agent::{
    AdkAgentRequest, AdkAgentResponse, AdkDirectClient, AdkToolDef,
    AdkToolExecutor, strip_thinking_from_text,
};
#[cfg(feature = "mcp-stdio")]
pub use agent::{McpStdioServerConfig, connect_mcp_stdio};
#[cfg(feature = "blocking")]
pub use blocking::BlockingAdkClient;
pub use cancel::CancelToken;
pub use cloud_llm::{
    ALLOWED_RUUM_TOOLS, CloudGatewayError, CloudStageCallback,
    CloudStreamStage, CloudTokenProvider, DEFAULT_CLOUD_CONNECT_TIMEOUT,
    DEFAULT_CLOUD_INFERENCE_TIMEOUT, MAX_CLOUD_COMPACTED_BYTES,
    MAX_CLOUD_ITERATIONS, MAX_CLOUD_MAX_TOKENS, MAX_CLOUD_MESSAGES,
    RuumCloudConfig, RuumCloudLlm, RuumTask, compact_chat_request_payload,
    is_allowed_ruum_tool,
};
pub use model::{
    AdkClientError, AdkModelConfig, AiProtocol, ConnectionProbeResult,
    ModelProbeResult, NormalizedEndpoints, RemoteModel, build_llm,
    normalize_endpoints, sanitize_error_message,
};
