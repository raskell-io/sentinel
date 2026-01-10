# Sentinel Benchmarking Roadmap

**Goal:** Establish Sentinel as the most transparently benchmarked reverse proxy with reproducible, comprehensive, and competitive performance data.

**Principles:**
1. **Reproducibility** — Anyone can run the same benchmarks and get similar results
2. **Transparency** — Methodology, raw data, and limitations are published
3. **Fairness** — Competitors are configured optimally, not strawmanned
4. **Relevance** — Benchmarks reflect real-world usage patterns
5. **Automation** — Continuous regression detection in CI

---

## Current State

| Category | Status | Coverage |
|----------|--------|----------|
| Load testing | Basic | Single environment, single workload |
| Soak testing | Good | 24-72h with leak detection |
| Chaos testing | Good | 10 scenarios covering failures |
| Competitor comparison | Minimal | Envoy only, single config |
| Reproducibility | Poor | No public benchmark suite |
| CI integration | None | Manual runs only |

---

## Phase 1: Foundation (4-6 weeks)

### 1.1 Create Public Benchmark Repository

**Deliverable:** `github.com/raskell-io/sentinel-bench`

```
sentinel-bench/
├── README.md                 # Methodology, how to run
├── METHODOLOGY.md            # Detailed explanation of choices
├── hardware/
│   └── specifications.md     # Reference hardware specs
├── workloads/
│   ├── baseline/             # Simple echo, no processing
│   ├── realistic/            # Mixed GET/POST, varied sizes
│   ├── api-gateway/          # Auth + rate limit + validation
│   └── waf-heavy/            # Full WAF pipeline
├── scenarios/
│   ├── throughput/           # Max RPS at target latency
│   ├── latency/              # Latency distribution under load
│   ├── concurrency/          # Scaling with connections
│   └── saturation/           # Behavior at/beyond limits
├── configs/
│   ├── sentinel/
│   ├── envoy/
│   ├── nginx/
│   ├── haproxy/
│   └── traefik/
├── scripts/
│   ├── run-benchmark.sh
│   ├── collect-metrics.sh
│   └── generate-report.sh
├── results/                  # Raw data from runs
└── reports/                  # Generated comparison reports
```

### 1.2 Define Standard Workloads

| Workload | Description | Rationale |
|----------|-------------|-----------|
| **Baseline** | Echo server, 1KB response | Pure proxy overhead |
| **Static** | Cached 10KB file | CDN/static serving use case |
| **API Light** | JSON API, 500B req/resp | Typical microservice |
| **API Heavy** | JSON API, 50KB response | Larger payload handling |
| **Mixed** | 70% GET, 25% POST, 5% PUT | Realistic traffic mix |
| **Authenticated** | JWT validation on each request | Auth overhead |
| **Rate Limited** | 1000 RPS limit per client | Rate limiting overhead |
| **WAF Enabled** | Full OWASP CRS ruleset | Security overhead |
| **Full Pipeline** | Auth + Rate Limit + WAF | Realistic production |

### 1.3 Standardize Metrics Collection

**Primary Metrics:**

| Metric | Tool | Description |
|--------|------|-------------|
| Throughput (RPS) | wrk2/oha | Requests per second at target latency |
| Latency (p50/p95/p99/p999) | wrk2/oha | Response time distribution |
| Max Latency | wrk2/oha | Worst-case latency |
| Error Rate | wrk2/oha | 4xx/5xx percentage |
| CPU Usage | pidstat/prometheus | Proxy CPU consumption |
| Memory (RSS) | pidstat/prometheus | Resident memory |
| Memory (Heap) | prometheus | Heap allocation |
| Connections | netstat/prometheus | Active/idle connections |

**Secondary Metrics:**

| Metric | Description |
|--------|-------------|
| Time to First Byte (TTFB) | Latency until first response byte |
| Connection Setup Time | TLS handshake + TCP |
| Throughput per Core | Efficiency metric |
| Memory per Connection | Scaling efficiency |

