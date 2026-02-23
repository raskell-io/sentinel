//! Zentinel API Generator
//!
//! Reads `agents/registry/*.toml` files and generates:
//! - Static JSON API files for `api.zentinelproxy.io`
//! - `bundle-versions.lock` for backward compatibility

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "api-gen", about = "Generate Zentinel API JSON from registry TOML files")]
struct Cli {
    /// Path to agents/registry/ directory
    #[arg(long, default_value = "agents/registry")]
    registry: PathBuf,

    /// Output directory for JSON API files
    #[arg(long, default_value = "api-output")]
    output: PathBuf,

    /// Also regenerate bundle-versions.lock at this path
    #[arg(long)]
    lock_file: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Registry TOML schema (input)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BundleToml {
    bundle: BundleMeta,
}

#[derive(Debug, Deserialize)]
struct BundleMeta {
    version: String,
}

#[derive(Debug, Deserialize)]
struct AgentToml {
    agent: AgentDef,
    metadata: AgentMetadata,
    bundle: Option<AgentBundleConfig>,
}

#[derive(Debug, Deserialize)]
struct AgentDef {
    name: String,
    version: String,
    repository: String,
    binary_name: String,
}

#[derive(Debug, Deserialize)]
struct AgentMetadata {
    description: String,
    author: String,
    license: String,
    status: String,
    category: String,
    tags: Vec<String>,
    protocol_version: String,
    min_zentinel_version: String,
}

#[derive(Debug, Deserialize)]
struct AgentBundleConfig {
    included: bool,
    group: String,
}

// ---------------------------------------------------------------------------
// JSON API schema (output)
// ---------------------------------------------------------------------------

const PLATFORMS: &[(&str, &str)] = &[
    ("linux-x86_64", "linux"),
    ("linux-aarch64", "linux"),
    ("darwin-x86_64", "darwin"),
    ("darwin-aarch64", "darwin"),
];

fn platform_arch(platform: &str) -> &str {
    if platform.contains("x86_64") {
        "x86_64"
    } else {
        "aarch64"
    }
}

#[derive(Serialize)]
struct ApiBundleJson {
    schema_version: u32,
    bundle: ApiBundleInfo,
    agents: BTreeMap<String, ApiBundleAgent>,
}

#[derive(Serialize)]
struct ApiBundleInfo {
    version: String,
    generated_at: String,
}

#[derive(Serialize)]
struct ApiBundleAgent {
    version: String,
    repository: String,
    binary_name: String,
    download_urls: BTreeMap<String, String>,
    checksums: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct ApiAgentsJson {
    schema_version: u32,
    agents: Vec<ApiAgentSummary>,
}

#[derive(Serialize)]
struct ApiAgentSummary {
    name: String,
    version: String,
    description: String,
    author: String,
    license: String,
    status: String,
    category: String,
    tags: Vec<String>,
    protocol_version: String,
    homepage: String,
    repository: String,
}

#[derive(Serialize)]
struct ApiAgentDetailJson {
    schema_version: u32,
    agent: ApiAgentDetail,
}

#[derive(Serialize)]
struct ApiAgentDetail {
    name: String,
    version: String,
    description: String,
    author: String,
    license: String,
    status: String,
    category: String,
    tags: Vec<String>,
    protocol_version: String,
    min_zentinel_version: String,
    homepage: String,
    repository: String,
    binary_name: String,
    download_urls: BTreeMap<String, String>,
    checksums: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct ApiHealthJson {
    status: String,
    bundle_version: String,
    agent_count: usize,
    api_version: String,
    generated_at: String,
}

// ---------------------------------------------------------------------------
// Logic
// ---------------------------------------------------------------------------

fn download_url(repo: &str, version: &str, binary_name: &str, os: &str, arch: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/v{}/{}-{}-{}-{}.tar.gz",
        repo, version, binary_name, version, os, arch
    )
}

fn load_registry(registry_dir: &Path) -> Result<(BundleMeta, Vec<AgentToml>)> {
    // Load bundle.toml
    let bundle_path = registry_dir.join("bundle.toml");
    let bundle_content = std::fs::read_to_string(&bundle_path)
        .with_context(|| format!("Failed to read {}", bundle_path.display()))?;
    let bundle: BundleToml = toml::from_str(&bundle_content)
        .with_context(|| format!("Failed to parse {}", bundle_path.display()))?;

    // Load all agent TOML files
    let mut agents = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(registry_dir)
        .with_context(|| format!("Failed to read directory {}", registry_dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml")
            && path.file_name().is_some_and(|name| name != "bundle.toml")
        {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let agent: AgentToml = toml::from_str(&content)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            agents.push(agent);
        }
    }

    Ok((bundle.bundle, agents))
}

fn generate_bundle_json(
    bundle: &BundleMeta,
    agents: &[AgentToml],
    generated_at: &str,
) -> ApiBundleJson {
    let mut api_agents = BTreeMap::new();

    for agent in agents {
        let is_bundled = agent
            .bundle
            .as_ref()
            .is_some_and(|b| b.included);

        if !is_bundled {
            continue;
        }

        let mut urls = BTreeMap::new();
        let checksums = BTreeMap::new();

        for &(platform, os) in PLATFORMS {
            let arch = platform_arch(platform);
            urls.insert(
                platform.to_string(),
                download_url(
                    &agent.agent.repository,
                    &agent.agent.version,
                    &agent.agent.binary_name,
                    os,
                    arch,
                ),
            );
        }

        api_agents.insert(
            agent.agent.name.clone(),
            ApiBundleAgent {
                version: agent.agent.version.clone(),
                repository: agent.agent.repository.clone(),
                binary_name: agent.agent.binary_name.clone(),
                download_urls: urls,
                checksums,
            },
        );
    }

    ApiBundleJson {
        schema_version: 1,
        bundle: ApiBundleInfo {
            version: bundle.version.clone(),
            generated_at: generated_at.to_string(),
        },
        agents: api_agents,
    }
}

fn generate_agents_json(agents: &[AgentToml]) -> ApiAgentsJson {
    let summaries = agents
        .iter()
        .map(|a| ApiAgentSummary {
            name: a.agent.name.clone(),
            version: a.agent.version.clone(),
            description: a.metadata.description.clone(),
            author: a.metadata.author.clone(),
            license: a.metadata.license.clone(),
            status: a.metadata.status.clone(),
            category: a.metadata.category.clone(),
            tags: a.metadata.tags.clone(),
            protocol_version: a.metadata.protocol_version.clone(),
            homepage: format!("https://registry.zentinelproxy.io/{}/", a.agent.name),
            repository: a.agent.repository.clone(),
        })
        .collect();

    ApiAgentsJson {
        schema_version: 1,
        agents: summaries,
    }
}

fn generate_agent_detail_json(agent: &AgentToml) -> ApiAgentDetailJson {
    let mut urls = BTreeMap::new();
    let checksums = BTreeMap::new();

    for &(platform, os) in PLATFORMS {
        let arch = platform_arch(platform);
        urls.insert(
            platform.to_string(),
            download_url(
                &agent.agent.repository,
                &agent.agent.version,
                &agent.agent.binary_name,
                os,
                arch,
            ),
        );
    }

    ApiAgentDetailJson {
        schema_version: 1,
        agent: ApiAgentDetail {
            name: agent.agent.name.clone(),
            version: agent.agent.version.clone(),
            description: agent.metadata.description.clone(),
            author: agent.metadata.author.clone(),
            license: agent.metadata.license.clone(),
            status: agent.metadata.status.clone(),
            category: agent.metadata.category.clone(),
            tags: agent.metadata.tags.clone(),
            protocol_version: agent.metadata.protocol_version.clone(),
            min_zentinel_version: agent.metadata.min_zentinel_version.clone(),
            homepage: format!("https://registry.zentinelproxy.io/{}/", agent.agent.name),
            repository: agent.agent.repository.clone(),
            binary_name: agent.agent.binary_name.clone(),
            download_urls: urls,
            checksums,
        },
    }
}

fn generate_health_json(
    bundle: &BundleMeta,
    agent_count: usize,
    generated_at: &str,
) -> ApiHealthJson {
    ApiHealthJson {
        status: "ok".to_string(),
        bundle_version: bundle.version.clone(),
        agent_count,
        api_version: "v1".to_string(),
        generated_at: generated_at.to_string(),
    }
}

fn generate_lock_file(bundle: &BundleMeta, agents: &[AgentToml]) -> String {
    let mut out = String::new();
    out.push_str("# Zentinel Distribution Bundle - Version Lock File\n");
    out.push_str("#\n");
    out.push_str("# AUTO-GENERATED from agents/registry/*.toml by tools/api-gen\n");
    out.push_str("# Do not edit manually. Update the registry TOML files instead.\n");
    out.push_str("#\n");
    out.push_str("# Format: CalVer for bundle, SemVer for agents\n");
    out.push('\n');
    out.push_str("[bundle]\n");
    out.push_str(&format!("version = \"{}\"\n", bundle.version));
    out.push('\n');

    // Group agents by bundle group
    let mut groups: BTreeMap<String, Vec<&AgentToml>> = BTreeMap::new();
    for agent in agents {
        if let Some(ref bc) = agent.bundle {
            if bc.included {
                groups.entry(bc.group.clone()).or_default().push(agent);
            }
        }
    }

    // [agents] section
    out.push_str("[agents]\n");
    for (group, group_agents) in &groups {
        out.push_str(&format!("# {}\n", group));
        for agent in group_agents {
            out.push_str(&format!("{} = \"{}\"\n", agent.agent.name, agent.agent.version));
        }
        out.push('\n');
    }

    // [repositories] section
    out.push_str("[repositories]\n");
    for (group, group_agents) in &groups {
        out.push_str(&format!("# {}\n", group));
        for agent in group_agents {
            out.push_str(&format!(
                "{} = \"{}\"\n",
                agent.agent.name, agent.agent.repository
            ));
        }
        out.push('\n');
    }

    // [checksums] section
    out.push_str("[checksums]\n");
    out.push_str("# SHA256 checksums for verification (populated by release CI)\n");

    out
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, json).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let (bundle, agents) = load_registry(&cli.registry)?;
    let generated_at = Utc::now().to_rfc3339();
    let agent_count = agents.len();

    println!("Loaded {} agents from registry", agent_count);
    println!("Bundle version: {}", bundle.version);

    // Create output directory
    let v1 = cli.output.join("v1");
    std::fs::create_dir_all(&v1)?;

    // Generate v1/bundle.json
    let bundle_json = generate_bundle_json(&bundle, &agents, &generated_at);
    write_json(&v1.join("bundle.json"), &bundle_json)?;
    println!("  Generated v1/bundle.json ({} bundled agents)", bundle_json.agents.len());

    // Generate v1/agents.json
    let agents_json = generate_agents_json(&agents);
    write_json(&v1.join("agents.json"), &agents_json)?;
    println!("  Generated v1/agents.json");

    // Generate v1/agents/{name}.json
    let agents_dir = v1.join("agents");
    std::fs::create_dir_all(&agents_dir)?;
    for agent in &agents {
        let detail = generate_agent_detail_json(agent);
        write_json(
            &agents_dir.join(format!("{}.json", agent.agent.name)),
            &detail,
        )?;
    }
    println!("  Generated v1/agents/*.json ({} files)", agent_count);

    // Generate v1/health.json
    let health = generate_health_json(&bundle, agent_count, &generated_at);
    write_json(&v1.join("health.json"), &health)?;
    println!("  Generated v1/health.json");

    // Regenerate bundle-versions.lock if requested
    if let Some(lock_path) = &cli.lock_file {
        let lock_content = generate_lock_file(&bundle, &agents);
        std::fs::write(lock_path, &lock_content)
            .with_context(|| format!("Failed to write {}", lock_path.display()))?;
        println!("  Regenerated {}", lock_path.display());
    }

    println!("Done.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_bundle_meta() -> BundleMeta {
        BundleMeta {
            version: "26.02_13".to_string(),
        }
    }

    fn test_agent() -> AgentToml {
        AgentToml {
            agent: AgentDef {
                name: "waf".to_string(),
                version: "0.3.0".to_string(),
                repository: "zentinelproxy/zentinel-agent-waf".to_string(),
                binary_name: "zentinel-waf-agent".to_string(),
            },
            metadata: AgentMetadata {
                description: "Test WAF agent".to_string(),
                author: "Test".to_string(),
                license: "Apache-2.0".to_string(),
                status: "stable".to_string(),
                category: "security".to_string(),
                tags: vec!["security".to_string(), "waf".to_string()],
                protocol_version: "v2".to_string(),
                min_zentinel_version: "26.01.0".to_string(),
            },
            bundle: Some(AgentBundleConfig {
                included: true,
                group: "Core agents".to_string(),
            }),
        }
    }

    #[test]
    fn test_download_url_format() {
        let url = download_url(
            "zentinelproxy/zentinel-agent-waf",
            "0.3.0",
            "zentinel-waf-agent",
            "linux",
            "x86_64",
        );
        assert_eq!(
            url,
            "https://github.com/zentinelproxy/zentinel-agent-waf/releases/download/v0.3.0/zentinel-waf-agent-0.3.0-linux-x86_64.tar.gz"
        );
    }

    #[test]
    fn test_generate_bundle_json() {
        let bundle = test_bundle_meta();
        let agents = vec![test_agent()];
        let result = generate_bundle_json(&bundle, &agents, "2026-02-23T00:00:00Z");

        assert_eq!(result.schema_version, 1);
        assert_eq!(result.bundle.version, "26.02_13");
        assert!(result.agents.contains_key("waf"));

        let waf = &result.agents["waf"];
        assert_eq!(waf.version, "0.3.0");
        assert_eq!(waf.download_urls.len(), 4);
        assert!(waf.download_urls.contains_key("linux-x86_64"));
    }

    #[test]
    fn test_generate_agents_json() {
        let agents = vec![test_agent()];
        let result = generate_agents_json(&agents);

        assert_eq!(result.schema_version, 1);
        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].name, "waf");
        assert_eq!(result.agents[0].homepage, "https://registry.zentinelproxy.io/waf/");
    }

