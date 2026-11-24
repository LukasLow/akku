---
name: akku-module-impl
description: Worker playbook for implementing or fixing an akku module. Use when assigned a new module, a NewLib gate (G3/G4), a bugfix batch, or a migration of one module.
---

# Module implementation (Worker)

You are a Worker: your prompt opened with `hello Worker I am Orcha`. Work only in your assigned scope, spawn no subagents, and never commit.

## 1. Pre-flight

1. Read the module's `_dev/DESIGN.md` if it exists; it holds the agreed API and seams.
2. Check the public path against [`Namespace.md`](../../../Namespace.md): flat first; `server`/`client` live inside protocol modules; providers live inside domain modules (`kv::redis`, `sql::sqlite`).
3. Toolchain: Rust 1.85, edition 2021, `#![deny(missing_docs)]`, zero external dependencies for `native` modules (`std`/`alloc`/`core` only). `unsafe` only per [`akku-unsafe`](../akku-unsafe/SKILL.md).

Done when you can name the module path, its status (`native`/`fork`/`adapter`), and the seams you will test.

## 2. Layout

Follow [`akku-module-layout`](../akku-module-layout/SKILL.md) for folders, wiring, `meta.toml`, and what goes in rustdoc versus `_dev/`.

## 3. Tests first

Follow [`akku-tdd`](../akku-tdd/SKILL.md): red at the public API, then green. A bugfix starts with a regression test that fails for the reported reason ([`BugFix`](../../workflows/BugFix.md)).

## 4. Guardrails

Untrusted input is the default assumption. The code holds against every bug class in [`akku-security-audit`](../akku-security-audit/SKILL.md) §Checklist; in practice:

- Checked arithmetic and `.get(..)` on every untrusted length, offset, and index; malformed input returns a typed error.
- Allocation only after a length is validated against a cap; read progressively otherwise.
- Every DoS limit (`max_depth`, `max_bytes`, `max_items`, `max_expansion`, …) has a sane default and a builder setter, and is checked **before** the allocation or clone it guards.
- Iterative `Drop` for arbitrarily nested trees; depth tracked on every recursive path.
- Linear-time parsers and serializers; buffered I/O with one write per frame.

## 5. Docs

- Rustdoc on every public item, with a runnable example in the module-level `//!` docs. User-relevant consequences (not constant-time, partial writes on error, panics) go in rustdoc; the reasons go in `_dev/DESIGN.md`.
- New decisions, rejected options, and deferred APIs go to `_dev/DESIGN.md` / `_dev/TODO.md` when the module has `_dev/`.
- User-facing change → changie fragment per [`akku-pr`](../akku-pr/SKILL.md).

## 6. Verify in `smd`

```sh
smd cargo test -p akku --test <module>
smd cargo test -p akku --lib <module>
smd cargo test -p akku --doc <module>
smd cargo clippy -p akku --all-targets --all-features -- -D warnings
smd '{RUSTDOCFLAGS="-D warnings" cargo doc -p akku --no-deps}'
smd cargo fmt --all -- --check
```

Done when all six pass. If your module's `meta.toml` changed, run `smd cargo run -p akku-cli -- catalog gen` and `catalog check`, and list `docs/CATALOG.md` and `docs/catalog.json` in the commit request. Adding or removing a module (`lib.rs` wiring) stays with Orcha. A migration also runs the packaging check in [ModuleMigration](../../workflows/ModuleMigration.md) step 7.

## 7. Report

Report concisely to Orcha: files changed, test names added, anything left open. End with the `COMMIT REQUEST` block defined in [`PullRequest`](../../workflows/PullRequest.md).
