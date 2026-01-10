# Sentinel Proxy - Incident Response Runbook

## Table of Contents
1. [Incident Classification](#incident-classification)
2. [Initial Response](#initial-response)
3. [Incident Procedures](#incident-procedures)
4. [Post-Incident](#post-incident)
5. [Communication Templates](#communication-templates)

---

## Incident Classification

### Severity Levels

| Severity | Description | Response Time | Examples |
|----------|-------------|---------------|----------|
| **SEV1** | Complete outage, all traffic affected | Immediate (< 5 min) | Proxy down, all upstreams unreachable |
| **SEV2** | Partial outage, significant traffic affected | < 15 min | Multiple routes failing, > 10% error rate |
| **SEV3** | Degraded performance, limited impact | < 1 hour | Elevated latency, single upstream unhealthy |
| **SEV4** | Minor issue, minimal user impact | < 4 hours | Non-critical feature degraded |

### Escalation Matrix

| Severity | Primary | Secondary | Management |
|----------|---------|-----------|------------|
| SEV1 | On-call engineer | Team lead | Director (if > 30 min) |
| SEV2 | On-call engineer | Team lead | - |
| SEV3 | On-call engineer | - | - |
| SEV4 | Next business day | - | - |

---

## Initial Response

### First 5 Minutes Checklist

```
[ ] Acknowledge incident in alerting system
[ ] Assess severity using classification above
[ ] Open incident channel (Slack: #incident-YYYYMMDD-NNN)
[ ] Declare incident commander if SEV1/SEV2
[ ] Begin gathering initial diagnostics
```

### Quick Diagnostic Commands

```bash
# Check proxy health
curl -sf http://localhost:8080/health && echo "OK" || echo "UNHEALTHY"

# Check ready status
curl -sf http://localhost:8080/ready && echo "READY" || echo "NOT READY"

# Get current error rate (last 5 min)
curl -s localhost:9090/metrics | grep 'requests_total' | awk '/5[0-9][0-9]/ {sum+=$2} END {print "5xx errors:", sum}'

# Check process status
systemctl status sentinel

# Check recent logs for errors
journalctl -u sentinel --since "5 minutes ago" | grep -i error | tail -20

# Check upstream health
curl -s localhost:9090/metrics | grep upstream_health

# Check circuit breakers
curl -s localhost:9090/metrics | grep circuit_breaker_state
```

### Initial Triage Decision Tree

```
Is the proxy process running?
├─ NO → Go to: Process Crash Procedure
└─ YES
    └─ Is the health endpoint responding?
        ├─ NO → Go to: Health Check Failure Procedure
        └─ YES
            └─ Are all upstreams healthy?
                ├─ NO → Go to: Upstream Failure Procedure
                └─ YES
                    └─ Is error rate elevated?
                        ├─ YES → Go to: High Error Rate Procedure
                        └─ NO → Go to: Performance Degradation Procedure
```

---

## Incident Procedures

### Procedure: Process Crash

**Symptoms**: Sentinel process not running, connections refused

**Immediate Actions**:
```bash
# 1. Attempt restart
systemctl restart sentinel

# 2. Check if it stays up
sleep 5 && systemctl status sentinel

# 3. If still failing, check logs for crash reason
journalctl -u sentinel --since "10 minutes ago" | tail -100

# 4. Check for resource exhaustion
dmesg | grep -i "oom\|killed" | tail -10
free -h
df -h /var /tmp
```

**Common Causes & Fixes**:

| Cause | Diagnostic | Fix |
|-------|------------|-----|
| OOM killed | `dmesg | grep oom` shows sentinel | Increase memory limits, check for leaks |
| Config error | Logs show parse/validation error | Restore previous config, fix and reload |
| Disk full | `df -h` shows 100% | Clear logs, increase disk |
| Port conflict | Logs show "address in use" | Kill conflicting process |
| Certificate expired | TLS handshake errors | Renew certificates |

**Rollback**:
```bash
# Restore last known good config
cp /etc/sentinel/config.kdl.backup /etc/sentinel/config.kdl
systemctl restart sentinel
```

---

### Procedure: Upstream Failure

**Symptoms**: Specific routes returning 502/503, upstream health metrics showing 0

**Immediate Actions**:
```bash
# 1. Identify unhealthy upstreams
curl -s localhost:9090/metrics | grep 'upstream_health{' | grep ' 0'

# 2. Check upstream connectivity from proxy host
for target in $(grep -oP 'address "\K[^"]+' /etc/sentinel/config.kdl); do
    echo -n "$target: "
    nc -zv -w 3 ${target%:*} ${target#*:} 2>&1 | grep -o "succeeded\|failed"
done

# 3. Check DNS resolution
dig +short backend.service.internal

# 4. Check network path
traceroute -n <upstream_ip>
```

**Mitigation Options**:

1. **Remove unhealthy targets temporarily**:
```bash
# Edit config to remove/comment unhealthy target
vim /etc/sentinel/config.kdl
kill -HUP $(cat /var/run/sentinel.pid)
```

2. **Adjust health check thresholds**:
```kdl
health-check {
    unhealthy-threshold 5  // Increase tolerance
    timeout-secs 10        // Increase timeout
}
```

3. **Enable failover upstream**:
```kdl
route "api" {
    upstream "primary-backend"
    fallback-upstream "backup-backend"
}
```

---

### Procedure: High Error Rate

**Symptoms**: > 1% 5xx error rate, elevated latency

**Immediate Actions**:
```bash
# 1. Identify error distribution by route
curl -s localhost:9090/metrics | grep 'requests_total.*status="5' | sort -t'"' -k4

# 2. Check for specific error types
journalctl -u sentinel --since "5 minutes ago" | grep -oP 'error[^,]*' | sort | uniq -c | sort -rn | head

# 3. Sample recent errors
journalctl -u sentinel --since "5 minutes ago" -o json | jq -r 'select(.PRIORITY == "3") | .MESSAGE' | head -20

# 4. Check upstream latency
curl -s localhost:9090/metrics | grep 'upstream_request_duration.*quantile="0.99"'
```

**Error Type Decision Tree**:

```
Error Type?
├─ 502 Bad Gateway
│   └─ Upstream returning invalid response
│       → Check upstream application logs
│       → Verify upstream is returning valid HTTP
│
├─ 503 Service Unavailable
│   └─ All targets unhealthy or circuit breaker open
│       → Follow Upstream Failure procedure
│       → Check circuit breaker status
│
├─ 504 Gateway Timeout
│   └─ Upstream not responding in time
│       → Increase timeout temporarily
│       → Check upstream performance
│
└─ 500 Internal Server Error
    └─ Proxy internal error
        → Check proxy logs for stack traces
        → Restart proxy if persistent
```

**Temporary Mitigations**:
```bash
# Increase timeouts
# Edit config:
timeouts {
    request-secs 60    // Increase from 30
    read-secs 30       // Increase from 15
}

# Reduce load (if rate limiting enabled)
# Edit config:
rate-limit {
    requests-per-second 50  // Reduce to ease pressure
}

# Apply
kill -HUP $(cat /var/run/sentinel.pid)
```

---

### Procedure: Memory Exhaustion

**Symptoms**: High memory usage, slow responses, potential OOM

**Immediate Actions**:
```bash
# 1. Check current memory usage
curl -s localhost:9090/metrics | grep process_resident_memory_bytes

# 2. Check connection count
curl -s localhost:9090/metrics | grep open_connections

# 3. Check request queue depth
curl -s localhost:9090/metrics | grep pending_requests

# 4. Get memory breakdown
pmap -x $(pidof sentinel) | tail -5
```

**Mitigation Steps**:
```bash
# 1. Reduce connection limits immediately
cat >> /tmp/emergency-limits.kdl << 'EOF'
limits {
    max-connections 5000
    max-connections-per-client 50
}
EOF
cat /etc/sentinel/config.kdl >> /tmp/emergency-limits.kdl
mv /tmp/emergency-limits.kdl /etc/sentinel/config.kdl
kill -HUP $(cat /var/run/sentinel.pid)

# 2. If still critical, perform rolling restart
systemctl restart sentinel
```

---

### Procedure: TLS/Certificate Issues

**Symptoms**: TLS handshake failures, certificate errors in logs

**Immediate Actions**:
```bash
# 1. Check certificate expiration
openssl x509 -in /etc/sentinel/certs/server.crt -noout -dates

# 2. Verify certificate chain
openssl verify -CAfile /etc/sentinel/certs/ca.crt /etc/sentinel/certs/server.crt

# 3. Check certificate matches key
diff <(openssl x509 -in /etc/sentinel/certs/server.crt -noout -modulus) \
     <(openssl rsa -in /etc/sentinel/certs/server.key -noout -modulus)

# 4. Test TLS connection
openssl s_client -connect localhost:443 -servername your.domain.com </dev/null

# 5. Check client cert for mTLS upstreams
openssl x509 -in /etc/sentinel/certs/client.crt -noout -dates
```

**Certificate Renewal**:
```bash
# 1. Deploy new certificate
cp /path/to/new/cert.crt /etc/sentinel/certs/server.crt
cp /path/to/new/key.key /etc/sentinel/certs/server.key
chmod 600 /etc/sentinel/certs/server.key

# 2. Verify before reload
openssl x509 -in /etc/sentinel/certs/server.crt -noout -dates

# 3. Reload (zero-downtime)
kill -HUP $(cat /var/run/sentinel.pid)

# 4. Verify new cert is active
echo | openssl s_client -connect localhost:443 2>/dev/null | openssl x509 -noout -dates
```

---

### Procedure: Configuration Reload Failure

**Symptoms**: Config changes not taking effect, reload errors in logs

**Immediate Actions**:
```bash
# 1. Check reload metrics
curl -s localhost:9090/metrics | grep config_reload

# 2. View recent reload attempts
journalctl -u sentinel --since "10 minutes ago" | grep -i "reload\|config"

# 3. Validate current config file
./sentinel --validate --config /etc/sentinel/config.kdl

# 4. Check file permissions
ls -la /etc/sentinel/config.kdl
```

**Fix and Retry**:
```bash
# 1. Fix validation errors
vim /etc/sentinel/config.kdl

# 2. Validate
./sentinel --validate --config /etc/sentinel/config.kdl

# 3. Retry reload
kill -HUP $(cat /var/run/sentinel.pid)

# 4. Verify success
journalctl -u sentinel --since "1 minute ago" | grep "Configuration reload"
```

---

### Procedure: DDoS/Attack Response

**Symptoms**: Massive traffic spike, resource exhaustion, attack patterns in logs

**Immediate Actions**:
```bash
# 1. Check request rate
curl -s localhost:9090/metrics | grep requests_total

# 2. Identify top client IPs
journalctl -u sentinel --since "5 minutes ago" -o json | \
    jq -r '.client_ip' | sort | uniq -c | sort -rn | head -20

# 3. Check for attack patterns
journalctl -u sentinel --since "5 minutes ago" | \
    grep -oP 'path="[^"]*"' | sort | uniq -c | sort -rn | head -20
```

**Mitigation Steps**:

1. **Enable aggressive rate limiting**:
```kdl
policies {
    rate-limit {
        requests-per-second 5
        key "client_ip"
        action "block"
    }
}
```

2. **Block specific IPs via firewall**:
```bash
# Block attacking IPs
for ip in 1.2.3.4 5.6.7.8; do
    iptables -A INPUT -s $ip -j DROP
done
```

3. **Enable challenge mode (if available)**:
```kdl
security {
    challenge-mode "enabled"
    challenge-threshold 100  // requests per minute triggers challenge
}
```

4. **Reduce resource limits to preserve availability**:
```kdl
limits {
    max-connections 10000
    max-connections-per-client 10
    max-requests-per-connection 100
}
```

---

## Post-Incident

### Immediate Post-Incident (< 1 hour after resolution)

```
[ ] Update status page to "Resolved"
[ ] Send all-clear communication
[ ] Document timeline in incident channel
[ ] Preserve logs and metrics snapshots
[ ] Schedule post-mortem (SEV1/SEV2: within 48 hours)
```

### Log Preservation

```bash
# Create incident snapshot
INCIDENT_ID="INC-$(date +%Y%m%d)-001"
mkdir -p /var/log/sentinel/incidents/$INCIDENT_ID

# Save logs
journalctl -u sentinel --since "1 hour ago" > /var/log/sentinel/incidents/$INCIDENT_ID/sentinel.log

# Save metrics snapshot
curl -s localhost:9090/metrics > /var/log/sentinel/incidents/$INCIDENT_ID/metrics.txt

# Save config at time of incident
cp /etc/sentinel/config.kdl /var/log/sentinel/incidents/$INCIDENT_ID/

# Save system state
free -h > /var/log/sentinel/incidents/$INCIDENT_ID/memory.txt
df -h > /var/log/sentinel/incidents/$INCIDENT_ID/disk.txt
ss -s > /var/log/sentinel/incidents/$INCIDENT_ID/connections.txt
```

### Post-Mortem Template

```markdown
# Incident Post-Mortem: [INCIDENT_ID]

## Summary
- **Date**: YYYY-MM-DD
- **Duration**: X hours Y minutes
- **Severity**: SEVN
- **Impact**: [Brief description of user impact]

## Timeline
| Time (UTC) | Event |
|------------|-------|
| HH:MM | First alert triggered |
| HH:MM | Incident declared |
| HH:MM | Root cause identified |
| HH:MM | Mitigation applied |
| HH:MM | Full resolution |

## Root Cause
[Detailed explanation of what caused the incident]

## Contributing Factors
- [Factor 1]
- [Factor 2]

## Resolution
[What was done to resolve the incident]

## Action Items
| ID | Action | Owner | Due Date | Status |
|----|--------|-------|----------|--------|
| 1 | [Action item] | [Owner] | YYYY-MM-DD | Open |

## Lessons Learned
- What went well:
- What could be improved:

## Metrics
- Time to detect: X minutes
- Time to mitigate: X minutes
- Time to resolve: X minutes
- Customer impact: X% of requests affected
```

---

## Communication Templates

### Initial Incident Communication

```
Subject: [INVESTIGATING] [Service] - [Brief Description]

We are currently investigating an issue affecting [service/feature].

**Status**: Investigating
**Impact**: [Description of user impact]
**Started**: [Time UTC]

We will provide updates every [15/30] minutes until resolved.

Next update: [Time UTC]
```

### Update Communication

```
Subject: [UPDATE] [Service] - [Brief Description]

**Status**: [Investigating/Identified/Monitoring]
**Impact**: [Description of user impact]
**Duration**: [X] hours [Y] minutes

**Update**:
[What we've learned or done since last update]

**Next Steps**:
[What we're doing next]

Next update: [Time UTC]
```

### Resolution Communication

```
Subject: [RESOLVED] [Service] - [Brief Description]

The issue affecting [service/feature] has been resolved.

**Status**: Resolved
**Duration**: [X] hours [Y] minutes
**Impact**: [Description of what was affected]

**Root Cause**:
[Brief, non-technical explanation]

**Resolution**:
[What we did to fix it]

We apologize for any inconvenience. A detailed post-mortem will be
conducted and follow-up improvements will be made.
```

---

## Quick Reference Card

### Critical Commands
```bash
# Health check
curl -sf localhost:8080/health

# Reload config
kill -HUP $(cat /var/run/sentinel.pid)

# Graceful restart
systemctl restart sentinel

# Emergency stop
systemctl stop sentinel

# View errors
journalctl -u sentinel | grep ERROR | tail -20

# Check upstreams
curl -s localhost:9090/metrics | grep upstream_health
```

### Key Metrics to Check First
1. `sentinel_requests_total{status="5xx"}` - Error count
2. `sentinel_upstream_health` - Upstream availability
3. `sentinel_request_duration_seconds` - Latency
4. `sentinel_open_connections` - Connection count
5. `sentinel_circuit_breaker_state` - Circuit breaker status

### Escalation Contacts
| Role | Contact |
|------|---------|
| On-call | PagerDuty |
| Team Lead | [Contact info] |
| Infrastructure | [Contact info] |
| Security | [Contact info] |

---

**Remember**: Stay calm, communicate clearly, and document everything.
