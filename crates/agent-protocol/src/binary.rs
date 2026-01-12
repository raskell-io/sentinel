//! Binary protocol for Unix Domain Socket transport.
//!
//! This module provides a binary framing format for efficient communication
//! over UDS, eliminating JSON overhead and base64 encoding for body data.
//!
//! # Wire Format
//!
//! ```text
//! +----------------+---------------+-------------------+
//! | Length (4 BE)  | Type (1 byte) | Payload (N bytes) |
//! +----------------+---------------+-------------------+
//! ```
//!
//! - **Length**: 4-byte big-endian u32, total length of type + payload
//! - **Type**: 1-byte message type discriminator
//! - **Payload**: Variable-length payload (format depends on type)
//!
//! # Performance Benefits
//!
//! - No JSON parsing overhead (~10x faster for small messages)
//! - No base64 encoding for body data (saves 33% bandwidth)
//! - Zero-copy with `bytes::Bytes` where possible

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::collections::HashMap;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{AgentProtocolError, Decision, HeaderOp};

/// Maximum binary message size (10 MB)
pub const MAX_BINARY_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Binary message types
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// Handshake request (proxy -> agent)
    HandshakeRequest = 0x01,
    /// Handshake response (agent -> proxy)
    HandshakeResponse = 0x02,
    /// Request headers event
    RequestHeaders = 0x10,
    /// Request body chunk (raw bytes, no base64)
    RequestBodyChunk = 0x11,
    /// Response headers event
    ResponseHeaders = 0x12,
    /// Response body chunk (raw bytes, no base64)
    ResponseBodyChunk = 0x13,
    /// Request complete event
    RequestComplete = 0x14,
    /// WebSocket frame event
    WebSocketFrame = 0x15,
    /// Agent response
    AgentResponse = 0x20,
    /// Ping
    Ping = 0x30,
    /// Pong
    Pong = 0x31,
    /// Cancel request
    Cancel = 0x40,
    /// Error
    Error = 0xFF,
}

impl TryFrom<u8> for MessageType {
    type Error = AgentProtocolError;

    fn try_from(value: u8) -> Result<Self, AgentProtocolError> {
        match value {
            0x01 => Ok(MessageType::HandshakeRequest),
            0x02 => Ok(MessageType::HandshakeResponse),
            0x10 => Ok(MessageType::RequestHeaders),
            0x11 => Ok(MessageType::RequestBodyChunk),
            0x12 => Ok(MessageType::ResponseHeaders),
            0x13 => Ok(MessageType::ResponseBodyChunk),
            0x14 => Ok(MessageType::RequestComplete),
            0x15 => Ok(MessageType::WebSocketFrame),
            0x20 => Ok(MessageType::AgentResponse),
            0x30 => Ok(MessageType::Ping),
            0x31 => Ok(MessageType::Pong),
            0x40 => Ok(MessageType::Cancel),
            0xFF => Ok(MessageType::Error),
            _ => Err(AgentProtocolError::InvalidMessage(format!(
                "Unknown message type: 0x{:02x}",
                value
            ))),
        }
    }
}

/// Binary frame with header and payload.
#[derive(Debug, Clone)]
pub struct BinaryFrame {
    pub msg_type: MessageType,
    pub payload: Bytes,
}

impl BinaryFrame {
    /// Create a new binary frame.
    pub fn new(msg_type: MessageType, payload: impl Into<Bytes>) -> Self {
        Self {
            msg_type,
            payload: payload.into(),
        }
    }

    /// Encode frame to bytes.
    pub fn encode(&self) -> Bytes {
        let payload_len = self.payload.len();
        let total_len = 1 + payload_len; // type byte + payload

        let mut buf = BytesMut::with_capacity(4 + total_len);
        buf.put_u32(total_len as u32);
        buf.put_u8(self.msg_type as u8);
        buf.put_slice(&self.payload);

        buf.freeze()
    }

