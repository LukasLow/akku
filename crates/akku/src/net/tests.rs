//! Public-API tests for `akku::net`. Seams are listed in `_dev/DESIGN.md`.
//!
//! All sockets are loopback `127.0.0.1` (or `[::1]` when the host has IPv6
//! loopback) on port `0`. Every test body runs under [`watchdog`] so a lost
//! wakeup fails instead of hanging CI. Tests share fds through [`FD_GATE`]: the
//! registration-limit test takes it exclusively so its fd exhaustion cannot
//! starve concurrently running tests.
//!
//! The async socket types do not exist on `wasm32` (no OS sockets), so the
//! whole file is native-only.
#![cfg(not(target_arch = "wasm32"))]

use std::future::Future;
use std::io::{Read as _, Write as _};
use std::panic::{self, AssertUnwindSafe};
use std::pin::{pin, Pin};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use akku::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ErrorKind};
#[cfg(unix)]
use akku::net::AsyncUnixDatagram;
use akku::net::{
    AsyncTcpListener, AsyncTcpOwnedReadHalf, AsyncTcpOwnedWriteHalf, AsyncTcpReadHalf,
    AsyncTcpStream, AsyncTcpWriteHalf, AsyncUdpSocket, IpAddr, Ipv4Addr, Ipv6Addr, Shutdown,
    SocketAddr, SocketOpts, TcpKeepalive, TcpListener, TcpStream,
};
#[cfg(any(unix, windows))]
use akku::net::{
    AsyncUnixListener, AsyncUnixOwnedReadHalf, AsyncUnixOwnedWriteHalf, AsyncUnixReadHalf,
    AsyncUnixStream, AsyncUnixWriteHalf, UnixSocketAddr,
};
use akku::task::{self, Runtime};

// --- Harness ----------------------------------------------------------------

const WATCHDOG: Duration = Duration::from_secs(60);

/// Shared by every test; taken exclusively only by the fd-exhaustion test.
static FD_GATE: RwLock<()> = RwLock::new(());

fn run_watchdog_timeout<T: Send + 'static>(
    exclusive: bool,
    timeout: Duration,
    body: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("net-test-body".into())
        .spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(move || {
                if exclusive {
                    let _g = FD_GATE.write().unwrap_or_else(|e| e.into_inner());
                    body()
                } else {
                    let _g = FD_GATE.read().unwrap_or_else(|e| e.into_inner());
                    body()
                }
            }));
            let _ = tx.send(result);
        })
        .expect("spawn test body thread");
    match rx.recv_timeout(timeout) {
        Ok(Ok(value)) => {
            worker.join().expect("test body thread exits cleanly");
            value
        }
        Ok(Err(payload)) => panic::resume_unwind(payload),
        Err(_) => {
            panic!("watchdog: test body did not finish within {timeout:?} (hang or lost wakeup)")
        }
    }
}

fn run_watchdog<T: Send + 'static>(
    exclusive: bool,
    body: impl FnOnce() -> T + Send + 'static,
) -> T {
    run_watchdog_timeout(exclusive, WATCHDOG, body)
}

/// Runs `body` on a fresh thread and fails if it does not finish in time.
fn watchdog<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    run_watchdog(false, body)
}

/// Runs `body` under a shorter 10 s watchdog for tests that may hang.
fn watchdog_short<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    run_watchdog_timeout(false, Duration::from_secs(10), body)
}

/// Runs `fut` to completion on a fresh `akku::task::Runtime`.
fn run<F: Future>(fut: F) -> F::Output {
    Runtime::new().expect("create runtime").block_on(fut)
}

const LOCALHOST: &str = "127.0.0.1:0";

fn loopback_v4() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

/// An address on which nothing listens: bind an ephemeral port, then free it.
fn closed_port() -> SocketAddr {
    let l = std::net::TcpListener::bind(LOCALHOST).unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}

/// Deterministic xorshift64 bytes, so large-input tests need no RNG dependency.
fn pseudo_random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15 ^ seed;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 56) as u8
        })
        .collect()
}

async fn read_to_end<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// Connects a client to `listener` and accepts it, returning `(client, server)`.
async fn connected_pair(
    listener: &AsyncTcpListener,
) -> io::Result<(AsyncTcpStream, AsyncTcpStream)> {
    let addr = listener.local_addr()?;
    let connect = task::spawn(async move { AsyncTcpStream::connect(addr).await });
    let (server, _) = listener.accept().await?;
    let client = connect.await.expect("connect task")?;
    Ok((client, server))
}

/// Polls the inner future exactly once, reporting whether it finished.
struct PollOnce<'a, F: Future + Unpin>(&'a mut F);

impl<F: Future + Unpin> Future for PollOnce<'_, F> {
    type Output = Option<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut *self.0).poll(cx) {
            Poll::Ready(out) => Poll::Ready(Some(out)),
            Poll::Pending => Poll::Ready(None),
        }
    }
}

fn poll_once<F: Future + Unpin>(fut: &mut F) -> PollOnce<'_, F> {
    PollOnce(fut)
}

/// Counts polls of the wrapped future (to detect spurious wakeups / spin).
struct CountPolls<F> {
    inner: F,
    polls: Arc<AtomicUsize>,
}

impl<F: Future + Unpin> Future for CountPolls<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        Pin::new(&mut self.inner).poll(cx)
    }
}

/// Returns a clone of the current task's waker.
fn current_waker() -> impl Future<Output = Waker> {
    std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone()))
}

/// A one-shot latch a std thread can open.
#[derive(Clone, Default)]
struct Gate(Arc<Mutex<(bool, Vec<Waker>)>>);

impl Gate {
    fn open(&self) {
        let wakers = {
            let mut g = self.0.lock().unwrap();
            g.0 = true;
            std::mem::take(&mut g.1)
        };
        wakers.into_iter().for_each(Waker::wake);
    }

    async fn wait(&self) {
        std::future::poll_fn(|cx| {
            let mut g = self.0.lock().unwrap();
            if g.0 {
                Poll::Ready(())
            } else {
                g.1.push(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Std-only executor, used to poll async constructors with no akku runtime.
fn block_on_without_runtime<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
        thread::park_timeout(Duration::from_millis(10));
    }
}

fn assert_no_runtime_error(err: &io::Error) {
    assert_eq!(err.kind(), ErrorKind::Other, "{err}");
    assert!(err.to_string().contains("runtime"), "{err}");
}

/// Compile-time bound checks from §Public API (never executed).
#[allow(dead_code)]
fn public_types_have_documented_bounds() {
    fn send_sync_debug<T: Send + Sync + std::fmt::Debug>() {}
    fn value_opts<T: Send + Sync + std::fmt::Debug + Clone + Default + PartialEq + Eq>() {}
    fn reader<T: AsyncRead + Unpin + Send>() {}
    fn writer<T: AsyncWrite + Unpin + Send>() {}
    send_sync_debug::<AsyncTcpListener>();
    send_sync_debug::<AsyncTcpStream>();
    send_sync_debug::<AsyncTcpReadHalf<'static>>();
    send_sync_debug::<AsyncTcpWriteHalf<'static>>();
    send_sync_debug::<AsyncTcpOwnedReadHalf>();
    send_sync_debug::<AsyncTcpOwnedWriteHalf>();
    send_sync_debug::<AsyncUdpSocket>();
    value_opts::<SocketOpts>();
    value_opts::<TcpKeepalive>();
    fn copy_type<T: Copy>() {}
    copy_type::<TcpKeepalive>();
    reader::<AsyncTcpStream>();
    writer::<AsyncTcpStream>();
    reader::<AsyncTcpReadHalf<'static>>();
    writer::<AsyncTcpWriteHalf<'static>>();
    reader::<AsyncTcpOwnedReadHalf>();
    writer::<AsyncTcpOwnedWriteHalf>();
    #[cfg(any(unix, windows))]
    {
        fn addr_bounds<T: Send + Sync + std::fmt::Debug + Clone + PartialEq + Eq>() {}
        addr_bounds::<UnixSocketAddr>();
        send_sync_debug::<AsyncUnixListener>();
        send_sync_debug::<AsyncUnixStream>();
        send_sync_debug::<AsyncUnixReadHalf<'static>>();
        send_sync_debug::<AsyncUnixWriteHalf<'static>>();
        send_sync_debug::<AsyncUnixOwnedReadHalf>();
        send_sync_debug::<AsyncUnixOwnedWriteHalf>();
        reader::<AsyncUnixStream>();
        writer::<AsyncUnixStream>();
        reader::<AsyncUnixReadHalf<'static>>();
        writer::<AsyncUnixWriteHalf<'static>>();
        reader::<AsyncUnixOwnedReadHalf>();
        writer::<AsyncUnixOwnedWriteHalf>();
    }
    #[cfg(unix)]
    {
        fn fd<T: std::os::fd::AsFd + std::os::fd::AsRawFd>() {}
        fd::<AsyncTcpListener>();
        fd::<AsyncTcpStream>();
        fd::<AsyncUdpSocket>();
        fd::<AsyncUnixListener>();
        fd::<AsyncUnixStream>();
        send_sync_debug::<AsyncUnixDatagram>();
        fd::<AsyncUnixDatagram>();
        fn unix_addr_conv<
            T: From<std::os::unix::net::SocketAddr> + Into<std::os::unix::net::SocketAddr>,
        >() {
        }
        unix_addr_conv::<UnixSocketAddr>();
    }
    #[cfg(windows)]
    {
        fn sock<T: std::os::windows::io::AsSocket + std::os::windows::io::AsRawSocket>() {}
        sock::<AsyncTcpListener>();
        sock::<AsyncTcpStream>();
        sock::<AsyncUdpSocket>();
        sock::<AsyncUnixListener>();
        sock::<AsyncUnixStream>();
    }
    // The bind/connect futures must be spawnable.
    fn send_future<F: Future + Send>(_: F) {}
    send_future(AsyncTcpListener::bind(LOCALHOST));
    send_future(AsyncTcpStream::connect(loopback_v4()));
    send_future(AsyncUdpSocket::bind(LOCALHOST));
    let opts = SocketOpts::default();
    send_future(async move {
        let _ = AsyncTcpListener::bind_with(LOCALHOST, &opts).await;
        let _ = AsyncTcpStream::connect_with(loopback_v4(), &opts).await;
        let _ = AsyncUdpSocket::bind_with(LOCALHOST, &opts).await;
    });
    #[cfg(any(unix, windows))]
    {
        send_future(AsyncUnixStream::connect("nonexistent.sock"));
    }
}

// --- std::net re-exports ---------------------------------------------------------

#[test]
fn std_reexports_are_the_blocking_std_types() {
    watchdog(|| {
        // Type identity: values flow between the akku and std names freely.
        let listener: std::net::TcpListener = TcpListener::bind(LOCALHOST).unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client: TcpStream = std::net::TcpStream::connect(addr).unwrap();
        let (mut server, peer): (std::net::TcpStream, std::net::SocketAddr) =
            listener.accept().unwrap();
        assert_eq!(peer, client.local_addr().unwrap());
        client.write_all(b"sync").unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut got = String::new();
        server.read_to_string(&mut got).unwrap();
        assert_eq!(got, "sync");
        let parsed: Result<IpAddr, akku::net::AddrParseError> = "not an ip".parse();
        assert!(parsed.is_err());
        let udp: std::net::UdpSocket = akku::net::UdpSocket::bind(LOCALHOST).unwrap();
        assert_eq!(
            udp.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        let _: std::net::Ipv6Addr = Ipv6Addr::LOCALHOST;
    });
}

// --- bind / accept / connect --------------------------------------------------------

#[test]
fn bind_reports_loopback_address_with_ephemeral_port() {
    watchdog(|| {
        let addr = run(async { AsyncTcpListener::bind(LOCALHOST).await?.local_addr() }).unwrap();
        assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(addr.port(), 0);
    });
}

#[test]
fn bind_ipv6_loopback_and_echo() {
    watchdog(|| {
        // Skipped where the host has no IPv6 loopback (common in containers).
        if std::net::TcpListener::bind("[::1]:0").is_err() {
            eprintln!("skipping: no IPv6 loopback on this host");
            return;
        }
        let got = run(async {
            let listener = AsyncTcpListener::bind("[::1]:0").await?;
            let addr = listener.local_addr()?;
            assert_eq!(addr.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
            let (mut client, mut server) = connected_pair(&listener).await?;
            assert_eq!(client.peer_addr()?, addr);
            client.write_all(b"v6").await?;
            client.shutdown().await?;
            read_to_end(&mut server).await
        })
        .unwrap();
        assert_eq!(got, b"v6");
    });
}

#[test]
fn bind_to_address_in_use_is_addr_in_use() {
    watchdog(|| {
        let taken = std::net::TcpListener::bind(LOCALHOST).unwrap();
        let addr = taken.local_addr().unwrap();
        let err = run(async move { AsyncTcpListener::bind(addr).await }).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AddrInUse);
        drop(taken);
    });
}

#[test]
fn accept_and_connect_report_matching_addresses() {
    watchdog(|| {
        run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let addr = listener.local_addr()?;
            let connect = task::spawn(async move { AsyncTcpStream::connect(addr).await });
            let (server, peer) = listener.accept().await?;
            let client = connect.await.unwrap()?;
            assert_eq!(peer, client.local_addr()?);
            assert_eq!(server.peer_addr()?, client.local_addr()?);
            assert_eq!(client.peer_addr()?, addr);
            assert_eq!(server.local_addr()?, addr);
            Ok::<_, io::Error>(())
        })
        .unwrap();
    });
}

#[test]
fn connect_to_closed_port_is_connection_refused() {
    // On Windows this exercises the WSAPoll failed-connect
    // (POLLHUP | POLLERR | POLLWRNORM) path; loopback refusal takes ~2 s there.
    watchdog(|| {
        let addr = closed_port();
        let err = run(async move { AsyncTcpStream::connect(addr).await }).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ConnectionRefused);
    });
}

#[test]
fn connect_falls_back_to_the_next_address() {
    watchdog(|| {
        run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let live = listener.local_addr()?;
            let addrs = [closed_port(), live];
            let connect = task::spawn(async move { AsyncTcpStream::connect(&addrs[..]).await });
            let (_server, _) = listener.accept().await?;
            let client = connect.await.unwrap()?;
            assert_eq!(client.peer_addr()?, live);
            Ok::<_, io::Error>(())
        })
        .unwrap();
    });
}

#[test]
fn connect_returns_the_last_error_when_all_addresses_fail() {
    watchdog(|| {
        let addrs = [closed_port(), closed_port()];
        let err = run(async move { AsyncTcpStream::connect(&addrs[..]).await }).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ConnectionRefused);
    });
}

// --- Echo, concurrency, half-close ------------------------------------------------------

#[test]
fn loopback_echo_round_trip() {
    watchdog(|| {
        let (echoed, copied) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let addr = listener.local_addr()?;
            let server = task::spawn(async move {
                let (mut conn, _) = listener.accept().await?;
                let (mut rx, mut tx) = conn.split();
                rx.copy_to(&mut tx).await
            });
            let mut client = AsyncTcpStream::connect(addr).await?;
            client.write_all(b"ping").await?;
            client.shutdown().await?;
            let echoed = read_to_end(&mut client).await?;
            Ok::<_, io::Error>((echoed, server.await.unwrap()?))
        })
        .unwrap();
        assert_eq!(echoed, b"ping");
        assert_eq!(copied, 4);
    });
}

#[test]
fn many_concurrent_echo_streams_each_get_their_own_bytes() {
    const CLIENTS: usize = 64;
    watchdog(|| {
        let results = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let addr = listener.local_addr()?;
            let server = task::spawn(async move {
                let mut handlers = Vec::new();
                for _ in 0..CLIENTS {
                    let (conn, _) = listener.accept().await?;
                    handlers.push(task::spawn(async move {
                        let (mut rx, mut tx) = conn.into_split();
                        rx.copy_to(&mut tx).await
                    }));
                }
                let mut total = 0u64;
                for h in handlers {
                    total += h.await.unwrap()?;
                }
                Ok::<_, io::Error>(total)
            });
            let clients: Vec<_> = (0..CLIENTS)
                .map(|i| {
                    task::spawn(async move {
                        let payload = pseudo_random_bytes(1_024 + i, i as u64);
                        let mut s = AsyncTcpStream::connect(addr).await?;
                        s.write_all(&payload).await?;
                        s.shutdown().await?;
                        let back = read_to_end(&mut s).await?;
                        Ok::<_, io::Error>(back == payload)
                    })
                })
                .collect();
            let mut ok = Vec::new();
            for c in clients {
                ok.push(c.await.unwrap()?);
            }
            Ok::<_, io::Error>((ok, server.await.unwrap()?))
        })
        .unwrap();
        let (ok, total) = results;
        assert!(
            ok.iter().all(|&b| b),
            "every client received exactly its own payload"
        );
        let expected: u64 = (0..CLIENTS as u64).map(|i| 1_024 + i).sum();
        assert_eq!(total, expected);
    });
}

