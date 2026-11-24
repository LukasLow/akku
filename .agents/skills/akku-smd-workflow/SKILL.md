---
name: akku-smd-workflow
description: Command reference for running cargo test, clippy, fmt, doc, package, catalog, and benchmarks inside the smd container. Use before running any build, test, lint, or script command in akku.
---

# `smd` command reference

Every cargo/rustc/build command runs through `smd` from the repo root, using [`smd.v0.6.toml`](../../../smd.v0.6.toml) (Rust 1.85, Alpine/musl). Host-only exceptions: `git` (commits are blocked inside `smd`), `gh`, and extra toolchains via `nix shell .#` ([`flake.nix`](../../../flake.nix), e.g. the wasm32 check).

## Module (Worker)

```sh
smd cargo test -p akku --test <module>
smd cargo test -p akku --lib <module>
smd cargo test -p akku --doc <module>
smd cargo clippy -p akku --all-targets --all-features -- -D warnings
smd cargo fmt --all -- --check
```

## Workspace (Orcha, Checker commit gate)

These mirror CI ([`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml)):

```sh
smd cargo fmt --all -- --check
smd cargo clippy --workspace --all-targets -- -D warnings
smd cargo clippy --workspace --all-targets --all-features -- -D warnings
smd cargo test -p akku --no-default-features --locked
smd cargo test --workspace --all-features --locked
smd '{RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps}'
```

## Catalog

After a module is added or removed, or a `meta.toml` changes:

```sh
smd cargo run -p akku-cli -- catalog gen     # writes docs/CATALOG.md + docs/catalog.json
smd cargo run -p akku-cli -- catalog check
```

## Packaging

```sh
smd cargo package -p akku --list --allow-dirty   # no `_dev` path may appear
```

## Shell syntax

Wrap any line with `&&`, `||`, `|`, `>`, or env assignments in quoted braces so the container shell runs it:

```sh
smd '{cargo fmt --all && cargo test -p akku --test ws}'
```

## Benchmarks

Benchmark only in `--release` (debug distorts throughput 10-50x). Delete temporary harnesses afterwards and confirm `git status` shows only your scope.

```sh
smd cargo test -p akku --release --test <module> -- --nocapture
```

## Container

- `smd <cmd>` reuses the session container `akku_s` (warm target and registry caches).
- `smd -t <cmd>` runs in a throwaway container with `.env` protection.
- `smd --open` opens a shell; `smd --close` removes `akku_s` (for an image rebuild after editing `[dockerfile]`).
