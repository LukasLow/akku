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
//! # Rollback on encoder error or panic
//!
//! Every write on [`Encoder`] validates bounds and [`Limits::max_total_bytes`]
//! before mutating the output buffer:
//!
//! - Primitive and bounded-field writes (such as [`Encoder::encode_uint`],
//!   [`Encoder::encode_opaque_bounded`], and [`Encoder::encode_str_bounded`])
//!   write nothing on error and preserve all earlier completed writes in the
//!   same [`Encoder`].
//! - [`Encoder::nest`] (and the compound methods built on it) records a
//!   checkpoint on entry and truncates the buffer back to that checkpoint on
//!   error or panic, preserving writes made before entering the scope.
//! - [`Encoder::encode`] records a checkpoint (the buffer length at the call)
//!   and truncates back to it on error or panic, so a failed value leaves no
//!   half-written bytes while earlier writes stay intact.
//! - Top-level [`encode_into`] and [`encode_with`] start a fresh [`Encoder`],
//!   so on error or panic the buffer is back at `start_len`, preserving any
//!   pre-existing prefix bytes and leaving no half-written record behind.
//!
//! # Security
//!
//! Encoding and decoding are **not constant-time**; do not use them directly on
//! secret material where timing side channels matter. Untrusted wire lengths
//! and nesting depths are validated against [`Limits`] before allocating.
//! Decoded in-memory size still depends on the schema: each wire byte can
//! expand to roughly `size_of::<T>() / 4` bytes of `T` (for example a
//! `Vec<Option<FixedOpaque<N>>>` with large `N`). Keep `N` small for types
//! decoded from untrusted input, or lower [`Limits::max_array_len`].
//!
//! # Examples
//!
//! Round-tripping standard types with [`encode`] and [`decode`]:
//!
//! ```
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
//! ```
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

use std::fmt;

const DEFAULT_MAX_OPAQUE_LEN: u32 = 16 * 1024 * 1024;
const DEFAULT_MAX_ARRAY_LEN: u32 = 65_536;
const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_DEPTH: usize = 64;
const MAX_PREALLOC: usize = 256;
const MAX_PREALLOC_BYTES: usize = 4096;
const ZERO_PAD: [u8; 4] = [0u8; 4];

#[inline]
fn initial_array_capacity<T>(count: usize, remaining_bytes: usize) -> usize {
    let elem_size = std::mem::size_of::<T>();
    if elem_size == 0 {
        return 0;
    }
    let max_by_wire = remaining_bytes / 4;
    let max_by_mem = MAX_PREALLOC_BYTES / elem_size;
    count.min(MAX_PREALLOC).min(max_by_wire).min(max_by_mem)
}

/// Convenience result alias for [`akku::xdr`](self) operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Returns the number of zero padding bytes (`0..=3`) needed to align `len` to
/// a 4-byte XDR boundary.
///
/// Never overflows or panics for any `usize`.
///
/// # Examples
///
/// ```
/// assert_eq!(akku::xdr::padding_len(0), 0);
/// assert_eq!(akku::xdr::padding_len(1), 3);
/// assert_eq!(akku::xdr::padding_len(5), 3);
/// assert_eq!(akku::xdr::padding_len(8), 0);
/// ```
pub const fn padding_len(len: usize) -> usize {
    (4 - (len & 3)) & 3
}

/// Returns `len + padding_len(len)`, or `None` if rounding up overflows `usize`.
///
/// # Examples
///
/// ```
/// assert_eq!(akku::xdr::padded_len(0), Some(0));
/// assert_eq!(akku::xdr::padded_len(5), Some(8));
/// assert_eq!(akku::xdr::padded_len(usize::MAX), None);
/// ```
pub const fn padded_len(len: usize) -> Option<usize> {
    len.checked_add(padding_len(len))
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
/// ```
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
        Self {
            max_opaque_len: DEFAULT_MAX_OPAQUE_LEN,
            max_array_len: DEFAULT_MAX_ARRAY_LEN,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }

    /// Sets [`Self::max_opaque_len`] and returns the updated limits.
    pub const fn with_max_opaque_len(mut self, max: u32) -> Self {
        self.max_opaque_len = max;
        self
    }

    /// Sets [`Self::max_array_len`] and returns the updated limits.
    pub const fn with_max_array_len(mut self, max: u32) -> Self {
        self.max_array_len = max;
        self
    }

    /// Sets [`Self::max_total_bytes`] and returns the updated limits.
    pub const fn with_max_total_bytes(mut self, max: usize) -> Self {
        self.max_total_bytes = max;
        self
    }

    /// Sets [`Self::max_depth`] and returns the updated limits.
    pub const fn with_max_depth(mut self, max: usize) -> Self {
        self.max_depth = max;
        self
    }
}

