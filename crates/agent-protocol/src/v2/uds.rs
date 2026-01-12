//! Unix Domain Socket transport for Agent Protocol v2.
//!
//! This module provides a binary protocol implementation for v2 over UDS,
//! supporting bidirectional streaming with connection multiplexing.
//!
//! # Wire Format
//!
//! All messages use a length-prefixed binary format:
//! ```text
//! +--------+--------+------------------+
//! | Length | Type   | Payload          |
//! | 4 bytes| 1 byte | variable         |
//! | BE u32 | u8     | MessagePack/JSON |
//! +--------+--------+------------------+
//! ```
//!
//! # Message Types
//!
//! - 0x01: Handshake Request (proxy -> agent)
//! - 0x02: Handshake Response (agent -> proxy)
//! - 0x10: Request Headers Event
//! - 0x11: Request Body Chunk Event
//! - 0x12: Response Headers Event
//! - 0x13: Response Body Chunk Event
//! - 0x14: Request Complete Event
//! - 0x15: WebSocket Frame Event
//! - 0x16: Guardrail Inspect Event
//! - 0x17: Configure Event
//! - 0x20: Agent Response
//! - 0x30: Health Status
//! - 0x31: Metrics Report
//! - 0x32: Config Update Request
//! - 0x33: Flow Control Signal
//! - 0x40: Cancel Request
//! - 0x41: Ping
//! - 0x42: Pong

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tracing::{debug, error, info, trace, warn};

use crate::v2::{AgentCapabilities, AgentFeatures, AgentLimits, HealthConfig, PROTOCOL_VERSION_2};
use crate::{AgentProtocolError, AgentResponse, EventType};

use super::client::{ConfigUpdateCallback, FlowState, MetricsCallback};

/// Maximum message size for UDS transport (16 MB).
pub const MAX_UDS_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Message type identifiers for the binary protocol.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    // Handshake
    HandshakeRequest = 0x01,
    HandshakeResponse = 0x02,

    // Events (proxy -> agent)
    RequestHeaders = 0x10,
    RequestBodyChunk = 0x11,
    ResponseHeaders = 0x12,
    ResponseBodyChunk = 0x13,
    RequestComplete = 0x14,
    WebSocketFrame = 0x15,
    GuardrailInspect = 0x16,
    Configure = 0x17,

    // Response (agent -> proxy)
    AgentResponse = 0x20,

    // Control messages (bidirectional)
    HealthStatus = 0x30,
    MetricsReport = 0x31,
    ConfigUpdateRequest = 0x32,
    FlowControl = 0x33,

    // Management
    Cancel = 0x40,
    Ping = 0x41,
    Pong = 0x42,
}

impl TryFrom<u8> for MessageType {
    type Error = AgentProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(MessageType::HandshakeRequest),
            0x02 => Ok(MessageType::HandshakeResponse),
            0x10 => Ok(MessageType::RequestHeaders),
            0x11 => Ok(MessageType::RequestBodyChunk),
            0x12 => Ok(MessageType::ResponseHeaders),
            0x13 => Ok(MessageType::ResponseBodyChunk),
            0x14 => Ok(MessageType::RequestComplete),
            0x15 => Ok(MessageType::WebSocketFrame),
            0x16 => Ok(MessageType::GuardrailInspect),
            0x17 => Ok(MessageType::Configure),
            0x20 => Ok(MessageType::AgentResponse),
            0x30 => Ok(MessageType::HealthStatus),
            0x31 => Ok(MessageType::MetricsReport),
            0x32 => Ok(MessageType::ConfigUpdateRequest),
            0x33 => Ok(MessageType::FlowControl),
            0x40 => Ok(MessageType::Cancel),
            0x41 => Ok(MessageType::Ping),
            0x42 => Ok(MessageType::Pong),
            _ => Err(AgentProtocolError::InvalidMessage(format!(
                "Unknown message type: 0x{:02x}",
                value
            ))),
        }
    }
}