#[test]
fn client_half_close_then_server_responds() {
    watchdog(|| {
        let (response, eof_again, server_got) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, mut server) = connected_pair(&listener).await?;
            let server_task = task::spawn(async move {
                let got = read_to_end(&mut server).await?;
                server
                    .write_all(format!("got {}", got.len()).as_bytes())
                    .await?;
                server.shutdown().await?;
                Ok::<_, io::Error>(got.len())
            });
            client.write_all(&pseudo_random_bytes(1_000, 1)).await?;
            client.shutdown().await?;
            let response = read_to_end(&mut client).await?;
            let mut buf = [0u8; 8];
            let eof_again = client.read(&mut buf).await?;
            Ok::<_, io::Error>((response, eof_again, server_task.await.unwrap()?))
        })
        .unwrap();
        assert_eq!(response, b"got 1000");
        assert_eq!(eof_again, 0, "EOF is sticky");
        assert_eq!(server_got, 1_000);
    });
}

#[test]
fn server_shutdown_how_write_while_still_reading() {
    watchdog(|| {
        let (client_saw, server_got) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, mut server) = connected_pair(&listener).await?;
            server.shutdown_how(Shutdown::Write)?;
            let mut buf = [0u8; 4];
            let client_saw = client.read(&mut buf).await?;
            client.write_all(b"late").await?;
            client.shutdown().await?;
            let server_got = read_to_end(&mut server).await?;
            Ok::<_, io::Error>((client_saw, server_got))
        })
        .unwrap();
        assert_eq!(client_saw, 0, "peer sees EOF after shutdown_how(Write)");
        assert_eq!(server_got, b"late", "read half stays open");
    });
}

#[test]
fn dropping_owned_write_half_sends_eof_but_keeps_read_half() {
    watchdog(|| {
        let (server_got, client_got) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (client, mut server) = connected_pair(&listener).await?;
            let (mut rx, mut tx) = client.into_split();
            assert_eq!(rx.peer_addr()?, tx.peer_addr()?);
            assert_eq!(rx.local_addr()?, server.peer_addr()?);
            tx.write_all(b"abc").await?;
            drop(tx); // must shutdown(Write) so the server sees EOF
            let server_got = read_to_end(&mut server).await?;
            server.write_all(b"back").await?;
            server.shutdown().await?;
            let client_got = read_to_end(&mut rx).await?;
            Ok::<_, io::Error>((server_got, client_got))
        })
        .unwrap();
        assert_eq!(server_got, b"abc");
        assert_eq!(client_got, b"back");
    });
}

#[test]
fn borrowed_split_halves_work_concurrently() {
    watchdog(|| {
        let got = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, mut server) = connected_pair(&listener).await?;
            let echo = task::spawn(async move {
                let (mut rx, mut tx) = server.split();
                assert_eq!(rx.local_addr()?, tx.local_addr()?);
                assert_eq!(rx.peer_addr()?, tx.peer_addr()?);
                rx.copy_to(&mut tx).await
            });
            let payload = pseudo_random_bytes(300_000, 2);
            let (mut rx, mut tx) = client.split();
            let mut back = Vec::new();
            // Write and read on the same task: interleave via a manual loop so
            // neither direction blocks the other.
            let writer = async {
                tx.write_all(&payload).await?;
                tx.shutdown().await
            };
            let reader = read_to_end(&mut rx);
            let (w, r) = join2(writer, reader).await;
            w?;
            back.extend(r?);
            assert_eq!(echo.await.unwrap()?, payload.len() as u64);
            Ok::<_, io::Error>(back == payload)
        })
        .unwrap();
        assert!(got);
    });
}

/// Polls two futures concurrently on one task until both finish.
async fn join2<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let mut a = pin!(a);
    let mut b = pin!(b);
    let mut out_a = None;
    let mut out_b = None;
    std::future::poll_fn(|cx| {
        if out_a.is_none() {
            if let Poll::Ready(v) = a.as_mut().poll(cx) {
                out_a = Some(v);
            }
        }
        if out_b.is_none() {
            if let Poll::Ready(v) = b.as_mut().poll(cx) {
                out_b = Some(v);
            }
        }
        if out_a.is_some() && out_b.is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    (out_a.unwrap(), out_b.unwrap())
}

#[test]
fn read_exact_on_mid_frame_close_is_unexpected_eof() {
    watchdog(|| {
        let err = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, mut server) = connected_pair(&listener).await?;
            server.write_all(b"abc").await?;
            drop(server);
            let mut buf = [0u8; 10];
            Ok::<_, io::Error>(client.read_exact(&mut buf).await.unwrap_err())
        })
        .unwrap();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
    });
}

#[test]
fn zero_length_read_and_write_return_immediately() {
    watchdog(|| {
        let (r, w) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, _server) = connected_pair(&listener).await?;
            // No data is pending, so a real read would wait forever.
            let r = client.read(&mut []).await?;
            let w = client.write(&[]).await?;
            Ok::<_, io::Error>((r, w))
        })
        .unwrap();
        assert_eq!((r, w), (0, 0));
    });
}

// --- Backpressure, try_* and readiness -----------------------------------------------------

#[test]
fn large_payload_with_saturated_send_buffer_arrives_intact() {
    const TOTAL: usize = 16 * 1024 * 1024;
    watchdog(|| {
        let (would_block_seen, received_ok) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, mut server) = connected_pair(&listener).await?;
            let payload = pseudo_random_bytes(TOTAL, 3);
            // Fill kernel buffers until the socket reports WouldBlock.
            let mut sent = 0;
            let mut would_block_seen = false;
            while sent < TOTAL {
                match client.try_write(&payload[sent..(sent + 65_536).min(TOTAL)]) {
                    Ok(n) => sent += n,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        would_block_seen = true;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            let expected = payload.clone();
            let reader = task::spawn(async move {
                let got = read_to_end(&mut server).await?;
                Ok::<_, io::Error>(got == expected)
            });
            // write_all now has to survive repeated WouldBlock/short writes.
            client.write_all(&payload[sent..]).await?;
            client.shutdown().await?;
            Ok::<_, io::Error>((would_block_seen, reader.await.unwrap()?))
        })
        .unwrap();
        assert!(
            would_block_seen,
            "16 MiB must exceed loopback socket buffers"
        );
        assert!(received_ok, "every byte arrives exactly once, in order");
    });
}

#[test]
fn try_read_on_idle_socket_would_block_then_await_wakes() {
    watchdog(|| {
        let got = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, mut server) = connected_pair(&listener).await?;
            let mut buf = [0u8; 16];
            for _ in 0..3 {
                let err = client.try_read(&mut buf).unwrap_err();
                assert_eq!(err.kind(), ErrorKind::WouldBlock);
            }
            let writer = task::spawn(async move {
                task::yield_now().await;
                server.write_all(b"wake").await?;
                Ok::<_, io::Error>(server)
            });
            let n = client.read(&mut buf).await?;
            let _server = writer.await.unwrap()?;
            Ok::<_, io::Error>(buf[..n].to_vec())
        })
        .unwrap();
        assert_eq!(got, b"wake");
    });
}

#[test]
fn idle_connected_sockets_do_not_cause_spurious_wakeups() {
    // Catches level-triggered WSAPoll POLLWRNORM spin and edge-trigger misuse:
    // an idle read waiter must not be re-polled in a loop.
    watchdog(|| {
        let polls = Arc::new(AtomicUsize::new(0));
        let polls2 = Arc::clone(&polls);
        let got = run(async move {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, mut server) = connected_pair(&listener).await?;
            let reader = task::spawn(async move {
                let mut buf = [0u8; 4];
                let fut = CountPolls {
                    inner: Box::pin(async move {
                        let n = client.read(&mut buf).await?;
                        Ok::<_, io::Error>(buf[..n].to_vec())
                    }),
                    polls: polls2,
                };
                fut.await
            });
            let gate = Gate::default();
            let opener = gate.clone();
            let t = thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                opener.open();
            });
            gate.wait().await;
            t.join().unwrap();
            server.write_all(b"late").await?;
            reader.await.unwrap()
        })
        .unwrap();
        assert_eq!(got, b"late");
        let n = polls.load(Ordering::SeqCst);
        assert!(
            n <= 4,
            "idle read future polled {n} times in 300 ms (busy spin?)"
        );
    });
}

#[test]
fn cross_thread_wakeups_while_echo_runs() {
    watchdog(|| {
        let echoed = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let addr = listener.local_addr()?;
            let server = task::spawn(async move {
                let (conn, _) = listener.accept().await?;
                let (mut rx, mut tx) = conn.into_split();
                rx.copy_to(&mut tx).await
            });
            let waker = current_waker().await;
            let stormers: Vec<_> = (0..4)
                .map(|_| {
                    let w = waker.clone();
                    thread::spawn(move || {
                        for _ in 0..500 {
                            w.wake_by_ref();
                        }
                    })
                })
                .collect();
            let payload = pseudo_random_bytes(64 * 1024, 4);
            let mut client = AsyncTcpStream::connect(addr).await?;
            let (mut rx, mut tx) = client.split();
            let (w, r) = join2(
                async {
                    tx.write_all(&payload).await?;
                    tx.shutdown().await
                },
                read_to_end(&mut rx),
            )
            .await;
            w?;
            for s in stormers {
                s.join().unwrap();
            }
            assert_eq!(server.await.unwrap()?, payload.len() as u64);
            Ok::<_, io::Error>(r? == payload)
        })
        .unwrap();
        assert!(echoed);
    });
}

// --- Drop-cancellation ------------------------------------------------------------------------

#[test]
fn dropped_accept_future_does_not_lose_connections() {
    watchdog(|| {
        let got = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let addr = listener.local_addr()?;
            {
                let mut accept = Box::pin(listener.accept());
                assert!(poll_once(&mut accept).await.is_none(), "no client yet");
            }
            let mut client = AsyncTcpStream::connect(addr).await?;
            client.write_all(b"after").await?;
            client.shutdown().await?;
            let (mut server, _) = listener.accept().await?;
            read_to_end(&mut server).await
        })
        .unwrap();
        assert_eq!(got, b"after");
    });
}

#[test]
fn dropped_read_future_does_not_lose_data_or_wakeups() {
    watchdog(|| {
        let got = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, mut server) = connected_pair(&listener).await?;
            let mut buf = [0u8; 8];
            {
                let mut read = client.read(&mut buf);
                assert!(poll_once(&mut read).await.is_none(), "idle socket");
            }
            let writer = task::spawn(async move {
                task::yield_now().await;
                server.write_all(b"data").await?;
                Ok::<_, io::Error>(server)
            });
            let n = client.read(&mut buf).await?;
            let _server = writer.await.unwrap()?;
            Ok::<_, io::Error>(buf[..n].to_vec())
        })
        .unwrap();
        assert_eq!(got, b"data");
    });
}

#[test]
fn dropped_connect_future_after_one_poll_is_clean() {
    watchdog(|| {
        run(async {
            // The dropped connects target their own listener: on loopback the
            // kernel usually completes their handshakes, so they sit in that
            // accept queue and would be handed to the health check below.
            let doomed = AsyncTcpListener::bind(LOCALHOST).await?;
            let doomed_addr = doomed.local_addr()?;
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            for _ in 0..20 {
                let mut connect = Box::pin(AsyncTcpStream::connect(doomed_addr));
                // Either state is fine; the point is dropping mid-handshake.
                if let Some(result) = poll_once(&mut connect).await {
                    drop(result?);
                }
            }
            // The listener and runtime are still healthy afterwards.
            let (mut client, mut server) = connected_pair(&listener).await?;
            client.write_all(b"ok").await?;
            client.shutdown().await?;
            assert_eq!(read_to_end(&mut server).await?, b"ok");
            Ok::<_, io::Error>(())
        })
        .unwrap();
    });
}

#[test]
fn rapid_connect_accept_drop_cycles_reuse_registrations() {
    const CYCLES: usize = 400;
    watchdog(|| {
        let done = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let mut done = 0;
            for i in 0..CYCLES {
                let (mut client, mut server) = connected_pair(&listener).await?;
                let byte = [(i % 251) as u8];
                client.write_all(&byte).await?;
                let mut got = [0u8; 1];
                server.read_exact(&mut got).await?;
                assert_eq!(got, byte);
                drop(client);
                drop(server);
                done += 1;
            }
            Ok::<_, io::Error>(done)
        })
        .unwrap();
        assert_eq!(done, CYCLES);
    });
}

// --- Errors: reset, refused, no runtime ---------------------------------------------------------

#[test]
fn peer_reset_surfaces_as_error_without_panic_or_signal() {
    watchdog(|| {
        let (read_kind, write_kind) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, server) = connected_pair(&listener).await?;
            client.write_all(b"unread bytes").await?;
            // Closing with unread data in the receive queue makes the kernel
            // send RST instead of FIN (Linux, macOS, and Windows alike).
            let mut server_std = server.into_std()?;
            server_std.set_nonblocking(false)?;
            let mut first = [0u8; 1];
            server_std.read_exact(&mut first)?;
            drop(server_std);
            let mut buf = [0u8; 16];
            let read_kind = client.read(&mut buf).await.unwrap_err().kind();
            let mut write_kind = None;
            for _ in 0..100 {
                if let Err(e) = client.write_all(&[0u8; 1024]).await {
                    write_kind = Some(e.kind());
                    break;
                }
                task::yield_now().await;
            }
            Ok::<_, io::Error>((read_kind, write_kind))
        })
        .unwrap();
        assert!(
            matches!(
                read_kind,
                ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            ),
            "read after RST: {read_kind:?}"
        );
        let write_kind = write_kind.expect("writing to a reset connection must fail");
        assert!(
            matches!(
                write_kind,
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            ),
            "write after RST: {write_kind:?}"
        );
    });
}

#[test]
fn constructors_without_a_runtime_return_err_instead_of_panicking() {
    watchdog(|| {
        let err = block_on_without_runtime(AsyncTcpListener::bind(LOCALHOST)).unwrap_err();
        assert_no_runtime_error(&err);

        let live = std::net::TcpListener::bind(LOCALHOST).unwrap();
        let addr = live.local_addr().unwrap();
        let err = block_on_without_runtime(AsyncTcpStream::connect(addr)).unwrap_err();
        assert_no_runtime_error(&err);

        let err = AsyncTcpListener::from_std(std::net::TcpListener::bind(LOCALHOST).unwrap())
            .unwrap_err();
        assert_no_runtime_error(&err);

        let stream = std::net::TcpStream::connect(addr).unwrap();
        let err = AsyncTcpStream::from_std(stream).unwrap_err();
        assert_no_runtime_error(&err);
    });
}

#[test]
fn panicking_connection_handler_leaves_runtime_and_listener_healthy() {
    watchdog(|| {
        let (first_panicked, first_client_saw, second) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let addr = listener.local_addr()?;
            let mut c1 = AsyncTcpStream::connect(addr).await?;
            let (s1, _) = listener.accept().await?;
            let handler = task::spawn(async move {
                let mut s1 = s1;
                let mut b = [0u8; 1];
                s1.read_exact(&mut b).await.unwrap();
                panic!("handler boom");
            });
            c1.write_all(b"x").await?;
            let first_panicked = handler.await.unwrap_err().is_panic();
            // The handler's stream was dropped during unwinding: EOF or reset.
            let mut buf = [0u8; 4];
            let first_client_saw = match c1.read(&mut buf).await {
                Ok(n) => Ok(n),
                Err(e) => Err(e.kind()),
            };
            let (mut c2, mut s2) = connected_pair(&listener).await?;
            c2.write_all(b"second").await?;
            c2.shutdown().await?;
            let second = read_to_end(&mut s2).await?;
            Ok::<_, io::Error>((first_panicked, first_client_saw, second))
        })
        .unwrap();
        assert!(first_panicked);
        assert!(
            matches!(first_client_saw, Ok(0) | Err(ErrorKind::ConnectionReset)),
            "{first_client_saw:?}"
        );
        assert_eq!(second, b"second");
    });
}

// --- from_std / into_std / options ---------------------------------------------------------------

#[test]
fn listener_from_std_switches_to_nonblocking_and_into_std_keeps_it() {
    watchdog(|| {
        let std_listener = std::net::TcpListener::bind(LOCALHOST).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let back = run(async move {
            // A blocking accept here would freeze the single-threaded runtime
            // and the connect task could never run.
            let listener = AsyncTcpListener::from_std(std_listener)?;
            assert_eq!(listener.local_addr()?, addr);
            let connect = task::spawn(async move { AsyncTcpStream::connect(addr).await });
            let (_server, _) = listener.accept().await?;
            let _client = connect.await.unwrap()?;
            listener.into_std()
        })
        .unwrap();
        assert_eq!(back.local_addr().unwrap(), addr);
        let err = back.accept().unwrap_err();
        assert_eq!(
            err.kind(),
            ErrorKind::WouldBlock,
            "into_std returns a non-blocking listener"
        );
    });
}

