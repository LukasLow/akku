//! Public-API tests for `akku::task`. Seams are listed in `_dev/DESIGN.md`.
//!
//! Every test body runs under [`watchdog`] so a lost wakeup or a livelock
//! fails the test instead of hanging CI.

use std::any::Any;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use akku::task::{self, JoinError, JoinHandle, Limits, Runtime, SpawnError, YieldNow};

// --- Harness ----------------------------------------------------------------

const WATCHDOG: Duration = Duration::from_secs(30);

/// Runs `body` on a fresh thread and fails if it does not finish within
/// [`WATCHDOG`]. A panic inside `body` is re-raised with its original payload,
/// so `#[should_panic(expected = ...)]` still matches the real message.
fn watchdog<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("task-test-body".into())
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

fn runtime() -> Runtime {
    Runtime::new().expect("create runtime")
}

fn runtime_with(limits: Limits) -> Runtime {
    Runtime::with_limits(limits).expect("create runtime with limits")
}

/// Sets its flag when dropped, to observe that a future's state was dropped.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// A one-shot latch any thread can open; awaiting it parks the task until then.
#[derive(Clone, Default)]
struct Gate(Arc<Mutex<GateState>>);

#[derive(Default)]
struct GateState {
    open: bool,
    wakers: Vec<Waker>,
}

