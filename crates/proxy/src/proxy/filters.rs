//! Filter dispatch for route-level filters (Headers, Compress, CORS, Timeout, Log).
//!
//! These filters are applied per-request based on the route configuration.
//! Each filter type hooks into the appropriate phase of the request lifecycle.

use std::sync::Arc;

use pingora::http::ResponseHeader;
use pingora_proxy::Session;
use sentinel_config::{
    CompressFilter, Config, CorsFilter, Filter, FilterPhase, HeadersFilter, LogFilter,
    TimeoutFilter,
};
use tracing::{debug, trace};

use super::context::RequestContext;

/// Apply request-phase filters (CORS preflight, Timeout, Log, Headers).
///
/// Returns `Ok(true)` if a response was already sent (e.g. CORS preflight),
/// meaning the request should not continue to upstream.
pub async fn apply_request_filters(
    session: &mut Session,
    ctx: &mut RequestContext,
    config: &Config,
) -> pingora::Result<bool> {
    let route_config = match ctx.route_config.as_ref() {
        Some(rc) => Arc::clone(rc),
        None => return Ok(false),
    };

    for filter_id in &route_config.filters {
        let filter_config = match config.filters.get(filter_id) {
            Some(fc) => fc,
            None => continue,
        };

        match &filter_config.filter {
            Filter::Cors(cors) => {
                if apply_cors_preflight(session, ctx, cors).await? {
                    return Ok(true); // Preflight handled, short-circuit
                }
            }
            Filter::Timeout(timeout) => {
                apply_timeout_override(ctx, timeout);
            }
            Filter::Log(log) if log.log_request => {
                emit_request_log(ctx, log);
            }
            _ => {} // Other filter types handled in other phases
        }
    }

    Ok(false)
}

/// Apply request-phase header modifications to the upstream request.
pub fn apply_request_headers_filters(
    upstream_request: &mut pingora::http::RequestHeader,
    ctx: &RequestContext,
    config: &Config,
) {
    let route_config = match ctx.route_config.as_ref() {
        Some(rc) => rc,
        None => return,
    };

    for filter_id in &route_config.filters {
        let filter_config = match config.filters.get(filter_id) {
            Some(fc) => fc,
            None => continue,
        };

        if let Filter::Headers(h) = &filter_config.filter {
            if matches!(h.phase, FilterPhase::Request | FilterPhase::Both) {
                apply_headers_to_request(upstream_request, h, &ctx.trace_id);
            }
        }
    }
}

/// Apply response-phase filters (Headers, CORS, Compress setup, Log).
pub fn apply_response_filters(
    upstream_response: &mut ResponseHeader,
    ctx: &mut RequestContext,
    config: &Config,
) {
    let route_config = match ctx.route_config.as_ref() {
        Some(rc) => Arc::clone(rc),
        None => return,
    };

    for filter_id in &route_config.filters {
        let filter_config = match config.filters.get(filter_id) {
            Some(fc) => fc,
            None => continue,
        };

        match &filter_config.filter {
            Filter::Headers(h) => {
                if matches!(h.phase, FilterPhase::Response | FilterPhase::Both) {
                    apply_headers_to_response(upstream_response, h, &ctx.trace_id);
                }
            }
            Filter::Cors(cors) => {
                apply_cors_response_headers(upstream_response, ctx, cors);
            }
            Filter::Compress(compress) => {
                apply_compress_setup(upstream_response, ctx, compress);
            }
            Filter::Log(log) if log.log_response => {
                emit_response_log(ctx, log, upstream_response.status.as_u16());
            }
            _ => {}
        }
    }
}

// =============================================================================
// Headers Filter
// =============================================================================

fn apply_headers_to_request(
    req: &mut pingora::http::RequestHeader,
    filter: &HeadersFilter,
    trace_id: &str,
) {
    for (name, value) in &filter.set {
        req.insert_header(name.clone(), value.as_str()).ok();
    }
    for (name, value) in &filter.add {
        req.append_header(name.clone(), value.as_str()).ok();
    }
    for name in &filter.remove {
        req.remove_header(name);
    }

    trace!(
        correlation_id = %trace_id,
        set_count = filter.set.len(),
        add_count = filter.add.len(),
        remove_count = filter.remove.len(),
        "Applied headers filter to request"
    );
}