    #[test]
    fn test_generate_health_json() {
        let bundle = test_bundle_meta();
        let result = generate_health_json(&bundle, 22, "2026-02-23T00:00:00Z");

        assert_eq!(result.status, "ok");
        assert_eq!(result.agent_count, 22);
        assert_eq!(result.api_version, "v1");
    }

    #[test]
    fn test_generate_lock_file() {
        let bundle = test_bundle_meta();
        let agents = vec![test_agent()];
        let lock = generate_lock_file(&bundle, &agents);

        assert!(lock.contains("[bundle]"));
        assert!(lock.contains("version = \"26.02_13\""));
        assert!(lock.contains("[agents]"));
        assert!(lock.contains("waf = \"0.3.0\""));
        assert!(lock.contains("[repositories]"));
        assert!(lock.contains("waf = \"zentinelproxy/zentinel-agent-waf\""));
        assert!(lock.contains("AUTO-GENERATED"));
    }

    #[test]
    fn test_load_registry() {
        let temp = tempfile::tempdir().unwrap();

        // Write bundle.toml
        let mut f = std::fs::File::create(temp.path().join("bundle.toml")).unwrap();
        writeln!(f, "[bundle]\nversion = \"26.02_13\"").unwrap();

        // Write agent TOML
        let agent_toml = r#"
[agent]
name = "waf"
version = "0.3.0"
repository = "zentinelproxy/zentinel-agent-waf"
binary_name = "zentinel-waf-agent"

[metadata]
description = "Test"
author = "Test"
license = "Apache-2.0"
status = "stable"
category = "security"
tags = ["security"]
protocol_version = "v2"
min_zentinel_version = "26.01.0"

[bundle]
included = true
group = "Core agents"
"#;
        std::fs::write(temp.path().join("waf.toml"), agent_toml).unwrap();

        let (bundle, agents) = load_registry(temp.path()).unwrap();
        assert_eq!(bundle.version, "26.02_13");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent.name, "waf");
    }

    #[test]
    fn test_unbundled_agent_excluded_from_bundle_json() {
        let bundle = test_bundle_meta();
        let mut agent = test_agent();
        agent.bundle = Some(AgentBundleConfig {
            included: false,
            group: "Other".to_string(),
        });

        let result = generate_bundle_json(&bundle, &[agent], "2026-02-23T00:00:00Z");
        assert!(result.agents.is_empty());
    }

    #[test]
    fn test_agent_detail_json() {
        let agent = test_agent();
        let detail = generate_agent_detail_json(&agent);

        assert_eq!(detail.schema_version, 1);
        assert_eq!(detail.agent.name, "waf");
        assert_eq!(detail.agent.min_zentinel_version, "26.01.0");
        assert_eq!(detail.agent.download_urls.len(), 4);
    }
}
