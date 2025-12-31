//! GeoIP filtering for Sentinel proxy
//!
//! This module provides geolocation-based request filtering using MaxMind GeoLite2/GeoIP2
//! and IP2Location databases. Filters can block, allow, or log requests based on country.
//!
//! # Features
//! - Support for MaxMind (.mmdb) and IP2Location (.bin) databases
//! - Block mode (blocklist) and Allow mode (allowlist)
//! - Log-only mode for monitoring without blocking
//! - Per-filter IP→Country caching with configurable TTL
//! - Configurable fail-open/fail-closed on lookup errors
//! - X-GeoIP-Country response header injection

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, trace, warn};

use sentinel_config::{GeoDatabaseType, GeoFailureMode, GeoFilter, GeoFilterAction};

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during geo lookup
#[derive(Debug, Clone)]
pub enum GeoLookupError {
    /// IP address could not be parsed
    InvalidIp(String),
    /// Database error during lookup
    DatabaseError(String),
    /// Database file could not be loaded
    LoadError(String),
}

impl std::fmt::Display for GeoLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeoLookupError::InvalidIp(ip) => write!(f, "invalid IP address: {}", ip),
            GeoLookupError::DatabaseError(msg) => write!(f, "database error: {}", msg),
            GeoLookupError::LoadError(msg) => write!(f, "failed to load database: {}", msg),
        }
    }
}

impl std::error::Error for GeoLookupError {}

// =============================================================================
// GeoDatabase Trait
// =============================================================================

/// Trait for GeoIP database backends
pub trait GeoDatabase: Send + Sync {
    /// Look up the country code for an IP address
    fn lookup(&self, ip: IpAddr) -> Result<Option<String>, GeoLookupError>;

    /// Get the database type
    fn database_type(&self) -> GeoDatabaseType;
}

// =============================================================================
// MaxMind Database Backend
// =============================================================================

/// MaxMind GeoLite2/GeoIP2 database backend
pub struct MaxMindDatabase {
    reader: maxminddb::Reader<Vec<u8>>,
}

impl MaxMindDatabase {
    /// Open a MaxMind database file
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GeoLookupError> {
        let path = path.as_ref();
        let reader = maxminddb::Reader::open_readfile(path).map_err(|e| {
            GeoLookupError::LoadError(format!("failed to open MaxMind database {:?}: {}", path, e))
        })?;

        debug!(path = ?path, "Opened MaxMind GeoIP database");
        Ok(Self { reader })
    }
}

impl GeoDatabase for MaxMindDatabase {
    fn lookup(&self, ip: IpAddr) -> Result<Option<String>, GeoLookupError> {
        match self.reader.lookup::<maxminddb::geoip2::Country>(ip) {
            Ok(record) => {
                let country_code = record
                    .country
                    .and_then(|c| c.iso_code)
                    .map(|s| s.to_string());
                trace!(ip = %ip, country = ?country_code, "MaxMind lookup");
                Ok(country_code)
            }
            Err(maxminddb::MaxMindDBError::AddressNotFoundError(_)) => {
                trace!(ip = %ip, "IP not found in MaxMind database");
                Ok(None)
            }
            Err(e) => {
                warn!(ip = %ip, error = %e, "MaxMind lookup error");
                Err(GeoLookupError::DatabaseError(e.to_string()))
            }
        }
    }

    fn database_type(&self) -> GeoDatabaseType {
        GeoDatabaseType::MaxMind
    }
}

// =============================================================================
// IP2Location Database Backend
// =============================================================================

/// IP2Location database backend
pub struct Ip2LocationDatabase {
    db: ip2location::DB,
}

impl Ip2LocationDatabase {
    /// Open an IP2Location database file
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GeoLookupError> {
        let path = path.as_ref();
        let db = ip2location::DB::from_file(path).map_err(|e| {
            GeoLookupError::LoadError(format!(
                "failed to open IP2Location database {:?}: {}",
                path, e
            ))
        })?;

        debug!(path = ?path, "Opened IP2Location GeoIP database");
        Ok(Self { db })
    }
}

impl GeoDatabase for Ip2LocationDatabase {
    fn lookup(&self, ip: IpAddr) -> Result<Option<String>, GeoLookupError> {
        match self.db.ip_lookup(ip) {
            Ok(record) => {
                // Record is an enum - extract country from the LocationDb variant
                let country_code = match record {
                    ip2location::Record::LocationDb(loc) => {
                        loc.country.map(|c| c.short_name.to_string())
                    }
                    ip2location::Record::ProxyDb(proxy) => {
                        proxy.country.map(|c| c.short_name.to_string())
                    }
                };
                trace!(ip = %ip, country = ?country_code, "IP2Location lookup");
                Ok(country_code)
            }
            Err(ip2location::error::Error::RecordNotFound) => {
                trace!(ip = %ip, "IP not found in IP2Location database");
                Ok(None)
            }
            Err(e) => {
                warn!(ip = %ip, error = %e, "IP2Location lookup error");
                Err(GeoLookupError::DatabaseError(e.to_string()))
            }
        }
    }

