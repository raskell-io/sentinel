//! Agent connection pool for Protocol v2.
//!
//! This module provides a production-ready connection pool for managing
//! multiple connections to agents with:
//!
//! - **Connection pooling**: Maintain multiple connections per agent
//! - **Load balancing**: Round-robin, least-connections, or health-based routing
//! - **Health tracking**: Route requests based on agent health
//! - **Automatic reconnection**: Reconnect failed connections
//! - **Graceful shutdown**: Drain connections before closing

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, trace, warn};

use crate::v2::client::{AgentClientV2, CancelReason, ConfigUpdateCallback, MetricsCallback};
use crate::v2::control::ConfigUpdateType;
use crate::v2::observability::{ConfigPusher, ConfigUpdateHandler, MetricsCollector};
use crate::v2::reverse::ReverseConnectionClient;
use crate::v2::uds::AgentClientV2Uds;
use crate::v2::AgentCapabilities;
use crate::{
    AgentProtocolError, AgentResponse, RequestBodyChunkEvent, RequestHeadersEvent,
    ResponseBodyChunkEvent, ResponseHeadersEvent,
};

/// Load balancing strategy for the connection pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadBalanceStrategy {
    /// Round-robin across all healthy connections
    #[default]
    RoundRobin,
    /// Route to connection with fewest in-flight requests
    LeastConnections,
    /// Route based on health score (prefer healthier agents)
    HealthBased,
    /// Random selection
    Random,
}

/// Configuration for the agent connection pool.
#[derive(Debug, Clone)]
pub struct AgentPoolConfig {
    /// Number of connections to maintain per agent
    pub connections_per_agent: usize,
    /// Load balancing strategy
    pub load_balance_strategy: LoadBalanceStrategy,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Time between reconnection attempts
    pub reconnect_interval: Duration,
    /// Maximum reconnection attempts before marking agent unhealthy
    pub max_reconnect_attempts: usize,
    /// Time to wait for in-flight requests during shutdown
    pub drain_timeout: Duration,
    /// Maximum concurrent requests per connection
    pub max_concurrent_per_connection: usize,
    /// Health check interval
    pub health_check_interval: Duration,
}

impl Default for AgentPoolConfig {
    fn default() -> Self {
        Self {
            connections_per_agent: 4,
            load_balance_strategy: LoadBalanceStrategy::RoundRobin,
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
            reconnect_interval: Duration::from_secs(5),
            max_reconnect_attempts: 3,
            drain_timeout: Duration::from_secs(30),
            max_concurrent_per_connection: 100,
            health_check_interval: Duration::from_secs(10),
        }
    }
}

/// Transport layer for v2 agent connections.
///
/// Supports gRPC, Unix Domain Socket, and reverse connections.
pub enum V2Transport {
    /// gRPC over HTTP/2
    Grpc(AgentClientV2),
    /// Binary protocol over Unix Domain Socket
    Uds(AgentClientV2Uds),
    /// Reverse connection (agent connected to proxy)
    Reverse(ReverseConnectionClient),
}

impl V2Transport {
    /// Check if the transport is connected.
    pub async fn is_connected(&self) -> bool {
        match self {
            V2Transport::Grpc(client) => client.is_connected().await,
            V2Transport::Uds(client) => client.is_connected().await,
            V2Transport::Reverse(client) => client.is_connected().await,
        }
    }

    /// Check if the transport can accept new requests.
    pub async fn can_accept_requests(&self) -> bool {
        match self {
            V2Transport::Grpc(client) => client.can_accept_requests().await,
            V2Transport::Uds(_) => true, // UDS uses channel backpressure
            V2Transport::Reverse(_) => true, // Reverse uses channel backpressure
        }
    }

    /// Get negotiated capabilities.
    pub async fn capabilities(&self) -> Option<AgentCapabilities> {
        match self {
            V2Transport::Grpc(client) => client.capabilities().await,
            V2Transport::Uds(client) => client.capabilities().await,
            V2Transport::Reverse(client) => client.capabilities().await,
        }
    }

    /// Send a request headers event.
    pub async fn send_request_headers(
        &self,
        correlation_id: &str,
        event: &RequestHeadersEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        match self {
            V2Transport::Grpc(client) => client.send_request_headers(correlation_id, event).await,
            V2Transport::Uds(client) => client.send_request_headers(correlation_id, event).await,
            V2Transport::Reverse(client) => client.send_request_headers(correlation_id, event).await,
        }
    }

