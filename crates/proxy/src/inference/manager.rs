//! Inference rate limit manager
//!
//! Manages token-based rate limiters per route, integrating with the
//! request flow to apply token rate limiting for inference endpoints.

use dashmap::DashMap;
use http::HeaderMap;
use std::sync::Arc;
use tracing::{debug, info, trace};

use sentinel_config::{InferenceConfig, TokenEstimation};

use super::providers::create_provider;
use super::rate_limit::{TokenRateLimitResult, TokenRateLimiter};
use super::tokens::{TokenCounter, TokenEstimate, TokenSource};

/// Per-route inference rate limiter with token counter
struct RouteInferenceState {
    /// Token rate limiter
    rate_limiter: TokenRateLimiter,
    /// Token counter (for estimation and actual counting)
    token_counter: TokenCounter,
    /// Route ID for logging
    route_id: String,
}

/// Manager for inference rate limiting across routes
///
/// Each route with inference configuration gets its own TokenRateLimiter
/// and TokenCounter for that route's provider configuration.
pub struct InferenceRateLimitManager {
    /// Per-route inference state (keyed by route ID)
    routes: DashMap<String, Arc<RouteInferenceState>>,
}

impl InferenceRateLimitManager {
    /// Create a new inference rate limit manager
    pub fn new() -> Self {
        Self {
            routes: DashMap::new(),
        }
    }

    /// Register a route with inference configuration
    ///
    /// Creates a TokenRateLimiter and TokenCounter for the route.
    pub fn register_route(&self, route_id: &str, config: &InferenceConfig) {
        // Only register if rate limiting is configured
        if let Some(ref rate_limit) = config.rate_limit {
            let provider = create_provider(&config.provider);
            let estimation_method = rate_limit.estimation_method;

            let token_counter = TokenCounter::new(provider, estimation_method);
            let rate_limiter = TokenRateLimiter::new(rate_limit.clone());

            let state = RouteInferenceState {
                rate_limiter,
                token_counter,
                route_id: route_id.to_string(),
            };

            self.routes.insert(route_id.to_string(), Arc::new(state));

            info!(
                route_id = route_id,
                provider = ?config.provider,
                tokens_per_minute = rate_limit.tokens_per_minute,
                requests_per_minute = ?rate_limit.requests_per_minute,
                burst_tokens = rate_limit.burst_tokens,
                estimation_method = ?estimation_method,
                "Registered inference rate limiter"
            );
        }
    }

    /// Check if a route has inference rate limiting configured
    pub fn has_route(&self, route_id: &str) -> bool {
        self.routes.contains_key(route_id)
    }

    /// Check rate limit for a request
    ///
    /// Returns the rate limit result and the estimated token count.
    pub fn check(
        &self,
        route_id: &str,
        key: &str,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Option<InferenceCheckResult> {
        let state = self.routes.get(route_id)?;

        // Estimate tokens for the request
        let estimate = state.token_counter.estimate_request(headers, body);

        trace!(
            route_id = route_id,
            key = key,
            estimated_tokens = estimate.tokens,
            model = ?estimate.model,
            "Checking inference rate limit"
        );

        // Check rate limit
        let result = state.rate_limiter.check(key, estimate.tokens);

        Some(InferenceCheckResult {
            result,
            estimated_tokens: estimate.tokens,
            model: estimate.model,
        })
    }

    /// Record actual token usage from response
    ///
    /// This adjusts the rate limiter based on actual vs estimated usage.
    pub fn record_actual(
        &self,
        route_id: &str,
        key: &str,
        headers: &HeaderMap,
        body: &[u8],
        estimated_tokens: u64,
    ) -> Option<TokenEstimate> {
        let state = self.routes.get(route_id)?;

        // Get actual token count from response
        let actual = state.token_counter.tokens_from_response(headers, body);

        // Only record if we got actual tokens
        if actual.tokens > 0 && actual.source != TokenSource::Estimated {
            state
                .rate_limiter
                .record_actual(key, actual.tokens, estimated_tokens);

            debug!(
                route_id = route_id,
                key = key,
                actual_tokens = actual.tokens,
                estimated_tokens = estimated_tokens,
                source = ?actual.source,
                "Recorded actual token usage"
            );
        }

        Some(actual)
    }

    /// Get the number of registered routes
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Get stats for a route
    pub fn route_stats(&self, route_id: &str) -> Option<InferenceRouteStats> {
        let state = self.routes.get(route_id)?;
        let stats = state.rate_limiter.stats();

        Some(InferenceRouteStats {
            route_id: route_id.to_string(),
            active_keys: stats.active_keys,
            tokens_per_minute: stats.tokens_per_minute,
            requests_per_minute: stats.requests_per_minute,
        })
    }

    /// Clean up idle rate limiters (called periodically)
    pub fn cleanup(&self) {
        // Currently, cleanup is handled internally by the rate limiters
        // This is a hook for future cleanup logic
        trace!("Inference rate limit cleanup");
    }
}

impl Default for InferenceRateLimitManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of an inference rate limit check
#[derive(Debug)]
pub struct InferenceCheckResult {
    /// Rate limit decision
    pub result: TokenRateLimitResult,
    /// Estimated tokens for this request
    pub estimated_tokens: u64,
    /// Model name if detected
    pub model: Option<String>,
}

impl InferenceCheckResult {
    /// Returns true if the request is allowed
    pub fn is_allowed(&self) -> bool {
        self.result.is_allowed()
    }

    /// Get retry-after value in milliseconds (0 if allowed)
    pub fn retry_after_ms(&self) -> u64 {
        self.result.retry_after_ms()
    }
}

/// Stats for a route's inference rate limiter
#[derive(Debug, Clone)]
pub struct InferenceRouteStats {
    /// Route ID
    pub route_id: String,
    /// Number of active rate limit keys
    pub active_keys: usize,
    /// Configured tokens per minute
    pub tokens_per_minute: u64,
    /// Configured requests per minute (if any)
    pub requests_per_minute: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_config::{InferenceProvider, TokenRateLimit};

    fn test_inference_config() -> InferenceConfig {
        InferenceConfig {
            provider: InferenceProvider::OpenAi,
            model_header: None,
            rate_limit: Some(TokenRateLimit {
                tokens_per_minute: 10000,
                requests_per_minute: Some(100),
                burst_tokens: 2000,
                estimation_method: TokenEstimation::Chars,
            }),
            routing: None,
        }
    }

    #[test]
    fn test_register_route() {
        let manager = InferenceRateLimitManager::new();
        manager.register_route("test-route", &test_inference_config());

        assert!(manager.has_route("test-route"));
        assert!(!manager.has_route("other-route"));
    }

    #[test]
    fn test_check_rate_limit() {
        let manager = InferenceRateLimitManager::new();
        manager.register_route("test-route", &test_inference_config());

        let headers = HeaderMap::new();
        let body = br#"{"messages": [{"content": "Hello world"}]}"#;

        let result = manager.check("test-route", "client-1", &headers, body);
        assert!(result.is_some());

        let check = result.unwrap();
        assert!(check.is_allowed());
        assert!(check.estimated_tokens > 0);
    }

    #[test]
    fn test_no_rate_limit_config() {
        let manager = InferenceRateLimitManager::new();

        // Config without rate_limit should not register
        let config = InferenceConfig {
            provider: InferenceProvider::OpenAi,
            model_header: None,
            rate_limit: None,
            routing: None,
        };
        manager.register_route("no-limit-route", &config);

        assert!(!manager.has_route("no-limit-route"));
    }
}
