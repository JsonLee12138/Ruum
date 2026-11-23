//! Standalone reusable direct AI client facade built on `adk-*`.

pub mod agent;
#[cfg(feature = "blocking")]
pub mod blocking;
pub mod model;

pub use agent::{
    AdkAgentRequest, AdkAgentResponse, AdkDirectClient, AdkToolDef,
    AdkToolExecutor, strip_thinking_from_text,
};
#[cfg(feature = "mcp-stdio")]
pub use agent::{McpStdioServerConfig, connect_mcp_stdio};
#[cfg(feature = "blocking")]
pub use blocking::BlockingAdkClient;
pub use model::{
    AdkClientError, AdkModelConfig, AiProtocol, ConnectionProbeResult,
    NormalizedEndpoints, RemoteModel, build_llm, normalize_endpoints,
    sanitize_error_message,
};
