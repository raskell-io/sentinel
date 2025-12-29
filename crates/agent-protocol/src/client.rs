//! Agent client for communicating with external agents.

use serde::Serialize;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::errors::AgentProtocolError;
use crate::protocol::{AgentRequest, AgentResponse, EventType, MAX_MESSAGE_SIZE, PROTOCOL_VERSION};

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
    #[allow(dead_code)]
    Grpc(tonic::transport::Channel),
}

impl AgentClient {
    /// Create a new Unix socket agent client
    pub async fn unix_socket(
        id: impl Into<String>,
        path: impl AsRef<std::path::Path>,
        timeout: Duration,
    ) -> Result<Self, AgentProtocolError> {
        let stream = UnixStream::connect(path.as_ref())
            .await
            .map_err(|e| AgentProtocolError::ConnectionFailed(e.to_string()))?;

        Ok(Self {
            id: id.into(),
            connection: AgentConnection::UnixSocket(stream),
            timeout,
            max_retries: 3,
        })
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
            self.send_raw(&request_bytes).await?;
            self.receive_raw().await
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

    /// Send raw bytes to agent
    async fn send_raw(&mut self, data: &[u8]) -> Result<(), AgentProtocolError> {
        match &mut self.connection {
            AgentConnection::UnixSocket(stream) => {
                // Write message length (4 bytes, big-endian)
                let len_bytes = (data.len() as u32).to_be_bytes();
                stream.write_all(&len_bytes).await?;
                // Write message data
                stream.write_all(data).await?;
                stream.flush().await?;
                Ok(())
            }
            AgentConnection::Grpc(_channel) => {
                // TODO: Implement gRPC transport
                unimplemented!("gRPC transport not yet implemented")
            }
        }
    }

    /// Receive raw bytes from agent
    async fn receive_raw(&mut self) -> Result<Vec<u8>, AgentProtocolError> {
        match &mut self.connection {
            AgentConnection::UnixSocket(stream) => {
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
            AgentConnection::Grpc(_channel) => {
                // TODO: Implement gRPC transport
                unimplemented!("gRPC transport not yet implemented")
            }
        }
    }

    /// Close the agent connection
    pub async fn close(self) -> Result<(), AgentProtocolError> {
        match self.connection {
            AgentConnection::UnixSocket(mut stream) => {
                stream.shutdown().await?;
                Ok(())
            }
            AgentConnection::Grpc(_) => Ok(()),
        }
    }
}
