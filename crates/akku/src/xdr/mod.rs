//! Strict big-endian XDR (External Data Representation, RFC 4506 / STD 67)
//! binary encoding and decoding.
//!
//! Every XDR item is encoded in big-endian byte order and aligned to a 4-byte
//! boundary with `0..=3` trailing zero padding bytes.
//!
//! # Overview
//!
//! | Task | API |
//! |---|---|
//! | One-shot encode / decode | [`encode`], [`encode_into`], [`encode_with`], [`decode`], [`decode_with`] |
//! | Imperative codec | [`Encoder`], [`Decoder`] |
//! | Composite types | [`XdrEncode`], [`XdrDecode`], [`Opaque`], [`FixedOpaque`] |
//! | Alignment math | [`padding_len`], [`padded_len`] |
//! | Resource bounds | [`Limits`] |
//!
//! # Strict decoding rules
//!
//! - **Zero padding:** every alignment padding byte (`0..=3` bytes after fixed
//!   opaque, variable opaque, and strings) must be `0x00`; any non-zero byte
//!   returns [`Error::NonZeroPadding`].
//! - **Booleans:** wire values must be `0` (`false`) or `1` (`true`); any other
//!   32-bit word returns [`Error::InvalidBool`].
//! - **Strings:** [`Decoder::decode_str`] and [`Decoder::decode_string`]
//!   validate UTF-8 and return [`Error::InvalidUtf8`] on invalid sequences.
//!   Protocols that carry arbitrary non-UTF-8 byte strings (such as OS paths or
//!   NFSv3 filenames) should use [`Decoder::decode_opaque_slice`] or
//!   [`Decoder::decode_opaque`].
//! - **Trailing bytes:** [`decode`], [`decode_with`], and [`Decoder::finish`]
//!   reject unconsumed input with [`Error::TrailingBytes`]. To decode several
//!   concatenated messages from one slice, drive [`Decoder`] directly.
//! - **Byte arrays vs. opaque:** `u8` and `i8` intentionally do not implement
//!   [`XdrEncode`] or [`XdrDecode`]. Wrap variable-length byte payloads in
//!   [`Opaque`] and fixed-length byte payloads in [`FixedOpaque`].
//!
//! # Limits and defaults
//!
//! All encoding and decoding operations enforce [`Limits`]:
//!
//! - [`Limits::max_opaque_len`]: `16 * 1024 * 1024` (16 MiB) per variable-length
//!   opaque or string.
//! - [`Limits::max_array_len`]: `65_536` elements per variable-length array or
//!   optional-data linked list.
//! - [`Limits::max_total_bytes`]: `64 * 1024 * 1024` (64 MiB) cumulative bytes
//!   read by a [`Decoder`] or written by an [`Encoder`].
//! - [`Limits::max_depth`]: `64` nested compound scopes (`nest`, structs,
//!   unions, arrays, and optional values).
//!
//! Per-field bounds (`max_len` / `max_count`) combine with [`Limits`] as
//! `min(field_bound, limits_bound)` so a field can tighten its bound without
//! raising the global cap. Variable-length array and list decoders cap initial
//! `Vec` pre-allocation at `256` elements and grow incrementally as elements
//! decode.
//!
//! # Rollback on encoder error
//!
//! Every write on [`Encoder`] checks [`Limits::max_total_bytes`] before growing
//! the output buffer. On any error, [`encode_into`] and [`Encoder::nest`]
//! truncate the buffer back to the `start_len` recorded when the [`Encoder`]
//! was created, preserving any pre-existing prefix bytes and leaving no partial
//! record behind.
//!
//! # Security
//!
//! Encoding and decoding are **not constant-time**; do not use them directly on
//! secret material where timing side channels matter. Untrusted wire lengths
//! and nesting depths are validated against [`Limits`] before allocating.
//!
//! # Examples
//!
//! Round-tripping standard types with [`encode`] and [`decode`]:
//!
//! ```no_run
//! use akku::xdr::{self, FixedOpaque, Opaque};
//!
//! let bytes = xdr::encode("hello")?;
//! assert_eq!(bytes, b"\x00\x00\x00\x05hello\x00\x00\x00");
//! assert_eq!(xdr::decode::<String>(&bytes)?, "hello");
//!
//! let handle = Opaque(vec![0xde, 0xad, 0xbe]);
//! let encoded = xdr::encode(&handle)?;
//! assert_eq!(xdr::decode::<Opaque>(&encoded)?, handle);
//!
//! let verifier = FixedOpaque([1, 2, 3, 4, 5]);
//! let encoded = xdr::encode(&verifier)?;
//! assert_eq!(encoded.len(), 8);
//! assert_eq!(xdr::decode::<FixedOpaque<5>>(&encoded)?, verifier);
//! # Ok::<(), akku::xdr::Error>(())
//! ```
//!
//! Imperative encoding and zero-copy decoding with [`Encoder`] and [`Decoder`]:
//!
//! ```no_run
//! use akku::xdr::{Decoder, Encoder};
//!
//! let mut buf = Vec::new();
//! let mut enc = Encoder::new(&mut buf);
//! enc.encode_uint(42)?;
//! enc.encode_str_bounded("hi", 16)?;
//!
//! let mut dec = Decoder::new(&buf);
//! assert_eq!(dec.decode_uint()?, 42);
//! assert_eq!(dec.decode_str(Some(16))?, "hi");
//! dec.finish()?;
//! # Ok::<(), akku::xdr::Error>(())
//! ```

