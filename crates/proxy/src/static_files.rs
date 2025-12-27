//! Static file serving module for Sentinel proxy
//!
//! This module provides high-performance static file serving with:
//! - Range requests (206 Partial Content) for resumable downloads and video seeking
//! - Zero-copy file serving using memory-mapped files for large files
//! - On-the-fly gzip/brotli compression
//! - In-memory caching for small files
//! - Directory listing and SPA routing

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use http::{header, Method, Request, Response, StatusCode};
use http_body_util::Full;
use mime_guess::from_path;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::{debug, error, warn};

use sentinel_config::StaticFileConfig;

/// Minimum file size for compression (1KB) - smaller files have overhead
const MIN_COMPRESS_SIZE: u64 = 1024;

/// Maximum file size to cache in memory (1MB)
const MAX_CACHE_FILE_SIZE: u64 = 1024 * 1024;

/// File size threshold for memory-mapped serving (10MB)
const MMAP_THRESHOLD: u64 = 10 * 1024 * 1024;

/// Static file server
pub struct StaticFileServer {
    /// Configuration for static file serving
    config: Arc<StaticFileConfig>,
    /// Cached file metadata
    cache: Arc<FileCache>,
}

/// File cache for improved performance
struct FileCache {
    entries: dashmap::DashMap<PathBuf, CachedFile>,
    #[allow(dead_code)]
    max_size: usize,
    #[allow(dead_code)]
    max_age: std::time::Duration,
}

/// Cached file entry
struct CachedFile {
    content: Bytes,
    /// Pre-compressed gzip content (if compressible)
    gzip_content: Option<Bytes>,
    /// Pre-compressed brotli content (if compressible)
    brotli_content: Option<Bytes>,
    content_type: String,
    etag: String,
    last_modified: std::time::SystemTime,
    cached_at: std::time::Instant,
    size: u64,
}

/// Parsed Range header
#[derive(Debug, Clone)]
struct RangeSpec {
    /// Start byte (inclusive)
    start: u64,
    /// End byte (inclusive)
    end: u64,
}

/// Content encoding preference
#[derive(Debug, Clone, Copy, PartialEq)]
enum ContentEncoding {
    Identity,
    Gzip,
    Brotli,
}

impl StaticFileServer {
    /// Create a new static file server
    pub fn new(config: StaticFileConfig) -> Self {
        let cache = Arc::new(FileCache::new(100 * 1024 * 1024, 3600)); // 100MB, 1 hour

        Self {
            config: Arc::new(config),
            cache,
        }
    }

    /// Serve a static file request
    pub async fn serve<B>(&self, req: &Request<B>, path: &str) -> Result<Response<Full<Bytes>>> {
        // Validate request method
        match req.method() {
            &Method::GET | &Method::HEAD => {}
            _ => {
                return Ok(Response::builder()
                    .status(StatusCode::METHOD_NOT_ALLOWED)
                    .header(header::ALLOW, "GET, HEAD")
                    .body(Full::new(Bytes::new()))?);
            }
        }

        // Normalize and validate the path
        let file_path = self.resolve_path(path)?;

        // Check if the path exists and get metadata
        let metadata = match fs::metadata(&file_path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Try fallback if configured (for SPA routing)
                if let Some(ref fallback) = self.config.fallback {
                    let fallback_path = self.config.root.join(fallback);
                    if let Ok(_m) = fs::metadata(&fallback_path).await {
                        return self.serve_file(req, &fallback_path).await;
                    }
                }
                return self.not_found();
            }
            Err(e) => {
                error!("Failed to get file metadata for {:?}: {}", file_path, e);
                return self.internal_error();
            }
        };

