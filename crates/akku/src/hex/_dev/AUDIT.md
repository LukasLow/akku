# hex — audit log

## 2026-11-24 · full module @ 9de484d (`mod.rs`, `tests.rs`, `meta.toml`) · Worker C (Claude Opus 5.5), ModuleMigration pilot

Checklist: `akku-security-audit` §A–D. `TODO.md` Priority 1 had no earlier `hex` findings to carry over. Line numbers refer to the original file at `9de484d` unless marked "now".

| Sev | Location | Finding | Status |
|---|---|---|---|
| MEDIUM | `src/hex/mod.rs:80` (now `:196`) | `encoded_len` computes `input_len * 2` unchecked. In release builds this wraps and returns a too-small size; in debug builds it panics with a generic overflow message. Trigger: `encoded_len(usize::MAX / 2 + 1)` returns `0` in release. Slice lengths cannot reach this (≤ `isize::MAX`), but a caller sizing a buffer from an untrusted length can. | fixed on `chore/migrate-hex`: `checked_mul` + explicit panic in all builds, documented under `# Panics`, regression test `encoded_len_panics_on_overflow_instead_of_wrapping` (red before the fix) |
| LOW | `src/hex/mod.rs:141-151` (`decode_to_slice` fast path) | On `InvalidByte`, the pairs decoded before the error are already written to `output`. The slow path (`OddLength`/`OutputTooSmall`) leaves `output` untouched. The old rustdoc said neither. Same as Go `hex.Decode`. Trigger: `decode_to_slice("00zz", &mut [0x99; 2])` leaves `[0x00, 0x99]`. | documented in rustdoc (contents unspecified on `InvalidByte`); untouched-on-length-error covered by tests |
| LOW | `src/hex/mod.rs:158-165` (`decode_nibble`) | Decoding branches on byte value and exits early on error, so it is not constant-time; the old rustdoc did not say so. | documented in module rustdoc §Security; constant-time variant left as an open question in DESIGN.md |
| LOW | `src/hex/tests.rs:97-99` | `encode_upper_to_slice` was only tested on `"hi"` → `"6869"`, which has no letters, so an uppercase encoder that emitted lowercase would have passed. | fixed: `encode_to_slice_fills_exact_buffer`, `encode_to_slice_writes_prefix_of_larger_buffer_only`, `uppercase_encoders_emit_only_uppercase_letters` |
| LOW | `src/hex/mod.rs:22-27` (`BufferTooSmall`) | `BufferTooSmall` has a public field and no `#[non_exhaustive]`, so adding a field later is breaking. Its name also differs from `DecodeError::OutputTooSmall` for the same condition. | open (API change; DESIGN.md §Open questions) |
| LOW | `src/hex/mod.rs:79,84,89` | `encoded_len`, `encode` and `encode_upper` are not `#[must_use]`. | open (DESIGN.md §Open questions) |
| — | `src/hex/mod.rs:114` (`decode`) | Allocation `vec![0; input.len() / 2]` happens before validation. | false positive: the size is bounded by half the input already in memory, so there is no amplification |
| — | `src/hex/mod.rs:173` | `String::from_utf8` re-validates the ASCII output, which is an extra linear pass. | won't fix: avoiding it needs `unsafe` (`from_utf8_unchecked`), which the unsafe policy forbids for speed |

Verified with no finding: index arithmetic `i * 2 + 1` (encode) and `pair_idx * 2 + 1` (decode) cannot overflow or go out of bounds, because the length checks run first and `input.len() ≤ isize::MAX`. No `unwrap` on untrusted data. Both `expect`s are unreachable. All paths are single-pass linear. Layout, namespace and rustdoc now match `akku-module-layout`.

**Scores (0-10):** Security 9 · Correctness 9 (after fix) · Speed 8 · Architecture 9
**Verdict:** PASS (the only MEDIUM is fixed on this branch)
