# Sentinel Metrics Reference

Sentinel exposes Prometheus-compatible metrics at the `/metrics` endpoint (default port 9090).

## Quick Start

```bash
# Scrape metrics
curl http://localhost:9090/metrics

# Prometheus scrape config
scrape_configs:
  - job_name: 'sentinel'
    static_configs:
      - targets: ['localhost:9090']
```

---

## Metric Categories

- [Proxy Health](#proxy-health)
- [Request Metrics](#request-metrics)
- [Upstream Metrics](#upstream-metrics)
- [Agent Metrics](#agent-metrics)
- [Cache Metrics](#cache-metrics)
- [Connection Pool Metrics](#connection-pool-metrics)
- [TLS Metrics](#tls-metrics)
- [WebSocket Metrics](#websocket-metrics)
- [System Metrics](#system-metrics)

---

## Proxy Health

### sentinel_up
**Type:** Gauge

Indicates whether Sentinel is running. Value is always `1` when the metrics endpoint responds.

```promql
# Alert if Sentinel is down
alert: SentinelDown
expr: sentinel_up == 0
for: 1m
```

### sentinel_build_info
**Type:** Gauge

Build information with version label. Value is always `1`.

| Label | Description |
|-------|-------------|
| version | Sentinel version (e.g., "0.1.8") |

```promql
# Get current version
sentinel_build_info{version="0.1.8"}
```

---

## Request Metrics

### sentinel_requests_total
**Type:** Counter

Total number of HTTP requests processed.

| Label | Description |
|-------|-------------|
| route | Route ID that handled the request |
| method | HTTP method (GET, POST, etc.) |
| status | HTTP status code (200, 404, 500, etc.) |

```promql
# Request rate by route
sum(rate(sentinel_requests_total[5m])) by (route)

# Error rate (5xx)
sum(rate(sentinel_requests_total{status=~"5.."}[5m])) / sum(rate(sentinel_requests_total[5m]))

# Success rate
sum(rate(sentinel_requests_total{status=~"2.."}[5m])) / sum(rate(sentinel_requests_total[5m]))
```

### sentinel_request_duration_seconds
**Type:** Histogram

Request latency distribution in seconds.

| Label | Description |
|-------|-------------|
| route | Route ID |
| method | HTTP method |

**Buckets:** 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s, 5s, 10s

```promql
# P50 latency
histogram_quantile(0.50, sum(rate(sentinel_request_duration_seconds_bucket[5m])) by (le))

# P95 latency
histogram_quantile(0.95, sum(rate(sentinel_request_duration_seconds_bucket[5m])) by (le))

# P99 latency
histogram_quantile(0.99, sum(rate(sentinel_request_duration_seconds_bucket[5m])) by (le))

# P95 latency by route
histogram_quantile(0.95, sum(rate(sentinel_request_duration_seconds_bucket[5m])) by (le, route))
```

### sentinel_active_requests
**Type:** Gauge

Number of requests currently being processed.

```promql
# Current active requests
sentinel_active_requests

# Alert on high concurrency
alert: HighConcurrency
expr: sentinel_active_requests > 1000
for: 5m
```

### sentinel_blocked_requests_total
**Type:** Counter

Requests blocked by security policies.

| Label | Description |
|-------|-------------|
| reason | Block reason (rate_limit, waf, geo, auth, etc.) |

```promql
# Block rate by reason
sum(rate(sentinel_blocked_requests_total[5m])) by (reason)
```

### sentinel_request_body_size_bytes
**Type:** Histogram

Request body size distribution.

| Label | Description |
|-------|-------------|
| route | Route ID |

**Buckets:** 100B, 1KB, 10KB, 100KB, 1MB, 10MB, 100MB

### sentinel_response_body_size_bytes
**Type:** Histogram

Response body size distribution.

| Label | Description |
|-------|-------------|
| route | Route ID |

**Buckets:** 100B, 1KB, 10KB, 100KB, 1MB, 10MB, 100MB

---

## Upstream Metrics

### sentinel_upstream_attempts_total
**Type:** Counter

Total upstream connection attempts.

| Label | Description |
|-------|-------------|
| upstream | Upstream pool ID |
| route | Route ID |

```promql
# Upstream attempt rate
sum(rate(sentinel_upstream_attempts_total[5m])) by (upstream)
```

### sentinel_upstream_failures_total
**Type:** Counter

Failed upstream connections.

| Label | Description |
|-------|-------------|
| upstream | Upstream pool ID |
| route | Route ID |
| reason | Failure reason (timeout, connection_refused, etc.) |

```promql
# Upstream failure rate
sum(rate(sentinel_upstream_failures_total[5m])) by (upstream, reason)

# Upstream error ratio
sum(rate(sentinel_upstream_failures_total[5m])) by (upstream)
  / sum(rate(sentinel_upstream_attempts_total[5m])) by (upstream)
```

### sentinel_circuit_breaker_state
**Type:** Gauge

Circuit breaker state per component.

| Label | Description |
|-------|-------------|
| component | Component name (upstream ID or agent ID) |
| route | Route ID |

**Values:** 0 = closed (healthy), 1 = open (unhealthy)

```promql
# Alert on open circuit breakers
alert: CircuitBreakerOpen
expr: sentinel_circuit_breaker_state == 1
for: 1m
```

---

## Agent Metrics

### sentinel_agent_latency_seconds
**Type:** Histogram

Agent call latency distribution.

| Label | Description |
|-------|-------------|
| agent | Agent ID |
| event | Event type (request_headers, request_body, etc.) |

**Buckets:** 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s, 5s, 10s

```promql
# Agent P95 latency
histogram_quantile(0.95, sum(rate(sentinel_agent_latency_seconds_bucket[5m])) by (le, agent))
```

### sentinel_agent_timeouts_total
**Type:** Counter

Agent call timeouts.

| Label | Description |
|-------|-------------|
| agent | Agent ID |
| event | Event type |

```promql
# Agent timeout rate
sum(rate(sentinel_agent_timeouts_total[5m])) by (agent)
```

---

## Cache Metrics

### sentinel_cache_hits_total
**Type:** Counter

Total cache hits.

```promql
# Cache hit rate
rate(sentinel_cache_hits_total[5m])
```

### sentinel_cache_misses_total
**Type:** Counter

Total cache misses.

```promql
# Cache miss rate
rate(sentinel_cache_misses_total[5m])
```

### sentinel_cache_stores_total
**Type:** Counter

Total cache stores (new entries added).

### sentinel_cache_hit_ratio
**Type:** Gauge

Current cache hit ratio (0.0 to 1.0).

```promql
# Alert on low hit ratio
alert: LowCacheHitRatio
expr: sentinel_cache_hit_ratio < 0.5
for: 10m
```

---

## Connection Pool Metrics

### sentinel_connection_pool_size
**Type:** Gauge

Total connections in the pool per upstream.

| Label | Description |
|-------|-------------|
| upstream | Upstream pool ID |

### sentinel_connection_pool_idle
**Type:** Gauge

Idle connections in the pool per upstream.

| Label | Description |
|-------|-------------|
| upstream | Upstream pool ID |

```promql
# Connection utilization
(sentinel_connection_pool_size - sentinel_connection_pool_idle) / sentinel_connection_pool_size
```

### sentinel_connection_pool_acquired_total
**Type:** Counter

Total connections acquired from pool.

| Label | Description |
|-------|-------------|
| upstream | Upstream pool ID |

---

## TLS Metrics

### sentinel_tls_handshake_duration_seconds
**Type:** Histogram

TLS handshake latency distribution.

| Label | Description |
|-------|-------------|
| version | TLS version (TLS1.2, TLS1.3) |

**Buckets:** 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s, 5s, 10s

```promql
# TLS handshake P95
histogram_quantile(0.95, sum(rate(sentinel_tls_handshake_duration_seconds_bucket[5m])) by (le, version))
```

---

## WebSocket Metrics

### sentinel_websocket_connections_total
**Type:** Counter

Total WebSocket connections with inspection enabled.

| Label | Description |
|-------|-------------|
| route | Route ID |

### sentinel_websocket_frames_total
**Type:** Counter

Total WebSocket frames processed.

| Label | Description |
|-------|-------------|
| route | Route ID |
| direction | c2s (client to server) or s2c (server to client) |
| opcode | text, binary, ping, pong, close, continuation |
| decision | allow, drop, close |

```promql
# Frame rate by direction
sum(rate(sentinel_websocket_frames_total[5m])) by (direction)

# Blocked frame rate
sum(rate(sentinel_websocket_frames_total{decision!="allow"}[5m]))
```

### sentinel_websocket_inspection_duration_seconds
**Type:** Histogram

Frame inspection latency.

| Label | Description |
|-------|-------------|
| route | Route ID |

**Buckets:** 0.1ms, 0.5ms, 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms

### sentinel_websocket_frame_size_bytes
**Type:** Histogram

WebSocket frame payload size.

| Label | Description |
|-------|-------------|
| route | Route ID |
| direction | c2s or s2c |
| opcode | Frame type |

**Buckets:** 64B, 256B, 1KB, 4KB, 16KB, 64KB, 256KB, 1MB

---

## System Metrics

### sentinel_memory_usage_bytes
**Type:** Gauge

Current memory usage in bytes.

```promql
# Memory in MB
sentinel_memory_usage_bytes / 1024 / 1024
```

### sentinel_cpu_usage_percent
**Type:** Gauge

Current CPU usage percentage.

### sentinel_open_connections
**Type:** Gauge

Number of open client connections.

```promql
# Alert on high connection count
alert: HighConnectionCount
expr: sentinel_open_connections > 10000
for: 5m
```

---

## Example Alerts

```yaml
groups:
  - name: sentinel
    rules:
      # High error rate
      - alert: SentinelHighErrorRate
        expr: |
          sum(rate(sentinel_requests_total{status=~"5.."}[5m]))
          / sum(rate(sentinel_requests_total[5m])) > 0.05
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "High 5xx error rate ({{ $value | humanizePercentage }})"

      # High latency
      - alert: SentinelHighLatency
        expr: |
          histogram_quantile(0.95, sum(rate(sentinel_request_duration_seconds_bucket[5m])) by (le)) > 1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "P95 latency above 1 second"

      # Upstream failures
      - alert: SentinelUpstreamFailures
        expr: |
          sum(rate(sentinel_upstream_failures_total[5m])) by (upstream) > 0.1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Upstream {{ $labels.upstream }} experiencing failures"

      # Agent timeouts
      - alert: SentinelAgentTimeouts
        expr: |
          sum(rate(sentinel_agent_timeouts_total[5m])) by (agent) > 0
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Agent {{ $labels.agent }} experiencing timeouts"

      # Low cache hit ratio
      - alert: SentinelLowCacheHitRatio
        expr: sentinel_cache_hit_ratio < 0.3
        for: 15m
        labels:
          severity: info
        annotations:
          summary: "Cache hit ratio below 30%"
```

---

## Grafana Dashboard

Import the pre-built dashboard from `config/grafana/sentinel-dashboard.json`.

The dashboard includes:
- Overview panel with key health indicators
- Request rate and status code breakdown
- Latency percentiles (p50, p90, p95, p99)
- Cache performance graphs
- Upstream health status
- Connection and memory usage

---

## See Also

- [Grafana Dashboard](../config/grafana/sentinel-dashboard.json)
- [Observability Configuration](../config/sentinel.kdl) - `observability` block
- [Prometheus Documentation](https://prometheus.io/docs/)