    fn database_type(&self) -> GeoDatabaseType {
        GeoDatabaseType::Ip2Location
    }
}

// =============================================================================
// Cached Country Entry
// =============================================================================

/// Cached country lookup result
struct CachedCountry {
    /// The country code (or None if not found)
    country_code: Option<String>,
    /// When this entry was cached
    cached_at: Instant,
}

// =============================================================================
// GeoFilterResult
// =============================================================================

/// Result of a geo filter check
#[derive(Debug, Clone)]
pub struct GeoFilterResult {
    /// Whether the request is allowed
    pub allowed: bool,
    /// The country code (if found)
    pub country_code: Option<String>,
    /// Whether this was a cache hit
    pub cache_hit: bool,
    /// Whether to add the country header
    pub add_header: bool,
    /// HTTP status code to return if blocked
    pub status_code: u16,
    /// Block message to return if blocked
    pub block_message: Option<String>,
}

// =============================================================================
// GeoFilterPool
// =============================================================================

/// A single geo filter instance with its database and cache
pub struct GeoFilterPool {
    /// The underlying GeoIP database
    database: Arc<dyn GeoDatabase>,
    /// IP → Country cache
    cache: DashMap<IpAddr, CachedCountry>,
    /// Filter configuration
    config: GeoFilter,
    /// Pre-computed set of countries for fast lookup
    countries_set: HashSet<String>,
    /// Cache TTL duration
    cache_ttl: Duration,
}

impl GeoFilterPool {
    /// Create a new geo filter pool from configuration
    pub fn new(config: GeoFilter) -> Result<Self, GeoLookupError> {
        // Determine database type (auto-detect from extension if not specified)
        let db_type = config.database_type.clone().unwrap_or_else(|| {
            if config.database_path.ends_with(".mmdb") {
                GeoDatabaseType::MaxMind
            } else {
                GeoDatabaseType::Ip2Location
            }
        });

        // Open the database
        let database: Arc<dyn GeoDatabase> = match db_type {
            GeoDatabaseType::MaxMind => Arc::new(MaxMindDatabase::open(&config.database_path)?),
            GeoDatabaseType::Ip2Location => {
                Arc::new(Ip2LocationDatabase::open(&config.database_path)?)
            }
        };

        // Build countries set for fast lookup
        let countries_set: HashSet<String> = config.countries.iter().cloned().collect();

        let cache_ttl = Duration::from_secs(config.cache_ttl_secs);

        debug!(
            database_path = %config.database_path,
            database_type = ?db_type,
            action = ?config.action,
            countries_count = countries_set.len(),
            cache_ttl_secs = config.cache_ttl_secs,
            "Created GeoFilterPool"
        );

        Ok(Self {
            database,
            cache: DashMap::new(),
            config,
            countries_set,
            cache_ttl,
        })
    }

    /// Check if a client IP should be allowed or blocked
    pub fn check(&self, client_ip: &str) -> GeoFilterResult {
        // Parse the IP address
        let ip: IpAddr = match client_ip.parse() {
            Ok(ip) => ip,
            Err(_) => {
                warn!(client_ip = %client_ip, "Failed to parse client IP for geo filter");
                return self.handle_failure();
            }
        };

        // Check cache first
        let now = Instant::now();
        if let Some(entry) = self.cache.get(&ip) {
            if now.duration_since(entry.cached_at) < self.cache_ttl {
                trace!(ip = %ip, country = ?entry.country_code, "Geo cache hit");
                return self.evaluate(entry.country_code.clone(), true);
            }
            // Entry expired, will be replaced
        }

        // Lookup in database
        match self.database.lookup(ip) {
            Ok(country_code) => {
                // Cache the result
                self.cache.insert(
                    ip,
                    CachedCountry {
                        country_code: country_code.clone(),
                        cached_at: now,
                    },
                );
                self.evaluate(country_code, false)
            }
            Err(e) => {
                warn!(ip = %ip, error = %e, "Geo lookup failed");
                self.handle_failure()
            }
        }
    }

