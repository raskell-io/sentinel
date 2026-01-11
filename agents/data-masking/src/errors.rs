//! Error types for the data masking agent.

use thiserror::Error;

/// Result type for masking operations.
pub type MaskingResult<T> = Result<T, MaskingError>;

/// Errors that can occur during data masking operations.
#[derive(Debug, Error)]
pub enum MaskingError {
    /// Failed to parse JSON content.
    #[error("invalid JSON: {0}")]
    InvalidJson(String),

    /// Failed to parse XML content.
    #[error("invalid XML: {0}")]
    InvalidXml(String),

    /// Failed to parse form data.
    #[error("invalid form data: {0}")]
    InvalidForm(String),

    /// Content is not valid UTF-8.
    #[error("invalid UTF-8: {0}")]
    InvalidUtf8(String),

    /// Unsupported content type.
    #[error("unsupported content type: {0}")]
    UnsupportedContentType(String),

    /// Failed to serialize content.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Token not found during detokenization.
    #[error("token not found: {0}")]
    TokenNotFound(String),

    /// Token store error.
    #[error("token store error: {0}")]
    Store(#[from] TokenStoreError),

    /// FPE not configured.
    #[error("format-preserving encryption not configured")]
    FpeNotConfigured,

    /// FPE encryption/decryption error.
    #[error("FPE error: {0}")]
    FpeError(String),

    /// Invalid configuration.
    #[error("configuration error: {0}")]
    Config(String),

    /// Field access error.
    #[error("field access error: {0}")]
    FieldAccess(String),

    /// Regex compilation error.
    #[error("invalid regex pattern: {0}")]
    InvalidRegex(String),

    /// Buffer overflow.
    #[error("buffer overflow: body exceeds {max_bytes} bytes")]
    BufferOverflow { max_bytes: usize },

    /// Base64 decoding error.
    #[error("base64 decode error: {0}")]
    Base64Decode(String),
}

/// Token store specific errors.
#[derive(Debug, Error)]
pub enum TokenStoreError {
    /// Token generation failed.
    #[error("failed to generate token: {0}")]
    Generation(String),

    /// Storage capacity exceeded.
    #[error("token store capacity exceeded")]
    CapacityExceeded,

    /// Internal store error.
    #[error("internal store error: {0}")]
    Internal(String),
}
