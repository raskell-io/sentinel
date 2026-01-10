# Sentinel Proxy - Upgrade and Migration Runbook

## Table of Contents
1. [Version Management](#version-management)
2. [Pre-Upgrade Checklist](#pre-upgrade-checklist)
3. [Upgrade Procedures](#upgrade-procedures)
4. [Migration Scenarios](#migration-scenarios)
5. [Rollback Procedures](#rollback-procedures)
6. [Configuration Migration](#configuration-migration)
7. [Post-Upgrade Validation](#post-upgrade-validation)

---

## Version Management

### Version Numbering

Sentinel follows semantic versioning: `MAJOR.MINOR.PATCH`

| Version Change | Meaning | Upgrade Approach |
|----------------|---------|------------------|
| Patch (x.y.Z) | Bug fixes, no breaking changes | Rolling upgrade, minimal risk |
| Minor (x.Y.z) | New features, backward compatible | Rolling upgrade, test new features |
| Major (X.y.z) | Breaking changes possible | Staged rollout, careful testing |

### Compatibility Matrix

| Component | Compatible Versions | Notes |
|-----------|---------------------|-------|
| Config format | Same major version | Config migration may be needed |
| Agent protocol | Same major version | Agents must be upgraded together |
| Metrics format | All versions | Metric names stable |
| Admin API | Same major version | API may have additions |

### Version Checking

```bash
# Check current version
sentinel --version

# Check version in running instance
curl -s localhost:9090/admin/version

# Compare with latest release
curl -s https://api.github.com/repos/raskell-io/sentinel/releases/latest | jq -r '.tag_name'
```

---

## Pre-Upgrade Checklist

### Before Any Upgrade

```
## Pre-Upgrade Checklist

### Documentation Review
[ ] Read release notes for all versions between current and target
[ ] Identify breaking changes
[ ] Review configuration migration notes
[ ] Check known issues

### Environment Preparation
[ ] Backup current binary
[ ] Backup current configuration
[ ] Backup current certificates
[ ] Document current metrics baseline
[ ] Notify stakeholders of maintenance window

### Testing
[ ] Test new version in staging/dev environment
[ ] Run integration tests with new version
[ ] Verify configuration compatibility
[ ] Test rollback procedure

### Operational Readiness
[ ] Confirm rollback procedure documented
[ ] Ensure on-call coverage during upgrade
[ ] Prepare monitoring dashboards
[ ] Set up alert notifications
```

### Backup Procedure

```bash
#!/bin/bash
# backup-sentinel.sh - Create full backup before upgrade

set -e

BACKUP_DIR="/var/backups/sentinel/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"

echo "Creating backup in $BACKUP_DIR..."

# Backup binary
cp /usr/local/bin/sentinel "$BACKUP_DIR/"

# Backup configuration
cp -r /etc/sentinel "$BACKUP_DIR/config"

# Backup systemd unit (if exists)
if [ -f /etc/systemd/system/sentinel.service ]; then
    cp /etc/systemd/system/sentinel.service "$BACKUP_DIR/"
fi

# Save current version
sentinel --version > "$BACKUP_DIR/version.txt"

# Save current metrics snapshot
curl -s localhost:9090/metrics > "$BACKUP_DIR/metrics.txt" 2>/dev/null || true

# Save current routes
curl -s localhost:9090/admin/routes > "$BACKUP_DIR/routes.json" 2>/dev/null || true

# Create manifest
cat > "$BACKUP_DIR/manifest.txt" << EOF
Backup created: $(date)
Sentinel version: $(cat "$BACKUP_DIR/version.txt")
Host: $(hostname)
Files included:
$(ls -la "$BACKUP_DIR/")
EOF

echo "Backup complete: $BACKUP_DIR"
echo "Manifest:"
cat "$BACKUP_DIR/manifest.txt"
```

---

## Upgrade Procedures

### Procedure: Patch Upgrade (x.y.Z)

**Risk Level**: Low
**Downtime**: Zero (with rolling upgrade)

```bash
#!/bin/bash
# patch-upgrade.sh - Upgrade to new patch version

set -e

NEW_VERSION="$1"
if [ -z "$NEW_VERSION" ]; then
    echo "Usage: $0 <version>"
    exit 1
fi

echo "=== Patch Upgrade to $NEW_VERSION ==="

# 1. Download new version
echo "Downloading sentinel $NEW_VERSION..."
curl -L -o /tmp/sentinel-new \
    "https://github.com/raskell-io/sentinel/releases/download/v${NEW_VERSION}/sentinel-linux-amd64"

chmod +x /tmp/sentinel-new

# 2. Verify download
echo "Verifying binary..."
/tmp/sentinel-new --version

# 3. Validate config with new version
echo "Validating configuration..."
/tmp/sentinel-new --validate --config /etc/sentinel/config.kdl

# 4. Create backup
./backup-sentinel.sh

# 5. Replace binary
echo "Replacing binary..."
mv /tmp/sentinel-new /usr/local/bin/sentinel

# 6. Graceful restart
echo "Restarting sentinel..."
systemctl restart sentinel

# 7. Verify
sleep 5
if curl -sf localhost:8080/health; then
    echo "Upgrade successful!"
    sentinel --version
else
    echo "Health check failed - initiating rollback"
    ./rollback-sentinel.sh
    exit 1
fi
```

### Procedure: Minor Upgrade (x.Y.z)

**Risk Level**: Medium
**Downtime**: Zero (with rolling upgrade)

```bash
#!/bin/bash
# minor-upgrade.sh - Upgrade to new minor version

set -e

NEW_VERSION="$1"
if [ -z "$NEW_VERSION" ]; then
    echo "Usage: $0 <version>"
    exit 1
fi

echo "=== Minor Upgrade to $NEW_VERSION ==="

# 1. Extended pre-flight checks
echo "Running pre-flight checks..."

# Check disk space
AVAILABLE_SPACE=$(df -P /usr/local/bin | tail -1 | awk '{print $4}')
if [ "$AVAILABLE_SPACE" -lt 100000 ]; then
    echo "ERROR: Insufficient disk space"
    exit 1
fi

# Check memory
FREE_MEM=$(free -m | awk '/^Mem:/{print $4}')
if [ "$FREE_MEM" -lt 512 ]; then
    echo "WARNING: Low memory available: ${FREE_MEM}MB"
fi

# 2. Download and verify
echo "Downloading sentinel $NEW_VERSION..."
curl -L -o /tmp/sentinel-new \
    "https://github.com/raskell-io/sentinel/releases/download/v${NEW_VERSION}/sentinel-linux-amd64"
curl -L -o /tmp/sentinel-new.sha256 \
    "https://github.com/raskell-io/sentinel/releases/download/v${NEW_VERSION}/sentinel-linux-amd64.sha256"

echo "Verifying checksum..."
cd /tmp && sha256sum -c sentinel-new.sha256

chmod +x /tmp/sentinel-new

# 3. Test new binary
echo "Testing new binary..."
/tmp/sentinel-new --version
/tmp/sentinel-new --validate --config /etc/sentinel/config.kdl

# 4. Create backup
./backup-sentinel.sh

# 5. Stop old version gracefully
echo "Initiating graceful shutdown..."
kill -TERM $(cat /var/run/sentinel.pid)

# Wait for connections to drain (max 30 seconds)
for i in {1..30}; do
    if ! pgrep -f "sentinel" > /dev/null; then
        break
    fi
    echo "Waiting for shutdown... ($i/30)"
    sleep 1
done

# 6. Replace and start
echo "Replacing binary and starting..."
mv /tmp/sentinel-new /usr/local/bin/sentinel
systemctl start sentinel

# 7. Extended verification
echo "Running verification..."
sleep 5

# Health check
if ! curl -sf localhost:8080/health; then
    echo "Health check failed!"
    ./rollback-sentinel.sh
    exit 1
fi

# Ready check
if ! curl -sf localhost:8080/ready; then
    echo "Ready check failed!"
    ./rollback-sentinel.sh
    exit 1
fi

# Version check
RUNNING_VERSION=$(curl -s localhost:9090/admin/version | jq -r '.version')
if [ "$RUNNING_VERSION" != "$NEW_VERSION" ]; then
    echo "Version mismatch: expected $NEW_VERSION, got $RUNNING_VERSION"
    ./rollback-sentinel.sh
    exit 1
fi

echo "=== Upgrade to $NEW_VERSION complete ==="
```

### Procedure: Major Upgrade (X.y.z)

**Risk Level**: High
**Approach**: Blue-Green or Canary

#### Blue-Green Upgrade

```
Phase 1: Deploy new version alongside old
┌─────────────────────────────────────────────────────────┐
│                    Load Balancer                        │
│                         │                               │
│      ┌──────────────────┴──────────────────┐           │
│      │                                      │           │
│      ▼ (100%)                               ▼ (0%)      │
│ ┌─────────────┐                      ┌─────────────┐   │
│ │ Blue (v1.x) │                      │ Green (v2.x)│   │
│ │  Active     │                      │  Standby    │   │
│ └─────────────┘                      └─────────────┘   │
└─────────────────────────────────────────────────────────┘

Phase 2: Shift traffic to new version
┌─────────────────────────────────────────────────────────┐
│                    Load Balancer                        │
│                         │                               │
│      ┌──────────────────┴──────────────────┐           │
│      │                                      │           │
│      ▼ (0%)                                 ▼ (100%)    │
│ ┌─────────────┐                      ┌─────────────┐   │
│ │ Blue (v1.x) │                      │ Green (v2.x)│   │
│ │  Standby    │                      │  Active     │   │
│ └─────────────┘                      └─────────────┘   │
└─────────────────────────────────────────────────────────┘
```

```bash
#!/bin/bash
# blue-green-upgrade.sh - Blue-green upgrade for major versions

set -e

NEW_VERSION="$1"
GREEN_HOST="green-sentinel.internal"
LB_API="http://lb.internal:8080/api"

echo "=== Blue-Green Upgrade to $NEW_VERSION ==="

# Phase 1: Deploy to green
echo "Phase 1: Deploying to green environment..."
ssh $GREEN_HOST "./deploy-sentinel.sh $NEW_VERSION"

# Verify green deployment
echo "Verifying green deployment..."
if ! ssh $GREEN_HOST "curl -sf localhost:8080/health"; then
    echo "Green deployment failed health check"
    exit 1
fi

# Phase 2: Test green with synthetic traffic
echo "Phase 2: Testing green with synthetic traffic..."
./run-synthetic-tests.sh $GREEN_HOST
if [ $? -ne 0 ]; then
    echo "Synthetic tests failed on green"
    exit 1
fi

# Phase 3: Shift 10% traffic to green
echo "Phase 3: Shifting 10% traffic to green..."
curl -X POST "$LB_API/backends/green/weight" -d '{"weight": 10}'
sleep 300  # Monitor for 5 minutes

# Check error rates
ERROR_RATE=$(./check-error-rate.sh green)
if [ $(echo "$ERROR_RATE > 0.01" | bc) -eq 1 ]; then
    echo "Error rate too high: $ERROR_RATE"
    curl -X POST "$LB_API/backends/green/weight" -d '{"weight": 0}'
    exit 1
fi

# Phase 4: Shift 50% traffic
echo "Phase 4: Shifting 50% traffic to green..."
curl -X POST "$LB_API/backends/green/weight" -d '{"weight": 50}'
curl -X POST "$LB_API/backends/blue/weight" -d '{"weight": 50}'
sleep 600  # Monitor for 10 minutes

# Phase 5: Complete shift
echo "Phase 5: Shifting 100% traffic to green..."
curl -X POST "$LB_API/backends/green/weight" -d '{"weight": 100}'
curl -X POST "$LB_API/backends/blue/weight" -d '{"weight": 0}'

echo "=== Blue-Green Upgrade complete ==="
echo "Blue environment retained for rollback"
echo "Run './cleanup-blue.sh' after validation period"
```

#### Canary Upgrade

```bash
#!/bin/bash
# canary-upgrade.sh - Canary upgrade for gradual rollout

set -e

NEW_VERSION="$1"
CANARY_PERCENT=5
VALIDATION_MINUTES=30

echo "=== Canary Upgrade to $NEW_VERSION ==="

# Deploy to canary instance
echo "Deploying canary..."
./deploy-to-canary.sh $NEW_VERSION

# Enable canary routing
echo "Enabling canary at ${CANARY_PERCENT}%..."
./set-canary-weight.sh $CANARY_PERCENT

# Monitor canary
echo "Monitoring canary for ${VALIDATION_MINUTES} minutes..."
./monitor-canary.sh $VALIDATION_MINUTES

if [ $? -ne 0 ]; then
    echo "Canary validation failed - rolling back"
    ./set-canary-weight.sh 0
    exit 1
fi

# Gradual rollout
for PERCENT in 10 25 50 75 100; do
    echo "Rolling out to ${PERCENT}%..."
    ./set-canary-weight.sh $PERCENT

    ./monitor-canary.sh 10  # 10 minute validation at each stage

    if [ $? -ne 0 ]; then
        echo "Rollout failed at ${PERCENT}% - rolling back"
        ./rollback-canary.sh
        exit 1
    fi
done

echo "=== Canary Upgrade to $NEW_VERSION complete ==="
```

---

## Migration Scenarios

### Scenario: Single Instance to HA Cluster

```bash
#!/bin/bash
# migrate-to-ha.sh - Migrate from single instance to HA cluster

echo "=== Migrating to HA Cluster ==="

# 1. Set up shared configuration
echo "Setting up shared configuration..."
# Copy config to shared storage (NFS, S3, etc.)
aws s3 cp /etc/sentinel/config.kdl s3://sentinel-config/config.kdl

# 2. Deploy additional instances
echo "Deploying additional instances..."
for i in {2..3}; do
    ./deploy-instance.sh sentinel-$i
done

# 3. Configure load balancer
echo "Configuring load balancer..."
cat > /tmp/lb-config.json << EOF
{
    "backend": "sentinel",
    "targets": [
        {"host": "sentinel-1", "port": 8080, "weight": 1},
        {"host": "sentinel-2", "port": 8080, "weight": 1},
        {"host": "sentinel-3", "port": 8080, "weight": 1}
    ],
    "health_check": {
        "path": "/health",
        "interval": "10s"
    }
}
EOF

curl -X POST http://lb.internal:8080/api/backends -d @/tmp/lb-config.json

# 4. Shift traffic through load balancer
echo "Shifting traffic to load balancer..."
./update-dns.sh sentinel.example.com lb.example.com

# 5. Verify
echo "Verifying HA setup..."
for host in sentinel-{1..3}; do
    curl -sf http://$host:8080/health || echo "WARNING: $host unhealthy"
done

echo "=== HA Migration complete ==="
```

### Scenario: Migrate Configuration Format

When major versions change configuration format:

```bash
#!/bin/bash
# migrate-config.sh - Migrate configuration between versions

set -e

OLD_CONFIG="$1"
NEW_CONFIG="$2"

if [ -z "$OLD_CONFIG" ] || [ -z "$NEW_CONFIG" ]; then
    echo "Usage: $0 <old-config> <new-config>"
    exit 1
fi

echo "Migrating configuration..."

# Use built-in migration tool (if available)
if sentinel config migrate --help &>/dev/null; then
    sentinel config migrate "$OLD_CONFIG" -o "$NEW_CONFIG"
else
    # Manual migration steps
    echo "Running manual migration..."

    # Example: Convert YAML to KDL (v1 -> v2)
    # ./yaml-to-kdl.py "$OLD_CONFIG" > "$NEW_CONFIG"

    # Example: Update deprecated directives
    sed -i 's/timeout-secs/timeouts { request-secs/g' "$NEW_CONFIG"
fi

# Validate migrated config
echo "Validating migrated configuration..."
sentinel --validate --config "$NEW_CONFIG"

echo "Migration complete: $NEW_CONFIG"
echo "Review the configuration before applying!"
```

### Scenario: Certificate Rotation During Upgrade

```bash
#!/bin/bash
# upgrade-with-certs.sh - Upgrade with new certificates

set -e

NEW_VERSION="$1"
NEW_CERT="$2"
NEW_KEY="$3"

echo "=== Upgrade with Certificate Rotation ==="

# 1. Validate new certificates
echo "Validating new certificates..."
openssl x509 -in "$NEW_CERT" -noout -checkend 86400 || {
    echo "Certificate expires within 24 hours!"
    exit 1
}

# 2. Deploy new certificates first (before upgrade)
echo "Deploying new certificates..."
cp "$NEW_CERT" /etc/sentinel/certs/server.crt.new
cp "$NEW_KEY" /etc/sentinel/certs/server.key.new
chmod 600 /etc/sentinel/certs/server.key.new

# 3. Perform upgrade
echo "Upgrading sentinel..."
./minor-upgrade.sh "$NEW_VERSION"

# 4. Switch to new certificates
echo "Activating new certificates..."
mv /etc/sentinel/certs/server.crt /etc/sentinel/certs/server.crt.old
mv /etc/sentinel/certs/server.key /etc/sentinel/certs/server.key.old
mv /etc/sentinel/certs/server.crt.new /etc/sentinel/certs/server.crt
mv /etc/sentinel/certs/server.key.new /etc/sentinel/certs/server.key

# 5. Reload to pick up new certificates
kill -HUP $(cat /var/run/sentinel.pid)

# 6. Verify
sleep 5
echo | openssl s_client -connect localhost:443 2>/dev/null | \
    openssl x509 -noout -subject -dates

echo "=== Upgrade and certificate rotation complete ==="
```

---

## Rollback Procedures

### Quick Rollback (< 5 minutes)

```bash
#!/bin/bash
# rollback-sentinel.sh - Quick rollback to previous version

set -e

# Find latest backup
BACKUP_DIR=$(ls -td /var/backups/sentinel/*/ 2>/dev/null | head -1)

if [ -z "$BACKUP_DIR" ]; then
    echo "ERROR: No backup found!"
    exit 1
fi

echo "=== Rolling back from $BACKUP_DIR ==="

# 1. Stop current version
echo "Stopping current version..."
systemctl stop sentinel || true

# 2. Restore binary
echo "Restoring binary..."
cp "$BACKUP_DIR/sentinel" /usr/local/bin/sentinel
chmod +x /usr/local/bin/sentinel

# 3. Restore configuration
echo "Restoring configuration..."
cp -r "$BACKUP_DIR/config/"* /etc/sentinel/

# 4. Start
echo "Starting sentinel..."
systemctl start sentinel

# 5. Verify
sleep 5
if curl -sf localhost:8080/health; then
    echo "Rollback successful!"
    sentinel --version
else
    echo "Rollback verification failed!"
    exit 1
fi
```

### Blue-Green Rollback

```bash
#!/bin/bash
# rollback-blue-green.sh - Rollback blue-green deployment

LB_API="http://lb.internal:8080/api"

echo "=== Blue-Green Rollback ==="

# Shift all traffic back to blue
echo "Shifting traffic to blue..."
curl -X POST "$LB_API/backends/blue/weight" -d '{"weight": 100}'
curl -X POST "$LB_API/backends/green/weight" -d '{"weight": 0}'

# Verify blue is healthy
echo "Verifying blue environment..."
if curl -sf http://blue-sentinel.internal:8080/health; then
    echo "Rollback complete - traffic on blue"
else
    echo "WARNING: Blue environment unhealthy!"
    exit 1
fi
```

### Canary Rollback

```bash
#!/bin/bash
# rollback-canary.sh - Rollback canary deployment

echo "=== Canary Rollback ==="

# Disable canary routing immediately
./set-canary-weight.sh 0

# Verify main deployment
if curl -sf localhost:8080/health; then
    echo "Canary disabled - main deployment active"
else
    echo "WARNING: Main deployment unhealthy!"
    exit 1
fi
```

---

## Configuration Migration

### Common Migration Patterns

#### Deprecated Directive Replacement

```kdl
// Old (v1.x)
upstream "backend" {
    timeout-secs 30  // DEPRECATED
}

// New (v2.x)
upstream "backend" {
    timeouts {
        connect-secs 5
        request-secs 30
        read-secs 30
    }
}
```

#### Restructured Configuration

```kdl
// Old (v1.x) - flat structure
route "api" {
    path-prefix "/api/"
    upstream "backend"
    rate-limit-rps 100
    timeout-secs 30
}

// New (v2.x) - nested structure
routes {
    route "api" {
        matches {
            path-prefix "/api/"
        }
        upstream "backend"
        policies {
            rate-limit {
                requests-per-second 100
            }
            timeouts {
                request-secs 30
            }
        }
    }
}
```

### Migration Tool Usage

```bash
# Check for configuration issues
sentinel config check /etc/sentinel/config.kdl

# Migrate configuration to new format
sentinel config migrate /etc/sentinel/config.kdl -o /etc/sentinel/config.kdl.new

# Show migration diff
diff /etc/sentinel/config.kdl /etc/sentinel/config.kdl.new

# Validate migrated configuration
sentinel --validate --config /etc/sentinel/config.kdl.new
```

---

## Post-Upgrade Validation

### Immediate Validation (First 5 Minutes)

```bash
#!/bin/bash
# validate-upgrade.sh - Post-upgrade validation

echo "=== Post-Upgrade Validation ==="

ERRORS=0

# 1. Process running
echo -n "Process running: "
if pgrep -f sentinel > /dev/null; then
    echo "OK"
else
    echo "FAIL"
    ((ERRORS++))
fi

# 2. Health endpoint
echo -n "Health endpoint: "
if curl -sf localhost:8080/health > /dev/null; then
    echo "OK"
else
    echo "FAIL"
    ((ERRORS++))
fi

# 3. Ready endpoint
echo -n "Ready endpoint: "
if curl -sf localhost:8080/ready > /dev/null; then
    echo "OK"
else
    echo "FAIL"
    ((ERRORS++))
fi

# 4. Metrics endpoint
echo -n "Metrics endpoint: "
if curl -sf localhost:9090/metrics > /dev/null; then
    echo "OK"
else
    echo "FAIL"
    ((ERRORS++))
fi

# 5. Version correct
echo -n "Version: "
VERSION=$(sentinel --version | grep -oP '\d+\.\d+\.\d+')
echo "$VERSION"

# 6. Configuration loaded
echo -n "Config loaded: "
ROUTES=$(curl -s localhost:9090/admin/routes | jq length)
if [ "$ROUTES" -gt 0 ]; then
    echo "OK ($ROUTES routes)"
else
    echo "FAIL"
    ((ERRORS++))
fi

# 7. Upstreams healthy
echo -n "Upstreams: "
UNHEALTHY=$(curl -s localhost:9090/metrics | grep 'upstream_health 0' | wc -l)
if [ "$UNHEALTHY" -eq 0 ]; then
    echo "OK"
else
    echo "WARNING: $UNHEALTHY unhealthy"
fi

echo ""
if [ $ERRORS -eq 0 ]; then
    echo "=== Validation PASSED ==="
else
    echo "=== Validation FAILED ($ERRORS errors) ==="
    exit 1
fi
```

### Extended Validation (First Hour)

```bash
#!/bin/bash
# extended-validation.sh - Extended post-upgrade monitoring

DURATION=3600  # 1 hour
INTERVAL=60    # Check every minute

echo "=== Extended Validation ($((DURATION/60)) minutes) ==="

START=$(date +%s)
ERRORS=0

while [ $(($(date +%s) - START)) -lt $DURATION ]; do
    # Check error rate
    ERROR_RATE=$(curl -s localhost:9090/metrics | \
        grep 'requests_total.*status="5' | \
        awk '{sum+=$2} END {print sum}')

    # Check latency
    P99_LATENCY=$(curl -s localhost:9090/metrics | \
        grep 'request_duration.*quantile="0.99"' | \
        awk '{print $2}')

    # Check memory
    MEMORY=$(curl -s localhost:9090/metrics | \
        grep 'process_resident_memory_bytes' | \
        awk '{print $2/1024/1024}')

    echo "$(date): errors=$ERROR_RATE p99=${P99_LATENCY}s memory=${MEMORY}MB"

    # Alert on anomalies
    if [ $(echo "$P99_LATENCY > 0.5" | bc) -eq 1 ]; then
        echo "WARNING: High latency detected!"
        ((ERRORS++))
    fi

    sleep $INTERVAL
done

if [ $ERRORS -gt 0 ]; then
    echo "=== Extended Validation completed with $ERRORS warnings ==="
else
    echo "=== Extended Validation PASSED ==="
fi
```

### Comparison Report

```bash
#!/bin/bash
# compare-versions.sh - Compare metrics before and after upgrade

BEFORE_METRICS="$1"
AFTER_METRICS="$2"

if [ -z "$BEFORE_METRICS" ] || [ -z "$AFTER_METRICS" ]; then
    echo "Usage: $0 <before-metrics> <after-metrics>"
    exit 1
fi

echo "=== Version Comparison Report ==="
echo ""

# Extract key metrics
extract_metric() {
    local file="$1"
    local pattern="$2"
    grep "$pattern" "$file" | head -1 | awk '{print $2}'
}

echo "| Metric | Before | After | Change |"
echo "|--------|--------|-------|--------|"

METRICS=(
    "process_resident_memory_bytes"
    "process_cpu_seconds_total"
    "sentinel_requests_total"
    "sentinel_open_connections"
)

for metric in "${METRICS[@]}"; do
    BEFORE=$(extract_metric "$BEFORE_METRICS" "$metric")
    AFTER=$(extract_metric "$AFTER_METRICS" "$metric")

    if [ -n "$BEFORE" ] && [ -n "$AFTER" ]; then
        CHANGE=$(echo "scale=2; (($AFTER - $BEFORE) / $BEFORE) * 100" | bc 2>/dev/null || echo "N/A")
        echo "| $metric | $BEFORE | $AFTER | ${CHANGE}% |"
    fi
done
```

---

## Quick Reference

### Upgrade Commands

```bash
# Validate config before upgrade
sentinel --validate --config /etc/sentinel/config.kdl

# Create backup
./backup-sentinel.sh

# Perform upgrade (choose appropriate script)
./patch-upgrade.sh 2.1.5      # Patch version
./minor-upgrade.sh 2.2.0      # Minor version
./blue-green-upgrade.sh 3.0.0 # Major version

# Rollback if needed
./rollback-sentinel.sh
```

### Health Check Endpoints

| Endpoint | Purpose | Expected Response |
|----------|---------|-------------------|
| `/health` | Liveness probe | 200 OK |
| `/ready` | Readiness probe | 200 OK |
| `/admin/version` | Version info | JSON with version |
| `/metrics` | Prometheus metrics | Metrics text |

### Upgrade Timing

| Upgrade Type | Downtime | Recommended Window |
|--------------|----------|-------------------|
| Patch | Zero | Anytime |
| Minor | Zero | Business hours |
| Major | Minutes (with blue-green) | Maintenance window |

---

**Always test upgrades in a non-production environment first.**