/// Error returned by XDR encoding and decoding operations.
///
/// Every variant carries the byte `offset` in the input (for decoding) or
/// bytes written relative to the encoder's start position (for encoding).
///
/// # Examples
///
/// ```
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
    /// An array element consumed or produced zero wire bytes (for example
    /// `void` / `()` or [`FixedOpaque<0>`]).
    ///
    /// RFC 4506 §6.3 forbids `void` as an array element type; rejecting
    /// zero-byte elements on both encode and decode also prevents unbounded CPU
    /// and memory amplification from small wire counts. Empty arrays
    /// (`count == 0`) remain allowed.
    ZeroSizedElement {
        /// Byte offset where the zero-wire-byte element was encountered.
        offset: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof {
                offset,
                needed,
                remaining,
            } => write!(
                f,
                "unexpected EOF at offset {offset}: needed {needed} bytes, {remaining} remaining"
            ),
            Self::TrailingBytes { offset, remaining } => write!(
                f,
                "trailing bytes at offset {offset}: {remaining} bytes remaining"
            ),
            Self::NonZeroPadding { offset, byte } => write!(
                f,
                "non-zero XDR padding byte 0x{byte:02x} at offset {offset}"
            ),
            Self::InvalidBool { offset, value } => {
                write!(f, "invalid XDR boolean value {value} at offset {offset}")
            }
            Self::InvalidEnum { offset, value } => {
                write!(f, "invalid XDR enum value {value} at offset {offset}")
            }
            Self::InvalidDiscriminant { offset, value } => {
                write!(
                    f,
                    "invalid XDR union discriminant {value} at offset {offset}"
                )
            }
            Self::InvalidUtf8 {
                offset,
                valid_up_to,
            } => write!(
                f,
                "invalid UTF-8 in XDR string at offset {offset} (valid up to {valid_up_to} bytes)"
            ),
            Self::LengthLimit { offset, len, limit } => write!(
                f,
                "XDR length {len} exceeds limit {limit} at offset {offset}"
            ),
            Self::TotalBytesLimit { offset, limit } => write!(
                f,
                "XDR total bytes limit {limit} exceeded at offset {offset}"
            ),
            Self::DepthLimit { offset, limit } => write!(
                f,
                "XDR nesting depth limit {limit} exceeded at offset {offset}"
            ),
            Self::LengthOverflow { offset, len } => {
                write!(f, "XDR length {len} overflows usize at offset {offset}")
            }
            Self::ZeroSizedElement { offset } => {
                write!(f, "XDR array element has zero wire size at offset {offset}")
            }
        }
    }
}

impl std::error::Error for Error {}

struct EncoderNestGuard<'a, 'b> {
    enc: &'b mut Encoder<'a>,
    checkpoint: usize,
    committed: bool,
}

impl Drop for EncoderNestGuard<'_, '_> {
    fn drop(&mut self) {
        self.enc.depth -= 1;
        if !self.committed {
            self.enc.buf.truncate(self.checkpoint);
        }
    }
}

struct EncoderTruncateGuard<'a, 'b> {
    enc: &'b mut Encoder<'a>,
    truncate_to: usize,
    committed: bool,
}

impl Drop for EncoderTruncateGuard<'_, '_> {
    fn drop(&mut self) {
        if !self.committed {
            self.enc.buf.truncate(self.truncate_to);
        }
    }
}

struct DecoderDepthGuard<'a, 'b> {
    dec: &'b mut Decoder<'a>,
}

impl Drop for DecoderDepthGuard<'_, '_> {
    fn drop(&mut self) {
        self.dec.depth -= 1;
    }
}

/// Imperative XDR encoder writing big-endian, 4-byte-aligned bytes into a
/// caller-owned [`Vec<u8>`].
///
/// The buffer may already contain prefix bytes (for example an ONC-RPC record
/// header). [`Encoder::written`] and [`Limits::max_total_bytes`] are tracked
/// relative to the buffer's length when the `Encoder` was created
/// (`start_len`).
///
/// # Rollback semantics
///
/// - Primitive and bounded-field writes validate bounds before mutating `buf`;
///   on error they append nothing and leave earlier writes in `buf` intact.
/// - [`Encoder::nest`] (and compound helpers built on it) rolls `buf` back to
///   its entry checkpoint and restores `depth` on error or panic.
/// - [`Encoder::encode`] rolls `buf` back to its own call checkpoint on error
///   or panic, preserving earlier writes.
/// - [`encode_into`] and [`encode_with`] use a fresh `Encoder`, so on error
///   `buf` is back at `start_len`, preserving any pre-existing prefix bytes.
#[derive(Debug)]
pub struct Encoder<'a> {
    buf: &'a mut Vec<u8>,
    start_len: usize,
    limits: Limits,
    depth: usize,
}

impl<'a> Encoder<'a> {
    /// Creates an encoder appending to `buf` with default [`Limits`].
    pub fn new(buf: &'a mut Vec<u8>) -> Self {
        Self::with_limits(buf, Limits::new())
    }

    /// Creates an encoder appending to `buf` with custom [`Limits`].
    pub fn with_limits(buf: &'a mut Vec<u8>, limits: Limits) -> Self {
        let start_len = buf.len();
        Self {
            buf,
            start_len,
            limits,
            depth: 0,
        }
    }

    /// Returns the number of bytes written by this encoder since creation
    /// (excluding any prefix already in `buf`).
    pub fn written(&self) -> usize {
        self.buf.len().saturating_sub(self.start_len)
    }

    /// Returns the active [`Limits`] for this encoder.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    #[inline]
    fn reserve_bytes(&mut self, additional: usize) -> Result<()> {
        let offset = self.written();
        let Some(new_total) = offset.checked_add(additional) else {
            return Err(Error::LengthOverflow {
                offset,
                len: additional,
            });
        };
        if new_total > self.limits.max_total_bytes {
            return Err(Error::TotalBytesLimit {
                offset,
                limit: self.limits.max_total_bytes,
            });
        }
        Ok(())
    }

    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.reserve_bytes(bytes.len())?;
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    fn encode_opaque_inner(&mut self, bytes: &[u8], max_len: Option<u32>) -> Result<()> {
        let offset = self.written();
        let effective_limit = match max_len {
            Some(m) => m.min(self.limits.max_opaque_len),
            None => self.limits.max_opaque_len,
        };
        let Ok(len_u32) = u32::try_from(bytes.len()) else {
            return Err(Error::LengthLimit {
                offset,
                len: bytes.len(),
                limit: effective_limit as usize,
            });
        };
        if len_u32 > effective_limit {
            return Err(Error::LengthLimit {
                offset,
                len: bytes.len(),
                limit: effective_limit as usize,
            });
        }
        let Some(padded) = padded_len(bytes.len()) else {
            return Err(Error::LengthOverflow {
                offset,
                len: bytes.len(),
            });
        };
        let Some(total) = padded.checked_add(4) else {
            return Err(Error::LengthOverflow {
                offset,
                len: bytes.len(),
            });
        };
        self.reserve_bytes(total)?;
        let pad = padding_len(bytes.len());
        self.buf.reserve(total);
        self.buf.extend_from_slice(&len_u32.to_be_bytes());
        self.buf.extend_from_slice(bytes);
        if pad != 0 {
            self.buf.extend_from_slice(&ZERO_PAD[..pad]);
        }
        Ok(())
    }