// G3 stub-only allows (remove in G4 when bodies are implemented).
#![allow(unused_variables, clippy::ptr_arg)]

use std::fmt;
use std::marker::PhantomData;

/// Convenience result alias for [`akku::xdr`](self) operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Returns the number of zero padding bytes (`0..=3`) needed to align `len` to
/// a 4-byte XDR boundary.
///
/// Never overflows or panics for any `usize`.
///
/// # Examples
///
/// ```no_run
/// assert_eq!(akku::xdr::padding_len(0), 0);
/// assert_eq!(akku::xdr::padding_len(1), 3);
/// assert_eq!(akku::xdr::padding_len(5), 3);
/// assert_eq!(akku::xdr::padding_len(8), 0);
/// ```
pub const fn padding_len(len: usize) -> usize {
    todo!()
}

/// Returns `len + padding_len(len)`, or `None` if rounding up overflows `usize`.
///
/// # Examples
///
/// ```no_run
/// assert_eq!(akku::xdr::padded_len(0), Some(0));
/// assert_eq!(akku::xdr::padded_len(5), Some(8));
/// assert_eq!(akku::xdr::padded_len(usize::MAX), None);
/// ```
pub const fn padded_len(len: usize) -> Option<usize> {
    todo!()
}

/// Resource limits enforced by [`Encoder`] and [`Decoder`] against untrusted
/// input and accidental blowup.
///
/// # Defaults
///
/// - `max_opaque_len`: `16 * 1024 * 1024` (16 MiB)
/// - `max_array_len`: `65_536`
/// - `max_total_bytes`: `64 * 1024 * 1024` (64 MiB)
/// - `max_depth`: `64`
///
/// # Examples
///
/// ```no_run
/// use akku::xdr::Limits;
///
/// let limits = Limits::new()
///     .with_max_opaque_len(1024)
///     .with_max_array_len(128)
///     .with_max_total_bytes(8192)
///     .with_max_depth(16);
/// assert_eq!(limits.max_opaque_len, 1024);
/// assert_eq!(limits.max_depth, 16);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum byte length of a single variable-length opaque or string.
    ///
    /// Default: 16 MiB (`16 * 1024 * 1024`).
    pub max_opaque_len: u32,
    /// Maximum element count of a single variable-length array or optional-data
    /// linked list.
    ///
    /// Default: `65_536`.
    pub max_array_len: u32,
    /// Maximum cumulative bytes read by a [`Decoder`] or written by an
    /// [`Encoder`].
    ///
    /// Default: 64 MiB (`64 * 1024 * 1024`).
    pub max_total_bytes: usize,
    /// Maximum nesting depth of structs, unions, arrays, and optional-data.
    ///
    /// Default: `64`.
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

impl Limits {
    /// Creates a `Limits` value initialized with the default bounds.
    pub const fn new() -> Self {
        todo!()
    }

    /// Sets [`Self::max_opaque_len`] and returns the updated limits.
    pub const fn with_max_opaque_len(self, max: u32) -> Self {
        todo!()
    }

    /// Sets [`Self::max_array_len`] and returns the updated limits.
    pub const fn with_max_array_len(self, max: u32) -> Self {
        todo!()
    }

    /// Sets [`Self::max_total_bytes`] and returns the updated limits.
    pub const fn with_max_total_bytes(self, max: usize) -> Self {
        todo!()
    }

    /// Sets [`Self::max_depth`] and returns the updated limits.
    pub const fn with_max_depth(self, max: usize) -> Self {
        todo!()
    }
}

