//! End-to-end wiring for upstream service discovery (zentinelproxy/zentinel#419).
//!
//! These tests exercise the path a configuration actually travels: KDL text →
//! `UpstreamConfig::discovery` → a resolved `UpstreamPool` whose targets came
//! from the discovery source → a refresh that replaces those targets.
//!
//! `file` discovery is the source under test because it is the only one that
//! can be driven deterministically without a network service. The code path it
//! exercises — resolve at construction, re-resolve in `refreshed()`, reconcile
//! circuit breakers, rebuild the load balancer — is shared by every backend, so
//! DNS, Consul and Kubernetes differ only in which client produces the backend
//! set.

use std::io::Write;
use std::time::Duration;

use zentinel_common::types::CircuitBreakerState;
use zentinel_config::{Config, UpstreamDiscovery};
use zentinel_proxy::upstream::UpstreamPool;

/// Write `lines` to a fresh temporary file and return its path.
fn backends_file(name: &str, lines: &[&str]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("zentinel-discovery-{name}.txt"));
    let mut file = std::fs::File::create(&path).expect("create backends file");
    for line in lines {
        writeln!(file, "{line}").expect("write backend line");
    }
    file.flush().expect("flush backends file");
    path
}

/// Overwrite an existing backends file, ensuring its mtime moves.
///
/// `FileDiscovery` re-reads only when the modification time changes, and a
/// rewrite within the same filesystem timestamp granularity would otherwise be
/// invisible to it.
fn rewrite_backends(path: &std::path::Path, lines: &[&str]) {
    std::thread::sleep(Duration::from_millis(1100));
    let mut file = std::fs::File::create(path).expect("rewrite backends file");
    for line in lines {
        writeln!(file, "{line}").expect("write backend line");
    }
    file.flush().expect("flush backends file");
}

fn config_with_discovery(path: &std::path::Path, extra_target: Option<&str>) -> Config {
    let pinned = extra_target
        .map(|t| format!("        target \"{t}\"\n"))
        .unwrap_or_default();
    let kdl = format!(
        r#"
schema-version "1.0"
system {{ worker-threads 0 }}
listeners {{
    listener "http" {{ address "127.0.0.1:0" }}
}}
upstreams {{
    upstream "discovered" {{
{pinned}        discovery "file" {{
            path "{}"
            watch-interval 1
        }}
    }}
}}
routes {{
    route "all" {{
        matches {{ path-prefix "/" }}
        upstream "discovered"
    }}
}}
"#,
        path.display()
    );
    Config::from_kdl(&kdl).expect("config with discovery parses")
}

/// The regression this issue is about: a `discovery` block used to parse into
/// nothing at all, so an upstream that relied on it had no targets and routed
/// nowhere.
#[tokio::test]
async fn discovery_block_populates_pool_targets() {
    let path = backends_file("populate", &["127.0.0.1:19501", "127.0.0.1:19502"]);
    let config = config_with_discovery(&path, None);

    let upstream = config.upstreams.get("discovered").expect("upstream parsed");
    assert!(
        matches!(upstream.discovery, Some(UpstreamDiscovery::File { .. })),
        "discovery block should parse into UpstreamDiscovery::File, got {:?}",
        upstream.discovery
    );
    assert!(
        upstream.targets.is_empty(),
        "no targets are configured statically in this fixture"
    );

    let pool = UpstreamPool::new(upstream.clone())
        .await
        .expect("pool builds from discovery alone");

    let mut addresses = pool.target_addresses();
    addresses.sort();
    assert_eq!(
        addresses,
        vec!["127.0.0.1:19501".to_string(), "127.0.0.1:19502".to_string()],
        "pool targets should come from the discovery source"
    );
}

