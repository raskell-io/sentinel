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
