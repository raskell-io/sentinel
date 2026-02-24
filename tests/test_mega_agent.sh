#!/bin/bash
#
# Mega Agent Smoke Test: All 26 Agents
#
# Validates that ALL 26 agents can build, start, connect to Zentinel,
# and handle basic HTTP traffic without errors. Catches build breakage,
# protocol mismatches, config errors, and startup crashes across the
# entire agent ecosystem in a single run.
#
# Every agent must pass — missing binaries, build failures, and startup
# failures are all test failures.
#
# Prerequisites:
# - Rust toolchain (cargo) with wasm32-unknown-unknown target
# - Haskell toolchain (cabal) for policy agent
# - Python 3
# - curl, nc
# - All 26 agent repos as siblings at $REPO_ROOT/..
#
# Usage:
#   ./tests/test_mega_agent.sh
#   ZENTINEL_BIN=./target/release/zentinel ./tests/test_mega_agent.sh
#

set -euo pipefail

# ============================================================================
# Configuration
# ============================================================================

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

TEST_DIR="/tmp/zentinel-mega-agent-$$"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
AGENT_ROOT="$REPO_ROOT/.."
ZENTINEL_BIN="${ZENTINEL_BIN:-}"

CONFIGS="$TEST_DIR/configs"
SCRIPTS="$TEST_DIR/scripts"
WASM="$TEST_DIR/wasm"
RULES="$TEST_DIR/rules"
POLICIES="$TEST_DIR/policies"
SOCKETS="$TEST_DIR/sockets"
LOGS="$TEST_DIR/logs"

PROXY_PORT=""
BACKEND_PORT=""
METRICS_PORT=""
PROXY_PID=""
BACKEND_PID=""

SCRIPT_START=$(date +%s)
SCRIPT_TIMEOUT=300

# Test counters
TOTAL_AGENTS=26
BUILDS_OK=0
BUILDS_FAILED=0
STARTS_OK=0
STARTS_FAILED=0
REQUESTS_OK=0
REQUESTS_FAILED=0
LOGS_CLEAN=0
LOGS_DIRTY=0

# Per-agent tracking (associative arrays)
declare -A AGENT_PIDS
declare -A AGENT_GRPC_PORTS
declare -A AGENT_BUILD_STATUS
declare -A AGENT_START_STATUS
declare -A AGENT_REQUEST_STATUS
declare -A AGENT_LOG_STATUS

CLEAN_HEADERS=(-H "User-Agent: ZentinelTest" -H "Accept: text/html")

# ============================================================================
# Agent Registry
# ============================================================================

AGENT_NAMES=(
    ai-gateway
    api-deprecation
    audit-logger
    auth
    bot-management
    chaos
    content-scanner
    denylist
    graphql-security
    grpc-inspector
    image-optimization
    ip-reputation
    js
    lua
    mock-server
    modsec
    mqtt-gateway
    policy
    ratelimit
    soap
    spiffe
    transform
    waf
    wasm
    websocket-inspector
    zentinelsec
)

declare -A AGENT_BINARY=(
    [ai-gateway]=zentinel-ai-gateway-agent
    [api-deprecation]=zentinel-api-deprecation-agent
    [audit-logger]=zentinel-audit-logger-agent
    [auth]=zentinel-auth-agent
    [bot-management]=zentinel-bot-management-agent
    [chaos]=zentinel-chaos-agent
    [content-scanner]=zentinel-content-scanner-agent
    [denylist]=zentinel-denylist-agent
    [graphql-security]=zentinel-graphql-security-agent
    [grpc-inspector]=zentinel-grpc-inspector-agent
    [image-optimization]=zentinel-image-optimization-agent
    [ip-reputation]=zentinel-ip-reputation-agent
    [js]=zentinel-js-agent
    [lua]=zentinel-lua-agent
    [mock-server]=zentinel-mock-server-agent
    [modsec]=zentinel-modsec-agent
    [mqtt-gateway]=zentinel-mqtt-gateway-agent
    [policy]=zentinel-policy-agent
    [ratelimit]=zentinel-ratelimit-agent
    [soap]=zentinel-soap-agent
    [spiffe]=zentinel-spiffe-agent
    [transform]=zentinel-transform-agent
    [waf]=zentinel-waf-agent
    [wasm]=zentinel-wasm-agent
    [websocket-inspector]=zentinel-websocket-inspector-agent
    [zentinelsec]=zentinel-zentinelsec-agent
)

