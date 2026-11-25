---
name: akku-unsafe
description: akku's unsafe policy and mechanics. Use when code seems to need `unsafe` (FFI, OS syscalls, sockets, platform APIs), when touching a `sys` submodule, or when auditing an `unsafe` block.
---

# unsafe policy

**Rule:** `unsafe` is allowed only where no safe way exists: FFI, OS syscalls, socket options, platform APIs that `std` does not expose. Convenience and speed never justify it; a safe `std` path, a slower safe algorithm, or an audited adapter crate comes first.

**Current state:** `crates/akku/src/lib.rs` sets `#![deny(unsafe_code)]`, `#![deny(unsafe_op_in_unsafe_fn)]`, and `#![deny(clippy::undocumented_unsafe_blocks)]`, and `.github/unsafe-allowlist.txt` is currently empty (no module uses `unsafe` yet). `akku-drive` and `akku-cli` remain `#![forbid(unsafe_code)]`.

## Mechanics

1. Crate level (`crates/akku/src/lib.rs`): `#![deny(unsafe_code)]`, `#![deny(unsafe_op_in_unsafe_fn)]`, and `#![deny(clippy::undocumented_unsafe_blocks)]`.
2. `#![allow(unsafe_code)]` appears only inside an allowlisted `sys` submodule file: `crates/akku/src/<module>/sys/*.rs` (or `crates/akku/src/os/<platform>/sys/*.rs`). Everything outside `sys/` stays safe and calls `sys` through a safe wrapper.
3. Every `unsafe` block and `unsafe fn` carries a `// SAFETY:` comment naming the invariant and why it holds here. Enforced by `#![deny(clippy::undocumented_unsafe_blocks)]` and `#![deny(unsafe_op_in_unsafe_fn)]`.
4. CI enforces an allowlist of `crates/.../sys/*.rs` files that may contain `unsafe` or `#![allow(unsafe_code)]`: [`.github/unsafe-allowlist.txt`](../../../.github/unsafe-allowlist.txt), checked by `.github/scripts/check-unsafe.sh`. Adding a file to it requires Checker review and human approval in the PR.

## Writing a `sys` module

- The safe wrapper validates all inputs (lengths, null bytes, fd ranges) before crossing into `unsafe`; the `unsafe` block itself is as small as possible.
- Platform code is `#[cfg(target_os = "…")]`-gated; unsupported targets return a typed `Unsupported` error rather than failing to compile.
- `_dev/DESIGN.md` §Dependencies states which syscalls/FFI symbols are used and why no safe path exists.
- Tests in `tests.rs` exercise the safe wrapper, including invalid inputs.

```rust
// SAFETY: `fd` is owned by `self` and open for the lifetime of the borrow;
// `buf` is a valid writable slice of `buf.len()` bytes.
let n = unsafe { libc_read(fd, buf.as_mut_ptr(), buf.len()) };
```

## Checker review of `unsafe`

For every block, verify the `// SAFETY:` claim against the code (pointer validity, lengths, aliasing, lifetimes, thread safety, error-return handling), confirm no safe alternative exists, and confirm the block sits inside a `sys` module. Any failing item is a CRITICAL finding.
