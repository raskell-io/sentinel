# Sentinel Config Playground Roadmap

**Created:** 2026-01-06
**Status:** In Progress
**Target:** v0.3.0

---

## Overview

A browser-based interactive playground for Sentinel configurations that enables:
- Real-time KDL config parsing and validation with rich error messages
- Route decision simulation ("what route would match this request?")
- Upstream selection preview with load balancer behavior
- Agent hook visualization (which agents fire, in what order)

The playground runs entirely in the browser via WebAssembly, requiring no backend infrastructure.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Browser                                  │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────────┐    ┌─────────────────────────────────┐   │
│  │   Monaco Editor   │    │         Result Panel            │   │
│  │   (KDL config)    │    │  ┌─────────────────────────┐   │   │
│  │                   │    │  │ Errors / Warnings       │   │   │
│  │                   │    │  ├─────────────────────────┤   │   │
│  │                   │    │  │ Effective Config        │   │   │
│  │                   │    │  ├─────────────────────────┤   │   │
│  │                   │    │  │ Route Decision Trace    │   │   │
│  │                   │    │  └─────────────────────────┘   │   │
│  └──────────────────┘    └─────────────────────────────────┘   │
│           │                              ▲                      │
│           ▼                              │                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              sentinel-playground-wasm                     │  │
│  │  ┌────────────────┐  ┌────────────────────────────────┐  │  │
│  │  │ validate(kdl)  │  │ simulate(kdl, request) → json  │  │  │
│  │  └────────────────┘  └────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│           │                              │                      │
│           ▼                              ▼                      │
│  ┌──────────────────┐    ┌─────────────────────────────────┐   │
│  │  sentinel-config │    │         sentinel-sim            │   │
│  │ (no validation   │    │   (route matching + decision)   │   │
│  │  feature)        │    │                                 │   │
│  └──────────────────┘    └─────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

---

## Crate Structure

### 1. `sentinel-sim` (NEW)

Pure Rust crate with no async/networking dependencies. Provides:

```rust
// Core types
pub struct SimulatedRequest {
    pub method: String,
    pub host: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
}

pub struct RouteDecision {
    pub matched_route: Option<MatchedRoute>,
    pub match_trace: Vec<MatchStep>,
    pub applied_policies: AppliedPolicies,
    pub upstream_selection: Option<UpstreamSelection>,
    pub agent_hooks: Vec<AgentHook>,
    pub warnings: Vec<Warning>,
}

// Main entry point
pub fn simulate(config: &Config, request: &SimulatedRequest) -> RouteDecision;
```

**Dependencies:**
- `sentinel-config` (default-features = false)
- `sentinel-common`
- `regex` (for path matching)
- `serde` / `serde_json`

### 2. `sentinel-playground-wasm` (NEW)

Thin WASM wrapper exposing JS-friendly API:

```rust
#[wasm_bindgen]
pub fn validate(config_kdl: &str) -> JsValue;  // Returns ValidationResult as JSON

#[wasm_bindgen]
pub fn simulate(config_kdl: &str, request_json: &str) -> JsValue;  // Returns RouteDecision as JSON

#[wasm_bindgen]
pub fn get_effective_config(config_kdl: &str) -> JsValue;  // Returns normalized config
```

**Dependencies:**
- `sentinel-sim`
- `wasm-bindgen`
- `serde-wasm-bindgen`

### 3. Frontend Widget

TypeScript/React component for the docs site:

- Monaco editor with KDL syntax highlighting
- Sample request builder (method/host/path dropdowns + headers table)
- Real-time validation feedback
- Route decision visualization
- URL permalinks (config encoded in URL)

---

## Implementation Phases

### Phase 1: Core Simulation Engine ✓ COMPLETE

**Goal:** `sentinel-sim` crate with route matching

**Tasks:**
1. [x] Create `crates/sim/` directory structure
2. [x] Define `SimulatedRequest` and `RouteDecision` types
3. [x] Implement `RouteMatcher` (port from `proxy/src/routing.rs`)
   - PathPrefix matching
   - Path exact matching
   - PathRegex matching
   - Host matching
   - Header matching
   - Method matching
   - QueryParam matching
4. [x] Implement priority-based route evaluation
5. [x] Generate match trace explaining why route matched/didn't match
6. [x] Unit tests for all match conditions (24 tests passing)

