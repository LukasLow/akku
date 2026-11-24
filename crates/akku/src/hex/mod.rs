//! Hexadecimal (Base16) encoding and decoding.
//!
//! Every input byte becomes two ASCII hex digits, high nibble first, so
//! `[0xde, 0xad]` encodes to `"dead"`. This is the Base16 alphabet of
//! RFC 4648 §8.
//!
//! # Overview
//!
//! | Task | Allocating | Into a caller buffer |
//! |---|---|---|
//! | Encode, lowercase | [`encode`] | [`encode_to_slice`] |
//! | Encode, uppercase | [`encode_upper`] | [`encode_upper_to_slice`] |
//! | Decode | [`decode`] | [`decode_to_slice`] |
//! | Size an encode buffer | [`encoded_len`] | |
//!
//! All functions take `impl AsRef<[u8]>`, so `&str`, `String`, `&[u8]`,
//! `Vec<u8>` and byte arrays all work.
//!
//! # Accepted input
//!
//! - Decoding accepts lowercase (`0-9a-f`), uppercase (`0-9A-F`) and mixed
//!   case digits.
//! - The input must be exactly an even number of hex digits. Decoding
//!   rejects whitespace, separators (`:`, `-`), a `0x` prefix, and non-ASCII
//!   bytes.
//!
//! # Limits
//!
//! There are no configurable limits. [`encode`] allocates exactly
//! `2 * input.len()` bytes and [`decode`] allocates `input.len() / 2` bytes,
//! so memory use never exceeds twice the input size. The `_to_slice` variants
//! do not allocate.
//!
//! # Errors
//!
//! Decoding returns a [`DecodeError`]. When the input has several problems,
//! the error reported follows a fixed order:
//!
//! 1. [`DecodeError::InvalidByte`]: the first non-hex byte, scanning left to
//!    right.
//! 2. [`DecodeError::OddLength`]: all bytes are hex digits, but there is an
//!    odd number of them.
//! 3. [`DecodeError::OutputTooSmall`]: only from [`decode_to_slice`], when the
//!    input is valid but the output buffer is too short.
//!
//! Encoding into a caller buffer returns [`BufferTooSmall`] when the buffer is
//! too short. The allocating encoders cannot fail.
//!
//! # Security
//!
//! Encoding and decoding run in time linear in the input, but they are **not
//! constant-time**. Their timing can depend on the data, so do not use them on
//! secret keys where an attacker can measure timing.
//!
//! # Examples
//!
//! ```
//! use akku::hex;
//!
//! let encoded = hex::encode(b"\xde\xad\xbe\xef");
//! assert_eq!(encoded, "deadbeef");
//! assert_eq!(hex::encode_upper(b"\xde\xad\xbe\xef"), "DEADBEEF");
//!
//! // Decoding is case-insensitive.
//! assert_eq!(hex::decode("DeadBEEF")?, [0xde, 0xad, 0xbe, 0xef]);
//!
//! // Malformed input is a typed error with the offending position.
//! assert_eq!(
//!     hex::decode("0x00"),
//!     Err(hex::DecodeError::InvalidByte { index: 1, byte: b'x' }),
//! );
//! # Ok::<(), hex::DecodeError>(())
//! ```
//!
//! Encoding and decoding without allocating:
//!
//! ```
//! use akku::hex;
//!
//! let mut text = [0u8; 4];
//! let n = hex::encode_to_slice([0xca, 0xfe], &mut text)?;
//! assert_eq!(&text[..n], b"cafe");
//!
//! let mut bytes = [0u8; 2];
//! let n = hex::decode_to_slice(&text[..n], &mut bytes)?;
//! assert_eq!(&bytes[..n], [0xca, 0xfe]);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

const LOWER_DIGITS: &[u8; 16] = b"0123456789abcdef";
const UPPER_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Error returned by [`encode_to_slice`] and [`encode_upper_to_slice`] when
/// the output slice is shorter than [`encoded_len`] of the input.
///
/// # Examples
///
/// ```
/// use akku::hex::{self, BufferTooSmall};
///
/// let mut out = [0u8; 3];
/// assert_eq!(hex::encode_to_slice(b"hi", &mut out), Err(BufferTooSmall { needed: 4 }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferTooSmall {
    /// Minimum buffer length in bytes required for the operation.
    pub needed: usize,
}

impl fmt::Display for BufferTooSmall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "output buffer too small: needed {} bytes", self.needed)
    }
}

impl std::error::Error for BufferTooSmall {}

