# Sentinel Workflow

Commands, processes, and common tasks for working with Sentinel.

---

## Development Environment

### Prerequisites

- Rust 1.92.0+ (see `rust-toolchain.toml`)
- mise (task runner)
- Docker (for integration tests)

### Setup

```bash
# Install mise tasks
mise install

# Build all crates
cargo build --workspace

# Verify setup
cargo test --workspace
```

---

## Common Commands

### Building

```bash
# Debug build (fast compilation)
cargo build --workspace

# Release build (optimized)
cargo build --workspace --release

# Build specific crate
cargo build -p sentinel-proxy
cargo build -p sentinel-config
cargo build -p sentinel-agent-protocol
```

### Testing

```bash
# Run all tests
cargo test --workspace

# Run tests for specific crate
cargo test -p sentinel-proxy
cargo test -p sentinel-config
cargo test -p sentinel-agent-protocol

# Run specific test
cargo test -p sentinel-proxy route_matching

# Run tests with output
cargo test --workspace -- --nocapture

# Run ignored (slow) tests
cargo test --workspace -- --ignored
```

### Linting

```bash
# Format code
cargo fmt --all

# Check formatting (CI)
cargo fmt --all --check

# Run clippy (must pass with no warnings)
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run clippy with fixes
cargo clippy --workspace --all-targets --fix --allow-dirty
```

### Documentation

```bash
# Generate docs
cargo doc --workspace --no-deps

# Open docs in browser
cargo doc --workspace --no-deps --open

# Check doc links
cargo doc --workspace --no-deps 2>&1 | grep -i warning
```

---

## Running Sentinel

### Local Development

```bash
# Run with default config
cargo run --bin sentinel -- --config config/sentinel.kdl

# Run with debug logging
RUST_LOG=debug cargo run --bin sentinel -- --config config/sentinel.kdl

# Run with specific log levels
RUST_LOG=sentinel=debug,pingora=info cargo run --bin sentinel

# Run release build
cargo run --release --bin sentinel -- --config config/sentinel.kdl
```

### Docker

```bash
# Build image
docker build -t sentinel:dev .

# Run container
docker run -p 8080:8080 -v $(pwd)/config:/etc/sentinel sentinel:dev

# Docker Compose (with upstreams)
docker-compose up
```

---

## Testing Workflows

### Unit Tests

Run frequently during development:

```bash
cargo test -p sentinel-config --lib
cargo test -p sentinel-agent-protocol --lib
```

### Integration Tests

Run before committing:

```bash
# Start test dependencies
docker-compose -f docker-compose.test.yml up -d

# Run integration tests
cargo test --test '*'

# Cleanup
docker-compose -f docker-compose.test.yml down
```

### Benchmarks

Run for performance-sensitive changes:

```bash
# Run all benchmarks
cargo bench -p sentinel-proxy

# Run specific benchmark
cargo bench -p sentinel-proxy routing

# Compare against baseline
cargo bench -p sentinel-proxy -- --save-baseline main
cargo bench -p sentinel-proxy -- --baseline main
```

---

## Git Workflow

### Branch Naming

```
feature/add-grpc-health-checks
fix/route-matching-priority
docs/update-agent-protocol
refactor/simplify-config-parsing
```

### Commit Messages

Follow conventional commits:

```
feat(proxy): add request timeout configuration

Add per-route timeout configuration with validation.
Defaults to 30s if not specified.

Closes #123
```

```
fix(agent): handle connection reset during streaming

The agent client now properly handles ECONNRESET errors
during body streaming, triggering circuit breaker as expected.

Fixes #456
```

```
docs(config): document rate limiting options

Add comprehensive documentation for token bucket and
sliding window rate limiting configuration.
```

### Pre-commit Checklist

```bash
# 1. Format
cargo fmt --all

# 2. Lint
cargo clippy --workspace --all-targets -- -D warnings

# 3. Test
cargo test --workspace

# 4. Check docs compile
cargo doc --workspace --no-deps
```

---

## Release Process

### Version Bump

1. Update version in `Cargo.toml` (workspace)
2. Update `CHANGELOG.md`
3. Commit: `chore: bump version to X.Y.Z`
4. Tag: `git tag vX.Y.Z`
5. Push: `git push && git push --tags`

### Crates.io Publishing

```bash
# Publish in dependency order
cargo publish -p sentinel-common
cargo publish -p sentinel-config
cargo publish -p sentinel-agent-protocol
cargo publish -p sentinel-proxy
```

### GitHub Release

```bash
# Create release with notes
gh release create vX.Y.Z --title "vX.Y.Z" --notes-file RELEASE_NOTES.md

# Attach binaries (if not done by CI)
gh release upload vX.Y.Z target/release/sentinel
```

---

## Debugging

### Logging

```bash
# Maximum verbosity
RUST_LOG=trace cargo run --bin sentinel

# Specific modules
RUST_LOG=sentinel::routing=debug,sentinel::agents=trace cargo run --bin sentinel

# Filter by span
RUST_LOG=sentinel[request_id]=debug cargo run --bin sentinel
```

### Profiling

```bash
# CPU profiling with flamegraph
cargo flamegraph --bin sentinel -- --config config/sentinel.kdl

# Memory profiling with heaptrack
heaptrack cargo run --release --bin sentinel

# Perf stat
perf stat cargo run --release --bin sentinel
```

### Debugging Tests

```bash
# Run single test with backtrace
RUST_BACKTRACE=1 cargo test -p sentinel-proxy specific_test -- --nocapture

# Run under debugger
rust-lldb target/debug/deps/sentinel_proxy-xxx specific_test
```

---

## Configuration Testing

### Validate Config

```bash
# Check config syntax
cargo run --bin sentinel -- --config config/sentinel.kdl --check

# Dry run (parse and validate, don't start)
cargo run --bin sentinel -- --config config/sentinel.kdl --dry-run
```

### Config Examples

Test against example configs:

```bash
for config in config/examples/*.kdl; do
    echo "Testing $config"
    cargo run --bin sentinel -- --config "$config" --check
done
```

---

## Mise Tasks

Common tasks are defined in `mise.toml`:

```bash
# List available tasks
mise tasks

# Run specific task
mise run build
mise run test
mise run lint
mise run docs
```

---

## Troubleshooting

### Build Errors

```bash
# Clean and rebuild
cargo clean && cargo build --workspace

# Update dependencies
cargo update

# Check for outdated deps
cargo outdated
```

### Test Failures

```bash
# Run with verbose output
cargo test --workspace -- --nocapture

# Run single test isolated
cargo test -p sentinel-proxy test_name -- --test-threads=1

# Check for port conflicts
lsof -i :8080
```

### Performance Issues

```bash
# Build with optimizations for profiling
cargo build --release --features profiling

# Check for debug assertions in release
cargo build --release 2>&1 | grep debug_assertions
```

---

## CI/CD

### GitHub Actions

Workflows in `.github/workflows/`:

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | Push, PR | Build, test, lint |
| `release.yml` | Tag push | Build binaries, publish |
| `docs.yml` | Push to main | Deploy documentation |

### Local CI Simulation

```bash
# Run what CI runs
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```
