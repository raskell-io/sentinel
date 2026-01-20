//! Agent installation logic
//!
//! Handles placing downloaded binaries in the correct locations and
//! optionally setting up configuration and systemd services.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during installation
#[derive(Debug, Error)]
pub enum InstallError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Installation directory does not exist: {0}")]
    DirNotFound(String),

    #[error("Failed to create directory: {0}")]
    CreateDir(String),
}

/// Installation paths configuration
#[derive(Debug, Clone)]
pub struct InstallPaths {
    /// Directory for agent binaries
    pub bin_dir: PathBuf,

    /// Directory for agent configuration files
    pub config_dir: PathBuf,

    /// Directory for systemd service files (Linux only)
    pub systemd_dir: Option<PathBuf>,

    /// Whether this is a system-wide install (requires root)
    pub system_wide: bool,
}

impl InstallPaths {
    /// Get default system-wide installation paths
    pub fn system() -> Self {
        Self {
            bin_dir: PathBuf::from("/usr/local/bin"),
            config_dir: PathBuf::from("/etc/sentinel/agents"),
            systemd_dir: Some(PathBuf::from("/etc/systemd/system")),
            system_wide: true,
        }
    }

    /// Get user-local installation paths
    pub fn user() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            bin_dir: PathBuf::from(&home).join(".local/bin"),
            config_dir: PathBuf::from(&home).join(".config/sentinel/agents"),
            systemd_dir: Some(PathBuf::from(&home).join(".config/systemd/user")),
            system_wide: false,
        }
    }

    /// Get paths for a custom prefix
    pub fn with_prefix(prefix: &Path) -> Self {
        Self {
            bin_dir: prefix.join("bin"),
            config_dir: prefix.join("etc/sentinel/agents"),
            systemd_dir: Some(prefix.join("lib/systemd/system")),
            system_wide: false,
        }
    }

    /// Determine the best installation paths based on current user
    pub fn detect() -> Self {
        // Check if we're root
        #[cfg(unix)]
        {
            if unsafe { libc::geteuid() } == 0 {
                return Self::system();
            }
        }

        // Check if /usr/local/bin is writable
        let system_paths = Self::system();
        if is_writable(&system_paths.bin_dir) {
            return system_paths;
        }

        // Fall back to user paths
        Self::user()
    }

    /// Ensure all directories exist
    pub fn ensure_dirs(&self) -> Result<(), InstallError> {
        create_dir_if_missing(&self.bin_dir)?;
        create_dir_if_missing(&self.config_dir)?;
        if let Some(ref systemd_dir) = self.systemd_dir {
            create_dir_if_missing(systemd_dir)?;
        }
        Ok(())
    }
}

/// Check if a directory is writable
fn is_writable(path: &Path) -> bool {
    if !path.exists() {
        // Check if we can create it
        if let Some(parent) = path.parent() {
            return is_writable(parent);
        }
        return false;
    }

    // Try to access the directory
    std::fs::metadata(path)
        .map(|m| !m.permissions().readonly())
        .unwrap_or(false)
}

/// Create a directory if it doesn't exist
fn create_dir_if_missing(path: &Path) -> Result<(), InstallError> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                InstallError::PermissionDenied(path.display().to_string())
            } else {
                InstallError::CreateDir(format!("{}: {}", path.display(), e))
            }
        })?;
    }
    Ok(())
}

/// Install a binary to the target directory
pub fn install_binary(source: &Path, dest_dir: &Path, name: &str) -> Result<PathBuf, InstallError> {
    let dest_path = dest_dir.join(name);

    tracing::info!(
        source = %source.display(),
        dest = %dest_path.display(),
        "Installing binary"
    );

    // Copy the file
    std::fs::copy(source, &dest_path)?;

    // Set permissions (executable)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest_path, perms)?;
    }

    Ok(dest_path)
}

