# Sentinel Agents Roadmap

This document tracks ideas for future Sentinel agents. Agents extend Sentinel's capabilities through the [Agent Protocol](https://sentinel.raskell.io/docs/agent-protocol/).

## Architecture: Built-in vs Agent

Sentinel follows a **"boring dataplane, innovative agents"** philosophy:

**Built-in to Sentinel Core** (stable, bounded, predictable):
- Rate limiting (local + distributed)
- Load balancing (multiple algorithms)
- Circuit breakers
- Health checks + target ejection
- HTTP caching
- CORS handling
- GeoIP filtering
- TLS/mTLS termination
- Service discovery
- OpenTelemetry tracing
- Traffic mirroring
- Static file serving
- Header manipulation
- Body decompression/compression

**Agents** (out-of-process, extensible, isolated):
- Authentication & authorization (JWT, OAuth, API keys)
- WAF / attack detection
- Custom business logic
- Complex transformations
- Protocol-specific security (GraphQL, gRPC, MQTT)
- Scripting (Lua, JavaScript, WebAssembly)

This separation keeps the dataplane safe and bounded while allowing complex, potentially risky features to be isolated and independently upgraded.

---

## Current Agents

| Agent | Status | Description |
|-------|--------|-------------|
| [Auth](https://sentinel.raskell.io/agents/auth/) | Stable | JWT, API key, OAuth authentication with RBAC |
| [Denylist](https://sentinel.raskell.io/agents/denylist/) | Stable | IP, UA, header, path, query blocking |
| [WAF](https://sentinel.raskell.io/agents/waf/) | Beta | Lightweight Rust-native attack detection |
| [ModSecurity](https://sentinel.raskell.io/agents/modsec/) | Beta | Full OWASP CRS with 800+ rules |
| [AI Gateway](https://sentinel.raskell.io/agents/ai-gateway/) | Beta | LLM guardrails (input/output) |
| [WebSocket Inspector](https://sentinel.raskell.io/agents/websocket-inspector/) | Beta | WebSocket frame security |
| [Lua](https://sentinel.raskell.io/agents/lua/) | Beta | Lua scripting |
| [JavaScript](https://sentinel.raskell.io/agents/js/) | Beta | JavaScript scripting (QuickJS) |
| [WebAssembly](https://sentinel.raskell.io/agents/wasm/) | Beta | High-performance Wasm modules |
| [Bot Management](https://sentinel.raskell.io/agents/bot-management/) | Beta | Multi-signal bot detection with behavioral analysis |
| [Transform](https://sentinel.raskell.io/agents/transform/) | Beta | URL rewriting, header manipulation, JSON body transforms |
| [GraphQL Security](https://sentinel.raskell.io/agents/graphql-security/) | Beta | Query depth/complexity limiting, introspection control, field auth |
| [Audit Logger](https://sentinel.raskell.io/agents/audit-logger/) | Beta | Structured audit logging with PII redaction, SIEM formats |
| [API Deprecation](https://sentinel.raskell.io/agents/api-deprecation/) | Beta | Sunset headers, usage tracking, automatic redirects, migration support |
| [Data Masking](https://sentinel.raskell.io/agents/data-masking/) | Beta | PII tokenization, FPE, pattern-based masking for JSON/XML/form |

---

## Planned Agents

### Priority 1: High Value

#### ~~GraphQL Security~~ ✅
**Status:** Complete
**Complexity:** Medium
**Value:** High

GraphQL-specific security controls.

**Features:**
- [x] Query depth limiting
- [x] Query complexity analysis (cost calculation)
- [x] Field-level authorization
- [x] Introspection control (disable in production)
- [x] Batch query limits
- [x] Alias limits
- [x] Persisted queries / allowlist mode
- [ ] N+1 detection (future enhancement)

**Repository:** https://github.com/raskell-io/sentinel-agent-graphql-security
**Docs:** https://sentinel.raskell.io/agents/graphql-security/

---

### Priority 2: Observability

#### ~~Audit Logger~~ ✅
**Status:** Complete
**Complexity:** Low
**Value:** Medium

Structured compliance-focused audit logging.

**Features:**
- [x] Configurable log fields
- [x] Multiple output formats (JSON, CEF, LEEF)
- [x] Log shipping (file, syslog, HTTP)
- [x] PII redaction in logs
- [x] Request/response body logging (configurable)
- [x] Compliance templates (SOC2, HIPAA, PCI, GDPR)
- [x] Custom redaction patterns
- [x] Request sampling and filtering

**Repository:** https://github.com/raskell-io/sentinel-agent-audit-logger
**Docs:** https://sentinel.raskell.io/agents/audit-logger/

---

### Priority 3: Compliance & Data

#### ~~Data Masking~~ ✅
**Status:** Complete
**Complexity:** High
**Value:** Medium

PII protection and data minimization.

**Features:**
- [x] Field-level tokenization
- [x] Format-preserving encryption (FF1-style, AES-based)
- [x] Regex-based detection and masking
- [x] Header value masking
- [x] Request body field masking
- [x] Response body field masking
- [x] Reversible vs irreversible masking
- [ ] Redis token store backend (future enhancement)

**Use Cases:**
- GDPR compliance
- PCI DSS (card data protection)
- Secure logging
- Data minimization

**Repository:** https://github.com/raskell-io/sentinel (agents/data-masking)

---

### Priority 4: Protocol-Specific

#### gRPC Inspector
**Status:** Proposed
**Complexity:** High
**Value:** Medium

gRPC/Protocol Buffers security.

**Features:**
- [ ] Method-level authorization
- [ ] Message size limits
- [ ] Metadata inspection
- [ ] Rate limiting per method
- [ ] Schema validation
- [ ] Reflection control

**Use Cases:**
- gRPC API security
- Service mesh integration
- Internal API governance

---

#### MQTT Gateway
**Status:** Proposed
**Complexity:** High
**Value:** Low-Medium

IoT protocol security.

**Features:**
- [ ] Topic-based ACLs
- [ ] Payload inspection
- [ ] Client authentication
- [ ] Message rate limiting
- [ ] QoS enforcement
- [ ] Retained message control

**Use Cases:**
- IoT device management
- MQTT broker protection
- Industrial IoT security

---

### Priority 5: Developer Experience

#### ~~Mock Server~~ ✅
**Status:** Complete
**Complexity:** Low
**Value:** Low

Request matching and stub responses.

**Features:**
- [x] Request matching (path, headers, body, query params)
- [x] Static response stubs
- [x] Dynamic responses (Handlebars templates)
- [x] Latency simulation (fixed and random)
- [x] Failure injection (errors, timeouts, corruption, slow responses)
- [x] Match limits and priority matching
- [ ] Record and replay mode (future enhancement)

**Repository:** https://github.com/raskell-io/sentinel-agent-mock-server
**Docs:** https://sentinel.raskell.io/agents/mock-server/

---

#### ~~API Deprecation~~ ✅
**Status:** Complete
**Complexity:** Low
**Value:** Low

API lifecycle management.

**Features:**
- [x] Deprecation warning headers
- [x] Sunset date headers (RFC 8594)
- [x] Usage tracking for deprecated endpoints
- [x] Automatic redirects to new versions
- [x] Migration documentation links
- [ ] Gradual traffic shifting (future enhancement)

**Repository:** https://github.com/raskell-io/sentinel-agent-api-deprecation
**Docs:** https://sentinel.raskell.io/agents/api-deprecation/

---

## Rejected / Deferred Ideas

| Idea | Reason |
|------|--------|
| Rate Limiter Agent | Built-in: local (token bucket) and distributed (Redis, Memcached) rate limiting |
| Load Balancer Agent | Built-in: round-robin, least-connections, consistent hashing, P2C, adaptive |
| Cache Agent | Built-in: HTTP caching with Cache-Control, LRU eviction, stale-while-revalidate |
| Service Discovery Agent | Built-in: static, DNS, DNS SRV, Consul, Kubernetes |
| Circuit Breaker Agent | Built-in: per-upstream circuit breakers with failure thresholds, half-open state, metrics |
| OpenTelemetry Agent | Built-in: W3C trace context propagation, OTLP export, sampling configuration |
| CORS Agent | Built-in: per-route CORS with origins, methods, headers, credentials, preflight caching |
| GeoIP Agent | Built-in: MaxMind/IP2Location with allow/block modes, country header injection |
| Health Check Agent | Built-in: HTTP/TCP/gRPC health checks with target ejection |
| Retry Agent | Built-in: configurable retry policy with backoff |
| Traffic Mirroring Agent | Built-in: shadow/mirror traffic with sampling |
| Static File Agent | Built-in: file serving with directory listing, SPA support, range requests |

---

## Contributing

Want to work on one of these agents?

1. Open an issue to discuss the design
2. Check the [Agent SDK documentation](https://sentinel.raskell.io/docs/agent-sdk/)
3. Review existing agents for patterns
4. Submit a PR with implementation and docs

See [CONTRIBUTING.md](./CONTRIBUTING.md) for general contribution guidelines.
