//! Token store implementations.

mod memory;

pub use memory::MemoryTokenStore;

use crate::config::TokenFormat;
use crate::errors::TokenStoreError;
use async_trait::async_trait;

/// Token store trait for reversible tokenization.
#[async_trait]
pub trait TokenStore: Send + Sync {
    /// Store a value and return its token.
    async fn tokenize(
        &self,
        correlation_id: &str,
        original: &str,
        format: &TokenFormat,
    ) -> Result<String, TokenStoreError>;

    /// Retrieve original value from token.
    async fn detokenize(
        &self,
        correlation_id: &str,
        token: &str,
    ) -> Result<Option<String>, TokenStoreError>;

    /// Clean up tokens for a completed request.
    async fn cleanup(&self, correlation_id: &str) -> Result<usize, TokenStoreError>;
}
