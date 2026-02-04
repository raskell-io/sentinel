//! JSON parser with simple path-based field access.

use crate::errors::MaskingError;
use crate::parsers::{BodyParser, FieldAccessor};
use serde_json::Value;
use std::any::Any;

/// JSON body parser.
pub struct JsonParser;

impl BodyParser for JsonParser {
    fn parse(&self, body: &[u8]) -> Result<Box<dyn FieldAccessor>, MaskingError> {
        let value: Value =
            serde_json::from_slice(body).map_err(|e| MaskingError::InvalidJson(e.to_string()))?;
        Ok(Box::new(JsonAccessor { value }))
    }

    fn serialize(&self, accessor: &dyn FieldAccessor) -> Result<Vec<u8>, MaskingError> {
        let json_accessor = accessor
            .as_any()
            .downcast_ref::<JsonAccessor>()
            .ok_or_else(|| MaskingError::Serialization("type mismatch".to_string()))?;
        serde_json::to_vec(&json_accessor.value)
            .map_err(|e| MaskingError::Serialization(e.to_string()))
    }
}

/// JSON field accessor using simple path navigation.
pub struct JsonAccessor {
    value: Value,
}

impl FieldAccessor for JsonAccessor {
    fn get(&self, path: &str) -> Option<String> {
        let segments = parse_path_segments(path).ok()?;
        let mut current = &self.value;

        for segment in &segments {
            current = match segment {
                PathSegment::Key(key) => current.get(key)?,
                PathSegment::Index(idx) => current.get(*idx)?,
            };
        }

        match current {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Null => Some("null".to_string()),
            _ => None,
        }
    }

    fn set(&mut self, path: &str, value: String) -> Result<(), MaskingError> {
        set_json_value(&mut self.value, path, Value::String(value))
    }

    fn find_paths(&self, pattern: &str) -> Vec<String> {
        // For simple field names, search recursively
        let mut results = Vec::new();
        find_paths_recursive(&self.value, pattern, "$", &mut results);
        results
    }

    fn all_values(&self) -> Vec<(String, String)> {
        let mut results = Vec::new();
        collect_all_strings(&self.value, "$", &mut results);
        results
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Set a value at the specified path.
fn set_json_value(root: &mut Value, path: &str, new_value: Value) -> Result<(), MaskingError> {
    let segments = parse_path_segments(path)?;

    if segments.is_empty() {
        return Err(MaskingError::FieldAccess("empty path".to_string()));
    }

    let mut current = root;

    // Navigate to parent
    for segment in segments.iter().take(segments.len() - 1) {
        current = match segment {
            PathSegment::Key(key) => current
                .get_mut(key)
                .ok_or_else(|| MaskingError::FieldAccess(format!("key not found: {}", key)))?,
            PathSegment::Index(idx) => current
                .get_mut(*idx)
                .ok_or_else(|| MaskingError::FieldAccess(format!("index not found: {}", idx)))?,
        };
    }

    // Set the value
    match segments.last().unwrap() {
        PathSegment::Key(key) => {
            if let Value::Object(map) = current {
                map.insert(key.clone(), new_value);
                Ok(())
            } else {
                Err(MaskingError::FieldAccess(
                    "parent is not an object".to_string(),
                ))
            }
        }
        PathSegment::Index(idx) => {
            if let Value::Array(arr) = current {
                if *idx < arr.len() {
                    arr[*idx] = new_value;
                    Ok(())
                } else {
                    Err(MaskingError::FieldAccess(format!(
                        "index out of bounds: {}",
                        idx
                    )))
                }
            } else {
                Err(MaskingError::FieldAccess(
                    "parent is not an array".to_string(),
                ))
            }
        }
    }
}

#[derive(Debug)]
enum PathSegment {
    Key(String),
    Index(usize),
}

/// Parse path into segments.
/// Supports: $.user.name, user.name, user[0].name
fn parse_path_segments(path: &str) -> Result<Vec<PathSegment>, MaskingError> {
    let mut segments = Vec::new();
    let path = path.strip_prefix('$').unwrap_or(path);
    let path = path.strip_prefix('.').unwrap_or(path);

    if path.is_empty() {
        return Ok(segments);
    }

    for part in path.split('.').filter(|s| !s.is_empty()) {
        // Handle array notation: field[0]
        if let Some(bracket_pos) = part.find('[') {
            let key = &part[..bracket_pos];
            if !key.is_empty() {
                segments.push(PathSegment::Key(key.to_string()));
            }

            // Extract index
            let idx_str = part[bracket_pos + 1..]
                .strip_suffix(']')
                .ok_or_else(|| MaskingError::FieldAccess("invalid array syntax".to_string()))?;
            let idx: usize = idx_str
                .parse()
                .map_err(|_| MaskingError::FieldAccess("invalid array index".to_string()))?;
            segments.push(PathSegment::Index(idx));
        } else {
            segments.push(PathSegment::Key(part.to_string()));
        }
    }

    Ok(segments)
}

/// Find paths matching a pattern (field name).
fn find_paths_recursive(
    value: &Value,
    pattern: &str,
    current_path: &str,
    results: &mut Vec<String>,
) {
    // If pattern starts with $, try exact match
    if pattern.starts_with('$') {
        if let Ok(segments) = parse_path_segments(pattern) {
            let mut current = value;
            let mut valid = true;

            for segment in &segments {
                match segment {
                    PathSegment::Key(key) => {
                        if let Some(next) = current.get(key) {
                            current = next;
                        } else {
                            valid = false;
                            break;
                        }
                    }
                    PathSegment::Index(idx) => {
                        if let Some(next) = current.get(*idx) {
                            current = next;
                        } else {
                            valid = false;
                            break;
                        }
                    }
                }
            }

            if valid {
                results.push(pattern.to_string());
            }
        }
        return;
    }

    // Otherwise, search recursively for matching field names
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                let new_path = format!("{}.{}", current_path, key);

                // Check if this key matches the pattern
                if key == pattern {
                    results.push(new_path.clone());
                }

                // Recurse into nested objects/arrays
                find_paths_recursive(val, pattern, &new_path, results);
            }
        }
        Value::Array(arr) => {
            for (idx, val) in arr.iter().enumerate() {
                let new_path = format!("{}[{}]", current_path, idx);
                find_paths_recursive(val, pattern, &new_path, results);
            }
        }
        _ => {}
    }
}

