//! Public-API tests for `akku::io`. Seams are listed in `_dev/DESIGN.md`.
//!
//! `akku::io` does not depend on `akku::task`, so most tests drive futures with
//! the tiny std-only [`block_on`] below; one test checks the futures are
//! spawnable on `akku::task`. TCP end-to-end coverage lives in `net/tests.rs`.
//! Every test body runs under [`watchdog`] so a lost wakeup fails instead of
//! hanging CI.

use std::collections::VecDeque;
use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::pin::{pin, Pin};
use std::sync::{mpsc, Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use akku::io::{
    self, AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
    Cursor, ErrorKind,
};

// --- Harness ----------------------------------------------------------------

const WATCHDOG: Duration = Duration::from_secs(30);

fn watchdog<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("io-test-body".into())
        .spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(body));
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

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Minimal std-only executor: polls on the current thread and parks between
/// wakes. A future that returns `Pending` without arranging a wake hangs here,
/// which the watchdog turns into a failure.
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
        thread::park();
    }
}

/// Polls `fut` once with a waker that does nothing.
fn poll_once<F: Future + Unpin>(fut: &mut F) -> Poll<F::Output> {
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    Pin::new(fut).poll(&mut cx)
}

/// Deterministic xorshift64 bytes, so large-input tests need no RNG dependency.
fn pseudo_random_bytes(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 56) as u8
        })
        .collect()
}

// --- Scripted test doubles ------------------------------------------------------

#[derive(Debug, Clone)]
enum RStep {
    /// Deliver up to `buf.len()` of these bytes; the remainder stays queued.
    Data(Vec<u8>),
    /// Wake the task, then return `Pending`.
    Pending,
    /// Return this error kind once.
    Fail(ErrorKind),
    /// A buggy implementation reporting one byte more than the buffer holds.
    Overreport,
}

/// An `AsyncRead` that follows a script; an empty script means EOF.
#[derive(Debug, Default)]
struct ScriptedReader {
    steps: VecDeque<RStep>,
    polls: usize,
}

impl ScriptedReader {
    fn new(steps: impl IntoIterator<Item = RStep>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            polls: 0,
        }
    }

    /// One byte per poll, with a `Pending` (plus wake) before every byte.
    fn trickle(data: &[u8]) -> Self {
        Self::new(
            data.iter()
                .flat_map(|&b| [RStep::Pending, RStep::Data(vec![b])]),
        )
    }
}

impl AsyncRead for ScriptedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.polls += 1;
        match self.steps.pop_front() {
            None => Poll::Ready(Ok(0)),
            Some(RStep::Pending) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Some(RStep::Fail(kind)) => Poll::Ready(Err(io::Error::new(kind, "scripted"))),
            Some(RStep::Overreport) => Poll::Ready(Ok(buf.len() + 1)),
            Some(RStep::Data(mut data)) => {
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                if n < data.len() {
                    self.steps.push_front(RStep::Data(data.split_off(n)));
                }
                Poll::Ready(Ok(n))
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum WStep {
    /// Accept up to this many bytes.
    Accept(usize),
    /// Wake the task, then return `Pending`.
    Pending,
    /// Return this error kind once.
    Fail(ErrorKind),
    /// Return `Ok(0)` for a non-empty buffer.
    Zero,
    /// A buggy implementation reporting one byte more than offered.
    Overreport,
}

/// An `AsyncWrite` that follows a script, then repeats `fallback` forever.
#[derive(Debug)]
struct ScriptedWriter {
    steps: VecDeque<WStep>,
    fallback: WStep,
    accepted: Vec<u8>,
    polls: usize,
    flushes: usize,
    shutdowns: usize,
}

impl ScriptedWriter {
    fn new(steps: impl IntoIterator<Item = WStep>, fallback: WStep) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            fallback,
            accepted: Vec::new(),
            polls: 0,
            flushes: 0,
            shutdowns: 0,
        }
    }

    fn unlimited() -> Self {
        Self::new([], WStep::Accept(usize::MAX))
    }

    /// Accepts 1..=7 bytes per call (rotating) with a `Pending` in between.
    fn short_and_pending() -> Self {
        let steps = (0..100_000).flat_map(|i| [WStep::Pending, WStep::Accept(i % 7 + 1)]);
        Self::new(steps, WStep::Accept(3))
    }
}