# Transport: uds (Unix socket) or grpc (TCP)
declare -A AGENT_TRANSPORT=(
    [ai-gateway]=uds
    [api-deprecation]=uds
    [audit-logger]=uds
    [auth]=uds
    [bot-management]=uds
    [chaos]=uds
    [content-scanner]=grpc
    [denylist]=uds
    [graphql-security]=uds
    [grpc-inspector]=uds
    [image-optimization]=uds
    [ip-reputation]=uds
    [js]=grpc
    [lua]=uds
    [mock-server]=uds
    [modsec]=uds
    [mqtt-gateway]=uds
    [policy]=uds
    [ratelimit]=uds
    [soap]=uds
    [spiffe]=uds
    [transform]=grpc
    [waf]=uds
    [wasm]=grpc
    [websocket-inspector]=uds
    [zentinelsec]=grpc
)

# Events each agent subscribes to (valid: request_headers request_body response_headers response_body)
declare -A AGENT_EVENTS=(
    [ai-gateway]="request_headers request_body"
    [api-deprecation]="request_headers response_headers"
    [audit-logger]="request_headers request_body response_headers response_body"
    [auth]="request_headers"
    [bot-management]="request_headers"
    [chaos]="request_headers response_headers response_body"
    [content-scanner]="request_body"
    [denylist]="request_headers"
    [graphql-security]="request_headers request_body"
    [grpc-inspector]="request_headers request_body"
    [image-optimization]="request_headers response_headers response_body"
    [ip-reputation]="request_headers"
    [js]="request_headers response_headers"
    [lua]="request_headers response_headers"
    [mock-server]="request_headers request_body"
    [modsec]="request_headers request_body response_headers response_body"
    [mqtt-gateway]="request_body response_body"
    [policy]="request_headers"
    [ratelimit]="request_headers"
    [soap]="request_headers request_body"
    [spiffe]="request_headers"
    [transform]="request_headers request_body response_headers response_body"
    [waf]="request_headers"
    [wasm]="request_headers response_headers"
    [websocket-inspector]="request_headers response_headers request_body response_body"
    [zentinelsec]="request_headers request_body response_headers response_body"
)

