# Error Handling

Comprehensive error types with HTTP status mapping and observability.

## SentinelError

The main error type used throughout Sentinel.

```rust
use sentinel_common::{SentinelError, SentinelResult};

fn process_request() -> SentinelResult<Response> {
    // Return typed errors
    Err(SentinelError::RateLimit {
        message: "Too many requests".to_string(),
        retry_after_secs: Some(60),
    })
}
```

## Error Variants

### Configuration Errors

```rust
SentinelError::Config {
    message: String,
    source: Option<Box<dyn Error>>,
}
```

Configuration parsing or validation failures.

### Upstream Errors

```rust
SentinelError::Upstream {
    upstream: String,
    message: String,
    retryable: bool,
    source: Option<Box<dyn Error>>,
}
```

Backend connection or communication failures.

### Agent Errors

```rust
SentinelError::Agent {
    agent: String,
    message: String,
    event: Option<String>,
    source: Option<Box<dyn Error>>,
}
```

External processing agent failures.

### Validation Errors

```rust
SentinelError::RequestValidation {
    message: String,
    field: Option<String>,
}

SentinelError::ResponseValidation {
    message: String,
    field: Option<String>,
}
```

Request or response schema validation failures.

### Limit Errors

```rust
SentinelError::LimitExceeded {
    limit_type: LimitType,
    message: String,
    current_value: u64,
    limit: u64,
}

SentinelError::RateLimit {
    message: String,
    retry_after_secs: Option<u64>,
}
```

Resource limit or rate limit violations.

### Timeout Errors

```rust
SentinelError::Timeout {
    operation: String,
    duration_ms: u64,
    correlation_id: Option<String>,
}
```

Operation timeout exceeded.

### Circuit Breaker Errors

```rust
SentinelError::CircuitBreakerOpen {
    component: String,
    consecutive_failures: u32,
    last_error: Option<String>,
}
```

Circuit breaker rejected the request.

### Security Errors

```rust
SentinelError::WafBlocked {
    reason: String,
    rule_ids: Vec<String>,
    confidence: Option<f64>,
    correlation_id: Option<String>,
}

SentinelError::AuthenticationFailed {
    message: String,
}

SentinelError::AuthorizationFailed {
    message: String,
    required_permission: Option<String>,
}
```

Security policy violations.

### Infrastructure Errors

```rust
SentinelError::Tls {
    message: String,
    source: Option<Box<dyn Error>>,
}

SentinelError::Io {
    message: String,
    source: Option<Box<dyn Error>>,
}

SentinelError::Parse {
    message: String,
    input: Option<String>,
}

SentinelError::Internal {
    message: String,
}

SentinelError::ServiceUnavailable {
    message: String,
    retry_after_secs: Option<u64>,
}

SentinelError::NoHealthyUpstream {
    upstream: String,
}
```

## LimitType

Categories of resource limits.

```rust
use sentinel_common::LimitType;

let limit_type = LimitType::BodySize;
```

**Variants:**
- `HeaderSize` - Individual header too large
- `HeaderCount` - Too many headers
- `BodySize` - Request/response body too large
- `RequestRate` - Requests per second exceeded
- `ConnectionCount` - Too many connections
- `InFlightRequests` - Too many concurrent requests
- `DecompressionSize` - Decompressed content too large
- `BufferSize` - Buffer allocation limit
- `QueueDepth` - Queue depth exceeded

## HTTP Status Mapping

Errors automatically map to appropriate HTTP status codes:

```rust
let error = SentinelError::RateLimit { ... };
let status = error.to_http_status(); // 429
```

| Error Type | HTTP Status |
|------------|-------------|
| `RequestValidation`, `Parse` | 400 Bad Request |
| `AuthenticationFailed` | 401 Unauthorized |
| `WafBlocked`, `AuthorizationFailed` | 403 Forbidden |
| `LimitExceeded`, `RateLimit` | 429 Too Many Requests |
| `Upstream`, `ResponseValidation` | 502 Bad Gateway |
| `CircuitBreakerOpen`, `ServiceUnavailable`, `NoHealthyUpstream` | 503 Service Unavailable |
| `Timeout` | 504 Gateway Timeout |
| `Config`, `Agent`, `Tls`, `Io`, `Internal` | 500 Internal Server Error |

## Client-Safe Messages

Get messages safe to return to clients (no internal details):

```rust
let error = SentinelError::Internal {
    message: "Database connection failed: password incorrect".to_string(),
};

// Internal message (for logging)
println!("{}", error); // Full message with details

// Client message (safe to return)
let safe = error.client_message(); // "Internal server error"
```

## Error Properties

### Retryable Errors

Check if a request can be retried:

```rust
if error.is_retryable() {
    // Schedule retry
}
```

Retryable errors:
- `Upstream` with `retryable: true`
- `Timeout`
- `ServiceUnavailable`
- `CircuitBreakerOpen`

### Circuit Breaker Eligibility

Check if error should trigger circuit breaker:

```rust
if error.is_circuit_breaker_eligible() {
    breaker.record_failure();
}
```

Eligible errors:
- `Upstream`
- `Timeout`
- `Agent`

## Correlation IDs

Attach correlation ID for tracing:

```rust
let error = SentinelError::Timeout {
    operation: "upstream_request".to_string(),
    duration_ms: 30000,
    correlation_id: None,
};

// Add correlation ID
let error = error.with_correlation_id("abc-123-def".to_string());
```

## Error Handling Patterns

### Converting to HTTP Response

```rust
fn handle_error(error: SentinelError) -> Response {
    let status = error.to_http_status();
    let message = error.client_message();

    // Log full error
    tracing::error!(%error, "Request failed");

    // Return safe response
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(json!({ "error": message }).to_string())
        .unwrap()
}
```

### Propagating with Context

```rust
fn process() -> SentinelResult<()> {
    let config = load_config()
        .map_err(|e| SentinelError::Config {
            message: "Failed to load config".to_string(),
            source: Some(Box::new(e)),
        })?;

    Ok(())
}
```

### Metrics Integration

```rust
fn record_error(error: &SentinelError, metrics: &RequestMetrics) {
    match error {
        SentinelError::WafBlocked { reason, .. } => {
            metrics.record_blocked_request(reason);
        }
        SentinelError::RateLimit { .. } => {
            metrics.record_blocked_request("rate_limit");
        }
        SentinelError::CircuitBreakerOpen { component, .. } => {
            metrics.set_circuit_breaker_state(component, "default", true);
        }
        _ => {}
    }
}
```

## SentinelResult

Convenience type alias:

```rust
pub type SentinelResult<T> = Result<T, SentinelError>;
```

Usage:

```rust
use sentinel_common::SentinelResult;

fn process() -> SentinelResult<Response> {
    // ...
}
```