    /// Decode frame from reader.
    pub async fn decode<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Self, AgentProtocolError> {
        // Read length (4 bytes)
        let mut len_buf = [0u8; 4];
        reader.read_exact(&mut len_buf).await.map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                AgentProtocolError::ConnectionFailed("Connection closed".to_string())
            } else {
                AgentProtocolError::Io(e)
            }
        })?;
        let total_len = u32::from_be_bytes(len_buf) as usize;

        // Validate length
        if total_len == 0 {
            return Err(AgentProtocolError::InvalidMessage(
                "Empty message".to_string(),
            ));
        }
        if total_len > MAX_BINARY_MESSAGE_SIZE {
            return Err(AgentProtocolError::MessageTooLarge {
                size: total_len,
                max: MAX_BINARY_MESSAGE_SIZE,
            });
        }

        // Read type byte
        let mut type_buf = [0u8; 1];
        reader.read_exact(&mut type_buf).await?;
        let msg_type = MessageType::try_from(type_buf[0])?;

        // Read payload
        let payload_len = total_len - 1;
        let mut payload = BytesMut::with_capacity(payload_len);
        payload.resize(payload_len, 0);
        reader.read_exact(&mut payload).await?;

        Ok(Self {
            msg_type,
            payload: payload.freeze(),
        })
    }

    /// Write frame to writer.
    pub async fn write<W: AsyncWrite + Unpin>(&self, writer: &mut W) -> Result<(), AgentProtocolError> {
        let encoded = self.encode();
        writer.write_all(&encoded).await?;
        writer.flush().await?;
        Ok(())
    }
}

/// Binary request headers event.
///
/// Wire format:
/// - correlation_id: length-prefixed string
/// - method: length-prefixed string
/// - uri: length-prefixed string
/// - headers: count (u16) + [(name_len, name, value_len, value), ...]
/// - client_ip: length-prefixed string
/// - client_port: u16
#[derive(Debug, Clone)]
pub struct BinaryRequestHeaders {
    pub correlation_id: String,
    pub method: String,
    pub uri: String,
    pub headers: HashMap<String, Vec<String>>,
    pub client_ip: String,
    pub client_port: u16,
}

impl BinaryRequestHeaders {
    /// Encode to bytes.
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(256);

        // Correlation ID
        put_string(&mut buf, &self.correlation_id);
        // Method
        put_string(&mut buf, &self.method);
        // URI
        put_string(&mut buf, &self.uri);

        // Headers count
        let header_count: usize = self.headers.values().map(|v| v.len()).sum();
        buf.put_u16(header_count as u16);

        // Headers (flattened: each value gets its own entry)
        for (name, values) in &self.headers {
            for value in values {
                put_string(&mut buf, name);
                put_string(&mut buf, value);
            }
        }

        // Client IP
        put_string(&mut buf, &self.client_ip);
        // Client port
        buf.put_u16(self.client_port);

        buf.freeze()
    }

    /// Decode from bytes.
    pub fn decode(mut data: Bytes) -> Result<Self, AgentProtocolError> {
        let correlation_id = get_string(&mut data)?;
        let method = get_string(&mut data)?;
        let uri = get_string(&mut data)?;

        // Headers
        if data.remaining() < 2 {
            return Err(AgentProtocolError::InvalidMessage(
                "Missing header count".to_string(),
            ));
        }
        let header_count = data.get_u16() as usize;

        let mut headers: HashMap<String, Vec<String>> = HashMap::new();
        for _ in 0..header_count {
            let name = get_string(&mut data)?;
            let value = get_string(&mut data)?;
            headers.entry(name).or_default().push(value);
        }

        let client_ip = get_string(&mut data)?;

        if data.remaining() < 2 {
            return Err(AgentProtocolError::InvalidMessage(
                "Missing client port".to_string(),
            ));
        }
        let client_port = data.get_u16();

        Ok(Self {
            correlation_id,
            method,
            uri,
            headers,
            client_ip,
            client_port,
        })
    }
}

/// Binary body chunk event (zero-copy).
///
/// Wire format:
/// - correlation_id: length-prefixed string
/// - chunk_index: u32
/// - is_last: u8 (0 or 1)
/// - data_len: u32
/// - data: raw bytes (no base64!)
#[derive(Debug, Clone)]
pub struct BinaryBodyChunk {
    pub correlation_id: String,
    pub chunk_index: u32,
    pub is_last: bool,
    pub data: Bytes,
}

impl BinaryBodyChunk {
    /// Encode to bytes.
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(32 + self.data.len());

        put_string(&mut buf, &self.correlation_id);
        buf.put_u32(self.chunk_index);
        buf.put_u8(if self.is_last { 1 } else { 0 });
        buf.put_u32(self.data.len() as u32);
        buf.put_slice(&self.data);