/// Error returned by [`decode`] and [`decode_to_slice`].
///
/// When an input has several problems, `InvalidByte` is reported first, then
/// `OddLength`, then `OutputTooSmall` (see the [module docs](self#errors)).
///
/// # Examples
///
/// ```
/// use akku::hex::{self, DecodeError};
///
/// assert_eq!(hex::decode("abc"), Err(DecodeError::OddLength { len: 3 }));
/// // The invalid byte wins over the odd length.
/// assert_eq!(hex::decode("abz"), Err(DecodeError::InvalidByte { index: 2, byte: b'z' }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// Input contains a byte that is not an ASCII hex digit.
    InvalidByte {
        /// Zero-based byte offset of the first offending byte.
        index: usize,
        /// The offending byte value. For a non-ASCII character this is the
        /// first byte of its UTF-8 encoding.
        byte: u8,
    },
    /// Input consists solely of valid hex digits, but has an odd byte length.
    OddLength {
        /// The length of the input in bytes.
        len: usize,
    },
    /// The output slice passed to [`decode_to_slice`] is shorter than the
    /// decoded byte count.
    OutputTooSmall {
        /// Exact number of output bytes needed (input length / 2).
        needed: usize,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidByte { index, byte } => {
                write!(f, "invalid hex byte 0x{byte:02x} at index {index}")
            }
            Self::OddLength { len } => {
                write!(f, "odd hex input length {len}")
            }
            Self::OutputTooSmall { needed } => {
                write!(f, "output buffer too small: needed {needed} bytes")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Returns the encoded length in bytes of `input_len` input bytes, which is
/// `input_len * 2`.
///
/// Use it to size the output buffer for [`encode_to_slice`] and
/// [`encode_upper_to_slice`].
///
/// # Panics
///
/// Panics if `input_len * 2` overflows `usize`, that is if
/// `input_len > usize::MAX / 2`. This holds in both debug and release builds.
/// The length of any real slice is at most `isize::MAX`, so passing
/// `slice.len()` never panics.
///
/// # Examples
///
/// ```
/// assert_eq!(akku::hex::encoded_len(0), 0);
/// assert_eq!(akku::hex::encoded_len(3), 6);
/// ```
pub const fn encoded_len(input_len: usize) -> usize {
    match input_len.checked_mul(2) {
        Some(len) => len,
        None => panic!("hex::encoded_len: input length overflows usize"),
    }
}

/// Encodes `input` as a lowercase hexadecimal [`String`].
///
/// The result has [`encoded_len`]`(input.len())` bytes.
///
/// # Examples
///
/// ```
/// assert_eq!(akku::hex::encode([0x00, 0x7f, 0xff]), "007fff");
/// assert_eq!(akku::hex::encode("Hi"), "4869");
/// assert_eq!(akku::hex::encode(b""), "");
/// ```
pub fn encode(input: impl AsRef<[u8]>) -> String {
    encode_with_table(input.as_ref(), LOWER_DIGITS)
}

/// Encodes `input` as an uppercase hexadecimal [`String`].
///
/// # Examples
///
/// ```
/// assert_eq!(akku::hex::encode_upper([0xab, 0xcd, 0xef]), "ABCDEF");
/// ```
pub fn encode_upper(input: impl AsRef<[u8]>) -> String {
    encode_with_table(input.as_ref(), UPPER_DIGITS)
}

/// Encodes `input` as lowercase hexadecimal ASCII into the start of `output`
/// and returns the number of bytes written, which is
/// [`encoded_len`]`(input.len())`.
///
/// Bytes of `output` past the returned length are left untouched.
///
/// # Errors
///
/// Returns [`BufferTooSmall`] if `output` is shorter than
/// [`encoded_len`]`(input.len())`. `output` is not modified in that case.
///
/// # Examples
///
/// ```
/// use akku::hex;
///
/// let input = [0xbe, 0xef];
/// let mut out = [0u8; 8];
/// let n = hex::encode_to_slice(input, &mut out)?;
/// assert_eq!(&out[..n], b"beef");
/// # Ok::<(), hex::BufferTooSmall>(())
/// ```
pub fn encode_to_slice(
    input: impl AsRef<[u8]>,
    output: &mut [u8],
) -> Result<usize, BufferTooSmall> {
    encode_slice_with_table(input.as_ref(), output, LOWER_DIGITS)
}

/// Encodes `input` as uppercase hexadecimal ASCII into the start of `output`
/// and returns the number of bytes written.
///
/// Behaves like [`encode_to_slice`] except for the letter case.
///
/// # Errors
///
/// Returns [`BufferTooSmall`] if `output` is shorter than
/// [`encoded_len`]`(input.len())`. `output` is not modified in that case.
///
/// # Examples
///
/// ```
/// use akku::hex;
///
/// let mut out = [0u8; 4];
/// assert_eq!(hex::encode_upper_to_slice([0xbe, 0xef], &mut out), Ok(4));
/// assert_eq!(&out, b"BEEF");
/// ```
pub fn encode_upper_to_slice(
    input: impl AsRef<[u8]>,
    output: &mut [u8],
) -> Result<usize, BufferTooSmall> {
    encode_slice_with_table(input.as_ref(), output, UPPER_DIGITS)
}

/// Decodes hexadecimal `input` into a newly allocated `Vec<u8>`.
///
/// Accepts upper-, lower- and mixed-case digits. The result has
/// `input.len() / 2` bytes.
///
/// # Errors
///
/// - [`DecodeError::InvalidByte`] for the first byte that is not a hex digit.
/// - [`DecodeError::OddLength`] if every byte is a hex digit but the length
///   is odd.
///
/// # Examples
///
/// ```
/// use akku::hex::{self, DecodeError};
///
/// assert_eq!(hex::decode("48656c6C6F")?, b"Hello");
/// assert_eq!(hex::decode("4 8"), Err(DecodeError::InvalidByte { index: 1, byte: b' ' }));
/// assert_eq!(hex::decode("486"), Err(DecodeError::OddLength { len: 3 }));
/// # Ok::<(), DecodeError>(())
/// ```
pub fn decode(input: impl AsRef<[u8]>) -> Result<Vec<u8>, DecodeError> {
    let input = input.as_ref();
    let mut out = vec![0u8; input.len() / 2];
    let n = decode_to_slice(input, &mut out)?;
    debug_assert_eq!(n, out.len());
    Ok(out)
}

/// Decodes hexadecimal `input` into the start of `output` and returns the
/// number of bytes written, which is `input.len() / 2`.
///
/// `output` may be longer than needed; bytes past the returned length are
/// left untouched.
///
/// # Errors
///
/// Errors are reported in this order:
///
/// 1. [`DecodeError::InvalidByte`] for the first byte, left to right, that is
///    not a hex digit.
/// 2. [`DecodeError::OddLength`] if every byte is a hex digit but the length
///    is odd.
/// 3. [`DecodeError::OutputTooSmall`] if `output` is shorter than
///    `input.len() / 2`.
///
/// On `OddLength` and `OutputTooSmall`, `output` is not modified. On
/// `InvalidByte`, the bytes decoded before the error may already have been
/// written to `output`; treat its contents as unspecified.
///
/// # Examples
///
/// ```
/// use akku::hex::{self, DecodeError};
///
/// let mut out = [0u8; 4];
/// let n = hex::decode_to_slice("c0ffee", &mut out)?;
/// assert_eq!(&out[..n], [0xc0, 0xff, 0xee]);
///
/// let mut small = [0u8; 1];
/// assert_eq!(
///     hex::decode_to_slice("c0ffee", &mut small),
///     Err(DecodeError::OutputTooSmall { needed: 3 }),
/// );
/// # Ok::<(), DecodeError>(())
/// ```
pub fn decode_to_slice(input: impl AsRef<[u8]>, output: &mut [u8]) -> Result<usize, DecodeError> {
    let input = input.as_ref();
    let len = input.len();
    let needed = len / 2;

    if len % 2 != 0 || output.len() < needed {
        // Slow path: scan everything so an invalid byte wins over a length error.
        for (index, &byte) in input.iter().enumerate() {
            if decode_nibble(byte).is_none() {
                return Err(DecodeError::InvalidByte { index, byte });
            }
        }
        if len % 2 != 0 {
            return Err(DecodeError::OddLength { len });
        }
        return Err(DecodeError::OutputTooSmall { needed });
    }

    for (pair_idx, pair) in input.chunks_exact(2).enumerate() {
        let base = pair_idx * 2;
        let hi = decode_nibble(pair[0]).ok_or(DecodeError::InvalidByte {
            index: base,
            byte: pair[0],
        })?;
        let lo = decode_nibble(pair[1]).ok_or(DecodeError::InvalidByte {
            index: base + 1,
            byte: pair[1],
        })?;
        output[pair_idx] = (hi << 4) | lo;
    }

    Ok(needed)
}

#[inline]
const fn decode_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn encode_with_table(input: &[u8], table: &[u8; 16]) -> String {
    let needed = encoded_len(input.len());
    let mut buf = vec![0u8; needed];
    let written = encode_slice_with_table(input, &mut buf, table)
        .expect("allocated buffer has exact encoded_len");
    debug_assert_eq!(written, needed);
    String::from_utf8(buf).expect("hex output is valid ASCII")
}

fn encode_slice_with_table(
    input: &[u8],
    output: &mut [u8],
    table: &[u8; 16],
) -> Result<usize, BufferTooSmall> {
    let needed = encoded_len(input.len());
    if output.len() < needed {
        return Err(BufferTooSmall { needed });
    }

    for (i, &byte) in input.iter().enumerate() {
        output[i * 2] = table[(byte >> 4) as usize];
        output[i * 2 + 1] = table[(byte & 0x0f) as usize];
    }

    Ok(needed)
}
