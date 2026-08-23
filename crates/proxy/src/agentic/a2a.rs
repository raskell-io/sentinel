//! Agent2Agent protocol awareness.
//!
//! Implements per-method policy for A2A traffic against the v1.0 JSON-RPC
//! binding.
//!
//! # How this differs from MCP
//!
//! A2A defines `A2A-Version` and `A2A-Extensions` as service parameters, but
//! does not mirror the RPC method into a header. So there is no header/body
//! agreement to verify — the body is the only statement of what is being
//! invoked, and policy resolves against it directly.
//!
//! That makes A2A *simpler* to police than MCP, not harder. MCP's header
//! mirroring creates an evasion surface precisely because it gives a proxy a
//! second, cheaper source of truth that an attacker can desynchronise from the
//! first. A2A has one source of truth.
//!
//! # Agent cards
//!
//! An agent publishes a capability manifest at `/.well-known/agent-card.json`,
//! listing its skills, endpoints and accepted authentication. That is a
//! discovery document, and exposing it decides who can enumerate an agent's
//! capabilities — so it is treated as its own policy surface rather than an
//! ordinary path.

use std::collections::HashSet;

use super::jsonrpc::{self, ParseError};

/// Header carrying the protocol version.
pub const HEADER_VERSION: &str = "a2a-version";
/// Header carrying comma-separated extension URIs in use.
pub const HEADER_EXTENSIONS: &str = "a2a-extensions";

/// Well-known path for an agent's capability manifest (RFC 8615).
pub const AGENT_CARD_PATH: &str = "/.well-known/agent-card.json";

/// Registered media type for A2A content.
pub const MEDIA_TYPE: &str = "application/a2a+json";

/// Methods defined by A2A v1.0.
///
/// Held as data rather than an enum so that an unrecognised method is a value
/// this proxy can still name in a log line and a policy can still refuse. A
/// closed enum would force every spec addition to become a parse failure —
/// exactly the brittleness a proxy should not impose on traffic it forwards.
pub const KNOWN_METHODS: &[&str] = &[
    "SendMessage",
    "SendStreamingMessage",
    "GetTask",
    "ListTasks",
    "CancelTask",
    "SubscribeToTask",
    "CreateTaskPushNotificationConfig",
    "GetTaskPushNotificationConfig",
    "ListTaskPushNotificationConfigs",
    "DeleteTaskPushNotificationConfig",
    "GetExtendedAgentCard",
];

/// Methods that cause an agent to do work on the caller's behalf.
///
/// Offered as a named group so a policy can say "reading task state is fine,
/// starting new work is not" without enumerating method names — and without
/// that meaning silently drifting when the group is extended.
pub const WORK_INITIATING_METHODS: &[&str] = &["SendMessage", "SendStreamingMessage"];

/// What the proxy decided about an A2A request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Forward it.
    Allow {
        /// Method resolved from the body, for metrics and audit.
        method: Option<String>,
    },
    /// Reject it.
    Deny {
        /// Why.
        reason: DenyReason,
    },
}

/// Why an A2A request was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// The method is not permitted on this route.
    MethodNotAllowed {
        /// The method that was refused.
        method: String,
    },
    /// The method is not one this proxy knows, and the route refuses unknowns.
    UnknownMethod {
        /// The method that was refused.
        method: String,
    },
    /// The body could not be inspected and the route requires inspection.
    Unparseable {
        /// What went wrong.
        error: String,
    },
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MethodNotAllowed { method } => {
                write!(f, "A2A method {method:?} is not permitted on this route")
            }
            Self::UnknownMethod { method } => write!(
                f,
                "A2A method {method:?} is not one this proxy recognises, and this route \
                 refuses methods it cannot classify"
            ),
            Self::Unparseable { error } => write!(
                f,
                "request body could not be inspected ({error}) and this route requires \
                 inspection to apply its policy"
            ),
        }
    }
}

/// What to do with a method outside [`KNOWN_METHODS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnknownMethods {
    /// Forward it. The right default: A2A is young, and a proxy that refuses
    /// every method added after it was compiled becomes an obstacle to
    /// upgrading the agents behind it.
    #[default]
    Allow,
    /// Refuse it, for deployments that would rather fail than forward
    /// something they cannot classify.
    Deny,
}

/// Per-route A2A policy.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// Methods permitted. Empty means all.
    pub allowed_methods: HashSet<String>,
    /// Methods refused, applied after `allowed_methods`.
    pub denied_methods: HashSet<String>,
    /// What to do with methods this proxy does not recognise.
    pub unknown_methods: UnknownMethods,
    /// Whether a body that cannot be inspected is refused.
    pub deny_uninspectable: bool,
}

/// Whether a method is one this proxy recognises.
pub fn is_known_method(method: &str) -> bool {
    KNOWN_METHODS.contains(&method)
}

/// Whether a method causes the agent to begin work.
pub fn is_work_initiating(method: &str) -> bool {
    WORK_INITIATING_METHODS.contains(&method)
}

/// Whether a request path is the agent card.
///
/// Compared against the path only; a caller holding a full URI should strip
/// the query first. Trailing slashes are not accepted, because the well-known
/// path is exact.
pub fn is_agent_card_path(path: &str) -> bool {
    path == AGENT_CARD_PATH
}

