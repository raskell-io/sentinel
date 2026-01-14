//! Zero-copy header types for efficient header processing.
//!
//! This module provides header types that avoid allocation in the hot path
//! by using borrowed references and `Cow` for deferred cloning.
//!
//! # Performance
//!
//! - Header iteration: O(n) with zero allocations
//! - Header lookup: O(1) average (borrowed from source HashMap)
//! - Conversion to owned: Only allocates when actually needed
//! - SmallVec for values: Inline storage for single-value headers (most common)

use std::borrow::Cow;
use std::collections::HashMap;

use smallvec::SmallVec;

/// Header values using SmallVec for inline storage.
///
/// Most HTTP headers have a single value. Using SmallVec<[String; 1]>
/// avoids heap allocation for the Vec in the common case.
pub type HeaderValues = SmallVec<[String; 1]>;

/// Optimized header map using SmallVec for values.
///
/// This reduces allocations for typical requests where most headers
/// have only one value.
pub type OptimizedHeaderMap = HashMap<String, HeaderValues>;

/// Zero-copy header reference.
///
/// Wraps a reference to a header map without cloning.
#[derive(Debug)]
pub struct HeadersRef<'a> {
    inner: &'a HashMap<String, Vec<String>>,
}

impl<'a> HeadersRef<'a> {
    /// Create a new header reference.
    #[inline]
    pub fn new(headers: &'a HashMap<String, Vec<String>>) -> Self {
        Self { inner: headers }
    }

    /// Get a header value by name.
    #[inline]
    pub fn get(&self, name: &str) -> Option<&Vec<String>> {
        self.inner.get(name)
    }

    /// Get the first value for a header.
    #[inline]
    pub fn get_first(&self, name: &str) -> Option<&str> {
        self.inner.get(name).and_then(|v| v.first()).map(|s| s.as_str())
    }

    /// Check if a header exists.
    #[inline]
    pub fn contains(&self, name: &str) -> bool {
        self.inner.contains_key(name)
    }

    /// Get the number of unique header names.
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if headers are empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterate over header names and values (no allocation).
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Vec<String>)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Iterate over flattened header name-value pairs.
    #[inline]
    pub fn iter_flat(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner
            .iter()
            .flat_map(|(k, values)| values.iter().map(move |v| (k.as_str(), v.as_str())))
    }

    /// Convert to owned HashMap (clones).
    #[inline]
    pub fn to_owned(&self) -> HashMap<String, Vec<String>> {
        self.inner.clone()
    }

    /// Get the underlying reference.
    #[inline]
    pub fn as_inner(&self) -> &HashMap<String, Vec<String>> {
        self.inner
    }
}

/// Copy-on-write headers for deferred cloning.
///
/// Allows working with headers without cloning until mutation is needed.
#[derive(Debug, Clone)]
pub struct HeadersCow<'a> {
    inner: Cow<'a, HashMap<String, Vec<String>>>,
}

impl<'a> HeadersCow<'a> {
    /// Create from a borrowed reference.
    #[inline]
    pub fn borrowed(headers: &'a HashMap<String, Vec<String>>) -> Self {
        Self {
            inner: Cow::Borrowed(headers),
        }
    }

    /// Create from an owned HashMap.
    #[inline]
    pub fn owned(headers: HashMap<String, Vec<String>>) -> Self {
        Self {
            inner: Cow::Owned(headers),
        }
    }

    /// Get a header value by name.
    #[inline]
    pub fn get(&self, name: &str) -> Option<&Vec<String>> {
        self.inner.get(name)
    }

    /// Get the first value for a header.
    #[inline]
    pub fn get_first(&self, name: &str) -> Option<&str> {
        self.inner.get(name).and_then(|v| v.first()).map(|s| s.as_str())
    }

    /// Check if a header exists.
    #[inline]
    pub fn contains(&self, name: &str) -> bool {
        self.inner.contains_key(name)
    }

    /// Set a header value (triggers clone if borrowed).
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.inner.to_mut().insert(name.into(), vec![value.into()]);
    }

    /// Add a header value (triggers clone if borrowed).
    pub fn add(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.inner
            .to_mut()
            .entry(name.into())
            .or_default()
            .push(value.into());
    }

    /// Remove a header (triggers clone if borrowed).
    pub fn remove(&mut self, name: &str) -> Option<Vec<String>> {
        self.inner.to_mut().remove(name)
    }

    /// Check if the headers have been cloned.
    #[inline]
    pub fn is_owned(&self) -> bool {
        matches!(self.inner, Cow::Owned(_))
    }

    /// Convert to owned HashMap.
    #[inline]
    pub fn into_owned(self) -> HashMap<String, Vec<String>> {
        self.inner.into_owned()
    }

    /// Get the number of unique header names.
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if headers are empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterate over header names and values.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Vec<String>)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl Default for HeadersCow<'_> {
    fn default() -> Self {
        Self::owned(HashMap::new())
    }
}

