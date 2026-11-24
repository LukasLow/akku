# ModuleMigration — bring an existing module to the golden-example standard

## Purpose
Raise one existing module to the standard the pilot ("golden example", TODO 0.8) defines: `_dev/` docs, adversarial tests, complete rustdoc, accurate `meta.toml`.

## Inputs
- The module folder `crates/akku/src/<module>/`.
- Its gap report from an [Audit](Audit.md) Mode B scan, if one exists.
- The golden-example module once the pilot has merged (until then, [akku-module-layout](../skills/akku-module-layout/SKILL.md) is the reference).

## Preconditions
- The module is green in `smd`.
- One module per Worker and per PR; branch `chore/migrate-<module>`.
- Behaviour stays the same; defects found on the way go through [BugFix](BugFix.md) in a separate scope.

## Roles
- **Orcha:** picks the module, delegates, integrates.
- **Worker:** migrates.
- **Checker:** reviews and commits via [PullRequest](PullRequest.md).

## Steps
1. **`_dev/DESIGN.md`:** reconstruct the design record from the code, rustdoc, git history, and Namespace.md: purpose, non-goals, equivalents, dependencies, public API with limits and defaults, seams. Mark reconstructed rationale `GUESS:` where no source confirms it. Done when every public item appears in §Public API.
2. **`_dev/TODO.md`:** list known missing APIs (Go/Rust equivalents not covered). An empty list is fine.
3. **`_dev/AUDIT.md`:** move this module's past findings from [`TODO.md`](../../TODO.md) Priority 1 into the log with their status.
4. **Tests:** compare `tests.rs` against [akku-tdd](../skills/akku-tdd/SKILL.md) §Adversarial set; add missing classes; rewrite tautological tests. A new test that fails is a bug → stop and report it for BugFix.
5. **Rustdoc:** every public item documented; module `//!` docs have a runnable example and list limits/defaults; move rationale out of rustdoc into DESIGN.md.
6. **`meta.toml`:** status, `wasm32`, summary, and tags match reality.
7. **Verify:** module smd gate plus `smd cargo package -p akku --list --allow-dirty` (no `_dev` path). Report with a `COMMIT REQUEST` (`skip-changelog` unless rustdoc changes are user-visible enough to note).

## Artifacts
- `_dev/DESIGN.md`, `_dev/TODO.md`, `_dev/AUDIT.md`, added tests, rustdoc, corrected `meta.toml`.

## Review Gate
Checker verifies: public API unchanged (`git diff` touches no signatures), every DESIGN.md §Public API entry exists in code, new tests are non-tautological and green, gate green.

## Handoff
[PullRequest](PullRequest.md); next module from the gap report.