# ============================================================================
# Utility functions
# ============================================================================

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[PASS]${NC} $1"; }
log_failure() { echo -e "${RED}[FAIL]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }

log_phase() {
    echo -e "\n${CYAN}═══════════════════════════════════════${NC}"
    echo -e "${CYAN} $1${NC}"
    echo -e "${CYAN}═══════════════════════════════════════${NC}\n"
}

find_free_port() {
    python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

check_timeout() {
    local now
    now=$(date +%s)
    local elapsed=$((now - SCRIPT_START))
    if [[ $elapsed -ge $SCRIPT_TIMEOUT ]]; then
        echo -e "${RED}[TIMEOUT]${NC} Script exceeded ${SCRIPT_TIMEOUT}s timeout"
        exit 1
    fi
}

# ============================================================================
# Cleanup
# ============================================================================

cleanup() {
    log_info "Cleaning up..."

    for name in "${AGENT_NAMES[@]}"; do
        local pid="${AGENT_PIDS[$name]:-}"
        [[ -n "$pid" ]] && kill -TERM "$pid" 2>/dev/null || true
    done
    [[ -n "$PROXY_PID" ]] && kill -TERM "$PROXY_PID" 2>/dev/null || true
    [[ -n "$BACKEND_PID" ]] && kill -TERM "$BACKEND_PID" 2>/dev/null || true

    sleep 1

    for name in "${AGENT_NAMES[@]}"; do
        local pid="${AGENT_PIDS[$name]:-}"
        [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null || true
    done
    [[ -n "$PROXY_PID" ]] && kill -9 "$PROXY_PID" 2>/dev/null || true
    [[ -n "$BACKEND_PID" ]] && kill -9 "$BACKEND_PID" 2>/dev/null || true

    rm -rf "$TEST_DIR"
}

trap cleanup EXIT INT TERM

# ============================================================================
# Phase 1: Build all agents
# ============================================================================

build_zentinel() {
    if [[ -z "$ZENTINEL_BIN" ]]; then
        if [[ -f "$REPO_ROOT/target/release/zentinel" ]]; then
            ZENTINEL_BIN="$REPO_ROOT/target/release/zentinel"
            log_info "Using existing Zentinel binary: $ZENTINEL_BIN"
        else
            log_info "Building Zentinel proxy (release)..."
            (cd "$REPO_ROOT" && cargo build --release --bin zentinel)
            ZENTINEL_BIN="$REPO_ROOT/target/release/zentinel"
        fi
    fi

    if [[ ! -f "$ZENTINEL_BIN" ]]; then
        log_failure "Zentinel binary not found at $ZENTINEL_BIN"
        exit 1
    fi
}

build_wasm_module() {
    local wasm_example="$AGENT_ROOT/zentinel-agent-wasm/examples/wasm-module"

    if [[ ! -d "$wasm_example" ]]; then
        log_failure "  wasm module: example dir not found"
        return 1
    fi

    # Ensure target is available
    if ! rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
        log_info "  wasm module: installing wasm32-unknown-unknown target..."
        rustup target add wasm32-unknown-unknown
    fi

    local wasm_output="$wasm_example/target/wasm32-unknown-unknown/release/example_wasm_module.wasm"
    if [[ ! -f "$wasm_output" ]]; then
        log_info "  wasm module: building..."
        if ! (cd "$wasm_example" && cargo build --target wasm32-unknown-unknown --release 2>"$LOGS/wasm-module-build.log"); then
            log_failure "  wasm module: build failed"
            tail -5 "$LOGS/wasm-module-build.log" 2>/dev/null || true
            return 1
        fi
    fi

    cp "$wasm_output" "$WASM/passthrough.wasm"
    log_info "  wasm module: ready"
}

build_agent() {
    local name="$1"
    local binary="${AGENT_BINARY[$name]}"
    local repo="$AGENT_ROOT/zentinel-agent-$name"

    if [[ ! -d "$repo" ]]; then
        log_failure "  $name: repo not found at zentinel-agent-$name"
        AGENT_BUILD_STATUS[$name]="FAIL"
        ((BUILDS_FAILED++)) || true
        return 1
    fi

    # Policy agent is Haskell (cabal)
    if [[ "$name" == "policy" ]]; then
        local policy_bin
        policy_bin=$(cd "$repo" && cabal list-bin "$binary" 2>/dev/null || true)
        if [[ -n "$policy_bin" && -f "$policy_bin" ]]; then
            log_info "  $name: using existing binary"
            AGENT_BUILD_STATUS[$name]="OK"
            ((BUILDS_OK++)) || true
            return 0
        fi
        log_info "  $name: building with cabal..."
        if (cd "$repo" && cabal build 2>"$LOGS/$name-build.log"); then
            AGENT_BUILD_STATUS[$name]="OK"
            ((BUILDS_OK++)) || true
        else
            log_failure "  $name: cabal build failed"
            tail -5 "$LOGS/$name-build.log" 2>/dev/null || true
            AGENT_BUILD_STATUS[$name]="FAIL"
            ((BUILDS_FAILED++)) || true
            return 1
        fi
        return 0
    fi

    # Rust agents
    if [[ -f "$repo/target/release/$binary" ]]; then
        log_info "  $name: using existing binary"
        AGENT_BUILD_STATUS[$name]="OK"
        ((BUILDS_OK++)) || true
        return 0
    fi

    log_info "  $name: building (release)..."
    if (cd "$repo" && cargo build --release 2>"$LOGS/$name-build.log"); then
        AGENT_BUILD_STATUS[$name]="OK"
        ((BUILDS_OK++)) || true
    else
        log_failure "  $name: cargo build failed"
        tail -5 "$LOGS/$name-build.log" 2>/dev/null || true
        AGENT_BUILD_STATUS[$name]="FAIL"
        ((BUILDS_FAILED++)) || true
        return 1
    fi
}

build_all() {
    log_phase "Phase 1: Build all agents"

    build_zentinel
    build_wasm_module || true

    for name in "${AGENT_NAMES[@]}"; do
        check_timeout
        build_agent "$name" || true
    done

    log_info "Build results: $BUILDS_OK OK, $BUILDS_FAILED failed"
}

# ============================================================================
# Phase 2: Generate configs and stub files
# ============================================================================

generate_stubs() {
    log_phase "Phase 2: Generate configs and stub files"

    mkdir -p "$CONFIGS" "$SCRIPTS/lua" "$WASM" "$RULES" "$POLICIES" "$SOCKETS" "$LOGS"

    # --- YAML config stubs ---

    cat > "$CONFIGS/api-deprecation.yaml" <<'YAML'
deprecated_endpoints: []
YAML

    cat > "$CONFIGS/audit-logger.yaml" <<YAML
output:
  type: file
  path: "$TEST_DIR/audit.log"
YAML

    cat > "$CONFIGS/chaos.yaml" <<'YAML'
faults: []
YAML

    cat > "$CONFIGS/content-scanner.yaml" <<'YAML'
rules: []
YAML

    cat > "$CONFIGS/graphql-security.yaml" <<'YAML'
max_depth: 10
max_aliases: 5
YAML

    cat > "$CONFIGS/grpc-inspector.yaml" <<'YAML'
rules: []
YAML

    cat > "$CONFIGS/ip-reputation.yaml" <<'YAML'
providers: []
YAML

    cat > "$CONFIGS/mock-server.yaml" <<'YAML'
mocks: []
YAML

    cat > "$CONFIGS/soap.yaml" <<'YAML'
services: []
YAML

    cat > "$CONFIGS/transform.yaml" <<'YAML'
rules: []
YAML

    # --- Script stubs ---

    cat > "$SCRIPTS/passthrough.js" <<'JS'
function on_request_headers(headers) { return { action: "continue" }; }
function on_response_headers(headers) { return { action: "continue" }; }
JS

    cat > "$SCRIPTS/lua/passthrough.lua" <<'LUA'
function on_request_headers(headers)
  return { action = "continue" }
end
function on_response_headers(headers)
  return { action = "continue" }
end
LUA

    # --- ModSecurity rules ---

    cat > "$RULES/minimal.conf" <<'CONF'
SecRuleEngine On
CONF

    # --- Cedar policy ---

    cat > "$POLICIES/policy.cedar" <<'CEDAR'
permit(principal, action, resource);
CEDAR

    # --- Backend content ---

    mkdir -p "$TEST_DIR/www"
    echo "<html><body>Hello from backend</body></html>" > "$TEST_DIR/www/index.html"

    log_info "Stub files generated"
}

# ============================================================================
# Phase 3: Start backend + all agents
# ============================================================================

start_backend() {
    BACKEND_PORT=$(find_free_port)
    python3 -m http.server "$BACKEND_PORT" --directory "$TEST_DIR/www" \
        > "$LOGS/backend.log" 2>&1 &
    BACKEND_PID=$!

    local retries=10
    while ! curl -sf "http://127.0.0.1:$BACKEND_PORT/" >/dev/null 2>&1; do
        sleep 0.5
        ((retries--))
        if [[ $retries -eq 0 ]]; then
            log_failure "Backend failed to start"
            return 1
        fi
    done
    log_info "Backend started on port $BACKEND_PORT (PID: $BACKEND_PID)"
}

get_agent_bin_path() {
    local name="$1"
    local binary="${AGENT_BINARY[$name]}"
    local repo="$AGENT_ROOT/zentinel-agent-$name"

    if [[ "$name" == "policy" ]]; then
        (cd "$repo" && cabal list-bin "$binary" 2>/dev/null) || echo ""
    else
        echo "$repo/target/release/$binary"
    fi
}

# Build the CLI args array for a given agent
build_agent_args() {
    local name="$1"
    local transport="${AGENT_TRANSPORT[$name]}"
    local socket_path="$SOCKETS/$name.sock"

    # Transport args
    if [[ "$transport" == "uds" ]]; then
        # Agents using -s short flag
        case "$name" in
            api-deprecation|audit-logger|bot-management|chaos|denylist|\
            graphql-security|grpc-inspector|image-optimization|ip-reputation|\
            mock-server|mqtt-gateway|policy|ratelimit|soap)
                echo "-s $socket_path"
                ;;
            *)
                echo "--socket $socket_path"
                ;;
        esac
    else
        local port
        port=$(find_free_port)
        AGENT_GRPC_PORTS[$name]=$port
        echo "--grpc-address 127.0.0.1:$port"
    fi

    # Agent-specific extra args
    case "$name" in
        ai-gateway)       echo "--fail-open" ;;
        api-deprecation)  echo "-c $CONFIGS/api-deprecation.yaml" ;;
        audit-logger)     echo "-c $CONFIGS/audit-logger.yaml" ;;
        auth)             echo "--jwt-secret test-secret --fail-open" ;;
        chaos)            echo "-c $CONFIGS/chaos.yaml" ;;
        content-scanner)  echo "--config $CONFIGS/content-scanner.yaml" ;;
        graphql-security) echo "--config $CONFIGS/graphql-security.yaml" ;;
        grpc-inspector)   echo "-c $CONFIGS/grpc-inspector.yaml" ;;
        ip-reputation)    echo "-c $CONFIGS/ip-reputation.yaml" ;;
        js)               echo "--script $SCRIPTS/passthrough.js --fail-open" ;;
        lua)              echo "--script $SCRIPTS/lua/passthrough.lua" ;;
        mock-server)      echo "-c $CONFIGS/mock-server.yaml" ;;
        modsec)           echo "--rules $RULES/minimal.conf" ;;
        policy)           echo "--engine cedar --policy-dir $POLICIES" ;;
        ratelimit)        echo "--default-rps 100 --default-burst 200" ;;
        soap)             echo "--config $CONFIGS/soap.yaml" ;;
        transform)        echo "--config $CONFIGS/transform.yaml" ;;
        waf)              echo "--paranoia-level 1 --fail-open" ;;
        wasm)             echo "--module $WASM/passthrough.wasm --fail-open" ;;
    esac
}

