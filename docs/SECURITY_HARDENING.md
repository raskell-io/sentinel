# Sentinel Proxy - Security Hardening Runbook

## Table of Contents
1. [Security Baseline](#security-baseline)
2. [TLS Configuration](#tls-configuration)
3. [Header Security](#header-security)
4. [Access Control](#access-control)
5. [Rate Limiting](#rate-limiting)
6. [Logging and Auditing](#logging-and-auditing)
7. [File System Security](#file-system-security)
8. [Network Security](#network-security)
9. [Security Checklist](#security-checklist)
10. [Security Incident Procedures](#security-incident-procedures)

---

## Security Baseline

### Principle: Defense in Depth

Sentinel follows a defense-in-depth approach:
1. **Network layer**: Firewall rules, network segmentation
2. **Transport layer**: TLS, certificate validation
3. **Application layer**: Header validation, rate limiting, WAF
4. **Host layer**: File permissions, process isolation

### Default Security Posture

Sentinel ships with secure defaults:

```kdl
// These are the defaults - you don't need to set them
// Shown here for documentation

security {
    // Fail closed on errors by default
    failure-mode "closed"

    // Enforce request limits
    limits {
        max-header-size 8192
        max-header-count 100
        max-body-size 10485760  // 10MB
        max-uri-length 8192
    }

    // Enforce timeouts
    timeouts {
        request-header-secs 30
        request-body-secs 60
        response-header-secs 60
        response-body-secs 120
    }
}
```

---

## TLS Configuration

### Minimum TLS Requirements

```kdl
listeners {
    listener "https" {
        address "0.0.0.0:443"

        tls {
            cert "/etc/sentinel/certs/server.crt"
            key "/etc/sentinel/certs/server.key"

            // Minimum TLS 1.2 (TLS 1.3 preferred)
            min-version "1.2"

            // Strong cipher suites only
            ciphers [
                "TLS_AES_256_GCM_SHA384"
                "TLS_AES_128_GCM_SHA256"
                "TLS_CHACHA20_POLY1305_SHA256"
                "ECDHE-ECDSA-AES256-GCM-SHA384"
                "ECDHE-RSA-AES256-GCM-SHA384"
                "ECDHE-ECDSA-AES128-GCM-SHA256"
                "ECDHE-RSA-AES128-GCM-SHA256"
            ]

            // OCSP stapling
            ocsp-stapling true

            // Session resumption with rotation
            session-tickets true
            session-timeout-secs 3600
        }
    }
}
```

### Certificate Requirements

**Server Certificates**:
- Minimum 2048-bit RSA or 256-bit ECDSA
- SHA-256 or stronger signature algorithm
- Valid DNS names in SAN (Subject Alternative Name)
- Certificate chain complete (including intermediates)

**Validation Commands**:
```bash
# Check certificate details
openssl x509 -in /etc/sentinel/certs/server.crt -noout -text | grep -A2 "Public-Key\|Signature Algorithm"

# Verify certificate chain
openssl verify -CAfile /etc/sentinel/certs/ca-chain.crt /etc/sentinel/certs/server.crt

# Check for weak keys
openssl rsa -in /etc/sentinel/certs/server.key -text -noout 2>/dev/null | grep "Private-Key"

# Test TLS configuration
openssl s_client -connect localhost:443 -tls1_2 </dev/null 2>/dev/null | grep "Cipher is"
nmap --script ssl-enum-ciphers -p 443 localhost
```

### mTLS for Upstreams

When connecting to backend services requiring client authentication:

```kdl
upstreams {
    upstream "secure-backend" {
        target "10.0.0.1:8443"

        tls {
            sni "backend.internal"

            // Client certificate for authentication
            client-cert "/etc/sentinel/certs/client.crt"
            client-key "/etc/sentinel/certs/client.key"

            // Verify upstream server certificate
            ca-cert "/etc/sentinel/certs/backend-ca.crt"

            // Never skip verification in production
            insecure-skip-verify false
        }
    }
}
```

### Certificate Rotation

```bash
#!/bin/bash
# Certificate rotation script

set -e

CERT_DIR="/etc/sentinel/certs"
NEW_CERT="$1"
NEW_KEY="$2"

# Validate new certificate
if ! openssl x509 -in "$NEW_CERT" -noout; then
    echo "Invalid certificate" >&2
    exit 1
fi

# Check key matches certificate
CERT_MOD=$(openssl x509 -in "$NEW_CERT" -noout -modulus | md5sum)
KEY_MOD=$(openssl rsa -in "$NEW_KEY" -noout -modulus | md5sum)
if [ "$CERT_MOD" != "$KEY_MOD" ]; then
    echo "Certificate and key do not match" >&2
    exit 1
fi

# Backup current certificates
cp "$CERT_DIR/server.crt" "$CERT_DIR/server.crt.$(date +%Y%m%d)"
cp "$CERT_DIR/server.key" "$CERT_DIR/server.key.$(date +%Y%m%d)"

# Deploy new certificates
cp "$NEW_CERT" "$CERT_DIR/server.crt"
cp "$NEW_KEY" "$CERT_DIR/server.key"
chmod 644 "$CERT_DIR/server.crt"
chmod 600 "$CERT_DIR/server.key"

# Reload Sentinel
kill -HUP $(cat /var/run/sentinel.pid)

# Verify
sleep 2
if curl -sf https://localhost/health; then
    echo "Certificate rotation successful"
else
    echo "Rotation may have failed - check logs" >&2
    exit 1
fi
```

---

## Header Security

### Security Response Headers

```kdl
policies {
    security-headers {
        // Prevent clickjacking
        x-frame-options "DENY"

        // Prevent MIME type sniffing
        x-content-type-options "nosniff"

        // XSS protection (legacy browsers)
        x-xss-protection "1; mode=block"

        // Referrer policy
        referrer-policy "strict-origin-when-cross-origin"

        // Content Security Policy
        content-security-policy "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'"

        // HTTP Strict Transport Security
        strict-transport-security "max-age=31536000; includeSubDomains; preload"

        // Permissions policy
        permissions-policy "geolocation=(), microphone=(), camera=()"
    }
}
```

### Header Sanitization

Remove sensitive headers from upstream responses:

```kdl
policies {
    header-sanitization {
        // Remove server identification
        remove-response-headers [
            "Server"
            "X-Powered-By"
            "X-AspNet-Version"
        ]

        // Remove internal routing headers
        remove-request-headers [
            "X-Forwarded-For"      // Will be set by Sentinel
            "X-Real-IP"
            "X-Original-URL"
        ]
    }
}
```

### Request Validation

```kdl
policies {
    request-validation {
        // Block requests with suspicious patterns
        block-paths [
            "*.php"           // Block PHP access attempts
            "*/.git/*"        // Block git directory access
            "*/.env"          // Block env file access
            "*/wp-admin/*"    // Block WordPress admin if not used
        ]

        // Require specific headers
        require-headers ["Host", "User-Agent"]

        // Block known bad user agents
        block-user-agents [
            "*sqlmap*"
            "*nikto*"
            "*nmap*"
        ]
    }
}
```

---

## Access Control

### IP-Based Access Control

```kdl
policies {
    ip-access-control {
        // Default policy
        default-action "allow"

        // Allow list (checked first)
        allow [
            "10.0.0.0/8"      // Internal network
            "192.168.0.0/16"  // Private network
        ]

        // Block list (checked second)
        block [
            "0.0.0.0/8"       // Invalid
            "100.64.0.0/10"   // Carrier-grade NAT
            "169.254.0.0/16"  // Link-local
        ]
    }
}
```

### GeoIP Blocking

```kdl
policies {
    geo-access-control {
        database "/etc/sentinel/geoip/GeoLite2-Country.mmdb"

        // Allow only specific countries
        allow-countries ["US", "CA", "GB", "DE", "FR"]

        // Or block specific countries
        // block-countries ["XX", "YY"]

        // Action for blocked requests
        action "block"       // or "challenge"
    }
}
```

### Route-Level Access Control

```kdl
routes {
    route "admin" {
        matches { path-prefix "/admin/" }

        policies {
            // Restrict to internal IPs
            ip-access-control {
                allow ["10.0.0.0/8"]
                default-action "block"
            }

            // Require authentication header
            require-headers ["Authorization"]
        }

        upstream "admin-backend"
    }
}
```

---

## Rate Limiting

### Global Rate Limits

```kdl
policies {
    global-rate-limit {
        // Overall request rate
        requests-per-second 10000
        burst 1000

        // Connection limits
        max-connections 50000
        max-connections-per-ip 100
    }
}
```

### Per-Route Rate Limits

```kdl
routes {
    route "api" {
        matches { path-prefix "/api/" }

        policies {
            rate-limit {
                // Per-client rate limit
                key "client_ip"
                requests-per-second 100
                burst 50

                // Response for rate-limited requests
                action "block"
                status-code 429
                retry-after-secs 60
            }
        }

        upstream "api-backend"
    }

    route "login" {
        matches { path "/auth/login" }

        policies {
            rate-limit {
                // Stricter limit for authentication
                key "client_ip"
                requests-per-second 5
                burst 10
                action "block"
            }
        }

        upstream "auth-backend"
    }
}
```

### Distributed Rate Limiting

For multi-instance deployments:

```kdl
rate-limit {
    backend "redis" {
        endpoints ["redis://redis-cluster:6379"]
        key-prefix "sentinel:ratelimit:"
    }

    // Synchronization settings
    sync-interval-ms 100
    local-cache-ttl-ms 1000
}
```

---

## Logging and Auditing

### Security Event Logging

```kdl
logging {
    // Security events log
    security-log {
        path "/var/log/sentinel/security.log"
        format "json"

        // Events to log
        events [
            "rate_limit_triggered"
            "ip_blocked"
            "geo_blocked"
            "waf_blocked"
            "auth_failure"
            "tls_handshake_failure"
            "request_validation_failure"
        ]

        // Include request context
        include-headers ["User-Agent", "X-Forwarded-For"]
        include-client-ip true
        include-request-id true
    }

    // Audit log for admin operations
    audit-log {
        path "/var/log/sentinel/audit.log"
        format "json"

        events [
            "config_reload"
            "upstream_health_change"
            "circuit_breaker_state_change"
        ]
    }
}
```

### Log Rotation

```bash
# /etc/logrotate.d/sentinel
/var/log/sentinel/*.log {
    daily
    rotate 30
    compress
    delaycompress
    missingok
    notifempty
    create 0640 sentinel sentinel
    sharedscripts
    postrotate
        kill -USR1 $(cat /var/run/sentinel.pid 2>/dev/null) 2>/dev/null || true
    endscript
}
```

### SIEM Integration

```kdl
logging {
    // Forward to SIEM
    siem {
        type "syslog"
        endpoint "siem.internal:514"
        protocol "tcp"
        format "cef"  // Common Event Format

        // Or use structured format
        // format "json"
    }
}
```

---

## File System Security

### Directory Structure

```bash
# Recommended directory layout
/etc/sentinel/
├── config.kdl              # 0640 sentinel:sentinel
├── certs/
│   ├── server.crt          # 0644 sentinel:sentinel
│   ├── server.key          # 0600 sentinel:sentinel
│   ├── ca-chain.crt        # 0644 sentinel:sentinel
│   ├── client.crt          # 0644 sentinel:sentinel
│   └── client.key          # 0600 sentinel:sentinel
└── geoip/
    └── GeoLite2-Country.mmdb  # 0644 sentinel:sentinel

/var/log/sentinel/          # 0750 sentinel:sentinel
├── access.log
├── error.log
├── security.log
└── audit.log

/var/run/sentinel/          # 0755 sentinel:sentinel
└── sentinel.pid
```

### Permission Hardening Script

```bash
#!/bin/bash
# Harden file permissions for Sentinel

set -e

SENTINEL_USER="sentinel"
SENTINEL_GROUP="sentinel"

# Configuration
chown -R root:$SENTINEL_GROUP /etc/sentinel
chmod 750 /etc/sentinel
chmod 640 /etc/sentinel/config.kdl

# Certificates
chmod 750 /etc/sentinel/certs
chmod 644 /etc/sentinel/certs/*.crt
chmod 600 /etc/sentinel/certs/*.key
chown -R root:$SENTINEL_GROUP /etc/sentinel/certs

# Logs
chown -R $SENTINEL_USER:$SENTINEL_GROUP /var/log/sentinel
chmod 750 /var/log/sentinel
chmod 640 /var/log/sentinel/*.log

# Runtime
chown -R $SENTINEL_USER:$SENTINEL_GROUP /var/run/sentinel
chmod 755 /var/run/sentinel

# Binary
chmod 755 /usr/local/bin/sentinel
chown root:root /usr/local/bin/sentinel

echo "Permissions hardened"
```

### Systemd Security Options

```ini
# /etc/systemd/system/sentinel.service

[Unit]
Description=Sentinel Reverse Proxy
After=network.target

[Service]
Type=simple
User=sentinel
Group=sentinel
ExecStart=/usr/local/bin/sentinel --config /etc/sentinel/config.kdl

# Security hardening
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/log/sentinel /var/run/sentinel
ReadOnlyPaths=/etc/sentinel

# Capabilities
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
AmbientCapabilities=CAP_NET_BIND_SERVICE

# System call filtering
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources

# Namespace isolation
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes

# Memory protection
MemoryDenyWriteExecute=yes

# Restrict address families
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX

[Install]
WantedBy=multi-user.target
```

---

## Network Security

### Firewall Configuration

```bash
#!/bin/bash
# Firewall rules for Sentinel

# Allow incoming HTTP/HTTPS
iptables -A INPUT -p tcp --dport 80 -j ACCEPT
iptables -A INPUT -p tcp --dport 443 -j ACCEPT

# Allow metrics (internal only)
iptables -A INPUT -p tcp --dport 9090 -s 10.0.0.0/8 -j ACCEPT
iptables -A INPUT -p tcp --dport 9090 -j DROP

# Allow health checks (internal only)
iptables -A INPUT -p tcp --dport 8080 -s 10.0.0.0/8 -j ACCEPT

# Rate limit new connections
iptables -A INPUT -p tcp --syn -m limit --limit 100/s --limit-burst 200 -j ACCEPT
iptables -A INPUT -p tcp --syn -j DROP

# Drop invalid packets
iptables -A INPUT -m state --state INVALID -j DROP

# Allow established connections
iptables -A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT

# Log dropped packets
iptables -A INPUT -j LOG --log-prefix "DROPPED: " --log-level 4
```

### Network Segmentation

```
                    ┌─────────────────────────────────────┐
                    │           DMZ Network               │
                    │                                     │
    Internet ──────►│  ┌──────────────────────────────┐   │
                    │  │       Sentinel Proxy          │   │
                    │  │   (Public: 443, Internal: 80) │   │
                    │  └──────────────────────────────┘   │
                    │              │                      │
                    └──────────────│──────────────────────┘
                                   │
                    ┌──────────────│──────────────────────┐
                    │   Internal Network (10.0.0.0/8)     │
                    │              │                      │
                    │  ┌───────────▼──────────────────┐   │
                    │  │       Backend Services        │   │
                    │  │   (Port 8080, mTLS required)  │   │
                    │  └──────────────────────────────┘   │
                    │                                     │
                    └─────────────────────────────────────┘
```

---

## Security Checklist

### Pre-Deployment Checklist

```
TLS Configuration:
[ ] TLS 1.2 minimum version enforced
[ ] Strong cipher suites only
[ ] Valid certificates installed
[ ] Certificate chain complete
[ ] Private keys have restricted permissions (0600)
[ ] HSTS enabled
[ ] OCSP stapling configured

Access Control:
[ ] Admin endpoints restricted to internal IPs
[ ] Rate limiting configured
[ ] GeoIP blocking configured (if required)
[ ] Request size limits configured

Headers:
[ ] Security headers configured
[ ] Server identification headers removed
[ ] Internal headers stripped

Logging:
[ ] Security events logged
[ ] Audit logging enabled
[ ] Log rotation configured
[ ] SIEM integration configured (if required)

System:
[ ] Run as non-root user
[ ] File permissions hardened
[ ] Systemd security options enabled
[ ] Firewall rules configured
[ ] Network segmentation in place

Operational:
[ ] Incident response plan documented
[ ] Certificate expiry monitoring in place
[ ] Security scan scheduled
[ ] Vulnerability management process defined
```

### Regular Security Tasks

| Task | Frequency | Command/Action |
|------|-----------|----------------|
| Review security logs | Daily | `grep -i "blocked\|denied\|failed" /var/log/sentinel/security.log` |
| Check certificate expiry | Weekly | `openssl x509 -in /etc/sentinel/certs/server.crt -noout -enddate` |
| Update GeoIP database | Monthly | Download latest MaxMind DB |
| Review rate limit effectiveness | Weekly | Check rate limit metrics |
| Security scan | Monthly | Run security scanner against proxy |
| Review access patterns | Weekly | Analyze access logs for anomalies |
| Audit configuration | Monthly | Review config for security best practices |
| Update dependencies | Monthly | Check for security updates |

### Security Scanning

```bash
# TLS configuration scan
testssl.sh --severity HIGH https://your-domain.com

# HTTP security headers check
curl -s -D- https://your-domain.com -o /dev/null | grep -i "strict\|content-security\|x-frame\|x-content-type"

# Check for common vulnerabilities
nikto -h https://your-domain.com

# SSL Labs rating (online)
# https://www.ssllabs.com/ssltest/
```

---

## Security Incident Procedures

### Suspected Compromise

**Immediate Actions**:
```bash
# 1. Capture current state
mkdir -p /tmp/incident-$(date +%Y%m%d)
cp /etc/sentinel/config.kdl /tmp/incident-$(date +%Y%m%d)/
journalctl -u sentinel --since "24 hours ago" > /tmp/incident-$(date +%Y%m%d)/logs.txt
curl -s localhost:9090/metrics > /tmp/incident-$(date +%Y%m%d)/metrics.txt

# 2. Check for unauthorized changes
md5sum /usr/local/bin/sentinel
diff /etc/sentinel/config.kdl /etc/sentinel/config.kdl.backup

# 3. Review recent access
grep -E "POST|PUT|DELETE" /var/log/sentinel/access.log | tail -100

# 4. Check for suspicious processes
ps aux | grep -v "grep\|sentinel" | grep -E "nc|ncat|python|perl|ruby|bash"

# 5. Review network connections
ss -tnp | grep ESTABLISHED
```

### Credential Exposure

**If TLS private key is compromised**:
```bash
# 1. Generate new certificate immediately
# (Use your CA process)

# 2. Deploy new certificate
cp new-cert.crt /etc/sentinel/certs/server.crt
cp new-key.key /etc/sentinel/certs/server.key
chmod 600 /etc/sentinel/certs/server.key

# 3. Reload Sentinel
kill -HUP $(cat /var/run/sentinel.pid)

# 4. Revoke old certificate with CA

# 5. Update any pinned certificates in clients
```

### Active Attack Response

```bash
# 1. Enable emergency rate limiting
cat >> /etc/sentinel/emergency.kdl << 'EOF'
policies {
    emergency-rate-limit {
        requests-per-second 10
        key "client_ip"
        action "block"
    }
}
EOF

# 2. Block attacking IPs at firewall
for ip in $(grep "rate_limit_triggered" /var/log/sentinel/security.log | \
    jq -r '.client_ip' | sort | uniq -c | sort -rn | head -20 | awk '{print $2}'); do
    iptables -I INPUT -s $ip -j DROP
    echo "Blocked: $ip"
done

# 3. Enable challenge mode if available
# Update config to require CAPTCHA/challenge

# 4. Scale horizontally if possible
# Deploy additional proxy instances

# 5. Notify security team
```

---

## References

- [OWASP Secure Headers Project](https://owasp.org/www-project-secure-headers/)
- [Mozilla SSL Configuration Generator](https://ssl-config.mozilla.org/)
- [CIS Benchmarks](https://www.cisecurity.org/benchmark)
- [NIST TLS Guidelines](https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-52r2.pdf)

---

**Security is a process, not a destination. Review and update this configuration regularly.**
