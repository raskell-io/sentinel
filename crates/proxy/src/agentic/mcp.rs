//! Model Context Protocol awareness.
//!
//! Implements the checks a reverse proxy is uniquely placed to make on MCP
//! traffic, against the Streamable HTTP transport as of revision `2026-07-28`.
//!
//! # What the transport gives a proxy
//!
//! Streamable HTTP mirrors selected body fields into HTTP headers so that,
//! in the specification's words, "intermediaries (load balancers, gateways,
//! observability tooling) can route and inspect requests without parsing the
//! body":
//!
//! | Header                 | Mirrors                       |
//! |------------------------|-------------------------------|
//! | `MCP-Protocol-Version` | `_meta.io.modelcontextprotocol/protocolVersion` |
//! | `Mcp-Method`           | `method`                      |
//! | `Mcp-Name`             | `params.name` or `params.uri` |
//!
//! That is a gift to a policy engine — and a trap, which is the point of this
//! module.
//!
//! # The trap
//!
//! Headers are cheap to read; bodies are the source of truth. An intermediary
//! that allowlists on `Mcp-Name` while the server executes `params.name` is
//! enforcing nothing: send `Mcp-Name: read_file` with a body calling
//! `delete_everything` and the allowlist waves it through.
//!
//! The specification anticipates this and requires servers to reject
//! mismatches with `-32020 HeaderMismatch`. It also warns intermediaries
//! specifically:
//!
//! > Intermediaries that enforce policy based on mirrored headers (e.g.,
//! > routing or rate-limiting by tenant) SHOULD verify that the
//! > `MCP-Protocol-Version` header indicates a version that requires
//! > header–body validation. If the version is older or the header is absent,
//! > the intermediary SHOULD reject the request rather than trusting
//! > unvalidated header values.
//!
//! Both halves matter. Checking header against body is useless if an attacker
//! can opt out by claiming an older protocol version, because those versions
//! never required the two to agree.
//!
//! So this module resolves policy against **the body**, and treats a
//! header/body disagreement as an attack rather than a formatting error.

use std::collections::HashSet;

use super::jsonrpc::{self, Envelope, ParseError};

/// Header carrying the protocol version, mirrored from `_meta`.
pub const HEADER_PROTOCOL_VERSION: &str = "mcp-protocol-version";
/// Header mirroring the JSON-RPC `method`.
pub const HEADER_METHOD: &str = "mcp-method";
/// Header mirroring `params.name` or `params.uri`.
pub const HEADER_NAME: &str = "mcp-name";
/// Prefix of the headers mirroring individual tool arguments.
///
/// A server marks an argument with `x-mcp-header: Region` in its tool schema
/// and conforming clients then send `Mcp-Param-Region` alongside the body.
pub const HEADER_PARAM_PREFIX: &str = "mcp-param-";

/// Prefix of the Base64 sentinel used for header values that are not
/// representable as plain ASCII: `=?base64?{value}?=`.
const BASE64_SENTINEL_PREFIX: &str = "=?base64?";
/// Suffix of the Base64 sentinel.
const BASE64_SENTINEL_SUFFIX: &str = "?=";

/// First protocol revision that requires mirrored headers to match the body.
///
/// Revisions before this did not define `Mcp-Method` or `Mcp-Name`, so their
/// header values carry no guarantee and cannot be a basis for policy.
pub const FIRST_VALIDATED_VERSION: &str = "2026-07-28";

/// What the proxy decided about an MCP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Forward it.
    Allow {
        /// Method resolved from the body, for metrics and audit.
        method: Option<String>,
        /// Tool or resource resolved from the body.
        target: Option<String>,
    },
    /// Reject it, with a reason fit to show an operator.
    Deny {
        /// Why, in terms an operator can act on.
        reason: DenyReason,
    },
}

