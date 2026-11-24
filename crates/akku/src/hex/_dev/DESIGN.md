# hex — design record

## Purpose

Hex is the everyday way to show bytes to humans: hashes, keys, IDs, binary in logs and test fixtures. Almost every akku module that prints or parses binary data needs it, so akku ships it as a zero-dependency Wave 1 module and nobody has to add the `hex` crate for it. Introduced in `44cc307 feat(hex): add akku::hex`.

## Non-goals

- **Lenient decoding.** Whitespace, `:`/`-` separators, and `0x` prefixes are rejected, not skipped. Python's `bytes.fromhex` skips whitespace, but a strict decoder is easier to reason about, and callers can strip separators first. A lenient variant is in `_dev/TODO.md`.
- **Constant-time operation.** `decode_nibble` branches on the byte value and errors exit early. Secret-key handling belongs in `akku::tls` / crypto forks. The rustdoc §Security states this.
- **Hex dumps** (`xxd`/`hexdump -C` style). Go puts `Dump`/`Dumper` in `encoding/hex`. In akku that is a presentation concern; see `_dev/TODO.md`.
- **Trait-based API** (`ToHex`/`FromHex` like the `hex` crate). Free functions over `impl AsRef<[u8]>` cover the same calls without new trait names in user scope.
- **SIMD / `unsafe` speedups.** Forbidden by the unsafe policy; the scalar loop is fast enough for the use cases above.

## Namespace

`akku::hex` (flat, listed in `Namespace.md` §Wave 1 as "Hex-Encoding/Decoding"). Items: `encode`, `encode_upper`, `encode_to_slice`, `encode_upper_to_slice`, `decode`, `decode_to_slice`, `encoded_len`, `DecodeError`, `BufferTooSmall`. No clashes: `akku::base64` uses the same verb names in its own namespace. `akku::color` will parse `#rrggbb` itself and must not depend on hex for that.

## Equivalents

| Ecosystem | Item |
|---|---|
| Go | `encoding/hex`: `Encode`, `EncodeToString`, `Decode`, `DecodeString`, `EncodedLen`, `DecodedLen`, `AppendEncode`, `AppendDecode`, `NewEncoder`, `NewDecoder`, `Dump`, `Dumper`, `ErrLength`, `InvalidByteError` |
| Rust std | none (only `{:x}` formatting of integers) |
| Rust crates | `hex` 0.4.3 (`encode`, `encode_upper`, `decode`, `decode_to_slice`, `encode_to_slice`, `ToHex`, `FromHex`, `FromHexError`); `faster-hex`, `const-hex` (SIMD) |
| Python | `bytes.hex(sep, bytes_per_sep)`, `bytes.fromhex`, `binascii.hexlify`/`unhexlify` |

## Dependencies

`native`: `std` only (`String`, `Vec`, `fmt`, `std::error::Error`). No `unsafe`.

## References

Pinned to the commits read during the 2026-11-24 migration.

- **Go** `encoding/hex`, `golang/go@2e78a18`, `src/encoding/hex/hex.go`
  - `:39` `EncodedLen(n) = n * 2` with no overflow check.
  - `:115-202` `Decode`: decodes pairs left to right, returns `InvalidByteError(byte)` with **no index**, and for an odd length checks the dangling last byte before returning `ErrLength` (comment at `:192-193`: "the invalid char (if present) is an earlier problem"). akku uses the same precedence.
  - `:115` `Decode` writes into `dst` without a size check and panics on a short `dst` (`_ = dst[3]` bounds check). akku returns `OutputTooSmall` instead.
  - Returns "the number of bytes decoded before the error", so `dst` is partially written on error. akku documents the same.
  - `:85`, `:207` `AppendEncode`/`AppendDecode` (Go 1.22).
- **Rust `hex` crate** 0.4.3, `KokaKiwi/rust-hex@e25a870`
  - `src/lib.rs:321-336` `decode_to_slice`: checks `OddLength` **before** invalid characters, and requires `out.len() == data.len() / 2` **exactly** (`InvalidStringLength`). akku reports the invalid byte first (like Go) and accepts a larger buffer.
  - `src/lib.rs:424` `encode_to_slice`: requires an exact `input.len() * 2` buffer. akku accepts larger and returns the written length.
  - `src/lib.rs:195-208` (`val`) reports `InvalidHexCharacter { c: bytes[i] as char, index }`. That reads a non-ASCII byte as Latin-1 (`0xc3` becomes `'Ã'`). akku reports the raw `byte: u8`.
  - `src/error.rs:6` `FromHexError` is not `#[non_exhaustive]`. akku's `DecodeError` is.
