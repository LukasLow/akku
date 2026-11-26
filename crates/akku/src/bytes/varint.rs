//! LEB128 varints and zigzag-encoded signed varints.
//!
//! This is the wire format of Go's `encoding/binary` (`PutUvarint`, `Uvarint`,
//! `AppendUvarint`, `PutVarint`, `Varint`, `AppendVarint`) and of protobuf
//! `uint64`/`int64`/`sint64` fields: seven payload bits per byte, least
//! significant group first, high bit set on every byte except the last.
//! Signed values use zigzag (`0, -1, 1, -2, ...` map to `0, 1, 2, 3, ...`).
//!
//! | Go | akku |
//! | --- | --- |
//! | `PutUvarint` | [`encode_u64`] |
//! | `AppendUvarint` | [`append_u64`] |
//! | `Uvarint` | [`decode_u64`] |
//! | `PutVarint` | [`encode_i64`] |
//! | `AppendVarint` | [`append_i64`] |
//! | `Varint` | [`decode_i64`] |
//! | `MaxVarintLen64` | [`MAX_VARINT_LEN64`] |
//!
//! Decoding follows Go: a run of bytes that ends before a terminating byte is
//! [`VarintError::Truncated`]; more than 10 bytes, or a 10th byte above `1`,
//! is [`VarintError::Overflow`]. Go and protobuf also accept *overlong*
//! encodings such as `[0x80, 0x00]` for zero; so do [`decode_u64`] and
//! [`decode_i64`]. [`decode_u64_strict`] and [`decode_i64_strict`] reject them
//! with [`VarintError::NonCanonical`] for canonical-form protocols.
//!
//! Instead of panicking like Go on a short buffer, [`encode_u64`] and
//! [`encode_i64`] return [`VarintError::BufferTooSmall`] and write nothing.
//!
//! ```
//! use akku::bytes::varint;
//!
//! let mut out = Vec::new();
//! varint::append_u64(&mut out, 300);
//! assert_eq!(out, [0xac, 0x02]);
//! assert_eq!(varint::decode_u64(&out), Ok((300, 2)));
//!
//! let mut buf = [0u8; varint::MAX_VARINT_LEN64];
//! let n = varint::encode_i64(&mut buf, -1).unwrap();
//! assert_eq!(&buf[..n], [0x01]);
//! ```

use std::fmt;

/// Maximum length in bytes of a varint-encoded 64-bit integer (Go's
/// `MaxVarintLen64`).
pub const MAX_VARINT_LEN64: usize = 10;

/// Why a varint could not be encoded or decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarintError {
    /// The input is empty or ends before a byte without the continuation bit.
    Truncated,
    /// The value does not fit in 64 bits: more than 10 bytes, or a 10th byte
    /// greater than `1`.
    Overflow,
    /// The encoding is valid but not minimal (for example `[0x80, 0x00]`);
    /// only reported by the `*_strict` decoders.
    NonCanonical,
    /// The destination slice is shorter than the encoded value.
    BufferTooSmall {
        /// Bytes the encoding needs.
        needed: usize,
        /// Bytes the destination offers.
        available: usize,
    },
}

impl fmt::Display for VarintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("varint truncated: input ended before the last byte"),
            Self::Overflow => f.write_str("varint overflows 64 bits"),
            Self::NonCanonical => f.write_str("varint is not in canonical (minimal) form"),
            Self::BufferTooSmall { needed, available } => write!(
                f,
                "buffer too small for varint: need {needed} byte(s), have {available}",
            ),
        }
    }
}

impl std::error::Error for VarintError {}

