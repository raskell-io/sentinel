//! Certificate validation
//!
//! Validates TLS certificates including existence, expiry, and validity.

use super::{ErrorCategory, ValidationError, ValidationResult, ValidationWarning};
use crate::Config;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Validate TLS certificates
pub async fn validate_certificates(config: &Config) -> ValidationResult {
    let mut result = ValidationResult::new();

    for listener in &config.listeners {
        if let Some(ref tls) = listener.tls {
            // Check certificate file exists
            if !Path::new(&tls.cert_file).exists() {
                result.add_error(ValidationError::new(
                    ErrorCategory::Certificate,
                    format!("Certificate not found: {:?}", tls.cert_file),
                ));
                continue;
            }

            // Check key file exists
            if !Path::new(&tls.key_file).exists() {
                result.add_error(ValidationError::new(
                    ErrorCategory::Certificate,
                    format!("Private key not found: {:?}", tls.key_file),
                ));
                continue;
            }

            // Try to load and validate the certificate
            match load_and_validate_cert(&tls.cert_file) {
                Ok(Some(expiry_warning)) => {
                    result.add_warning(expiry_warning);
                }
                Ok(None) => {
                    // Certificate is valid
                }
                Err(e) => {
                    result.add_error(e);
                }
            }
        }
    }

    result
}

/// Load a certificate and check its expiry
fn load_and_validate_cert(cert_path: &Path) -> Result<Option<ValidationWarning>, ValidationError> {
    use std::fs;

    // Read certificate file
    let cert_pem = fs::read(cert_path).map_err(|e| {
        ValidationError::new(
            ErrorCategory::Certificate,
            format!("Failed to read certificate {:?}: {}", cert_path, e),
        )
    })?;

    // Parse PEM certificate
    let pem = pem::parse(&cert_pem).map_err(|e| {
        ValidationError::new(
            ErrorCategory::Certificate,
            format!("Failed to parse certificate {:?}: {}", cert_path, e),
        )
    })?;

    // Parse X509 certificate
    let (_, cert) = x509_parser::parse_x509_certificate(pem.contents()).map_err(|e| {
        ValidationError::new(
            ErrorCategory::Certificate,
            format!("Invalid X509 certificate {:?}: {}", cert_path, e),
        )
    })?;

    // Check expiry
    let now = SystemTime::now();
    let not_after = cert
        .validity()
        .not_after
        .to_datetime()
        .unix_timestamp() as u64;
    let expiry_time = SystemTime::UNIX_EPOCH + Duration::from_secs(not_after);

    if expiry_time < now {
        return Err(ValidationError::new(
            ErrorCategory::Certificate,
            format!(
                "Certificate expired: {:?} (expired at {})",
                cert_path,
                cert.validity().not_after
            ),
        ));
    }

    // Warn if expiring within 30 days
    let thirty_days = Duration::from_secs(30 * 86400);
    if expiry_time < now + thirty_days {
        return Ok(Some(ValidationWarning::new(format!(
            "Certificate expires soon: {:?} (expires at {})",
            cert_path,
            cert.validity().not_after
        ))));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ListenerConfig, ListenerProtocol, TlsConfig};
    use sentinel_common::types::TlsVersion;

    fn test_tls_config() -> TlsConfig {
        TlsConfig {
            cert_file: "/nonexistent/cert.pem".into(),
            key_file: "/nonexistent/key.pem".into(),
            additional_certs: vec![],
            ca_file: None,
            min_version: TlsVersion::Tls12,
            max_version: None,
            cipher_suites: vec![],
            client_auth: false,
            ocsp_stapling: false,
            session_resumption: false,
        }
    }

    fn test_listener_config() -> ListenerConfig {
        ListenerConfig {
            id: "test".to_string(),
            address: "0.0.0.0:443".to_string(),
            protocol: ListenerProtocol::Https,
            tls: Some(test_tls_config()),
            default_route: None,
            request_timeout_secs: 60,
            keepalive_timeout_secs: 75,
            max_concurrent_streams: 100,
        }
    }

    #[tokio::test]
    async fn test_validate_missing_certificate() {
        let mut config = Config::default_for_testing();
        config.listeners = vec![test_listener_config()];

        let result = validate_certificates(&config).await;

        assert!(!result.errors.is_empty());
        assert!(result
            .errors
            .iter()
            .any(|e| e.message.contains("Certificate not found")));
    }
}
