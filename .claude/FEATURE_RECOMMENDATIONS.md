# Sentinel Feature Recommendations
**Date:** 2026-01-05
**Current Status:** Production-ready with comprehensive feature set

## Executive Summary

Sentinel has achieved production readiness with strong fundamentals: TLS, caching, WAF, distributed rate limiting, service discovery, and observability. The remaining gaps are in **developer experience**, **advanced traffic management**, and **native transformations** that currently require custom agents.

---

## High-Impact Native Features

### 🎯 Priority 1: Traffic Management (Highest Value)

#### 1.1 Traffic Mirroring / Shadowing
**Impact:** CRITICAL for safe production rollouts
**Effort:** 2-3 weeks
**Use Case:** Deploy new service versions safely by mirroring production traffic

```kdl
route "api" {
    upstream "production"

    shadow {
        upstream "canary"
        percentage 10.0          // Mirror 10% of traffic
        ignore-responses #true   // Don't wait for shadow responses
        sample-header "X-Debug-Shadow" "true"  // Only shadow debug requests
    }
}
```

**Why native?**
- Requires access to request cloning before agent processing
- Performance-critical (zero overhead on main path)
- Needs intimate knowledge of upstream pooling

**Implementation:**
- Clone requests before proxying
- Fire-and-forget to shadow upstream (no waiting)
- Add metrics: `shadow_requests_total`, `shadow_errors_total`
- Configurable sampling (percentage, header-based, cookie-based)

---

#### 1.2 Weighted Routing / Canary Deployments
**Impact:** HIGH - Essential for gradual rollouts
**Effort:** 1-2 weeks
**Use Case:** Route 95% traffic to stable, 5% to canary

```kdl
route "api" {
    weighted-upstreams {
        upstream "stable" weight=95
        upstream "canary" weight=5
    }

    // Override based on headers
    header-routing {
        header "X-Canary-User" value="true" upstream="canary"
    }
}
```

**Why native?**
- Currently only upstream-level weighted LB, not route-level
- Needs integration with routing layer, not just LB
- Should support sticky sessions (cookie/header-based)

**Implementation:**
- Extend route matching to support multiple upstreams
- Add weighted selection at route level
- Support header/cookie overrides for manual testing
- Add metrics per upstream variant

---

#### 1.3 Request Retry with Exponential Backoff
**Impact:** MEDIUM-HIGH - Resilience for transient failures
**Effort:** 1 week
**Use Case:** Retry failed requests automatically

```kdl
route "api" {
    retry-policy {
        max-attempts 3
        backoff "exponential"  // or "linear"
        initial-delay-ms 100
        max-delay-ms 5000
        retryable-status-codes 502 503 504
        retryable-errors "connection_timeout" "connection_refused"
    }
}
```

**Why native?**
- Already has `RetryPolicy` type but limited implementation
- Needs tight integration with connection pool
- Performance-sensitive (hot path)

**Note:** Basic retry exists, needs enhancement for backoff and better error handling

---

### 🎯 Priority 2: Request/Response Transformation

#### 2.1 Header Transformations (Templates)
**Impact:** HIGH - Currently requires custom agents
**Effort:** 2 weeks
**Use Case:** Add computed headers, rewrite paths

```kdl
route "api" {
    policies {
        request-headers {
            set {
                "X-Forwarded-For" "${client_ip}"
                "X-Request-ID" "${uuid()}"
                "X-Real-IP" "${remote_addr}"
            }
            template {
                "X-User-Agent-Type" "${user_agent | classify}"
                "X-Geo-Country" "${geo_country(client_ip)}"
            }
        }
    }
}
```

**Why native?**
- Header manipulation is already present but limited
- Template engine would enable dynamic values
- Common need (90% of proxies need this)

**Implementation:**
- Add template parser (mini DSL or use existing like `tera`)
- Support variables: `${client_ip}`, `${request_header:name}`, `${uuid()}`
- Support filters: `${value | lowercase}`, `${value | hash:md5}`
- Compile templates at config load time

---

#### 2.2 Body Transformation (JSON)
**Impact:** MEDIUM - Advanced use case
**Effort:** 3-4 weeks
**Use Case:** Modify JSON request/response bodies

```kdl
route "api" {
    transform {
        request-body {
            type "json"

            // Add fields
            set {
                "$.metadata.proxy" "sentinel"
                "$.timestamp" "${now()}"
            }

            // Remove fields
            remove "$.internal_fields"

            // Rename fields
            rename {
                "$.old_name" "$.new_name"
            }
        }
    }
}
```

**Why native?**
- Agent-based is slower (serialization overhead)
- Common need for API gateways
- Can optimize with zero-copy transformations

**Warning:** This is complex and should be carefully scoped. Consider JSONPath or JQ-like syntax.