fn apply_headers_to_response(
    resp: &mut ResponseHeader,
    filter: &HeadersFilter,
    trace_id: &str,
) {
    for (name, value) in &filter.set {
        resp.insert_header(name.clone(), value.as_str()).ok();
    }
    for (name, value) in &filter.add {
        resp.append_header(name.clone(), value.as_str()).ok();
    }
    for name in &filter.remove {
        resp.remove_header(name);
    }

    trace!(
        correlation_id = %trace_id,
        set_count = filter.set.len(),
        add_count = filter.add.len(),
        remove_count = filter.remove.len(),
        "Applied headers filter to response"
    );
}

// =============================================================================
// CORS Filter
// =============================================================================

/// Handle CORS preflight (OPTIONS) requests. Returns true if handled.
async fn apply_cors_preflight(
    session: &mut Session,
    ctx: &mut RequestContext,
    cors: &CorsFilter,
) -> pingora::Result<bool> {
    let origin = match session
        .req_header()
        .headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
    {
        Some(o) => o.to_string(),
        None => return Ok(false), // No Origin header, not a CORS request
    };

    // Validate origin
    if !is_origin_allowed(&origin, &cors.allowed_origins) {
        return Ok(false); // Origin not allowed, continue normal processing
    }

    ctx.cors_origin = Some(origin.clone());

    // Check if this is a preflight OPTIONS request
    let is_preflight = session.req_header().method == http::Method::OPTIONS
        && session
            .req_header()
            .headers
            .get("access-control-request-method")
            .is_some();

    if !is_preflight {
        return Ok(false); // Not a preflight, CORS response headers applied later
    }

    debug!(
        correlation_id = %ctx.trace_id,
        origin = %origin,
        "Handling CORS preflight request"
    );

    // Build preflight response
    let mut header = ResponseHeader::build(204, None)?;
    header.insert_header("Access-Control-Allow-Origin", &origin)?;
    header.insert_header(
        "Access-Control-Allow-Methods",
        cors.allowed_methods.join(", "),
    )?;

    if !cors.allowed_headers.is_empty() {
        header.insert_header(
            "Access-Control-Allow-Headers",
            cors.allowed_headers.join(", "),
        )?;
    } else if let Some(requested) = session
        .req_header()
        .headers
        .get("access-control-request-headers")
        .and_then(|v| v.to_str().ok())
    {
        // Mirror the requested headers
        header.insert_header("Access-Control-Allow-Headers", requested)?;
    }

    if cors.allow_credentials {
        header.insert_header("Access-Control-Allow-Credentials", "true")?;
    }

    header.insert_header("Access-Control-Max-Age", cors.max_age_secs.to_string())?;
    header.insert_header("Content-Length", "0")?;

    session.write_response_header(Box::new(header), true).await?;
    Ok(true) // Preflight handled, short-circuit
}

/// Add CORS headers to a normal (non-preflight) response.
fn apply_cors_response_headers(
    resp: &mut ResponseHeader,
    ctx: &RequestContext,
    cors: &CorsFilter,
) {
    let origin = match &ctx.cors_origin {
        Some(o) => o.clone(),
        None => return, // No CORS origin matched
    };

    resp.insert_header("Access-Control-Allow-Origin", &origin).ok();

    if cors.allow_credentials {
        resp.insert_header("Access-Control-Allow-Credentials", "true")
            .ok();
    }

    if !cors.exposed_headers.is_empty() {
        resp.insert_header(
            "Access-Control-Expose-Headers",
            cors.exposed_headers.join(", "),
        )
        .ok();
    }

    // Vary header to indicate origin-dependent responses
    resp.append_header("Vary", "Origin").ok();

    trace!(
        correlation_id = %ctx.trace_id,
        origin = %origin,
        "Applied CORS response headers"
    );
}

