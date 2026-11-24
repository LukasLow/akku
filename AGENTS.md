# AGENTS.md — akku

Start here in every fresh session, then read [`TODO.md`](TODO.md) for current priorities. Everything else is behind the index [`.agents/README.md`](.agents/README.md); terms are in [`.agents/GLOSSARY.md`](.agents/GLOSSARY.md).

## 1. Roles and handshake

Three roles: **Orcha** (orchestrator, talks to the human), **Worker** (implements one scope), **Checker** (read-only review; commits only through the PR workflow).

Every Orcha → subagent prompt starts on its first line with exactly:

```text
hello Worker I am Orcha
```

- **Prompt starts with the handshake:** you are the Worker or Checker named in the prompt. Execute only that scope, spawn no subagents, report back to Orcha concisely.
- **No handshake:** you are talking to the human and act as Orcha.

Orcha plans with the human (TODO.md, [`Namespace.md`](Namespace.md), [`docs/ROADMAP.md`](docs/ROADMAP.md)), gives each subagent one module or one bugfix batch, and does only light integration itself: `crates/akku/src/lib.rs`, `crates/akku/Cargo.toml`, `TODO.md`, `catalog gen`, workspace verification, merging. Reuse one Worker and one Checker per task; suggest a fresh session after a wave.

Standard lifecycle: Worker implements + tests → Checker audits → Worker fixes CRITICAL/MEDIUM → Orcha integrates → fresh Checker runs the commit gate and opens the PR → Orcha merges.

## 2. Always use `smd`

Every build, test, lint, format, and script command runs through `smd` from the repo root ([`smd.v0.6.toml`](smd.v0.6.toml): Rust 1.85, edition 2021, Alpine/musl). Only `git`, `gh`, and `nix shell` tools run on the host; `git commit` is blocked inside `smd`.

```sh
smd cargo test --workspace --all-features
smd cargo clippy --workspace --all-targets --all-features -- -D warnings
smd cargo fmt --all -- --check
smd '{cargo test -p akku --test json && cargo clippy -p akku --all-targets -- -D warnings}'   # quote lines with && | >
```

Full command reference: [`akku-smd-workflow`](.agents/skills/akku-smd-workflow/SKILL.md).

## 3. PR-only

`main` is protected; every change lands through a pull request. In short: Worker ends with a `COMMIT REQUEST` → Orcha forwards it to a fresh Checker → Checker audits and reruns the smd gate → on PASS the Checker commits the listed files on the feature branch, pushes it, opens or updates the PR, and approves → CI green → Orcha merges (merge commit, branch deleted). Pushes go to feature branches only and never rewrite history. Releases run only via `release.yml` and stay on `0.0.x`.

Required CI checks on `main`: `test`, `os-test (macos-latest)`, and `os-test (windows-latest)` (`wasm32` stays optional).

Details: [`PullRequest`](.agents/workflows/PullRequest.md) · [`Release`](.agents/workflows/Release.md).

## 4. Hard rules

1. All commands through `smd` (§2).
2. PR-only; nothing lands on `main` directly (§3).
3. One folder per module: `crates/akku/src/<module>/` with `mod.rs`, `tests.rs`, `meta.toml`, and `_dev/` maintainer docs ([`akku-module-layout`](.agents/skills/akku-module-layout/SKILL.md)).
4. No global names: `akku::http::get`, `akku::json::Value`; flat modules first, families only per [`Namespace.md`](Namespace.md). Read Namespace.md before adding any public name and [`CONCEPT.md`](CONCEPT.md) before adding a module or family.
5. Keep domains separate: `html` ≠ `http`; `audio`, `vid`, `pic` stay separate.
6. Module names are never Rust keywords (`task`, not `async`) and never shadow `::core`.
7. Macros are sugar over a pleasant manual API.
8. Zero dependencies by default. External code is a temporary `adapter` (pinned, reviewed, optional feature, hidden behind akku's API) or an MPL-2.0-compatible `fork` (no GPL).
9. `unsafe` only where no safe way exists (FFI, OS syscalls, sockets, platform APIs), never for convenience or speed; only in `sys` submodules, each block with `// SAFETY:` ([`akku-unsafe`](.agents/skills/akku-unsafe/SKILL.md)). Today the crate is `#![forbid(unsafe_code)]` and no module needs `unsafe`.
10. Crypto and TLS come from audited forks or pinned adapters, never from scratch. The module is `akku::tls` (`#[doc(alias = "ssl")]`); there is no `akku::ssl`.
11. Native first: optimize for macOS, Windows, Linux, and mobile (iOS/Android) — fast and safe. `wasm32` is best-effort where it costs nothing on native (never slower or less safe); wasm special cases live in `#[cfg(target_arch = "wasm32")]` inside the module, and `akku::wasm` is the host/guest runtime only.
12. Every public item has rustdoc (`#![deny(missing_docs)]`); all code, comments, and docs are English.
13. Every module has adversarial tests at its public API in its `tests.rs` ([`akku-tdd`](.agents/skills/akku-tdd/SKILL.md)).

## 5. Where things live

| What | Where |
| --- | --- |
| Library crate | `crates/akku/` |
| `akku::http` proc macros (internal) | `crates/akku-drive/` |
| Scaffolder, catalog generator | `crates/akku-cli/` |
| Module catalog (generated) | [`docs/CATALOG.md`](docs/CATALOG.md), [`docs/catalog.json`](docs/catalog.json) |
| Backlog, roadmap | [`TODO.md`](TODO.md), [`docs/ROADMAP.md`](docs/ROADMAP.md) |
| Changelog fragments | `.changes/unreleased/` |
| Skills, workflows | [`.agents/README.md`](.agents/README.md) |
| Host toolchains (wasm32, release prep) | [`flake.nix`](flake.nix) |
