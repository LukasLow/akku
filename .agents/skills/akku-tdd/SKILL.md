---
name: akku-tdd
description: Test-first red→green loop at an akku module's public API, with adversarial cases. Use when writing tests.rs for a new module (NewLib G3), adding a regression test for a bug, or judging whether existing tests are worth keeping.
metadata:
  credits:
    skill: tdd
    author: Matt Pocock
    license: MIT
    url: "https://github.com/mattpocock/skills/blob/main/skills/engineering/tdd/SKILL.md"
---

# Test-first at the public API

Read [`.agents/GLOSSARY.md`](../../GLOSSARY.md) so test names use project vocabulary.

## Seams

A **seam** is the public boundary a test observes: `akku::<module>::…` items as a user calls them. Tests in `tests.rs` use only the public API; that is what makes them survive refactors.

- List the seams in `_dev/DESIGN.md` §Seams before writing tests, one line each: what it catches, what it misses.
- In NewLib the seams are approved with the API at G2. For a bugfix, the seam is the public call that reproduces the report.
- Internal helpers get `#[cfg(test)]` unit tests only when a seam cannot reach the behaviour; name the reason in a comment.

## What a good test is

- Reads like a spec: `rejects_nesting_beyond_max_depth`, `round_trips_strings_with_single_quotes`.
- Expected values come from an independent source: an RFC example, a reference implementation's output, a hand-worked literal.
- Asserts the specific outcome (`Err(Error::DepthLimit { .. })`, exact bytes), not "did not panic".

## Anti-patterns

- **Tautological:** the expected value is computed the way the code computes it, so the test passes by construction.
- **Implementation-coupled:** reaches into private state or breaks on a refactor that keeps behaviour.
- **Horizontal slicing:** all tests first, then all code. Work in vertical slices instead: one test → minimal code → next.

## Adversarial set (every module)

Cover each class that applies to the module; the bug classes are listed in [`akku-security-audit`](../akku-security-audit/SKILL.md) §Checklist.

- **Bounds:** empty input, one byte, max length, length fields at `0`, `u32::MAX`, `usize::MAX`.
- **Malformed:** truncated at every boundary, invalid UTF-8, wrong magic, duplicate keys, bad escapes.
- **DoS limits:** each `max_*` limit exactly at and one past the default; deep nesting (`"[".repeat(100_000)`), expansion bombs; the result is a typed error, fast.
- **Round-trips:** `parse(serialize(x)) == x` for tricky values (quotes, markers, CRLF, NUL, non-ASCII).
- **Streaming:** the same input fed whole, byte-by-byte, and split at every chunk boundary gives the same result.
- **Interop:** where a reference tool exists in the smd image, compare against its real output.

## Loop

1. **Red:** write one test at a seam; run `smd cargo test -p akku --test <module> <name>`; it fails for the expected reason (a compile error for a missing item counts at G3).
2. **Green:** write the minimal code that passes it.
3. Repeat per slice. Refactoring happens after green, under review, never inside the loop.

Done when every seam in §Seams has tests, every applicable adversarial class is covered, and the module's test target is green in `smd`.