    /// Encodes a signed 32-bit integer (`int`, RFC 4506 §4.1).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_int(&mut self, v: i32) -> Result<()> {
        self.write_bytes(&v.to_be_bytes())
    }

    /// Encodes an unsigned 32-bit integer (`unsigned int`, RFC 4506 §4.2).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_uint(&mut self, v: u32) -> Result<()> {
        self.write_bytes(&v.to_be_bytes())
    }

    /// Encodes a signed 32-bit enumeration value (`enum`, RFC 4506 §4.3).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_enum(&mut self, v: i32) -> Result<()> {
        self.encode_int(v)
    }

    /// Encodes a boolean (`bool`, RFC 4506 §4.4) as `0` (`false`) or `1`
    /// (`true`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_bool(&mut self, v: bool) -> Result<()> {
        self.encode_uint(u32::from(v))
    }

    /// Encodes a signed 64-bit integer (`hyper`, RFC 4506 §4.5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 8 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_hyper(&mut self, v: i64) -> Result<()> {
        self.write_bytes(&v.to_be_bytes())
    }

    /// Encodes an unsigned 64-bit integer (`unsigned hyper`, RFC 4506 §4.5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 8 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_uhyper(&mut self, v: u64) -> Result<()> {
        self.write_bytes(&v.to_be_bytes())
    }

    /// Encodes an IEEE 754 32-bit float (`float`, RFC 4506 §4.6) in big-endian
    /// bit order.
    ///
    /// Raw IEEE 754 bit patterns (`f32::to_bits`) pass through unchanged
    /// without NaN canonicalization or signaling-NaN quietening; non-canonical
    /// NaN payloads and `-0.0` are preserved verbatim.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 4 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_float(&mut self, v: f32) -> Result<()> {
        self.write_bytes(&v.to_bits().to_be_bytes())
    }

    /// Encodes an IEEE 754 64-bit float (`double`, RFC 4506 §4.7) in big-endian
    /// bit order.
    ///
    /// Raw IEEE 754 bit patterns (`f64::to_bits`) pass through unchanged
    /// without NaN canonicalization or signaling-NaN quietening; non-canonical
    /// NaN payloads and `-0.0` are preserved verbatim.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TotalBytesLimit`] if writing 8 bytes would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_double(&mut self, v: f64) -> Result<()> {
        self.write_bytes(&v.to_bits().to_be_bytes())
    }

    /// Encodes XDR `void` (RFC 4506 §4.16), writing zero bytes.
    pub fn encode_void(&mut self) -> Result<()> {
        Ok(())
    }

    /// Encodes fixed-length opaque data (`opaque[n]`, RFC 4506 §4.9), followed
    /// by `0..=3` zero padding bytes.
    ///
    /// Validates bounds before writing; on error, nothing is appended and
    /// earlier writes in the buffer are preserved.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthOverflow`] if rounding `bytes.len()` up to a
    /// 4-byte multiple overflows `usize`, or [`Error::TotalBytesLimit`] if the
    /// padded write would exceed [`Limits::max_total_bytes`].
    pub fn encode_fixed_opaque(&mut self, bytes: &[u8]) -> Result<()> {
        let offset = self.written();
        let Some(total) = padded_len(bytes.len()) else {
            return Err(Error::LengthOverflow {
                offset,
                len: bytes.len(),
            });
        };
        self.reserve_bytes(total)?;
        let pad = padding_len(bytes.len());
        self.buf.reserve(total);
        self.buf.extend_from_slice(bytes);
        if pad != 0 {
            self.buf.extend_from_slice(&ZERO_PAD[..pad]);
        }
        Ok(())
    }

    /// Encodes variable-length opaque data (`opaque<>`, RFC 4506 §4.10) bounded
    /// by [`Limits::max_opaque_len`].
    ///
    /// Validates bounds before writing; on error, nothing is appended and
    /// earlier writes in the buffer are preserved.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`] if `bytes.len()` exceeds
    /// [`Limits::max_opaque_len`], [`Error::LengthOverflow`] on length
    /// overflow, or [`Error::TotalBytesLimit`] if writing would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_opaque(&mut self, bytes: &[u8]) -> Result<()> {
        self.encode_opaque_inner(bytes, None)
    }

    /// Encodes variable-length opaque data (`opaque<max_len>`, RFC 4506 §4.10)
    /// bounded by `min(max_len, self.limits().max_opaque_len)`.
    ///
    /// Validates bounds before writing; on error, nothing is appended and
    /// earlier writes in the buffer are preserved.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`] if `bytes.len()` exceeds the effective
    /// bound, [`Error::LengthOverflow`] on length overflow, or
    /// [`Error::TotalBytesLimit`] if writing would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_opaque_bounded(&mut self, bytes: &[u8], max_len: u32) -> Result<()> {
        self.encode_opaque_inner(bytes, Some(max_len))
    }

    /// Encodes a UTF-8 string (`string<>`, RFC 4506 §4.11) bounded by
    /// [`Limits::max_opaque_len`].
    ///
    /// Validates bounds before writing; on error, nothing is appended and
    /// earlier writes in the buffer are preserved.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`] if `s.len()` exceeds
    /// [`Limits::max_opaque_len`], [`Error::LengthOverflow`] on length
    /// overflow, or [`Error::TotalBytesLimit`] if writing would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_str(&mut self, s: &str) -> Result<()> {
        self.encode_opaque_inner(s.as_bytes(), None)
    }

    /// Encodes a UTF-8 string (`string<max_len>`, RFC 4506 §4.11) bounded by
    /// `min(max_len, self.limits().max_opaque_len)`.
    ///
    /// Validates bounds before writing; on error, nothing is appended and
    /// earlier writes in the buffer are preserved.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthLimit`] if `s.len()` exceeds the effective bound,
    /// [`Error::LengthOverflow`] on length overflow, or
    /// [`Error::TotalBytesLimit`] if writing would exceed
    /// [`Limits::max_total_bytes`].
    pub fn encode_str_bounded(&mut self, s: &str, max_len: u32) -> Result<()> {
        self.encode_opaque_inner(s.as_bytes(), Some(max_len))
    }

    /// Runs `f` inside a depth-guarded compound scope.
    ///
    /// Increments the current depth before calling `f` and restores it when `f`
    /// exits (whether via `Ok`, `Err`, or unwind). Records the buffer length on
    /// entry as a checkpoint; if `f` returns an error or panics, the underlying
    /// buffer is truncated back to that checkpoint, preserving earlier writes
    /// made before entering this scope. If the depth limit is already reached
    /// on entry, nothing is written or truncated.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`] if entering the scope would exceed
    /// [`Limits::max_depth`], or any error returned by `f`.
    pub fn nest<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        if self.depth >= self.limits.max_depth {
            return Err(Error::DepthLimit {
                offset: self.written(),
                limit: self.limits.max_depth,
            });
        }
        let checkpoint = self.buf.len();
        self.depth += 1;
        let mut guard = EncoderNestGuard {
            enc: self,
            checkpoint,
            committed: false,
        };
        let res = f(guard.enc);
        if res.is_ok() {
            guard.committed = true;
        }
        res
    }

    /// Encodes a fixed-length array (`T[n]`, RFC 4506 §4.12) inside a
    /// depth-guarded scope.
    ///
    /// Symmetric with [`Decoder::decode_fixed_array_vec`], an element that
    /// writes zero wire bytes is rejected with [`Error::ZeroSizedElement`]
    /// when `items` is non-empty (RFC 4506 §6.3); empty arrays succeed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::ZeroSizedElement`], or any
    /// error returned by `encode_item`.
    pub fn encode_fixed_array<T>(
        &mut self,
        items: &[T],
        mut encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()> {
        self.nest(|enc| {
            for item in items {
                let before = enc.written();
                encode_item(enc, item)?;
                if enc.written() == before {
                    return Err(Error::ZeroSizedElement { offset: before });
                }
            }
            Ok(())
        })
    }

    /// Encodes a variable-length array (`T<max_count>`, RFC 4506 §4.13) inside
    /// a depth-guarded scope.
    ///
    /// The effective item limit is `min(max_count, limits.max_array_len)` when
    /// `max_count` is `Some`, or `limits.max_array_len` when `None`.
    ///
    /// Symmetric with [`Decoder::decode_array`], an element that writes zero
    /// wire bytes is rejected with [`Error::ZeroSizedElement`] when `items` is
    /// non-empty (RFC 4506 §6.3); empty arrays succeed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::LengthLimit`],
    /// [`Error::TotalBytesLimit`], [`Error::ZeroSizedElement`], or any error
    /// returned by `encode_item`.
    pub fn encode_array<T>(
        &mut self,
        items: &[T],
        max_count: Option<u32>,
        mut encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()> {
        self.nest(|enc| {
            let offset = enc.written();
            let effective_limit = match max_count {
                Some(m) => m.min(enc.limits.max_array_len),
                None => enc.limits.max_array_len,
            };
            let Ok(count_u32) = u32::try_from(items.len()) else {
                return Err(Error::LengthLimit {
                    offset,
                    len: items.len(),
                    limit: effective_limit as usize,
                });
            };
            if count_u32 > effective_limit {
                return Err(Error::LengthLimit {
                    offset,
                    len: items.len(),
                    limit: effective_limit as usize,
                });
            }
            enc.encode_uint(count_u32)?;
            for item in items {
                let before = enc.written();
                encode_item(enc, item)?;
                if enc.written() == before {
                    return Err(Error::ZeroSizedElement { offset: before });
                }
            }
            Ok(())
        })
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
        self.nest(|enc| match value {
            None => enc.encode_bool(false),
            Some(v) => {
                enc.encode_bool(true)?;
                encode_val(enc, v)
            }
        })
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
        mut encode_item: impl FnMut(&mut Self, &T) -> Result<()>,
    ) -> Result<()> {
        self.nest(|enc| {
            let offset = enc.written();
            let effective_limit = match max_count {
                Some(m) => m.min(enc.limits.max_array_len),
                None => enc.limits.max_array_len,
            };
            let Ok(count_u32) = u32::try_from(items.len()) else {
                return Err(Error::LengthLimit {
                    offset,
                    len: items.len(),
                    limit: effective_limit as usize,
                });
            };
            if count_u32 > effective_limit {
                return Err(Error::LengthLimit {
                    offset,
                    len: items.len(),
                    limit: effective_limit as usize,
                });
            }
            for item in items {
                enc.encode_bool(true)?;
                encode_item(enc, item)?;
            }
            enc.encode_bool(false)
        })
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
        self.nest(|enc| {
            enc.encode_int(discriminant)?;
            encode_arm(enc)
        })
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
        self.nest(|enc| {
            enc.encode_uint(discriminant)?;
            encode_arm(enc)
        })
    }

    /// Encodes `value` using its [`XdrEncode`] implementation.
    ///
    /// On error or panic, truncates the underlying buffer back to its length
    /// at the start of this call, so no half-written value is left behind and
    /// earlier writes (including any pre-existing prefix bytes) are preserved.
    ///
    /// # Errors
    ///
    /// Propagates any error returned by [`XdrEncode::encode`].
    pub fn encode<T: XdrEncode + ?Sized>(&mut self, value: &T) -> Result<()> {
        let truncate_to = self.buf.len();
        let mut guard = EncoderTruncateGuard {
            enc: self,
            truncate_to,
            committed: false,
        };
        let res = value.encode(guard.enc);
        if res.is_ok() {
            guard.committed = true;
        }
        res
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
    input: &'a [u8],
    pos: usize,
    limits: Limits,
    depth: usize,
}

impl<'a> Decoder<'a> {
    /// Creates a decoder over `input` with default [`Limits`].
    pub fn new(input: &'a [u8]) -> Self {
        Self::with_limits(input, Limits::new())
    }

    /// Creates a decoder over `input` with custom [`Limits`].
    pub fn with_limits(input: &'a [u8], limits: Limits) -> Self {
        Self {
            input,
            pos: 0,
            limits,
            depth: 0,
        }
    }

    /// Returns the current byte offset from the start of the input slice.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Returns the number of unread bytes remaining in the underlying input
    /// slice.
    ///
    /// This reflects the entire underlying slice regardless of
    /// [`Limits::max_total_bytes`]; it is not capped by the remaining byte
    /// budget.
    pub fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.pos)
    }

    /// Returns `true` if no unread bytes remain in the underlying input slice
    /// (independent of [`Limits::max_total_bytes`]).
    pub fn is_empty(&self) -> bool {
        self.pos >= self.input.len()
    }

    /// Returns the unread tail of the underlying input slice without advancing
    /// [`Self::position`].
    ///
    /// This exposes the entire remaining slice regardless of
    /// [`Limits::max_total_bytes`]. Constructing a fresh [`Decoder`] from
    /// `dec.rest()` starts a new `max_total_bytes` counter at zero.
    pub fn rest(&self) -> &'a [u8] {
        self.input.get(self.pos..).unwrap_or(&[])
    }

    /// Returns the active [`Limits`] for this decoder.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    #[inline]
    fn remaining_budget(&self) -> usize {
        self.remaining()
            .min(self.limits.max_total_bytes.saturating_sub(self.pos))
    }

    #[inline]
    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let offset = self.pos;
        let end = offset
            .checked_add(n)
            .ok_or(Error::LengthOverflow { offset, len: n })?;
        if end > self.limits.max_total_bytes {
            return Err(Error::TotalBytesLimit {
                offset,
                limit: self.limits.max_total_bytes,
            });
        }
        let Some(slice) = self.input.get(offset..end) else {
            return Err(Error::UnexpectedEof {
                offset,
                needed: n,
                remaining: self.remaining(),
            });
        };
        self.pos = end;
        Ok(slice)
    }

    #[inline]
    fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.read_bytes(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    /// Verifies that all bytes in the underlying input slice have been
    /// consumed.
    ///
    /// Unlike [`decode`] and [`decode_with`], callers driving [`Decoder::new`]
    /// or [`Decoder::with_limits`] directly must call `finish()` if unconsumed
    /// trailing bytes (including bytes past [`Limits::max_total_bytes`])
    /// should be rejected.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TrailingBytes`] if [`Self::remaining`] is non-zero.
    pub fn finish(self) -> Result<()> {
        let remaining = self.remaining();
        if remaining != 0 {
            return Err(Error::TrailingBytes {
                offset: self.pos,
                remaining,
            });
        }
        Ok(())
    }

    /// Decodes a signed 32-bit integer (`int`, RFC 4506 §4.1).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_int(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.read_array::<4>()?))
    }

    /// Decodes an unsigned 32-bit integer (`unsigned int`, RFC 4506 §4.2).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_uint(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.read_array::<4>()?))
    }

    /// Decodes a signed 32-bit enumeration word (`enum`, RFC 4506 §4.3) and
    /// converts it via `from_i32`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidEnum`] if `from_i32` returns `None`, or
    /// [`Error::UnexpectedEof`] / [`Error::TotalBytesLimit`].
    pub fn decode_enum<E>(&mut self, from_i32: impl FnOnce(i32) -> Option<E>) -> Result<E> {
        let offset = self.pos;
        let value = self.decode_int()?;
        from_i32(value).ok_or(Error::InvalidEnum { offset, value })
    }

    /// Decodes a strict boolean (`bool`, RFC 4506 §4.4): `0` yields `false` and
    /// `1` yields `true`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidBool`] for any wire word other than `0` or `1`,
    /// or [`Error::UnexpectedEof`] / [`Error::TotalBytesLimit`].
    pub fn decode_bool(&mut self) -> Result<bool> {
        let offset = self.pos;
        match self.decode_int()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(Error::InvalidBool { offset, value }),
        }
    }

    /// Decodes a signed 64-bit integer (`hyper`, RFC 4506 §4.5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_hyper(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.read_array::<8>()?))
    }

    /// Decodes an unsigned 64-bit integer (`unsigned hyper`, RFC 4506 §4.5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_uhyper(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.read_array::<8>()?))
    }

    /// Decodes an IEEE 754 32-bit float (`float`, RFC 4506 §4.6).
    ///
    /// Raw IEEE 754 bit patterns (`f32::from_bits`) pass through unchanged
    /// without NaN canonicalization or signaling-NaN quietening; non-canonical
    /// NaN payloads and `-0.0` are preserved verbatim.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_float(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.decode_uint()?))
    }

    /// Decodes an IEEE 754 64-bit float (`double`, RFC 4506 §4.7).
    ///
    /// Raw IEEE 754 bit patterns (`f64::from_bits`) pass through unchanged
    /// without NaN canonicalization or signaling-NaN quietening; non-canonical
    /// NaN payloads and `-0.0` are preserved verbatim.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedEof`] or [`Error::TotalBytesLimit`].
    pub fn decode_double(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.decode_uhyper()?))
    }

    /// Decodes XDR `void` (RFC 4506 §4.16), consuming zero bytes.
    pub fn decode_void(&mut self) -> Result<()> {
        Ok(())
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
        let start = self.pos;
        let total = padded_len(len).ok_or(Error::LengthOverflow { offset: start, len })?;
        let bytes = self.read_bytes(total)?;
        for (i, &byte) in bytes[len..].iter().enumerate() {
            if byte != 0 {
                return Err(Error::NonZeroPadding {
                    offset: start + len + i,
                    byte,
                });
            }
        }
        Ok(&bytes[..len])
    }

    /// Decodes a fixed-length opaque array (`opaque[N]`, RFC 4506 §4.9) and
    /// verifies that the `0..=3` alignment padding bytes are zero.
    ///
    /// # Stack usage
    ///
    /// Materializes `[u8; N]` by value on the call stack; a large `N` can
    /// overflow the thread stack even on empty input. For large fixed-length
    /// payloads, prefer [`Self::decode_fixed_opaque_slice`] or
    /// [`Self::decode_fixed_opaque_into`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthOverflow`], [`Error::TotalBytesLimit`],
    /// [`Error::UnexpectedEof`], or [`Error::NonZeroPadding`].
    pub fn decode_fixed_opaque<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.decode_fixed_opaque_slice(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    /// Decodes `dst.len()` fixed-length opaque bytes into `dst` and verifies
    /// that the `0..=3` alignment padding bytes are zero.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LengthOverflow`], [`Error::TotalBytesLimit`],
    /// [`Error::UnexpectedEof`], or [`Error::NonZeroPadding`].
    pub fn decode_fixed_opaque_into(&mut self, dst: &mut [u8]) -> Result<()> {
        let slice = self.decode_fixed_opaque_slice(dst.len())?;
        dst.copy_from_slice(slice);
        Ok(())
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
        let item_offset = self.pos;
        let len_u32 = self.decode_uint()?;
        let effective_limit = match max_len {
            Some(m) => m.min(self.limits.max_opaque_len),
            None => self.limits.max_opaque_len,
        };
        if len_u32 > effective_limit {
            return Err(Error::LengthLimit {
                offset: item_offset,
                len: len_u32 as usize,
                limit: effective_limit as usize,
            });
        }
        let len = usize::try_from(len_u32).map_err(|_| Error::LengthOverflow {
            offset: item_offset,
            len: usize::MAX,
        })?;
        self.decode_fixed_opaque_slice(len)
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
        self.decode_opaque_slice(max_len).map(<[u8]>::to_vec)
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
        let payload_offset = self.pos.saturating_add(4);
        let bytes = self.decode_opaque_slice(max_len)?;
        std::str::from_utf8(bytes).map_err(|err| Error::InvalidUtf8 {
            offset: payload_offset,
            valid_up_to: err.valid_up_to(),
        })
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
        self.decode_str(max_len).map(str::to_owned)
    }

    /// Runs `f` inside a depth-guarded compound scope.
    ///
    /// Increments the current depth before calling `f` and restores it when `f`
    /// exits (whether via `Ok`, `Err`, or unwind).
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`] if entering the scope would exceed
    /// [`Limits::max_depth`], or any error returned by `f`.
    pub fn nest<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        if self.depth >= self.limits.max_depth {
            return Err(Error::DepthLimit {
                offset: self.pos,
                limit: self.limits.max_depth,
            });
        }
        self.depth += 1;
        let guard = DecoderDepthGuard { dec: self };
        f(guard.dec)
    }

    /// Decodes a fixed-length array (`T[N]`, RFC 4506 §4.12) inside a
    /// depth-guarded scope.
    ///
    /// # Stack usage
    ///
    /// Decodes elements into a heap-allocated `Vec<T>` and then moves `[T; N]`
    /// by value onto the call stack; large `N * size_of::<T>()` can overflow
    /// the thread stack. For large fixed-length arrays, prefer
    /// [`Self::decode_fixed_array_vec`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::ZeroSizedElement`], or any
    /// error returned by `decode_item`.
    pub fn decode_fixed_array<T, const N: usize>(
        &mut self,
        decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<[T; N]> {
        let vec = self.decode_fixed_array_vec(N, decode_item)?;
        Ok(vec
            .try_into()
            .unwrap_or_else(|_| unreachable!("decoded exact fixed array length")))
    }

    /// Decodes a fixed-length array of `count` elements into a [`Vec<T>`]
    /// inside a depth-guarded scope.
    ///
    /// Initial pre-allocation is capped at `256` elements and grows
    /// incrementally as items decode.
    ///
    /// If `count > 0` and `decode_item` consumes zero wire bytes, decoding
    /// fails immediately with [`Error::ZeroSizedElement`] (RFC 4506 §6.3);
    /// `count == 0` succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::ZeroSizedElement`], or any
    /// error returned by `decode_item`.
    pub fn decode_fixed_array_vec<T>(
        &mut self,
        count: usize,
        mut decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        self.nest(|dec| {
            let mut out =
                Vec::with_capacity(initial_array_capacity::<T>(count, dec.remaining_budget()));
            for _ in 0..count {
                let before = dec.pos;
                let item = decode_item(dec)?;
                if dec.pos == before {
                    return Err(Error::ZeroSizedElement { offset: before });
                }
                out.push(item);
            }
            Ok(out)
        })
    }

    /// Decodes a variable-length array (`T<max_count>`, RFC 4506 §4.13) inside
    /// a depth-guarded scope.
    ///
    /// Initial pre-allocation is capped at `256` elements and grows
    /// incrementally so a truncated buffer with a large count cannot
    /// over-allocate.
    ///
    /// If `count > 0` and `decode_item` consumes zero wire bytes, decoding
    /// fails immediately with [`Error::ZeroSizedElement`] (RFC 4506 §6.3);
    /// `count == 0` succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::LengthLimit`],
    /// [`Error::TotalBytesLimit`], [`Error::UnexpectedEof`],
    /// [`Error::ZeroSizedElement`], or any error returned by `decode_item`.
    pub fn decode_array<T>(
        &mut self,
        max_count: Option<u32>,
        mut decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        self.nest(|dec| {
            let item_offset = dec.pos;
            let count_u32 = dec.decode_uint()?;
            let effective_limit = match max_count {
                Some(m) => m.min(dec.limits.max_array_len),
                None => dec.limits.max_array_len,
            };
            if count_u32 > effective_limit {
                return Err(Error::LengthLimit {
                    offset: item_offset,
                    len: count_u32 as usize,
                    limit: effective_limit as usize,
                });
            }
            let count = usize::try_from(count_u32).map_err(|_| Error::LengthOverflow {
                offset: item_offset,
                len: usize::MAX,
            })?;
            let mut out =
                Vec::with_capacity(initial_array_capacity::<T>(count, dec.remaining_budget()));
            for _ in 0..count {
                let before = dec.pos;
                let item = decode_item(dec)?;
                if dec.pos == before {
                    return Err(Error::ZeroSizedElement { offset: before });
                }
                out.push(item);
            }
            Ok(out)
        })
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
        self.nest(|dec| {
            if dec.decode_bool()? {
                Ok(Some(decode_val(dec)?))
            } else {
                Ok(None)
            }
        })
    }

    /// Iteratively decodes an RFC 4506 §4.19 optional-data linked list
    /// (`1, item_0, 1, item_1, ..., 0`).
    ///
    /// Uses O(1) call-stack depth regardless of list length, enforcing
    /// `min(max_count, limits.max_array_len)` on the element count.
    ///
    /// Zero-wire-byte items are accepted here and do not need a
    /// [`Error::ZeroSizedElement`] check: every element is preceded by a 4-byte
    /// `TRUE` presence flag, so iterations are bounded by the input length and
    /// [`Limits::max_total_bytes`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::DepthLimit`], [`Error::InvalidBool`],
    /// [`Error::LengthLimit`], [`Error::UnexpectedEof`],
    /// [`Error::TotalBytesLimit`], or any error returned by `decode_item`.
    pub fn decode_list<T>(
        &mut self,
        max_count: Option<u32>,
        mut decode_item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        self.nest(|dec| {
            let effective_limit = match max_count {
                Some(m) => m.min(dec.limits.max_array_len),
                None => dec.limits.max_array_len,
            } as usize;
            let mut out = Vec::new();
            loop {
                let flag_offset = dec.pos;
                if !dec.decode_bool()? {
                    break;
                }
                if out.len() >= effective_limit {
                    return Err(Error::LengthLimit {
                        offset: flag_offset,
                        len: out.len().saturating_add(1),
                        limit: effective_limit,
                    });
                }
                out.push(decode_item(dec)?);
            }
            Ok(out)
        })
    }

    /// Decodes a signed 32-bit union discriminant inside a depth-guarded scope
    /// and dispatches to `decode_arm`.
    ///
    /// This method does not construct [`Error::InvalidDiscriminant`] itself;
    /// callers build and return it from `decode_arm` for unknown tags. Because
    /// the 4-byte discriminant is already consumed when `decode_arm` runs, the
    /// discriminant's byte offset is `dec.position().saturating_sub(4)` (not
    /// `dec.position()`):
    ///
    /// ```
    /// use akku::xdr::{Decoder, Error};
    ///
    /// let wire = 99i32.to_be_bytes();
    /// let mut dec = Decoder::new(&wire);
    /// let err = dec
    ///     .decode_union_i32(|tag, d| match tag {
    ///         0 => d.decode_void(),
    ///         other => Err(Error::InvalidDiscriminant {
    ///             offset: d.position().saturating_sub(4),
    ///             value: other,
    ///         }),
    ///     })
    ///     .unwrap_err();
    /// assert_eq!(err, Error::InvalidDiscriminant { offset: 0, value: 99 });
    /// ```
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
        self.nest(|dec| {
            let tag = dec.decode_int()?;
            decode_arm(tag, dec)
        })
    }

    /// Decodes an unsigned 32-bit union discriminant inside a depth-guarded
    /// scope and dispatches to `decode_arm`.
    ///
    /// This method does not construct [`Error::InvalidDiscriminant`] itself;
    /// callers build and return it from `decode_arm` for unknown tags. Because
    /// the 4-byte discriminant is already consumed when `decode_arm` runs, pass
    /// `offset: dec.position().saturating_sub(4)` and `value: tag as i32` when
    /// constructing [`Error::InvalidDiscriminant`].
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
        self.nest(|dec| {
            let tag = dec.decode_uint()?;
            decode_arm(tag, dec)
        })
    }

    /// Decodes a value of type `T` using its [`XdrDecode`] implementation.
    ///
    /// # Errors
    ///
    /// Propagates any error returned by [`XdrDecode::decode`].
    pub fn decode<T: XdrDecode>(&mut self) -> Result<T> {
        T::decode(self)
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
///
/// # Stack usage
///
/// Stores `[u8; N]` inline by value; a large `N` can overflow the call stack
/// when constructed or decoded via [`XdrDecode`]. For large fixed-length
/// payloads, prefer [`Decoder::decode_fixed_opaque_slice`] or
/// [`Decoder::decode_fixed_opaque_into`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FixedOpaque<const N: usize>(pub [u8; N]);

impl XdrEncode for i32 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_int(*self)
    }
}