#[test]
fn stream_from_std_into_std_round_trip() {
    watchdog(|| {
        let std_listener = std::net::TcpListener::bind(LOCALHOST).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let blocking_client = std::net::TcpStream::connect(addr).unwrap();
        let (mut std_server, _) = std_listener.accept().unwrap();
        let local = blocking_client.local_addr().unwrap();
        let mut back = run(async move {
            let mut s = AsyncTcpStream::from_std(blocking_client)?;
            assert_eq!(s.local_addr()?, local);
            assert_eq!(s.peer_addr()?, addr);
            s.write_all(b"from async").await?;
            s.into_std()
        })
        .unwrap();
        let mut got = [0u8; 10];
        std_server.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"from async");
        let mut buf = [0u8; 4];
        assert_eq!(
            back.read(&mut buf).unwrap_err().kind(),
            ErrorKind::WouldBlock,
            "into_std returns a non-blocking stream"
        );
        back.set_nonblocking(false).unwrap();
        std_server.write_all(b"sync").unwrap();
        back.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"sync");
    });
}

#[test]
fn socket_options_round_trip() {
    watchdog(|| {
        run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            listener.set_ttl(77)?;
            assert_eq!(listener.ttl()?, 77);
            assert!(listener.take_error()?.is_none());
            let (client, _server) = connected_pair(&listener).await?;
            client.set_nodelay(true)?;
            assert!(client.nodelay()?);
            client.set_nodelay(false)?;
            assert!(!client.nodelay()?);
            client.set_ttl(42)?;
            assert_eq!(client.ttl()?, 42);
            assert!(client.take_error()?.is_none());
            Ok::<_, io::Error>(())
        })
        .unwrap();
    });
}

#[test]
fn half_try_methods_report_would_block_on_idle_socket() {
    watchdog(|| {
        run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await?;
            let (mut client, _server) = connected_pair(&listener).await?;
            {
                let (rx, tx) = client.split();
                let mut buf = [0u8; 4];
                assert_eq!(
                    rx.try_read(&mut buf).unwrap_err().kind(),
                    ErrorKind::WouldBlock
                );
                assert_eq!(tx.try_write(b"hi")?, 2);
            }
            let (rx, tx) = client.into_split();
            let mut buf = [0u8; 4];
            assert_eq!(
                rx.try_read(&mut buf).unwrap_err().kind(),
                ErrorKind::WouldBlock
            );
            assert_eq!(tx.try_write(b"yo")?, 2);
            Ok::<_, io::Error>(())
        })
        .unwrap();
    });
}

// --- Registration limit / fd exhaustion -------------------------------------------------------------

#[test]
fn registration_limit_or_fd_exhaustion_is_an_error_and_recovers() {
    // Registers duplicated fds until the reactor cap (65_536) or the OS fd
    // limit (EMFILE / WSAENOBUFS) stops it. Runs exclusively (FD_GATE) so the
    // exhaustion cannot fail concurrently running tests.
    const ATTEMPTS: usize = 70_000;
    run_watchdog(true, || {
        let std_listener = std::net::TcpListener::bind(LOCALHOST).unwrap();
        let base = std::net::TcpStream::connect(std_listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = std_listener.accept().unwrap();
        let outcome = run(async move {
            let mut held = Vec::new();
            let mut failure = None;
            for _ in 0..ATTEMPTS {
                let dup = match base.try_clone() {
                    Ok(d) => d,
                    Err(e) => {
                        failure = Some(("dup", e));
                        break;
                    }
                };
                match AsyncTcpStream::from_std(dup) {
                    Ok(s) => held.push(s),
                    Err(e) => {
                        failure = Some(("register", e));
                        break;
                    }
                }
            }
            let registered = held.len();
            drop(held);
            // Recovery: once registrations are dropped, registering works again.
            let again = AsyncTcpStream::from_std(base.try_clone()?).map(drop);
            Ok::<_, io::Error>((registered, failure, again))
        })
        .unwrap();
        let (registered, failure, again) = outcome;
        let (stage, err) = failure.expect("some limit must stop 70_000 registrations");
        assert!(
            registered <= 65_536,
            "reactor cap is 65_536, registered {registered}"
        );
        if stage == "register" && registered == 65_536 {
            assert_eq!(err.kind(), ErrorKind::Other, "{err}");
            assert!(err.to_string().contains("registration limit"), "{err}");
        } else {
            assert!(
                err.raw_os_error().is_some(),
                "OS limit error expected at {stage}: {err}"
            );
        }
        again.expect("registration works again after dropping streams");
    });
}

// --- G5 regressions: cooperative budget, shared-listener wakers ---------------

/// Ready I/O operations one task may run before a sibling must have been
/// polled. Far above the runtime's internal per-poll budget, so this does not
/// pin its exact value, and far below [`HOG_OPS`].
const SIBLING_BOUND: usize = 4_096;

/// Upper bound on the ready operations a hog loop runs before giving up.
const HOG_OPS: usize = 100_000;

/// Spawns a task that counts its `yield_now` turns until `stop` is set.
fn spawn_turn_counter(
    turns: &Arc<AtomicUsize>,
    stop: &Arc<std::sync::atomic::AtomicBool>,
) -> task::JoinHandle<()> {
    let turns = Arc::clone(turns);
    let stop = Arc::clone(stop);
    task::spawn(async move {
        while !stop.load(Ordering::SeqCst) {
            task::yield_now().await;
            turns.fetch_add(1, Ordering::SeqCst);
        }
    })
}

/// Turns a connected std stream into a "listener" whose `accept` always fails
/// at once (`EINVAL` / `WSAEINVAL`: the socket is not listening). Like an
/// `EMFILE` accept loop, every attempt is an immediately ready error.
fn always_failing_listener() -> io::Result<(TcpListener, TcpStream, TcpStream)> {
    let real = TcpListener::bind(LOCALHOST)?;
    let peer = TcpStream::connect(real.local_addr()?)?;
    let (accepted, _) = real.accept()?;
    let client = TcpStream::connect(real.local_addr()?)?;
    #[cfg(unix)]
    let fake = TcpListener::from(std::os::fd::OwnedFd::from(client));
    #[cfg(windows)]
    let fake = TcpListener::from(std::os::windows::io::OwnedSocket::from(client));
    Ok((fake, peer, accepted))
}

#[test]
fn accept_error_loop_does_not_starve_siblings() {
    watchdog(|| {
        let (fake, _peer, _accepted) = always_failing_listener().unwrap();
        let (errors_before_sibling, first_error) = run(async move {
            let listener = AsyncTcpListener::from_std(fake).unwrap();
            let turns = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sibling = spawn_turn_counter(&turns, &stop);
            let turns_hog = Arc::clone(&turns);
            let hog = task::spawn(async move {
                let start = turns_hog.load(Ordering::SeqCst);
                let mut first_error = None;
                for attempt in 0..HOG_OPS {
                    if turns_hog.load(Ordering::SeqCst) > start {
                        return (attempt, first_error);
                    }
                    match listener.accept().await {
                        Ok(_) => panic!("a non-listening socket must not accept"),
                        Err(e) => {
                            first_error.get_or_insert(e.kind());
                        }
                    }
                }
                (HOG_OPS, first_error)
            });
            let out = hog.await.unwrap();
            stop.store(true, Ordering::SeqCst);
            sibling.await.unwrap();
            out
        });
        assert!(first_error.is_some(), "accept must fail on a non-listener");
        assert!(
            errors_before_sibling <= SIBLING_BOUND,
            "sibling starved for {errors_before_sibling} accept errors (bound {SIBLING_BOUND})"
        );
    });
}

#[test]
fn ready_one_byte_read_loop_yields_to_siblings() {
    const PAYLOAD: usize = 64 * 1024;
    watchdog(|| {
        let reads_before_sibling = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await.unwrap();
            let addr = listener.local_addr().unwrap();
            // A blocking std writer queues the whole payload before the reader
            // starts, so every 1-byte read is immediately ready.
            let writer = thread::spawn(move || {
                let mut client = TcpStream::connect(addr).unwrap();
                client.write_all(&vec![7u8; PAYLOAD]).unwrap();
                client
            });
            let (mut server, _) = listener.accept().await.unwrap();
            let client = writer.join().unwrap();

            let turns = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sibling = spawn_turn_counter(&turns, &stop);
            let turns_hog = Arc::clone(&turns);
            let hog = task::spawn(async move {
                let start = turns_hog.load(Ordering::SeqCst);
                let mut byte = [0u8; 1];
                for read in 0..PAYLOAD {
                    if turns_hog.load(Ordering::SeqCst) > start {
                        return read;
                    }
                    assert_eq!(server.read(&mut byte).await.unwrap(), 1);
                }
                PAYLOAD
            });
            let out = hog.await.unwrap();
            stop.store(true, Ordering::SeqCst);
            sibling.await.unwrap();
            drop(client);
            out
        });
        assert!(
            reads_before_sibling <= SIBLING_BOUND,
            "sibling starved for {reads_before_sibling} ready reads (bound {SIBLING_BOUND})"
        );
    });
}

#[test]
fn eof_read_loop_in_root_future_yields_to_spawned_tasks() {
    watchdog(|| {
        let eofs_before_sibling = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await.unwrap();
            let (mut client, server) = connected_pair(&listener).await.unwrap();
            drop(server);
            let turns = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sibling = spawn_turn_counter(&turns, &stop);
            let start = turns.load(Ordering::SeqCst);
            let mut buf = [0u8; 16];
            let mut eofs = HOG_OPS;
            for read in 0..HOG_OPS {
                if turns.load(Ordering::SeqCst) > start {
                    eofs = read;
                    break;
                }
                // EOF is permanently ready: every read returns `Ok(0)` at once.
                assert_eq!(client.read(&mut buf).await.unwrap(), 0);
            }
            stop.store(true, Ordering::SeqCst);
            sibling.await.unwrap();
            eofs
        });
        assert!(
            eofs_before_sibling <= SIBLING_BOUND,
            "spawned task starved for {eofs_before_sibling} EOF reads (bound {SIBLING_BOUND})"
        );
    });
}

#[test]
fn copy_to_between_ready_sockets_yields_to_siblings() {
    const PAYLOAD: usize = 64 * 1024;
    watchdog(|| {
        let (copied, sibling_turns_during_copy) = run(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let writer = thread::spawn(move || {
                let mut client = TcpStream::connect(addr).unwrap();
                client.write_all(&vec![3u8; PAYLOAD]).unwrap();
                client.shutdown(Shutdown::Write).unwrap();
                client
            });
            let (server, _) = listener.accept().await.unwrap();
            let client = writer.join().unwrap();

            let turns = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sibling = spawn_turn_counter(&turns, &stop);
            let turns_hog = Arc::clone(&turns);
            let hog = task::spawn(async move {
                let start = turns_hog.load(Ordering::SeqCst);
                // A 1-byte-at-a-time reader over the buffered payload and an
                // always-writable sink: every operation is ready.
                let mut src = OneByteReader(server);
                let mut sink = CountingSink::default();
                let copied = src.copy_to(&mut sink).await.unwrap();
                (copied, turns_hog.load(Ordering::SeqCst) - start)
            });
            let out = hog.await.unwrap();
            stop.store(true, Ordering::SeqCst);
            sibling.await.unwrap();
            drop(client);
            out
        });
        assert_eq!(copied, PAYLOAD as u64);
        assert!(
            sibling_turns_during_copy > 0,
            "sibling never ran during a {PAYLOAD}-step ready copy"
        );
    });
}

/// Forwards reads one byte at a time, so a copy needs one ready read per byte.
struct OneByteReader(AsyncTcpStream);

impl AsyncRead for OneByteReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let len = buf.len().min(1);
        self.0.poll_read(cx, &mut buf[..len])
    }
}

/// An always-ready writer that discards bytes.
#[derive(Default)]
struct CountingSink(u64);

impl AsyncWrite for CountingSink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0 += buf.len() as u64;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[test]
fn concurrent_acceptors_on_shared_listener_all_complete() {
    const ACCEPTORS: usize = 4;
    watchdog(|| {
        let accepted = run(async {
            let listener = Arc::new(AsyncTcpListener::bind(LOCALHOST).await.unwrap());
            let addr = listener.local_addr().unwrap();
            let acceptors: Vec<_> = (0..ACCEPTORS)
                .map(|_| {
                    let listener = Arc::clone(&listener);
                    task::spawn(async move { listener.accept().await.map(|(_, peer)| peer) })
                })
                .collect();
            // Let every acceptor park on the empty backlog with its own waker.
            for _ in 0..8 {
                task::yield_now().await;
            }
            let clients: Vec<TcpStream> = (0..ACCEPTORS)
                .map(|_| TcpStream::connect(addr).unwrap())
                .collect();
            let mut peers = Vec::new();
            for acceptor in acceptors {
                peers.push(acceptor.await.unwrap().unwrap());
            }
            let mut expected: Vec<SocketAddr> =
                clients.iter().map(|c| c.local_addr().unwrap()).collect();
            peers.sort();
            expected.sort();
            assert_eq!(peers, expected, "each client is accepted exactly once");
            peers.len()
        });
        assert_eq!(accepted, ACCEPTORS);
    });
}

/// Counts how often it was woken.
struct CountingWaker(AtomicUsize);

impl Wake for CountingWaker {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn poll_accept_with_many_distinct_wakers_wakes_every_one() {
    // More distinct wakers than the runtime has task slots: the listener's
    // waiter set is bounded, yet no waiter may be lost (overflow wakes early).
    const WAKERS: usize = 100;
    watchdog(|| {
        let rt = Runtime::with_limits(task::Limits::new().with_max_tasks(4)).unwrap();
        let woken = rt.block_on(async {
            let listener = AsyncTcpListener::bind(LOCALHOST).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let counters: Vec<Arc<CountingWaker>> = (0..WAKERS)
                .map(|_| Arc::new(CountingWaker(AtomicUsize::new(0))))
                .collect();
            for counter in &counters {
                let waker = Waker::from(Arc::clone(counter));
                let mut cx = Context::from_waker(&waker);
                assert!(listener.poll_accept(&mut cx).is_pending());
            }
            let _client = TcpStream::connect(addr).unwrap();
            for _ in 0..100_000 {
                if counters.iter().all(|c| c.0.load(Ordering::SeqCst) > 0) {
                    break;
                }
                task::yield_now().await;
            }
            counters
                .iter()
                .filter(|c| c.0.load(Ordering::SeqCst) > 0)
                .count()
        });
        assert_eq!(woken, WAKERS, "every waiting waker must be woken");
    });
}

#[test]
fn immediate_connect_error_loop_does_not_starve_siblings() {
    watchdog(|| {
        let errors_before_sibling = run(async {
            let turns = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sibling = spawn_turn_counter(&turns, &stop);
            let start = turns.load(Ordering::SeqCst);
            let mut errors = HOG_OPS;
            for attempt in 0..HOG_OPS {
                if turns.load(Ordering::SeqCst) > start {
                    errors = attempt;
                    break;
                }
                // No address to try: fails at once, without touching the reactor.
                let err = AsyncTcpStream::connect(&[][..] as &[SocketAddr])
                    .await
                    .unwrap_err();
                assert_eq!(err.kind(), ErrorKind::InvalidInput);
            }
            stop.store(true, Ordering::SeqCst);
            sibling.await.unwrap();
            errors
        });
        assert!(
            errors_before_sibling <= SIBLING_BOUND,
            "sibling starved for {errors_before_sibling} failing connects (bound {SIBLING_BOUND})"
        );
    });
}

// ============================================================================
// Phase 2 tests (DESIGN.md §P2.8)
// ============================================================================

/// Runs `body` exclusively under `FD_GATE` so fd-count assertions see no noise
/// from sibling test threads.
#[cfg(unix)]
fn watchdog_exclusive<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    run_watchdog(true, body)
}

/// An unused UDP port: bind an ephemeral UDP socket, record its address, drop.
fn closed_udp_port() -> SocketAddr {
    let s = std::net::UdpSocket::bind(LOCALHOST).unwrap();
    let addr = s.local_addr().unwrap();
    drop(s);
    addr
}

/// Returns the number of currently open file descriptors in this process.
#[cfg(unix)]
fn open_fd_count() -> usize {
    #[cfg(target_os = "linux")]
    let dir = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    let dir = "/dev/fd";
    std::fs::read_dir(dir).expect("read fd directory").count()
}

/// Short, unique temporary directory for Unix/AF_UNIX socket files, cleaned up
/// on drop. Uses `/tmp` on Unix when shorter than `std::env::temp_dir()` so
/// macOS's 103-byte `sun_path` limit leaves ample room for path-limit tests.
#[cfg(any(unix, windows))]
struct TempSocketDir(std::path::PathBuf);

