//! WASM agent runtime management.

use crate::config::WasmAgentConfig;
use crate::error::WasmRuntimeError;
use crate::host::{WasmAgentBuilder, WasmAgentInfo, WasmAgentInstance};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, instrument, warn};
use wasmtime::*;

/// The WASM agent runtime.
///
/// Manages the Wasmtime engine, compiled modules, and agent instances.
pub struct WasmAgentRuntime {
    /// Wasmtime engine
    engine: Engine,
    /// Runtime configuration
    config: WasmAgentConfig,
    /// Compiled modules cache (module_id -> Module)
    modules: RwLock<HashMap<String, Module>>,
    /// Active agent instances (agent_id -> Instance)
    agents: RwLock<HashMap<String, Arc<WasmAgentInstance>>>,
    /// Shutdown flag
    shutdown: std::sync::atomic::AtomicBool,
}

impl WasmAgentRuntime {
    /// Create a new WASM runtime with the given configuration.
    pub fn new(config: WasmAgentConfig) -> Result<Self, WasmRuntimeError> {
        let engine = Self::create_engine(&config)?;

        info!(
            fuel_enabled = config.fuel_enabled,
            epoch_enabled = config.epoch_enabled,
            max_memory = config.limits.max_memory,
            "WASM runtime initialized"
        );

        Ok(Self {
            engine,
            config,
            modules: RwLock::new(HashMap::new()),
            agents: RwLock::new(HashMap::new()),
            shutdown: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Create the Wasmtime engine with configured limits.
    fn create_engine(config: &WasmAgentConfig) -> Result<Engine, WasmRuntimeError> {
        let mut engine_config = Config::new();

        // Enable fuel metering for CPU limits
        if config.fuel_enabled {
            engine_config.consume_fuel(true);
        }

        // Enable epoch-based interruption
        if config.epoch_enabled {
            engine_config.epoch_interruption(true);
        }

        // Configure memory limits
        engine_config.max_wasm_stack(512 * 1024); // 512 KB stack

        // Enable async support
        engine_config.async_support(true);

        // Cranelift optimizations
        engine_config.cranelift_opt_level(OptLevel::Speed);

        // Create engine
        Engine::new(&engine_config)
            .map_err(|e| WasmRuntimeError::EngineCreation(e.to_string()))
    }

    /// Get the Wasmtime engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Get runtime configuration.
    pub fn config(&self) -> &WasmAgentConfig {
        &self.config
    }

    /// Compile a WASM module from bytes.
    #[instrument(skip(self, wasm_bytes))]
    pub fn compile_module(
        &self,
        module_id: &str,
        wasm_bytes: &[u8],
    ) -> Result<(), WasmRuntimeError> {
        debug!(module_id = module_id, size = wasm_bytes.len(), "compiling WASM module");

        // Validate module size
        if wasm_bytes.len() > self.config.limits.max_function_size * 10 {
            return Err(WasmRuntimeError::InvalidModule(format!(
                "module too large: {} bytes",
                wasm_bytes.len()
            )));
        }

        // Compile module
        let module = Module::new(&self.engine, wasm_bytes)
            .map_err(|e| WasmRuntimeError::Compilation(e.to_string()))?;

        // Cache compiled module
        self.modules.write().insert(module_id.to_string(), module);

        info!(module_id = module_id, "WASM module compiled and cached");
        Ok(())
    }

    /// Compile a WASM module from a file.
    #[instrument(skip(self, path))]
    pub fn compile_module_file(
        &self,
        module_id: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), WasmRuntimeError> {
        let path = path.as_ref();
        debug!(module_id = module_id, path = %path.display(), "loading WASM module from file");

        let wasm_bytes = std::fs::read(path)?;
        self.compile_module(module_id, &wasm_bytes)
    }

    /// Load and instantiate an agent from a compiled module.
    #[instrument(skip(self, config_json))]
    pub fn load_agent(
        &self,
        agent_id: &str,
        module_id: &str,
        config_json: &str,
    ) -> Result<Arc<WasmAgentInstance>, WasmRuntimeError> {
        if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(WasmRuntimeError::Shutdown);
        }

        // Get compiled module
        let modules = self.modules.read();
        let module = modules
            .get(module_id)
            .ok_or_else(|| WasmRuntimeError::InvalidModule(format!("module not found: {}", module_id)))?;

        // Check instance limit
        let agent_count = self.agents.read().len();
        if agent_count >= self.config.max_instances as usize {
            return Err(WasmRuntimeError::ResourceLimit(format!(
                "maximum agent instances reached: {}",
                self.config.max_instances
            )));
        }

        // Create agent instance
        let instance = WasmAgentBuilder::new(agent_id)
            .config(config_json)
            .limits(self.config.limits.clone())
            .build(&self.engine, module)?;

        let instance = Arc::new(instance);

        // Register agent
        self.agents.write().insert(agent_id.to_string(), Arc::clone(&instance));

        info!(
            agent_id = agent_id,
            module_id = module_id,
            "WASM agent loaded"
        );

        Ok(instance)
    }

    /// Load an agent directly from WASM bytes (compiles and loads).
    #[instrument(skip(self, wasm_bytes, config_json))]
    pub fn load_agent_from_bytes(
        &self,
        agent_id: &str,
        wasm_bytes: &[u8],
        config_json: &str,
    ) -> Result<Arc<WasmAgentInstance>, WasmRuntimeError> {
        // Use agent_id as module_id for simplicity
        self.compile_module(agent_id, wasm_bytes)?;
        self.load_agent(agent_id, agent_id, config_json)
    }

    /// Get an agent by ID.
    pub fn get_agent(&self, agent_id: &str) -> Option<Arc<WasmAgentInstance>> {
        self.agents.read().get(agent_id).cloned()
    }

    /// List all loaded agents.
    pub fn list_agents(&self) -> Vec<WasmAgentInfo> {
        self.agents
            .read()
            .values()
            .map(|a| a.info().clone())
            .collect()
    }

    /// Unload an agent.
    #[instrument(skip(self))]
    pub fn unload_agent(&self, agent_id: &str) -> bool {
        let removed = self.agents.write().remove(agent_id);
        if let Some(agent) = removed {
            agent.shutdown();
            info!(agent_id = agent_id, "WASM agent unloaded");
            true
        } else {
            false
        }
    }

    /// Unload a compiled module.
    pub fn unload_module(&self, module_id: &str) -> bool {
        self.modules.write().remove(module_id).is_some()
    }

    /// Get runtime statistics.
    pub fn stats(&self) -> WasmRuntimeStats {
        WasmRuntimeStats {
            compiled_modules: self.modules.read().len(),
            active_agents: self.agents.read().len(),
            max_instances: self.config.max_instances as usize,
        }
    }

    /// Shutdown the runtime.
    pub fn shutdown(&self) {
        info!("shutting down WASM runtime");
        self.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);

        // Shutdown all agents
        let agents: Vec<_> = self.agents.write().drain().collect();
        for (agent_id, agent) in agents {
            debug!(agent_id = agent_id, "shutting down agent");
            agent.shutdown();
        }

        // Clear modules
        self.modules.write().clear();

        info!("WASM runtime shutdown complete");
    }
}

impl Drop for WasmAgentRuntime {
    fn drop(&mut self) {
        if !self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            self.shutdown();
        }
    }
}

