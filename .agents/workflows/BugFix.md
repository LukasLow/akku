# BugFix

## Purpose
Fix a confirmed defect: reproduce (red) → root cause → minimal fix → regression test green → review.

## Inputs
- The report: observed vs expected behaviour, trigger input, or an audit finding (`file:line` + trigger).
- The affected module's `tests.rs`, rustdoc, and `_dev/` docs.

## Preconditions
- One defect or one batch of findings in one module per Worker.
- No code changes before a red reproduction exists.

## Roles
- **Orcha:** scopes the batch, opens the branch `fix/<module>-<topic>`, delegates.
- **Worker:** reproduces, fixes, verifies.
- **Checker:** reviews and commits via [PullRequest](PullRequest.md).

## Steps
1. **Red.** Worker writes a test at the public seam in `tests.rs` that encodes the *expected* behaviour and runs it: `smd cargo test -p akku --test <module> <name>`. Done when it fails for the reported reason. If it passes, report "not reproduced" with the test, and stop.
2. **Root cause.** Name the cause in one sentence (`file:line`, mechanism). If it is unclear, run the diagnosis loop below first.
3. **Fix.** Change the minimal code that removes the cause; keep the public API and unrelated code unchanged.
4. **Green.** Run the module smd gate ([akku-module-impl](../skills/akku-module-impl/SKILL.md) §6). Done when the new test and all existing tests pass.
5. **Record.** Rustdoc updated if documented behaviour changed; `_dev/AUDIT.md` row set to `fixed` if it came from an audit; changie fragment (`kind: Fixed` or `Security`).
6. **Report** with the root-cause sentence and a `COMMIT REQUEST`.

## Diagnosis loop (unclear bugs)
Adapted from [mattpocock/skills `diagnosing-bugs`](https://github.com/mattpocock/skills/blob/main/skills/engineering/diagnosing-bugs/SKILL.md) (MIT).
1. **Loop:** build one command that goes red on *this* symptom: a failing test, a fixture-driven CLI run, a byte-by-byte replay, a property loop over random inputs, or a differential run against a reference tool. Done when you have run it, it is deterministic, and it takes seconds.
2. **Minimise:** cut input and setup one piece at a time while it stays red. Done when every remaining element is load-bearing.
3. **Hypothesise:** write 3–5 ranked, falsifiable hypotheses ("if X, then changing Y makes it green").
4. **Probe:** test one variable at a time; tag temporary logs `[DEBUG-<id>]` and grep them out afterwards.
5. The minimised repro becomes the regression test of step 1.

## Artifacts
- Regression test, minimal fix, root-cause sentence in the commit body and PR Evidence.

## Review Gate
Checker verifies: the test was red before the fix (Worker shows the failing run), the root cause is fixed rather than the symptom, the full module suite is green, no scope creep, no `[DEBUG-` leftovers.

## Handoff
[PullRequest](PullRequest.md). A finding with no reachable public seam is itself reported to Orcha as an architecture finding.
