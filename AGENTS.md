# AGENTS.md — akku

## What this is

Akku is a pragmatic Rust battery pack. It is not a replacement for the Rust
standard library.

The public contract is:

- one dependency for applications: `akku`
- one namespace: `akku::<domain>::<name>`
- `akku-drive` is the proc-macro implementation behind `akku::http`
- `akku-cli` is the optional scaffolder

Read [`Namespace.md`](Namespace.md) before adding a public name. Read
[`CONCEPT.md`](CONCEPT.md) before adding a domain.

## Commands

```sh
cargo test --workspace --all-features
cargo test -p akku --no-default-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo doc --workspace --no-deps
```

`changie` fragments live in `.changes/unreleased/`. Releases use the version in
`[workspace.package]` and `release.yml` with crates.io Trusted Publishing.

## Rules

1. No global names. Use `akku::http::get`, not `akku::get`.
2. `akku::html` is separate from `akku::http`.
3. Macros are sugar over a pleasant manual API. Do not make a macro the only
   way to use a feature.
4. A dependency needs a reason, a pinned version and a review.
5. Crypto and TLS are not reimplemented in Akku.
6. Every public item has docs; `#![deny(missing_docs)]` enforces it.
7. Every feature has an example or a test.
8. `main` is released; do not publish from a dirty tree without a release PR.
