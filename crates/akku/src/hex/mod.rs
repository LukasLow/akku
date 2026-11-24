//! Hexadecimal (Base16) encoding and decoding.
//!
//! Decoding accepts both lowercase (`0-9a-f`) and uppercase (`0-9A-F`) ASCII
//! digits. When decoding fails, the first invalid byte left-to-right takes
//! precedence over an odd input length.
//!
//! # Examples
//!
//! ```
//! use akku::hex;
//!
//! let encoded = hex::encode(b"\xde\xad\xbe\xef");
//! assert_eq!(encoded, "deadbeef");
//! assert_eq!(hex::decode(&encoded).unwrap(), [0xde, 0xad, 0xbe, 0xef]);
//! ```

use std::fmt;

const LOWER_DIGITS: &[u8; 16] = b"0123456789abcdef";
const UPPER_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Error returned when an encoding output slice is smaller than required.
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

/// Errors returned when decoding hexadecimal input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// Input contains a non-hexadecimal byte.
    InvalidByte {
        /// Zero-based byte offset of the first offending byte.
        index: usize,
        /// The offending byte value.
        byte: u8,
    },
    /// Input consists solely of valid hex digits, but has an odd byte length.
    OddLength {
        /// The length of the input in bytes.
        len: usize,
    },
    /// The caller-provided output slice is smaller than the decoded byte count.
    OutputTooSmall {
        /// Exact number of output bytes needed.
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

/// Compute the exact encoded length in bytes (`input_len * 2`).
pub const fn encoded_len(input_len: usize) -> usize {
    input_len * 2
}

/// Encode `input` as a lowercase hexadecimal [`String`].
pub fn encode(input: impl AsRef<[u8]>) -> String {
    encode_with_table(input.as_ref(), LOWER_DIGITS)
}

/// Encode `input` as an uppercase hexadecimal [`String`].
pub fn encode_upper(input: impl AsRef<[u8]>) -> String {
    encode_with_table(input.as_ref(), UPPER_DIGITS)
}

/// Encode `input` as lowercase hexadecimal bytes into `output`, returning the
/// number of bytes written.
pub fn encode_to_slice(
    input: impl AsRef<[u8]>,
    output: &mut [u8],
) -> Result<usize, BufferTooSmall> {
    encode_slice_with_table(input.as_ref(), output, LOWER_DIGITS)
}

/// Encode `input` as uppercase hexadecimal bytes into `output`, returning the
/// number of bytes written.
pub fn encode_upper_to_slice(
    input: impl AsRef<[u8]>,
    output: &mut [u8],
) -> Result<usize, BufferTooSmall> {
    encode_slice_with_table(input.as_ref(), output, UPPER_DIGITS)
}

/// Decode hexadecimal `input` into a newly allocated `Vec<u8>`.
pub fn decode(input: impl AsRef<[u8]>) -> Result<Vec<u8>, DecodeError> {
    let input = input.as_ref();
    let mut out = vec![0u8; input.len() / 2];
    let n = decode_to_slice(input, &mut out)?;
    debug_assert_eq!(n, out.len());
    Ok(out)
}

/// Decode hexadecimal `input` into `output`, returning the number of bytes written.
///
/// The first invalid byte left-to-right is reported before [`DecodeError::OddLength`]
/// or [`DecodeError::OutputTooSmall`].
pub fn decode_to_slice(input: impl AsRef<[u8]>, output: &mut [u8]) -> Result<usize, DecodeError> {
    let input = input.as_ref();
    let len = input.len();
    let needed = len / 2;

    if len % 2 != 0 || output.len() < needed {
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