/// Why a request was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// A mirrored header disagreed with the body it mirrors.
    ///
    /// Treated as hostile: the plausible innocent explanation (a client bug)
    /// and the hostile one (policy evasion) are indistinguishable from here,
    /// and only one of them is dangerous.
    HeaderBodyMismatch {
        /// Which header disagreed.
        header: &'static str,
        /// What the header claimed.
        header_value: String,
        /// What the body actually said.
        body_value: String,
    },
    /// An `Mcp-Param-*` header disagreed with the tool argument it mirrors.
    ///
    /// Same defect as [`Self::HeaderBodyMismatch`] on a different header: the
    /// specification's own example routes by region, so a header and body that
    /// disagree route one way and execute another.
    ParamHeaderMismatch {
        /// The header, lowercased.
        header: String,
        /// What the header claimed, after sentinel decoding.
        header_value: String,
        /// What the argument actually said.
        body_value: String,
    },
    /// Protocol version predates header/body validation, so mirrored headers
    /// cannot be trusted for policy.
    UnvalidatedProtocolVersion {
        /// The version claimed, or `None` if the header was absent.
        claimed: Option<String>,
    },
    /// The tool or resource is not permitted on this route.
    TargetNotAllowed {
        /// The method being invoked.
        method: String,
        /// The tool or resource that was refused.
        target: String,
    },
    /// The method itself is not permitted on this route.
    MethodNotAllowed {
        /// The method that was refused.
        method: String,
    },
    /// The body could not be inspected and the route requires inspection.
    Unparseable {
        /// What went wrong.
        error: String,
    },
}

impl DenyReason {
    /// The JSON-RPC method this refusal concerned, where the refusal was about
    /// a specific call rather than the request's shape.
    ///
    /// Used to label metrics, so a denial can be attributed to the tool it
    /// refused rather than only counted.
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::TargetNotAllowed { method, .. } | Self::MethodNotAllowed { method } => {
                Some(method)
            }
            Self::HeaderBodyMismatch { .. }
            | Self::ParamHeaderMismatch { .. }
            | Self::UnvalidatedProtocolVersion { .. }
            | Self::Unparseable { .. } => None,
        }
    }

    /// The tool or resource this refusal concerned, if it named one.
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::TargetNotAllowed { target, .. } => Some(target),
            Self::MethodNotAllowed { .. }
            | Self::HeaderBodyMismatch { .. }
            | Self::ParamHeaderMismatch { .. }
            | Self::UnvalidatedProtocolVersion { .. }
            | Self::Unparseable { .. } => None,
        }
    }
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HeaderBodyMismatch {
                header,
                header_value,
                body_value,
            } => write!(
                f,
                "{header} header says {header_value:?} but the request body says \
                 {body_value:?}; policy is resolved against the body, and a request \
                 that disagrees with itself is refused"
            ),
            Self::ParamHeaderMismatch {
                header,
                header_value,
                body_value,
            } => write!(
                f,
                "{header} header says {header_value:?} but the matching tool argument says \
                 {body_value:?}; a request whose headers and body disagree routes one way \
                 and executes another"
            ),
            Self::UnvalidatedProtocolVersion { claimed } => match claimed {
                Some(version) => write!(
                    f,
                    "protocol version {version:?} predates {FIRST_VALIDATED_VERSION}, which is \
                     the first revision requiring mirrored headers to match the body; header \
                     values from older revisions cannot be trusted for policy"
                ),
                None => write!(
                    f,
                    "no {HEADER_PROTOCOL_VERSION} header, so the request claims a revision \
                     that predates header/body validation"
                ),
            },
            Self::TargetNotAllowed { method, target } => {
                write!(f, "{target:?} is not permitted for {method} on this route")
            }
            Self::MethodNotAllowed { method } => {
                write!(f, "method {method:?} is not permitted on this route")
            }
            Self::Unparseable { error } => write!(
                f,
                "request body could not be inspected ({error}) and this route requires \
                 inspection to apply its policy"
            ),
        }
    }
}

/// What to do when a body cannot be inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UninspectableBody {
    /// Refuse it. The right answer where an allowlist is the security control:
    /// a body you cannot read is a body you cannot allowlist.
    #[default]
    Deny,
    /// Forward it. Appropriate where MCP awareness is for observability and
    /// the enforcement lives elsewhere.
    Allow,
}