impl Gate {
    fn open(&self) {
        let wakers = {
            let mut state = self.0.lock().unwrap();
            state.open = true;
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    fn wait(&self) -> GateWait {
        GateWait(self.clone())
    }
}

struct GateWait(Gate);

impl Future for GateWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = (self.0).0.lock().unwrap();
        if state.open {
            Poll::Ready(())
        } else {
            state.wakers.push(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// Polls the inner future exactly once and reports whether it finished.
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

/// Yields until `cond` holds, failing after `max` yields.
async fn yield_until(max: usize, mut cond: impl FnMut() -> bool) {
    for _ in 0..max {
        if cond() {
            return;
        }
        task::yield_now().await;
    }
    assert!(cond(), "condition not reached after {max} yields");
}

/// Counts how often it is woken; used to drive futures by hand.
#[derive(Default)]
struct CountingWaker(AtomicUsize);

impl Wake for CountingWaker {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        String::from("<non-string panic payload>")
    }
}

/// Compile-time trait-bound checks from §Public API (never executed).
#[allow(dead_code)]
fn public_types_have_documented_bounds() {
    fn send_sync<T: Send + Sync>() {}
    fn sync<T: Sync>() {}
    fn unpin<T: Unpin>() {}
    fn error<T: std::error::Error + Send + Sync + 'static>() {}
    fn future<T: Future<Output = O>, O>() {}
    send_sync::<JoinError>();
    error::<JoinError>();
    error::<SpawnError>();
    send_sync::<JoinHandle<u8>>();
    unpin::<JoinHandle<u8>>();
    future::<JoinHandle<u8>, Result<u8, JoinError>>();
    send_sync::<YieldNow>();
    unpin::<YieldNow>();
    future::<YieldNow, ()>();
    // Cross-thread `Runtime::spawn` (§Seams item 5) needs a shareable runtime.
    sync::<Runtime>();
    let _: fn(JoinError) -> io::Error = io::Error::from;
    let _: fn(SpawnError) -> io::Error = io::Error::from;
}

// --- Limits -------------------------------------------------------------------

#[test]
fn limits_defaults_match_documented_values() {
    watchdog(|| {
        let expected = Limits {
            max_tasks: 65_536,
            max_polls_per_tick: 64,
            max_io_events_per_tick: 1_024,
        };
        assert_eq!(Limits::new(), expected);
        assert_eq!(Limits::default(), expected);
    });
}

#[test]
fn limits_builders_set_only_their_own_field() {
    watchdog(|| {
        let limits = Limits::new()
            .with_max_tasks(7)
            .with_max_polls_per_tick(3)
            .with_max_io_events_per_tick(9);
        assert_eq!(
            limits,
            Limits {
                max_tasks: 7,
                max_polls_per_tick: 3,
                max_io_events_per_tick: 9,
            }
        );
        assert_eq!(
            Limits::new().with_max_polls_per_tick(5),
            Limits {
                max_tasks: 65_536,
                max_polls_per_tick: 5,
                max_io_events_per_tick: 1_024,
            }
        );
    });
}

#[test]
fn runtime_reports_its_limits() {
    watchdog(|| {
        assert_eq!(runtime().limits(), Limits::default());
        let custom = Limits::new().with_max_tasks(3).with_max_polls_per_tick(2);
        assert_eq!(runtime_with(custom).limits(), custom);
    });
}

// --- block_on / try_block_on ----------------------------------------------------

#[test]
fn block_on_returns_ready_value() {
    watchdog(|| {
        assert_eq!(task::block_on(async { 40u32 + 2 }), 42);
        assert_eq!(runtime().block_on(async { "ready" }), "ready");
    });
}

#[test]
fn try_block_on_returns_ok_value() {
    watchdog(|| {
        let out = task::try_block_on(async { vec![1u8, 2, 3] }).expect("runtime init");
        assert_eq!(out, [1, 2, 3]);
    });
}

/// Returns `Pending` `remaining` times, waking itself each time.
struct SelfWaking {
    remaining: usize,
    polls: Arc<AtomicUsize>,
}

impl Future for SelfWaking {
    type Output = usize;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
        let polls = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.remaining == 0 {
            Poll::Ready(polls)
        } else {
            self.remaining -= 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[test]
fn block_on_repolls_root_after_same_thread_wake() {
    watchdog(|| {
        let polls = Arc::new(AtomicUsize::new(0));
        let fut = SelfWaking {
            remaining: 5,
            polls: Arc::clone(&polls),
        };
        assert_eq!(task::block_on(fut), 6);
        assert_eq!(polls.load(Ordering::SeqCst), 6);
    });
}

#[test]
fn block_on_wakes_from_another_thread_without_lost_wakeups() {
    watchdog(|| {
        let rt = runtime();
        for round in 0..500u32 {
            let gate = Gate::default();
            let opener = gate.clone();
            let waiter = gate.wait();
            let out = rt.block_on(async move {
                let t = thread::spawn(move || opener.open());
                waiter.await;
                t.join().unwrap();
                round
            });
            assert_eq!(out, round);
        }
    });
}

#[test]
fn block_on_root_is_woken_by_spawned_task() {
    watchdog(|| {
        let out = runtime().block_on(async {
            let gate = Gate::default();
            let opener = gate.clone();
            let handle = task::spawn(async move {
                task::yield_now().await;
                opener.open();
                "opened"
            });
            gate.wait().await;
            handle.await.unwrap()
        });
        assert_eq!(out, "opened");
    });
}

#[test]
#[should_panic(expected = "block_on")]
fn nested_block_on_panics() {
    watchdog(|| {
        task::block_on(async {
            task::block_on(async { 1u8 });
        });
    });
}

#[test]
#[should_panic(expected = "block_on")]
fn nested_runtime_block_on_panics() {
    watchdog(|| {
        let outer = runtime();
        let inner = runtime();
        outer.block_on(async move {
            inner.block_on(async { 1u8 });
        });
    });
}

#[test]
fn block_on_is_usable_again_after_nested_panic() {
    watchdog(|| {
        let nested = panic::catch_unwind(|| {
            task::block_on(async {
                task::block_on(async { 1u8 });
            })
        });
        assert!(nested.is_err(), "nested block_on must panic");
        // The thread-local guard must have been restored by unwinding.
        assert_eq!(task::block_on(async { 5u8 }), 5);
        assert_eq!(runtime().block_on(async { 6u8 }), 6);
    });
}

#[test]
fn sequential_block_on_calls_reuse_one_runtime() {
    watchdog(|| {
        let rt = runtime();
        let first = rt.block_on(async { task::spawn(async { 1u32 }).await.unwrap() });
        let second = rt.block_on(async { task::spawn(async { 2u32 }).await.unwrap() });
        assert_eq!((first, second), (1, 2));
    });
}

// --- spawn / try_spawn / JoinHandle ----------------------------------------------

#[test]
fn spawn_returns_value_through_join_handle() {
    watchdog(|| {
        let out = runtime().block_on(async { task::spawn(async { 7u64 * 6 }).await });
        assert_eq!(out.unwrap(), 42);
    });
}

#[test]
fn fan_out_tasks_all_complete_with_their_own_values() {
    watchdog(|| {
        let sum = runtime().block_on(async {
            let handles: Vec<_> = (0..100u64)
                .map(|i| task::spawn(async move { i * i }))
                .collect();
            let mut sum = 0u64;
            for h in handles {
                sum += h.await.unwrap();
            }
            sum
        });
        // Sum of squares 0..100 = 99 * 100 * 199 / 6.
        assert_eq!(sum, 328_350);
    });
}

#[test]
fn nested_spawn_from_inside_a_task() {
    watchdog(|| {
        let out = runtime().block_on(async {
            task::spawn(async {
                let inner = task::spawn(async { String::from("inner") });
                format!("outer+{}", inner.await.unwrap())
            })
            .await
            .unwrap()
        });
        assert_eq!(out, "outer+inner");
    });
}

#[test]
fn spawned_tasks_first_run_in_fifo_order() {
    watchdog(|| {
        let order = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&order);
        runtime().block_on(async move {
            let handles: Vec<_> = (0..10u32)
                .map(|i| {
                    let seen = Arc::clone(&seen);
                    task::spawn(async move { seen.lock().unwrap().push(i) })
                })
                .collect();
            for h in handles {
                h.await.unwrap();
            }
        });
        assert_eq!(*order.lock().unwrap(), (0..10).collect::<Vec<_>>());
    });
}

#[test]
fn runtime_spawn_outside_block_on_runs_on_next_block_on() {
    watchdog(|| {
        let rt = runtime();
        let ran = flag();
        let ran2 = Arc::clone(&ran);
        let handle = rt.spawn(async move {
            ran2.store(true, Ordering::SeqCst);
            11u8
        });
        assert!(
            !ran.load(Ordering::SeqCst),
            "task must not run before block_on"
        );
        assert_eq!(rt.block_on(handle).unwrap(), 11);
        assert!(ran.load(Ordering::SeqCst));

        let second = rt
            .try_spawn(async { 12u8 })
            .expect("try_spawn on idle runtime");
        assert_eq!(rt.block_on(second).unwrap(), 12);
    });
}

#[test]
fn try_spawn_inside_runtime_succeeds() {
    watchdog(|| {
        let out = runtime().block_on(async {
            let h = task::try_spawn(async { 3i32 }).expect("inside runtime");
            h.await.unwrap()
        });
        assert_eq!(out, 3);
    });
}

#[test]
fn try_spawn_outside_runtime_is_no_runtime() {
    watchdog(|| {
        let result = task::try_spawn(async { 1u8 });
        assert!(
            matches!(result, Err(SpawnError::NoRuntime)),
            "got {result:?}"
        );
    });
}

#[test]
#[should_panic(expected = "runtime")]
fn spawn_outside_runtime_panics() {
    watchdog(|| {
        drop(task::spawn(async { 1u8 }));
    });
}

#[test]
fn cross_thread_runtime_spawn_wakes_parked_runtime() {
    watchdog(|| {
        let rt = runtime();
        for round in 0..100u32 {
            let gate = Gate::default();
            let opener = gate.clone();
            let waiter = gate.wait();
            thread::scope(|s| {
                let rt_ref = &rt;
                s.spawn(move || {
                    // Detached: the task itself opens the gate the root waits on.
                    drop(rt_ref.spawn(async move { opener.open() }));
                });
                let out = rt.block_on(async move {
                    waiter.await;
                    round
                });
                assert_eq!(out, round);
            });
        }
    });
}

// --- Detach vs abort --------------------------------------------------------------

#[test]
fn dropped_join_handle_detaches_and_task_still_runs() {
    watchdog(|| {
        let done = flag();
        let observed = Arc::clone(&done);
        runtime().block_on(async move {
            let gate = Gate::default();
            let waiter = gate.wait();
            let done2 = Arc::clone(&done);
            drop(task::spawn(async move {
                waiter.await;
                done2.store(true, Ordering::SeqCst);
            }));
            task::yield_now().await;
            assert!(!done.load(Ordering::SeqCst));
            gate.open();
            yield_until(1_000, || done.load(Ordering::SeqCst)).await;
        });
        assert!(observed.load(Ordering::SeqCst));
    });
}

#[test]
fn join_handle_can_be_polled_again_after_an_abandoned_await() {
    watchdog(|| {
        let out = runtime().block_on(async {
            let gate = Gate::default();
            let waiter = gate.wait();
            let mut handle = task::spawn(async move {
                waiter.await;
                9u8
            });
            assert!(
                poll_once(&mut handle).await.is_none(),
                "task must still be pending"
            );
            gate.open();
            handle.await
        });
        assert_eq!(out.unwrap(), 9);
    });
}

#[test]
fn abort_before_first_poll_cancels_without_running() {
    watchdog(|| {
        let ran = flag();
        let ran2 = Arc::clone(&ran);
        let dropped = flag();
        let guard = DropFlag(Arc::clone(&dropped));
        let err = runtime().block_on(async move {
            let h = task::spawn(async move {
                let _guard = guard;
                ran2.store(true, Ordering::SeqCst);
            });
            h.abort();
            h.await.unwrap_err()
        });
        assert!(err.is_cancelled());
        assert!(!err.is_panic());
        assert!(
            !ran.load(Ordering::SeqCst),
            "aborted task must never be polled"
        );
        assert!(
            dropped.load(Ordering::SeqCst),
            "aborted future must be dropped"
        );
    });
}

#[test]
fn abort_pending_task_drops_its_state_and_reports_cancelled() {
    watchdog(|| {
        let dropped = flag();
        let guard = DropFlag(Arc::clone(&dropped));
        let observed = Arc::clone(&dropped);
        runtime().block_on(async move {
            let h = task::spawn(async move {
                let _guard = guard;
                std::future::pending::<()>().await;
            });
            task::yield_now().await;
            assert!(!h.is_finished());
            assert!(!observed.load(Ordering::SeqCst));
            h.abort();
            yield_until(1_000, || h.is_finished()).await;
            assert!(
                observed.load(Ordering::SeqCst),
                "drop guard runs promptly on abort"
            );
            let err = h.await.unwrap_err();
            assert!(err.is_cancelled());
            assert!(!err.is_panic());
        });
    });
}

#[test]
fn abort_is_idempotent() {
    watchdog(|| {
        let err = runtime().block_on(async {
            let h = task::spawn(std::future::pending::<()>());
            task::yield_now().await;
            h.abort();
            h.abort();
            h.abort();
            h.await.unwrap_err()
        });
        assert!(err.is_cancelled());
    });
}

#[test]
fn abort_after_completion_still_returns_output() {
    watchdog(|| {
        let out = runtime().block_on(async {
            let h = task::spawn(async { 5u16 });
            yield_until(1_000, || h.is_finished()).await;
            h.abort();
            h.abort();
            h.await
        });
        assert_eq!(out.unwrap(), 5);
    });
}

#[test]
fn is_finished_tracks_completion() {
    watchdog(|| {
        runtime().block_on(async {
            let gate = Gate::default();
            let waiter = gate.wait();
            let h = task::spawn(waiter);
            task::yield_now().await;
            assert!(!h.is_finished());
            gate.open();
            yield_until(1_000, || h.is_finished()).await;
            assert!(h.await.is_ok());
        });
    });
}

#[test]
fn runtime_drop_drops_unfinished_tasks() {
    watchdog(|| {
        let polled = flag();
        let never_polled = flag();
        let g1 = DropFlag(Arc::clone(&polled));
        let g2 = DropFlag(Arc::clone(&never_polled));
        let rt = runtime();
        rt.block_on(async move {
            drop(task::spawn(async move {
                let _g = g1;
                std::future::pending::<()>().await;
            }));
            task::yield_now().await;
        });
        drop(rt.spawn(async move {
            let _g = g2;
        }));
        assert!(!polled.load(Ordering::SeqCst));
        assert!(!never_polled.load(Ordering::SeqCst));
        drop(rt);
        assert!(
            polled.load(Ordering::SeqCst),
            "polled pending task dropped by Runtime::drop"
        );
        assert!(
            never_polled.load(Ordering::SeqCst),
            "queued task dropped by Runtime::drop"
        );
    });
}

#[test]
fn join_handle_reports_cancelled_after_runtime_drop() {
    watchdog(|| {
        let rt = runtime();
        let mut handle = rt.spawn(std::future::pending::<()>());
        drop(rt);
        assert!(handle.is_finished());
        let waker = Waker::from(Arc::new(CountingWaker::default()));
        let mut cx = Context::from_waker(&waker);
        match Pin::new(&mut handle).poll(&mut cx) {
            Poll::Ready(Err(e)) => assert!(e.is_cancelled()),
            other => panic!(
                "expected Ready(Err(cancelled)), got {:?}",
                other.map(|r| r.is_ok())
            ),
        }
    });
}

// --- Panic isolation -----------------------------------------------------------------

#[test]
fn panic_with_str_payload_is_caught() {
    watchdog(|| {
        let err =
            runtime().block_on(async { task::spawn(async { panic!("boom") }).await.unwrap_err() });
        assert!(err.is_panic());
        assert!(!err.is_cancelled());
        let payload = err.into_panic();
        assert_eq!(payload.downcast_ref::<&'static str>(), Some(&"boom"));
    });
}

#[test]
fn panic_with_formatted_payload_is_caught() {
    watchdog(|| {
        let err = runtime().block_on(async {
            let code = 7;
            task::spawn(async move { panic!("code {code}") })
                .await
                .unwrap_err()
        });
        let payload = err.try_into_panic().expect("is a panic");
        assert_eq!(
            payload.downcast_ref::<String>().map(String::as_str),
            Some("code 7")
        );
    });
}

#[test]
fn panic_with_custom_payload_is_caught() {
    watchdog(|| {
        let err = runtime().block_on(async {
            task::spawn(async { std::panic::panic_any(42u32) })
                .await
                .unwrap_err()
        });
        assert!(err.is_panic());
        assert_eq!(err.into_panic().downcast_ref::<u32>(), Some(&42));
    });
}

#[test]
fn sibling_tasks_complete_when_one_panics() {
    watchdog(|| {
        let (a, bad, b) = runtime().block_on(async {
            let a = task::spawn(async {
                task::yield_now().await;
                1u8
            });
            let bad = task::spawn(async {
                task::yield_now().await;
                panic!("sibling boom");
            });
            let b = task::spawn(async {
                task::yield_now().await;
                task::yield_now().await;
                2u8
            });
            (a.await, bad.await, b.await)
        });
        assert_eq!(a.unwrap(), 1);
        assert!(bad.unwrap_err().is_panic());
        assert_eq!(b.unwrap(), 2);
    });
}

#[test]
fn detached_panicking_task_leaves_runtime_healthy() {
    watchdog(|| {
        let out = runtime().block_on(async {
            drop(task::spawn(async { panic!("detached boom") }));
            for _ in 0..10 {
                task::yield_now().await;
            }
            task::spawn(async { 3u8 }).await
        });
        assert_eq!(out.unwrap(), 3);
    });
}

struct PanicOnDrop(&'static str);

impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        if !thread::panicking() {
            panic!("{}", self.0);
        }
    }
}

#[test]
fn panic_in_destructor_during_abort_is_reported_as_panic() {
    watchdog(|| {
        let (err, after) = runtime().block_on(async {
            let bomb = PanicOnDrop("drop boom");
            let h = task::spawn(async move {
                let _bomb = bomb;
                std::future::pending::<()>().await;
            });
            task::yield_now().await;
            h.abort();
            let err = h.await.unwrap_err();
            (err, task::spawn(async { 4u8 }).await)
        });
        assert!(err.is_panic(), "destructor panic is reported as a panic");
        assert_eq!(panic_message(&*err.into_panic()), "drop boom");
        assert_eq!(after.unwrap(), 4, "runtime stays healthy");
    });
}

/// A hand-written future (not an async block) whose state survives a poll
/// panic, so the executor itself must drop it, and that drop panics too.
struct PanicsInPollAndDrop {
    _bomb: PanicOnDrop,
}

impl Future for PanicsInPollAndDrop {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        panic!("poll boom");
    }
}

#[test]
fn panic_in_destructor_after_poll_panic_is_contained() {
    watchdog(|| {
        let (err, after) = runtime().block_on(async {
            let h = task::spawn(PanicsInPollAndDrop {
                _bomb: PanicOnDrop("drop boom"),
            });
            let err = h.await.unwrap_err();
            (err, task::spawn(async { 8u8 }).await)
        });
        assert!(err.is_panic());
        let msg = panic_message(&*err.into_panic());
        assert!(
            msg == "poll boom" || msg == "drop boom",
            "unexpected payload {msg:?}"
        );
        assert_eq!(after.unwrap(), 8);
    });
}

#[test]
fn root_future_panic_propagates_out_of_block_on() {
    watchdog(|| {
        let rt = runtime();
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            rt.block_on(async { panic!("root boom") });
        }));
        let payload = result.expect_err("root panic must propagate");
        assert_eq!(panic_message(&*payload), "root boom");
        // The same runtime keeps working afterwards.
        assert_eq!(rt.block_on(async { 1u8 }), 1);
    });
}