/// Error returned by XDR encoding and decoding operations.
///
/// Every variant carries the byte `offset` in the input (for decoding) or
/// bytes written relative to the encoder's start position (for encoding).
///
/// # Examples
///
/// ```no_run
/// use akku::xdr::{self, Error};
///
/// assert_eq!(
///     xdr::decode::<bool>(&[0, 0, 0, 2]),
///     Err(Error::InvalidBool { offset: 0, value: 2 }),
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Input ended before the required bytes could be read.
    UnexpectedEof {
        /// Byte offset where the read began.
        offset: usize,
        /// Number of bytes required by the operation.
        needed: usize,
        /// Number of bytes remaining in the input slice.
        remaining: usize,
    },
    /// Unconsumed bytes remained when [`Decoder::finish`] (or [`decode`]) was
    /// called.
    TrailingBytes {
        /// Byte offset where the unconsumed trailing bytes begin.
        offset: usize,
        /// Number of unconsumed bytes remaining.
        remaining: usize,
    },
    /// A padding byte in the `0..=3` alignment tail was non-zero.
    NonZeroPadding {
        /// Exact byte offset of the first non-zero padding byte.
        offset: usize,
        /// The offending non-zero byte value.
        byte: u8,
    },
    /// Boolean wire word was neither `0` (`false`) nor `1` (`true`).
    InvalidBool {
        /// Byte offset of the 4-byte boolean word.
        offset: usize,
        /// Decoded 32-bit signed wire value.
        value: i32,
    },
    /// Enumeration wire word was rejected by the caller's mapping.
    InvalidEnum {
        /// Byte offset of the 4-byte enumeration word.
        offset: usize,
        /// Decoded 32-bit signed wire value.
        value: i32,
    },
    /// Discriminated union tag was rejected by the caller.
    ///
    /// Unsigned discriminants are reported by their 32-bit pattern cast to
    /// `i32`.
    InvalidDiscriminant {
        /// Byte offset of the 4-byte discriminant word.
        offset: usize,
        /// Decoded 32-bit wire discriminant.
        value: i32,
    },
    /// String payload was not valid UTF-8.
    InvalidUtf8 {
        /// Byte offset where the string payload begins (`offset + valid_up_to`
        /// is the first invalid byte in the input).
        offset: usize,
        /// Number of valid UTF-8 bytes preceding the error.
        valid_up_to: usize,
    },
    /// A variable opaque, string, array, or list exceeded `u32::MAX`, its
    /// field bound, or [`Limits`].
    LengthLimit {
        /// Byte offset where the length-prefixed item began.
        offset: usize,
        /// Requested byte length or element count.
        len: usize,
        /// Effective maximum allowed length or count.
        limit: usize,
    },
    /// Cumulative encoded or decoded bytes exceeded
    /// [`Limits::max_total_bytes`].
    TotalBytesLimit {
        /// Byte offset at which the limit would be exceeded.
        offset: usize,
        /// Configured `max_total_bytes` limit.
        limit: usize,
    },
    /// Nesting depth exceeded [`Limits::max_depth`].
    DepthLimit {
        /// Byte offset at which the depth limit was exceeded.
        offset: usize,
        /// Configured `max_depth` limit.
        limit: usize,
    },
    /// Byte-length calculation overflowed `usize` (for example on 32-bit or
    /// `wasm32` targets).
    LengthOverflow {
        /// Byte offset where the overflowing length appeared.
        offset: usize,
        /// Offending length value.
        len: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("Error::fmt")
    }
}

impl std::error::Error for Error {}

/// Imperative XDR encoder writing big-endian, 4-byte-aligned bytes into a
/// caller-owned [`Vec<u8>`].
///
/// The buffer may already contain prefix bytes (for example an ONC-RPC record
/// header). [`Encoder::written`] and [`Limits::max_total_bytes`] are tracked
/// relative to the buffer's length when the `Encoder` was created
/// (`start_len`). On error, [`encode_into`] and [`Encoder::nest`] truncate the
/// buffer back to `start_len`, preserving the prefix.
#[derive(Debug)]
pub struct Encoder<'a> {
    _marker: PhantomData<&'a mut Vec<u8>>,
}

impl<'a> Encoder<'a> {
    /// Creates an encoder appending to `buf` with default [`Limits`].
    pub fn new(buf: &'a mut Vec<u8>) -> Self {
        todo!("Encoder::new")
    }

    /// Creates an encoder appending to `buf` with custom [`Limits`].
    pub fn with_limits(buf: &'a mut Vec<u8>, limits: Limits) -> Self {
        todo!("Encoder::with_limits")
    }

    /// Returns the number of bytes written by this encoder since creation
    /// (excluding any prefix already in `buf`).
    pub fn written(&self) -> usize {
        todo!("Encoder::written")
    }

    /// Returns the active [`Limits`] for this encoder.
    pub fn limits(&self) -> Limits {
        todo!("Encoder::limits")
    }

