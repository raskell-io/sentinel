//! Network connectivity validation
//!
//! Validates that upstream targets are reachable.

use super::{ErrorCategory, ValidationError, ValidationResult, ValidationWarning};
use crate::Config;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Validate upstream connectivity
pub async fn validate_upstreams(config: &Config) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (name, upstream) in &config.upstreams {
        for target in &upstream.targets {
            // Try to connect to upstream with timeout
            match timeout(
                Duration::from_secs(5),
                TcpStream::connect(&target.address),
            )
            .await
            {
                Ok(Ok(_)) => {
                    // Connection successful
                }
                Ok(Err(e)) => {
                    result.add_error(ValidationError::new(
                        ErrorCategory::Network,
                        format!(
                            "Upstream '{}' target '{}' unreachable: {}",
                            name, target.address, e
                        ),
                    ));
                }
                Err(_) => {
                    result.add_warning(ValidationWarning::new(format!(
                        "Upstream '{}' target '{}' connection timeout (5s)",
                        name, target.address
                    )));
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{UpstreamConfig, UpstreamTarget};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_validate_upstreams_unreachable() {
        let mut upstreams = HashMap::new();
        upstreams.insert(
            "test".to_string(),
            UpstreamConfig {
                targets: vec![UpstreamTarget {
                    address: "192.0.2.1:9999".to_string(), // TEST-NET-1 (unreachable)
                    weight: 1,
                }],
                ..Default::default()
            },
        );

        let config = Config {
            upstreams,
            ..Default::default()
        };

        let result = validate_upstreams(&config).await;

        // Should have either an error or warning (depending on timeout)
        assert!(!result.errors.is_empty() || !result.warnings.is_empty());
    }
}
