//! KDL parsing helper functions.
//!
//! Common utilities for extracting values from KDL nodes.

use std::collections::HashMap;

use crate::upstreams::UpstreamTarget;

use anyhow::Result;

/// Convert a byte offset to line and column numbers (1-indexed)
pub fn offset_to_line_col(content: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in content.chars().enumerate() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Helper to get a string entry from a KDL node
pub fn get_string_entry(node: &kdl::KdlNode, name: &str) -> Option<String> {
    node.children()
        .and_then(|children| children.get(name))
        .and_then(|n| n.entries().first())
        .and_then(|e| e.value().as_string())
        .map(|s| s.to_string())
}

/// Helper to get an integer entry from a KDL node
pub fn get_int_entry(node: &kdl::KdlNode, name: &str) -> Option<i128> {
    node.children()
        .and_then(|children| children.get(name))
        .and_then(|n| n.entries().first())
        .and_then(|e| e.value().as_integer())
}

/// Helper to get a boolean entry from a KDL node
pub fn get_bool_entry(node: &kdl::KdlNode, name: &str) -> Option<bool> {
    node.children()
        .and_then(|children| children.get(name))
        .and_then(|n| n.entries().first())
        .and_then(|e| e.value().as_bool())
}

/// Helper to get a float entry from a KDL node
pub fn get_float_entry(node: &kdl::KdlNode, name: &str) -> Option<f64> {
    node.children()
        .and_then(|children| children.get(name))
        .and_then(|n| n.entries().first())
        .and_then(|e| {
            // Try as float first, then as integer converted to float
            e.value()
                .as_float()
                .or_else(|| e.value().as_integer().map(|i| i as f64))
        })
}

/// Helper to get the first argument of a node as a string
pub fn get_first_arg_string(node: &kdl::KdlNode) -> Option<String> {
    node.entries()
        .first()
        .and_then(|e| e.value().as_string())
        .map(|s| s.to_string())
}

/// Read a named property entry as a string (e.g. `address="host:port"`).
fn named_string_entry(node: &kdl::KdlNode, name: &str) -> Option<String> {
    node.entries()
        .iter()
        .find(|e| e.name().map(|n| n.value()) == Some(name))
        .and_then(|e| e.value().as_string())
        .map(|s| s.to_string())
}

/// Read a named property entry as an integer (e.g. `weight=2`).
fn named_int_entry(node: &kdl::KdlNode, name: &str) -> Option<i128> {
    node.entries()
        .iter()
        .find(|e| e.name().map(|n| n.value()) == Some(name))
        .and_then(|e| e.value().as_integer())
}

/// Resolve an `address` given as a property (`address="x"`) or child node (`address "x"`).
fn target_address_field(node: &kdl::KdlNode) -> Option<String> {
    named_string_entry(node, "address").or_else(|| get_string_entry(node, "address"))
}

/// Resolve a single `target` node's address and weight across every accepted form.
fn parse_single_target(node: &kdl::KdlNode) -> Option<UpstreamTarget> {
    // Address: first positional arg, else an `address` property/child node.
    let address = get_first_arg_string(node).or_else(|| target_address_field(node))?;

    // Weight: `weight=N` property, else `weight N` child node; defaults to 1.
    let weight = named_int_entry(node, "weight")
        .or_else(|| get_int_entry(node, "weight"))
        .map(|v| v as u32)
        .unwrap_or(1);

    Some(UpstreamTarget {
        address,
        weight,
        max_requests: None,
        metadata: HashMap::new(),
    })
}

/// Parse upstream targets from an `upstream` node.
///
/// Shared by the single-file and multi-file parsers so both accept the same
/// syntax — the divergence here was the root cause of zentinelproxy/zentinel#254.
/// Every form below is accepted:
///
/// ```kdl
/// // Shorthand: address as the first argument
/// target "127.0.0.1:8081"
/// target "127.0.0.1:8082" weight=2
///
/// // Block form: address (and weight) as child nodes or properties
/// target { address "127.0.0.1:8081"; weight 2 }
/// target address="127.0.0.1:8081" weight=2
///
/// // Wrapped in a `targets` block
/// targets {
///     target "127.0.0.1:8081"
///     target { address "127.0.0.1:8082" weight=2 }
/// }
///
/// // Single-target shorthand on the upstream itself
/// address "127.0.0.1:8081"
/// ```
///
/// Targets without a resolvable address are skipped (rather than silently
/// defaulted), so a misconfigured upstream surfaces as "no targets" during
/// validation instead of pointing at a bogus address.
pub fn parse_upstream_targets(upstream: &kdl::KdlNode) -> Vec<UpstreamTarget> {
    let mut targets = Vec::new();

    if let Some(children) = upstream.children() {
        for node in children.nodes() {
            match node.name().value() {
                "target" => {
                    if let Some(target) = parse_single_target(node) {
                        targets.push(target);
                    }
                }
                "targets" => {
                    if let Some(target_children) = node.children() {
                        for target_node in target_children.nodes() {
                            if target_node.name().value() == "target" {
                                if let Some(target) = parse_single_target(target_node) {
                                    targets.push(target);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Single-target shorthand: `address "host:port"` directly on the upstream.
    if targets.is_empty() {
        if let Some(address) = target_address_field(upstream) {
            targets.push(UpstreamTarget {
                address,
                weight: 1,
                max_requests: None,
                metadata: HashMap::new(),
            });
        }
    }

    targets
}

/// Tries to parse a u32 from a kdlnode, and makes sure it is non-zero
pub fn extract_u32_with_limits(node: &kdl::KdlNode) -> Result<u32> {
    let first_value = match node.entries().first() {
        Some(v) => v,
        None => {
            return Err(anyhow::anyhow!(
                "Tried to parse u32 for key {} but did not find a value",
                node.name()
            ))
        }
    };
    let u32_val = match first_value.value().as_integer() {
        Some(v) => u32::try_from(v).map_err(anyhow::Error::msg)?,
        None => {
            return Err(anyhow::anyhow!(
                "Tried to convert value in {} to u32, but failed",
                node.name()
            ))
        }
    };

    if u32_val == 0 {
        return Err(anyhow::anyhow!("Implausible value for {}", node.name()));
    }

    Ok(u32_val)
}

/// Tries to parse a u32 from a kdlnode, and makes sure it is non-zero
pub fn extract_u64_with_limits(node: &kdl::KdlNode) -> Result<u64> {
    let first_value = match node.entries().first() {
        Some(v) => v,
        None => {
            return Err(anyhow::anyhow!(
                "Tried to parse u64 for key {} but did not find a value",
                node.name()
            ))
        }
    };
    let u64_val = match first_value.value().as_integer() {
        Some(v) => u64::try_from(v).map_err(anyhow::Error::msg)?,
        None => {
            return Err(anyhow::anyhow!(
                "Tried to convert value in {} to u64, but failed",
                node.name()
            ))
        }
    };

    if u64_val == 0 {
        return Err(anyhow::anyhow!("Implausible value for {}", node.name()));
    }

    Ok(u64_val)
}

/// Suggest the closest candidate for a (probably misspelled) input.
///
/// Returns `Some(candidate)` when a candidate is within a small edit distance
/// of the input (scaled with input length), so error messages can offer a
/// "did you mean" hint. Returns `None` when nothing is plausibly close.
pub fn did_you_mean<'a>(input: &str, candidates: &[&'a str]) -> Option<&'a str> {
    // Allow more typo distance for longer inputs; at least 1, at most 3.
    let max_distance = (input.len() / 4).clamp(1, 3);

    candidates
        .iter()
        .map(|c| (edit_distance(input, c), *c))
        .filter(|(d, _)| *d <= max_distance)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Edit distance between two strings (over `char`s).
///
/// Optimal-string-alignment variant of Levenshtein: insertions, deletions,
/// and substitutions cost 1, and an adjacent transposition ("hots" → "host")
/// also costs 1, which matches how config keys are typically mistyped.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    // rows[i][j] = distance between a[..i] and b[..j]
    let mut rows: Vec<Vec<usize>> = vec![(0..=b.len()).collect()];

    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![0usize; b.len() + 1];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution_cost = if ca == cb { 0 } else { 1 };
            let mut d = (rows[i][j] + substitution_cost) // substitute
                .min(rows[i][j + 1] + 1) // delete
                .min(row[j] + 1); // insert
            if i > 0 && j > 0 && *ca == b[j - 1] && a[i - 1] == *cb {
                d = d.min(rows[i - 1][j - 1] + 1); // transpose
            }
            row[j + 1] = d;
        }
        rows.push(row);
    }

    rows[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("path_prefix", "path-prefix"), 1);
        // Adjacent transposition costs 1 (OSA variant)
        assert_eq!(edit_distance("hots", "host"), 1);
    }

    #[test]
    fn did_you_mean_suggests_close_candidates() {
        let candidates = ["path", "path-prefix", "path-regex", "host", "method"];
        assert_eq!(
            did_you_mean("path_prefix", &candidates),
            Some("path-prefix")
        );
        assert_eq!(did_you_mean("hots", &candidates), Some("host"));
        assert_eq!(did_you_mean("pth", &candidates), Some("path"));
    }

    #[test]
    fn did_you_mean_rejects_distant_candidates() {
        let candidates = ["path", "host", "method"];
        assert_eq!(did_you_mean("upstream", &candidates), None);
        assert_eq!(did_you_mean("zzzzzz", &candidates), None);
    }
}
