//! Agent download functionality
//!
//! Downloads agent binaries from their GitHub releases.

use crate::bundle::lock::AgentInfo;
use flate2::read::GzDecoder;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use tar::Archive;
use thiserror::Error;

/// Errors that can occur during download
#[derive(Debug, Error)]
pub enum FetchError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Failed to create temporary file: {0}")]
    TempFile(#[from] io::Error),

    #[error("Download failed with status {status}: {url}")]
    DownloadFailed { url: String, status: u16 },

    #[error("Checksum verification failed for {agent}")]
    ChecksumMismatch { agent: String },

    #[error("Failed to extract archive: {0}")]
    Extract(String),

    #[error("Binary not found in archive: {0}")]
    BinaryNotFound(String),
}

/// Result of a download operation
pub struct DownloadResult {
    /// Path to the downloaded and extracted binary
    pub binary_path: PathBuf,

    /// Size of the downloaded archive in bytes
    pub archive_size: u64,

    /// Whether checksum was verified
    pub checksum_verified: bool,
}

/// Detect the current operating system
pub fn detect_os() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "unknown"
    }
}

/// Detect the current architecture
pub fn detect_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "amd64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        "unknown"
    }
}

/// Download an agent binary to a temporary directory
///
/// Returns the path to the extracted binary.
pub async fn download_agent(
    agent: &AgentInfo,
    temp_dir: &Path,
    verify_checksum: bool,
) -> Result<DownloadResult, FetchError> {
    let os = detect_os();
    let arch = detect_arch();

    let url = agent.download_url(os, arch);
    let checksum_url = agent.checksum_url(os, arch);

    tracing::info!(
        agent = %agent.name,
        version = %agent.version,
        url = %url,
        "Downloading agent"
    );

    let client = reqwest::Client::builder()
        .user_agent("sentinel-bundle")
        .build()?;

    // Download the archive
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        return Err(FetchError::DownloadFailed {
            url,
            status: response.status().as_u16(),
        });
    }

    let archive_bytes = response.bytes().await?;
    let archive_size = archive_bytes.len() as u64;

    // Verify checksum if requested
    let checksum_verified = if verify_checksum {
        match verify_sha256(&client, &checksum_url, &archive_bytes).await {
            Ok(true) => {
                tracing::debug!(agent = %agent.name, "Checksum verified");
                true
            }
            Ok(false) => {
                return Err(FetchError::ChecksumMismatch {
                    agent: agent.name.clone(),
                });
            }
            Err(e) => {
                tracing::warn!(
                    agent = %agent.name,
                    error = %e,
                    "Checksum verification skipped (file not available)"
                );
                false
            }
        }
    } else {
        false
    };

    // Extract the archive
    let binary_path = extract_archive(&archive_bytes, &agent.binary_name, temp_dir)?;

    Ok(DownloadResult {
        binary_path,
        archive_size,
        checksum_verified,
    })
}

/// Verify SHA256 checksum of downloaded data
async fn verify_sha256(
    client: &reqwest::Client,
    checksum_url: &str,
    data: &[u8],
) -> Result<bool, FetchError> {
    use sha2::{Digest, Sha256};

    // Download checksum file
    let response = client.get(checksum_url).send().await?;

    if !response.status().is_success() {
        return Err(FetchError::DownloadFailed {
            url: checksum_url.to_string(),
            status: response.status().as_u16(),
        });
    }

    let checksum_content = response.text().await?;

    // Parse expected checksum (format: "sha256hash  filename")
    let expected = checksum_content
        .split_whitespace()
        .next()
        .ok_or_else(|| FetchError::Extract("Invalid checksum file format".to_string()))?
        .to_lowercase();

    // Calculate actual checksum
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = hex::encode(hasher.finalize());

    Ok(expected == actual)
}

/// Extract a tarball and find the binary
fn extract_archive(
    archive_bytes: &[u8],
    binary_name: &str,
    dest_dir: &Path,
) -> Result<PathBuf, FetchError> {
    let decoder = GzDecoder::new(archive_bytes);
    let mut archive = Archive::new(decoder);

    // Create destination directory
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| FetchError::Extract(format!("Failed to create directory: {}", e)))?;

    // Extract all files
    archive
        .unpack(dest_dir)
        .map_err(|e| FetchError::Extract(format!("Failed to extract: {}", e)))?;

    // Find the binary (might be at top level or in a subdirectory)
    let binary_path = find_binary(dest_dir, binary_name)?;

    // Make it executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&binary_path)
            .map_err(|e| FetchError::Extract(e.to_string()))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&binary_path, perms)
            .map_err(|e| FetchError::Extract(e.to_string()))?;
    }

    Ok(binary_path)
}

/// Find the binary in the extracted directory
fn find_binary(dir: &Path, binary_name: &str) -> Result<PathBuf, FetchError> {
    // Check top level
    let direct_path = dir.join(binary_name);
    if direct_path.exists() {
        return Ok(direct_path);
    }

    // Check bin subdirectory
    let bin_path = dir.join("bin").join(binary_name);
    if bin_path.exists() {
        return Ok(bin_path);
    }

    // Search recursively
    for entry in walkdir(dir) {
        if let Ok(entry) = entry {
            if entry.file_name().to_string_lossy() == binary_name {
                return Ok(entry.path().to_path_buf());
            }
        }
    }

    Err(FetchError::BinaryNotFound(binary_name.to_string()))
}

/// Simple recursive directory walker
fn walkdir(dir: &Path) -> impl Iterator<Item = io::Result<std::fs::DirEntry>> + '_ {
    WalkDir::new(dir)
}

struct WalkDir {
    stack: Vec<PathBuf>,
}

impl WalkDir {
    fn new(dir: &Path) -> Self {
        Self {
            stack: vec![dir.to_path_buf()],
        }
    }
}

impl Iterator for WalkDir {
    type Item = io::Result<std::fs::DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(dir) = self.stack.pop() {
            match std::fs::read_dir(&dir) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(e) => {
                                if e.path().is_dir() {
                                    self.stack.push(e.path());
                                }
                                return Some(Ok(e));
                            }
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }
                Err(e) => return Some(Err(e)),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_platform() {
        let os = detect_os();
        let arch = detect_arch();

        // Should detect something
        assert!(!os.is_empty());
        assert!(!arch.is_empty());

        // On common platforms
        #[cfg(target_os = "linux")]
        assert_eq!(os, "linux");

        #[cfg(target_os = "macos")]
        assert_eq!(os, "darwin");

        #[cfg(target_arch = "x86_64")]
        assert_eq!(arch, "amd64");

        #[cfg(target_arch = "aarch64")]
        assert_eq!(arch, "arm64");
    }
}