// --- JoinError / SpawnError -------------------------------------------------------------

fn cancelled_error() -> JoinError {
    runtime().block_on(async {
        let h = task::spawn(std::future::pending::<()>());
        h.abort();
        h.await.unwrap_err()
    })
}

fn panic_error(msg: &'static str) -> JoinError {
    runtime().block_on(async move {
        task::spawn(async move { panic!("{msg}") })
            .await
            .unwrap_err()
    })
}

#[test]
fn try_into_panic_on_cancelled_returns_the_error() {
    watchdog(|| {
        let err = cancelled_error();
        let back = err.try_into_panic().expect_err("not a panic");
        assert!(back.is_cancelled());
    });
}

#[test]
fn into_panic_on_cancelled_error_panics() {
    watchdog(|| {
        let err = cancelled_error();
        let result = panic::catch_unwind(AssertUnwindSafe(move || err.into_panic()));
        assert!(
            result.is_err(),
            "into_panic on a cancelled JoinError must panic"
        );
    });
}

#[test]
fn join_error_display_and_debug_name_the_cause() {
    watchdog(|| {
        let cancelled = cancelled_error();
        let panicked = panic_error("disp boom");
        let c = cancelled.to_string().to_lowercase();
        let p = panicked.to_string().to_lowercase();
        assert!(c.contains("cancel"), "{c:?}");
        assert!(p.contains("panic"), "{p:?}");
        assert_ne!(c, p);
        assert!(format!("{cancelled:?}").to_lowercase().contains("cancel"));
        assert!(format!("{panicked:?}").to_lowercase().contains("panic"));
    });
}

