//! Reports configuration keys that no parser reads.
//!
//! KDL blocks accept any key. A name the parser does not recognise is dropped
//! without complaint, so a misspelling produces no error, no warning, and no
//! effect — the proxy runs on the default while the config file says otherwise.
//! That has cost real behaviour more than once: `max-lifetime-secs` left
//! connection pools with no lifetime cap, and the whole `timeouts` block was
//! discarded in favour of defaults, because in each case the parser read a
//! slightly different name than every config wrote.
//!
//! This check works from the KDL document rather than the parsed `Config`,
//! because by the time a `Config` exists the unknown keys are already gone.
//!
//! # Only closed blocks are checked
//!
//! Some blocks legitimately hold arbitrary keys — a JSON schema's properties,
//! a header map, agent configuration forwarded verbatim. Warning about those
//! would be noise, and noise in a linter is worse than silence because it
//! teaches people to skip the output.
//!
//! So this checks an explicit list of blocks whose key set is known and fixed
//! (`CLOSED_BLOCKS`). Anything not on that list is not inspected. Adding a
//! block to the list is deliberately a decision, and `shipped_configs_use_only_known_keys`
//! guards it: if a real config uses a key the list does not know about, that
//! test fails rather than operators getting a false warning.
//!
//! Part of zentinelproxy/zentinel#365.

use super::{ValidationResult, ValidationWarning};

/// A block whose full set of valid keys is known.
struct ClosedBlock {
    /// The block's node name in KDL.
    name: &'static str,
    /// Every key the parser reads inside it, including nested block names.
    keys: &'static [&'static str],
}

/// Blocks this check inspects.
///
/// Each entry must list *every* key the corresponding parser reads, aliases
/// included. Under-listing produces false warnings on valid configs, which is
/// the failure mode to avoid.
const CLOSED_BLOCKS: &[ClosedBlock] = &[
    ClosedBlock {
        name: "connection-pool",
        keys: &[
            "max-connections",
            "max-idle",
            "idle-timeout-secs",
            "max-lifetime-secs",
            // Accepted aliases; see parse_connection_pool.
            "idle-timeout",
            "max-lifetime",
        ],
    },
    ClosedBlock {
        name: "timeouts",
        keys: &[
            "connect-secs",
            "request-secs",
            "read-secs",
            "write-secs",
            // Accepted aliases; see parse_upstream_timeouts.
            "connect",
            "request",
            "read",
            "write",
        ],
    },
    ClosedBlock {
        name: "ruleset",
        keys: &[
            "crs-version",
            "custom-rules-dir",
            "paranoia-level",
            "anomaly-threshold",
            // Exclusions are accepted grouped in a wrapper or listed directly.
            "exclusions",
            "exclusion",
        ],
    },
    // `route` and `system` both keep their key list beside their parser, so
    // that adding a setting and forgetting the list is visible at the point of
    // the change rather than here.
    ClosedBlock {
        name: "route",
        keys: crate::kdl::routes::RECOGNIZED_ROUTE_CHILDREN,
    },
    ClosedBlock {
        name: "system",
        keys: crate::kdl::server::RECOGNIZED_SYSTEM_KEYS,
    },
    ClosedBlock {
        // Deprecated spelling of `system`, parsed by the same function.
        name: "server",
        keys: crate::kdl::server::RECOGNIZED_SYSTEM_KEYS,
    },
    ClosedBlock {
        name: "listener",
        keys: crate::kdl::server::RECOGNIZED_LISTENER_KEYS,
    },
    ClosedBlock {
        name: "policies",
        keys: &[
            "request-headers",
            "response-headers",
            "cache",
            "timeout-secs",
            "max-body-size",
            "failure-mode",
            "rate-limit",
            // "buffer-requests" / "buffer-responses" were listed here to keep
            // this check quiet about a known gap. #366 removed the fields, so
            // they are now reported like any other key nothing reads.
        ],
    },
];

/// Warn about keys inside closed blocks that no parser reads.
pub fn check_unknown_keys(kdl_source: &str, result: &mut ValidationResult) {
    let Ok(document) = kdl_source.parse::<kdl::KdlDocument>() else {
        // A document that does not parse is not this check's problem; the
        // parser will have produced a better error already.
        return;
    };

    for node in document.nodes() {
        walk(node, result);
    }
}

