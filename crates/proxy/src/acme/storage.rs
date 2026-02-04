//! Certificate and account storage for ACME
//!
//! Provides persistent storage for ACME account credentials and issued certificates.
//!
//! # Directory Structure
//!
//! ```text
//! storage/
//! ├── account.json          # ACME account credentials (opaque, serialized)
//! └── domains/
//!     └── example.com/
//!         ├── cert.pem      # Certificate chain
//!         ├── key.pem       # Private key
//!         └── meta.json     # Certificate metadata (expiry, issued date)
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace, warn};

use super::error::StorageError;

/// Certificate metadata stored alongside the certificate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateMeta {
    /// When the certificate expires
    pub expires: DateTime<Utc>,
    /// When the certificate was issued
    pub issued: DateTime<Utc>,
    /// Domains covered by this certificate
    pub domains: Vec<String>,
    /// Issuer (e.g., "Let's Encrypt")
    #[serde(default)]
    pub issuer: Option<String>,
}

/// A stored certificate with its metadata
#[derive(Debug, Clone)]
pub struct StoredCertificate {
    /// PEM-encoded certificate chain
    pub cert_pem: String,
    /// PEM-encoded private key
    pub key_pem: String,
    /// Certificate metadata
    pub meta: CertificateMeta,
}

/// ACME account metadata for storage
///
/// Stores metadata about the ACME account alongside the credentials JSON.
/// The actual `instant_acme::AccountCredentials` is stored separately as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccountCredentials {
    /// Contact email (for reference)
    #[serde(default)]
    pub contact_email: Option<String>,
    /// When the account was created
    pub created: DateTime<Utc>,
}

/// Certificate storage manager
///
/// Handles persistent storage of ACME account credentials and certificates.
/// Uses a simple filesystem-based storage with restrictive permissions.
#[derive(Debug)]
pub struct CertificateStorage {
    /// Base storage directory
    base_path: PathBuf,
}

