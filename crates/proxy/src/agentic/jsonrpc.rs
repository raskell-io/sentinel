//! JSON-RPC 2.0 envelope parsing, shared by the agentic protocols.
//!
//! MCP and A2A are both JSON-RPC 2.0 carried over HTTP POST, so the parts a
//! proxy cares about — which method is being invoked, and what it is being
//! invoked on — are the same shape in both. This module extracts exactly those
//! parts and nothing else.
//!
//! # Why not deserialize the whole message
//!
//! The proxy is not an MCP or A2A implementation and should not pretend to be
//! one. Deserializing into full protocol types would mean tracking every schema
//! change in two specifications, and rejecting bodies this proxy failed to
//! model — turning a parser gap into an outage for traffic it was only meant to
//! observe.
//!
//! So the envelope is read as [`serde_json::Value`] and only the routing-
//! relevant fields are pulled out. Anything unrecognised passes through
//! untouched, which is the behaviour an intermediary should have.
//!
//! # Bounded by construction
//!
//! Parsing is refused above [`MAX_ENVELOPE_BYTES`] rather than attempted. A
//! proxy that buffers an unbounded body to inspect it has swapped a security
//! control for a memory exhaustion vector.

use serde::{Deserialize, Serialize};

/// Largest request body this module will parse.
///
/// Bodies above this are not inspected. The caller decides whether that means
/// allow or deny — see `on_unparseable` in the route policy — because the safe
/// answer differs between "observe traffic" and "enforce an allowlist".
///
/// 1 MiB is far above any realistic JSON-RPC envelope; a `tools/call` with
/// large arguments is the only thing that approaches it.
pub const MAX_ENVELOPE_BYTES: usize = 1024 * 1024;

/// The parts of a JSON-RPC 2.0 message a proxy needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// The `method` field. Absent on responses.
    pub method: Option<String>,
    /// The `id` field rendered as a string, if present.
    ///
    /// JSON-RPC allows a string, a number, or null. It is kept as a string
    /// because the proxy only ever correlates or logs it — never arithmetic.
    pub id: Option<String>,
    /// `params.name`, then `params.uri` as a fallback.
    ///
    /// MCP mirrors exactly this precedence into its `Mcp-Name` header: the
    /// tool name for `tools/call`, the resource URI for `resources/read`, the
    /// prompt name for `prompts/get`.
    pub target: Option<String>,
    /// Whether the message carried an `id`.
    ///
    /// A JSON-RPC message with a method and no id is a *notification*: it
    /// expects no response, and under MCP's Streamable HTTP transport the
    /// server answers `202 Accepted` with no body.
    pub is_notification: bool,
    /// Top-level entries of `params.arguments`, rendered as strings.
    ///
    /// MCP lets a tool schema mirror individual arguments into
    /// `Mcp-Param-{Name}` headers, which means an intermediary can route on an
    /// argument — and therefore needs to be able to check that the header and
    /// the argument still agree.
    ///
    /// Values are rendered the way the specification says clients encode them:
    /// strings as-is, integers in decimal, booleans lowercase. Anything else
    /// (objects, arrays, floats) is omitted, because the specification does not
    /// permit those to be mirrored and a rendering of them would be a guess.
    pub arguments: Vec<(String, String)>,
}

/// Why an envelope could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Body exceeded [`MAX_ENVELOPE_BYTES`] and was not parsed.
    TooLarge {
        /// Actual body size in bytes.
        size: usize,
    },
    /// Body was not valid JSON.
    NotJson,
    /// Body was valid JSON but not a JSON-RPC object (an array, a bare string).
    ///
    /// Batch requests land here: they are a JSON array, and this proxy does not
    /// take them apart. A route enforcing a policy should treat that as
    /// undecidable rather than guess.
    NotAnObject,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { size } => write!(
                f,
                "body is {size} bytes, above the {MAX_ENVELOPE_BYTES} byte inspection limit"
            ),
            Self::NotJson => write!(f, "body is not valid JSON"),
            Self::NotAnObject => write!(f, "body is not a JSON-RPC object"),
        }
    }
}