    /// Send a request body chunk event.
    pub async fn send_request_body_chunk(
        &self,
        correlation_id: &str,
        event: &RequestBodyChunkEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        match self {
            V2Transport::Grpc(client) => client.send_request_body_chunk(correlation_id, event).await,
            V2Transport::Uds(client) => client.send_request_body_chunk(correlation_id, event).await,
            V2Transport::Reverse(client) => client.send_request_body_chunk(correlation_id, event).await,
        }
    }

    /// Send a response headers event.
    pub async fn send_response_headers(
        &self,
        correlation_id: &str,
        event: &ResponseHeadersEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        match self {
            V2Transport::Grpc(client) => client.send_response_headers(correlation_id, event).await,
            V2Transport::Uds(client) => client.send_response_headers(correlation_id, event).await,
            V2Transport::Reverse(client) => client.send_response_headers(correlation_id, event).await,
        }
    }

    /// Send a response body chunk event.
    pub async fn send_response_body_chunk(
        &self,
        correlation_id: &str,
        event: &ResponseBodyChunkEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        match self {
            V2Transport::Grpc(client) => client.send_response_body_chunk(correlation_id, event).await,
            V2Transport::Uds(client) => client.send_response_body_chunk(correlation_id, event).await,
            V2Transport::Reverse(client) => client.send_response_body_chunk(correlation_id, event).await,
        }
    }

    /// Cancel a specific request.
    pub async fn cancel_request(
        &self,
        correlation_id: &str,
        reason: CancelReason,
    ) -> Result<(), AgentProtocolError> {
        match self {
            V2Transport::Grpc(client) => client.cancel_request(correlation_id, reason).await,
            V2Transport::Uds(client) => client.cancel_request(correlation_id, reason).await,
            V2Transport::Reverse(client) => client.cancel_request(correlation_id, reason).await,
        }
    }

    /// Cancel all in-flight requests.
    pub async fn cancel_all(&self, reason: CancelReason) -> Result<usize, AgentProtocolError> {
        match self {
            V2Transport::Grpc(client) => client.cancel_all(reason).await,
            V2Transport::Uds(client) => client.cancel_all(reason).await,
            V2Transport::Reverse(client) => client.cancel_all(reason).await,
        }
    }

    /// Close the transport.
    pub async fn close(&self) -> Result<(), AgentProtocolError> {
        match self {
            V2Transport::Grpc(client) => client.close().await,
            V2Transport::Uds(client) => client.close().await,
            V2Transport::Reverse(client) => client.close().await,
        }
    }

    /// Get agent ID.
    pub fn agent_id(&self) -> &str {
        match self {
            V2Transport::Grpc(client) => client.agent_id(),
            V2Transport::Uds(client) => client.agent_id(),
            V2Transport::Reverse(client) => client.agent_id(),
        }
    }
}

/// A pooled connection to an agent.
struct PooledConnection {
    client: V2Transport,
    created_at: Instant,
    last_used: RwLock<Instant>,
    in_flight: AtomicU64,
    request_count: AtomicU64,
    error_count: AtomicU64,
    consecutive_errors: AtomicU64,
    concurrency_limiter: Semaphore,
}

impl PooledConnection {
    fn new(client: V2Transport, max_concurrent: usize) -> Self {
        Self {
            client,
            created_at: Instant::now(),
            last_used: RwLock::new(Instant::now()),
            in_flight: AtomicU64::new(0),
            request_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            consecutive_errors: AtomicU64::new(0),
            concurrency_limiter: Semaphore::new(max_concurrent),
        }
    }

    fn in_flight(&self) -> u64 {
        self.in_flight.load(Ordering::Relaxed)
    }

    fn error_rate(&self) -> f64 {
        let requests = self.request_count.load(Ordering::Relaxed);
        let errors = self.error_count.load(Ordering::Relaxed);
        if requests == 0 {
            0.0
        } else {
            errors as f64 / requests as f64
        }
    }

    async fn is_healthy(&self) -> bool {
        self.client.is_connected().await
            && self.consecutive_errors.load(Ordering::Relaxed) < 3
            && self.client.can_accept_requests().await
    }
}

/// Statistics for a single agent in the pool.
#[derive(Debug, Clone)]
pub struct AgentPoolStats {
    /// Agent identifier
    pub agent_id: String,
    /// Number of active connections
    pub active_connections: usize,
    /// Number of healthy connections
    pub healthy_connections: usize,
    /// Total in-flight requests across all connections
    pub total_in_flight: u64,
    /// Total requests processed
    pub total_requests: u64,
    /// Total errors
    pub total_errors: u64,
    /// Average error rate
    pub error_rate: f64,
    /// Whether the agent is considered healthy
    pub is_healthy: bool,
}