    /// Encodes a signed 32-bit integer (`int`, RFC 4506 §4.1).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_int(&mut self, v: i32) -> Result<()> {
        todo!("Encoder::encode_int")
    }

    /// Encodes an unsigned 32-bit integer (`unsigned int`, RFC 4506 §4.2).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_uint(&mut self, v: u32) -> Result<()> {
        todo!("Encoder::encode_uint")
    }

    /// Encodes a signed 32-bit enumeration value (`enum`, RFC 4506 §4.3).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_enum(&mut self, v: i32) -> Result<()> {
        todo!("Encoder::encode_enum")
    }

    /// Encodes a boolean (`bool`, RFC 4506 §4.4) as `0` (`false`) or `1`
    /// (`true`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_bool(&mut self, v: bool) -> Result<()> {
        todo!("Encoder::encode_bool")
    }

    /// Encodes a signed 64-bit integer (`hyper`, RFC 4506 §4.5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 8 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_hyper(&mut self, v: i64) -> Result<()> {
        todo!("Encoder::encode_hyper")
    }

    /// Encodes an unsigned 64-bit integer (`unsigned hyper`, RFC 4506 §4.5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 8 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_uhyper(&mut self, v: u64) -> Result<()> {
        todo!("Encoder::encode_uhyper")
    }

    /// Encodes an IEEE 754 32-bit float (`float`, RFC 4506 §4.6) in big-endian
    /// bit order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_float(&mut self, v: f32) -> Result<()> {
        todo!("Encoder::encode_float")
    }

    /// Encodes an IEEE 754 64-bit float (`double`, RFC 4506 §4.7) in big-endian
    /// bit order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 8 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_double(&mut self, v: f64) -> Result<()> {
        todo!("Encoder::encode_double")
    }

    /// Encodes XDR `void` (RFC 4506 §4.16), writing zero bytes.
    pub fn encode_void(&mut self) -> Result<()> {
        todo!("Encoder::encode_void")
    }

    /// Encodes fixed-length opaque data (`opaque[n]`, RFC 4506 §4.9), followed
    /// by `0..=3` zero padding bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthOverflow`] if rounding `bytes.len()` up to a
    /// 4-byte multiple overflows `usize`, or [`Error::TotalBytesLimit`] if the
    /// padded write would exceed [`Limits::max_total_bytes`].
    pub fn encode_fixed_opaque(&mut self, bytes: &[u8]) -> Result<()> {
        todo!("Encoder::encode_fixed_opaque")
    }

    /// Encodes variable-length opaque data (`opaque<>`, RFC 4506 §4.10) bounded
    /// by [`Limits::max_opaque_len`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`] if `bytes.len()` exceeds
    /// [`Limits::max_opaque_len`], [`Error::LengthOverflow`] on length
    /// overflow, or [`Error::TotalBytesLimit`] if writing would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_opaque(&mut self, bytes: &[u8]) -> Result<()> {
        todo!("Encoder::encode_opaque")
    }

    /// Encodes variable-length opaque data (`opaque<max_len>`, RFC 4506 §4.10)
    /// bounded by `min(max_len, self.limits().max_opaque_len)`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`] if `bytes.len()` exceeds the effective
    /// bound, [`Error::LengthOverflow`] on length overflow, or
    /// [`Error::TotalBytesLimit`] if writing would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_opaque_bounded(&mut self, bytes: &[u8], max_len: u32) -> Result<()> {
        todo!("Encoder::encode_opaque_bounded")
    }

    /// Encodes a UTF-8 string (`string<>`, RFC 4506 §4.11) bounded by
    /// [`Limits::max_opaque_len`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`] if `s.len()` exceeds
    /// [`Limits::max_opaque_len`], [`Error::LengthOverflow`] on length
    /// overflow, or [`Error::TotalBytesLimit`] if writing would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_str(&mut self, s: &str) -> Result<()> {
        todo!("Encoder::encode_str")
    }

    /// Encodes a UTF-8 string (`string<max_len>`, RFC 4506 §4.11) bounded by
    /// `min(max_len, self.limits().max_opaque_len)`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`] if `s.len()` exceeds the effective bound,
    /// [`Error::LengthOverflow`] on length overflow, or
    /// [`Error::TotalBytesLimit`] if writing would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_str_bounded(&mut self, s: &str, max_len: u32) -> Result<()> {
        todo!("Encoder::encode_str_bounded")
    }

    /// Runs `f` inside a depth-guarded compound scope.
    ///
    /// Increments the current depth before calling `f` and restores it when `f`
    /// returns. If `f` returns an error (or the depth limit is exceeded), the
    /// underlying buffer is truncated back to `start_len`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`] if entering the scope would exceed
    /// [`Limits::max_depth`], or any error returned by `f`.
    pub fn nest<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        todo!("Encoder::nest")
    }

    /// Encodes a fixed-length array (`T[n]`, RFC 4506 §4.12) inside a
    /// depth-guarded scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`] or any error returned by `encode_item`.
    pub fn encode_fixed_array<T>(
        &mut self,
        items: &[T],
        encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()> {
        todo!("Encoder::encode_fixed_array")
    }

    /// Encodes a variable-length array (`T<max_count>`, RFC 4506 §4.13) inside
    /// a depth-guarded scope.
    ///
    /// The effective item limit is `min(max_count, limits.max_array_len)` when
    /// `max_count` is `Some`, or `limits.max_array_len` when `None`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::LengthLimit`],
    /// [`Error::TotalBytesLimit`], or any error returned by `encode_item`.
    pub fn encode_array<T>(
        &mut self,
        items: &[T],
        max_count: Option<u32>,
        encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()> {
        todo!("Encoder::encode_array")
    }

    /// Encodes RFC 4506 §4.19 optional-data (`false` or `true` followed by the
    /// element) inside a depth-guarded scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::TotalBytesLimit`], or any error
    /// returned by `encode_val`.
    pub fn encode_option<T>(
        &mut self,
        value: Option<&T>,
        encode_val: impl FnOnce(&mut Self, &T) -> Result<()>,
    ) -> Result<()> {
        todo!("Encoder::encode_option")
    }

    /// Encodes a slice iteratively as an RFC 4506 §4.19 optional-data linked
    /// list (`1, item_0, 1, item_1, ..., 0`).
    ///
    /// Uses O(1) nesting depth regardless of `items.len()`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::LengthLimit`],
    /// [`Error::TotalBytesLimit`], or any error returned by `encode_item`.
    pub fn encode_list<T>(
        &mut self,
        items: &[T],
        max_count: Option<u32>,
        encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()> {
        todo!("Encoder::encode_list")
    }

    /// Encodes a discriminated union with a signed 32-bit discriminant
    /// (RFC 4506 §4.15) inside a depth-guarded scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::TotalBytesLimit`], or any error
    /// returned by `encode_arm`.
    pub fn encode_union_i32(
        &mut self,
        discriminant: i32,
        encode_arm: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        todo!("Encoder::encode_union_i32")
    }

    /// Encodes a discriminated union with an unsigned 32-bit discriminant
    /// (RFC 4506 §4.15) inside a depth-guarded scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::TotalBytesLimit`], or any error
    /// returned by `encode_arm`.
    pub fn encode_union_u32(
        &mut self,
        discriminant: u32,
        encode_arm: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        todo!("Encoder::encode_union_u32")
    }

    /// Encodes `value` using its [`XdrEncode`] implementation.
    ///
    /// # Errors
    ///
    /// Propagates any error returned by [`XdrEncode::encode`].
    pub fn encode<T: XdrEncode + ?Sized>(&mut self, value: &T) -> Result<()> {
        todo!("Encoder::encode")
    }
}

/// Strict XDR decoder over a borrowed byte slice `&'a [u8]`.
///
/// Rejects truncated input, non-zero alignment padding, booleans outside
/// `0`/`1`, invalid UTF-8 strings, and any [`Limits`] violation. Zero-copy
/// methods ([`Decoder::decode_fixed_opaque_slice`],
/// [`Decoder::decode_opaque_slice`], [`Decoder::decode_str`]) borrow directly
/// from the input slice.
#[derive(Debug)]
pub struct Decoder<'a> {
    _marker: PhantomData<&'a [u8]>,
}

impl<'a> Decoder<'a> {
    /// Creates a decoder over `input` with default [`Limits`].
    pub fn new(input: &'a [u8]) -> Self {
        todo!("Decoder::new")
    }

    /// Creates a decoder over `input` with custom [`Limits`].
    pub fn with_limits(input: &'a [u8], limits: Limits) -> Self {
        todo!("Decoder::with_limits")
    }

    /// Returns the current byte offset from the start of the input slice.
    pub fn position(&self) -> usize {
        todo!("Decoder::position")
    }

    /// Returns the number of unread bytes remaining in the input slice.
    pub fn remaining(&self) -> usize {
        todo!("Decoder::remaining")
    }

    /// Returns `true` if no unread bytes remain.
    pub fn is_empty(&self) -> bool {
        todo!("Decoder::is_empty")
    }

    /// Returns the unread tail of the input slice.
    pub fn rest(&self) -> &'a [u8] {
        todo!("Decoder::rest")
    }

    /// Returns the active [`Limits`] for this decoder.
    pub fn limits(&self) -> Limits {
        todo!("Decoder::limits")
    }

    /// Verifies that all input bytes have been consumed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TrailingBytes`] if [`Self::remaining`] is non-zero.
    pub fn finish(self) -> Result<()> {
        todo!("Decoder::finish")
    }

    /// Decodes a signed 32-bit integer (`int`, RFC 4506 §4.1).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_int(&mut self) -> Result<i32> {
        todo!("Decoder::decode_int")
    }

    /// Decodes an unsigned 32-bit integer (`unsigned int`, RFC 4506 §4.2).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_uint(&mut self) -> Result<u32> {
        todo!("Decoder::decode_uint")
    }

    /// Decodes a signed 32-bit enumeration word (`enum`, RFC 4506 §4.3) and
    /// converts it via `from_i32`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidEnum`] if `from_i32` returns `None`, or
    /// [`Error::UnexpectedEof`] / [`Error::TotalBytesLimit`].
    pub fn decode_enum<E>(&mut self, from_i32: impl FnOnce(i32) -> Option<E>) -> Result<E> {
        todo!("Decoder::decode_enum")
    }

    /// Decodes a strict boolean (`bool`, RFC 4506 §4.4): `0` yields `false` and
    /// `1` yields `true`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidBool`] for any wire word other than `0` or `1`,
    /// or [`Error::UnexpectedEof`] / [`Error::TotalBytesLimit`].
    pub fn decode_bool(&mut self) -> Result<bool> {
        todo!("Decoder::decode_bool")
    }

    /// Decodes a signed 64-bit integer (`hyper`, RFC 4506 §4.5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_hyper(&mut self) -> Result<i64> {
        todo!("Decoder::decode_hyper")
    }

    /// Decodes an unsigned 64-bit integer (`unsigned hyper`, RFC 4506 §4.5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_uhyper(&mut self) -> Result<u64> {
        todo!("Decoder::decode_uhyper")
    }

    /// Decodes an IEEE 754 32-bit float (`float`, RFC 4506 §4.6).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_float(&mut self) -> Result<f32> {
        todo!("Decoder::decode_float")
    }

    /// Decodes an IEEE 754 64-bit float (`double`, RFC 4506 §4.7).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_double(&mut self) -> Result<f64> {
        todo!("Decoder::decode_double")
    }

    /// Decodes XDR `void` (RFC 4506 §4.16), consuming zero bytes.
    pub fn decode_void(&mut self) -> Result<()> {
        todo!("Decoder::decode_void")
    }

    /// Borrows `len` fixed-length opaque bytes (`opaque[len]`, RFC 4506 §4.9)
    /// directly from the input slice and verifies that the `0..=3` alignment
    /// padding bytes are zero.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthOverflow`], [`Error::TotalBytesLimit`],
    /// [`Error::UnexpectedEof`], or [`Error::NonZeroPadding`].
    pub fn decode_fixed_opaque_slice(&mut self, len: usize) -> Result<&'a [u8]> {
        todo!("Decoder::decode_fixed_opaque_slice")
    }

    /// Decodes a fixed-length opaque array (`opaque[N]`, RFC 4506 §4.9) and
    /// verifies that the `0..=3` alignment padding bytes are zero.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthOverflow`], [`Error::TotalBytesLimit`],
    /// [`Error::UnexpectedEof`], or [`Error::NonZeroPadding`].
    pub fn decode_fixed_opaque<const N: usize>(&mut self) -> Result<[u8; N]> {
        todo!("Decoder::decode_fixed_opaque")
    }

    /// Decodes `dst.len()` fixed-length opaque bytes into `dst` and verifies
    /// that the `0..=3` alignment padding bytes are zero.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthOverflow`], [`Error::TotalBytesLimit`],
    /// [`Error::UnexpectedEof`], or [`Error::NonZeroPadding`].
    pub fn decode_fixed_opaque_into(&mut self, dst: &mut [u8]) -> Result<()> {
        todo!("Decoder::decode_fixed_opaque_into")
    }

    /// Borrows variable-length opaque bytes (`opaque<>`, RFC 4506 §4.10)
    /// directly from the input slice and validates zero padding.
    ///
    /// The effective byte limit is `min(max_len, limits.max_opaque_len)` when
    /// `max_len` is `Some`, or `limits.max_opaque_len` when `None`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`], [`Error::LengthOverflow`],
    /// [`Error::TotalBytesLimit`], [`Error::UnexpectedEof`], or
    /// [`Error::NonZeroPadding`].
    pub fn decode_opaque_slice(&mut self, max_len: Option<u32>) -> Result<&'a [u8]> {
        todo!("Decoder::decode_opaque_slice")
    }

    /// Decodes variable-length opaque bytes (`opaque<>`, RFC 4506 §4.10) into
    /// an owned [`Vec<u8>`] and validates zero padding.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`], [`Error::LengthOverflow`],
    /// [`Error::TotalBytesLimit`], [`Error::UnexpectedEof`], or
    /// [`Error::NonZeroPadding`].
    pub fn decode_opaque(&mut self, max_len: Option<u32>) -> Result<Vec<u8>> {
        todo!("Decoder::decode_opaque")
    }

    /// Borrows a UTF-8 string (`string<>`, RFC 4506 §4.11) directly from the
    /// input slice, validating both zero padding and UTF-8.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`], [`Error::LengthOverflow`],
    /// [`Error::TotalBytesLimit`], [`Error::UnexpectedEof`],
    /// [`Error::NonZeroPadding`], or [`Error::InvalidUtf8`].
    pub fn decode_str(&mut self, max_len: Option<u32>) -> Result<&'a str> {
        todo!("Decoder::decode_str")
    }

    /// Decodes a UTF-8 string (`string<>`, RFC 4506 §4.11) into an owned
    /// [`String`], validating both zero padding and UTF-8.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`], [`Error::LengthOverflow`],
    /// [`Error::TotalBytesLimit`], [`Error::UnexpectedEof`],
    /// [`Error::NonZeroPadding`], or [`Error::InvalidUtf8`].
    pub fn decode_string(&mut self, max_len: Option<u32>) -> Result<String> {
        todo!("Decoder::decode_string")
    }

    /// Runs `f` inside a depth-guarded compound scope.
    ///
    /// Increments the current depth before calling `f` and restores it when `f`
    /// returns (whether `Ok` or `Err`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`] if entering the scope would exceed
    /// [`Limits::max_depth`], or any error returned by `f`.
    pub fn nest<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        todo!("Decoder::nest")
    }

    /// Decodes a fixed-length array (`T[N]`, RFC 4506 §4.12) inside a
    /// depth-guarded scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`] or any error returned by `decode_item`.
    pub fn decode_fixed_array<T, const N: usize>(
        &mut self,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<[T; N]> {
        todo!("Decoder::decode_fixed_array")
    }

    /// Decodes a fixed-length array of `count` elements into a [`Vec<T>`]
    /// inside a depth-guarded scope.
    ///
    /// Initial pre-allocation is capped at `256` elements and grows
    /// incrementally as items decode.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`] or any error returned by `decode_item`.
    pub fn decode_fixed_array_vec<T>(
        &mut self,
        count: usize,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        todo!("Decoder::decode_fixed_array_vec")
    }

    /// Decodes a variable-length array (`T<max_count>`, RFC 4506 §4.13) inside
    /// a depth-guarded scope.
    ///
    /// Initial pre-allocation is capped at `256` elements and grows
    /// incrementally so a truncated buffer with a large count cannot
    /// over-allocate.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::LengthLimit`],
    /// [`Error::TotalBytesLimit`], [`Error::UnexpectedEof`], or any error
    /// returned by `decode_item`.
    pub fn decode_array<T>(
        &mut self,
        max_count: Option<u32>,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        todo!("Decoder::decode_array")
    }

    /// Decodes RFC 4506 §4.19 optional-data (`0` -> `None`, `1` -> `Some(val)`)
    /// inside a depth-guarded scope.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::InvalidBool`],
    /// [`Error::UnexpectedEof`], [`Error::TotalBytesLimit`], or any error
    /// returned by `decode_val`.
    pub fn decode_option<T>(
        &mut self,
        decode_val: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<Option<T>> {
        todo!("Decoder::decode_option")
    }

    /// Iteratively decodes an RFC 4506 §4.19 optional-data linked list
    /// (`1, item_0, 1, item_1, ..., 0`).
    ///
    /// Uses O(1) call-stack depth regardless of list length, enforcing
    /// `min(max_count, limits.max_array_len)` on the element count.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::InvalidBool`],
    /// [`Error::LengthLimit`], [`Error::UnexpectedEof`],
    /// [`Error::TotalBytesLimit`], or any error returned by `decode_item`.
    pub fn decode_list<T>(
        &mut self,
        max_count: Option<u32>,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        todo!("Decoder::decode_list")
    }

    /// Decodes a signed 32-bit union discriminant inside a depth-guarded scope
    /// and dispatches to `decode_arm`.
    ///
    /// This method does not construct [`Error::InvalidDiscriminant`] itself;
    /// callers build and return it from `decode_arm` for unknown tags.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::UnexpectedEof`],
    /// [`Error::TotalBytesLimit`], or any error returned by `decode_arm` (such
    /// as [`Error::InvalidDiscriminant`]).
    pub fn decode_union_i32<T>(
        &mut self,
        decode_arm: impl FnOnce(i32, &mut Self) -> Result<T>,
    ) -> Result<T> {
        todo!("Decoder::decode_union_i32")
    }

    /// Decodes an unsigned 32-bit union discriminant inside a depth-guarded
    /// scope and dispatches to `decode_arm`.
    ///
    /// This method does not construct [`Error::InvalidDiscriminant`] itself;
    /// callers build and return it from `decode_arm` for unknown tags.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::UnexpectedEof`],
    /// [`Error::TotalBytesLimit`], or any error returned by `decode_arm` (such
    /// as [`Error::InvalidDiscriminant`]).
    pub fn decode_union_u32<T>(
        &mut self,
        decode_arm: impl FnOnce(u32, &mut Self) -> Result<T>,
    ) -> Result<T> {
        todo!("Decoder::decode_union_u32")
    }

    /// Decodes a value of type `T` using its [`XdrDecode`] implementation.
    ///
    /// # Errors
    ///
    /// Propagates any error returned by [`XdrDecode::decode`].
    pub fn decode<T: XdrDecode>(&mut self) -> Result<T> {
        todo!("Decoder::decode")
    }
}

/// Trait for types that can be encoded into XDR wire format.
///
/// Implemented for RFC 4506 primitives, `String`, `str`, [`Opaque`],
/// [`FixedOpaque`], `Option<T>`, `Vec<T>`, `[T]`, `[T; N]`, `Box<T>`, and `&T`.
pub trait XdrEncode {
    /// Writes `self` into `enc` in big-endian, 4-byte-aligned XDR format.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if encoding violates any [`Limits`] or field bound.
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()>;
}

/// Trait for owned types that can be decoded from XDR wire format.
pub trait XdrDecode: Sized {
    /// Reads a value of `Self` from `dec`.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the input is malformed, truncated, or violates
    /// [`Limits`].
    fn decode(dec: &mut Decoder<'_>) -> Result<Self>;
}

/// Variable-length XDR opaque bytes (`opaque<>`, RFC 4506 §4.10).
///
/// Distinct from `Vec<T>` variable-length arrays, whose elements are each
/// 4-byte aligned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Opaque(pub Vec<u8>);

/// Fixed-length XDR opaque bytes (`opaque[N]`, RFC 4506 §4.9), padded to a
/// 4-byte boundary.
///
/// Distinct from `[T; N]` fixed-length arrays, whose elements are each 4-byte
/// aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FixedOpaque<const N: usize>(pub [u8; N]);

impl XdrEncode for i32 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for i32")
    }
}

