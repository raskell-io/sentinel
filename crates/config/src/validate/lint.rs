//! Configuration linting for best practices
//!
//! Checks configuration for missing best practices and potential issues.

use super::{ValidationResult, ValidationWarning};
use crate::Config;

/// Lint configuration for best practices
pub fn lint_config(config: &Config) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Check routes for missing best practices
    for route in &config.routes {
        // Check for missing retry policy
        if route.retry_policy.is_none() {
            result.add_warning(ValidationWarning::new(format!(
                "Route '{}' has no retry policy (recommended for production)",
                route.id
            )));
        }

        // Check for missing timeout
        if route.policies.timeout_secs.is_none() {
            result.add_warning(ValidationWarning::new(format!(
                "Route '{}' has no timeout (recommended for production)",
                route.id
            )));
        }

        // Check for missing upstream (skip for static and builtin service types)
        use crate::routes::ServiceType;
        if route.upstream.is_none()
            && !matches!(route.service_type, ServiceType::Static | ServiceType::Builtin)
        {
            result.add_warning(ValidationWarning::new(format!(
                "Route '{}' has no upstream configured",
                route.id
            )));
        }
    }

    // Check upstreams for missing health checks
    for (name, upstream) in &config.upstreams {
        if upstream.health_check.is_none() {
            result.add_warning(ValidationWarning::new(format!(
                "Upstream '{}' has no health check (recommended for production)",
                name
            )));
        }

        // Check for single target without health check
        if upstream.targets.len() == 1 && upstream.health_check.is_none() {
            result.add_warning(ValidationWarning::new(format!(
                "Upstream '{}' has only one target and no health check (no failover possible)",
                name
            )));
        }
    }

    // Check listeners for security best practices
    for listener in &config.listeners {
        // Check for HTTP listener on standard port without redirect to HTTPS
        if listener.address.ends_with(":80") && listener.tls.is_none() {
            result.add_warning(ValidationWarning::new(format!(
                "Listener '{}' serves HTTP on port 80 without TLS (consider HTTPS redirect)",
                listener.address
            )));
        }

        // Check for TLS listener without HSTS
        if listener.tls.is_some() {
            // TODO: Check for HSTS header in security policies
            // This would require inspecting route policies
        }
    }

    // Check observability configuration
    if !config.observability.metrics.enabled {
        result.add_warning(ValidationWarning::new(
            "Metrics are disabled (recommended for production monitoring)".to_string(),
        ));
    }

    // Check for access logs
    if let Some(ref access_log) = config.observability.logging.access_log {
        if !access_log.enabled {
            result.add_warning(ValidationWarning::new(
                "Access logs are disabled (recommended for debugging and compliance)".to_string(),
            ));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ListenerConfig, RouteConfig, UpstreamConfig, UpstreamTarget};
    use std::collections::HashMap;

    #[test]
    fn test_lint_missing_retry_policy() {
        let config = Config {
            routes: vec![RouteConfig {
                id: "test".to_string(),
                retry_policy: None,
                ..Default::default()
            }],
            ..Default::default()
        };

        let result = lint_config(&config);

        assert!(result
            .warnings
            .iter()
            .any(|w| w.message.contains("no retry policy")));
    }

    #[test]
    fn test_lint_missing_health_check() {
        let mut upstreams = HashMap::new();
        upstreams.insert(
            "test".to_string(),
            UpstreamConfig {
                targets: vec![UpstreamTarget {
                    address: "127.0.0.1:8080".to_string(),
                    weight: 1,
                }],
                health_check: None,
                ..Default::default()
            },
        );

        let config = Config {
            upstreams,
            ..Default::default()
        };

        let result = lint_config(&config);

        assert!(result
            .warnings
            .iter()
            .any(|w| w.message.contains("no health check")));
    }

    #[test]
    fn test_lint_http_on_port_80() {
        let config = Config {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".to_string(),
                tls: None,
                ..Default::default()
            }],
            ..Default::default()
        };

        let result = lint_config(&config);

        assert!(result
            .warnings
            .iter()
            .any(|w| w.message.contains("without TLS")));
    }
}