    /// Evaluate the filter action based on country code
    fn evaluate(&self, country_code: Option<String>, cache_hit: bool) -> GeoFilterResult {
        let in_list = country_code
            .as_ref()
            .map(|c| self.countries_set.contains(c))
            .unwrap_or(false);

        let allowed = match self.config.action {
            GeoFilterAction::Block => {
                // Block mode: block if country is in the list
                !in_list
            }
            GeoFilterAction::Allow => {
                // Allow mode: allow only if country is in the list
                // If no country found and list is not empty, block
                if self.countries_set.is_empty() {
                    true
                } else {
                    in_list
                }
            }
            GeoFilterAction::LogOnly => {
                // Log-only mode: always allow
                true
            }
        };

        trace!(
            country = ?country_code,
            in_list = in_list,
            action = ?self.config.action,
            allowed = allowed,
            "Geo filter evaluation"
        );

        GeoFilterResult {
            allowed,
            country_code,
            cache_hit,
            add_header: self.config.add_country_header,
            status_code: self.config.status_code,
            block_message: self.config.block_message.clone(),
        }
    }

    /// Handle lookup failure based on failure mode
    fn handle_failure(&self) -> GeoFilterResult {
        let allowed = match self.config.on_failure {
            GeoFailureMode::Open => true,
            GeoFailureMode::Closed => false,
        };

        GeoFilterResult {
            allowed,
            country_code: None,
            cache_hit: false,
            add_header: false,
            status_code: self.config.status_code,
            block_message: self.config.block_message.clone(),
        }
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> (usize, usize) {
        let now = Instant::now();
        let total = self.cache.len();
        let valid = self
            .cache
            .iter()
            .filter(|e| now.duration_since(e.cached_at) < self.cache_ttl)
            .count();
        (total, valid)
    }

    /// Clear expired cache entries
    pub fn clear_expired(&self) {
        let now = Instant::now();
        self.cache
            .retain(|_, v| now.duration_since(v.cached_at) < self.cache_ttl);
    }
}

// =============================================================================
// GeoFilterManager
// =============================================================================

/// Manages all geo filter instances
pub struct GeoFilterManager {
    /// Filter ID → GeoFilterPool mapping
    filter_pools: DashMap<String, Arc<GeoFilterPool>>,
}

impl GeoFilterManager {
    /// Create a new empty geo filter manager
    pub fn new() -> Self {
        Self {
            filter_pools: DashMap::new(),
        }
    }

    /// Register a geo filter from configuration
    pub fn register_filter(
        &self,
        filter_id: &str,
        config: GeoFilter,
    ) -> Result<(), GeoLookupError> {
        let pool = GeoFilterPool::new(config)?;
        self.filter_pools
            .insert(filter_id.to_string(), Arc::new(pool));
        debug!(filter_id = %filter_id, "Registered geo filter");
        Ok(())
    }

    /// Check a client IP against a specific filter
    pub fn check(&self, filter_id: &str, client_ip: &str) -> Option<GeoFilterResult> {
        self.filter_pools
            .get(filter_id)
            .map(|pool| pool.check(client_ip))
    }

    /// Get a reference to a filter pool
    pub fn get_pool(&self, filter_id: &str) -> Option<Arc<GeoFilterPool>> {
        self.filter_pools.get(filter_id).map(|r| r.clone())
    }

    /// Check if a filter exists
    pub fn has_filter(&self, filter_id: &str) -> bool {
        self.filter_pools.contains_key(filter_id)
    }

    /// Get all filter IDs
    pub fn filter_ids(&self) -> Vec<String> {
        self.filter_pools.iter().map(|r| r.key().clone()).collect()
    }

    /// Clear expired cache entries in all pools
    pub fn clear_expired_caches(&self) {
        for pool in self.filter_pools.iter() {
            pool.clear_expired();
        }
    }
}

impl Default for GeoFilterManager {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_geo_lookup_error_display() {
        let err = GeoLookupError::InvalidIp("not-an-ip".to_string());
        assert!(err.to_string().contains("invalid IP"));

        let err = GeoLookupError::DatabaseError("db error".to_string());
        assert!(err.to_string().contains("database error"));

        let err = GeoLookupError::LoadError("load error".to_string());
        assert!(err.to_string().contains("failed to load"));
    }

    #[test]
    fn test_geo_filter_result_default() {
        let result = GeoFilterResult {
            allowed: true,
            country_code: Some("US".to_string()),
            cache_hit: false,
            add_header: true,
            status_code: 403,
            block_message: None,
        };

        assert!(result.allowed);
        assert_eq!(result.country_code, Some("US".to_string()));
        assert!(!result.cache_hit);
        assert!(result.add_header);
    }

    #[test]
    fn test_geo_filter_manager_new() {
        let manager = GeoFilterManager::new();
        assert!(manager.filter_ids().is_empty());
        assert!(!manager.has_filter("test"));
    }

    // Integration tests would require actual database files
    // These are covered in the integration test suite
}
