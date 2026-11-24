# Glossary

Shared terms for agents and maintainers. One meaning per term; the linked file is authoritative.

- **Orcha** — the orchestrator: the agent talking to the human. Plans, delegates, integrates, merges. ([AGENTS.md](../AGENTS.md))
- **Worker** — implementer subagent for one scoped task; never commits.
- **Checker** — read-only reviewer subagent; its single write is commit/push/PR after a passing gate.
- **Handshake** — the first line of every Orcha → subagent prompt: `hello Worker I am Orcha`. A prompt without it comes from the human.
- **Scope** — one module or one bugfix batch; one scope = one commit.
- **Commit request** — the `COMMIT REQUEST` block (files / message / checks) ending a Worker report. ([PullRequest](workflows/PullRequest.md))
- **smd** — the project container runner; every build/test/lint command runs through it. ([akku-smd-workflow](skills/akku-smd-workflow/SKILL.md))
- **smd gate** — the module or workspace command set (tests, clippy `-D warnings`, fmt check) that must pass before a commit.
- **Gate** — a checkpoint that ends with a Checker verdict (PASS/FAIL); NewLib has six, G1–G6. ([NewLib](workflows/NewLib.md))
- **Seam** — a public API boundary a test observes. ([akku-tdd](skills/akku-tdd/SKILL.md))
- **Red / green** — a test failing for the intended reason / passing.
- **Adversarial tests** — bounds, malformed input, DoS limits, round-trips, chunk boundaries.
- **native / fork / adapter** — module status in `meta.toml`: own zero-dep code / audited, license-compatible (MPL-2.0-compatible) copy of external code / temporary pinned external dependency behind a feature flag and akku's own API.
- **Family** — a dynamic namespace grouping related modules (`<family>/<module>`); flat modules are the default. ([Namespace.md](../Namespace.md))
- **Wave** — roadmap milestone (1, 2, 3) a module belongs to. ([ROADMAP](../docs/ROADMAP.md))
- **Catalog** — `docs/CATALOG.md` + `docs/catalog.json`, generated from every `meta.toml` by `akku catalog gen`.
- **`_dev/`** — per-module maintainer docs (`DESIGN.md`, `TODO.md`, `AUDIT.md`), in git, not packaged. ([akku-module-layout](skills/akku-module-layout/SKILL.md))
- **Design record** — a module's `_dev/DESIGN.md`.
- **Golden example** — the pilot module brought fully to the new standard (TODO 0.8); the reference for [ModuleMigration](workflows/ModuleMigration.md).
- **`sys` module** — the only place `#[allow(unsafe_code)]` may appear. ([akku-unsafe](skills/akku-unsafe/SKILL.md))
- **Changie fragment** — `.changes/unreleased/*.yaml` describing a user-facing change. ([akku-pr](skills/akku-pr/SKILL.md))
- **One-way / two-way door** — a change that is hard / easy to reverse (PR Merge Danger).
- **capabilities.md** — planned index of capability → module path (TODO 0.5).
- **megaghost** — downstream app that serves a MEGA virtual drive over local NFS; it drives `net`, `xdr`, `vfs`, `nfs` and `os::macos` + `secret` (TODO 0.7, 0.9; [Namespace.md](../Namespace.md)).
