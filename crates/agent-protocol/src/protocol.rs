//! Agent protocol types and constants.
//!
//! This module defines the wire protocol types for communication between
//! the proxy dataplane and external processing agents.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent protocol version
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum message size (10MB)
pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Agent event type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// Request headers received
    RequestHeaders,
    /// Request body chunk received
    RequestBodyChunk,
    /// Response headers received
    ResponseHeaders,
    /// Response body chunk received
    ResponseBodyChunk,
    /// Request/response complete (for logging)
    RequestComplete,
}

/// Agent decision
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Allow the request/response to continue
    Allow,
    /// Block the request/response
    Block {
        /// HTTP status code to return
        status: u16,
        /// Optional response body
        body: Option<String>,
        /// Optional response headers
        headers: Option<HashMap<String, String>>,
    },
    /// Redirect the request
    Redirect {
        /// Redirect URL
        url: String,
        /// HTTP status code (301, 302, 303, 307, 308)
        status: u16,
    },
    /// Challenge the client (e.g., CAPTCHA)
    Challenge {
        /// Challenge type
        challenge_type: String,
        /// Challenge parameters
        params: HashMap<String, String>,
    },
}

impl Default for Decision {
    fn default() -> Self {
        Self::Allow
    }
}

/// Header modification operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderOp {
    /// Set a header (replace if exists)
    Set { name: String, value: String },
    /// Add a header (append if exists)
    Add { name: String, value: String },
    /// Remove a header
    Remove { name: String },
}

/// Request metadata sent to agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMetadata {
    /// Correlation ID for request tracing
    pub correlation_id: String,
    /// Request ID (internal)
    pub request_id: String,
    /// Client IP address
    pub client_ip: String,
    /// Client port
    pub client_port: u16,
    /// Server name (SNI or Host header)
    pub server_name: Option<String>,
    /// Protocol (HTTP/1.1, HTTP/2, etc.)
    pub protocol: String,
    /// TLS version if applicable
    pub tls_version: Option<String>,
    /// TLS cipher suite if applicable
    pub tls_cipher: Option<String>,
    /// Route ID that matched
    pub route_id: Option<String>,
    /// Upstream ID
    pub upstream_id: Option<String>,
    /// Request start timestamp (RFC3339)
    pub timestamp: String,
}

/// Request headers event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestHeadersEvent {
    /// Event metadata
    pub metadata: RequestMetadata,
    /// HTTP method
    pub method: String,
    /// Request URI
    pub uri: String,
    /// HTTP headers
    pub headers: HashMap<String, Vec<String>>,
}

/// Request body chunk event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBodyChunkEvent {
    /// Correlation ID
    pub correlation_id: String,
    /// Body chunk data (base64 encoded for JSON transport)
    pub data: String,
    /// Is this the last chunk?
    pub is_last: bool,
    /// Total body size if known
    pub total_size: Option<usize>,
}

/// Response headers event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseHeadersEvent {
    /// Correlation ID
    pub correlation_id: String,
    /// HTTP status code
    pub status: u16,
    /// HTTP headers
    pub headers: HashMap<String, Vec<String>>,
}

/// Response body chunk event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseBodyChunkEvent {
    /// Correlation ID
    pub correlation_id: String,
    /// Body chunk data (base64 encoded for JSON transport)
    pub data: String,
    /// Is this the last chunk?
    pub is_last: bool,
    /// Total body size if known
    pub total_size: Option<usize>,
}

/// Request complete event (for logging/audit)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestCompleteEvent {
    /// Correlation ID
    pub correlation_id: String,
    /// Final HTTP status code
    pub status: u16,
    /// Request duration in milliseconds
    pub duration_ms: u64,
    /// Request body size
    pub request_body_size: usize,
    /// Response body size
    pub response_body_size: usize,
    /// Upstream attempts
    pub upstream_attempts: u32,
    /// Error if any
    pub error: Option<String>,
}

/// Agent request message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Protocol version
    pub version: u32,
    /// Event type
    pub event_type: EventType,
    /// Event payload (JSON)
    pub payload: serde_json::Value,
}

/// Agent response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Protocol version
    pub version: u32,
    /// Decision
    pub decision: Decision,
    /// Header modifications for request
    #[serde(default)]
    pub request_headers: Vec<HeaderOp>,
    /// Header modifications for response
    #[serde(default)]
    pub response_headers: Vec<HeaderOp>,
    /// Routing metadata modifications
    #[serde(default)]
    pub routing_metadata: HashMap<String, String>,
    /// Audit metadata
    #[serde(default)]
    pub audit: AuditMetadata,
}

impl AgentResponse {
    /// Create a default allow response
    pub fn default_allow() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            decision: Decision::Allow,
            request_headers: vec![],
            response_headers: vec![],
            routing_metadata: HashMap::new(),
            audit: AuditMetadata::default(),
        }
    }

    /// Create a block response
    pub fn block(status: u16, body: Option<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            decision: Decision::Block {
                status,
                body,
                headers: None,
            },
            request_headers: vec![],
            response_headers: vec![],
            routing_metadata: HashMap::new(),
            audit: AuditMetadata::default(),
        }
    }

    /// Create a redirect response
    pub fn redirect(url: String, status: u16) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            decision: Decision::Redirect { url, status },
            request_headers: vec![],
            response_headers: vec![],
            routing_metadata: HashMap::new(),
            audit: AuditMetadata::default(),
        }
    }

    /// Add a request header modification
    pub fn add_request_header(mut self, op: HeaderOp) -> Self {
        self.request_headers.push(op);
        self
    }

    /// Add a response header modification
    pub fn add_response_header(mut self, op: HeaderOp) -> Self {
        self.response_headers.push(op);
        self
    }

    /// Add audit metadata
    pub fn with_audit(mut self, audit: AuditMetadata) -> Self {
        self.audit = audit;
        self
    }
}

/// Audit metadata from agent
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditMetadata {
    /// Tags for logging/metrics
    #[serde(default)]
    pub tags: Vec<String>,
    /// Rule IDs that matched
    #[serde(default)]
    pub rule_ids: Vec<String>,
    /// Confidence score (0.0 - 1.0)
    pub confidence: Option<f32>,
    /// Reason codes
    #[serde(default)]
    pub reason_codes: Vec<String>,
    /// Custom metadata
    #[serde(default)]
    pub custom: HashMap<String, serde_json::Value>,
}
