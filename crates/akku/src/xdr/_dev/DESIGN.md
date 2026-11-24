# xdr — design record

## Purpose

XDR (External Data Representation, RFC 4506 / STD 67) is the big-endian, 4-byte-aligned binary wire encoding underneath ONC-RPC (RFC 5531) and NFSv3 (RFC 1813).

**Primary driver (megaghost):** megaghost serves a MEGA virtual drive to the local OS over an embedded NFSv3 server (`akku::nfs`, backed by `akku::vfs`). Every NFSv3, MOUNTv3, and PORTMAP message is an ONC-RPC call/reply whose header, credentials, file handles, attributes, directory listings, and read/write payloads are XDR-encoded. Shipping `akku::xdr` as a standalone zero-dependency Wave 2 module keeps the wire codec cleanly separated from ONC-RPC framing and NFSv3 semantics, makes it reusable for future RPC/storage protocols, and removes any need for external XDR crates.

## Non-goals

- **No XDR language compiler / `rpcgen` in v1.** Parsing `.x` IDL files (RFC 4506 §6) and generating Rust code adds a full compiler pipeline for a small, fixed set of RFC 1813/5531 structures. Hand-written `Encoder`/`Decoder` calls plus optional declarative macros (`xdr_struct!`, `xdr_enum!`) are smaller, easier to audit, and require no build script. Deferred to [`TODO.md`](TODO.md).
- **No ONC-RPC protocol layer or RPCSEC_GSS.** ONC-RPC call/reply envelopes, `AUTH_NONE`/`AUTH_SYS`, `RPCSEC_GSS` (RFC 2203), and TCP Record Marking (RFC 5531 §11) belong above XDR. Per [`Namespace.md`](../../../../../Namespace.md), ONC-RPC stays internal to `akku::nfs` for now; GSS/auth belongs to `akku::auth` / `akku::crypto`.
- **No quadruple-precision floating point (`quadruple`, RFC 4506 §4.8).** Rust stable has no `f128` primitive, ONC-RPC and NFSv3 do not use 128-bit floats, and none of the surveyed Go, Rust, or Python XDR libraries implement it. Raw 16-byte pass-through can be done via fixed opaque if ever needed; deferred to [`TODO.md`](TODO.md).
- **No lenient decoding mode.** RFC 4506 §3, §4.4, and §5(5) require 0-byte alignment padding (so encodings are canonical and checksum-comparable) and restrict booleans to `0` (`FALSE`) and `1` (`TRUE`). Non-zero padding and invalid booleans are always rejected. A lenient escape hatch is deferred to [`TODO.md`](TODO.md) unless a real peer requires it.
- **No `serde` integration.** Per [`CONCEPT.md`](../../../../../CONCEPT.md) §2 and §8, akku ships zero dependencies by default and owns its own API rather than wrapping `serde`. Serde's data model also clashes with XDR's sparse/signed enums and opaque-vs-byte-array distinction (see §References, `serde-xdr`).
- **No `std::io::Read` / `Write` streaming codecs in v1.** ONC-RPC over TCP frames each message via record marking into a bounded byte buffer before XDR decoding, and UDP datagrams are already contiguous slices. Slice-based decoding enables zero-copy borrows (`&[u8]`, `&str`), upfront length checks without `io::Error`, and simpler DoS accounting. Streaming wrappers are deferred to [`TODO.md`](TODO.md).
- **No `unsafe` optimizations.** Forbidden by akku's unsafe policy (`#![forbid(unsafe_code)]`); safe big-endian slice conversions optimize cleanly.

## Namespace

`akku::xdr` — flat level-1 module, already listed in [`Namespace.md`](../../../../../Namespace.md) under *Network, transport & storage protocols*:

> `akku::xdr` | XDR encoding and decoding (RFC 4506) (Wave 2, megaghost-driven)

The `Namespace.md` row is accurate as-is; no edit is needed.

Proposed public items under `akku::xdr`:
- `Encoder`, `Decoder`, `Limits`
- `XdrEncode`, `XdrDecode`
- `Opaque`, `FixedOpaque`
- `Error`, `Result`
- Free functions: `encode`, `encode_into`, `encode_with`, `decode`, `decode_with`, `padding_len`, `padded_len`
- Declarative macro sugar: `xdr_struct!`, `xdr_enum!`

No name clashes: `akku::xdr` is a flat module (not a reserved keyword, does not shadow `::core`), and all generic names (`Encoder`, `Decoder`, `Error`, `Result`) stay inside `akku::xdr::*` per [`Namespace.md`](../../../../../Namespace.md) rule 2 and rule 5.

## Equivalents

| Ecosystem | Item |
|---|---|
| Go stdlib | None (Go stdlib has `encoding/binary` only; see [`docs/go-stdlib-gap.md`](../../../../../docs/go-stdlib-gap.md)) |
| Go third-party | `github.com/stellar/go-xdr` (`xdr3`: security-hardened fork of `davecgh/go-xdr` with `DecodeOptions`, `MaxDepth`, `MaxInputLen`, `MaxMemoryBytes`, strict padding); `github.com/davecgh/go-xdr` (`xdr2`: reflection + manual `Encoder`/`Decoder`, `UnmarshalLimited`) |
| Rust std | None (only `to_be_bytes` / `from_be_bytes` on primitives) |
| Rust crates | `serde-xdr` (Serde `Serializer`/`Deserializer` over `Read`/`Write`); `xdr-codec` (`Pack`/`Unpack` traits + `xdrgen` codegen + `record` framing); `nfsserve`'s internal `src/xdr.rs` (`XDR` trait + `xdr_struct!`/`xdr_enum_serde!`/`xdr_bool_union!` macros) |
| Python | `xdrlib` (`Packer`/`Unpacker`, deprecated in Python 3.11 via PEP 594 and removed in Python 3.13) |
| C | `rpc/xdr.h` (`libtirpc` / SunRPC: bidirectional `XDR` handle with `XDR_ENCODE`/`XDR_DECODE`/`XDR_FREE`, `xdr_*` routines, `xdrmem`/`xdrstdio`/`xdrrec`) |

