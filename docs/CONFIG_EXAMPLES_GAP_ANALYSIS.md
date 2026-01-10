# Config Examples Gap Analysis

**Date**: 2026-01-10
**Comparison**: `config/examples/` (code repo) vs `sentinel.raskell.io-docs/content/examples/` (docs site)

## Summary

| Category | Count |
|----------|-------|
| In both repos (matched) | 4 |
| Code repo only (not documented) | 4 |
| Docs only (no code example) | 9 |
| Docs with outdated KDL syntax | 6+ |

---

## 1. Matched Examples (Both Repos)

| Code Example | Docs Example | Sync Status |
|--------------|--------------|-------------|
| `basic.kdl` | `simple-proxy.md` | **OUTDATED** - Docs use old target syntax |
| `api-schema-validation.kdl` | `api-validation.md` | Needs review |
| `ai-guardrails.kdl` | `configurations.md` (AI Gateway) | **OUTDATED** - Docs use old transport syntax |
| `shadow-traffic.kdl` | `traffic-mirroring.md` | Needs review |

### Syntax Issues Found in Docs

#### 1. Target Syntax (simple-proxy.md, load-balancer.md, others)
```kdl
// WRONG (in docs)
upstreams {
    upstream "backend" {
        targets {
            target {
                address "127.0.0.1:3000"
            }
        }
    }
}

// CORRECT (validated in code)
upstreams {
    upstream "backend" {
        target "127.0.0.1:3000" weight=1
    }
}
```

#### 2. Agent Transport Syntax (configurations.md, api-gateway.md)
```kdl
// WRONG (in docs)
agent "ai-gateway" {
    transport "unix_socket" {
        path "/var/run/sentinel/ai-gateway.sock"
    }
}

// CORRECT (validated in code)
agent "ai-gateway" {
    unix-socket path="/var/run/sentinel/ai-gateway.sock"
}
```

#### 3. Policy Field Names (configurations.md)
```kdl
// WRONG (in docs)
policies {
    timeout_secs 120      // underscore
    max_body_size "10MB"  // underscore
}

// CORRECT (validated in code)
policies {
    timeout-secs 120      // hyphen
    max-body-size "10MB"  // hyphen
}
```

---

## 2. Code Examples NOT in Docs

These validated examples exist in the code repo but have no corresponding documentation:

| Code Example | Topic | Priority |
|--------------|-------|----------|
| `distributed-rate-limit.kdl` | Redis/Memcached distributed rate limiting | **HIGH** |
| `http-caching.kdl` | HTTP response caching (memory/disk/hybrid) | **HIGH** |
| `namespaces.kdl` | Hierarchical namespace organization | **MEDIUM** |
| `inference-routing.kdl` | LLM token-based routing & budgets | **MEDIUM** |

### Recommended New Docs Pages

1. **distributed-rate-limit.md** - Document:
   - Redis backend configuration
   - Memcached backend configuration
   - Sliding window algorithms
   - Distributed quota management
   - Failover strategies

2. **http-caching.md** - Document:
   - Memory cache backend
   - Disk cache backend
   - Hybrid (tiered) caching
   - Cache-Control header handling
   - Cache purge APIs
   - Per-route cache policies

3. **namespaces.md** - Document:
   - Namespace isolation
   - Resource scoping (upstreams, routes, filters, agents)
   - Cross-namespace exports
   - Service definitions within namespaces
   - Hierarchical limits

4. **inference-routing.md** - Document:
   - Token-based rate limiting
   - Token budget management
   - Model routing rules
   - Cost attribution
   - Provider-specific configurations

---

## 3. Docs Examples WITHOUT Code Examples

These docs exist but have no corresponding validated `.kdl` file in the code repo:

| Docs Example | Topic | Action Needed |
|--------------|-------|---------------|
| `api-gateway.md` | Full API gateway with auth | Create `api-gateway.kdl` |
| `load-balancer.md` | Load balancing algorithms | Create `load-balancer.kdl` |
| `mixed-services.md` | Microservices routing | Create `mixed-services.kdl` |
| `prometheus.md` | Prometheus metrics | Create `observability-prometheus.kdl` |
| `grafana.md` | Grafana dashboards | N/A (dashboard JSON, not KDL) |
| `static-site.md` | Static file serving | Create `static-site.kdl` |
| `tracing.md` | Distributed tracing | Create `tracing.kdl` |
| `websocket.md` | WebSocket proxying | Create `websocket.kdl` |

---

## 4. Detailed Sync Issues

### simple-proxy.md vs basic.kdl

| Aspect | Docs | Code | Status |
|--------|------|------|--------|
| Target syntax | Nested `targets { target { address } }` | Flat `target "addr" weight=N` | **OUTDATED** |
| Health check | Uses old nested syntax | Uses correct syntax | **OUTDATED** |
| Observability | Basic config | Full config with path | Minor diff |

### configurations.md (AI Gateway) vs ai-guardrails.kdl

| Aspect | Docs | Code | Status |
|--------|------|------|--------|
| Agent transport | `transport "unix_socket" { path }` | `unix-socket path=` | **OUTDATED** |
| Policy fields | `timeout_secs` (underscore) | `timeout-secs` (hyphen) | **OUTDATED** |
| Route structure | Uses `agents` attribute | Uses `filters` for agents | Needs review |

### traffic-mirroring.md vs shadow-traffic.kdl

| Aspect | Docs | Code | Status |
|--------|------|------|--------|
| Shadow block | `shadow { }` in route | Similar structure | Needs detailed review |
| Target syntax | Old nested syntax | Correct flat syntax | Likely **OUTDATED** |

---

## 5. Recommended Actions

### Immediate (High Priority)

1. **Update docs KDL syntax** - Fix all instances of:
   - [ ] Target syntax: `targets { target { address } }` → `target "addr" weight=N`
   - [ ] Agent transport: `transport "unix_socket" { path }` → `unix-socket path=`
   - [ ] Policy fields: underscores → hyphens

2. **Create missing code examples**:
   - [ ] `api-gateway.kdl`
   - [ ] `load-balancer.kdl`
   - [ ] `websocket.kdl`

### Short-term (Medium Priority)

3. **Create new docs pages**:
   - [ ] `distributed-rate-limit.md`
   - [ ] `http-caching.md`
   - [ ] `namespaces.md`

4. **Create additional code examples**:
   - [ ] `static-site.kdl`
   - [ ] `tracing.kdl`
   - [ ] `mixed-services.kdl`

### Long-term

5. **Add validation to docs CI**:
   - Extract KDL code blocks from markdown
   - Validate against sentinel-config parser
   - Fail build on invalid syntax

6. **Single source of truth**:
   - Consider embedding code examples from `config/examples/*.kdl` into docs
   - Use Hugo shortcodes or includes to pull from validated files

---

## 6. Files to Update

### Docs Repository

| File | Changes Needed |
|------|---------------|
| `simple-proxy.md` | Fix target syntax, health check syntax |
| `configurations.md` | Fix agent transport, policy field names |
| `api-gateway.md` | Fix agent transport, target syntax |
| `api-validation.md` | Review for syntax issues |
| `load-balancer.md` | Fix target syntax |
| `mixed-services.md` | Fix target syntax, agent syntax |
| `traffic-mirroring.md` | Fix target syntax |
| `websocket.md` | Review for syntax issues |
| `tracing.md` | Review for syntax issues |
| `static-site.md` | Review for syntax issues |

### Code Repository

| File | Changes Needed |
|------|---------------|
| `config/examples/api-gateway.kdl` | Create new file |
| `config/examples/load-balancer.kdl` | Create new file |
| `config/examples/websocket.kdl` | Create new file |
| `config/examples/static-site.kdl` | Create new file |
| `config/examples/tracing.kdl` | Create new file |
| `config/examples/mixed-services.kdl` | Create new file |
| `crates/config/examples/validate_examples.rs` | Add new examples to validation |