---

### 🎯 Priority 3: Developer Experience

#### 3.1 Structured Access Logs
**Impact:** HIGH - Observability gap
**Effort:** 1 week
**Use Case:** Detailed request/response logging

```kdl
observability {
    access-logs {
        enabled #true
        format "json"  // or "apache", "nginx"
        output "/var/log/sentinel/access.log"

        fields {
            timestamp #true
            client_ip #true
            method #true
            path #true
            status #true
            latency_ms #true
            bytes_sent #true
            user_agent #true
            referer #true
            correlation_id #true
            upstream #true
            route_id #true
        }

        // Sampling for high-traffic
        sample-rate 0.1  // Log 10% of requests
        sample-status-codes 400 401 403 404 500 502 503 504  // Always log errors
    }
}
```

**Why native?**
- Audit logs exist but focus on security events
- Access logs are standard for ops/debugging
- Should be fast (structured, buffered writes)

**Implementation:**
- Add access log writer with rotation
- Support JSON and standard formats
- Add sampling to reduce volume
- Integrate with existing correlation ID system

---

#### 3.2 Response Compression
**Impact:** MEDIUM - Performance improvement
**Effort:** 1-2 weeks
**Use Case:** Compress responses to save bandwidth

```kdl
route "api" {
    compression {
        enabled #true
        encodings "gzip" "brotli"
        min-size 1024  // Don't compress < 1KB
        types "application/json" "text/html" "text/css" "application/javascript"
        level 6  // Compression level (1-9)
    }
}
```

**Why native?**
- Decompression already exists
- Common performance optimization (40-60% bandwidth reduction)
- Pingora likely has primitives for this

**Note:** Check if Pingora has built-in compression support before implementing from scratch.

---

#### 3.3 Configuration Validation & Linting
**Impact:** MEDIUM - Prevents production issues
**Effort:** 1 week
**Use Case:** Validate configs before deployment

```bash
# Validate configuration
sentinel validate -c config.kdl

# Lint for best practices
sentinel lint -c config.kdl
  Warning: route 'api' has no retry policy (recommended for production)
  Warning: upstream 'backend' has no health check
  Error: TLS certificate '/etc/certs/missing.crt' not found

# Dry-run mode
sentinel --dry-run -c config.kdl
  ✓ Configuration loaded successfully
  ✓ All upstreams reachable
  ✓ All certificates valid
  ✓ All agents connectable
```

**Why native?**
- Config errors are the #1 cause of outages
- Better UX than cryptic error messages
- Can validate upstreams, certificates, agents before starting

**Implementation:**
- Add `sentinel validate` subcommand
- Check file paths exist
- Validate certificate expiry
- Test upstream connectivity
- Check agent socket availability
- Add warnings for missing best practices

---

### 🎯 Priority 4: Protocol Support

#### 4.1 gRPC-Native Proxying
**Impact:** MEDIUM - Growing importance
**Effort:** 2-3 weeks
**Use Case:** Proxy gRPC services with proto-aware routing

```kdl
route "grpc-api" {
    protocol "grpc"

    matches {
        grpc-service "user.UserService"
        grpc-method "GetUser"
    }

    upstream "user-service"

    grpc {
        timeout-ms 5000
        max-message-size 4194304  // 4MB
        enable-compression #true
        metadata-headers "authorization" "x-trace-id"
    }
}
```

**Why native?**
- gRPC is increasingly common in microservices
- Needs HTTP/2 frame awareness
- Can optimize connection pooling for gRPC

**Implementation:**
- Parse gRPC frames (HTTP/2 with specific headers)
- Extract service/method from `:path` header
- Support gRPC-specific load balancing (affinity)
- Handle gRPC status codes properly

---

#### 4.2 gRPC-Web Support
**Impact:** LOW-MEDIUM - Browser compatibility
**Effort:** 1 week
**Use Case:** Allow browsers to call gRPC services

```kdl
route "grpc-web" {
    protocol "grpc-web"
    upstream "grpc-backend"

    grpc-web {
        enable-cors #true
        allowed-origins "https://app.example.com"
    }
}
```

**Why native?**
- Simple protocol translation (HTTP/1.1 → HTTP/2)
- Growing use case for web apps
- Can reuse gRPC infrastructure

---

### 🎯 Priority 5: Security Enhancements

#### 5.1 IP-Based Access Control
**Impact:** MEDIUM - Basic security need
**Effort:** 1 week
**Use Case:** Allow/deny based on client IP

```kdl
route "admin" {
    ip-acl {
        mode "allowlist"  // or "denylist"
        allow-cidrs "10.0.0.0/8" "172.16.0.0/12" "192.168.0.0/16"
        allow-ips "203.0.113.42"
        deny-ips "198.51.100.100"
    }
}
```