#[cfg(any(unix, windows))]
impl TempSocketDir {
    fn new(tag: &str) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let base = Self::short_temp_base();
        let dir = base.join(format!("ak{pid}_{id}_{tag}"));
        std::fs::create_dir_all(&dir).expect("create temp socket dir");
        Self(dir)
    }

    fn short_temp_base() -> std::path::PathBuf {
        let sys_tmp = std::env::temp_dir();
        #[cfg(unix)]
        {
            let short = std::path::Path::new("/tmp");
            if short.is_dir() && short.as_os_str().len() < sys_tmp.as_os_str().len() {
                return short.to_path_buf();
            }
        }
        sys_tmp
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

#[cfg(any(unix, windows))]
impl Drop for TempSocketDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// --- Phase 2: UDP (tests 1-12) ----------------------------------------------

#[test]
fn p2_01_udp_v4_v6_echo_connect_and_unconnected_errors() {
    watchdog(|| {
        run(async {
            // IPv4 send_to / recv_from echo + unconnected peer_addr/send errors.
            let a = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let b = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let a_addr = a.local_addr().unwrap();
            let b_addr = b.local_addr().unwrap();

            assert_eq!(a.peer_addr().unwrap_err().kind(), ErrorKind::NotConnected);
            assert!(a.send(b"unconnected").await.is_err());

            assert_eq!(a.send_to(b"hello-v4", b_addr).await.unwrap(), 8);
            let mut buf = [0u8; 64];
            let (n, from) = b.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"hello-v4");
            assert_eq!(from, a_addr);

            // Connected send / recv round-trip.
            a.connect(b_addr).await.unwrap();
            b.connect(a_addr).await.unwrap();
            assert_eq!(a.peer_addr().unwrap(), b_addr);
            assert_eq!(b.peer_addr().unwrap(), a_addr);

            assert_eq!(a.send(b"conn-ping").await.unwrap(), 9);
            let n = b.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"conn-ping");
            assert_eq!(b.send(b"conn-pong").await.unwrap(), 9);
            let n = a.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"conn-pong");

            // IPv6 loopback when available on the host.
            if std::net::UdpSocket::bind("[::1]:0").is_ok() {
                let v6_a = AsyncUdpSocket::bind("[::1]:0").await.unwrap();
                let v6_b = AsyncUdpSocket::bind("[::1]:0").await.unwrap();
                let v6_a_addr = v6_a.local_addr().unwrap();
                let v6_b_addr = v6_b.local_addr().unwrap();
                assert_eq!(v6_a_addr.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));

                assert_eq!(v6_a.send_to(b"hello-v6", v6_b_addr).await.unwrap(), 8);
                let (n, from) = v6_b.recv_from(&mut buf).await.unwrap();
                assert_eq!(&buf[..n], b"hello-v6");
                assert_eq!(from, v6_a_addr);
            }
        });
    });
}

#[test]
fn p2_02_udp_zero_length_datagrams_per_os() {
    watchdog(|| {
        run(async {
            let rx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let tx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let rx_addr = rx.local_addr().unwrap();
            let tx_addr = tx.local_addr().unwrap();

            // Empty datagram arrives as Ok((0, sender)), is not EOF, and the
            // following datagram still arrives.
            assert_eq!(tx.send_to(&[], rx_addr).await.unwrap(), 0);
            assert_eq!(tx.send_to(b"after-empty", rx_addr).await.unwrap(), 11);

            let mut buf = [0u8; 64];
            let (n, from) = rx.recv_from(&mut buf).await.unwrap();
            assert_eq!(n, 0);
            assert_eq!(from, tx_addr);

            let (n, from) = rx.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"after-empty");
            assert_eq!(from, tx_addr);

            // recv(&mut []) on a NON-EMPTY datagram (per-OS semantics, Q9).
            rx.connect(tx_addr).await.unwrap();
            tx.connect(rx_addr).await.unwrap();
            assert_eq!(tx.send(b"non-empty").await.unwrap(), 9);
            assert_eq!(tx.send(b"following").await.unwrap(), 9);

            let res = rx.recv(&mut []).await;
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                assert_eq!(
                    res.unwrap(),
                    0,
                    "Linux/Android consumes into empty buf as Ok(0)"
                );
            }
            #[cfg(windows)]
            {
                let err = res.unwrap_err();
                assert_eq!(
                    err.raw_os_error(),
                    Some(10040),
                    "Windows reports WSAEMSGSIZE (10040) on empty buf, got {err:?}"
                );
            }
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
                eprintln!("p2_02 Darwin recv(&mut []) on non-empty UDP datagram => {res:?}");
                assert_eq!(res.unwrap(), 0, "Darwin consumes into empty buf as Ok(0)");
            }

            // Regardless of OS, the non-empty datagram was consumed/discarded
            // and the following datagram arrives cleanly.
            let n = rx.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"following");
        });
    });
}

#[test]
fn p2_03_udp_truncation_per_os_and_following_datagram() {
    watchdog(|| {
        run(async {
            let rx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let tx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let rx_addr = rx.local_addr().unwrap();
            let tx_addr = tx.local_addr().unwrap();
            rx.connect(tx_addr).await.unwrap();
            tx.connect(rx_addr).await.unwrap();

            let payload = pseudo_random_bytes(1000, 3);
            assert_eq!(tx.send(&payload).await.unwrap(), 1000);
            assert_eq!(tx.send(b"next-dgram").await.unwrap(), 10);

            let mut small = [0u8; 10];
            let res = rx.recv(&mut small).await;
            #[cfg(unix)]
            {
                assert_eq!(res.unwrap(), 10);
                assert_eq!(&small[..], &payload[..10]);
            }
            #[cfg(windows)]
            {
                let err = res.unwrap_err();
                assert_eq!(
                    err.raw_os_error(),
                    Some(10040),
                    "expected WSAEMSGSIZE, got {err:?}"
                );
                assert_eq!(&small[..], &payload[..10]);
            }

            let mut buf = [0u8; 64];
            let n = rx.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"next-dgram");
        });
    });
}

#[test]
fn p2_04_udp_peek_and_peek_from_preserve_readiness() {
    watchdog_short(|| {
        run(async {
            let rx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let tx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let rx_addr = rx.local_addr().unwrap();
            let tx_addr = tx.local_addr().unwrap();

            assert_eq!(tx.send_to(b"peek-me", rx_addr).await.unwrap(), 7);
            let mut b1 = [0u8; 32];
            let (n1, from1) = rx.peek_from(&mut b1).await.unwrap();
            assert_eq!(&b1[..n1], b"peek-me");
            assert_eq!(from1, tx_addr);

            // Second peek sees the exact same datagram.
            let mut b2 = [0u8; 32];
            let (n2, from2) = rx.peek_from(&mut b2).await.unwrap();
            assert_eq!(&b2[..n2], b"peek-me");
            assert_eq!(from2, tx_addr);

            // Awaited recv_from must return immediately without hanging on ET.
            let mut b3 = [0u8; 32];
            let (n3, from3) = rx.recv_from(&mut b3).await.unwrap();
            assert_eq!(&b3[..n3], b"peek-me");
            assert_eq!(from3, tx_addr);

            // Connected peek followed by awaited recv.
            rx.connect(tx_addr).await.unwrap();
            tx.connect(rx_addr).await.unwrap();
            assert_eq!(tx.send(b"conn-peek").await.unwrap(), 9);
            let mut b4 = [0u8; 32];
            let n4 = rx.peek(&mut b4).await.unwrap();
            assert_eq!(&b4[..n4], b"conn-peek");

            let mut b5 = [0u8; 32];
            let n5 = rx.recv(&mut b5).await.unwrap();
            assert_eq!(&b5[..n5], b"conn-peek");
        });
    });
}

#[test]
fn p2_05_udp_connected_filters_other_senders() {
    watchdog(|| {
        run(async {
            let peer_a = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let peer_b = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let receiver = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();

            let a_addr = peer_a.local_addr().unwrap();
            let rx_addr = receiver.local_addr().unwrap();
            receiver.connect(a_addr).await.unwrap();

            // Send from unrelated peer B first, then from connected peer A.
            assert_eq!(peer_b.send_to(b"from-b", rx_addr).await.unwrap(), 6);
            assert_eq!(peer_a.send_to(b"from-a", rx_addr).await.unwrap(), 6);

            let mut buf = [0u8; 32];
            let n = receiver.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"from-a");
            assert_eq!(
                receiver.try_recv(&mut buf).unwrap_err().kind(),
                ErrorKind::WouldBlock
            );
        });
    });
}

#[test]
fn p2_06_udp_connected_refused_and_no_spin() {
    watchdog_short(|| {
        run(async {
            let dead = closed_udp_port();
            let sock = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            sock.connect(dead).await.unwrap();

            #[cfg(unix)]
            {
                let err = match sock.send(b"ping").await {
                    Err(e) => e,
                    Ok(_) => {
                        let mut buf = [0u8; 16];
                        sock.recv(&mut buf).await.unwrap_err()
                    }
                };
                assert_eq!(
                    err.kind(),
                    ErrorKind::ConnectionRefused,
                    "expected ConnectionRefused from connected UDP to closed port, got {err:?}"
                );
            }

            #[cfg(windows)]
            {
                // On Windows, SIO_UDP_CONNRESET is disabled so the socket stays
                // waiting without spinning.
                sock.send(b"ping").await.unwrap();
                let mut buf = [0u8; 16];
                assert_eq!(
                    sock.try_recv(&mut buf).unwrap_err().kind(),
                    ErrorKind::WouldBlock
                );

                let polls = Arc::new(AtomicUsize::new(0));
                let gate = Gate::default();
                let gate_thread = gate.clone();
                let releaser = thread::spawn(move || {
                    thread::sleep(Duration::from_millis(80));
                    gate_thread.open();
                });

                let polls_rx = Arc::clone(&polls);
                let waiter = task::spawn(async move {
                    let fut = CountPolls {
                        inner: Box::pin(async move {
                            let mut b = [0u8; 16];
                            let _ = sock.recv(&mut b).await;
                        }),
                        polls: polls_rx,
                    };
                    fut.await;
                });
                gate.wait().await;
                drop(waiter);
                releaser.join().unwrap();

                let count = polls.load(Ordering::SeqCst);
                assert!(
                    count <= 4,
                    "connected UDP recv spun on Windows: {count} polls"
                );
            }
        });
    });
}

#[test]
fn p2_07_udp_unconnected_immune_to_icmp_port_unreachable() {
    watchdog_short(|| {
        run(async {
            let server = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let live_peer = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let server_addr = server.local_addr().unwrap();
            let peer_addr = live_peer.local_addr().unwrap();
            let dead = closed_udp_port();

            // Send to a closed port to elicit ICMP port-unreachable.
            for _ in 0..3 {
                let _ = server.send_to(b"to-dead", dead).await;
            }

            // Wait briefly so any loopback ICMP error is delivered before recv_from.
            let gate = Gate::default();
            let gate_t = gate.clone();
            let sleeper = thread::spawn(move || {
                thread::sleep(Duration::from_millis(30));
                gate_t.open();
            });
            gate.wait().await;
            sleeper.join().unwrap();

            assert_eq!(
                live_peer
                    .send_to(b"still-alive", server_addr)
                    .await
                    .unwrap(),
                11
            );
            let mut buf = [0u8; 64];
            let (n, from) = server
                .recv_from(&mut buf)
                .await
                .expect("unconnected recv_from must not fail with ConnectionReset");
            assert_eq!(&buf[..n], b"still-alive");
            assert_eq!(from, peer_addr);
        });
    });
}

#[test]
fn p2_08_udp_eight_shared_waiters_all_complete() {
    const WAITERS: usize = 8;
    watchdog_short(|| {
        run(async {
            let rx = Arc::new(AsyncUdpSocket::bind(loopback_v4()).await.unwrap());
            let rx_addr = rx.local_addr().unwrap();
            let tx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();

            let handles: Vec<_> = (0..WAITERS)
                .map(|_| {
                    let rx = Arc::clone(&rx);
                    task::spawn(async move {
                        let mut buf = [0u8; 32];
                        let (n, _) = rx.recv_from(&mut buf).await.unwrap();
                        buf[..n].to_vec()
                    })
                })
                .collect();

            // Let all eight tasks park on the shared read waiter set.
            for _ in 0..16 {
                task::yield_now().await;
            }

            for i in 0..WAITERS {
                let msg = format!("msg-{i}");
                tx.send_to(msg.as_bytes(), rx_addr).await.unwrap();
            }

            let mut received = Vec::with_capacity(WAITERS);
            for h in handles {
                received.push(h.await.unwrap());
            }
            received.sort();
            let mut expected: Vec<Vec<u8>> = (0..WAITERS)
                .map(|i| format!("msg-{i}").into_bytes())
                .collect();
            expected.sort();
            assert_eq!(received, expected);
        });
    });
}

#[test]
fn p2_09_udp_flood_10000_datagrams_yields_to_sibling() {
    const COUNT: usize = 10_000;
    watchdog(|| {
        let sibling_turns = run(async {
            let a = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let b = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let a_addr = a.local_addr().unwrap();
            let b_addr = b.local_addr().unwrap();
            a.connect(b_addr).await.unwrap();
            b.connect(a_addr).await.unwrap();

            let turns = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sibling = spawn_turn_counter(&turns, &stop);
            let start = turns.load(Ordering::SeqCst);

            let hog = task::spawn(async move {
                let mut buf = [0u8; 16];
                for i in 0..COUNT {
                    let payload = (i as u32).to_le_bytes();
                    assert_eq!(a.send(&payload).await.unwrap(), 4);
                    assert_eq!(b.recv(&mut buf).await.unwrap(), 4);
                    assert_eq!(&buf[..4], &payload);
                }
            });
            hog.await.unwrap();
            let delta = turns.load(Ordering::SeqCst) - start;
            stop.store(true, Ordering::SeqCst);
            sibling.await.unwrap();
            delta
        });
        assert!(
            sibling_turns > 0,
            "sibling task starved during {COUNT}-datagram UDP flood"
        );
    });
}

#[test]
fn p2_10_udp_size_limits_emsgsize() {
    watchdog(|| {
        run(async {
            let rx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let tx = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let rx_addr = rx.local_addr().unwrap();

            // 70,000 bytes exceeds IPv4 UDP maximum (65,507) on every OS.
            let huge = vec![0xABu8; 70_000];
            let err = tx.send_to(&huge, rx_addr).await.unwrap_err();
            #[cfg(target_os = "linux")]
            assert_eq!(
                err.raw_os_error(),
                Some(90),
                "expected EMSGSIZE (90), got {err:?}"
            );
            #[cfg(target_os = "macos")]
            assert_eq!(
                err.raw_os_error(),
                Some(40),
                "expected EMSGSIZE (40), got {err:?}"
            );
            #[cfg(windows)]
            assert_eq!(
                err.raw_os_error(),
                Some(10040),
                "expected WSAEMSGSIZE (10040), got {err:?}"
            );

            // 9,217 bytes exceeds macOS default net.inet.udp.maxdgram (9,216),
            // but fits on Linux and Windows.
            let medium = pseudo_random_bytes(9_217, 10);
            #[cfg(target_os = "macos")]
            {
                let err = tx.send_to(&medium, rx_addr).await.unwrap_err();
                assert_eq!(
                    err.raw_os_error(),
                    Some(40),
                    "expected macOS EMSGSIZE for 9,217-byte UDP datagram, got {err:?}"
                );
            }
            #[cfg(not(target_os = "macos"))]
            {
                assert_eq!(tx.send_to(&medium, rx_addr).await.unwrap(), 9_217);
                let mut buf = vec![0u8; 10_000];
                let (n, _) = rx.recv_from(&mut buf).await.unwrap();
                assert_eq!(n, 9_217);
                assert_eq!(&buf[..n], &medium[..]);
            }
        });
    });
}

#[test]
fn p2_11_udp_multicast_and_broadcast_options_and_errors() {
    watchdog(|| {
        run(async {
            let sock = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();

            sock.set_broadcast(true).unwrap();
            assert!(sock.broadcast().unwrap());
            sock.set_broadcast(false).unwrap();
            assert!(!sock.broadcast().unwrap());

            sock.set_ttl(42).unwrap();
            assert_eq!(sock.ttl().unwrap(), 42);

            sock.set_multicast_loop_v4(false).unwrap();
            assert!(!sock.multicast_loop_v4().unwrap());
            sock.set_multicast_loop_v4(true).unwrap();
            assert!(sock.multicast_loop_v4().unwrap());

            sock.set_multicast_ttl_v4(3).unwrap();
            assert_eq!(sock.multicast_ttl_v4().unwrap(), 3);

            // Joining a unicast address as a multicast group must fail.
            assert!(sock
                .join_multicast_v4(&Ipv4Addr::LOCALHOST, &Ipv4Addr::UNSPECIFIED)
                .is_err());

            // Leaving a group that was never joined must fail.
            let group = Ipv4Addr::new(239, 255, 0, 42);
            assert!(sock
                .leave_multicast_v4(&group, &Ipv4Addr::LOCALHOST)
                .is_err());

            if std::net::UdpSocket::bind("[::1]:0").is_ok() {
                let v6 = AsyncUdpSocket::bind("[::1]:0").await.unwrap();
                v6.set_multicast_loop_v6(false).unwrap();
                assert!(!v6.multicast_loop_v6().unwrap());
                v6.set_multicast_loop_v6(true).unwrap();
                assert!(v6.multicast_loop_v6().unwrap());
            }
        });
    });
}

