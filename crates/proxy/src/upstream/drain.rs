//! Backend drain lifecycle tracking.
//!
//! Monitors old upstream pools after a config reload, tracking active
//! connections and emitting structured events when backends are fully drained.
//!
//! Lifecycle states:
//! - `Active`: backend is receiving new connections
//! - `Draining`: backend removed from config, existing connections finishing
//! - `Drained`: all connections completed, safe to terminate backend
//!
//! This replaces the previous blind 60-second sleep with active monitoring.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::UpstreamPool;

/// Backend drain lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendState {
    /// Backend is active and receiving new connections.
    Active,
    /// Backend has been removed from config. No new connections are being
    /// sent, but existing connections are still in flight.
    Draining,
    /// All connections have completed. Safe to shut down the backend.
    Drained,
}

impl std::fmt::Display for BackendState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendState::Active => write!(f, "active"),
            BackendState::Draining => write!(f, "draining"),
            BackendState::Drained => write!(f, "drained"),
        }
    }
}

/// Tracks the drain lifecycle of old upstream pools after a config reload.
///
/// When pools are replaced during a reload, the old pools are handed to
/// the `DrainTracker` which monitors their active request count and
/// emits structured log events when they transition through the lifecycle.
pub struct DrainTracker {
    /// Maximum time to wait for drain before force-shutting down.
    max_drain_time: Duration,
    /// Poll interval for checking connection counts.
    poll_interval: Duration,
}

impl DrainTracker {
    pub fn new(max_drain_time: Duration, poll_interval: Duration) -> Self {
        Self {
            max_drain_time,
            poll_interval,
        }
    }

    /// Monitor old pools and emit drain lifecycle events.
    ///
    /// This runs as a spawned task. It polls each pool's active request count
    /// and emits structured events as they transition from Draining to Drained.
    pub async fn track_pools(&self, pools: HashMap<String, Arc<UpstreamPool>>) {
        if pools.is_empty() {
            return;
        }

        let pool_count = pools.len();
        info!(
            pool_count = pool_count,
            "Starting drain tracking for removed upstream pools"
        );

        // Emit Draining events
        for (name, pool) in &pools {
            let active = pool.active_request_count();
            info!(
                upstream_id = %name,
                active_requests = active,
                state = %BackendState::Draining,
                "Backend entering drain state"
            );
        }

        let start = Instant::now();
        let mut pending: HashMap<String, Arc<UpstreamPool>> = pools;

        while !pending.is_empty() && start.elapsed() < self.max_drain_time {
            tokio::time::sleep(self.poll_interval).await;

            let mut newly_drained = Vec::new();

            for (name, pool) in &pending {
                let active = pool.active_request_count();

                if active == 0 {
                    let drain_duration = start.elapsed();
                    info!(
                        upstream_id = %name,
                        drain_duration_ms = drain_duration.as_millis(),
                        drain_duration_secs = drain_duration.as_secs_f64(),
                        state = %BackendState::Drained,
                        "Backend fully drained, safe to terminate"
                    );
                    newly_drained.push(name.clone());
                } else {
                    debug!(
                        upstream_id = %name,
                        active_requests = active,
                        elapsed_ms = start.elapsed().as_millis(),
                        state = %BackendState::Draining,
                        "Backend still draining"
                    );
                }
            }

            for name in newly_drained {
                if let Some(pool) = pending.remove(&name) {
                    pool.shutdown().await;
                }
            }
        }

        // Force shutdown any remaining pools that didn't drain in time
        for (name, pool) in &pending {
            let active = pool.active_request_count();
            warn!(
                upstream_id = %name,
                active_requests = active,
                max_drain_time_secs = self.max_drain_time.as_secs(),
                state = "drain_timeout",
                "Backend drain timeout exceeded, force shutting down"
            );
            pool.shutdown().await;
        }

        if pool_count > 0 {
            info!(
                pool_count = pool_count,
                total_duration_ms = start.elapsed().as_millis(),
                "Drain tracking complete for all removed pools"
            );
        }
    }
}

impl Default for DrainTracker {
    fn default() -> Self {
        Self {
            max_drain_time: Duration::from_secs(60),
            poll_interval: Duration::from_secs(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_state_display() {
        assert_eq!(BackendState::Active.to_string(), "active");
        assert_eq!(BackendState::Draining.to_string(), "draining");
        assert_eq!(BackendState::Drained.to_string(), "drained");
    }
}
