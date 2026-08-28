//! Periodic re-resolution of upstream service discovery.
//!
//! Pools whose targets come from a discovery source (an upstream's `discovery`
//! block) are resolved once while the proxy starts and then re-resolved on the
//! interval their source declares. This module owns that schedule.
//!
//! # Why one supervisor rather than a task per upstream
//!
//! Pools are replaced wholesale on `SIGHUP`: `Registry::replace` swaps the
//! whole map for one built from the new configuration. A task holding an
//! `Arc<UpstreamPool>` would keep refreshing the pool it captured and write its
//! results back over the pool the reload just installed, so every reload would
//! need to find and abort the old tasks and spawn new ones — with the pools and
//! the tasks tracked separately and able to drift apart.
//!
//! A single supervisor reads the registry on each tick instead. It therefore
//! sees exactly the pools that are installed right now: reloaded pools are
//! picked up automatically, removed ones simply stop appearing, and there is no
//! task lifecycle to keep in step with the pool lifecycle.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use prometheus::{register_int_counter_vec, register_int_gauge_vec, IntCounterVec, IntGaugeVec};
use tracing::{debug, info, warn};

use zentinel_common::{Registry, ScopedRegistry};

use super::UpstreamPool;

/// How often the supervisor wakes to check which pools are due.
///
/// Refresh intervals are configured in whole seconds, so a one-second tick
/// resolves every interval exactly. The tick itself does no work beyond
/// comparing deadlines when nothing is due.
const TICK: Duration = Duration::from_secs(1);

/// Upper bound on a single refresh.
///
/// A registry that accepts a connection and then never answers would otherwise
/// hold its slot forever. The refresh is abandoned and retried on the next
/// interval; the pool keeps serving its current targets meanwhile.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(10);

static DISCOVERY_REFRESHES: LazyLock<Option<IntCounterVec>> = LazyLock::new(|| {
    register_int_counter_vec!(
        "zentinel_upstream_discovery_refreshes_total",
        "Service discovery refreshes that changed an upstream's target set",
        &["upstream"]
    )
    .ok()
});

static DISCOVERY_TARGETS: LazyLock<Option<IntGaugeVec>> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "zentinel_upstream_discovery_targets",
        "Targets currently resolved for an upstream backed by service discovery",
        &["upstream"]
    )
    .ok()
});

/// Start the refresh supervisor.
///
/// Returns immediately; the supervisor runs until the process exits. It is a
/// no-op for pools without a discovery source, so it is safe to start
/// unconditionally.
pub(crate) fn spawn(global: Registry<UpstreamPool>, scoped: ScopedRegistry<UpstreamPool>) {
    tokio::spawn(async move {
        run(global, scoped).await;
    });
}

/// Which registry a pool came from, so a refreshed pool goes back where it
/// belongs. Both are keyed by a string id; only the write method differs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Source {
    Global,
    Scoped,
}

async fn run(global: Registry<UpstreamPool>, scoped: ScopedRegistry<UpstreamPool>) {
    // When each pool is next due. Keyed by source and id so a namespaced
    // upstream and a global one of the same name keep separate schedules.
    let mut due: HashMap<(Source, String), Instant> = HashMap::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        let now = Instant::now();

        let mut live: Vec<(Source, String, Arc<UpstreamPool>, Duration)> = Vec::new();
        for (id, pool) in global.snapshot().await {
            if let Some(interval) = pool.discovery_refresh_interval() {
                live.push((Source::Global, id, pool, interval));
            }
        }
        for (id, pool) in scoped.snapshot().await {
            if let Some(interval) = pool.discovery_refresh_interval() {
                live.push((Source::Scoped, id, pool, interval));
            }
        }

        // Drop schedules for pools that are no longer registered, so the map
        // cannot grow across reloads that rename or remove upstreams.
        let present: std::collections::HashSet<(Source, String)> =
            live.iter().map(|(s, id, _, _)| (*s, id.clone())).collect();
        due.retain(|key, _| present.contains(key));

        for (source, id, pool, interval) in live {
            let key = (source, id.clone());

            // First sighting: the pool was resolved when it was built, so the
            // first refresh is one full interval away rather than immediate.
            let deadline = *due.entry(key.clone()).or_insert(now + interval);
            if now < deadline {
                continue;
            }

            // Re-arm before spawning so a refresh that outlives its interval
            // cannot have a second one started underneath it.
            due.insert(key, now + interval);

            let global = global.clone();
            let scoped = scoped.clone();
            tokio::spawn(async move {
                refresh_one(source, id, pool, global, scoped).await;
            });
        }
    }
}

/// Re-resolve one pool and install the replacement, if the targets changed.
async fn refresh_one(
    source: Source,
    id: String,
    pool: Arc<UpstreamPool>,
    global: Registry<UpstreamPool>,
    scoped: ScopedRegistry<UpstreamPool>,
) {
    let refreshed = match tokio::time::timeout(REFRESH_TIMEOUT, pool.refreshed()).await {
        Ok(result) => result,
        Err(_) => {
            warn!(
                upstream_id = %id,
                timeout_secs = REFRESH_TIMEOUT.as_secs(),
                "Service discovery refresh timed out; keeping current targets"
            );
            return;
        }
    };

    // `None` means nothing changed, or the source could not be reached and the
    // pool kept its targets. Either way there is nothing to install.
    let Some(new_pool) = refreshed else {
        debug!(upstream_id = %id, "Service discovery refresh: no change");
        return;
    };

    let target_count = new_pool.target_count();
    let new_pool = Arc::new(new_pool);

    let installed = match source {
        Source::Global => global.insert(id.clone(), new_pool).await.is_some(),
        Source::Scoped => scoped.replace_item(&id, new_pool).await.is_some(),
    };

    if !installed {
        // The upstream was removed (or renamed) by a reload while this refresh
        // was in flight. Dropping the result is correct: the pool it was built
        // from is gone.
        debug!(
            upstream_id = %id,
            "Upstream disappeared during discovery refresh; discarding result"
        );
        return;
    }

    if let Some(counter) = DISCOVERY_REFRESHES.as_ref() {
        counter.with_label_values(&[&id]).inc();
    }
    if let Some(gauge) = DISCOVERY_TARGETS.as_ref() {
        gauge.with_label_values(&[&id]).set(target_count as i64);
    }

    info!(
        upstream_id = %id,
        target_count,
        "Installed refreshed upstream pool"
    );
}