        buf.freeze()
    }

    /// Decode from bytes.
    pub fn decode(mut data: Bytes) -> Result<Self, AgentProtocolError> {
        let correlation_id = get_string(&mut data)?;

        if data.remaining() < 9 {
            return Err(AgentProtocolError::InvalidMessage(
                "Missing body chunk fields".to_string(),
            ));
        }

        let chunk_index = data.get_u32();
        let is_last = data.get_u8() != 0;
        let data_len = data.get_u32() as usize;

        if data.remaining() < data_len {
            return Err(AgentProtocolError::InvalidMessage(
                "Body data truncated".to_string(),
            ));
        }

        let body_data = data.copy_to_bytes(data_len);

        Ok(Self {
            correlation_id,
            chunk_index,
            is_last,
            data: body_data,
        })
    }
}

/// Binary agent response.
///
/// Wire format:
/// - correlation_id: length-prefixed string
/// - decision_type: u8 (0=Allow, 1=Block, 2=Redirect, 3=Challenge)
/// - decision_data: varies by type
/// - request_headers_ops: count (u16) + ops
/// - response_headers_ops: count (u16) + ops
/// - needs_more: u8
#[derive(Debug, Clone)]
pub struct BinaryAgentResponse {
    pub correlation_id: String,
    pub decision: Decision,
    pub request_headers: Vec<HeaderOp>,
    pub response_headers: Vec<HeaderOp>,
    pub needs_more: bool,
}

impl BinaryAgentResponse {
    /// Encode to bytes.
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(128);

        put_string(&mut buf, &self.correlation_id);

        // Decision
        match &self.decision {
            Decision::Allow => {
                buf.put_u8(0);
            }
            Decision::Block { status, body, headers } => {
                buf.put_u8(1);
                buf.put_u16(*status);
                put_optional_string(&mut buf, body.as_deref());
                // Block headers
                let h_count = headers.as_ref().map(|h| h.len()).unwrap_or(0);
                buf.put_u16(h_count as u16);
                if let Some(headers) = headers {
                    for (k, v) in headers {
                        put_string(&mut buf, k);
                        put_string(&mut buf, v);
                    }
                }
            }
            Decision::Redirect { url, status } => {
                buf.put_u8(2);
                put_string(&mut buf, url);
                buf.put_u16(*status);
            }
            Decision::Challenge { challenge_type, params } => {
                buf.put_u8(3);
                put_string(&mut buf, challenge_type);
                buf.put_u16(params.len() as u16);
                for (k, v) in params {
                    put_string(&mut buf, k);
                    put_string(&mut buf, v);
                }
            }
        }

        // Request header ops
        buf.put_u16(self.request_headers.len() as u16);
        for op in &self.request_headers {
            encode_header_op(&mut buf, op);
        }

        // Response header ops
        buf.put_u16(self.response_headers.len() as u16);
        for op in &self.response_headers {
            encode_header_op(&mut buf, op);
        }

        // Needs more
        buf.put_u8(if self.needs_more { 1 } else { 0 });

