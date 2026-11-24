---
name: akku-module-impl
description: Step-by-step Worker playbook for implementing or fixing a zero-dependency Rust module in akku with tests, docs, DoS guardrails, and meta.toml.
---

# Akku Module Implementation Skill (Worker)

Use this skill when acting as a **Worker** implementing a new `akku::<module>` or applying verified bugfixes to an existing module.

## 1. Pre-Flight Checks

1. Read [`Namespace.md`](../../../Namespace.md) to verify the exact module path, sub-structure (`server`/`client` live inside protocol modules; provider names live inside domain modules like `kv::redis`, `sql::sqlite`), and forbidden top-level names.
2. Check [`crates/akku/src/lib.rs`](../../../crates/akku/src/lib.rs) and [`crates/akku/Cargo.toml`](../../../crates/akku/Cargo.toml) to see existing conventions and `[[test]]` registrations.
3. Confirm Rust toolchain constraints: **Rust 1.85**, **Edition 2021**, `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`, **zero external dependencies** (for `native` modules; `std`/`alloc`/`core` only).

## 2. Module Folder Layout

Create or update everything self-contained under `crates/akku/src/<module>/`:

```text
crates/akku/src/<module>/
  mod.rs          # public API, module-level rustdoc with quick example
  ...             # clean internal submodules (keep files modular & readable)
  tests.rs        # public-API integration tests (registered via [[test]])
  meta.toml       # catalog metadata
```

### `meta.toml` Schema

```toml
name = "<module>"
status = "native" # "native" | "fork" | "adapter"
wasm32 = true     # true if usable on wasm32 without OS-only sockets/fs
wave = 2          # 1 | 2 | 3
summary = "One-line English summary of the module."
owners = ["LukasLow"]
tags = ["<module>", "<domain>", "<category>"]
```

### Wiring (when adding a brand-new module)

- Export `pub mod <module>;` in alphabetical order in [`crates/akku/src/lib.rs`](../../../crates/akku/src/lib.rs).
- Register the integration test in [`crates/akku/Cargo.toml`](../../../crates/akku/Cargo.toml):
  ```toml
  [[test]]
  name = "<module>"
  path = "src/<module>/tests.rs"
  ```

## 3. Mandatory Engineering & Security Guardrails

Every `akku` module MUST be resilient against untrusted input by default:

- **Bounds & Checked Arithmetic:** Use `checked_add`/`checked_sub`/`checked_mul` and slice `.get(..)` on all untrusted lengths, offsets, headers, and indices. Never panic on malformed input.
- **No Pre-Validation OOM:** Never `vec![0; untrusted_len]` or `Vec::with_capacity(untrusted_len)` before validating against a configurable cap or reading progressively.
- **Explicit DoS Limits:** Enforce sensible default caps (`max_depth`, `max_bytes`, `max_output_bytes`, `max_items`, `max_expansion`) with builder setters so callers can tune them.
- **Stack Safety:** Watch both **parsing recursion** AND **recursive `Drop`** on deeply nested trees/ASTs/values (implement iterative `Drop` where arbitrary nesting is possible).
- **No Amplification Bombs:** Check expansion limits *before* cloning/replacing/allocating (e.g. template `replace`, YAML anchors/aliases, archive decompression ratios, regex repetitions).
- **Path Safety (Archives/FS):** Canonicalize/validate relative paths; reject `..`, absolute paths, symlinks/hardlinks escaping the destination root.
- **Streaming & I/O Correctness:** Handle chunk boundaries cleanly (never hang or lose buffered events on partial reads), buffer header+payload writes to avoid Nagle stalls, and avoid per-message syscalls (e.g. opening `/dev/urandom` per frame).
- **Algorithmic Complexity:** Avoid hidden $O(n^2)$ loops in parsers, serializers, diffing, text processing, and namespace/table lookups.
- **Clean Recovery:** On WAL/file persistence, treat partial or zero-filled crash tails safely without silently dropping earlier valid records.

## 4. Verification Inside `smd`

Run all checks for your module inside `smd` before reporting back to Orcha:

```sh
smd cargo test -p akku --test <module>
smd cargo test -p akku --lib <module>
smd cargo test -p akku --doc <module>
smd cargo clippy -p akku --all-targets --all-features -- -D warnings
smd cargo fmt --all -- --check
```

If you added or edited `meta.toml`, note it in your report so Orcha can run `smd cargo run -p akku-cli -- catalog gen`.
