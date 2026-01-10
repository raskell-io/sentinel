//! Match trace types for explaining routing decisions
//!
//! These types provide detailed information about why a route matched
//! or didn't match, enabling users to understand and debug their configurations.

use serde::{Deserialize, Serialize};

/// A step in the route matching trace
///
/// Each step represents the evaluation of one route against the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchStep {
    /// Route ID being evaluated
    pub route_id: String,

    /// Result of this evaluation
    pub result: MatchStepResult,

    /// Human-readable explanation of the result
    pub reason: String,

    /// Number of conditions checked
    pub conditions_checked: usize,

    /// Number of conditions that passed
    pub conditions_passed: usize,

    /// Details for each condition (if available)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub condition_details: Vec<ConditionDetail>,
}

/// Result of evaluating a route
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchStepResult {
    /// Route matched the request
    Match,
    /// Route did not match the request
    NoMatch,
    /// Route was skipped (e.g., lower priority than already-matched route)
    Skipped,
}

/// Details about a single match condition evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionDetail {
    /// Type of condition (PathPrefix, Path, Host, etc.)
    #[serde(rename = "type")]
    pub condition_type: String,

    /// The pattern being matched against
    pub pattern: String,

    /// Whether this condition matched
    pub matched: bool,

    /// The actual value from the request (for debugging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_value: Option<String>,

    /// Additional explanation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

impl MatchStep {
    /// Create a new match step for a successful match
    pub fn matched(route_id: String, conditions: Vec<ConditionDetail>) -> Self {
        let passed = conditions.iter().filter(|c| c.matched).count();
        Self {
            route_id,
            result: MatchStepResult::Match,
            reason: format!("All {} conditions matched", passed),
            conditions_checked: conditions.len(),
            conditions_passed: passed,
            condition_details: conditions,
        }
    }

    /// Create a new match step for a failed match
    pub fn no_match(route_id: String, conditions: Vec<ConditionDetail>) -> Self {
        let passed = conditions.iter().filter(|c| c.matched).count();
        let failed = conditions.iter().find(|c| !c.matched);

        let reason = if let Some(failed_condition) = failed {
            format!(
                "{} '{}' did not match",
                failed_condition.condition_type, failed_condition.pattern
            )
        } else {
            "No conditions matched".to_string()
        };

        Self {
            route_id,
            result: MatchStepResult::NoMatch,
            reason,
            conditions_checked: conditions.len(),
            conditions_passed: passed,
            condition_details: conditions,
        }
    }

    /// Create a new match step for a skipped route
    pub fn skipped(route_id: String, reason: &str) -> Self {
        Self {
            route_id,
            result: MatchStepResult::Skipped,
            reason: reason.to_string(),
            conditions_checked: 0,
            conditions_passed: 0,
            condition_details: Vec::new(),
        }
    }
}

impl ConditionDetail {
    /// Create a path prefix condition detail
    pub fn path_prefix(pattern: &str, path: &str, matched: bool) -> Self {
        Self {
            condition_type: "PathPrefix".to_string(),
            pattern: pattern.to_string(),
            matched,
            actual_value: Some(path.to_string()),
            explanation: if matched {
                Some(format!("Path '{}' starts with '{}'", path, pattern))
            } else {
                Some(format!("Path '{}' does not start with '{}'", path, pattern))
            },
        }
    }

    /// Create an exact path condition detail
    pub fn path(pattern: &str, path: &str, matched: bool) -> Self {
        Self {
            condition_type: "Path".to_string(),
            pattern: pattern.to_string(),
            matched,
            actual_value: Some(path.to_string()),
            explanation: if matched {
                Some(format!("Path '{}' exactly matches '{}'", path, pattern))
            } else {
                Some(format!("Path '{}' does not equal '{}'", path, pattern))
            },
        }
    }

    /// Create a path regex condition detail
    pub fn path_regex(pattern: &str, path: &str, matched: bool) -> Self {
        Self {
            condition_type: "PathRegex".to_string(),
            pattern: pattern.to_string(),
            matched,
            actual_value: Some(path.to_string()),
            explanation: if matched {
                Some(format!("Path '{}' matches regex '{}'", path, pattern))
            } else {
                Some(format!("Path '{}' does not match regex '{}'", path, pattern))
            },
        }
    }