#[test]
fn join_error_converts_into_io_error_keeping_message() {
    watchdog(|| {
        let err = cancelled_error();
        let msg = err.to_string();
        let io_err = io::Error::from(err);
        assert_eq!(io_err.to_string(), msg);
        let inner = io_err.get_ref().and_then(|e| e.downcast_ref::<JoinError>());
        assert!(inner.is_some_and(JoinError::is_cancelled));
    });
}

#[test]
fn spawn_error_display_is_distinct_per_variant() {
    watchdog(|| {
        let no_rt = SpawnError::NoRuntime.to_string();
        let shut = SpawnError::ShuttingDown.to_string();
        let limit = SpawnError::TaskLimitExceeded { limit: 4321 }.to_string();
        assert!(no_rt.to_lowercase().contains("runtime"), "{no_rt:?}");
        assert!(shut.to_lowercase().contains("shut"), "{shut:?}");
        assert!(limit.contains("4321"), "{limit:?}");
        assert_ne!(no_rt, shut);
        assert_ne!(shut, limit);
    });
}

#[test]
fn spawn_error_converts_into_io_error_with_source() {
    watchdog(|| {
        let err = SpawnError::TaskLimitExceeded { limit: 2 };
        let io_err = io::Error::from(err);
        assert_eq!(io_err.to_string(), err.to_string());
        let inner = io_err
            .get_ref()
            .and_then(|e| e.downcast_ref::<SpawnError>());
        assert_eq!(inner, Some(&SpawnError::TaskLimitExceeded { limit: 2 }));
    });
}

