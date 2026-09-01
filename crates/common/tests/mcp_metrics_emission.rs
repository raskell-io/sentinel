//! What `zentinel_mcp_calls_total` actually emits.
//!
//! The unit tests in `observability` cover the label-bounding function in
//! isolation. This covers the step after it: that the counter is registered,
//! that recording a call increments it, and that the labels reaching Prometheus
//! are the bounded ones rather than whatever the client sent.
//!
//! # What this does not prove
//!
//! It does not prove the proxy's request path ever calls `record_mcp_call`.
//! That call site lives in `http_trait.rs` behind a full proxy service and is
//! only reachable end to end — `tests/integration_test.sh` is where that
//! belongs. A metric that is correct and never incremented looks identical to
//! one that is never emitted, and this file cannot tell them apart.
//!
//! # Why one test rather than several
//!
//! `register_int_counter_vec!` registers into the process-global default
//! registry, so a second `RequestMetrics::new()` in the same process fails as
//! already-registered, and `prometheus::gather()` sees every test's writes.
//! Keeping it to a single test keeps both facts local instead of spread across
//! a shared fixture.

use zentinel_common::observability::RequestMetrics;

/// Find the sample for one label combination in the gathered registry.
fn sample(families: &[prometheus::proto::MetricFamily], labels: &[(&str, &str)]) -> Option<u64> {
    let family = families
        .iter()
        .find(|f| f.get_name() == "zentinel_mcp_calls_total")?;

    family.get_metric().iter().find_map(|m| {
        let matches = labels.iter().all(|(k, v)| {
            m.get_label()
                .iter()
                .any(|l| l.get_name() == *k && l.get_value() == *v)
        });
        matches.then(|| m.get_counter().value() as u64)
    })
}

#[test]
fn mcp_calls_are_counted_with_bounded_labels() {
    let metrics = RequestMetrics::new().expect("metrics register");

    let allowed_tools = vec!["get_weather".to_string()];
    let allowed_methods = vec!["tools/call".to_string()];

    // A declared tool, permitted.
    metrics.record_mcp_call(
        "api",
        RequestMetrics::bounded_mcp_label("tools/call", &allowed_methods),
        RequestMetrics::bounded_mcp_label("get_weather", &allowed_tools),
        "allowed",
    );

    // A tool the route does not declare, refused. The name is client-supplied
    // and must not reach Prometheus.
    metrics.record_mcp_call(
        "api",
        RequestMetrics::bounded_mcp_label("tools/call", &allowed_methods),
        RequestMetrics::bounded_mcp_label("delete_everything", &allowed_tools),
        "denied",
    );

    // Two more undeclared names. If the bound were missing these would create
    // two further series; with it they land on the same one.
    for name in ["../../etc/passwd", "wipe_database"] {
        metrics.record_mcp_call(
            "api",
            RequestMetrics::bounded_mcp_label("tools/call", &allowed_methods),
            RequestMetrics::bounded_mcp_label(name, &allowed_tools),
            "denied",
        );
    }

    let families = prometheus::gather();

    assert_eq!(
        sample(
            &families,
            &[
                ("route", "api"),
                ("method", "tools/call"),
                ("target", "get_weather"),
                ("decision", "allowed"),
            ]
        ),
        Some(1),
        "a declared tool should be counted under its own name"
    );

    assert_eq!(
        sample(
            &families,
            &[
                ("route", "api"),
                ("method", "tools/call"),
                ("target", RequestMetrics::MCP_TARGET_OTHER),
                ("decision", "denied"),
            ]
        ),
        Some(3),
        "three undeclared names should collapse onto one series, not three"
    );

    // The names the client sent must appear nowhere in the exported labels.
    let family = families
        .iter()
        .find(|f| f.get_name() == "zentinel_mcp_calls_total")
        .expect("metric family registered");

    for leaked in ["delete_everything", "../../etc/passwd", "wipe_database"] {
        assert!(
            !family
                .get_metric()
                .iter()
                .any(|m| m.get_label().iter().any(|l| l.get_value() == leaked)),
            "client-supplied name {leaked:?} reached Prometheus as a label"
        );
    }

    // Bounded, so the whole route produces one series per decision at most.
    let series_for_route = family
        .get_metric()
        .iter()
        .filter(|m| {
            m.get_label()
                .iter()
                .any(|l| l.get_name() == "route" && l.get_value() == "api")
        })
        .count();
    assert_eq!(
        series_for_route, 2,
        "one series per decision; got {series_for_route}"
    );
}