/// An agent entry in the pool.
struct AgentEntry {
    agent_id: String,
    endpoint: String,
    connections: RwLock<Vec<Arc<PooledConnection>>>,
    capabilities: RwLock<Option<AgentCapabilities>>,
    round_robin_index: AtomicUsize,
    reconnect_attempts: AtomicUsize,
    last_reconnect_attempt: RwLock<Option<Instant>>,
    healthy: RwLock<bool>,
}

impl AgentEntry {
    fn new(agent_id: String, endpoint: String) -> Self {
        Self {
            agent_id,
            endpoint,
            connections: RwLock::new(Vec::new()),
            capabilities: RwLock::new(None),
            round_robin_index: AtomicUsize::new(0),
            reconnect_attempts: AtomicUsize::new(0),
            last_reconnect_attempt: RwLock::new(None),
            healthy: RwLock::new(true),
        }
    }
}

/// Agent connection pool.
///
/// Manages multiple connections to multiple agents with load balancing,
/// health tracking, automatic reconnection, and metrics collection.
pub struct AgentPool {
    config: AgentPoolConfig,
    agents: RwLock<HashMap<String, Arc<AgentEntry>>>,
    total_requests: AtomicU64,
    total_errors: AtomicU64,
    /// Shared metrics collector for all agents
    metrics_collector: Arc<MetricsCollector>,
    /// Callback used to record metrics from clients
    metrics_callback: MetricsCallback,
    /// Config pusher for distributing config updates to agents
    config_pusher: Arc<ConfigPusher>,
    /// Handler for config update requests from agents
    config_update_handler: Arc<ConfigUpdateHandler>,
    /// Callback used to handle config updates from clients
    config_update_callback: ConfigUpdateCallback,
}

impl AgentPool {
    /// Create a new agent pool with default configuration.
    pub fn new() -> Self {
        Self::with_config(AgentPoolConfig::default())
    }

    /// Create a new agent pool with custom configuration.
    pub fn with_config(config: AgentPoolConfig) -> Self {
        let metrics_collector = Arc::new(MetricsCollector::new());
        let collector_clone = Arc::clone(&metrics_collector);

        // Create a callback that records metrics to the collector
        let metrics_callback: MetricsCallback = Arc::new(move |report| {
            collector_clone.record(&report);
        });

        // Create config pusher and handler
        let config_pusher = Arc::new(ConfigPusher::new());
        let config_update_handler = Arc::new(ConfigUpdateHandler::new());
        let handler_clone = Arc::clone(&config_update_handler);

        // Create a callback that handles config update requests from agents
        let config_update_callback: ConfigUpdateCallback = Arc::new(move |agent_id, request| {
            debug!(
                agent_id = %agent_id,
                request_id = %request.request_id,
                "Processing config update request from agent"
            );
            handler_clone.handle(request)
        });

        Self {
            config,
            agents: RwLock::new(HashMap::new()),
            total_requests: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            metrics_collector,
            metrics_callback,
            config_pusher,
            config_update_handler,
            config_update_callback,
        }
    }

    /// Get the metrics collector for accessing aggregated agent metrics.
    pub fn metrics_collector(&self) -> &MetricsCollector {
        &self.metrics_collector
    }

    /// Get an Arc to the metrics collector.
    ///
    /// This is useful for registering the collector with a MetricsManager.
    pub fn metrics_collector_arc(&self) -> Arc<MetricsCollector> {
        Arc::clone(&self.metrics_collector)
    }

    /// Export all agent metrics in Prometheus format.
    pub fn export_prometheus(&self) -> String {
        self.metrics_collector.export_prometheus()
    }

    /// Get the config pusher for pushing configuration updates to agents.
    pub fn config_pusher(&self) -> &ConfigPusher {
        &self.config_pusher
    }

    /// Get the config update handler for processing agent config requests.
    pub fn config_update_handler(&self) -> &ConfigUpdateHandler {
        &self.config_update_handler
    }

    /// Push a configuration update to a specific agent.
    ///
    /// Returns the push ID if the agent supports config push, None otherwise.
    pub fn push_config_to_agent(&self, agent_id: &str, update_type: ConfigUpdateType) -> Option<String> {
        self.config_pusher.push_to_agent(agent_id, update_type)
    }

    /// Push a configuration update to all agents that support config push.
    ///
    /// Returns the push IDs for each agent that received the update.
    pub fn push_config_to_all(&self, update_type: ConfigUpdateType) -> Vec<String> {
        self.config_pusher.push_to_all(update_type)
    }

    /// Acknowledge a config push by its push ID.
    pub fn acknowledge_config_push(&self, push_id: &str, accepted: bool, error: Option<String>) {
        self.config_pusher.acknowledge(push_id, accepted, error);
    }