impl XdrDecode for i32 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for i32")
    }
}

impl XdrEncode for u32 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for u32")
    }
}

impl XdrDecode for u32 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for u32")
    }
}

impl XdrEncode for i64 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for i64")
    }
}

impl XdrDecode for i64 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for i64")
    }
}

impl XdrEncode for u64 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for u64")
    }
}

impl XdrDecode for u64 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for u64")
    }
}

impl XdrEncode for bool {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for bool")
    }
}

impl XdrDecode for bool {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for bool")
    }
}

impl XdrEncode for f32 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for f32")
    }
}

impl XdrDecode for f32 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for f32")
    }
}

impl XdrEncode for f64 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for f64")
    }
}

impl XdrDecode for f64 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for f64")
    }
}

impl XdrEncode for () {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for ()")
    }
}

impl XdrDecode for () {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for ()")
    }
}

impl XdrEncode for str {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for str")
    }
}

impl XdrEncode for String {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for String")
    }
}

impl XdrDecode for String {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for String")
    }
}

impl XdrEncode for Opaque {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for Opaque")
    }
}

impl XdrDecode for Opaque {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for Opaque")
    }
}

impl<const N: usize> XdrEncode for FixedOpaque<N> {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for FixedOpaque")
    }
}

impl<const N: usize> XdrDecode for FixedOpaque<N> {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for FixedOpaque")
    }
}