impl XdrDecode for i32 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_int()
    }
}

impl XdrEncode for u32 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_uint(*self)
    }
}

impl XdrDecode for u32 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_uint()
    }
}

impl XdrEncode for i64 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_hyper(*self)
    }
}

impl XdrDecode for i64 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_hyper()
    }
}

impl XdrEncode for u64 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_uhyper(*self)
    }
}

impl XdrDecode for u64 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_uhyper()
    }
}

impl XdrEncode for bool {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_bool(*self)
    }
}

impl XdrDecode for bool {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_bool()
    }
}

impl XdrEncode for f32 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_float(*self)
    }
}

impl XdrDecode for f32 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_float()
    }
}

impl XdrEncode for f64 {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_double(*self)
    }
}

impl XdrDecode for f64 {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_double()
    }
}

impl XdrEncode for () {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_void()
    }
}

impl XdrDecode for () {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_void()
    }
}

impl XdrEncode for str {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_str(self)
    }
}

impl XdrEncode for String {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_str(self)
    }
}

impl XdrDecode for String {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_string(None)
    }
}

impl XdrEncode for Opaque {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_opaque(&self.0)
    }
}

impl XdrDecode for Opaque {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_opaque(None).map(Self)
    }
}

impl<const N: usize> XdrEncode for FixedOpaque<N> {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_fixed_opaque(&self.0)
    }
}