/// Handshake request sent from proxy to agent over UDS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UdsHandshakeRequest {
    pub supported_versions: Vec<u32>,
    pub proxy_id: String,
    pub proxy_version: String,
    pub config: Option<serde_json::Value>,
}

/// Handshake response from agent to proxy over UDS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UdsHandshakeResponse {
    pub protocol_version: u32,
    pub capabilities: UdsCapabilities,
    pub success: bool,
    pub error: Option<String>,
}

/// Agent capabilities for UDS protocol.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UdsCapabilities {
    pub agent_id: String,
    pub name: String,
    pub version: String,
    pub supported_events: Vec<i32>,
    pub features: UdsFeatures,
    pub limits: UdsLimits,
}

/// Agent features.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UdsFeatures {
    pub streaming_body: bool,
    pub websocket: bool,
    pub guardrails: bool,
    pub config_push: bool,
    pub metrics_export: bool,
    pub concurrent_requests: u32,
    pub cancellation: bool,
    pub flow_control: bool,
    pub health_reporting: bool,
}

/// Agent limits.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UdsLimits {
    pub max_body_size: u64,
    pub max_concurrency: u32,
    pub preferred_chunk_size: u64,
}

impl From<UdsCapabilities> for AgentCapabilities {
    fn from(caps: UdsCapabilities) -> Self {
        AgentCapabilities {
            protocol_version: PROTOCOL_VERSION_2 as u32,
            agent_id: caps.agent_id,
            name: caps.name,
            version: caps.version,
            supported_events: caps
                .supported_events
                .into_iter()
                .filter_map(event_type_from_i32)
                .collect(),
            features: AgentFeatures {
                streaming_body: caps.features.streaming_body,
                websocket: caps.features.websocket,
                guardrails: caps.features.guardrails,
                config_push: caps.features.config_push,
                metrics_export: caps.features.metrics_export,
                concurrent_requests: caps.features.concurrent_requests,
                cancellation: caps.features.cancellation,
                flow_control: caps.features.flow_control,
                health_reporting: caps.features.health_reporting,
            },
            limits: AgentLimits {
                max_body_size: caps.limits.max_body_size as usize,
                max_concurrency: caps.limits.max_concurrency,
                preferred_chunk_size: caps.limits.preferred_chunk_size as usize,
                max_memory: None,
                max_processing_time_ms: None,
            },
            health: HealthConfig::default(),
        }
    }
}

/// Convert i32 to EventType.
fn event_type_from_i32(value: i32) -> Option<EventType> {
    match value {
        0 => Some(EventType::Configure),
        1 => Some(EventType::RequestHeaders),
        2 => Some(EventType::RequestBodyChunk),
        3 => Some(EventType::ResponseHeaders),
        4 => Some(EventType::ResponseBodyChunk),
        5 => Some(EventType::RequestComplete),
        6 => Some(EventType::WebSocketFrame),
        7 => Some(EventType::GuardrailInspect),
        _ => None,
    }
}

/// v2 agent client over Unix Domain Socket.
///
/// This client maintains a single connection and multiplexes multiple requests
/// over it using correlation IDs, similar to the gRPC client.
pub struct AgentClientV2Uds {
    /// Agent identifier
    agent_id: String,
    /// Socket path
    socket_path: String,
    /// Request timeout
    timeout: Duration,
    /// Negotiated capabilities
    capabilities: RwLock<Option<AgentCapabilities>>,
    /// Negotiated protocol version
    protocol_version: AtomicU64,
    /// Pending requests by correlation ID
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<AgentResponse>>>>,
    /// Sender for outbound messages
    outbound_tx: Mutex<Option<mpsc::Sender<(MessageType, Vec<u8>)>>>,
    /// Sequence counter for pings
    ping_sequence: AtomicU64,
    /// Connection state
    connected: RwLock<bool>,
    /// Flow control state
    flow_state: RwLock<FlowState>,
    /// Last known health state
    health_state: RwLock<i32>,
    /// In-flight request count
    in_flight: AtomicU64,
    /// Callback for metrics reports
    metrics_callback: Option<MetricsCallback>,
    /// Callback for config update requests
    config_update_callback: Option<ConfigUpdateCallback>,
}

