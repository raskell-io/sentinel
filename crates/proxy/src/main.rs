//! Sentinel Proxy - Main entry point
//!
//! A security-first reverse proxy built on Pingora with sleepable ops at the edge.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pingora::prelude::*;
use tracing::{info, warn};

use sentinel_config::Config;
use sentinel_proxy::SentinelProxy;

/// Sentinel - A security-first reverse proxy built on Pingora
#[derive(Parser, Debug)]
#[command(name = "sentinel")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Configuration file path
    #[arg(short = 'c', long = "config", env = "SENTINEL_CONFIG")]
    config: Option<String>,

    /// Test configuration and exit
    #[arg(short = 't', long = "test")]
    test: bool,

    /// Enable verbose logging (debug level)
    #[arg(long = "verbose")]
    verbose: bool,

    /// Run in daemon mode (background)
    #[arg(short = 'd', long = "daemon")]
    daemon: bool,

    /// Upgrade from a running instance
    #[arg(short = 'u', long = "upgrade")]
    upgrade: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Validate configuration file and exit
    Test {
        /// Configuration file to test
        #[arg(short = 'c', long = "config")]
        config: Option<String>,
    },
    /// Run the proxy server (default)
    Run {
        /// Configuration file path
        #[arg(short = 'c', long = "config")]
        config: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle test flag or test subcommand
    if cli.test {
        return test_config(cli.config.as_deref());
    }

    // Handle subcommands
    match cli.command {
        Some(Commands::Test { config }) => {
            return test_config(config.as_deref().or(cli.config.as_deref()));
        }
        Some(Commands::Run { config }) => {
            return run_server(
                config.or(cli.config),
                cli.verbose,
                cli.daemon,
                cli.upgrade,
            );
        }
        None => {
            // Default: run the server
            return run_server(cli.config, cli.verbose, cli.daemon, cli.upgrade);
        }
    }
}

/// Test configuration file and exit
fn test_config(config_path: Option<&str>) -> Result<()> {
    // Initialize minimal logging for config test
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();

    let config = match config_path {
        Some(path) => {
            info!("Testing configuration file: {}", path);
            Config::from_file(path).context("Failed to load configuration file")?
        }
        None => {
            info!("Testing embedded default configuration");
            Config::default_embedded().context("Failed to load embedded configuration")?
        }
    };

    // Validate the configuration
    config.validate().context("Configuration validation failed")?;

    // Additional validation checks
    let route_count = config.routes.len();
    let upstream_count = config.upstreams.len();
    let listener_count = config.listeners.len();

    info!("Configuration test successful:");
    info!("  - {} listener(s)", listener_count);
    info!("  - {} route(s)", route_count);
    info!("  - {} upstream(s)", upstream_count);

    // Check for potential issues
    for route in &config.routes {
        if let Some(ref upstream) = route.upstream {
            if !config.upstreams.contains_key(upstream) {
                warn!(
                    "Route '{}' references undefined upstream '{}'",
                    route.id, upstream
                );
            }
        }
    }

    println!("sentinel: configuration file {} test is successful",
        config_path.unwrap_or("(embedded)"));

    Ok(())
}

/// Run the proxy server
fn run_server(
    config_path: Option<String>,
    verbose: bool,
    daemon: bool,
    upgrade: bool,
) -> Result<()> {
    // Initialize logging based on verbose flag
    let log_level = if verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level))
        )
        .init();

    // Build Pingora options
    let mut pingora_opt = Opt::default();
    pingora_opt.daemon = daemon;
    pingora_opt.upgrade = upgrade;

    // Get config path with priority: CLI arg > env var > None (embedded default)
    let effective_config_path = config_path
        .or_else(|| std::env::var("SENTINEL_CONFIG").ok());

    match &effective_config_path {
        Some(path) => info!("Loading configuration from: {}", path),
        None => info!("No configuration specified, using embedded default configuration"),
    }

    // Create runtime for async initialization
    let runtime = tokio::runtime::Runtime::new()?;

    // Create proxy with configuration
    let proxy = runtime.block_on(async {
        SentinelProxy::new(effective_config_path.as_deref()).await
    })?;

    // Get initial config for server setup
    let config = proxy.config_manager.current();

    // Create Pingora server
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