/// Collect all string values with their paths.
fn collect_all_strings(value: &Value, path: &str, results: &mut Vec<(String, String)>) {
    match value {
        Value::String(s) => {
            results.push((path.to_string(), s.clone()));
        }
        Value::Number(n) => {
            results.push((path.to_string(), n.to_string()));
        }
        Value::Object(map) => {
            for (key, val) in map {
                let new_path = format!("{}.{}", path, key);
                collect_all_strings(val, &new_path, results);
            }
        }
        Value::Array(arr) => {
            for (idx, val) in arr.iter().enumerate() {
                let new_path = format!("{}[{}]", path, idx);
                collect_all_strings(val, &new_path, results);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_parse_and_get() {
        let parser = JsonParser;
        let json = r#"{"user": {"name": "John", "ssn": "123-45-6789"}}"#;

        let accessor = parser.parse(json.as_bytes()).unwrap();
        assert_eq!(accessor.get("$.user.name"), Some("John".to_string()));
        assert_eq!(accessor.get("$.user.ssn"), Some("123-45-6789".to_string()));
    }

    #[test]
    fn test_json_set() {
        let parser = JsonParser;
        let json = r#"{"user": {"ssn": "123-45-6789"}}"#;

        let mut accessor = parser.parse(json.as_bytes()).unwrap();
        accessor.set("$.user.ssn", "MASKED".to_string()).unwrap();

        assert_eq!(accessor.get("$.user.ssn"), Some("MASKED".to_string()));
    }

    #[test]
    fn test_json_serialize() {
        let parser = JsonParser;
        let json = r#"{"name":"test"}"#;

        let accessor = parser.parse(json.as_bytes()).unwrap();
        let serialized = parser.serialize(accessor.as_ref()).unwrap();
        let result: Value = serde_json::from_slice(&serialized).unwrap();

        assert_eq!(result["name"], "test");
    }

    #[test]
    fn test_find_paths() {
        let parser = JsonParser;
        let json = r#"{"user": {"ssn": "123"}, "admin": {"ssn": "456"}}"#;

        let accessor = parser.parse(json.as_bytes()).unwrap();
        let paths = accessor.find_paths("ssn");

        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"$.user.ssn".to_string()));
        assert!(paths.contains(&"$.admin.ssn".to_string()));
    }
}
