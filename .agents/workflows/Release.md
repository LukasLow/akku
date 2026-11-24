# Release

## Purpose
Publish the three crates `akku`, `akku-drive`, `akku-cli` to crates.io at an explicit `0.0.N` version.

## Inputs
- `[workspace.package] version` in [`Cargo.toml`](../../Cargo.toml) (single source of the version).
- Fragments in `.changes/unreleased/`.
- [`.github/scripts/release.py`](../../.github/scripts/release.py), [`.github/workflows/release.yml`](../../.github/workflows/release.yml), [`.changie.yaml`](../../.changie.yaml).

## Preconditions
- The human asked for a release.
- `main` is green and every change to ship is merged.
- Version rule: stay on `0.0.x` until at least 2027-Q2. The next version is `0.0.(N+1)`, always passed explicitly.

## Roles
- **Orcha:** creates the branch, runs the prepare step (step 2) on the host via the flake, merges.
- **Checker:** reviews the release diff (version, changelog, notes), commits, and opens the PR per [PullRequest](PullRequest.md).
- **Human:** requests the release.

## Steps
1. **Branch:** `git switch -c release/0.0.N main`. (`release/*` branches skip the changie-fragment CI check.)
2. **Prepare:** `nix shell .#release -c python3 .github/scripts/release.py 0.0.N` (the flake's `release` env provides `python3`, `changie`, and the pinned `cargo`, which smd lacks). It sets the workspace and internal dependency versions, runs `cargo update --workspace`, `changie batch 0.0.N`, `changie merge`, and writes `releases/v0.0.N-<date>.md`. It does not commit, tag, or publish.
   - Always an explicit `0.0.N`. The release guard (`RELEASE_SERIES` in `release.py`) refuses `auto`/`minor`/`major` and any version outside `0.0.x`; `.changie.yaml` maps every kind to `patch`.
3. **Check:** `nix shell .#release -c python3 .github/scripts/release.py --check` reports "prepared"; `CHANGELOG.md` has `## 0.0.N - <date>`; `.changes/unreleased/` is empty; the workspace smd gate passes ([akku-smd-workflow](../skills/akku-smd-workflow/SKILL.md) §Workspace).
4. **PR:** Orcha writes the commit request (files: `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, `.changes/`, `releases/`) → Checker → PR titled `chore(release): 0.0.N` on branch `release/0.0.N` → merge per [PullRequest](PullRequest.md).
5. **Publish (automatic):** the merge changes `Cargo.toml` on `main`, which triggers `release.yml`. It skips (without error) if tag `v0.0.N` exists, crates.io already has the version, or `CHANGELOG.md` has no `## 0.0.N` section; it fails if the notes file is missing or unreleased fragments remain; then publishes `akku-drive` → `akku` → `akku-cli` via crates.io Trusted Publishing and creates the GitHub release and tag `v0.0.N`.
6. **Verify:** `gh run watch` on the release run; `gh release view v0.0.N`; the three crates show `0.0.N` on crates.io. Done when all three are visible.

## Rules
- Publishing happens only through `release.yml` on a merge to `main`; agents never run `cargo publish` or create tags by hand.
- A failed run is fixed on a new PR; re-running is safe because already-published crates are skipped.
- Release guard: `release.py --check` fails on a workspace version outside `0.0.x`; it runs in `ci.yml` (job `test`) and in `release.yml` before publishing. Lifting it after 2027-Q2 is the one-line change `RELEASE_SERIES = None` in `release.py`.

## Artifacts
- Release PR (version bump, `CHANGELOG.md`, `.changes/0.0.N.md`, `releases/v0.0.N-<date>.md`), tag `v0.0.N`, GitHub release, three crates on crates.io.

## Review Gate
Checker verifies: version is `0.0.(N+1)` of the previous release, every fragment appears in the CHANGELOG section, notes match the section, workspace gate is green.

## Handoff
Done → Orcha records the release in [`TODO.md`](../../TODO.md). A defect found during release → [BugFix](BugFix.md), then restart at step 1.