start_agent() {
    local name="$1"

    # Skip agents that failed to build
    if [[ "${AGENT_BUILD_STATUS[$name]:-}" != "OK" ]]; then
        AGENT_START_STATUS[$name]="SKIP"
        ((STARTS_FAILED++)) || true
        return 1
    fi

    local bin_path
    bin_path=$(get_agent_bin_path "$name")

    if [[ -z "$bin_path" || ! -f "$bin_path" ]]; then
        log_failure "  $name: binary not found"
        AGENT_START_STATUS[$name]="FAIL"
        ((STARTS_FAILED++)) || true
        return 1
    fi

    local log_file="$LOGS/$name.log"
    local transport="${AGENT_TRANSPORT[$name]}"

    # Build args (this also sets AGENT_GRPC_PORTS for grpc agents)
    local args_str
    args_str=$(build_agent_args "$name")

    # Start agent process
    # shellcheck disable=SC2086
    RUST_LOG=info "$bin_path" $args_str > "$log_file" 2>&1 &
    AGENT_PIDS[$name]=$!

    # Wait for agent to be ready
    local retries=20
    if [[ "$transport" == "uds" ]]; then
        local socket_path="$SOCKETS/$name.sock"
        while [[ ! -S "$socket_path" ]] && [[ $retries -gt 0 ]]; do
            if ! kill -0 "${AGENT_PIDS[$name]}" 2>/dev/null; then
                log_failure "  $name: process died during startup"
                tail -20 "$log_file"
                AGENT_START_STATUS[$name]="FAIL"
                ((STARTS_FAILED++)) || true
                return 1
            fi
            sleep 0.5
            ((retries--))
        done

        if [[ -S "$socket_path" ]]; then
            log_success "  $name: started (UDS, PID ${AGENT_PIDS[$name]})"
            AGENT_START_STATUS[$name]="OK"
            ((STARTS_OK++)) || true
        else
            log_failure "  $name: socket not created after 10s"
            tail -20 "$log_file"
            AGENT_START_STATUS[$name]="FAIL"
            ((STARTS_FAILED++)) || true
            return 1
        fi
    else
        local port="${AGENT_GRPC_PORTS[$name]}"
        while ! nc -z 127.0.0.1 "$port" 2>/dev/null && [[ $retries -gt 0 ]]; do
            if ! kill -0 "${AGENT_PIDS[$name]}" 2>/dev/null; then
                log_failure "  $name: process died during startup"
                tail -20 "$log_file"
                AGENT_START_STATUS[$name]="FAIL"
                ((STARTS_FAILED++)) || true
                return 1
            fi
            sleep 0.5
            ((retries--))
        done

        if nc -z 127.0.0.1 "$port" 2>/dev/null; then
            log_success "  $name: started (gRPC :$port, PID ${AGENT_PIDS[$name]})"
            AGENT_START_STATUS[$name]="OK"
            ((STARTS_OK++)) || true
        else
            log_failure "  $name: port $port not listening after 10s"
            tail -20 "$log_file"
            AGENT_START_STATUS[$name]="FAIL"
            ((STARTS_FAILED++)) || true
            return 1
        fi
    fi
}