/// An upstream with no targets and no discovery is still an error: relaxing the
/// check for discovery must not have relaxed it for everyone.
#[tokio::test]
async fn upstream_without_targets_or_discovery_is_still_rejected() {
    let kdl = r#"
schema-version "1.0"
system { worker-threads 0 }
listeners { listener "http" { address "127.0.0.1:0" } }
upstreams {
    upstream "empty" {
        load-balancing "round-robin"
    }
}
routes {
    route "all" { matches { path-prefix "/" } ; upstream "empty" }
}
"#;
    let err = Config::from_kdl(kdl).expect_err("an upstream with nothing must not load");
    let message = err.to_string();
    assert!(
        message.contains("target"),
        "error should name the missing targets, got: {message}"
    );
}

/// Discovered targets are added to configured ones rather than replacing them.
#[tokio::test]
async fn static_targets_are_kept_alongside_discovered_ones() {
    let path = backends_file("mixed", &["127.0.0.1:19511"]);
    let config = config_with_discovery(&path, Some("127.0.0.1:19510"));
    let upstream = config.upstreams.get("discovered").expect("upstream parsed");

    let pool = UpstreamPool::new(upstream.clone())
        .await
        .expect("pool builds");

    let mut addresses = pool.target_addresses();
    addresses.sort();
    assert_eq!(
        addresses,
        vec!["127.0.0.1:19510".to_string(), "127.0.0.1:19511".to_string()],
        "the pinned target and the discovered one should both be present"
    );
}

/// A refresh that finds the same backends should not produce a replacement
/// pool: swapping on every tick would churn the registry for no reason.
#[tokio::test]
async fn refresh_without_change_returns_none() {
    let path = backends_file("nochange", &["127.0.0.1:19521"]);
    let config = config_with_discovery(&path, None);
    let upstream = config.upstreams.get("discovered").expect("upstream parsed");
    let pool = UpstreamPool::new(upstream.clone())
        .await
        .expect("pool builds");

    tokio::time::sleep(Duration::from_millis(1100)).await;

    assert!(
        pool.refreshed().await.is_none(),
        "an unchanged discovery result should not rebuild the pool"
    );
}

/// The core of stage three: a changed backend list produces a pool serving the
/// new targets.
#[tokio::test]
async fn refresh_picks_up_changed_backends() {
    let path = backends_file("changed", &["127.0.0.1:19531"]);
    let config = config_with_discovery(&path, None);
    let upstream = config.upstreams.get("discovered").expect("upstream parsed");
    let pool = UpstreamPool::new(upstream.clone())
        .await
        .expect("pool builds");
    assert_eq!(pool.target_addresses(), vec!["127.0.0.1:19531".to_string()]);

    rewrite_backends(&path, &["127.0.0.1:19532", "127.0.0.1:19533"]);

    let refreshed = pool
        .refreshed()
        .await
        .expect("a changed backend list should rebuild the pool");

    let mut addresses = refreshed.target_addresses();
    addresses.sort();
    assert_eq!(
        addresses,
        vec!["127.0.0.1:19532".to_string(), "127.0.0.1:19533".to_string()],
        "the refreshed pool should serve the new backends"
    );
    assert_eq!(refreshed.target_count(), 2);
}

/// Circuit breaker state has to survive the swap. If it did not, a backend that
/// was failing would look healthy again on every refresh interval — a flapping
/// backend would never stay ejected.
#[tokio::test]
async fn refresh_preserves_circuit_breaker_state_for_surviving_targets() {
    let path = backends_file("breakers", &["127.0.0.1:19541"]);
    let config = config_with_discovery(&path, None);
    let upstream = config.upstreams.get("discovered").expect("upstream parsed");
    let pool = UpstreamPool::new(upstream.clone())
        .await
        .expect("pool builds");

    // Trip the breaker for the target that will survive the refresh. The
    // default failure threshold is 5.
    for _ in 0..10 {
        pool.report_result("127.0.0.1:19541", false).await;
    }
    assert_eq!(
        pool.circuit_breaker_state("127.0.0.1:19541").await,
        Some(CircuitBreakerState::Open),
        "the breaker should be open before the refresh"
    );

    // Add a second backend, keeping the first.
    rewrite_backends(&path, &["127.0.0.1:19541", "127.0.0.1:19542"]);

    let refreshed = pool.refreshed().await.expect("target set changed");

    assert_eq!(
        refreshed.circuit_breaker_state("127.0.0.1:19541").await,
        Some(CircuitBreakerState::Open),
        "a target that survived the refresh must keep its open breaker"
    );
    assert_eq!(
        refreshed.circuit_breaker_state("127.0.0.1:19542").await,
        Some(CircuitBreakerState::Closed),
        "a newly discovered target should start with a closed breaker"
    );
}

