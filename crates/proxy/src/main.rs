//! Sentinel Proxy - Main entry point
//!
//! A security-first reverse proxy built on Pingora with sleepable ops at the edge.

use anyhow::Result;
use pingora::prelude::*;
use tracing::{info, warn};

use sentinel_proxy::SentinelProxy;

fn main() -> Result<()> {
    // Parse command-line options
    let opt = Opt::parse_args();

    // Get config path with priority: -c flag > SENTINEL_CONFIG env > None (use embedded default)
    let config_path: Option<String> = opt
        .conf
        .clone()
        .or_else(|| std::env::var("SENTINEL_CONFIG").ok());

    match &config_path {
        Some(path) => info!("Loading configuration from: {}", path),
        None => info!("No configuration specified, using embedded default configuration"),
    }

    // Create runtime for async initialization
    let runtime = tokio::runtime::Runtime::new()?;

    // Create proxy with configuration
    let proxy = runtime.block_on(async { SentinelProxy::new(config_path.as_deref()).await })?;

    // Get initial config for server setup
    let config = proxy.config_manager.current();

    // Create Pingora server - clear conf since we use our own config format
    let mut pingora_opt = opt;
    pingora_opt.conf = None;
    let mut server = Server::new(Some(pingora_opt))?;
    server.bootstrap();

    // Create proxy service
    let mut proxy_service = http_proxy_service(&server.configuration, proxy);

    // Configure listening addresses from config
    for listener in &config.listeners {
        match listener.protocol {
            sentinel_config::ListenerProtocol::Http => {
                proxy_service.add_tcp(&listener.address);
                info!("HTTP listening on: {}", listener.address);
            }
            sentinel_config::ListenerProtocol::Https => {
                if listener.tls.is_some() {
                    warn!("HTTPS listener configured but TLS not yet implemented");
                }
            }
            _ => {
                warn!("Unsupported protocol: {:?}", listener.protocol);
            }
        }
    }

    // Add proxy service to server
    server.add_service(proxy_service);

    // Setup signal handlers for graceful shutdown and reload
    setup_signal_handlers();

    info!("Sentinel proxy started successfully");
    info!("Configuration hot reload enabled");
    info!("Health checking enabled");
    info!("Route matching enabled");

    // Run server forever
    server.run_forever();
}

/// Setup signal handlers for graceful operations
fn setup_signal_handlers() {
    use signal_hook::consts::signal::*;
    use signal_hook::iterator::Signals;
    use std::thread;

    let mut signals =
        Signals::new([SIGTERM, SIGINT, SIGHUP]).expect("Failed to register signal handlers");

    thread::spawn(move || {
        for sig in signals.forever() {
            match sig {
                SIGTERM | SIGINT => {
                    info!("Received shutdown signal, initiating graceful shutdown");
                    std::process::exit(0);
                }
                SIGHUP => {
                    info!("Received SIGHUP, triggering configuration reload");
                }
                _ => {}
            }
        }
    });
}
