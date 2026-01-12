//! Error types for the WASM runtime.

use thiserror::Error;

/// Errors that can occur in the WASM runtime.
#[derive(Debug, Error)]
pub enum WasmRuntimeError {
    /// Failed to create the Wasmtime engine
    #[error("failed to create WASM engine: {0}")]
    EngineCreation(String),

    /// Failed to compile WASM module
    #[error("failed to compile WASM module: {0}")]
    Compilation(String),

    /// Failed to instantiate WASM module
    #[error("failed to instantiate WASM module: {0}")]
    Instantiation(String),

    /// Failed to load WASM file
    #[error("failed to load WASM file: {0}")]
    LoadFile(#[from] std::io::Error),

    /// Agent configuration error
    #[error("agent configuration error: {0}")]
    Configuration(String),

    /// Function call failed
    #[error("function call failed: {0}")]
    FunctionCall(String),

    /// Function not found in WASM module
    #[error("function not found: {0}")]
    FunctionNotFound(String),

    /// Resource limit exceeded
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(String),

    /// Execution timeout
    #[error("execution timeout after {0:?}")]
    Timeout(std::time::Duration),

    /// Invalid WASM module
    #[error("invalid WASM module: {0}")]
    InvalidModule(String),

    /// Serialization error
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Agent returned an error
    #[error("agent error: {0}")]
    AgentError(String),

    /// Runtime is shutting down
    #[error("runtime is shutting down")]
    Shutdown,

    /// Internal error
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<anyhow::Error> for WasmRuntimeError {
    fn from(err: anyhow::Error) -> Self {
        // Check for specific error types (wasmtime::Error is anyhow::Error)
        let msg = err.to_string();
        if msg.contains("fuel") || msg.contains("out of fuel") {
            WasmRuntimeError::ResourceLimit("CPU fuel exhausted".to_string())
        } else if msg.contains("memory") {
            WasmRuntimeError::ResourceLimit(format!("memory limit: {}", msg))
        } else {
            WasmRuntimeError::Internal(msg)
        }
    }
}
