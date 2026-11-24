# TODO.md — akku

> **Current Snapshot:** **37 implemented modules** (+ `akku::diff` re-export = **38 / 77** entries from [`Namespace.md`](Namespace.md), **~84,400 LOC** pure Rust, `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`).
>
> - **Wave 1:** 25 / 25 implemented (24 native zero-dep + `http` adapter)
> - **Wave 2:** 11 / 17 implemented (`ws`, `sse`, `template`, `markdown`, `xml`, `yaml`, `text`+`diff`, `regex`, `archive`, `kv`, `cache`) — **Audit Bugfix Pass ✅ done (2026-11-24)**
> - **Wave 3:** 1 / ~35 implemented (`algo`)

---

## Priority 0 — Governance & Repository Restructure (agreed 2026-11-24, in progress)

> Master checklist from the human/Orcha planning session. Nothing here is started unless marked. Repo content is **English**.

### 0.1 Alignment
- [ ] Purpose statement ("Why akku?") confirmed by the human → becomes the README intro (English).
- [x] OS layer design approved (see 0.7): Rust `std::os` names (`macos`, not `darwin`); unified APIs as flat modules (no `unios`). Platform code stays inside `akku` (no extra crates, see 0.7).
- [x] Decided: **agents merge** PRs once CI is green and a Checker approved.