- **Python** `python/cpython@cc056ab`
  - `Objects/bytesobject.c:2751-2779` `bytes.fromhex` skips ASCII whitespace between pairs and reports "non-hexadecimal number found in fromhex() arg at position N". Its separate odd-length message is at `:2793`.
  - `Modules/binascii.c:2284-2320` `unhexlify` with the new `ignorechars` parameter (RFC 4648 §3.3); without it, any non-digit raises "Non-hexadecimal digit found".
  - `bytes.hex(sep, bytes_per_sep)` added in Python 3.8 (docs: <https://docs.python.org/3/library/stdtypes.html#bytes.hex>).
- **RFC 4648** §8 (Base16 alphabet), §10 (test vectors; used in `tests.rs`), §3.3 (implementations must reject non-alphabet characters unless the spec says otherwise). <https://www.rfc-editor.org/rfc/rfc4648>

## Public API

| Item | Semantics | Errors / panics |
|---|---|---|
| `const fn encoded_len(input_len: usize) -> usize` | `input_len * 2` | panics if `input_len > usize::MAX / 2` (debug and release) |
| `fn encode(input: impl AsRef<[u8]>) -> String` | lowercase hex | none |
| `fn encode_upper(input: impl AsRef<[u8]>) -> String` | uppercase hex | none |
| `fn encode_to_slice(input, output: &mut [u8]) -> Result<usize, BufferTooSmall>` | lowercase into `output[..2n]`; rest untouched | `BufferTooSmall { needed }`; `output` untouched on error |
| `fn encode_upper_to_slice(input, output) -> Result<usize, BufferTooSmall>` | uppercase, otherwise identical | same |
| `fn decode(input: impl AsRef<[u8]>) -> Result<Vec<u8>, DecodeError>` | case-insensitive decode | `InvalidByte`, `OddLength` |
| `fn decode_to_slice(input, output: &mut [u8]) -> Result<usize, DecodeError>` | decode into `output[..n/2]`; larger buffer allowed | `InvalidByte` > `OddLength` > `OutputTooSmall`; partial write possible on `InvalidByte` |
| `struct BufferTooSmall { pub needed: usize }` | encode buffer error; `Display`, `Error`, `Copy`, `Eq` | — |
| `#[non_exhaustive] enum DecodeError { InvalidByte { index, byte }, OddLength { len }, OutputTooSmall { needed } }` | decode error; `Display`, `Error`, `Copy`, `Eq` | — |

DoS limits: none needed. Allocation is exactly `2n` (encode) or `n/2` (decode) for an `n`-byte input, so there is no amplification. All loops are single-pass linear; the decode error path makes at most one extra linear scan.

## API rationale

- **`impl AsRef<[u8]>` everywhere.** `&str`, `String`, `Vec<u8>`, arrays and slices all work without conversions, matching the `hex` crate's ergonomics.
- **Invalid byte before odd length.** Same as Go (`hex.go:192-193`): the invalid byte is the earlier problem and its index is more useful. The `hex` crate does the opposite; akku follows Go.
- **`index` and raw `byte` in `InvalidByte`.** Go gives no position; Python gives one. akku gives both, and a raw `u8` rather than `char` avoids the `hex` crate's Latin-1 misreport.
- **Larger output buffers allowed and the written length returned.** This allows reusing one scratch buffer for many calls. It differs from the `hex` crate's exact-length rule.
- **`OutputTooSmall` and `OddLength` leave `output` untouched.** The length checks run before any write. The fast path does not pre-validate, so `InvalidByte` may leave a partial prefix (as in Go). Pre-validating would cost a second pass on the success path.
- **Separate `BufferTooSmall` for encoding.** Encoding can fail only on buffer size, so a one-field struct is clearer than reusing `DecodeError`. GUESS: no source records why the names differ (`BufferTooSmall` vs `OutputTooSmall`); see Open questions.
- **`encoded_len` panics instead of wrapping.** It is a `const fn` with a `usize` signature, so it cannot return `Option` without an API change. A loud panic is better than a wrapped, too-small size in release builds (AUDIT 2026-11-24, finding 1).
- **No `decoded_len`.** Go's `DecodedLen(x) = x / 2` hides the odd-length case; GUESS: it was left out as trivial. Listed in `_dev/TODO.md`.

## Seams

| Seam | Catches | Misses |
|---|---|---|
| `encode` / `encode_upper` vs RFC 4648 §10 and hand literals | wrong alphabet, nibble order, case | — |
| `decode` over all 256 byte values in both nibble positions | any wrong accept/reject or nibble value | — |
| `decode` error index/byte at each position class | off-by-one indices, precedence | — |
| `decode_to_slice` / `encode_to_slice` with short, exact, larger, empty buffers | bounds, untouched-on-error, prefix-only writes | contents after a partial `InvalidByte` write (unspecified by design) |
| `encoded_len` at `usize::MAX / 2` and one past | overflow handling | — |
| 1 MiB round trip and errors at its end | indexing at scale, performance regressions to quadratic | timing side channels |
| `Display` strings, `Box<dyn Error>` | message stability, trait impls | — |

## Open questions

- Should `DecodeError::OutputTooSmall` and `BufferTooSmall` share one name? Renaming is a breaking change; decide before 0.1.
- Should `BufferTooSmall` be `#[non_exhaustive]` (it has a public field and no constructor)? That is also breaking; decide before 0.1.
- Add `#[must_use]` to `encode`, `encode_upper`, `encoded_len`? It is harmless, but it changes attributes on public items, so it needs Orcha's approval.
- Should `decode_to_slice` guarantee `output` is untouched on every error (pre-validate), at the cost of a second pass?
- Should there be a constant-time decoder for secrets, or does that belong to the crypto fork?
