//! Health reporting for Protocol v2.

use serde::{Deserialize, Serialize};

/// Health status reported by agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub agent_id: String,
    pub state: HealthState,
    pub message: Option<String>,
    pub load: Option<LoadMetrics>,
    pub resources: Option<ResourceMetrics>,
    pub valid_until_ms: Option<u64>,
    pub timestamp_ms: u64,
}

impl HealthStatus {
    pub fn healthy(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            state: HealthState::Healthy,
            message: None,
            load: None,
            resources: None,
            valid_until_ms: None,
            timestamp_ms: now_ms(),
        }
    }

    pub fn degraded(agent_id: impl Into<String>, disabled: Vec<String>, multiplier: f32) -> Self {
        Self {
            agent_id: agent_id.into(),
            state: HealthState::Degraded {
                disabled_features: disabled,
                timeout_multiplier: multiplier,
            },
            message: None,
            load: None,
            resources: None,
            valid_until_ms: None,
            timestamp_ms: now_ms(),
        }
    }

    pub fn unhealthy(
        agent_id: impl Into<String>,
        reason: impl Into<String>,
        recoverable: bool,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            state: HealthState::Unhealthy {
                reason: reason.into(),
                recoverable,
            },
            message: None,
            load: None,
            resources: None,
            valid_until_ms: None,
            timestamp_ms: now_ms(),
        }
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self.state, HealthState::Healthy)
    }
}

/// Health state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum HealthState {
    Healthy,
    Degraded {
        disabled_features: Vec<String>,
        timeout_multiplier: f32,
    },
    Draining {
        eta_ms: Option<u64>,
    },
    Unhealthy {
        reason: String,
        recoverable: bool,
    },
}

/// Load metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoadMetrics {
    pub in_flight: u32,
    pub queue_depth: u32,
    pub avg_latency_ms: f32,
    pub p50_latency_ms: f32,
    pub p95_latency_ms: f32,
    pub p99_latency_ms: f32,
    pub requests_processed: u64,
    pub requests_rejected: u64,
    pub requests_timed_out: u64,
}

/// Resource metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceMetrics {
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub memory_limit: Option<u64>,
    pub active_threads: Option<u32>,
    pub open_fds: Option<u32>,
    pub fd_limit: Option<u32>,
    pub connections: Option<u32>,
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
    fn test_health_status_builders() {
        let healthy = HealthStatus::healthy("test-agent");
        assert!(healthy.is_healthy());

        let unhealthy = HealthStatus::unhealthy("test-agent", "OOM", true);
        assert!(!unhealthy.is_healthy());
    }
}
