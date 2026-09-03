//! Configuration for agentic protocol awareness (MCP, A2A).
//!
//! These types are the config-side mirror of `zentinel_proxy::agentic`. They
//! deliberately carry no behaviour: the proxy owns enforcement, this crate owns
//! only what an operator wrote down.

use serde::{Deserialize, Serialize};

/// What to do with a request body the proxy could not inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UninspectableBody {
    /// Refuse it.
    ///
    /// The default, because the feature exists to enforce an allowlist and a
    /// body that cannot be read cannot be checked against one. Failing open
    /// here would mean an oversized or malformed body is a way around policy.
    #[default]
    Deny,
    /// Forward it.
    Allow,
}

/// Per-route Model Context Protocol policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    /// JSON-RPC methods permitted. Empty means all.
    #[serde(default)]
    pub allowed_methods: Vec<String>,
    /// JSON-RPC methods refused, applied after `allowed_methods`.
    #[serde(default)]
    pub denied_methods: Vec<String>,
    /// Tools and resources permitted. Empty means all.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Tools and resources refused, applied after `allowed_tools`.
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// Require a protocol revision that guarantees mirrored headers match the
    /// request body.
    ///
    /// Defaults to true. With it off, a client can present
    /// `Mcp-Name: safe_tool` alongside a body calling something else, and any
    /// policy keyed on the header is bypassed. Zentinel resolves policy from
    /// the body regardless — this setting governs whether requests that
    /// *cannot* be checked are accepted at all.
    #[serde(default = "default_true")]
    pub require_validated_version: bool,
    /// Check `Mcp-Param-*` headers against the tool arguments they mirror.
    ///
    /// Defaults to true. Only headers whose suffix matches an argument name
    /// can be checked — the label comes from the tool's schema, which the proxy
    /// never sees — so keep `x-mcp-header` labels equal to their property names
    /// if you route on these headers.
    #[serde(default = "default_true")]
    pub validate_param_headers: bool,
    /// What to do with a body that cannot be inspected.
    #[serde(default)]
    pub on_uninspectable_body: UninspectableBody,
    /// Remove entries a call would be refused from `tools/list`,
    /// `resources/list` and `prompts/list` responses.
    ///
    /// Defaults to true, and does nothing at all unless a tool allow or deny
    /// list is set. Without it the advertised surface and the enforced one
    /// disagree: the proxy refuses a call to a tool the route forbids, having
    /// let the upstream advertise it. That costs a wasted round trip, spends
    /// the model's context on tools it may not call, and hands every client an
    /// inventory of the upstream's capabilities.
    ///
    /// A listing that cannot be rewritten -- compressed, oversized, or not
    /// parseable -- is refused rather than forwarded, on the grounds that
    /// forwarding it would advertise what the route forbids. Set this to false
    /// on a route where that trade is the wrong way round.
    #[serde(default = "default_true")]
    pub filter_tool_list: bool,
    /// Upstream MCP servers presented to the client as one endpoint.
    ///
    /// Empty is the ordinary case: the route forwards to its own `upstream` and
    /// nothing here applies. With entries, the route becomes a multiplexer —
    /// `tools/list` is merged across them and a call is routed by the prefix on
    /// its tool name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstreams: Vec<McpUpstream>,
    /// Key for the session token, hex-encoded, 32 bytes.
    ///
    /// Required once `upstreams` has more than one entry, because a client
    /// session then spans several upstream sessions and the mapping between
    /// them travels in the token rather than in the proxy.
    ///
    /// It must be configured rather than generated: a per-process key would
    /// rotate on every reload and drop every live session, and two gateway
    /// instances could not read each other's tokens — which is what would force
    /// session affinity back onto the load balancer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

/// One upstream MCP server behind a multiplexing route.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpUpstream {
    /// The `upstreams` entry to forward to.
    pub upstream: String,
    /// Prefix applied to every tool this upstream offers.
    ///
    /// Declared rather than derived. A tool's name is what a model reasons
    /// about, so it must not change because an unrelated upstream was added, or
    /// because this one was renamed.
    pub prefix: String,
    /// Path to send MCP requests to on this upstream.
    #[serde(default = "default_mcp_path")]
    pub path: String,
}

fn default_mcp_path() -> String {
    "/".to_string()
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            allowed_methods: Vec::new(),
            denied_methods: Vec::new(),
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            require_validated_version: true,
            validate_param_headers: true,
            on_uninspectable_body: UninspectableBody::Deny,
            filter_tool_list: true,
            upstreams: Vec::new(),
            session_key: None,
        }
    }
}

/// What to do with an A2A method the proxy does not recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UnknownMethods {
    /// Forward it.
    ///
    /// The default: A2A is young, and refusing every method added after this
    /// proxy was built would make upgrading the agents behind it require
    /// upgrading the proxy first.
    #[default]
    Allow,
    /// Refuse it.
    Deny,
}

/// Per-route Agent2Agent policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aConfig {
    /// Methods permitted. Empty means all.
    #[serde(default)]
    pub allowed_methods: Vec<String>,
    /// Methods refused, applied after `allowed_methods`.
    #[serde(default)]
    pub denied_methods: Vec<String>,
    /// What to do with methods the proxy does not recognise.
    #[serde(default)]
    pub unknown_methods: UnknownMethods,
    /// Refuse bodies that cannot be inspected.
    #[serde(default = "default_true")]
    pub deny_uninspectable_body: bool,
}

impl Default for A2aConfig {
    fn default() -> Self {
        Self {
            allowed_methods: Vec::new(),
            denied_methods: Vec::new(),
            unknown_methods: UnknownMethods::Allow,
            deny_uninspectable_body: true,
        }
    }
}

fn default_true() -> bool {
    true
}
