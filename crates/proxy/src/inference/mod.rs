//! Inference routing module for LLM/AI traffic patterns
//!
//! This module provides:
//! - Token-based rate limiting (tokens/minute instead of requests/second)
//! - Multi-provider token counting (OpenAI, Anthropic, generic)
//! - Model-aware load balancing (LeastTokensQueued strategy)
//!
//! # Example Usage
//!
//! ```kdl
//! route "/v1/chat/completions" {
//!     inference {
//!         provider "openai"
//!         rate-limit {
//!             tokens-per-minute 100000
//!             burst-tokens 10000
//!         }
//!         routing {
//!             strategy "least-tokens-queued"
//!         }
//!     }
//!     upstream "llm-pool" { ... }
//! }
//! ```

mod manager;
mod providers;
mod rate_limit;
mod tokens;

pub use manager::{InferenceCheckResult, InferenceRateLimitManager, InferenceRouteStats};
pub use providers::{create_provider, InferenceProviderAdapter};
pub use rate_limit::{TokenRateLimitResult, TokenRateLimiter};
pub use tokens::{TokenCounter, TokenEstimate, TokenSource};

use sentinel_config::{InferenceConfig, InferenceProvider};

/// Create a provider adapter based on the configured provider type
pub fn create_inference_provider(config: &InferenceConfig) -> Box<dyn InferenceProviderAdapter> {
    create_provider(&config.provider)
}