/// Depth-first walk, checking any node that names a closed block.
fn walk(node: &kdl::KdlNode, result: &mut ValidationResult) {
    let name = node.name().value();

    if let Some(block) = CLOSED_BLOCKS.iter().find(|b| b.name == name) {
        if let Some(children) = node.children() {
            for child in children.nodes() {
                let key = child.name().value();
                if !block.keys.contains(&key) {
                    result
                        .add_warning(ValidationWarning::new(unknown_key_message(block.name, key)));
                }
                check_run_together(block, child, result);
            }
        }
    }

    if let Some(children) = node.children() {
        for child in children.nodes() {
            walk(child, result);
        }
    }
}

/// Warn when a node's surplus arguments name a sibling key.
///
/// KDL separates nodes by newline or `;`. Written on one line without either,
///
/// ```kdl
/// listener "public" { address "0.0.0.0:8080" namespace "iso" }
/// ```
///
/// is not two settings — it is a single `address` node carrying three
/// arguments, and the entry helpers read only the first. `namespace` is
/// dropped before any parser sees it, so the unknown-key check above cannot
/// see it either: there is no `namespace` child node to be unknown. The config
/// loads, validates and starts, and the setting simply does not exist. That is
/// how a listener's namespace isolation came to be silently absent in #396.
///
/// The signature is specific: an argument *after the first* whose value is the
/// name of a key in this same block. A node legitimately taking a list —
/// `hostnames "a.com" "b.com"` — never matches, because hostnames are not
/// listener keys. Only the first argument is exempt, since that is the node's
/// own value.
fn check_run_together(block: &ClosedBlock, child: &kdl::KdlNode, result: &mut ValidationResult) {
    let key = child.name().value();

    for entry in child.entries().iter().skip(1) {
        // Property syntax (`name=value`) is not a run-together line.
        if entry.name().is_some() {
            continue;
        }
        let Some(value) = entry.value().as_string() else {
            continue;
        };
        if block.keys.contains(&value) {
            result.add_warning(ValidationWarning::new(run_together_message(
                block.name, key, value,
            )));
        }
    }
}

/// Build the run-together warning.
fn run_together_message(block: &str, key: &str, swallowed: &str) -> String {
    format!(
        "'{swallowed}' is being read as an argument to '{key}' rather than as a setting, \
         so it is ignored. In the '{block}' block, put each setting on its own line \
         (or separate them with ';')."
    )
}

/// Build the warning, with a suggestion when one key is clearly meant.
fn unknown_key_message(block: &str, key: &str) -> String {
    match closest_key(block, key) {
        Some(suggestion) => format!(
            "'{key}' is not a setting in the '{block}' block and is being ignored. \
             Did you mean '{suggestion}'?"
        ),
        None => format!("'{key}' is not a setting in the '{block}' block and is being ignored."),
    }
}

/// The nearest known key, when it is near enough to be worth naming.
fn closest_key(block: &str, key: &str) -> Option<&'static str> {
    let candidates = CLOSED_BLOCKS.iter().find(|b| b.name == block)?.keys;

    // A third of the length, so short keys need a close match and long ones
    // tolerate a suffix like `-secs`. Beyond that a suggestion is a guess, and
    // a wrong suggestion is worse than none.
    let budget = (key.len() / 3).max(2);

    candidates
        .iter()
        .map(|candidate| (*candidate, edit_distance(key, candidate)))
        .filter(|(_, distance)| *distance <= budget)
        .min_by_key(|(_, distance)| *distance)
        .map(|(candidate, _)| candidate)
}