start_all_agents() {
    log_phase "Phase 3: Start backend + all agents"

    start_backend || exit 1

    for name in "${AGENT_NAMES[@]}"; do
        check_timeout
        start_agent "$name" || true
    done

    log_info "Start results: $STARTS_OK OK, $STARTS_FAILED failed"
}

# ============================================================================
# Phase 4: Generate KDL config and start Zentinel
# ============================================================================

generate_kdl_config() {
    PROXY_PORT=$(find_free_port)
    METRICS_PORT=$(find_free_port)

    local config_file="$TEST_DIR/config.kdl"

    # System, listeners
    cat > "$config_file" <<EOF
system {
    worker-threads 2
    max-connections 1000
    graceful-shutdown-timeout-secs 5
}

listeners {
    listener "http" {
        address "127.0.0.1:$PROXY_PORT"
        protocol "http"
        request-timeout-secs 30
    }
}

EOF

    # Agents block
    echo "agents {" >> "$config_file"
    for name in "${AGENT_NAMES[@]}"; do
        [[ "${AGENT_START_STATUS[$name]:-}" != "OK" ]] && continue

        local transport="${AGENT_TRANSPORT[$name]}"
        local events="${AGENT_EVENTS[$name]}"

        echo "    agent \"$name-agent\" type=\"custom\" {" >> "$config_file"

        if [[ "$transport" == "uds" ]]; then
            echo "        unix-socket \"$SOCKETS/$name.sock\"" >> "$config_file"
        else
            local port="${AGENT_GRPC_PORTS[$name]}"
            echo "        grpc \"http://127.0.0.1:$port\"" >> "$config_file"
        fi

        printf "        events" >> "$config_file"
        for event in $events; do
            printf " \"%s\"" "$event" >> "$config_file"
        done
        echo "" >> "$config_file"

        echo "        timeout-ms 500" >> "$config_file"
        echo "        failure-mode \"open\"" >> "$config_file"
        echo "    }" >> "$config_file"
    done
    echo "}" >> "$config_file"
    echo "" >> "$config_file"

    # Routes block
    echo "routes {" >> "$config_file"

    # Control route (no agents)
    cat >> "$config_file" <<'EOF'
    route "control" {
        priority "high"
        matches {
            path-prefix "/control/"
        }
        upstream "test-backend"
    }
EOF

    # Per-agent routes
    for name in "${AGENT_NAMES[@]}"; do
        [[ "${AGENT_START_STATUS[$name]:-}" != "OK" ]] && continue

        cat >> "$config_file" <<EOF
    route "test-$name" {
        priority "high"
        matches {
            path-prefix "/test-$name/"
        }
        upstream "test-backend"
        agents "$name-agent"
    }
EOF
    done

    # Default fallback
    cat >> "$config_file" <<'EOF'
    route "default" {
        priority "low"
        matches {
            path-prefix "/"
        }
        upstream "test-backend"
    }
}
EOF

    # Upstreams, limits, observability
    cat >> "$config_file" <<EOF