    /// Add an agent to the pool.
    ///
    /// This creates the configured number of connections to the agent.
    pub async fn add_agent(
        &self,
        agent_id: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Result<(), AgentProtocolError> {
        let agent_id = agent_id.into();
        let endpoint = endpoint.into();

        info!(agent_id = %agent_id, endpoint = %endpoint, "Adding agent to pool");

        let entry = Arc::new(AgentEntry::new(agent_id.clone(), endpoint.clone()));

        // Create initial connections
        let mut connections = Vec::with_capacity(self.config.connections_per_agent);
        for i in 0..self.config.connections_per_agent {
            match self.create_connection(&agent_id, &endpoint).await {
                Ok(conn) => {
                    connections.push(Arc::new(conn));
                    debug!(
                        agent_id = %agent_id,
                        connection = i,
                        "Created connection"
                    );
                }
                Err(e) => {
                    warn!(
                        agent_id = %agent_id,
                        connection = i,
                        error = %e,
                        "Failed to create connection"
                    );
                    // Continue - we'll try to reconnect later
                }
            }
        }

        if connections.is_empty() {
            return Err(AgentProtocolError::ConnectionFailed(format!(
                "Failed to create any connections to agent {}",
                agent_id
            )));
        }

        // Store capabilities from first successful connection and register with ConfigPusher
        if let Some(conn) = connections.first() {
            if let Some(caps) = conn.client.capabilities().await {
                // Register with ConfigPusher based on capabilities
                let supports_config_push = caps.features.config_push;
                let agent_name = caps.name.clone();
                self.config_pusher.register_agent(
                    &agent_id,
                    &agent_name,
                    supports_config_push,
                );
                debug!(
                    agent_id = %agent_id,
                    supports_config_push = supports_config_push,
                    "Registered agent with ConfigPusher"
                );

                *entry.capabilities.write().await = Some(caps);
            }
        }

        *entry.connections.write().await = connections;
        self.agents.write().await.insert(agent_id.clone(), entry);

        info!(
            agent_id = %agent_id,
            connections = self.config.connections_per_agent,
            "Agent added to pool"
        );

        Ok(())
    }

    /// Remove an agent from the pool.
    ///
    /// This gracefully closes all connections to the agent.
    pub async fn remove_agent(&self, agent_id: &str) -> Result<(), AgentProtocolError> {
        info!(agent_id = %agent_id, "Removing agent from pool");

        // Unregister from ConfigPusher
        self.config_pusher.unregister_agent(agent_id);

        let entry = self
            .agents
            .write()
            .await
            .remove(agent_id)
            .ok_or_else(|| AgentProtocolError::InvalidMessage(format!("Agent {} not found", agent_id)))?;

        // Close all connections
        let connections = entry.connections.read().await;
        for conn in connections.iter() {
            let _ = conn.client.close().await;
        }

        info!(agent_id = %agent_id, "Agent removed from pool");
        Ok(())
    }

    /// Add a reverse connection to the pool.
    ///
    /// This is called by the ReverseConnectionListener when an agent connects.
    /// The connection is wrapped in a V2Transport and added to the agent's
    /// connection pool.
    pub async fn add_reverse_connection(
        &self,
        agent_id: &str,
        client: ReverseConnectionClient,
        capabilities: AgentCapabilities,
    ) -> Result<(), AgentProtocolError> {
        info!(
            agent_id = %agent_id,
            connection_id = %client.connection_id(),
            "Adding reverse connection to pool"
        );

        let transport = V2Transport::Reverse(client);
        let conn = Arc::new(PooledConnection::new(
            transport,
            self.config.max_concurrent_per_connection,
        ));

        // Check if agent already exists
        let mut agents = self.agents.write().await;
        if let Some(entry) = agents.get(agent_id) {
            // Add to existing agent's connections
            let mut connections = entry.connections.write().await;

            // Check connection limit
            if connections.len() >= self.config.connections_per_agent {
                warn!(
                    agent_id = %agent_id,
                    current = connections.len(),
                    max = self.config.connections_per_agent,
                    "Reverse connection rejected: at connection limit"
                );
                return Err(AgentProtocolError::ConnectionFailed(format!(
                    "Agent {} already has maximum connections ({})",
                    agent_id, self.config.connections_per_agent
                )));
            }

            connections.push(conn);
            info!(
                agent_id = %agent_id,
                total_connections = connections.len(),
                "Added reverse connection to existing agent"
            );
        } else {
            // Create new agent entry
            let entry = Arc::new(AgentEntry::new(
                agent_id.to_string(),
                format!("reverse://{}", agent_id),
            ));

            // Register with ConfigPusher
            let supports_config_push = capabilities.features.config_push;
            let agent_name = capabilities.name.clone();
            self.config_pusher.register_agent(
                agent_id,
                &agent_name,
                supports_config_push,
            );
            debug!(
                agent_id = %agent_id,
                supports_config_push = supports_config_push,
                "Registered reverse connection agent with ConfigPusher"
            );

            *entry.capabilities.write().await = Some(capabilities);
            *entry.connections.write().await = vec![conn];
            agents.insert(agent_id.to_string(), entry);

            info!(
                agent_id = %agent_id,
                "Created new agent entry for reverse connection"
            );
        }

        Ok(())
    }

    /// Send a request headers event to an agent.
    ///
    /// The pool selects the best connection based on the load balancing strategy.
    pub async fn send_request_headers(
        &self,
        agent_id: &str,
        correlation_id: &str,
        event: &RequestHeadersEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        let conn = self.select_connection(agent_id).await?;

        // Acquire concurrency permit
        let _permit = conn
            .concurrency_limiter
            .acquire()
            .await
            .map_err(|_| AgentProtocolError::ConnectionFailed("Concurrency limit reached".to_string()))?;

        conn.in_flight.fetch_add(1, Ordering::Relaxed);
        *conn.last_used.write().await = Instant::now();

        let result = conn.client.send_request_headers(correlation_id, event).await;

        conn.in_flight.fetch_sub(1, Ordering::Relaxed);
        conn.request_count.fetch_add(1, Ordering::Relaxed);

        match &result {
            Ok(_) => {
                conn.consecutive_errors.store(0, Ordering::Relaxed);
            }
            Err(_) => {
                conn.error_count.fetch_add(1, Ordering::Relaxed);
                conn.consecutive_errors.fetch_add(1, Ordering::Relaxed);
                self.total_errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        result
    }

    /// Send a request body chunk to an agent.
    ///
    /// The pool uses correlation_id to route the chunk to the same connection
    /// that received the request headers (for connection affinity).
    pub async fn send_request_body_chunk(
        &self,
        agent_id: &str,
        correlation_id: &str,
        event: &RequestBodyChunkEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        let conn = self.select_connection(agent_id).await?;

        let _permit = conn
            .concurrency_limiter
            .acquire()
            .await
            .map_err(|_| AgentProtocolError::ConnectionFailed("Concurrency limit reached".to_string()))?;

        conn.in_flight.fetch_add(1, Ordering::Relaxed);
        *conn.last_used.write().await = Instant::now();

        let result = conn.client.send_request_body_chunk(correlation_id, event).await;

        conn.in_flight.fetch_sub(1, Ordering::Relaxed);
        conn.request_count.fetch_add(1, Ordering::Relaxed);

        match &result {
            Ok(_) => {
                conn.consecutive_errors.store(0, Ordering::Relaxed);
            }
            Err(_) => {
                conn.error_count.fetch_add(1, Ordering::Relaxed);
                conn.consecutive_errors.fetch_add(1, Ordering::Relaxed);
                self.total_errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        result
    }

    /// Send response headers to an agent.
    ///
    /// Called when upstream response headers are received, allowing the agent
    /// to inspect/modify response headers before they're sent to the client.
    pub async fn send_response_headers(
        &self,
        agent_id: &str,
        correlation_id: &str,
        event: &ResponseHeadersEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        let conn = self.select_connection(agent_id).await?;

        let _permit = conn
            .concurrency_limiter
            .acquire()
            .await
            .map_err(|_| AgentProtocolError::ConnectionFailed("Concurrency limit reached".to_string()))?;

        conn.in_flight.fetch_add(1, Ordering::Relaxed);
        *conn.last_used.write().await = Instant::now();

        let result = conn.client.send_response_headers(correlation_id, event).await;

        conn.in_flight.fetch_sub(1, Ordering::Relaxed);
        conn.request_count.fetch_add(1, Ordering::Relaxed);

        match &result {
            Ok(_) => {
                conn.consecutive_errors.store(0, Ordering::Relaxed);
            }
            Err(_) => {
                conn.error_count.fetch_add(1, Ordering::Relaxed);
                conn.consecutive_errors.fetch_add(1, Ordering::Relaxed);
                self.total_errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        result
    }

    /// Send a response body chunk to an agent.
    ///
    /// For streaming response body inspection, chunks are sent sequentially.
    /// The agent can inspect and optionally modify response body data.
    pub async fn send_response_body_chunk(
        &self,
        agent_id: &str,
        correlation_id: &str,
        event: &ResponseBodyChunkEvent,
    ) -> Result<AgentResponse, AgentProtocolError> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        let conn = self.select_connection(agent_id).await?;

        let _permit = conn
            .concurrency_limiter
            .acquire()
            .await
            .map_err(|_| AgentProtocolError::ConnectionFailed("Concurrency limit reached".to_string()))?;

        conn.in_flight.fetch_add(1, Ordering::Relaxed);
        *conn.last_used.write().await = Instant::now();

        let result = conn.client.send_response_body_chunk(correlation_id, event).await;

        conn.in_flight.fetch_sub(1, Ordering::Relaxed);
        conn.request_count.fetch_add(1, Ordering::Relaxed);

        match &result {
            Ok(_) => {
                conn.consecutive_errors.store(0, Ordering::Relaxed);
            }
            Err(_) => {
                conn.error_count.fetch_add(1, Ordering::Relaxed);
                conn.consecutive_errors.fetch_add(1, Ordering::Relaxed);
                self.total_errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        result
    }

    /// Cancel a request on all connections for an agent.
    pub async fn cancel_request(
        &self,
        agent_id: &str,
        correlation_id: &str,
        reason: CancelReason,
    ) -> Result<(), AgentProtocolError> {
        let agents = self.agents.read().await;
        let entry = agents
            .get(agent_id)
            .ok_or_else(|| AgentProtocolError::InvalidMessage(format!("Agent {} not found", agent_id)))?;

        let connections = entry.connections.read().await;
        for conn in connections.iter() {
            let _ = conn.client.cancel_request(correlation_id, reason).await;
        }

        Ok(())
    }

    /// Get statistics for all agents in the pool.
    pub async fn stats(&self) -> Vec<AgentPoolStats> {
        let agents = self.agents.read().await;
        let mut stats = Vec::with_capacity(agents.len());

        for (agent_id, entry) in agents.iter() {
            let connections = entry.connections.read().await;
            let mut healthy_count = 0;
            let mut total_in_flight = 0;
            let mut total_requests = 0;
            let mut total_errors = 0;

            for conn in connections.iter() {
                if conn.is_healthy().await {
                    healthy_count += 1;
                }
                total_in_flight += conn.in_flight();
                total_requests += conn.request_count.load(Ordering::Relaxed);
                total_errors += conn.error_count.load(Ordering::Relaxed);
            }

            let error_rate = if total_requests == 0 {
                0.0
            } else {
                total_errors as f64 / total_requests as f64
            };

            stats.push(AgentPoolStats {
                agent_id: agent_id.clone(),
                active_connections: connections.len(),
                healthy_connections: healthy_count,
                total_in_flight,
                total_requests,
                total_errors,
                error_rate,
                is_healthy: *entry.healthy.read().await,
            });
        }

        stats
    }

    /// Get statistics for a specific agent.
    pub async fn agent_stats(&self, agent_id: &str) -> Option<AgentPoolStats> {
        self.stats()
            .await
            .into_iter()
            .find(|s| s.agent_id == agent_id)
    }

    /// Get the capabilities of an agent.
    pub async fn agent_capabilities(&self, agent_id: &str) -> Option<AgentCapabilities> {
        let entry = {
            let agents = self.agents.read().await;
            match agents.get(agent_id) {
                Some(e) => e.clone(),
                None => return None,
            }
        };
        let caps = entry.capabilities.read().await.clone();
        caps
    }

    /// Check if an agent is healthy.
    pub async fn is_agent_healthy(&self, agent_id: &str) -> bool {
        let entry = {
            let agents = self.agents.read().await;
            agents.get(agent_id).cloned()
        };
        if let Some(entry) = entry {
            *entry.healthy.read().await
        } else {
            false
        }
    }

    /// Get all agent IDs in the pool.
    pub async fn agent_ids(&self) -> Vec<String> {
        self.agents.read().await.keys().cloned().collect()
    }

    /// Gracefully shut down the pool.
    ///
    /// This drains all connections and waits for in-flight requests to complete.
    pub async fn shutdown(&self) -> Result<(), AgentProtocolError> {
        info!("Shutting down agent pool");

        let agents: Vec<_> = self.agents.write().await.drain().collect();

        for (agent_id, entry) in agents {
            debug!(agent_id = %agent_id, "Draining agent connections");

            let connections = entry.connections.read().await;
            for conn in connections.iter() {
                // Cancel all pending requests
                let _ = conn.client.cancel_all(CancelReason::ProxyShutdown).await;
            }

            // Wait for in-flight requests to complete
            let drain_deadline = Instant::now() + self.config.drain_timeout;
            loop {
                let total_in_flight: u64 = connections.iter().map(|c| c.in_flight()).sum();
                if total_in_flight == 0 {
                    break;
                }
                if Instant::now() > drain_deadline {
                    warn!(
                        agent_id = %agent_id,
                        in_flight = total_in_flight,
                        "Drain timeout, forcing close"
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            // Close all connections
            for conn in connections.iter() {
                let _ = conn.client.close().await;
            }
        }

        info!("Agent pool shutdown complete");
        Ok(())
    }

    /// Run background maintenance tasks.
    ///
    /// This should be spawned as a background task. It handles:
    /// - Health checking
    /// - Reconnection of failed connections
    /// - Cleanup of idle connections
    pub async fn run_maintenance(&self) {
        let mut interval = tokio::time::interval(self.config.health_check_interval);

        loop {
            interval.tick().await;

            let agents = self.agents.read().await;
            for (agent_id, entry) in agents.iter() {
                // Check connection health
                let connections = entry.connections.read().await;
                let mut healthy_count = 0;

                for conn in connections.iter() {
                    if conn.is_healthy().await {
                        healthy_count += 1;
                    }
                }

                // Update agent health status
                let was_healthy = *entry.healthy.read().await;
                let is_healthy = healthy_count > 0;
                *entry.healthy.write().await = is_healthy;

                if was_healthy && !is_healthy {
                    warn!(agent_id = %agent_id, "Agent marked unhealthy");
                } else if !was_healthy && is_healthy {
                    info!(agent_id = %agent_id, "Agent recovered");
                }

                // Try to reconnect failed connections
                if healthy_count < self.config.connections_per_agent {
                    let should_reconnect = {
                        let last = entry.last_reconnect_attempt.read().await;
                        last.map_or(true, |t| t.elapsed() > self.config.reconnect_interval)
                    };

                    if should_reconnect {
                        drop(connections); // Release read lock
                        if let Err(e) = self.reconnect_agent(agent_id, entry).await {
                            trace!(agent_id = %agent_id, error = %e, "Reconnect failed");
                        }
                    }
                }
            }
        }
    }

    // =========================================================================
    // Internal Methods
    // =========================================================================

    async fn create_connection(
        &self,
        agent_id: &str,
        endpoint: &str,
    ) -> Result<PooledConnection, AgentProtocolError> {
        // Detect transport type from endpoint
        let transport = if is_uds_endpoint(endpoint) {
            // Unix Domain Socket transport
            let socket_path = endpoint
                .strip_prefix("unix:")
                .unwrap_or(endpoint);

            let mut client =
                AgentClientV2Uds::new(agent_id, socket_path, self.config.request_timeout).await?;

            // Set callbacks before connecting
            client.set_metrics_callback(Arc::clone(&self.metrics_callback));
            client.set_config_update_callback(Arc::clone(&self.config_update_callback));

            client.connect().await?;
            V2Transport::Uds(client)
        } else {
            // gRPC transport (default)
            let mut client =
                AgentClientV2::new(agent_id, endpoint, self.config.request_timeout).await?;

            // Set callbacks before connecting
            client.set_metrics_callback(Arc::clone(&self.metrics_callback));
            client.set_config_update_callback(Arc::clone(&self.config_update_callback));

            client.connect().await?;
            V2Transport::Grpc(client)
        };

        Ok(PooledConnection::new(
            transport,
            self.config.max_concurrent_per_connection,
        ))
    }

    async fn select_connection(
        &self,
        agent_id: &str,
    ) -> Result<Arc<PooledConnection>, AgentProtocolError> {
        let agents = self.agents.read().await;
        let entry = agents
            .get(agent_id)
            .ok_or_else(|| AgentProtocolError::InvalidMessage(format!("Agent {} not found", agent_id)))?;

        let connections = entry.connections.read().await;
        if connections.is_empty() {
            return Err(AgentProtocolError::ConnectionFailed(format!(
                "No connections available for agent {}",
                agent_id
            )));
        }

        // Filter to healthy connections
        let mut healthy: Vec<_> = Vec::new();
        for conn in connections.iter() {
            if conn.is_healthy().await {
                healthy.push(conn.clone());
            }
        }

        if healthy.is_empty() {
            return Err(AgentProtocolError::ConnectionFailed(format!(
                "No healthy connections for agent {}",
                agent_id
            )));
        }

        let selected = match self.config.load_balance_strategy {
            LoadBalanceStrategy::RoundRobin => {
                let idx = entry.round_robin_index.fetch_add(1, Ordering::Relaxed);
                healthy[idx % healthy.len()].clone()
            }
            LoadBalanceStrategy::LeastConnections => {
                healthy
                    .iter()
                    .min_by_key(|c| c.in_flight())
                    .cloned()
                    .unwrap()
            }
            LoadBalanceStrategy::HealthBased => {
                // Prefer connections with lower error rates
                healthy
                    .iter()
                    .min_by(|a, b| {
                        a.error_rate()
                            .partial_cmp(&b.error_rate())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .cloned()
                    .unwrap()
            }
            LoadBalanceStrategy::Random => {
                use std::collections::hash_map::RandomState;
                use std::hash::{BuildHasher, Hasher};
                let idx = RandomState::new().build_hasher().finish() as usize % healthy.len();
                healthy[idx].clone()
            }
        };

        Ok(selected)
    }

    async fn reconnect_agent(
        &self,
        agent_id: &str,
        entry: &AgentEntry,
    ) -> Result<(), AgentProtocolError> {
        *entry.last_reconnect_attempt.write().await = Some(Instant::now());
        let attempts = entry.reconnect_attempts.fetch_add(1, Ordering::Relaxed);

        if attempts >= self.config.max_reconnect_attempts {
            debug!(
                agent_id = %agent_id,
                attempts = attempts,
                "Max reconnect attempts reached"
            );
            return Ok(());
        }

        debug!(agent_id = %agent_id, attempt = attempts + 1, "Attempting reconnect");

        match self.create_connection(agent_id, &entry.endpoint).await {
            Ok(conn) => {
                let mut connections = entry.connections.write().await;
                connections.push(Arc::new(conn));
                entry.reconnect_attempts.store(0, Ordering::Relaxed);
                info!(agent_id = %agent_id, "Reconnected successfully");
                Ok(())
            }
            Err(e) => {
                debug!(agent_id = %agent_id, error = %e, "Reconnect failed");
                Err(e)
            }
        }
    }
}

impl Default for AgentPool {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AgentPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPool")
            .field("config", &self.config)
            .field("total_requests", &self.total_requests.load(Ordering::Relaxed))
            .field("total_errors", &self.total_errors.load(Ordering::Relaxed))
            .finish()
    }
}

/// Check if an endpoint is a Unix Domain Socket path.
///
/// Returns true for endpoints that:
/// - Start with "unix:" prefix
/// - Are absolute paths (start with "/")
/// - Have ".sock" extension
fn is_uds_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with("unix:")
        || endpoint.starts_with('/')
        || endpoint.ends_with(".sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_default() {
        let config = AgentPoolConfig::default();
        assert_eq!(config.connections_per_agent, 4);
        assert_eq!(config.load_balance_strategy, LoadBalanceStrategy::RoundRobin);
    }

    #[test]
    fn test_load_balance_strategy() {
        assert_eq!(LoadBalanceStrategy::default(), LoadBalanceStrategy::RoundRobin);
    }

    #[test]
    fn test_pool_creation() {
        let pool = AgentPool::new();
        assert_eq!(pool.total_requests.load(Ordering::Relaxed), 0);
        assert_eq!(pool.total_errors.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_pool_with_config() {
        let config = AgentPoolConfig {
            connections_per_agent: 8,
            load_balance_strategy: LoadBalanceStrategy::LeastConnections,
            ..Default::default()
        };
        let pool = AgentPool::with_config(config.clone());
        assert_eq!(pool.config.connections_per_agent, 8);
    }

    #[tokio::test]
    async fn test_agent_ids_empty() {
        let pool = AgentPool::new();
        assert!(pool.agent_ids().await.is_empty());
    }

    #[tokio::test]
    async fn test_is_agent_healthy_not_found() {
        let pool = AgentPool::new();
        assert!(!pool.is_agent_healthy("nonexistent").await);
    }

    #[tokio::test]
    async fn test_stats_empty() {
        let pool = AgentPool::new();
        assert!(pool.stats().await.is_empty());
    }

    #[test]
    fn test_is_uds_endpoint() {
        // Unix prefix
        assert!(is_uds_endpoint("unix:/var/run/agent.sock"));
        assert!(is_uds_endpoint("unix:agent.sock"));

        // Absolute path
        assert!(is_uds_endpoint("/var/run/agent.sock"));
        assert!(is_uds_endpoint("/tmp/test.sock"));

        // .sock extension
        assert!(is_uds_endpoint("agent.sock"));

        // Not UDS
        assert!(!is_uds_endpoint("http://localhost:8080"));
        assert!(!is_uds_endpoint("localhost:50051"));
        assert!(!is_uds_endpoint("127.0.0.1:8080"));
    }
}