/// Uninstall a binary
pub fn uninstall_binary(bin_dir: &Path, name: &str) -> Result<bool, InstallError> {
    let path = bin_dir.join(name);

    if path.exists() {
        tracing::info!(path = %path.display(), "Removing binary");
        std::fs::remove_file(&path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Check if a binary is installed and get its version
pub fn get_installed_version(bin_dir: &Path, binary_name: &str) -> Option<String> {
    let path = bin_dir.join(binary_name);

    if !path.exists() {
        return None;
    }

    // Try to run the binary with --version
    let output = std::process::Command::new(&path)
        .arg("--version")
        .output()
        .ok()?;

    if !output.status.success() {
        // Binary exists but doesn't support --version
        return Some("unknown".to_string());
    }

    // Parse version from output
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_version_output(&stdout)
}

/// Parse version from command output
fn parse_version_output(output: &str) -> Option<String> {
    // Common patterns:
    // "sentinel-waf-agent 0.2.0"
    // "version 0.2.0"
    // "0.2.0"

    for line in output.lines() {
        let line = line.trim();

        // Look for semver-like pattern
        for word in line.split_whitespace() {
            if word.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
                && word.contains('.')
            {
                // Remove any trailing metadata (e.g., "0.2.0-beta" -> "0.2.0")
                let version = word.split('-').next().unwrap_or(word);
                let version = version.split('+').next().unwrap_or(version);
                return Some(version.to_string());
            }
        }
    }

    None
}

/// Generate a default configuration file for an agent
pub fn generate_default_config(agent_name: &str) -> String {
    match agent_name {
        "waf" => include_str!("../../../../deploy/bundle/agents/waf.yaml").to_string(),
        "ratelimit" => include_str!("../../../../deploy/bundle/agents/ratelimit.yaml").to_string(),
        "denylist" => include_str!("../../../../deploy/bundle/agents/denylist.yaml").to_string(),
        _ => format!(
            "# {} agent configuration\n\
             # See https://sentinel.raskell.io/docs/agents/{}\n\n\
             socket:\n\
             \x20 path: /var/run/sentinel/{}.sock\n\
             \x20 mode: 0660\n\n\
             logging:\n\
             \x20 level: info\n\
             \x20 format: json\n",
            agent_name, agent_name, agent_name
        ),
    }
}

/// Install a configuration file
pub fn install_config(
    config_dir: &Path,
    agent_name: &str,
    content: &str,
    force: bool,
) -> Result<PathBuf, InstallError> {
    let config_path = config_dir.join(format!("{}.yaml", agent_name));

    // Don't overwrite existing config unless forced
    if config_path.exists() && !force {
        tracing::info!(
            path = %config_path.display(),
            "Config file already exists, skipping (use --force to overwrite)"
        );
        return Ok(config_path);
    }

    tracing::info!(
        path = %config_path.display(),
        "Installing configuration file"
    );

    std::fs::write(&config_path, content)?;
    Ok(config_path)
}

/// Generate a systemd service file for an agent
pub fn generate_systemd_service(agent_name: &str, bin_path: &Path, config_path: &Path) -> String {
    let binary_name = format!("sentinel-{}-agent", agent_name);

    format!(
        r#"[Unit]
Description=Sentinel {} Agent
Documentation=https://sentinel.raskell.io/docs/agents/{}
After=sentinel.service
BindsTo=sentinel.service
PartOf=sentinel.target

[Service]
Type=simple
ExecStart={} --config {}
Restart=on-failure
RestartSec=5s

User=sentinel
Group=sentinel

Environment="RUST_LOG=info,sentinel_{}_agent=info"

RuntimeDirectory=sentinel
RuntimeDirectoryMode=0755

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true

StandardOutput=journal
StandardError=journal
SyslogIdentifier=sentinel-{}

[Install]
WantedBy=sentinel.target
"#,
        agent_name,
        agent_name,
        bin_path.display(),
        config_path.display(),
        agent_name,
        agent_name
    )
}

/// Install a systemd service file
pub fn install_systemd_service(
    systemd_dir: &Path,
    agent_name: &str,
    content: &str,
) -> Result<PathBuf, InstallError> {
    let service_path = systemd_dir.join(format!("sentinel-{}.service", agent_name));

    tracing::info!(
        path = %service_path.display(),
        "Installing systemd service"
    );

    std::fs::write(&service_path, content)?;
    Ok(service_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_output() {
        assert_eq!(
            parse_version_output("sentinel-waf-agent 0.2.0"),
            Some("0.2.0".to_string())
        );
        assert_eq!(
            parse_version_output("version 1.0.0"),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            parse_version_output("0.3.1"),
            Some("0.3.1".to_string())
        );
        assert_eq!(
            parse_version_output("0.2.0-beta+build123"),
            Some("0.2.0".to_string())
        );
        assert_eq!(parse_version_output("no version here"), None);
    }

    #[test]
    fn test_install_paths_user() {
        let paths = InstallPaths::user();
        assert!(!paths.system_wide);
        assert!(paths.bin_dir.to_string_lossy().contains(".local"));
    }

    #[test]
    fn test_generate_default_config() {
        let config = generate_default_config("waf");
        assert!(config.contains("socket:"));
        assert!(config.contains("modsecurity:") || config.contains("waf"));

        let unknown = generate_default_config("unknown");
        assert!(unknown.contains("unknown agent configuration"));
    }
}
