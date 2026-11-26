//! Public-API adversarial tests for `akku::sync` (PR-A scope).
//!
//! Test names map directly to the numbered items in `_dev/DESIGN.md`
//! §"G3 test plan (adversarial)". Every runtime or multi-threaded test runs
//! under [`watchdog`] so a lost wakeup or livelock fails fast instead of
//! hanging CI.

use std::cell::Cell;
use std::error::Error;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::pin::{pin, Pin};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use akku::sync::{
    self, channel, oneshot, AcquireError, AsyncMutex, AsyncMutexGuard, AsyncMutexOwnedGuard,
    Notified, Notify, Semaphore, SemaphoreOwnedPermit, SemaphorePermit, TryAcquireError,
};
use akku::task::{self, Runtime};

// --- Test harness ------------------------------------------------------------

const WATCHDOG: Duration = Duration::from_secs(30);

/// Runs `body` on a fresh thread and panics if it does not finish within
/// [`WATCHDOG`]. Re-raises any panic payload unchanged so
/// `#[should_panic(expected = ...)]` still matches.
fn watchdog<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std_mpsc::channel();
    let worker = thread::Builder::new()
        .name("sync-test-body".into())
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

/// Waker that counts every `wake` / `wake_by_ref` call.
struct CountingWaker {
    wakes: AtomicUsize,
}

impl CountingWaker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            wakes: AtomicUsize::new(0),
        })
    }

    fn count(&self) -> usize {
        self.wakes.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.wakes.store(0, Ordering::SeqCst);
    }

    fn waker(self: &Arc<Self>) -> Waker {
        Waker::from(Arc::clone(self))
    }
}

impl Wake for CountingWaker {
    fn wake(self: Arc<Self>) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }
}

/// Waker that panics when woken.
struct PanickingWaker;

impl Wake for PanickingWaker {
    fn wake(self: Arc<Self>) {
        panic!("waker boom");
    }

    fn wake_by_ref(self: &Arc<Self>) {
        panic!("waker boom");
    }
}

/// Value that increments a shared counter when dropped.
#[derive(Debug)]
struct DropCounter {
    id: usize,
    drops: Arc<AtomicUsize>,
}