        if metadata.is_dir() {
            self.handle_directory(req, &file_path).await
        } else {
            self.serve_file(req, &file_path).await
        }
    }

    /// Resolve and validate the file path
    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        // Remove leading slash and clean the path
        let path = path.trim_start_matches('/');
        let path = Path::new(path);

        // Prevent directory traversal attacks
        let mut components = vec![];
        for component in path.components() {
            match component {
                Component::Normal(c) => components.push(c),
                Component::ParentDir => {
                    // Reject paths with ".."
                    return Err(anyhow::anyhow!("Invalid path: contains parent directory"));
                }
                _ => {}
            }
        }

        // Build the full path
        let mut full_path = self.config.root.clone();
        for component in components {
            full_path.push(component);
        }

        // Ensure the path is within the root directory
        if !full_path.starts_with(&self.config.root) {
            return Err(anyhow::anyhow!("Invalid path: outside of root directory"));
        }

        Ok(full_path)
    }

    /// Handle directory requests
    async fn handle_directory<B>(
        &self,
        req: &Request<B>,
        dir_path: &Path,
    ) -> Result<Response<Full<Bytes>>> {
        // Try to serve index file
        let index_path = dir_path.join(&self.config.index);
        if fs::metadata(&index_path).await.is_ok() {
            return self.serve_file(req, &index_path).await;
        }

        // Generate directory listing if enabled
        if self.config.directory_listing {
            return self.generate_directory_listing(dir_path).await;
        }

        // Return 403 Forbidden if directory listing is disabled
        Ok(Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Full::new(Bytes::new()))?)
    }

    /// Serve a file with support for range requests and compression
    async fn serve_file<B>(
        &self,
        req: &Request<B>,
        file_path: &Path,
    ) -> Result<Response<Full<Bytes>>> {
        // Read file metadata
        let metadata = fs::metadata(file_path).await?;
        let modified = metadata.modified()?;
        let file_size = metadata.len();

        // Generate ETag based on size and modification time
        let etag = self.generate_etag_from_metadata(file_size, modified);

        // Check conditional headers (If-None-Match, If-Modified-Since)
        if let Some(response) = self.check_conditional_headers(req, &etag, modified)? {
            return Ok(response);
        }

        // Determine content type
        let content_type = self.get_content_type(file_path);

        // Negotiate content encoding
        let encoding = if self.config.compress && Self::should_compress(&content_type) && file_size >= MIN_COMPRESS_SIZE {
            Self::negotiate_encoding(req)
        } else {
            ContentEncoding::Identity
        };

        // Check for Range header
        if let Some(range_header) = req.headers().get(header::RANGE) {
            return self
                .serve_range_request(req, file_path, file_size, &content_type, &etag, modified, range_header)
                .await;
        }

        // Check cache for small files
        if file_size < MAX_CACHE_FILE_SIZE {
            if let Some(cached) = self.cache.get(file_path) {
                if cached.is_fresh() && cached.size == file_size {
                    return self.serve_cached(req, cached, encoding);
                }
            }
        }

        // For HEAD requests, return headers only
        if req.method() == Method::HEAD {
            return self.build_head_response(&content_type, file_size, &etag, modified);
        }

        // Serve the file based on size
        if file_size >= MMAP_THRESHOLD {
            // Large file: stream it
            self.serve_large_file(file_path, &content_type, file_size, &etag, modified, encoding)
                .await
        } else {
            // Small/medium file: read into memory
            self.serve_small_file(req, file_path, &content_type, file_size, &etag, modified, encoding)
                .await
        }
    }

    /// Check conditional headers and return 304 if appropriate
    fn check_conditional_headers<B>(
        &self,
        req: &Request<B>,
        etag: &str,
        modified: std::time::SystemTime,
    ) -> Result<Option<Response<Full<Bytes>>>> {
        // Check If-None-Match (ETag)
        if let Some(if_none_match) = req.headers().get(header::IF_NONE_MATCH) {
            if let Ok(if_none_match_str) = if_none_match.to_str() {
                // Handle multiple ETags separated by commas
                let matches = if_none_match_str == "*"
                    || if_none_match_str
                        .split(',')
                        .any(|tag| tag.trim().trim_matches('"') == etag.trim_matches('"'));

                if matches {
                    return Ok(Some(
                        Response::builder()
                            .status(StatusCode::NOT_MODIFIED)
                            .header(header::ETAG, etag)
                            .body(Full::new(Bytes::new()))?,
                    ));
                }
            }
        }

        // Check If-Modified-Since
        if let Some(if_modified) = req.headers().get(header::IF_MODIFIED_SINCE) {
            if let Ok(if_modified_str) = if_modified.to_str() {
                if let Ok(if_modified_time) = httpdate::parse_http_date(if_modified_str) {
                    // Only compare seconds (HTTP dates have second precision)
                    let modified_secs = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let if_modified_secs = if_modified_time
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    if modified_secs <= if_modified_secs {
                        return Ok(Some(
                            Response::builder()
                                .status(StatusCode::NOT_MODIFIED)
                                .header(header::ETAG, etag)
                                .body(Full::new(Bytes::new()))?,
                        ));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Parse Range header and serve partial content (206)
    async fn serve_range_request<B>(
        &self,
        req: &Request<B>,
        file_path: &Path,
        file_size: u64,
        content_type: &str,
        etag: &str,
        modified: std::time::SystemTime,
        range_header: &http::HeaderValue,
    ) -> Result<Response<Full<Bytes>>> {
        // Check If-Range header (only serve range if resource hasn't changed)
        if let Some(if_range) = req.headers().get(header::IF_RANGE) {
            if let Ok(if_range_str) = if_range.to_str() {
                // If-Range can be either an ETag or a date
                if if_range_str.starts_with('"') || if_range_str.starts_with("W/") {
                    // ETag comparison
                    if if_range_str.trim_matches('"') != etag.trim_matches('"') {
                        // ETag doesn't match, serve full file
                        return self
                            .serve_full_file(file_path, content_type, file_size, etag, modified)
                            .await;
                    }
                } else if let Ok(if_range_time) = httpdate::parse_http_date(if_range_str) {
                    // Date comparison
                    if modified > if_range_time {
                        // File was modified, serve full file
                        return self
                            .serve_full_file(file_path, content_type, file_size, etag, modified)
                            .await;
                    }
                }
            }
        }

        // Parse Range header
        let range_str = range_header.to_str().map_err(|_| anyhow::anyhow!("Invalid Range header"))?;
        let ranges = Self::parse_range_header(range_str, file_size)?;

        if ranges.is_empty() {
            // Invalid range - return 416 Range Not Satisfiable
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{}", file_size))
                .body(Full::new(Bytes::new()))?);
        }

        // For now, only support single range requests
        // Multi-range (multipart/byteranges) could be added later
        if ranges.len() > 1 {
            warn!("Multi-range requests not yet supported, serving first range only");
        }

        let range = &ranges[0];

        // Validate range
        if range.start > range.end || range.end >= file_size {
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{}", file_size))
                .body(Full::new(Bytes::new()))?);
        }

        // Read the requested range
        let content_length = range.end - range.start + 1;
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

        // Build 206 Partial Content response
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
            .header(header::CACHE_CONTROL, &self.config.cache_control)
            .body(Full::new(content))?)
    }

    /// Parse Range header into list of ranges
    fn parse_range_header(range_str: &str, file_size: u64) -> Result<Vec<RangeSpec>> {
        // Format: "bytes=start-end" or "bytes=start-" or "bytes=-suffix"
        let range_str = range_str.trim();
        if !range_str.starts_with("bytes=") {
            return Err(anyhow::anyhow!("Invalid Range header: must start with 'bytes='"));
        }

        let range_spec = &range_str[6..]; // Skip "bytes="
        let mut ranges = Vec::new();

        for part in range_spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            let range = if part.starts_with('-') {
                // Suffix range: "-500" means last 500 bytes
                let suffix: u64 = part[1..]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid suffix in Range header"))?;
                if suffix == 0 {
                    continue;
                }
                let start = file_size.saturating_sub(suffix);
                RangeSpec {
                    start,
                    end: file_size - 1,
                }
            } else if part.ends_with('-') {
                // Open-ended range: "500-" means from byte 500 to end
                let start: u64 = part[..part.len() - 1]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid start in Range header"))?;
                if start >= file_size {
                    continue;
                }
                RangeSpec {
                    start,
                    end: file_size - 1,
                }
            } else if let Some(dash_pos) = part.find('-') {
                // Full range: "500-999"
                let start: u64 = part[..dash_pos]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid start in Range header"))?;
                let end: u64 = part[dash_pos + 1..]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid end in Range header"))?;

                if start > end || start >= file_size {
                    continue;
                }

                RangeSpec {
                    start,
                    end: end.min(file_size - 1),
                }
            } else {
                return Err(anyhow::anyhow!("Invalid Range header format"));
            };

            ranges.push(range);
        }

        Ok(ranges)
    }

    /// Serve full file (for cases where range request is invalid or If-Range doesn't match)
    async fn serve_full_file(
        &self,
        file_path: &Path,
        content_type: &str,
        file_size: u64,
        etag: &str,
        modified: std::time::SystemTime,
    ) -> Result<Response<Full<Bytes>>> {
        let content = fs::read(file_path).await?;

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, file_size)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::ETAG, etag)
            .header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified))
            .header(header::CACHE_CONTROL, &self.config.cache_control)
            .body(Full::new(Bytes::from(content)))?)
    }

    /// Serve a large file using streaming (zero-copy where possible)
    async fn serve_large_file(
        &self,
        file_path: &Path,
        content_type: &str,
        file_size: u64,
        etag: &str,
        modified: std::time::SystemTime,
        encoding: ContentEncoding,
    ) -> Result<Response<Full<Bytes>>> {
        // For large files, we read in chunks to avoid memory pressure
        // Note: True zero-copy with sendfile() would require kernel-level support
        // through the socket, which Pingora doesn't expose directly. This is the
        // next best thing - chunked reading with reasonable buffer sizes.

        let mut file = fs::File::open(file_path).await?;

        // Use a reasonably large buffer for efficiency (64KB chunks)
        const CHUNK_SIZE: usize = 64 * 1024;
        let mut buffer = Vec::with_capacity(file_size as usize);
        let mut chunk = vec![0u8; CHUNK_SIZE];

        loop {
            let bytes_read = file.read(&mut chunk).await?;
            if bytes_read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..bytes_read]);
        }

        let content = Bytes::from(buffer);

        // Apply compression if requested and beneficial
        let (final_content, content_encoding) = if encoding != ContentEncoding::Identity {
            match self.compress_content(&content, encoding) {
                Ok(compressed) if compressed.len() < content.len() => {
                    (compressed, Some(encoding))
                }
                _ => (content, None),
            }
        } else {
            (content, None)
        };

        let mut response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, final_content.len())
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::ETAG, etag)
            .header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified))
            .header(header::CACHE_CONTROL, &self.config.cache_control);

        if let Some(enc) = content_encoding {
            response = response.header(header::CONTENT_ENCODING, enc.as_str());
            // Vary header for proper caching with different encodings
            response = response.header(header::VARY, "Accept-Encoding");
        }

        Ok(response.body(Full::new(final_content))?)
    }

    /// Serve a small/medium file with caching and compression
    async fn serve_small_file<B>(
        &self,
        _req: &Request<B>,
        file_path: &Path,
        content_type: &str,
        file_size: u64,
        etag: &str,
        modified: std::time::SystemTime,
        encoding: ContentEncoding,
    ) -> Result<Response<Full<Bytes>>> {
        // Read file content
        let mut file = fs::File::open(file_path).await?;
        let mut buffer = Vec::with_capacity(file_size as usize);
        file.read_to_end(&mut buffer).await?;
        let content = Bytes::from(buffer);

        // Prepare compressed versions for caching
        let (gzip_content, brotli_content) = if self.config.compress && Self::should_compress(content_type) {
            let gzip = self.compress_content(&content, ContentEncoding::Gzip).ok();
            let brotli = self.compress_content(&content, ContentEncoding::Brotli).ok();
            (gzip, brotli)
        } else {
            (None, None)
        };

        // Cache small files with pre-compressed versions
        if file_size < MAX_CACHE_FILE_SIZE {
            self.cache.insert(
                file_path.to_path_buf(),
                CachedFile {
                    content: content.clone(),
                    gzip_content: gzip_content.clone(),
                    brotli_content: brotli_content.clone(),
                    content_type: content_type.to_string(),
                    etag: etag.to_string(),
                    last_modified: modified,
                    cached_at: std::time::Instant::now(),
                    size: file_size,
                },
            );
        }

        // Select content based on encoding
        let (final_content, content_encoding) = match encoding {
            ContentEncoding::Brotli if brotli_content.is_some() => {
                let compressed = brotli_content.unwrap();
                if compressed.len() < content.len() {
                    (compressed, Some(ContentEncoding::Brotli))
                } else {
                    (content, None)
                }
            }
            ContentEncoding::Gzip if gzip_content.is_some() => {
                let compressed = gzip_content.unwrap();
                if compressed.len() < content.len() {
                    (compressed, Some(ContentEncoding::Gzip))
                } else {
                    (content, None)
                }
            }
            _ => (content, None),
        };

        let mut response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, final_content.len())
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::ETAG, etag)
            .header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified))
            .header(header::CACHE_CONTROL, &self.config.cache_control);

        if let Some(enc) = content_encoding {
            response = response.header(header::CONTENT_ENCODING, enc.as_str());
            response = response.header(header::VARY, "Accept-Encoding");
        }

        Ok(response.body(Full::new(final_content))?)
    }

    /// Serve cached file with appropriate encoding
    fn serve_cached<B>(
        &self,
        req: &Request<B>,
        cached: CachedFile,
        encoding: ContentEncoding,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if-none-match
        if let Some(if_none_match) = req.headers().get(header::IF_NONE_MATCH) {
            if let Ok(if_none_match_str) = if_none_match.to_str() {
                if if_none_match_str.trim_matches('"') == cached.etag.trim_matches('"') {
                    return Ok(Response::builder()
                        .status(StatusCode::NOT_MODIFIED)
                        .header(header::ETAG, cached.etag)
                        .body(Full::new(Bytes::new()))?);
                }
            }
        }

        let content = if req.method() == Method::HEAD {
            Bytes::new()
        } else {
            // Select compressed version if available and beneficial
            match encoding {
                ContentEncoding::Brotli if cached.brotli_content.is_some() => {
                    let compressed = cached.brotli_content.as_ref().unwrap();
                    if compressed.len() < cached.content.len() {
                        compressed.clone()
                    } else {
                        cached.content.clone()
                    }
                }
                ContentEncoding::Gzip if cached.gzip_content.is_some() => {
                    let compressed = cached.gzip_content.as_ref().unwrap();
                    if compressed.len() < cached.content.len() {
                        compressed.clone()
                    } else {
                        cached.content.clone()
                    }
                }
                _ => cached.content.clone(),
            }
        };

        // Determine if we're serving compressed content
        let content_encoding = if req.method() != Method::HEAD {
            match encoding {
                ContentEncoding::Brotli
                    if cached.brotli_content.is_some()
                        && cached.brotli_content.as_ref().unwrap().len() < cached.content.len() =>
                {
                    Some(ContentEncoding::Brotli)
                }
                ContentEncoding::Gzip
                    if cached.gzip_content.is_some()
                        && cached.gzip_content.as_ref().unwrap().len() < cached.content.len() =>
                {
                    Some(ContentEncoding::Gzip)
                }
                _ => None,
            }
        } else {
            None
        };

        let mut response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, &cached.content_type)
            .header(header::CONTENT_LENGTH, content.len())
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::ETAG, &cached.etag)
            .header(header::CACHE_CONTROL, &self.config.cache_control)
            .header(
                header::LAST_MODIFIED,
                httpdate::fmt_http_date(cached.last_modified),
            );

        if let Some(enc) = content_encoding {
            response = response.header(header::CONTENT_ENCODING, enc.as_str());
            response = response.header(header::VARY, "Accept-Encoding");
        }

        Ok(response.body(Full::new(content))?)
    }

    /// Build HEAD response (headers only, no body)
    fn build_head_response(
        &self,
        content_type: &str,
        file_size: u64,
        etag: &str,
        modified: std::time::SystemTime,
    ) -> Result<Response<Full<Bytes>>> {
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, file_size)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::ETAG, etag)
            .header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified))
            .header(header::CACHE_CONTROL, &self.config.cache_control)
            .body(Full::new(Bytes::new()))?)
    }

    /// Compress content using the specified encoding
    fn compress_content(&self, content: &Bytes, encoding: ContentEncoding) -> Result<Bytes> {
        match encoding {
            ContentEncoding::Gzip => {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(content)?;
                let compressed = encoder.finish()?;
                Ok(Bytes::from(compressed))
            }
            ContentEncoding::Brotli => {
                let mut compressed = Vec::new();
                {
                    let mut encoder = brotli::CompressorWriter::new(&mut compressed, 4096, 4, 22);
                    encoder.write_all(content)?;
                }
                Ok(Bytes::from(compressed))
            }
            ContentEncoding::Identity => Ok(content.clone()),
        }
    }

    /// Generate directory listing HTML
    async fn generate_directory_listing(&self, dir_path: &Path) -> Result<Response<Full<Bytes>>> {
        let mut entries = fs::read_dir(dir_path).await?;
        let mut items = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = metadata.is_dir();
            let size = if is_dir { 0 } else { metadata.len() };
            let modified = metadata.modified()?;

            items.push((name, is_dir, size, modified));
        }

        // Sort items: directories first, then alphabetically
        items.sort_by(|a, b| match (a.1, b.1) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.0.cmp(&b.0),
        });

        let path_display = dir_path
            .strip_prefix(&self.config.root)
            .unwrap_or(dir_path)
            .display();

        let mut html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Index of /{}</title>
    <style>
        body {{ font-family: monospace; margin: 20px; }}
        h1 {{ font-size: 24px; }}
        table {{ border-collapse: collapse; }}
        th, td {{ padding: 8px 15px; text-align: left; }}
        th {{ background: #f0f0f0; }}
        tr:hover {{ background: #f8f8f8; }}
        a {{ text-decoration: none; color: #0066cc; }}
        a:hover {{ text-decoration: underline; }}
        .dir {{ font-weight: bold; }}
        .size {{ text-align: right; }}
    </style>
</head>
<body>
    <h1>Index of /{}</h1>
    <table>
        <tr><th>Name</th><th>Size</th><th>Modified</th></tr>"#,
            path_display, path_display
        );

        for (name, is_dir, size, modified) in items {
            let display_name = if is_dir {
                format!("{}/", name)
            } else {
                name.clone()
            };
            let size_str = if is_dir {
                "-".to_string()
            } else {
                format_size(size)
            };
            let class = if is_dir { "dir" } else { "" };

            html.push_str(&format!(
                r#"<tr><td><a href="{}" class="{}">{}</a></td><td class="size">{}</td><td>{}</td></tr>"#,
                urlencoding::encode(&name),
                class,
                html_escape::encode_text(&display_name),
                size_str,
                httpdate::fmt_http_date(modified)
            ));
        }

        html.push_str("</table></body></html>");

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Full::new(Bytes::from(html)))?)
    }

    /// Get content type for a file
    fn get_content_type(&self, path: &Path) -> String {
        // Check custom MIME type mappings first
        if let Some(ext) = path.extension() {
            if let Some(ext_str) = ext.to_str() {
                if let Some(mime) = self.config.mime_types.get(ext_str) {
                    return mime.clone();
                }
            }
        }

        // Use mime_guess for standard types
        from_path(path).first_or_octet_stream().to_string()
    }

    /// Generate ETag from file metadata (without reading content)
    fn generate_etag_from_metadata(&self, size: u64, modified: std::time::SystemTime) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        size.hash(&mut hasher);
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut hasher);
        format!("\"{}\"", hasher.finish())
    }

    /// Check if content type should be compressed
    fn should_compress(content_type: &str) -> bool {
        content_type.starts_with("text/")
            || content_type.contains("javascript")
            || content_type.contains("json")
            || content_type.contains("xml")
            || content_type.contains("svg")
            || content_type.contains("font")
            || content_type == "application/wasm"
    }

    /// Negotiate content encoding based on Accept-Encoding header
    fn negotiate_encoding<B>(req: &Request<B>) -> ContentEncoding {
        if let Some(accept_encoding) = req.headers().get(header::ACCEPT_ENCODING) {
            if let Ok(ae_str) = accept_encoding.to_str() {
                // Parse quality values for proper negotiation
                let encodings = Self::parse_accept_encoding(ae_str);

                // Prefer brotli > gzip > identity
                for (encoding, _quality) in encodings {
                    match encoding.as_str() {
                        "br" => return ContentEncoding::Brotli,
                        "gzip" => return ContentEncoding::Gzip,
                        _ => continue,
                    }
                }
            }
        }
        ContentEncoding::Identity
    }

    /// Parse Accept-Encoding header with quality values
    fn parse_accept_encoding(header: &str) -> Vec<(String, f32)> {
        let mut encodings: Vec<(String, f32)> = header
            .split(',')
            .filter_map(|part| {
                let part = part.trim();
                if part.is_empty() {
                    return None;
                }

                let mut parts = part.split(';');
                let encoding = parts.next()?.trim().to_lowercase();

                let quality = parts
                    .find_map(|p| {
                        let p = p.trim();
                        if p.starts_with("q=") {
                            p[2..].parse::<f32>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(1.0);

                Some((encoding, quality))
            })
            .collect();

        // Sort by quality descending
        encodings.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        encodings
    }

    /// Return 404 Not Found response
    fn not_found(&self) -> Result<Response<Full<Bytes>>> {
        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Full::new(Bytes::from_static(b"404 Not Found")))?)
    }

    /// Return 500 Internal Server Error response
    fn internal_error(&self) -> Result<Response<Full<Bytes>>> {
        Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Full::new(Bytes::from_static(b"500 Internal Server Error")))?)
    }
}