// --- Limits enforcement -------------------------------------------------------------------

fn assert_limit_exceeded<T>(result: Result<JoinHandle<T>, SpawnError>, limit: usize) {
    match result {
        Err(SpawnError::TaskLimitExceeded { limit: got }) => assert_eq!(got, limit),
        Err(other) => panic!("expected TaskLimitExceeded {{ limit: {limit} }}, got {other:?}"),
        Ok(_) => panic!("expected TaskLimitExceeded {{ limit: {limit} }}, got Ok"),
    }
}

#[test]
fn max_tasks_allows_exactly_the_limit() {
    watchdog(|| {
        runtime_with(Limits::new().with_max_tasks(4)).block_on(async {
            let gate = Gate::default();
            let handles: Vec<_> = (0..4)
                .map(|i| {
                    let w = gate.wait();
                    task::try_spawn(async move {
                        w.await;
                        i
                    })
                    .expect("within limit")
                })
                .collect();
            assert_limit_exceeded(task::try_spawn(async {}), 4);
            gate.open();
            let mut got = Vec::new();
            for h in handles {
                got.push(h.await.unwrap());
            }
            assert_eq!(got, [0, 1, 2, 3]);
        });
    });
}

#[test]
fn max_tasks_slot_is_released_after_completion() {
    watchdog(|| {
        runtime_with(Limits::new().with_max_tasks(1)).block_on(async {
            let gate = Gate::default();
            let w = gate.wait();
            let first = task::try_spawn(w).unwrap();
            assert_limit_exceeded(task::try_spawn(async {}), 1);
            gate.open();
            first.await.unwrap();
            let second = task::try_spawn(async { 2u8 }).expect("slot released on completion");
            assert_eq!(second.await.unwrap(), 2);
        });
    });
}