impl AgentClientV2Uds {
    /// Create a new UDS v2 client.
    pub async fn new(
        agent_id: impl Into<String>,
        socket_path: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, AgentProtocolError> {
        let agent_id = agent_id.into();
        let socket_path = socket_path.into();

        debug!(
            agent_id = %agent_id,
            socket_path = %socket_path,
            timeout_ms = timeout.as_millis(),
            "Creating UDS v2 client"
        );

        Ok(Self {
            agent_id,
            socket_path,
            timeout,
            capabilities: RwLock::new(None),
            protocol_version: AtomicU64::new(0),
            pending: Arc::new(Mutex::new(HashMap::new())),
            outbound_tx: Mutex::new(None),
            ping_sequence: AtomicU64::new(0),
            connected: RwLock::new(false),
            flow_state: RwLock::new(FlowState::Normal),
            health_state: RwLock::new(1), // HEALTHY
            in_flight: AtomicU64::new(0),
            metrics_callback: None,
            config_update_callback: None,
        })
    }

    /// Set the metrics callback.
    pub fn set_metrics_callback(&mut self, callback: MetricsCallback) {
        self.metrics_callback = Some(callback);
    }

    /// Set the config update callback.
    pub fn set_config_update_callback(&mut self, callback: ConfigUpdateCallback) {
        self.config_update_callback = Some(callback);
    }

    /// Connect and perform handshake.
    pub async fn connect(&self) -> Result<(), AgentProtocolError> {
        info!(
            agent_id = %self.agent_id,
            socket_path = %self.socket_path,
            "Connecting to agent via UDS v2"
        );

        // Connect to Unix socket
        let stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
            error!(
                agent_id = %self.agent_id,
                socket_path = %self.socket_path,
                error = %e,
                "Failed to connect to agent via UDS"
            );
            AgentProtocolError::ConnectionFailed(e.to_string())
        })?;

