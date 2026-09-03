//! Presenting several MCP upstreams to a client as one server.
//!
//! A multiplexing route answers `tools/list` by asking each upstream and
//! merging what comes back, with every tool name carrying its upstream's
//! prefix. A `tools/call` then arrives naming one of those prefixed tools, and
//! is routed to the upstream that prefix belongs to with the prefix stripped.
//!
//! The pieces this composes are deliberately separate and separately tested:
//! [`super::namespace`] owns the name mapping, [`super::session`] owns the
//! per-client session state, and [`super::listing`] owns filtering a listing to
//! what a route permits. This module is the part that knows they go together.
//!
//! # Order of operations, and why it is this way
//!
//! Merge and namespace **before** filtering. The route's `allow`/`deny` lists
//! name tools as a client sees them — `docs.search`, not `search` — because
//! that is the only name an operator can write down when two upstreams both
//! offer `search`. Filtering first would apply the list to bare names, and a
//! rule meant for one upstream would silently apply to every upstream offering
//! the same tool.

use std::collections::BTreeMap;

use serde_json::Value;
use zentinel_config::agentic::{McpConfig, McpUpstream};

use super::namespace;
use super::session::{SessionCodec, SessionError};

/// A route's multiplexing configuration, once validated.
///
/// Its [`std::fmt::Debug`] omits the codec, which holds key material.
pub struct Multiplexer {
    upstreams: Vec<McpUpstream>,
    codec: SessionCodec,
}

impl std::fmt::Debug for Multiplexer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Multiplexer")
            .field("upstreams", &self.upstreams)
            .finish_non_exhaustive()
    }
}

/// Why a route's multiplexing configuration cannot be used.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// More than one upstream but no `session-key`.
    MissingSessionKey,
    /// The key was not 64 hex characters.
    BadSessionKey(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSessionKey => write!(
                f,
                "an mcp block with more than one upstream needs `session-key`, because a client \
                 session then spans several upstream sessions"
            ),
            Self::BadSessionKey(why) => write!(f, "mcp session-key is unusable: {why}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Multiplexer {
    /// Build a multiplexer from a route's `mcp` block, or `None` when the route
    /// does not multiplex.
    ///
    /// A single upstream needs no session token: there is exactly one upstream
    /// session and the client's own `Mcp-Session-Id` can be passed straight
    /// through. The key only becomes necessary at two.
    pub fn from_config(cfg: &McpConfig) -> Result<Option<Self>, ConfigError> {
        if cfg.upstreams.len() < 2 {
            return Ok(None);
        }

        let key_hex = cfg
            .session_key
            .as_deref()
            .ok_or(ConfigError::MissingSessionKey)?;
        let key = decode_key(key_hex)?;
        let codec =
            SessionCodec::new(&key).map_err(|e| ConfigError::BadSessionKey(e.to_string()))?;

        Ok(Some(Self {
            upstreams: cfg.upstreams.clone(),
            codec,
        }))
    }

    /// The upstreams this route fans out to.
    pub fn upstreams(&self) -> &[McpUpstream] {
        &self.upstreams
    }

    /// The configured prefixes, in declaration order.
    pub fn prefixes(&self) -> Vec<String> {
        self.upstreams.iter().map(|u| u.prefix.clone()).collect()
    }

    /// Which upstream a call to `qualified` belongs to, and its name there.
    pub fn route(&self, qualified: &str) -> Option<(&McpUpstream, String)> {
        let (prefix, bare) = namespace::split(qualified)?;
        self.upstreams
            .iter()
            .find(|u| u.prefix == prefix)
            .map(|u| (u, bare.to_string()))
    }

    /// Read the per-upstream sessions out of a client's `Mcp-Session-Id`.
    ///
    /// A token that does not decrypt yields an empty map rather than an error:
    /// to a client that is the same as not having a session yet, which is a
    /// state MCP already requires servers to handle. Returning an error would
    /// turn a rotated key into a wall of failures rather than a wave of
    /// re-initialisations.
    pub fn sessions(&self, token: Option<&str>) -> BTreeMap<String, String> {
        token
            .and_then(|t| self.codec.decode(t).ok())
            .unwrap_or_default()
    }

    /// Build the `Mcp-Session-Id` to hand back to the client.
    pub fn token(&self, sessions: &BTreeMap<String, String>) -> Result<String, SessionError> {
        self.codec.encode(sessions)
    }
}

/// Decode a 64-character hex key into 32 bytes.
fn decode_key(hex: &str) -> Result<Vec<u8>, ConfigError> {
    if hex.len() != 64 {
        return Err(ConfigError::BadSessionKey(format!(
            "expected 64 hex characters (32 bytes), got {}",
            hex.len()
        )));
    }
    (0..64)
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| ConfigError::BadSessionKey("expected hex characters only".to_string()))
        })
        .collect()
}

