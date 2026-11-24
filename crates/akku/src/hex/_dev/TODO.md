# hex — backlog

## Sizing and appending
- `decoded_len` — `input_len / 2`, for sizing `decode_to_slice` buffers. (origin: Go `hex.DecodedLen`)
- `encode_into(&mut String)` / `decode_into(&mut Vec<u8>)` — append without a fresh allocation. (origin: Go `hex.AppendEncode` / `hex.AppendDecode`)

## Streaming
- `Encoder<W: Write>` / `Decoder<R: Read>` — streaming hex over `std::io`. (origin: Go `hex.NewEncoder` / `hex.NewDecoder`)

## Formatting
- `Display` adapter (`hex::display(&bytes)`) — format without allocating a `String`. (origin: `hex` crate `ToHex`; a free function, not a trait, per DESIGN.md §Non-goals "Trait-based API")
- `encode_with_separator(input, sep, bytes_per_sep)` — `de:ad:be:ef` style. (origin: Python `bytes.hex(sep, bytes_per_sep)`)
- hex dump (`xxd` / `hexdump -C` layout) — decide whether it lives here or in a presentation module. (origin: Go `hex.Dump` / `hex.Dumper`)

## Decoding variants
- lenient decode that skips ASCII whitespace or caller-chosen ignore chars. (origin: Python `bytes.fromhex`, `binascii.unhexlify(ignorechars=)`)
- `decode_array::<N>` — decode into `[u8; N]` with an exact-length check. (origin: `hex` crate `FromHex for [u8; N]`)
- in-place decode (`decode_in_slice`). (origin: `hex` crate `decode_in_slice`)