/// Maps a signed integer to an unsigned one so small magnitudes stay small:
/// `0, -1, 1, -2, 2, ...` become `0, 1, 2, 3, 4, ...`.
///
/// ```
/// use akku::bytes::varint::zigzag_encode;
/// assert_eq!(zigzag_encode(-2), 3);
/// ```
pub const fn zigzag_encode(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

/// Inverse of [`zigzag_encode`].
///
/// ```
/// use akku::bytes::varint::zigzag_decode;
/// assert_eq!(zigzag_decode(3), -2);
/// ```
pub const fn zigzag_decode(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// Returns how many bytes `value` occupies as a varint (1 to 10).
///
/// ```
/// use akku::bytes::varint::encoded_len_u64;
/// assert_eq!(encoded_len_u64(127), 1);
/// assert_eq!(encoded_len_u64(128), 2);
/// ```
pub const fn encoded_len_u64(value: u64) -> usize {
    let bits = 64 - (value | 1).leading_zeros() as usize;
    bits.div_ceil(7)
}

/// Encodes into a stack buffer, returning it with the used length.
fn encode_raw(mut value: u64) -> ([u8; MAX_VARINT_LEN64], usize) {
    let mut out = [0u8; MAX_VARINT_LEN64];
    let mut i = 0;
    while value >= 0x80 {
        out[i] = (value as u8) | 0x80;
        value >>= 7;
        i += 1;
    }
    out[i] = value as u8;
    (out, i + 1)
}

/// Writes `value` into the start of `buf` and returns the bytes written
/// (Go `PutUvarint`).
///
/// Writes nothing and returns [`VarintError::BufferTooSmall`] when `buf` is
/// shorter than [`encoded_len_u64`]`(value)`.
///
/// ```
/// use akku::bytes::varint::{encode_u64, VarintError};
///
/// let mut buf = [0u8; 2];
/// assert_eq!(encode_u64(&mut buf, 300), Ok(2));
/// assert_eq!(buf, [0xac, 0x02]);
/// assert_eq!(
///     encode_u64(&mut buf[..1], 300),
///     Err(VarintError::BufferTooSmall { needed: 2, available: 1 })
/// );
/// ```
pub fn encode_u64(buf: &mut [u8], value: u64) -> Result<usize, VarintError> {
    let (raw, len) = encode_raw(value);
    if buf.len() < len {
        return Err(VarintError::BufferTooSmall {
            needed: len,
            available: buf.len(),
        });
    }
    buf[..len].copy_from_slice(&raw[..len]);
    Ok(len)
}

/// Appends `value` as a varint to `out` (Go `AppendUvarint`).
///
/// ```
/// use akku::bytes::varint::append_u64;
///
/// let mut out = vec![0xff];
/// append_u64(&mut out, 1);
/// assert_eq!(out, [0xff, 0x01]);
/// ```
pub fn append_u64(out: &mut Vec<u8>, value: u64) {
    let (raw, len) = encode_raw(value);
    out.extend_from_slice(&raw[..len]);
}

/// Writes the zigzag varint of `value` into `buf` (Go `PutVarint`).
///
/// See [`encode_u64`] for the error behaviour.
///
/// ```
/// use akku::bytes::varint::encode_i64;
///
/// let mut buf = [0u8; 10];
/// let n = encode_i64(&mut buf, -65).unwrap();
/// assert_eq!(&buf[..n], [0x81, 0x01]);
/// ```
pub fn encode_i64(buf: &mut [u8], value: i64) -> Result<usize, VarintError> {
    encode_u64(buf, zigzag_encode(value))
}

/// Appends the zigzag varint of `value` to `out` (Go `AppendVarint`).
///
/// ```
/// use akku::bytes::varint::append_i64;
///
/// let mut out = Vec::new();
/// append_i64(&mut out, -1);
/// assert_eq!(out, [0x01]);
/// ```
pub fn append_i64(out: &mut Vec<u8>, value: i64) {
    append_u64(out, zigzag_encode(value));
}

/// Decodes one varint from the start of `buf`, returning the value and the
/// bytes consumed (Go `Uvarint`).
///
/// Accepts overlong encodings like Go and protobuf; see
/// [`decode_u64_strict`] to reject them.
///
/// ```
/// use akku::bytes::varint::{decode_u64, VarintError};
///
/// assert_eq!(decode_u64(&[0xac, 0x02, 0xff]), Ok((300, 2)));
/// assert_eq!(decode_u64(&[0x80]), Err(VarintError::Truncated));
/// assert_eq!(decode_u64(&[0x80, 0x00]), Ok((0, 2)));
/// ```
pub fn decode_u64(buf: &[u8]) -> Result<(u64, usize), VarintError> {
    let mut value = 0u64;
    for (i, &byte) in buf.iter().enumerate() {
        if i == MAX_VARINT_LEN64 {
            return Err(VarintError::Overflow);
        }
        if byte < 0x80 {
            if i == MAX_VARINT_LEN64 - 1 && byte > 1 {
                return Err(VarintError::Overflow);
            }
            return Ok((value | u64::from(byte) << (7 * i), i + 1));
        }
        value |= u64::from(byte & 0x7f) << (7 * i);
    }
    Err(VarintError::Truncated)
}

/// Like [`decode_u64`] but rejects non-minimal encodings with
/// [`VarintError::NonCanonical`].
///
/// ```
/// use akku::bytes::varint::{decode_u64_strict, VarintError};
///
/// assert_eq!(decode_u64_strict(&[0x00]), Ok((0, 1)));
/// assert_eq!(decode_u64_strict(&[0x80, 0x00]), Err(VarintError::NonCanonical));
/// ```
pub fn decode_u64_strict(buf: &[u8]) -> Result<(u64, usize), VarintError> {
    let (value, len) = decode_u64(buf)?;
    // A minimal encoding never ends in a zero byte unless it is the only byte.
    if len > 1 && buf[len - 1] == 0 {
        return Err(VarintError::NonCanonical);
    }
    Ok((value, len))
}

/// Decodes one zigzag varint, returning the value and bytes consumed (Go
/// `Varint`).
///
/// ```
/// use akku::bytes::varint::decode_i64;
///
/// assert_eq!(decode_i64(&[0x01]), Ok((-1, 1)));
/// ```
pub fn decode_i64(buf: &[u8]) -> Result<(i64, usize), VarintError> {
    let (raw, len) = decode_u64(buf)?;
    Ok((zigzag_decode(raw), len))
}

/// Like [`decode_i64`] but rejects non-minimal encodings with
/// [`VarintError::NonCanonical`].
///
/// ```
/// use akku::bytes::varint::{decode_i64_strict, VarintError};
///
/// assert_eq!(decode_i64_strict(&[0x01]), Ok((-1, 1)));
/// assert_eq!(decode_i64_strict(&[0x81, 0x00]), Err(VarintError::NonCanonical));
/// ```
pub fn decode_i64_strict(buf: &[u8]) -> Result<(i64, usize), VarintError> {
    let (raw, len) = decode_u64_strict(buf)?;
    Ok((zigzag_decode(raw), len))
}
