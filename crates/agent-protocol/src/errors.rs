//! Agent protocol error types.

use std::time::Duration;
use thiserror::Error;

/// Agent protocol errors
#[derive(Error, Debug)]
pub enum AgentProtocolError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Connection closed")]
    ConnectionClosed,

    #[error("Protocol version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: u32, actual: u32 },

    #[error("Message too large: {size} bytes (max: {max}")]
    MessageTooLarge { size: usize, max: usize },

    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    #[error("Timeout after {0:?}")]
    Timeout(Duration),

    #[error("Agent unavailable")]
    Unavailable,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Wrong connection type: {0}")]
    WrongConnectionType(String),
}
