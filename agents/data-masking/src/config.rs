//! Configuration schema for the Data Masking Agent.

use serde::{Deserialize, Serialize};

/// Root configuration for the Data Masking Agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataMaskingConfig {
    /// Token store configuration.
    #[serde(default)]
    pub store: TokenStoreConfig,

    /// Field masking rules.
    #[serde(default)]
    pub fields: Vec<FieldRule>,

    /// Header masking rules.
    #[serde(default)]
    pub headers: Vec<HeaderRule>,

    /// Pattern definitions (built-in + custom).
    #[serde(default)]
    pub patterns: PatternConfig,

    /// Format-preserving encryption settings.
    #[serde(default)]
    pub fpe: FpeConfig,

    /// Buffering settings for streaming.
    #[serde(default)]
    pub buffering: BufferingConfig,
}

impl Default for DataMaskingConfig {
    fn default() -> Self {
        Self {
            store: TokenStoreConfig::default(),
            fields: Vec::new(),
            headers: Vec::new(),
            patterns: PatternConfig::default(),
            fpe: FpeConfig::default(),
            buffering: BufferingConfig::default(),
        }
    }
}

/// Token store configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TokenStoreConfig {
    /// In-memory store (single instance, non-distributed).
    Memory {
        /// Default TTL for tokens in seconds.
        #[serde(default = "default_ttl")]
        ttl_seconds: u64,
        /// Maximum entries (LRU eviction).
        #[serde(default = "default_max_entries")]
        max_entries: usize,
    },
}

impl Default for TokenStoreConfig {
    fn default() -> Self {
        Self::Memory {
            ttl_seconds: default_ttl(),
            max_entries: default_max_entries(),
        }
    }
}

fn default_ttl() -> u64 {
    300
} // 5 minutes
fn default_max_entries() -> usize {
    100_000
}

/// Field masking rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldRule {
    /// JSON path, XPath, or form field name.
    pub path: String,

    /// Path type (auto-detected from content-type if not specified).
    #[serde(default)]
    pub path_type: Option<PathType>,

    /// Masking action.
    pub action: MaskingAction,

    /// Apply to request, response, or both.
    #[serde(default)]
    pub direction: Direction,
}

/// Path type for field selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathType {
    #[default]
    JsonPath,
    XPath,
    FormField,
}

/// Masking action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MaskingAction {
    /// Reversible tokenization (requires store).
    Tokenize {
        /// Token format (uuid, prefixed, etc.).
        #[serde(default)]
        format: TokenFormat,
    },

    /// Format-preserving encryption (reversible, no store needed).
    Fpe {
        /// Alphabet for FPE (digits, alphanumeric, etc.).
        alphabet: FpeAlphabet,
    },

    /// Pattern-based masking (irreversible).
    Mask {
        /// Masking character.
        #[serde(default = "default_mask_char")]
        char: char,
        /// Number of characters to preserve at start.
        #[serde(default)]
        preserve_start: usize,
        /// Number of characters to preserve at end.
        #[serde(default)]
        preserve_end: usize,
    },

    /// Complete redaction (irreversible).
    Redact {
        /// Replacement string.
        #[serde(default = "default_redact")]
        replacement: String,
    },

    /// Hash the value (irreversible but deterministic).
    Hash {
        /// Hash algorithm.
        #[serde(default)]
        algorithm: HashAlgorithm,
        /// Truncate hash to this many characters (0 = full hash).
        #[serde(default)]
        truncate: usize,
    },
}

fn default_mask_char() -> char {
    '*'
}
fn default_redact() -> String {
    "[REDACTED]".to_string()
}

/// Token format options.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TokenFormat {
    #[default]
    Uuid,
    Prefixed {
        prefix: String,
    },
}

/// FPE alphabet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FpeAlphabet {
    /// 0-9 only.
    Digits,
    /// 0-9, a-z, A-Z.
    Alphanumeric,
    /// 0-9, a-z only.
    AlphanumericLower,
    /// Credit card format (16 digits).
    CreditCard,
    /// SSN format (9 digits).
    Ssn,
}

