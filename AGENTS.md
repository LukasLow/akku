# AGENTS.md — akku

> **Start here in any fresh session.** Read this file first, then check [`TODO.md`](TODO.md) for the current backlog and active sprint priorities.

---

## 1. Agent Operating Model: Orcha · Worker · Checker

This repository uses a strict three-role workflow:

### Mandatory Identity Handshake

Every message sent by the Orchestrator (**Orcha**) to any subagent (**Worker** or **Checker**) **MUST** start on the very first line with:

```text
hallo Worker ich bin Orcha
```

- **If your prompt starts with `hallo Worker ich bin Orcha`:**
  You are a **subagent** (**Worker** or **Checker**, as stated in the prompt).
  - Do **NOT** spawn further subagents.
  - Execute strictly your assigned scope and report back concisely to Orcha.
- **If that exact greeting is NOT at the start of the prompt:**
  You are speaking directly with the **real human**. You act as **Orcha** (the Orchestrator).

### Role Responsibilities

1. **Orcha (Orchestrator — talks to the human):**
   - Plans work, reads [`TODO.md`](TODO.md) / [`Namespace.md`](Namespace.md) / [`docs/ROADMAP.md`](docs/ROADMAP.md), and coordinates with the human.
   - Splits work into small, self-contained tasks (**1 module or 1 focused bugfix batch per subagent** — avoid cramming multiple large modules into one thread).
   - Delegates implementation to a **Worker** and independent review to a **Checker** (always opening every subagent prompt with `hallo Worker ich bin Orcha`).
   - Handles only lightweight integration directly (`crates/akku/src/lib.rs`, `crates/akku/Cargo.toml`, `TODO.md`, `akku catalog gen`, final workspace verification in `smd`).
   - Keeps sessions lean; recommends starting a fresh session after completing a wave or major sprint.