upstreams {
    upstream "test-backend" {
        target "127.0.0.1:$BACKEND_PORT" weight=1
        load-balancing "round_robin"
    }
}

limits {
    max-header-count 100
    max-header-size-bytes 8192
    max-body-size-bytes 1048576
}

observability {
    metrics {
        enabled #true
        address "127.0.0.1:$METRICS_PORT"
        path "/metrics"
    }
    logging {
        level "info"
        format "json"
    }
}
EOF

    log_info "KDL config generated at $config_file"
}

start_zentinel() {
    log_phase "Phase 4: Generate KDL config and start Zentinel"

    generate_kdl_config

    log_info "Starting Zentinel proxy (port $PROXY_PORT)..."

    RUST_LOG=info ZENTINEL_CONFIG="$TEST_DIR/config.kdl" \
        "$ZENTINEL_BIN" > "$LOGS/proxy.log" 2>&1 &
    PROXY_PID=$!

    local retries=20
    while ! curl -sf "${CLEAN_HEADERS[@]}" "http://127.0.0.1:$PROXY_PORT/control/" >/dev/null 2>&1; do
        sleep 0.5
        ((retries--))
        if [[ $retries -eq 0 ]]; then
            log_failure "Zentinel failed to start"
            tail -30 "$LOGS/proxy.log"
            exit 1
        fi
    done

    log_info "Zentinel started on port $PROXY_PORT (PID: $PROXY_PID)"
}