/// Levenshtein distance, iterative with a single row.
fn edit_distance(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b_chars.len()).collect();
    let mut current = vec![0; b_chars.len() + 1];

    for (i, a_char) in a.chars().enumerate() {
        current[0] = i + 1;
        for (j, b_char) in b_chars.iter().enumerate() {
            let substitution = previous[j] + usize::from(a_char != *b_char);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current[j + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[b_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warnings_for(kdl: &str) -> Vec<String> {
        let mut result = ValidationResult::new();
        check_unknown_keys(kdl, &mut result);
        result.warnings.into_iter().map(|w| w.message).collect()
    }

    #[test]
    fn a_valid_block_produces_no_warnings() {
        let kdl = r#"
            upstreams {
                upstream "backend" {
                    connection-pool {
                        max-connections 100
                        max-idle 20
                        idle-timeout-secs 60
                        max-lifetime-secs 300
                    }
                }
            }
        "#;
        assert!(warnings_for(kdl).is_empty());
    }

    /// The bug this check exists for: a name one hyphen away from the real one.
    #[test]
    fn an_underscore_instead_of_a_hyphen_is_reported_with_a_suggestion() {
        let kdl = r#"
            routes {
                route "api" {
                    policies {
                        failure_mode "closed"
                    }
                }
            }
        "#;
        let warnings = warnings_for(kdl);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("failure_mode"));
        assert!(warnings[0].contains("is being ignored"));
        assert!(
            warnings[0].contains("Did you mean 'failure-mode'?"),
            "expected a suggestion, got: {}",
            warnings[0]
        );
    }

    #[test]
    fn a_missing_hyphen_is_reported_with_a_suggestion() {
        let warnings = warnings_for(r#"routes { route "api" { policies { ratelimit { } } } }"#);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("Did you mean 'rate-limit'?"),
            "got: {}",
            warnings[0]
        );
    }

    /// The real-world case from #367: the config carried the unit suffix and
    /// the parser did not.
    #[test]
    fn a_unit_suffix_mismatch_is_reported() {
        let warnings =
            warnings_for(r#"upstreams { upstream "b" { timeouts { connect-seconds 5 } } }"#);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("connect-seconds"));
        assert!(warnings[0].contains("Did you mean 'connect-secs'?"));
    }

    #[test]
    fn a_key_resembling_nothing_is_reported_without_a_suggestion() {
        let warnings =
            warnings_for(r#"upstreams { upstream "b" { connection-pool { zzzzzzzzzzzz 1 } } }"#);
        assert_eq!(warnings.len(), 1);
        assert!(
            !warnings[0].contains("Did you mean"),
            "got: {}",
            warnings[0]
        );
    }

    /// Blocks that legitimately carry arbitrary keys must stay silent.
    #[test]
    fn blocks_outside_the_list_are_not_inspected() {
        let kdl = r#"
            routes {
                route "api" {
                    api-schema {
                        properties {
                            avatar_url "string"
                            created_at "string"
                        }
                    }
                }
            }
        "#;
        assert!(warnings_for(kdl).is_empty());
    }

    #[test]
    fn closed_blocks_are_found_at_any_depth() {
        let kdl = r#"
            namespace "team-a" {
                upstreams {
                    upstream "backend" {
                        timeouts { bogus-key 1 }
                    }
                }
            }
        "#;
        assert_eq!(warnings_for(kdl).len(), 1);
    }

    /// Guards the registry against under-listing. If a shipped config uses a
    /// key this check does not know, operators would get a warning about
    /// perfectly valid configuration — so that failure belongs here, loudly,
    /// rather than in someone's terminal.
    /// `workers` shipped in two configs and was read by nothing, so both ran on
    /// the default worker count while the file said 4 and 1.
    ///
    /// No suggestion is offered here, and that is worth recording rather than
    /// asserting away: `workers` is not a typo of `worker-threads`, it is a
    /// different, shorter name for the same idea, and it is too far away for
    /// the edit-distance heuristic to reach. The warning still names the key
    /// and says it is ignored, which is the part that matters.
    #[test]
    fn the_workers_key_that_shipped_in_two_configs_is_reported() {
        let kdl = r#"
            system {
                workers 4
                daemon #false
            }
        "#;
        let warnings = warnings_for(kdl);
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(warnings[0].contains("workers"));
        assert!(warnings[0].contains("is being ignored"));
    }

    #[test]
    fn a_valid_system_block_produces_no_warnings() {
        let kdl = r#"
            system {
                worker-threads 4
                max-connections 10000
                graceful-shutdown-timeout-secs 30
                daemon #false
                pid-file "/var/run/zentinel.pid"
                user "zentinel"
                group "zentinel"
                working-directory "/var/lib/zentinel"
                trace-id-format "uuid"
                auto-reload #false
                route-cache-size 1024
            }
        "#;
        assert!(warnings_for(kdl).is_empty(), "{:?}", warnings_for(kdl));
    }

    /// `server` is the deprecated spelling and is parsed by the same function,
    /// so it has to be checked against the same keys.
    #[test]
    fn the_deprecated_server_block_is_checked_too() {
        let kdl = r#"
            server {
                worker_threads 4
            }
        "#;
        let warnings = warnings_for(kdl);
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(warnings[0].contains("worker_threads"));
    }

    #[test]
    fn an_unknown_directive_in_a_route_is_reported() {
        let kdl = r#"
            routes {
                route "api" {
                    path_prefix "/api"
                    upstream "backend"
                }
            }
        "#;
        let warnings = warnings_for(kdl);
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(warnings[0].contains("path_prefix"));
    }

    #[test]
    fn a_valid_route_produces_no_warnings() {
        let kdl = r#"
            routes {
                route "api" {
                    matches {
                        path-prefix "/api"
                    }
                    upstream "backend"
                    priority 10
                    waf-enabled #true
                }
            }
        "#;
        assert!(warnings_for(kdl).is_empty(), "{:?}", warnings_for(kdl));
    }

    /// The #396 case: `namespace` swallowed as an argument to `address`.
    #[test]
    fn a_run_together_listener_line_is_reported() {
        let kdl = r#"
            listeners {
                listener "public" { address "0.0.0.0:8080" namespace "iso" }
            }
        "#;
        let mut result = ValidationResult::default();
        check_unknown_keys(kdl, &mut result);

        let warnings = warning_texts(&result);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one warning, got: {warnings:?}"
        );
        assert!(
            warnings[0].contains("'namespace'") && warnings[0].contains("'address'"),
            "warning should name both the swallowed setting and its host: {}",
            warnings[0]
        );
    }

    /// The same config written correctly must be silent.
    #[test]
    fn separated_listener_settings_are_not_reported() {
        for separator in ["\n", ";"] {
            let kdl = format!(
                r#"
                listeners {{
                    listener "public" {{ address "0.0.0.0:8080"{separator}namespace "iso" }}
                }}
                "#
            );
            let mut result = ValidationResult::default();
            check_unknown_keys(&kdl, &mut result);
            assert!(
                result.warnings.is_empty(),
                "separator {separator:?} should parse as two settings, got: {:?}",
                warning_texts(&result)
            );
        }
    }

    /// A node that legitimately takes a list must not be flagged. This is the
    /// false positive that would make the check unusable.
    #[test]
    fn a_node_taking_a_list_of_values_is_not_reported() {
        let kdl = r#"
            route "r" {
                policies {
                    cache {
                        cacheable-methods "GET" "HEAD"
                    }
                }
            }
        "#;
        let mut result = ValidationResult::default();
        check_unknown_keys(kdl, &mut result);
        assert!(
            result.warnings.is_empty(),
            "a value list is not a run-together line: {:?}",
            warning_texts(&result)
        );
    }

    /// Only arguments *after* the first are candidates: the first is the
    /// node's own value, and may legitimately equal a key name.
    #[test]
    fn a_value_equal_to_a_key_name_is_allowed_in_first_position() {
        let kdl = r#"
            listeners {
                listener "public" { address "0.0.0.0:8080" }
                listener "odd" { default-route "namespace" }
            }
        "#;
        let mut result = ValidationResult::default();
        check_unknown_keys(kdl, &mut result);
        assert!(
            result.warnings.is_empty(),
            "a first-position value is the node's own: {:?}",
            warning_texts(&result)
        );
    }

    /// Several settings can be swallowed by one line, and each is worth naming.
    #[test]
    fn every_swallowed_setting_on_a_line_is_reported() {
        let kdl = r#"
            listeners {
                listener "public" {
                    address "0.0.0.0:8080" namespace "iso" protocol "https"
                }
            }
        "#;
        let mut result = ValidationResult::default();
        check_unknown_keys(kdl, &mut result);

        let warnings = warning_texts(&result);
        assert_eq!(warnings.len(), 2, "got: {warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("'namespace'")));
        assert!(warnings.iter().any(|w| w.contains("'protocol'")));
    }

    /// Property syntax is a different construct and not this check's business.
    #[test]
    fn property_syntax_is_not_a_run_together_line() {
        let kdl = r#"
            listeners {
                listener "public" { address "0.0.0.0:8080" namespace="iso" }
            }
        "#;
        let mut result = ValidationResult::default();
        check_unknown_keys(kdl, &mut result);
        assert!(
            result.warnings.is_empty(),
            "named entries are properties, not swallowed nodes: {:?}",
            warning_texts(&result)
        );
    }

    fn warning_texts(result: &ValidationResult) -> Vec<String> {
        result.warnings.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn shipped_configs_use_only_known_keys() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let mut checked = 0;
        let mut complaints = Vec::new();

        for dir in ["config", "config/examples"] {
            let path = std::path::Path::new(root).join(dir);
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let file = entry.path();
                if file.extension().and_then(|e| e.to_str()) != Some("kdl") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&file) else {
                    continue;
                };
                checked += 1;
                for warning in warnings_for(&text) {
                    complaints.push(format!("{}: {warning}", file.display()));
                }
            }
        }

        assert!(checked > 0, "should have found shipped configs to check");
        assert!(
            complaints.is_empty(),
            "shipped configs contain keys this check does not know about. Either the \
             config is wrong or CLOSED_BLOCKS is under-listed:\n{}",
            complaints.join("\n")
        );
    }
}
