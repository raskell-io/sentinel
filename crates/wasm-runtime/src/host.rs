//! WASM agent host bindings and instance management.

use crate::config::WasmResourceLimits;
use crate::error::WasmRuntimeError;
use parking_lot::Mutex;
use sentinel_agent_protocol::{AgentResponse, RequestMetadata};
use std::collections::HashMap;
use tracing::{debug, instrument};
use wasmtime::*;

/// Information about a loaded WASM agent.
#[derive(Debug, Clone)]
pub struct WasmAgentInfo {
    /// Agent identifier
    pub agent_id: String,
    /// Human-readable name
    pub name: String,
    /// Version string
    pub version: String,
    /// Supported event types
    pub supported_events: Vec<String>,
    /// Maximum body size the agent can inspect
    pub max_body_size: u64,
    /// Whether agent supports streaming
    pub supports_streaming: bool,
}

/// A loaded WASM agent instance.
pub struct WasmAgentInstance {
    /// Agent information
    info: WasmAgentInfo,
    /// Wasmtime store with state
    store: Mutex<Store<AgentState>>,
    /// Compiled instance
    instance: Instance,
    /// Resource limits
    limits: WasmResourceLimits,
}

/// State stored in the Wasmtime store.
struct AgentState {
    /// Fuel consumed in current call
    fuel_consumed: u64,
    /// Agent configuration (JSON)
    config: String,
    /// Whether agent is configured
    configured: bool,
}

impl WasmAgentInstance {
    /// Create a new WASM agent instance from compiled module.
    pub(crate) fn new(
        engine: &Engine,
        module: &Module,
        limits: WasmResourceLimits,
        config_json: &str,
    ) -> Result<Self, WasmRuntimeError> {
        // Create store with state
        let state = AgentState {
            fuel_consumed: 0,
            config: config_json.to_string(),
            configured: false,
        };
        let mut store = Store::new(engine, state);

        // Configure fuel metering
        store.set_fuel(limits.max_fuel)?;

        // Create linker and add imports
        let mut linker = Linker::new(engine);
        Self::add_host_functions(&mut linker)?;

        // Instantiate module
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| WasmRuntimeError::Instantiation(e.to_string()))?;

        // Get agent info
        let info = Self::call_get_info(&mut store, &instance)?;

        // Configure agent
        Self::call_configure(&mut store, &instance, config_json)?;
        store.data_mut().configured = true;