# ============================================================================
# Phase 5: Send requests and validate
# ============================================================================

send_requests() {
    log_phase "Phase 5: Send requests and validate"

    # Control route (no agents — must return 200)
    local status
    status=$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 \
        "${CLEAN_HEADERS[@]}" "http://127.0.0.1:$PROXY_PORT/control/" || echo "000")

    if [[ "$status" == "200" ]]; then
        log_success "  control: $status"
    else
        log_failure "  control: $status (expected 200)"
    fi

    # Per-agent routes
    for name in "${AGENT_NAMES[@]}"; do
        if [[ "${AGENT_START_STATUS[$name]:-}" != "OK" ]]; then
            AGENT_REQUEST_STATUS[$name]="SKIP"
            ((REQUESTS_FAILED++)) || true
            continue
        fi

        check_timeout

        status=$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 \
            "${CLEAN_HEADERS[@]}" "http://127.0.0.1:$PROXY_PORT/test-$name/" || echo "000")

        if [[ "$status" == "200" ]]; then
            log_success "  $name: $status"
            AGENT_REQUEST_STATUS[$name]="OK"
            ((REQUESTS_OK++)) || true
        else
            log_failure "  $name: $status (expected 200)"
            AGENT_REQUEST_STATUS[$name]="FAIL($status)"
            ((REQUESTS_FAILED++)) || true
        fi
    done

    log_info "Request results: $REQUESTS_OK OK, $REQUESTS_FAILED failed"
}

# ============================================================================
# Phase 6: Scan logs for errors
# ============================================================================

scan_logs() {
    log_phase "Phase 6: Scan logs for errors"

    local error_pattern='ERROR|FATAL|panic|SIGSEGV|thread.*panicked'
    local any_dirty=false

    # Agent logs
    for name in "${AGENT_NAMES[@]}"; do
        local log_file="$LOGS/$name.log"
        if [[ ! -f "$log_file" ]]; then
            AGENT_LOG_STATUS[$name]="N/A"
            continue
        fi

        local matches
        matches=$(grep -cE "$error_pattern" "$log_file" 2>/dev/null || echo "0")

        if [[ "$matches" -gt 0 ]]; then
            log_failure "  $name: $matches error(s) in log"
            grep -E "$error_pattern" "$log_file" | head -5
            AGENT_LOG_STATUS[$name]="DIRTY"
            ((LOGS_DIRTY++)) || true
            any_dirty=true
        else
            AGENT_LOG_STATUS[$name]="CLEAN"
            ((LOGS_CLEAN++)) || true
        fi
    done

    # Proxy log
    if [[ -f "$LOGS/proxy.log" ]]; then
        local matches
        matches=$(grep -cE "$error_pattern" "$LOGS/proxy.log" 2>/dev/null || echo "0")
        if [[ "$matches" -gt 0 ]]; then
            log_failure "  proxy: $matches error(s) in log"
            grep -E "$error_pattern" "$LOGS/proxy.log" | head -5
            any_dirty=true
        else
            log_success "  proxy: clean"
        fi
    fi

    if [[ "$any_dirty" == "false" ]]; then
        log_info "All logs clean"
    fi
}

