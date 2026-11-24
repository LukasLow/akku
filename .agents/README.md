# `.agents/` index

Pick the line matching your situation and open that file. Terms: [GLOSSARY.md](GLOSSARY.md).

## Workflows (what to do, in order)

- [workflows/NewLib.md](workflows/NewLib.md) — human asks to add a module ("open a PR: add X because Y"): gates G1–G6.
- [workflows/BugFix.md](workflows/BugFix.md) — fixing a reported bug or verified audit finding; diagnosis loop for unclear bugs.
- [workflows/Audit.md](workflows/Audit.md) — auditing one module deeply, or scanning all modules with cheaper models.
- [workflows/PullRequest.md](workflows/PullRequest.md) — any change reaching `main`: commit request → Checker → PR → merge.
- [workflows/Release.md](workflows/Release.md) — publishing a `0.0.N` release.
- [workflows/ModuleMigration.md](workflows/ModuleMigration.md) — bringing an existing module up to the golden-example standard.

## Skills (how to do one thing well)

- [skills/akku-module-impl](skills/akku-module-impl/SKILL.md) — Worker: implementing or fixing a module.
- [skills/akku-module-layout](skills/akku-module-layout/SKILL.md) — module folder, wiring, `meta.toml`, `_dev/` docs, rustdoc vs `_dev/`.
- [skills/akku-tdd](skills/akku-tdd/SKILL.md) — writing tests: red→green at the public API, adversarial cases.
- [skills/akku-security-audit](skills/akku-security-audit/SKILL.md) — Checker: auditing code or gating a commit request.
- [skills/akku-unsafe](skills/akku-unsafe/SKILL.md) — code that seems to need `unsafe`, or a `sys` module.
- [skills/akku-pr](skills/akku-pr/SKILL.md) — writing a PR body; deciding on a changie fragment.
- [skills/akku-smd-workflow](skills/akku-smd-workflow/SKILL.md) — running any cargo/build/test/lint command.

## Session hygiene

- Start at most one Worker and one Checker per task and resume them when the role is needed again; parallel sessions only for independent work (e.g. a fan-out audit). The commit gate uses a fresh Checker.
- Start a fresh Orcha session after a wave or a major sprint.
