//! Content compression for static file serving
//!
//! This module provides on-the-fly compression for static files,
//! supporting both gzip and brotli encoding.

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use http::{header, Request};
use std::io::Write;
use tracing::trace;

// ============================================================================
// Content Encoding
// ============================================================================

/// Content encoding preference
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContentEncoding {
    Identity,
    Gzip,
    Brotli,
}

impl ContentEncoding {
    /// Get the HTTP header value for this encoding
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentEncoding::Identity => "identity",
            ContentEncoding::Gzip => "gzip",
            ContentEncoding::Brotli => "br",
        }
    }
}

// ============================================================================
// Compression Functions
// ============================================================================

/// Check if content type should be compressed
pub fn should_compress(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type.contains("javascript")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("svg")
        || content_type == "application/wasm"
}

/// Negotiate content encoding based on Accept-Encoding header
pub fn negotiate_encoding<B>(req: &Request<B>) -> ContentEncoding {
    if let Some(accept_encoding) = req.headers().get(header::ACCEPT_ENCODING) {
        if let Ok(accept_str) = accept_encoding.to_str() {
            // Check for brotli first (better compression)
            if accept_str.contains("br") {
                trace!(
                    accept_encoding = %accept_str,
                    selected = "brotli",
                    "Negotiated content encoding"
                );
                return ContentEncoding::Brotli;
            }
            // Fall back to gzip
            if accept_str.contains("gzip") {
                trace!(
                    accept_encoding = %accept_str,
                    selected = "gzip",
                    "Negotiated content encoding"
                );
                return ContentEncoding::Gzip;
            }
        }
    }
    trace!(selected = "identity", "No compression encoding accepted");
    ContentEncoding::Identity
}

/// Compress content using the specified encoding
pub fn compress_content(content: &Bytes, encoding: ContentEncoding) -> Result<Bytes> {
    let original_size = content.len();

    match encoding {
        ContentEncoding::Gzip => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(content)?;
            let compressed = encoder.finish()?;
            let compressed_size = compressed.len();

            trace!(
                encoding = "gzip",
                original_size = original_size,
                compressed_size = compressed_size,
                ratio = format!("{:.1}%", (compressed_size as f64 / original_size as f64) * 100.0),
                "Compressed content"
            );

            Ok(Bytes::from(compressed))
        }
        ContentEncoding::Brotli => {
            let mut compressed = Vec::new();
            {
                let mut encoder = brotli::CompressorWriter::new(&mut compressed, 4096, 4, 22);
                encoder.write_all(content)?;
            }
            let compressed_size = compressed.len();

            trace!(
                encoding = "brotli",
                original_size = original_size,
                compressed_size = compressed_size,
                ratio = format!("{:.1}%", (compressed_size as f64 / original_size as f64) * 100.0),
                "Compressed content"
            );

            Ok(Bytes::from(compressed))
        }
        ContentEncoding::Identity => {
            trace!(
                encoding = "identity",
                size = original_size,
                "No compression applied"
            );
            Ok(content.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_compress() {
        assert!(should_compress("text/html"));
        assert!(should_compress("text/css"));
        assert!(should_compress("application/javascript"));
        assert!(should_compress("application/json"));
        assert!(should_compress("image/svg+xml"));
        assert!(should_compress("application/wasm"));

        assert!(!should_compress("image/png"));
        assert!(!should_compress("image/jpeg"));
        assert!(!should_compress("application/octet-stream"));
    }

    #[test]
    fn test_content_encoding_as_str() {
        assert_eq!(ContentEncoding::Identity.as_str(), "identity");
        assert_eq!(ContentEncoding::Gzip.as_str(), "gzip");
        assert_eq!(ContentEncoding::Brotli.as_str(), "br");
    }

    #[test]
    fn test_compress_content_gzip() {
        let content = Bytes::from("Hello, World!");
        let compressed = compress_content(&content, ContentEncoding::Gzip).unwrap();

        // Compressed content should be different (though might be larger for small inputs)
        assert!(!compressed.is_empty());
    }

    #[test]
    fn test_compress_content_identity() {
        let content = Bytes::from("Hello, World!");
        let result = compress_content(&content, ContentEncoding::Identity).unwrap();
        assert_eq!(result, content);
    }
}