/// Per-route MCP policy.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// Methods permitted. Empty means all.
    pub allowed_methods: HashSet<String>,
    /// Methods refused, applied after `allowed_methods`.
    pub denied_methods: HashSet<String>,
    /// Tools and resources permitted. Empty means all.
    pub allowed_targets: HashSet<String>,
    /// Tools and resources refused, applied after `allowed_targets`.
    pub denied_targets: HashSet<String>,
    /// Whether to require a protocol revision that guarantees header/body
    /// agreement.
    ///
    /// Defaults to true in the config layer. Turning it off means accepting
    /// that mirrored headers may be unvalidated, which is only safe if nothing
    /// downstream makes decisions from them.
    pub require_validated_version: bool,
    /// Whether to check `Mcp-Param-*` headers against the tool arguments they
    /// mirror.
    ///
    /// # What this can and cannot verify
    ///
    /// The header *name* comes from an `x-mcp-header` label in the tool's
    /// schema, which the proxy has never seen — `x-mcp-header: "Region"` on a
    /// property called `region` is the specification's own example, but the
    /// label and the property name are not required to match.
    ///
    /// So a header is checked when its suffix matches an argument key
    /// case-insensitively, and left alone when it does not. Denying the
    /// unmatched case would reject every schema whose label differs from its
    /// property name — legitimate traffic, refused on a naming convention.
    ///
    /// The practical consequence: if you route or rate-limit on a
    /// `Mcp-Param-*` header, keep the `x-mcp-header` label equal to the
    /// property name, or the proxy cannot confirm the two still agree.
    pub validate_param_headers: bool,
    /// What to do with a body that cannot be inspected.
    pub on_uninspectable: UninspectableBody,
}

/// Decode a header value that may use the Base64 sentinel.
///
/// Tool names and resource URIs are only *SHOULD*-constrained to header-safe
/// characters, so anything outside that set arrives as
/// `=?base64?{base64}?=`. Comparing the encoded form against a decoded body
/// value would make every non-ASCII tool name look like a mismatch — which,
/// given a mismatch is treated as hostile, would deny legitimate traffic.
pub fn decode_header_value(raw: &str) -> Option<String> {
    let Some(inner) = raw
        .strip_prefix(BASE64_SENTINEL_PREFIX)
        .and_then(|rest| rest.strip_suffix(BASE64_SENTINEL_SUFFIX))
    else {
        return Some(raw.to_string());
    };

    // The sentinel is case-sensitive and exact, so anything that opens with it
    // but does not decode is malformed rather than literal.
    let bytes = base64_decode(inner)?;
    String::from_utf8(bytes).ok()
}

