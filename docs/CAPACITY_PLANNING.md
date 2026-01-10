# Sentinel Proxy - Capacity Planning Guide

## Table of Contents
1. [Resource Requirements](#resource-requirements)
2. [Sizing Guidelines](#sizing-guidelines)
3. [Performance Characteristics](#performance-characteristics)
4. [Capacity Metrics](#capacity-metrics)
5. [Scaling Strategies](#scaling-strategies)
6. [Load Testing](#load-testing)
7. [Capacity Planning Process](#capacity-planning-process)

---

## Resource Requirements

### Minimum Requirements

| Resource | Minimum | Recommended | Notes |
|----------|---------|-------------|-------|
| CPU | 2 cores | 4+ cores | Scales linearly with request rate |
| Memory | 512 MB | 2 GB+ | Depends on connection count |
| Disk | 1 GB | 10 GB | Logs, certificates, GeoIP DB |
| Network | 100 Mbps | 1 Gbps+ | Based on traffic volume |

### Resource Consumption Model

**CPU Usage**:
- TLS handshakes: ~2ms CPU per handshake
- Request processing: ~0.1ms CPU per request (proxy-only)
- WAF inspection: ~1-5ms CPU per request (when enabled)
- Compression: ~0.5-2ms CPU per MB compressed

**Memory Usage**:
- Base process: ~50 MB
- Per connection: ~2-8 KB (idle) / ~16-64 KB (active)
- Per worker thread: ~8 MB
- Request buffering: configurable via `max-body-size`
- Connection pool: ~1 KB per pooled connection

**Disk I/O**:
- Logs: ~500 bytes per request (access log)
- Config reload: minimal, single file read
- Certificate reload: minimal

---

## Sizing Guidelines

### Small Deployment
**Traffic**: < 1,000 requests/second

```
┌─────────────────────────────────────────────┐
│           Single Sentinel Instance          │
│                                             │
│  CPU: 2 cores    Memory: 1 GB               │
│  Workers: 2      Connections: 5,000         │
│                                             │
└─────────────────────────────────────────────┘
```

**Configuration**:
```kdl
server {
    worker-threads 2
    max-connections 5000
}

connection-pool {
    max-connections 100
    max-idle 20
}
```

### Medium Deployment
**Traffic**: 1,000 - 10,000 requests/second

```
┌──────────────────────────────────────────────────────────┐
│                   Load Balancer                          │
│                        │                                 │
│         ┌──────────────┼──────────────┐                  │
│         ▼              ▼              ▼                  │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐           │
│  │ Sentinel 1 │ │ Sentinel 2 │ │ Sentinel 3 │           │
│  │ 4 cores    │ │ 4 cores    │ │ 4 cores    │           │
│  │ 4 GB       │ │ 4 GB       │ │ 4 GB       │           │
│  └────────────┘ └────────────┘ └────────────┘           │
└──────────────────────────────────────────────────────────┘
```

**Configuration**:
```kdl
server {
    worker-threads 4
    max-connections 20000
}

connection-pool {
    max-connections 200
    max-idle 50
}
```

### Large Deployment
**Traffic**: 10,000 - 100,000 requests/second

```
┌─────────────────────────────────────────────────────────────────────┐
│                      Global Load Balancer                           │
│                             │                                       │
│     ┌───────────────────────┼───────────────────────┐               │
│     ▼                       ▼                       ▼               │
│ ┌────────────┐       ┌────────────┐          ┌────────────┐         │
│ │ Region A   │       │ Region B   │          │ Region C   │         │
│ │            │       │            │          │            │         │
│ │ Sentinel   │       │ Sentinel   │          │ Sentinel   │         │
│ │ Pool (5x)  │       │ Pool (5x)  │          │ Pool (5x)  │         │
│ │            │       │            │          │            │         │
│ │ 8 cores    │       │ 8 cores    │          │ 8 cores    │         │
│ │ 16 GB      │       │ 16 GB      │          │ 16 GB      │         │
│ └────────────┘       └────────────┘          └────────────┘         │
└─────────────────────────────────────────────────────────────────────┘
```

**Configuration**:
```kdl
server {
    worker-threads 0  // Use all available cores
    max-connections 50000
}

connection-pool {
    max-connections 500
    max-idle 100
    idle-timeout-secs 120
}

rate-limit {
    backend "redis" {
        endpoints ["redis://redis-cluster:6379"]
    }
}
```

---

## Performance Characteristics

### Request Processing Latency

| Component | Latency (p50) | Latency (p99) |
|-----------|---------------|---------------|
| TCP accept | < 0.1 ms | < 0.5 ms |
| TLS handshake (new) | 2-5 ms | 10-20 ms |
| TLS handshake (resumed) | 0.5-1 ms | 2-5 ms |
| Header parsing | < 0.1 ms | < 0.5 ms |
| Route matching | < 0.05 ms | < 0.2 ms |
| Upstream selection | < 0.01 ms | < 0.05 ms |
| Agent call (if enabled) | 1-5 ms | 10-50 ms |
| Proxy overhead (total) | 0.5-2 ms | 5-15 ms |

### Throughput Limits

| Scenario | Approximate Limit | Bottleneck |
|----------|-------------------|------------|
| Simple proxy (HTTP) | 50,000 RPS/core | CPU |
| TLS termination | 10,000 new conn/s/core | CPU (crypto) |
| Large body (1MB) | 1 Gbps / 8 Gbps | Network/Memory |
| WAF enabled | 5,000-10,000 RPS/core | Agent latency |

### Connection Limits

Formula for max connections:
```
Max Connections = Available Memory (MB) / Memory per Connection (KB) * 1024

Example:
4096 MB / 16 KB * 1024 = 262,144 connections (theoretical max)
Practical max: ~50% of theoretical for headroom
```

---

## Capacity Metrics

### Key Metrics for Capacity Planning

Monitor these metrics to understand current capacity usage:

```bash
# Current request rate
curl -s localhost:9090/metrics | grep 'requests_total' | tail -1

# Request rate (calculated)
rate(sentinel_requests_total[1m])

# Active connections
curl -s localhost:9090/metrics | grep 'open_connections'

# Connection pool utilization
curl -s localhost:9090/metrics | grep 'connection_pool'

# Memory usage
curl -s localhost:9090/metrics | grep 'process_resident_memory_bytes'

# CPU usage
curl -s localhost:9090/metrics | grep 'process_cpu_seconds_total'

# Request latency percentiles
curl -s localhost:9090/metrics | grep 'request_duration.*quantile'
```

### Capacity Thresholds

| Metric | Warning | Critical | Action |
|--------|---------|----------|--------|
| CPU utilization | > 70% | > 85% | Scale horizontally |
| Memory utilization | > 75% | > 90% | Increase memory or scale |
| Connection count | > 70% max | > 85% max | Increase limits or scale |
| p99 latency | > 100ms | > 500ms | Investigate or scale |
| Error rate | > 0.1% | > 1% | Investigate upstream/config |
| Connection pool wait | > 10ms | > 100ms | Increase pool size |

### Prometheus Alerting Rules

```yaml
groups:
  - name: sentinel-capacity
    rules:
      # CPU utilization warning
      - alert: SentinelHighCPU
        expr: rate(process_cpu_seconds_total{job="sentinel"}[5m]) > 0.7
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Sentinel CPU usage high"
          description: "CPU utilization > 70% for 5 minutes"

      # Memory warning
      - alert: SentinelHighMemory
        expr: process_resident_memory_bytes{job="sentinel"} / 1024 / 1024 / 1024 > 0.75 * node_memory_MemTotal_bytes / 1024 / 1024 / 1024
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Sentinel memory usage high"

      # Connection limit warning
      - alert: SentinelConnectionsHigh
        expr: sentinel_open_connections / sentinel_max_connections > 0.7
        for: 2m
        labels:
          severity: warning
        annotations:
          summary: "Sentinel approaching connection limit"

      # Latency degradation
      - alert: SentinelLatencyHigh
        expr: histogram_quantile(0.99, rate(sentinel_request_duration_seconds_bucket[5m])) > 0.1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Sentinel p99 latency > 100ms"
```

---

## Scaling Strategies

### Vertical Scaling

**When to use**: Quick fix, single-instance deployments

```bash
# Increase worker threads (if CPU-bound)
# Edit config.kdl:
server {
    worker-threads 8  # Increase from 4
}

# Increase connection limits (if connection-bound)
server {
    max-connections 50000  # Increase from 20000
}

# Reload
kill -HUP $(cat /var/run/sentinel.pid)
```

**Limits**:
- Single machine limits (typically 64 cores, 256 GB RAM)
- Single point of failure
- Diminishing returns above 8-16 cores for proxy workloads

### Horizontal Scaling

**When to use**: Production deployments, high availability

**Architecture**:
```
                    ┌───────────────────┐
                    │   Load Balancer   │
                    │   (L4 or L7)      │
                    └─────────┬─────────┘
                              │
          ┌───────────────────┼───────────────────┐
          │                   │                   │
    ┌─────▼─────┐      ┌─────▼─────┐      ┌─────▼─────┐
    │ Sentinel  │      │ Sentinel  │      │ Sentinel  │
    │ Instance 1│      │ Instance 2│      │ Instance 3│
    └───────────┘      └───────────┘      └───────────┘
```

**Load Balancer Options**:
- L4 (TCP): Lower latency, simpler, no TLS termination at LB
- L7 (HTTP): More features, but adds latency

**Scaling Formula**:
```
Instances Needed = (Peak RPS × Safety Factor) / RPS per Instance

Example:
Peak RPS: 50,000
Safety Factor: 1.5 (for headroom)
RPS per Instance: 15,000 (with WAF)

Instances = (50,000 × 1.5) / 15,000 = 5 instances
```

### Auto-Scaling

**Kubernetes HPA**:
```yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: sentinel-hpa
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: sentinel
  minReplicas: 3
  maxReplicas: 20
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 70
    - type: Pods
      pods:
        metric:
          name: sentinel_requests_per_second
        target:
          type: AverageValue
          averageValue: "10000"
```

**Scaling Triggers**:
| Metric | Scale Up Threshold | Scale Down Threshold |
|--------|-------------------|---------------------|
| CPU | > 70% for 2 min | < 50% for 10 min |
| Request rate | > 80% capacity | < 50% capacity |
| Connections | > 70% limit | < 50% limit |

---

## Load Testing

### Test Environment Setup

```bash
# Use a separate load testing environment
# Never load test production!

# Recommended tools
# - wrk: High-performance HTTP benchmarking
# - k6: Modern load testing tool
# - hey: Simple HTTP load generator
```

### Baseline Test

```bash
# Establish baseline with simple proxy path
# wrk installation: https://github.com/wg/wrk

# Simple throughput test
wrk -t12 -c400 -d30s http://sentinel:8080/health

# Latency-focused test
wrk -t4 -c50 -d60s --latency http://sentinel:8080/api/endpoint

# Expected output analysis:
# - Requests/sec: baseline throughput
# - Latency distribution: p50, p75, p90, p99
# - Errors: should be 0
```

### Capacity Test Script

```bash
#!/bin/bash
# find-capacity.sh - Find maximum sustainable throughput

SENTINEL_URL="http://sentinel:8080/api/endpoint"
DURATION=60
OUTPUT_DIR="./capacity-results"

mkdir -p $OUTPUT_DIR

echo "Starting capacity test..."

for CONNECTIONS in 100 500 1000 2000 5000 10000; do
    echo "Testing with $CONNECTIONS connections..."

    wrk -t12 -c$CONNECTIONS -d${DURATION}s --latency $SENTINEL_URL \
        > $OUTPUT_DIR/wrk-${CONNECTIONS}c.txt 2>&1

    # Capture metrics during test
    curl -s localhost:9090/metrics > $OUTPUT_DIR/metrics-${CONNECTIONS}c.txt

    # Parse key metrics
    RPS=$(grep "Requests/sec" $OUTPUT_DIR/wrk-${CONNECTIONS}c.txt | awk '{print $2}')
    P99=$(grep "99%" $OUTPUT_DIR/wrk-${CONNECTIONS}c.txt | awk '{print $2}')
    ERRORS=$(grep "Non-2xx" $OUTPUT_DIR/wrk-${CONNECTIONS}c.txt | awk '{print $4}')

    echo "$CONNECTIONS connections: $RPS RPS, p99=$P99, errors=$ERRORS"

    # Check for degradation
    if [ -n "$ERRORS" ] && [ "$ERRORS" -gt 0 ]; then
        echo "Errors detected - capacity limit reached"
        break
    fi

    sleep 30  # Cool down between tests
done

echo "Results saved to $OUTPUT_DIR/"
```

### Stress Test (Find Breaking Point)

```bash
#!/bin/bash
# stress-test.sh - Find when the system breaks

echo "WARNING: This test will push the system to failure"
echo "Only run on non-production systems!"

SENTINEL_URL="http://sentinel:8080/api/endpoint"

# Ramp up until failure
for RATE in 1000 5000 10000 20000 50000 100000; do
    echo "Testing at $RATE RPS..."

    # Use hey for rate-limited testing
    hey -n 100000 -c 500 -q $RATE $SENTINEL_URL > stress-${RATE}.txt 2>&1

    # Check error rate
    ERRORS=$(grep "Status code distribution" -A5 stress-${RATE}.txt | grep -v "200" | wc -l)

    if [ $ERRORS -gt 0 ]; then
        echo "Errors at $RATE RPS - stress limit found"
        cat stress-${RATE}.txt
        break
    fi

    sleep 60  # Recovery between tests
done
```

---

## Capacity Planning Process

### 1. Gather Requirements

```markdown
## Capacity Requirements Worksheet

### Traffic Profile
- Peak requests per second: _______________
- Average requests per second: _______________
- Peak concurrent connections: _______________
- Average request size: _______________
- Average response size: _______________
- TLS termination required: [ ] Yes [ ] No
- WAF/Agent processing: [ ] Yes [ ] No

### Growth Projections
- Expected growth rate: _______________% per year
- Seasonal peaks: _______________ (describe)
- Special events: _______________ (describe)

### SLA Requirements
- Target availability: _______________% (e.g., 99.9%)
- Target latency (p99): _______________ms
- Maximum error rate: _______________%

### Constraints
- Budget: _______________
- Deployment environment: _______________
- Geographic requirements: _______________
```

### 2. Calculate Base Capacity

```python
# capacity_calculator.py

def calculate_capacity(
    peak_rps: int,
    avg_request_size_kb: float,
    avg_response_size_kb: float,
    tls_enabled: bool,
    waf_enabled: bool,
    safety_factor: float = 1.5,
    growth_rate: float = 0.25,  # 25% annual growth
    planning_horizon_years: int = 2
) -> dict:
    """Calculate required capacity for Sentinel deployment."""

    # RPS per core (base assumptions)
    rps_per_core_base = 50000

    # Adjust for features
    if tls_enabled:
        rps_per_core_base *= 0.5  # TLS overhead
    if waf_enabled:
        rps_per_core_base *= 0.3  # Agent overhead

    # Calculate future RPS
    future_rps = peak_rps * ((1 + growth_rate) ** planning_horizon_years)

    # Calculate required cores
    required_cores = (future_rps * safety_factor) / rps_per_core_base

    # Calculate memory (connection-based)
    # Assume 1 connection per 10 RPS average
    connections = future_rps / 10
    memory_per_connection_kb = 16 if waf_enabled else 8
    required_memory_gb = (connections * memory_per_connection_kb) / 1024 / 1024
    required_memory_gb = max(required_memory_gb, 2)  # Minimum 2 GB

    # Calculate bandwidth
    total_size_kb = avg_request_size_kb + avg_response_size_kb
    required_bandwidth_gbps = (future_rps * total_size_kb * 8) / 1024 / 1024

    # Calculate instances (assume 8 cores per instance)
    cores_per_instance = 8
    required_instances = max(2, int(required_cores / cores_per_instance) + 1)

    return {
        "current_peak_rps": peak_rps,
        "projected_peak_rps": int(future_rps),
        "required_total_cores": int(required_cores) + 1,
        "required_memory_gb_per_instance": round(required_memory_gb / required_instances, 1),
        "required_bandwidth_gbps": round(required_bandwidth_gbps, 2),
        "recommended_instances": required_instances,
        "instance_spec": f"{cores_per_instance} cores, {round(required_memory_gb / required_instances, 1)} GB RAM"
    }

# Example usage
result = calculate_capacity(
    peak_rps=10000,
    avg_request_size_kb=2,
    avg_response_size_kb=10,
    tls_enabled=True,
    waf_enabled=True
)
print(result)
```

### 3. Size and Validate

```markdown
## Sizing Validation Checklist

[ ] Load test with expected peak traffic
[ ] Verify p99 latency within SLA
[ ] Confirm error rate within SLA
[ ] Test failover scenarios (N-1 capacity)
[ ] Verify auto-scaling triggers work
[ ] Test graceful degradation under overload
[ ] Validate monitoring/alerting
```

### 4. Document and Review

```markdown
## Capacity Plan Summary

### Deployment Specification
- Environment: _______________
- Instance type: _______________
- Instance count: _______________
- Region(s): _______________

### Capacity Limits
- Maximum sustainable RPS: _______________
- Maximum connections: _______________
- Headroom: _______________%

### Scaling Thresholds
- Scale up trigger: _______________
- Scale down trigger: _______________
- Minimum instances: _______________
- Maximum instances: _______________

### Review Schedule
- Next capacity review: _______________
- Growth review trigger: _______________% increase
```

---

## Quick Reference

### Capacity Rules of Thumb

1. **CPU**: 1 core ≈ 10,000-50,000 simple proxy RPS
2. **Memory**: 16 KB per active connection (more with WAF)
3. **TLS**: Halves throughput, 10K new connections/sec/core
4. **WAF**: Reduces throughput by 50-70%
5. **Instances**: Minimum 3 for HA, N+1 for maintenance

### Common Bottlenecks

| Symptom | Likely Bottleneck | Solution |
|---------|-------------------|----------|
| High CPU, low connections | Processing capacity | Add cores/instances |
| High connections, low CPU | Connection limits | Increase limits, optimize keepalive |
| High p99, moderate CPU | Upstream latency | Optimize upstreams, increase timeouts |
| Errors under load | Resource exhaustion | Scale up/out, increase limits |

### Monitoring Dashboard Essentials

```
┌────────────────────────────────────────────────────────────────────┐
│                    Sentinel Capacity Dashboard                      │
├────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Request Rate          CPU Usage           Memory Usage             │
│  ┌──────────────┐     ┌──────────────┐    ┌──────────────┐         │
│  │ 15,234 RPS   │     │    45%       │    │    62%       │         │
│  │ ▁▃▅▇█▇▅▃▁   │     │ ▁▃▅▇▇▅▃▁    │    │ ▁▁▂▃▃▃▃▃    │         │
│  └──────────────┘     └──────────────┘    └──────────────┘         │
│                                                                      │
│  Connections           p99 Latency         Error Rate               │
│  ┌──────────────┐     ┌──────────────┐    ┌──────────────┐         │
│  │  8,432       │     │   23ms       │    │   0.01%      │         │
│  │ ▁▂▃▄▄▃▂▁    │     │ ▁▁▂▂▁▁▁▁    │    │ ▁▁▁▁▁▁▁▁    │         │
│  └──────────────┘     └──────────────┘    └──────────────┘         │
│                                                                      │
│  Capacity Headroom: 55% │ Instances: 5/5 healthy                    │
└────────────────────────────────────────────────────────────────────┘
```

---

**Review this capacity plan quarterly or when traffic increases by 25%.**