impl<T: XdrEncode> XdrEncode for Option<T> {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for Option")
    }
}

impl<T: XdrDecode> XdrDecode for Option<T> {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for Option")
    }
}

impl<T: XdrEncode> XdrEncode for [T] {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for [T]")
    }
}

impl<T: XdrEncode> XdrEncode for Vec<T> {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for Vec<T>")
    }
}

impl<T: XdrDecode> XdrDecode for Vec<T> {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for Vec<T>")
    }
}

impl<T: XdrEncode, const N: usize> XdrEncode for [T; N] {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for [T; N]")
    }
}

impl<T: XdrDecode, const N: usize> XdrDecode for [T; N] {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for [T; N]")
    }
}

impl<T: XdrEncode + ?Sized> XdrEncode for Box<T> {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for Box<T>")
    }
}

impl<T: XdrDecode> XdrDecode for Box<T> {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        todo!("XdrDecode for Box<T>")
    }
}

impl<T: XdrEncode + ?Sized> XdrEncode for &T {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        todo!("XdrEncode for &T")
    }
}

/// Encodes `value` into a newly allocated [`Vec<u8>`] using default [`Limits`].
///
/// # Errors
///
/// Returns any [`Error`] produced while encoding `value`.
///
/// # Examples
///
/// ```no_run
/// let encoded = akku::xdr::encode(&42u32)?;
/// assert_eq!(encoded, [0x00, 0x00, 0x00, 0x2a]);
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn encode<T: XdrEncode + ?Sized>(value: &T) -> Result<Vec<u8>> {
    todo!("xdr::encode")
}

