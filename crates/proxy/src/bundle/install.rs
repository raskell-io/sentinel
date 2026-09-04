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
            config_dir: PathBuf::from("/etc/zentinel/agents"),
            systemd_dir: Some(PathBuf::from("/etc/systemd/system")),
            system_wide: true,
        }
    }

    /// Get user-local installation paths
    pub fn user() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            bin_dir: PathBuf::from(&home).join(".local/bin"),
            config_dir: PathBuf::from(&home).join(".config/zentinel/agents"),
            systemd_dir: Some(PathBuf::from(&home).join(".config/systemd/user")),
            system_wide: false,
        }
    }

    /// Get paths for a custom prefix
    pub fn with_prefix(prefix: &Path) -> Self {
        Self {
            bin_dir: prefix.join("bin"),
            config_dir: prefix.join("etc/zentinel/agents"),
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

        // Every directory the install will create must be writable, not just
        // the binary one. Checking `bin_dir` alone chose a system-wide install
        // and then failed on `/etc/zentinel/agents`, which is the error users
        // actually saw.
        let system_paths = Self::system();
        if system_paths.all_writable() {
            return system_paths;
        }

        // Fall back to user paths
        Self::user()
    }

    /// Whether every directory this install would create is writable.
    ///
    /// Deliberately the same set [`Self::ensure_dirs`] creates: a check that
    /// covers fewer paths than the operation is a check that passes and then
    /// fails.
    pub fn all_writable(&self) -> bool {
        is_writable(&self.bin_dir)
            && is_writable(&self.config_dir)
            && self
                .systemd_dir
                .as_ref()
                .map(|d| is_writable(d))
                .unwrap_or(true)
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
/// Whether **this process** can create entries in `path`.
///
/// The obvious implementation is wrong, and was here:
///
/// ```ignore
/// std::fs::metadata(path).map(|m| !m.permissions().readonly())
/// ```
///
/// On Unix `Permissions::readonly()` is `mode & 0o222 == 0` — it answers "can
/// *anyone* write to this?", not "can *I*?". A root-owned `drwxr-xr-x`
/// directory has the owner write bit set, so that returned `true` for every
/// user on the system. `/usr/local/bin` is exactly such a directory on a
/// standard install, which meant [`InstallPaths::detect`] chose a system-wide
/// install for every non-root user and then failed creating `/etc/zentinel/agents`.
/// The user-local fallback below it was unreachable.
///
/// `access(2)` asks the question actually being asked, and answers it for the
/// real user rather than by inspecting mode bits.
fn is_writable(path: &Path) -> bool {
    if !path.exists() {
        // Not there yet: we can create it if we can write to its parent.
        return match path.parent() {
            Some(parent) => is_writable(parent),
            None => false,
        };
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // W_OK | X_OK: creating an entry needs write *and* search on a directory.
        unsafe { libc::access(c_path.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
    }

    #[cfg(not(unix))]
    {
        std::fs::metadata(path)
            .map(|m| !m.permissions().readonly())
            .unwrap_or(false)
    }
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
    // "zentinel-waf-agent 0.2.0"
    // "version 0.2.0"
    // "0.2.0"

    for line in output.lines() {
        let line = line.trim();

        // Look for semver-like pattern
        for word in line.split_whitespace() {
            if word
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
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
        "waf" => r#"# WAF Agent Configuration
# ModSecurity-based Web Application Firewall
# See https://zentinelproxy.io/docs/agents/waf

socket:
  path: /var/run/zentinel/waf.sock
  mode: 0660

logging:
  level: info
  format: json

modsecurity:
  engine: "On"

crs:
  paranoia_level: 1
  inbound_anomaly_score_threshold: 5
  outbound_anomaly_score_threshold: 4
"#
        .to_string(),

        "ratelimit" => r#"# Rate Limit Agent Configuration
# Token bucket rate limiting
# See https://zentinelproxy.io/docs/agents/ratelimit

socket:
  path: /var/run/zentinel/ratelimit.sock
  mode: 0660

logging:
  level: info
  format: json

rules:
  - name: default
    match:
      path_prefix: /
    limit:
      requests_per_second: 100
      burst: 200
    key: client_ip
"#
        .to_string(),

        "denylist" => r#"# Denylist Agent Configuration
# IP and path blocking
# See https://zentinelproxy.io/docs/agents/denylist

socket:
  path: /var/run/zentinel/denylist.sock
  mode: 0660

logging:
  level: info
  format: json

ip_denylist:
  enabled: true
  # Add IPs to block:
  # ips:
  #   - 192.168.1.100
  #   - 10.0.0.0/8

path_denylist:
  enabled: true
  patterns:
    - ".*\\.php$"
    - "/wp-admin.*"
    - "/wp-login.*"
    - "/.env"
    - "/\\.git.*"
"#
        .to_string(),

        _ => format!(
            "# {} agent configuration\n\
             # See https://zentinelproxy.io/docs/agents/{}\n\n\
             socket:\n\
               path: /var/run/zentinel/{}.sock\n\
               mode: 0660\n\n\
             logging:\n\
               level: info\n\
               format: json\n",
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
    let binary_name = format!("zentinel-{}-agent", agent_name);

    format!(
        r#"[Unit]
Description=Zentinel {} Agent
Documentation=https://zentinelproxy.io/docs/agents/{}
After=zentinel.service
BindsTo=zentinel.service
PartOf=zentinel.target

[Service]
Type=simple
ExecStart={} --config {}
Restart=on-failure
RestartSec=5s

User=zentinel
Group=zentinel

Environment="RUST_LOG=info,zentinel_{}_agent=info"

RuntimeDirectory=zentinel
RuntimeDirectoryMode=0755

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true

StandardOutput=journal
StandardError=journal
SyslogIdentifier=zentinel-{}

[Install]
WantedBy=zentinel.target
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
    let service_path = systemd_dir.join(format!("zentinel-{}.service", agent_name));

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
            parse_version_output("zentinel-waf-agent 0.2.0"),
            Some("0.2.0".to_string())
        );
        assert_eq!(
            parse_version_output("version 1.0.0"),
            Some("1.0.0".to_string())
        );
        assert_eq!(parse_version_output("0.3.1"), Some("0.3.1".to_string()));
        assert_eq!(
            parse_version_output("0.2.0-beta+build123"),
            Some("0.2.0".to_string())
        );
        assert_eq!(parse_version_output("no version here"), None);
    }

    #[test]
    fn test_parse_version_multiline() {
        let output = "zentinel-waf-agent\nversion: 0.2.0\nbuilt with rustc";
        assert_eq!(parse_version_output(output), Some("0.2.0".to_string()));
    }

    #[test]
    fn test_parse_version_with_v_prefix() {
        // Version number must start with digit, so "v0.2.0" wouldn't match
        assert_eq!(
            parse_version_output("version v0.2.0"),
            None // v0.2.0 starts with 'v', not a digit
        );
        // Test with version number not in parentheses
        assert_eq!(
            parse_version_output("myapp v0.2.0 version 0.2.0"),
            Some("0.2.0".to_string())
        );
    }

    /// The defect this replaced: `readonly()` asks whether *anyone* can write,
    /// so a root-owned `drwxr-xr-x` directory looked writable to every user and
    /// `detect()` chose a system-wide install that then failed on `/etc`.
    ///
    /// Skipped when running as root, where the answer is legitimately yes.
    #[test]
    fn a_root_owned_directory_is_not_writable_by_an_ordinary_user() {
        #[cfg(unix)]
        {
            if unsafe { libc::geteuid() } == 0 {
                return; // root can write it, and should
            }
            let system = Path::new("/usr/local/bin");
            if system.exists() {
                assert!(
                    !is_writable(system),
                    "/usr/local/bin reported writable to a non-root user; \
                     detect() would choose a system-wide install that cannot complete"
                );
            }
            // `/etc` is root-owned on every supported platform.
            assert!(!is_writable(Path::new("/etc")));
        }
    }

    /// A directory this user owns is writable, so the check is not simply
    /// always-false.
    #[test]
    fn a_directory_we_own_is_writable() {
        let dir = std::env::temp_dir();
        assert!(is_writable(&dir), "temp dir should be writable: {dir:?}");
    }

    /// A path that does not exist yet is writable if its parent is, since that
    /// is where it would be created.
    #[test]
    fn a_missing_directory_defers_to_its_parent() {
        let missing = std::env::temp_dir().join("zentinel-install-check-does-not-exist");
        assert!(is_writable(&missing));
        assert!(!is_writable(Path::new(
            "/etc/zentinel-install-check-does-not-exist"
        )));
    }

    /// `detect()` must not choose a system-wide install for a non-root user:
    /// that is the bug users hit, and it failed only later, at `ensure_dirs`.
    #[test]
    fn detect_does_not_choose_system_paths_without_the_rights_to_use_them() {
        #[cfg(unix)]
        {
            if unsafe { libc::geteuid() } == 0 {
                return;
            }
            let paths = InstallPaths::detect();
            assert!(
                !paths.system_wide,
                "detect() chose a system-wide install as a non-root user"
            );
            assert!(
                paths.all_writable(),
                "detect() returned paths it cannot actually create"
            );
        }
    }

    /// The writability check must cover the same directories the install
    /// creates. Checking only `bin_dir` is what let a doomed install start.
    #[test]
    fn all_writable_covers_every_directory_ensure_dirs_creates() {
        let system = InstallPaths::system();
        #[cfg(unix)]
        {
            if unsafe { libc::geteuid() } != 0 {
                assert!(
                    !system.all_writable(),
                    "system paths cannot all be writable to a non-root user"
                );
            }
        }
        let user = InstallPaths::user();
        assert!(user.all_writable(), "the user fallback must be usable");
    }

    #[test]
    fn test_install_paths_system() {
        let paths = InstallPaths::system();
        assert!(paths.system_wide);
        assert_eq!(paths.bin_dir, PathBuf::from("/usr/local/bin"));
        assert_eq!(paths.config_dir, PathBuf::from("/etc/zentinel/agents"));
        assert_eq!(
            paths.systemd_dir,
            Some(PathBuf::from("/etc/systemd/system"))
        );
    }

    #[test]
    fn test_install_paths_user() {
        let paths = InstallPaths::user();
        assert!(!paths.system_wide);
        assert!(paths.bin_dir.to_string_lossy().contains(".local"));
        assert!(paths.config_dir.to_string_lossy().contains(".config"));
    }

    #[test]
    fn test_install_paths_with_prefix() {
        let paths = InstallPaths::with_prefix(Path::new("/opt/zentinel"));
        assert!(!paths.system_wide);
        assert_eq!(paths.bin_dir, PathBuf::from("/opt/zentinel/bin"));
        assert_eq!(
            paths.config_dir,
            PathBuf::from("/opt/zentinel/etc/zentinel/agents")
        );
    }

    #[test]
    fn test_generate_default_config_waf() {
        let config = generate_default_config("waf");
        assert!(config.contains("socket:"));
        assert!(config.contains("modsecurity:"));
        assert!(config.contains("crs:"));
        assert!(config.contains("paranoia_level"));
        assert!(config.contains("/var/run/zentinel/waf.sock"));
    }

    #[test]
    fn test_generate_default_config_ratelimit() {
        let config = generate_default_config("ratelimit");
        assert!(config.contains("socket:"));
        assert!(config.contains("rules:"));
        assert!(config.contains("requests_per_second"));
        assert!(config.contains("burst"));
        assert!(config.contains("/var/run/zentinel/ratelimit.sock"));
    }

    #[test]
    fn test_generate_default_config_denylist() {
        let config = generate_default_config("denylist");
        assert!(config.contains("socket:"));
        assert!(config.contains("ip_denylist:"));
        assert!(config.contains("path_denylist:"));
        assert!(config.contains("patterns:"));
        assert!(config.contains("/var/run/zentinel/denylist.sock"));
    }

    #[test]
    fn test_generate_default_config_unknown() {
        let config = generate_default_config("custom");
        assert!(config.contains("custom agent configuration"));
        assert!(config.contains("/var/run/zentinel/custom.sock"));
    }

    #[test]
    fn test_generate_systemd_service() {
        let service = generate_systemd_service(
            "waf",
            Path::new("/usr/local/bin/zentinel-waf-agent"),
            Path::new("/etc/zentinel/agents/waf.yaml"),
        );

        assert!(service.contains("[Unit]"));
        assert!(service.contains("[Service]"));
        assert!(service.contains("[Install]"));
        assert!(service.contains("Description=Zentinel waf Agent"));
        assert!(service.contains("ExecStart=/usr/local/bin/zentinel-waf-agent"));
        assert!(service.contains("--config /etc/zentinel/agents/waf.yaml"));
        assert!(service.contains("User=zentinel"));
        assert!(service.contains("WantedBy=zentinel.target"));
        assert!(service.contains("After=zentinel.service"));
    }

    #[test]
    fn test_install_binary() {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("source");
        let dest_dir = temp.path().join("dest");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();

        // Create source binary
        let source_path = source_dir.join("test-binary");
        std::fs::write(&source_path, "binary content").unwrap();

        let result = install_binary(&source_path, &dest_dir, "test-binary");
        assert!(result.is_ok());

        let installed = result.unwrap();
        assert!(installed.exists());
        assert_eq!(installed.file_name().unwrap(), "test-binary");
    }

    #[test]
    fn test_uninstall_binary_exists() {
        let temp = tempfile::tempdir().unwrap();
        let binary_path = temp.path().join("test-binary");
        std::fs::write(&binary_path, "content").unwrap();

        let result = uninstall_binary(temp.path(), "test-binary");
        assert!(result.is_ok());
        assert!(result.unwrap()); // true = was removed
        assert!(!binary_path.exists());
    }

    #[test]
    fn test_uninstall_binary_not_exists() {
        let temp = tempfile::tempdir().unwrap();

        let result = uninstall_binary(temp.path(), "nonexistent");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // false = wasn't there
    }

    #[test]
    fn test_install_config_new() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path();

        let result = install_config(config_dir, "waf", "test: content", false);
        assert!(result.is_ok());

        let config_path = result.unwrap();
        assert!(config_path.exists());
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            "test: content"
        );
    }

    #[test]
    fn test_install_config_skip_existing() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path();

        // Create existing config
        let existing_path = config_dir.join("waf.yaml");
        std::fs::write(&existing_path, "original content").unwrap();

        // Try to install without force
        let result = install_config(config_dir, "waf", "new content", false);
        assert!(result.is_ok());

        // Should not have overwritten
        assert_eq!(
            std::fs::read_to_string(&existing_path).unwrap(),
            "original content"
        );
    }

    #[test]
    fn test_install_config_force_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path();

        // Create existing config
        let existing_path = config_dir.join("waf.yaml");
        std::fs::write(&existing_path, "original content").unwrap();

        // Install with force
        let result = install_config(config_dir, "waf", "new content", true);
        assert!(result.is_ok());

        // Should have overwritten
        assert_eq!(
            std::fs::read_to_string(&existing_path).unwrap(),
            "new content"
        );
    }

    #[test]
    fn test_install_paths_ensure_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let paths = InstallPaths {
            bin_dir: temp.path().join("bin"),
            config_dir: temp.path().join("config"),
            systemd_dir: Some(temp.path().join("systemd")),
            system_wide: false,
        };

        assert!(!paths.bin_dir.exists());
        assert!(!paths.config_dir.exists());

        let result = paths.ensure_dirs();
        assert!(result.is_ok());

        assert!(paths.bin_dir.exists());
        assert!(paths.config_dir.exists());
        assert!(paths.systemd_dir.as_ref().unwrap().exists());
    }

    #[test]
    fn test_install_error_display() {
        let err = InstallError::PermissionDenied("/root".to_string());
        assert!(err.to_string().contains("/root"));

        let err = InstallError::DirNotFound("/missing".to_string());
        assert!(err.to_string().contains("/missing"));
    }

    #[test]
    fn test_get_installed_version_not_exists() {
        let temp = tempfile::tempdir().unwrap();
        let result = get_installed_version(temp.path(), "nonexistent");
        assert!(result.is_none());
    }
}