**Exit Criteria:**
- ✅ `cargo test -p sentinel-sim` passes
- ✅ Can match routes against simulated requests
- ✅ Match trace explains routing decisions

### Phase 2: Policy & Upstream Simulation

**Goal:** Show what policies apply and which upstream would be selected

**Tasks:**
1. [ ] Extract applied policies from matched route
2. [ ] Simulate load balancer selection (deterministic with seed)
   - Round Robin (position-based)
   - Weighted (probability-based with seed)
   - Consistent Hash (hash of request attributes)
   - P2C (mock latency values)
3. [ ] Show health check status simulation
4. [ ] Generate warnings for misconfiguration

**Exit Criteria:**
- Can show which upstream would handle request
- Load balancer choice is deterministic and explainable

### Phase 3: Agent Hook Visualization

**Goal:** Show which agents would fire and in what order

**Tasks:**
1. [ ] Extract agent filters from route config
2. [ ] Determine hook execution order
3. [ ] Show agent timeout/circuit-breaker config
4. [ ] Indicate fail-open vs fail-closed behavior

**Exit Criteria:**
- Agent execution order is visible
- Failure modes are clearly indicated

### Phase 4: WASM Compilation ✓ COMPLETE

**Goal:** Compile simulation engine to WebAssembly

**Status:** ✅ COMPLETE

**Resolution Applied:**
- Made `uuid`, `tokio`, `sysinfo`, `tracing-subscriber` optional in `sentinel-common` behind `runtime` feature
- Feature-gated runtime modules (circuit_breaker, registry, scoped_registry, observability)
- Removed unused `jsonschema` dependency from `sentinel-config`
- Added `runtime` feature to `sentinel-config` to propagate sentinel-common runtime features
- Set `default-features = false` for sentinel-common in sentinel-config and sentinel-sim

**Tasks:**
1. [x] Create `crates/playground-wasm/` crate
2. [x] Verify `sentinel-config` compiles without `validation` feature
3. [x] Add `wasm-bindgen` exports
4. [x] Refactor sentinel-common with optional runtime deps
5. [x] Build WASM bundle with `wasm-pack`
6. [ ] Test in browser environment

**Exit Criteria:**
- ✅ `wasm-pack build` succeeds
- ✅ Can call `validate()` and `simulate()` from JavaScript
- ⚠️ Bundle size: 1.8MB (target was < 1MB, acceptable for now)

### Phase 5: Frontend Integration

**Goal:** Embed playground in docs site

**Tasks:**
1. [ ] Create React component with Monaco editor
2. [ ] Add KDL syntax highlighting
3. [ ] Implement sample request builder UI
4. [ ] Add result visualization panel
5. [ ] Implement URL permalinks
6. [ ] Integrate with Zola docs site

**Exit Criteria:**
- Playground works on docs site
- Examples from docs are pre-loaded
- URL sharing works

---

## API Design

### ValidationResult

```json
{
  "valid": false,
  "errors": [
    {
      "message": "Unknown field 'timout' in route policies, did you mean 'timeout_secs'?",
      "severity": "error",
      "location": {
        "line": 15,
        "column": 8,
        "span": [142, 149]
      },
      "hint": "Rename 'timout' to 'timeout_secs'"
    }
  ],
  "warnings": [
    {
      "message": "Route 'api' has no upstream defined",
      "severity": "warning",
      "location": { "line": 10, "column": 1 }
    }
  ],
  "effective_config": { /* normalized config with defaults applied */ }
}
```

### RouteDecision

