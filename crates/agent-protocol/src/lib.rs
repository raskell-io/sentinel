// Allow large enum variants in generated protobuf code
#![allow(clippy::large_enum_variant)]

//! Agent protocol for Zentinel proxy
//!
//! This crate defines the protocol for communication between the proxy dataplane
//! and external processing agents (WAF, auth, rate limiting, custom logic).
//!
//! The protocol is inspired by SPOE (Stream Processing Offload Engine) and Envoy's ext_proc,
//! designed for bounded, predictable behavior with strong failure isolation.
//!
//! # Architecture
//!
//! - [`AgentHandlerV2`](v2::server::AgentHandlerV2): Trait for implementing agent logic
//! - [`AgentResponse`]: Response from agent with decision and mutations
//! - [`AgentClientV2`](v2::AgentClientV2): Client for sending events to agents from the proxy
//! - [`GrpcAgentServerV2`](v2::server::GrpcAgentServerV2): gRPC server for agents
//! - [`UdsAgentServerV2`](v2::uds_server::UdsAgentServerV2): UDS server for agents
//!
//! # Transports
//!
//! Two transport options are supported:
//!
//! ## Unix Domain Sockets (Default)
//! Messages are length-prefixed with negotiated encoding (JSON or MessagePack):
//! - 4-byte big-endian length prefix
//! - Encoded payload (max 10MB)
//!
//! ## gRPC
//! Binary protocol using Protocol Buffers over HTTP/2:
//! - Better performance for high-throughput scenarios
//! - Native support for TLS/mTLS
//! - Language-agnostic (agents can be written in any language with gRPC support)

#![allow(dead_code)]

pub mod binary;
pub mod buffer_pool;
mod errors;
pub mod headers;
#[cfg(feature = "mmap-buffers")]
pub mod mmap_buffer;
mod protocol;

/// Protocol v2 types with bidirectional streaming, capabilities, and flow control
pub mod v2;

/// gRPC v2 protocol definitions generated from proto/agent_v2.proto
pub mod grpc_v2 {
    tonic::include_proto!("zentinel.agent.v2");
}

// Re-export error types
pub use errors::AgentProtocolError;

// Re-export protocol types
pub use protocol::{
    AgentResponse, AuditMetadata, BinaryRequestBodyChunkEvent, BinaryResponseBodyChunkEvent,
    BodyMutation, Decision, DetectionSeverity, EventType, GuardrailDetection,
    GuardrailInspectEvent, GuardrailInspectionType, GuardrailResponse, HeaderOp,
    RequestBodyChunkEvent, RequestCompleteEvent, RequestHeadersEvent, RequestMetadata,
    ResponseBodyChunkEvent, ResponseHeadersEvent, TextSpan, WebSocketDecision,
    WebSocketFrameEvent, WebSocketOpcode, MAX_MESSAGE_SIZE,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_body_mutation_types() {
        // Test pass-through mutation
        let pass_through = BodyMutation::pass_through(0);
        assert!(pass_through.is_pass_through());
        assert!(!pass_through.is_drop());
        assert_eq!(pass_through.chunk_index, 0);

        // Test drop mutation
        let drop = BodyMutation::drop_chunk(1);
        assert!(!drop.is_pass_through());
        assert!(drop.is_drop());
        assert_eq!(drop.chunk_index, 1);

        // Test replace mutation
        let replace = BodyMutation::replace(2, "modified content".to_string());
        assert!(!replace.is_pass_through());
        assert!(!replace.is_drop());
        assert_eq!(replace.chunk_index, 2);
        assert_eq!(replace.data, Some("modified content".to_string()));
    }

    #[test]
    fn test_agent_response_streaming() {
        // Test needs_more_data response
        let response = AgentResponse::needs_more_data();
        assert!(response.needs_more);
        assert_eq!(response.decision, Decision::Allow);

        // Test response with body mutation
        let mutation = BodyMutation::replace(0, "new content".to_string());
        let response = AgentResponse::default_allow().with_request_body_mutation(mutation.clone());
        assert!(!response.needs_more);
        assert!(response.request_body_mutation.is_some());
        assert_eq!(
            response.request_body_mutation.unwrap().data,
            Some("new content".to_string())
        );

        // Test set_needs_more
        let response = AgentResponse::default_allow().set_needs_more(true);
        assert!(response.needs_more);
    }
}
