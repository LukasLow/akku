//! Pre-bind / pre-connect socket options (`SocketOpts` and `TcpKeepalive`).

use std::time::Duration;

/// Socket options applied when `akku` creates a socket, before `bind`/`connect`
/// (and before `listen`).
///
/// Every option is unset by default. Unset options keep the exact behaviour of
/// the plain constructor, so `bind_with(addr, &SocketOpts::new())` behaves like
/// `bind(addr)` (including `SO_REUSEADDR` on Unix TCP listeners, backlog `128`,
/// and the OS default for `IPV6_V6ONLY`).
///
/// # Validation
///
/// `bind_with` and `connect_with` validate every option before touching any
/// socket and return `ErrorKind::InvalidInput` for out-of-range values or for
/// options that do not apply to the protocol (`nodelay`, `keepalive`,
/// `zero_linger`, and `backlog` on UDP; `broadcast` on TCP; `backlog` on
/// `connect_with`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SocketOpts {
    _private: (),
}

#[allow(unused_variables)]
impl SocketOpts {
    /// Creates a `SocketOpts` with every option unset.
    pub const fn new() -> Self {
        todo!()
    }

    /// Configures `SO_REUSEADDR`.
    ///
    /// When unset, TCP listeners on Unix enable it (matching `std::net`), while
    /// all other sockets leave it off.
    ///
    /// On Windows TCP sockets, `SO_REUSEADDR` is never set (to prevent port
    /// hijacking): `true` is a no-op because Windows already permits rebinding
    /// over `TIME_WAIT`. On Windows UDP sockets, `true` sets `SO_REUSEADDR` for
    /// multicast port sharing.
    pub fn reuse_address(self, on: bool) -> Self {
        todo!()
    }

    /// Configures `SO_REUSEPORT` (Unix only).
    ///
    /// On Linux and Android this enables kernel load-balancing across listeners
    /// and UDP sockets bound to the same address; on Darwin it enables BSD port
    /// sharing. Absent on Windows at compile time.
    #[cfg(unix)]
    pub fn reuse_port(self, on: bool) -> Self {
        todo!()
    }

    /// Configures `IPV6_V6ONLY`.
    ///
    /// Applied only to IPv6 addresses and skipped for IPv4 addresses so
    /// multi-address resolution keeps working. When unset, the OS default is
    /// kept (dual-stack on Linux and Darwin, IPv6-only on Windows).
    pub fn only_v6(self, on: bool) -> Self {
        todo!()
    }

    /// Configures `TCP_NODELAY` (TCP only).
    ///
    /// Rejected with `ErrorKind::InvalidInput` on UDP sockets.
    pub fn nodelay(self, on: bool) -> Self {
        todo!()
    }

    /// Configures `SO_KEEPALIVE` and optional probe timers (TCP only).
    ///
    /// `None` disables keepalive; `Some(params)` enables it with the given
    /// [`TcpKeepalive`] parameters. Rejected with `ErrorKind::InvalidInput` on
    /// UDP sockets.
    pub fn keepalive(self, keepalive: Option<TcpKeepalive>) -> Self {
        todo!()
    }

    /// Configures `SO_LINGER` with a zero timeout (TCP only).
    ///
    /// When enabled, closing the socket sends `RST` and discards unsent data.
    /// Non-zero linger timeouts are not offered because they would block the
    /// single-threaded runtime on `close`. Rejected with
    /// `ErrorKind::InvalidInput` on UDP sockets.
    pub fn zero_linger(self, on: bool) -> Self {
        todo!()
    }

    /// Configures `SO_RCVBUF` in bytes (`1..=i32::MAX`).
    ///
    /// Set on listeners before `listen` so accepted sockets inherit it and TCP
    /// window scaling takes effect. OS caps (such as Linux `rmem_max` or macOS
    /// `kern.ipc.maxsockbuf`) are passed through.
    pub fn recv_buffer_size(self, bytes: usize) -> Self {
        todo!()
    }

    /// Configures `SO_SNDBUF` in bytes (`1..=i32::MAX`).
    ///
    /// Set on listeners before `listen` so accepted sockets inherit it. OS caps
    /// (such as Linux `wmem_max` or macOS `kern.ipc.maxsockbuf`) are passed
    /// through.
    pub fn send_buffer_size(self, bytes: usize) -> Self {
        todo!()
    }

    /// Configures `IP_TTL` / unicast hops (`1..=255`).
    pub fn ttl(self, ttl: u32) -> Self {
        todo!()
    }

    /// Configures `SO_BROADCAST` (UDP only).
    ///
    /// Rejected with `ErrorKind::InvalidInput` on TCP sockets.
    pub fn broadcast(self, on: bool) -> Self {
        todo!()
    }

    /// Configures the `listen` backlog (TCP listeners only).
    ///
    /// Defaults to `128` when unset. Values above `i32::MAX` are clamped, and
    /// the OS may cap further (such as Linux `somaxconn` or Darwin
    /// `kern.ipc.somaxconn`). Rejected with `ErrorKind::InvalidInput` on UDP
    /// sockets and `connect_with`.
    pub fn backlog(self, backlog: u32) -> Self {
        todo!()
    }
}

/// TCP keepalive parameters for [`SocketOpts::keepalive`] and
/// [`super::AsyncTcpStream::set_keepalive`].
///
/// Unset fields keep the OS default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TcpKeepalive {
    _private: (),
}

#[allow(unused_variables)]
impl TcpKeepalive {
    /// Creates a `TcpKeepalive` with every parameter unset (OS defaults).
    pub const fn new() -> Self {
        todo!()
    }

    /// Idle time before the first keepalive probe (`TCP_KEEPIDLE`; Darwin
    /// `TCP_KEEPALIVE`; Windows `SIO_KEEPALIVE_VALS`).
    ///
    /// Valid range: `1 s..=32_767 s` (rounded up to whole seconds on Unix,
    /// milliseconds on Windows). Out-of-range values return
    /// `ErrorKind::InvalidInput`.
    pub fn idle(self, idle: Duration) -> Self {
        todo!()
    }

    /// Interval between keepalive probes (`TCP_KEEPINTVL`; Windows
    /// `SIO_KEEPALIVE_VALS`).
    ///
    /// Valid range: `1 s..=32_767 s`. Out-of-range values return
    /// `ErrorKind::InvalidInput`.
    pub fn interval(self, interval: Duration) -> Self {
        todo!()
    }

    /// Unanswered probes before the connection drops (`TCP_KEEPCNT`).
    ///
    /// Valid range: `1..=127`. Out-of-range values return
    /// `ErrorKind::InvalidInput`.
    pub fn retries(self, retries: u32) -> Self {
        todo!()
    }
}