```json
{
  "matched_route": {
    "id": "api-v2",
    "priority": 100
  },
  "match_trace": [
    {
      "route_id": "static-assets",
      "result": "no_match",
      "reason": "PathPrefix '/static' did not match '/api/v2/users'",
      "conditions_checked": 1,
      "conditions_passed": 0
    },
    {
      "route_id": "api-v2",
      "result": "match",
      "reason": "All 2 conditions matched",
      "conditions_checked": 2,
      "conditions_passed": 2,
      "condition_details": [
        { "type": "PathPrefix", "pattern": "/api/v2", "matched": true },
        { "type": "Method", "pattern": ["GET", "POST"], "matched": true }
      ]
    }
  ],
  "applied_policies": {
    "timeout_secs": 30,
    "max_body_size": "10MB",
    "failure_mode": "closed",
    "rate_limit": {
      "requests_per_second": 100,
      "burst": 200
    },
    "cache": {
      "enabled": true,
      "ttl_secs": 3600
    }
  },
  "upstream_selection": {
    "upstream_id": "api-backend",
    "selected_target": "10.0.1.5:8080",
    "load_balancer": "round_robin",
    "selection_reason": "Next in rotation (position 3 of 5)",
    "health_status": "healthy"
  },
  "agent_hooks": [
    {
      "agent_id": "rate-limiter",
      "hook": "on_request_headers",
      "timeout_ms": 100,
      "failure_mode": "open"
    },
    {
      "agent_id": "waf",
      "hook": "on_request_headers",
      "timeout_ms": 500,
      "failure_mode": "closed"
    },
    {
      "agent_id": "waf",
      "hook": "on_request_body",
      "timeout_ms": 1000,
      "failure_mode": "closed",
      "body_inspection": {
        "enabled": true,
        "max_bytes": 1048576
      }
    }
  ],
  "warnings": [
    {
      "code": "SHADOW_NO_BODY_BUFFER",
      "message": "Shadow config on POST route without buffer_body=true; request bodies won't be mirrored"
    }
  ]
}
```

---

## WASM Considerations

### Dependencies to Exclude

The following must NOT be compiled into the WASM bundle:
- `tokio` (async runtime)
- `notify` (file watching)
- `pem` / `x509-parser` (cert validation)
- Any networking crates

### Feature Flags

```toml
# sentinel-config/Cargo.toml
[features]
default = ["validation"]
validation = ["tokio", "pem", "x509-parser"]  # Disabled for WASM

# sentinel-sim/Cargo.toml
[features]
default = []
# No optional features - always WASM-compatible
```

### Bundle Size Targets

| Component | Target Size |
|-----------|-------------|
| sentinel-sim | < 500KB |
| sentinel-config (no validation) | < 300KB |
| playground-wasm total | < 1MB |

---

## Integration with Docs Site

### Embedding Strategy

The playground will be embedded as a custom Zola shortcode:

```markdown
<!-- In docs/configuration/routes.md -->

{{ playground(example="basic-routing") }}
```

This loads a pre-configured example that users can modify.

### Example Library

Pre-built examples stored in `docs/playground-examples/`:

```
playground-examples/
├── basic-routing.kdl
├── api-gateway.kdl
├── static-files.kdl
├── rate-limiting.kdl
├── waf-enabled.kdl
└── load-balancing.kdl
```

Each example includes:
- KDL config
- Sample request(s) to try
- Expected routing behavior description

---

## Success Metrics

1. **Validation Accuracy**: 100% parity with `sentinel --check`
2. **Simulation Accuracy**: Route decisions match actual proxy behavior
3. **Performance**: < 50ms for validation + simulation
4. **Bundle Size**: < 1MB WASM
5. **User Experience**: Instant feedback on config changes

---

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Route matching diverges from proxy | Share test cases, property-based testing |
| WASM bundle too large | Tree-shaking, feature flags, code splitting |
| KDL parsing differences | Use exact same `kdl` crate version |
| Complex configs timeout browser | Add iteration limits, web worker offloading |

---

## Files Reference

**New Crates:**
- `crates/sim/Cargo.toml` - Simulation crate manifest
- `crates/sim/src/lib.rs` - Main simulation logic
- `crates/sim/src/matcher.rs` - Route matching
- `crates/sim/src/types.rs` - SimulatedRequest, RouteDecision
- `crates/sim/src/trace.rs` - Match trace generation
- `crates/playground-wasm/Cargo.toml` - WASM wrapper manifest
- `crates/playground-wasm/src/lib.rs` - wasm-bindgen exports

**Existing (reference):**
- `crates/config/src/routes.rs` - Route config types (MatchCondition enum)
- `crates/proxy/src/routing.rs` - Runtime route matching (to port)
- `crates/config/Cargo.toml` - Feature flag reference

---

## Next Steps

1. **Immediate**: Create `crates/sim/` with core types
2. **This week**: Implement route matching with tests
3. **Next week**: WASM compilation verification
4. **Following**: Frontend widget prototype
