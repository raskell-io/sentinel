# Agent Protocol Documentation

This directory contains documentation for the Zentinel Agent Protocol, which defines how the proxy communicates with external processing agents.

## [Protocol Documentation](./v2/)

The agent protocol provides:

- **Multiple transports**: gRPC, Binary UDS, and Reverse Connections
- **Connection pooling**: Built-in `AgentPool` with load balancing
- **Request cancellation**: Cancel in-flight requests
- **Enhanced observability**: Metrics export in Prometheus format
- **Reverse connections**: Agents can connect to the proxy (NAT traversal)

| Document | Description |
|----------|-------------|
| [protocol.md](./v2/protocol.md) | Wire protocol specification |
| [api.md](./v2/api.md) | Client and server APIs |
| [pooling.md](./v2/pooling.md) | Connection pooling and load balancing |
| [transports.md](./v2/transports.md) | Transport options (gRPC, UDS, Reverse) |
| [reverse-connections.md](./v2/reverse-connections.md) | Reverse connection setup |
| [performance-roadmap.md](./performance-roadmap.md) | Performance bottlenecks and optimization plans |

## Architecture

See [architecture.md](./architecture.md) for system architecture diagrams covering the agent protocol architecture.

## Quick Start

```rust
use zentinel_agent_protocol::v2::{AgentPool, AgentPoolConfig, LoadBalanceStrategy};
use std::time::Duration;

// Create a connection pool
let config = AgentPoolConfig {
    connections_per_agent: 4,
    load_balance_strategy: LoadBalanceStrategy::LeastConnections,
    request_timeout: Duration::from_secs(30),
    ..Default::default()
};

let pool = AgentPool::with_config(config);

// Add agents (transport auto-detected)
pool.add_agent("waf", "localhost:50051").await?;       // gRPC
pool.add_agent("auth", "/var/run/auth.sock").await?;   // UDS

// Send requests
let response = pool.send_request_headers("waf", &headers).await?;
```

## Related Documentation

- [Zentinel CLAUDE.md](../../../.claude/CLAUDE.md) - Overall project documentation
- [Performance Roadmap](./performance-roadmap.md) - Bottleneck analysis and optimization plans