impl<'a> From<&'a HashMap<String, Vec<String>>> for HeadersCow<'a> {
    fn from(headers: &'a HashMap<String, Vec<String>>) -> Self {
        Self::borrowed(headers)
    }
}

impl From<HashMap<String, Vec<String>>> for HeadersCow<'_> {
    fn from(headers: HashMap<String, Vec<String>>) -> Self {
        Self::owned(headers)
    }
}

/// Header name/value iterator that yields references.
pub struct HeaderIterator<'a> {
    inner: std::collections::hash_map::Iter<'a, String, Vec<String>>,
    current_name: Option<&'a str>,
    current_values: Option<std::slice::Iter<'a, String>>,
}

impl<'a> HeaderIterator<'a> {
    /// Create a new header iterator.
    pub fn new(headers: &'a HashMap<String, Vec<String>>) -> Self {
        Self {
            inner: headers.iter(),
            current_name: None,
            current_values: None,
        }
    }
}

impl<'a> Iterator for HeaderIterator<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Try to get next value from current header
            if let (Some(name), Some(values)) = (self.current_name, self.current_values.as_mut()) {
                if let Some(value) = values.next() {
                    return Some((name, value.as_str()));
                }
            }

            // Move to next header
            let (name, values) = self.inner.next()?;
            self.current_name = Some(name.as_str());
            self.current_values = Some(values.iter());
        }
    }
}

/// Common HTTP header names as constants (avoids string allocation).
pub mod names {
    pub const HOST: &str = "host";
    pub const CONTENT_TYPE: &str = "content-type";
    pub const CONTENT_LENGTH: &str = "content-length";
    pub const USER_AGENT: &str = "user-agent";
    pub const ACCEPT: &str = "accept";
    pub const ACCEPT_ENCODING: &str = "accept-encoding";
    pub const AUTHORIZATION: &str = "authorization";
    pub const COOKIE: &str = "cookie";
    pub const SET_COOKIE: &str = "set-cookie";
    pub const CACHE_CONTROL: &str = "cache-control";
    pub const X_FORWARDED_FOR: &str = "x-forwarded-for";
    pub const X_REAL_IP: &str = "x-real-ip";
    pub const X_REQUEST_ID: &str = "x-request-id";
    pub const X_CORRELATION_ID: &str = "x-correlation-id";
}

/// Convert standard headers to optimized format.
///
/// This is useful when receiving headers from external sources (JSON, gRPC)
/// and converting them for internal processing.
#[inline]
pub fn to_optimized(headers: HashMap<String, Vec<String>>) -> OptimizedHeaderMap {
    headers
        .into_iter()
        .map(|(name, values)| (name, HeaderValues::from_vec(values)))
        .collect()
}

/// Convert optimized headers back to standard format.
///
/// This is useful when serializing headers for external transmission.
#[inline]
pub fn from_optimized(headers: OptimizedHeaderMap) -> HashMap<String, Vec<String>> {
    headers
        .into_iter()
        .map(|(name, values)| (name, values.into_vec()))
        .collect()
}

/// Iterate over headers yielding (name, value) pairs without allocation.
///
/// This is the most efficient way to convert headers to gRPC format.
#[inline]
pub fn iter_flat(headers: &HashMap<String, Vec<String>>) -> impl Iterator<Item = (&str, &str)> {
    headers
        .iter()
        .flat_map(|(name, values)| values.iter().map(move |v| (name.as_str(), v.as_str())))
}