        buf.freeze()
    }

    /// Decode from bytes.
    pub fn decode(mut data: Bytes) -> Result<Self, AgentProtocolError> {
        let correlation_id = get_string(&mut data)?;

        if data.remaining() < 1 {
            return Err(AgentProtocolError::InvalidMessage(
                "Missing decision type".to_string(),
            ));
        }

        let decision_type = data.get_u8();
        let decision = match decision_type {
            0 => Decision::Allow,
            1 => {
                if data.remaining() < 2 {
                    return Err(AgentProtocolError::InvalidMessage(
                        "Missing block status".to_string(),
                    ));
                }
                let status = data.get_u16();
                let body = get_optional_string(&mut data)?;
                if data.remaining() < 2 {
                    return Err(AgentProtocolError::InvalidMessage(
                        "Missing block headers count".to_string(),
                    ));
                }
                let h_count = data.get_u16() as usize;
                let headers = if h_count > 0 {
                    let mut h = HashMap::new();
                    for _ in 0..h_count {
                        let k = get_string(&mut data)?;
                        let v = get_string(&mut data)?;
                        h.insert(k, v);
                    }
                    Some(h)
                } else {
                    None
                };
                Decision::Block { status, body, headers }
            }
            2 => {
                let url = get_string(&mut data)?;
                if data.remaining() < 2 {
                    return Err(AgentProtocolError::InvalidMessage(
                        "Missing redirect status".to_string(),
                    ));
                }
                let status = data.get_u16();
                Decision::Redirect { url, status }
            }
            3 => {
                let challenge_type = get_string(&mut data)?;
                if data.remaining() < 2 {
                    return Err(AgentProtocolError::InvalidMessage(
                        "Missing challenge params count".to_string(),
                    ));
                }
                let p_count = data.get_u16() as usize;
                let mut params = HashMap::new();
                for _ in 0..p_count {
                    let k = get_string(&mut data)?;
                    let v = get_string(&mut data)?;
                    params.insert(k, v);
                }
                Decision::Challenge { challenge_type, params }
            }
            _ => {
                return Err(AgentProtocolError::InvalidMessage(format!(
                    "Unknown decision type: {}",
                    decision_type
                )));
            }
        };

        // Request header ops
        if data.remaining() < 2 {
            return Err(AgentProtocolError::InvalidMessage(
                "Missing request headers count".to_string(),
            ));
        }
        let req_h_count = data.get_u16() as usize;
        let mut request_headers = Vec::with_capacity(req_h_count);
        for _ in 0..req_h_count {
            request_headers.push(decode_header_op(&mut data)?);
        }

        // Response header ops
        if data.remaining() < 2 {
            return Err(AgentProtocolError::InvalidMessage(
                "Missing response headers count".to_string(),
            ));
        }
        let resp_h_count = data.get_u16() as usize;
        let mut response_headers = Vec::with_capacity(resp_h_count);
        for _ in 0..resp_h_count {
            response_headers.push(decode_header_op(&mut data)?);
        }

        // Needs more
        if data.remaining() < 1 {
            return Err(AgentProtocolError::InvalidMessage(
                "Missing needs_more".to_string(),
            ));
        }
        let needs_more = data.get_u8() != 0;

        Ok(Self {
            correlation_id,
            decision,
            request_headers,
            response_headers,
            needs_more,
        })
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

fn put_string(buf: &mut BytesMut, s: &str) {
    let bytes = s.as_bytes();
    buf.put_u16(bytes.len() as u16);
    buf.put_slice(bytes);
}

fn get_string(data: &mut Bytes) -> Result<String, AgentProtocolError> {
    if data.remaining() < 2 {
        return Err(AgentProtocolError::InvalidMessage(
            "Missing string length".to_string(),
        ));
    }
    let len = data.get_u16() as usize;
    if data.remaining() < len {
        return Err(AgentProtocolError::InvalidMessage(
            "String data truncated".to_string(),
        ));
    }
    let bytes = data.copy_to_bytes(len);
    String::from_utf8(bytes.to_vec())
        .map_err(|e| AgentProtocolError::InvalidMessage(format!("Invalid UTF-8: {}", e)))
}

fn put_optional_string(buf: &mut BytesMut, s: Option<&str>) {
    match s {
        Some(s) => {
            buf.put_u8(1);
            put_string(buf, s);
        }
        None => {
            buf.put_u8(0);
        }
    }
}

fn get_optional_string(data: &mut Bytes) -> Result<Option<String>, AgentProtocolError> {
    if data.remaining() < 1 {
        return Err(AgentProtocolError::InvalidMessage(
            "Missing optional string flag".to_string(),
        ));
    }
    let present = data.get_u8() != 0;
    if present {
        get_string(data).map(Some)
    } else {
        Ok(None)
    }
}

fn encode_header_op(buf: &mut BytesMut, op: &HeaderOp) {
    match op {
        HeaderOp::Set { name, value } => {
            buf.put_u8(0);
            put_string(buf, name);
            put_string(buf, value);
        }
        HeaderOp::Add { name, value } => {
            buf.put_u8(1);
            put_string(buf, name);
            put_string(buf, value);
        }
        HeaderOp::Remove { name } => {
            buf.put_u8(2);
            put_string(buf, name);
        }
    }
}

fn decode_header_op(data: &mut Bytes) -> Result<HeaderOp, AgentProtocolError> {
    if data.remaining() < 1 {
        return Err(AgentProtocolError::InvalidMessage(
            "Missing header op type".to_string(),
        ));
    }
    let op_type = data.get_u8();
    match op_type {
        0 => {
            let name = get_string(data)?;
            let value = get_string(data)?;
            Ok(HeaderOp::Set { name, value })
        }
        1 => {
            let name = get_string(data)?;
            let value = get_string(data)?;
            Ok(HeaderOp::Add { name, value })
        }
        2 => {
            let name = get_string(data)?;
            Ok(HeaderOp::Remove { name })
        }
        _ => Err(AgentProtocolError::InvalidMessage(format!(
            "Unknown header op type: {}",
            op_type
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_roundtrip() {
        for t in [
            MessageType::HandshakeRequest,
            MessageType::HandshakeResponse,
            MessageType::RequestHeaders,
            MessageType::RequestBodyChunk,
            MessageType::AgentResponse,
            MessageType::Ping,
            MessageType::Pong,
            MessageType::Cancel,
            MessageType::Error,
        ] {
            let byte = t as u8;
            let decoded = MessageType::try_from(byte).unwrap();
            assert_eq!(t, decoded);
        }
    }

    #[test]
    fn test_binary_frame_encode_decode() {
        let frame = BinaryFrame::new(MessageType::Ping, Bytes::from_static(b"hello"));
        let encoded = frame.encode();

        // Verify structure
        assert_eq!(encoded.len(), 4 + 1 + 5); // len + type + payload
        assert_eq!(&encoded[0..4], &[0, 0, 0, 6]); // length = 6 (type + payload)
        assert_eq!(encoded[4], MessageType::Ping as u8);
        assert_eq!(&encoded[5..], b"hello");
    }

    #[test]
    fn test_binary_request_headers_roundtrip() {
        let headers = BinaryRequestHeaders {
            correlation_id: "req-123".to_string(),
            method: "POST".to_string(),
            uri: "/api/test".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("content-type".to_string(), vec!["application/json".to_string()]);
                h.insert("x-custom".to_string(), vec!["value1".to_string(), "value2".to_string()]);
                h
            },
            client_ip: "192.168.1.1".to_string(),
            client_port: 12345,
        };

        let encoded = headers.encode();
        let decoded = BinaryRequestHeaders::decode(encoded).unwrap();

        assert_eq!(decoded.correlation_id, "req-123");
        assert_eq!(decoded.method, "POST");
        assert_eq!(decoded.uri, "/api/test");
        assert_eq!(decoded.client_ip, "192.168.1.1");
        assert_eq!(decoded.client_port, 12345);
        assert_eq!(decoded.headers.get("content-type").unwrap(), &vec!["application/json".to_string()]);
    }

    #[test]
    fn test_binary_body_chunk_roundtrip() {
        let chunk = BinaryBodyChunk {
            correlation_id: "req-456".to_string(),
            chunk_index: 2,
            is_last: true,
            data: Bytes::from_static(b"binary data here"),
        };

        let encoded = chunk.encode();
        let decoded = BinaryBodyChunk::decode(encoded).unwrap();

        assert_eq!(decoded.correlation_id, "req-456");
        assert_eq!(decoded.chunk_index, 2);
        assert!(decoded.is_last);
        assert_eq!(&decoded.data[..], b"binary data here");
    }

    #[test]
    fn test_binary_agent_response_allow() {
        let response = BinaryAgentResponse {
            correlation_id: "req-789".to_string(),
            decision: Decision::Allow,
            request_headers: vec![HeaderOp::Set {
                name: "X-Added".to_string(),
                value: "true".to_string(),
            }],
            response_headers: vec![],
            needs_more: false,
        };

        let encoded = response.encode();
        let decoded = BinaryAgentResponse::decode(encoded).unwrap();

        assert_eq!(decoded.correlation_id, "req-789");
        assert!(matches!(decoded.decision, Decision::Allow));
        assert_eq!(decoded.request_headers.len(), 1);
        assert!(!decoded.needs_more);
    }

    #[test]
    fn test_binary_agent_response_block() {
        let response = BinaryAgentResponse {
            correlation_id: "req-block".to_string(),
            decision: Decision::Block {
                status: 403,
                body: Some("Forbidden".to_string()),
                headers: None,
            },
            request_headers: vec![],
            response_headers: vec![],
            needs_more: false,
        };

        let encoded = response.encode();
        let decoded = BinaryAgentResponse::decode(encoded).unwrap();

        assert_eq!(decoded.correlation_id, "req-block");
        match decoded.decision {
            Decision::Block { status, body, headers } => {
                assert_eq!(status, 403);
                assert_eq!(body, Some("Forbidden".to_string()));
                assert!(headers.is_none());
            }
            _ => panic!("Expected Block decision"),
        }
    }
}