#[test]
fn p2_12_udp_from_std_into_std_no_runtime_and_slab_reuse() {
    watchdog(|| {
        // Outside a runtime: all constructors must return the runtime error.
        let err = block_on_without_runtime(AsyncUdpSocket::bind(loopback_v4())).unwrap_err();
        assert_no_runtime_error(&err);
        let err =
            block_on_without_runtime(AsyncUdpSocket::bind_with(loopback_v4(), &SocketOpts::new()))
                .unwrap_err();
        assert_no_runtime_error(&err);
        let std_sock = std::net::UdpSocket::bind(LOCALHOST).unwrap();
        let err = AsyncUdpSocket::from_std(std_sock).unwrap_err();
        assert_no_runtime_error(&err);

        run(async {
            // from_std -> async I/O -> into_std -> std I/O.
            let raw = std::net::UdpSocket::bind(LOCALHOST).unwrap();
            let raw_addr = raw.local_addr().unwrap();
            let peer = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
            let peer_addr = peer.local_addr().unwrap();

            let async_sock = AsyncUdpSocket::from_std(raw).unwrap();
            peer.send_to(b"via-async", raw_addr).await.unwrap();
            let mut buf = [0u8; 32];
            let (n, from) = async_sock.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"via-async");
            assert_eq!(from, peer_addr);

            let back = async_sock.into_std().unwrap();
            back.set_nonblocking(false).unwrap();
            back.send_to(b"via-std", peer_addr).unwrap();
            let (n, from) = peer.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"via-std");
            assert_eq!(from, raw_addr);

            // 500-socket bind/drop loop must reuse reactor slab slots without leak.
            for _ in 0..500 {
                let s = AsyncUdpSocket::bind(loopback_v4()).await.unwrap();
                assert_ne!(s.local_addr().unwrap().port(), 0);
                drop(s);
            }
        });
    });
}

// --- Phase 2: Unix streams (tests 13-23b) -----------------------------------

