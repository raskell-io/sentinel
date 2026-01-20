# Bundle Command

The `sentinel bundle` command manages the installation of bundled agents - a curated set of agents that are tested to work together with a specific version of Sentinel.

## Overview

Instead of manually downloading and configuring each agent, the bundle command:

1. Reads a version lock file that pins compatible agent versions
2. Downloads agents from their respective GitHub releases
3. Installs binaries to the appropriate locations
4. Optionally generates configuration and systemd service files

## Quick Start

```bash
# Install Sentinel first
curl -fsSL https://getsentinel.raskell.io | sh

# Install all bundled agents
sudo sentinel bundle install

# Check what's installed
sentinel bundle status

# Start everything
sudo systemctl start sentinel.target
```

## Commands

### `sentinel bundle install`

Downloads and installs bundled agents.

```bash
# Install all agents
sentinel bundle install

# Install a specific agent
sentinel bundle install waf

# Preview without installing
sentinel bundle install --dry-run

# Force reinstall
sentinel bundle install --force

# Include systemd services
sentinel bundle install --systemd

# Custom installation prefix
sentinel bundle install --prefix /opt/sentinel
```

**Options:**

| Option | Description |
|--------|-------------|
| `--dry-run, -n` | Preview what would be installed |
| `--force, -f` | Reinstall even if already up to date |
| `--systemd` | Also install systemd service files |
| `--prefix PATH` | Custom installation prefix |
| `--skip-verify` | Skip SHA256 checksum verification |

### `sentinel bundle status`

Shows the installation status of all bundled agents.

```bash
sentinel bundle status
```

Example output:

```
Sentinel Bundle Status
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Bundle version: 26.01_1
Install path:   /usr/local/bin

Agent           Installed    Expected     Status
─────────────────────────────────────────────────
denylist        0.2.0        0.2.0        ✓ up to date
ratelimit       0.2.0        0.2.0        ✓ up to date
waf             -            0.2.0        ✗ not installed

Total: 3 | Up to date: 2 | Outdated: 0 | Not installed: 1
```

### `sentinel bundle list`

Lists available agents in the bundle.

```bash
sentinel bundle list
sentinel bundle list --verbose  # Show download URLs
```

### `sentinel bundle uninstall`

Removes installed agents.

```bash
# Uninstall all agents
sentinel bundle uninstall

# Uninstall a specific agent
sentinel bundle uninstall waf

# Preview
sentinel bundle uninstall --dry-run
```

### `sentinel bundle update`

Checks for available updates.

```bash
# Check for updates
sentinel bundle update

# Show and apply updates
sentinel bundle update --apply
```

## Bundled Agents

The bundle includes agents that cover ~80% of production use cases:

| Agent | Purpose |
|-------|---------|
| **waf** | ModSecurity-based Web Application Firewall |
| **ratelimit** | Token bucket rate limiting |
| **denylist** | IP and path blocking |

## Installation Paths

**System-wide (requires root):**
- Binaries: `/usr/local/bin/sentinel-{agent}-agent`
- Configs: `/etc/sentinel/agents/{agent}.yaml`
- Systemd: `/etc/systemd/system/sentinel-{agent}.service`

**User-local:**
- Binaries: `~/.local/bin/sentinel-{agent}-agent`
- Configs: `~/.config/sentinel/agents/{agent}.yaml`
- Systemd: `~/.config/systemd/user/sentinel-{agent}.service`

The command automatically detects whether to use system-wide or user-local paths based on permissions.

## Version Lock File

Agent versions are coordinated via `bundle-versions.lock`:

```toml
[bundle]
version = "26.01_1"

[agents]
waf = "0.2.0"
ratelimit = "0.2.0"
denylist = "0.2.0"

[repositories]
waf = "raskell-io/sentinel-agent-waf"
ratelimit = "raskell-io/sentinel-agent-ratelimit"
denylist = "raskell-io/sentinel-agent-denylist"
```

The lock file is embedded in the Sentinel binary at build time, ensuring reproducible installations.

## Configuration

After installation, configure agents in your `sentinel.kdl`:

```kdl
agents {
    agent "waf" {
        endpoint "unix:///var/run/sentinel/waf.sock"
        timeout-ms 100
        failure-mode "open"
    }

    agent "ratelimit" {
        endpoint "unix:///var/run/sentinel/ratelimit.sock"
        timeout-ms 50
        failure-mode "open"
    }

    agent "denylist" {
        endpoint "unix:///var/run/sentinel/denylist.sock"
        timeout-ms 20
        failure-mode "open"
    }
}
```

Then reference them in routes:

```kdl
routes {
    route "api" {
        matches { path-prefix "/api" }
        upstream "backend"
        policies {
            agents "denylist" "ratelimit" "waf"
        }
    }
}
```

## Systemd Integration

With `--systemd`, the command installs service files and a target:

```bash
# Install with systemd
sudo sentinel bundle install --systemd

# Reload systemd
sudo systemctl daemon-reload

# Enable and start all services
sudo systemctl enable sentinel.target
sudo systemctl start sentinel.target

# Check status
sudo systemctl status sentinel.target
```

The `sentinel.target` starts the proxy and all agent services together.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    sentinel bundle                       │
│                                                         │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐ │
│  │   lock.rs   │───▶│  fetch.rs   │───▶│ install.rs  │ │
│  │ Parse lock  │    │  Download   │    │ Place files │ │
│  │   file      │    │  from GH    │    │ Set perms   │ │
│  └─────────────┘    └─────────────┘    └─────────────┘ │
│         │                  │                  │         │
│         ▼                  ▼                  ▼         │
│  bundle-versions     GitHub Releases    /usr/local/bin  │
│      .lock           (per agent)        /etc/sentinel   │
└─────────────────────────────────────────────────────────┘
```

## Troubleshooting

### Permission denied

Run with `sudo` for system-wide installation:

```bash
sudo sentinel bundle install
```

Or use user-local paths:

```bash
sentinel bundle install --prefix ~/.local
```

### Download failed

Check network connectivity and verify the agent release exists:

```bash
sentinel bundle list --verbose  # Shows download URLs
```

### Agent won't start

Check logs:

```bash
journalctl -u sentinel-waf -f
```

Verify socket permissions:

```bash
ls -la /var/run/sentinel/
```

### Version mismatch

Force reinstall:

```bash
sudo sentinel bundle install --force
```

## See Also

- [Agent Protocol](agents.md) - How agents communicate with the proxy
- [Configuration Reference](../../../config/docs/agents.md) - Agent configuration options
- [Deployment Guide](https://sentinel.raskell.io/docs/deployment/sentinel-stack) - Full stack deployment
