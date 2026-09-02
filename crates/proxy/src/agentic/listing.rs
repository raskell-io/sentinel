//! Filtering what an MCP upstream advertises down to what the route permits.
//!
//! [`super::mcp`] refuses a `tools/call` naming a tool the route forbids. That
//! is enforcement, and on its own it leaves the advertised surface and the
//! enforced one disagreeing: `tools/list` still returns everything the upstream
//! offers, so a client discovers tools it will only ever be refused.
//!
//! Three consequences, in increasing order of how much they matter:
//!
//! 1. A model picks a forbidden tool, calls it, and gets an error. The round
//!    trip is wasted and the failure looks like a bug in the upstream.
//! 2. Tool descriptions are spent from the model's context window. MCP clients
//!    degrade well before a hundred tools, so a route that permits five of an
//!    upstream's eighty should be advertising five.
//! 3. The list is an inventory. Names and descriptions routinely say which
//!    internal systems exist and what they can be made to do, and a client that
//!    may call none of them still gets to read all of it.
//!
//! So this module removes from a listing response exactly the entries that
//! [`super::mcp`] would refuse a call to — using the same identity rule and the
//! same [`permitted`](super::mcp::permitted) predicate, because a filter that
//! hid a different set than the enforcer refuses would be worse than no filter
//! at all: it would make the advertised surface a *misleading* description of
//! the policy rather than merely an incomplete one.

use std::collections::HashSet;

use serde_json::Value;
use zentinel_config::agentic::McpConfig;

use super::mcp::permitted;

/// Largest listing response the proxy will buffer in order to rewrite it.
///
/// Matches [`super::jsonrpc::MAX_ENVELOPE_BYTES`]. A tool list this large is
/// already unusable by any MCP client, so the bound is a guard against an
/// upstream streaming something unbounded into proxy memory rather than a
/// limit real configurations are expected to meet.
pub const MAX_LISTING_BYTES: usize = 1024 * 1024;

/// The `result` field holding entries a route's tool policy governs, if this
/// method returns one.
///
/// `resources/templates/list` is deliberately absent. Its entries are keyed by
/// `uriTemplate`, which names a *family* of URIs rather than one, and the
/// request path never resolves a target that could be compared against it. A
/// template can only be matched by expanding it, and a filter that guessed
/// would hide entries the enforcer permits. Calls to the URIs a template
/// expands to are still checked on `resources/read`.
pub fn listed_field(method: &str) -> Option<&'static str> {
    match method {
        "tools/list" => Some("tools"),
        "resources/list" => Some("resources"),
        "prompts/list" => Some("prompts"),
        _ => None,
    }
}

/// How the upstream framed the listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// `application/json` — one JSON-RPC response.
    Json,
    /// `text/event-stream` — Streamable HTTP, the response inside `data:`.
    Sse,
}

/// A rewrite the response path has committed to.
///
/// Built once the response headers are in hand and carried until the body has
/// arrived, so the body path does not have to look the route up again.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The `result` field holding the entries.
    pub field: &'static str,
    /// How the upstream framed the response.
    pub framing: Framing,
    /// Tools and resources permitted. Empty means all.
    pub allowed: HashSet<String>,
    /// Tools and resources refused, applied after `allowed`.
    pub denied: HashSet<String>,
}

impl Plan {
    /// The rewrite a route's MCP policy calls for on a response to `method`,
    /// or `None` when it calls for none.
    ///
    /// This is the whole config-to-behaviour decision, kept here rather than in
    /// the response path so it can be tested against a parsed route. The gap
    /// between what an operator wrote and what the proxy enforced is what made
    /// the module next door dead code for a release.
    pub fn for_method(cfg: &McpConfig, method: &str, framing: Framing) -> Option<Self> {
        if !cfg.filter_tool_list {
            return None;
        }
        // A route that permits everything hides nothing, and must not pay to
        // buffer a response in order to discover that.
        if cfg.allowed_tools.is_empty() && cfg.denied_tools.is_empty() {
            return None;
        }
        Some(Self {
            field: listed_field(method)?,
            framing,
            allowed: cfg.allowed_tools.iter().cloned().collect(),
            denied: cfg.denied_tools.iter().cloned().collect(),
        })
    }
}

