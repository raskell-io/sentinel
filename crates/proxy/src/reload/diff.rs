//! Incremental configuration changes.
//!
//! A full reload replaces every route, upstream and listener at once. That is
//! the right model for a config file, and the wrong one for "add a backend to
//! this pool": it re-reads a file that may have drifted, re-validates
//! everything, and lets an unrelated typo take down routes that were working.
//!
//! A [`ConfigChange`] names one mutation. Applying it clones the live
//! configuration, makes that single change, and hands the result to
//! [`super::ConfigManager::apply_config`], which already validates, swaps
//! atomically, emits events and rolls back on failure. Nothing here
//! re-implements any of
//! that — the value is in describing the change precisely and refusing the
//! ones that do not make sense.
//!
//! Every change reports what it did. A removal that matched nothing is an
//! error, not a quiet success: an operator asking to remove a backend and
//! being told "done" when no such backend existed has been misinformed about
//! the state of their proxy.
//!
//! Part of zentinelproxy/zentinel#127.

use zentinel_common::errors::{ZentinelError, ZentinelResult};
use zentinel_config::{Config, RouteConfig, UpstreamTarget};

/// Build a configuration error with no underlying cause.
fn config_error(message: String) -> ZentinelError {
    ZentinelError::Config {
        message,
        source: None,
    }
}

/// A single, atomic configuration change.
#[derive(Debug, Clone)]
pub enum ConfigChange {
    /// Add a target to an existing upstream pool.
    AddUpstreamTarget {
        /// Upstream to add to. Must already exist.
        upstream: String,
        /// The target to add. Its address must not already be present.
        target: UpstreamTarget,
    },
    /// Remove a target from an upstream pool by address.
    RemoveUpstreamTarget {
        /// Upstream to remove from.
        upstream: String,
        /// Address of the target to remove.
        address: String,
    },
    /// Add a route. Its ID must not already exist.
    AddRoute(Box<RouteConfig>),
    /// Replace an existing route wholesale, matched by ID.
    ReplaceRoute(Box<RouteConfig>),
    /// Remove a route by ID.
    RemoveRoute {
        /// ID of the route to remove.
        id: String,
    },
}

impl ConfigChange {
    /// A short description for logs and audit trails.
    pub fn summary(&self) -> String {
        match self {
            ConfigChange::AddUpstreamTarget { upstream, target } => {
                format!("add target {} to upstream '{}'", target.address, upstream)
            }
            ConfigChange::RemoveUpstreamTarget { upstream, address } => {
                format!("remove target {address} from upstream '{upstream}'")
            }
            ConfigChange::AddRoute(route) => format!("add route '{}'", route.id),
            ConfigChange::ReplaceRoute(route) => format!("replace route '{}'", route.id),
            ConfigChange::RemoveRoute { id } => format!("remove route '{id}'"),
        }
    }