impl AsyncWrite for ScriptedWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.polls += 1;
        let step = self.steps.pop_front().unwrap_or(self.fallback);
        match step {
            WStep::Accept(max) => {
                let n = max.min(buf.len());
                self.accepted.extend_from_slice(&buf[..n]);
                Poll::Ready(Ok(n))
            }
            WStep::Pending => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            WStep::Fail(kind) => Poll::Ready(Err(io::Error::new(kind, "scripted"))),
            WStep::Zero => Poll::Ready(Ok(0)),
            WStep::Overreport => Poll::Ready(Ok(buf.len() + 1)),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.flushes += 1;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.shutdowns += 1;
        Poll::Ready(Ok(()))
    }
}

/// Shared state for the flush-deadlock test: the writer only makes bytes
/// visible on flush, and the reader refuses to continue until it sees them.
#[derive(Default)]
struct Pipe {
    buffered: Vec<u8>,
    flushed: Vec<u8>,
    reader_waker: Option<Waker>,
}

struct EchoGatedReader {
    pipe: Arc<Mutex<Pipe>>,
    chunks: VecDeque<Vec<u8>>,
    delivered: usize,
}

impl AsyncRead for EchoGatedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let flushed = self.pipe.lock().unwrap().flushed.len();
        if flushed < self.delivered {
            // The previous chunk has not reached the far side yet.
            self.pipe.lock().unwrap().reader_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        match self.chunks.pop_front() {
            None => Poll::Ready(Ok(0)),
            Some(chunk) => {
                assert!(chunk.len() <= buf.len());
                buf[..chunk.len()].copy_from_slice(&chunk);
                self.delivered += chunk.len();
                Poll::Ready(Ok(chunk.len()))
            }
        }
    }
}

struct FlushToPipeWriter {
    pipe: Arc<Mutex<Pipe>>,
}

