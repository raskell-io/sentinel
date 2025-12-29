//! Server and listener KDL parsing.

use anyhow::Result;
use std::path::PathBuf;
use tracing::trace;

use sentinel_common::types::TraceIdFormat;

use crate::server::*;

use super::helpers::{get_bool_entry, get_first_arg_string, get_int_entry, get_string_entry};

/// Parse server configuration block
pub fn parse_server_config(node: &kdl::KdlNode) -> Result<ServerConfig> {
    trace!("Parsing server configuration block");

    let trace_id_format = get_string_entry(node, "trace-id-format")
        .map(|s| TraceIdFormat::from_str_loose(&s))
        .unwrap_or_default();

    let config = ServerConfig {
        worker_threads: get_int_entry(node, "worker-threads")
            .map(|v| v as usize)
            .unwrap_or_else(default_worker_threads),
        max_connections: get_int_entry(node, "max-connections")
            .map(|v| v as usize)
            .unwrap_or_else(default_max_connections),
        graceful_shutdown_timeout_secs: get_int_entry(node, "graceful-shutdown-timeout-secs")
            .map(|v| v as u64)
            .unwrap_or_else(default_graceful_shutdown_timeout),
        daemon: get_bool_entry(node, "daemon").unwrap_or(false),
        pid_file: get_string_entry(node, "pid-file").map(PathBuf::from),
        user: get_string_entry(node, "user"),
        group: get_string_entry(node, "group"),
        working_directory: get_string_entry(node, "working-directory").map(PathBuf::from),
        trace_id_format,
        auto_reload: get_bool_entry(node, "auto-reload").unwrap_or(false),
    };

    trace!(
        worker_threads = config.worker_threads,
        max_connections = config.max_connections,
        daemon = config.daemon,
        auto_reload = config.auto_reload,
        "Parsed server configuration"
    );

    Ok(config)
}

/// Parse listeners configuration block
pub fn parse_listeners(node: &kdl::KdlNode) -> Result<Vec<ListenerConfig>> {
    trace!("Parsing listeners configuration block");
    let mut listeners = Vec::new();

    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == "listener" {
                let id = get_first_arg_string(child).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Listener requires an ID argument, e.g., listener \"http\" {{ ... }}"
                    )
                })?;

                trace!(listener_id = %id, "Parsing listener");

                let address = get_string_entry(child, "address").ok_or_else(|| {
                    anyhow::anyhow!(
                        "Listener '{}' requires an 'address' field, e.g., address \"0.0.0.0:8080\"",
                        id
                    )
                })?;

                let protocol_str =
                    get_string_entry(child, "protocol").unwrap_or_else(|| "http".to_string());
                let protocol = match protocol_str.to_lowercase().as_str() {
                    "http" => ListenerProtocol::Http,
                    "https" => ListenerProtocol::Https,
                    "h2" => ListenerProtocol::Http2,
                    "h3" => ListenerProtocol::Http3,
                    other => {
                        return Err(anyhow::anyhow!(
                            "Invalid protocol '{}' for listener '{}'. Valid protocols: http, https, h2, h3",
                            other,
                            id
                        ));
                    }
                };

                trace!(
                    listener_id = %id,
                    address = %address,
                    protocol = ?protocol,
                    "Parsed listener"
                );

                listeners.push(ListenerConfig {
                    id,
                    address,
                    protocol,
                    tls: None, // TODO: Parse TLS config
                    default_route: get_string_entry(child, "default-route"),
                    request_timeout_secs: get_int_entry(child, "request-timeout-secs")
                        .map(|v| v as u64)
                        .unwrap_or_else(default_request_timeout),
                    keepalive_timeout_secs: get_int_entry(child, "keepalive-timeout-secs")
                        .map(|v| v as u64)
                        .unwrap_or_else(default_keepalive_timeout),
                    max_concurrent_streams: get_int_entry(child, "max-concurrent-streams")
                        .map(|v| v as u32)
                        .unwrap_or_else(default_max_concurrent_streams),
                });
            }
        }
    }

    trace!(listener_count = listeners.len(), "Finished parsing listeners");
    Ok(listeners)
}
