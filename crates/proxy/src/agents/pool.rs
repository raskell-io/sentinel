//! Agent connection pooling.

use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sentinel_agent_protocol::AgentClient;
use tokio::sync::RwLock;
use tracing::{debug, trace};

/// Agent connection pool for efficient connection reuse.
pub struct AgentConnectionPool {
    /// Pool configuration
    pub(super) max_connections: usize,
    pub(super) min_idle: usize,
    pub(super) max_idle: usize,
    pub(super) idle_timeout: Duration,
    /// Available connections
    pub(super) connections: Arc<RwLock<Vec<AgentConnection>>>,
    /// Active connections count
    pub(super) active_count: AtomicU32,
    /// Total connections created
    pub(super) total_created: AtomicU64,
}

/// Pooled agent connection.
pub(super) struct AgentConnection {
    /// The actual client
    pub client: AgentClient,
    /// Creation time
    pub created_at: Instant,
    /// Last used time
    pub last_used: Instant,
    /// Is healthy
    pub healthy: bool,
}

impl AgentConnectionPool {
    /// Create a new connection pool.
    pub fn new(
        max_connections: usize,
        min_idle: usize,
        max_idle: usize,
        idle_timeout: Duration,
    ) -> Self {
        trace!(
            max_connections = max_connections,
            min_idle = min_idle,
            max_idle = max_idle,
            idle_timeout_secs = idle_timeout.as_secs(),
            "Creating agent connection pool"
        );

        debug!(
            max_connections = max_connections,
            "Agent connection pool initialized"
        );

        Self {
            max_connections,
            min_idle,
            max_idle,
            idle_timeout,
            connections: Arc::new(RwLock::new(Vec::new())),
            active_count: AtomicU32::new(0),
            total_created: AtomicU64::new(0),
        }
    }

    /// Get active connection count.
    pub fn active_count(&self) -> u32 {
        let count = self.active_count.load(std::sync::atomic::Ordering::Relaxed);
        trace!(
            active_connections = count,
            "Retrieved active connection count"
        );
        count
    }

    /// Get total connections created.
    pub fn total_created(&self) -> u64 {
        let total = self
            .total_created
            .load(std::sync::atomic::Ordering::Relaxed);
        trace!(total_created = total, "Retrieved total connections created");
        total
    }
}
