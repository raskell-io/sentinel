//! HTTP Range request handling for static file serving
//!
//! This module provides support for HTTP Range requests (RFC 7233),
//! enabling resumable downloads and video seeking.

use anyhow::Result;
use bytes::Bytes;
use http::{header, Method, Request, Response, StatusCode};
use http_body_util::Full;
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::{debug, warn};

// ============================================================================
// Range Types
// ============================================================================

/// Parsed Range header
#[derive(Debug, Clone)]
pub struct RangeSpec {
    /// Start byte (inclusive)
    pub start: u64,
    /// End byte (inclusive)
    pub end: u64,
}

impl RangeSpec {
    /// Create a new range specification
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// Get the content length for this range
    pub fn content_length(&self) -> u64 {
        self.end - self.start + 1
    }

    /// Check if the range is valid for a given file size
    pub fn is_valid(&self, file_size: u64) -> bool {
        self.start <= self.end && self.end < file_size
    }
}

// ============================================================================
// Range Parsing
// ============================================================================

/// Parse Range header into range specifications
///
/// Supports formats:
/// - `bytes=0-499` - First 500 bytes
/// - `bytes=-500` - Last 500 bytes
/// - `bytes=500-` - From byte 500 to end
/// - `bytes=0-499,1000-1499` - Multiple ranges (returns first only currently)
pub fn parse_range_header(range_str: &str, file_size: u64) -> Result<Vec<RangeSpec>> {
    if !range_str.starts_with("bytes=") {
        return Ok(vec![]);
    }

    let ranges_str = &range_str[6..];
    let mut ranges = Vec::new();

    for range_part in ranges_str.split(',') {
        let range_part = range_part.trim();

        if range_part.starts_with('-') {
            // Suffix range: -500 means last 500 bytes
            let suffix: u64 = range_part[1..].parse()?;
            if suffix > file_size {
                ranges.push(RangeSpec::new(0, file_size - 1));
            } else {
                ranges.push(RangeSpec::new(file_size - suffix, file_size - 1));
            }
        } else if range_part.ends_with('-') {
            // Open-ended range: 500- means from byte 500 to end
            let start: u64 = range_part[..range_part.len() - 1].parse()?;
            if start < file_size {
                ranges.push(RangeSpec::new(start, file_size - 1));
            }
        } else if let Some(dash_pos) = range_part.find('-') {
            // Full range: 0-499 means bytes 0 to 499
            let start: u64 = range_part[..dash_pos].parse()?;
            let end: u64 = range_part[dash_pos + 1..].parse()?;
            if start <= end && start < file_size {
                ranges.push(RangeSpec::new(start, end.min(file_size - 1)));
            }
        }
    }

    Ok(ranges)
}

// ============================================================================
// Range Response Building
// ============================================================================

/// Serve a range request (206 Partial Content)
pub async fn serve_range_request<B>(
    req: &Request<B>,
    file_path: &Path,
    file_size: u64,
    content_type: &str,
    etag: &str,
    modified: std::time::SystemTime,
    range_header: &http::HeaderValue,
    cache_control: &str,
) -> Result<Response<Full<Bytes>>> {
    // Check If-Range header
    if let Some(if_range) = req.headers().get(header::IF_RANGE) {
        if let Ok(if_range_str) = if_range.to_str() {
            // ETag comparison
            if if_range_str.starts_with('"') || if_range_str.starts_with("W/") {
                if if_range_str.trim_matches('"') != etag.trim_matches('"') {
                    return serve_full_file(file_path, content_type, file_size, etag, modified, cache_control).await;
                }
            // Date comparison
            } else if let Ok(if_range_time) = httpdate::parse_http_date(if_range_str) {
                if modified > if_range_time {
                    return serve_full_file(file_path, content_type, file_size, etag, modified, cache_control).await;
                }
            }
        }
    }

    // Parse Range header
    let range_str = range_header
        .to_str()
        .map_err(|_| anyhow::anyhow!("Invalid Range header"))?;
    let ranges = parse_range_header(range_str, file_size)?;

    if ranges.is_empty() {
        return Ok(Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{}", file_size))
            .body(Full::new(Bytes::new()))?);
    }

    if ranges.len() > 1 {
        warn!("Multi-range requests not yet supported, serving first range only");
    }

    let range = &ranges[0];

    if !range.is_valid(file_size) {
        return Ok(Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{}", file_size))
            .body(Full::new(Bytes::new()))?);
    }

    let content_length = range.content_length();
    let content = if req.method() == Method::HEAD {
        Bytes::new()
    } else {
        let mut file = fs::File::open(file_path).await?;
        file.seek(std::io::SeekFrom::Start(range.start)).await?;

        let mut buffer = vec![0u8; content_length as usize];
        file.read_exact(&mut buffer).await?;
        Bytes::from(buffer)
    };

    debug!(
        path = ?file_path,
        range_start = range.start,
        range_end = range.end,
        total_size = file_size,
        "Serving range request"
    );

    Ok(Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, content_length)
        .header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", range.start, range.end, file_size),
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified))
        .body(Full::new(content))?)
}

/// Serve a full file (for failed If-Range conditions)
pub async fn serve_full_file(
    file_path: &Path,
    content_type: &str,
    file_size: u64,
    etag: &str,
    modified: std::time::SystemTime,
    cache_control: &str,
) -> Result<Response<Full<Bytes>>> {
    let content = fs::read(file_path).await?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, file_size)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified))
        .header(header::CACHE_CONTROL, cache_control)
        .body(Full::new(Bytes::from(content)))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_full() {
        let ranges = parse_range_header("bytes=0-499", 1000).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges[0].end, 499);
    }

    #[test]
    fn test_parse_range_suffix() {
        let ranges = parse_range_header("bytes=-500", 1000).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 500);
        assert_eq!(ranges[0].end, 999);
    }

    #[test]
    fn test_parse_range_open_ended() {
        let ranges = parse_range_header("bytes=500-", 1000).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 500);
        assert_eq!(ranges[0].end, 999);
    }

    #[test]
    fn test_parse_range_clamp_to_file_size() {
        let ranges = parse_range_header("bytes=0-2000", 1000).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges[0].end, 999);
    }

    #[test]
    fn test_range_spec_content_length() {
        let range = RangeSpec::new(0, 499);
        assert_eq!(range.content_length(), 500);
    }

    #[test]
    fn test_range_spec_is_valid() {
        let range = RangeSpec::new(0, 499);
        assert!(range.is_valid(1000));
        assert!(range.is_valid(500));
        assert!(!range.is_valid(499));
    }
}
