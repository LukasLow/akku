# Audit

## Purpose
Find verified bugs in existing code, either deep in one module or as a cheap scan across many, and turn them into tracked work.

## Inputs
- Scope: one module, a diff, or "all modules".
- [akku-security-audit](../skills/akku-security-audit/SKILL.md) (checklist and output format).

## Preconditions
- The code in scope builds in `smd`.
- Audits are read-only; fixes run later through [BugFix](BugFix.md).

## Roles
- **Orcha:** sets the scope and model, collects reports, files findings.
- **Checker:** audits (read-only).
- **Worker:** later fixes, per BugFix.

## Mode A — single module
1. Orcha starts one Checker on `crates/akku/src/<module>/` with the audit skill.
2. Checker applies every checklist item to every file and reports the table, scores, and top-3 regression tests. Done when every file in the folder is covered.
3. Orcha starts a second Checker to verify each CRITICAL/MEDIUM against the code (cross-audit); unverified findings are dropped or marked `false positive`.

## Mode B — fan-out scan
1. Orcha starts one cheaper-model Checker (Haiku/Sonnet) per module or per small group of modules, with the audit skill plus a gap list: missing `_dev/`, missing adversarial test classes, missing rustdoc examples, `meta.toml` drift.
2. Each scanner reports findings and gaps in the audit format. Done when every module in scope has a report.
3. Orcha verifies every CRITICAL/MEDIUM with a stronger-model Checker before filing it (scanners over-report).

## Filing
- Module has `_dev/`: append a dated entry to `_dev/AUDIT.md` ([akku-module-layout](../skills/akku-module-layout/SKILL.md) §AUDIT.md), status `open`.
- Always: one line per verified finding in [`TODO.md`](../../TODO.md) under the module's section, with `file:line` and trigger.
- Gap reports from Mode B feed [ModuleMigration](ModuleMigration.md).

## Artifacts
- Audit reports, `_dev/AUDIT.md` entries, TODO.md lines. Filing is a docs change landed via [PullRequest](PullRequest.md) (`skip-changelog`).

## Review Gate
Every filed finding has `file:line`, a mechanism, a trigger, and was verified by a second Checker.

## Handoff
[BugFix](BugFix.md) for findings, batched per module; [ModuleMigration](ModuleMigration.md) for gaps.