impl CertificateStorage {
    /// Create a new certificate storage at the given path
    ///
    /// Creates the directory structure if it doesn't exist and sets
    /// restrictive permissions (0700 on Unix).
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or permissions
    /// cannot be set.
    pub fn new(base_path: &Path) -> Result<Self, StorageError> {
        // Create base directory
        fs::create_dir_all(base_path)?;

        // Create domains subdirectory
        let domains_path = base_path.join("domains");
        fs::create_dir_all(&domains_path)?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o700);
            fs::set_permissions(base_path, perms.clone())?;
            fs::set_permissions(&domains_path, perms)?;
        }

        info!(
            storage_path = %base_path.display(),
            "Initialized ACME certificate storage"
        );

        Ok(Self {
            base_path: base_path.to_path_buf(),
        })
    }

    /// Get the storage base path
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    // =========================================================================
    // Account Operations
    // =========================================================================

    /// Load stored account credentials
    pub fn load_account(&self) -> Result<Option<StoredAccountCredentials>, StorageError> {
        let account_path = self.base_path.join("account.json");

        if !account_path.exists() {
            trace!("No stored ACME account found");
            return Ok(None);
        }

        let content = fs::read_to_string(&account_path)?;
        let creds: StoredAccountCredentials = serde_json::from_str(&content)?;

        debug!(
            contact = ?creds.contact_email,
            created = %creds.created,
            "Loaded ACME account credentials"
        );
        Ok(Some(creds))
    }

    /// Save account credentials
    pub fn save_account(&self, creds: &StoredAccountCredentials) -> Result<(), StorageError> {
        let account_path = self.base_path.join("account.json");
        let content = serde_json::to_string_pretty(creds)?;
        fs::write(&account_path, content)?;

        // Set restrictive permissions on the account file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&account_path, fs::Permissions::from_mode(0o600))?;
        }

        info!(contact = ?creds.contact_email, "Saved ACME account credentials");
        Ok(())
    }

    /// Load raw credentials JSON (for instant_acme::AccountCredentials)
    pub fn load_credentials_json(&self) -> Result<Option<String>, StorageError> {
        let creds_path = self.base_path.join("credentials.json");

        if !creds_path.exists() {
            trace!("No stored ACME credentials found");
            return Ok(None);
        }

        let content = fs::read_to_string(&creds_path)?;
        debug!("Loaded ACME credentials JSON");
        Ok(Some(content))
    }

    /// Save raw credentials JSON (for instant_acme::AccountCredentials)
    pub fn save_credentials_json(&self, json: &str) -> Result<(), StorageError> {
        let creds_path = self.base_path.join("credentials.json");
        fs::write(&creds_path, json)?;

        // Set restrictive permissions on the credentials file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&creds_path, fs::Permissions::from_mode(0o600))?;
        }

        info!("Saved ACME credentials JSON");
        Ok(())
    }

    // =========================================================================
    // Certificate Operations
    // =========================================================================

    /// Get the path to a domain's certificate directory
    fn domain_path(&self, domain: &str) -> PathBuf {
        self.base_path.join("domains").join(domain)
    }

    /// Load a stored certificate for a domain
    pub fn load_certificate(
        &self,
        domain: &str,
    ) -> Result<Option<StoredCertificate>, StorageError> {
        let domain_path = self.domain_path(domain);
        let cert_path = domain_path.join("cert.pem");
        let key_path = domain_path.join("key.pem");
        let meta_path = domain_path.join("meta.json");

        if !cert_path.exists() {
            trace!(domain = %domain, "No stored certificate found");
            return Ok(None);
        }

        let cert_pem = fs::read_to_string(&cert_path)?;
        let key_pem = fs::read_to_string(&key_path)?;
        let meta_content = fs::read_to_string(&meta_path)?;
        let meta: CertificateMeta = serde_json::from_str(&meta_content)?;

        debug!(
            domain = %domain,
            expires = %meta.expires,
            "Loaded stored certificate"
        );

        Ok(Some(StoredCertificate {
            cert_pem,
            key_pem,
            meta,
        }))
    }

    /// Save a certificate for a domain
    pub fn save_certificate(
        &self,
        domain: &str,
        cert_pem: &str,
        key_pem: &str,
        expires: DateTime<Utc>,
        all_domains: &[String],
    ) -> Result<(), StorageError> {
        let domain_path = self.domain_path(domain);
        fs::create_dir_all(&domain_path)?;

        let cert_path = domain_path.join("cert.pem");
        let key_path = domain_path.join("key.pem");
        let meta_path = domain_path.join("meta.json");

        // Write certificate
        fs::write(&cert_path, cert_pem)?;

        // Write private key with restrictive permissions
        fs::write(&key_path, key_pem)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
        }

        // Write metadata
        let meta = CertificateMeta {
            expires,
            issued: Utc::now(),
            domains: all_domains.to_vec(),
            issuer: Some("Let's Encrypt".to_string()),
        };
        let meta_content = serde_json::to_string_pretty(&meta)?;
        fs::write(&meta_path, meta_content)?;

        info!(
            domain = %domain,
            expires = %expires,
            "Saved certificate to storage"
        );

        Ok(())
    }

    /// Check if a certificate needs renewal
    ///
    /// Returns `true` if:
    /// - No certificate exists for the domain
    /// - Certificate expires within `renew_before_days` days
    pub fn needs_renewal(
        &self,
        domain: &str,
        renew_before_days: u32,
    ) -> Result<bool, StorageError> {
        let Some(cert) = self.load_certificate(domain)? else {
            debug!(domain = %domain, "No certificate exists, needs issuance");
            return Ok(true);
        };

        let renew_threshold = Utc::now() + chrono::Duration::days(i64::from(renew_before_days));
        let needs_renewal = cert.meta.expires <= renew_threshold;

        if needs_renewal {
            debug!(
                domain = %domain,
                expires = %cert.meta.expires,
                threshold = %renew_threshold,
                "Certificate needs renewal"
            );
        } else {
            trace!(
                domain = %domain,
                expires = %cert.meta.expires,
                "Certificate is still valid"
            );
        }

        Ok(needs_renewal)
    }

    /// Get certificate paths for a domain
    ///
    /// Returns the paths to cert.pem and key.pem if they exist.
    pub fn certificate_paths(&self, domain: &str) -> Option<(PathBuf, PathBuf)> {
        let domain_path = self.domain_path(domain);
        let cert_path = domain_path.join("cert.pem");
        let key_path = domain_path.join("key.pem");

        if cert_path.exists() && key_path.exists() {
            Some((cert_path, key_path))
        } else {
            None
        }
    }

    /// List all stored domains
    pub fn list_domains(&self) -> Result<Vec<String>, StorageError> {
        let domains_path = self.base_path.join("domains");

        if !domains_path.exists() {
            return Ok(Vec::new());
        }

        let mut domains = Vec::new();
        for entry in fs::read_dir(&domains_path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    domains.push(name.to_string());
                }
            }
        }

        Ok(domains)
    }

    /// Delete stored certificate for a domain
    pub fn delete_certificate(&self, domain: &str) -> Result<(), StorageError> {
        let domain_path = self.domain_path(domain);

        if domain_path.exists() {
            fs::remove_dir_all(&domain_path)?;
            info!(domain = %domain, "Deleted stored certificate");
        } else {
            warn!(domain = %domain, "Certificate to delete not found");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_storage() -> (TempDir, CertificateStorage) {
        let temp_dir = TempDir::new().unwrap();
        let storage = CertificateStorage::new(temp_dir.path()).unwrap();
        (temp_dir, storage)
    }

    #[test]
    fn test_storage_creation() {
        let (_temp_dir, storage) = setup_storage();
        assert!(storage.base_path().exists());
        assert!(storage.base_path().join("domains").exists());
    }

    #[test]
    fn test_credentials_json_save_load() {
        let (_temp_dir, storage) = setup_storage();

        let test_json = r#"{"test": "credentials"}"#;
        storage.save_credentials_json(test_json).unwrap();

        let loaded = storage.load_credentials_json().unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap(), test_json);
    }

    #[test]
    fn test_certificate_save_load() {
        let (_temp_dir, storage) = setup_storage();

        let cert_pem = "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----";
        let key_pem = "-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----";
        let expires = Utc::now() + chrono::Duration::days(90);

        storage
            .save_certificate(
                "example.com",
                cert_pem,
                key_pem,
                expires,
                &["example.com".to_string()],
            )
            .unwrap();

        let loaded = storage.load_certificate("example.com").unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.cert_pem, cert_pem);
        assert_eq!(loaded.key_pem, key_pem);
    }

    #[test]
    fn test_needs_renewal_no_cert() {
        let (_temp_dir, storage) = setup_storage();
        assert!(storage.needs_renewal("nonexistent.com", 30).unwrap());
    }

    #[test]
    fn test_needs_renewal_expiring_soon() {
        let (_temp_dir, storage) = setup_storage();

        // Save a certificate expiring in 15 days
        let expires = Utc::now() + chrono::Duration::days(15);
        storage
            .save_certificate(
                "expiring.com",
                "cert",
                "key",
                expires,
                &["expiring.com".to_string()],
            )
            .unwrap();

        // Should need renewal if we renew 30 days before expiry
        assert!(storage.needs_renewal("expiring.com", 30).unwrap());
    }

    #[test]
    fn test_needs_renewal_still_valid() {
        let (_temp_dir, storage) = setup_storage();

        // Save a certificate expiring in 60 days
        let expires = Utc::now() + chrono::Duration::days(60);
        storage
            .save_certificate(
                "valid.com",
                "cert",
                "key",
                expires,
                &["valid.com".to_string()],
            )
            .unwrap();

        // Should NOT need renewal if we renew 30 days before expiry
        assert!(!storage.needs_renewal("valid.com", 30).unwrap());
    }

    #[test]
    fn test_list_domains() {
        let (_temp_dir, storage) = setup_storage();

        storage
            .save_certificate(
                "a.com",
                "cert",
                "key",
                Utc::now() + chrono::Duration::days(90),
                &["a.com".to_string()],
            )
            .unwrap();
        storage
            .save_certificate(
                "b.com",
                "cert",
                "key",
                Utc::now() + chrono::Duration::days(90),
                &["b.com".to_string()],
            )
            .unwrap();

        let domains = storage.list_domains().unwrap();
        assert_eq!(domains.len(), 2);
        assert!(domains.contains(&"a.com".to_string()));
        assert!(domains.contains(&"b.com".to_string()));
    }

    #[test]
    fn test_delete_certificate() {
        let (_temp_dir, storage) = setup_storage();

        storage
            .save_certificate(
                "delete.com",
                "cert",
                "key",
                Utc::now() + chrono::Duration::days(90),
                &["delete.com".to_string()],
            )
            .unwrap();

        assert!(storage.load_certificate("delete.com").unwrap().is_some());

        storage.delete_certificate("delete.com").unwrap();

        assert!(storage.load_certificate("delete.com").unwrap().is_none());
    }
}
