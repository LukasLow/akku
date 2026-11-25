# xdr — backlog

## Streaming & framing
- `StreamEncoder<W: Write>` / `StreamDecoder<R: Read>` — streaming XDR over `std::io` with explicit byte budgets. (origin: `DESIGN.md` §References / `stellar/go-xdr`, `xdr-codec`, `libtirpc` `xdrstdio_create`)
- RFC 5531 §11 Record Marking helpers (`RecordReader` / `RecordWriter` for 31-bit fragment headers + last-fragment bit) — decide whether this lives in `akku::nfs` (internal) or in `akku::xdr` / `akku::rpc` once multiple ONC-RPC protocols exist. (origin: `DESIGN.md` §References / `xdr-codec/src/record.rs`, `libtirpc` `xdrrec_create`)

## Extended RFC 4506 types
- `quadruple` (128-bit IEEE 754 float, RFC 4506 §4.8) — add once Rust stabilizes `f128` (or as raw `[u8; 16]` helper if a protocol requires it). (origin: `DESIGN.md` §Non-goals / RFC 4506 §4.8, `libtirpc` `xdr_quadruple`)
- `decode_ascii_str` / `encode_ascii_str` — strict 7-bit US-ASCII string variant per RFC 4506 §4.11 if a caller wants ASCII rejection beyond UTF-8. (origin: `DESIGN.md` §Decisions 1 / RFC 4506 §4.11)
- `encoded_size` / `XdrSize` — compute exact wire byte length without allocating or writing a buffer. (origin: `DESIGN.md` §References / `libtirpc` `xdr_sizeof`)

## Ergonomics & codegen
- `xdr_struct!` / `xdr_enum!` declarative macros — after `akku::nfs` exercises the manual API. (origin: `DESIGN.md` §Decisions 6)
- `#[derive(XdrEncode, XdrDecode)]` proc macros in `akku-drive` — only if declarative `xdr_struct!` / `xdr_enum!` prove insufficient once `akku::nfs` is built. (origin: `DESIGN.md` §Public API)
- `.x` XDR language parser / code generator (`rpcgen` equivalent, RFC 4506 §6) — build-time codegen via `[build-dependencies]` `akku` if large XDR specs beyond NFSv3 are targeted. (origin: `DESIGN.md` §Non-goals / `jsgf/rust-xdr` `xdrgen`, Sun `rpcgen`)
- `akku::bytes::Bytes` zero-copy opaque decoder (`decode_opaque_bytes`) — O(1) `Bytes::slice` sub-views when decoding from an `akku::bytes::Bytes` buffer rather than `&[u8]`. (origin: `DESIGN.md` §Dependencies / `crates/akku/src/bytes/mod.rs`)

## Decoding variants
- Lenient mode (`Limits::allow_nonzero_padding`, `allow_nonzero_bool`) — only add if interoperability with a buggy real-world NFS/ONC-RPC peer requires relaxing RFC 4506 §3 / §4.4. (origin: `DESIGN.md` §Non-goals / `libtirpc` `src/xdr.c:533,600`, `nfsserve/src/xdr.rs:48,128`)
