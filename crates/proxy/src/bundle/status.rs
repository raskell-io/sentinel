//! Bundle status checking
//!
//! Compares installed agent versions against the lock file.

use crate::bundle::install::{get_installed_version, InstallPaths};
use crate::bundle::lock::{AgentInfo, BundleLock};
use std::fmt;

/// Status of an individual agent
#[derive(Debug, Clone)]
pub struct AgentStatus {
    /// Agent name
    pub name: String,

    /// Expected version from lock file
    pub expected_version: String,

    /// Currently installed version (if any)
    pub installed_version: Option<String>,

    /// Status indicator
    pub status: Status,
}

/// Agent installation status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Installed and up to date
    UpToDate,

    /// Installed but different version
    Outdated,

    /// Not installed
    NotInstalled,

    /// Built into the proxy (echo agent)
    BuiltIn,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::UpToDate => write!(f, "up to date"),
            Status::Outdated => write!(f, "outdated"),
            Status::NotInstalled => write!(f, "not installed"),
            Status::BuiltIn => write!(f, "built-in"),
        }
    }
}

/// Overall bundle status
#[derive(Debug)]
pub struct BundleStatus {
    /// Bundle version from lock file
    pub bundle_version: String,

    /// Status of each agent
    pub agents: Vec<AgentStatus>,

    /// Installation paths being checked
    pub paths: InstallPaths,
}

impl BundleStatus {
    /// Check the status of all bundled agents
    pub fn check(lock: &BundleLock, paths: &InstallPaths) -> Self {
        let mut agents = Vec::new();

        for agent_info in lock.agents() {
            let status = check_agent_status(&agent_info, paths);
            agents.push(status);
        }

        // Sort by name for consistent output
        agents.sort_by(|a, b| a.name.cmp(&b.name));

        Self {
            bundle_version: lock.bundle.version.clone(),
            agents,
            paths: paths.clone(),
        }
    }

    /// Check if all agents are installed and up to date
    pub fn is_complete(&self) -> bool {
        self.agents
            .iter()
            .all(|a| a.status == Status::UpToDate || a.status == Status::BuiltIn)
    }

    /// Get agents that need to be installed or updated
    pub fn pending_agents(&self) -> Vec<&AgentStatus> {
        self.agents
            .iter()
            .filter(|a| a.status == Status::NotInstalled || a.status == Status::Outdated)
            .collect()
    }

    /// Get count of each status type
    pub fn summary(&self) -> StatusSummary {
        let mut summary = StatusSummary::default();
        for agent in &self.agents {
            match agent.status {
                Status::UpToDate => summary.up_to_date += 1,
                Status::Outdated => summary.outdated += 1,
                Status::NotInstalled => summary.not_installed += 1,
                Status::BuiltIn => summary.built_in += 1,
            }
        }
        summary.total = self.agents.len();
        summary
    }

    /// Format status for display
    pub fn display(&self) -> String {
        use std::fmt::Write;

        let mut output = String::new();

        writeln!(output, "Sentinel Bundle Status").unwrap();
        writeln!(output, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━").unwrap();
        writeln!(output, "Bundle version: {}", self.bundle_version).unwrap();
        writeln!(output, "Install path:   {}", self.paths.bin_dir.display()).unwrap();
        writeln!(output).unwrap();

        // Header
        writeln!(
            output,
            "{:<15} {:<12} {:<12} {}",
            "Agent", "Installed", "Expected", "Status"
        )
        .unwrap();
        writeln!(output, "{}", "─".repeat(55)).unwrap();

        // Agent rows
        for agent in &self.agents {
            let installed = agent
                .installed_version
                .as_deref()
                .unwrap_or("-");
            let status_icon = match agent.status {
                Status::UpToDate => "✓",
                Status::Outdated => "↑",
                Status::NotInstalled => "✗",
                Status::BuiltIn => "•",
            };

            writeln!(
                output,
                "{:<15} {:<12} {:<12} {} {}",
                agent.name, installed, agent.expected_version, status_icon, agent.status
            )
            .unwrap();
        }

        // Summary
        let summary = self.summary();
        writeln!(output).unwrap();
        writeln!(
            output,
            "Total: {} | Up to date: {} | Outdated: {} | Not installed: {}",
            summary.total,
            summary.up_to_date + summary.built_in,
            summary.outdated,
            summary.not_installed
        )
        .unwrap();

        output
    }
}

/// Summary counts
#[derive(Debug, Default)]
pub struct StatusSummary {
    pub total: usize,
    pub up_to_date: usize,
    pub outdated: usize,
    pub not_installed: usize,
    pub built_in: usize,
}

/// Check the status of a single agent
fn check_agent_status(agent: &AgentInfo, paths: &InstallPaths) -> AgentStatus {
    let installed_version = get_installed_version(&paths.bin_dir, &agent.binary_name);

    let status = match &installed_version {
        Some(v) if v == &agent.version => Status::UpToDate,
        Some(_) => Status::Outdated,
        None => Status::NotInstalled,
    };

    AgentStatus {
        name: agent.name.clone(),
        expected_version: agent.version.clone(),
        installed_version,
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_display() {
        let status = AgentStatus {
            name: "waf".to_string(),
            expected_version: "0.2.0".to_string(),
            installed_version: Some("0.2.0".to_string()),
            status: Status::UpToDate,
        };

        assert_eq!(format!("{}", status.status), "up to date");
    }

    #[test]
    fn test_bundle_status_summary() {
        let status = BundleStatus {
            bundle_version: "26.01_1".to_string(),
            agents: vec![
                AgentStatus {
                    name: "waf".to_string(),
                    expected_version: "0.2.0".to_string(),
                    installed_version: Some("0.2.0".to_string()),
                    status: Status::UpToDate,
                },
                AgentStatus {
                    name: "ratelimit".to_string(),
                    expected_version: "0.2.0".to_string(),
                    installed_version: None,
                    status: Status::NotInstalled,
                },
            ],
            paths: InstallPaths::user(),
        };

        let summary = status.summary();
        assert_eq!(summary.total, 2);
        assert_eq!(summary.up_to_date, 1);
        assert_eq!(summary.not_installed, 1);
    }
}