        let (read_half, write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut writer = BufWriter::new(write_half);

        // Send handshake request
        let handshake_req = UdsHandshakeRequest {
            supported_versions: vec![PROTOCOL_VERSION_2 as u32],
            proxy_id: "sentinel-proxy".to_string(),
            proxy_version: env!("CARGO_PKG_VERSION").to_string(),
            config: None,
        };

        let payload = serde_json::to_vec(&handshake_req)
            .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;

        write_message(&mut writer, MessageType::HandshakeRequest, &payload).await?;

        // Read handshake response
        let (msg_type, response_bytes) = read_message(&mut reader).await?;

        if msg_type != MessageType::HandshakeResponse {
            return Err(AgentProtocolError::InvalidMessage(format!(
                "Expected HandshakeResponse, got {:?}",
                msg_type
            )));
        }

        let response: UdsHandshakeResponse = serde_json::from_slice(&response_bytes)
            .map_err(|e| AgentProtocolError::InvalidMessage(e.to_string()))?;

        if !response.success {
            return Err(AgentProtocolError::ConnectionFailed(
                response.error.unwrap_or_else(|| "Unknown handshake error".to_string()),
            ));
        }

        // Store capabilities
        let capabilities: AgentCapabilities = response.capabilities.into();
        *self.capabilities.write().await = Some(capabilities);
        self.protocol_version
            .store(response.protocol_version as u64, Ordering::SeqCst);

        info!(
            agent_id = %self.agent_id,
            protocol_version = response.protocol_version,
            "UDS v2 handshake successful"
        );

        // Create message channel
        let (tx, mut rx) = mpsc::channel::<(MessageType, Vec<u8>)>(1024);
        *self.outbound_tx.lock().await = Some(tx);
        *self.connected.write().await = true;

        // Spawn writer task
        let agent_id_clone = self.agent_id.clone();
        tokio::spawn(async move {
            while let Some((msg_type, payload)) = rx.recv().await {
                if let Err(e) = write_message(&mut writer, msg_type, &payload).await {
                    error!(
                        agent_id = %agent_id_clone,
                        error = %e,
                        "Failed to write message to UDS"
                    );
                    break;
                }
            }
            debug!(agent_id = %agent_id_clone, "UDS writer task ended");
        });

        // Spawn reader task
        let pending = Arc::clone(&self.pending);
        let agent_id = self.agent_id.clone();
        let flow_state = Arc::new(RwLock::new(FlowState::Normal));
        let health_state = Arc::new(RwLock::new(1i32));
        let flow_state_clone = Arc::clone(&flow_state);
        let health_state_clone = Arc::clone(&health_state);
        let metrics_callback = self.metrics_callback.clone();
        let config_update_callback = self.config_update_callback.clone();

        tokio::spawn(async move {
            loop {
                match read_message(&mut reader).await {
                    Ok((msg_type, payload)) => {
                        match msg_type {
                            MessageType::AgentResponse => {
                                match serde_json::from_slice::<AgentResponse>(&payload) {
                                    Ok(response) => {
                                        // Extract correlation ID from the response
                                        // For UDS, we include correlation_id in the response
                                        if let Some(sender) = pending.lock().await.remove(&response.audit.custom.get("correlation_id").and_then(|v| v.as_str()).unwrap_or("").to_string()) {
                                            let _ = sender.send(response);
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            agent_id = %agent_id,
                                            error = %e,
                                            "Failed to parse agent response"
                                        );
                                    }
                                }
                            }
                            MessageType::HealthStatus => {
                                if let Ok(health) = serde_json::from_slice::<serde_json::Value>(&payload) {
                                    if let Some(state) = health.get("state").and_then(|s| s.as_i64()) {
                                        *health_state_clone.write().await = state as i32;
                                    }
                                }
                            }
                            MessageType::MetricsReport => {
                                if let Some(ref callback) = metrics_callback {
                                    if let Ok(report) = serde_json::from_slice(&payload) {
                                        callback(report);
                                    }
                                }
                            }
                            MessageType::FlowControl => {
                                if let Ok(fc) = serde_json::from_slice::<serde_json::Value>(&payload) {
                                    let action = fc.get("action").and_then(|a| a.as_i64()).unwrap_or(0);
                                    let new_state = match action {
                                        1 => FlowState::Paused,
                                        2 => FlowState::Normal,
                                        _ => FlowState::Normal,
                                    };
                                    *flow_state_clone.write().await = new_state;
                                }
                            }
                            MessageType::ConfigUpdateRequest => {
                                if let Some(ref callback) = config_update_callback {
                                    if let Ok(request) = serde_json::from_slice(&payload) {
                                        let _response = callback(agent_id.clone(), request);
                                    }
                                }
                            }
                            MessageType::Pong => {
                                trace!(agent_id = %agent_id, "Received pong");
                            }
                            _ => {
                                trace!(
                                    agent_id = %agent_id,
                                    msg_type = ?msg_type,
                                    "Received unhandled message type"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        if !matches!(e, AgentProtocolError::ConnectionClosed) {
                            error!(
                                agent_id = %agent_id,
                                error = %e,
                                "Error reading from UDS"
                            );
                        }
                        break;
                    }
                }
            }
            debug!(agent_id = %agent_id, "UDS reader task ended");
        });

        Ok(())
    }

    /// Get negotiated capabilities.
    pub async fn capabilities(&self) -> Option<AgentCapabilities> {
        self.capabilities.read().await.clone()
    }

    /// Check if connected.
    pub async fn is_connected(&self) -> bool {
        *self.connected.read().await
    }

    /// Send a request headers event.
    pub async fn send_request_headers(
        &self,
        correlation_id: &str,
        event: &crate::RequestHeadersEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        self.send_event(MessageType::RequestHeaders, correlation_id, event).await
    }

    /// Send a request body chunk event.
    pub async fn send_request_body_chunk(
        &self,
        correlation_id: &str,
        event: &crate::RequestBodyChunkEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        self.send_event(MessageType::RequestBodyChunk, correlation_id, event).await
    }

    /// Send a response headers event.
    pub async fn send_response_headers(
        &self,
        correlation_id: &str,
        event: &crate::ResponseHeadersEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        self.send_event(MessageType::ResponseHeaders, correlation_id, event).await
    }

    /// Send a response body chunk event.
    pub async fn send_response_body_chunk(
        &self,
        correlation_id: &str,
        event: &crate::ResponseBodyChunkEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        self.send_event(MessageType::ResponseBodyChunk, correlation_id, event).await
    }

    /// Send an event and wait for response.
    async fn send_event<T: serde::Serialize>(
        &self,
        msg_type: MessageType,
        correlation_id: &str,
        event: &T,
    ) -> Result<AgentResponse, AgentProtocolError> {
        // Create response channel
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(correlation_id.to_string(), tx);

        // Serialize event with correlation ID
        let mut payload = serde_json::to_value(event)
            .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;

        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "correlation_id".to_string(),
                serde_json::Value::String(correlation_id.to_string()),
            );
        }

        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;

        // Send message
        {
            let outbound = self.outbound_tx.lock().await;
            if let Some(tx) = outbound.as_ref() {
                tx.send((msg_type, payload_bytes))
                    .await
                    .map_err(|_| AgentProtocolError::ConnectionClosed)?;
            } else {
                return Err(AgentProtocolError::ConnectionClosed);
            }
        }

        self.in_flight.fetch_add(1, Ordering::Relaxed);

        // Wait for response with timeout
        let response = tokio::time::timeout(self.timeout, rx)
            .await
            .map_err(|_| {
                self.pending.try_lock().ok().map(|mut p| p.remove(correlation_id));
                AgentProtocolError::Timeout(self.timeout)
            })?
            .map_err(|_| AgentProtocolError::ConnectionClosed)?;

        self.in_flight.fetch_sub(1, Ordering::Relaxed);

        Ok(response)
    }

    /// Send a cancel request for a specific correlation ID.
    pub async fn cancel_request(
        &self,
        correlation_id: &str,
        reason: super::client::CancelReason,
    ) -> Result<(), AgentProtocolError> {
        let cancel = serde_json::json!({
            "correlation_id": correlation_id,
            "reason": reason as i32,
            "timestamp_ms": now_ms(),
        });

        let payload = serde_json::to_vec(&cancel)
            .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;

        let outbound = self.outbound_tx.lock().await;
        if let Some(tx) = outbound.as_ref() {
            tx.send((MessageType::Cancel, payload))
                .await
                .map_err(|_| AgentProtocolError::ConnectionClosed)?;
        }

        // Remove pending request
        self.pending.lock().await.remove(correlation_id);

        Ok(())
    }

    /// Cancel all in-flight requests.
    pub async fn cancel_all(
        &self,
        reason: super::client::CancelReason,
    ) -> Result<usize, AgentProtocolError> {
        let pending_ids: Vec<String> = self.pending.lock().await.keys().cloned().collect();
        let count = pending_ids.len();

        for correlation_id in pending_ids {
            let _ = self.cancel_request(&correlation_id, reason).await;
        }

        Ok(count)
    }

    /// Send a ping.
    pub async fn ping(&self) -> Result<(), AgentProtocolError> {
        let seq = self.ping_sequence.fetch_add(1, Ordering::Relaxed);
        let ping = serde_json::json!({
            "sequence": seq,
            "timestamp_ms": now_ms(),
        });

        let payload = serde_json::to_vec(&ping)
            .map_err(|e| AgentProtocolError::Serialization(e.to_string()))?;

        let outbound = self.outbound_tx.lock().await;
        if let Some(tx) = outbound.as_ref() {
            tx.send((MessageType::Ping, payload))
                .await
                .map_err(|_| AgentProtocolError::ConnectionClosed)?;
        }

        Ok(())
    }

    /// Close the connection.
    pub async fn close(&self) -> Result<(), AgentProtocolError> {
        *self.connected.write().await = false;
        *self.outbound_tx.lock().await = None;
        Ok(())
    }

    /// Get in-flight request count.
    pub fn in_flight(&self) -> u64 {
        self.in_flight.load(Ordering::Relaxed)
    }

    /// Get agent ID.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

/// Write a message to the stream.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg_type: MessageType,
    payload: &[u8],
) -> Result<(), AgentProtocolError> {
    if payload.len() > MAX_UDS_MESSAGE_SIZE {
        return Err(AgentProtocolError::MessageTooLarge {
            size: payload.len(),
            max: MAX_UDS_MESSAGE_SIZE,
        });
    }

    // Write length (4 bytes, big-endian) - includes type byte
    let total_len = (payload.len() + 1) as u32;
    writer.write_all(&total_len.to_be_bytes()).await?;

    // Write message type (1 byte)
    writer.write_all(&[msg_type as u8]).await?;

    // Write payload
    writer.write_all(payload).await?;
    writer.flush().await?;

    Ok(())
}

/// Read a message from the stream.
pub async fn read_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<(MessageType, Vec<u8>), AgentProtocolError> {
    // Read length (4 bytes, big-endian)
    let mut len_bytes = [0u8; 4];
    match reader.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(AgentProtocolError::ConnectionClosed);
        }
        Err(e) => return Err(e.into()),
    }

    let total_len = u32::from_be_bytes(len_bytes) as usize;

    if total_len == 0 {
        return Err(AgentProtocolError::InvalidMessage(
            "Zero-length message".to_string(),
        ));
    }

    if total_len > MAX_UDS_MESSAGE_SIZE {
        return Err(AgentProtocolError::MessageTooLarge {
            size: total_len,
            max: MAX_UDS_MESSAGE_SIZE,
        });
    }

    // Read message type (1 byte)
    let mut type_byte = [0u8; 1];
    reader.read_exact(&mut type_byte).await?;
    let msg_type = MessageType::try_from(type_byte[0])?;

    // Read payload
    let payload_len = total_len - 1;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload).await?;
    }

    Ok((msg_type, payload))
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
    fn test_message_type_roundtrip() {
        let types = [
            MessageType::HandshakeRequest,
            MessageType::HandshakeResponse,
            MessageType::RequestHeaders,
            MessageType::AgentResponse,
            MessageType::HealthStatus,
            MessageType::Ping,
            MessageType::Pong,
        ];

        for msg_type in types {
            let byte = msg_type as u8;
            let parsed = MessageType::try_from(byte).unwrap();
            assert_eq!(parsed, msg_type);
        }
    }

    #[test]
    fn test_invalid_message_type() {
        let result = MessageType::try_from(0xFF);
        assert!(result.is_err());
    }

    #[test]
    fn test_handshake_serialization() {
        let req = UdsHandshakeRequest {
            supported_versions: vec![2],
            proxy_id: "test-proxy".to_string(),
            proxy_version: "1.0.0".to_string(),
            config: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: UdsHandshakeRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.supported_versions, vec![2]);
        assert_eq!(parsed.proxy_id, "test-proxy");
    }

    #[tokio::test]
    async fn test_write_read_message() {
        use tokio::io::duplex;

        let (mut client, mut server) = duplex(1024);

        // Write from client
        let payload = b"test payload";
        write_message(&mut client, MessageType::Ping, payload)
            .await
            .unwrap();

        // Read from server
        let (msg_type, data) = read_message(&mut server).await.unwrap();
        assert_eq!(msg_type, MessageType::Ping);
        assert_eq!(data, payload);
    }
}
