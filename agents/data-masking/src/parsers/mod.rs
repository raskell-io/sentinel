//! Content type parsers for body processing.

mod form;
mod json;
mod xml;

pub use form::FormParser;
pub use json::JsonParser;
pub use xml::XmlParser;

use crate::errors::MaskingError;

/// Trait for parsing and modifying body content.
pub trait BodyParser: Send + Sync {
    /// Parse body bytes into a field accessor.
    fn parse(&self, body: &[u8]) -> Result<Box<dyn FieldAccessor>, MaskingError>;

    /// Serialize the accessor back to bytes.
    fn serialize(&self, accessor: &dyn FieldAccessor) -> Result<Vec<u8>, MaskingError>;
}

/// Trait for accessing and modifying fields in parsed content.
pub trait FieldAccessor: Send + Sync {
    /// Get value at the specified path.
    fn get(&self, path: &str) -> Option<String>;

    /// Set value at the specified path.
    fn set(&mut self, path: &str, value: String) -> Result<(), MaskingError>;

    /// Get all paths matching the pattern.
    fn find_paths(&self, pattern: &str) -> Vec<String>;

    /// Iterate all string values with their paths.
    fn all_values(&self) -> Vec<(String, String)>;

    /// Downcast to concrete type for serialization.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Downcast to mutable concrete type.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Get a parser for the specified content type.
pub fn get_parser(content_type: &str) -> Result<Box<dyn BodyParser>, MaskingError> {
    let ct_lower = content_type.to_lowercase();

    if ct_lower.contains("application/json") || ct_lower.contains("text/json") {
        Ok(Box::new(JsonParser))
    } else if ct_lower.contains("application/xml") || ct_lower.contains("text/xml") {
        Ok(Box::new(XmlParser))
    } else if ct_lower.contains("application/x-www-form-urlencoded") {
        Ok(Box::new(FormParser))
    } else {
        Err(MaskingError::UnsupportedContentType(content_type.to_string()))
    }
}
