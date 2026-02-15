//! Temporary HTTP challenge server for initial ACME certificate acquisition
//!
//! During startup, when no certificate exists yet, Pingora cannot bind an
//! HTTPS listener. This module provides a minimal HTTP server that handles
//! only `/.well-known/acme-challenge/<token>` requests for HTTP-01 validation.
//!
//! The server is started before the main proxy, used to complete the initial
//! ACME challenge, and then shut down once certificates are obtained.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use super::challenge::ChallengeManager;
use super::error::AcmeError;

/// Maximum request size to read (8 KB is plenty for challenge requests)
const MAX_REQUEST_SIZE: usize = 8192;

/// ACME challenge path prefix
const CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

/// Run a temporary HTTP server for ACME HTTP-01 challenge validation
///
/// This server only handles `GET /.well-known/acme-challenge/<token>` requests,
/// responding with the key authorization from the challenge manager. All other
/// requests receive a 404 response.
///
/// The server shuts down when the `shutdown` watch channel receives `true`.
///
/// # Arguments
///
/// * `addr` - Socket address to bind to (e.g., "0.0.0.0:80")
/// * `challenge_manager` - Challenge manager containing pending token/key-auth pairs
/// * `shutdown` - Watch channel receiver; server stops when value becomes `true`
///
/// # Errors
///
/// Returns an error if the TCP listener cannot be bound.
pub async fn run_challenge_server(
    addr: &str,
    challenge_manager: Arc<ChallengeManager>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), AcmeError> {
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        AcmeError::Protocol(format!(
            "Failed to bind ACME challenge server on {}: {}",
            addr, e
        ))
    })?;

    info!(
        address = %addr,
        "ACME challenge server started"
    );

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((mut stream, peer)) => {
                        let cm = Arc::clone(&challenge_manager);
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(&mut stream, &cm).await {
                                debug!(
                                    peer = %peer,
                                    error = %e,
                                    "Challenge server connection error"
                                );
                            }
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "Challenge server accept error");
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("ACME challenge server shutting down");
                    return Ok(());
                }
            }
        }
    }
}

/// Handle a single HTTP connection on the challenge server
async fn handle_connection(
    stream: &mut tokio::net::TcpStream,
    challenge_manager: &ChallengeManager,
) -> Result<(), AcmeError> {
    let mut buf = vec![0u8; MAX_REQUEST_SIZE];
    let n = stream.read(&mut buf).await.map_err(|e| {
        AcmeError::Protocol(format!("Failed to read from challenge connection: {}", e))
    })?;

    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the request line (e.g., "GET /.well-known/acme-challenge/token HTTP/1.1\r\n...")
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1));

    let response = match path {
        Some(p) if p.starts_with(CHALLENGE_PREFIX) => {
            let token = &p[CHALLENGE_PREFIX.len()..];
            match challenge_manager.get_response(token) {
                Some(key_auth) => {
                    debug!(token = %token, "Challenge server serving token response");
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        key_auth.len(),
                        key_auth
                    )
                }
                None => {
                    debug!(token = %token, "Challenge server token not found");
                    http_404()
                }
            }
        }
        _ => http_404(),
    };

    stream.write_all(response.as_bytes()).await.map_err(|e| {
        AcmeError::Protocol(format!("Failed to write challenge response: {}", e))
    })?;

    Ok(())
}

/// Build a minimal 404 response
fn http_404() -> String {
    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn challenge_server_serves_registered_token() {
        let cm = Arc::new(ChallengeManager::new());
        cm.add_challenge("test-token-123", "key-auth-value-abc");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Bind to a random port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let addr_str = addr.to_string();
        let cm_clone = Arc::clone(&cm);
        let server_handle = tokio::spawn(async move {
            run_challenge_server(&addr_str, cm_clone, shutdown_rx).await
        });

        // Give server time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send a challenge request
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = "GET /.well-known/acme-challenge/test-token-123 HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut response = vec![0u8; 4096];
        let n = stream.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);

        assert!(response_str.starts_with("HTTP/1.1 200 OK"));
        assert!(response_str.contains("key-auth-value-abc"));

        // Shutdown
        shutdown_tx.send(true).unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server_handle).await;
    }

    #[tokio::test]
    async fn challenge_server_returns_404_for_unknown_token() {
        let cm = Arc::new(ChallengeManager::new());

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let addr_str = addr.to_string();
        let cm_clone = Arc::clone(&cm);
        let server_handle = tokio::spawn(async move {
            run_challenge_server(&addr_str, cm_clone, shutdown_rx).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = "GET /.well-known/acme-challenge/unknown-token HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut response = vec![0u8; 4096];
        let n = stream.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);

        assert!(response_str.starts_with("HTTP/1.1 404 Not Found"));

        shutdown_tx.send(true).unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server_handle).await;
    }

    #[tokio::test]
    async fn challenge_server_returns_404_for_non_challenge_path() {
        let cm = Arc::new(ChallengeManager::new());

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let addr_str = addr.to_string();
        let cm_clone = Arc::clone(&cm);
        let server_handle = tokio::spawn(async move {
            run_challenge_server(&addr_str, cm_clone, shutdown_rx).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = "GET /some/other/path HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut response = vec![0u8; 4096];
        let n = stream.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);

        assert!(response_str.starts_with("HTTP/1.1 404 Not Found"));

        shutdown_tx.send(true).unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server_handle).await;
    }
}