#[cfg(unix)]
#[test]
fn p2_13_unix_stream_pathname_echo_halves_half_close_and_drop_eof() {
    watchdog(|| {
        let dir = TempSocketDir::new("u13");
        let path = dir.path("echo.sock");
        run(async {
            let listener = AsyncUnixListener::bind(&path).unwrap();
            assert_eq!(
                listener.local_addr().unwrap().as_pathname(),
                Some(path.as_path())
            );

            // Basic echo + address checks.
            let p1 = path.clone();
            let c_task = task::spawn(async move { AsyncUnixStream::connect(&p1).await.unwrap() });
            let (mut server, peer) = listener.accept().await.unwrap();
            let mut client = c_task.await.unwrap();
            assert!(peer.is_unnamed());

            client.write_all(b"unix-ping").await.unwrap();
            let mut buf = [0u8; 9];
            server.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"unix-ping");

            // Borrowed split() bidirectional echo.
            let (mut cr, mut cw) = client.split();
            let (mut sr, mut sw) = server.split();
            cw.write_all(b"c->s").await.unwrap();
            sw.write_all(b"s->c").await.unwrap();
            let mut b1 = [0u8; 4];
            let mut b2 = [0u8; 4];
            sr.read_exact(&mut b1).await.unwrap();
            cr.read_exact(&mut b2).await.unwrap();
            assert_eq!(&b1, b"c->s");
            assert_eq!(&b2, b"s->c");

            // Half-close: client shuts down write side; server reads EOF, then replies.
            client.shutdown().await.unwrap();
            assert_eq!(read_to_end(&mut server).await.unwrap(), b"");
            server.write_all(b"after-half-close").await.unwrap();
            server.shutdown().await.unwrap();
            assert_eq!(read_to_end(&mut client).await.unwrap(), b"after-half-close");

            // Owned into_split(): dropping owned write half gives peer EOF.
            let p2 = path.clone();
            let c2_task = task::spawn(async move { AsyncUnixStream::connect(&p2).await.unwrap() });
            let (mut server2, _) = listener.accept().await.unwrap();
            let client2 = c2_task.await.unwrap();
            let (_cr2, mut cw2) = client2.into_split();
            cw2.write_all(b"bye").await.unwrap();
            drop(cw2);
            assert_eq!(read_to_end(&mut server2).await.unwrap(), b"bye");
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_14_unix_stream_pair_echo_and_unnamed() {
    watchdog(|| {
        run(async {
            let (mut a, mut b) = AsyncUnixStream::pair().unwrap();
            assert!(a.local_addr().unwrap().is_unnamed());
            assert!(a.peer_addr().unwrap().is_unnamed());
            assert!(b.local_addr().unwrap().is_unnamed());
            assert!(b.peer_addr().unwrap().is_unnamed());

            a.write_all(b"pair-a").await.unwrap();
            b.write_all(b"pair-b").await.unwrap();
            let mut ba = [0u8; 6];
            let mut bb = [0u8; 6];
            b.read_exact(&mut ba).await.unwrap();
            a.read_exact(&mut bb).await.unwrap();
            assert_eq!(&ba, b"pair-a");
            assert_eq!(&bb, b"pair-b");
        });
    });
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn p2_15_unix_stream_abstract_names_embedded_nul_and_no_file() {
    watchdog(|| {
        run(async {
            let mut name = format!("akku-p2-15-{}-", std::process::id()).into_bytes();
            name.extend_from_slice(b"a\0b\0c");
            let addr = UnixSocketAddr::from_abstract_name(&name).unwrap();
            assert_eq!(addr.as_abstract_name(), Some(name.as_slice()));
            assert!(addr.as_pathname().is_none());
            assert!(!addr.is_unnamed());

            let listener = AsyncUnixListener::bind_addr(&addr).unwrap();
            assert_eq!(
                listener.local_addr().unwrap().as_abstract_name(),
                Some(name.as_slice())
            );

            // Abstract sockets create no filesystem entry.
            let as_str = String::from_utf8_lossy(&name);
            assert!(!std::path::Path::new(as_str.as_ref()).exists());

            let addr_c = addr.clone();
            let c_task =
                task::spawn(async move { AsyncUnixStream::connect_addr(&addr_c).await.unwrap() });
            let (mut server, _) = listener.accept().await.unwrap();
            let mut client = c_task.await.unwrap();

            client.write_all(b"abstract-ok").await.unwrap();
            let mut buf = [0u8; 11];
            server.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"abstract-ok");
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_16_unix_path_limits_validation_and_no_fd_leak() {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const MAX_PATH: usize = 103;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const MAX_PATH: usize = 107;

    watchdog_exclusive(|| {
        let dir = TempSocketDir::new("u16");
        let prefix = dir.0.as_os_str().as_encoded_bytes();
        // Need prefix + "/" + leaf == MAX_PATH.
        assert!(
            prefix.len() + 2 <= MAX_PATH,
            "temp dir prefix too long for limit test: {}",
            prefix.len()
        );
        let leaf_len = MAX_PATH - prefix.len() - 1;
        let max_path = dir.path(&"a".repeat(leaf_len));
        let over_path = dir.path(&"a".repeat(leaf_len + 1));
        assert_eq!(max_path.as_os_str().len(), MAX_PATH);
        assert_eq!(over_path.as_os_str().len(), MAX_PATH + 1);

        run(async {
            // Maximum path length succeeds end-to-end.
            let addr = UnixSocketAddr::from_pathname(&max_path).unwrap();
            assert_eq!(addr.as_pathname(), Some(max_path.as_path()));
            let listener = AsyncUnixListener::bind(&max_path).unwrap();
            let mp = max_path.clone();
            let c_task = task::spawn(async move { AsyncUnixStream::connect(&mp).await.unwrap() });
            let (_server, _) = listener.accept().await.unwrap();
            let _client = c_task.await.unwrap();
            drop(_server);
            drop(_client);
            drop(listener);

            // Rejections must all return InvalidInput without creating any socket/fd.
            let fds_before = open_fd_count();
            for bad in [
                over_path.as_path(),
                std::path::Path::new(""),
                std::path::Path::new("foo\0bar"),
                std::path::Path::new("\0leading"),
            ] {
                assert_eq!(
                    UnixSocketAddr::from_pathname(bad).unwrap_err().kind(),
                    ErrorKind::InvalidInput,
                    "from_pathname({bad:?})"
                );
                assert_eq!(
                    AsyncUnixListener::bind(bad).unwrap_err().kind(),
                    ErrorKind::InvalidInput,
                    "bind({bad:?})"
                );
                assert_eq!(
                    AsyncUnixStream::connect(bad).await.unwrap_err().kind(),
                    ErrorKind::InvalidInput,
                    "connect({bad:?})"
                );
            }
            let fds_after = open_fd_count();
            assert_eq!(
                fds_before, fds_after,
                "rejected Unix paths leaked file descriptors"
            );
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_17_unix_listener_unlink_on_drop_policy() {
    watchdog(|| {
        let dir = TempSocketDir::new("u17");
        run(async {
            // 1. Default drop removes the socket file.
            let p1 = dir.path("default.sock");
            let l1 = AsyncUnixListener::bind(&p1).unwrap();
            assert!(p1.exists());
            drop(l1);
            assert!(!p1.exists(), "default drop must unlink socket file");

            // 2. set_unlink_on_drop(false) keeps the file.
            let p2 = dir.path("keep.sock");
            let l2 = AsyncUnixListener::bind(&p2).unwrap();
            l2.set_unlink_on_drop(false).unwrap();
            drop(l2);
            assert!(p2.exists(), "set_unlink_on_drop(false) must keep file");
            std::fs::remove_file(&p2).unwrap();

            // 3. Rename away + rebind with a second listener; dropping the
            //    first must NOT remove the second listener's socket file.
            let p3 = dir.path("identity.sock");
            let p3_moved = dir.path("identity.moved.sock");
            let first = AsyncUnixListener::bind(&p3).unwrap();
            std::fs::rename(&p3, &p3_moved).unwrap();
            let second = AsyncUnixListener::bind(&p3).unwrap();
            drop(first);
            assert!(
                p3.exists(),
                "dropping old listener must not unlink replaced socket file"
            );
            assert!(p3_moved.exists());
            drop(second);
            assert!(!p3.exists());
            let _ = std::fs::remove_file(&p3_moved);

            // 4. into_std disarms unlink-on-drop; from_std starts disabled.
            let p4 = dir.path("std.sock");
            let l4 = AsyncUnixListener::bind(&p4).unwrap();
            let std_l4 = l4.into_std().unwrap();
            assert!(p4.exists(), "into_std must not unlink file");
            let adopted = AsyncUnixListener::from_std(std_l4).unwrap();
            drop(adopted);
            assert!(p4.exists(), "from_std must start with unlink-on-drop off");
            std::fs::remove_file(&p4).unwrap();

            // 5. Abstract addresses touch nothing and reject enabling unlink.
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                let abs_name = format!("akku-p2-17-abs-{}", std::process::id());
                let abs = UnixSocketAddr::from_abstract_name(abs_name.as_bytes()).unwrap();
                let l_abs = AsyncUnixListener::bind_addr(&abs).unwrap();
                assert_eq!(
                    l_abs.set_unlink_on_drop(true).unwrap_err().kind(),
                    ErrorKind::InvalidInput
                );
                drop(l_abs);
            }

            // 6. Binding over an existing regular file returns AddrInUse and
            //    never deletes that file.
            let p6 = dir.path("existing.txt");
            std::fs::write(&p6, b"precious").unwrap();
            let err = AsyncUnixListener::bind(&p6).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::AddrInUse);
            assert_eq!(std::fs::read(&p6).unwrap(), b"precious");
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_18_unix_stream_missing_and_stale_socket_errors() {
    watchdog(|| {
        let dir = TempSocketDir::new("u18");
        run(async {
            let missing = dir.path("does-not-exist.sock");
            let err = AsyncUnixStream::connect(&missing).await.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::NotFound);

            // Create a stale socket file by disabling unlink-on-drop.
            let stale = dir.path("stale.sock");
            let listener = AsyncUnixListener::bind(&stale).unwrap();
            listener.set_unlink_on_drop(false).unwrap();
            drop(listener);
            assert!(stale.exists());

            let err = AsyncUnixStream::connect(&stale).await.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::ConnectionRefused);
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_19_unix_stream_full_backlog_nonblocking_and_sibling_runs() {
    watchdog_short(|| {
        let dir = TempSocketDir::new("u19");
        let path = dir.path("backlog.sock");
        run(async {
            let listener = AsyncUnixListener::bind(&path).unwrap();

            let turns = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sibling = spawn_turn_counter(&turns, &stop);
            let start_turns = turns.load(Ordering::SeqCst);

            let mut clients = Vec::new();
            let mut full_err = None;
            let mut first_poll_pending = false;

            for _ in 0..4096 {
                let mut fut = pin!(AsyncUnixStream::connect(&path));
                let outcome = match poll_once(&mut fut).await {
                    Some(res) => res,
                    None => {
                        first_poll_pending = true;
                        fut.await
                    }
                };
                match outcome {
                    Ok(stream) => {
                        clients.push(stream);
                        task::yield_now().await;
                    }
                    Err(e) => {
                        full_err = Some((e, first_poll_pending));
                        break;
                    }
                }
            }

            let (err, saw_pending) = full_err.expect("backlog must eventually saturate");
            #[cfg(target_os = "linux")]
            {
                assert_eq!(err.kind(), ErrorKind::WouldBlock, "got {err:?}");
                assert_eq!(
                    err.raw_os_error(),
                    Some(11),
                    "expected EAGAIN (11), got {err:?}"
                );
                assert!(
                    !saw_pending,
                    "Linux EAGAIN on AF_UNIX connect must return directly without Pending (Q14)"
                );
            }
            #[cfg(target_os = "macos")]
            {
                let _ = saw_pending;
                assert_eq!(err.kind(), ErrorKind::ConnectionRefused, "got {err:?}");
                // Record std blocking UnixStream::connect behaviour against the
                // saturated backlog (missing item g).
                let std_res = std::os::unix::net::UnixStream::connect(&path);
                eprintln!("p2_19 Darwin std::os::unix::net::UnixStream::connect full backlog => {std_res:?}");
                assert!(std_res.is_err());
            }

            let sibling_delta = turns.load(Ordering::SeqCst) - start_turns;
            stop.store(true, Ordering::SeqCst);
            sibling.await.unwrap();
            assert!(
                sibling_delta > 0,
                "sibling task must keep running during full-backlog connects"
            );

            drop(clients);
            drop(listener);
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_20_unix_and_tcp_streams_broken_pipe_public_api() {
    // Note: the child-process SIGPIPE=SIG_DFL probe (using test-only signal()
    // in an allowlisted sys/*.rs file) runs in `--lib` under
    // `net::tests_sigpipe::p2_20_sigpipe_child_probe_streams`. This test
    // exercises the same public-API BrokenPipe paths from the integration test
    // binary.
    watchdog(|| {
        let dir = TempSocketDir::new("u20");
        let path = dir.path("sigpipe.sock");
        run(async {
            // 1. Connected + accepted Unix streams.
            let listener = AsyncUnixListener::bind(&path).unwrap();
            let p = path.clone();
            let c_task = task::spawn(async move { AsyncUnixStream::connect(&p).await.unwrap() });
            let (mut server, _) = listener.accept().await.unwrap();
            let client = c_task.await.unwrap();
            drop(client);
            let mut got_broken = false;
            for _ in 0..64 {
                match server.write(b"after-drop").await {
                    Err(e)
                        if matches!(
                            e.kind(),
                            ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                        ) =>
                    {
                        got_broken = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => panic!("unexpected write error: {e:?}"),
                }
            }
            assert!(got_broken, "expected BrokenPipe on accepted Unix stream");

            // 2. pair() Unix streams.
            let (mut a, b) = AsyncUnixStream::pair().unwrap();
            drop(b);
            let err = a.write(b"pair-drop").await.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::BrokenPipe);
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_21_cloexec_sockets_not_inherited() {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::FileTypeExt as _;

    const CHILD_ENV: &str = "AKKU_NET_P2_21_CLOEXEC_FDS";
    if let Ok(fd_list) = std::env::var(CHILD_ENV) {
        for raw in fd_list.split(',').filter(|s| !s.is_empty()) {
            let fd: i32 = raw.parse().expect("valid fd integer");
            let dev_fd = format!("/dev/fd/{fd}");
            if let Ok(meta) = std::fs::metadata(&dev_fd) {
                assert!(
                    !meta.file_type().is_socket(),
                    "child inherited open socket fd {fd} ({dev_fd})"
                );
            }
        }
        return;
    }

    watchdog_exclusive(|| {
        let dir = TempSocketDir::new("u21");
        let unix_path = dir.path("cloexec.sock");
        run(async {
            let unix_listener = AsyncUnixListener::bind(&unix_path).unwrap();
            let up = unix_path.clone();
            let u_conn = task::spawn(async move { AsyncUnixStream::connect(&up).await.unwrap() });
            let (unix_accepted, _) = unix_listener.accept().await.unwrap();
            let unix_connected = u_conn.await.unwrap();

            let tcp_listener = AsyncTcpListener::bind_with(loopback_v4(), &SocketOpts::new())
                .await
                .unwrap();
            let tcp_addr = tcp_listener.local_addr().unwrap();
            let t_conn = task::spawn(async move {
                AsyncTcpStream::connect_with(tcp_addr, &SocketOpts::new())
                    .await
                    .unwrap()
            });
            let (_tcp_accepted, _) = tcp_listener.accept().await.unwrap();
            let tcp_connected = t_conn.await.unwrap();

            let udp = AsyncUdpSocket::bind_with(loopback_v4(), &SocketOpts::new())
                .await
                .unwrap();

            let fds = [
                ("unix_connected", unix_connected.as_raw_fd()),
                ("unix_accepted", unix_accepted.as_raw_fd()),
                ("tcp_bind_with", tcp_listener.as_raw_fd()),
                ("tcp_connect_with", tcp_connected.as_raw_fd()),
                ("udp_bind_with", udp.as_raw_fd()),
            ];

            #[cfg(target_os = "linux")]
            {
                const O_CLOEXEC: u64 = 0o2_000_000;
                for (label, fd) in fds {
                    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))
                        .expect("read /proc/self/fdinfo");
                    let flags_line = info
                        .lines()
                        .find(|l| l.starts_with("flags:"))
                        .expect("flags line in fdinfo");
                    let octal = flags_line.trim_start_matches("flags:").trim();
                    let flags = u64::from_str_radix(octal, 8).expect("octal flags");
                    assert_ne!(
                        flags & O_CLOEXEC,
                        0,
                        "{label} (fd {fd}) missing O_CLOEXEC in /proc/self/fdinfo (flags=0o{flags:o})"
                    );
                }
            }

            let fd_csv = fds
                .iter()
                .map(|(_, fd)| fd.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let exe = std::env::current_exe().expect("current_exe");
            let output = std::process::Command::new(exe)
                .env(CHILD_ENV, &fd_csv)
                .args([
                    "--exact",
                    "p2_21_cloexec_sockets_not_inherited",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .output()
                .expect("spawn CLOEXEC child");
            assert!(
                output.status.success(),
                "CLOEXEC child failed ({:?}):\nstdout: {}\nstderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );

            // Keep sockets alive until after child finishes.
            drop((
                unix_connected,
                unix_accepted,
                unix_listener,
                tcp_connected,
                _tcp_accepted,
                tcp_listener,
                udp,
            ));
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_22_unix_stream_backpressure_4mib_byte_exact() {
    const SIZE: usize = 4 * 1024 * 1024;
    watchdog(|| {
        let dir = TempSocketDir::new("u22");
        let path = dir.path("bp.sock");
        let expected = pseudo_random_bytes(SIZE, 22);
        let payload = expected.clone();
        let received = run(async move {
            let listener = AsyncUnixListener::bind(&path).unwrap();
            let p = path.clone();
            let writer = task::spawn(async move {
                let mut client = AsyncUnixStream::connect(&p).await.unwrap();
                client.write_all(&payload).await.unwrap();
                client.shutdown().await.unwrap();
            });
            let (mut server, _) = listener.accept().await.unwrap();
            let got = read_to_end(&mut server).await.unwrap();
            writer.await.unwrap();
            got
        });
        assert_eq!(received.len(), expected.len());
        assert_eq!(received, expected);
    });
}

#[cfg(unix)]
#[test]
fn p2_23_unix_stream_drop_cancel_and_300_cycles() {
    watchdog(|| {
        let dir = TempSocketDir::new("u23");
        let path = dir.path("cycles.sock");
        run(async {
            let listener = AsyncUnixListener::bind(&path).unwrap();

            // Cancel an in-flight accept future by dropping it after Pending.
            {
                let mut accept_fut = pin!(listener.accept());
                assert!(poll_once(&mut accept_fut).await.is_none());
            }

            // Cancel a connect future by dropping it after polling once.
            {
                let mut connect_fut = pin!(AsyncUnixStream::connect(&path));
                let _ = poll_once(&mut connect_fut).await;
            }
            // Drain any connection that may have completed before drop.
            {
                let mut drain_fut = pin!(listener.accept());
                let _ = poll_once(&mut drain_fut).await;
            }

            // 300 clean connect/accept/echo/drop cycles.
            for i in 0..300u32 {
                let p = path.clone();
                let c_task =
                    task::spawn(async move { AsyncUnixStream::connect(&p).await.unwrap() });
                let (mut server, _) = listener.accept().await.unwrap();
                let mut client = c_task.await.unwrap();
                let msg = i.to_le_bytes();
                client.write_all(&msg).await.unwrap();
                let mut buf = [0u8; 4];
                server.read_exact(&mut buf).await.unwrap();
                assert_eq!(buf, msg);
            }
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_23b_unix_listener_unlinks_before_close_under_rebind_race() {
    watchdog(|| {
        let dir = TempSocketDir::new("u23b");
        let path = dir.path("race.sock");
        run(async {
            for round in 0..20 {
                let l1 = AsyncUnixListener::bind(&path).unwrap();
                let p_thread = path.clone();
                let racer = thread::spawn(move || loop {
                    match std::os::unix::net::UnixListener::bind(&p_thread) {
                        Ok(l2) => return l2,
                        Err(e) if e.kind() == ErrorKind::AddrInUse => {
                            thread::yield_now();
                        }
                        Err(e) => panic!("round {round}: unexpected bind error: {e:?}"),
                    }
                });
                thread::sleep(Duration::from_millis(2));
                drop(l1);
                let l2 = racer.join().unwrap();
                assert!(
                    path.exists(),
                    "round {round}: l1 drop unlinked l2's newly bound socket file"
                );
                // Verify l2 is genuinely reachable at `path`.
                let probe = std::os::unix::net::UnixStream::connect(&path);
                assert!(
                    probe.is_ok(),
                    "round {round}: l2 unreachable after race: {probe:?}"
                );
                drop(probe);
                drop(l2);
                let _ = std::fs::remove_file(&path);
            }
        });
    });
}

// --- Phase 2: Windows AF_UNIX streams (tests W1-W6) -------------------------

#[cfg(windows)]
#[test]
fn p2_w1_windows_af_unix_echo_and_halves() {
    watchdog(|| {
        let dir = TempSocketDir::new("w1");
        let path = dir.path("echo.sock");
        run(async {
            let listener = AsyncUnixListener::bind(&path).unwrap();
            assert_eq!(
                listener.local_addr().unwrap().as_pathname(),
                Some(path.as_path())
            );

            let p1 = path.clone();
            let c_task = task::spawn(async move { AsyncUnixStream::connect(&p1).await.unwrap() });
            let (mut server, peer) = listener.accept().await.unwrap();
            let mut client = c_task.await.unwrap();
            assert!(peer.is_unnamed());
            assert!(server.peer_addr().unwrap().is_unnamed());

            client.write_all(b"win-unix").await.unwrap();
            let mut buf = [0u8; 8];
            server.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"win-unix");

            // Borrowed split().
            let (mut cr, mut cw) = client.split();
            let (mut sr, mut sw) = server.split();
            cw.write_all(b"ab").await.unwrap();
            sw.write_all(b"cd").await.unwrap();
            let mut b1 = [0u8; 2];
            let mut b2 = [0u8; 2];
            sr.read_exact(&mut b1).await.unwrap();
            cr.read_exact(&mut b2).await.unwrap();
            assert_eq!(&b1, b"ab");
            assert_eq!(&b2, b"cd");

            // Owned into_split().
            let (mut cr, mut cw) = client.into_split();
            let (mut sr, mut sw) = server.into_split();
            let t1 = task::spawn(async move {
                cw.write_all(b"owned-c").await.unwrap();
                let mut r = [0u8; 7];
                cr.read_exact(&mut r).await.unwrap();
                r
            });
            let t2 = task::spawn(async move {
                sw.write_all(b"owned-s").await.unwrap();
                let mut r = [0u8; 7];
                sr.read_exact(&mut r).await.unwrap();
                r
            });
            assert_eq!(&t1.await.unwrap(), b"owned-s");
            assert_eq!(&t2.await.unwrap(), b"owned-c");
        });
    });
}

#[cfg(windows)]
#[test]
fn p2_w2_windows_af_unix_half_close_and_drop_eof() {
    watchdog(|| {
        let dir = TempSocketDir::new("w2");
        let path = dir.path("half.sock");
        run(async {
            let listener = AsyncUnixListener::bind(&path).unwrap();

            let p1 = path.clone();
            let c_task = task::spawn(async move { AsyncUnixStream::connect(&p1).await.unwrap() });
            let (mut server, _) = listener.accept().await.unwrap();
            let mut client = c_task.await.unwrap();

            client.write_all(b"req").await.unwrap();
            client.shutdown().await.unwrap();
            assert_eq!(read_to_end(&mut server).await.unwrap(), b"req");

            server.write_all(b"resp").await.unwrap();
            server.shutdown().await.unwrap();
            assert_eq!(read_to_end(&mut client).await.unwrap(), b"resp");

            // Dropping AsyncUnixOwnedWriteHalf delivers EOF to peer.
            let p2 = path.clone();
            let c2_task = task::spawn(async move { AsyncUnixStream::connect(&p2).await.unwrap() });
            let (mut server2, _) = listener.accept().await.unwrap();
            let client2 = c2_task.await.unwrap();
            let (_cr, mut cw) = client2.into_split();
            cw.write_all(b"drop-eof").await.unwrap();
            drop(cw);
            assert_eq!(read_to_end(&mut server2).await.unwrap(), b"drop-eof");
        });
    });
}

#[cfg(windows)]
#[test]
fn p2_w3_windows_af_unix_unlink_on_drop() {
    watchdog(|| {
        let dir = TempSocketDir::new("w3");
        run(async {
            // Drop removes the reparse-point socket file.
            let p1 = dir.path("default.sock");
            let l1 = AsyncUnixListener::bind(&p1).unwrap();
            assert!(p1.exists());
            drop(l1);
            assert!(!p1.exists());

            // set_unlink_on_drop(false) keeps it.
            let p2 = dir.path("keep.sock");
            let l2 = AsyncUnixListener::bind(&p2).unwrap();
            l2.set_unlink_on_drop(false).unwrap();
            drop(l2);
            assert!(p2.exists());
            std::fs::remove_file(&p2).unwrap();

            // Replaced path (different creation_time) is not removed when first drops.
            let p3 = dir.path("id.sock");
            let p3_moved = dir.path("id.moved.sock");
            let first = AsyncUnixListener::bind(&p3).unwrap();
            std::fs::rename(&p3, &p3_moved).unwrap();
            // Ensure distinct Windows FILETIME creation_time (100ns tick; sleep 20ms).
            thread::sleep(Duration::from_millis(20));
            let second = AsyncUnixListener::bind(&p3).unwrap();
            drop(first);
            assert!(
                p3.exists(),
                "dropping first listener must not delete second's file"
            );
            drop(second);
            assert!(!p3.exists());
            let _ = std::fs::remove_file(&p3_moved);

            // Binding over an existing regular file is AddrInUse and never deletes it.
            let p4 = dir.path("existing.txt");
            std::fs::write(&p4, b"keep-me").unwrap();
            let err = AsyncUnixListener::bind(&p4).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::AddrInUse);
            assert_eq!(std::fs::read(&p4).unwrap(), b"keep-me");
        });
    });
}

#[cfg(windows)]
#[test]
fn p2_w4_windows_af_unix_full_backlog() {
    watchdog_short(|| {
        let dir = TempSocketDir::new("w4");
        let path = dir.path("backlog.sock");
        run(async {
            let listener = AsyncUnixListener::bind(&path).unwrap();
            let turns = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sibling = spawn_turn_counter(&turns, &stop);
            let start = turns.load(Ordering::SeqCst);

            let mut clients = Vec::new();
            let mut err = None;
            for _ in 0..4096 {
                match AsyncUnixStream::connect(&path).await {
                    Ok(s) => {
                        clients.push(s);
                        task::yield_now().await;
                    }
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }

            let e = err.expect("Windows AF_UNIX backlog must eventually return an error");
            eprintln!("p2_w4 Windows AF_UNIX full-backlog error => {e:?}");
            let delta = turns.load(Ordering::SeqCst) - start;
            stop.store(true, Ordering::SeqCst);
            sibling.await.unwrap();
            assert!(delta > 0, "sibling must keep running during full backlog");
            drop(clients);
            drop(listener);
        });
    });
}

#[cfg(windows)]
#[test]
fn p2_w5_windows_af_unix_path_validation() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt as _;

    const MAX_PATH: usize = 107;
    watchdog(|| {
        let dir = TempSocketDir::new("w5");
        let prefix = dir.0.to_str().expect("UTF-8 temp dir");
        assert!(prefix.len() + 2 <= MAX_PATH);
        let leaf_len = MAX_PATH - prefix.len() - 1;
        let max_path = dir.path(&"a".repeat(leaf_len));
        let over_path = dir.path(&"a".repeat(leaf_len + 1));
        assert_eq!(max_path.to_str().unwrap().len(), MAX_PATH);
        assert_eq!(over_path.to_str().unwrap().len(), MAX_PATH + 1);

        run(async {
            let addr = UnixSocketAddr::from_pathname(&max_path).unwrap();
            assert_eq!(addr.as_pathname(), Some(max_path.as_path()));
            let listener = AsyncUnixListener::bind(&max_path).unwrap();
            drop(listener);

            let non_utf8 =
                std::path::PathBuf::from(OsString::from_wide(&[b'a' as u16, 0xD800, b'b' as u16]));
            for bad in [
                over_path.as_path(),
                std::path::Path::new(""),
                std::path::Path::new("foo\0bar"),
                non_utf8.as_path(),
            ] {
                assert_eq!(
                    UnixSocketAddr::from_pathname(bad).unwrap_err().kind(),
                    ErrorKind::InvalidInput
                );
                assert_eq!(
                    AsyncUnixListener::bind(bad).unwrap_err().kind(),
                    ErrorKind::InvalidInput
                );
                assert_eq!(
                    AsyncUnixStream::connect(bad).await.unwrap_err().kind(),
                    ErrorKind::InvalidInput
                );
            }
        });
    });
}

#[cfg(windows)]
#[test]
fn p2_w6_windows_af_unix_backpressure_and_idle_no_spin() {
    const SIZE: usize = 4 * 1024 * 1024;
    watchdog(|| {
        let dir = TempSocketDir::new("w6");
        let path = dir.path("bp.sock");
        let expected = pseudo_random_bytes(SIZE, 6);
        let payload = expected.clone();
        let (received, idle_polls) = run(async move {
            let listener = AsyncUnixListener::bind(&path).unwrap();
            let p1 = path.clone();
            let writer = task::spawn(async move {
                let mut c = AsyncUnixStream::connect(&p1).await.unwrap();
                c.write_all(&payload).await.unwrap();
                c.shutdown().await.unwrap();
            });
            let (mut server, _) = listener.accept().await.unwrap();
            let got = read_to_end(&mut server).await.unwrap();
            writer.await.unwrap();

            // Idle pair parks without spinning WSAPoll, then wakes on write.
            let p2 = path.clone();
            let c2 = task::spawn(async move { AsyncUnixStream::connect(&p2).await.unwrap() });
            let (mut s2, _) = listener.accept().await.unwrap();
            let mut client2 = c2.await.unwrap();

            let polls = Arc::new(AtomicUsize::new(0));
            let gate = Gate::default();
            let gate_t = gate.clone();
            let releaser = thread::spawn(move || {
                thread::sleep(Duration::from_millis(80));
                gate_t.open();
            });
            let polls_r = Arc::clone(&polls);
            let reader = task::spawn(async move {
                let fut = CountPolls {
                    inner: Box::pin(async move {
                        let mut b = [0u8; 4];
                        s2.read_exact(&mut b).await.unwrap();
                        b
                    }),
                    polls: polls_r,
                };
                fut.await
            });
            gate.wait().await;
            releaser.join().unwrap();
            client2.write_all(b"wake").await.unwrap();
            assert_eq!(&reader.await.unwrap(), b"wake");
            (got, polls.load(Ordering::SeqCst))
        });
        assert_eq!(received, expected);
        assert!(
            idle_polls <= 4,
            "idle Windows AF_UNIX stream spun WSAPoll ({idle_polls} polls)"
        );
    });
}

// --- Phase 2: Unix datagrams (tests 24-29) ----------------------------------

#[cfg(unix)]
#[test]
fn p2_24_unix_datagram_bind_unbound_pair_send_to_and_connect() {
    watchdog(|| {
        let dir = TempSocketDir::new("u24");
        let srv_path = dir.path("srv.dgram");
        let cli_path = dir.path("cli.dgram");
        run(async {
            // bind + unbound send_to -> recv_from reports unnamed sender.
            let srv = AsyncUnixDatagram::bind(&srv_path).unwrap();
            assert_eq!(
                srv.local_addr().unwrap().as_pathname(),
                Some(srv_path.as_path())
            );
            let unbound = AsyncUnixDatagram::unbound().unwrap();
            assert!(unbound.local_addr().unwrap().is_unnamed());

            assert_eq!(
                unbound.send_to(b"from-unbound", &srv_path).await.unwrap(),
                12
            );
            let mut buf = [0u8; 64];
            let (n, from) = srv.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"from-unbound");
            assert!(from.is_unnamed(), "unbound sender must report is_unnamed()");

            // Bound sender + connect + send/recv.
            let cli = AsyncUnixDatagram::bind(&cli_path).unwrap();
            cli.connect(&srv_path).unwrap();
            assert_eq!(
                cli.peer_addr().unwrap().as_pathname(),
                Some(srv_path.as_path())
            );
            assert_eq!(cli.send(b"connected-dg").await.unwrap(), 12);
            let (n, from) = srv.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"connected-dg");
            assert_eq!(from.as_pathname(), Some(cli_path.as_path()));

            // pair() echo + unnamed.
            let (a, b) = AsyncUnixDatagram::pair().unwrap();
            assert!(a.local_addr().unwrap().is_unnamed());
            assert!(b.local_addr().unwrap().is_unnamed());
            assert_eq!(a.send(b"p1").await.unwrap(), 2);
            let n = b.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"p1");
            assert_eq!(b.send(b"p2").await.unwrap(), 2);
            let n = a.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"p2");

            // Drop unlinks bound datagram socket files by default.
            drop(srv);
            drop(cli);
            assert!(!srv_path.exists());
            assert!(!cli_path.exists());
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_25_unix_datagram_zero_length_and_truncation_per_os() {
    watchdog(|| {
        run(async {
            let (tx, rx) = AsyncUnixDatagram::pair().unwrap();

            // Empty datagram arrives as Ok(0) and is not EOF.
            assert_eq!(tx.send(&[]).await.unwrap(), 0);
            assert_eq!(tx.send(b"after-zero").await.unwrap(), 10);
            let mut buf = [0u8; 64];
            assert_eq!(rx.recv(&mut buf).await.unwrap(), 0);
            let n = rx.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"after-zero");

            // recv(&mut []) on a non-empty datagram (per-OS, Q9).
            assert_eq!(tx.send(b"non-empty-dg").await.unwrap(), 12);
            assert_eq!(tx.send(b"after-empty-buf").await.unwrap(), 15);
            let res = rx.recv(&mut []).await;
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            eprintln!("p2_25 Darwin UnixDatagram recv(&mut []) on non-empty => {res:?}");
            assert_eq!(res.unwrap(), 0);
            let n = rx.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"after-empty-buf");

            // Truncation: 1,000 bytes into 10-byte buffer -> Ok(10) + prefix,
            // next datagram intact.
            let payload = pseudo_random_bytes(1000, 25);
            assert_eq!(tx.send(&payload).await.unwrap(), 1000);
            assert_eq!(tx.send(b"post-trunc").await.unwrap(), 10);
            let mut small = [0u8; 10];
            assert_eq!(rx.recv(&mut small).await.unwrap(), 10);
            assert_eq!(&small[..], &payload[..10]);
            let n = rx.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"post-trunc");
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_26_unix_datagram_size_limit_2048_macos_vs_linux() {
    watchdog(|| {
        run(async {
            let (tx, rx) = AsyncUnixDatagram::pair().unwrap();
            let big = pseudo_random_bytes(4_096, 26);

            #[cfg(target_os = "macos")]
            {
                let err = tx.send(&big).await.unwrap_err();
                assert_eq!(
                    err.raw_os_error(),
                    Some(40),
                    "expected macOS EMSGSIZE (40) for >2,048-byte Unix datagram, got {err:?}"
                );
            }
            #[cfg(target_os = "linux")]
            {
                assert_eq!(tx.send(&big).await.unwrap(), 4_096);
                let mut buf = vec![0u8; 4_096];
                let n = rx.recv(&mut buf).await.unwrap();
                assert_eq!(n, 4_096);
                assert_eq!(buf, big);
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = (tx, rx, big);
            }
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_27_unix_datagram_connected_send_after_peer_drop_errors() {
    // Note: the child-process SIGPIPE=SIG_DFL probe (and diagnostic std probe
    // on Linux) runs in `--lib` under
    // `net::tests_sigpipe::p2_27_sigpipe_child_probe_unix_datagram`.
    watchdog(|| {
        run(async {
            let (tx, rx) = AsyncUnixDatagram::pair().unwrap();
            drop(rx);
            let err = tx.send(b"to-dropped-peer").await.unwrap_err();
            assert!(
                matches!(
                    err.kind(),
                    ErrorKind::ConnectionRefused
                        | ErrorKind::NotConnected
                        | ErrorKind::BrokenPipe
                        | ErrorKind::ConnectionReset
                ),
                "unexpected error sending to dropped Unix datagram peer: {err:?}"
            );
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_28_unix_datagram_receiver_full_backpressure() {
    watchdog_short(|| {
        let dir = TempSocketDir::new("u28");
        let path = dir.path("full.dgram");
        run(async {
            let rx = AsyncUnixDatagram::bind(&path).unwrap();
            let tx_unconn = AsyncUnixDatagram::unbound().unwrap();
            let payload = [0x5Au8; 1024];

            #[cfg(target_os = "macos")]
            {
                // On Darwin, an unconnected send_to to a full receiver returns
                // ENOBUFS (55) immediately.
                let mut got_enobufs = false;
                for _ in 0..4096 {
                    match tx_unconn.send_to(&payload, &path).await {
                        Ok(_) => {}
                        Err(e) => {
                            assert_eq!(
                                e.raw_os_error(),
                                Some(55),
                                "expected macOS ENOBUFS (55), got {e:?}"
                            );
                            got_enobufs = true;
                            break;
                        }
                    }
                }
                assert!(got_enobufs, "expected ENOBUFS on macOS full receiver queue");
                drop((tx_unconn, rx));
            }

            #[cfg(target_os = "linux")]
            {
                // Fill the receiver queue via non-blocking try_send_to_addr.
                let rx_addr = UnixSocketAddr::from_pathname(&path).unwrap();
                let mut filled = false;
                for _ in 0..8192 {
                    match tx_unconn.try_send_to_addr(&payload, &rx_addr) {
                        Ok(_) => {}
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            filled = true;
                            break;
                        }
                        Err(e) => panic!("unexpected try_send_to_addr error: {e:?}"),
                    }
                }
                assert!(filled, "could not fill Linux Unix datagram receive queue");

                // Unconnected send_to either completes after the receiver
                // drains OR returns WouldBlock directly (P2.3.1 / missing item b).
                let rx = Arc::new(rx);
                let rx_drain = Arc::clone(&rx);
                let drainer = task::spawn(async move {
                    for _ in 0..4 {
                        task::yield_now().await;
                    }
                    let mut buf = [0u8; 2048];
                    let mut drained = 0usize;
                    while rx_drain.try_recv_from(&mut buf).is_ok() {
                        drained += 1;
                    }
                    drained
                });
                let unconn_res = tx_unconn.send_to(&payload, &path).await;
                let _ = drainer.await.unwrap();
                match unconn_res {
                    Ok(n) => assert_eq!(n, payload.len()),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => panic!("unexpected unconnected send_to result: {e:?}"),
                }

                // Connected send must complete after the receiver drains.
                let (conn_tx, conn_rx) = AsyncUnixDatagram::pair().unwrap();
                let mut conn_filled = false;
                for _ in 0..8192 {
                    match conn_tx.try_send(&payload) {
                        Ok(_) => {}
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            conn_filled = true;
                            break;
                        }
                        Err(e) => panic!("unexpected try_send error: {e:?}"),
                    }
                }
                assert!(conn_filled, "could not fill connected Unix datagram queue");

                let drainer2 = task::spawn(async move {
                    for _ in 0..4 {
                        task::yield_now().await;
                    }
                    let mut buf = [0u8; 2048];
                    while conn_rx.try_recv(&mut buf).is_ok() {}
                    conn_rx
                });
                assert_eq!(conn_tx.send(&payload).await.unwrap(), payload.len());
                let _conn_rx = drainer2.await.unwrap();
            }

            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = (rx, tx_unconn, payload);
            }
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_29_unix_datagram_eight_shared_waiters_all_complete() {
    const WAITERS: usize = 8;
    watchdog_short(|| {
        run(async {
            let (tx, rx) = AsyncUnixDatagram::pair().unwrap();
            let rx = Arc::new(rx);

            let handles: Vec<_> = (0..WAITERS)
                .map(|_| {
                    let rx = Arc::clone(&rx);
                    task::spawn(async move {
                        let mut buf = [0u8; 32];
                        let n = rx.recv(&mut buf).await.unwrap();
                        buf[..n].to_vec()
                    })
                })
                .collect();

            for _ in 0..16 {
                task::yield_now().await;
            }

            for i in 0..WAITERS {
                let msg = format!("udg-{i}");
                tx.send(msg.as_bytes()).await.unwrap();
            }

            let mut received = Vec::with_capacity(WAITERS);
            for h in handles {
                received.push(h.await.unwrap());
            }
            received.sort();
            let mut expected: Vec<Vec<u8>> = (0..WAITERS)
                .map(|i| format!("udg-{i}").into_bytes())
                .collect();
            expected.sort();
            assert_eq!(received, expected);
        });
    });
}

// --- Phase 2: SocketOpts (tests 30-41) --------------------------------------

#[test]
fn p2_30_bind_with_default_opts_matches_bind() {
    watchdog(|| {
        run(async {
            let default_opts = SocketOpts::new();
            let plain = AsyncTcpListener::bind(loopback_v4()).await.unwrap();
            let with_opts = AsyncTcpListener::bind_with(loopback_v4(), &default_opts)
                .await
                .unwrap();

            // Active-port collision behaves identically (AddrInUse).
            assert_eq!(
                AsyncTcpListener::bind(plain.local_addr().unwrap())
                    .await
                    .unwrap_err()
                    .kind(),
                ErrorKind::AddrInUse
            );
            assert_eq!(
                AsyncTcpListener::bind_with(with_opts.local_addr().unwrap(), &default_opts)
                    .await
                    .unwrap_err()
                    .kind(),
                ErrorKind::AddrInUse
            );

            // Both accept queued connections cleanly (default backlog 128).
            for listener in [&plain, &with_opts] {
                let addr = listener.local_addr().unwrap();
                let mut clients = Vec::new();
                for _ in 0..8 {
                    clients.push(TcpStream::connect(addr).unwrap());
                }
                for _ in 0..8 {
                    let (_srv, _) = listener.accept().await.unwrap();
                }
                drop(clients);
            }

            // If IPv6 dual-stack probing is possible, default bind_with matches
            // bind's OS-default IPV6_V6ONLY behaviour.
            if std::net::TcpListener::bind("[::]:0").is_ok() {
                let l_plain = AsyncTcpListener::bind("[::]:0").await.unwrap();
                let l_opts = AsyncTcpListener::bind_with("[::]:0", &default_opts)
                    .await
                    .unwrap();
                let v4_to_plain = SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::LOCALHOST),
                    l_plain.local_addr().unwrap().port(),
                );
                let v4_to_opts = SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::LOCALHOST),
                    l_opts.local_addr().unwrap().port(),
                );
                let plain_accepts_v4 = TcpStream::connect(v4_to_plain).is_ok();
                let opts_accepts_v4 = TcpStream::connect(v4_to_opts).is_ok();
                assert_eq!(
                    plain_accepts_v4, opts_accepts_v4,
                    "bind_with(&SocketOpts::new()) must match bind() IPV6_V6ONLY default"
                );
            }
        });
    });
}

#[cfg(unix)]
#[test]
fn p2_31_unix_reuse_port_per_os() {
    watchdog_short(|| {
        run(async {
            // Without reuse_port, a second bind to the same port fails with AddrInUse.
            let l0 =
                AsyncTcpListener::bind_with(loopback_v4(), &SocketOpts::new().reuse_address(true))
                    .await
                    .unwrap();
            let addr0 = l0.local_addr().unwrap();
            let err = AsyncTcpListener::bind_with(addr0, &SocketOpts::new().reuse_address(true))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::AddrInUse);
            drop(l0);

            // With reuse_port(true), two listeners bind to the exact same port.
            let opts = SocketOpts::new().reuse_address(true).reuse_port(true);
            let l1 = AsyncTcpListener::bind_with(loopback_v4(), &opts)
                .await
                .unwrap();
            let shared_addr = l1.local_addr().unwrap();
            let l2 = AsyncTcpListener::bind_with(shared_addr, &opts)
                .await
                .unwrap();
            assert_eq!(l2.local_addr().unwrap(), shared_addr);

            // On Linux/Android, kernel load-balances across reuse_port listeners:
            // connect until each listener has accepted at least one connection.
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                let c1 = Arc::new(AtomicUsize::new(0));
                let c2 = Arc::new(AtomicUsize::new(0));
                let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

                let c1_a = Arc::clone(&c1);
                let done_1 = Arc::clone(&done);
                let t1 = task::spawn(async move {
                    while !done_1.load(Ordering::SeqCst) {
                        if l1.accept().await.is_ok() {
                            c1_a.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });
                let c2_a = Arc::clone(&c2);
                let done_2 = Arc::clone(&done);
                let t2 = task::spawn(async move {
                    while !done_2.load(Ordering::SeqCst) {
                        if l2.accept().await.is_ok() {
                            c2_a.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });

                for _ in 0..256 {
                    if c1.load(Ordering::SeqCst) > 0 && c2.load(Ordering::SeqCst) > 0 {
                        break;
                    }
                    let _ = AsyncTcpStream::connect(shared_addr).await;
                    task::yield_now().await;
                }
                done.store(true, Ordering::SeqCst);
                drop(t1);
                drop(t2);
                assert!(
                    c1.load(Ordering::SeqCst) > 0 && c2.load(Ordering::SeqCst) > 0,
                    "expected both Linux reuse_port listeners to accept (got l1={}, l2={})",
                    c1.load(Ordering::SeqCst),
                    c2.load(Ordering::SeqCst),
                );
            }

            // On Darwin, SO_REUSEPORT does not load-balance; asserting both
            // binds succeeded is the contract (Q11).
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            {
                drop((l1, l2));
            }
        });
    });
}

#[test]
fn p2_32_tcp_reuse_address_hijack_guard_is_addr_in_use() {
    watchdog(|| {
        run(async {
            let active =
                AsyncTcpListener::bind_with(loopback_v4(), &SocketOpts::new().reuse_address(true))
                    .await
                    .unwrap();
            let addr = active.local_addr().unwrap();

            let err = AsyncTcpListener::bind_with(addr, &SocketOpts::new().reuse_address(true))
                .await
                .unwrap_err();
            assert_eq!(
                err.kind(),
                ErrorKind::AddrInUse,
                "reuse_address(true) must never hijack an active TCP listener port"
            );
            drop(active);
        });
    });
}

#[test]
fn p2_33_tcp_rebind_over_time_wait_with_default_opts() {
    watchdog(|| {
        run(async {
            let listener = AsyncTcpListener::bind_with(loopback_v4(), &SocketOpts::new())
                .await
                .unwrap();
            let addr = listener.local_addr().unwrap();

            // Establish a connection where the server initiates active close,
            // leaving the server port in TIME_WAIT.
            let (mut client, mut server) = connected_pair(&listener).await.unwrap();
            server.write_all(b"bye").await.unwrap();
            server.shutdown().await.unwrap();
            drop(server);
            let got = read_to_end(&mut client).await.unwrap();
            assert_eq!(got, b"bye");
            drop(client);
            drop(listener);

            // Rebinding immediately with default options must succeed on all OSes.
            let rebound = AsyncTcpListener::bind_with(addr, &SocketOpts::new())
                .await
                .expect("rebind over TIME_WAIT must succeed");
            assert_eq!(rebound.local_addr().unwrap(), addr);
        });
    });
}

#[test]
fn p2_34_socket_opts_only_v6_true_and_false() {
    watchdog(|| {
        // Construct SocketOpts before any IPv6 availability skip so this test
        // still fails against G3 stubs on IPv4-only containers.
        let opts_v6_only = SocketOpts::new().only_v6(true);
        let opts_dual = SocketOpts::new().only_v6(false);

        if std::net::TcpListener::bind("[::]:0").is_err() {
            eprintln!("skipping p2_34: no IPv6 support on this host");
            return;
        }

        run(async {
            // only_v6(true): IPv4 connection to the same port must be refused.
            let l_only = AsyncTcpListener::bind_with("[::]:0", &opts_v6_only)
                .await
                .unwrap();
            let port_only = l_only.local_addr().unwrap().port();
            let v4_only = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port_only);
            let err = AsyncTcpStream::connect(v4_only).await.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::ConnectionRefused);

            // only_v6(false): IPv4-mapped connection must succeed.
            let l_dual = AsyncTcpListener::bind_with("[::]:0", &opts_dual)
                .await
                .unwrap();
            let port_dual = l_dual.local_addr().unwrap().port();
            let v4_dual = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port_dual);
            let c_task = task::spawn(async move { AsyncTcpStream::connect(v4_dual).await });
            let (_server, _peer) = l_dual.accept().await.unwrap();
            let _client = c_task.await.unwrap().unwrap();
        });
    });
}

#[test]
fn p2_35_socket_opts_round_trip_setters_and_accepted_inheritance() {
    watchdog(|| {
        run(async {
            let ka = TcpKeepalive::new()
                .idle(Duration::from_secs(30))
                .interval(Duration::from_secs(5))
                .retries(4);
            let listener_opts = SocketOpts::new()
                .nodelay(true)
                .keepalive(Some(ka))
                .zero_linger(true)
                .ttl(42)
                .send_buffer_size(64 * 1024)
                .recv_buffer_size(64 * 1024);

            let listener = AsyncTcpListener::bind_with(loopback_v4(), &listener_opts)
                .await
                .unwrap();
            let addr = listener.local_addr().unwrap();

            let client_opts = SocketOpts::new()
                .nodelay(true)
                .keepalive(Some(ka))
                .zero_linger(true)
                .ttl(42)
                .send_buffer_size(64 * 1024)
                .recv_buffer_size(64 * 1024);

            let c_task = task::spawn(async move {
                AsyncTcpStream::connect_with(addr, &client_opts)
                    .await
                    .unwrap()
            });
            let (accepted, _) = listener.accept().await.unwrap();
            let client = c_task.await.unwrap();

            // Client connect_with round-trip.
            assert!(client.nodelay().unwrap());
            assert!(client.keepalive().unwrap());
            assert_eq!(client.linger().unwrap(), Some(Duration::ZERO));
            assert_eq!(client.ttl().unwrap(), 42);
            assert!(client.send_buffer_size().unwrap() >= 64 * 1024);
            assert!(client.recv_buffer_size().unwrap() >= 64 * 1024);

            // Runtime setters round-trip on client.
            client.set_keepalive(None).unwrap();
            assert!(!client.keepalive().unwrap());
            client.set_keepalive(Some(ka)).unwrap();
            assert!(client.keepalive().unwrap());
            client.set_zero_linger(false).unwrap();
            assert_eq!(client.linger().unwrap(), None);
            client.set_zero_linger(true).unwrap();
            assert_eq!(client.linger().unwrap(), Some(Duration::ZERO));
            client.set_send_buffer_size(64 * 1024).unwrap();
            assert!(client.send_buffer_size().unwrap() >= 64 * 1024);
            client.set_recv_buffer_size(64 * 1024).unwrap();
            assert!(client.recv_buffer_size().unwrap() >= 64 * 1024);

            // Accepted stream re-applied options (nodelay, keepalive, zero_linger)
            // and inherited options (ttl, send_buffer_size; Q15).
            assert!(accepted.nodelay().unwrap());
            assert!(accepted.keepalive().unwrap());
            assert_eq!(accepted.linger().unwrap(), Some(Duration::ZERO));
            assert_eq!(
                accepted.ttl().unwrap(),
                42,
                "accepted stream must inherit listener ttl"
            );
            assert!(
                accepted.send_buffer_size().unwrap() >= 64 * 1024,
                "accepted stream must inherit listener send_buffer_size"
            );
        });
    });
}

#[test]
fn p2_35b_accept_path_reset_peer_never_fails_accept() {
    watchdog_short(|| {
        run(async {
            let ka = TcpKeepalive::new().idle(Duration::from_secs(10));
            let opts = SocketOpts::new()
                .nodelay(true)
                .keepalive(Some(ka))
                .zero_linger(true);
            let listener = AsyncTcpListener::bind_with(loopback_v4(), &opts)
                .await
                .unwrap();
            let addr = listener.local_addr().unwrap();

            // Connect and immediately abort with RST (zero_linger + drop)
            // before the server calls accept.
            for _ in 0..4 {
                let abort_client =
                    AsyncTcpStream::connect_with(addr, &SocketOpts::new().zero_linger(true))
                        .await
                        .unwrap();
                drop(abort_client);
            }

            // Now connect a healthy client.
            let healthy = task::spawn(async move {
                let mut c = AsyncTcpStream::connect(addr).await.unwrap();
                c.write_all(b"healthy").await.unwrap();
                c.shutdown().await.unwrap();
                c
            });

            // accept() must never fail due to the reset peers; loop until we
            // receive the healthy client's payload.
            let mut got_healthy = false;
            for _ in 0..8 {
                let (mut srv, _) = listener
                    .accept()
                    .await
                    .expect("accept must never surface an accept-path option error");
                if let Ok(data) = read_to_end(&mut srv).await {
                    if data == b"healthy" {
                        got_healthy = true;
                        break;
                    }
                }
            }
            assert!(
                got_healthy,
                "never received healthy connection after resets"
            );
            let _ = healthy.await.unwrap();
        });
    });
}

#[test]
fn p2_36_zero_linger_drop_delivers_connection_reset() {
    watchdog_short(|| {
        run(async {
            let listener = AsyncTcpListener::bind(loopback_v4()).await.unwrap();
            let addr = listener.local_addr().unwrap();

            let opts = SocketOpts::new().zero_linger(true);
            let accept_task = task::spawn(async move { listener.accept().await.unwrap() });
            let client = AsyncTcpStream::connect_with(addr, &opts).await.unwrap();
            let (mut server, _) = accept_task.await.unwrap();

            // Dropping a zero-linger socket sends RST instead of FIN.
            drop(client);

            let mut buf = [0u8; 16];
            let err = server.read(&mut buf).await.unwrap_err();
            assert_eq!(
                err.kind(),
                ErrorKind::ConnectionReset,
                "expected ConnectionReset after zero_linger drop, got {err:?}"
            );
        });
    });
}

#[test]
fn p2_37_socket_opts_range_validation_and_backlog_u32_max() {
    let body = || {
        run(async {
            #[cfg(unix)]
            let fds_before = open_fd_count();

            let invalid_cases = [
                SocketOpts::new().keepalive(Some(TcpKeepalive::new().idle(Duration::ZERO))),
                SocketOpts::new()
                    .keepalive(Some(TcpKeepalive::new().idle(Duration::from_secs(32_768)))),
                SocketOpts::new().keepalive(Some(TcpKeepalive::new().interval(Duration::ZERO))),
                SocketOpts::new().keepalive(Some(
                    TcpKeepalive::new().interval(Duration::from_secs(32_768)),
                )),
                SocketOpts::new().keepalive(Some(TcpKeepalive::new().retries(0))),
                SocketOpts::new().keepalive(Some(TcpKeepalive::new().retries(128))),
                SocketOpts::new().recv_buffer_size(0),
                SocketOpts::new().recv_buffer_size(i32::MAX as usize + 1),
                SocketOpts::new().send_buffer_size(0),
                SocketOpts::new().send_buffer_size(i32::MAX as usize + 1),
                SocketOpts::new().ttl(0),
                SocketOpts::new().ttl(256),
            ];

            for opts in &invalid_cases {
                let err = AsyncTcpListener::bind_with(loopback_v4(), opts)
                    .await
                    .unwrap_err();
                assert_eq!(err.kind(), ErrorKind::InvalidInput, "opts={opts:?}");
            }

            #[cfg(unix)]
            {
                let fds_after = open_fd_count();
                assert_eq!(
                    fds_before, fds_after,
                    "invalid SocketOpts created or leaked a file descriptor"
                );
            }

            // backlog(u32::MAX) clamps to i32::MAX and succeeds.
            let listener =
                AsyncTcpListener::bind_with(loopback_v4(), &SocketOpts::new().backlog(u32::MAX))
                    .await
                    .unwrap();
            assert_ne!(listener.local_addr().unwrap().port(), 0);
        });
    };

    #[cfg(unix)]
    watchdog_exclusive(body);
    #[cfg(not(unix))]
    watchdog(body);
}

#[test]
fn p2_38_socket_opts_inapplicable_options_return_invalid_input() {
    watchdog(|| {
        run(async {
            // UDP rejects TCP-only options.
            for opts in [
                SocketOpts::new().nodelay(true),
                SocketOpts::new().keepalive(Some(TcpKeepalive::new())),
                SocketOpts::new().zero_linger(true),
                SocketOpts::new().backlog(16),
            ] {
                let err = AsyncUdpSocket::bind_with(loopback_v4(), &opts)
                    .await
                    .unwrap_err();
                assert_eq!(err.kind(), ErrorKind::InvalidInput, "UDP opts={opts:?}");
            }

            // TCP rejects broadcast.
            let bcast = SocketOpts::new().broadcast(true);
            assert_eq!(
                AsyncTcpListener::bind_with(loopback_v4(), &bcast)
                    .await
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidInput
            );
            assert_eq!(
                AsyncTcpStream::connect_with(loopback_v4(), &bcast)
                    .await
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidInput
            );

            // connect_with rejects backlog.
            let backlog = SocketOpts::new().backlog(16);
            assert_eq!(
                AsyncTcpStream::connect_with(loopback_v4(), &backlog)
                    .await
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidInput
            );
        });
    });
}

#[test]
fn p2_39_socket_opts_buffer_sizes_and_macos_cap() {
    watchdog(|| {
        run(async {
            let udp = AsyncUdpSocket::bind_with(
                loopback_v4(),
                &SocketOpts::new()
                    .recv_buffer_size(64 * 1024)
                    .send_buffer_size(64 * 1024),
            )
            .await
            .unwrap();
            assert!(udp.recv_buffer_size().unwrap() >= 64 * 1024);
            assert!(udp.send_buffer_size().unwrap() >= 64 * 1024);

            udp.set_recv_buffer_size(64 * 1024).unwrap();
            udp.set_send_buffer_size(64 * 1024).unwrap();
            assert!(udp.recv_buffer_size().unwrap() >= 64 * 1024);
            assert!(udp.send_buffer_size().unwrap() >= 64 * 1024);

            // On macOS, a size above kern.ipc.maxsockbuf (8 MiB default)
            // returns Err and never panics.
            #[cfg(target_os = "macos")]
            {
                let huge = i32::MAX as usize;
                assert!(udp.set_recv_buffer_size(huge).is_err());
                assert!(AsyncUdpSocket::bind_with(
                    loopback_v4(),
                    &SocketOpts::new().recv_buffer_size(huge),
                )
                .await
                .is_err());
            }
        });
    });
}

#[test]
fn p2_40_connect_with_falls_back_across_addresses() {
    watchdog(|| {
        run(async {
            let listener = AsyncTcpListener::bind(loopback_v4()).await.unwrap();
            let open_v4 = listener.local_addr().unwrap();

            // Prefer a closed IPv6 address first when IPv6 loopback is available;
            // otherwise use a closed IPv4 address.
            let closed_first = if let Ok(v6) = std::net::TcpListener::bind("[::1]:0") {
                let addr = v6.local_addr().unwrap();
                drop(v6);
                addr
            } else {
                closed_port()
            };

            let addrs = [closed_first, open_v4];
            let opts = SocketOpts::new().nodelay(true);
            let accept_task = task::spawn(async move { listener.accept().await.unwrap() });
            let client = AsyncTcpStream::connect_with(&addrs[..], &opts)
                .await
                .unwrap();
            let (_server, _) = accept_task.await.unwrap();
            assert_eq!(client.peer_addr().unwrap(), open_v4);
            assert!(client.nodelay().unwrap());
        });
    });
}

#[test]
fn p2_41_udp_multicast_port_sharing() {
    watchdog(|| {
        run(async {
            #[cfg(windows)]
            let opts = SocketOpts::new().reuse_address(true);
            #[cfg(unix)]
            let opts = SocketOpts::new().reuse_address(true).reuse_port(true);

            let s1 = AsyncUdpSocket::bind_with(loopback_v4(), &opts)
                .await
                .unwrap();
            let shared_addr = s1.local_addr().unwrap();
            let s2 = AsyncUdpSocket::bind_with(shared_addr, &opts).await.unwrap();
            assert_eq!(s2.local_addr().unwrap(), shared_addr);
        });
    });
}

// --- Phase 2: TCP peek (tests 42-44) ----------------------------------------

#[test]
fn p2_42_tcp_peek_then_awaited_read_preserves_readiness() {
    watchdog_short(|| {
        run(async {
            let listener = AsyncTcpListener::bind(loopback_v4()).await.unwrap();
            let (mut client, mut server) = connected_pair(&listener).await.unwrap();

            client.write_all(b"peek-tcp").await.unwrap();

            let mut p1 = [0u8; 8];
            let n1 = server.peek(&mut p1).await.unwrap();
            assert!(n1 > 0 && n1 <= 8);
            assert_eq!(&p1[..n1], &b"peek-tcp"[..n1]);

            // Repeated peek still sees the same leading bytes.
            let mut p2 = [0u8; 8];
            let n2 = server.peek(&mut p2).await.unwrap();
            assert!(n2 >= n1);
            assert_eq!(&p2[..n1], &p1[..n1]);

            // Awaited read must not hang (ET / WSAPoll readiness kept after peek).
            let mut full = [0u8; 8];
            server.read_exact(&mut full).await.unwrap();
            assert_eq!(&full, b"peek-tcp");
        });
    });
}

#[test]
fn p2_43_tcp_peek_eof_sticky_and_try_peek_would_block_clears_readiness() {
    watchdog_short(|| {
        run(async {
            let listener = AsyncTcpListener::bind(loopback_v4()).await.unwrap();
            let (mut client, mut server) = connected_pair(&listener).await.unwrap();

            // Idle try_peek returns WouldBlock and clears readiness so a later
            // peek parks until data arrives.
            let mut buf = [0u8; 16];
            assert_eq!(
                server.try_peek(&mut buf).unwrap_err().kind(),
                ErrorKind::WouldBlock
            );
            {
                let mut peek_fut = pin!(server.peek(&mut buf));
                assert!(
                    poll_once(&mut peek_fut).await.is_none(),
                    "peek after WouldBlock try_peek must park"
                );
            }

            // Deliver bytes, consume them, then peer shuts down write half.
            client.write_all(b"x").await.unwrap();
            assert_eq!(server.peek(&mut buf).await.unwrap(), 1);
            assert_eq!(buf[0], b'x');
            let mut one = [0u8; 1];
            server.read_exact(&mut one).await.unwrap();
            assert_eq!(one[0], b'x');

            client.shutdown().await.unwrap();

            // EOF via peek returns Ok(0) and stays Ok(0) on repeated calls.
            assert_eq!(server.peek(&mut buf).await.unwrap(), 0);
            assert_eq!(server.peek(&mut buf).await.unwrap(), 0);
            assert_eq!(server.try_peek(&mut buf).unwrap(), 0);
        });
    });
}

#[test]
fn p2_44_tcp_peek_on_borrowed_and_owned_read_halves_concurrent_with_writes() {
    watchdog_short(|| {
        run(async {
            let listener = AsyncTcpListener::bind(loopback_v4()).await.unwrap();

            // 1. Borrowed AsyncTcpReadHalf: peek, poll_peek, try_peek.
            let (mut c1, mut s1) = connected_pair(&listener).await.unwrap();
            let (mut sr1, mut sw1) = s1.split();
            let (mut cr1, mut cw1) = c1.split();

            let mut buf = [0u8; 16];
            assert_eq!(
                sr1.try_peek(&mut buf).unwrap_err().kind(),
                ErrorKind::WouldBlock
            );
            cw1.write_all(b"half-peek").await.unwrap();
            sw1.write_all(b"echo-back").await.unwrap();

            let n = sr1.peek(&mut buf).await.unwrap();
            assert!(n > 0);
            assert_eq!(&buf[..n], &b"half-peek"[..n]);

            // poll_peek directly via poll_fn.
            let mut pbuf = [0u8; 16];
            let pn = std::future::poll_fn(|cx| sr1.poll_peek(cx, &mut pbuf))
                .await
                .unwrap();
            assert!(pn >= n);
            assert_eq!(&pbuf[..n], &buf[..n]);

            let mut exact = [0u8; 9];
            sr1.read_exact(&mut exact).await.unwrap();
            assert_eq!(&exact, b"half-peek");
            cr1.read_exact(&mut exact).await.unwrap();
            assert_eq!(&exact, b"echo-back");

            // 2. Owned AsyncTcpOwnedReadHalf across spawned tasks with concurrent writes.
            let (c2, s2) = connected_pair(&listener).await.unwrap();
            let (mut sr2, mut sw2) = s2.into_split();
            let (mut cr2, mut cw2) = c2.into_split();

            let writer = task::spawn(async move {
                sw2.write_all(b"from-server").await.unwrap();
                cw2.write_all(b"from-client").await.unwrap();
            });
            let reader_s = task::spawn(async move {
                let mut pb = [0u8; 16];
                let pn = std::future::poll_fn(|cx| sr2.poll_peek(cx, &mut pb))
                    .await
                    .unwrap();
                assert!(pn > 0);
                assert_eq!(&pb[..pn], &b"from-client"[..pn]);
                let n2 = sr2.peek(&mut pb).await.unwrap();
                assert!(n2 >= pn);
                let tn = sr2.try_peek(&mut pb).unwrap();
                assert!(tn >= pn);
                let mut out = [0u8; 11];
                sr2.read_exact(&mut out).await.unwrap();
                out
            });
            let reader_c = task::spawn(async move {
                let mut pb = [0u8; 16];
                let n = cr2.peek(&mut pb).await.unwrap();
                assert!(n > 0);
                assert_eq!(&pb[..n], &b"from-server"[..n]);
                let mut out = [0u8; 11];
                cr2.read_exact(&mut out).await.unwrap();
                out
            });

            writer.await.unwrap();
            assert_eq!(&reader_s.await.unwrap(), b"from-client");
            assert_eq!(&reader_c.await.unwrap(), b"from-server");
        });
    });
}
