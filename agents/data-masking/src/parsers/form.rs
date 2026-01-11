//! Form URL-encoded parser.

use crate::errors::MaskingError;
use crate::parsers::{BodyParser, FieldAccessor};
use std::any::Any;
use std::collections::HashMap;

/// Form data parser.
pub struct FormParser;

impl BodyParser for FormParser {
    fn parse(&self, body: &[u8]) -> Result<Box<dyn FieldAccessor>, MaskingError> {
        let body_str = std::str::from_utf8(body)
            .map_err(|e| MaskingError::InvalidUtf8(e.to_string()))?;

        let fields: HashMap<String, String> = serde_urlencoded::from_str(body_str)
            .map_err(|e| MaskingError::InvalidForm(e.to_string()))?;

        Ok(Box::new(FormAccessor { fields }))
    }

    fn serialize(&self, accessor: &dyn FieldAccessor) -> Result<Vec<u8>, MaskingError> {
        let form_accessor = accessor
            .as_any()
            .downcast_ref::<FormAccessor>()
            .ok_or_else(|| MaskingError::Serialization("type mismatch".to_string()))?;

        serde_urlencoded::to_string(&form_accessor.fields)
            .map(|s| s.into_bytes())
            .map_err(|e| MaskingError::Serialization(e.to_string()))
    }
}

/// Form data accessor.
pub struct FormAccessor {
    fields: HashMap<String, String>,
}

impl FieldAccessor for FormAccessor {
    fn get(&self, path: &str) -> Option<String> {
        self.fields.get(path).cloned()
    }

    fn set(&mut self, path: &str, value: String) -> Result<(), MaskingError> {
        self.fields.insert(path.to_string(), value);
        Ok(())
    }

    fn find_paths(&self, pattern: &str) -> Vec<String> {
        // Try to compile as regex, fall back to exact match
        match regex::Regex::new(pattern) {
            Ok(re) => self
                .fields
                .keys()
                .filter(|k| re.is_match(k))
                .cloned()
                .collect(),
            Err(_) => {
                // Exact match
                if self.fields.contains_key(pattern) {
                    vec![pattern.to_string()]
                } else {
                    vec![]
                }
            }
        }
    }

    fn all_values(&self) -> Vec<(String, String)> {
        self.fields
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_form_parse_and_get() {
        let parser = FormParser;
        let form = b"name=John&ssn=123-45-6789";

        let accessor = parser.parse(form).unwrap();
        assert_eq!(accessor.get("name"), Some("John".to_string()));
        assert_eq!(accessor.get("ssn"), Some("123-45-6789".to_string()));
    }

    #[test]
    fn test_form_set() {
        let parser = FormParser;
        let form = b"ssn=123-45-6789";

        let mut accessor = parser.parse(form).unwrap();
        accessor.set("ssn", "MASKED".to_string()).unwrap();

        assert_eq!(accessor.get("ssn"), Some("MASKED".to_string()));
    }

    #[test]
    fn test_form_serialize() {
        let parser = FormParser;
        let form = b"name=test";

        let accessor = parser.parse(form).unwrap();
        let serialized = parser.serialize(accessor.as_ref()).unwrap();
        let result = String::from_utf8(serialized).unwrap();

        assert!(result.contains("name=test"));
    }

    #[test]
    fn test_form_url_encoded() {
        let parser = FormParser;
        let form = b"email=test%40example.com";

        let accessor = parser.parse(form).unwrap();
        assert_eq!(accessor.get("email"), Some("test@example.com".to_string()));
    }
}
