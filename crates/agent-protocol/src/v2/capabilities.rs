//! Agent capability negotiation for Protocol v2.

use crate::EventType;
use serde::{Deserialize, Serialize};

/// Agent capabilities declared during handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub protocol_version: u32,
    pub agent_id: String,
    pub name: String,
    pub version: String,
    pub supported_events: Vec<EventType>,
    #[serde(default)]
    pub features: AgentFeatures,
    #[serde(default)]
    pub limits: AgentLimits,
    #[serde(default)]
    pub health: HealthConfig,
}

impl AgentCapabilities {
    pub fn new(
        agent_id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            protocol_version: super::PROTOCOL_VERSION_2,
            agent_id: agent_id.into(),
            name: name.into(),
            version: version.into(),
            supported_events: vec![EventType::RequestHeaders],
            features: AgentFeatures::default(),
            limits: AgentLimits::default(),
            health: HealthConfig::default(),
        }
    }

    pub fn supports_event(&self, event_type: EventType) -> bool {
        self.supported_events.contains(&event_type)
    }

    pub fn with_event(mut self, event_type: EventType) -> Self {
        if !self.supported_events.contains(&event_type) {
            self.supported_events.push(event_type);
        }
        self
    }

    pub fn with_features(mut self, features: AgentFeatures) -> Self {
        self.features = features;
        self
    }

    pub fn with_limits(mut self, limits: AgentLimits) -> Self {
        self.limits = limits;
        self
    }
}

/// Features this agent supports.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFeatures {
    #[serde(default)]
    pub streaming_body: bool,
    #[serde(default)]
    pub websocket: bool,
    #[serde(default)]
    pub guardrails: bool,
    #[serde(default)]
    pub config_push: bool,
    #[serde(default)]
    pub metrics_export: bool,
    #[serde(default)]
    pub concurrent_requests: u32,
    #[serde(default)]
    pub cancellation: bool,
    #[serde(default)]
    pub flow_control: bool,
    #[serde(default)]
    pub health_reporting: bool,
}

impl AgentFeatures {
    pub fn simple() -> Self {
        Self::default()
    }
    pub fn full() -> Self {
        Self {
            streaming_body: true,
            websocket: true,
            guardrails: true,
            config_push: true,
            metrics_export: true,
            concurrent_requests: 100,
            cancellation: true,
            flow_control: true,
            health_reporting: true,
        }
    }
}

/// Resource limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLimits {
    pub max_body_size: usize,
    pub max_concurrency: u32,
    pub preferred_chunk_size: usize,
    pub max_memory: Option<usize>,
    pub max_processing_time_ms: Option<u64>,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_body_size: 10 * 1024 * 1024,
            max_concurrency: 100,
            preferred_chunk_size: 64 * 1024,
            max_memory: None,
            max_processing_time_ms: Some(5000),
        }
    }
}

/// Health check configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    pub report_interval_ms: u32,
    pub include_load_metrics: bool,
    pub include_resource_metrics: bool,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            report_interval_ms: 10_000,
            include_load_metrics: true,
            include_resource_metrics: false,
        }
    }
}

/// Handshake request from proxy to agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeRequest {
    pub supported_versions: Vec<u32>,
    pub proxy_id: String,
    pub proxy_version: String,
    pub config: serde_json::Value,
}

impl HandshakeRequest {
    pub fn new(proxy_id: impl Into<String>, proxy_version: impl Into<String>) -> Self {
        Self {
            supported_versions: vec![super::PROTOCOL_VERSION_2, 1],
            proxy_id: proxy_id.into(),
            proxy_version: proxy_version.into(),
            config: serde_json::Value::Null,
        }
    }

    pub fn max_version(&self) -> u32 {
        self.supported_versions.first().copied().unwrap_or(1)
    }
}

/// Handshake response from agent to proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    pub protocol_version: u32,
    pub capabilities: AgentCapabilities,
    pub success: bool,
    pub error: Option<String>,
}

impl HandshakeResponse {
    pub fn success(capabilities: AgentCapabilities) -> Self {
        Self {
            protocol_version: capabilities.protocol_version,
            capabilities,
            success: true,
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            protocol_version: 0,
            capabilities: AgentCapabilities::new("", "", ""),
            success: false,
            error: Some(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities_builder() {
        let caps = AgentCapabilities::new("test-agent", "Test Agent", "1.0.0")
            .with_event(EventType::RequestHeaders)
            .with_features(AgentFeatures::full());

        assert_eq!(caps.agent_id, "test-agent");
        assert!(caps.supports_event(EventType::RequestHeaders));
        assert!(caps.features.streaming_body);
    }

    #[test]
    fn test_handshake() {
        let request = HandshakeRequest::new("proxy-1", "0.2.5");
        assert_eq!(request.max_version(), 2);

        let caps = AgentCapabilities::new("agent-1", "My Agent", "1.0.0");
        let response = HandshakeResponse::success(caps);
        assert!(response.success);
    }

    #[test]
    fn test_features() {
        let simple = AgentFeatures::simple();
        assert!(!simple.streaming_body);

        let full = AgentFeatures::full();
        assert!(full.streaming_body);
    }
}