/// Minimal standard-alphabet Base64 decoder.
///
/// Hand-rolled to avoid a dependency for one small, well-specified job on a
/// path that already has the body in memory.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn sextet(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let raw = input.as_bytes();
    let unpadded = raw
        .strip_suffix(b"==")
        .or_else(|| raw.strip_suffix(b"="))
        .unwrap_or(raw);
    let padding = raw.len() - unpadded.len();

    if !raw.len().is_multiple_of(4) || padding > 2 {
        return None;
    }

    let mut out = Vec::with_capacity(raw.len() / 4 * 3);
    for chunk in unpadded.chunks(4) {
        let mut accumulator = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            accumulator |= sextet(*byte)? << (18 - 6 * index);
        }
        // A 4-char chunk yields 3 bytes, a 3-char chunk 2, a 2-char chunk 1.
        let produced = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => return None,
        };
        for byte_index in 0..produced {
            out.push(((accumulator >> (16 - 8 * byte_index)) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// Whether a protocol version guarantees that mirrored headers match the body.
///
/// Revisions are dated (`YYYY-MM-DD`), so lexicographic ordering is
/// chronological ordering. An unparseable version is treated as not
/// guaranteeing anything, which is the safe direction.
pub fn version_is_validated(version: &str) -> bool {
    let looks_dated = version.len() == 10
        && version.as_bytes()[4] == b'-'
        && version.as_bytes()[7] == b'-'
        && version
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());

    looks_dated && version >= FIRST_VALIDATED_VERSION
}

/// Apply MCP policy to a request.
///
/// `headers` supplies mirrored header values by lowercase name. `body` is the
/// raw request body.
///
/// Policy is resolved against the body throughout. Headers are checked only to
/// confirm they agree with it — never as the basis for a decision.
pub fn evaluate(policy: &Policy, headers: &[(String, String)], body: &[u8]) -> Decision {
    let lookup = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };

    // The version gate comes first. Everything downstream assumes headers and
    // body are required to agree, which is only true from a certain revision
    // on — so checking anything else first would be reasoning from an
    // assumption not yet established.
    if policy.require_validated_version {
        let claimed = lookup(HEADER_PROTOCOL_VERSION);
        let validated = claimed.as_deref().is_some_and(version_is_validated);
        if !validated {
            return Decision::Deny {
                reason: DenyReason::UnvalidatedProtocolVersion { claimed },
            };
        }
    }

    let envelope = match jsonrpc::parse(body) {
        Ok(envelope) => envelope,
        Err(error) => {
            return match policy.on_uninspectable {
                UninspectableBody::Allow => Decision::Allow {
                    method: None,
                    target: None,
                },
                UninspectableBody::Deny => Decision::Deny {
                    reason: DenyReason::Unparseable {
                        error: error.to_string(),
                    },
                },
            };
        }
    };

    if let Some(decision) = check_header_agreement(policy, headers, &lookup, &envelope) {
        return decision;
    }

    if let Some(method) = envelope.method.as_deref() {
        if !permitted(method, &policy.allowed_methods, &policy.denied_methods) {
            return Decision::Deny {
                reason: DenyReason::MethodNotAllowed {
                    method: method.to_string(),
                },
            };
        }

        if let Some(target) = envelope.target.as_deref() {
            if !permitted(target, &policy.allowed_targets, &policy.denied_targets) {
                return Decision::Deny {
                    reason: DenyReason::TargetNotAllowed {
                        method: method.to_string(),
                        target: target.to_string(),
                    },
                };
            }
        }
    }

    Decision::Allow {
        method: envelope.method,
        target: envelope.target,
    }
}

/// Confirm mirrored headers agree with the body they mirror.
///
/// Returns `None` when they agree or the header is absent. A header that is
/// present and disagrees is the case this whole module exists for.
fn check_header_agreement(
    policy: &Policy,
    headers: &[(String, String)],
    lookup: &dyn Fn(&str) -> Option<String>,
    envelope: &Envelope,
) -> Option<Decision> {
    let mismatch = |header: &'static str, header_value: String, body_value: &str| {
        Some(Decision::Deny {
            reason: DenyReason::HeaderBodyMismatch {
                header,
                header_value,
                body_value: body_value.to_string(),
            },
        })
    };

    if let Some(raw) = lookup(HEADER_METHOD) {
        // `Mcp-Method` is a plain token; it is not sentinel-encoded.
        match envelope.method.as_deref() {
            Some(body_method) if body_method == raw => {}
            Some(body_method) => return mismatch(HEADER_METHOD, raw, body_method),
            // A method header on a body with no method is a disagreement too.
            None => return mismatch(HEADER_METHOD, raw, ""),
        }
    }

    if let Some(raw) = lookup(HEADER_NAME) {
        // An undecodable sentinel is not "different from the body", it is
        // malformed — but it still cannot be shown to agree, and agreement is
        // what forwarding depends on.
        let decoded = decode_header_value(&raw).unwrap_or_else(|| raw.clone());
        match envelope.target.as_deref() {
            Some(body_target) if body_target == decoded => {}
            Some(body_target) => return mismatch(HEADER_NAME, decoded, body_target),
            None => return mismatch(HEADER_NAME, decoded, ""),
        }
    }

    if policy.validate_param_headers {
        for (name, raw) in headers {
            let Some(suffix) = name.strip_prefix(HEADER_PARAM_PREFIX) else {
                continue;
            };

            // The suffix is a schema-chosen label, not necessarily the argument
            // name. Check the ones that line up; leave the rest, because
            // denying them would refuse legitimate traffic over a naming
            // convention the proxy cannot see. See `validate_param_headers`.
            let Some((_, body_value)) = envelope
                .arguments
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(suffix))
            else {
                continue;
            };

            let decoded = decode_header_value(raw).unwrap_or_else(|| raw.clone());
            if &decoded != body_value {
                return Some(Decision::Deny {
                    reason: DenyReason::ParamHeaderMismatch {
                        header: name.clone(),
                        header_value: decoded,
                        body_value: body_value.clone(),
                    },
                });
            }
        }
    }

    None
}

