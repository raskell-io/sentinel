//! Type-safe identifier newtypes for Sentinel proxy.
//!
//! These types provide compile-time safety for identifiers, preventing
//! accidental mixing of different ID types (e.g., passing a RouteId
//! where an UpstreamId is expected).

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Unique correlation ID for request tracing across components.
///
/// Correlation IDs follow requests through the entire proxy pipeline,
/// enabling end-to-end tracing and log correlation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CorrelationId(String);

impl CorrelationId {
    /// Create a new random correlation ID
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create from an existing string
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Get the inner string value
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert to owned String
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for CorrelationId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for CorrelationId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Unique request ID for internal tracking.
///
/// Request IDs are generated per-request and used for internal
/// metrics, logging, and debugging.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(String);

impl RequestId {
    /// Create a new random request ID
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Get the inner string value
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Route identifier.
///
/// Identifies a configured route in the proxy. Routes define
/// how requests are matched and forwarded to upstreams.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteId(String);

impl RouteId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RouteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Upstream identifier.
///
/// Identifies a configured upstream pool. Upstreams are groups
/// of backend servers that handle requests.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UpstreamId(String);

impl UpstreamId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UpstreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Agent identifier.
///
/// Identifies a configured external processing agent (WAF, auth, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correlation_id() {
        let id1 = CorrelationId::new();
        let id2 = CorrelationId::from_string("test-id");

        assert_ne!(id1, id2);
        assert_eq!(id2.as_str(), "test-id");
    }

    #[test]
    fn test_route_id() {
        let id = RouteId::new("my-route");
        assert_eq!(id.as_str(), "my-route");
        assert_eq!(id.to_string(), "my-route");
    }

    #[test]
    fn test_upstream_id() {
        let id = UpstreamId::new("backend-pool");
        assert_eq!(id.as_str(), "backend-pool");
    }

    #[test]
    fn test_agent_id() {
        let id = AgentId::new("waf-agent");
        assert_eq!(id.as_str(), "waf-agent");
    }
}
