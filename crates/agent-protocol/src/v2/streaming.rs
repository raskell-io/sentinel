//! Bidirectional streaming types for Protocol v2.

use crate::{AuditMetadata, Decision, HeaderOp};
use serde::{Deserialize, Serialize};

/// Flow control signal for backpressure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowControlSignal {
    pub correlation_id: Option<String>,
    pub action: FlowAction,
    pub timestamp_ms: u64,
}

impl FlowControlSignal {
    pub fn pause_all() -> Self {
        Self {
            correlation_id: None,
            action: FlowAction::Pause,
            timestamp_ms: now_ms(),
        }
    }

    pub fn resume_all() -> Self {
        Self {
            correlation_id: None,
            action: FlowAction::Resume,
            timestamp_ms: now_ms(),
        }
    }

    pub fn is_global(&self) -> bool {
        self.correlation_id.is_none()
    }
}

/// Flow control action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum FlowAction {
    Pause,
    Resume,
    UpdateCapacity { buffer_available: usize },
}

/// Body chunk event with flow control support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodyChunkEventV2 {
    pub correlation_id: String,
    pub chunk_index: u32,
    pub data: String,
    pub is_last: bool,
    pub total_size: Option<usize>,
    pub bytes_transferred: usize,
    pub proxy_buffer_available: usize,
    pub timestamp_ms: u64,
}

/// Agent response to a processing event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub correlation_id: String,
    pub decision: Decision,
    #[serde(default)]
    pub request_headers: Vec<HeaderOp>,
    #[serde(default)]
    pub response_headers: Vec<HeaderOp>,
    #[serde(default)]
    pub audit: AuditMetadata,
    pub processing_time_ms: Option<u64>,
    pub needs_more: bool,
}

impl AgentResponse {
    pub fn allow(correlation_id: impl Into<String>) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            decision: Decision::Allow,
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            audit: AuditMetadata::default(),
            processing_time_ms: None,
            needs_more: false,
        }
    }

    pub fn block(correlation_id: impl Into<String>, status: u16) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            decision: Decision::Block {
                status,
                body: None,
                headers: None,
            },
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            audit: AuditMetadata::default(),
            processing_time_ms: None,
            needs_more: false,
        }
    }

    pub fn with_request_header(mut self, op: HeaderOp) -> Self {
        self.request_headers.push(op);
        self
    }

    pub fn with_processing_time(mut self, ms: u64) -> Self {
        self.processing_time_ms = Some(ms);
        self
    }

    pub fn with_audit(mut self, audit: AuditMetadata) -> Self {
        self.audit = audit;
        self
    }
}

/// Stream state tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamState {
    #[default]
    Disconnected,
    Handshaking,
    Active,
    Paused,
    Draining,
    Closed,
}

impl StreamState {
    pub fn can_accept_requests(&self) -> bool {
        matches!(self, StreamState::Active)
    }
    pub fn is_connected(&self) -> bool {
        !matches!(self, StreamState::Disconnected | StreamState::Closed)
    }
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
    fn test_flow_control_signal() {
        let pause = FlowControlSignal::pause_all();
        assert!(pause.is_global());
        assert_eq!(pause.action, FlowAction::Pause);
    }

    #[test]
    fn test_agent_response() {
        let response = AgentResponse::allow("req-123").with_processing_time(5);
        assert!(matches!(response.decision, Decision::Allow));
        assert_eq!(response.processing_time_ms, Some(5));
    }

    #[test]
    fn test_stream_state() {
        assert!(!StreamState::Disconnected.can_accept_requests());
        assert!(StreamState::Active.can_accept_requests());
        assert!(StreamState::Active.is_connected());
        assert!(!StreamState::Closed.is_connected());
    }
}