/// Allowlist-then-denylist membership.
///
/// An empty allowlist means "no allowlist configured", not "allow nothing" —
/// otherwise adding a denylist alone would silently refuse everything.
/// Whether a method or target passes an allow/deny pair.
///
/// `pub(super)` so [`super::listing`] can hide exactly what this refuses.
pub(super) fn permitted(value: &str, allowed: &HashSet<String>, denied: &HashSet<String>) -> bool {
    if !allowed.is_empty() && !allowed.contains(value) {
        return false;
    }
    !denied.contains(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_from(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.to_string()))
            .collect()
    }

    fn permissive_policy() -> Policy {
        Policy {
            allowed_methods: HashSet::new(),
            denied_methods: HashSet::new(),
            allowed_targets: HashSet::new(),
            denied_targets: HashSet::new(),
            require_validated_version: true,
            validate_param_headers: true,
            on_uninspectable: UninspectableBody::Deny,
        }
    }

    fn tools_call(name: &str) -> Vec<u8> {
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}"}}}}"#)
            .into_bytes()
    }

    fn good_headers(name: &str) -> Vec<(String, String)> {
        headers_from(&[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "tools/call"),
            ("mcp-name", name),
        ])
    }

    #[test]
    fn a_consistent_request_is_allowed() {
        let decision = evaluate(
            &permissive_policy(),
            &good_headers("get_weather"),
            &tools_call("get_weather"),
        );
        assert_eq!(
            decision,
            Decision::Allow {
                method: Some("tools/call".to_string()),
                target: Some("get_weather".to_string()),
            }
        );
    }

    /// The attack this module exists for: header names a permitted tool, body
    /// calls a different one. A proxy allowlisting on the header alone would
    /// forward this.
    #[test]
    fn a_header_naming_a_different_tool_than_the_body_is_denied() {
        let mut policy = permissive_policy();
        policy.allowed_targets = ["read_file".to_string()].into_iter().collect();

        let decision = evaluate(
            &policy,
            &good_headers("read_file"),
            &tools_call("delete_everything"),
        );

        match decision {
            Decision::Deny {
                reason:
                    DenyReason::HeaderBodyMismatch {
                        header,
                        header_value,
                        body_value,
                    },
            } => {
                assert_eq!(header, HEADER_NAME);
                assert_eq!(header_value, "read_file");
                assert_eq!(body_value, "delete_everything");
            }
            other => panic!("expected a header/body mismatch denial, got {other:?}"),
        }
    }

    /// The same evasion via the method header.
    #[test]
    fn a_method_header_disagreeing_with_the_body_is_denied() {
        let headers = headers_from(&[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "tools/list"),
            ("mcp-name", "get_weather"),
        ]);
        let decision = evaluate(&permissive_policy(), &headers, &tools_call("get_weather"));
        assert!(
            matches!(
                decision,
                Decision::Deny {
                    reason: DenyReason::HeaderBodyMismatch {
                        header: HEADER_METHOD,
                        ..
                    }
                }
            ),
            "got {decision:?}"
        );
    }

    /// Opting out of validation by claiming an older revision must not work:
    /// those revisions never required header and body to agree, so their
    /// headers cannot support policy.
    #[test]
    fn an_older_protocol_version_cannot_be_used_to_escape_validation() {
        let headers = headers_from(&[
            ("mcp-protocol-version", "2025-06-18"),
            ("mcp-method", "tools/call"),
            ("mcp-name", "read_file"),
        ]);
        let decision = evaluate(
            &permissive_policy(),
            &headers,
            &tools_call("delete_everything"),
        );
        assert!(
            matches!(
                decision,
                Decision::Deny {
                    reason: DenyReason::UnvalidatedProtocolVersion { .. }
                }
            ),
            "got {decision:?}"
        );
    }

    #[test]
    fn a_missing_protocol_version_header_is_denied() {
        let headers = headers_from(&[("mcp-method", "tools/call")]);
        let decision = evaluate(&permissive_policy(), &headers, &tools_call("t"));
        assert!(matches!(
            decision,
            Decision::Deny {
                reason: DenyReason::UnvalidatedProtocolVersion { claimed: None }
            }
        ));
    }

    #[test]
    fn version_validation_can_be_turned_off_deliberately() {
        let mut policy = permissive_policy();
        policy.require_validated_version = false;
        let headers = headers_from(&[("mcp-method", "tools/call"), ("mcp-name", "t")]);
        assert!(matches!(
            evaluate(&policy, &headers, &tools_call("t")),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn a_denied_tool_is_refused_even_when_headers_agree() {
        let mut policy = permissive_policy();
        policy.denied_targets = ["execute_sql".to_string()].into_iter().collect();
        let decision = evaluate(
            &policy,
            &good_headers("execute_sql"),
            &tools_call("execute_sql"),
        );
        assert!(matches!(
            decision,
            Decision::Deny {
                reason: DenyReason::TargetNotAllowed { .. }
            }
        ));
    }

    #[test]
    fn a_tool_outside_the_allowlist_is_refused() {
        let mut policy = permissive_policy();
        policy.allowed_targets = ["get_weather".to_string()].into_iter().collect();
        let decision = evaluate(
            &policy,
            &good_headers("send_email"),
            &tools_call("send_email"),
        );
        assert!(matches!(
            decision,
            Decision::Deny {
                reason: DenyReason::TargetNotAllowed { .. }
            }
        ));
    }

    /// An empty allowlist means unconfigured, not "deny everything" — a
    /// denylist alone must not refuse all traffic.
    #[test]
    fn an_empty_allowlist_does_not_deny_everything() {
        let mut policy = permissive_policy();
        policy.denied_targets = ["dangerous".to_string()].into_iter().collect();
        assert!(matches!(
            evaluate(&policy, &good_headers("harmless"), &tools_call("harmless")),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn a_denied_method_is_refused() {
        let mut policy = permissive_policy();
        policy.denied_methods = ["tools/call".to_string()].into_iter().collect();
        let decision = evaluate(&policy, &good_headers("anything"), &tools_call("anything"));
        assert!(matches!(
            decision,
            Decision::Deny {
                reason: DenyReason::MethodNotAllowed { .. }
            }
        ));
    }

    /// A body that cannot be read cannot be allowlisted, so the default
    /// refuses it.
    #[test]
    fn an_uninspectable_body_is_denied_by_default() {
        let headers = headers_from(&[("mcp-protocol-version", "2026-07-28")]);
        let decision = evaluate(&permissive_policy(), &headers, b"{not json");
        assert!(matches!(
            decision,
            Decision::Deny {
                reason: DenyReason::Unparseable { .. }
            }
        ));
    }

    #[test]
    fn an_uninspectable_body_can_be_allowed_deliberately() {
        let mut policy = permissive_policy();
        policy.on_uninspectable = UninspectableBody::Allow;
        let headers = headers_from(&[("mcp-protocol-version", "2026-07-28")]);
        assert!(matches!(
            evaluate(&policy, &headers, b"{not json"),
            Decision::Allow { .. }
        ));
    }

    /// A batch array is undecidable for this proxy, and must not be treated as
    /// an inspectable single request.
    #[test]
    fn a_batch_request_is_undecidable_and_denied_by_default() {
        let headers = headers_from(&[("mcp-protocol-version", "2026-07-28")]);
        let body = br#"[{"jsonrpc":"2.0","id":1,"method":"tools/call"}]"#;
        assert!(matches!(
            evaluate(&permissive_policy(), &headers, body),
            Decision::Deny {
                reason: DenyReason::Unparseable { .. }
            }
        ));
    }

    #[test]
    fn absent_mirrored_headers_leave_the_body_to_decide() {
        let mut policy = permissive_policy();
        policy.denied_targets = ["blocked".to_string()].into_iter().collect();
        let headers = headers_from(&[("mcp-protocol-version", "2026-07-28")]);

        assert!(matches!(
            evaluate(&policy, &headers, &tools_call("fine")),
            Decision::Allow { .. }
        ));
        assert!(matches!(
            evaluate(&policy, &headers, &tools_call("blocked")),
            Decision::Deny {
                reason: DenyReason::TargetNotAllowed { .. }
            }
        ));
    }

    /// `Mcp-Param-*` headers mirror individual tool arguments. The
    /// specification's own worked example routes by region, so a header and
    /// body that disagree send a request one way and execute it another — the
    /// same defect as the Mcp-Name desync, on a header that is easier to
    /// overlook because it is schema-defined rather than protocol-defined.
    mod param_headers {
        use super::*;

        fn execute_sql(region: &str) -> Vec<u8> {
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{
                     "name":"execute_sql",
                     "arguments":{{"region":"{region}","query":"SELECT 1"}}}}}}"#
            )
            .into_bytes()
        }

        fn headers_with_region(region: &str) -> Vec<(String, String)> {
            headers_from(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "execute_sql"),
                ("mcp-param-region", region),
            ])
        }

        #[test]
        fn an_agreeing_param_header_is_allowed() {
            assert!(matches!(
                evaluate(
                    &permissive_policy(),
                    &headers_with_region("us-west1"),
                    &execute_sql("us-west1")
                ),
                Decision::Allow { .. }
            ));
        }

        /// The attack: route to one region, execute in another.
        #[test]
        fn a_param_header_disagreeing_with_the_argument_is_denied() {
            let decision = evaluate(
                &permissive_policy(),
                &headers_with_region("us-west1"),
                &execute_sql("eu-central-1"),
            );
            match decision {
                Decision::Deny {
                    reason:
                        DenyReason::ParamHeaderMismatch {
                            header,
                            header_value,
                            body_value,
                        },
                } => {
                    assert_eq!(header, "mcp-param-region");
                    assert_eq!(header_value, "us-west1");
                    assert_eq!(body_value, "eu-central-1");
                }
                other => panic!("expected a param mismatch, got {other:?}"),
            }
        }

        /// The label in `x-mcp-header` is chosen by the tool schema and need
        /// not equal the property name. The proxy has never seen that schema,
        /// so an unmatched header is left alone — denying it would refuse
        /// legitimate traffic over a naming convention.
        #[test]
        fn a_header_matching_no_argument_is_left_alone() {
            let headers = headers_from(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "execute_sql"),
                ("mcp-param-reg", "us-west1"),
            ]);
            assert!(
                matches!(
                    evaluate(&permissive_policy(), &headers, &execute_sql("eu-central-1")),
                    Decision::Allow { .. }
                ),
                "an unmatchable label must not deny a legitimate request"
            );
        }

        /// Header names are case-insensitive and `x-mcp-header: \"Region\"`
        /// against a property called `region` is the specification's example,
        /// so the comparison has to survive the case difference.
        #[test]
        fn the_argument_match_is_case_insensitive() {
            let headers = headers_from(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "execute_sql"),
                ("Mcp-Param-Region", "eu-central-1"),
            ]);
            assert!(matches!(
                evaluate(&permissive_policy(), &headers, &execute_sql("eu-central-1")),
                Decision::Allow { .. }
            ));
        }

        #[test]
        fn param_validation_can_be_turned_off() {
            let mut policy = permissive_policy();
            policy.validate_param_headers = false;
            assert!(matches!(
                evaluate(
                    &policy,
                    &headers_with_region("us-west1"),
                    &execute_sql("eu-central-1")
                ),
                Decision::Allow { .. }
            ));
        }

        #[test]
        fn a_sentinel_encoded_param_is_decoded_before_comparison() {
            let headers = headers_from(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "execute_sql"),
                // "eu-central-1 " with a trailing space, which cannot be a
                // plain header value.
                ("mcp-param-region", "=?base64?ZXUtY2VudHJhbC0xIA==?="),
            ]);
            assert!(matches!(
                evaluate(
                    &permissive_policy(),
                    &headers,
                    &execute_sql("eu-central-1 ")
                ),
                Decision::Allow { .. }
            ));
        }

        /// Integers and booleans are mirrorable and must render the way a
        /// conforming client would encode them, or every such call reads as a
        /// mismatch.
        #[test]
        fn integer_and_boolean_arguments_compare_correctly() {
            let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"t","arguments":{"limit":42,"dry_run":true}}}"#;
            let headers = headers_from(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "t"),
                ("mcp-param-limit", "42"),
                ("mcp-param-dry_run", "true"),
            ]);
            assert!(matches!(
                evaluate(&permissive_policy(), &headers, body),
                Decision::Allow { .. }
            ));
        }

        /// A float cannot be mirrored per the specification, so it is absent
        /// from the comparison set rather than rendered — otherwise a
        /// legitimate request would be denied over a formatting difference.
        #[test]
        fn a_float_argument_is_not_compared() {
            let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"t","arguments":{"ratio":1.5}}}"#;
            let headers = headers_from(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "t"),
                ("mcp-param-ratio", "1.5"),
            ]);
            assert!(matches!(
                evaluate(&permissive_policy(), &headers, body),
                Decision::Allow { .. }
            ));
        }
    }

    mod base64_sentinel {
        use super::*;

        #[test]
        fn a_plain_value_passes_through() {
            assert_eq!(decode_header_value("us-west1").as_deref(), Some("us-west1"));
        }

        /// From the specification's own encoding examples.
        #[test]
        fn a_sentinel_encoded_value_is_decoded() {
            assert_eq!(
                decode_header_value("=?base64?SGVsbG8sIOS4lueVjA==?=").as_deref(),
                Some("Hello, 世界")
            );
        }

        #[test]
        fn padding_variants_decode() {
            assert_eq!(
                decode_header_value("=?base64?IHBhZGRlZCA=?=").as_deref(),
                Some(" padded ")
            );
            assert_eq!(
                decode_header_value("=?base64?bGluZTEKbGluZTI=?=").as_deref(),
                Some("line1\nline2")
            );
        }

        /// A non-ASCII tool name must compare equal to its body form, or every
        /// such call would look like an attack and be denied.
        #[test]
        fn an_encoded_header_agrees_with_the_decoded_body_value() {
            let headers = headers_from(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "=?base64?5aSp5rCX?="),
            ]);
            let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"天気"}}"#;
            assert!(
                matches!(
                    evaluate(&permissive_policy(), &headers, body.as_bytes()),
                    Decision::Allow { .. }
                ),
                "an encoded header must not be mistaken for a mismatch"
            );
        }

        #[test]
        fn a_malformed_sentinel_does_not_decode() {
            assert_eq!(decode_header_value("=?base64?not!valid!?="), None);
        }

        /// Two characters apart in the encoded form is a different string, and
        /// must still read as a mismatch. `5aSp5rCU` is 天气 (simplified) where
        /// `5aSp5rCX` is 天気 (Japanese) — a near-identical rendering that a
        /// reviewer would not catch by eye, which is exactly the kind of
        /// substitution the body comparison exists to catch.
        #[test]
        fn a_near_identical_encoded_value_is_still_a_mismatch() {
            let headers = headers_from(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "=?base64?5aSp5rCU?="),
            ]);
            let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"天気"}}"#;
            assert!(
                matches!(
                    evaluate(&permissive_policy(), &headers, body.as_bytes()),
                    Decision::Deny {
                        reason: DenyReason::HeaderBodyMismatch { .. }
                    }
                ),
                "天气 and 天気 are different tools"
            );
        }
    }

    mod version_ordering {
        use super::*;

        #[test]
        fn the_first_validated_revision_qualifies() {
            assert!(version_is_validated(FIRST_VALIDATED_VERSION));
        }

        #[test]
        fn later_revisions_qualify() {
            assert!(version_is_validated("2026-11-01"));
            assert!(version_is_validated("2027-01-01"));
        }

        #[test]
        fn earlier_revisions_do_not() {
            assert!(!version_is_validated("2025-06-18"));
            assert!(!version_is_validated("2025-03-26"));
            assert!(!version_is_validated("2024-11-05"));
        }

        /// Anything not shaped like a dated revision guarantees nothing, and
        /// must not be read as a large number that sorts high.
        #[test]
        fn a_malformed_version_does_not_qualify() {
            assert!(!version_is_validated("latest"));
            assert!(!version_is_validated("9999"));
            assert!(!version_is_validated(""));
            assert!(!version_is_validated("2026-07-28-extra"));
        }
    }
}

#[cfg(test)]
mod deny_attribution_tests {
    use super::*;

    #[test]
    fn a_refused_tool_is_attributable() {
        let reason = DenyReason::TargetNotAllowed {
            method: "tools/call".into(),
            target: "delete_everything".into(),
        };
        assert_eq!(reason.method(), Some("tools/call"));
        assert_eq!(reason.target(), Some("delete_everything"));
    }

    #[test]
    fn a_refused_method_names_no_target() {
        let reason = DenyReason::MethodNotAllowed {
            method: "resources/read".into(),
        };
        assert_eq!(reason.method(), Some("resources/read"));
        assert_eq!(reason.target(), None);
    }

    #[test]
    fn a_spoofed_header_attributes_nothing() {
        // The header and body disagree, so neither is trustworthy enough to
        // report as the call that was refused.
        let reason = DenyReason::HeaderBodyMismatch {
            header: "mcp-name",
            header_value: "read_file".into(),
            body_value: "delete_everything".into(),
        };
        assert_eq!(reason.method(), None);
        assert_eq!(reason.target(), None);
    }
}
