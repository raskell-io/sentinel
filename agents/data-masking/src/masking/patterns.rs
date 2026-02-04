//! Built-in and custom pattern detection.

use crate::config::{BuiltinPatterns, MaskingAction, PatternConfig};
use crate::errors::MaskingError;
use regex::Regex;

/// Compiled pattern matchers.
pub struct CompiledPatterns {
    /// Credit card pattern.
    credit_card: Option<Regex>,
    /// SSN pattern.
    ssn: Option<Regex>,
    /// Email pattern.
    email: Option<Regex>,
    /// Phone pattern.
    phone: Option<Regex>,
    /// Custom patterns with their actions.
    custom: Vec<(Regex, MaskingAction)>,
}

impl CompiledPatterns {
    /// Create compiled patterns from configuration.
    pub fn from_config(config: &PatternConfig) -> Result<Self, MaskingError> {
        let credit_card = if config.builtins.credit_card {
            Some(
                Regex::new(r"\b(?:4[0-9]{12}(?:[0-9]{3})?|5[1-5][0-9]{14}|3[47][0-9]{13}|6(?:011|5[0-9]{2})[0-9]{12})\b")
                    .map_err(|e| MaskingError::InvalidRegex(e.to_string()))?,
            )
        } else {
            None
        };

        let ssn = if config.builtins.ssn {
            Some(
                Regex::new(r"\b\d{3}-\d{2}-\d{4}\b|\b\d{9}\b")
                    .map_err(|e| MaskingError::InvalidRegex(e.to_string()))?,
            )
        } else {
            None
        };

        let email = if config.builtins.email {
            Some(
                Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b")
                    .map_err(|e| MaskingError::InvalidRegex(e.to_string()))?,
            )
        } else {
            None
        };

        let phone = if config.builtins.phone {
            Some(
                Regex::new(r"(?:\+1[-. ]?)?(?:\([0-9]{3}\)|[0-9]{3})[-. ]?[0-9]{3}[-. ]?[0-9]{4}")
                    .map_err(|e| MaskingError::InvalidRegex(e.to_string()))?,
            )
        } else {
            None
        };

        let mut custom = Vec::new();
        for pattern in &config.custom {
            let re = Regex::new(&pattern.regex)
                .map_err(|e| MaskingError::InvalidRegex(format!("{}: {}", pattern.name, e)))?;
            custom.push((re, pattern.action.clone()));
        }

        Ok(Self {
            credit_card,
            ssn,
            email,
            phone,
            custom,
        })
    }

    /// Create with default built-in patterns enabled.
    pub fn default_builtins() -> Self {
        let config = PatternConfig {
            builtins: BuiltinPatterns {
                credit_card: true,
                ssn: true,
                email: true,
                phone: true,
            },
            custom: Vec::new(),
        };
        Self::from_config(&config).expect("default patterns should compile")
    }

    /// Check if a value looks like a credit card number.
    pub fn is_credit_card(&self, value: &str) -> bool {
        if let Some(ref re) = self.credit_card {
            if re.is_match(value) {
                // Additional Luhn check
                return luhn_check(value);
            }
        }
        false
    }

    /// Check if a value looks like an SSN.
    pub fn is_ssn(&self, value: &str) -> bool {
        self.ssn.as_ref().is_some_and(|re| re.is_match(value))
    }

    /// Check if a value looks like an email.
    pub fn is_email(&self, value: &str) -> bool {
        self.email.as_ref().is_some_and(|re| re.is_match(value))
    }

    /// Check if a value looks like a phone number.
    pub fn is_phone(&self, value: &str) -> bool {
        self.phone.as_ref().is_some_and(|re| re.is_match(value))
    }

    /// Detect if a value matches any pattern and return the action.
    pub fn detect(&self, value: &str) -> Option<&MaskingAction> {
        // Check custom patterns first (higher priority)
        for (re, action) in &self.custom {
            if re.is_match(value) {
                return Some(action);
            }
        }

        // Check built-in patterns
        if self.is_credit_card(value) {
            return Some(&DEFAULT_CREDIT_CARD_ACTION);
        }
        if self.is_ssn(value) {
            return Some(&DEFAULT_SSN_ACTION);
        }
        if self.is_email(value) {
            return Some(&DEFAULT_EMAIL_ACTION);
        }
        if self.is_phone(value) {
            return Some(&DEFAULT_PHONE_ACTION);
        }

        None
    }
}

/// Luhn algorithm check for credit card validation.
fn luhn_check(number: &str) -> bool {
    let digits: Vec<u32> = number
        .chars()
        .filter(|c| c.is_ascii_digit())
        .filter_map(|c| c.to_digit(10))
        .collect();

    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }

    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                d
            }
        })
        .sum();

    sum.is_multiple_of(10)
}

// Default actions for built-in patterns
static DEFAULT_CREDIT_CARD_ACTION: MaskingAction = MaskingAction::Mask {
    char: '*',
    preserve_start: 4,
    preserve_end: 4,
};

static DEFAULT_SSN_ACTION: MaskingAction = MaskingAction::Mask {
    char: '*',
    preserve_start: 0,
    preserve_end: 4,
};

static DEFAULT_EMAIL_ACTION: MaskingAction = MaskingAction::Mask {
    char: '*',
    preserve_start: 2,
    preserve_end: 0,
};

static DEFAULT_PHONE_ACTION: MaskingAction = MaskingAction::Mask {
    char: '*',
    preserve_start: 0,
    preserve_end: 4,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomPattern;

    #[test]
    fn test_luhn_valid() {
        // Valid test credit card numbers
        assert!(luhn_check("4111111111111111")); // Visa
        assert!(luhn_check("5500000000000004")); // Mastercard
        assert!(luhn_check("378282246310005")); // Amex
    }

    #[test]
    fn test_luhn_invalid() {
        assert!(!luhn_check("1234567890123456"));
        assert!(!luhn_check("4111111111111112")); // Invalid check digit
    }

    #[test]
    fn test_credit_card_detection() {
        let patterns = CompiledPatterns::default_builtins();
        assert!(patterns.is_credit_card("4111111111111111"));
        assert!(!patterns.is_credit_card("1234567890123456"));
    }

    #[test]
    fn test_ssn_detection() {
        let patterns = CompiledPatterns::default_builtins();
        assert!(patterns.is_ssn("123-45-6789"));
        assert!(patterns.is_ssn("123456789"));
        assert!(!patterns.is_ssn("12-345-6789"));
    }

    #[test]
    fn test_email_detection() {
        let patterns = CompiledPatterns::default_builtins();
        assert!(patterns.is_email("test@example.com"));
        assert!(patterns.is_email("user.name+tag@domain.co.uk"));
        assert!(!patterns.is_email("not-an-email"));
    }

    #[test]
    fn test_phone_detection() {
        let patterns = CompiledPatterns::default_builtins();
        assert!(patterns.is_phone("555-123-4567"));
        assert!(patterns.is_phone("(555) 123-4567"));
        assert!(patterns.is_phone("+1-555-123-4567"));
    }

    #[test]
    fn test_custom_pattern() {
        let config = PatternConfig {
            builtins: BuiltinPatterns::default(),
            custom: vec![CustomPattern {
                name: "api_key".to_string(),
                regex: r"sk_[a-zA-Z0-9]{24,}".to_string(),
                action: MaskingAction::Redact {
                    replacement: "[API_KEY]".to_string(),
                },
            }],
        };

        let patterns = CompiledPatterns::from_config(&config).unwrap();
        let action = patterns.detect("sk_abcdefghijklmnopqrstuvwxyz");
        assert!(action.is_some());
    }
}