/// Encodes `value` by appending to `buf` using default [`Limits`].
///
/// Any pre-existing bytes in `buf` are preserved. On error, `buf` is truncated
/// back to its original length before returning.
///
/// # Errors
///
/// Returns any [`Error`] produced while encoding `value`.
///
/// # Examples
///
/// ```no_run
/// let mut buf = vec![0xde, 0xad, 0xbe, 0xef];
/// akku::xdr::encode_into(&1u32, &mut buf)?;
/// assert_eq!(buf, [0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x01]);
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn encode_into<T: XdrEncode + ?Sized>(value: &T, buf: &mut Vec<u8>) -> Result<()> {
    todo!("xdr::encode_into")
}

/// Encodes `value` into a newly allocated [`Vec<u8>`] with custom [`Limits`].
///
/// # Errors
///
/// Returns any [`Error`] produced while encoding `value`.
///
/// # Examples
///
/// ```no_run
/// use akku::xdr::{self, Limits};
///
/// let limits = Limits::new().with_max_total_bytes(8);
/// let encoded = xdr::encode_with(&1u64, limits)?;
/// assert_eq!(encoded.len(), 8);
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn encode_with<T: XdrEncode + ?Sized>(value: &T, limits: Limits) -> Result<Vec<u8>> {
    todo!("xdr::encode_with")
}

