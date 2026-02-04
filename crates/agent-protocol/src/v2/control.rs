//! Control plane messages for Protocol v2.

use serde::{Deserialize, Serialize};

/// Request to cancel an in-flight request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelRequest {
    pub correlation_id: String,
    pub reason: CancelReason,
    pub timestamp_ms: u64,
}

impl CancelRequest {
    pub fn new(correlation_id: impl Into<String>, reason: CancelReason) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            reason,
            timestamp_ms: now_ms(),
        }
    }

    pub fn timeout(correlation_id: impl Into<String>) -> Self {
        Self::new(correlation_id, CancelReason::Timeout)
    }
}

/// Reason for request cancellation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CancelReason {
    ClientDisconnect,
    Timeout,
    BlockedByAgent { agent_id: String },
    UpstreamError,
    ProxyShutdown,
    Manual { reason: String },
}

/// Configuration update request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdateRequest {
    pub update_type: ConfigUpdateType,
    pub request_id: String,
    pub timestamp_ms: u64,
}

/// Type of configuration update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ConfigUpdateType {
    RequestReload,
    RuleUpdate {
        rule_set: String,
        rules: Vec<RuleDefinition>,
        remove_rules: Vec<String>,
    },
    ListUpdate {
        list_id: String,
        add: Vec<String>,
        remove: Vec<String>,
    },
    RestartRequired {
        reason: String,
        grace_period_ms: u64,
    },
    ConfigError {
        error: String,
        field: Option<String>,
    },
}

/// A rule definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleDefinition {
    pub id: String,
    pub priority: i32,
    pub definition: serde_json::Value,
    pub enabled: bool,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Response to a configuration update request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdateResponse {
    pub request_id: String,
    pub accepted: bool,
    pub error: Option<String>,
    pub timestamp_ms: u64,
}

impl ConfigUpdateResponse {
    pub fn success(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            accepted: true,
            error: None,
            timestamp_ms: now_ms(),
        }
    }

    pub fn failure(request_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            accepted: false,
            error: Some(error.into()),
            timestamp_ms: now_ms(),
        }
    }
}

/// Shutdown request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownRequest {
    pub reason: ShutdownReason,
    pub grace_period_ms: u64,
    pub timestamp_ms: u64,
}

/// Reason for shutdown.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    Graceful,
    Immediate,
    ConfigReload,
    Upgrade,
}

/// Drain request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrainRequest {
    pub duration_ms: u64,
    pub reason: DrainReason,
    pub timestamp_ms: u64,
}

/// Reason for draining.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DrainReason {
    ConfigReload,
    Maintenance,
    HealthCheckFailed,
    Manual,
}

/// Log message from agent to proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogMessage {
    pub level: LogLevel,
    pub message: String,
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub fields: std::collections::HashMap<String, serde_json::Value>,
    pub timestamp_ms: u64,
}

/// Log level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cancel_request() {
        let cancel = CancelRequest::timeout("req-123");
        assert_eq!(cancel.correlation_id, "req-123");
        assert_eq!(cancel.reason, CancelReason::Timeout);
    }

    #[test]
    fn test_config_update_response() {
        let success = ConfigUpdateResponse::success("update-1");
        assert!(success.accepted);

        let failure = ConfigUpdateResponse::failure("update-2", "Error");
        assert!(!failure.accepted);
    }
}
