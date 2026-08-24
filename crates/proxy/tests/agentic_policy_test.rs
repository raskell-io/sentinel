//! MCP and A2A policy, driven from configuration.
//!
//! # Why this file exists
//!
//! `crates/proxy/src/agentic/` carries 73 unit tests and every one of them
//! builds a `Policy` by hand and calls `evaluate()`. All of them passed for a
//! full release during which **nothing in the proxy ever called `evaluate()`**:
//! `RouteConfig::mcp` was parsed, validated, and read by no one. A route saying
//! `tools { allow "get_weather" }` permitted every tool there is.
//!
//! That is the same shape as three other defects found the same week — a test
//! that constructs the object under test cannot notice that nothing constructs
//! it in production.
//!
//! So these tests start from KDL and end at a decision, crossing every boundary
//! the defect hid in: parse → `RouteConfig` → `Policy` → `Outcome`.

use zentinel_config::Config;
use zentinel_proxy::agentic::{decide, Outcome};

/// A config with one MCP route and one ordinary route.
fn config_with(mcp_block: &str) -> Config {
    let kdl = format!(
        r#"
        system {{
            worker-threads 2
        }}
        listeners {{
            listener "http" {{
                address "127.0.0.1:8080"
            }}
        }}
        upstreams {{
            upstream "backend" {{
                target "127.0.0.1:9000"
            }}
        }}
        routes {{
            route "mcp" {{
                matches {{
                    path-prefix "/mcp"
                }}
                upstream "backend"
{mcp_block}
            }}
            route "plain" {{
                matches {{
                    path-prefix "/"
                }}
                upstream "backend"
            }}
        }}
        "#
    );
    Config::from_kdl(&kdl).expect("config should parse")
}

fn route<'a>(config: &'a Config, id: &str) -> &'a zentinel_config::RouteConfig {
    config
        .routes
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("route {id} should exist"))
}

const TOOLS_BLOCK: &str = r#"
                mcp {
                    tools {
                        allow "get_weather"
                        deny "execute_sql"
                    }
                }
"#;

fn call(tool: &str) -> Vec<u8> {
    format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}"}}}}"#)
        .into_bytes()
}

fn mcp_headers(name: &str) -> Vec<(String, String)> {
    vec![
        ("mcp-protocol-version".to_string(), "2026-07-28".to_string()),
        ("mcp-method".to_string(), "tools/call".to_string()),
        ("mcp-name".to_string(), name.to_string()),
    ]
}

/// The headline case. Before the wiring existed this returned `Allow`, because
/// nothing consulted the route's policy at all.
#[test]
fn a_tool_outside_the_allowlist_is_denied_from_config() {
    let config = config_with(TOOLS_BLOCK);
    let outcome = decide(
        route(&config, "mcp"),
        &mcp_headers("delete_everything"),
        &call("delete_everything"),
    );

    match outcome {
        Some(Outcome::Deny { reason, kind }) => {
            assert_eq!(kind, "mcp_policy");
            assert!(
                reason.contains("delete_everything"),
                "reason should name the tool, got: {reason}"
            );
        }
        other => panic!("expected Deny, got {other:?}"),
    }
}

#[test]
fn an_allowlisted_tool_is_permitted_and_reports_what_the_body_said() {
    let config = config_with(TOOLS_BLOCK);
    let outcome = decide(
        route(&config, "mcp"),
        &mcp_headers("get_weather"),
        &call("get_weather"),
    );

    match outcome {
        Some(Outcome::Allow {
            mcp_method,
            mcp_target,
            ..
        }) => {
            assert_eq!(mcp_method.as_deref(), Some("tools/call"));
            assert_eq!(mcp_target.as_deref(), Some("get_weather"));
        }
        other => panic!("expected Allow, got {other:?}"),
    }
}

/// The desync the module exists for: a header naming a permitted tool over a
/// body calling a denied one. Resolving policy from the header would forward
/// this.
#[test]
fn a_header_that_disagrees_with_the_body_is_denied() {
    let config = config_with(TOOLS_BLOCK);
    let outcome = decide(
        route(&config, "mcp"),
        &mcp_headers("get_weather"),
        &call("execute_sql"),
    );

    match outcome {
        Some(Outcome::Deny { reason, kind }) => {
            assert_eq!(kind, "mcp_policy");
            assert!(
                reason.contains("get_weather") && reason.contains("execute_sql"),
                "reason should name both sides, got: {reason}"
            );
        }
        other => panic!("expected Deny, got {other:?}"),
    }
}