/// Runtime statistics.
#[derive(Debug, Clone)]
pub struct WasmRuntimeStats {
    /// Number of compiled modules in cache
    pub compiled_modules: usize,
    /// Number of active agent instances
    pub active_agents: usize,
    /// Maximum allowed instances
    pub max_instances: usize,
}

/// Create a minimal WASM module for testing.
///
/// This creates a valid but empty WASM module that can be used for tests.
pub fn create_test_module() -> Vec<u8> {
    // Minimal valid WASM module (empty)
    vec![
        0x00, 0x61, 0x73, 0x6D, // magic: \0asm
        0x01, 0x00, 0x00, 0x00, // version: 1
    ]
}

/// Create a simple WASM module that exports a function.
///
/// This creates a WASM module with a single function that returns 42.
pub fn create_simple_module() -> Vec<u8> {
    // WASM module with:
    // - Type section: () -> i32
    // - Function section: 1 function
    // - Export section: exports "answer" function
    // - Code section: returns 42
    vec![
        // Magic and version
        0x00, 0x61, 0x73, 0x6D, // magic: \0asm
        0x01, 0x00, 0x00, 0x00, // version: 1
        // Type section (1)
        0x01, 0x05,             // section id, size
        0x01,                   // 1 type
        0x60, 0x00, 0x01, 0x7F, // () -> i32
        // Function section (3)
        0x03, 0x02,             // section id, size
        0x01, 0x00,             // 1 function, type index 0
        // Export section (7)
        0x07, 0x0A,             // section id, size
        0x01,                   // 1 export
        0x06, 0x61, 0x6E, 0x73, 0x77, 0x65, 0x72, // "answer"
        0x00, 0x00,             // function, index 0
        // Code section (10)
        0x0A, 0x06,             // section id, size
        0x01,                   // 1 function body
        0x04,                   // body size
        0x00,                   // 0 locals
        0x41, 0x2A,             // i32.const 42
        0x0B,                   // end
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_runtime() {
        let config = WasmAgentConfig::minimal();
        let runtime = WasmAgentRuntime::new(config).unwrap();
        assert_eq!(runtime.stats().compiled_modules, 0);
        assert_eq!(runtime.stats().active_agents, 0);
    }

    #[test]
    fn test_compile_minimal_module() {
        let config = WasmAgentConfig::minimal();
        let runtime = WasmAgentRuntime::new(config).unwrap();

        let wasm = create_test_module();
        runtime.compile_module("test", &wasm).unwrap();

        assert_eq!(runtime.stats().compiled_modules, 1);
    }

    #[test]
    fn test_compile_simple_module() {
        let config = WasmAgentConfig::minimal();
        let runtime = WasmAgentRuntime::new(config).unwrap();

        let wasm = create_simple_module();
        runtime.compile_module("simple", &wasm).unwrap();

        assert_eq!(runtime.stats().compiled_modules, 1);
    }

    #[test]
    fn test_runtime_shutdown() {
        let config = WasmAgentConfig::minimal();
        let runtime = WasmAgentRuntime::new(config).unwrap();

        let wasm = create_test_module();
        runtime.compile_module("test", &wasm).unwrap();

        runtime.shutdown();

        assert_eq!(runtime.stats().compiled_modules, 0);
        assert_eq!(runtime.stats().active_agents, 0);
    }

    #[test]
    fn test_invalid_wasm() {
        let config = WasmAgentConfig::minimal();
        let runtime = WasmAgentRuntime::new(config).unwrap();

        let invalid_wasm = vec![0x00, 0x01, 0x02, 0x03];
        let result = runtime.compile_module("invalid", &invalid_wasm);

        assert!(result.is_err());
    }
}
