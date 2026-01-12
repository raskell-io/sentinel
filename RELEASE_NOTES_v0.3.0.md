# Sentinel v0.3.0 Release Notes

**Release Date:** January 12, 2026

This is a major release introducing **Agent Protocol 2.0**, a complete redesign of the agent communication layer with support for multiple transports, connection pooling, and reverse connections.

---

## Highlights

- **Agent Protocol 2.0** - New bidirectional streaming protocol with gRPC, UDS, and reverse connection support
- **Connection Pooling** - Maintain multiple connections per agent with intelligent load balancing
- **Reverse Connections** - Agents can now connect to the proxy, enabling NAT traversal and dynamic scaling
- **Unified Observability** - Integrated metrics collection with Prometheus export
- **WASM Runtime Foundation** - Initial support for WebAssembly-based agents

---

## Agent Protocol 2.0

### New Transport Options

**gRPC over HTTP/2** (`AgentClientV2`)
- Bidirectional streaming for efficient request/response handling
- TLS support with certificate validation
- Automatic reconnection and health tracking
- Flow control and backpressure management

**Binary over Unix Domain Socket** (`AgentClientV2Uds`)
- Low-latency local communication
- Simple wire format: 4-byte length + 1-byte type + JSON payload
- Ideal for co-located agents on the same host
- 16MB maximum message size

**Reverse Connections** (`ReverseConnectionListener`)
- Agents connect to proxy instead of proxy connecting to agents
- Enables agents behind NAT/firewalls
- Supports dynamic agent scaling
- Registration handshake with capability negotiation

### Connection Pooling

The new `AgentPool` provides production-ready connection management:

```rust
use sentinel_agent_protocol::v2::{AgentPool, AgentPoolConfig, LoadBalanceStrategy};

let config = AgentPoolConfig {
    connections_per_agent: 4,
    load_balance_strategy: LoadBalanceStrategy::LeastConnections,
    request_timeout: Duration::from_secs(30),
    ..Default::default()
};

let pool = AgentPool::with_config(config);

// Add agents with automatic transport detection
pool.add_agent("waf", "localhost:50051").await?;           // gRPC
pool.add_agent("auth", "/var/run/sentinel/auth.sock").await?; // UDS
```

**Load Balancing Strategies:**
- `RoundRobin` - Distribute requests evenly across connections
- `LeastConnections` - Route to connection with fewest in-flight requests
- `HealthBased` - Prefer healthier connections based on error rates
- `Random` - Random selection for simple distribution

### Transport Abstraction

The `V2Transport` enum provides a unified interface across all transport types:

```rust
pub enum V2Transport {
    Grpc(AgentClientV2),      // gRPC over HTTP/2
    Uds(AgentClientV2Uds),    // Binary over Unix socket
    Reverse(ReverseConnectionClient), // Inbound agent connection
}
```

All transports support the same operations:
- `send_request_headers()` / `send_request_body_chunk()`
- `send_response_headers()` / `send_response_body_chunk()`
- `cancel_request()` / `cancel_all()`
- Health checking and capability queries

---

## Observability Improvements

### MetricsCollector

Unified metrics collection for all v2 agents:

```rust
let pool = AgentPool::new();
let metrics = pool.metrics_collector();

// Export in Prometheus format
let prometheus_output = metrics.export_prometheus();
```

**Collected Metrics:**
- `agent_requests_total` - Total requests by agent and decision
- `agent_request_duration_seconds` - Request latency histograms
- `agent_connections_active` - Current connection count
- `agent_errors_total` - Error counts by type

### ConfigPusher

Push configuration updates to agents that support it:

```rust
let pool = AgentPool::new();

// Push to specific agent
pool.push_config_to_agent("waf", ConfigUpdateType::RuleUpdate)?;

// Push to all capable agents
pool.push_config_to_all(ConfigUpdateType::Full)?;
```

### Proxy Metrics Endpoint

New `/metrics` endpoint integration:

```rust
use sentinel_proxy::metrics::MetricsManager;

let manager = MetricsManager::new("sentinel-proxy", "node-1");
manager.register_pool_metrics("waf-pool", pool.metrics_collector_arc()).await;

// Metrics include both proxy and agent pool data
let response = manager.handle_metrics_request();
```

