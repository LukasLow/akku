---
name: akku-module-layout
description: Canonical akku module folder (mod.rs, submodules, tests.rs, meta.toml, _dev/). Use when creating a module, adding files to one, writing meta.toml or _dev/ docs, or deciding where a piece of documentation belongs.
---

# Module layout (single source of truth)

Every other skill and workflow points here for layout. Change the layout here first.

## Folder

```text
crates/akku/src/<module>/            # or src/<family>/<module>/ for a dynamic family
  mod.rs        # public entry: `//!` module rustdoc (end-user docs) + re-exports
  <part>.rs     # internal submodules, one concern each (parser, value, ser, error, ...)
  sys/          # only when OS/FFI code needs `unsafe` (see akku-unsafe)
  tests.rs      # public-API integration tests ([[test]] target)
  meta.toml     # catalog metadata
  _dev/         # maintainer docs; in git, excluded from the crates.io package
    DESIGN.md   # design record
    TODO.md     # backlog of researched-but-unshipped APIs
    AUDIT.md    # audit log
```

- Tests live in `tests.rs` inside the module folder; there is no top-level `crates/akku/tests/`.
- `_dev/` is excluded from packaging by `"!src/**/_dev/**"` in `include` of [`crates/akku/Cargo.toml`](../../../crates/akku/Cargo.toml) (Cargo ignores `exclude` when `include` is set). Verify: `smd cargo package -p akku --list --allow-dirty` lists no `_dev` path.
- Modules without `_dev/` yet are migrated by [`ModuleMigration`](../../workflows/ModuleMigration.md).

## Wiring a new module

1. `pub mod <module>;` in alphabetical order in [`crates/akku/src/lib.rs`](../../../crates/akku/src/lib.rs).
2. Register the test target in [`crates/akku/Cargo.toml`](../../../crates/akku/Cargo.toml):
   ```toml
   [[test]]
   name = "<module>"
   path = "src/<module>/tests.rs"
   ```
3. Orcha runs `smd cargo run -p akku-cli -- catalog gen` (catalog check fails while `lib.rs` and `meta.toml` disagree).

## `meta.toml`

Flat TOML only (no tables, no dotted keys); validated by `akku catalog check`.

```toml
name = "<module>"
status = "native"   # native | fork | adapter (see GLOSSARY)
wasm32 = true       # usable on wasm32 without OS sockets/fs
wave = 2            # 1 | 2 | 3
summary = "One-line English summary."
owners = ["LukasLow"]
tags = ["<module>", "<domain>", "<category>"]
```

## Two readers, two places

| Reader | Lives in | Contains |
| --- | --- | --- |
| End user | rustdoc (`//!` in `mod.rs`, `///` on every public item) | what it is, how to call it, errors, limits/defaults, a runnable example |
| Maintainer | `_dev/` | why it is built this way, prior art, rejected options, open questions, audit history |
| Person editing a line | `//` comment beside the code | only a non-obvious decision; the code shows the *how* |

Rustdoc carries no rationale, status bookkeeping, or prior-art citations; those go to `_dev/DESIGN.md`.

## `_dev/DESIGN.md`

```markdown
# <module> — design record

## Purpose          <!-- why akku needs it; the human's "because Y" -->
## Non-goals        <!-- deliberately not built or not copied, with reason -->
## Namespace        <!-- path per Namespace.md (flat first), names checked for clashes -->
## Equivalents      <!-- Go stdlib package, Rust std item, major crates -->
## Dependencies     <!-- native | fork | adapter, with license and reason -->
## References       <!-- sourced prior art: Go, Rust std + crates, Python, C; URL or repo/path:line; unsourced = `GUESS:` -->
## Public API       <!-- signatures + one-line semantics; errors; DoS limits and defaults -->
## API rationale    <!-- "akku does X because Y (ref)" per decision -->
## Seams            <!-- public boundaries the tests target (see akku-tdd) -->
## Open questions
```

Update it when the API changes; it is kept for the life of the module.

## `_dev/TODO.md`

Live backlog. One line per candidate; delete a line when it ships (git is the history). An empty list is the expected end state.

```markdown
# <module> — backlog

## <theme>
- `name` — one-phrase meaning. (origin: DESIGN.md §References / Go `pkg.Func`)
```

## `_dev/AUDIT.md`

Append-only log, newest first.

```markdown
# <module> — audit log

## 2026-11-24 · <scope, e.g. full module @ <sha>> · <auditor role/model>
| Sev | Location | Finding | Status |
|---|---|---|---|
| CRITICAL | `src/<module>/parser.rs:485` | <mechanism -> trigger> | fixed in #12 / open / false positive |
```