/// A revision predating mandatory header/body agreement is refused by default,
/// or the check is optional at the caller's discretion.
#[test]
fn an_unvalidated_protocol_version_is_denied_by_default() {
    let config = config_with(TOOLS_BLOCK);
    let mut headers = mcp_headers("get_weather");
    headers[0].1 = "2025-06-18".to_string();

    let outcome = decide(route(&config, "mcp"), &headers, &call("get_weather"));
    assert!(
        matches!(outcome, Some(Outcome::Deny { .. })),
        "expected Deny, got {outcome:?}"
    );
}

/// `require-validated-version #false` has to actually reach the evaluator —
/// asserting the default would prove nothing about the config boundary.
#[test]
fn require_validated_version_false_reaches_the_evaluator() {
    let config = config_with(
        r#"
                mcp {
                    require-validated-version #false
                    tools {
                        allow "get_weather"
                    }
                }
"#,
    );
    let mut headers = mcp_headers("get_weather");
    headers[0].1 = "2025-06-18".to_string();

    let outcome = decide(route(&config, "mcp"), &headers, &call("get_weather"));
    assert!(
        matches!(outcome, Some(Outcome::Allow { .. })),
        "an old revision should be accepted once the check is off, got {outcome:?}"
    );
}

/// A body that cannot be parsed cannot be checked against an allowlist, so the
/// default refuses it.
#[test]
fn an_unparseable_body_is_denied_by_default() {
    let config = config_with(TOOLS_BLOCK);
    let outcome = decide(
        route(&config, "mcp"),
        &mcp_headers("get_weather"),
        b"this is not json",
    );
    assert!(
        matches!(outcome, Some(Outcome::Deny { .. })),
        "expected Deny, got {outcome:?}"
    );
}

#[test]
fn on_uninspectable_body_allow_reaches_the_evaluator() {
    let config = config_with(
        r#"
                mcp {
                    on-uninspectable-body "allow"
                }
"#,
    );
    let outcome = decide(
        route(&config, "mcp"),
        &mcp_headers("get_weather"),
        b"this is not json",
    );
    assert!(
        matches!(outcome, Some(Outcome::Allow { .. })),
        "expected Allow once the route opts in, got {outcome:?}"
    );
}

/// The common case has to stay free: a route with neither block is not
/// inspected at all.
#[test]
fn a_route_without_an_agentic_block_is_not_evaluated() {
    let config = config_with(TOOLS_BLOCK);
    let outcome = decide(
        route(&config, "plain"),
        &mcp_headers("anything"),
        &call("anything"),
    );
    assert_eq!(outcome, None);
}

// ============================================================================
// A2A
// ============================================================================

fn a2a_config(block: &str) -> Config {
    config_with(block)
}

fn a2a_call(method: &str) -> Vec<u8> {
    format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}"}}"#).into_bytes()
}

#[test]
fn a_denied_a2a_method_is_refused_from_config() {
    let config = a2a_config(
        r#"
                a2a {
                    methods {
                        deny "SendMessage"
                    }
                }
"#,
    );
    let outcome = decide(route(&config, "mcp"), &[], &a2a_call("SendMessage"));

    match outcome {
        Some(Outcome::Deny { reason, kind }) => {
            assert_eq!(kind, "a2a_policy");
            assert!(reason.contains("SendMessage"), "got: {reason}");
        }
        other => panic!("expected Deny, got {other:?}"),
    }
}

#[test]
fn a_permitted_a2a_method_reports_what_the_body_said() {
    let config = a2a_config(
        r#"
                a2a {
                    methods {
                        deny "SendMessage"
                    }
                }
"#,
    );
    let outcome = decide(route(&config, "mcp"), &[], &a2a_call("GetTask"));

    match outcome {
        Some(Outcome::Allow { a2a_method, .. }) => {
            assert_eq!(a2a_method.as_deref(), Some("GetTask"));
        }
        other => panic!("expected Allow, got {other:?}"),
    }
}

/// A2A is young; refusing every method added after this proxy was built would
/// make it an obstacle to upgrading agents. The default forwards them.
#[test]
fn unknown_a2a_methods_are_forwarded_by_default() {
    let config = a2a_config(
        r#"
                a2a {
                    methods {
                        deny "SendMessage"
                    }
                }
"#,
    );
    let outcome = decide(route(&config, "mcp"), &[], &a2a_call("SomeFutureMethod"));
    assert!(
        matches!(outcome, Some(Outcome::Allow { .. })),
        "expected Allow, got {outcome:?}"
    );
}

#[test]
fn unknown_methods_deny_reaches_the_evaluator() {
    let config = a2a_config(
        r#"
                a2a {
                    unknown-methods "deny"
                }
"#,
    );
    let outcome = decide(route(&config, "mcp"), &[], &a2a_call("SomeFutureMethod"));
    assert!(
        matches!(outcome, Some(Outcome::Deny { .. })),
        "expected Deny once the route opts in, got {outcome:?}"
    );
}