impl DropCounter {
    fn new(id: usize, drops: &Arc<AtomicUsize>) -> Self {
        Self {
            id,
            drops: Arc::clone(drops),
        }
    }
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn poll_once<F: Future>(mut fut: Pin<&mut F>, waker: &Arc<CountingWaker>) -> Poll<F::Output> {
    let w = waker.waker();
    let mut cx = Context::from_waker(&w);
    fut.as_mut().poll(&mut cx)
}

fn poll_noop<F: Future>(mut fut: Pin<&mut F>) -> Poll<F::Output> {
    let mut cx = Context::from_waker(Waker::noop());
    fut.as_mut().poll(&mut cx)
}

// =============================================================================
// Item 1: Drop while waiting & drop after grant (cancellation table)
// =============================================================================

#[test]
fn item01_mutex_drop_while_queued_leaves_no_hole() {
    let m = AsyncMutex::new(10u32);
    let guard = m.try_lock().expect("initial try_lock succeeds");

    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let mut fut_a = Box::pin(m.lock());
    let mut fut_b = pin!(m.lock());

    assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
    assert!(poll_once(fut_b.as_mut(), &wb).is_pending());

    // Cancel A while queued, before release.
    drop(fut_a);
    assert_eq!(wb.count(), 0);

    // Releasing the lock must hand ownership directly to B.
    drop(guard);
    assert_eq!(wb.count(), 1);
    let Poll::Ready(mut g_b) = poll_once(fut_b.as_mut(), &wb) else {
        panic!("waiter B must complete after A was cancelled and lock released");
    };
    *g_b += 5;
    drop(g_b);
    assert_eq!(*m.try_lock().expect("lock free after B drops"), 15);
}

#[test]
fn item01_mutex_drop_after_grant_hands_off_to_next_or_frees() {
    let m = AsyncMutex::new(1u32);
    let guard = m.try_lock().expect("initial lock");

    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let mut fut_a = Box::pin(m.lock());
    let mut fut_b = pin!(m.lock());

    assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
    assert!(poll_once(fut_b.as_mut(), &wb).is_pending());

    // Release hands ownership to A, not B.
    drop(guard);
    assert_eq!(wa.count(), 1);
    assert_eq!(wb.count(), 0);
    assert!(
        m.try_lock().is_none(),
        "try_lock must not barge while A holds grant"
    );

    // Drop A unpolled: ownership must forward to B.
    drop(fut_a);
    assert_eq!(wb.count(), 1);
    assert!(
        m.try_lock().is_none(),
        "try_lock must not barge while B holds forwarded grant"
    );

    let Poll::Ready(g_b) = poll_once(fut_b.as_mut(), &wb) else {
        panic!("waiter B must acquire forwarded ownership");
    };
    assert_eq!(*g_b, 1);

    // Now test dropping a granted waiter when no other waiter exists.
    let wc = CountingWaker::new();
    let mut fut_c = Box::pin(m.lock());
    assert!(poll_once(fut_c.as_mut(), &wc).is_pending());
    drop(g_b);
    assert_eq!(wc.count(), 1);
    assert!(m.try_lock().is_none());
    drop(fut_c);
    assert!(
        m.try_lock().is_some(),
        "dropping sole granted waiter must free lock"
    );
}

#[test]
fn item01_mutex_lock_owned_drop_while_queued_and_after_grant() {
    let m = Arc::new(AsyncMutex::new(42u32));
    let guard = Arc::clone(&m)
        .try_lock_owned()
        .expect("initial try_lock_owned");

    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let wc = CountingWaker::new();
    let mut fut_a = Box::pin(Arc::clone(&m).lock_owned());
    let mut fut_b = Box::pin(Arc::clone(&m).lock_owned());
    let mut fut_c = pin!(Arc::clone(&m).lock_owned());

    assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
    assert!(poll_once(fut_b.as_mut(), &wb).is_pending());
    assert!(poll_once(fut_c.as_mut(), &wc).is_pending());

    // Drop A while queued.
    drop(fut_a);
    // Release grants B.
    drop(guard);
    assert_eq!(wb.count(), 1);
    assert_eq!(wc.count(), 0);
    // Drop B unpolled -> forwards to C.
    drop(fut_b);
    assert_eq!(wc.count(), 1);

    let Poll::Ready(g_c) = poll_once(fut_c.as_mut(), &wc) else {
        panic!("owned waiter C must receive forwarded grant");
    };
    assert_eq!(*g_c, 42);
}

#[test]
fn item01_semaphore_drop_while_queued_and_after_grant() {
    let sem = Semaphore::new(0);
    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let wc = CountingWaker::new();

    let mut fut_a = Box::pin(sem.acquire());
    let mut fut_b = Box::pin(sem.acquire());
    let mut fut_c = pin!(sem.acquire());

    assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
    assert!(poll_once(fut_b.as_mut(), &wb).is_pending());
    assert!(poll_once(fut_c.as_mut(), &wc).is_pending());

    // Cancel A while queued.
    drop(fut_a);
    // Add 1 permit -> grants B.
    sem.add_permits(1);
    assert_eq!(wb.count(), 1);
    assert_eq!(wc.count(), 0);
    assert_eq!(sem.available_permits(), 0);

    // Drop B unpolled -> forwards permit to C.
    drop(fut_b);
    assert_eq!(wc.count(), 1);
    let Poll::Ready(Ok(p_c)) = poll_once(fut_c.as_mut(), &wc) else {
        panic!("waiter C must receive forwarded permit");
    };
    assert_eq!(p_c.num_permits(), 1);
    drop(p_c);
    assert_eq!(sem.available_permits(), 1);

    // Sole granted waiter dropped unpolled returns permit to pool.
    let p_hold = sem.try_acquire().expect("take sole permit");
    let wd = CountingWaker::new();
    let mut fut_d = Box::pin(sem.acquire());
    assert!(poll_once(fut_d.as_mut(), &wd).is_pending());
    drop(p_hold);
    assert_eq!(wd.count(), 1);
    assert_eq!(sem.available_permits(), 0);
    drop(fut_d);
    assert_eq!(sem.available_permits(), 1);
}

#[test]
fn item01_semaphore_partial_grant_drop_returns_permits_to_next() {
    let sem = Semaphore::new(3);
    let wa = CountingWaker::new();
    let wb = CountingWaker::new();

    // Head asks 5 of 3 available -> receives 3 as partial grant.
    let mut fut_a = Box::pin(sem.acquire_many(5));
    let mut fut_b = pin!(sem.acquire_many(2));

    assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
    assert_eq!(sem.available_permits(), 0);
    assert!(poll_once(fut_b.as_mut(), &wb).is_pending());
    assert_eq!(wb.count(), 0);

    // Drop head: 2 of its 3 partial permits satisfy B, 1 returns to pool.
    drop(fut_a);
    assert_eq!(wb.count(), 1);
    assert_eq!(sem.available_permits(), 1);

    let Poll::Ready(Ok(p_b)) = poll_once(fut_b.as_mut(), &wb) else {
        panic!("waiter B must receive 2 of the 3 returned partial permits");
    };
    assert_eq!(p_b.num_permits(), 2);
    drop(p_b);
    assert_eq!(sem.available_permits(), 3);
}

#[test]
fn item01_semaphore_owned_drop_after_grant_returns_permits() {
    let sem = Arc::new(Semaphore::new(2));
    let hold = Arc::clone(&sem).try_acquire_many_owned(2).expect("hold 2");

    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let mut fut_a = Box::pin(Arc::clone(&sem).acquire_many_owned(2));
    let mut fut_b = pin!(Arc::clone(&sem).acquire_owned());

    assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
    assert!(poll_once(fut_b.as_mut(), &wb).is_pending());

    drop(hold);
    assert_eq!(wa.count(), 1);
    assert_eq!(wb.count(), 0);

    // Drop granted owned waiter A unpolled: B gets 1 permit and 1 returns to pool.
    drop(fut_a);
    assert_eq!(wb.count(), 1);
    assert_eq!(sem.available_permits(), 1);

    let Poll::Ready(Ok(p_b)) = poll_once(fut_b.as_mut(), &wb) else {
        panic!("owned waiter B must complete");
    };
    assert!(Arc::ptr_eq(p_b.semaphore(), &sem));
    assert_eq!(p_b.num_permits(), 1);
    drop(p_b);
    assert_eq!(sem.available_permits(), 2);
}

#[test]
fn item01_notify_drop_while_queued_and_notify_one_forwarding() {
    let n = Notify::new();
    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let wc = CountingWaker::new();

    let mut a = n.notified();
    let mut b = n.notified();
    let mut c = n.notified();

    assert!(poll_once(Pin::new(&mut a), &wa).is_pending());
    assert!(poll_once(Pin::new(&mut b), &wb).is_pending());
    assert!(poll_once(Pin::new(&mut c), &wc).is_pending());

    // Drop A while queued -> leaves no hole.
    drop(a);
    // notify_one hands token to B.
    n.notify_one();
    assert_eq!(wb.count(), 1);
    assert_eq!(wc.count(), 0);

    // Drop B unpolled -> forwards token to C.
    drop(b);
    assert_eq!(wc.count(), 1);
    assert_eq!(poll_once(Pin::new(&mut c), &wc), Poll::Ready(()));

    // Sole waiter dropped after notify_one -> permit is stored.
    let wd = CountingWaker::new();
    let mut d = n.notified();
    assert!(poll_once(Pin::new(&mut d), &wd).is_pending());
    n.notify_one();
    assert_eq!(wd.count(), 1);
    drop(d);

    let mut e = n.notified();
    assert_eq!(
        poll_noop(Pin::new(&mut e)),
        Poll::Ready(()),
        "stored permit must complete the next notified() immediately"
    );

    // Stored permit was consumed; a further waiter must wait.
    let mut f = n.notified();
    assert!(poll_noop(Pin::new(&mut f)).is_pending());
}

#[test]
fn item01_notify_waiters_epoch_enable_and_no_forwarding_on_drop() {
    let n = Notify::new();

    // notify_one coalesces to at most one stored permit.
    n.notify_one();
    n.notify_one();
    let mut p1 = n.notified();
    let mut p2 = n.notified();
    assert_eq!(poll_noop(Pin::new(&mut p1)), Poll::Ready(()));
    assert!(poll_noop(Pin::new(&mut p2)).is_pending());
    drop(p2);

    // Created BEFORE notify_waiters -> completes even if never polled before notify_waiters.
    let mut unpolled_before = n.notified();
    let mut polled_before = n.notified();
    let wp = CountingWaker::new();
    assert!(poll_once(Pin::new(&mut polled_before), &wp).is_pending());

    n.notify_waiters();
    assert_eq!(wp.count(), 1);

    // Drop polled_before unpolled: notify_waiters MUST NOT forward or store a permit.
    drop(polled_before);

    let mut created_after = n.notified();
    assert!(
        poll_noop(Pin::new(&mut created_after)).is_pending(),
        "notify_waiters must neither store a permit nor forward on drop"
    );
    drop(created_after);

    assert_eq!(
        poll_noop(Pin::new(&mut unpolled_before)),
        Poll::Ready(()),
        "Notified created before notify_waiters must complete without prior poll"
    );

    // Notified::enable joins the queue before notify_one without a poll.
    let mut en = n.notified();
    assert!(!en.enable());
    let mut second = n.notified();
    let w2 = CountingWaker::new();
    assert!(poll_once(Pin::new(&mut second), &w2).is_pending());
    n.notify_one();
    assert_eq!(
        w2.count(),
        0,
        "notify_one must go to the earlier enable()d waiter, not the second waiter"
    );
    assert!(en.enable(), "enable must report true once notified");
    assert_eq!(poll_noop(Pin::new(&mut en)), Poll::Ready(()));
    assert!(poll_once(Pin::new(&mut second), &w2).is_pending());
    drop(second);
}

#[test]
fn item01_oneshot_drop_while_waiting_preserves_value_and_closed() {
    let (mut tx, mut rx) = oneshot::channel::<u32>();

    // Poll &mut rx then drop the poll context; value sent later is preserved.
    let wr = CountingWaker::new();
    assert!(poll_once(Pin::new(&mut rx), &wr).is_pending());
    assert_eq!(rx.try_recv(), Err(oneshot::TryRecvError::Empty));

    // Poll tx.closed() then cancel it before rx closes.
    let ws = CountingWaker::new();
    {
        let mut closed_fut = pin!(tx.closed());
        assert!(poll_once(closed_fut.as_mut(), &ws).is_pending());
    }

    assert!(!tx.is_closed());
    tx.send(123).expect("send succeeds while rx alive");
    assert_eq!(wr.count(), 1);
    assert_eq!(poll_once(Pin::new(&mut rx), &wr), Poll::Ready(Ok(123)));
}

#[test]
fn item01_channel_send_drop_while_queued_vs_after_grant() {
    let drops = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = channel::bounded::<DropCounter>(1);

    tx.try_send(DropCounter::new(1, &drops))
        .expect("fill buffer");

    // Queue A (id=2) then B (id=3).
    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let mut fut_a = Box::pin(tx.send(DropCounter::new(2, &drops)));
    let mut fut_b = Box::pin(tx.send(DropCounter::new(3, &drops)));
    assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
    assert!(poll_once(fut_b.as_mut(), &wb).is_pending());
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    // Drop A while queued -> its value is dropped immediately and NOT sent.
    drop(fut_a);
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    // Receive item 1 -> B is granted (item 3 moves into buffer).
    let first = rx.try_recv().expect("recv 1");
    assert_eq!(first.id, 1);
    drop(first);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert_eq!(wb.count(), 1);

    // Drop B unpolled AFTER grant -> item 3 was already moved into buffer and must be delivered!
    drop(fut_b);
    assert_eq!(drops.load(Ordering::SeqCst), 2);

    let second = rx.try_recv().expect("recv 3 delivered after B dropped");
    assert_eq!(second.id, 3);
    drop(second);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
}

#[test]
fn item01_channel_reserve_and_permit_drop_releases_slot() {
    let (tx, rx) = channel::bounded::<u32>(1);
    tx.try_send(10).expect("fill");

    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let wc = CountingWaker::new();
    let mut res_a = Box::pin(tx.reserve());
    let mut res_b = Box::pin(tx.reserve());
    let mut res_c = pin!(tx.reserve());

    assert!(poll_once(res_a.as_mut(), &wa).is_pending());
    assert!(poll_once(res_b.as_mut(), &wb).is_pending());
    assert!(poll_once(res_c.as_mut(), &wc).is_pending());

    // Drop A while queued.
    drop(res_a);
    // Free slot -> grants B.
    assert_eq!(rx.try_recv(), Ok(10));
    assert_eq!(wb.count(), 1);
    assert_eq!(wc.count(), 0);

    // Drop B unpolled after grant -> forwards slot to C.
    drop(res_b);
    assert_eq!(wc.count(), 1);
    let Poll::Ready(Ok(permit_c)) = poll_once(res_c.as_mut(), &wc) else {
        panic!("reserve C must receive forwarded slot");
    };

    // While permit_c is held, buffer has no free slot for try_send; queue a FIFO reserve waiter.
    assert_eq!(tx.try_send(99), Err(channel::TrySendError::Full(99)));
    let wd = CountingWaker::new();
    let mut res_d = pin!(tx.reserve());
    assert!(poll_once(res_d.as_mut(), &wd).is_pending());

    // Drop permit_c unused -> hands the reserved slot directly to queued waiter D.
    drop(permit_c);
    assert_eq!(
        wd.count(),
        1,
        "Permit::drop must hand the released slot directly to the next queued waiter"
    );
    assert_eq!(
        tx.try_send(99),
        Err(channel::TrySendError::Full(99)),
        "try_send must not barge past waiter D granted by Permit::drop"
    );
    let Poll::Ready(Ok(permit_d)) = poll_once(res_d.as_mut(), &wd) else {
        panic!("reserve D must receive slot handed off by Permit::drop");
    };
    permit_d.send(20);
    assert_eq!(rx.try_recv(), Ok(20));

    // Dropping a permit with no waiter queued returns the slot to the channel.
    let permit_e = tx.try_reserve().expect("reserve when empty");
    drop(permit_e);
    assert!(tx.is_empty());
    let permit_f = tx
        .try_reserve()
        .expect("slot free after unused permit drop with no waiters");
    permit_f.send(21);
    assert_eq!(rx.try_recv(), Ok(21));
}

#[test]
fn item01_channel_recv_claim_drop_forwards_preserves_order_and_capacity() {
    let (tx, rx) = channel::bounded::<u32>(2);

    let wa = CountingWaker::new();
    let wb = CountingWaker::new();
    let mut recv_a = Box::pin(rx.recv());
    let mut recv_b = pin!(rx.recv());

    assert!(poll_once(recv_a.as_mut(), &wa).is_pending());
    assert!(poll_once(recv_b.as_mut(), &wb).is_pending());

    // Send 1 -> grants claim to A; len is 1 (<= capacity 2).
    tx.try_send(1).expect("send 1");
    assert_eq!(wa.count(), 1);
    assert_eq!(wb.count(), 0);
    assert_eq!(tx.len(), 1);

    // Newcomer try_recv must not barge past A's claim.
    assert_eq!(rx.try_recv(), Err(channel::TryRecvError::Empty));

    // Drop A unpolled BEFORE sending 2 -> A's claim must forward directly to unclaimed B
    // and wake B right away, while try_recv still cannot barge past B's forwarded claim.
    drop(recv_a);
    assert_eq!(
        wb.count(),
        1,
        "dropping claimed recv_a must forward the claim and wake unclaimed recv_b"
    );
    assert_eq!(
        rx.try_recv(),
        Err(channel::TryRecvError::Empty),
        "try_recv must not barge past recv_b's forwarded claim"
    );
    assert_eq!(tx.len(), 1);

    // Send 2 -> buffer reaches capacity 2 without exceeding it.
    tx.try_send(2).expect("send 2");
    assert_eq!(tx.len(), 2);
    assert_eq!(tx.try_send(3), Err(channel::TrySendError::Full(3)));

    // B must receive the front message (1), and try_recv gets 2 in exact order.
    let Poll::Ready(Ok(v1)) = poll_once(recv_b.as_mut(), &wb) else {
        panic!("receiver B must take front message");
    };
    assert_eq!(v1, 1);
    assert_eq!(rx.try_recv(), Ok(2));

    // Sole claimed receiver dropped unpolled -> claim lapses and next try_recv gets the message.
    let wc = CountingWaker::new();
    let mut recv_c = Box::pin(rx.recv());
    assert!(poll_once(recv_c.as_mut(), &wc).is_pending());
    tx.try_send(42).expect("send 42");
    assert_eq!(wc.count(), 1);
    assert_eq!(rx.try_recv(), Err(channel::TryRecvError::Empty));
    drop(recv_c);
    assert_eq!(rx.try_recv(), Ok(42));
}

// =============================================================================
// Item 2: Completed future dropped after slot reuse (ABA)
// =============================================================================

#[test]
fn item02_aba_completed_future_dropped_after_slot_reuse() {
    // AsyncMutex
    {
        let m = AsyncMutex::new(0u32);
        let g0 = m.try_lock().expect("g0");
        let wa = CountingWaker::new();
        let mut fut_a = Box::pin(m.lock());
        assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
        drop(g0);
        let Poll::Ready(g_a) = poll_once(fut_a.as_mut(), &wa) else {
            panic!("A must acquire mutex");
        };
        // Keep fut_a alive while B queues (reusing A's slab slot).
        let wb = CountingWaker::new();
        let mut fut_b = pin!(m.lock());
        assert!(poll_once(fut_b.as_mut(), &wb).is_pending());
        drop(fut_a); // Must NOT touch B's reused slot!
        drop(g_a);
        assert_eq!(wb.count(), 1);
        assert!(poll_once(fut_b.as_mut(), &wb).is_ready());
    }

    // Semaphore
    {
        let sem = Semaphore::new(0);
        let wa = CountingWaker::new();
        let mut fut_a = Box::pin(sem.acquire());
        assert!(poll_once(fut_a.as_mut(), &wa).is_pending());
        sem.add_permits(1);
        let Poll::Ready(Ok(p_a)) = poll_once(fut_a.as_mut(), &wa) else {
            panic!("A must acquire semaphore");
        };
        let wb = CountingWaker::new();
        let mut fut_b = pin!(sem.acquire());
        assert!(poll_once(fut_b.as_mut(), &wb).is_pending());
        drop(fut_a);
        drop(p_a);
        assert_eq!(wb.count(), 1);
        assert!(matches!(poll_once(fut_b.as_mut(), &wb), Poll::Ready(Ok(_))));
    }

    // Notify
    {
        let n = Notify::new();
        let wa = CountingWaker::new();
        let mut a = n.notified();
        assert!(poll_once(Pin::new(&mut a), &wa).is_pending());
        n.notify_one();
        assert_eq!(poll_once(Pin::new(&mut a), &wa), Poll::Ready(()));

        let wb = CountingWaker::new();
        let mut b = n.notified();
        assert!(poll_once(Pin::new(&mut b), &wb).is_pending());
        drop(a);
        n.notify_one();
        assert_eq!(wb.count(), 1);
        assert_eq!(poll_once(Pin::new(&mut b), &wb), Poll::Ready(()));
    }

    // Channel recv & send
    {
        let (tx, rx) = channel::bounded::<u32>(1);
        let wa = CountingWaker::new();
        let mut recv_a = Box::pin(rx.recv());
        assert!(poll_once(recv_a.as_mut(), &wa).is_pending());
        tx.try_send(1).expect("send 1");
        assert_eq!(poll_once(recv_a.as_mut(), &wa), Poll::Ready(Ok(1)));

        let wb = CountingWaker::new();
        let mut recv_b = pin!(rx.recv());
        assert!(poll_once(recv_b.as_mut(), &wb).is_pending());
        drop(recv_a);
        tx.try_send(2).expect("send 2");
        assert_eq!(wb.count(), 1);
        assert_eq!(poll_once(recv_b.as_mut(), &wb), Poll::Ready(Ok(2)));

        // Sender ABA
        tx.try_send(10).expect("fill");
        let wsa = CountingWaker::new();
        let mut send_a = Box::pin(tx.send(11));
        assert!(poll_once(send_a.as_mut(), &wsa).is_pending());
        assert_eq!(rx.try_recv(), Ok(10));
        assert_eq!(poll_once(send_a.as_mut(), &wsa), Poll::Ready(Ok(())));

        let wsb = CountingWaker::new();
        let mut send_b = pin!(tx.send(12));
        assert!(poll_once(send_b.as_mut(), &wsb).is_pending());
        drop(send_a);
        assert_eq!(rx.try_recv(), Ok(11));
        assert_eq!(wsb.count(), 1);
        assert_eq!(poll_once(send_b.as_mut(), &wsb), Poll::Ready(Ok(())));
        assert_eq!(rx.try_recv(), Ok(12));
    }
}

// =============================================================================
// Item 3: Lost wakeups & 1,000-round cross-thread ping-pong
// =============================================================================

#[test]
fn item03_lost_wakeup_release_before_and_between_polls() {
    // Release BEFORE first poll (future created while contended, released before poll).
    {
        let m = AsyncMutex::new(7u32);
        let g = m.try_lock().expect("lock");
        let mut fut = pin!(m.lock());
        drop(g);
        let Poll::Ready(g2) = poll_noop(fut.as_mut()) else {
            panic!("mutex lock must complete when released before first poll");
        };
        assert_eq!(*g2, 7);
    }

    {
        let sem = Semaphore::new(0);
        let mut fut = pin!(sem.acquire());
        sem.add_permits(1);
        assert!(matches!(poll_noop(fut.as_mut()), Poll::Ready(Ok(_))));
    }

    {
        let n = Notify::new();
        let mut fut = n.notified();
        n.notify_one();
        assert_eq!(poll_noop(Pin::new(&mut fut)), Poll::Ready(()));
    }

    {
        let (tx, mut rx) = oneshot::channel::<u32>();
        tx.send(55).expect("send before poll");
        assert_eq!(poll_noop(Pin::new(&mut rx)), Poll::Ready(Ok(55)));
    }

    {
        let (tx, rx) = channel::bounded::<u32>(1);
        let mut rf = pin!(rx.recv());
        tx.try_send(88).expect("send before recv poll");
        assert_eq!(poll_noop(rf.as_mut()), Poll::Ready(Ok(88)));
    }

    // Release from another thread while task::block_on is parked.
    watchdog(|| {
        let m = Arc::new(AsyncMutex::new(0u32));
        let g = Arc::clone(&m).try_lock_owned().expect("lock");
        let n = Arc::new(Notify::new());
        let n2 = Arc::clone(&n);
        let (otx, orx) = oneshot::channel::<u32>();
        let (ctx, crx) = channel::bounded::<u32>(1);

        let helper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            drop(g);
            n2.notify_one();
            otx.send(9).expect("oneshot send");
            ctx.try_send(10).expect("channel send");
        });

        task::block_on(async {
            let _g = m.lock().await;
            n.notified().await;
            assert_eq!(orx.await.expect("oneshot recv"), 9);
            assert_eq!(crx.recv().await.expect("channel recv"), 10);
        });
        helper.join().expect("helper thread");
    });
}