/// What happened to a listing body.
#[derive(Debug, PartialEq, Eq)]
pub enum Filtered {
    /// Entries were removed; carries the replacement body.
    Rewritten {
        /// The new body.
        body: Vec<u8>,
        /// How many entries were removed, for logging and metrics.
        hidden: usize,
    },
    /// Every entry is permitted, or the body carries no listing to filter.
    Unchanged,
    /// The body could not be rewritten. Written to be read by an operator.
    ///
    /// The caller refuses the response rather than forwarding it: this module
    /// exists because forwarding an unfiltered listing advertises tools the
    /// route forbids, and that is no less true when the reason is that the
    /// proxy could not parse it.
    Unfilterable(&'static str),
}

/// Remove the entries a route's tool policy would refuse a call to.
pub fn filter(
    body: &[u8],
    framing: Framing,
    field: &str,
    allowed: &HashSet<String>,
    denied: &HashSet<String>,
) -> Filtered {
    match framing {
        Framing::Json => filter_json(body, field, allowed, denied),
        Framing::Sse => filter_sse(body, field, allowed, denied),
    }
}

fn filter_json(
    body: &[u8],
    field: &str,
    allowed: &HashSet<String>,
    denied: &HashSet<String>,
) -> Filtered {
    let Ok(mut doc) = serde_json::from_slice::<Value>(body) else {
        return Filtered::Unfilterable("listing response is not valid JSON");
    };

    match filter_document(&mut doc, field, allowed, denied) {
        Removed::None => Filtered::Unchanged,
        Removed::Some(hidden) => match serde_json::to_vec(&doc) {
            Ok(body) => Filtered::Rewritten { body, hidden },
            Err(_) => Filtered::Unfilterable("filtered listing could not be re-serialised"),
        },
    }
}

/// Rewrite the `data:` payload of every event, leaving the framing alone.
///
/// A JSON-RPC response may be split across several `data:` lines, which the SSE
/// grammar joins with a newline. They are re-emitted as one line, which is the
/// same event to any conforming client.
fn filter_sse(
    body: &[u8],
    field: &str,
    allowed: &HashSet<String>,
    denied: &HashSet<String>,
) -> Filtered {
    let Ok(text) = std::str::from_utf8(body) else {
        return Filtered::Unfilterable("event stream is not valid UTF-8");
    };

    let mut out = String::with_capacity(text.len());
    let mut hidden = 0usize;
    // Whether the terminating blank line was present on the last event, so a
    // stream that ends mid-event is reproduced as it arrived.
    let ends_with_separator = text.ends_with("\n\n") || text.ends_with("\r\n\r\n");

    let events: Vec<&str> = split_events(text);
    let last = events.len().saturating_sub(1);

    for (i, event) in events.iter().enumerate() {
        let mut data = String::new();
        let mut prefix = String::new();
        let mut saw_data = false;

        for line in event.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if let Some(rest) = line.strip_prefix("data:") {
                saw_data = true;
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            } else {
                prefix.push_str(line);
                prefix.push('\n');
            }
        }

        out.push_str(&prefix);

        if saw_data {
            // A data payload that is not the listing -- a notification, a
            // keep-alive, a different response multiplexed onto the same
            // stream -- passes through untouched.
            match serde_json::from_str::<Value>(&data) {
                Ok(mut doc) => {
                    if let Removed::Some(n) = filter_document(&mut doc, field, allowed, denied) {
                        hidden += n;
                        match serde_json::to_string(&doc) {
                            Ok(s) => data = s,
                            Err(_) => {
                                return Filtered::Unfilterable(
                                    "filtered listing could not be re-serialised",
                                )
                            }
                        }
                    }
                }
                Err(_) => {
                    return Filtered::Unfilterable(
                        "event stream carries a data payload \
                                                   that is not valid JSON",
                    )
                }
            }
            out.push_str("data: ");
            out.push_str(&data);
            out.push('\n');
        }

        if i < last || ends_with_separator {
            out.push('\n');
        }
    }

    if hidden == 0 {
        Filtered::Unchanged
    } else {
        Filtered::Rewritten {
            body: out.into_bytes(),
            hidden,
        }
    }
}