/// Merge one upstream's listing into an accumulating result.
///
/// Entries are namespaced on the way in, so what the client is shown is what
/// [`Multiplexer::route`] can route back.
pub fn merge_listing(into: &mut Vec<Value>, mut entries: Vec<Value>, prefix: &str) {
    namespace::qualify_listing(&mut entries, prefix);
    into.append(&mut entries);
}

/// Remove from a merged listing the entries a call would be refused.
///
/// The route's allow/deny lists name tools as the client sees them —
/// `docs.search`, not `search` — so this runs **after** merging and
/// namespacing. Filtering each upstream's listing before the prefix was applied
/// would match rules against bare names, and a rule meant for one upstream
/// would silently apply to every upstream offering the same tool.
///
/// Returns how many entries were removed.
pub fn filter_merged(
    entries: &mut Vec<Value>,
    allowed: &std::collections::HashSet<String>,
    denied: &std::collections::HashSet<String>,
) -> usize {
    if allowed.is_empty() && denied.is_empty() {
        return 0;
    }
    let before = entries.len();
    entries.retain(|e| match e.get("name").and_then(Value::as_str) {
        Some(name) => super::mcp::permitted(name, allowed, denied),
        // Kept: the enforcer would not refuse a call it cannot name either.
        None => true,
    });
    before - entries.len()
}

/// Extract the entry array of a listing response, if it has one.
pub fn entries_of(doc: &Value, field: &str) -> Option<Vec<Value>> {
    doc.get("result")
        .and_then(|r| r.get(field))
        .and_then(Value::as_array)
        .cloned()
}

