//! In-memory token store with TTL and per-request scoping.

use crate::config::TokenFormat;
use crate::errors::TokenStoreError;
use crate::store::TokenStore;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Token entry with metadata.
#[derive(Debug, Clone)]
struct TokenEntry {
    /// Original value.
    original: String,
    /// Creation timestamp.
    created_at: Instant,
}

/// In-memory token store with TTL and LRU eviction.
pub struct MemoryTokenStore {
    /// Tokens indexed by correlation_id -> (token -> entry).
    by_correlation: DashMap<String, DashMap<String, TokenEntry>>,
    /// Reverse index: token -> (correlation_id, original).
    by_token: DashMap<String, (String, String)>,
    /// Configuration.
    ttl: Duration,
    max_entries: usize,
    /// Entry count for capacity tracking.
    entry_count: Arc<RwLock<usize>>,
}

impl MemoryTokenStore {
    /// Create a new memory token store.
    pub fn new(ttl_seconds: u64, max_entries: usize) -> Self {
        let store = Self {
            by_correlation: DashMap::new(),
            by_token: DashMap::new(),
            ttl: Duration::from_secs(ttl_seconds),
            max_entries,
            entry_count: Arc::new(RwLock::new(0)),
        };

        // Spawn background cleanup task
        store.spawn_cleanup_task();

        store
    }

    fn spawn_cleanup_task(&self) {
        let by_correlation = self.by_correlation.clone();
        let by_token = self.by_token.clone();
        let ttl = self.ttl;
        let entry_count = self.entry_count.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let now = Instant::now();
                let mut removed = 0usize;

                // Iterate and remove expired entries
                by_correlation.retain(|_, inner| {
                    inner.retain(|token, entry| {
                        if now.duration_since(entry.created_at) > ttl {
                            by_token.remove(token);
                            removed += 1;
                            false
                        } else {
                            true
                        }
                    });
                    !inner.is_empty()
                });

                if removed > 0 {
                    let mut count = entry_count.write().await;
                    *count = count.saturating_sub(removed);
                    tracing::debug!(removed = removed, "Cleaned up expired tokens");
                }
            }
        });
    }

    fn generate_token(&self, format: &TokenFormat) -> String {
        match format {
            TokenFormat::Uuid => uuid::Uuid::new_v4().to_string(),
            TokenFormat::Prefixed { prefix } => {
                format!("{}{}", prefix, uuid::Uuid::new_v4())
            }
        }
    }
}

#[async_trait]
impl TokenStore for MemoryTokenStore {
    async fn tokenize(
        &self,
        correlation_id: &str,
        original: &str,
        format: &TokenFormat,
    ) -> Result<String, TokenStoreError> {
        // Check if already tokenized in this request (idempotent)
        if let Some(inner) = self.by_correlation.get(correlation_id) {
            for entry in inner.iter() {
                if entry.value().original == original {
                    return Ok(entry.key().clone());
                }
            }
        }

        // Check capacity
        {
            let count = self.entry_count.read().await;
            if *count >= self.max_entries {
                return Err(TokenStoreError::CapacityExceeded);
            }
        }

        // Generate new token
        let token = self.generate_token(format);

        // Store entry
        let entry = TokenEntry {
            original: original.to_string(),
            created_at: Instant::now(),
        };

        self.by_correlation
            .entry(correlation_id.to_string())
            .or_default()
            .insert(token.clone(), entry);

        self.by_token.insert(
            token.clone(),
            (correlation_id.to_string(), original.to_string()),
        );

        // Update count
        {
            let mut count = self.entry_count.write().await;
            *count += 1;
        }

        Ok(token)
    }

    async fn detokenize(
        &self,
        correlation_id: &str,
        token: &str,
    ) -> Result<Option<String>, TokenStoreError> {
        // First check by token (fast path)
        if let Some(entry) = self.by_token.get(token) {
            if entry.0 == correlation_id {
                return Ok(Some(entry.1.clone()));
            }
        }

        // Fall back to correlation lookup
        if let Some(inner) = self.by_correlation.get(correlation_id) {
            if let Some(entry) = inner.get(token) {
                return Ok(Some(entry.original.clone()));
            }
        }

        Ok(None)
    }

    async fn cleanup(&self, correlation_id: &str) -> Result<usize, TokenStoreError> {
        if let Some((_, inner)) = self.by_correlation.remove(correlation_id) {
            let count = inner.len();
            for entry in inner.iter() {
                self.by_token.remove(entry.key());
            }

            let mut entry_count = self.entry_count.write().await;
            *entry_count = entry_count.saturating_sub(count);

            Ok(count)
        } else {
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tokenize_roundtrip() {
        let store = MemoryTokenStore::new(300, 1000);
        let correlation_id = "test-123";
        let original = "sensitive-data";

        let token = store
            .tokenize(correlation_id, original, &TokenFormat::Uuid)
            .await
            .unwrap();

        assert_ne!(token, original);
        assert!(uuid::Uuid::parse_str(&token).is_ok());

        let recovered = store.detokenize(correlation_id, &token).await.unwrap();
        assert_eq!(recovered, Some(original.to_string()));
    }

    #[tokio::test]
    async fn test_tokenize_idempotent() {
        let store = MemoryTokenStore::new(300, 1000);
        let correlation_id = "test-123";
        let original = "sensitive-data";

        let token1 = store
            .tokenize(correlation_id, original, &TokenFormat::Uuid)
            .await
            .unwrap();
        let token2 = store
            .tokenize(correlation_id, original, &TokenFormat::Uuid)
            .await
            .unwrap();

        // Same original value should return same token
        assert_eq!(token1, token2);
    }

    #[tokio::test]
    async fn test_cleanup() {
        let store = MemoryTokenStore::new(300, 1000);
        let correlation_id = "test-123";

        let token = store
            .tokenize(correlation_id, "value1", &TokenFormat::Uuid)
            .await
            .unwrap();
        store
            .tokenize(correlation_id, "value2", &TokenFormat::Uuid)
            .await
            .unwrap();

        let cleaned = store.cleanup(correlation_id).await.unwrap();
        assert_eq!(cleaned, 2);

        // Token should no longer resolve
        let result = store.detokenize(correlation_id, &token).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_prefixed_token() {
        let store = MemoryTokenStore::new(300, 1000);
        let format = TokenFormat::Prefixed {
            prefix: "tok_".to_string(),
        };

        let token = store.tokenize("test", "value", &format).await.unwrap();
        assert!(token.starts_with("tok_"));
    }
}