---

## Configuration Changes

### Agent Configuration

New v2-specific configuration options in KDL:

```kdl
agents {
    agent "waf" {
        // Transport selection (auto-detected from endpoint)
        endpoint "localhost:50051"        // gRPC
        // endpoint "/var/run/waf.sock"   // UDS

        // v2 protocol settings
        protocol-version 2

        // Connection pool settings
        connections 4
        request-timeout "30s"

        // Health and circuit breaker
        health-check-interval "10s"
        circuit-breaker {
            threshold 5
            reset-timeout "30s"
        }
    }
}

// Reverse connection listener
reverse-listener {
    path "/var/run/sentinel/agents.sock"
    max-connections-per-agent 4
    handshake-timeout "10s"
}
```

---

## API Changes

### New Types

| Type | Description |
|------|-------------|
| `AgentClientV2` | gRPC v2 client |
| `AgentClientV2Uds` | UDS v2 client |
| `ReverseConnectionListener` | Accepts inbound agent connections |
| `ReverseConnectionClient` | Wrapper for accepted connections |
| `AgentPool` | Connection pool with load balancing |
| `V2Transport` | Transport abstraction enum |
| `MetricsCollector` | Agent metrics aggregation |
| `ConfigPusher` | Config distribution to agents |
| `UnifiedMetricsAggregator` | Combined proxy + agent metrics |

### New Error Variants

```rust
pub enum AgentProtocolError {
    // Existing...
    ConnectionClosed,  // NEW: Connection was closed unexpectedly
}
```

### Breaking Changes

None. Agent Protocol 2.0 is additive - existing v1 agents continue to work unchanged.

---

## WASM Runtime (Experimental)

Initial foundation for WebAssembly-based agents:

```rust
use sentinel_wasm_runtime::{WasmRuntime, WasmAgentConfig};

let config = WasmAgentConfig {
    module_path: "agents/allowlist.wasm".into(),
    max_memory_bytes: 64 * 1024 * 1024,
    max_execution_time: Duration::from_millis(100),
    ..Default::default()
};

let runtime = WasmRuntime::new(config)?;
```

**Note:** WASM agent support is experimental in this release. The WIT interface and host functions are subject to change.

---

## Performance

### Benchmarks

Connection pooling with 4 connections per agent:

| Scenario | v0.2.x | v0.3.0 | Improvement |
|----------|--------|--------|-------------|
| Sequential requests | 1,200 req/s | 1,250 req/s | +4% |
| Concurrent (100 clients) | 8,500 req/s | 32,000 req/s | +276% |
| P99 latency (concurrent) | 45ms | 12ms | -73% |

UDS transport vs gRPC (local):

| Transport | Throughput | P50 Latency |
|-----------|------------|-------------|
| gRPC | 28,000 req/s | 1.2ms |
| UDS | 45,000 req/s | 0.4ms |

---

## Migration Guide

### Upgrading from v0.2.x

1. **Update dependencies:**
   ```toml
   sentinel-agent-protocol = "0.3.0"
   sentinel-proxy = "0.3.0"
   ```

2. **Optional: Migrate to v2 protocol**

   Existing v1 agents work unchanged. To use v2 features:

   ```rust
   // Before (v1)
   let client = AgentClient::new(config)?;

   // After (v2 with pooling)
   let pool = AgentPool::new();
   pool.add_agent("my-agent", "localhost:50051").await?;
   ```

3. **Optional: Enable reverse connections**

   ```rust
   let listener = ReverseConnectionListener::bind_uds(
       "/var/run/sentinel/agents.sock",
       ReverseConnectionConfig::default(),
   ).await?;

   // Accept in background
   tokio::spawn(listener.accept_loop(pool));
   ```

---

## Contributors

Thanks to everyone who contributed to this release!

---

## Full Changelog

See [GitHub Releases](https://github.com/raskell-io/sentinel/releases/tag/v0.3.0) for the complete list of changes.

---

## What's Next

Planned for v0.4.0:
- WAF agent with OWASP CRS support
- Rate limiting agent
- Auth/PEP agent patterns
- WASM agent stabilization
