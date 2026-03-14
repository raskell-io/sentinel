//! Error types for the Gateway API controller.

use thiserror::Error;

/// Errors that can occur in the Gateway API controller.
#[derive(Debug, Error)]
pub enum GatewayError {
    /// Kubernetes API error
    #[error("Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    /// Invalid Gateway API resource
    #[error("Invalid Gateway API resource '{name}': {reason}")]
    InvalidResource { name: String, reason: String },

    /// Cross-namespace reference denied
    #[error(
        "Cross-namespace reference denied: {source_namespace}/{source_kind} → \
         {target_namespace}/{target_kind}/{target_name}"
    )]
    ReferenceNotPermitted {
        source_namespace: String,
        source_kind: String,
        target_namespace: String,
        target_kind: String,
        target_name: String,
    },

    /// Config translation error
    #[error("Config translation error: {0}")]
    Translation(String),

    /// Finalizer error
    #[error("Finalizer error: {0}")]
    Finalizer(#[source] Box<kube::runtime::finalizer::Error<GatewayError>>),
}

/// Result type for Gateway API controller operations.
pub type Result<T> = std::result::Result<T, GatewayError>;