impl<const N: usize> XdrDecode for FixedOpaque<N> {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_fixed_opaque::<N>().map(Self)
    }
}

impl<T: XdrEncode> XdrEncode for Option<T> {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_option(self.as_ref(), |e, v| v.encode(e))
    }
}

impl<T: XdrDecode> XdrDecode for Option<T> {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_option(T::decode)
    }
}

impl<T: XdrEncode> XdrEncode for [T] {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_array(self, None, |e, v| v.encode(e))
    }
}

impl<T: XdrEncode> XdrEncode for Vec<T> {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_array(self, None, |e, v| v.encode(e))
    }
}

impl<T: XdrDecode> XdrDecode for Vec<T> {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_array(None, T::decode)
    }
}

impl<T: XdrEncode, const N: usize> XdrEncode for [T; N] {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.encode_fixed_array(self, |e, v| v.encode(e))
    }
}

impl<T: XdrDecode, const N: usize> XdrDecode for [T; N] {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_fixed_array(T::decode)
    }
}

impl<T: XdrEncode + ?Sized> XdrEncode for Box<T> {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.nest(|e| (**self).encode(e))
    }
}

impl<T: XdrDecode> XdrDecode for Box<T> {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.nest(|d| T::decode(d).map(Self::new))
    }
}

impl<T: XdrEncode + ?Sized> XdrEncode for &T {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        (*self).encode(enc)
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
/// ```
/// let encoded = akku::xdr::encode(&42u32)?;
/// assert_eq!(encoded, [0x00, 0x00, 0x00, 0x2a]);
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn encode<T: XdrEncode + ?Sized>(value: &T) -> Result<Vec<u8>> {
    encode_with(value, Limits::new())
}

/// Encodes `value` by appending to `buf` using default [`Limits`].
///
/// Any pre-existing bytes in `buf` are preserved. On error or panic, `buf` is
/// truncated back to its original length before returning or unwinding.
///
/// # Errors
///
/// Returns any [`Error`] produced while encoding `value`.
///
/// # Examples
///
/// ```
/// let mut buf = vec![0xde, 0xad, 0xbe, 0xef];
/// akku::xdr::encode_into(&1u32, &mut buf)?;
/// assert_eq!(buf, [0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x01]);
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn encode_into<T: XdrEncode + ?Sized>(value: &T, buf: &mut Vec<u8>) -> Result<()> {
    Encoder::new(buf).encode(value)
}

/// Encodes `value` into a newly allocated [`Vec<u8>`] with custom [`Limits`].
///
/// # Errors
///
/// Returns any [`Error`] produced while encoding `value`.
///
/// # Examples
///
/// ```
/// use akku::xdr::{self, Limits};
///
/// let limits = Limits::new().with_max_total_bytes(8);
/// let encoded = xdr::encode_with(&1u64, limits)?;
/// assert_eq!(encoded.len(), 8);
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn encode_with<T: XdrEncode + ?Sized>(value: &T, limits: Limits) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    Encoder::with_limits(&mut buf, limits).encode(value)?;
    Ok(buf)
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
/// ```
/// let value: i32 = akku::xdr::decode(&[0xff, 0xff, 0xff, 0xff])?;
/// assert_eq!(value, -1);
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn decode<T: XdrDecode>(input: &[u8]) -> Result<T> {
    decode_with(input, Limits::new())
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
/// ```
/// use akku::xdr::{self, Limits};
///
/// let limits = Limits::new().with_max_opaque_len(16);
/// let s: String = xdr::decode_with(b"\x00\x00\x00\x02hi\x00\x00", limits)?;
/// assert_eq!(s, "hi");
/// # Ok::<(), akku::xdr::Error>(())
/// ```
pub fn decode_with<T: XdrDecode>(input: &[u8], limits: Limits) -> Result<T> {
    let mut dec = Decoder::with_limits(input, limits);
    let value = dec.decode()?;
    dec.finish()?;
    Ok(value)
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    // Public-API tests cannot inspect the internal `Vec` capacity before
    // element 0 fails and drops the `Vec`, so we verify `initial_array_capacity`
    // and `remaining_budget` directly here.
    #[test]
    fn initial_array_capacity_is_zero_above_4096_bytes_and_respects_budget() {
        assert_eq!(initial_array_capacity::<[u8; 4097]>(1, 4096), 0);
        assert_eq!(initial_array_capacity::<[u8; 65_536]>(65_536, 65_536), 0);
        assert_eq!(initial_array_capacity::<[u8; 4096]>(4, 4096), 1);
        assert_eq!(initial_array_capacity::<u32>(256, 0), 0);

        let mut wire = vec![0u8; 4096];
        wire[..4].copy_from_slice(&256u32.to_be_bytes());
        let mut dec = Decoder::with_limits(&wire, Limits::new().with_max_total_bytes(4));
        assert_eq!(dec.decode_uint().unwrap(), 256);
        assert_eq!(dec.remaining(), 4092);
        assert_eq!(dec.remaining_budget(), 0);
        assert_eq!(
            initial_array_capacity::<u32>(256, dec.remaining_budget()),
            0
        );
    }
}