#[test]
fn item03_cross_thread_ping_pong_1000_rounds() {
    const ROUNDS: usize = 1_000;

    // Channel bounded(1) ping-pong
    watchdog(|| {
        let (tx_a, rx_a) = channel::bounded::<usize>(1);
        let (tx_b, rx_b) = channel::bounded::<usize>(1);

        let peer = thread::spawn(move || {
            task::block_on(async {
                for i in 0..ROUNDS {
                    let v = rx_a.recv().await.expect("peer recv");
                    assert_eq!(v, i);
                    tx_b.send(v + 1).await.expect("peer send");
                }
            });
        });

        task::block_on(async {
            for i in 0..ROUNDS {
                tx_a.send(i).await.expect("main send");
                let v = rx_b.recv().await.expect("main recv");
                assert_eq!(v, i + 1);
            }
        });
        peer.join().expect("peer join");
    });

    // Channel bounded(0) rendezvous ping-pong
    watchdog(|| {
        let (tx_a, rx_a) = channel::bounded::<usize>(0);
        let (tx_b, rx_b) = channel::bounded::<usize>(0);

        let peer = thread::spawn(move || {
            task::block_on(async {
                for i in 0..ROUNDS {
                    let v = rx_a.recv().await.expect("rendezvous peer recv");
                    assert_eq!(v, i);
                    tx_b.send(v + 1).await.expect("rendezvous peer send");
                }
            });
        });

        task::block_on(async {
            for i in 0..ROUNDS {
                tx_a.send(i).await.expect("rendezvous main send");
                let v = rx_b.recv().await.expect("rendezvous main recv");
                assert_eq!(v, i + 1);
            }
        });
        peer.join().expect("peer join");
    });

    // Oneshot chain ping-pong
    watchdog(|| {
        let (seed_tx, seed_rx) = oneshot::channel::<(usize, oneshot::Sender<usize>)>();
        let (req_tx, req_rx) = channel::bounded::<(usize, oneshot::Sender<usize>)>(1);

        let peer = thread::spawn(move || {
            task::block_on(async {
                let (v, reply) = seed_rx.await.expect("seed");
                reply.send(v + 1).expect("seed reply");
                for _ in 1..ROUNDS {
                    let (v, reply) = req_rx.recv().await.expect("req");
                    reply.send(v + 1).expect("reply");
                }
            });
        });

        task::block_on(async {
            let (rtx, rrx) = oneshot::channel();
            seed_tx.send((0, rtx)).expect("send seed");
            assert_eq!(rrx.await.expect("recv seed reply"), 1);
            for i in 1..ROUNDS {
                let (rtx, rrx) = oneshot::channel();
                req_tx.send((i, rtx)).await.expect("send req");
                assert_eq!(rrx.await.expect("recv reply"), i + 1);
            }
        });
        peer.join().expect("peer join");
    });

    // Notify + AsyncMutex ping-pong
    watchdog(|| {
        let turn = Arc::new(AsyncMutex::new(0usize));
        let ping = Arc::new(Notify::new());
        let pong = Arc::new(Notify::new());

        let turn2 = Arc::clone(&turn);
        let ping2 = Arc::clone(&ping);
        let pong2 = Arc::clone(&pong);

        let peer = thread::spawn(move || {
            task::block_on(async {
                for i in 0..ROUNDS {
                    loop {
                        let notified = ping2.notified();
                        let mut g = turn2.lock().await;
                        if *g == 2 * i + 1 {
                            *g = 2 * i + 2;
                            drop(g);
                            pong2.notify_one();
                            break;
                        }
                        drop(g);
                        notified.await;
                    }
                }
            });
        });

        task::block_on(async {
            for i in 0..ROUNDS {
                {
                    let mut g = turn.lock().await;
                    assert_eq!(*g, 2 * i);
                    *g = 2 * i + 1;
                }
                ping.notify_one();
                loop {
                    let notified = pong.notified();
                    let g = turn.lock().await;
                    if *g == 2 * i + 2 {
                        break;
                    }
                    drop(g);
                    notified.await;
                }
            }
        });
        peer.join().expect("peer join");
        assert_eq!(*turn.try_lock().expect("unlocked"), 2 * ROUNDS);
    });
}

// =============================================================================
// Item 4: Wake storms and spurious polls
// =============================================================================

#[test]
fn item04_spurious_polls_10000_changing_wakers() {
    const POLLS: usize = 10_000;

    // AsyncMutex
    {
        let m = AsyncMutex::new(0u32);
        let g = m.try_lock().expect("lock");
        let mut fut_a = pin!(m.lock());
        let mut fut_b = pin!(m.lock());

        let mut last_wa = CountingWaker::new();
        for _ in 0..POLLS {
            last_wa = CountingWaker::new();
            assert!(poll_once(fut_a.as_mut(), &last_wa).is_pending());
        }
        let wb = CountingWaker::new();
        assert!(poll_once(fut_b.as_mut(), &wb).is_pending());

        drop(g);
        assert_eq!(last_wa.count(), 1);
        assert_eq!(wb.count(), 0);
        let Poll::Ready(ga) = poll_once(fut_a.as_mut(), &last_wa) else {
            panic!("A must acquire after spurious polls");
        };
        drop(ga);
        assert_eq!(wb.count(), 1);
        assert!(poll_once(fut_b.as_mut(), &wb).is_ready());
    }

    // Semaphore
    {
        let sem = Semaphore::new(0);
        let mut fut_a = pin!(sem.acquire());
        let mut fut_b = pin!(sem.acquire());
        let mut last_wa = CountingWaker::new();
        for _ in 0..POLLS {
            last_wa = CountingWaker::new();
            assert!(poll_once(fut_a.as_mut(), &last_wa).is_pending());
        }
        let wb = CountingWaker::new();
        assert!(poll_once(fut_b.as_mut(), &wb).is_pending());

        sem.add_permits(1);
        assert_eq!(last_wa.count(), 1);
        assert_eq!(wb.count(), 0);
        let Poll::Ready(Ok(pa)) = poll_once(fut_a.as_mut(), &last_wa) else {
            panic!("A must acquire semaphore");
        };
        drop(pa);
        assert_eq!(wb.count(), 1);
        assert!(matches!(poll_once(fut_b.as_mut(), &wb), Poll::Ready(Ok(_))));
    }

    // Notify
    {
        let n = Notify::new();
        let mut a = n.notified();
        let mut b = n.notified();
        let mut last_wa = CountingWaker::new();
        for _ in 0..POLLS {
            last_wa = CountingWaker::new();
            assert!(poll_once(Pin::new(&mut a), &last_wa).is_pending());
        }
        let wb = CountingWaker::new();
        assert!(poll_once(Pin::new(&mut b), &wb).is_pending());

        n.notify_one();
        assert_eq!(last_wa.count(), 1);
        assert_eq!(wb.count(), 0);
        assert_eq!(poll_once(Pin::new(&mut a), &last_wa), Poll::Ready(()));
        n.notify_one();
        assert_eq!(wb.count(), 1);
        assert_eq!(poll_once(Pin::new(&mut b), &wb), Poll::Ready(()));
    }

    // Channel recv & send
    {
        let (tx, rx) = channel::bounded::<u32>(1);
        let mut ra = pin!(rx.recv());
        let mut rb = pin!(rx.recv());
        let mut last_wa = CountingWaker::new();
        for _ in 0..POLLS {
            last_wa = CountingWaker::new();
            assert!(poll_once(ra.as_mut(), &last_wa).is_pending());
        }
        let wb = CountingWaker::new();
        assert!(poll_once(rb.as_mut(), &wb).is_pending());

        tx.try_send(1).expect("send 1");
        assert_eq!(last_wa.count(), 1);
        assert_eq!(wb.count(), 0);
        assert_eq!(poll_once(ra.as_mut(), &last_wa), Poll::Ready(Ok(1)));

        tx.try_send(2).expect("send 2");
        assert_eq!(wb.count(), 1);
        assert_eq!(poll_once(rb.as_mut(), &wb), Poll::Ready(Ok(2)));

        // Queued sends polled 10,000 times with changing wakers.
        tx.try_send(10).expect("fill buffer");
        let mut sa = pin!(tx.send(11));
        let mut sb = pin!(tx.send(12));
        let mut last_wsa = CountingWaker::new();
        for _ in 0..POLLS {
            last_wsa = CountingWaker::new();
            assert!(poll_once(sa.as_mut(), &last_wsa).is_pending());
        }
        let wsb = CountingWaker::new();
        assert!(poll_once(sb.as_mut(), &wsb).is_pending());

        assert_eq!(rx.try_recv(), Ok(10));
        assert_eq!(last_wsa.count(), 1);
        assert_eq!(wsb.count(), 0);
        assert_eq!(poll_once(sa.as_mut(), &last_wsa), Poll::Ready(Ok(())));

        assert_eq!(rx.try_recv(), Ok(11));
        assert_eq!(wsb.count(), 1);
        assert_eq!(poll_once(sb.as_mut(), &wsb), Poll::Ready(Ok(())));
        assert_eq!(rx.try_recv(), Ok(12));
    }
}

