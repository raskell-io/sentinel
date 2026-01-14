//! ACME automatic certificate management
//!
//! Provides zero-config TLS via Let's Encrypt and compatible CAs.
//!
//! # Features
//!
//! - Automatic certificate issuance and renewal
//! - HTTP-01 challenge handling
//! - Persistent storage for certificates and account credentials
//! - Background renewal scheduler
//!
//! # Architecture
//!
//! The ACME module consists of four main components:
//!
//! - [`AcmeClient`] - Wrapper around `instant-acme` for ACME protocol operations
//! - [`CertificateStorage`] - Persistent storage for certificates and account keys
//! - [`ChallengeManager`] - Manages pending HTTP-01 challenges for serving
//! - [`RenewalScheduler`] - Background task for checking and renewing certificates
//!
//! # Example
//!
//! ```kdl
//! listener "https" {
//!     address "0.0.0.0:443"
//!     protocol "https"
//!
//!     tls {
//!         acme {
//!             email "admin@example.com"
//!             domains "example.com" "www.example.com"
//!             staging false
//!             storage "/var/lib/sentinel/acme"
//!             renew-before-days 30
//!         }
//!     }
//! }
//! ```
//!
//! # Challenge Flow
//!
//! When a certificate needs to be obtained or renewed:
//!
//! 1. [`AcmeClient`] creates a new order with the ACME server
//! 2. For each domain, the ACME server provides a challenge token
//! 3. [`ChallengeManager`] registers the token and key authorization
//! 4. The ACME server validates by requesting `/.well-known/acme-challenge/<token>`
//! 5. Sentinel's request filter intercepts and returns the key authorization
//! 6. Once validated, [`AcmeClient`] finalizes the order and receives the certificate
//! 7. [`CertificateStorage`] persists the certificate and triggers TLS reload

mod challenge;
mod client;
mod error;
mod scheduler;
mod storage;

pub use challenge::ChallengeManager;
pub use client::AcmeClient;
pub use error::AcmeError;
pub use scheduler::RenewalScheduler;
pub use storage::CertificateStorage;