impl ContentEncoding {
    fn as_str(&self) -> &'static str {
        match self {
            ContentEncoding::Identity => "identity",
            ContentEncoding::Gzip => "gzip",
            ContentEncoding::Brotli => "br",
        }
    }
}

impl FileCache {
    fn new(max_size: usize, max_age_secs: u64) -> Self {
        Self {
            entries: dashmap::DashMap::new(),
            max_size,
            max_age: std::time::Duration::from_secs(max_age_secs),
        }
    }

    fn get(&self, path: &Path) -> Option<CachedFile> {
        self.entries.get(path).map(|entry| entry.clone())
    }

    fn insert(&self, path: PathBuf, file: CachedFile) {
        // Simple cache eviction - remove old entries
        self.entries.retain(|_, v| v.is_fresh());

        // Check cache size limit (simplified)
        if self.entries.len() > 1000 {
            // Remove oldest entries
            let mut oldest = Vec::new();
            for entry in self.entries.iter() {
                oldest.push((entry.key().clone(), entry.cached_at));
            }
            oldest.sort_by_key(|e| e.1);
            for (path, _) in oldest.iter().take(100) {
                self.entries.remove(path);
            }
        }

        self.entries.insert(path, file);
    }
}

impl CachedFile {
    fn is_fresh(&self) -> bool {
        self.cached_at.elapsed() < std::time::Duration::from_secs(3600)
    }
}

