# io — audit log

## 2026-11-26 · full module @ 862104b (`mod.rs`, `tests.rs`, `meta.toml`) · Checker (Claude), NewLib G5

Checklist: `akku-security-audit` §A–D. `io` contains 0 `unsafe` blocks. Threat model: buggy or hostile `AsyncRead`/`AsyncWrite` implementations (over-reporting byte counts, `Ok(0)` writers, endless `Interrupted`, `Pending` after `Ready`) and untrusted stream contents. Line numbers refer to `862104b`. `git diff 8da365c 862104b -- crates/akku/src/io/tests.rs | grep '^-[^-]'` is empty, so no G3 test was weakened.

## `io`

| Sev | Location | Mechanism -> Trigger |
|---|---|---|
| LOW | `crates/akku/src/io/mod.rs:762-787` (`FillBufFuture::poll`) | `poll_fill_buf` is called twice on every completion, a second time with the full `'a` borrow. This relies on an unstated contract that a `Ready` without `consume` returns the same bytes without new I/O. That holds for the bundled `&[u8]`, `Cursor`, and `Empty`. A third-party buffered reader at EOF (`Ready(Ok(&[]))`) instead goes back to its source a second time, which costs an extra syscall and can return different data (a "transient EOF" source such as a tty) or `Pending`. In the `Pending` case the future reports a synthetic `Error::other`. Memory-safe. -> Trigger: an `AsyncBufRead` that returns `Ready(Ok(&[]))` and then `Pending` on the immediate re-poll yields `Err("…returned Pending right after Ready")`. Fix: state the idempotence requirement in the `AsyncBufRead::poll_fill_buf` rustdoc, or restructure once Polonius-style borrows allow a single poll. |
| LOW | `crates/akku/src/io/mod.rs:547-561, 596-642, 695-703` (`read_exact`, `copy_to`, `write_all`) | `Interrupted` is retried in an unbounded loop inside one poll. A stream that always returns `Interrupted` spins forever without yielding. std's `read_exact`/`copy` behave the same, and the trigger is a buggy stream, not peer input. -> Trigger: `poll_read` that always returns `Err(Interrupted)`. Document only, or cap retries and surface the error. |
| LOW (cross-ref) | `crates/akku/src/io/mod.rs:594-643` (`CopyToFuture`) | `copy_to` between two always-ready sockets never returns `Pending`, which is the `net`/`task` CRITICAL (no cooperative budget). The fix belongs in `net::reactor`, not in `io`. Once the budget exists, `copy_to` gets it for free. |
| — | `mod.rs` (all futures) | **Verified clean:** every reported `n` is checked against the slice length before it is used (`over_reported` → `InvalidData`), so no out-of-range slicing or `filled += n` overflow is possible. `Ok(0)` on non-empty input gives `WriteZero` (`write_all`, `copy_to`). Zero-length operations return without polling. `copy_to` writes each chunk fully before the next read, flushes before parking on a pending reader and on EOF, and saturates `total: u64`. It allocates one fixed 8 KiB buffer when the future is created, and nothing grows with input. Dropping any future mid-flight keeps the progress it made (`read_exact` and `write_all` progress is visible to the caller), as documented. `AsyncBufRead::consume` on `&[u8]` clamps `amt`. The `Pin<P>`/`&mut T`/`Box<T>` forwarding impls are correct and `Unpin`-bounded. The dyn-compatibility of all three traits is preserved. |

**Scores (0-10):** Security 9 · Correctness 9 · Speed 9 · Architecture 9

**Top 3 regression tests to add first:**
1. `fill_buf_on_reader_that_is_pending_after_eof_is_documented_behavior`: pins the double-poll contract, or its removal, with a custom `AsyncBufRead`.
2. `copy_to_between_ready_sockets_yields` (after the `net` budget fix): a sibling task makes progress during a large loopback `copy_to`.
3. `read_exact_and_write_all_resume_after_drop_mid_flight`: `filled` and `remaining` progress survives cancellation across a `Pending` partial transfer (exercises the documented cancel semantics on a real socket).

**Verdict:** PASS (0 CRITICAL, 0 MEDIUM; LOWs documented)

## Resolution (Worker, 2026-11-26)

No `io` source change was needed.

| Finding | Fix (file:line) | Regression test |
|---|---|---|
| LOW (cross-ref): `copy_to` between always-ready sockets never yields | Fixed by the cooperative budget in `net::reactor` (`crates/akku/src/net/reactor.rs:393`, `:464`), which `copy_to` reaches through `poll_read`/`poll_write` | `net/tests.rs`: `copy_to_between_ready_sockets_yields_to_siblings` |
| LOW: `FillBufFuture` double poll | Left as documented | — |
| LOW: unbounded `Interrupted` retry | Left as documented (std parity) | — |

## Re-audit (Checker, 2026-11-26): PASS

No `io` source changed (`git diff 862104b -- crates/akku/src/io` touches only this file). `copy_to` now yields through the `net::reactor` budget. `copy_to_between_ready_sockets_yields_to_siblings` uses a 1-byte reader into an always-ready sink and asserts that the sibling ran during the copy. Without the budget the sibling would never run, so the test fails without the fix. The `io` tests are unchanged. Remaining LOWs, carried over: the `FillBufFuture` double poll and the unbounded `Interrupted` retry (std parity).