#[test]
fn max_tasks_slot_is_released_after_abort() {
    watchdog(|| {
        runtime_with(Limits::new().with_max_tasks(1)).block_on(async {
            let first = task::try_spawn(std::future::pending::<()>()).unwrap();
            assert_limit_exceeded(task::try_spawn(async {}), 1);
            first.abort();
            assert!(first.await.unwrap_err().is_cancelled());
            let second = task::try_spawn(async { 3u8 }).expect("slot released on abort");
            assert_eq!(second.await.unwrap(), 3);
        });
    });
}

#[test]
fn max_tasks_slot_is_released_after_panic() {
    watchdog(|| {
        runtime_with(Limits::new().with_max_tasks(1)).block_on(async {
            let first = task::try_spawn(async { panic!("slot boom") }).unwrap();
            assert!(first.await.unwrap_err().is_panic());
            let second = task::try_spawn(async { 4u8 }).expect("slot released on panic");
            assert_eq!(second.await.unwrap(), 4);
        });
    });
}

#[test]
fn max_tasks_counts_detached_tasks_until_they_finish() {
    watchdog(|| {
        runtime_with(Limits::new().with_max_tasks(1)).block_on(async {
            let gate = Gate::default();
            let w = gate.wait();
            let done = flag();
            let done2 = Arc::clone(&done);
            drop(task::try_spawn(async move {
                w.await;
                done2.store(true, Ordering::SeqCst);
            }));
            assert_limit_exceeded(task::try_spawn(async {}), 1);
            gate.open();
            yield_until(1_000, || done.load(Ordering::SeqCst)).await;
            // The slot is released once the detached task's future is dropped.
            let mut result = task::try_spawn(async {});
            for _ in 0..100 {
                if result.is_ok() {
                    break;
                }
                task::yield_now().await;
                result = task::try_spawn(async {});
            }
            result
                .expect("detached task's slot is released")
                .await
                .unwrap();
        });
    });
}

#[test]
fn spawn_over_the_limit_panics_but_runtime_survives() {
    watchdog(|| {
        let rt = runtime_with(Limits::new().with_max_tasks(1));
        rt.block_on(async {
            let gate = Gate::default();
            let w = gate.wait();
            let first = task::spawn(w);
            let over = panic::catch_unwind(AssertUnwindSafe(|| task::spawn(async {})));
            assert!(over.is_err(), "spawn beyond max_tasks must panic");
            gate.open();
            first.await.unwrap();
        });
        let rt_over = panic::catch_unwind(AssertUnwindSafe(|| {
            let _hold = rt.spawn(std::future::pending::<()>());
            rt.spawn(async {})
        }));
        assert!(
            rt_over.is_err(),
            "Runtime::spawn beyond max_tasks must panic"
        );
    });
}