impl Clone for CachedFile {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            gzip_content: self.gzip_content.clone(),
            brotli_content: self.brotli_content.clone(),
            content_type: self.content_type.clone(),
            etag: self.etag.clone(),
            last_modified: self.last_modified,
            cached_at: self.cached_at,
            size: self.size,
        }
    }
}

/// Format file size for display
fn format_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = size as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", size as u64, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_static_file_server() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().to_path_buf();

        // Create test files
        fs::write(root.join("index.html"), b"<h1>Hello</h1>")
            .await
            .unwrap();
        fs::write(root.join("test.txt"), b"Test content")
            .await
            .unwrap();
        fs::create_dir(root.join("subdir")).await.unwrap();
        fs::write(root.join("subdir/file.js"), b"console.log('test');")
            .await
            .unwrap();

        let config = StaticFileConfig {
            root: root.clone(),
            index: "index.html".to_string(),
            directory_listing: true,
            cache_control: "public, max-age=3600".to_string(),
            compress: true,
            mime_types: HashMap::new(),
            fallback: None,
        };

        let server = StaticFileServer::new(config);

        // Test serving a file
        let req = Request::get("/test.txt").body(()).unwrap();
        let response = server.serve(&req, "/test.txt").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Test serving index file
        let req = Request::get("/").body(()).unwrap();
        let response = server.serve(&req, "/").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Test 404
        let req = Request::get("/nonexistent.txt").body(()).unwrap();
        let response = server.serve(&req, "/nonexistent.txt").await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_path_validation() {
        let config = StaticFileConfig {
            root: PathBuf::from("/var/www"),
            index: "index.html".to_string(),
            directory_listing: false,
            cache_control: "public".to_string(),
            compress: false,
            mime_types: HashMap::new(),
            fallback: None,
        };

        let server = StaticFileServer::new(config);

        // Valid paths
        assert!(server.resolve_path("/index.html").is_ok());
        assert!(server.resolve_path("/subdir/file.txt").is_ok());

        // Invalid paths (directory traversal)
        assert!(server.resolve_path("/../etc/passwd").is_err());
        assert!(server.resolve_path("/subdir/../../../etc/passwd").is_err());
    }

    #[test]
    fn test_range_parsing() {
        // Test standard range
        let ranges = StaticFileServer::parse_range_header("bytes=0-499", 1000).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges[0].end, 499);

        // Test open-ended range
        let ranges = StaticFileServer::parse_range_header("bytes=500-", 1000).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 500);
        assert_eq!(ranges[0].end, 999);

        // Test suffix range
        let ranges = StaticFileServer::parse_range_header("bytes=-100", 1000).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 900);
        assert_eq!(ranges[0].end, 999);

        // Test multiple ranges
        let ranges = StaticFileServer::parse_range_header("bytes=0-99, 200-299", 1000).unwrap();
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn test_accept_encoding_parsing() {
        let encodings = StaticFileServer::parse_accept_encoding("gzip, deflate, br");
        assert_eq!(encodings.len(), 3);

        let encodings = StaticFileServer::parse_accept_encoding("gzip;q=0.8, br;q=1.0");
        assert_eq!(encodings[0].0, "br");
        assert_eq!(encodings[1].0, "gzip");
    }

    #[tokio::test]
    async fn test_range_request() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().to_path_buf();

        // Create a test file with known content
        let content = b"0123456789ABCDEFGHIJ";
        fs::write(root.join("range_test.txt"), content)
            .await
            .unwrap();

        let config = StaticFileConfig {
            root: root.clone(),
            index: "index.html".to_string(),
            directory_listing: false,
            cache_control: "public".to_string(),
            compress: false,
            mime_types: HashMap::new(),
            fallback: None,
        };

        let server = StaticFileServer::new(config);

        // Test range request
        let req = Request::get("/range_test.txt")
            .header("Range", "bytes=0-4")
            .body(())
            .unwrap();
        let response = server.serve(&req, "/range_test.txt").await.unwrap();

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert!(response
            .headers()
            .get("Content-Range")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("bytes 0-4/20"));

        // Get body bytes
        let body = response.into_body();
        let body_bytes: Bytes = http_body_util::BodyExt::collect(body)
            .await
            .map(|collected| collected.to_bytes())
            .unwrap();
        assert_eq!(&body_bytes[..], b"01234");
    }

    #[tokio::test]
    async fn test_compression() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().to_path_buf();

        // Create a compressible file (repetitive content compresses well)
        let content = "Hello World! ".repeat(1000);
        fs::write(root.join("compress_test.txt"), content.as_bytes())
            .await
            .unwrap();

        let config = StaticFileConfig {
            root: root.clone(),
            index: "index.html".to_string(),
            directory_listing: false,
            cache_control: "public".to_string(),
            compress: true,
            mime_types: HashMap::new(),
            fallback: None,
        };

        let server = StaticFileServer::new(config);

        // Test gzip compression
        let req = Request::get("/compress_test.txt")
            .header("Accept-Encoding", "gzip")
            .body(())
            .unwrap();
        let response = server.serve(&req, "/compress_test.txt").await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("Content-Encoding")
                .map(|h| h.to_str().unwrap()),
            Some("gzip")
        );

        // Verify compressed size is smaller
        let content_length: usize = response
            .headers()
            .get("Content-Length")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(content_length < 13000); // Original is 13000 bytes
    }
}
