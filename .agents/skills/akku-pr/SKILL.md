---
name: akku-pr
description: PR title and body for akku (Summary / Evidence / Merge Danger) plus the changie fragment rule. Use when a Checker opens or updates a pull request, or when deciding whether a change needs a changelog fragment.
metadata:
  credits:
    skill: pr
    author: Matt Pocock
    license: MIT
    url: "https://github.com/mattpocock/skills/blob/main/skills/engineering/pr/SKILL.md"
---

# Pull request body

Title = the commit message subject (Conventional Commits, English): `feat(json): add streaming pointer lookup`.

```markdown
## Summary

<smallest view that makes the point: file-tree diff, call tree, API signature diff, or pseudocode>

## Evidence

- **Before:** <failing test / output / missing item>
  **After:** <passing test run, smd gate output (trimmed)>

## Merge Danger

**Door:** <one-way | two-way>
**Blast Radius:** <one word, e.g. none / docs / module / public-API / release>

<optional: what could break for users or other modules>

## Checker

<verdict + head SHA, filled by the Checker>

🤖 Generated with [Claude Code](https://claude.com/claude-code)
```

## Sections

Skip preambles; keep prose brief; use [GLOSSARY](../../GLOSSARY.md) terms.

- **Summary:** pick one view. A new public API → the signatures. A layout change → a file-tree `diff`. A bugfix → the state/flow `diff` that changed.
- **Evidence:** execution-based. Name the exact test that was red and is now green, and the gate commands that passed.
- **Merge Danger:** a **one-way door** is hard to reverse (published API, release, data format, deleted files); a **two-way door** reverts cleanly. Public API on crates.io is one-way once released.

## Changie fragment

CI (`check-changes.sh`) fails a PR without a fragment in `.changes/unreleased/` unless the PR has the `skip-changelog` label (or is a `release/*` branch).

- **User-facing** (new/changed/removed public API, behaviour fix, security fix): the Worker adds `.changes/unreleased/<yyyy-mm-dd>-<slug>.yaml` and lists it in the commit request:
  ```yaml
  kind: Added        # Added | Changed | Deprecated | Removed | Fixed | Security
  body: One English sentence a user understands.
  time: 2026-11-24T00:00:00Z
  ```
  Leave out the custom `version:` key; release versions are always set explicitly (see [Release](../../workflows/Release.md)).
- **Not user-facing** (docs, governance, CI, tests only, internal refactor): the Checker adds the label: `gh pr edit <n> --add-label skip-changelog`.