### 1.4 Document Methodology

Create `METHODOLOGY.md` covering:

1. **Hardware Selection**
   - Why specific instance types
   - Network configuration
   - Kernel tuning applied

2. **Load Generation**
   - Why wrk2 over wrk (coordinated omission)
   - Connection counts and warm-up
   - Duration and repetitions

3. **Configuration Philosophy**
   - "Optimal realistic" vs "maximum performance"
   - Which features enabled/disabled
   - How configs were reviewed (by maintainers if possible)

4. **Statistical Rigor**
   - Number of runs per benchmark
   - Warm-up period
   - Outlier handling
   - Confidence intervals

5. **Known Limitations**
   - What benchmarks DON'T measure
   - Environmental factors
   - Version-specific caveats

---

## Phase 2: Comprehensive Testing (4-6 weeks)

### 2.1 Multi-Environment Testing

| Environment | Instance | Purpose |
|-------------|----------|---------|
| **Cloud Small** | c6i.xlarge (4 vCPU, 8GB) | Entry-level production |
| **Cloud Medium** | c6i.4xlarge (16 vCPU, 32GB) | Typical production |
| **Cloud Large** | c6i.8xlarge (32 vCPU, 64GB) | High-traffic production |
| **Bare Metal** | Dedicated server | Eliminate virtualization noise |
| **ARM64** | c6g.4xlarge | ARM architecture |
| **Local Dev** | M1/M2 Mac, Linux desktop | Developer experience |

### 2.2 Latency-Focused Benchmarks

**Latency Under Load Matrix:**

```
Load Level    │ 10% │ 25% │ 50% │ 75% │ 90% │ 100% │ 110%
──────────────┼─────┼─────┼─────┼─────┼─────┼──────┼──────
p50 Latency   │     │     │     │     │     │      │
p95 Latency   │     │     │     │     │     │      │
p99 Latency   │     │     │     │     │     │      │
p999 Latency  │     │     │     │     │     │      │
Error Rate    │     │     │     │     │     │      │
```

Key questions to answer:
- At what load does p99 latency start degrading?
- What's the latency cliff (where errors start)?
- How graceful is degradation under overload?

### 2.3 Concurrency Scaling

Test with varying concurrent connections:

| Connections | Throughput | p99 Latency | Memory | CPU |
|-------------|------------|-------------|--------|-----|
| 100 | | | | |
| 1,000 | | | | |
| 10,000 | | | | |
| 50,000 | | | | |
| 100,000 | | | | |

### 2.4 Feature Overhead Analysis

Measure incremental cost of each feature:

| Configuration | RPS | p99 | Overhead vs Baseline |
|---------------|-----|-----|----------------------|
| Baseline (passthrough) | | | — |
| + Access logging | | | |
| + Metrics collection | | | |
| + TLS termination | | | |
| + Header manipulation | | | |
| + Rate limiting (local) | | | |
| + Rate limiting (Redis) | | | |
| + JWT validation | | | |
| + Request body inspection | | | |
| + WAF (paranoia level 1) | | | |
| + WAF (paranoia level 2) | | | |
| Full production config | | | |

---

## Phase 3: Competitor Analysis (3-4 weeks)

### 3.1 Fair Comparison Framework

**Proxy Versions:**
- Always use latest stable release
- Document exact versions and build flags
- Link to configurations used

**Configuration Review:**
- Share configs with proxy maintainers/community before publishing
- Accept feedback and corrections
- Document any disagreements

**Workload Parity:**
- Identical backend for all proxies
- Same client load generator
- Same network path

### 3.2 Comparison Matrix

| Proxy | Version | Baseline RPS | p99 Latency | Memory | Config |
|-------|---------|--------------|-------------|--------|--------|
| Sentinel | 0.2.x | | | | [link] |
| Envoy | 1.29.x | | | | [link] |
| NGINX | 1.25.x | | | | [link] |
| HAProxy | 2.9.x | | | | [link] |
| Traefik | 3.0.x | | | | [link] |
| Caddy | 2.7.x | | | | [link] |