/// The length of the longest prefix of `buf` that ends on an event boundary.
///
/// An SSE response to a POST is not required to close once it has carried the
/// response, so the caller cannot wait for end of stream before rewriting: a
/// server that holds the stream open for later notifications would have its
/// listing held with it. Filtering whole events as they complete costs at most
/// one event of latency and never withholds a stream that has more to say.
pub fn complete_events_len(buf: &[u8]) -> usize {
    let mut end = 0usize;
    let mut i = 0usize;
    while i < buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            i += 4;
            end = i;
        } else if buf[i..].starts_with(b"\n\n") {
            i += 2;
            end = i;
        } else {
            i += 1;
        }
    }
    end
}

/// Split an event stream on blank lines, dropping the empty trailing chunk.
fn split_events(text: &str) -> Vec<&str> {
    let mut events = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let sep = if bytes[i..].starts_with(b"\r\n\r\n") {
            4
        } else if bytes[i..].starts_with(b"\n\n") {
            2
        } else {
            i += 1;
            continue;
        };
        events.push(&text[start..i]);
        i += sep;
        start = i;
    }

    if start < text.len() {
        events.push(&text[start..]);
    }

    events
}

enum Removed {
    None,
    Some(usize),
}

/// Filter `result.<field>` in place, if this document has one.
fn filter_document(
    doc: &mut Value,
    field: &str,
    allowed: &HashSet<String>,
    denied: &HashSet<String>,
) -> Removed {
    let Some(entries) = doc
        .get_mut("result")
        .and_then(|r| r.get_mut(field))
        .and_then(Value::as_array_mut)
    else {
        return Removed::None;
    };

    let before = entries.len();
    // An entry whose identity cannot be read is kept. Hiding it would be a
    // guess, and the enforcer would not refuse a call it cannot name either.
    entries.retain(|e| match identity(e) {
        Some(name) => permitted(name, allowed, denied),
        None => true,
    });
    let hidden = before - entries.len();

    if hidden == 0 {
        Removed::None
    } else {
        Removed::Some(hidden)
    }
}