### 0.2 Land the current state (PR-only from now on)
- [x] (PR #3 merged as `5626535`: https://github.com/LukasLow/akku/pull/3) Create a feature branch and land the uncommitted work (37 modules, Wave-2 bugfix pass, `flake.nix`, AGENTS commit workflow) through PRs via the Commit Workflow (one commit per module/batch).
- [x] `main` protected (2026-11-24): PR required (0 approvals so agents can merge), required check `test`, enforced for admins, no force-push/deletion. The release workflow only reacts to merges and creates tags via API, so it is unaffected.

### 0.3 Governance system (`.agents/`)
- [ ] Study `~/home/repos/mojoakku` (skills, workflows, per-lib docs) and Matt Pocock's public skills; adopt what fits.
- [ ] `.agents/skills/` — expand into a full skill set (module impl, audit, smd, docs, release, PR, scan, …).
- [ ] `.agents/workflows/` — at least: **new-lib intake** (human says "open a PR: add X because Y" → proposal PR → Worker/Checker PRs), bugfix, audit, commit/PR, release, module migration.
- [ ] Rewrite the Commit Workflow for PR-only (Checker commits on a branch and opens or updates the PR; nothing lands on `main` directly).
- [ ] AGENTS.md = everything maintainers/agents need, with details linked into `.agents/`. README = everything newcomers need.
- [ ] Translate German docs to English: `CONCEPT.md`, `Namespace.md`, and the remaining sections of `docs/`. (Open: keep `hallo Worker ich bin Orcha` as a fixed handshake token?)

### 0.4 Policies
- [ ] **unsafe policy:** allowed **only** where unavoidable (FFI, OS syscalls/sockets, platform APIs), never for convenience or speed. Mechanics: crate-level `#![deny(unsafe_code)]`, `#[allow(unsafe_code)]` only in designated `sys` submodules, every block has a `// SAFETY:` comment (`clippy::undocumented_unsafe_blocks`), CI enforces an allowlist of files. Update AGENTS.md, CONCEPT.md and the skills.
- [ ] Namespace: flat-first stays; dissolve the planned `net` family → flat `net` (std::net superset), `dns`, `quic`; add `xdr`, `vfs`, `nfs` (`nfs::server`/`client`; ONC-RPC internal for now).
- [ ] Only the 3 crates on crates.io (`akku`, `akku-drive`, `akku-cli`); everything else stays inside `akku`.

### 0.5 Docs structure
- [ ] `capabilities.md` — one `#### <Capability>` entry per capability with a short text and its path (e.g. `#### TCP` → `akku::net`).
- [ ] Per-module docs live in the module folder (notes, decisions, test strategy, audit log), following the mojoakku pattern; only cross-cutting docs live in `docs/`.
- [ ] `docs/go-stdlib-gap.md` — every Go stdlib package marked as "in Rust std" / "in akku" / "missing → wave X"; missing items go into the ROADMAP.
- [ ] README rewrite (English) and GitHub Pages site using `~/home/repos/designspace`.

### 0.6 CI (`.github/`)
- [ ] Gates on every PR: fmt, clippy `-D warnings`, tests (all features and no default features), rustdoc `-Dwarnings`, `catalog check`, unsafe allowlist, wasm32 check (`--no-default-features`).
- [ ] **Release guard:** fail any release/version bump outside `0.0.x` (stay on `0.0.x` until at least 2027-Q2). Note: `.changie.yaml` has `Added/Changed/…: auto: minor`, so `changie batch auto` would produce `0.1.0`; change it to `patch` or forbid `auto`.
- [ ] macOS / Linux / Windows runners (needed for the OS layer).

### 0.7 OS layer (design first, as an RFC PR)
- [ ] RFC: platform modules `akku::os::{unix, linux, macos, ios, android, windows}` (superset of `std::os`, cfg-gated); unified APIs stay flat domain modules (`clipboard`, `notify`, `secret`, `power`, …; media capture via `audio::device` / `vid::device`); errors `Unsupported` / `NotAvailable` / `PermissionDenied`; a per-platform support matrix in `meta.toml` → catalog/capabilities.
- [x] Decision (2026-11-24): **no `akku-<platform>` crates.** Platform code lives in `akku` as `#[cfg(target_os = …)]` modules under `akku::os::<platform>`; platform-only deps go in `[target.'cfg(…)'.dependencies]`. Reason: a crate that published `akku` depends on must itself be on crates.io, and we keep exactly 3 published crates (this also avoids name squatting). Splitting a module into its own crate later stays possible without changing the public path.
- [ ] First slice driven by megaghost: `os::macos` + `secret` (Keychain), if the RFC is accepted.

### 0.8 Quality migration of existing modules
- [ ] Pilot: bring one small module fully to the new standard as the golden example.
- [ ] Fan-out scan with cheaper models (Haiku/Sonnet) over all modules → gap report per module.
- [ ] Migration PRs batch-wise until all modules are on the same level.

### 0.9 megaghost-driven libs (after 0.2–0.4)
- [ ] `net` and `xdr` (parallel) → `vfs` → `nfs::server` (read-only first).

---

## Priority 1 — Wave-2 Audit Bugfix Pass ✅ (completed 2026-11-24)

All findings below were verified line-by-line during read-only cross-audits. Fix every **CRITICAL** (plus key **MEDIUM** items) with regression tests in each module's `tests.rs`, then run a Checker verification pass.

- [x] **CRITICAL** [`crates/akku/src/archive/mod.rs:217`](crates/akku/src/archive/mod.rs#L217) — Off-by-one symlink target validation allows escaping destination root (`../outside`).
- [x] **CRITICAL** [`crates/akku/src/archive/tar.rs:386`](crates/akku/src/archive/tar.rs#L386) — Hardlink extraction via symlink can overwrite arbitrary host files outside destination.
- [x] **CRITICAL** [`crates/akku/src/archive/tar.rs:913`](crates/akku/src/archive/tar.rs#L913) — PAX header `pos + len` integer overflow panic on malformed input.
- [x] **CRITICAL** [`crates/akku/src/archive/zip.rs:483`](crates/akku/src/archive/zip.rs#L483) — Central directory `cd.len() - pos` integer underflow panic on malformed input.
- [x] **CRITICAL** [`crates/akku/src/archive/zip.rs:350`](crates/akku/src/archive/zip.rs#L350) & [`:389`](crates/akku/src/archive/zip.rs#L389) — Pre-validation allocation up to 4 GiB enables zip-bomb OOM before decompression limit checks.
- [x] **CRITICAL** [`crates/akku/src/archive/deflate.rs:700`](crates/akku/src/archive/deflate.rs#L700) & [`crates/akku/src/archive/gzip.rs:739`](crates/akku/src/archive/gzip.rs#L739) — Streaming decoder hangs or emits spurious errors on chunked streams (>32 KiB / split boundaries); also cap unbounded Gzip `FNAME` header allocation.

### 1.2 `yaml` (4 Criticals)
- [x] **CRITICAL** [`crates/akku/src/yaml/parser.rs:485`](crates/akku/src/yaml/parser.rs#L485) & [`:494`](crates/akku/src/yaml/parser.rs#L494) — `max_depth` bypass allows unbounded recursion stack overflow (trigger: `"- ".repeat(200_000)`).
- [x] **CRITICAL** [`crates/akku/src/yaml/parser.rs:1440`](crates/akku/src/yaml/parser.rs#L1440) — Nested anchor cloning bomb (~4 GB RAM) bypasses alias expansion cap without explicit alias count growth.
- [x] **CRITICAL** [`crates/akku/src/yaml/value.rs:389`](crates/akku/src/yaml/value.rs#L389) — Recursive `Drop` on deeply nested mapping keys causes stack overflow; convert to iterative drop.
- [x] **CRITICAL** [`crates/akku/src/yaml/ser.rs`](crates/akku/src/yaml/ser.rs) — Serializer round-trip corruption on strings containing single quotes (`{"it's": 1}`) and YAML document markers (`"---"`).

### 1.3 `template` & `markdown` (4 Criticals)
- [x] **CRITICAL** [`crates/akku/src/template/parser.rs:502`](crates/akku/src/template/parser.rs#L502) — Deterministic panic on input `{% raw %}{%}`.
- [x] **CRITICAL** [`crates/akku/src/template/filters.rs:130`](crates/akku/src/template/filters.rs#L130) — `replace` filter can allocate ~10 GB OOM before `output_len` cap is checked; also verify `super()` stack/depth guard.
- [x] **CRITICAL** [`crates/akku/src/markdown/render.rs:436`](crates/akku/src/markdown/render.rs#L436) — Form-feed `\x0c` bypasses GFM raw-HTML tag filter (XSS).
- [x] **CRITICAL** [`crates/akku/src/markdown/autolink.rs:47`](crates/akku/src/markdown/autolink.rs#L47) — Bare-domain autolink swallows trailing space.

### 1.4 `kv` (3 Criticals)
- [x] **CRITICAL** [`crates/akku/src/kv/file.rs:331`](crates/akku/src/kv/file.rs#L331) — Zero-filled crash tail in WAL causes permanent open failure instead of clean tail recovery.
- [x] **CRITICAL** [`crates/akku/src/kv/file.rs:338`](crates/akku/src/kv/file.rs#L338) — Corrupted `val_len` silently truncates and drops subsequent valid records while returning `Ok`.
- [x] **CRITICAL** [`crates/akku/src/kv/redis.rs:469`](crates/akku/src/kv/redis.rs#L469) — Connection desync after partial read/error can return another key's value on subsequent commands.

### 1.5 `regex` & `algo` (2 Criticals + 2 Test Lints)
- [x] **CRITICAL** [`crates/akku/src/regex/pikevm.rs:234`](crates/akku/src/regex/pikevm.rs#L234) — Early `break` on empty `clist` breaks anchored/boundary matches like `\bfoo`, `$`, `\B` in `find`. — *Verified false positive (differential test added; defensive guard kept).*
- [x] **CRITICAL** [`crates/akku/src/regex/mod.rs:675`](crates/akku/src/regex/mod.rs#L675) — Unchecked `i * 2` overflow in replacement group parsing leaks capture group 0 (trigger: `$9223372036854775808`).
- [x] **LINT** [`crates/akku/src/algo/tests.rs:648`](crates/akku/src/algo/tests.rs#L648) & [`:704`](crates/akku/src/algo/tests.rs#L704) — Fix 2 `clippy::reversed_empty_ranges` test warnings so `--all-targets -- -D warnings` is 100% green.

### 1.6 High-Value Medium Fixes (`ws`, `sse`, `xml`, `text`/`diff`)
- [x] **MEDIUM** [`crates/akku/src/ws/sha1.rs:103`](crates/akku/src/ws/sha1.rs#L103) — Avoid opening `/dev/urandom` on every client frame; seed a proper CSPRNG/buffer without predictable silent fallback.
- [x] **MEDIUM** [`crates/akku/src/ws/frame.rs:335`](crates/akku/src/ws/frame.rs#L335) — Buffer frame header + payload into one write to eliminate TCP Nagle/delayed-ACK stalls.
- [x] **MEDIUM** [`crates/akku/src/sse/parser.rs:150`](crates/akku/src/sse/parser.rs#L150) — After `EventTooLarge`, skip remaining bytes until next line break (prevent field injection) and preserve already-completed events from earlier in the same chunk.
- [x] **MEDIUM** [`crates/akku/src/xml/`](crates/akku/src/xml/) — Escape/validate quotes in `XmlDecl`, buffer serializer writes (avoid 1-char `write_all`), optimize $O(n^2)$ namespace scan.
- [x] **MEDIUM** [`crates/akku/src/text/`](crates/akku/src/text/) — Replace $O(n^2)$ canonical bubble-sort, enforce diff budget *before* allocating Myers `vf`/`vb` vectors, fix CRLF patch mismatch.

### 1.7 Sprint Housekeeping (Orcha)
- [x] Run `smd cargo run -p akku-cli -- catalog gen` and `smd cargo run -p akku-cli -- catalog check` (updates `docs/CATALOG.md` & `docs/catalog.json` to all 37 modules).
- [x] Update checkboxes in [`docs/ROADMAP.md`](docs/ROADMAP.md) to reflect completed Wave-1/Wave-2/Wave-3 modules.
- [x] Verify full workspace green in `smd`: `test --workspace --all-features`, `test -p akku --no-default-features`, `clippy --workspace --all-targets --all-features -- -D warnings`, `doc --workspace --no-deps`, `fmt --all -- --check`.

### 1.8 Follow-ups from the Checker passes (all LOW, open)
- [ ] `kv/file.rs:~180-189,~749` — Double fault (mid-log bit flip + torn last record) is truncated silently instead of `CorruptData`; treat a chain that ends in a header-valid record extending past EOF as reaching the tail.
- [ ] `kv/file.rs:~926,951,969,1079` — Failing auto-compaction after a durable append returns `Err` (and `delete` loses `existed`); report compaction separately or document it.
- [ ] `kv/redis.rs` — Decoder still accepts RESP3 `_ # % ~` despite "RESP2-only"; tighten or adjust docs.
- [ ] `text/diff.rs:~419-424` — `apply_patch` silently skips `+`/`-`/space lines outside a hunk (and multi-file patches); return an error.
- [ ] `text/normalize.rs:~44-49` — `normalize(.., Nfc)` builds NFC twice for non-NFC input; use one pass / quick check.
- [ ] `yaml` — In debug builds the default `max_depth=128` uses ~1.5 MiB of stack (fine on 2 MiB threads, but tight); reduce per-level stack frames.
- [ ] CI: add a wasm32 job — `nix shell .# -c cargo check -p akku --no-default-features --target wasm32-unknown-unknown` (green locally; the `smd` container has no wasm32 target, see `flake.nix`). With default features (`http`/tokio/mio), wasm32 does not build.

---

## Priority 2 — Remaining Wave-2 Modules (6 Families / 8 Modules)

> Note: Check [`AGENTS.md`](AGENTS.md) rules #7–#8 before starting `crypto` or `tls` (never blindly roll custom crypto; use audited MPL-2.0-compatible forks or pinned optional adapters).

- [ ] `net` — `ip`, `socket`, `tcp`, `udp`, `uds`, `dns`, `quic`
- [ ] `task`, `sync`, `schedule` — async tasks, channels, semaphore, cron/retry/backoff (+ `wasm32` spawn)
- [ ] `crypto` — `hash`, `rand`, `aead`, `sign`, `kem`, `kdf` (audited only)
- [ ] `tls` — TLS config, streams, certificates (`#[doc(alias = "ssl")]`; never `akku::ssl`)
- [ ] `auth` — `password` (Argon2id), `jwt`, `oauth2`, `totp`, `webauthn`, `session`, `acl` (over `crypto`)
- [ ] `sql` — unified SQL API, pool, `migrate`, `sql::sqlite`, `sql::postgres`
- [ ] `http` server/client boundary expansion (`dree` compatibility without importing `axum`)

---

## Priority 3 — Wave-3 Backlog (~34 Modules)

- **Wire Formats (great standalone zero-dep Workers):** `cbor`, `msgpack`, `protobuf`, `flatbuffers`
- **Dev, Text & Helpers:** `color`, `i18n`, `tui`, `trace`, `bench`, `geo`
- **Media (separated domains):** `pic`, `audio`, `vid`, `pdf`
- **Concurrency & Distribution:** `actor`, `cluster`, `broker`
- **Protocols:** `webpush`, `rpc`, `mail`, `mqtt`, `ssh`
- **Science & Data:** `phy`, `dataframe`, `chem`, `bio`, `econ`
- **GPU, WASM & UI:** `gpu`, `wasm`, `ui`, `webview`
- **Milestone:** Native HTTP/1.1 core behind `akku::http` (benchmarked against `axum`)
