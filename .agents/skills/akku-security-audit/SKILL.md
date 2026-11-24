---
name: akku-security-audit
description: Read-only Checker audit of akku code for verified security, panic, data-loss, speed, and architecture bugs. Use when auditing a module or a diff, running a NewLib G5 gate, or acting as the commit gate on a COMMIT REQUEST.
---

# Security & correctness audit (Checker)

You are a Checker: your prompt opened with `hello Worker I am Orcha`. Spawn no subagents.

## Rules

- **Read-only.** You change no files. The single write you perform is the commit/push/PR step of [`PullRequest`](../../workflows/PullRequest.md), and only after the audit and the smd gate pass.
- **Verified findings only.** Trace each suspected bug through the real code path; if a guard elsewhere prevents it, drop it. Report what you proved.
- **Concrete proof.** Every finding has an exact `file:line`, the mechanism, and a minimal trigger input.

## Checklist

Apply every item to every file in scope.

### A. Security & adversarial input
- **Path escapes:** `..`, absolute paths, symlink chains, hardlinks leaving the destination root (archives, fs helpers).
- **Injection / XSS:** escaping gaps in HTML, Markdown, XML, templates (control chars like `\x0c`, unquoted attributes, PI/CDATA breakouts, `javascript:`/`data:` URLs).
- **DoS / OOM / amplification:** allocation sized by an unvalidated length (`vec![0; n]`, `with_capacity(n)`); expansion checked after allocating (YAML anchors, template `replace`, decompression ratio, regex repetition); nesting that skips the depth counter; recursive `Drop` on deep trees.
- **RNG / protocol:** predictable fallback RNG, request smuggling (`Content-Length` vs `Transfer-Encoding`), lenient number parsing (leading `+` via `from_str_radix`).
- **unsafe:** every block per [`akku-unsafe`](../akku-unsafe/SKILL.md) §Checker review.

### B. Correctness, panics, data loss
- **Panics:** unchecked `+ - *` on `usize`, out-of-range indexing, slicing off a UTF-8 char boundary, `unwrap()` on untrusted data.
- **Streaming / state machines:** hangs or spurious errors at chunk boundaries, lost events after a mid-chunk error, missing resync after an oversized frame, connection desync.
- **Persistence & round-trips:** crash-tail recovery (zero-filled tails, torn records), silent truncation on corruption, serializer output that fails to parse back identically.

### C. Speed & allocations
- Hidden O(n²): linear scans in loops, quadratic sorts, repeated re-scanning or re-allocation.
- Unbuffered byte-wise I/O; several `write_all` calls per frame (Nagle stalls); a syscall per operation.

### D. Architecture
- Path and names match [`Namespace.md`](../../../Namespace.md); layout matches [`akku-module-layout`](../akku-module-layout/SKILL.md).
- Rustdoc on every public item; rationale lives in `_dev/`, not in rustdoc.
- Parser, state machine, I/O, and error types are separated; tests follow [`akku-tdd`](../akku-tdd/SKILL.md) (public seams, no tautologies).

## Output

```markdown
## `<module>`

| Sev | Location | Mechanism -> Trigger |
|---|---|---|
| CRITICAL | `crates/akku/src/<mod>/<file>.rs:<line>` | <mechanism> -> Trigger: `<minimal input>` |

**Scores (0-10):** Security X · Correctness X · Speed X · Architecture X
**Top 3 regression tests to add first:** …
**Verdict:** PASS | FAIL (FAIL when any CRITICAL or MEDIUM is open)
```

Severity: **CRITICAL** = exploitable, panic/crash on input, or data loss. **MEDIUM** = wrong result, significant slowdown, or a weakened guard. **LOW** = edge case or hygiene. Orcha records findings in the module's `_dev/AUDIT.md` (see [`Audit`](../../workflows/Audit.md)).
