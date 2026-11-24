//! Synchronous zero-dependency RFC 6455 WebSocket client over [`std::net::TcpStream`].

use std::io::{self, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::url::Url;
use crate::ws::error::{map_io_err, Error, Result};
use crate::ws::handshake::{generate_key, read_server_response, write_client_request};
use crate::ws::socket::{Config, Role, WebSocket};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Connects to a `ws://` WebSocket endpoint using default settings.
///
/// # Errors
///
/// Returns [`Error::InvalidUrl`] for malformed URLs, [`Error::UnsupportedScheme`] for
/// `wss://` or non-WebSocket schemes, [`Error::Handshake`] / [`Error::HandshakeStatus`]
/// if the server rejects or fails the upgrade, or [`Error::Io`] / [`Error::TimedOut`].
pub fn connect(url: &str) -> Result<WebSocket<TcpStream>> {
    ClientBuilder::new().connect(url)
}

/// Builder for customizing outgoing WebSocket client connections.
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    timeout: Option<Duration>,
    headers: Vec<(String, String)>,
    protocols: Vec<String>,
    config: Config,
}

impl ClientBuilder {
    /// Creates a [`ClientBuilder`] with default settings (30s timeout, default [`Config`]).
    pub fn new() -> Self {
        Self {
            timeout: Some(DEFAULT_TIMEOUT),
            headers: Vec::new(),
            protocols: Vec::new(),
            config: Config::default(),
        }
    }

    /// Sets the connect, read, and write timeout for the WebSocket connection.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Disables socket timeouts.
    pub fn no_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }

    /// Adds a custom HTTP header to the opening upgrade request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Offers a WebSocket subprotocol in `Sec-WebSocket-Protocol`.
    pub fn protocol(mut self, protocol: impl Into<String>) -> Self {
        self.protocols.push(protocol.into());
        self
    }

    /// Sets the [`Config`] applied to the established [`WebSocket`].
    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Connects to `url`, completes the RFC 6455 HTTP/1.1 upgrade handshake, and returns
    /// an active [`WebSocket<TcpStream>`].
    pub fn connect(self, url: &str) -> Result<WebSocket<TcpStream>> {
        let parsed = Url::parse(url).map_err(|e| Error::InvalidUrl(e.to_string()))?;

        match parsed.scheme() {
            "ws" => {}
            other => return Err(Error::UnsupportedScheme(other.to_string())),
        }

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(Error::InvalidUrl(
                "userinfo in WebSocket URL is not supported".to_string(),
            ));
        }

        let host = parsed
            .host()
            .ok_or_else(|| Error::InvalidUrl(format!("missing host in '{url}'")))?
            .to_string();
        let port = parsed
            .port_or_default()
            .ok_or_else(|| Error::InvalidUrl(format!("missing port in '{url}'")))?;

        let connect_addr = format!("{host}:{port}");
        let host_header = if port == 80 {
            host
        } else {
            format!("{host}:{port}")
        };

        let path = if parsed.path().is_empty() {
            "/"
        } else {
            parsed.path()
        };
        let target = match parsed.query() {
            Some(q) => format!("{path}?{q}"),
            None => path.to_string(),
        };

        let has_timeout = self.timeout.is_some();
        let mut stream = connect_tcp(&connect_addr, self.timeout)?;

        let key = generate_key();
        write_client_request(
            &mut stream,
            &target,
            &host_header,
            &key,
            &self.protocols,
            &self.headers,
        )
        .map_err(|e| match e {
            Error::Io(io_err) => map_io_err(io_err, has_timeout),
            other => other,
        })?;

        let mut reader = BufReader::new(stream);
        let response =
            read_server_response(&mut reader, &key, &self.protocols).map_err(|e| match e {
                Error::Io(io_err) => map_io_err(io_err, has_timeout),
                other => other,
            })?;

        Ok(WebSocket::from_buffered(
            reader,
            Role::Client,
            self.config,
            response.protocol,
        ))
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn connect_tcp(addr: &str, timeout: Option<Duration>) -> Result<TcpStream> {
    let has_timeout = timeout.is_some();
    let addrs: Vec<_> = addr
        .to_socket_addrs()
        .map_err(|e| map_io_err(e, has_timeout))?
        .collect();
    if addrs.is_empty() {
        return Err(Error::InvalidUrl(format!(
            "failed to resolve WebSocket host '{addr}'"
        )));
    }

    let mut last_err = None;
    for sock_addr in addrs {
        let res = match timeout {
            Some(dur) => TcpStream::connect_timeout(&sock_addr, dur),
            None => TcpStream::connect(sock_addr),
        };
        match res {
            Ok(stream) => {
                stream
                    .set_read_timeout(timeout)
                    .map_err(|e| map_io_err(e, has_timeout))?;
                stream
                    .set_write_timeout(timeout)
                    .map_err(|e| map_io_err(e, has_timeout))?;
                return Ok(stream);
            }
            Err(err) => last_err = Some(err),
        }
    }

    Err(map_io_err(
        last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "connect failed")),
        has_timeout,
    ))
}