/// Apply A2A policy to a request body.
pub fn evaluate(policy: &Policy, body: &[u8]) -> Decision {
    let envelope = match jsonrpc::parse(body) {
        Ok(envelope) => envelope,
        Err(error) => {
            return if policy.deny_uninspectable {
                Decision::Deny {
                    reason: DenyReason::Unparseable {
                        error: error.to_string(),
                    },
                }
            } else {
                Decision::Allow { method: None }
            };
        }
    };

    let Some(method) = envelope.method.as_deref() else {
        // No method at all is not an A2A request this proxy can classify. It
        // is left to the upstream, which owns the protocol.
        return Decision::Allow { method: None };
    };

    if policy.unknown_methods == UnknownMethods::Deny && !is_known_method(method) {
        return Decision::Deny {
            reason: DenyReason::UnknownMethod {
                method: method.to_string(),
            },
        };
    }

    let allowed = policy.allowed_methods.is_empty() || policy.allowed_methods.contains(method);
    if !allowed || policy.denied_methods.contains(method) {
        return Decision::Deny {
            reason: DenyReason::MethodNotAllowed {
                method: method.to_string(),
            },
        };
    }

    Decision::Allow {
        method: Some(method.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissive() -> Policy {
        Policy {
            allowed_methods: HashSet::new(),
            denied_methods: HashSet::new(),
            unknown_methods: UnknownMethods::Allow,
            deny_uninspectable: true,
        }
    }

    fn request(method: &str) -> Vec<u8> {
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}"}}"#).into_bytes()
    }

    #[test]
    fn a_permitted_method_is_allowed() {
        assert_eq!(
            evaluate(&permissive(), &request("GetTask")),
            Decision::Allow {
                method: Some("GetTask".to_string())
            }
        );
    }

    #[test]
    fn a_denied_method_is_refused() {
        let mut policy = permissive();
        policy.denied_methods = ["CancelTask".to_string()].into_iter().collect();
        assert!(matches!(
            evaluate(&policy, &request("CancelTask")),
            Decision::Deny {
                reason: DenyReason::MethodNotAllowed { .. }
            }
        ));
    }

    /// The read-only deployment: task state can be queried, but nothing may
    /// ask the agent to start working.
    #[test]
    fn work_initiating_methods_can_be_refused_as_a_group() {
        let mut policy = permissive();
        policy.denied_methods = WORK_INITIATING_METHODS
            .iter()
            .map(|m| (*m).to_string())
            .collect();

        for method in WORK_INITIATING_METHODS {
            assert!(
                matches!(
                    evaluate(&policy, &request(method)),
                    Decision::Deny {
                        reason: DenyReason::MethodNotAllowed { .. }
                    }
                ),
                "{method} should be refused"
            );
        }
        assert!(matches!(
            evaluate(&policy, &request("GetTask")),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn an_allowlist_refuses_everything_outside_it() {
        let mut policy = permissive();
        policy.allowed_methods = ["GetTask".to_string(), "ListTasks".to_string()]
            .into_iter()
            .collect();
        assert!(matches!(
            evaluate(&policy, &request("GetTask")),
            Decision::Allow { .. }
        ));
        assert!(matches!(
            evaluate(&policy, &request("SendMessage")),
            Decision::Deny {
                reason: DenyReason::MethodNotAllowed { .. }
            }
        ));
    }

    /// A2A is young; refusing every method added after this proxy was compiled
    /// would make the proxy an obstacle to upgrading agents behind it.
    #[test]
    fn unknown_methods_are_forwarded_by_default() {
        assert!(matches!(
            evaluate(&permissive(), &request("SomeMethodAddedLater")),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn unknown_methods_can_be_refused_deliberately() {
        let mut policy = permissive();
        policy.unknown_methods = UnknownMethods::Deny;
        assert!(matches!(
            evaluate(&policy, &request("SomeMethodAddedLater")),
            Decision::Deny {
                reason: DenyReason::UnknownMethod { .. }
            }
        ));
        assert!(matches!(
            evaluate(&policy, &request("GetTask")),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn an_uninspectable_body_is_refused_when_configured() {
        assert!(matches!(
            evaluate(&permissive(), b"{not json"),
            Decision::Deny {
                reason: DenyReason::Unparseable { .. }
            }
        ));
    }

    #[test]
    fn an_uninspectable_body_can_be_forwarded() {
        let mut policy = permissive();
        policy.deny_uninspectable = false;
        assert!(matches!(
            evaluate(&policy, b"{not json"),
            Decision::Allow { method: None }
        ));
    }

    /// A JSON-RPC response has no method. It is not this proxy's business to
    /// classify, so it passes to the upstream that owns the protocol.
    #[test]
    fn a_body_without_a_method_is_left_to_the_upstream() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        assert_eq!(
            evaluate(&permissive(), body),
            Decision::Allow { method: None }
        );
    }

    #[test]
    fn every_v1_method_is_recognised() {
        for method in KNOWN_METHODS {
            assert!(is_known_method(method), "{method} should be known");
        }
        assert!(
            !is_known_method("message/send"),
            "that is the v0.x spelling"
        );
    }

    #[test]
    fn work_initiating_methods_are_a_subset_of_known_methods() {
        for method in WORK_INITIATING_METHODS {
            assert!(
                KNOWN_METHODS.contains(method),
                "{method} is listed as work-initiating but is not a known method"
            );
        }
    }

    mod agent_card {
        use super::*;

        #[test]
        fn the_well_known_path_is_recognised() {
            assert!(is_agent_card_path("/.well-known/agent-card.json"));
        }

        /// The well-known path is exact; near misses are ordinary paths and
        /// must not inherit agent-card policy.
        #[test]
        fn near_misses_are_not_the_agent_card() {
            assert!(!is_agent_card_path("/.well-known/agent-card.json/"));
            assert!(!is_agent_card_path("/.well-known/agent.json"));
            assert!(!is_agent_card_path("/agent-card.json"));
            assert!(!is_agent_card_path("/.well-known/agent-card.json?x=1"));
        }
    }
}
