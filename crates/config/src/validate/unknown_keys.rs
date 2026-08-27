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

/// Where a block has to appear for its key list to apply.
///
/// Block names are not unique. `cache` at the top level configures the storage
/// backend (`backend`, `disk-path`, `max-size`); `cache` inside a `route`
/// configures that route's policy (`default-ttl-secs`, `stale-if-error-secs`).
/// The two share no keys at all, so matching on the node name alone would force
/// this check to accept the union — and accepting the union is precisely how
/// #90 went unreported, where `backend "disk"` and `disk-path` were written into
/// a route's `cache` block and silently did nothing.
#[derive(PartialEq, Eq)]
enum Nesting {
    /// Matches wherever the name appears. Correct when the name is unique.
    Anywhere,
    /// Matches only as a top-level block.
    Root,
    /// Matches only as a direct child of the named block.
    In(&'static str),
}

/// A block whose full set of valid keys is known.
struct ClosedBlock {
    /// The block's node name in KDL.
    name: &'static str,
    /// Where this key list applies.
    nesting: Nesting,
    /// Every key the parser reads inside it, including nested block names.
    keys: &'static [&'static str],
}

impl ClosedBlock {
    /// Whether this entry describes a node of `name` directly inside `parent`.
    fn matches(&self, name: &str, parent: Option<&str>) -> bool {
        self.name == name
            && match self.nesting {
                Nesting::Anywhere => true,
                Nesting::Root => parent.is_none(),
                Nesting::In(required) => parent == Some(required),
            }
    }
}

/// The entry describing this node, preferring a nesting-qualified match.
///
/// A qualified entry is always more specific than `Anywhere`, so it wins.
fn block_for<'a>(name: &str, parent: Option<&str>) -> Option<&'a ClosedBlock> {
    let candidates = CLOSED_BLOCKS.iter().filter(|b| b.matches(name, parent));
    let mut fallback = None;
    for block in candidates {
        if block.nesting == Nesting::Anywhere {
            fallback = Some(block);
        } else {
            return Some(block);
        }
    }
    fallback
}

/// Blocks this check inspects.
///
/// Each entry must list *every* key the corresponding parser reads, aliases
/// included. Under-listing produces false warnings on valid configs, which is
/// the failure mode to avoid.
const CLOSED_BLOCKS: &[ClosedBlock] = &[
    ClosedBlock {
        name: "connection-pool",
        nesting: Nesting::Anywhere,
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
        nesting: Nesting::Anywhere,
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
        nesting: Nesting::Anywhere,
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
        nesting: Nesting::Anywhere,
        keys: crate::kdl::routes::RECOGNIZED_ROUTE_CHILDREN,
    },
    ClosedBlock {
        name: "system",
        nesting: Nesting::Anywhere,
        keys: crate::kdl::server::RECOGNIZED_SYSTEM_KEYS,
    },
    ClosedBlock {
        // Deprecated spelling of `system`, parsed by the same function.
        name: "server",
        nesting: Nesting::Anywhere,
        keys: crate::kdl::server::RECOGNIZED_SYSTEM_KEYS,
    },
    ClosedBlock {
        name: "listener",
        nesting: Nesting::Anywhere,
        keys: crate::kdl::server::RECOGNIZED_LISTENER_KEYS,
    },
    ClosedBlock {
        // Storage backend. Distinct from the per-route `cache` policy below:
        // they share a name and not one key.
        name: "cache",
        nesting: Nesting::Root,
        keys: &[
            "enabled",
            "backend",
            "max-size",
            "eviction-limit",
            "lock-timeout",
            "disk-path",
            "disk-shards",
            "disk-max-size",
            "status-header",
            "status-header-name",
        ],
    },
    ClosedBlock {
        // Per-route cache policy. `backend`, `disk-path` and the other storage
        // settings belong in the top-level block and do nothing here -- #90.
        name: "cache",
        nesting: Nesting::In("route"),
        keys: &[
            "enabled",
            "default-ttl-secs",
            "max-size-bytes",
            "cache-private",
            "stale-while-revalidate-secs",
            "stale-if-error-secs",
            "cacheable-methods",
            "cacheable-status-codes",
            "exclude-extensions",
            "exclude-paths",
            "ignore-query-params",
            "vary-headers",
        ],
    },
    ClosedBlock {
        name: "sni",
        nesting: Nesting::Anywhere,
        keys: &[
            "hostnames",
            "priority-hostnames",
            "cert-file",
            "key-file",
            "acme",
        ],
    },
    ClosedBlock {
        name: "sni-certs",
        nesting: Nesting::Anywhere,
        keys: &["cert-folder", "reload-mode", "reload-interval"],
    },
    ClosedBlock {
        name: "acme",
        nesting: Nesting::Anywhere,
        keys: &[
            "email",
            "domains",
            "storage",
            "staging",
            "server-url",
            "challenge-type",
            "dns-provider",
            "key-type",
            "renew-before-days",
            "eab",
        ],
    },
    ClosedBlock {
        // External Account Binding, nested inside `acme`.
        name: "eab",
        nesting: Nesting::Anywhere,
        keys: &["kid", "hmac-key"],
    },
    ClosedBlock {
        name: "policies",
        nesting: Nesting::Anywhere,
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
        walk(node, None, result);
    }
}