## Dependencies

`native`: zero external dependencies (`std` only, `#![forbid(unsafe_code)]`, `wasm32 = true`).

**Internal reuse of `akku::bytes` ([`crates/akku/src/bytes/mod.rs`](../../bytes/mod.rs)):**
- `akku::bytes` provides `Bytes` (refcounted O(1) zero-copy slice), `BytesMut` (growable `Vec<u8>` builder with big-endian `put_u32`/`put_i32`/`put_u64`/`put_i64`/`put_f32`/`put_f64`/`put_slice` and `freeze() -> Bytes`), `Cursor<'a>` (bounds-checked slice reader with big-endian `get_u32`/`get_i32`/`get_u64`/`get_i64`/`get_f32`/`get_f64`/`get_slice`, `remaining()`, `position()` returning `BytesError { needed, remaining }`), and `BytesError`.
- `akku::xdr::Decoder<'a>` can wrap `crate::bytes::Cursor<'a>` internally for bounds-checked big-endian reads (mapping `BytesError` to `xdr::Error::UnexpectedEof { offset, needed, remaining }`) without duplicating slice-slicing and endianness helpers.
- `Encoder<'a>` operates directly on `&'a mut Vec<u8>` (keeping `akku::xdr`'s public signatures free of `akku::bytes` types unless callers opt in, while `Bytes::from(vec)` converts an encoded `Vec<u8>` into `akku::bytes::Bytes` with no copy).

## References

Pinned to the commits read on 2026-11-24.

- **RFC 4506 (STD 67)** — *XDR: External Data Representation Standard* (<https://www.rfc-editor.org/rfc/rfc4506>)
  - §3 Basic Block Size: all items occupy a multiple of 4 bytes; `0..=3` residual zero bytes pad any non-multiple length.
  - §4.1–§4.7: 32-bit big-endian two's complement `int`/`unsigned int`, `enum` (same wire format as signed `int`), `bool` (`enum { FALSE = 0, TRUE = 1 }`), 64-bit `hyper`/`unsigned hyper`, IEEE 754 big-endian `float` (32-bit) and `double` (64-bit).
  - §4.8: 128-bit `quadruple` float (omitted in v1).
  - §4.9–§4.11: fixed opaque (`n` bytes + `r` zero pad), variable opaque (`u32` len + `n` bytes + `r` zero pad, bounded `<m>` or unbounded `<>`), string (same wire layout as variable opaque; spec says ASCII, modern deployments use UTF-8 or opaque bytes for OS filenames).
  - §4.12–§4.16, §4.19: fixed array (`n` elements), variable array (`u32` count + `n` elements), structure (sequential fields), discriminated union (`int`/`uint`/`enum`/`bool` discriminant + implied arm), `void` (0 bytes), optional-data (`bool` + element or `void`, used for linked lists such as `struct stringentry { string item<>; stringentry *next; }`).
  - §5(5): zero-padding exists so identical data encodes identically across machines for comparison and checksumming.
  - §8 Security Considerations: explicitly warns against (1) unbounded variable-length allocations, (2) embedded NUL bytes in C strings, (3) application-illegal path characters like `'/'`, and (4) stack-overflow DoS from recursive decoders/encoders on optional-data linked lists — recommends non-recursive decoding or depth/list bounds.
- **RFC 5531** (ONC-RPC v2, <https://www.rfc-editor.org/rfc/rfc5531>) and **RFC 1813** (NFSv3, <https://www.rfc-editor.org/rfc/rfc1813>): downstream protocols built on XDR; NFSv3 uses sparse `enum nfsstat3` values up to `10008`, fixed 8-byte opaque verifiers (`cookieverf3`, `writeverf3`), variable opaque file handles (`opaque fhandle3<64>`), raw-byte filenames (`filename3`), and optional-data linked lists (`entry3 *nextentry` in `READDIR3resok`).
- **Go `stellar/go-xdr`** `stellar/go-xdr@213301bde5564ad4856cc34b30626a85eb37505f` (security-hardened fork of `davecgh/go-xdr`)
  - `xdr3/decode.go:36,45`: `DecodeDefaultMaxDepth = 1500`; `MaxPrealloc = 256` caps upfront slice pre-allocation for variable-length arrays and grows incrementally so a doctored length cannot OOM before reading elements (`:636-673`).
  - `xdr3/decode.go:48-73`: `DecodeOptions` exposes `MaxDepth`, `MaxInputLen` (enforced via `io.LimitedReader` at `:188-203`), and `MaxMemoryBytes` (cumulative allocation budget).
  - `xdr3/decode.go:291-306`: `DecodeBool` strictly accepts `0` or `1`, returning `ErrBadEnumValue` otherwise.
  - `xdr3/decode.go:404-406`: RFC 4506 §4.8 quadruple float explicitly unimplemented.
  - `xdr3/decode.go:463-483`: `DecodeFixedOpaqueInplace` validates every padding byte is `0x00` and rejects non-zero padding (`"non-zero padding"` at `:477-481`).
  - `xdr3/decode.go:521-556`: `DecodeString` treats strings as UTF-8 and notes callers needing non-UTF-8 bytes should use `DecodeOpaque`.
  - `xdr3/encode.go:58-61`: documents Go reflection's ambiguity between `[]byte` (opaque) and `[]uint8` (array), requiring struct tag `xdropaque:"false"`.
  - `xdr3/error.go:26-77`: typed `ErrorCode` enum (`ErrBadEnumValue`, `ErrOverflow`, `ErrBadUnionSwitch`, `ErrMaxDecodingDepth`, `ErrIO`, ...).
- **Go `davecgh/go-xdr`** `davecgh/go-xdr@e6a2ba005892b6a5b27cb5352f64c2e96942dd28`
  - `xdr2/decode.go:68-69`: documents no recursion guard ("Cyclic data structures ... will result in infinite loops").
  - `xdr2/decode.go:84-87,101-104`: `UnmarshalLimited` sets `maxReadSize`, but `Unmarshal` defaults to `0` (unlimited).
  - `xdr2/decode.go:185-200`: `DecodeBool` strictly rejects values other than `0`/`1`.
  - `xdr2/decode.go:313-336`: `DecodeFixedOpaque` allocates `paddedSize` bytes and returns `buf[0:size]` **without checking padding bytes are zero** (and without rejecting negative `size`).
- **Rust `jvff/serde-xdr`** `jvff/serde-xdr@fa7fe7754dd070d31422be88083e2b759c210d58`
  - `src/lib.rs:70-74`: depends on `serde`, `byteorder`, and deprecated `failure`.
  - `src/de/mod.rs:86-103`: `deserialize_opaque` computes `let padding_size = 4 - (length + 3) % 4 - 1; let buffer_length = length + padding_size;` (`:91-92`, overflows `u32` for `length >= u32::MAX - 2`), allocates `Vec` directly from untrusted wire `length` (`:94-96`, no limit — 4 GiB OOM DoS), and truncates padding without checking for zeros (`:97-100`).
  - `src/de/deserializer/mod.rs:31-45,196-214`: strictly rejects bools and options outside `0`/`1` (`InvalidBool`, `InvalidOption`).
  - `src/de/deserializer/mod.rs:309-342`: maps Serde enums by dense 0-based `u32` variant index, which does not naturally express sparse/signed RFC 4506 enum constants (e.g., NFSv3 `nfsstat3`).
- **Rust `xdr-codec` (`jsgf/rust-xdr`)** `jsgf/rust-xdr@aa64396d69f41928a739dc740e938e6dc61a5681`
  - `xdr-codec/src/lib.rs:11-19`: deliberately omits `Pack`/`Unpack` for `u8`/`i8` by default to prevent byte arrays from silently encoding as 4-bytes-per-byte XDR arrays; uses `Opaque<'a>(Cow<'a, [u8]>)` (`:57`) instead.
  - `xdr-codec/src/lib.rs:47-49`: `padding(sz) = &PADDING[..(4 - (sz % 4)) % 4]`.
  - `xdr-codec/src/lib.rs:271-301`: `unpack_opaque_array` discards padding without checking zeros (`:295`) and loops on `input.read(...)` without checking `r == 0` (`:279-282`).
  - `xdr-codec/src/lib.rs:304-355,687-691`: `unpack_flex` / `unpack_opaque_flex` take `maxsz: Option<usize>`, but `Unpack for Vec<T>` passes `None` and calls `Vec::with_capacity(elems)` (`:314`, `:344`) directly from wire count (OOM DoS), discards padding unchecked (`:324`, `:350`), and `unpack_opaque_flex` uses `take().read_to_end()` (`:346`) without verifying `out.len() == elems`.
  - `xdr-codec/src/lib.rs:374-376,582-584`: `Pack<Out: Write>` and `Unpack<In: Read>` traits returning byte counts; `Unpack for bool` (`:662-671`) strictly requires `0`/`1`; `Unpack for String` (`:693-698`) validates UTF-8.
  - `xdr-codec/src/record.rs:1-74`: implements RFC 1831/5531 TCP Record Marking separately from core XDR.
  - `xdr-codec/src/error.rs:1-21`: uses deprecated `error_chain!`.
- **Rust `nfsserve` (`xetdata/nfsserve`)** `xetdata/nfsserve@327b45530f04664da678297ce2568c47bf3fbf81`
  - `src/xdr.rs:9-12`: `pub trait XDR { fn serialize<R: Write>(&self, ..); fn deserialize<R: Read>(&mut self, ..); }` (mutating `&mut self`, forcing `Default` on decoded types at `:222`).
  - `src/xdr.rs:18-38,165-181,199-230`: lightweight declarative macros `xdr_enum_serde!`, `xdr_struct!`, `xdr_bool_union!` show how pleasant derive-free NFSv3 wire definitions are (though `xdr_enum_serde!` relies on external `num_traits::FromPrimitive` and casts enums to `u32` instead of `i32`).
  - `src/xdr.rs:46-50`: `bool::deserialize` uses `*self = val > 0` (lenient; accepts values other than `0`/`1`).
  - `src/xdr.rs:97-104`: `[u8; N]` reads/writes `N` bytes with **no 4-byte alignment padding** (violates RFC 4506 §4.9 whenever `N % 4 != 0`).
  - `src/xdr.rs:106-131,142-161`: `Vec<u8>` and `Vec<u32>` `serialize` panic via `assert!(self.len() < u32::MAX as usize)` (`:108`, `:144`), while `deserialize` calls `self.resize(length as usize, 0)` with no upper bound (4–16 GiB OOM DoS) and discards padding without checking zeros (`:126-128`).
  - `src/nfs.rs:44,87-88,104-198`: `filename3`/`nfspath3` wrap raw `Vec<u8>` (`nfsstring`) rather than UTF-8 `String`, and `nfsstat3` uses sparse discriminants (`0, 1, 2, 5, ..., 10008`).
- **Python `xdrlib`** `python/cpython@40533a46d2dcef742cd56d6e4098da23c813a518` (Python 3.12; deprecated in `c2b579741dc36f4b7da00d4361d651544996d8f4` for PEP 594, removed in 3.13 in `087c1a6539eac330372a8b759bf3e2d472f6148a`)
  - `Lib/xdrlib.py:12`: `warnings._deprecated(__name__, remove=(3, 13))`.
  - `Lib/xdrlib.py:50-132,135-243`: imperative `Packer` (into `BytesIO`) and `Unpacker` (over `bytes`) with `done()` (`:154-156`) to reject trailing bytes.
  - `Lib/xdrlib.py:176-177`: `unpack_bool` uses `bool(self.unpack_int())` (lenient).
  - `Lib/xdrlib.py:206-214`: `unpack_fstring` skips padding without checking zeros.
  - `Lib/xdrlib.py:116-120,225-232`: `pack_list` / `unpack_list` encode and decode RFC 4506 §4.19 optional-data linked lists **iteratively** (`while (x := self.unpack_uint()) != 0: if x != 1: raise ConversionError(...)`), avoiding call-stack recursion.
- **C `rpc/xdr.h` (`libtirpc`)** `alisw/libtirpc@5ca4ca92f629d9d83e83544b9239abaaacf0a527`
  - `tirpc/rpc/xdr.h:83-87,102-127,143`: single bidirectional `XDR` handle switching on `enum xdr_op { XDR_ENCODE, XDR_DECODE, XDR_FREE }`.
  - `tirpc/rpc/xdr.h:92-94`: `BYTES_PER_XDR_UNIT (4)` and `RNDUP(x)`.
  - `tirpc/rpc/xdr.h:288-330`: full primitive surface (`xdr_void`, `xdr_int`, `xdr_u_int`, `xdr_hyper`, `xdr_u_hyper`, `xdr_bool`, `xdr_enum`, `xdr_float`, `xdr_double`, `xdr_quadruple`, `xdr_opaque`, `xdr_bytes`, `xdr_string`, `xdr_vector`, `xdr_array`, `xdr_union`, `xdr_pointer`, `xdr_sizeof`).
  - `src/xdr.c:517-541`: `xdr_bool` is lenient on decode (`*bp = (lb == XDR_FALSE) ? FALSE : TRUE;` at `:533`).
  - `src/xdr.c:573-617`: `xdr_opaque` reads padding into a shared `static int crud[4]` buffer (`:579`, `:600`) without checking zeros (neither thread-safe nor strict).
  - `src/xdr.c:625-684,759-835` and `src/xdr_array.c:58-123`: `xdr_bytes`, `xdr_string`, and `xdr_array` all require an explicit `maxsize` bound, and `xdr_array.c:77` checks `UINT_MAX/elsize < c` against multiplication overflow.

## Public API

Signatures and contracts only (no implementation). All fallible operations return `akku::xdr::Result<T>`.

### Alignment helpers

```rust
/// Returns the number of zero padding bytes (`0..=3`) needed to align `len` to 4 bytes.
/// Never overflows for any `usize`.
pub const fn padding_len(len: usize) -> usize;

/// Returns `len + padding_len(len)`, or `None` on `usize` overflow.
pub const fn padded_len(len: usize) -> Option<usize>;
```

### DoS limits (`Limits`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum byte length of a single variable-length opaque or string.
    /// Default: 16 MiB (`16 * 1024 * 1024`).
    pub max_opaque_len: u32,
    /// Maximum element count of a single variable-length array or optional-data list.
    /// Default: `65_536`.
    pub max_array_len: u32,
    /// Maximum cumulative bytes read by a `Decoder` or written by an `Encoder`.
    /// Default: 64 MiB (`64 * 1024 * 1024`).
    pub max_total_bytes: usize,
    /// Maximum nesting depth of structs, unions, arrays, and optional-data.
    /// Default: `64`.
    pub max_depth: usize,
}

impl Default for Limits { /* defaults above */ }

impl Limits {
    pub const fn new() -> Self;
    pub const fn with_max_opaque_len(self, max: u32) -> Self;
    pub const fn with_max_array_len(self, max: u32) -> Self;
    pub const fn with_max_total_bytes(self, max: usize) -> Self;
    pub const fn with_max_depth(self, max: usize) -> Self;
}
```

Per-field bounds (`max_len: Option<u32>` / `max_count: Option<u32>`) are combined with `Limits` as `min(field_bound, limits_bound)` so a caller can tighten a field (e.g., `NFS3_FHSIZE = 64`) without raising the global cap. Variable-length array and list decoders also cap initial `Vec::with_capacity` pre-allocation at `256` elements and grow incrementally as elements actually decode.

### Error type (`Error`, `Result`)

```rust
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Input ended before the required bytes could be read.
    UnexpectedEof { offset: usize, needed: usize, remaining: usize },
    /// Unconsumed bytes remained when `Decoder::finish` (or `decode`) was called.
    TrailingBytes { offset: usize, remaining: usize },
    /// A padding byte in the 0..=3 alignment tail was non-zero.
    NonZeroPadding { offset: usize, byte: u8 },
    /// Boolean wire word was neither `0` nor `1`.
    InvalidBool { offset: usize, value: i32 },
    /// Enumeration wire word was rejected by the caller's mapping.
    InvalidEnum { offset: usize, value: i32 },
    /// Discriminated union tag was rejected by the caller.
    InvalidDiscriminant { offset: usize, value: i32 },
    /// String payload was not valid UTF-8.
    InvalidUtf8 { offset: usize, valid_up_to: usize },
    /// A variable opaque, string, array, or list exceeded `u32::MAX`, its field bound, or `Limits`.
    LengthLimit { offset: usize, len: usize, limit: usize },
    /// Cumulative encoded or decoded bytes exceeded `Limits::max_total_bytes`.
    TotalBytesLimit { offset: usize, limit: usize },
    /// Nesting depth exceeded `Limits::max_depth`.
    DepthLimit { offset: usize, limit: usize },
    /// Byte-length calculation overflowed `usize` (e.g., on 32-bit / `wasm32` targets).
    LengthOverflow { offset: usize, len: usize },
}
// Implements `fmt::Display` and `std::error::Error`.
```

### Encoder (`Encoder<'a>`)

Writes big-endian, 4-byte-aligned XDR into a caller-owned `Vec<u8>`. Every write checks `Limits::max_total_bytes` before growing the buffer.

```rust
pub struct Encoder<'a> { /* &'a mut Vec<u8>, start_len, limits, depth */ }

impl<'a> Encoder<'a> {
    pub fn new(buf: &'a mut Vec<u8>) -> Self;
    pub fn with_limits(buf: &'a mut Vec<u8>, limits: Limits) -> Self;
    pub fn written(&self) -> usize;
    pub fn limits(&self) -> Limits;

    // Primitives (RFC 4506 §4.1–§4.7, §4.16)
    pub fn encode_int(&mut self, v: i32) -> Result<()>;
    pub fn encode_uint(&mut self, v: u32) -> Result<()>;
    pub fn encode_enum(&mut self, v: i32) -> Result<()>;
    pub fn encode_bool(&mut self, v: bool) -> Result<()>;
    pub fn encode_hyper(&mut self, v: i64) -> Result<()>;
    pub fn encode_uhyper(&mut self, v: u64) -> Result<()>;
    pub fn encode_float(&mut self, v: f32) -> Result<()>;
    pub fn encode_double(&mut self, v: f64) -> Result<()>;
    pub fn encode_void(&mut self) -> Result<()>;

    // Opaque & String (RFC 4506 §4.9–§4.11); pads with 0..=3 zero bytes
    pub fn encode_fixed_opaque(&mut self, bytes: &[u8]) -> Result<()>;
    pub fn encode_opaque(&mut self, bytes: &[u8]) -> Result<()>;
    pub fn encode_opaque_bounded(&mut self, bytes: &[u8], max_len: u32) -> Result<()>;
    pub fn encode_str(&mut self, s: &str) -> Result<()>;
    pub fn encode_str_bounded(&mut self, s: &str, max_len: u32) -> Result<()>;

    // Compound types (RFC 4506 §4.12–§4.15, §4.19); all enforce `max_depth`
    pub fn nest<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R>;
    pub fn encode_fixed_array<T>(
        &mut self,
        items: &[T],
        encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()>;
    pub fn encode_array<T>(
        &mut self,
        items: &[T],
        max_count: Option<u32>,
        encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()>;
    pub fn encode_option<T>(
        &mut self,
        value: Option<&T>,
        encode_val: impl FnOnce(&mut Self, &T) -> Result<()>,
    ) -> Result<()>;
    /// Encodes a slice as an RFC 4506 §4.19 optional-data linked list (`1, item, ..., 0`), iteratively.
    pub fn encode_list<T>(
        &mut self,
        items: &[T],
        max_count: Option<u32>,
        encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()>;
    pub fn encode_union_i32(
        &mut self,
        discriminant: i32,
        encode_arm: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()>;
    pub fn encode_union_u32(
        &mut self,
        discriminant: u32,
        encode_arm: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()>;

    // Trait bridge
    pub fn encode<T: XdrEncode + ?Sized>(&mut self, value: &T) -> Result<()>;
}
```

### Decoder (`Decoder<'a>`)

Strictly decodes XDR from `&'a [u8]` (internally backed by `akku::bytes::Cursor<'a>`). Rejects non-zero padding, bools outside `0`/`1`, invalid UTF-8 in strings, and any limit violation.

```rust
pub struct Decoder<'a> { /* akku::bytes::Cursor<'a>, limits, depth */ }

impl<'a> Decoder<'a> {
    pub fn new(input: &'a [u8]) -> Self;
    pub fn with_limits(input: &'a [u8], limits: Limits) -> Self;
    pub fn position(&self) -> usize;
    pub fn remaining(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn rest(&self) -> &'a [u8];
    pub fn limits(&self) -> Limits;
    /// Succeeds only if all input bytes were consumed; otherwise returns `Error::TrailingBytes`.
    pub fn finish(self) -> Result<()>;

    // Primitives (RFC 4506 §4.1–§4.7, §4.16)
    pub fn decode_int(&mut self) -> Result<i32>;
    pub fn decode_uint(&mut self) -> Result<u32>;
    /// Decodes a signed 32-bit enum discriminant and maps it; `None` yields `Error::InvalidEnum`.
    pub fn decode_enum<E>(&mut self, from_i32: impl FnOnce(i32) -> Option<E>) -> Result<E>;
    /// Strictly accepts `0` (`false`) or `1` (`true`); anything else yields `Error::InvalidBool`.
    pub fn decode_bool(&mut self) -> Result<bool>;
    pub fn decode_hyper(&mut self) -> Result<i64>;
    pub fn decode_uhyper(&mut self) -> Result<u64>;
    pub fn decode_float(&mut self) -> Result<f32>;
    pub fn decode_double(&mut self) -> Result<f64>;
    pub fn decode_void(&mut self) -> Result<()>;

    // Opaque & String (RFC 4506 §4.9–§4.11); always validates 0..=3 zero padding bytes
    pub fn decode_fixed_opaque_slice(&mut self, len: usize) -> Result<&'a [u8]>;
    pub fn decode_fixed_opaque<const N: usize>(&mut self) -> Result<[u8; N]>;
    pub fn decode_fixed_opaque_into(&mut self, dst: &mut [u8]) -> Result<()>;
    pub fn decode_opaque_slice(&mut self, max_len: Option<u32>) -> Result<&'a [u8]>;
    pub fn decode_opaque(&mut self, max_len: Option<u32>) -> Result<Vec<u8>>;
    pub fn decode_str(&mut self, max_len: Option<u32>) -> Result<&'a str>;
    pub fn decode_string(&mut self, max_len: Option<u32>) -> Result<String>;

    // Compound types (RFC 4506 §4.12–§4.15, §4.19); all enforce `max_depth`
    pub fn nest<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R>;
    pub fn decode_fixed_array<T, const N: usize>(
        &mut self,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<[T; N]>;
    pub fn decode_fixed_array_vec<T>(
        &mut self,
        count: usize,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>>;
    pub fn decode_array<T>(
        &mut self,
        max_count: Option<u32>,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>>;
    pub fn decode_option<T>(
        &mut self,
        decode_val: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<Option<T>>;
    /// Iteratively decodes an RFC 4506 §4.19 optional-data linked list (`1, item, ..., 0`).
    /// Uses O(1) call-stack depth regardless of list length.
    pub fn decode_list<T>(
        &mut self,
        max_count: Option<u32>,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>>;
    /// Decodes a signed discriminant inside a depth-guarded scope and dispatches to `decode_arm`.
    pub fn decode_union_i32<T>(
        &mut self,
        decode_arm: impl FnOnce(i32, &mut Self) -> Result<T>,
    ) -> Result<T>;
    /// Decodes an unsigned discriminant inside a depth-guarded scope and dispatches to `decode_arm`.
    pub fn decode_union_u32<T>(
        &mut self,
        decode_arm: impl FnOnce(u32, &mut Self) -> Result<T>,
    ) -> Result<T>;

    // Trait bridge
    pub fn decode<T: XdrDecode>(&mut self) -> Result<T>;
}
```

### Traits, opaque wrappers, and top-level helpers

```rust
pub trait XdrEncode {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()>;
}

pub trait XdrDecode: Sized {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self>;
}

/// Variable-length XDR opaque bytes (`opaque<>`). Distinct from `Vec<T>` arrays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Opaque(pub Vec<u8>);

/// Fixed-length XDR opaque bytes (`opaque[N]`, padded to 4 bytes). Distinct from `[T; N]` arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FixedOpaque<const N: usize>(pub [u8; N]);

// Standard `XdrEncode` / `XdrDecode` impls:
// - `i32`, `u32`, `i64`, `u64`, `bool`, `f32`, `f64`, `()` (void)
// - `String` (and `&str` for `XdrEncode`)
// - `Opaque`, `FixedOpaque<N>`
// - `Option<T>` (RFC 4506 §4.19 optional-data)
// - `Vec<T>` (and `&[T]` for `XdrEncode`; RFC 4506 §4.13 variable-length array)
// - `[T; N]` (RFC 4506 §4.12 fixed-length array)
// - `Box<T>` (delegates via `nest`)
// Note: `u8`/`i8` intentionally do NOT implement `XdrEncode`/`XdrDecode`.

pub fn encode<T: XdrEncode + ?Sized>(value: &T) -> Result<Vec<u8>>;
pub fn encode_into<T: XdrEncode + ?Sized>(value: &T, buf: &mut Vec<u8>) -> Result<()>;
pub fn encode_with<T: XdrEncode + ?Sized>(value: &T, limits: Limits) -> Result<Vec<u8>>;
pub fn decode<T: XdrDecode>(input: &[u8]) -> Result<T>;
pub fn decode_with<T: XdrDecode>(input: &[u8], limits: Limits) -> Result<T>;
```

`decode` and `decode_with` call `T::decode(&mut dec)` followed by `dec.finish()?`, rejecting trailing bytes by default. Callers decoding multiple concatenated messages use `Decoder::new(input)` directly.

### Optional declarative macro sugar (no proc macros)

```rust
// Generates `XdrEncode` and `XdrDecode` (depth-guarded via `nest`) for a struct whose fields implement them:
akku::xdr::xdr_struct! {
    pub struct Nfstime3 {
        pub seconds: u32,
        pub nseconds: u32,
    }
}

// Generates `XdrEncode` and `XdrDecode` for a C-like enum with explicit `i32` discriminants
// (supports sparse values like NFSv3 `nfsstat3`, without `num_derive`):
akku::xdr::xdr_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Ftype3 {
        Reg = 1,
        Dir = 2,
        Lnk = 5,
    }
}
```

Manual `impl XdrEncode` / `impl XdrDecode` remains first-class and is the natural path for bounded fields (`opaque fhandle3<64>`), discriminated unions, and iterative `decode_list` directory entries.

## API rationale

- **Both imperative `Encoder`/`Decoder` and `XdrEncode`/`XdrDecode` traits.** Protocol decoders like `akku::nfs` need per-field RFC bounds (`fhandle3<64>`), zero-copy slice borrows (`WRITE3args` payload `&'a [u8]`), and iterative linked lists (`READDIR3resok`), which map cleanly to `Decoder` methods without wrapper types. At the same time, composite structs (`Fattr3`, `WccAttr`, `Nfstime3`) compose concisely via `XdrEncode`/`XdrDecode` and `encode`/`decode`.
- **`&mut Vec<u8>` and `&[u8]` instead of `Write`/`Read`.** Avoids leaking `std::io::Error` into `xdr::Error`, allows callers to reuse one `Vec<u8>` across RPC replies and prepend/patch a 4-byte RFC 5531 record-marking header in place, and gives `Decoder` zero-copy `&'a [u8]`/`&'a str` borrows plus upfront `remaining()` knowledge (unlike `xdr-codec`, where `unpack_opaque_flex` on `Read` silently accepted truncated data at `xdr-codec/src/lib.rs:346`).
- **Strict zero-padding and strict `0`/`1` booleans.** Follows RFC 4506 §3, §4.4, §5(5) and `stellar/go-xdr` (`xdr3/decode.go:291-306,463-483`). Rejects the permissive pitfalls found in `davecgh/go-xdr` (`xdr2/decode.go:335`), `serde-xdr` (`src/de/mod.rs:100`), `xdr-codec` (`src/lib.rs:350`), `nfsserve` (`src/xdr.rs:48,128`), `xdrlib` (`Lib/xdrlib.py:177,214`), and `libtirpc` (`src/xdr.c:533,600`).
- **Separate `Opaque` / `FixedOpaque<N>` instead of `XdrEncode` on `u8` / `[u8; N]`.** RFC 4506 has no byte primitive; implementing `XdrEncode` for `u8` would silently bloat byte arrays 4×, while implementing both `[T; N]` and `[u8; N]` collides under Rust's coherence rules. Explicit `Opaque` and `FixedOpaque<N>` newtypes mirror `xdr-codec`'s `Opaque` (`xdr-codec/src/lib.rs:11-15,57`) and avoid Go reflection's `xdropaque:"false"` tag (`stellar/go-xdr` `xdr3/encode.go:58-61`).
- **Iterative `encode_list` / `decode_list` for RFC 4506 §4.19 linked lists.** Directly addresses RFC 4506 §8's recursive-decoder stack-overflow warning and mirrors Python `xdrlib`'s `pack_list`/`unpack_list` (`Lib/xdrlib.py:116-120,225-232`), so NFSv3 `READDIR3`/`READDIRPLUS3` responses with thousands of entries decode in O(1) stack space.
- **Four explicit DoS limits + capped pre-allocation.**
  - `max_opaque_len = 16 MiB`: comfortably covers NFSv3 `READ`/`WRITE` blocks (typically 32 KiB–1 MiB) while stopping 4 GiB wire-length OOMs (seen in `serde-xdr`, `xdr-codec`, `nfsserve`).
  - `max_array_len = 65_536`: stops CPU-spin bombs on arrays of zero-byte `void` (`()`) elements where `count = u32::MAX` fits in 4 wire bytes.
  - `max_total_bytes = 64 MiB`: bounds cumulative work across many fields in one message (inspired by `stellar/go-xdr`'s `MaxInputLen`/`MaxMemoryBytes` at `xdr3/decode.go:55-73`).
  - `max_depth = 64`: prevents stack overflow even in debug builds on 2 MiB threads while exceeding any real ONC-RPC/NFSv3 nesting depth.
  - Incremental growth cap (`256` initial `Vec::with_capacity`, mirroring `stellar/go-xdr` `MaxPrealloc = 256` at `xdr3/decode.go:45,642-673`): prevents a short truncated buffer with a large valid count from pre-allocating gigabytes before hitting `UnexpectedEof`.
- **Overflow-safe padding math.** `padding_len(len) = (4 - (len & 3)) & 3` and `padded_len` via `checked_add` avoid `serde-xdr`'s `u32` wraparound at `length >= u32::MAX - 2` (`src/de/mod.rs:91-92`) and stay panic-free on 32-bit `wasm32` targets.
- **Strings validated as UTF-8; raw bytes use opaque.** Matches `stellar/go-xdr` (`xdr3/decode.go:523`), `xdr-codec` (`src/lib.rs:361`), and `serde-xdr` (`src/de/deserializer/mod.rs:167`). UTF-8 is a strict superset of RFC 4506 §4.11 ASCII; NFSv3 `filename3`/`nfspath3` (arbitrary OS bytes, `nfsserve/src/nfs.rs:44,87-88`) use `decode_opaque_slice` / `decode_opaque` directly.
- **`Copy`, `#[non_exhaustive]` `Error` with byte `offset`.** Zero-allocation error paths, forward-compatible enum variants, and exact byte offsets for protocol debugging.
- **Macros are sugar only.** `xdr_struct!` and `xdr_enum!` are plain `macro_rules!` over `Encoder`/`Decoder`, keeping `akku-drive` untouched and following `AGENTS.md` rule 7 and `CONCEPT.md` §8.5.

## Seams

Public boundaries targeted by `crates/akku/src/xdr/tests.rs` (G3):

| Seam | Catches | Misses |
|---|---|---|
| `padding_len` / `padded_len` at `0..=8`, `u32::MAX - 3..=u32::MAX`, `usize::MAX - 3..=usize::MAX` | off-by-one padding, integer wrap/panic on 32- and 64-bit | — |
| `Encoder` / `Decoder` primitives (`int`, `uint`, `enum`, `bool`, `hyper`, `uhyper`, `float`, `double`, `void`) against hand-crafted RFC 4506 big-endian byte literals | endianness bugs, sign extension, IEEE 754 bit loss (`-0.0`, `±inf`, subnormals, quiet NaN bits), void consuming bytes | — |
| `decode_bool` on `0`, `1`, `2`, `-1`, `i32::MIN`, `i32::MAX`, `0x0001_0000` | lenient truthy decoding | — |
| `encode_fixed_opaque` / `decode_fixed_opaque*` across lengths `0, 1, 2, 3, 4, 5, 7, 8` | missing padding on fixed opaque (like `nfsserve`), wrong slice length, unvalidated padding | — |
| `encode_opaque*` / `decode_opaque*` and `encode_str*` / `decode_str*` across lengths `0..=9` and UTF-8 / invalid UTF-8 | length prefix bugs, non-zero padding acceptance, UTF-8 validation gaps, `max_len` off-by-one on encode and decode | — |
| `encode_array` / `decode_array`, `encode_fixed_array` / `decode_fixed_array*` | element count mismatches, missing per-element 4-byte alignment, depth accounting | — |
| `encode_option` / `decode_option` and `encode_list` / `decode_list` | invalid optional tag (`2`, `-1`), empty vs multi-element lists, stack overflow on long linked lists | — |
| `encode_union_i32` / `decode_union_i32` and `*_u32` (including `void` arm and unknown discriminant) | wrong discriminant width/signedness, arm dispatch bugs, missing `InvalidDiscriminant` | — |
| `XdrEncode` / `XdrDecode`, `encode` / `decode` / `decode_with`, `xdr_struct!`, `xdr_enum!` | trait coherence, macro hygiene, trailing-byte rejection in `decode`, sparse/negative enum discriminants | — |
| `Limits` and `Error` (`Display`, `std::error::Error`, exact `offset`) | limit bypasses, off-by-one offsets, depth counter leaks after error | — |

### Adversarial test list for G3

Covering every applicable class in `akku-tdd` and `akku-security-audit`:

1. **Bounds & integer overflow:**
   - Empty input `b""` and 1/2/3-byte inputs for every 4-byte and 8-byte decoder (`UnexpectedEof { offset: 0, needed, remaining }`).
   - Truncation at every byte offset `0..encoded.len()` of a representative composite message (NFSv3-shaped struct with opaque, string, array, union, option).
   - Wire lengths `0`, `1`, `u32::MAX - 3`, `u32::MAX - 2`, `u32::MAX - 1`, `u32::MAX` for opaque, string, and array (must return `LengthLimit` or `LengthOverflow` immediately — never wrap, panic, or allocate).
   - `padded_len(usize::MAX)`, `padded_len(usize::MAX - 1)`, `padded_len(usize::MAX - 2)`, `padded_len(usize::MAX - 3)`.
   - `encode_opaque_bounded` / `encode_str_bounded` / `encode_array` at `len == max` (pass) and `len == max + 1` (fail with `LengthLimit`, buffer untouched or documented).
2. **Strict malformed rejection:**
   - Non-zero padding in **every** padding byte position (1st, 2nd, 3rd pad byte for lengths `1`, `2`, `3`, `5`, `6`, `7`) for fixed opaque, variable opaque, and string -> `Error::NonZeroPadding { offset, byte }` with exact offset and byte.
   - Invalid booleans (`2`, `-1`, `i32::MIN`, `i32::MAX`, `0x0100_0000`) in `decode_bool`, `decode_option`, and `decode_list` -> `Error::InvalidBool`.
   - Invalid UTF-8 sequences (lone continuation byte, overlong encoding, surrogate half, truncated multi-byte) in `decode_str`/`decode_string` -> `Error::InvalidUtf8 { offset, valid_up_to }`.
   - Trailing garbage bytes (1 byte, 4 zero bytes) passed to `decode` / `Decoder::finish` -> `Error::TrailingBytes { offset, remaining }`.
   - Unknown enum value and unknown union discriminant -> `Error::InvalidEnum` / `Error::InvalidDiscriminant`.
3. **DoS & resource amplification:**
   - Each limit tested at **exact limit** (succeeds) and **limit + 1** (returns the typed error fast):
     - `max_opaque_len` (opaque and string) -> `Error::LengthLimit`.
     - `max_array_len` (variable array and linked list) -> `Error::LengthLimit`.
     - `max_total_bytes` (decode and encode) -> `Error::TotalBytesLimit`.
     - `max_depth` (nested `nest` / structs / arrays / unions / `Option`) -> `Error::DepthLimit`.
   - **Pre-allocation bomb:** 4-byte wire buffer claiming array count `65_536` of large elements (or `u32::MAX` with raised limit) -> hits `UnexpectedEof` immediately without allocating `count * size_of::<T>()` bytes.
   - **Zero-byte (`void`) array CPU bomb:** 4-byte wire buffer `[0xff, 0xff, 0xff, 0xff]` decoded as `Vec<()>` -> rejected immediately with `Error::LengthLimit` before looping.
   - **Linked-list stack-overflow defense:** 10_000-element RFC 4506 §4.19 linked list (`encode_list` / `decode_list`) succeeds without blowing the stack (and rejects at `max_array_len + 1`).
   - **Depth counter balance:** failing decode inside `nest` does not corrupt subsequent reuse or underflow depth.
4. **Round-trips & protocol-shaped vectors:**
   - Exact RFC 4506 §7 *An Example of an XDR Data Description* (`struct file` with `filename`, `filetype` union, `owner`, `data`) verified byte-for-byte against hand-worked hex.
   - NFSv3-shaped composite round-trip: `fhandle3` (`opaque<64>`), `cookieverf3` (`opaque[8]`), `fattr3` (enums + `u32` + `u64`), `READDIR3resok` (`decode_list` of `entry3`), and `nfsstat3` discriminated union (`NFS3_OK = 0` vs `NFS3ERR_JUKEBOX = 10008`).
   - Edge values: `i32::{MIN, -1, 0, 1, MAX}`, `u32::{0, MAX}`, `i64::{MIN, MAX}`, `u64::{0, MAX}`, `f32`/`f64` `(-0.0, +0.0, ±inf, MIN_POSITIVE, subnormal, NaN bit-pattern round-trip)`, strings with embedded `\0`, CRLF, and multi-byte UTF-8.
5. **Buffer reuse & offset tracking:**
   - Encoding into a non-empty `Vec<u8>` (e.g., 4-byte RPC record header already present) preserves the prefix, reports `written()` relative to start, and counts `max_total_bytes` against bytes written by this `Encoder`.

## Open questions

1. **String encoding: UTF-8 (proposed) vs strict 7-bit US-ASCII (RFC 4506 §4.11)?**
   RFC 4506 §4.11 defines `string` as ASCII, while modern XDR libraries (`stellar/go-xdr`, `xdr-codec`, `serde-xdr`) decode strings as UTF-8 (a superset of ASCII), and NFSv3 filenames are arbitrary bytes (`decode_opaque`). Should `decode_str`/`decode_string` validate UTF-8 as proposed, or also offer an ASCII-only helper / `Limits` toggle?
2. **Trait names: `XdrEncode`/`XdrDecode` vs `Encode`/`Decode`?**
   `XdrEncode`/`XdrDecode` avoid scope collisions when imported alongside other codec traits (mirroring `akku::json::{ToJson, FromJson}`). Shorter `akku::xdr::{Encode, Decode}` matches `Namespace.md` rule 2 (module-qualified names). Which naming style do you prefer?
3. **Owned `XdrDecode: Sized` vs borrowed `XdrDecode<'a>`?**
   Proposed `XdrDecode` decodes owned values (keeping NFSv3 struct definitions lifetime-free), while zero-copy borrows live on `Decoder<'a>` (`decode_opaque_slice`, `decode_str`). Is that split right, or should the trait itself carry `'a`?
4. **Partial writes on `Encoder` error:**
   If `Encoder` fails mid-struct (e.g., `TotalBytesLimit` or `LengthLimit` on field 3 of 5), should `Encoder` leave already-written bytes in `&mut Vec<u8>` (fast, zero rollback overhead, documented in rustdoc like `akku::hex::decode_to_slice`), or truncate back to `start_len` inside `encode_into` / `nest`?
5. **Default `Limits` values:**
   Are `max_opaque_len = 16 MiB`, `max_array_len = 65_536`, `max_total_bytes = 64 MiB`, `max_depth = 64` the right defaults for megaghost / NFSv3 and general callers, or should `max_opaque_len` be tighter (e.g., `4 MiB`) by default?
6. **Macros in v1:**
   Should `xdr_struct!` and `xdr_enum!` ship in v1 as `macro_rules!` sugar inside `akku::xdr`, or wait until `akku::nfs` exercises the manual API first?
