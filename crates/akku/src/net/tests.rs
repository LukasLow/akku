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
use akku::net::{
    AsyncTcpListener, AsyncTcpOwnedReadHalf, AsyncTcpOwnedWriteHalf, AsyncTcpReadHalf,
    AsyncTcpStream, AsyncTcpWriteHalf, IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr,
    TcpListener, TcpStream,
};
use akku::task::{self, Runtime};

// --- Harness ----------------------------------------------------------------

const WATCHDOG: Duration = Duration::from_secs(60);

/// Shared by every test; taken exclusively only by the fd-exhaustion test.
static FD_GATE: RwLock<()> = RwLock::new(());

fn run_watchdog<T: Send + 'static>(
    exclusive: bool,
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
    match rx.recv_timeout(WATCHDOG) {
        Ok(Ok(value)) => {
            worker.join().expect("test body thread exits cleanly");
            value
        }
        Ok(Err(payload)) => panic::resume_unwind(payload),
        Err(_) => {
            panic!("watchdog: test body did not finish within {WATCHDOG:?} (hang or lost wakeup)")
        }
    }
}

/// Runs `body` on a fresh thread and fails if it does not finish in time.
fn watchdog<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    run_watchdog(false, body)
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
    fn reader<T: AsyncRead + Unpin + Send>() {}
    fn writer<T: AsyncWrite + Unpin + Send>() {}
    send_sync_debug::<AsyncTcpListener>();
    send_sync_debug::<AsyncTcpStream>();
    send_sync_debug::<AsyncTcpReadHalf<'static>>();
    send_sync_debug::<AsyncTcpWriteHalf<'static>>();
    send_sync_debug::<AsyncTcpOwnedReadHalf>();
    send_sync_debug::<AsyncTcpOwnedWriteHalf>();
    reader::<AsyncTcpStream>();
    writer::<AsyncTcpStream>();
    reader::<AsyncTcpReadHalf<'static>>();
    writer::<AsyncTcpWriteHalf<'static>>();
    reader::<AsyncTcpOwnedReadHalf>();
    writer::<AsyncTcpOwnedWriteHalf>();
    #[cfg(unix)]
    {
        fn fd<T: std::os::fd::AsFd + std::os::fd::AsRawFd>() {}
        fd::<AsyncTcpListener>();
        fd::<AsyncTcpStream>();
    }
    #[cfg(windows)]
    {
        fn sock<T: std::os::windows::io::AsSocket + std::os::windows::io::AsRawSocket>() {}
        sock::<AsyncTcpListener>();
        sock::<AsyncTcpStream>();
    }
    // The bind/connect futures must be spawnable.
    fn send_future<F: Future + Send>(_: F) {}
    send_future(AsyncTcpListener::bind(LOCALHOST));
    send_future(AsyncTcpStream::connect(loopback_v4()));
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