        Ok(Self {
            info,
            store: Mutex::new(store),
            instance,
            limits,
        })
    }

    /// Add host functions to the linker.
    fn add_host_functions(linker: &mut Linker<AgentState>) -> Result<(), WasmRuntimeError> {
        // Add logging function
        linker
            .func_wrap("env", "log", |_caller: Caller<'_, AgentState>, level: i32, ptr: i32, len: i32| {
                // In a real implementation, we'd read the string from WASM memory
                debug!(level = level, ptr = ptr, len = len, "WASM agent log");
            })
            .map_err(|e| WasmRuntimeError::Internal(format!("failed to add log function: {}", e)))?;

        // Add timestamp function
        linker
            .func_wrap("env", "now_ms", |_caller: Caller<'_, AgentState>| -> i64 {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)
            })
            .map_err(|e| WasmRuntimeError::Internal(format!("failed to add now_ms function: {}", e)))?;

        Ok(())
    }

    /// Call get_info to retrieve agent information.
    fn call_get_info(
        _store: &mut Store<AgentState>,
        _instance: &Instance,
    ) -> Result<WasmAgentInfo, WasmRuntimeError> {
        // For now, return a default info since we don't have full component model bindings
        // In production, this would call the actual WASM function
        Ok(WasmAgentInfo {
            agent_id: "wasm-agent".to_string(),
            name: "WASM Agent".to_string(),
            version: "1.0.0".to_string(),
            supported_events: vec!["request_headers".to_string()],
            max_body_size: 1024 * 1024, // 1MB
            supports_streaming: false,
        })
    }

    /// Call configure to initialize the agent.
    fn call_configure(
        _store: &mut Store<AgentState>,
        _instance: &Instance,
        config_json: &str,
    ) -> Result<(), WasmRuntimeError> {
        // For now, just store the config
        // In production, this would call the actual WASM function
        debug!(config_len = config_json.len(), "configuring WASM agent");
        Ok(())
    }

    /// Get agent information.
    pub fn info(&self) -> &WasmAgentInfo {
        &self.info
    }

    /// Get agent ID.
    pub fn agent_id(&self) -> &str {
        &self.info.agent_id
    }

    /// Process request headers.
    #[instrument(skip(self, headers), fields(agent_id = %self.info.agent_id))]
    pub fn on_request_headers(
        &self,
        metadata: &RequestMetadata,
        method: &str,
        uri: &str,
        headers: &HashMap<String, Vec<String>>,
    ) -> Result<AgentResponse, WasmRuntimeError> {
        let mut store = self.store.lock();

        // Reset fuel for this call
        store.set_fuel(self.limits.max_fuel)?;

        // In production, this would:
        // 1. Serialize metadata, method, uri, headers to WASM memory
        // 2. Call the on_request_headers export
        // 3. Deserialize the response

        // For now, return a default allow response
        debug!(
            method = method,
            uri = uri,
            header_count = headers.len(),
            "processing request headers in WASM agent"
        );

        // Check fuel consumed
        let remaining = store.get_fuel().unwrap_or(0);
        let consumed = self.limits.max_fuel.saturating_sub(remaining);
        store.data_mut().fuel_consumed = consumed;

        Ok(AgentResponse::default_allow())
    }

    /// Process request body chunk.
    #[instrument(skip(self, data), fields(agent_id = %self.info.agent_id))]
    pub fn on_request_body(
        &self,
        correlation_id: &str,
        data: &[u8],
        chunk_index: u32,
        is_last: bool,
    ) -> Result<AgentResponse, WasmRuntimeError> {
        let mut store = self.store.lock();
        store.set_fuel(self.limits.max_fuel)?;

        debug!(
            correlation_id = correlation_id,
            chunk_index = chunk_index,
            data_len = data.len(),
            is_last = is_last,
            "processing request body in WASM agent"
        );

        // Default: allow with needs_more if not last chunk
        let mut response = AgentResponse::default_allow();
        if !is_last {
            response = response.set_needs_more(true);
        }
        Ok(response)
    }

    /// Process response headers.
    #[instrument(skip(self, headers), fields(agent_id = %self.info.agent_id))]
    pub fn on_response_headers(
        &self,
        correlation_id: &str,
        status: u16,
        headers: &HashMap<String, Vec<String>>,
    ) -> Result<AgentResponse, WasmRuntimeError> {
        let mut store = self.store.lock();
        store.set_fuel(self.limits.max_fuel)?;

        debug!(
            correlation_id = correlation_id,
            status = status,
            header_count = headers.len(),
            "processing response headers in WASM agent"
        );

        Ok(AgentResponse::default_allow())
    }

    /// Process response body chunk.
    #[instrument(skip(self, data), fields(agent_id = %self.info.agent_id))]
    pub fn on_response_body(
        &self,
        correlation_id: &str,
        data: &[u8],
        chunk_index: u32,
        is_last: bool,
    ) -> Result<AgentResponse, WasmRuntimeError> {
        let mut store = self.store.lock();
        store.set_fuel(self.limits.max_fuel)?;

        debug!(
            correlation_id = correlation_id,
            chunk_index = chunk_index,
            data_len = data.len(),
            is_last = is_last,
            "processing response body in WASM agent"
        );

        let mut response = AgentResponse::default_allow();
        if !is_last {
            response = response.set_needs_more(true);
        }
        Ok(response)
    }

    /// Health check.
    pub fn health_check(&self) -> Result<String, WasmRuntimeError> {
        // For now, always healthy
        Ok("healthy".to_string())
    }

    /// Graceful shutdown.
    pub fn shutdown(&self) {
        debug!(agent_id = %self.info.agent_id, "shutting down WASM agent");
    }

    /// Get fuel consumed in last call.
    pub fn last_fuel_consumed(&self) -> u64 {
        self.store.lock().data().fuel_consumed
    }
}

/// Builder for creating WASM agent instances.
pub struct WasmAgentBuilder {
    agent_id: String,
    config_json: String,
    limits: WasmResourceLimits,
}

impl WasmAgentBuilder {
    /// Create a new builder.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            config_json: "{}".to_string(),
            limits: WasmResourceLimits::default(),
        }
    }

    /// Set agent configuration (JSON).
    pub fn config(mut self, config_json: impl Into<String>) -> Self {
        self.config_json = config_json.into();
        self
    }

    /// Set resource limits.
    pub fn limits(mut self, limits: WasmResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Build the agent instance.
    pub fn build(
        self,
        engine: &Engine,
        module: &Module,
    ) -> Result<WasmAgentInstance, WasmRuntimeError> {
        WasmAgentInstance::new(engine, module, self.limits, &self.config_json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_info() {
        let info = WasmAgentInfo {
            agent_id: "test".to_string(),
            name: "Test Agent".to_string(),
            version: "1.0.0".to_string(),
            supported_events: vec!["request_headers".to_string()],
            max_body_size: 1024,
            supports_streaming: false,
        };

        assert_eq!(info.agent_id, "test");
        assert!(!info.supports_streaming);
    }

    #[test]
    fn test_builder() {
        let builder = WasmAgentBuilder::new("my-agent")
            .config(r#"{"key": "value"}"#)
            .limits(WasmResourceLimits::strict());

        assert_eq!(builder.agent_id, "my-agent");
    }
}
