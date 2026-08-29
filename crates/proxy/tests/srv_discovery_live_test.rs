//! Live SRV resolution against public records.
//!
//! Ignored by default: these hit real DNS, so they do not belong in CI where a
//! network blip would look like a code failure. Run deliberately with:
//!
//! ```bash
//! cargo test -p zentinel-proxy --test srv_discovery_live_test -- --ignored --nocapture
//! ```
//!
//! What they demonstrate that a unit test cannot: the port and weight come from
//! the SRV record itself. The previous implementation reduced
//! `_xmpp-client._tcp.jabber.org` to `jabber.org` and resolved A/AAAA on port
//! 80, which is the wrong host and the wrong port.

use std::time::Duration;

use pingora_load_balancing::discovery::ServiceDiscovery;
use zentinel_proxy::SrvDiscovery;

#[tokio::test]
#[ignore = "requires network access to public DNS"]
async fn resolves_port_and_weight_from_the_record() {
    let discovery = SrvDiscovery::new(
        "_xmpp-client._tcp.jabber.org".to_string(),
        Duration::from_secs(30),
    );

    let (backends, _) = discovery.discover().await.expect("SRV lookup succeeds");
    assert!(!backends.is_empty(), "jabber.org publishes SRV records");

    for backend in &backends {
        let pingora_core::protocols::l4::socket::SocketAddr::Inet(addr) = &backend.addr else {
            panic!("SRV discovery should only produce inet addresses");
        };
        assert_eq!(
            addr.port(),
            5222,
            "the port must come from the SRV record, not a default"
        );
        assert!(
            backend.weight >= 1,
            "a zero SRV weight must not exclude the backend"
        );
    }
}

/// A name with no SRV records must fail rather than quietly resolving something
/// else — the old fallback would have stripped the underscore labels and
/// resolved the bare domain instead.
#[tokio::test]
#[ignore = "requires network access to public DNS"]
async fn a_name_without_srv_records_fails() {
    let discovery = SrvDiscovery::new(
        "_definitely-not-a-service._tcp.example.com".to_string(),
        Duration::from_secs(30),
    );
    assert!(
        discovery.discover().await.is_err(),
        "missing SRV records must be an error, not a silent fallback"
    );
}