    /// Apply this change to a configuration.
    ///
    /// Returns an error rather than mutating when the change does not apply:
    /// adding something that exists, or removing something that does not. Both
    /// are cases where succeeding would tell the caller their configuration is
    /// in a state it is not.
    pub fn apply_to(&self, config: &mut Config) -> ZentinelResult<()> {
        match self {
            ConfigChange::AddUpstreamTarget { upstream, target } => {
                let pool = config.upstreams.get_mut(upstream).ok_or_else(|| {
                    config_error(format!(
                        "cannot add a target: upstream '{upstream}' does not exist"
                    ))
                })?;

                if pool.targets.iter().any(|t| t.address == target.address) {
                    return Err(config_error(format!(
                        "upstream '{}' already has a target at {}",
                        upstream, target.address
                    )));
                }

                pool.targets.push(target.clone());
                Ok(())
            }

            ConfigChange::RemoveUpstreamTarget { upstream, address } => {
                let pool = config.upstreams.get_mut(upstream).ok_or_else(|| {
                    config_error(format!(
                        "cannot remove a target: upstream '{upstream}' does not exist"
                    ))
                })?;

                // Both checks run before anything is removed. Retaining first
                // and validating after would leave the pool emptied even when
                // the change is rejected.
                let Some(position) = pool.targets.iter().position(|t| &t.address == address) else {
                    return Err(config_error(format!(
                        "upstream '{upstream}' has no target at {address}"
                    )));
                };

                // An upstream with no targets accepts requests it can never
                // route. Refuse rather than let a pool be emptied one target
                // at a time without anyone noticing.
                if pool.targets.len() == 1 {
                    return Err(config_error(format!(
                        "removing {address} would leave upstream '{upstream}' with no targets; \
                         remove the routes that use it first, or replace the target"
                    )));
                }

                pool.targets.remove(position);
                Ok(())
            }

            ConfigChange::AddRoute(route) => {
                if config.routes.iter().any(|r| r.id == route.id) {
                    return Err(config_error(format!(
                        "route '{}' already exists; use ReplaceRoute to change it",
                        route.id
                    )));
                }
                config.routes.push((**route).clone());
                Ok(())
            }

            ConfigChange::ReplaceRoute(route) => {
                let existing = config
                    .routes
                    .iter_mut()
                    .find(|r| r.id == route.id)
                    .ok_or_else(|| {
                        config_error(format!(
                            "cannot replace route '{}': it does not exist",
                            route.id
                        ))
                    })?;
                *existing = (**route).clone();
                Ok(())
            }

            ConfigChange::RemoveRoute { id } => {
                let before = config.routes.len();
                config.routes.retain(|r| &r.id != id);

                if config.routes.len() == before {
                    return Err(config_error(format!("route '{id}' does not exist")));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_pool() -> Config {
        let mut config = Config::default_for_testing();
        for pool in config.upstreams.values_mut() {
            pool.targets = vec![target("10.0.0.1:8080"), target("10.0.0.2:8080")];
        }
        config
    }

    fn a_pool_name(config: &Config) -> String {
        config.upstreams.keys().next().unwrap().clone()
    }

    // Fields spelled out rather than `..Default::default()`, so adding a
    // field to UpstreamTarget breaks this test instead of silently defaulting.
    fn target(address: &str) -> UpstreamTarget {
        UpstreamTarget {
            address: address.to_string(),
            weight: 1,
            max_requests: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn adding_a_target_appends_it() {
        let mut config = config_with_pool();
        let pool = a_pool_name(&config);

        ConfigChange::AddUpstreamTarget {
            upstream: pool.clone(),
            target: target("10.0.0.3:8080"),
        }
        .apply_to(&mut config)
        .expect("should apply");

        let addresses: Vec<_> = config.upstreams[&pool]
            .targets
            .iter()
            .map(|t| t.address.as_str())
            .collect();
        assert!(addresses.contains(&"10.0.0.3:8080"));
        assert_eq!(addresses.len(), 3);
    }

    /// Adding a duplicate would silently create a pool that sends twice the
    /// share of traffic to one backend.
    #[test]
    fn adding_a_duplicate_target_is_refused() {
        let mut config = config_with_pool();
        let pool = a_pool_name(&config);

        let err = ConfigChange::AddUpstreamTarget {
            upstream: pool,
            target: target("10.0.0.1:8080"),
        }
        .apply_to(&mut config)
        .expect_err("a duplicate address should be refused");

        assert!(err.to_string().contains("already has a target"));
    }

    #[test]
    fn adding_to_an_unknown_upstream_is_refused() {
        let mut config = config_with_pool();
        let err = ConfigChange::AddUpstreamTarget {
            upstream: "nonexistent".to_string(),
            target: target("10.0.0.9:8080"),
        }
        .apply_to(&mut config)
        .expect_err("should be refused");
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn removing_a_target_removes_only_that_one() {
        let mut config = config_with_pool();
        let pool = a_pool_name(&config);

        ConfigChange::RemoveUpstreamTarget {
            upstream: pool.clone(),
            address: "10.0.0.1:8080".to_string(),
        }
        .apply_to(&mut config)
        .expect("should apply");

        let addresses: Vec<_> = config.upstreams[&pool]
            .targets
            .iter()
            .map(|t| t.address.as_str())
            .collect();
        assert_eq!(addresses, vec!["10.0.0.2:8080"]);
    }

    /// The case this whole module is careful about: an operator asks to remove
    /// a backend that is not there. Reporting success would tell them their
    /// proxy is in a state it is not in.
    #[test]
    fn removing_an_absent_target_is_an_error_not_a_quiet_success() {
        let mut config = config_with_pool();
        let pool = a_pool_name(&config);

        let err = ConfigChange::RemoveUpstreamTarget {
            upstream: pool,
            address: "10.9.9.9:8080".to_string(),
        }
        .apply_to(&mut config)
        .expect_err("removing something absent must not report success");

        assert!(err.to_string().contains("has no target at"));
    }

    /// An upstream with no targets accepts requests it can never route, so the
    /// last one cannot be removed by this path.
    #[test]
    fn emptying_a_pool_is_refused() {
        let mut config = config_with_pool();
        let pool = a_pool_name(&config);

        ConfigChange::RemoveUpstreamTarget {
            upstream: pool.clone(),
            address: "10.0.0.1:8080".to_string(),
        }
        .apply_to(&mut config)
        .expect("first removal is fine");

        let err = ConfigChange::RemoveUpstreamTarget {
            upstream: pool.clone(),
            address: "10.0.0.2:8080".to_string(),
        }
        .apply_to(&mut config)
        .expect_err("removing the last target should be refused");

        assert!(err.to_string().contains("no targets"));
        // And the pool is left as it was, not emptied.
        assert_eq!(config.upstreams[&pool].targets.len(), 1);
    }

    #[test]
    fn routes_can_be_added_replaced_and_removed() {
        let mut config = Config::default_for_testing();
        let mut route = config.routes.first().cloned().expect("a route to copy");
        route.id = "new-route".to_string();

        ConfigChange::AddRoute(Box::new(route.clone()))
            .apply_to(&mut config)
            .expect("add");
        assert!(config.routes.iter().any(|r| r.id == "new-route"));

        let mut updated = route.clone();
        updated.upstream = Some("changed".to_string());
        ConfigChange::ReplaceRoute(Box::new(updated))
            .apply_to(&mut config)
            .expect("replace");
        let stored = config.routes.iter().find(|r| r.id == "new-route").unwrap();
        assert_eq!(stored.upstream.as_deref(), Some("changed"));

        ConfigChange::RemoveRoute {
            id: "new-route".to_string(),
        }
        .apply_to(&mut config)
        .expect("remove");
        assert!(!config.routes.iter().any(|r| r.id == "new-route"));
    }

    #[test]
    fn adding_a_duplicate_route_id_is_refused() {
        let mut config = Config::default_for_testing();
        let existing = config.routes.first().cloned().expect("a route");

        let err = ConfigChange::AddRoute(Box::new(existing))
            .apply_to(&mut config)
            .expect_err("a duplicate id should be refused");
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn replacing_or_removing_an_unknown_route_is_refused() {
        let mut config = Config::default_for_testing();
        let mut route = config.routes.first().cloned().expect("a route");
        route.id = "not-present".to_string();

        assert!(ConfigChange::ReplaceRoute(Box::new(route))
            .apply_to(&mut config)
            .is_err());
        assert!(ConfigChange::RemoveRoute {
            id: "not-present".to_string()
        }
        .apply_to(&mut config)
        .is_err());
    }

    /// A rejected change must leave the configuration untouched, or a failed
    /// command would still have altered the proxy.
    #[test]
    fn a_rejected_change_does_not_mutate_the_configuration() {
        let mut config = config_with_pool();
        let pool = a_pool_name(&config);
        let before = config.upstreams[&pool].targets.len();
        let routes_before = config.routes.len();

        let _ = ConfigChange::AddUpstreamTarget {
            upstream: pool.clone(),
            target: target("10.0.0.1:8080"),
        }
        .apply_to(&mut config);
        let _ = ConfigChange::RemoveRoute {
            id: "nope".to_string(),
        }
        .apply_to(&mut config);

        assert_eq!(config.upstreams[&pool].targets.len(), before);
        assert_eq!(config.routes.len(), routes_before);
    }
}
