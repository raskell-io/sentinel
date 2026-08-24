//! Native awareness of agentic protocols.
//!
//! MCP and A2A are both JSON-RPC 2.0 over HTTP POST, with SSE for streaming
//! responses. They differ in what a proxy can trust:
//!
//! - **MCP** mirrors `method` and the tool/resource name into HTTP headers
//!   expressly so intermediaries can route without reading the body. That is
//!   convenient and, taken at face value, unsound — see [`mcp`].
//! - **A2A** carries no such mirror, so the method is only in the body. There
//!   is nothing to disagree with, and correspondingly nothing to spoof.
//!
//! # Why this is in the proxy rather than an agent
//!
//! Zentinel's architecture puts complex logic in external agents, and that is
//! the right default. These checks are the exception for one reason: they are
//! decisions about whether to forward a request at all, made from data the
//! proxy already has in hand. Round-tripping every tool call to an agent to ask
//! "does this header match this body" would add a network hop to a string
//! comparison, and would put the enforcement of a security control behind a
//! failure mode (agent unavailable) that the control exists to survive.
//!
//! Anything that needs to reason about tool *arguments* — prompt injection in a
//! parameter, PII in a payload — remains agent work. This module decides
//! whether a request is coherent and permitted; agents decide whether its
//! contents are safe.

pub mod a2a;
pub mod jsonrpc;
pub mod mcp;

use zentinel_config::agentic::{
    A2aConfig, McpConfig, UninspectableBody as ConfigUninspectable, UnknownMethods as ConfigUnknown,
};
use zentinel_config::RouteConfig;

/// What a route's agentic policy decided about a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Forward it, carrying what was resolved **from the body** for audit.
    Allow {
        /// MCP method, if this route carries an `mcp` block.
        mcp_method: Option<String>,
        /// MCP tool or resource.
        mcp_target: Option<String>,
        /// A2A method, if this route carries an `a2a` block.
        a2a_method: Option<String>,
    },
    /// Refuse it.
    Deny {
        /// Written to be read by an operator, not parsed.
        reason: String,
        /// Metrics label: `mcp_policy` or `a2a_policy`.
        kind: &'static str,
    },
}

/// Apply a route's MCP and A2A policy to a request.
///
/// Returns `None` when the route declares neither, which is the common case and
/// must stay free.
///
/// This is deliberately a function of [`RouteConfig`] rather than of the
/// already-converted policy types. The gap that made this module dead code for
/// a release was between configuration and enforcement, so that is the boundary
/// worth testing across.
pub fn decide(route: &RouteConfig, headers: &[(String, String)], body: &[u8]) -> Option<Outcome> {
    if route.mcp.is_none() && route.a2a.is_none() {
        return None;
    }

    let mut mcp_method = None;
    let mut mcp_target = None;
    let mut a2a_method = None;

    if let Some(cfg) = route.mcp.as_ref() {
        match mcp::evaluate(&mcp::Policy::from(cfg), headers, body) {
            mcp::Decision::Allow { method, target } => {
                mcp_method = method;
                mcp_target = target;
            }
            mcp::Decision::Deny { reason } => {
                return Some(Outcome::Deny {
                    reason: reason.to_string(),
                    kind: "mcp_policy",
                })
            }
        }
    }

    if let Some(cfg) = route.a2a.as_ref() {
        match a2a::evaluate(&a2a::Policy::from(cfg), body) {
            a2a::Decision::Allow { method } => a2a_method = method,
            a2a::Decision::Deny { reason } => {
                return Some(Outcome::Deny {
                    reason: reason.to_string(),
                    kind: "a2a_policy",
                })
            }
        }
    }

    Some(Outcome::Allow {
        mcp_method,
        mcp_target,
        a2a_method,
    })
}

impl From<&McpConfig> for mcp::Policy {
    fn from(c: &McpConfig) -> Self {
        Self {
            allowed_methods: c.allowed_methods.iter().cloned().collect(),
            denied_methods: c.denied_methods.iter().cloned().collect(),
            allowed_targets: c.allowed_tools.iter().cloned().collect(),
            denied_targets: c.denied_tools.iter().cloned().collect(),
            require_validated_version: c.require_validated_version,
            validate_param_headers: c.validate_param_headers,
            on_uninspectable: match c.on_uninspectable_body {
                ConfigUninspectable::Deny => mcp::UninspectableBody::Deny,
                ConfigUninspectable::Allow => mcp::UninspectableBody::Allow,
            },
        }
    }
}

impl From<&A2aConfig> for a2a::Policy {
    fn from(c: &A2aConfig) -> Self {
        Self {
            allowed_methods: c.allowed_methods.iter().cloned().collect(),
            denied_methods: c.denied_methods.iter().cloned().collect(),
            unknown_methods: match c.unknown_methods {
                ConfigUnknown::Allow => a2a::UnknownMethods::Allow,
                ConfigUnknown::Deny => a2a::UnknownMethods::Deny,
            },
            deny_uninspectable: c.deny_uninspectable_body,
        }
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::*;

    /// Every field must cross the config boundary. A policy that silently keeps
    /// a default here is the same defect this module exists to prevent, one
    /// layer up: the config would say "deny" and the proxy would allow.
    #[test]
    fn mcp_config_crosses_the_boundary_with_non_default_values() {
        let cfg = McpConfig {
            allowed_methods: vec!["tools/call".into()],
            denied_methods: vec!["resources/read".into()],
            allowed_tools: vec!["get_weather".into()],
            denied_tools: vec!["execute_sql".into()],
            require_validated_version: false,
            validate_param_headers: false,
            on_uninspectable_body: ConfigUninspectable::Allow,
        };
        let p = mcp::Policy::from(&cfg);

        assert!(p.allowed_methods.contains("tools/call"));
        assert!(p.denied_methods.contains("resources/read"));
        assert!(p.allowed_targets.contains("get_weather"));
        assert!(p.denied_targets.contains("execute_sql"));
        // Both of these default to true; asserting the false case is the point.
        assert!(!p.require_validated_version);
        assert!(!p.validate_param_headers);
        assert_eq!(p.on_uninspectable, mcp::UninspectableBody::Allow);
    }

    #[test]
    fn a2a_config_crosses_the_boundary_with_non_default_values() {
        let cfg = A2aConfig {
            allowed_methods: vec!["GetTask".into()],
            denied_methods: vec!["SendMessage".into()],
            unknown_methods: ConfigUnknown::Deny,
            deny_uninspectable_body: false,
        };
        let p = a2a::Policy::from(&cfg);

        assert!(p.allowed_methods.contains("GetTask"));
        assert!(p.denied_methods.contains("SendMessage"));
        assert_eq!(p.unknown_methods, a2a::UnknownMethods::Deny);
        assert!(!p.deny_uninspectable);
    }
}