**Why native?**
- Geo filtering exists but not IP-specific allowlist/denylist
- Simple security primitive
- Fast (hash table lookup)

**Implementation:**
- Use `ipnetwork` crate for CIDR matching
- Cache results in session context
- Add metrics: `ip_acl_allowed`, `ip_acl_denied`

---

#### 5.2 Native JWT Validation
**Impact:** LOW-MEDIUM - Currently agent-based works fine
**Effort:** 2 weeks
**Use Case:** Validate JWTs at the edge

```kdl
route "api" {
    jwt {
        enabled #true
        issuer "https://auth.example.com"
        audience "api.example.com"
        jwks-url "https://auth.example.com/.well-known/jwks.json"
        required-claims "sub" "exp" "iat"

        // Extract claims to headers
        claim-headers {
            "X-User-ID" "sub"
            "X-User-Email" "email"
        }
    }
}
```

**Why maybe NOT native:**
- Agent-based JWT validation works well
- Keeps core focused
- Less code in hot path

**Recommendation:** Keep as agent unless performance is critical

---

## Effort vs Impact Matrix

```
High Impact, Low Effort:
├─ Traffic Mirroring (2-3 weeks)
├─ Structured Access Logs (1 week)
├─ IP-Based ACL (1 week)
├─ Response Compression (1-2 weeks)
└─ Config Validation CLI (1 week)

High Impact, Medium Effort:
├─ Weighted Routing (1-2 weeks)
├─ Header Templates (2 weeks)
└─ gRPC Proxying (2-3 weeks)

Medium Impact:
├─ Retry with Backoff (1 week)
├─ Body Transformation (3-4 weeks)
└─ gRPC-Web (1 week)
```

---

## Recommended Roadmap

### Phase 1: Traffic Management (4-6 weeks)
1. **Traffic Mirroring** - Most requested, highest value
2. **Weighted Routing** - Complete canary deployment story
3. **Retry with Backoff** - Improve existing retry implementation

### Phase 2: Developer Experience (3-4 weeks)
4. **Structured Access Logs** - Critical ops gap
5. **Config Validation CLI** - Prevent production issues
6. **Response Compression** - Easy performance win

### Phase 3: Advanced Features (4-6 weeks)
7. **Header Templates** - Reduce need for custom agents
8. **gRPC Proxying** - Future-proof for microservices
9. **IP-Based ACL** - Common security primitive

### Phase 4: Optional Enhancements
10. **Body Transformation** - Complex, high effort
11. **gRPC-Web** - Niche but growing
12. **JWT Validation** - Keep as agent for now

---

## What NOT to Build (Keep as Agents)

1. **WAF** - Agent architecture is perfect for this
2. **Authentication** - Too many variants, agent flexibility needed
3. **Custom Business Logic** - This is why agents exist
4. **Machine Learning** - Python agents are better suited
5. **Rate Limiting** - Already has excellent native + distributed implementation

---

## Technical Debt to Address First

Before adding new features, consider addressing:

1. **Hardcoded Pool Sizes** - Make configurable (low effort, prevents tuning issues)
2. **Async Pool Shutdown** - Bound spawned tasks (prevents resource leaks)
3. **Error Message Quality** - Better config validation errors
4. **Documentation Gaps** - Ensure all features are documented

---

## My Top 3 Recommendations

If I had to pick only 3 features to implement next:

### 🥇 1. Traffic Mirroring
- **Why:** Critical for safe production deployments
- **Impact:** Enables zero-risk canary testing
- **Effort:** 2-3 weeks
- **Dependencies:** None

### 🥈 2. Structured Access Logs
- **Why:** Huge observability gap (only audit logs exist now)
- **Impact:** Enables debugging, analytics, compliance
- **Effort:** 1 week
- **Dependencies:** None

### 🥉 3. Config Validation CLI
- **Why:** Prevents 80% of production issues
- **Impact:** Better UX, fewer outages
- **Effort:** 1 week
- **Dependencies:** None

---

## Conclusion

Sentinel is already production-ready with a strong foundation. The remaining features fall into three categories:

1. **Traffic Management** - Enable advanced deployment patterns
2. **Developer Experience** - Make operations smoother
3. **Protocol Support** - Future-proof for gRPC/modern stacks

**The highest ROI features are those that:**
- Reduce operational burden (access logs, config validation)
- Enable safer deployments (mirroring, weighted routing)
- Solve common problems without agents (compression, IP ACL)

**Avoid building:**
- Features that agents handle better (authentication, WAF)
- Overly complex transformations (keep it simple)
- Niche features with limited demand

Focus on making Sentinel **the safest, most observable, and easiest-to-operate reverse proxy** rather than the most feature-complete one.