/// Breakers for targets that went away must be dropped, or the map grows
/// without bound as backends churn.
#[tokio::test]
async fn refresh_drops_breakers_for_removed_targets() {
    let path = backends_file("removed", &["127.0.0.1:19551", "127.0.0.1:19552"]);
    let config = config_with_discovery(&path, None);
    let upstream = config.upstreams.get("discovered").expect("upstream parsed");
    let pool = UpstreamPool::new(upstream.clone())
        .await
        .expect("pool builds");

    assert!(pool
        .circuit_breaker_state("127.0.0.1:19552")
        .await
        .is_some());

    rewrite_backends(&path, &["127.0.0.1:19551"]);
    let refreshed = pool.refreshed().await.expect("target set changed");

    assert_eq!(
        refreshed.circuit_breaker_state("127.0.0.1:19552").await,
        None,
        "a target that left the discovery set should not keep a breaker"
    );
    assert!(
        refreshed
            .circuit_breaker_state("127.0.0.1:19551")
            .await
            .is_some(),
        "the surviving target should still have one"
    );
}

/// A source that resolves to nothing leaves the pool empty rather than failing
/// to build: a scaled-to-zero service should not stop the proxy from starting.
#[tokio::test]
async fn empty_discovery_result_builds_an_empty_pool() {
    let path = backends_file("empty", &[]);
    let config = config_with_discovery(&path, None);
    let upstream = config.upstreams.get("discovered").expect("upstream parsed");

    let pool = UpstreamPool::new(upstream.clone())
        .await
        .expect("an empty discovery result should not fail pool construction");

    assert_eq!(pool.target_count(), 0);
    assert!(!pool.has_healthy_targets().await);
}

/// `static` discovery cannot change, so it must not be scheduled for refresh.
#[tokio::test]
async fn static_discovery_is_not_scheduled_for_refresh() {
    let kdl = r#"
schema-version "1.0"
system { worker-threads 0 }
listeners { listener "http" { address "127.0.0.1:0" } }
upstreams {
    upstream "fixed" {
        discovery "static" {
            backends "127.0.0.1:19561" "127.0.0.1:19562"
        }
    }
}
routes {
    route "all" { matches { path-prefix "/" } ; upstream "fixed" }
}
"#;
    let config = Config::from_kdl(kdl).expect("static discovery parses");
    let upstream = config.upstreams.get("fixed").expect("upstream parsed");
    let pool = UpstreamPool::new(upstream.clone())
        .await
        .expect("pool builds");

    assert_eq!(pool.target_count(), 2, "static backends become targets");
    assert!(
        pool.discovery_refresh_interval().is_none(),
        "a static list has nothing to refresh"
    );
}

/// Settings that belong to another discovery backend are rejected outright.
/// Accepting and ignoring them is the failure shape this feature exists to
/// remove.
#[test]
fn settings_from_the_wrong_backend_are_rejected() {
    let kdl = r#"
schema-version "1.0"
system { worker-threads 0 }
listeners { listener "http" { address "127.0.0.1:0" } }
upstreams {
    upstream "wrong" {
        discovery "consul" {
            address "http://consul:8500"
            service "api"
            hostname "not-a-consul-setting"
        }
    }
}
routes {
    route "all" { matches { path-prefix "/" } ; upstream "wrong" }
}
"#;
    let message = Config::from_kdl(kdl)
        .expect_err("a consul block must not accept dns settings")
        .to_string();
    assert!(
        message.contains("hostname"),
        "the error should name the offending key, got: {message}"
    );
}
