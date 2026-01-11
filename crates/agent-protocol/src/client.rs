//! Agent client for communicating with external agents.
//!
//! Supports two transport mechanisms:
//! - Unix domain sockets (length-prefixed JSON)
//! - gRPC (Protocol Buffers over HTTP/2, with optional TLS)

use serde::Serialize;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use tracing::{debug, error, trace, warn};

use crate::errors::AgentProtocolError;
use crate::grpc::{self, agent_processor_client::AgentProcessorClient};
use crate::protocol::{
    AgentRequest, AgentResponse, AuditMetadata, BodyMutation, Decision, EventType, HeaderOp,
    RequestBodyChunkEvent, RequestCompleteEvent, RequestHeadersEvent, RequestMetadata,
    ResponseBodyChunkEvent, ResponseHeadersEvent, WebSocketDecision, WebSocketFrameEvent,
    MAX_MESSAGE_SIZE, PROTOCOL_VERSION,
};

/// TLS configuration for gRPC agent connections
#[derive(Debug, Clone, Default)]
pub struct GrpcTlsConfig {
    /// Skip certificate verification (DANGEROUS - only for testing)
    pub insecure_skip_verify: bool,
    /// CA certificate PEM data for verifying the server
    pub ca_cert_pem: Option<Vec<u8>>,
    /// Client certificate PEM data for mTLS
    pub client_cert_pem: Option<Vec<u8>>,
    /// Client key PEM data for mTLS
    pub client_key_pem: Option<Vec<u8>>,
    /// Domain name to use for TLS SNI and certificate validation
    pub domain_name: Option<String>,
}

impl GrpcTlsConfig {
    /// Create a new TLS config builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Load CA certificate from a file
    pub async fn with_ca_cert_file(mut self, path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        self.ca_cert_pem = Some(tokio::fs::read(path).await?);
        Ok(self)
    }

    /// Set CA certificate from PEM data
    pub fn with_ca_cert_pem(mut self, pem: impl Into<Vec<u8>>) -> Self {
        self.ca_cert_pem = Some(pem.into());
        self
    }

    /// Load client certificate and key from files (for mTLS)
    pub async fn with_client_cert_files(
        mut self,
        cert_path: impl AsRef<Path>,
        key_path: impl AsRef<Path>,
    ) -> Result<Self, std::io::Error> {
        self.client_cert_pem = Some(tokio::fs::read(cert_path).await?);
        self.client_key_pem = Some(tokio::fs::read(key_path).await?);
        Ok(self)
    }

    /// Set client certificate and key from PEM data (for mTLS)
    pub fn with_client_identity(mut self, cert_pem: impl Into<Vec<u8>>, key_pem: impl Into<Vec<u8>>) -> Self {
        self.client_cert_pem = Some(cert_pem.into());
        self.client_key_pem = Some(key_pem.into());
        self
    }

    /// Set the domain name for TLS SNI and certificate validation
    pub fn with_domain_name(mut self, domain: impl Into<String>) -> Self {
        self.domain_name = Some(domain.into());
        self
    }

    /// Skip certificate verification (DANGEROUS - only for testing)
    pub fn with_insecure_skip_verify(mut self) -> Self {
        self.insecure_skip_verify = true;
        self
    }
}

/// Agent client for communicating with external agents
pub struct AgentClient {
    /// Agent ID
    id: String,
    /// Connection to agent
    connection: AgentConnection,
    /// Timeout for agent calls
    timeout: Duration,
    /// Maximum retries
    #[allow(dead_code)]
    max_retries: u32,
}

/// Agent connection type
enum AgentConnection {
    UnixSocket(UnixStream),
    Grpc(AgentProcessorClient<Channel>),
}

impl AgentClient {
    /// Create a new Unix socket agent client
    pub async fn unix_socket(
        id: impl Into<String>,
        path: impl AsRef<std::path::Path>,
        timeout: Duration,
    ) -> Result<Self, AgentProtocolError> {
        let id = id.into();
        let path = path.as_ref();

        trace!(
            agent_id = %id,
            socket_path = %path.display(),
            timeout_ms = timeout.as_millis() as u64,
            "Connecting to agent via Unix socket"
        );

        let stream = UnixStream::connect(path).await.map_err(|e| {
            error!(
                agent_id = %id,
                socket_path = %path.display(),
                error = %e,
                "Failed to connect to agent via Unix socket"
            );
            AgentProtocolError::ConnectionFailed(e.to_string())
        })?;

        debug!(
            agent_id = %id,
            socket_path = %path.display(),
            "Connected to agent via Unix socket"
        );

        Ok(Self {
            id,
            connection: AgentConnection::UnixSocket(stream),
            timeout,
            max_retries: 3,
        })
    }