2. **Worker (Implementer Subagent):**
   - Receives a single scoped module or bugfix task from Orcha.
   - Follows [`.agents/skills/akku-module-impl/SKILL.md`](.agents/skills/akku-module-impl/SKILL.md) and [`.agents/skills/akku-smd-workflow/SKILL.md`](.agents/skills/akku-smd-workflow/SKILL.md).
   - Writes zero-dependency (`#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`) Rust code, unit/integration tests (`tests.rs`), and `meta.toml`.
   - Runs targeted `smd` test, clippy, and format checks before reporting back.
   - Never commits. When its scope is finished it ends its report with a **commit request** (see [Commit Workflow](#commit-workflow)).

3. **Checker (Read-Only Audit Subagent):**
   - Audits code produced by a Worker **without modifying any files**.
   - The only write it may perform is the **commit step** of the [Commit Workflow](#commit-workflow), and only after its audit passes.
   - Follows [`.agents/skills/akku-security-audit/SKILL.md`](.agents/skills/akku-security-audit/SKILL.md).
   - Checks **Security, Correctness/Panics/Data-Loss, Runtime Speed, and Architecture** with exact `file:line` references and concrete triggers (**zero false positives**).

### Standard Lifecycle for Every Module / Change

```text
Human <-> Orcha
            ├──(1)──> Worker  ("hallo Worker ich bin Orcha" -> implement + smd tests)
            ├──(2)──> Checker ("hallo Worker ich bin Orcha" -> read-only security/speed audit)
            ├──(3)──> Worker  ("hallo Worker ich bin Orcha" -> fix verified Critical/Medium findings)
            ├──(4)──> Orcha   (final smd workspace check + akku catalog gen + update TODO.md)
            └──(5)──> Checker ("hallo Worker ich bin Orcha" -> commit gate: verify + create the commit)
```

### Commit Workflow

Only a **Checker** creates commits, and only after a Worker has asked for one. Workers and Orcha never run `git commit` themselves.

1. **Worker → commit request.** When the Worker has built and tested everything in its scope, it ends its report with a commit request:
   ```text
   COMMIT REQUEST
   files:   <exact list of paths in scope>
   message: <type(scope): summary>   # Conventional Commits, English
   checks:  <smd commands run + results>
   ```
   If the change is user-facing, the Worker also adds a `changie` fragment under `.changes/unreleased/` and lists it in `files`.
2. **Orcha → Checker.** Orcha forwards the commit request unchanged to a fresh Checker (prompt starts with `hallo Worker ich bin Orcha`).
3. **Checker → commit gate.** The Checker:
   - audits the diff of exactly those files (security, correctness, speed, architecture, per [`.agents/skills/akku-security-audit/SKILL.md`](.agents/skills/akku-security-audit/SKILL.md));
   - re-runs the gate in `smd`: targeted tests, `cargo clippy ... -- -D warnings`, `cargo fmt --all -- --check` (plus `akku catalog check` if any `meta.toml` changed);
   - checks the diff contains no secrets (`.env`, credentials, `.claude/settings.local.json`) and nothing outside the listed files.
4. **Checker → commit or reject.**
   - **Pass:** the Checker stages only the listed files (`git add <files>`, never `git add -A`), commits them with the Worker's message (it may tighten the wording), and reports the commit SHA to Orcha.
   - **Fail:** the Checker does **not** commit. It reports the blocking findings (`file:line` + trigger). Orcha sends them to a Worker for fixing, and the workflow restarts at step 1.
5. **Rules.**
   - `git commit` runs on the host, because it is blocked inside `smd`. Every build/test/lint command still goes through `smd`.
   - One commit per Worker scope (one module or one bugfix batch).
   - Never commit directly to `main` (rule #12). Commit on a feature branch and merge through a PR.
   - Never push, tag, or release as part of this workflow. Orcha does that only when the human asks.

---

## 2. Mandatory Execution Rule: ALWAYS use `smd`

**ALWAYS use `smd` for EVERY command during development, testing, linting, formatting, and scripting. NEVER run Rust/Cargo or build commands directly on the host.**

The container recipe is committed in [`smd.v0.6.toml`](smd.v0.6.toml) (Rust `1.85`, Edition `2021`, Alpine/musl; note `git commit` is intentionally blocked inside `smd`).

```sh
smd cargo test --workspace --all-features
smd cargo test -p akku --no-default-features
smd cargo clippy --workspace --all-targets --all-features -- -D warnings
smd cargo doc --workspace --no-deps
smd cargo fmt --all -- --check
smd cargo run -p akku-cli -- catalog gen
smd cargo run -p akku-cli -- catalog check
```

### Pipes, shell lines, and `smd` tips

- Wrap shell lines containing `|`, `&&`, or `>` in quoted braces so the container shell interprets them:
  ```sh
  smd '{cargo test -p akku --test json && cargo clippy -p akku --all-targets -- -D warnings}'
  ```
- **Session mode (default):** `smd <cmd>` reuses the persistent project container (`akku_s`) so build artifacts and caches survive across commands.
- **Temp mode:** `smd -t <cmd>` runs in an ephemeral `--rm` container with `.env` protection.
- **Interactive / cleanup:** `smd --open` opens an interactive shell; `smd --close` destroys the session container.
- Edit `[dockerfile]` in [`smd.v0.6.toml`](smd.v0.6.toml) if the container image needs additional tools, and keep it committed in git.

---

## 3. Project Overview & Quick Orientation

Akku is an expansive Rust standard library alongside `std` — **one library crate (`akku`)** with an ultimate target of **zero external dependencies** (`native` code or audited, license-clean `fork`s replacing temporary `adapter` dependencies).

Key docs to know:
- [`TODO.md`](TODO.md) — Current sprint priorities, verified open bugfixes, and remaining modules.
- [`Namespace.md`](Namespace.md) — Naming constitution (flat-by-default modules + dynamic families). Read **before** adding any public name.
- [`CONCEPT.md`](CONCEPT.md) — Architecture vision and zero-dep philosophy. Read **before** adding a new module or family.
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — Wave 1 / Wave 2 / Wave 3 milestone roadmap.
- [`docs/CATALOG.md`](docs/CATALOG.md) & [`docs/catalog.json`](docs/catalog.json) — Auto-generated module catalog (`akku catalog gen`).

Public workspace layout:
- `crates/akku/` — The single library crate (`#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`).
- `crates/akku-drive/` — Proc-macro implementation behind `akku::http` (internal detail; users import `akku`).
- `crates/akku-cli/` — Scaffolder and catalog generator (`akku new`, `akku catalog gen|check`).

---

## 4. Repository Structure (One Folder per Module)

Every library module in `akku` lives self-contained in its own folder under `crates/akku/src/<module>/` (or `crates/akku/src/<family>/<module>/`), including its source files, its public-API integration tests (`tests.rs`, registered via `[[test]]` in [`crates/akku/Cargo.toml`](crates/akku/Cargo.toml)), and its `meta.toml`:

```text
crates/akku/src/<module>/
  mod.rs          # public module entry point
  ...             # optional internal submodules
  tests.rs        # integration tests for the public API ([[test]] target)
  meta.toml       # module metadata (name, status, wasm32, wave, summary, owners, tags)
```

Do **not** place module tests in a separate top-level `crates/akku/tests/` directory.

`changie` fragments live in `.changes/unreleased/`. Releases use the version in `[workspace.package]` and `release.yml` with crates.io Trusted Publishing.

---

## 5. Project Skills (`.agents/skills/`)

Reusable task playbooks live in `.agents/skills/`:

- [`.agents/skills/akku-module-impl/SKILL.md`](.agents/skills/akku-module-impl/SKILL.md) — Worker guide for implementing or fixing `akku` modules.
- [`.agents/skills/akku-security-audit/SKILL.md`](.agents/skills/akku-security-audit/SKILL.md) — Checker guide for read-only security, correctness, and speed audits.
- [`.agents/skills/akku-smd-workflow/SKILL.md`](.agents/skills/akku-smd-workflow/SKILL.md) — `smd` container command cheatsheet for testing, linting, docs, and benchmarks.

---

## 6. Hard Engineering Rules

1. **Always run every build/test/lint command through `smd`.** Never bypass `smd` on the host.
2. **One folder per module** (`crates/akku/src/<module>/`), with `tests.rs` and `meta.toml` inside that module folder.
3. **No global names.** Use `akku::http::get` and `akku::json::Value`, never `akku::get` or `akku::Value`.
4. **Keep media & markup domains separated:** `akku::html` is separate from `akku::http`; `audio`, `vid`, and `pic` stay separate.
5. **No Rust keywords as module names** (`akku::task`, never `akku::async` or `akku::gen`). Avoid shadowing `::core`.
6. **Macros are sugar** over a pleasant manual API. Never make a macro the only way to use a feature.
7. **Zero-dep default (`#![forbid(unsafe_code)]`).** External dependencies are temporary `adapter` speedups: pinned, reviewed, optional behind feature flags, and hidden behind Akku's own API. Forks must be MPL-2.0-compatible (no GPL).
8. **Crypto and TLS are never blindly reimplemented from scratch;** use audited forks or pinned adapters. Never add an `akku::ssl` module (use `akku::tls` with `#[doc(alias = "ssl")]`).
9. **`wasm32` target support** lives via `#[cfg(target_arch = "wasm32")]` inside client/compute modules (never `forwasm::`); `akku::wasm` is host/guest runtime only.
10. **Every public item has rustdoc;** `#![deny(missing_docs)]` enforces it. Keep code, comments, and docs in English.
11. **Every module has real adversarial tests** (bounds, DoS limits, malformed input, round-trips) inside its `tests.rs`.
12. **`main` is released;** do not publish from a dirty tree without a release PR.