#[test]
fn item04_notify_waiters_10000_waiters_woken_once() {
    const N: usize = 10_000;
    let notify = Notify::new();
    let mut waiters: Vec<(Notified<'_>, Arc<CountingWaker>)> = Vec::with_capacity(N);

    for _ in 0..N {
        let mut fut = notify.notified();
        let w = CountingWaker::new();
        assert!(poll_once(Pin::new(&mut fut), &w).is_pending());
        waiters.push((fut, w));
    }

    notify.notify_waiters();

    for (mut fut, w) in waiters {
        assert_eq!(w.count(), 1, "each waiter must be woken exactly once");
        assert_eq!(poll_once(Pin::new(&mut fut), &w), Poll::Ready(()));
    }
}

#[test]
fn item04_panicking_waker_in_notify_waiters_isolates_and_stays_usable() {
    let notify = Notify::new();
    let wa = CountingWaker::new();
    let wc = CountingWaker::new();
    let bad = Waker::from(Arc::new(PanickingWaker));

    let mut a = notify.notified();
    let mut b = notify.notified();
    let mut c = notify.notified();

    assert!(poll_once(Pin::new(&mut a), &wa).is_pending());
    {
        let mut cx = Context::from_waker(&bad);
        assert!(Pin::new(&mut b).poll(&mut cx).is_pending());
    }
    assert!(poll_once(Pin::new(&mut c), &wc).is_pending());

    let res = panic::catch_unwind(AssertUnwindSafe(|| notify.notify_waiters()));
    assert!(res.is_err(), "panicking waker must surface to caller");

    assert_eq!(wa.count(), 1, "waiter A before panicking waker must wake");
    assert_eq!(wc.count(), 1, "waiter C after panicking waker must wake");
    assert_eq!(poll_once(Pin::new(&mut a), &wa), Poll::Ready(()));
    assert_eq!(poll_once(Pin::new(&mut c), &wc), Poll::Ready(()));

    // Notify stays usable afterwards.
    let mut d = notify.notified();
    let wd = CountingWaker::new();
    assert!(poll_once(Pin::new(&mut d), &wd).is_pending());
    notify.notify_one();
    assert_eq!(wd.count(), 1);
    assert_eq!(poll_once(Pin::new(&mut d), &wd), Poll::Ready(()));
}

// =============================================================================
// Item 5: Fairness (strict FIFO, no barging, Semaphore head-of-line)
// =============================================================================

#[test]
fn item05_mutex_strict_fifo_and_no_try_lock_barging() {
    const N: usize = 16;
    let m = AsyncMutex::new(0usize);
    let initial = m.try_lock().expect("initial lock");

    let mut waiters = Vec::with_capacity(N);
    for _ in 0..N {
        let w = CountingWaker::new();
        let mut fut = Box::pin(m.lock());
        assert!(poll_once(fut.as_mut(), &w).is_pending());
        waiters.push((fut, w));
    }

    assert!(m.try_lock().is_none());
    drop(initial);

    for i in 0..N {
        assert!(
            m.try_lock().is_none(),
            "try_lock must not barge while waiter {i} is queued/granted"
        );
        for (j, (_, w)) in waiters.iter().enumerate().skip(i) {
            if j == i {
                assert_eq!(w.count(), 1, "waiter {i} must be woken in FIFO order");
            } else {
                assert_eq!(w.count(), 0, "later waiter {j} must not be woken early");
            }
        }
        let (fut, w) = &mut waiters[i];
        let Poll::Ready(mut guard) = poll_once(fut.as_mut(), w) else {
            panic!("waiter {i} must acquire mutex");
        };
        assert_eq!(*guard, i);
        *guard += 1;
        drop(guard);
    }

    assert_eq!(*m.try_lock().expect("unlocked after all waiters"), N);
}

#[test]
fn item05_semaphore_head_of_line_blocks_small_requests() {
    let sem = Semaphore::new(2);
    let w_big = CountingWaker::new();
    let w_s1 = CountingWaker::new();
    let w_s2 = CountingWaker::new();

    // Big waiter asks for 4 when 2 exist -> takes 2 partial permits, blocks behind it.
    let mut big = pin!(sem.acquire_many(4));
    let mut s1 = pin!(sem.acquire());
    let mut s2 = pin!(sem.acquire());

    assert!(poll_once(big.as_mut(), &w_big).is_pending());
    assert_eq!(sem.available_permits(), 0);
    assert_eq!(sem.try_acquire().unwrap_err(), TryAcquireError::NoPermits);

    assert!(poll_once(s1.as_mut(), &w_s1).is_pending());
    assert!(poll_once(s2.as_mut(), &w_s2).is_pending());

    // Adding 1 permit goes to `big` (now 3/4), NOT to `s1` or `try_acquire`.
    sem.add_permits(1);
    assert_eq!(sem.available_permits(), 0);
    assert_eq!(w_big.count(), 0);
    assert_eq!(w_s1.count(), 0);
    assert_eq!(sem.try_acquire().unwrap_err(), TryAcquireError::NoPermits);

    // Adding 1 more completes `big` (4/4), still not `s1`.
    sem.add_permits(1);
    assert_eq!(w_big.count(), 1);
    assert_eq!(w_s1.count(), 0);
    let Poll::Ready(Ok(p_big)) = poll_once(big.as_mut(), &w_big) else {
        panic!("big waiter must complete once 4 permits reached");
    };
    assert_eq!(p_big.num_permits(), 4);
    assert!(poll_once(s1.as_mut(), &w_s1).is_pending());

    // Dropping p_big releases 4 permits: satisfies s1 (1), s2 (1), leaves 2 available.
    drop(p_big);
    assert_eq!(w_s1.count(), 1);
    assert_eq!(w_s2.count(), 1);
    assert_eq!(sem.available_permits(), 2);
    assert!(matches!(poll_once(s1.as_mut(), &w_s1), Poll::Ready(Ok(_))));
    assert!(matches!(poll_once(s2.as_mut(), &w_s2), Poll::Ready(Ok(_))));
}

#[test]
fn item05_channel_fifo_senders_and_receivers_no_barging() {
    // Senders FIFO
    {
        let (tx, rx) = channel::bounded::<usize>(1);
        tx.try_send(0).expect("fill");

        let mut senders = Vec::new();
        for i in 1..=8 {
            let w = CountingWaker::new();
            let mut fut = Box::pin(tx.send(i));
            assert!(poll_once(fut.as_mut(), &w).is_pending());
            senders.push((fut, w));
        }

        let mut received = Vec::new();
        for (idx, (fut, w)) in senders.iter_mut().enumerate() {
            received.push(rx.try_recv().expect("recv"));
            assert_eq!(w.count(), 1, "sender {} must wake in FIFO order", idx + 1);
            // Buffer is immediately refilled by granted sender -> try_send cannot barge.
            assert_eq!(tx.try_send(999), Err(channel::TrySendError::Full(999)));
            assert_eq!(poll_once(fut.as_mut(), w), Poll::Ready(Ok(())));
        }
        received.push(rx.try_recv().expect("final recv"));
        assert_eq!(received, (0..=8).collect::<Vec<_>>());
    }

    // Receivers FIFO
    {
        let (tx, rx) = channel::bounded::<usize>(4);
        let mut receivers = Vec::new();
        for _ in 0..8 {
            let w = CountingWaker::new();
            let mut fut = Box::pin(rx.recv());
            assert!(poll_once(fut.as_mut(), &w).is_pending());
            receivers.push((fut, w));
        }

        for (i, (fut, w)) in receivers.iter_mut().enumerate() {
            tx.try_send(i).expect("send");
            assert_eq!(w.count(), 1, "receiver {i} must wake in FIFO order");
            assert_eq!(
                rx.try_recv(),
                Err(channel::TryRecvError::Empty),
                "try_recv must not barge past claimed receiver {i}"
            );
            assert_eq!(poll_once(fut.as_mut(), w), Poll::Ready(Ok(i)));
        }
    }
}

// =============================================================================
// Item 6: Close semantics
// =============================================================================

#[test]
fn item06_channel_close_idempotent_returns_values_and_drains() {
    let (tx, rx) = channel::bounded::<u32>(1);
    let tx2 = tx.clone();
    let rx2 = rx.clone();

    assert!(tx.same_channel(&tx2));
    assert!(rx.same_channel(&rx2));
    assert_eq!(tx.sender_count(), 2);
    assert_eq!(tx.receiver_count(), 2);
    assert_eq!(rx.sender_count(), 2);
    assert_eq!(rx.receiver_count(), 2);

    tx.try_send(100).expect("buffer 100");
    let w1 = CountingWaker::new();
    let w2 = CountingWaker::new();
    let w_closed = CountingWaker::new();
    let mut s1 = pin!(tx.send(201));
    let mut s2 = pin!(tx2.send(202));
    let mut closed_fut = pin!(tx.closed());

    assert!(poll_once(s1.as_mut(), &w1).is_pending());
    assert!(poll_once(s2.as_mut(), &w2).is_pending());
    assert!(poll_once(closed_fut.as_mut(), &w_closed).is_pending());

    // First close returns true, subsequent closes return false.
    assert!(tx.close());
    assert!(!tx.close());
    assert!(!rx.close());
    assert!(tx.is_closed());
    assert!(rx.is_closed());

    // Queued senders get their exact own values back.
    assert_eq!(w1.count(), 1);
    assert_eq!(w2.count(), 1);
    assert_eq!(
        poll_once(s1.as_mut(), &w1),
        Poll::Ready(Err(channel::SendError(201)))
    );
    assert_eq!(
        poll_once(s2.as_mut(), &w2),
        Poll::Ready(Err(channel::SendError(202)))
    );

    // closed() completes.
    assert_eq!(w_closed.count(), 1);
    assert_eq!(poll_once(closed_fut.as_mut(), &w_closed), Poll::Ready(()));

    // New sends after close fail with value returned.
    assert_eq!(tx.try_send(300), Err(channel::TrySendError::Closed(300)));
    assert_eq!(
        poll_noop(pin!(tx.send(301))),
        Poll::Ready(Err(channel::SendError(301)))
    );
    assert_eq!(
        tx.try_reserve().unwrap_err(),
        channel::TrySendError::Closed(())
    );
    assert_eq!(
        poll_noop(pin!(tx.reserve())).map(|r| r.map(|_| ())),
        Poll::Ready(Err(channel::SendError(())))
    );

    // Receivers still drain buffered messages, then get RecvError / Closed.
    assert_eq!(rx.len(), 1);
    assert!(!rx.is_empty());
    assert_eq!(rx.try_recv(), Ok(100));
    assert!(rx.is_empty());
    assert_eq!(rx.try_recv(), Err(channel::TryRecvError::Closed));
    assert_eq!(
        poll_noop(pin!(rx.recv())),
        Poll::Ready(Err(channel::RecvError))
    );
}

#[test]
fn item06_channel_last_sender_drop_and_last_receiver_drop_no_deadlock() {
    // Last sender dropped: buffered items drained, then RecvError.
    {
        let (tx, rx) = channel::bounded::<u32>(2);
        let tx2 = tx.clone();
        tx.try_send(1).expect("send 1");
        tx2.try_send(2).expect("send 2");
        drop(tx);
        assert!(!rx.is_closed());
        assert_eq!(rx.sender_count(), 1);
        drop(tx2);
        assert!(rx.is_closed());
        assert_eq!(rx.sender_count(), 0);
        assert_eq!(rx.try_recv(), Ok(1));
        assert_eq!(poll_noop(pin!(rx.recv())), Poll::Ready(Ok(2)));
        assert_eq!(
            poll_noop(pin!(rx.recv())),
            Poll::Ready(Err(channel::RecvError))
        );
    }

    // Last receiver dropped: buffered values dropped exactly once, AFTER releasing
    // internal lock (so a message owning a Sender of the same channel does not deadlock).
    watchdog(|| {
        #[derive(Debug)]
        struct SelfReferentialMsg {
            _tx: channel::Sender<SelfReferentialMsg>,
            drops: Arc<AtomicUsize>,
        }
        impl Drop for SelfReferentialMsg {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel::bounded::<SelfReferentialMsg>(4);
        let w_closed = CountingWaker::new();
        let mut closed_fut = pin!(tx.closed());
        assert!(poll_once(closed_fut.as_mut(), &w_closed).is_pending());

        for _ in 0..3 {
            tx.try_send(SelfReferentialMsg {
                _tx: tx.clone(),
                drops: Arc::clone(&drops),
            })
            .expect("enqueue self-referential msg");
        }
        assert_eq!(tx.sender_count(), 4);
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        // Dropping the last receiver drops the 3 buffered messages outside the lock.
        drop(rx);
        assert_eq!(drops.load(Ordering::SeqCst), 3);
        assert!(tx.is_closed());
        assert_eq!(tx.sender_count(), 1);
        assert_eq!(tx.receiver_count(), 0);
        assert_eq!(w_closed.count(), 1);
        assert_eq!(poll_once(closed_fut.as_mut(), &w_closed), Poll::Ready(()));
    });
}

#[test]
fn item06_semaphore_close_and_partial_grant_return() {
    // Held permits remain valid across close; queued waiters fail with AcquireError.
    {
        let sem = Semaphore::new(2);
        let held = sem.try_acquire().expect("held permit");
        assert_eq!(sem.available_permits(), 1);
        assert_eq!(sem.forget_permits(1), 1);
        assert_eq!(sem.available_permits(), 0);
        sem.add_permits(1);
        assert_eq!(sem.available_permits(), 1);

        let w = CountingWaker::new();
        let mut waiter = pin!(sem.acquire_many(2));
        assert!(poll_once(waiter.as_mut(), &w).is_pending());

        assert!(!sem.is_closed());
        sem.close();
        sem.close(); // idempotent
        assert!(sem.is_closed());

        assert_eq!(w.count(), 1);
        assert!(matches!(
            poll_once(waiter.as_mut(), &w),
            Poll::Ready(Err(AcquireError { .. }))
        ));

        // Even with permits available, try_acquire* returns Closed.
        assert_eq!(sem.available_permits(), 1);
        assert_eq!(sem.try_acquire().unwrap_err(), TryAcquireError::Closed);
        assert_eq!(
            sem.try_acquire_many(0).unwrap_err(),
            TryAcquireError::Closed
        );

        // Held permit still drops cleanly and returns its permit.
        drop(held);
        assert_eq!(sem.available_permits(), 2);
    }

    // M2(b): close() while head waiter holds a partial grant returns permits immediately.
    {
        let sem = Semaphore::new(3);
        let w = CountingWaker::new();
        let mut head = pin!(sem.acquire_many(5));
        assert!(poll_once(head.as_mut(), &w).is_pending());
        assert_eq!(
            sem.available_permits(),
            0,
            "partial grant removes permits from available_permits"
        );

        sem.close();
        assert_eq!(
            sem.available_permits(),
            3,
            "close() must return partial grant permits immediately"
        );
        assert_eq!(w.count(), 1);
        assert!(matches!(
            poll_once(head.as_mut(), &w),
            Poll::Ready(Err(AcquireError { .. }))
        ));
        assert_eq!(sem.available_permits(), 3);
    }

    // Permit::forget permanently removes permits.
    {
        let sem = Semaphore::new(3);
        let p = sem.try_acquire_many(2).expect("take 2");
        assert_eq!(p.num_permits(), 2);
        p.forget();
        assert_eq!(sem.available_permits(), 1);
        assert_eq!(sem.forget_permits(5), 1);
        assert_eq!(sem.available_permits(), 0);
    }
}

#[test]
fn item06_oneshot_close_drop_and_value_before_close() {
    // Sender dropped without sending -> RecvError / TryRecvError::Closed.
    {
        let (tx, mut rx) = oneshot::channel::<u32>();
        let wr = CountingWaker::new();
        assert!(poll_once(Pin::new(&mut rx), &wr).is_pending());
        drop(tx);
        assert_eq!(wr.count(), 1);
        assert_eq!(rx.try_recv(), Err(oneshot::TryRecvError::Closed));
    }
    {
        let (tx, mut rx) = oneshot::channel::<u32>();
        let wr = CountingWaker::new();
        assert!(poll_once(Pin::new(&mut rx), &wr).is_pending());
        drop(tx);
        assert_eq!(wr.count(), 1);
        assert_eq!(
            poll_once(Pin::new(&mut rx), &wr),
            Poll::Ready(Err(oneshot::RecvError))
        );
    }

    // Receiver::close wakes Sender::closed() and fails subsequent send with value.
    {
        let (mut tx, mut rx) = oneshot::channel::<String>();
        let ws = CountingWaker::new();
        {
            let mut closed_fut = pin!(tx.closed());
            assert!(poll_once(closed_fut.as_mut(), &ws).is_pending());
            rx.close();
            assert_eq!(ws.count(), 1);
            assert_eq!(poll_once(closed_fut.as_mut(), &ws), Poll::Ready(()));
        }
        assert!(tx.is_closed());
        assert_eq!(
            tx.send("hello".into()),
            Err(oneshot::SendError("hello".into()))
        );
    }

    // Receiver dropped wakes Sender::closed() and fails send.
    {
        let (mut tx, rx) = oneshot::channel::<u32>();
        let ws = CountingWaker::new();
        {
            let mut closed_fut = pin!(tx.closed());
            assert!(poll_once(closed_fut.as_mut(), &ws).is_pending());
            drop(rx);
            assert_eq!(ws.count(), 1);
            assert_eq!(poll_once(closed_fut.as_mut(), &ws), Poll::Ready(()));
        }
        assert!(tx.is_closed());
        assert_eq!(tx.send(7), Err(oneshot::SendError(7)));
    }

    // Value sent BEFORE rx.close() is still received.
    {
        let (tx, mut rx) = oneshot::channel::<u32>();
        tx.send(99).expect("send before close");
        rx.close();
        assert_eq!(rx.try_recv(), Ok(99));
        assert_eq!(rx.try_recv(), Err(oneshot::TryRecvError::Closed));
    }
}

#[test]
#[should_panic(expected = "polled after completion")]
fn item06_oneshot_receiver_polled_after_completion_panics() {
    let (tx, mut rx) = oneshot::channel::<u32>();
    tx.send(1).expect("send");
    assert_eq!(poll_noop(Pin::new(&mut rx)), Poll::Ready(Ok(1)));
    let _ = poll_noop(Pin::new(&mut rx));
}

// =============================================================================
// Item 7: Capacity bounds (rendezvous, bounded(1), bounded(usize::MAX), Semaphore)
// =============================================================================

#[test]
fn item07_channel_bounded_zero_rendezvous_steps_1_to_7() {
    // Steps 1, 2, 5, 6: send waits until receiver takes; try_send / try_reserve return Full.
    {
        let (tx, rx) = channel::bounded::<u32>(0);
        assert_eq!(tx.capacity(), Some(0));
        assert_eq!(rx.capacity(), Some(0));
        assert_eq!(tx.len(), 0);
        assert!(tx.is_empty());

        let wr = CountingWaker::new();
        let mut recv_fut = pin!(rx.recv());
        assert!(poll_once(recv_fut.as_mut(), &wr).is_pending());

        // Step 5 & 6: even with a queued receiver, try_send and try_reserve return Full.
        assert_eq!(tx.try_send(1), Err(channel::TrySendError::Full(1)));
        assert_eq!(
            tx.try_reserve().unwrap_err(),
            channel::TrySendError::Full(())
        );

        // Step 1: send queues value, grants claim to receiver, stays Pending.
        let ws = CountingWaker::new();
        let mut send_fut = pin!(tx.send(42));
        assert!(poll_once(send_fut.as_mut(), &ws).is_pending());
        assert_eq!(wr.count(), 1);
        assert_eq!(ws.count(), 0);
        assert_eq!(tx.len(), 1);

        // Step 2: polling receiver takes value and wakes sender; send completes only now.
        assert_eq!(poll_once(recv_fut.as_mut(), &wr), Poll::Ready(Ok(42)));
        assert_eq!(ws.count(), 1);
        assert_eq!(tx.len(), 0);
        assert_eq!(poll_once(send_fut.as_mut(), &ws), Poll::Ready(Ok(())));
        assert_eq!(rx.try_recv(), Err(channel::TryRecvError::Empty));
    }

    // Step 3 (Q3(i)): claimed receiver dropped unpolled -> sender stays pending,
    // len() == 1, claim forwards to B (or lapses with no phantom item).
    {
        let (tx, rx) = channel::bounded::<u32>(0);
        let wa = CountingWaker::new();
        let wb = CountingWaker::new();
        let ws = CountingWaker::new();

        let mut recv_a = Box::pin(rx.recv());
        let mut recv_b = pin!(rx.recv());
        assert!(poll_once(recv_a.as_mut(), &wa).is_pending());
        assert!(poll_once(recv_b.as_mut(), &wb).is_pending());

        let mut send_fut = pin!(tx.send(77));
        assert!(poll_once(send_fut.as_mut(), &ws).is_pending());
        assert_eq!(wa.count(), 1);
        assert_eq!(wb.count(), 0);
        assert_eq!(tx.len(), 1);

        // Drop A unpolled -> B is woken, sender STILL pending, len() STILL 1.
        drop(recv_a);
        assert_eq!(wb.count(), 1);
        assert_eq!(ws.count(), 0);
        assert_eq!(tx.len(), 1);
        assert!(poll_once(send_fut.as_mut(), &ws).is_pending());

        // B receives 77, completing sender.
        assert_eq!(poll_once(recv_b.as_mut(), &wb), Poll::Ready(Ok(77)));
        assert_eq!(ws.count(), 1);
        assert_eq!(poll_once(send_fut.as_mut(), &ws), Poll::Ready(Ok(())));
        assert_eq!(tx.len(), 0);
        assert_eq!(rx.try_recv(), Err(channel::TryRecvError::Empty));
    }

    // Step 3 (claim lapses with no B): sender still pending, next receiver takes value.
    {
        let (tx, rx) = channel::bounded::<u32>(0);
        let wa = CountingWaker::new();
        let ws = CountingWaker::new();

        let mut recv_a = Box::pin(rx.recv());
        assert!(poll_once(recv_a.as_mut(), &wa).is_pending());

        let mut send_fut = pin!(tx.send(88));
        assert!(poll_once(send_fut.as_mut(), &ws).is_pending());
        assert_eq!(wa.count(), 1);
        assert_eq!(tx.len(), 1);

        drop(recv_a);
        assert_eq!(ws.count(), 0);
        assert!(poll_once(send_fut.as_mut(), &ws).is_pending());
        assert_eq!(tx.len(), 1);

        let mut recv_c = pin!(rx.recv());
        assert_eq!(poll_noop(recv_c.as_mut()), Poll::Ready(Ok(88)));
        assert_eq!(ws.count(), 1);
        assert_eq!(poll_once(send_fut.as_mut(), &ws), Poll::Ready(Ok(())));
        assert_eq!(rx.try_recv(), Err(channel::TryRecvError::Empty));
    }

    // Step 4: queued send cancelled after claim was granted -> value dropped,
    // claimed receiver's claim is revoked (spurious wake, re-waits).
    {
        let drops = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel::bounded::<DropCounter>(0);
        let wr = CountingWaker::new();
        let ws = CountingWaker::new();

        let mut recv_fut = pin!(rx.recv());
        assert!(poll_once(recv_fut.as_mut(), &wr).is_pending());

        let mut send_fut = Box::pin(tx.send(DropCounter::new(1, &drops)));
        assert!(poll_once(send_fut.as_mut(), &ws).is_pending());
        assert_eq!(wr.count(), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        // Cancel send before receiver polls -> value dropped, claim revoked.
        drop(send_fut);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(tx.len(), 0);

        // Receiver sees spurious wake and re-waits.
        wr.reset();
        assert!(poll_once(recv_fut.as_mut(), &wr).is_pending());

        // Subsequent send delivers to the same waiting receiver.
        let ws2 = CountingWaker::new();
        let mut send_fut2 = pin!(tx.send(DropCounter::new(2, &drops)));
        assert!(poll_once(send_fut2.as_mut(), &ws2).is_pending());
        assert_eq!(wr.count(), 1);
        let Poll::Ready(Ok(item)) = poll_once(recv_fut.as_mut(), &wr) else {
            panic!("receiver must receive second send after first was cancelled");
        };
        assert_eq!(item.id, 2);
        assert!(matches!(
            poll_once(send_fut2.as_mut(), &ws2),
            Poll::Ready(Ok(()))
        ));
    }

    // Step 7: close() on bounded(0) returns queued senders' values and does not
    // deliver un-taken queued sends to receivers.
    {
        let (tx, rx) = channel::bounded::<u32>(0);
        let ws = CountingWaker::new();
        let mut send_fut = pin!(tx.send(555));
        assert!(poll_once(send_fut.as_mut(), &ws).is_pending());

        assert!(tx.close());
        assert_eq!(ws.count(), 1);
        assert_eq!(
            poll_once(send_fut.as_mut(), &ws),
            Poll::Ready(Err(channel::SendError(555)))
        );
        assert_eq!(rx.try_recv(), Err(channel::TryRecvError::Closed));
        assert_eq!(
            poll_noop(pin!(rx.recv())),
            Poll::Ready(Err(channel::RecvError))
        );
        assert_eq!(tx.try_send(1), Err(channel::TrySendError::Closed(1)));
        assert_eq!(
            tx.try_reserve().unwrap_err(),
            channel::TrySendError::Closed(())
        );
    }
}

#[test]
#[should_panic(expected = "rendezvous")]
fn item07_channel_bounded_zero_reserve_panics() {
    let (tx, _rx) = channel::bounded::<u32>(0);
    let mut fut = pin!(tx.reserve());
    let _ = poll_noop(fut.as_mut());
}

#[test]
#[should_panic(expected = "rendezvous")]
fn item07_channel_bounded_zero_poll_reserve_panics() {
    let (mut tx, _rx) = channel::bounded::<u32>(0);
    let mut cx = Context::from_waker(Waker::noop());
    let _ = tx.poll_reserve(&mut cx);
}

#[test]
fn item07_channel_bounded_one_usize_max_and_unbounded() {
    // bounded(1): second try_send is Full.
    {
        let (tx, rx) = channel::bounded::<u32>(1);
        assert_eq!(tx.capacity(), Some(1));
        assert_eq!(rx.capacity(), Some(1));
        tx.try_send(1).expect("first fits");
        assert_eq!(tx.try_send(2), Err(channel::TrySendError::Full(2)));
        assert_eq!(rx.try_recv(), Ok(1));
        tx.try_send(2).expect("second fits after recv");
        assert_eq!(rx.try_recv(), Ok(2));
    }

    // bounded(usize::MAX): no preallocation abort, thousands of sends work.
    {
        let (tx, rx) = channel::bounded::<usize>(usize::MAX);
        assert_eq!(tx.capacity(), Some(usize::MAX));
        assert_eq!(rx.capacity(), Some(usize::MAX));
        for i in 0..4_096 {
            tx.try_send(i).expect("send into usize::MAX bounded");
        }
        assert_eq!(tx.len(), 4_096);
        for i in 0..4_096 {
            assert_eq!(rx.try_recv(), Ok(i));
        }
        assert!(rx.is_empty());
    }

    // unbounded(): capacity is None, never blocks.
    {
        let (tx, rx) = channel::unbounded::<usize>();
        assert_eq!(tx.capacity(), None);
        assert_eq!(rx.capacity(), None);
        for i in 0..4_096 {
            tx.try_send(i).expect("unbounded try_send");
        }
        assert_eq!(tx.len(), 4_096);
        for i in 0..4_096 {
            assert_eq!(rx.try_recv(), Ok(i));
        }
        assert!(rx.is_empty());
    }
}

#[test]
fn item07_semaphore_bounds_and_zero_acquire() {
    let s0 = Semaphore::new(0);
    assert_eq!(s0.available_permits(), 0);
    assert_eq!(s0.try_acquire().unwrap_err(), TryAcquireError::NoPermits);

    // acquire_many(0) and try_acquire_many(0) complete immediately even at 0 permits.
    let p0 = s0.try_acquire_many(0).expect("try_acquire_many(0)");
    assert_eq!(p0.num_permits(), 0);
    drop(p0);
    let Poll::Ready(Ok(p0_async)) = poll_noop(pin!(s0.acquire_many(0))) else {
        panic!("acquire_many(0) must complete immediately");
    };
    assert_eq!(p0_async.num_permits(), 0);
    drop(p0_async);

    // Closed semaphore rejects acquire_many(0).
    s0.close();
    assert!(matches!(
        poll_noop(pin!(s0.acquire_many(0))),
        Poll::Ready(Err(AcquireError { .. }))
    ));

    // MAX_PERMITS is valid.
    let smax = Semaphore::new(Semaphore::MAX_PERMITS);
    assert_eq!(smax.available_permits(), Semaphore::MAX_PERMITS);
}

#[test]
#[should_panic(expected = "MAX_PERMITS")]
fn item07_semaphore_new_over_max_panics() {
    let _ = Semaphore::new(Semaphore::MAX_PERMITS + 1);
}

#[test]
#[should_panic(expected = "MAX_PERMITS")]
fn item07_semaphore_add_permits_overflow_panics() {
    let sem = Semaphore::new(Semaphore::MAX_PERMITS);
    sem.add_permits(1);
}

#[test]
#[should_panic(expected = "MAX_PERMITS")]
fn item07_semaphore_acquire_many_over_max_panics() {
    let sem = Semaphore::new(1);
    let mut fut = pin!(sem.acquire_many(Semaphore::MAX_PERMITS + 1));
    let _ = poll_noop(fut.as_mut());
}

#[test]
fn item07_semaphore_unsatisfiable_head_stalls_until_close_or_add() {
    // M2(a) resolved via close().
    {
        let sem = Semaphore::new(4);
        let wh = CountingWaker::new();
        let wt = CountingWaker::new();

        let mut head = pin!(sem.acquire_many(5));
        let mut tail = pin!(sem.acquire());

        assert!(poll_once(head.as_mut(), &wh).is_pending());
        assert!(poll_once(tail.as_mut(), &wt).is_pending());
        assert_eq!(sem.available_permits(), 0);
        assert_eq!(sem.try_acquire().unwrap_err(), TryAcquireError::NoPermits);

        sem.close();
        assert_eq!(wh.count(), 1);
        assert_eq!(wt.count(), 1);
        assert_eq!(sem.available_permits(), 4);
        assert!(matches!(
            poll_once(head.as_mut(), &wh),
            Poll::Ready(Err(AcquireError { .. }))
        ));
        assert!(matches!(
            poll_once(tail.as_mut(), &wt),
            Poll::Ready(Err(AcquireError { .. }))
        ));
    }

    // M2(a) resolved via add_permits(1).
    {
        let sem = Semaphore::new(4);
        let wh = CountingWaker::new();
        let wt = CountingWaker::new();

        let mut head = pin!(sem.acquire_many(5));
        let mut tail = pin!(sem.acquire());

        assert!(poll_once(head.as_mut(), &wh).is_pending());
        assert!(poll_once(tail.as_mut(), &wt).is_pending());

        sem.add_permits(1);
        assert_eq!(wh.count(), 1);
        assert_eq!(wt.count(), 0);
        let Poll::Ready(Ok(p_head)) = poll_once(head.as_mut(), &wh) else {
            panic!("head must complete after add_permits(1)");
        };
        assert_eq!(p_head.num_permits(), 5);
        assert!(poll_once(tail.as_mut(), &wt).is_pending());

        drop(p_head);
        assert_eq!(wt.count(), 1);
        let Poll::Ready(Ok(p_tail)) = poll_once(tail.as_mut(), &wt) else {
            panic!("tail must complete after head drops");
        };
        assert_eq!(p_tail.num_permits(), 1);
        assert_eq!(sem.available_permits(), 4);
    }
}

// =============================================================================
// Item 8: Cooperative budget
// =============================================================================

#[test]
fn item08_budget_unbounded_recv_yields_to_sibling() {
    watchdog(|| {
        const BOUND: usize = 4_096;
        let (tx, rx) = channel::unbounded::<usize>();
        for i in 0..=BOUND {
            tx.try_send(i).expect("prefill unbounded");
        }

        let sibling_ran = Arc::new(AtomicBool::new(false));
        let sibling_flag = Arc::clone(&sibling_ran);

        let ops = task::block_on(async move {
            let sibling = task::spawn(async move {
                sibling_flag.store(true, Ordering::SeqCst);
            });

            let mut count = 0usize;
            while count < BOUND {
                let _ = rx.recv().await.expect("always-ready recv");
                count += 1;
                if sibling_ran.load(Ordering::SeqCst) {
                    break;
                }
            }
            sibling.await.expect("sibling task completes");
            count
        });

        assert!(
            ops < BOUND,
            "unbounded recv loop did not yield to sibling within {BOUND} ops (ran {ops})"
        );
    });
}

#[test]
fn item08_budget_uncontended_mutex_yields_to_sibling() {
    watchdog(|| {
        const BOUND: usize = 4_096;
        let m = AsyncMutex::new(0usize);
        let sibling_ran = Arc::new(AtomicBool::new(false));
        let sibling_flag = Arc::clone(&sibling_ran);

        let ops = task::block_on(async move {
            let sibling = task::spawn(async move {
                sibling_flag.store(true, Ordering::SeqCst);
            });

            let mut count = 0usize;
            while count < BOUND {
                let mut g = m.lock().await;
                *g += 1;
                drop(g);
                count += 1;
                if sibling_ran.load(Ordering::SeqCst) {
                    break;
                }
            }
            sibling.await.expect("sibling task completes");
            count
        });

        assert!(
            ops < BOUND,
            "uncontended mutex loop did not yield to sibling within {BOUND} ops (ran {ops})"
        );
    });
}

#[test]
fn item08_budget_try_methods_never_touch_budget() {
    watchdog(|| {
        let m = AsyncMutex::new(0usize);
        let sem = Semaphore::new(1);
        let (tx, rx) = channel::bounded::<usize>(1);
        let sibling_ran = Arc::new(AtomicBool::new(false));
        let sibling_flag = Arc::clone(&sibling_ran);

        task::block_on(async move {
            let sibling = task::spawn(async move {
                sibling_flag.store(true, Ordering::SeqCst);
            });

            // Exceed the 128-unit budget many times using only try_* methods.
            for i in 0..512 {
                let _g = m.try_lock().expect("try_lock");
                let _p = sem.try_acquire().expect("try_acquire");
                tx.try_send(i).expect("try_send");
                assert_eq!(rx.try_recv(), Ok(i));
            }

            // If try_* did not spend budget, the very next ready async calls
            // must complete in this same poll without yielding to `sibling`.
            let _g = m.lock().await;
            let _p = sem.acquire().await.expect("acquire");
            tx.send(999).await.expect("send");
            assert_eq!(rx.recv().await, Ok(999));

            assert!(
                !sibling_ran.load(Ordering::SeqCst),
                "try_* methods must never spend the cooperative budget"
            );

            // Yielding explicitly lets sibling run.
            task::yield_now().await;
            sibling.await.expect("sibling completes");
            assert!(sibling_ran.load(Ordering::SeqCst));
        });
    });
}

// =============================================================================
// Item 9: Blocking variants
// =============================================================================

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn item09_blocking_variants_from_threads_against_async_tasks() {
    watchdog(|| {
        // AsyncMutex::blocking_lock & blocking_lock_owned
        {
            let m = Arc::new(AsyncMutex::new(0u32));
            let m_bg = Arc::clone(&m);
            let m_owned = Arc::clone(&m);
            let g_init = Arc::clone(&m).try_lock_owned().expect("initial lock");

            let t1 = thread::spawn(move || {
                let mut g = m_bg.blocking_lock();
                *g += 10;
            });
            let t2 = thread::spawn(move || {
                let mut g = m_owned.blocking_lock_owned();
                *g += 20;
            });

            task::block_on(async {
                thread::sleep(Duration::from_millis(5));
                drop(g_init);
            });
            t1.join().expect("t1");
            t2.join().expect("t2");
            assert_eq!(*m.blocking_lock(), 30);
        }

        // Semaphore::blocking_acquire & blocking_acquire_many
        {
            let sem = Arc::new(Semaphore::new(0));
            let s1 = Arc::clone(&sem);
            let s2 = Arc::clone(&sem);

            let t1 = thread::spawn(move || {
                let p = s1.blocking_acquire().expect("blocking_acquire");
                assert_eq!(p.num_permits(), 1);
            });
            let t2 = thread::spawn(move || {
                let p = s2.blocking_acquire_many(2).expect("blocking_acquire_many");
                assert_eq!(p.num_permits(), 2);
            });

            task::block_on(async {
                thread::sleep(Duration::from_millis(5));
                sem.add_permits(3);
            });
            t1.join().expect("t1");
            t2.join().expect("t2");
            assert_eq!(sem.available_permits(), 3);
        }

        // oneshot::Receiver::blocking_recv
        {
            let (tx, rx) = oneshot::channel::<&'static str>();
            let t = thread::spawn(move || rx.blocking_recv().expect("blocking_recv"));
            task::block_on(async {
                thread::sleep(Duration::from_millis(5));
                tx.send("ok").expect("send");
            });
            assert_eq!(t.join().expect("t"), "ok");
        }

        // channel::Sender::blocking_send & Receiver::blocking_recv (bounded(1) & bounded(0))
        for cap in [0usize, 1] {
            let (tx, rx) = channel::bounded::<usize>(cap);
            let (tx_back, rx_back) = channel::bounded::<usize>(cap);

            let producer = thread::spawn(move || {
                for i in 0..32 {
                    tx.blocking_send(i).expect("blocking_send");
                    let echoed = rx_back.blocking_recv().expect("blocking_recv");
                    assert_eq!(echoed, i * 2);
                }
            });

            task::block_on(async {
                for i in 0..32 {
                    let v = rx.recv().await.expect("async recv");
                    assert_eq!(v, i);
                    tx_back.send(v * 2).await.expect("async send");
                }
            });
            producer.join().expect("producer");
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[should_panic(expected = "blocking")]
fn item09_blocking_lock_inside_runtime_panics() {
    let m = AsyncMutex::new(1u32);
    task::block_on(async {
        let _g = m.blocking_lock();
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[should_panic(expected = "blocking")]
fn item09_blocking_lock_owned_inside_runtime_panics() {
    let m = Arc::new(AsyncMutex::new(1u32));
    task::block_on(async {
        let _g = Arc::clone(&m).blocking_lock_owned();
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[should_panic(expected = "blocking")]
fn item09_blocking_acquire_inside_runtime_panics() {
    let sem = Semaphore::new(1);
    task::block_on(async {
        let _ = sem.blocking_acquire();
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[should_panic(expected = "blocking")]
fn item09_blocking_acquire_many_inside_runtime_panics() {
    let sem = Semaphore::new(2);
    task::block_on(async {
        let _ = sem.blocking_acquire_many(2);
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[should_panic(expected = "blocking")]
fn item09_oneshot_blocking_recv_inside_runtime_panics() {
    let (tx, rx) = oneshot::channel::<u32>();
    tx.send(1).expect("send");
    task::block_on(async {
        let _ = rx.blocking_recv();
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[should_panic(expected = "blocking")]
fn item09_channel_blocking_send_inside_runtime_panics() {
    let (tx, _rx) = channel::bounded::<u32>(1);
    task::block_on(async {
        let _ = tx.blocking_send(1);
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[should_panic(expected = "blocking")]
fn item09_channel_blocking_recv_inside_runtime_panics() {
    let (tx, rx) = channel::bounded::<u32>(1);
    tx.try_send(1).expect("send");
    task::block_on(async {
        let _ = rx.blocking_recv();
    });
}

// =============================================================================
// Item 10: Task integration
// =============================================================================

#[test]
fn item10_aborted_task_holding_mutex_guard_releases_lock() {
    watchdog(|| {
        task::block_on(async {
            let m = Arc::new(AsyncMutex::new(0u32));
            let (locked_tx, locked_rx) = oneshot::channel::<()>();
            let m2 = Arc::clone(&m);

            let handle = task::spawn(async move {
                let mut guard = m2.lock_owned().await;
                *guard = 7;
                let _ = locked_tx.send(());
                // Pend forever while holding the guard.
                std::future::pending::<()>().await;
                drop(guard);
            });

            locked_rx.await.expect("task acquired lock");
            assert!(m.try_lock().is_none());

            handle.abort();
            let err = handle.await.unwrap_err();
            assert!(err.is_cancelled());

            let guard = m.try_lock().expect("lock freed after task abort");
            assert_eq!(*guard, 7);
        });
    });
}

#[test]
fn item10_panicking_task_holding_mutex_guard_releases_without_poisoning() {
    watchdog(|| {
        task::block_on(async {
            let m = Arc::new(AsyncMutex::new(1u32));
            let m2 = Arc::clone(&m);

            let handle = task::spawn(async move {
                let mut guard = m2.lock_owned().await;
                *guard = 99;
                panic!("boom while holding AsyncMutexOwnedGuard");
            });

            let err = handle.await.unwrap_err();
            assert!(err.is_panic());

            let guard = m.try_lock().expect("no poisoning after task panic");
            assert_eq!(*guard, 99);
        });
    });
}

#[test]
fn item10_runtime_drop_while_tasks_wait_leaves_mutex_usable() {
    watchdog(|| {
        let m = Arc::new(AsyncMutex::new(5u32));
        let root_guard = Arc::clone(&m).try_lock_owned().expect("root holds lock");

        let rt1 = Runtime::new().expect("create rt1");
        rt1.block_on(async {
            for _ in 0..8 {
                let m2 = Arc::clone(&m);
                drop(task::spawn(async move {
                    let _g = m2.lock_owned().await;
                }));
            }
            // Yield so spawned tasks poll once and queue on `m`.
            task::yield_now().await;
        });

        // Drop runtime while 8 tasks wait on `m`, then release root's guard.
        drop(rt1);
        drop(root_guard);

        // Fresh runtime must lock `m` cleanly.
        let rt2 = Runtime::new().expect("create rt2");
        let val = rt2.block_on(async {
            let mut g = m.lock().await;
            *g += 1;
            *g
        });
        assert_eq!(val, 6);
    });
}

// =============================================================================
// Item 11: Multi-thread stress (8 threads x 10,000 ops)
// =============================================================================

#[test]
fn item11_stress_semaphore_conservation_8x10000() {
    watchdog(|| {
        const THREADS: usize = 8;
        const OPS: usize = 10_000;
        const INITIAL: usize = 4;

        let sem = Arc::new(Semaphore::new(INITIAL));
        let mut handles = Vec::with_capacity(THREADS);
        for t in 0..THREADS {
            let sem = Arc::clone(&sem);
            handles.push(thread::spawn(move || {
                task::block_on(async move {
                    for i in 0..OPS {
                        let need = if (t + i) % 5 == 0 { 2 } else { 1 };
                        let permit = sem.acquire_many(need).await.expect("acquire");
                        assert_eq!(permit.num_permits(), need);
                        drop(permit);
                    }
                });
            }));
        }

        for h in handles {
            h.join().expect("worker thread");
        }
        assert_eq!(
            sem.available_permits(),
            INITIAL,
            "permits must be conserved at quiescence"
        );
    });
}

#[test]
fn item11_stress_mutex_counter_8x10000() {
    watchdog(|| {
        const THREADS: usize = 8;
        const OPS: usize = 10_000;

        let m = Arc::new(AsyncMutex::new(0usize));
        let mut handles = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            let m = Arc::clone(&m);
            handles.push(thread::spawn(move || {
                task::block_on(async move {
                    for _ in 0..OPS {
                        let mut g = m.lock().await;
                        *g += 1;
                    }
                });
            }));
        }

        for h in handles {
            h.join().expect("worker thread");
        }
        assert_eq!(*m.try_lock().expect("unlocked"), THREADS * OPS);
    });
}

#[test]
fn item11_stress_channel_mpmc_8x10000() {
    watchdog(|| {
        const PRODUCERS: usize = 4;
        const CONSUMERS: usize = 4;
        const OPS_PER_PRODUCER: usize = 10_000;
        const TOTAL: usize = PRODUCERS * OPS_PER_PRODUCER;

        let (tx, rx) = channel::bounded::<usize>(64);
        let mut prod_handles = Vec::with_capacity(PRODUCERS);
        for p in 0..PRODUCERS {
            let tx = tx.clone();
            prod_handles.push(thread::spawn(move || {
                task::block_on(async move {
                    let base = p * OPS_PER_PRODUCER;
                    for i in 0..OPS_PER_PRODUCER {
                        tx.send(base + i).await.expect("producer send");
                    }
                });
            }));
        }
        drop(tx);

        let mut cons_handles = Vec::with_capacity(CONSUMERS);
        for _ in 0..CONSUMERS {
            let rx = rx.clone();
            cons_handles.push(thread::spawn(move || {
                task::block_on(async move {
                    let mut got = Vec::new();
                    while let Ok(v) = rx.recv().await {
                        got.push(v);
                    }
                    got
                })
            }));
        }
        drop(rx);

        for h in prod_handles {
            h.join().expect("producer join");
        }

        let mut seen = vec![false; TOTAL];
        let mut count = 0usize;
        let mut sum = 0u64;
        for h in cons_handles {
            for v in h.join().expect("consumer join") {
                assert!(v < TOTAL, "out-of-range message {v}");
                assert!(!seen[v], "duplicate message {v}");
                seen[v] = true;
                count += 1;
                sum += v as u64;
            }
        }
        assert_eq!(count, TOTAL);
        let expected_sum = (TOTAL as u64) * ((TOTAL as u64) - 1) / 2;
        assert_eq!(sum, expected_sum);
    });
}

// =============================================================================
// Item 12: Types (std::sync re-exports, Send/Sync/Unpin, futures Send, const fn)
// =============================================================================

#[test]
fn item12_std_sync_reexports_type_identity() {
    fn same_type<A, B>()
    where
        A: Into<B>,
        B: Into<A>,
    {
    }

    // Type identity checks against std::sync.
    same_type::<sync::Arc<u8>, std::sync::Arc<u8>>();
    same_type::<sync::Weak<u8>, std::sync::Weak<u8>>();
    same_type::<sync::Mutex<u8>, std::sync::Mutex<u8>>();
    same_type::<sync::RwLock<u8>, std::sync::RwLock<u8>>();
    same_type::<sync::OnceLock<u8>, std::sync::OnceLock<u8>>();
    same_type::<sync::LazyLock<u8>, std::sync::LazyLock<u8>>();
    same_type::<sync::Barrier, std::sync::Barrier>();
    same_type::<sync::Condvar, std::sync::Condvar>();
    same_type::<sync::Once, std::sync::Once>();
    same_type::<sync::PoisonError<u8>, std::sync::PoisonError<u8>>();
    same_type::<sync::TryLockError<u8>, std::sync::TryLockError<u8>>();
    same_type::<sync::WaitTimeoutResult, std::sync::WaitTimeoutResult>();
    same_type::<sync::atomic::AtomicUsize, std::sync::atomic::AtomicUsize>();
    same_type::<sync::mpsc::Sender<u8>, std::sync::mpsc::Sender<u8>>();
    same_type::<sync::mpsc::Receiver<u8>, std::sync::mpsc::Receiver<u8>>();

    // Guard, result, and state types.
    let m: sync::Mutex<u8> = std::sync::Mutex::new(1);
    let _g: sync::MutexGuard<'_, u8> = m.lock().unwrap();
    let _lr: sync::LockResult<u8> = Ok(1);
    let _tlr: sync::TryLockResult<u8> = Ok(1);
    let rw: sync::RwLock<u8> = std::sync::RwLock::new(1);
    let _rg: sync::RwLockReadGuard<'_, u8> = rw.read().unwrap();
    drop(_rg);
    let _wg: sync::RwLockWriteGuard<'_, u8> = rw.write().unwrap();
    drop(_wg);
    let b = sync::Barrier::new(1);
    let _br: sync::BarrierWaitResult = b.wait();
    let once = sync::Once::new();
    once.call_once_force(|state: &sync::OnceState| {
        assert!(!state.is_poisoned());
    });

    #[allow(deprecated)]
    let _once_init: sync::Once = sync::ONCE_INIT;
}

#[test]
fn item12_send_sync_unpin_and_future_send_bounds() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    fn assert_unpin<T: Unpin>() {}
    fn assert_error<T: Error + Send + Sync + 'static>() {}

    // Autoref-specialization negative trait probes (stable Rust, no compile-fail harness).
    struct Probe<T>(PhantomData<T>);
    trait FallbackSend {
        fn is_send(&self) -> bool {
            false
        }
    }
    impl<T> FallbackSend for Probe<T> {}
    impl<T: Send> Probe<T> {
        fn is_send(&self) -> bool {
            true
        }
    }

    trait FallbackSync {
        fn is_sync(&self) -> bool {
            false
        }
    }
    impl<T> FallbackSync for Probe<T> {}
    impl<T: Sync> Probe<T> {
        fn is_sync(&self) -> bool {
            true
        }
    }

    // AsyncMutex & guards
    assert_send::<AsyncMutex<u32>>();
    assert_sync::<AsyncMutex<u32>>();
    assert_send::<AsyncMutex<Cell<u8>>>();
    assert_sync::<AsyncMutex<Cell<u8>>>();
    assert!(Probe::<AsyncMutex<Cell<u8>>>(PhantomData).is_send());
    assert!(Probe::<AsyncMutex<Cell<u8>>>(PhantomData).is_sync());
    assert!(!Probe::<AsyncMutex<Rc<u8>>>(PhantomData).is_send());
    assert!(!Probe::<AsyncMutex<Rc<u8>>>(PhantomData).is_sync());

    assert_send::<AsyncMutexGuard<'_, u32>>();
    assert_sync::<AsyncMutexGuard<'_, u32>>();
    assert_send::<AsyncMutexGuard<'_, Cell<u8>>>();
    assert!(!Probe::<AsyncMutexGuard<'_, Cell<u8>>>(PhantomData).is_sync());
    assert!(!Probe::<AsyncMutexGuard<'_, Rc<u8>>>(PhantomData).is_send());

    assert_send::<AsyncMutexOwnedGuard<u32>>();
    assert_sync::<AsyncMutexOwnedGuard<u32>>();
    assert_send::<AsyncMutexOwnedGuard<Cell<u8>>>();
    assert!(!Probe::<AsyncMutexOwnedGuard<Cell<u8>>>(PhantomData).is_sync());

    // Semaphore
    assert_send::<Semaphore>();
    assert_sync::<Semaphore>();
    assert_send::<SemaphorePermit<'_>>();
    assert_sync::<SemaphorePermit<'_>>();
    assert_send::<SemaphoreOwnedPermit>();
    assert_sync::<SemaphoreOwnedPermit>();
    assert_error::<AcquireError>();
    assert_error::<TryAcquireError>();

    // Notify & Notified
    assert_send::<Notify>();
    assert_sync::<Notify>();
    assert_send::<Notified<'_>>();
    assert_sync::<Notified<'_>>();
    assert_unpin::<Notified<'_>>();

    // oneshot
    assert_send::<oneshot::Sender<u32>>();
    assert_sync::<oneshot::Sender<u32>>();
    assert_send::<oneshot::Sender<Cell<u8>>>();
    assert_sync::<oneshot::Sender<Cell<u8>>>();
    assert!(!Probe::<oneshot::Sender<Rc<u8>>>(PhantomData).is_send());
    assert_send::<oneshot::Receiver<u32>>();
    assert_sync::<oneshot::Receiver<u32>>();
    assert_send::<oneshot::Receiver<Cell<u8>>>();
    assert_sync::<oneshot::Receiver<Cell<u8>>>();
    assert_unpin::<oneshot::Receiver<PhantomPinned>>();
    assert_error::<oneshot::SendError<u32>>();
    assert_error::<oneshot::RecvError>();
    assert_error::<oneshot::TryRecvError>();

    // channel
    assert_send::<channel::Sender<u32>>();
    assert_sync::<channel::Sender<u32>>();
    assert_send::<channel::Sender<Cell<u8>>>();
    assert_sync::<channel::Sender<Cell<u8>>>();
    assert!(!Probe::<channel::Sender<Rc<u8>>>(PhantomData).is_send());
    assert_send::<channel::Receiver<u32>>();
    assert_sync::<channel::Receiver<u32>>();
    assert_send::<channel::Receiver<Cell<u8>>>();
    assert_sync::<channel::Receiver<Cell<u8>>>();
    assert_send::<channel::Permit<'_, u32>>();
    assert_sync::<channel::Permit<'_, u32>>();
    assert_send::<channel::Permit<'_, Cell<u8>>>();
    assert_sync::<channel::Permit<'_, Cell<u8>>>();
    assert_error::<channel::SendError<u32>>();
    assert_error::<channel::TrySendError<u32>>();
    assert_error::<channel::RecvError>();
    assert_error::<channel::TryRecvError>();

    // Clone on Sender/Receiver must not require T: Clone; Debug must not require T: Debug.
    struct Plain(u8);
    fn assert_clone<T: Clone>() {}
    fn assert_debug<T: std::fmt::Debug>() {}
    assert_clone::<channel::Sender<Plain>>();
    assert_clone::<channel::Receiver<Plain>>();
    assert_debug::<channel::Sender<Plain>>();
    assert_debug::<channel::Receiver<Plain>>();
    assert_debug::<channel::Permit<'_, Plain>>();
    assert_debug::<oneshot::Sender<Plain>>();
    assert_debug::<oneshot::Receiver<Plain>>();

    // Returned futures must be Send whenever T: Send.
    fn assert_future_send<F: Future + Send>(_: F) {}

    struct FutureCheck<'a, T> {
        m: &'a AsyncMutex<T>,
        m_arc: Arc<AsyncMutex<T>>,
        sem: &'a Semaphore,
        sem_arc: Arc<Semaphore>,
        notify: &'a Notify,
        otx: &'a mut oneshot::Sender<T>,
        orx: oneshot::Receiver<T>,
        ctx: &'a channel::Sender<T>,
        crx: &'a channel::Receiver<T>,
        val: T,
    }

    fn check_futures_send<T: Send + 'static>(h: FutureCheck<'_, T>) {
        assert_future_send(h.m.lock());
        assert_future_send(h.m_arc.lock_owned());
        assert_future_send(h.sem.acquire());
        assert_future_send(h.sem.acquire_many(2));
        assert_future_send(Arc::clone(&h.sem_arc).acquire_owned());
        assert_future_send(h.sem_arc.acquire_many_owned(2));
        assert_future_send(h.notify.notified());
        assert_future_send(h.otx.closed());
        assert_future_send(h.orx);
        assert_future_send(h.ctx.send(h.val));
        assert_future_send(h.ctx.reserve());
        assert_future_send(h.ctx.closed());
        assert_future_send(h.crx.recv());
    }

    // Instantiate `check_futures_send` for both `u32` and `Cell<u8>` (Send + !Sync).
    let _fn_u32: fn(FutureCheck<'_, u32>) = check_futures_send::<u32>;
    let _fn_cell: fn(FutureCheck<'_, Cell<u8>>) = check_futures_send::<Cell<u8>>;
    let _ = (_fn_u32, _fn_cell, Plain(0).0);
}

#[test]
fn item12_const_fn_static_init_and_use() {
    static STATIC_MUTEX: AsyncMutex<u32> = AsyncMutex::new(7);
    static STATIC_SEM: Semaphore = Semaphore::new(2);
    static STATIC_NOTIFY: Notify = Notify::new();

    watchdog(|| {
        task::block_on(async {
            let mut g = STATIC_MUTEX.lock().await;
            assert_eq!(*g, 7);
            *g = 8;
            drop(g);

            let p = STATIC_SEM.acquire().await.expect("static sem acquire");
            assert_eq!(p.num_permits(), 1);
            drop(p);

            STATIC_NOTIFY.notify_one();
            STATIC_NOTIFY.notified().await;
        });
    });
}

// =============================================================================
// Item 13: Guard leaks and AsyncMutex address stability
// =============================================================================

#[test]
fn item13_mutex_forget_guard_keeps_locked() {
    let m = AsyncMutex::new(10u32);
    let g = m.try_lock().expect("lock");
    mem::forget(g);
    assert!(m.try_lock().is_none());

    let m_arc = Arc::new(AsyncMutex::new(20u32));
    let g_owned = Arc::clone(&m_arc).try_lock_owned().expect("lock_owned");
    mem::forget(g_owned);
    assert!(Arc::clone(&m_arc).try_lock_owned().is_none());
}

#[test]
#[should_panic(expected = "forgotten")]
fn item13_mutex_forget_guard_get_mut_panics() {
    let mut m = AsyncMutex::new(10u32);
    let g = m.try_lock().expect("lock");
    mem::forget(g);
    let _ = m.get_mut();
}

#[test]
#[should_panic(expected = "forgotten")]
fn item13_mutex_forget_guard_into_inner_panics() {
    let m = AsyncMutex::new(10u32);
    let g = m.try_lock().expect("lock");
    mem::forget(g);
    let _ = m.into_inner();
}

#[test]
fn item13_mutex_address_stability_and_traits() {
    #[inline(never)]
    fn lock_addr(m: &AsyncMutex<u64>) -> usize {
        let g = m.try_lock().expect("second lock");
        (&*g as *const u64) as usize
    }

    let mut m = AsyncMutex::from(123u64);
    assert_eq!(*m.get_mut(), 123);

    let g1_stack = m.try_lock().expect("first lock");
    let addr1_stack = (&*g1_stack as *const u64) as usize;
    assert_eq!(format!("{g1_stack}"), "123");
    assert_eq!(format!("{g1_stack:?}"), "123");
    assert!(
        format!("{m:?}").contains("<locked>"),
        "Debug while locked must print <locked>"
    );
    // Moving the guard onto the heap must keep the protected value's address unchanged.
    let g1_boxed = Box::new(g1_stack);
    let addr1_boxed = (&**g1_boxed as *const u64) as usize;
    assert_eq!(
        addr1_stack, addr1_boxed,
        "moving AsyncMutexGuard must not change the protected value's address"
    );
    drop(g1_boxed);

    let addr2 = lock_addr(&m);
    assert_eq!(
        addr1_boxed, addr2,
        "AsyncMutex value address must remain stable across consecutive locks"
    );

    assert_eq!(m.into_inner(), 123);

    let m_def = AsyncMutex::<u32>::default();
    assert_eq!(m_def.into_inner(), 0);
}

// =============================================================================
// Item 15: Wake-only registrations (Sender::poll_reserve)
// =============================================================================

#[test]
fn item15_poll_reserve_wake_only_vs_fifo_reserve() {
    let (tx, rx) = channel::bounded::<u32>(1);
    let mut tx_poll = tx.clone();

    tx.try_send(1).expect("fill buffer");

    // Register wake-only poll_reserve first, then queue a FIFO reserve().
    let w_poll = CountingWaker::new();
    let w_fifo = CountingWaker::new();
    {
        let w = w_poll.waker();
        let mut cx = Context::from_waker(&w);
        assert!(tx_poll.poll_reserve(&mut cx).is_pending());
    }
    let mut fifo_res = pin!(tx.reserve());
    assert!(poll_once(fifo_res.as_mut(), &w_fifo).is_pending());

    // Free one slot: FIFO reserve MUST get it, NOT wake-only poll_reserve.
    assert_eq!(rx.try_recv(), Ok(1));
    assert_eq!(w_fifo.count(), 1);
    assert_eq!(
        w_poll.count(),
        0,
        "wake-only poll_reserve must not wake when a FIFO waiter claimed the slot"
    );

    // Polling tx_poll now must still be Pending because FIFO waiter holds the slot.
    {
        let w = w_poll.waker();
        let mut cx = Context::from_waker(&w);
        assert!(tx_poll.poll_reserve(&mut cx).is_pending());
    }

    // FIFO waiter takes its permit and drops it unused -> slot is now unclaimed -> tx_poll wakes!
    let Poll::Ready(Ok(fifo_permit)) = poll_once(fifo_res.as_mut(), &w_fifo) else {
        panic!("FIFO reserve must get the slot");
    };
    drop(fifo_permit);
    assert!(
        w_poll.count() >= 1,
        "wake-only poll_reserve must wake when an unclaimed slot becomes available"
    );

    // Abandon tx_poll without polling it again: a later FIFO reserve / try_reserve
    // must NOT be blocked by the unpolled wake-only handle!
    let later_permit = tx
        .try_reserve()
        .expect("abandoned poll_reserve handle must not hold or block slots");
    later_permit.send(2);
    assert_eq!(rx.try_recv(), Ok(2));

    // When an unclaimed slot exists, polling tx_poll succeeds and sends cleanly.
    {
        let w = w_poll.waker();
        let mut cx = Context::from_waker(&w);
        let Poll::Ready(Ok(permit)) = tx_poll.poll_reserve(&mut cx) else {
            panic!("poll_reserve must succeed when an unclaimed slot is free");
        };
        permit.send(3);
    }
    assert_eq!(rx.try_recv(), Ok(3));

    // Closed channel makes poll_reserve return Ready(Err(SendError(()))).
    tx.close();
    {
        let w = w_poll.waker();
        let mut cx = Context::from_waker(&w);
        match tx_poll.poll_reserve(&mut cx) {
            Poll::Ready(Err(channel::SendError(()))) => {}
            other => panic!("expected Ready(Err(SendError(()))) after close, got {other:?}"),
        };
    }
}
