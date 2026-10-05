// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::error::code::ErrorCode;
use crate::modules::settings::proxy::Proxy;
use crate::modules::utils::tls::establish_tls_stream;
use crate::modules::{error::RustMailerResult, imap::session::SessionStream};
use crate::raise_error;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_io_timeout::TimeoutStream;
use tokio_socks::tcp::Socks5Stream;
use tracing::error;

/// Socket-level connect/read/write timeout for IMAP connections. This is the
/// last-resort backstop: command sequences already carry their own (shorter)
/// timeout in the executor. Previously 3600s, which let a stalled or throttled
/// server pin a pool connection for a full hour (INCIDENT-6717207556896619).
pub(crate) const TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) async fn establish_tcp_connection_with_timeout(
    address: SocketAddr,
    use_proxy: Option<u64>,
) -> RustMailerResult<Pin<Box<TimeoutStream<TcpStream>>>> {
    // Establish the TCP connection with a timeout
    let tcp_stream = connect_with_optional_proxy(use_proxy, address).await?;

    // Disable Nagle's algorithm for more efficient network communication
    tcp_stream
        .set_nodelay(true)
        .map_err(|e| raise_error!(e.to_string(), ErrorCode::NetworkError))?;

    // Wrap the TCP stream in a TimeoutStream for timeout management
    let mut timeout_stream = TimeoutStream::new(tcp_stream);

    // Set read and write timeouts
    timeout_stream.set_write_timeout(Some(TIMEOUT));
    timeout_stream.set_read_timeout(Some(TIMEOUT));

    // Return the timeout-wrapped TCP stream as a Pin
    Ok(Box::pin(timeout_stream))
}

pub(crate) async fn establish_tls_connection(
    address: SocketAddr,
    server_hostname: &str,
    alpn_protocols: &[&str],
    use_proxy: Option<u64>,
) -> RustMailerResult<impl SessionStream> {
    // Establish the TCP connection with timeout
    let tcp_stream = establish_tcp_connection_with_timeout(address, use_proxy).await?;

    // Wrap the TCP stream with TLS encryption
    let tls_stream = establish_tls_stream(server_hostname, alpn_protocols, tcp_stream).await?;

    // Return the TLS stream wrapped in a SessionStream
    Ok(tls_stream)
}

pub fn parse_proxy_addr(input: &str) -> RustMailerResult<SocketAddr> {
    // Normalize and check protocol prefix
    let (scheme, stripped) = if let Some(rest) = input
        .strip_prefix("socks5://")
        .or_else(|| input.strip_prefix("SOCKS5://"))
        .or_else(|| input.strip_prefix("Socks5://"))
    {
        ("socks5", rest)
    } else if let Some(rest) = input
        .strip_prefix("http://")
        .or_else(|| input.strip_prefix("HTTP://"))
        .or_else(|| input.strip_prefix("Http://"))
    {
        ("http", rest)
    } else {
        return Err(raise_error!(
            format!(
                "Invalid proxy URL: must start with 'http://' or 'socks5://', got '{}'",
                input
            ),
            ErrorCode::InvalidParameter
        ));
    };

    // Parse the remaining address
    let addr = stripped.parse::<SocketAddr>().map_err(|e| {
        raise_error!(
            format!(
                "Failed to parse {} proxy address '{}': {}",
                scheme, stripped, e
            ),
            ErrorCode::InvalidParameter
        )
    })?;

    Ok(addr)
}

/// Try to connect via SOCKS5 proxy or TCP with timeout
async fn connect_with_optional_proxy(
    use_proxy: Option<u64>,
    address: SocketAddr,
) -> RustMailerResult<TcpStream> {
    // Try if proxy is enabled
    if let Some(proxy_id) = use_proxy {
        let proxy = Proxy::get(proxy_id).await?;
        let proxy = parse_proxy_addr(&proxy.url)?;
        return timeout(TIMEOUT, Socks5Stream::connect(proxy, address))
            .await
            .map_err(|_| {
                error!(
                    "SOCKS5 proxy connection to {} via {} timed out after {}s",
                    address,
                    proxy,
                    TIMEOUT.as_secs()
                );
                raise_error!(
                    format!(
                        "SOCKS5 proxy connection to {} via {} timed out after {}s",
                        address,
                        proxy,
                        TIMEOUT.as_secs()
                    ),
                    ErrorCode::ConnectionTimeout
                )
            })?
            .map(|s| s.into_inner())
            .map_err(|e| raise_error!(e.to_string(), ErrorCode::NetworkError));
    }
    // Fallback to direct TCP connection
    timeout(TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| {
            error!(
                "TCP connection to {} timed out after {}s",
                address,
                TIMEOUT.as_secs()
            );
            raise_error!(
                format!(
                    "TCP connection to {} timed out after {}s",
                    address,
                    TIMEOUT.as_secs()
                ),
                ErrorCode::ConnectionTimeout
            )
        })?
        .map_err(|e| raise_error!(e.to_string(), ErrorCode::NetworkError))
}
