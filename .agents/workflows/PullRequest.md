# PullRequest — commit, push, PR, merge

Single source of truth for how any change reaches `main`. AGENTS.md only summarizes this file.

## Purpose
Land one Worker scope on `main` through a reviewed pull request. `main` is protected: a PR and green `test`, `os-test (macos-latest)`, and `os-test (windows-latest)` checks are required, force-push is disabled.

## Inputs
- A Worker report ending in a `COMMIT REQUEST`.
- The feature branch the Worker worked on.

## Preconditions
- Work happens on a feature branch, never on `main`. Branch names: `feat/<module>-<topic>`, `fix/<module>-<topic>`, `docs/<topic>`, `chore/<topic>`, `propose/<module>` and `lib/<module>` ([NewLib](NewLib.md)), `chore/migrate-<module>` ([ModuleMigration](ModuleMigration.md)), `release/0.0.N` ([Release](Release.md)).
- The Worker's scope is one module or one bugfix batch.

## Roles
- **Worker:** builds, tests, writes the commit request. Never runs `git commit`.
- **Orcha:** integrates (`lib.rs`, catalog, TODO), forwards the request to a fresh Checker, merges after approval.
- **Checker:** audits, reruns the gate, commits, pushes the branch, opens/updates the PR, approves.

## Steps

1. **Worker → commit request.** The Worker ends its report with:
   ```text
   COMMIT REQUEST
   files:   <exact list of paths in scope>
   message: <type(scope): summary>   # Conventional Commits, English
   checks:  <smd commands run + results>
   ```
   A user-facing change adds a changie fragment and lists it in `files` ([akku-pr](../skills/akku-pr/SKILL.md) §Changie).
2. **Orcha → integrate, then forward.** If the scope added a module or changed `meta.toml`, Orcha wires `lib.rs`/`Cargo.toml`, runs `smd cargo run -p akku-cli -- catalog gen`, ticks [`TODO.md`](../../TODO.md), and appends those paths to `files`. Orcha then forwards the request to a fresh Checker (prompt opens with `hello Worker I am Orcha`).
3. **Checker → audit.** Audit the diff of exactly the listed files with [akku-security-audit](../skills/akku-security-audit/SKILL.md). Done when every listed file is reviewed and the verdict is PASS.
4. **Checker → gate.** Rerun in `smd`: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` always; plus the touched modules' tests (`cargo test -p akku --test <module>`) when Rust code changed; plus the full workspace set from [akku-smd-workflow](../skills/akku-smd-workflow/SKILL.md) when shared code, `Cargo.toml` or `lib.rs` changed; plus `catalog check` if a `meta.toml` changed. A docs-only change needs only fmt and clippy. Confirm `git status` shows nothing outside `files` staged, and no secrets (`.env`, credentials, `.claude/settings.local.json`).
5. **On FAIL:** the Checker reports blocking findings (`file:line` + trigger) to Orcha and commits nothing. Orcha sends them to a Worker; restart at step 1.
6. **On PASS → commit and push** (host):
   ```sh
   git switch <feature-branch>          # never main
   git add <files>                      # exactly the listed files
   git commit -m "<message>" -m "Co-Authored-By: <agent trailer>"   # second -m = separate trailer paragraph
   git push -u origin <feature-branch>  # a normal push; history is never rewritten
   ```
7. **Open or update the PR.** New: `gh pr create --base main --title "<message>" --body-file <body>` with the body from [akku-pr](../skills/akku-pr/SKILL.md), written after the commit so it can name the head SHA. Existing: the push updates it; refresh the body when the summary changed (`gh pr edit <n> --body-file <body>`). Add `skip-changelog` for non-user-facing changes.
8. **Checker → approval.** `gh pr comment <n> --body "CHECKER APPROVED <head-sha>"` (GitHub blocks formal self-approval, so the comment is the approval record). Report the commit SHA and PR URL to Orcha.
9. **Orcha → merge.** `gh pr checks <n> --watch` until green; confirm the approval names the current head SHA; then `gh pr merge <n> --merge --delete-branch`. A red check goes back to a Worker ([BugFix](BugFix.md)).

## Rules
- One commit per Worker scope; a PR may carry several commits on its branch (e.g. NewLib gates).
- Pushes go to feature branches only; `main` changes only through a merge. Every push is fast-forward (no `--force`, no rebase of pushed commits).
- Releases happen only through [Release](Release.md); this workflow never tags or publishes.
- Agents merge once CI is green and a Checker approved. A G1 proposal PR and a G2 API approval belong to the human ([NewLib](NewLib.md)).

## Artifacts
- A commit on the feature branch, a PR with an akku-pr body, a `CHECKER APPROVED <sha>` comment, a merge commit on `main`.

## Review Gate
Mergeable when: `test`, `os-test (macos-latest)`, and `os-test (windows-latest)` are green, the Checker approval names the head SHA, and a changie fragment or the `skip-changelog` label is present.

## Handoff
Merged → Orcha reports the merge to the human and suggests a fresh session after a wave. Rejected → [BugFix](BugFix.md) or back to the Worker scope.