# ============================================================================
# Phase 7: Summary
# ============================================================================

print_summary() {
    log_phase "Phase 7: Summary"

    printf "\n%-25s %-8s %-8s %-12s %-8s\n" \
        "Agent" "Build" "Start" "Request" "Logs"
    printf "%-25s %-8s %-8s %-12s %-8s\n" \
        "-------------------------" "--------" "--------" "------------" "--------"

    local all_pass=true

    for name in "${AGENT_NAMES[@]}"; do
        local build="${AGENT_BUILD_STATUS[$name]:-N/A}"
        local start="${AGENT_START_STATUS[$name]:-N/A}"
        local request="${AGENT_REQUEST_STATUS[$name]:-N/A}"
        local logs="${AGENT_LOG_STATUS[$name]:-N/A}"

        # Color codes
        local bc sc rc lc
        [[ "$build" == "OK" ]]    && bc="$GREEN" || bc="$RED"
        [[ "$start" == "OK" ]]    && sc="$GREEN" || sc="$RED"
        [[ "$request" == "OK" ]]  && rc="$GREEN" || rc="$RED"
        [[ "$logs" == "CLEAN" ]]  && lc="$GREEN" || { [[ "$logs" == "N/A" ]] && lc="$YELLOW" || lc="$RED"; }

        printf "%-25s ${bc}%-8s${NC} ${sc}%-8s${NC} ${rc}%-12s${NC} ${lc}%-8s${NC}\n" \
            "$name" "$build" "$start" "$request" "$logs"

        if [[ "$build" != "OK" || "$start" != "OK" || "$request" != "OK" || "$logs" == "DIRTY" ]]; then
            all_pass=false
        fi
    done

    echo
    echo "==========================================="
    echo "Totals"
    echo "==========================================="
    echo "Agents:   $TOTAL_AGENTS"
    echo "Built:    $BUILDS_OK OK, $BUILDS_FAILED failed"
    echo "Started:  $STARTS_OK OK, $STARTS_FAILED failed"
    echo "Requests: $REQUESTS_OK OK, $REQUESTS_FAILED failed"
    echo "Logs:     $LOGS_CLEAN clean, $LOGS_DIRTY dirty"
    echo

    if [[ "$all_pass" == "true" ]]; then
        echo -e "${GREEN}All $TOTAL_AGENTS agents passed!${NC}"
        return 0
    else
        echo -e "${RED}Some agents failed!${NC}"
        echo
        echo "Logs directory: $LOGS"
        echo "Config file:    $TEST_DIR/config.kdl"
        return 1
    fi
}

# ============================================================================
# Main
# ============================================================================

main() {
    echo "==========================================="
    echo "Mega Agent Smoke Test (all $TOTAL_AGENTS agents)"
    echo "==========================================="

    mkdir -p "$TEST_DIR" "$LOGS"

    generate_stubs
    build_all

    if [[ $BUILDS_FAILED -gt 0 ]]; then
        log_warn "$BUILDS_FAILED agent(s) failed to build — continuing with remaining agents"
    fi

    start_all_agents

    if [[ $STARTS_OK -eq 0 ]]; then
        log_failure "No agents started successfully — aborting"
        exit 1
    fi

    start_zentinel

    # Give agents time to stabilize connections
    sleep 2

    send_requests
    scan_logs

    if print_summary; then
        exit 0
    else
        exit 1
    fi
}

main "$@"