#[test]
fn runtime_try_spawn_reports_the_configured_limit() {
    watchdog(|| {
        let rt = runtime_with(Limits::new().with_max_tasks(2));
        let a = rt.try_spawn(async { 1u8 }).unwrap();
        let b = rt.try_spawn(async { 2u8 }).unwrap();
        assert_limit_exceeded(rt.try_spawn(async { 3u8 }), 2);
        assert_eq!(
            rt.block_on(async { (a.await.unwrap(), b.await.unwrap()) }),
            (1, 2)
        );
    });
}

#[test]
fn max_tasks_zero_rejects_spawn_but_block_on_runs() {
    watchdog(|| {
        let rt = runtime_with(Limits::new().with_max_tasks(0));
        assert_limit_exceeded(rt.try_spawn(async {}), 0);
        let out = rt.block_on(async {
            assert_limit_exceeded(task::try_spawn(async {}), 0);
            42u8
        });
        assert_eq!(out, 42);
    });
}

#[test]
fn zero_poll_and_io_limits_are_clamped_and_do_not_hang() {
    watchdog(|| {
        let limits = Limits::new()
            .with_max_polls_per_tick(0)
            .with_max_io_events_per_tick(0);
        let rt = runtime_with(limits);
        assert_eq!(
            rt.limits(),
            limits,
            "limits() reports the configured values"
        );
        let out = rt.block_on(async {
            let handles: Vec<_> = (0..20u32)
                .map(|i| {
                    task::spawn(async move {
                        task::yield_now().await;
                        i
                    })
                })
                .collect();
            let mut sum = 0;
            for h in handles {
                sum += h.await.unwrap();
            }
            sum
        });
        assert_eq!(out, 190);
    });
}

// --- yield_now, fairness, waker storms ---------------------------------------------------------

#[test]
fn yield_now_outside_runtime_wakes_and_then_completes() {
    watchdog(|| {
        let counter = Arc::new(CountingWaker::default());
        let waker = Waker::from(Arc::clone(&counter));
        let mut cx = Context::from_waker(&waker);
        let mut fut = task::yield_now();
        assert!(Pin::new(&mut fut).poll(&mut cx).is_pending());
        assert_eq!(
            counter.0.load(Ordering::SeqCst),
            1,
            "wakes immediately outside a runtime"
        );
        assert!(Pin::new(&mut fut).poll(&mut cx).is_ready());
    });
}

#[test]
fn spinning_yield_task_does_not_starve_root_or_siblings() {
    watchdog(|| {
        let spins = Arc::new(AtomicUsize::new(0));
        let spins2 = Arc::clone(&spins);
        let rt = runtime_with(Limits::new().with_max_polls_per_tick(4));
        let out = rt.block_on(async move {
            drop(task::spawn(async move {
                loop {
                    spins2.fetch_add(1, Ordering::SeqCst);
                    task::yield_now().await;
                }
            }));
            let sibling = task::spawn(async {
                for _ in 0..3 {
                    task::yield_now().await;
                }
                "sibling"
            });
            for _ in 0..10 {
                task::yield_now().await;
            }
            sibling.await.unwrap()
        });
        assert_eq!(out, "sibling");
        assert!(
            spins.load(Ordering::SeqCst) >= 1,
            "spinner made progress too"
        );
    });
}

#[test]
fn spinning_task_does_not_starve_cross_thread_wakeup() {
    watchdog(|| {
        runtime().block_on(async {
            drop(task::spawn(async {
                loop {
                    task::yield_now().await;
                }
            }));
            let gate = Gate::default();
            let opener = gate.clone();
            let t = thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                opener.open();
            });
            gate.wait().await;
            t.join().unwrap();
        });
    });
}

/// Polled task that wakes itself `storm` times during its first poll, then
/// stays pending (without waking) until `release` is set.
struct StormTask {
    storm: usize,
    polls: Arc<AtomicUsize>,
    release: Arc<AtomicBool>,
    waker_slot: Arc<Mutex<Option<Waker>>>,
}

impl Future for StormTask {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
        let n = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.release.load(Ordering::SeqCst) {
            return Poll::Ready(n);
        }
        *self.waker_slot.lock().unwrap() = Some(cx.waker().clone());
        if n == 1 {
            for _ in 0..self.storm {
                cx.waker().wake_by_ref();
            }
        }
        Poll::Pending
    }
}

#[test]
fn same_thread_waker_storm_coalesces_to_one_repoll() {
    watchdog(|| {
        let polls = Arc::new(AtomicUsize::new(0));
        let release = flag();
        let slot = Arc::new(Mutex::new(None::<Waker>));
        let fut = StormTask {
            storm: 10_000,
            polls: Arc::clone(&polls),
            release: Arc::clone(&release),
            waker_slot: Arc::clone(&slot),
        };
        let total = runtime().block_on(async move {
            let h = task::spawn(fut);
            yield_until(1_000, || polls.load(Ordering::SeqCst) >= 2).await;
            // Give duplicate queue entries, if any, a chance to run.
            for _ in 0..50 {
                task::yield_now().await;
            }
            assert_eq!(
                polls.load(Ordering::SeqCst),
                2,
                "10_000 wakes coalesce into one re-poll"
            );
            release.store(true, Ordering::SeqCst);
            let waker = slot.lock().unwrap().take().expect("waker stored");
            waker.wake();
            h.await.unwrap()
        });
        assert_eq!(total, 3);
    });
}