    /// Create a host condition detail
    pub fn host(pattern: &str, host: &str, matched: bool) -> Self {
        Self {
            condition_type: "Host".to_string(),
            pattern: pattern.to_string(),
            matched,
            actual_value: Some(host.to_string()),
            explanation: if matched {
                Some(format!("Host '{}' matches '{}'", host, pattern))
            } else {
                Some(format!("Host '{}' does not match '{}'", host, pattern))
            },
        }
    }

    /// Create a method condition detail
    pub fn method(allowed_methods: &[String], actual_method: &str, matched: bool) -> Self {
        Self {
            condition_type: "Method".to_string(),
            pattern: allowed_methods.join(", "),
            matched,
            actual_value: Some(actual_method.to_string()),
            explanation: if matched {
                Some(format!(
                    "Method '{}' is in allowed list [{}]",
                    actual_method,
                    allowed_methods.join(", ")
                ))
            } else {
                Some(format!(
                    "Method '{}' is not in allowed list [{}]",
                    actual_method,
                    allowed_methods.join(", ")
                ))
            },
        }
    }

    /// Create a header condition detail
    pub fn header(
        name: &str,
        expected_value: Option<&str>,
        actual_value: Option<&str>,
        matched: bool,
    ) -> Self {
        let pattern = if let Some(v) = expected_value {
            format!("{}: {}", name, v)
        } else {
            format!("{} (presence)", name)
        };

        Self {
            condition_type: "Header".to_string(),
            pattern,
            matched,
            actual_value: actual_value.map(|s| s.to_string()),
            explanation: if matched {
                if expected_value.is_some() {
                    Some(format!("Header '{}' has expected value", name))
                } else {
                    Some(format!("Header '{}' is present", name))
                }
            } else if actual_value.is_none() {
                Some(format!("Header '{}' is not present", name))
            } else {
                Some(format!(
                    "Header '{}' value '{}' does not match expected '{}'",
                    name,
                    actual_value.unwrap_or(""),
                    expected_value.unwrap_or("")
                ))
            },
        }
    }

    /// Create a query param condition detail
    pub fn query_param(
        name: &str,
        expected_value: Option<&str>,
        actual_value: Option<&str>,
        matched: bool,
    ) -> Self {
        let pattern = if let Some(v) = expected_value {
            format!("{}={}", name, v)
        } else {
            format!("{} (presence)", name)
        };

        Self {
            condition_type: "QueryParam".to_string(),
            pattern,
            matched,
            actual_value: actual_value.map(|s| s.to_string()),
            explanation: if matched {
                if expected_value.is_some() {
                    Some(format!("Query param '{}' has expected value", name))
                } else {
                    Some(format!("Query param '{}' is present", name))
                }
            } else if actual_value.is_none() {
                Some(format!("Query param '{}' is not present", name))
            } else {
                Some(format!(
                    "Query param '{}' value '{}' does not match expected '{}'",
                    name,
                    actual_value.unwrap_or(""),
                    expected_value.unwrap_or("")
                ))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_step_matched() {
        let conditions = vec![
            ConditionDetail::path_prefix("/api", "/api/users", true),
            ConditionDetail::method(&["GET".to_string()], "GET", true),
        ];

        let step = MatchStep::matched("api-route".to_string(), conditions);

        assert_eq!(step.result, MatchStepResult::Match);
        assert_eq!(step.conditions_checked, 2);
        assert_eq!(step.conditions_passed, 2);
    }

    #[test]
    fn test_match_step_no_match() {
        let conditions = vec![
            ConditionDetail::path_prefix("/api", "/other", false),
        ];

        let step = MatchStep::no_match("api-route".to_string(), conditions);

        assert_eq!(step.result, MatchStepResult::NoMatch);
        assert!(step.reason.contains("PathPrefix"));
    }

    #[test]
    fn test_condition_detail_serialization() {
        let detail = ConditionDetail::path_prefix("/api", "/api/users", true);
        let json = serde_json::to_string(&detail).unwrap();

        assert!(json.contains("PathPrefix"));
        assert!(json.contains("/api"));
    }
}