    /// Create a new gRPC agent client
    ///
    /// # Arguments
    /// * `id` - Agent identifier
    /// * `address` - gRPC server address (e.g., "http://localhost:50051")
    /// * `timeout` - Timeout for agent calls
    pub async fn grpc(
        id: impl Into<String>,
        address: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, AgentProtocolError> {
        let id = id.into();
        let address = address.into();

        trace!(
            agent_id = %id,
            address = %address,
            timeout_ms = timeout.as_millis() as u64,
            "Connecting to agent via gRPC"
        );

        let channel = Channel::from_shared(address.clone())
            .map_err(|e| {
                error!(
                    agent_id = %id,
                    address = %address,
                    error = %e,
                    "Invalid gRPC URI"
                );
                AgentProtocolError::ConnectionFailed(format!("Invalid URI: {}", e))
            })?
            .timeout(timeout)
            .connect()
            .await
            .map_err(|e| {
                error!(
                    agent_id = %id,
                    address = %address,
                    error = %e,
                    "Failed to connect to agent via gRPC"
                );
                AgentProtocolError::ConnectionFailed(format!("gRPC connect failed: {}", e))
            })?;

        let client = AgentProcessorClient::new(channel);

        debug!(
            agent_id = %id,
            address = %address,
            "Connected to agent via gRPC"
        );

        Ok(Self {
            id,
            connection: AgentConnection::Grpc(client),
            timeout,
            max_retries: 3,
        })
    }

    /// Create a new gRPC agent client with TLS
    ///
    /// # Arguments
    /// * `id` - Agent identifier
    /// * `address` - gRPC server address (e.g., "https://localhost:50051")
    /// * `timeout` - Timeout for agent calls
    /// * `tls_config` - TLS configuration
    pub async fn grpc_tls(
        id: impl Into<String>,
        address: impl Into<String>,
        timeout: Duration,
        tls_config: GrpcTlsConfig,
    ) -> Result<Self, AgentProtocolError> {
        let id = id.into();
        let address = address.into();

        trace!(
            agent_id = %id,
            address = %address,
            timeout_ms = timeout.as_millis() as u64,
            has_ca_cert = tls_config.ca_cert_pem.is_some(),
            has_client_cert = tls_config.client_cert_pem.is_some(),
            insecure = tls_config.insecure_skip_verify,
            "Connecting to agent via gRPC with TLS"
        );

        // Build TLS config
        let mut client_tls_config = ClientTlsConfig::new();

        // Set domain name for SNI if provided, otherwise extract from address
        if let Some(domain) = &tls_config.domain_name {
            client_tls_config = client_tls_config.domain_name(domain.clone());
        } else {
            // Try to extract domain from address URL
            if let Some(domain) = Self::extract_domain(&address) {
                client_tls_config = client_tls_config.domain_name(domain);
            }
        }

        // Add CA certificate if provided
        if let Some(ca_pem) = &tls_config.ca_cert_pem {
            let ca_cert = Certificate::from_pem(ca_pem);
            client_tls_config = client_tls_config.ca_certificate(ca_cert);
            debug!(
                agent_id = %id,
                "Using custom CA certificate for gRPC TLS"
            );
        }

        // Add client identity for mTLS if provided
        if let (Some(cert_pem), Some(key_pem)) = (&tls_config.client_cert_pem, &tls_config.client_key_pem) {
            let identity = Identity::from_pem(cert_pem, key_pem);
            client_tls_config = client_tls_config.identity(identity);
            debug!(
                agent_id = %id,
                "Using client certificate for mTLS to gRPC agent"
            );
        }

        // Handle insecure skip verify (dangerous - only for testing)
        if tls_config.insecure_skip_verify {
            warn!(
                agent_id = %id,
                address = %address,
                "SECURITY WARNING: TLS certificate verification disabled for gRPC agent connection"
            );
            // Note: tonic doesn't have a direct "skip verify" option like some other libraries
            // The best we can do is use a permissive TLS config. For truly insecure connections,
            // users should use the non-TLS grpc() method instead.
        }

        // Build channel with TLS
        let channel = Channel::from_shared(address.clone())
            .map_err(|e| {
                error!(
                    agent_id = %id,
                    address = %address,
                    error = %e,
                    "Invalid gRPC URI"
                );
                AgentProtocolError::ConnectionFailed(format!("Invalid URI: {}", e))
            })?
            .tls_config(client_tls_config)
            .map_err(|e| {
                error!(
                    agent_id = %id,
                    address = %address,
                    error = %e,
                    "Invalid TLS configuration"
                );
                AgentProtocolError::ConnectionFailed(format!("TLS config error: {}", e))
            })?
            .timeout(timeout)
            .connect()
            .await
            .map_err(|e| {
                error!(
                    agent_id = %id,
                    address = %address,
                    error = %e,
                    "Failed to connect to agent via gRPC with TLS"
                );
                AgentProtocolError::ConnectionFailed(format!("gRPC TLS connect failed: {}", e))
            })?;

        let client = AgentProcessorClient::new(channel);

        debug!(
            agent_id = %id,
            address = %address,
            "Connected to agent via gRPC with TLS"
        );

        Ok(Self {
            id,
            connection: AgentConnection::Grpc(client),
            timeout,
            max_retries: 3,
        })
    }

    /// Extract domain name from a URL for TLS SNI
    fn extract_domain(address: &str) -> Option<String> {
        // Try to parse as URL and extract host
        let address = address.trim();

        // Handle URLs like "https://example.com:443" or "http://example.com:8080"
        if let Some(rest) = address.strip_prefix("https://").or_else(|| address.strip_prefix("http://")) {
            // Split off port and path
            let host = rest.split(':').next()?.split('/').next()?;
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }

        None
    }

    /// Get the agent ID
    #[allow(dead_code)]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Send an event to the agent and get a response
    pub async fn send_event(
        &mut self,
        event_type: EventType,
        payload: impl Serialize,
    ) -> Result<AgentResponse, AgentProtocolError> {
        match &mut self.connection {
            AgentConnection::UnixSocket(_) => {
                self.send_event_unix_socket(event_type, payload).await
            }
            AgentConnection::Grpc(_) => self.send_event_grpc(event_type, payload).await,
        }
    }

    /// Send event via Unix socket (length-prefixed JSON)
    async fn send_event_unix_socket(
        &mut self,
        event_type: EventType,
        payload: impl Serialize,
    ) -> Result<AgentResponse, AgentProtocolError> {
        let request = AgentRequest {
            version: PROTOCOL_VERSION,
            event_type,
            payload: serde_json::to_value(payload)
                .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?,
        };

        // Serialize request
        let request_bytes = serde_json::to_vec(&request)
            .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;

        // Check message size
        if request_bytes.len() > MAX_MESSAGE_SIZE {
            return Err(AgentProtocolError::MessageTooLarge {
                size: request_bytes.len(),
                max: MAX_MESSAGE_SIZE,
            });
        }

        // Send with timeout
        let response = tokio::time::timeout(self.timeout, async {
            self.send_raw_unix(&request_bytes).await?;
            self.receive_raw_unix().await
        })
        .await
        .map_err(|_| AgentProtocolError::Timeout(self.timeout))??;

        // Parse response
        let agent_response: AgentResponse = serde_json::from_slice(&response)
            .map_err(|e| AgentProtocolError::InvalidMessage(e.to_string()))?;

        // Verify protocol version
        if agent_response.version != PROTOCOL_VERSION {
            return Err(AgentProtocolError::VersionMismatch {
                expected: PROTOCOL_VERSION,
                actual: agent_response.version,
            });
        }

        Ok(agent_response)
    }

    /// Send event via gRPC
    async fn send_event_grpc(
        &mut self,
        event_type: EventType,
        payload: impl Serialize,
    ) -> Result<AgentResponse, AgentProtocolError> {
        // Build request first (doesn't need mutable borrow)
        let grpc_request = Self::build_grpc_request(event_type, payload)?;

        let AgentConnection::Grpc(client) = &mut self.connection else {
            return Err(AgentProtocolError::WrongConnectionType(
                "Expected gRPC connection but found Unix socket".to_string()
            ));
        };

        // Send with timeout
        let response = tokio::time::timeout(self.timeout, client.process_event(grpc_request))
            .await
            .map_err(|_| AgentProtocolError::Timeout(self.timeout))?
            .map_err(|e| {
                AgentProtocolError::ConnectionFailed(format!("gRPC call failed: {}", e))
            })?;

        // Convert gRPC response to internal format
        Self::convert_grpc_response(response.into_inner())
    }

    /// Build a gRPC request from internal types
    fn build_grpc_request(
        event_type: EventType,
        payload: impl Serialize,
    ) -> Result<grpc::AgentRequest, AgentProtocolError> {
        let payload_json = serde_json::to_value(&payload)
            .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;

        let grpc_event_type = match event_type {
            EventType::Configure => {
                return Err(AgentProtocolError::Serialization(
                    "Configure events are not supported via gRPC".to_string(),
                ))
            }
            EventType::RequestHeaders => grpc::EventType::RequestHeaders,
            EventType::RequestBodyChunk => grpc::EventType::RequestBodyChunk,
            EventType::ResponseHeaders => grpc::EventType::ResponseHeaders,
            EventType::ResponseBodyChunk => grpc::EventType::ResponseBodyChunk,
            EventType::RequestComplete => grpc::EventType::RequestComplete,
            EventType::WebSocketFrame => grpc::EventType::WebsocketFrame,
            EventType::GuardrailInspect => {
                return Err(AgentProtocolError::Serialization(
                    "GuardrailInspect events are not yet supported via gRPC".to_string(),
                ))
            }
        };

        let event = match event_type {
            EventType::Configure => {
                return Err(AgentProtocolError::InvalidMessage(
                    "Configure event should be handled separately".to_string()
                ));
            },
            EventType::RequestHeaders => {
                let event: RequestHeadersEvent = serde_json::from_value(payload_json)
                    .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;
                grpc::agent_request::Event::RequestHeaders(grpc::RequestHeadersEvent {
                    metadata: Some(Self::convert_metadata_to_grpc(&event.metadata)),
                    method: event.method,
                    uri: event.uri,
                    headers: event
                        .headers
                        .into_iter()
                        .map(|(k, v)| (k, grpc::HeaderValues { values: v }))
                        .collect(),
                })
            }
            EventType::RequestBodyChunk => {
                let event: RequestBodyChunkEvent = serde_json::from_value(payload_json)
                    .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;
                grpc::agent_request::Event::RequestBodyChunk(grpc::RequestBodyChunkEvent {
                    correlation_id: event.correlation_id,
                    data: event.data.into_bytes(),
                    is_last: event.is_last,
                    total_size: event.total_size.map(|s| s as u64),
                    chunk_index: event.chunk_index,
                    bytes_received: event.bytes_received as u64,
                })
            }
            EventType::ResponseHeaders => {
                let event: ResponseHeadersEvent = serde_json::from_value(payload_json)
                    .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;
                grpc::agent_request::Event::ResponseHeaders(grpc::ResponseHeadersEvent {
                    correlation_id: event.correlation_id,
                    status: event.status as u32,
                    headers: event
                        .headers
                        .into_iter()
                        .map(|(k, v)| (k, grpc::HeaderValues { values: v }))
                        .collect(),
                })
            }
            EventType::ResponseBodyChunk => {
                let event: ResponseBodyChunkEvent = serde_json::from_value(payload_json)
                    .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;
                grpc::agent_request::Event::ResponseBodyChunk(grpc::ResponseBodyChunkEvent {
                    correlation_id: event.correlation_id,
                    data: event.data.into_bytes(),
                    is_last: event.is_last,
                    total_size: event.total_size.map(|s| s as u64),
                    chunk_index: event.chunk_index,
                    bytes_sent: event.bytes_sent as u64,
                })
            }
            EventType::RequestComplete => {
                let event: RequestCompleteEvent = serde_json::from_value(payload_json)
                    .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;
                grpc::agent_request::Event::RequestComplete(grpc::RequestCompleteEvent {
                    correlation_id: event.correlation_id,
                    status: event.status as u32,
                    duration_ms: event.duration_ms,
                    request_body_size: event.request_body_size as u64,
                    response_body_size: event.response_body_size as u64,
                    upstream_attempts: event.upstream_attempts,
                    error: event.error,
                })
            }
            EventType::WebSocketFrame => {
                use base64::{engine::general_purpose::STANDARD, Engine as _};
                let event: WebSocketFrameEvent = serde_json::from_value(payload_json)
                    .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;
                grpc::agent_request::Event::WebsocketFrame(grpc::WebSocketFrameEvent {
                    correlation_id: event.correlation_id,
                    opcode: event.opcode,
                    data: STANDARD.decode(&event.data).unwrap_or_default(),
                    client_to_server: event.client_to_server,
                    frame_index: event.frame_index,
                    fin: event.fin,
                    route_id: event.route_id,
                    client_ip: event.client_ip,
                })
            }
            EventType::GuardrailInspect => {
                return Err(AgentProtocolError::InvalidMessage(
                    "GuardrailInspect events are not yet supported via gRPC".to_string()
                ));
            }
        };

        Ok(grpc::AgentRequest {
            version: PROTOCOL_VERSION,
            event_type: grpc_event_type as i32,
            event: Some(event),
        })
    }

    /// Convert internal metadata to gRPC format
    fn convert_metadata_to_grpc(metadata: &RequestMetadata) -> grpc::RequestMetadata {
        grpc::RequestMetadata {
            correlation_id: metadata.correlation_id.clone(),
            request_id: metadata.request_id.clone(),
            client_ip: metadata.client_ip.clone(),
            client_port: metadata.client_port as u32,
            server_name: metadata.server_name.clone(),
            protocol: metadata.protocol.clone(),
            tls_version: metadata.tls_version.clone(),
            tls_cipher: metadata.tls_cipher.clone(),
            route_id: metadata.route_id.clone(),
            upstream_id: metadata.upstream_id.clone(),
            timestamp: metadata.timestamp.clone(),
            traceparent: metadata.traceparent.clone(),
        }
    }

    /// Convert gRPC response to internal format
    fn convert_grpc_response(
        response: grpc::AgentResponse,
    ) -> Result<AgentResponse, AgentProtocolError> {
        let decision = match response.decision {
            Some(grpc::agent_response::Decision::Allow(_)) => Decision::Allow,
            Some(grpc::agent_response::Decision::Block(b)) => Decision::Block {
                status: b.status as u16,
                body: b.body,
                headers: if b.headers.is_empty() {
                    None
                } else {
                    Some(b.headers)
                },
            },
            Some(grpc::agent_response::Decision::Redirect(r)) => Decision::Redirect {
                url: r.url,
                status: r.status as u16,
            },
            Some(grpc::agent_response::Decision::Challenge(c)) => Decision::Challenge {
                challenge_type: c.challenge_type,
                params: c.params,
            },
            None => Decision::Allow, // Default to allow if no decision
        };

        let request_headers: Vec<HeaderOp> = response
            .request_headers
            .into_iter()
            .filter_map(Self::convert_header_op_from_grpc)
            .collect();

        let response_headers: Vec<HeaderOp> = response
            .response_headers
            .into_iter()
            .filter_map(Self::convert_header_op_from_grpc)
            .collect();

        let audit = response.audit.map(|a| AuditMetadata {
            tags: a.tags,
            rule_ids: a.rule_ids,
            confidence: a.confidence,
            reason_codes: a.reason_codes,
            custom: a
                .custom
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect(),
        });

        // Convert body mutations
        let request_body_mutation = response.request_body_mutation.map(|m| BodyMutation {
            data: m.data.map(|d| String::from_utf8_lossy(&d).to_string()),
            chunk_index: m.chunk_index,
        });

        let response_body_mutation = response.response_body_mutation.map(|m| BodyMutation {
            data: m.data.map(|d| String::from_utf8_lossy(&d).to_string()),
            chunk_index: m.chunk_index,
        });

        // Convert WebSocket decision
        let websocket_decision = response
            .websocket_decision
            .map(|ws_decision| match ws_decision {
                grpc::agent_response::WebsocketDecision::WebsocketAllow(_) => {
                    WebSocketDecision::Allow
                }
                grpc::agent_response::WebsocketDecision::WebsocketDrop(_) => {
                    WebSocketDecision::Drop
                }
                grpc::agent_response::WebsocketDecision::WebsocketClose(c) => {
                    WebSocketDecision::Close {
                        code: c.code as u16,
                        reason: c.reason,
                    }
                }
            });

        Ok(AgentResponse {
            version: response.version,
            decision,
            request_headers,
            response_headers,
            routing_metadata: response.routing_metadata,
            audit: audit.unwrap_or_default(),
            needs_more: response.needs_more,
            request_body_mutation,
            response_body_mutation,
            websocket_decision,
        })
    }

    /// Convert gRPC header operation to internal format
    fn convert_header_op_from_grpc(op: grpc::HeaderOp) -> Option<HeaderOp> {
        match op.operation? {
            grpc::header_op::Operation::Set(s) => Some(HeaderOp::Set {
                name: s.name,
                value: s.value,
            }),
            grpc::header_op::Operation::Add(a) => Some(HeaderOp::Add {
                name: a.name,
                value: a.value,
            }),
            grpc::header_op::Operation::Remove(r) => Some(HeaderOp::Remove { name: r.name }),
        }
    }

    /// Send raw bytes to agent (Unix socket only)
    async fn send_raw_unix(&mut self, data: &[u8]) -> Result<(), AgentProtocolError> {
        let AgentConnection::UnixSocket(stream) = &mut self.connection else {
            return Err(AgentProtocolError::WrongConnectionType(
                "Expected Unix socket connection but found gRPC".to_string()
            ));
        };
        // Write message length (4 bytes, big-endian)
        let len_bytes = (data.len() as u32).to_be_bytes();
        stream.write_all(&len_bytes).await?;
        // Write message data
        stream.write_all(data).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Receive raw bytes from agent (Unix socket only)
    async fn receive_raw_unix(&mut self) -> Result<Vec<u8>, AgentProtocolError> {
        let AgentConnection::UnixSocket(stream) = &mut self.connection else {
            return Err(AgentProtocolError::WrongConnectionType(
                "Expected Unix socket connection but found gRPC".to_string()
            ));
        };
        // Read message length (4 bytes, big-endian)
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await?;
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
        Ok(buffer)
    }

    /// Close the agent connection
    pub async fn close(self) -> Result<(), AgentProtocolError> {
        match self.connection {
            AgentConnection::UnixSocket(mut stream) => {
                stream.shutdown().await?;
                Ok(())
            }
            AgentConnection::Grpc(_) => Ok(()), // gRPC channels close automatically
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_domain_https() {
        assert_eq!(
            AgentClient::extract_domain("https://example.com:443"),
            Some("example.com".to_string())
        );
        assert_eq!(
            AgentClient::extract_domain("https://agent.internal:50051"),
            Some("agent.internal".to_string())
        );
        assert_eq!(
            AgentClient::extract_domain("https://localhost:8080/path"),
            Some("localhost".to_string())
        );
    }

    #[test]
    fn test_extract_domain_http() {
        assert_eq!(
            AgentClient::extract_domain("http://example.com:8080"),
            Some("example.com".to_string())
        );
        assert_eq!(
            AgentClient::extract_domain("http://localhost:50051"),
            Some("localhost".to_string())
        );
    }

    #[test]
    fn test_extract_domain_invalid() {
        assert_eq!(AgentClient::extract_domain("example.com:443"), None);
        assert_eq!(AgentClient::extract_domain("tcp://example.com:443"), None);
        assert_eq!(AgentClient::extract_domain(""), None);
    }

    #[test]
    fn test_grpc_tls_config_builder() {
        let config = GrpcTlsConfig::new()
            .with_ca_cert_pem(b"test-ca-cert".to_vec())
            .with_client_identity(b"test-cert".to_vec(), b"test-key".to_vec())
            .with_domain_name("example.com");

        assert!(config.ca_cert_pem.is_some());
        assert!(config.client_cert_pem.is_some());
        assert!(config.client_key_pem.is_some());
        assert_eq!(config.domain_name, Some("example.com".to_string()));
        assert!(!config.insecure_skip_verify);
    }

    #[test]
    fn test_grpc_tls_config_insecure() {
        let config = GrpcTlsConfig::new().with_insecure_skip_verify();

        assert!(config.insecure_skip_verify);
        assert!(config.ca_cert_pem.is_none());
    }
}
