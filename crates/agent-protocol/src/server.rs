//! Agent server for implementing external agents.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info};

use crate::errors::AgentProtocolError;
use crate::protocol::{
    AgentRequest, AgentResponse, AuditMetadata, EventType, HeaderOp, RequestBodyChunkEvent,
    RequestCompleteEvent, RequestHeadersEvent, ResponseBodyChunkEvent, ResponseHeadersEvent,
    MAX_MESSAGE_SIZE,
};

/// Agent server for testing and reference implementations
pub struct AgentServer {
    /// Agent ID
    id: String,
    /// Unix socket path
    socket_path: std::path::PathBuf,
    /// Request handler
    handler: Arc<dyn AgentHandler>,
}

/// Trait for implementing agent logic
#[async_trait]
pub trait AgentHandler: Send + Sync {
    /// Handle a request headers event
    async fn on_request_headers(&self, _event: RequestHeadersEvent) -> AgentResponse {
        AgentResponse::default_allow()
    }

    /// Handle a request body chunk event
    async fn on_request_body_chunk(&self, _event: RequestBodyChunkEvent) -> AgentResponse {
        AgentResponse::default_allow()
    }

    /// Handle a response headers event
    async fn on_response_headers(&self, _event: ResponseHeadersEvent) -> AgentResponse {
        AgentResponse::default_allow()
    }

    /// Handle a response body chunk event
    async fn on_response_body_chunk(&self, _event: ResponseBodyChunkEvent) -> AgentResponse {
        AgentResponse::default_allow()
    }

    /// Handle a request complete event
    async fn on_request_complete(&self, _event: RequestCompleteEvent) -> AgentResponse {
        AgentResponse::default_allow()
    }
}

impl AgentServer {
    /// Create a new agent server
    pub fn new(
        id: impl Into<String>,
        socket_path: impl Into<std::path::PathBuf>,
        handler: Box<dyn AgentHandler>,
    ) -> Self {
        Self {
            id: id.into(),
            socket_path: socket_path.into(),
            handler: Arc::from(handler),
        }
    }

    /// Start the agent server
    pub async fn run(&self) -> Result<(), AgentProtocolError> {
        // Remove existing socket file if it exists
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        // Create Unix socket listener
        let listener = UnixListener::bind(&self.socket_path)?;

        info!(
            "Agent server '{}' listening on {:?}",
            self.id, self.socket_path
        );

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let handler = Arc::clone(&self.handler);
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, handler.as_ref()).await {
                            error!("Error handling agent connection: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    /// Handle a single connection
    async fn handle_connection(
        mut stream: UnixStream,
        handler: &dyn AgentHandler,
    ) -> Result<(), AgentProtocolError> {
        loop {
            // Read message length
            let mut len_bytes = [0u8; 4];
            match stream.read_exact(&mut len_bytes).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Client disconnected
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }

            let message_len = u32::from_be_bytes(len_bytes) as usize;

            // Check message size
            if message_len > MAX_MESSAGE_SIZE {
                return Err(AgentProtocolError::MessageTooLarge {
                    size: message_len,
                    max: MAX_MESSAGE_SIZE,
                });
            }

            // Read message data
            let mut buffer = vec![0u8; message_len];
            stream.read_exact(&mut buffer).await?;

            // Parse request
            let request: AgentRequest = serde_json::from_slice(&buffer)
                .map_err(|e| AgentProtocolError::InvalidMessage(e.to_string()))?;

            // Handle request based on event type
            let response = match request.event_type {
                EventType::RequestHeaders => {
                    let event: RequestHeadersEvent = serde_json::from_value(request.payload)
                        .map_err(|e| AgentProtocolError::InvalidMessage(e.to_string()))?;
                    handler.on_request_headers(event).await
                }
                EventType::RequestBodyChunk => {
                    let event: RequestBodyChunkEvent = serde_json::from_value(request.payload)
                        .map_err(|e| AgentProtocolError::InvalidMessage(e.to_string()))?;
                    handler.on_request_body_chunk(event).await
                }
                EventType::ResponseHeaders => {
                    let event: ResponseHeadersEvent = serde_json::from_value(request.payload)
                        .map_err(|e| AgentProtocolError::InvalidMessage(e.to_string()))?;
                    handler.on_response_headers(event).await
                }
                EventType::ResponseBodyChunk => {
                    let event: ResponseBodyChunkEvent = serde_json::from_value(request.payload)
                        .map_err(|e| AgentProtocolError::InvalidMessage(e.to_string()))?;
                    handler.on_response_body_chunk(event).await
                }
                EventType::RequestComplete => {
                    let event: RequestCompleteEvent = serde_json::from_value(request.payload)
                        .map_err(|e| AgentProtocolError::InvalidMessage(e.to_string()))?;
                    handler.on_request_complete(event).await
                }
            };

            // Send response
            let response_bytes = serde_json::to_vec(&response)
                .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;

            // Write message length
            let len_bytes = (response_bytes.len() as u32).to_be_bytes();
            stream.write_all(&len_bytes).await?;
            // Write message data
            stream.write_all(&response_bytes).await?;
            stream.flush().await?;
        }
    }
}

/// Reference implementation: Echo agent (for testing)
pub struct EchoAgent;

#[async_trait]
impl AgentHandler for EchoAgent {
    async fn on_request_headers(&self, event: RequestHeadersEvent) -> AgentResponse {
        debug!(
            "Echo agent: request headers for {}",
            event.metadata.correlation_id
        );

        // Echo back correlation ID as a header
        AgentResponse::default_allow()
            .add_request_header(HeaderOp::Set {
                name: "X-Echo-Agent".to_string(),
                value: event.metadata.correlation_id.clone(),
            })
            .with_audit(AuditMetadata {
                tags: vec!["echo".to_string()],
                ..Default::default()
            })
    }
}

/// Reference implementation: Denylist agent
pub struct DenylistAgent {
    blocked_paths: Vec<String>,
    blocked_ips: Vec<String>,
}

impl DenylistAgent {
    pub fn new(blocked_paths: Vec<String>, blocked_ips: Vec<String>) -> Self {
        Self {
            blocked_paths,
            blocked_ips,
        }
    }
}

#[async_trait]
impl AgentHandler for DenylistAgent {
    async fn on_request_headers(&self, event: RequestHeadersEvent) -> AgentResponse {
        // Check if path is blocked
        for blocked_path in &self.blocked_paths {
            if event.uri.starts_with(blocked_path) {
                return AgentResponse::block(403, Some("Forbidden path".to_string())).with_audit(
                    AuditMetadata {
                        tags: vec!["denylist".to_string(), "blocked_path".to_string()],
                        reason_codes: vec!["PATH_BLOCKED".to_string()],
                        ..Default::default()
                    },
                );
            }
        }

        // Check if IP is blocked
        if self.blocked_ips.contains(&event.metadata.client_ip) {
            return AgentResponse::block(403, Some("Forbidden IP".to_string())).with_audit(
                AuditMetadata {
                    tags: vec!["denylist".to_string(), "blocked_ip".to_string()],
                    reason_codes: vec!["IP_BLOCKED".to_string()],
                    ..Default::default()
                },
            );
        }

        AgentResponse::default_allow()
    }
}