/// Build a JSON-RPC response carrying a merged listing.
pub fn listing_response(id: &Value, field: &str, entries: Vec<Value>) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { field: entries },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn upstream(name: &str, prefix: &str) -> McpUpstream {
        McpUpstream {
            upstream: name.to_string(),
            prefix: prefix.to_string(),
            path: "/".to_string(),
        }
    }

    fn cfg(upstreams: Vec<McpUpstream>, key: Option<&str>) -> McpConfig {
        McpConfig {
            upstreams,
            session_key: key.map(str::to_string),
            ..McpConfig::default()
        }
    }

    /// The ordinary route: no upstreams declared, nothing multiplexes, and no
    /// session key is demanded of anyone who is not using the feature.
    #[test]
    fn a_route_with_no_upstreams_does_not_multiplex() {
        assert!(Multiplexer::from_config(&cfg(vec![], None))
            .expect("ok")
            .is_none());
    }

    /// One upstream needs no token: there is a single upstream session and the
    /// client's own header can pass straight through.
    #[test]
    fn a_single_upstream_needs_no_session_key() {
        let c = cfg(vec![upstream("docs", "docs")], None);
        assert!(Multiplexer::from_config(&c).expect("ok").is_none());
    }

    /// Two upstreams without a key is a configuration error, not a silent
    /// fallback to something that half works.
    #[test]
    fn two_upstreams_without_a_key_are_refused() {
        let c = cfg(vec![upstream("a", "a"), upstream("b", "b")], None);
        assert_eq!(
            Multiplexer::from_config(&c).unwrap_err(),
            ConfigError::MissingSessionKey
        );
    }

    #[test]
    fn a_malformed_key_is_refused_with_the_reason() {
        let c = cfg(vec![upstream("a", "a"), upstream("b", "b")], Some("abc"));
        let err = Multiplexer::from_config(&c).unwrap_err();
        assert!(format!("{err}").contains("got 3"), "{err}");

        let c = cfg(
            vec![upstream("a", "a"), upstream("b", "b")],
            Some(&"z".repeat(64)),
        );
        let err = Multiplexer::from_config(&c).unwrap_err();
        assert!(format!("{err}").contains("hex characters only"), "{err}");
    }

    #[test]
    fn a_call_routes_by_its_prefix() {
        let c = cfg(
            vec![upstream("docs", "docs"), upstream("wh", "warehouse")],
            Some(KEY),
        );
        let m = Multiplexer::from_config(&c)
            .expect("ok")
            .expect("multiplexer");

        let (up, bare) = m.route("warehouse.query").expect("routed");
        assert_eq!(up.upstream, "wh");
        assert_eq!(bare, "query");

        assert!(m.route("unknown.thing").is_none());
        assert!(m.route("unqualified").is_none());
    }

    #[test]
    fn sessions_round_trip_through_the_token() {
        let c = cfg(
            vec![upstream("docs", "docs"), upstream("wh", "warehouse")],
            Some(KEY),
        );
        let m = Multiplexer::from_config(&c)
            .expect("ok")
            .expect("multiplexer");

        let mut s = BTreeMap::new();
        s.insert("docs".to_string(), "sess-a1".to_string());
        let token = m.token(&s).expect("encoded");
        assert_eq!(m.sessions(Some(&token)), s);
    }

    /// A client with no session, or one holding a token this gateway cannot
    /// read, is treated as not having a session yet — a state MCP already
    /// requires servers to handle. Erroring instead would turn a key rotation
    /// into a wall of failures rather than a wave of re-initialisations.
    #[test]
    fn an_absent_or_unreadable_token_reads_as_no_sessions() {
        let c = cfg(
            vec![upstream("docs", "docs"), upstream("wh", "warehouse")],
            Some(KEY),
        );
        let m = Multiplexer::from_config(&c)
            .expect("ok")
            .expect("multiplexer");

        assert!(m.sessions(None).is_empty());
        assert!(m.sessions(Some("nonsense")).is_empty());
    }

    #[test]
    fn merging_namespaces_each_upstreams_entries() {
        let mut merged = Vec::new();
        merge_listing(
            &mut merged,
            serde_json::from_str(r#"[{"name":"search"}]"#).expect("json"),
            "docs",
        );
        merge_listing(
            &mut merged,
            serde_json::from_str(r#"[{"name":"search"},{"name":"query"}]"#).expect("json"),
            "warehouse",
        );

        let names: Vec<&str> = merged
            .iter()
            .map(|e| e["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            ["docs.search", "warehouse.search", "warehouse.query"]
        );
    }

    /// The collision that motivates the whole feature: both upstreams offer
    /// `search`, and after merging the client can still reach either.
    #[test]
    fn a_collision_survives_the_merge_and_still_routes() {
        let c = cfg(
            vec![upstream("docs", "docs"), upstream("wh", "warehouse")],
            Some(KEY),
        );
        let m = Multiplexer::from_config(&c)
            .expect("ok")
            .expect("multiplexer");

        let mut merged = Vec::new();
        merge_listing(
            &mut merged,
            serde_json::from_str(r#"[{"name":"search"}]"#).expect("json"),
            "docs",
        );
        merge_listing(
            &mut merged,
            serde_json::from_str(r#"[{"name":"search"}]"#).expect("json"),
            "warehouse",
        );

        for entry in &merged {
            let advertised = entry["name"].as_str().expect("name");
            assert!(m.route(advertised).is_some(), "cannot route {advertised}");
        }
        assert_eq!(m.route("docs.search").expect("routed").0.upstream, "docs");
        assert_eq!(
            m.route("warehouse.search").expect("routed").0.upstream,
            "wh"
        );
    }

    /// The route's lists name tools as the client sees them, so filtering
    /// happens after the prefix is applied.
    #[test]
    fn a_merged_listing_is_filtered_on_namespaced_names() {
        let mut merged = Vec::new();
        merge_listing(
            &mut merged,
            serde_json::from_str(r#"[{"name":"search"},{"name":"execute_sql"}]"#).expect("json"),
            "docs",
        );
        merge_listing(
            &mut merged,
            serde_json::from_str(r#"[{"name":"execute_sql"}]"#).expect("json"),
            "warehouse",
        );

        let denied: std::collections::HashSet<String> =
            ["docs.execute_sql".to_string()].into_iter().collect();
        let hidden = filter_merged(&mut merged, &Default::default(), &denied);

        assert_eq!(hidden, 1);
        let names: Vec<&str> = merged
            .iter()
            .map(|e| e["name"].as_str().expect("name"))
            .collect();
        // Only the one that was named: the other upstream's identically-named
        // tool is untouched, which is the whole reason rules name the prefix.
        assert_eq!(names, ["docs.search", "warehouse.execute_sql"]);
    }

    #[test]
    fn a_route_naming_no_tools_filters_nothing() {
        let mut merged: Vec<Value> =
            serde_json::from_str(r#"[{"name":"docs.search"}]"#).expect("json");
        assert_eq!(
            filter_merged(&mut merged, &Default::default(), &Default::default()),
            0
        );
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn entries_are_read_out_of_a_listing_response() {
        let doc: Value =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"a"}]}}"#)
                .expect("json");
        assert_eq!(entries_of(&doc, "tools").expect("entries").len(), 1);
        assert!(entries_of(&doc, "prompts").is_none());

        let err: Value =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1}}"#).expect("json");
        assert!(entries_of(&err, "tools").is_none());
    }

    #[test]
    fn a_merged_response_keeps_the_requests_id() {
        let r = listing_response(&Value::from("req-7"), "tools", vec![]);
        assert_eq!(r["id"], "req-7");
        assert_eq!(r["jsonrpc"], "2.0");
        assert!(r["result"]["tools"].as_array().expect("array").is_empty());
    }
}