/// Read the routing-relevant fields out of a JSON-RPC request body.
///
/// # Errors
///
/// Returns [`ParseError`] when the body is too large to inspect, is not JSON,
/// or is not a JSON object. None of these are treated as protocol violations
/// here — the caller decides what an unreadable body means for its policy.
pub fn parse(body: &[u8]) -> Result<Envelope, ParseError> {
    if body.len() > MAX_ENVELOPE_BYTES {
        return Err(ParseError::TooLarge { size: body.len() });
    }

    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| ParseError::NotJson)?;

    let object = value.as_object().ok_or(ParseError::NotAnObject)?;

    let method = object
        .get("method")
        .and_then(|m| m.as_str())
        .map(str::to_string);

    // `id` may be a string or a number; both render to the same string form.
    // An explicit JSON null is *not* an id — it is the absence of one.
    let raw_id = object.get("id").filter(|id| !id.is_null());
    let id = raw_id.map(|id| match id {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    });

    let target = object
        .get("params")
        .and_then(|params| params.as_object())
        .and_then(|params| {
            params
                .get("name")
                .or_else(|| params.get("uri"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_string);

    let arguments = object
        .get("params")
        .and_then(|params| params.as_object())
        .and_then(|params| params.get("arguments"))
        .and_then(|arguments| arguments.as_object())
        .map(|arguments| {
            arguments
                .iter()
                .filter_map(|(key, value)| render_argument(value).map(|v| (key.clone(), v)))
                .collect()
        })
        .unwrap_or_default();

    Ok(Envelope {
        method,
        is_notification: raw_id.is_none(),
        id,
        target,
        arguments,
    })
}

/// Render an argument the way MCP says clients encode it into a header.
///
/// Returns `None` for types the specification does not permit to be mirrored,
/// so that an unmirror­able argument is absent rather than represented by a
/// rendering the client would never have produced — which would read as a
/// mismatch and deny a legitimate request.
fn render_argument(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        // Integers only. `number` is explicitly not permitted for mirroring,
        // and a float has no single decimal rendering both ends would agree on.
        serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_method_id_and_tool_name() {
        let body = br#"{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "get_weather", "arguments": { "location": "Seattle" } }
        }"#;
        let envelope = parse(body).expect("should parse");
        assert_eq!(envelope.method.as_deref(), Some("tools/call"));
        assert_eq!(envelope.id.as_deref(), Some("1"));
        assert_eq!(envelope.target.as_deref(), Some("get_weather"));
        assert!(!envelope.is_notification);
    }

    /// `resources/read` carries a URI where `tools/call` carries a name, and
    /// MCP mirrors whichever is present into the same header.
    #[test]
    fn falls_back_to_params_uri() {
        let body = br#"{
            "jsonrpc": "2.0",
            "id": 2,
            "method": "resources/read",
            "params": { "uri": "file:///etc/passwd" }
        }"#;
        let envelope = parse(body).expect("should parse");
        assert_eq!(envelope.target.as_deref(), Some("file:///etc/passwd"));
    }

    #[test]
    fn prefers_name_over_uri_when_both_are_present() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"m","params":{"name":"n","uri":"u"}}"#;
        assert_eq!(parse(body).unwrap().target.as_deref(), Some("n"));
    }

    #[test]
    fn a_string_id_survives_as_written() {
        let body = br#"{"jsonrpc":"2.0","id":"req-abc","method":"tools/list"}"#;
        assert_eq!(parse(body).unwrap().id.as_deref(), Some("req-abc"));
    }

    /// No id means a notification, which expects no response at all.
    #[test]
    fn a_message_without_an_id_is_a_notification() {
        let body = br#"{"jsonrpc":"2.0","method":"notifications/progress"}"#;
        let envelope = parse(body).expect("should parse");
        assert!(envelope.is_notification);
        assert_eq!(envelope.id, None);
    }

    /// An explicit null id is the absence of an id, not an id whose value is
    /// null — otherwise a notification would be mistaken for a request.
    #[test]
    fn an_explicit_null_id_is_still_a_notification() {
        let body = br#"{"jsonrpc":"2.0","id":null,"method":"notifications/progress"}"#;
        let envelope = parse(body).expect("should parse");
        assert!(envelope.is_notification);
        assert_eq!(envelope.id, None);
    }

    #[test]
    fn missing_params_is_not_an_error() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let envelope = parse(body).expect("should parse");
        assert_eq!(envelope.method.as_deref(), Some("tools/list"));
        assert_eq!(envelope.target, None);
    }

    /// Unknown fields are the normal case for a proxy that does not model the
    /// whole protocol, and must not be an error.
    #[test]
    fn unrecognised_fields_are_ignored() {
        let body = br#"{
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "t", "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } },
            "somethingEntirelyNew": { "nested": [1, 2, 3] }
        }"#;
        let envelope = parse(body).expect("unknown fields must not fail the parse");
        assert_eq!(envelope.target.as_deref(), Some("t"));
    }

    #[test]
    fn a_body_above_the_limit_is_refused_rather_than_parsed() {
        let body = vec![b'x'; MAX_ENVELOPE_BYTES + 1];
        assert_eq!(
            parse(&body),
            Err(ParseError::TooLarge {
                size: MAX_ENVELOPE_BYTES + 1
            })
        );
    }

    #[test]
    fn invalid_json_is_reported_as_such() {
        assert_eq!(parse(b"{not json"), Err(ParseError::NotJson));
    }

    /// Batch requests are a JSON array. This module does not decompose them,
    /// and saying so lets the caller apply its undecidable-body policy rather
    /// than silently inspecting only the first element.
    #[test]
    fn a_batch_array_is_reported_as_not_an_object() {
        let body = br#"[{"jsonrpc":"2.0","id":1,"method":"tools/call"}]"#;
        assert_eq!(parse(body), Err(ParseError::NotAnObject));
    }

    #[test]
    fn an_empty_body_is_not_json() {
        assert_eq!(parse(b""), Err(ParseError::NotJson));
    }
}
