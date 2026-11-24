---
name: akku-smd-workflow
description: Operational reference for running cargo tests, clippy, fmt, docs, catalog generation, and release benchmarks inside the smd container.
---

# Akku `smd` Container Workflow Skill

**Never run `cargo`, `rustc`, or build scripts on the host.** Always execute via `smd` using [`smd.v0.6.toml`](../../../smd.v0.6.toml).

## 1. Everyday Commands

```sh
# Single module tests (fast feedback for Workers)
smd cargo test -p akku --test <module>
smd cargo test -p akku --lib <module>
smd cargo test -p akku --doc <module>

# Full workspace checks (for Orcha before finishing a batch)
smd cargo test --workspace --all-features
smd cargo test -p akku --no-default-features
smd cargo clippy --workspace --all-targets --all-features -- -D warnings
smd cargo doc --workspace --no-deps
smd cargo fmt --all -- --check
```

## 2. Chained Commands & Shell Redirection

Any command containing `&&`, `||`, `|`, or `>` MUST be wrapped in `'{...}'`:

```sh
smd '{cargo fmt --all && cargo clippy -p akku --all-targets --all-features -- -D warnings && cargo test -p akku --test ws}'
```

## 3. Catalog Generation & Verification

Whenever a module is added, removed, or its `meta.toml` changes:

```sh
smd cargo run -p akku-cli -- catalog gen
smd cargo run -p akku-cli -- catalog check
```

This regenerates [`docs/CATALOG.md`](../../../docs/CATALOG.md) and [`docs/catalog.json`](../../../docs/catalog.json).

## 4. Release Benchmarks & Stress Checks

Always benchmark in `--release` inside `smd` (debug builds distort throughput by 10x-50x). If using a temporary example/test harness, delete it immediately after running and verify `git status` is clean.

```sh
smd cargo test -p akku --release --test <module> -- --nocapture
```

## 5. Container Lifecycle & Notes

- Default `smd <cmd>` reuses persistent container `akku_s` (keeps Cargo target/registry cache warm).
- `smd --close` removes `akku_s` if a clean rebuild of the container image is ever needed.
- `git commit` is blocked inside `smd` by design (`block_cmd = ["git commit"]` in `smd.v0.6.toml`); commits are created on the host only by a Checker via the [Commit Workflow](../../../AGENTS.md#commit-workflow) (Worker commit request → Checker gate → commit), or by the human.