#[test]
fn cross_thread_waker_storm_completes_with_bounded_polls() {
    watchdog(|| {
        let polls = Arc::new(AtomicUsize::new(0));
        let release = flag();
        let slot = Arc::new(Mutex::new(None::<Waker>));
        let fut = StormTask {
            storm: 0,
            polls: Arc::clone(&polls),
            release: Arc::clone(&release),
            waker_slot: Arc::clone(&slot),
        };
        let polls_seen = Arc::clone(&polls);
        let total = runtime().block_on(async move {
            let h = task::spawn(fut);
            yield_until(1_000, || polls.load(Ordering::SeqCst) >= 1).await;
            let waker = slot.lock().unwrap().clone().expect("waker stored");
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let w = waker.clone();
                    thread::spawn(move || {
                        for _ in 0..2_500 {
                            w.wake_by_ref();
                        }
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
            release.store(true, Ordering::SeqCst);
            waker.wake();
            h.await.unwrap()
        });
        assert!(total >= 2);
        assert!(
            polls_seen.load(Ordering::SeqCst) <= 10_002,
            "at most one poll per wake"
        );
    });
}

#[test]
fn self_wake_during_poll_does_not_deadlock() {
    watchdog(|| {
        let polls = Arc::new(AtomicUsize::new(0));
        let fut = SelfWaking {
            remaining: 3,
            polls: Arc::clone(&polls),
        };
        let out = runtime().block_on(async move { task::spawn(fut).await.unwrap() });
        assert_eq!(out, 4);
    });
}

#[test]
fn waking_a_finished_or_aborted_task_is_a_no_op() {
    watchdog(|| {
        let polls = Arc::new(AtomicUsize::new(0));
        let slot = Arc::new(Mutex::new(None::<Waker>));
        let rt = runtime();
        let stale = rt.block_on(async {
            // Finished task: record the waker it saw, then complete.
            let recorder_slot = Arc::clone(&slot);
            let recorder_polls = Arc::clone(&polls);
            let finished = task::spawn(std::future::poll_fn(move |cx| {
                recorder_polls.fetch_add(1, Ordering::SeqCst);
                *recorder_slot.lock().unwrap() = Some(cx.waker().clone());
                Poll::Ready(())
            }));
            finished.await.unwrap();
            let stale = slot.lock().unwrap().take().expect("waker recorded");
            stale.wake_by_ref();
            stale.wake_by_ref();
            for _ in 0..20 {
                task::yield_now().await;
            }
            assert_eq!(
                polls.load(Ordering::SeqCst),
                1,
                "finished task is never re-polled"
            );

            // Aborted task: the same holds after cancellation.
            let aborted = task::spawn(StormTask {
                storm: 0,
                polls: Arc::new(AtomicUsize::new(0)),
                release: Arc::new(AtomicBool::new(false)),
                waker_slot: Arc::clone(&slot),
            });
            task::yield_now().await;
            aborted.abort();
            assert!(aborted.await.unwrap_err().is_cancelled());
            let aborted_waker = slot.lock().unwrap().take().expect("waker recorded");
            aborted_waker.wake_by_ref();
            for _ in 0..20 {
                task::yield_now().await;
            }
            stale
        });
        drop(rt);
        // Waking after the runtime is gone must also be harmless.
        stale.wake();
    });
}

#[test]
fn many_queued_tasks_drain_with_small_poll_budget() {
    watchdog(|| {
        let rt = runtime_with(Limits::new().with_max_polls_per_tick(1));
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        rt.block_on(async move {
            let handles: Vec<_> = (0..1_000)
                .map(|_| {
                    let c = Arc::clone(&c);
                    task::spawn(async move {
                        c.fetch_add(1, Ordering::SeqCst);
                    })
                })
                .collect();
            for h in handles {
                h.await.unwrap();
            }
        });
        assert_eq!(counter.load(Ordering::SeqCst), 1_000);
    });
}

#[test]
fn spawned_task_future_with_deque_state_round_trips() {
    // A non-trivial output type proves the result travels intact through the
    // JoinHandle (no clone, no truncation).
    watchdog(|| {
        let out = runtime().block_on(async {
            task::spawn(async {
                let mut q: VecDeque<u32> = (1..=5).collect();
                q.rotate_left(2);
                q
            })
            .await
            .unwrap()
        });
        assert_eq!(out, VecDeque::from([3, 4, 5, 1, 2]));
    });
}
