//! Agent connectivity validation
//!
//! Validates that agent sockets are reachable.

use super::{ErrorCategory, ValidationError, ValidationResult};
use crate::Config;

/// Validate agent connectivity
pub async fn validate_agents(config: &Config) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Collect unique filter names from all routes
    let mut filter_names = std::collections::HashSet::new();
    for route in &config.routes {
        for filter_name in &route.filters {
            filter_names.insert(filter_name.clone());
        }
    }

    // Check if each filter exists in config
    for filter_name in &filter_names {
        if !config.filters.contains_key(filter_name) {
            result.add_error(ValidationError::new(
                ErrorCategory::Agent,
                format!("Filter '{}' referenced in route but not defined", filter_name),
            ));
            continue;
        }

        // TODO: For agent filters, check socket connectivity
        // This would require knowing which filters are agent-based
        // For now, we just check that the filter is defined
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MatchCondition, RoutePolicies, RouteConfig, ServiceType};
    use sentinel_common::types::Priority;

    fn test_route_with_filter(filter: &str) -> RouteConfig {
        RouteConfig {
            id: "test".to_string(),
            priority: Priority::Normal,
            matches: vec![MatchCondition::PathPrefix("/".to_string())],
            upstream: None,
            service_type: ServiceType::Web,
            policies: RoutePolicies::default(),
            filters: vec![filter.to_string()],
            builtin_handler: None,
            waf_enabled: false,
            circuit_breaker: None,
            retry_policy: None,
            static_files: None,
            api_schema: None,
            error_pages: None,
            websocket: false,
            websocket_inspection: false,
            inference: None,
            shadow: None,
        }
    }

    #[tokio::test]
    async fn test_validate_missing_filter() {
        let mut config = Config::default_for_testing();
        config.routes = vec![test_route_with_filter("nonexistent-filter")];

        let result = validate_agents(&config).await;

        // Should have an error about undefined filter
        assert!(!result.errors.is_empty());
        assert!(result
            .errors
            .iter()
            .any(|e| e.message.contains("not defined")));
    }
}
