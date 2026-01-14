//! HTTP-01 ACME challenge management
//!
//! Manages pending ACME HTTP-01 challenges for serving via
//! `/.well-known/acme-challenge/<token>`.

use dashmap::DashMap;
use std::sync::Arc;
use tracing::{debug, trace};

/// HTTP-01 challenge path prefix
pub const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

/// Manages pending ACME HTTP-01 challenges
///
/// When the ACME server needs to validate domain ownership, it requests
/// a specific URL path. This manager stores the token -> key authorization
/// mapping so the proxy can serve the correct response.
///
/// # Thread Safety
///
/// Uses `DashMap` for lock-free concurrent access from multiple request
/// handling threads.
#[derive(Debug)]
pub struct ChallengeManager {
    /// Map of challenge token -> key authorization response
    challenges: Arc<DashMap<String, String>>,
}

impl ChallengeManager {
    /// Create a new challenge manager
    pub fn new() -> Self {
        Self {
            challenges: Arc::new(DashMap::new()),
        }
    }

    /// Register a pending challenge
    ///
    /// Called when starting the ACME challenge flow. The key authorization
    /// will be served when the ACME server requests the challenge URL.
    ///
    /// # Arguments
    ///
    /// * `token` - The challenge token from the ACME server
    /// * `key_authorization` - The response to return (token + account key thumbprint)
    pub fn add_challenge(&self, token: &str, key_authorization: &str) {
        debug!(token = %token, "Registering ACME HTTP-01 challenge");
        self.challenges
            .insert(token.to_string(), key_authorization.to_string());
    }

    /// Remove a completed or expired challenge
    ///
    /// Called after the challenge is validated or times out.
    pub fn remove_challenge(&self, token: &str) {
        if self.challenges.remove(token).is_some() {
            debug!(token = %token, "Removed ACME challenge");
        }
    }

    /// Get the key authorization response for a challenge token
    ///
    /// Returns `Some(key_authorization)` if the token is registered,
    /// `None` otherwise.
    pub fn get_response(&self, token: &str) -> Option<String> {
        let result = self.challenges.get(token).map(|v| v.clone());
        if result.is_some() {
            trace!(token = %token, "ACME challenge token found");
        } else {
            trace!(token = %token, "ACME challenge token not found");
        }
        result
    }

    /// Check if this is an ACME challenge request path
    ///
    /// Returns `Some(token)` if the path matches the challenge prefix,
    /// `None` otherwise.
    pub fn extract_token(path: &str) -> Option<&str> {
        path.strip_prefix(ACME_CHALLENGE_PREFIX)
    }

    /// Get the number of pending challenges
    pub fn pending_count(&self) -> usize {
        self.challenges.len()
    }

    /// Clear all pending challenges
    ///
    /// Called during shutdown or reset.
    pub fn clear(&self) {
        let count = self.challenges.len();
        self.challenges.clear();
        if count > 0 {
            debug!(cleared = count, "Cleared all pending ACME challenges");
        }
    }
}

impl Default for ChallengeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ChallengeManager {
    fn clone(&self) -> Self {
        Self {
            challenges: Arc::clone(&self.challenges),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_challenge() {
        let manager = ChallengeManager::new();

        manager.add_challenge("test-token", "test-key-auth");

        let response = manager.get_response("test-token");
        assert_eq!(response, Some("test-key-auth".to_string()));
    }

    #[test]
    fn test_get_nonexistent_challenge() {
        let manager = ChallengeManager::new();

        let response = manager.get_response("nonexistent");
        assert_eq!(response, None);
    }

    #[test]
    fn test_remove_challenge() {
        let manager = ChallengeManager::new();

        manager.add_challenge("test-token", "test-key-auth");
        assert_eq!(manager.pending_count(), 1);

        manager.remove_challenge("test-token");
        assert_eq!(manager.pending_count(), 0);

        let response = manager.get_response("test-token");
        assert_eq!(response, None);
    }

    #[test]
    fn test_extract_token() {
        assert_eq!(
            ChallengeManager::extract_token("/.well-known/acme-challenge/abc123"),
            Some("abc123")
        );

        assert_eq!(
            ChallengeManager::extract_token("/.well-known/acme-challenge/"),
            Some("")
        );

        assert_eq!(ChallengeManager::extract_token("/other/path"), None);

        assert_eq!(
            ChallengeManager::extract_token("/.well-known/acme-challenge"),
            None
        );
    }

    #[test]
    fn test_clear_challenges() {
        let manager = ChallengeManager::new();

        manager.add_challenge("token1", "auth1");
        manager.add_challenge("token2", "auth2");
        assert_eq!(manager.pending_count(), 2);

        manager.clear();
        assert_eq!(manager.pending_count(), 0);
    }

    #[test]
    fn test_clone_shares_state() {
        let manager1 = ChallengeManager::new();
        let manager2 = manager1.clone();

        manager1.add_challenge("token", "auth");

        // Clone should see the same challenge
        assert_eq!(manager2.get_response("token"), Some("auth".to_string()));
    }
}