/// Iterate over optimized headers yielding (name, value) pairs.
#[inline]
pub fn iter_flat_optimized(headers: &OptimizedHeaderMap) -> impl Iterator<Item = (&str, &str)> {
    headers
        .iter()
        .flat_map(|(name, values)| values.iter().map(move |v| (name.as_str(), v.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_headers() -> HashMap<String, Vec<String>> {
        let mut h = HashMap::new();
        h.insert("content-type".to_string(), vec!["application/json".to_string()]);
        h.insert("accept".to_string(), vec!["text/html".to_string(), "application/json".to_string()]);
        h.insert("x-custom".to_string(), vec!["value".to_string()]);
        h
    }

    #[test]
    fn test_headers_ref() {
        let headers = sample_headers();
        let ref_ = HeadersRef::new(&headers);

        assert_eq!(ref_.get_first("content-type"), Some("application/json"));
        assert_eq!(ref_.get("accept").map(|v| v.len()), Some(2));
        assert!(ref_.contains("x-custom"));
        assert!(!ref_.contains("not-present"));
        assert_eq!(ref_.len(), 3);
    }

    #[test]
    fn test_headers_ref_iter() {
        let headers = sample_headers();
        let ref_ = HeadersRef::new(&headers);

        let flat: Vec<_> = ref_.iter_flat().collect();
        assert!(flat.contains(&("content-type", "application/json")));
        assert!(flat.contains(&("accept", "text/html")));
        assert!(flat.contains(&("accept", "application/json")));
    }

    #[test]
    fn test_headers_cow_borrowed() {
        let headers = sample_headers();
        let cow = HeadersCow::borrowed(&headers);

        assert!(!cow.is_owned());
        assert_eq!(cow.get_first("content-type"), Some("application/json"));
    }

    #[test]
    fn test_headers_cow_mutation() {
        let headers = sample_headers();
        let mut cow = HeadersCow::borrowed(&headers);

        assert!(!cow.is_owned());

        // Mutation triggers clone
        cow.set("x-new", "new-value");
        assert!(cow.is_owned());

        assert_eq!(cow.get_first("x-new"), Some("new-value"));
        // Original headers unchanged
        assert!(!headers.contains_key("x-new"));
    }

    #[test]
    fn test_headers_cow_add() {
        let headers = sample_headers();
        let mut cow = HeadersCow::borrowed(&headers);

        cow.add("accept", "text/plain");
        assert!(cow.is_owned());

        let accept = cow.get("accept").unwrap();
        assert_eq!(accept.len(), 3);
    }

    #[test]
    fn test_header_iterator() {
        let headers = sample_headers();
        let iter = HeaderIterator::new(&headers);

        let pairs: Vec<_> = iter.collect();
        assert!(pairs.contains(&("content-type", "application/json")));
        assert!(pairs.contains(&("accept", "text/html")));
        assert!(pairs.contains(&("accept", "application/json")));
        assert!(pairs.contains(&("x-custom", "value")));
    }

    #[test]
    fn test_header_names() {
        use names::*;

        // Just verify the constants exist and are lowercase
        assert_eq!(CONTENT_TYPE, "content-type");
        assert_eq!(AUTHORIZATION, "authorization");
        assert_eq!(X_FORWARDED_FOR, "x-forwarded-for");
    }

    #[test]
    fn test_optimized_header_map() {
        let mut optimized: OptimizedHeaderMap = HashMap::new();

        // Single value - stored inline (no Vec allocation)
        optimized.insert("content-type".to_string(), HeaderValues::from_iter(["application/json".to_string()]));

        // Multiple values
        optimized.insert("accept".to_string(), HeaderValues::from_iter([
            "text/html".to_string(),
            "application/json".to_string(),
        ]));

        assert_eq!(optimized.get("content-type").map(|v| v.len()), Some(1));
        assert_eq!(optimized.get("accept").map(|v| v.len()), Some(2));
    }

    #[test]
    fn test_to_from_optimized() {
        let headers = sample_headers();

        // Convert to optimized
        let optimized = to_optimized(headers.clone());
        assert_eq!(optimized.len(), headers.len());

        // Convert back
        let back = from_optimized(optimized);
        assert_eq!(back, headers);
    }

    #[test]
    fn test_iter_flat_helper() {
        let headers = sample_headers();
        let pairs: Vec<_> = iter_flat(&headers).collect();

        // Should have 4 pairs (1 content-type + 2 accept + 1 x-custom)
        assert_eq!(pairs.len(), 4);
        assert!(pairs.contains(&("content-type", "application/json")));
        assert!(pairs.contains(&("accept", "text/html")));
        assert!(pairs.contains(&("accept", "application/json")));
        assert!(pairs.contains(&("x-custom", "value")));
    }

    #[test]
    fn test_iter_flat_optimized_helper() {
        let headers = sample_headers();
        let optimized = to_optimized(headers);
        let pairs: Vec<_> = iter_flat_optimized(&optimized).collect();

        assert_eq!(pairs.len(), 4);
        assert!(pairs.contains(&("content-type", "application/json")));
    }

    #[test]
    fn test_smallvec_single_value_inline() {
        // Verify SmallVec stores single value inline
        let values: HeaderValues = HeaderValues::from_iter(["single".to_string()]);

        // SmallVec<[String; 1]> should not spill to heap for single value
        assert!(!values.spilled());
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "single");
    }

    #[test]
    fn test_smallvec_multiple_values_spill() {
        // Verify SmallVec spills to heap for multiple values
        let values: HeaderValues = HeaderValues::from_iter([
            "first".to_string(),
            "second".to_string(),
        ]);

        // SmallVec<[String; 1]> should spill for 2+ values
        assert!(values.spilled());
        assert_eq!(values.len(), 2);
    }
}
