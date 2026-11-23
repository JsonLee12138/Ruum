//! Synchronous facade over [`AdkDirectClient`] with shared Tokio runtime.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Builder, Handle, Runtime};

use crate::agent::{AdkAgentRequest, AdkAgentResponse, AdkDirectClient};
use crate::model::{
    AdkClientError, AdkModelConfig, ConnectionProbeResult, RemoteModel,
};

static SHARED_RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

fn shared_runtime() -> Result<&'static Runtime, AdkClientError> {
    let entry = SHARED_RUNTIME.get_or_init(|| {
        adk_core::ensure_crypto_provider();
        Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ruum-adk-rt")
            .enable_all()
            .build()
            .map_err(|err| format!("failed to initialize Tokio runtime: {err}"))
    });
    entry
        .as_ref()
        .map_err(|msg| AdkClientError::Model(msg.clone()))
}

/// Blocking wrapper around [`AdkDirectClient`] using a shared `OnceLock` runtime.
#[derive(Debug, Clone)]
pub struct BlockingAdkClient {
    inner: AdkDirectClient,
}

impl BlockingAdkClient {
    /// Creates a blocking client, initializes `rustls`, and warms the runtime.
    pub fn new() -> Result<Self, AdkClientError> {
        adk_core::ensure_crypto_provider();
        let _rt = shared_runtime()?;
        Ok(Self {
            inner: AdkDirectClient::new()?,
        })
    }

    /// Synchronously queries `/models` on the configured endpoint.
    pub fn list_models(
        &self,
        config: &AdkModelConfig,
    ) -> Result<Vec<RemoteModel>, AdkClientError> {
        Self::block_on(self.inner.list_models(config))
    }

    /// Synchronously tests endpoint connectivity.
    pub fn test_connection(
        &self,
        config: &AdkModelConfig,
    ) -> Result<ConnectionProbeResult, AdkClientError> {
        Self::block_on(self.inner.test_connection(config))
    }

    /// Synchronously runs a multi-turn agent loop.
    pub fn run_agent(
        &self,
        config: &AdkModelConfig,
        request: AdkAgentRequest,
    ) -> Result<AdkAgentResponse, AdkClientError> {
        Self::block_on(self.inner.run_agent(config, request))
    }

    fn block_on<F, T>(fut: F) -> Result<T, AdkClientError>
    where
        F: Future<Output = Result<T, AdkClientError>> + Send,
        T: Send,
    {
        let rt = shared_runtime()?;
        if Handle::try_current().is_ok() {
            // Avoid nested-runtime panics when invoked from inside a Tokio task.
            return std::thread::scope(|scope| {
                scope.spawn(move || rt.block_on(fut)).join().unwrap_or_else(
                    |_| {
                        Err(AdkClientError::Model(
                            "blocking ADK worker thread panicked".to_string(),
                        ))
                    },
                )
            });
        }
        rt.block_on(fut)
    }
}