fn is_origin_allowed(origin: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|a| a == "*" || a == origin)
}

// =============================================================================
// Compress Filter
// =============================================================================

/// Set up compression by modifying response headers.
///
/// We remove Content-Length (since compressed size differs) and add
/// Content-Encoding if the client supports it and the response is compressible.
fn apply_compress_setup(
    resp: &mut ResponseHeader,
    ctx: &mut RequestContext,
    compress: &CompressFilter,
) {
    // Check if response content type is compressible
    let content_type = resp
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let is_compressible = compress.content_types.iter().any(|ct| {
        // Match on the MIME type prefix (ignore charset/params)
        content_type.starts_with(ct.as_str())
    });

    if !is_compressible {
        return;
    }

    // Check Content-Length against min_size (if present)
    if let Some(cl) = resp
        .headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
    {
        if cl < compress.min_size {
            return;
        }
    }

    // Check if response is already encoded
    if resp.headers.get("content-encoding").is_some() {
        return;
    }

    // Mark that compression should be applied (Pingora handles actual compression
    // via its built-in compression module when downstream_compression is enabled)
    ctx.compress_enabled = true;

    trace!(
        correlation_id = %ctx.trace_id,
        content_type = %content_type,
        "Compression eligible, delegating to Pingora compression module"
    );
}

// =============================================================================
// Timeout Filter
// =============================================================================

fn apply_timeout_override(ctx: &mut RequestContext, timeout: &TimeoutFilter) {
    if let Some(connect) = timeout.connect_timeout_secs {
        ctx.filter_connect_timeout_secs = Some(connect);
    }
    if let Some(upstream) = timeout.upstream_timeout_secs {
        ctx.filter_upstream_timeout_secs = Some(upstream);
    }

    trace!(
        correlation_id = %ctx.trace_id,
        connect_timeout_secs = ?timeout.connect_timeout_secs,
        upstream_timeout_secs = ?timeout.upstream_timeout_secs,
        "Applied timeout filter overrides"
    );
}

// =============================================================================
// Log Filter
// =============================================================================

fn emit_request_log(ctx: &RequestContext, log: &LogFilter) {
    match log.level.as_str() {
        "trace" => trace!(
            correlation_id = %ctx.trace_id,
            method = %ctx.method,
            path = %ctx.path,
            client_ip = %ctx.client_ip,
            host = ?ctx.host,
            user_agent = ?ctx.user_agent,
            filter = "log",
            "Log filter: incoming request"
        ),
        "debug" => debug!(
            correlation_id = %ctx.trace_id,
            method = %ctx.method,
            path = %ctx.path,
            client_ip = %ctx.client_ip,
            host = ?ctx.host,
            user_agent = ?ctx.user_agent,
            filter = "log",
            "Log filter: incoming request"
        ),
        _ => tracing::info!(
            correlation_id = %ctx.trace_id,
            method = %ctx.method,
            path = %ctx.path,
            client_ip = %ctx.client_ip,
            host = ?ctx.host,
            user_agent = ?ctx.user_agent,
            filter = "log",
            "Log filter: incoming request"
        ),
    }
}

fn emit_response_log(ctx: &RequestContext, log: &LogFilter, status: u16) {
    let duration_ms = ctx.elapsed().as_millis();

    match log.level.as_str() {
        "trace" => trace!(
            correlation_id = %ctx.trace_id,
            status = status,
            duration_ms = duration_ms,
            response_bytes = ctx.response_bytes,
            upstream = ?ctx.upstream,
            filter = "log",
            "Log filter: response"
        ),
        "debug" => debug!(
            correlation_id = %ctx.trace_id,
            status = status,
            duration_ms = duration_ms,
            response_bytes = ctx.response_bytes,
            upstream = ?ctx.upstream,
            filter = "log",
            "Log filter: response"
        ),
        _ => tracing::info!(
            correlation_id = %ctx.trace_id,
            status = status,
            duration_ms = duration_ms,
            response_bytes = ctx.response_bytes,
            upstream = ?ctx.upstream,
            filter = "log",
            "Log filter: response"
        ),
    }
}