impl FpeAlphabet {
    /// Get the character set for this alphabet.
    pub fn chars(&self) -> &'static str {
        match self {
            Self::Digits | Self::CreditCard | Self::Ssn => "0123456789",
            Self::Alphanumeric => "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
            Self::AlphanumericLower => "0123456789abcdefghijklmnopqrstuvwxyz",
        }
    }
}

/// Hash algorithm.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HashAlgorithm {
    #[default]
    Sha256,
}

/// Direction for rule application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Request,
    Response,
    #[default]
    Both,
}

impl Direction {
    /// Check if this direction applies to requests.
    pub fn applies_to_request(&self) -> bool {
        matches!(self, Self::Request | Self::Both)
    }

    /// Check if this direction applies to responses.
    pub fn applies_to_response(&self) -> bool {
        matches!(self, Self::Response | Self::Both)
    }
}

/// Header masking rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderRule {
    /// Header name (case-insensitive).
    pub name: String,
    /// Masking action.
    pub action: MaskingAction,
    /// Direction.
    #[serde(default)]
    pub direction: Direction,
}

/// Pattern configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PatternConfig {
    /// Enable built-in patterns.
    #[serde(default)]
    pub builtins: BuiltinPatterns,
    /// Custom patterns.
    #[serde(default)]
    pub custom: Vec<CustomPattern>,
}

/// Built-in pattern toggles.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuiltinPatterns {
    #[serde(default)]
    pub credit_card: bool,
    #[serde(default)]
    pub ssn: bool,
    #[serde(default)]
    pub email: bool,
    #[serde(default)]
    pub phone: bool,
}

/// Custom pattern definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPattern {
    /// Pattern name.
    pub name: String,
    /// Regex pattern.
    pub regex: String,
    /// Default action when pattern matches.
    pub action: MaskingAction,
}

/// FPE configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FpeConfig {
    /// Encryption key (hex encoded, 32 bytes for AES-256).
    /// Should be loaded from environment or secrets manager.
    #[serde(default)]
    pub key: Option<String>,
    /// Key environment variable name.
    #[serde(default = "default_key_env")]
    pub key_env: String,
}

impl Default for FpeConfig {
    fn default() -> Self {
        Self {
            key: None,
            key_env: default_key_env(),
        }
    }
}

fn default_key_env() -> String {
    "DATA_MASKING_FPE_KEY".to_string()
}

/// Buffering configuration for streaming bodies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferingConfig {
    /// Maximum body size to buffer (bytes).
    #[serde(default = "default_max_buffer")]
    pub max_buffer_bytes: usize,
}

impl Default for BufferingConfig {
    fn default() -> Self {
        Self {
            max_buffer_bytes: default_max_buffer(),
        }
    }
}

fn default_max_buffer() -> usize {
    10 * 1024 * 1024
} // 10MB

/// Validate configuration.
pub fn validate_config(config: &DataMaskingConfig) -> Result<(), String> {
    // Validate field rules
    for (i, rule) in config.fields.iter().enumerate() {
        if rule.path.is_empty() {
            return Err(format!("field rule {}: path cannot be empty", i));
        }
    }

    // Validate header rules
    for (i, rule) in config.headers.iter().enumerate() {
        if rule.name.is_empty() {
            return Err(format!("header rule {}: name cannot be empty", i));
        }
    }

    // Validate custom patterns
    for pattern in &config.patterns.custom {
        if pattern.name.is_empty() {
            return Err("custom pattern: name cannot be empty".to_string());
        }
        if regex::Regex::new(&pattern.regex).is_err() {
            return Err(format!(
                "custom pattern '{}': invalid regex '{}'",
                pattern.name, pattern.regex
            ));
        }
    }

    // Validate FPE key if provided
    if let Some(ref key) = config.fpe.key {
        if key.len() != 64 {
            return Err("FPE key must be 64 hex characters (32 bytes)".to_string());
        }
        if hex::decode(key).is_err() {
            return Err("FPE key must be valid hex".to_string());
        }
    }

    Ok(())
}

/// Hex decoding helper.
mod hex {
    pub fn decode(s: &str) -> Result<Vec<u8>, ()> {
        if s.len() % 2 != 0 {
            return Err(());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
            .collect()
    }
}