/// Depth-first walk, checking any node that names a closed block.
///
/// `parent` is the name of the block this node sits directly inside, or `None`
/// at the top level. It is what lets two blocks share a name and keep separate
/// key lists -- see [`Nesting`].
fn walk(node: &kdl::KdlNode, parent: Option<&str>, result: &mut ValidationResult) {
    let name = node.name().value();

    if let Some(block) = block_for(name, parent) {
        if let Some(children) = node.children() {
            for child in children.nodes() {
                let key = child.name().value();
                if !block.keys.contains(&key) {
                    result.add_warning(ValidationWarning::new(unknown_key_message(block, key)));
                }
                check_run_together(block, child, result);
            }
        }
    }

    if let Some(children) = node.children() {
        for child in children.nodes() {
            walk(child, Some(name), result);
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

/// How to refer to a block in a message, given where it has to appear.
fn describe(block: &ClosedBlock) -> String {
    match block.nesting {
        Nesting::Root => format!("the top-level '{}' block", block.name),
        Nesting::In(parent) => format!("a '{parent}' block's '{}' block", block.name),
        Nesting::Anywhere => format!("the '{}' block", block.name),
    }
}

/// Build the warning, with a suggestion when one key is clearly meant.
fn unknown_key_message(block: &ClosedBlock, key: &str) -> String {
    // A key that is valid in a *same-named* block elsewhere is not a typo, it
    // is in the wrong place -- `disk-path` in a route's `cache` rather than the
    // top-level one. Saying where it belongs is far more use than an
    // edit-distance guess, which would otherwise suggest the key back to itself.
    if let Some(other) = sibling_block_defining(block, key) {
        return format!(
            "'{key}' is a setting of {}, not of {}, so it is ignored here.",
            describe(other),
            describe(block)
        );
    }

    match closest_key(block, key) {
        Some(suggestion) => format!(
            "'{key}' is not a setting in the '{}' block and is being ignored. \
             Did you mean '{suggestion}'?",
            block.name
        ),
        None => format!(
            "'{key}' is not a setting in the '{}' block and is being ignored.",
            block.name
        ),
    }
}

/// A block of the same name, in a different position, that does define `key`.
fn sibling_block_defining<'a>(block: &ClosedBlock, key: &str) -> Option<&'a ClosedBlock> {
    CLOSED_BLOCKS
        .iter()
        .find(|b| b.name == block.name && b.nesting != block.nesting && b.keys.contains(&key))
}

/// The nearest known key, when it is near enough to be worth naming.
///
/// Scoped to this block's own key list, so a block sharing a name with another
/// cannot borrow its keys as suggestions.
fn closest_key(block: &ClosedBlock, key: &str) -> Option<&'static str> {
    let candidates = block.keys;

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

    /// The #90 case: storage settings written into a route's cache policy,
    /// where they parse and do nothing. Reporting these is the whole reason the
    /// two `cache` blocks need separate key lists.
    #[test]
    fn storage_settings_in_a_route_cache_block_are_reported() {
        let kdl = r#"
            route "app" {
                cache {
                    enabled #true
                    backend "disk"
                    disk-path "/etc/zentinel/cache"
                    disk-shards 16
                }
            }
        "#;
        let warnings = warnings_for(kdl);

        for key in ["backend", "disk-path", "disk-shards"] {
            assert!(
                warnings.iter().any(|w| w.contains(&format!("'{key}'"))),
                "{key} belongs to the top-level cache block, not a route's: {warnings:?}"
            );
        }
        assert!(
            !warnings.iter().any(|w| w.contains("'enabled'")),
            "enabled is valid in both: {warnings:?}"
        );
    }

    /// Each `cache` block accepts its own keys and only its own.
    #[test]
    fn the_two_cache_blocks_keep_separate_key_lists() {
        let top_level = r#"
            cache {
                enabled #true
                backend "disk"
                disk-path "/var/cache/zentinel"
                max-size 1073741824
                status-header #true
            }
        "#;
        assert!(
            warnings_for(top_level).is_empty(),
            "{:?}",
            warnings_for(top_level)
        );

        let per_route = r#"
            route "app" {
                cache {
                    enabled #true
                    default-ttl-secs 86400
                    stale-if-error-secs 300
                    vary-headers "Accept"
                }
            }
        "#;
        assert!(
            warnings_for(per_route).is_empty(),
            "{:?}",
            warnings_for(per_route)
        );
    }

    /// Route policy settings in the top-level block are equally wrong, in the
    /// other direction.
    #[test]
    fn policy_settings_in_the_top_level_cache_block_are_reported() {
        let kdl = r#"
            cache {
                backend "memory"
                default-ttl-secs 3600
            }
        "#;
        let warnings = warnings_for(kdl);
        assert!(
            warnings.iter().any(|w| w.contains("'default-ttl-secs'")),
            "{warnings:?}"
        );
    }

    #[test]
    fn tls_certificate_blocks_are_checked() {
        let valid = r#"
            tls {
                sni {
                    hostnames "example.com"
                    cert-file "/c.crt"
                    key-file "/c.key"
                }
                sni-certs {
                    cert-folder "/etc/certs/dynamic/"
                    reload-mode "watch"
                    reload-interval "30s"
                }
            }
        "#;
        assert!(warnings_for(valid).is_empty(), "{:?}", warnings_for(valid));

        // `hostname` is the singular typo of a real key; `cert-dir` is not a key.
        let typos = r#"
            tls {
                sni { hostname "example.com" }
                sni-certs { cert-dir "/etc/certs/" }
            }
        "#;
        let warnings = warnings_for(typos);
        assert!(
            warnings.iter().any(|w| w.contains("'hostname'")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("'cert-dir'")),
            "{warnings:?}"
        );
    }

    #[test]
    fn acme_and_its_eab_block_keep_separate_keys() {
        let valid = r#"
            acme {
                email "admin@example.com"
                domains "example.com"
                storage "/var/lib/zentinel/acme"
                challenge-type "http-01"
                eab {
                    kid "abc"
                    hmac-key "def"
                }
            }
        "#;
        assert!(warnings_for(valid).is_empty(), "{:?}", warnings_for(valid));

        // `kid` is an eab key, not an acme one.
        let misplaced = r#"
            acme {
                email "admin@example.com"
                kid "abc"
            }
        "#;
        assert!(
            warnings_for(misplaced).iter().any(|w| w.contains("'kid'")),
            "{:?}",
            warnings_for(misplaced)
        );
    }

    /// A run-together line inside one of the newly closed blocks is caught by
    /// the same check, since it reads the block's key list.
    #[test]
    fn run_together_lines_are_caught_in_the_new_blocks() {
        let kdl = r#"
            tls {
                sni-certs { cert-folder "/etc/certs/" reload-mode "watch" }
            }
        "#;
        let warnings = warnings_for(kdl);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("'reload-mode'") && w.contains("'cert-folder'")),
            "{warnings:?}"
        );
    }

    /// A misplaced key must be told where it belongs, not offered an
    /// edit-distance guess. Looking up suggestions by block *name* alone found
    /// the sibling list and produced "'backend' ... Did you mean 'backend'?",
    /// which reads as a linter malfunction.
    #[test]
    fn a_misplaced_key_names_the_block_it_belongs_to() {
        let kdl = r#"
            route "app" {
                cache { disk-path "/var/cache" }
            }
        "#;
        let warnings = warnings_for(kdl);
        assert_eq!(warnings.len(), 1, "{warnings:?}");

        let warning = &warnings[0];
        assert!(
            warning.contains("top-level 'cache'"),
            "should say where it belongs: {warning}"
        );
        assert!(
            !warning.contains("Did you mean"),
            "a misplaced key is not a typo: {warning}"
        );
    }

    /// Suggestions must come from the block actually being checked, never from
    /// a same-named block elsewhere.
    #[test]
    fn suggestions_do_not_leak_between_same_named_blocks() {
        // `disk-pathh` is a typo of a *top-level* key, inside a route's cache.
        // It is not a key of either list, so the only honest suggestion comes
        // from the route cache's own keys -- or none at all.
        let kdl = r#"
            route "app" {
                cache { disk-pathh "/var/cache" }
            }
        "#;
        let warnings = warnings_for(kdl);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            !warnings[0].contains("'disk-path'"),
            "suggestion leaked from the top-level block: {}",
            warnings[0]
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
