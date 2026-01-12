//! Configuration for the WASM runtime.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for the WASM agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmAgentConfig {
    /// Resource limits for WASM agents
    #[serde(default)]
    pub limits: WasmResourceLimits,

    /// Whether to enable fuel metering (CPU limits)
    #[serde(default = "default_fuel_enabled")]
    pub fuel_enabled: bool,

    /// Whether to enable epoch-based interruption
    #[serde(default = "default_epoch_enabled")]
    pub epoch_enabled: bool,

    /// Epoch tick interval for interruption checks
    #[serde(default = "default_epoch_tick_interval")]
    pub epoch_tick_interval: Duration,

    /// Whether to cache compiled modules
    #[serde(default = "default_cache_enabled")]
    pub cache_enabled: bool,

    /// Directory for compiled module cache
    #[serde(default)]
    pub cache_dir: Option<String>,

    /// Maximum number of instances per agent
    #[serde(default = "default_max_instances")]
    pub max_instances: u32,
}

fn default_fuel_enabled() -> bool { true }
fn default_epoch_enabled() -> bool { true }
fn default_epoch_tick_interval() -> Duration { Duration::from_millis(1) }
fn default_cache_enabled() -> bool { true }
fn default_max_instances() -> u32 { 4 }

impl Default for WasmAgentConfig {
    fn default() -> Self {
        Self {
            limits: WasmResourceLimits::default(),
            fuel_enabled: default_fuel_enabled(),
            epoch_enabled: default_epoch_enabled(),
            epoch_tick_interval: default_epoch_tick_interval(),
            cache_enabled: default_cache_enabled(),
            cache_dir: None,
            max_instances: default_max_instances(),
        }
    }
}

impl WasmAgentConfig {
    /// Create a new configuration with custom limits.
    pub fn with_limits(limits: WasmResourceLimits) -> Self {
        Self {
            limits,
            ..Default::default()
        }
    }

    /// Create a minimal configuration for testing.
    pub fn minimal() -> Self {
        Self {
            limits: WasmResourceLimits::minimal(),
            fuel_enabled: true,
            epoch_enabled: false,
            epoch_tick_interval: Duration::from_millis(10),
            cache_enabled: false,
            cache_dir: None,
            max_instances: 1,
        }
    }

    /// Create a high-performance configuration.
    pub fn high_performance() -> Self {
        Self {
            limits: WasmResourceLimits::high_performance(),
            fuel_enabled: true,
            epoch_enabled: true,
            epoch_tick_interval: Duration::from_micros(100),
            cache_enabled: true,
            cache_dir: None,
            max_instances: 8,
        }
    }
}

/// Resource limits for WASM agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmResourceLimits {
    /// Maximum memory per instance (bytes)
    #[serde(default = "default_max_memory")]
    pub max_memory: usize,

    /// Maximum execution time per call
    #[serde(default = "default_max_execution_time")]
    pub max_execution_time: Duration,

    /// Maximum fuel (instructions) per call
    #[serde(default = "default_max_fuel")]
    pub max_fuel: u64,

    /// Maximum table elements
    #[serde(default = "default_max_table_elements")]
    pub max_table_elements: u32,

    /// Maximum number of tables
    #[serde(default = "default_max_tables")]
    pub max_tables: u32,

    /// Maximum number of memories
    #[serde(default = "default_max_memories")]
    pub max_memories: u32,

    /// Maximum size of a single function (bytes)
    #[serde(default = "default_max_function_size")]
    pub max_function_size: usize,
}

fn default_max_memory() -> usize { 64 * 1024 * 1024 } // 64 MB
fn default_max_execution_time() -> Duration { Duration::from_millis(100) }
fn default_max_fuel() -> u64 { 10_000_000 }
fn default_max_table_elements() -> u32 { 10_000 }
fn default_max_tables() -> u32 { 1 }
fn default_max_memories() -> u32 { 1 }
fn default_max_function_size() -> usize { 1024 * 1024 } // 1 MB

impl Default for WasmResourceLimits {
    fn default() -> Self {
        Self {
            max_memory: default_max_memory(),
            max_execution_time: default_max_execution_time(),
            max_fuel: default_max_fuel(),
            max_table_elements: default_max_table_elements(),
            max_tables: default_max_tables(),
            max_memories: default_max_memories(),
            max_function_size: default_max_function_size(),
        }
    }
}

impl WasmResourceLimits {
    /// Create minimal limits for testing.
    pub fn minimal() -> Self {
        Self {
            max_memory: 16 * 1024 * 1024, // 16 MB
            max_execution_time: Duration::from_millis(50),
            max_fuel: 1_000_000,
            max_table_elements: 1_000,
            max_tables: 1,
            max_memories: 1,
            max_function_size: 256 * 1024,
        }
    }

    /// Create generous limits for high-performance scenarios.
    pub fn high_performance() -> Self {
        Self {
            max_memory: 256 * 1024 * 1024, // 256 MB
            max_execution_time: Duration::from_millis(500),
            max_fuel: 100_000_000,
            max_table_elements: 100_000,
            max_tables: 4,
            max_memories: 1,
            max_function_size: 4 * 1024 * 1024,
        }
    }

    /// Create strict limits for untrusted modules.
    pub fn strict() -> Self {
        Self {
            max_memory: 8 * 1024 * 1024, // 8 MB
            max_execution_time: Duration::from_millis(10),
            max_fuel: 100_000,
            max_table_elements: 100,
            max_tables: 1,
            max_memories: 1,
            max_function_size: 64 * 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WasmAgentConfig::default();
        assert!(config.fuel_enabled);
        assert!(config.epoch_enabled);
        assert!(config.cache_enabled);
        assert_eq!(config.max_instances, 4);
    }

    #[test]
    fn test_default_limits() {
        let limits = WasmResourceLimits::default();
        assert_eq!(limits.max_memory, 64 * 1024 * 1024);
        assert_eq!(limits.max_fuel, 10_000_000);
    }

    #[test]
    fn test_strict_limits() {
        let limits = WasmResourceLimits::strict();
        assert!(limits.max_memory < WasmResourceLimits::default().max_memory);
        assert!(limits.max_fuel < WasmResourceLimits::default().max_fuel);
    }
}