impl AsyncWrite for FlushToPipeWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.pipe.lock().unwrap().buffered.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let waker = {
            let mut pipe = self.pipe.lock().unwrap();
            let pending = std::mem::take(&mut pipe.buffered);
            pipe.flushed.extend_from_slice(&pending);
            pipe.reader_waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

/// Reads everything through the `&mut T` delegation impl.
async fn drain<R: AsyncRead + Unpin>(mut reader: R) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 5];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// Compile-time bound checks from §Public API and §Seams (never executed).
#[allow(dead_code)]
fn futures_are_send_and_traits_are_dyn_compatible() {
    fn send<T: Send>() {}
    send::<io::ReadFuture<'static, &'static [u8]>>();
    send::<io::ReadExactFuture<'static, Cursor<Vec<u8>>>>();
    send::<io::CopyToFuture<'static, &'static [u8], Vec<u8>>>();
    send::<io::WriteFuture<'static, Vec<u8>>>();
    send::<io::WriteAllFuture<'static, Vec<u8>>>();
    send::<io::FlushFuture<'static, io::Sink>>();
    send::<io::ShutdownFuture<'static, io::Sink>>();
    send::<io::FillBufFuture<'static, &'static [u8]>>();
    fn dyn_read(_: &mut dyn AsyncRead) {}
    fn dyn_write(_: Pin<&mut dyn AsyncWrite>) {}
    fn dyn_buf(_: Box<dyn AsyncBufRead + Unpin + Send>) {}
    // `akku::io::copy` must stay the synchronous `std::io::copy`.
    let _: fn(&mut &'static [u8], &mut Vec<u8>) -> io::Result<u64> =
        io::copy::<&'static [u8], Vec<u8>>;
}

// --- std::io re-exports -------------------------------------------------------------

#[test]
fn std_reexports_are_the_std_items() {
    watchdog(|| {
        let e: std::io::Error = io::Error::other("x");
        let _: io::Error = e;
        let _: io::Result<()> = std::io::Result::Ok(());
        let _: std::io::SeekFrom = io::SeekFrom::Start(0);
        let mut src: &[u8] = b"sync copy";
        let mut dst = Vec::new();
        // Synchronous `std::io::copy`, outside any async runtime.
        assert_eq!(io::copy(&mut src, &mut dst).unwrap(), 9);
        assert_eq!(dst, b"sync copy");
        assert_eq!(io::read_to_string(io::empty()).unwrap(), "");
    });
}

// --- AsyncReadExt::read ------------------------------------------------------------------

#[test]
fn read_from_slice_returns_prefix_and_advances() {
    watchdog(|| {
        let mut src: &[u8] = b"hello";
        let mut buf = [0u8; 3];
        assert_eq!(block_on(src.read(&mut buf)).unwrap(), 3);
        assert_eq!(&buf, b"hel");
        assert_eq!(src, b"lo");
        let mut big = [0u8; 8];
        assert_eq!(block_on(src.read(&mut big)).unwrap(), 2);
        assert_eq!(&big[..2], b"lo");
        assert_eq!(block_on(src.read(&mut big)).unwrap(), 0, "EOF");
    });
}

#[test]
fn read_into_empty_buffer_returns_zero_without_polling() {
    watchdog(|| {
        let mut r = ScriptedReader::new([RStep::Fail(ErrorKind::Other)]);
        assert_eq!(block_on(r.read(&mut [])).unwrap(), 0);
        assert_eq!(r.polls, 0, "zero-length read must not touch the stream");
    });
}

#[test]
fn read_propagates_errors_and_waits_through_pending() {
    watchdog(|| {
        let mut r = ScriptedReader::new([
            RStep::Pending,
            RStep::Data(b"ab".to_vec()),
            RStep::Fail(ErrorKind::ConnectionReset),
        ]);
        let mut buf = [0u8; 4];
        assert_eq!(block_on(r.read(&mut buf)).unwrap(), 2);
        assert_eq!(&buf[..2], b"ab");
        let err = block_on(r.read(&mut buf)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ConnectionReset);
    });
}

#[test]
fn cursor_and_empty_implement_async_read() {
    watchdog(|| {
        let mut c = Cursor::new(b"cursor!".to_vec());
        let mut buf = [0u8; 6];
        assert_eq!(block_on(c.read(&mut buf)).unwrap(), 6);
        assert_eq!(&buf, b"cursor");
        assert_eq!(c.position(), 6);
        let mut e = io::empty();
        assert_eq!(block_on(e.read(&mut buf)).unwrap(), 0);
    });
}

// --- AsyncReadExt::read_exact ---------------------------------------------------------------

#[test]
fn read_exact_reassembles_a_one_byte_trickle() {
    watchdog(|| {
        let payload = pseudo_random_bytes(4_096);
        let mut r = ScriptedReader::trickle(&payload);
        let mut buf = vec![0u8; 4_096];
        block_on(r.read_exact(&mut buf)).unwrap();
        assert_eq!(buf, payload);
    });
}

#[test]
fn read_exact_retries_interrupted() {
    watchdog(|| {
        let mut r = ScriptedReader::new([
            RStep::Fail(ErrorKind::Interrupted),
            RStep::Data(b"ab".to_vec()),
            RStep::Fail(ErrorKind::Interrupted),
            RStep::Fail(ErrorKind::Interrupted),
            RStep::Data(b"cd".to_vec()),
        ]);
        let mut buf = [0u8; 4];
        block_on(r.read_exact(&mut buf)).unwrap();
        assert_eq!(&buf, b"abcd");
    });
}

#[test]
fn read_exact_short_stream_is_unexpected_eof() {
    watchdog(|| {
        let mut src: &[u8] = b"abc";
        let mut buf = [0u8; 10];
        let err = block_on(src.read_exact(&mut buf)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
    });
}

#[test]
fn read_exact_into_empty_buffer_is_ok_without_polling() {
    watchdog(|| {
        let mut r = ScriptedReader::new([RStep::Fail(ErrorKind::Other)]);
        block_on(r.read_exact(&mut [])).unwrap();
        assert_eq!(r.polls, 0);
        // Even an already-exhausted reader.
        let mut eof: &[u8] = b"";
        block_on(eof.read_exact(&mut [])).unwrap();
    });
}

#[test]
fn read_exact_rejects_overreporting_reader() {
    watchdog(|| {
        let mut r = ScriptedReader::new([RStep::Overreport]);
        let mut buf = [0u8; 4];
        let err = block_on(r.read_exact(&mut buf)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    });
}

#[test]
fn read_exact_propagates_non_interrupted_errors() {
    watchdog(|| {
        let mut r = ScriptedReader::new([
            RStep::Data(b"x".to_vec()),
            RStep::Fail(ErrorKind::BrokenPipe),
        ]);
        let mut buf = [0u8; 4];
        let err = block_on(r.read_exact(&mut buf)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
    });
}

// --- AsyncWriteExt ---------------------------------------------------------------------------

#[test]
fn write_to_vec_appends_and_reports_length() {
    watchdog(|| {
        let mut v = b"pre-".to_vec();
        assert_eq!(block_on(v.write(b"abc")).unwrap(), 3);
        assert_eq!(v, b"pre-abc");
    });
}

#[test]
fn write_empty_returns_zero_without_polling() {
    watchdog(|| {
        let mut w = ScriptedWriter::new([], WStep::Fail(ErrorKind::Other));
        assert_eq!(block_on(w.write(&[])).unwrap(), 0);
        assert_eq!(w.polls, 0);
    });
}

#[test]
fn write_returns_short_count_without_retrying() {
    watchdog(|| {
        let mut w = ScriptedWriter::new([WStep::Accept(2)], WStep::Accept(usize::MAX));
        assert_eq!(block_on(w.write(b"abcdef")).unwrap(), 2);
        assert_eq!(w.accepted, b"ab");
    });
}

#[test]
fn write_all_survives_short_writes_and_pending() {
    watchdog(|| {
        let payload = pseudo_random_bytes(10_000);
        let mut w = ScriptedWriter::short_and_pending();
        block_on(w.write_all(&payload)).unwrap();
        assert_eq!(w.accepted, payload, "every byte once, in order");
    });
}

#[test]
fn write_all_retries_interrupted() {
    watchdog(|| {
        let mut w = ScriptedWriter::new(
            [
                WStep::Fail(ErrorKind::Interrupted),
                WStep::Accept(2),
                WStep::Fail(ErrorKind::Interrupted),
            ],
            WStep::Accept(usize::MAX),
        );
        block_on(w.write_all(b"hello")).unwrap();
        assert_eq!(w.accepted, b"hello");
    });
}

#[test]
fn write_all_on_zero_progress_is_write_zero() {
    watchdog(|| {
        let mut w = ScriptedWriter::new([WStep::Accept(1)], WStep::Zero);
        let err = block_on(w.write_all(b"abc")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::WriteZero);
        assert_eq!(w.accepted, b"a");
        assert_eq!(w.polls, 2, "fails on the first Ok(0), no spinning");
    });
}

#[test]
fn write_all_empty_is_ok_even_on_a_write_zero_writer() {
    watchdog(|| {
        let mut w = ScriptedWriter::new([], WStep::Zero);
        block_on(w.write_all(&[])).unwrap();
        assert_eq!(w.polls, 0);
    });
}

#[test]
fn write_all_rejects_overreporting_writer() {
    watchdog(|| {
        let mut w = ScriptedWriter::new([], WStep::Overreport);
        let err = block_on(w.write_all(b"abc")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    });
}

#[test]
fn write_all_propagates_non_interrupted_errors() {
    watchdog(|| {
        let mut w = ScriptedWriter::new([WStep::Fail(ErrorKind::ConnectionAborted)], WStep::Zero);
        let err = block_on(w.write_all(b"abc")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ConnectionAborted);
    });
}

#[test]
fn flush_and_shutdown_reach_the_writer_once() {
    watchdog(|| {
        let mut w = ScriptedWriter::unlimited();
        block_on(w.flush()).unwrap();
        assert_eq!((w.flushes, w.shutdowns), (1, 0));
        block_on(w.shutdown()).unwrap();
        assert_eq!(w.shutdowns, 1);
        let mut v = Vec::new();
        block_on(v.flush()).unwrap();
        block_on(v.shutdown()).unwrap();
    });
}

#[test]
fn sink_accepts_and_discards_everything() {
    watchdog(|| {
        let mut s = io::sink();
        assert_eq!(block_on(s.write(b"12345")).unwrap(), 5);
        block_on(s.write_all(&pseudo_random_bytes(100_000))).unwrap();
        block_on(s.flush()).unwrap();
        block_on(s.shutdown()).unwrap();
    });
}

// --- AsyncReadExt::copy_to -----------------------------------------------------------------------

#[test]
fn copy_to_moves_all_bytes_and_counts_them() {
    watchdog(|| {
        let payload = pseudo_random_bytes(40_000);
        let mut src: &[u8] = &payload;
        let mut dst = Vec::new();
        assert_eq!(block_on(src.copy_to(&mut dst)).unwrap(), 40_000);
        assert_eq!(dst, payload);
        assert!(src.is_empty());
    });
}

#[test]
fn copy_to_with_trickle_reader_and_short_writer_crosses_buffer_boundaries() {
    watchdog(|| {
        // 33_000 bytes span more than four 8 KiB buffer fills.
        let payload = pseudo_random_bytes(33_000);
        let chunks = payload
            .chunks(13)
            .flat_map(|c| [RStep::Pending, RStep::Data(c.to_vec())]);
        let mut r = ScriptedReader::new(chunks);
        let mut w = ScriptedWriter::short_and_pending();
        assert_eq!(block_on(r.copy_to(&mut w)).unwrap(), 33_000);
        assert_eq!(w.accepted, payload);
    });
}

#[test]
fn copy_to_from_empty_reader_is_zero_and_flushes() {
    watchdog(|| {
        let mut w = ScriptedWriter::unlimited();
        assert_eq!(block_on(io::empty().copy_to(&mut w)).unwrap(), 0);
        assert!(w.accepted.is_empty());
        assert!(w.flushes >= 1, "copy_to flushes the writer at EOF");
    });
}

#[test]
fn copy_to_never_shuts_the_writer_down() {
    watchdog(|| {
        let mut src: &[u8] = b"no shutdown";
        let mut w = ScriptedWriter::unlimited();
        assert_eq!(block_on(src.copy_to(&mut w)).unwrap(), 11);
        assert_eq!(w.shutdowns, 0);
        assert!(w.flushes >= 1);
    });
}

#[test]
fn copy_to_on_zero_progress_writer_is_write_zero() {
    watchdog(|| {
        let mut src: &[u8] = b"stuck";
        let mut w = ScriptedWriter::new([], WStep::Zero);
        let err = block_on(src.copy_to(&mut w)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::WriteZero);
        assert_eq!(w.polls, 1);
    });
}

#[test]
fn copy_to_retries_interrupted_on_both_sides() {
    watchdog(|| {
        let mut r = ScriptedReader::new([
            RStep::Fail(ErrorKind::Interrupted),
            RStep::Data(b"abc".to_vec()),
            RStep::Fail(ErrorKind::Interrupted),
            RStep::Data(b"def".to_vec()),
        ]);
        let mut w = ScriptedWriter::new(
            [
                WStep::Fail(ErrorKind::Interrupted),
                WStep::Accept(1),
                WStep::Fail(ErrorKind::Interrupted),
            ],
            WStep::Accept(usize::MAX),
        );
        assert_eq!(block_on(r.copy_to(&mut w)).unwrap(), 6);
        assert_eq!(w.accepted, b"abcdef");
    });
}

#[test]
fn copy_to_rejects_overreporting_reader() {
    watchdog(|| {
        let mut r = ScriptedReader::new([RStep::Overreport]);
        let mut w = ScriptedWriter::unlimited();
        let err = block_on(r.copy_to(&mut w)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    });
}

#[test]
fn copy_to_rejects_overreporting_writer() {
    watchdog(|| {
        let mut src: &[u8] = b"overreport";
        let mut w = ScriptedWriter::new([], WStep::Overreport);
        let err = block_on(src.copy_to(&mut w)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    });
}

#[test]
fn copy_to_propagates_reader_and_writer_errors() {
    watchdog(|| {
        let mut r = ScriptedReader::new([
            RStep::Data(b"ok".to_vec()),
            RStep::Fail(ErrorKind::TimedOut),
        ]);
        let mut w = ScriptedWriter::unlimited();
        assert_eq!(
            block_on(r.copy_to(&mut w)).unwrap_err().kind(),
            ErrorKind::TimedOut
        );
        assert_eq!(w.accepted, b"ok");

        let mut src: &[u8] = b"data";
        let mut w = ScriptedWriter::new([WStep::Fail(ErrorKind::PermissionDenied)], WStep::Zero);
        assert_eq!(
            block_on(src.copy_to(&mut w)).unwrap_err().kind(),
            ErrorKind::PermissionDenied
        );
    });
}

#[test]
fn copy_to_flushes_when_reader_goes_pending() {
    // Without a flush on `Pending`, the reader waits for bytes that sit in the
    // writer's buffer forever (request/response deadlock); the watchdog fails.
    watchdog(|| {
        let pipe = Arc::new(Mutex::new(Pipe::default()));
        let mut r = EchoGatedReader {
            pipe: Arc::clone(&pipe),
            chunks: VecDeque::from([b"request".to_vec(), b"-more".to_vec(), b"-end".to_vec()]),
            delivered: 0,
        };
        let mut w = FlushToPipeWriter {
            pipe: Arc::clone(&pipe),
        };
        assert_eq!(block_on(r.copy_to(&mut w)).unwrap(), 16);
        let pipe = pipe.lock().unwrap();
        assert_eq!(pipe.flushed, b"request-more-end");
        assert!(pipe.buffered.is_empty(), "final flush at EOF");
    });
}

// --- AsyncBufRead / fill_buf / consume -----------------------------------------------------------

#[test]
fn fill_buf_on_slice_returns_remaining_bytes_until_consumed() {
    watchdog(|| {
        let mut src: &[u8] = b"buffered";
        assert_eq!(block_on(src.fill_buf()).unwrap(), b"buffered");
        assert_eq!(
            block_on(src.fill_buf()).unwrap(),
            b"buffered",
            "no consume, same bytes"
        );
        Pin::new(&mut src).consume(3);
        assert_eq!(block_on(src.fill_buf()).unwrap(), b"fered");
        Pin::new(&mut src).consume(5);
        assert_eq!(
            block_on(src.fill_buf()).unwrap(),
            b"",
            "EOF is an empty slice"
        );
    });
}

#[test]
fn fill_buf_on_cursor_tracks_position() {
    watchdog(|| {
        let mut c = Cursor::new(b"0123456789".to_vec());
        c.set_position(4);
        assert_eq!(block_on(c.fill_buf()).unwrap(), b"456789");
        Pin::new(&mut c).consume(2);
        assert_eq!(c.position(), 6);
        assert_eq!(block_on(c.fill_buf()).unwrap(), b"6789");
        c.set_position(10);
        assert_eq!(block_on(c.fill_buf()).unwrap(), b"");
    });
}

#[test]
fn fill_buf_on_empty_is_eof() {
    watchdog(|| {
        let mut e = io::empty();
        assert_eq!(block_on(e.fill_buf()).unwrap(), b"");
        Pin::new(&mut e).consume(0);
    });
}

// --- Delegation impls and trait objects ------------------------------------------------------------

#[test]
fn mut_ref_box_and_pin_delegate_reads() {
    watchdog(|| {
        let mut src: &[u8] = b"via &mut";
        assert_eq!(block_on(drain(&mut src)).unwrap(), b"via &mut");
        assert!(src.is_empty());

        let boxed: Box<dyn AsyncRead + Unpin + Send> = Box::new(Cursor::new(b"via box".to_vec()));
        assert_eq!(block_on(drain(boxed)).unwrap(), b"via box");

        let pinned = Box::pin(Cursor::new(b"via pin".to_vec()));
        assert_eq!(block_on(drain(pinned)).unwrap(), b"via pin");

        let mut dyn_ref: &[u8] = b"via dyn";
        let obj: &mut (dyn AsyncRead + Unpin) = &mut dyn_ref;
        assert_eq!(block_on(drain(obj)).unwrap(), b"via dyn");
    });
}

#[test]
fn mut_ref_box_and_pin_delegate_writes() {
    watchdog(|| {
        let mut inner = ScriptedWriter::unlimited();
        {
            // Explicit `Self = &mut ScriptedWriter` so the `&mut T` impl is used.
            let mut by_ref = &mut inner;
            block_on(<&mut ScriptedWriter as AsyncWriteExt>::write_all(
                &mut by_ref,
                b"ref",
            ))
            .unwrap();
            block_on(<&mut ScriptedWriter as AsyncWriteExt>::flush(&mut by_ref)).unwrap();
            block_on(<&mut ScriptedWriter as AsyncWriteExt>::shutdown(
                &mut by_ref,
            ))
            .unwrap();
        }
        assert_eq!(inner.accepted, b"ref");
        assert_eq!((inner.flushes, inner.shutdowns), (1, 1));

        let mut boxed: Box<dyn AsyncWrite + Unpin + Send> = Box::new(ScriptedWriter::unlimited());
        block_on(boxed.write_all(b"box")).unwrap();
        block_on(boxed.shutdown()).unwrap();

        let mut pinned = Box::pin(Vec::<u8>::new());
        block_on(pinned.write_all(b"pin")).unwrap();
        assert_eq!(&**pinned, b"pin");
    });
}

#[test]
fn boxed_buf_read_delegates_fill_buf_and_consume() {
    watchdog(|| {
        let mut boxed: Box<dyn AsyncBufRead + Unpin + Send> =
            Box::new(Cursor::new(b"abcdef".to_vec()));
        assert_eq!(block_on(boxed.fill_buf()).unwrap(), b"abcdef");
        Pin::new(&mut boxed).consume(4);
        assert_eq!(block_on(boxed.fill_buf()).unwrap(), b"ef");
        let mut buf = [0u8; 2];
        block_on(boxed.read_exact(&mut buf)).unwrap();
        assert_eq!(&buf, b"ef");
    });
}

// --- Drop-cancellation -------------------------------------------------------------------------------

#[test]
fn dropping_read_exact_mid_transfer_leaves_stream_usable() {
    watchdog(|| {
        let mut r = ScriptedReader::new([
            RStep::Data(b"abc".to_vec()),
            RStep::Pending,
            RStep::Data(b"defgh".to_vec()),
        ]);
        let mut buf = [0u8; 8];
        {
            let mut fut = r.read_exact(&mut buf);
            assert!(
                poll_once(&mut fut).is_pending(),
                "partial progress, then Pending"
            );
        }
        // The cancelled future consumed "abc"; the stream continues after it.
        let mut rest = [0u8; 5];
        block_on(r.read_exact(&mut rest)).unwrap();
        assert_eq!(&rest, b"defgh");
        assert_eq!(
            &buf[..3],
            b"abc",
            "partially filled buffer is visible to the caller"
        );
    });
}

#[test]
fn dropping_write_all_mid_transfer_leaves_writer_usable() {
    watchdog(|| {
        let mut w = ScriptedWriter::new(
            [WStep::Accept(2), WStep::Pending],
            WStep::Accept(usize::MAX),
        );
        {
            let mut fut = w.write_all(b"abcdef");
            assert!(poll_once(&mut fut).is_pending());
        }
        assert_eq!(w.accepted, b"ab");
        block_on(w.write_all(b"XYZ")).unwrap();
        assert_eq!(w.accepted, b"abXYZ");
    });
}

#[test]
fn dropping_copy_to_mid_transfer_leaves_both_streams_usable() {
    watchdog(|| {
        let mut r = ScriptedReader::new([
            RStep::Data(b"first".to_vec()),
            RStep::Pending,
            RStep::Data(b"second".to_vec()),
        ]);
        let mut w = ScriptedWriter::unlimited();
        {
            let mut fut = r.copy_to(&mut w);
            assert!(poll_once(&mut fut).is_pending());
        }
        assert_eq!(w.accepted, b"first");
        assert_eq!(block_on(r.copy_to(&mut w)).unwrap(), 6);
        assert_eq!(w.accepted, b"firstsecond");
    });
}

// --- E0034: sync and async traits in one scope ---------------------------------------------------------

#[test]
fn explicit_disambiguation_with_sync_and_async_traits_in_scope() {
    use akku::io::{BufRead, Read, Write};
    watchdog(|| {
        // &[u8]: Read + AsyncRead + BufRead + AsyncBufRead.
        let mut src: &[u8] = b"abcdefgh";
        let mut a = [0u8; 2];
        let mut b = [0u8; 2];
        assert_eq!(Read::read(&mut src, &mut a).unwrap(), 2);
        assert_eq!(block_on(AsyncReadExt::read(&mut src, &mut b)).unwrap(), 2);
        assert_eq!((&a, &b), (b"ab", b"cd"));
        assert_eq!(BufRead::fill_buf(&mut src).unwrap(), b"efgh");
        assert_eq!(
            block_on(AsyncBufReadExt::fill_buf(&mut src)).unwrap(),
            b"efgh"
        );

        // Cursor<Vec<u8>>.
        let mut c = Cursor::new(b"0123".to_vec());
        assert_eq!(Read::read(&mut c, &mut a).unwrap(), 2);
        assert_eq!(block_on(AsyncReadExt::read(&mut c, &mut b)).unwrap(), 2);
        assert_eq!((&a, &b), (b"01", b"23"));

        // Vec<u8>: Write + AsyncWrite.
        let mut v = Vec::new();
        assert_eq!(Write::write(&mut v, b"sync,").unwrap(), 5);
        assert_eq!(block_on(AsyncWriteExt::write(&mut v, b"async")).unwrap(), 5);
        Write::flush(&mut v).unwrap();
        block_on(AsyncWriteExt::flush(&mut v)).unwrap();
        assert_eq!(v, b"sync,async");

        // Empty: Read + AsyncRead + BufRead + AsyncBufRead.
        let mut e = io::empty();
        assert_eq!(Read::read(&mut e, &mut a).unwrap(), 0);
        assert_eq!(block_on(AsyncReadExt::read(&mut e, &mut a)).unwrap(), 0);
        assert_eq!(BufRead::fill_buf(&mut e).unwrap(), b"");
        assert_eq!(block_on(AsyncBufReadExt::fill_buf(&mut e)).unwrap(), b"");
    });
}

// --- Send futures on akku::task --------------------------------------------------------------------------

#[test]
fn ext_futures_run_inside_spawned_tasks() {
    watchdog(|| {
        let out = akku::task::block_on(async {
            akku::task::spawn(async {
                let payload = pseudo_random_bytes(20_000);
                let mut src = Cursor::new(payload.clone());
                let mut dst = Vec::new();
                let n = src.copy_to(&mut dst).await?;
                let mut tail = Vec::new();
                tail.write_all(&dst[n as usize - 4..]).await?;
                Ok::<_, io::Error>((n, dst == payload, tail.len()))
            })
            .await
            .unwrap()
        });
        assert_eq!(out.unwrap(), (20_000, true, 4));
    });
}