/// Decodes an owned value of type `T` from `input` using default [`Limits`].
///
/// Rejects any trailing bytes after the decoded value with
/// [`Error::TrailingBytes`].
///
/// # Errors
///
/// Returns any [`Error`] encountered while decoding `T` or verifying that no
/// trailing bytes remain.
///
/// # Examples
///
/// ```no_run
/// let value: i32 = akku::xdr::decode(&[0xff, 0xff, 0xff, 0xff])?;
/// assert_eq!(value, -1);
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn decode<T: XdrDecode>(input: &[u8]) -> Result<T> {
    todo!("xdr::decode")
}

/// Decodes an owned value of type `T` from `input` with custom [`Limits`].
///
/// Rejects any trailing bytes after the decoded value with
/// [`Error::TrailingBytes`].
///
/// # Errors
///
/// Returns any [`Error`] encountered while decoding `T` or verifying that no
/// trailing bytes remain.
///
/// # Examples
///
/// ```no_run
/// use akku::xdr::{self, Limits};
///
/// let limits = Limits::new().with_max_opaque_len(16);
/// let s: String = xdr::decode_with(b"\x00\x00\x00\x02hi\x00\x00", limits)?;
/// assert_eq!(s, "hi");
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn decode_with<T: XdrDecode>(input: &[u8], limits: Limits) -> Result<T> {
    todo!("xdr::decode_with")
}