/// An entry's identity, by the rule the request path uses: `name`, then `uri`.
///
/// [`super::jsonrpc::Envelope`] resolves a call's target as `params.name`
/// falling back to `params.uri`, so a tool is identified by its name, a
/// resource by its URI, and a prompt by its name -- on both sides.
fn identity(entry: &Value) -> Option<&str> {
    entry
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| entry.get("uri").and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn tools_list(names: &[&str]) -> Vec<u8> {
        let entries: Vec<String> = names
            .iter()
            .map(|n| format!(r#"{{"name":"{n}","description":"d"}}"#))
            .collect();
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"tools":[{}]}}}}"#,
            entries.join(",")
        )
        .into_bytes()
    }

    fn names_in(body: &[u8]) -> Vec<String> {
        let doc: Value = serde_json::from_slice(body).expect("valid json");
        doc["result"]["tools"]
            .as_array()
            .expect("array")
            .iter()
            .map(|e| e["name"].as_str().expect("name").to_string())
            .collect()
    }

    #[test]
    fn only_listing_methods_carry_a_field() {
        assert_eq!(listed_field("tools/list"), Some("tools"));
        assert_eq!(listed_field("resources/list"), Some("resources"));
        assert_eq!(listed_field("prompts/list"), Some("prompts"));
        assert_eq!(listed_field("tools/call"), None);
        assert_eq!(listed_field("initialize"), None);
    }

    /// Templates are keyed by `uriTemplate`, which names a family of URIs the
    /// request path never resolves. Filtering them by exact match would hide
    /// entries the enforcer permits.
    #[test]
    fn resource_templates_are_left_alone() {
        assert_eq!(listed_field("resources/templates/list"), None);
    }

    #[test]
    fn an_allowlist_hides_everything_not_on_it() {
        let body = tools_list(&["get_weather", "execute_sql", "search_docs"]);
        let Filtered::Rewritten { body, hidden } = filter(
            &body,
            Framing::Json,
            "tools",
            &set(&["get_weather", "search_docs"]),
            &HashSet::new(),
        ) else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
        assert_eq!(names_in(&body), ["get_weather", "search_docs"]);
    }

    #[test]
    fn a_denylist_hides_what_is_on_it() {
        let body = tools_list(&["get_weather", "execute_sql"]);
        let Filtered::Rewritten { body, hidden } = filter(
            &body,
            Framing::Json,
            "tools",
            &HashSet::new(),
            &set(&["execute_sql"]),
        ) else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
        assert_eq!(names_in(&body), ["get_weather"]);
    }

    /// The whole point of the module: what it hides is what `mcp::evaluate`
    /// refuses. Deny wins over allow there, so it must win here.
    #[test]
    fn deny_beats_allow_exactly_as_the_enforcer_does() {
        let body = tools_list(&["both"]);
        let Filtered::Rewritten { hidden, .. } = filter(
            &body,
            Framing::Json,
            "tools",
            &set(&["both"]),
            &set(&["both"]),
        ) else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
        assert!(!permitted("both", &set(&["both"]), &set(&["both"])));
    }

    #[test]
    fn a_listing_with_nothing_to_hide_is_left_byte_for_byte_alone() {
        let body = tools_list(&["a", "b"]);
        assert_eq!(
            filter(
                &body,
                Framing::Json,
                "tools",
                &set(&["a", "b"]),
                &HashSet::new()
            ),
            Filtered::Unchanged
        );
    }

    /// Everything outside the entry array survives, `nextCursor` above all:
    /// dropping it would silently truncate a paginated listing.
    #[test]
    fn pagination_and_envelope_fields_survive_the_rewrite() {
        let body = br#"{"jsonrpc":"2.0","id":"abc","result":{
            "tools":[{"name":"keep"},{"name":"drop"}],"nextCursor":"page2"}}"#;
        let Filtered::Rewritten { body, .. } = filter(
            body,
            Framing::Json,
            "tools",
            &set(&["keep"]),
            &HashSet::new(),
        ) else {
            panic!("expected a rewrite");
        };
        let doc: Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(doc["id"], "abc");
        assert_eq!(doc["jsonrpc"], "2.0");
        assert_eq!(doc["result"]["nextCursor"], "page2");
        assert_eq!(names_in(&body), ["keep"]);
    }

    /// Resources are identified by `uri`, matching the fallback the request
    /// path uses when `params.name` is absent.
    #[test]
    fn resources_are_matched_on_their_uri() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"resources":[
            {"uri":"file:///public/a.txt"},{"uri":"file:///secret/b.txt"}]}}"#;
        let Filtered::Rewritten { body, hidden } = filter(
            body,
            Framing::Json,
            "resources",
            &HashSet::new(),
            &set(&["file:///secret/b.txt"]),
        ) else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
        let doc: Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(
            doc["result"]["resources"].as_array().expect("array").len(),
            1
        );
    }

    /// An entry the proxy cannot name is kept, because the enforcer would not
    /// refuse a call it cannot name either. Hiding it would be a guess.
    #[test]
    fn an_unidentifiable_entry_is_kept() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"description":"no name"}]}}"#;
        assert_eq!(
            filter(
                body,
                Framing::Json,
                "tools",
                &set(&["only_this"]),
                &HashSet::new()
            ),
            Filtered::Unchanged
        );
    }

    #[test]
    fn a_body_that_is_not_json_is_refused_rather_than_forwarded() {
        assert!(matches!(
            filter(
                b"<html>nope</html>",
                Framing::Json,
                "tools",
                &set(&["a"]),
                &HashSet::new()
            ),
            Filtered::Unfilterable(_)
        ));
    }

    /// An error response carries no `result`, so there is nothing to filter and
    /// nothing to refuse.
    #[test]
    fn an_error_response_passes_through() {
        let body = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#;
        assert_eq!(
            filter(body, Framing::Json, "tools", &set(&["a"]), &HashSet::new()),
            Filtered::Unchanged
        );
    }

    // === Streamable HTTP ===

    #[test]
    fn an_sse_listing_is_filtered_inside_its_data_field() {
        let body = b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"keep\"},{\"name\":\"drop\"}]}}\n\n";
        let Filtered::Rewritten { body, hidden } = filter(
            body,
            Framing::Sse,
            "tools",
            &set(&["keep"]),
            &HashSet::new(),
        ) else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
        let text = String::from_utf8(body).expect("utf-8");
        assert!(
            text.starts_with("event: message\n"),
            "framing kept: {text:?}"
        );
        assert!(text.ends_with("\n\n"), "event terminator kept: {text:?}");
        assert!(text.contains("keep"));
        assert!(!text.contains("drop"));
    }

    /// The grammar joins consecutive `data:` lines with a newline. They are
    /// re-emitted as one line, which is the same event to any client.
    #[test]
    fn a_data_payload_split_across_lines_is_reassembled() {
        let body = b"data: {\"jsonrpc\":\"2.0\",\"id\":1,\ndata: \"result\":{\"tools\":[{\"name\":\"drop\"}]}}\n\n";
        let Filtered::Rewritten { body, hidden } = filter(
            body,
            Framing::Sse,
            "tools",
            &set(&["keep"]),
            &HashSet::new(),
        ) else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
        let text = String::from_utf8(body).expect("utf-8");
        assert_eq!(text.matches("data:").count(), 1);
        assert!(!text.contains("drop"));
    }

    /// Keep-alive comments and unrelated events share the stream and must
    /// survive untouched.
    #[test]
    fn unrelated_events_and_comments_survive() {
        let body = b": keep-alive\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"drop\"}]}}\n\n";
        let Filtered::Rewritten { body, hidden } = filter(
            body,
            Framing::Sse,
            "tools",
            &set(&["keep"]),
            &HashSet::new(),
        ) else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
        let text = String::from_utf8(body).expect("utf-8");
        assert!(text.contains(": keep-alive"));
        assert!(text.contains("notifications/progress"));
        assert!(!text.contains("drop"));
    }

    #[test]
    fn crlf_framing_is_understood() {
        let body = b"event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"drop\"}]}}\r\n\r\n";
        let Filtered::Rewritten { hidden, .. } = filter(
            body,
            Framing::Sse,
            "tools",
            &set(&["keep"]),
            &HashSet::new(),
        ) else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
    }

    // === From configuration to a filtered response ===

    fn cfg(allow: &[&str], deny: &[&str]) -> McpConfig {
        McpConfig {
            allowed_tools: allow.iter().map(|s| s.to_string()).collect(),
            denied_tools: deny.iter().map(|s| s.to_string()).collect(),
            ..McpConfig::default()
        }
    }

    /// The route names tools, so the listing is cut down to them. This is the
    /// path a real request takes, from the config an operator wrote to the
    /// bytes the client receives.
    #[test]
    fn a_route_that_names_tools_gets_a_plan_that_hides_the_rest() {
        let c = cfg(&["get_weather"], &[]);
        let plan = Plan::for_method(&c, "tools/list", Framing::Json).expect("a plan");
        let body = tools_list(&["get_weather", "execute_sql"]);
        let Filtered::Rewritten { body, hidden } =
            filter(&body, plan.framing, plan.field, &plan.allowed, &plan.denied)
        else {
            panic!("expected a rewrite");
        };
        assert_eq!(hidden, 1);
        assert_eq!(names_in(&body), ["get_weather"]);
    }

    /// The common case, and the one that must stay free: no tool policy means
    /// no plan, so no response is ever buffered.
    #[test]
    fn a_route_naming_no_tools_is_never_buffered() {
        assert!(Plan::for_method(&McpConfig::default(), "tools/list", Framing::Json).is_none());
    }

    #[test]
    fn turning_the_setting_off_produces_no_plan() {
        let c = McpConfig {
            filter_tool_list: false,
            ..cfg(&["get_weather"], &[])
        };
        assert!(Plan::for_method(&c, "tools/list", Framing::Json).is_none());
    }

    /// A route may forbid a tool without listing what it allows.
    #[test]
    fn a_denylist_alone_is_enough_to_get_a_plan() {
        assert!(
            Plan::for_method(&cfg(&[], &["execute_sql"]), "tools/list", Framing::Json).is_some()
        );
    }

    /// Only listing methods produce a plan; a `tools/call` response is not a
    /// listing and must not be touched.
    #[test]
    fn a_non_listing_method_produces_no_plan() {
        let c = cfg(&["get_weather"], &[]);
        assert!(Plan::for_method(&c, "tools/call", Framing::Json).is_none());
        assert!(Plan::for_method(&c, "initialize", Framing::Json).is_none());
    }

    #[test]
    fn event_boundaries_are_found_for_incremental_filtering() {
        assert_eq!(complete_events_len(b"data: 1\n\ndata: 2"), 9);
        assert_eq!(complete_events_len(b"data: 1\r\n\r\ndata: 2"), 11);
        assert_eq!(complete_events_len(b"data: partial"), 0);
        assert_eq!(complete_events_len(b"a\n\nb\n\n"), 6);
    }
}
