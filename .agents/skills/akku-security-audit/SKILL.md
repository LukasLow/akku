---
name: akku-security-audit
description: Read-only Checker audit checklist for finding verified security, crash/panic, data-loss, and performance bugs in akku modules with zero false positives.
---

# Akku Security & Correctness Audit Skill (Checker)

Use this skill when acting as a **Checker** performing an independent, **strictly read-only** audit of one or more `akku` modules.

## 1. Strict Rules

- **Read-only:** Do NOT edit files, do NOT create scratch files, and do NOT spawn subagents.
  - **Only exception — commit gate:** if Orcha forwarded a Worker `COMMIT REQUEST`, and only after the audit and the `smd` gate pass, you may `git add` exactly the listed files and `git commit` on the host (never on `main`, never push). See [`AGENTS.md` → Commit Workflow](../../../AGENTS.md#commit-workflow).
- **Zero False Positives:** Trace every suspected bug through the actual code paths before reporting it. If a guard elsewhere prevents the bug, drop the finding — never report speculative `[CRITICAL]` issues.
- **Concrete Proof:** Every finding MUST include exact `file:line`, the bug mechanism, and a minimal concrete trigger input.

## 2. Mandatory Audit Checklist

Inspect every module in scope across these 4 dimensions:

### A. Security & Adversarial Inputs
- **Path Traversal & Link Escapes:** Archive extraction or FS helpers allowing `../`, absolute paths, symlink chains, or hardlinks targeting files outside destination root.
- **Injection & XSS Bypasses:** HTML/Markdown/XML/Template escaping gaps (e.g. control chars like `\x0c`, unquoted attributes, `XmlDecl`/PI/CDATA breakouts, `javascript:`/`data:` URL schemes).
- **DoS / OOM / Amplification:**
  - Pre-validation allocation (`vec![0; header_len]`, `with_capacity(untrusted)`).
  - Quadratic/exponential expansion (YAML anchor cloning without alias limits, template string `replace`/`repeat` checked *after* allocation, decompression bombs, ReDoS).
  - Depth bypasses (nested constructs that skip `depth += 1`) and **recursive `Drop` stack overflow** on deeply nested AST/Value types.
- **RNG / Crypto / Protocol Smuggling:** Predictable fallback RNGs, unvalidated extensions/headers, `Content-Length` / `Transfer-Encoding` smuggling, leading `+` accepted by `from_str_radix`.

### B. Correctness, Panics & Data Loss
- **Deterministic Panics:** Unchecked arithmetic (`+`, `-`, `*` on `usize`), slice indexing out of bounds, UTF-8 char-boundary slicing panics, `.unwrap()` on untrusted input.
- **Streaming & State-Machine Bugs:** Infinite loops or spurious errors on chunked reads, lost already-parsed events on mid-chunk errors, missing resync after oversized lines/frames, connection desync returning another request's response.
- **Persistence & Round-Trip Corruption:** WAL/file recovery failing on zero-filled crash tails or silently truncating valid records on corruption; serializer producing output that fails to round-trip (e.g. quotes, YAML document markers `---`, CRLF patch mismatches).

### C. Runtime Speed & Allocations
- Hidden $O(n^2)$ loops (linear scans inside loops, bubble sort, repeated `String` re-scanning/re-allocation, `mem::take` dropping reusable buffer capacity per line).
- Unbuffered byte-by-byte I/O or unbuffered multi-`write_all` socket sends (Nagle + delayed ACK stalls).
- Syscall-per-operation patterns (e.g. opening `/dev/urandom` per frame).

### D. Architecture & API Hygiene
- Alignment with [`Namespace.md`](../../../Namespace.md) and `#![forbid(unsafe_code)]` / `#![deny(missing_docs)]`.
- Clean separation of parser / state machine / I/O / error types.

## 3. Required Output Format

Report per module using a concise severity table (`CRITICAL`, `MEDIUM`, `LOW`):

```markdown
## `<module>`

| Sev | Location | Mechanism -> Concrete Trigger |
|---|---|---|
| CRITICAL | `crates/akku/src/<mod>/<file>.rs:<line>` | <exact mechanism> -> Trigger: `<minimal input>` |

**Scores (0-10):** Security X · Correctness X · Speed X · Architecture X
```

End with a **Top 3 Regression Tests to Add First** list prioritized by real impact.