### 3.3 Specific Comparisons

**Sentinel vs Envoy:**
- Both are modern, extensible proxies
- Focus: Extension model overhead, observability features

**Sentinel vs NGINX:**
- NGINX is the incumbent
- Focus: Raw performance, memory efficiency

**Sentinel vs HAProxy:**
- HAProxy is the performance benchmark
- Focus: Throughput, latency consistency

**Sentinel vs Traefik:**
- Both target modern deployment patterns
- Focus: Kubernetes integration, ease of use

---

## Phase 4: CI Integration (2-3 weeks)

### 4.1 Automated Regression Detection

```yaml
# .github/workflows/benchmark.yml
name: Performance Regression Check

on:
  pull_request:
    paths:
      - 'crates/proxy/**'
      - 'crates/config/**'

jobs:
  benchmark:
    runs-on: self-hosted  # Dedicated benchmark runner
    steps:
      - uses: actions/checkout@v4

      - name: Run baseline benchmark
        run: ./bench/run-baseline.sh

      - name: Compare with main branch
        run: ./bench/compare.sh main

      - name: Fail if regression > 5%
        run: ./bench/check-regression.sh --threshold 5
```

### 4.2 Nightly Comprehensive Benchmarks

- Full benchmark suite runs nightly
- Results published to dashboard
- Historical tracking over time
- Alerts on significant changes

### 4.3 Benchmark Dashboard

Public dashboard showing:
- Current performance metrics
- Historical trends
- Comparison with competitors
- Per-commit tracking

Options:
- GitHub Pages with static charts
- Grafana Cloud public dashboard
- Custom dashboard in docs site

---

## Phase 5: Advanced Scenarios (4-6 weeks)

### 5.1 Real-World Workload Simulation

**E-Commerce Pattern:**
```
- 60% product catalog browsing (cached, read-heavy)
- 25% search queries (dynamic, varied size)
- 10% cart operations (write, session-aware)
- 5% checkout (authenticated, multi-step)
```

**API Gateway Pattern:**
```
- Mixed authentication methods (JWT, API key, mTLS)
- Rate limiting (per-user, per-endpoint)
- Request validation (OpenAPI schema)
- Response transformation
```

**Security-Heavy Pattern:**
```
- Full WAF with OWASP CRS level 2
- Bot detection
- IP reputation checking
- Request body inspection
```

### 5.2 Failure Mode Benchmarks

| Scenario | Metric | Target |
|----------|--------|--------|
| Backend 50% failure rate | Successful RPS maintained | >90% |
| Backend 500ms latency spike | p99 latency increase | <2x |
| Agent crash during load | Recovery time | <1s |
| Config reload under load | Dropped requests | 0 |
| Memory pressure (80% used) | Latency degradation | <20% |

### 5.3 Long-Running Stability

| Duration | Metrics Tracked |
|----------|-----------------|
| 1 hour | Memory, latency, throughput |
| 24 hours | Memory growth, GC pauses, connection leaks |
| 7 days | Long-term stability, memory fragmentation |
| 30 days | Production simulation |

### 5.4 Edge Cases

- **Slow clients:** 10KB/s upload speed (slowloris-style)
- **Large headers:** 32KB+ header sizes
- **Large bodies:** 100MB+ uploads
- **WebSocket longevity:** 10,000 connections for 24h
- **Connection churn:** 10,000 new connections/second
- **TLS handshake storms:** Mass reconnection scenario

---

## Phase 6: Transparency & Credibility (Ongoing)

### 6.1 Raw Data Publication

For every benchmark run, publish:
- Raw CSV/JSON data
- Load generator output
- System metrics (sar, pidstat)
- Proxy logs and metrics
- Environment details (kernel, CPU, memory)

### 6.2 Reproducibility Package

Docker Compose setup for anyone to run:

```bash
git clone https://github.com/raskell-io/sentinel-bench
cd sentinel-bench
./run-all.sh --proxy sentinel envoy nginx
```

### 6.3 Third-Party Validation

- Invite community members to run benchmarks
- Accept and publish external results
- Partner with cloud providers for official benchmarks
- Submit to independent benchmark suites (TechEmpower?)

### 6.4 Honest Reporting

**Always Include:**
- Where Sentinel loses
- Known limitations
- Unfair advantages (e.g., "Sentinel wins because X feature is missing")
- Caveats and context

**Example Honest Framing:**
> "Sentinel achieves 23K RPS compared to NGINX's 45K RPS in pure passthrough mode. However, when WAF and rate limiting are enabled, the gap narrows to 12K vs 14K RPS. Sentinel's architecture prioritizes extensibility over raw passthrough speed."

---

## Success Metrics

### Credibility Indicators

| Indicator | Target |
|-----------|--------|
| External reproduction of results | Within 10% variance |
| Community benchmark contributions | 5+ per quarter |
| Maintainer acknowledgment from other proxies | "Fair comparison" |
| Citations in technical discussions | Regular references |

### Performance Targets

| Metric | Target | Rationale |
|--------|--------|-----------|
| Baseline throughput | Within 20% of NGINX | Competitive for most use cases |
| p99 latency | Within 10% of HAProxy | Latency-sensitive workloads |
| Memory efficiency | < 2x Envoy | Reasonable for feature set |
| Full pipeline throughput | #1 among extensible proxies | Our differentiator |

### Process Metrics

| Metric | Target |
|--------|--------|
| CI benchmark on every PR | 100% |
| Nightly full benchmark | 100% |
| Results published within 24h | 95% |
| Regression detection accuracy | <5% false positives |

---

## Timeline Summary

| Phase | Duration | Key Deliverable |
|-------|----------|-----------------|
| **Phase 1: Foundation** | 4-6 weeks | Public benchmark repo with methodology |
| **Phase 2: Comprehensive** | 4-6 weeks | Multi-environment, multi-workload results |
| **Phase 3: Competitors** | 3-4 weeks | Fair comparison with 5+ proxies |
| **Phase 4: CI** | 2-3 weeks | Automated regression detection |
| **Phase 5: Advanced** | 4-6 weeks | Real-world scenarios, edge cases |
| **Phase 6: Transparency** | Ongoing | Raw data, reproducibility, third-party validation |

**Total: ~20 weeks to comprehensive benchmarking infrastructure**

---

## Immediate Next Steps

1. **Create `sentinel-bench` repository** with basic structure
2. **Port existing benchmarks** to reproducible scripts
3. **Document current methodology** (even if basic)
4. **Add Criterion microbenchmarks** to core crates
5. **Set up nightly benchmark job** (even if manual trigger initially)

---

## Anti-Patterns to Avoid

1. **Cherry-picking** — Only publishing favorable results
2. **Strawman configs** — Misconfiguring competitors
3. **Unrealistic workloads** — Benchmarks that don't reflect real usage
4. **Missing context** — Numbers without methodology
5. **Stale data** — Old benchmarks against new competitor versions
6. **Ignoring losses** — Pretending weaknesses don't exist
7. **Benchmark gaming** — Optimizing for benchmarks over real performance

---

## References

- [How NOT to Benchmark](https://www.brendangregg.com/blog/2018-06-30/benchmarking-checklist.html) — Brendan Gregg
- [wrk2 and Coordinated Omission](https://github.com/giltene/wrk2) — Gil Tene
- [TechEmpower Benchmarks](https://www.techempower.com/benchmarks/) — Industry standard
- [Envoy Performance](https://www.envoyproxy.io/docs/envoy/latest/faq/performance/performance) — How Envoy documents performance
- [NGINX Benchmarking Guide](https://www.nginx.com/blog/testing-the-performance-of-nginx-and-nginx-plus-web-servers/) — NGINX's approach
