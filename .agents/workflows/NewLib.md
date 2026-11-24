# NewLib — add a module in 6 gates

## Purpose
Take a new `akku::<module>` from the human's request to a merged, audited module. Each gate ends with a Checker review; nothing advances past a failed gate.

## Inputs
- The human's request: "open a PR: add X because Y".
- [`Namespace.md`](../../Namespace.md), [`CONCEPT.md`](../../CONCEPT.md), [`docs/ROADMAP.md`](../../docs/ROADMAP.md).

## Preconditions
- The module does not exist yet under `crates/akku/src/`.
- The human named the reason ("because Y"). Without a reason, Orcha asks for one.

## Roles
- **Human:** approves the proposal (G1) and the public API (G2).
- **Orcha:** runs the gates, delegates, integrates, merges after G6.
- **Worker:** writes the artifacts of each gate.
- **Checker:** reviews each gate and commits it via [PullRequest](PullRequest.md).
Reuse one Worker and one Checker session for the whole module (resume instead of respawning); a fresh Checker is required only for the final commit gate of G6.

## Branches
- G1: branch `propose/<module>` → proposal PR, merged by the human.
- G2–G6: branch `lib/<module>` → one draft PR, opened at G2, marked ready at G6. Each gate is one commit on it.

## Steps

**G1 Intake.** Worker writes `crates/akku/src/<module>/_dev/DESIGN.md` §Purpose, §Non-goals, §Namespace (checked against Namespace.md, flat first), §Equivalents (Go stdlib, Rust std), §Dependencies (native/fork/adapter), and the Namespace.md entry. The Checker reviews it and opens the proposal PR per [PullRequest](PullRequest.md). Done when the human merged it.

**G2 Research + API.** Worker fills §References with sourced prior art (Go stdlib, Rust std + major crates, Python, C where relevant); every claim carries a URL or `repo/path:line`, unsourced claims start with `GUESS:`. Then §Public API (signatures, errors, DoS limits with defaults), §API rationale, §Seams, and `_dev/TODO.md` with deferred candidates. Done when the Checker passed it and the human approved the API in the draft PR.

**G3 Tests first.** Worker scaffolds the module per [akku-module-layout](../skills/akku-module-layout/SKILL.md) (stubs with rustdoc) and writes `tests.rs` per [akku-tdd](../skills/akku-tdd/SKILL.md): every seam, every applicable adversarial class. Done when the tests compile against the stubs (or fail to compile only on missing bodies) and are red; the Checker confirms they are non-tautological.

**G4 Implementation.** Worker implements per [akku-module-impl](../skills/akku-module-impl/SKILL.md) until green. Done when the module smd gate passes and no test was weakened (Checker diffs `tests.rs` against G3).

**G5 Audit.** Checker audits the whole module per [akku-security-audit](../skills/akku-security-audit/SKILL.md); findings go to `_dev/AUDIT.md`. Worker fixes every CRITICAL and MEDIUM with a regression test. Done when a re-audit reports PASS.

**G6 PR + merge.** Orcha wires `lib.rs` (if not done at G3), runs `catalog gen`, updates [`TODO.md`](../../TODO.md) and `docs/ROADMAP.md`, adds the changie fragment (`kind: Added`). A fresh Checker runs the commit gate, marks the PR ready (`gh pr ready <n>`), and approves. Orcha merges per [PullRequest](PullRequest.md).

## Artifacts
- `_dev/DESIGN.md`, `_dev/TODO.md`, `_dev/AUDIT.md`; module source, `tests.rs`, `meta.toml`; Namespace.md entry; catalog, TODO, ROADMAP updates; changie fragment.

## Review Gate
Each gate: the Checker's verdict is PASS and recorded in the PR (comment `G<n> PASS <sha>`). G1 and G2 additionally need the human's approval.

## Handoff
Merged → the module ships in the next [Release](Release.md). A gate that uncovers a flaw in an earlier gate returns to that gate on the same branch.
