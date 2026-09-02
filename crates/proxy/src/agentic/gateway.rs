//! Talking MCP to an upstream on the gateway's own behalf.
//!
//! A multiplexing route cannot be served by forwarding. `tools/list` has to ask
//! every upstream and merge what comes back, which is N requests where proxying
//! makes one; and the merged answer is something the gateway composes rather
//! than relays. So for these routes Zentinel stops forwarding and originates —
//! the same inversion [#443] identifies for serving MCP generally, arriving
//! early and in a much narrower form.
//!
//! **What that costs, stated plainly:** these requests do not use Pingora's
//! connection pool, its retry policy, or its per-peer TLS settings. They *do*
//! still go through the route's `UpstreamPool`, so load balancing, service
//! discovery and health checking all apply — a fan-out reaches the same targets
//! a proxied request would. Putting tool calls back on the proxy path, and
//! keeping only the listing fan-out here, is a worthwhile follow-up; it needs
//! body-driven peer selection, which is a larger change than this.
//!
//! [#443]: https://github.com/zentinelproxy/zentinel/issues/443

use std::time::Duration;

use serde_json::Value;

/// Protocol revision the gateway announces upstream.
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// Header carrying a session, in the casing the spec uses.
pub const SESSION_HEADER: &str = "Mcp-Session-Id";

/// How long any single upstream call may take.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(10);

/// Largest upstream response the gateway will read.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// What an upstream said.
pub struct UpstreamReply {
    /// The JSON-RPC document.
    pub doc: Value,
    /// A session ID the upstream issued, if it did.
    pub session: Option<String>,
}

/// Why a call to an upstream did not produce a usable reply.
#[derive(Debug)]
pub enum GatewayError {
    /// The upstream could not be reached or did not answer in time.
    Unreachable(String),
    /// The upstream answered, but not with a JSON-RPC document.
    Malformed(String),
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) => write!(f, "upstream unreachable: {e}"),
            Self::Malformed(e) => write!(f, "upstream reply not usable: {e}"),
        }
    }
}

impl std::error::Error for GatewayError {}

/// The HTTP client used for gateway-originated MCP calls.
///
/// One client, built once: it owns a connection pool, and building one per
/// request would open a fresh connection to every upstream on every fan-out.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

/// Send one JSON-RPC message to an upstream and read its reply.
pub async fn call(
    url: &str,
    body: &Value,
    session: Option<&str>,
) -> Result<UpstreamReply, GatewayError> {
    let mut req = client()
        .post(url)
        .header("content-type", "application/json")
        // Both framings, because a Streamable HTTP server may answer either and
        // a gateway that accepted only one would fail against conforming
        // servers.
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", PROTOCOL_VERSION);

    if let Some(s) = session {
        req = req.header(SESSION_HEADER, s);
    }

    let resp = req
        .json(body)
        .send()
        .await
        .map_err(|e| GatewayError::Unreachable(e.to_string()))?;

    let status = resp.status();
    let issued = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let text = resp
        .text()
        .await
        .map_err(|e| GatewayError::Unreachable(e.to_string()))?;

    if !status.is_success() {
        return Err(GatewayError::Unreachable(format!("HTTP {status}")));
    }
    if text.len() > MAX_RESPONSE_BYTES {
        return Err(GatewayError::Malformed(format!(
            "reply is {} bytes, over the {MAX_RESPONSE_BYTES} limit",
            text.len()
        )));
    }

    let doc = parse_reply(&text)?;
    Ok(UpstreamReply {
        doc,
        session: issued,
    })
}

/// Parse a reply that may be plain JSON or a Streamable HTTP event.
pub fn parse_reply(text: &str) -> Result<Value, GatewayError> {
    let payload = if text.contains("data:") {
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(|l| l.strip_prefix(' ').unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text.to_string()
    };

    serde_json::from_str(payload.trim()).map_err(|e| {
        let preview: String = payload.trim().chars().take(200).collect();
        GatewayError::Malformed(format!("{e}: {preview}"))
    })
}

/// The `initialize` message the gateway sends to open an upstream session.
pub fn initialize_body() -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": "zentinel-init",
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "zentinel-gateway", "version": "1.0" },
        },
    })
}

/// A JSON-RPC error response, for telling the client something the gateway
/// itself decided.
pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_json_reply_parses() {
        let doc = parse_reply(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).expect("parsed");
        assert_eq!(doc["id"], 1);
    }

    /// Streamable HTTP wraps the reply in an event; a gateway that only read
    /// `application/json` would fail against a conforming server.
    #[test]
    fn an_event_stream_reply_parses() {
        let doc =
            parse_reply("event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n")
                .expect("parsed");
        assert_eq!(doc["id"], 1);
    }

    #[test]
    fn a_multi_line_data_payload_is_reassembled() {
        let doc = parse_reply("data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":7}\n\n").expect("parsed");
        assert_eq!(doc["id"], 7);
    }

    /// The error names what came back, truncated, because "not JSON" alone
    /// sends an operator to the wrong place.
    #[test]
    fn a_non_json_reply_is_reported_with_a_preview() {
        let err = parse_reply("<html>gateway timeout</html>").expect_err("should fail");
        assert!(format!("{err}").contains("<html>"), "{err}");
    }

    #[test]
    fn the_initialize_body_announces_the_protocol_version() {
        let b = initialize_body();
        assert_eq!(b["method"], "initialize");
        assert_eq!(b["params"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn an_error_response_carries_the_requests_id() {
        let r = error_response(&Value::from("req-3"), -32601, "no such tool");
        assert_eq!(r["id"], "req-3");
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(r["error"]["message"], "no such tool");
    }
}
