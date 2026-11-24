//! RFC 4648 Base64 encoding and decoding.
//!
//! By default, decoding is strict: padding is required for padded engines,
//! forbidden for unpadded engines, and non-zero trailing bits are rejected.
//!
//! # Examples
//!
//! ```
//! use akku::base64;
//!
//! let encoded = base64::encode(b"hello world");
//! assert_eq!(encoded, "aGVsbG8gd29ybGQ=");
//! assert_eq!(base64::decode(&encoded).unwrap(), b"hello world");
//! ```

use std::fmt;

const INVALID_SYM: i8 = -1;

/// A 64-byte Base64 alphabet.
#[derive(Debug, Clone, Copy, Eq)]
pub struct Alphabet {
    encode_table: [u8; 64],
    decode_table: [i8; 256],
}

impl PartialEq for Alphabet {
    fn eq(&self, other: &Self) -> bool {
        self.encode_table == other.encode_table
    }
}

impl Alphabet {
    /// The standard RFC 4648 §4 alphabet (`A-Z`, `a-z`, `0-9`, `+`, `/`).
    pub const STANDARD: Alphabet =
        match Alphabet::new(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/") {
            Ok(a) => a,
            Err(_) => unreachable!(),
        };

    /// The URL- and filename-safe RFC 4648 §5 alphabet (`A-Z`, `a-z`, `0-9`, `-`, `_`).
    pub const URL_SAFE: Alphabet =
        match Alphabet::new(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_") {
            Ok(a) => a,
            Err(_) => unreachable!(),
        };

    /// Construct and validate a custom 64-byte Base64 alphabet.
    ///
    /// Rejects `'='` (`PaddingByte`), non-printable or whitespace ASCII /
    /// non-ASCII bytes (`InvalidByte`), and duplicate symbols (`Duplicate`).
    pub const fn new(symbols: &[u8; 64]) -> Result<Alphabet, AlphabetError> {
        let mut decode_table = [INVALID_SYM; 256];
        let mut i = 0;
        while i < 64 {
            let b = symbols[i];
            if b == b'=' {
                return Err(AlphabetError::PaddingByte);
            }
            if !b.is_ascii_graphic() {
                return Err(AlphabetError::InvalidByte(b));
            }
            if decode_table[b as usize] != INVALID_SYM {
                return Err(AlphabetError::Duplicate(b));
            }
            decode_table[b as usize] = i as i8;
            i += 1;
        }
        Ok(Alphabet {
            encode_table: *symbols,
            decode_table,
        })
    }

    /// Return the 64 encoding symbols in index order.
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.encode_table
    }
}

/// Errors returned when constructing a custom [`Alphabet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AlphabetError {
    /// A symbol is outside printable non-whitespace ASCII (`0x21..=0x7e`).
    InvalidByte(u8),
    /// A symbol appears more than once in the 64-byte alphabet.
    Duplicate(u8),
    /// The padding byte `'='` cannot appear inside the alphabet.
    PaddingByte,
}

impl fmt::Display for AlphabetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidByte(b) => write!(f, "invalid alphabet byte 0x{b:02x}"),
            Self::Duplicate(b) => write!(f, "duplicate alphabet byte 0x{b:02x}"),
            Self::PaddingByte => f.write_str("padding byte '=' cannot be part of an alphabet"),
        }
    }
}

impl std::error::Error for AlphabetError {}

/// How trailing `'='` padding is handled during decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecodePadding {
    /// Input length must be a multiple of 4, with canonical `'='` padding when needed.
    #[default]
    Required,
    /// `'='` padding bytes are forbidden; input must be unpadded.
    Forbidden,
    /// Either canonical `'='` padding or no padding is accepted.
    Optional,
}

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

/// Errors returned when decoding Base64 input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// Input contains a byte that is neither in the alphabet nor `'='`.
    InvalidByte {
        /// Zero-based byte offset of the invalid byte.
        index: usize,
        /// The offending byte value.
        byte: u8,
    },
    /// Input length is invalid for Base64 (e.g. `len % 4 == 1`, or unpadded when padding is required).
    InvalidLength {
        /// The length of the input in bytes.
        len: usize,
    },
    /// Unexpected, misplaced, or forbidden `'='` padding byte.
    InvalidPadding {
        /// Zero-based byte offset of the invalid padding.
        index: usize,
    },
    /// Unused trailing bits in the final quantum are non-zero.
    InvalidTrailingBits {
        /// Zero-based byte offset of the symbol carrying non-zero trailing bits.
        index: usize,
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
                write!(f, "invalid base64 byte 0x{byte:02x} at index {index}")
            }
            Self::InvalidLength { len } => {
                write!(f, "invalid base64 input length {len}")
            }
            Self::InvalidPadding { index } => {
                write!(f, "invalid base64 padding at index {index}")
            }
            Self::InvalidTrailingBits { index } => {
                write!(f, "non-zero trailing bits at index {index}")
            }
            Self::OutputTooSmall { needed } => {
                write!(f, "output buffer too small: needed {needed} bytes")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Configurable Base64 encoder and decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Engine {
    alphabet: Alphabet,
    pad: bool,
    decode_padding: DecodePadding,
    allow_trailing_bits: bool,
}

impl Engine {
    /// Standard RFC 4648 Base64 engine with `'='` padding.
    pub const STANDARD: Engine = Engine::new(Alphabet::STANDARD, true);

    /// Standard RFC 4648 Base64 engine without `'='` padding.
    pub const STANDARD_NO_PAD: Engine = Engine::new(Alphabet::STANDARD, false);

    /// URL-safe RFC 4648 §5 Base64 engine with `'='` padding.
    pub const URL_SAFE: Engine = Engine::new(Alphabet::URL_SAFE, true);

    /// URL-safe RFC 4648 §5 Base64 engine without `'='` padding.
    pub const URL_SAFE_NO_PAD: Engine = Engine::new(Alphabet::URL_SAFE, false);

    /// Create a new engine from an [`Alphabet`] and whether encoding emits `'='` padding.
    ///
    /// When `pad` is `true`, decoding defaults to [`DecodePadding::Required`].
    /// When `pad` is `false`, decoding defaults to [`DecodePadding::Forbidden`].
    pub const fn new(alphabet: Alphabet, pad: bool) -> Engine {
        Engine {
            alphabet,
            pad,
            decode_padding: if pad {
                DecodePadding::Required
            } else {
                DecodePadding::Forbidden
            },
            allow_trailing_bits: false,
        }
    }

    /// Override the padding policy used when decoding.
    pub const fn with_decode_padding(mut self, mode: DecodePadding) -> Engine {
        self.decode_padding = mode;
        self
    }

    /// Control whether non-zero unused trailing bits are accepted when decoding.
    pub const fn allow_trailing_bits(mut self, allow: bool) -> Engine {
        self.allow_trailing_bits = allow;
        self
    }

    /// Return the [`Alphabet`] used by this engine.
    pub const fn alphabet(&self) -> &Alphabet {
        &self.alphabet
    }

    /// Compute the exact encoded length in bytes for `input_len` input bytes.
    pub const fn encoded_len(&self, input_len: usize) -> usize {
        if self.pad {
            input_len.div_ceil(3) * 4
        } else {
            let full = (input_len / 3) * 4;
            match input_len % 3 {
                0 => full,
                1 => full + 2,
                _ => full + 3,
            }
        }
    }

    /// Encode `input` into a newly allocated [`String`].
    pub fn encode(&self, input: impl AsRef<[u8]>) -> String {
        let input = input.as_ref();
        let needed = self.encoded_len(input.len());
        let mut buf = vec![0u8; needed];
        let written = self
            .encode_slice(input, &mut buf)
            .expect("allocated buffer has exact encoded_len");
        debug_assert_eq!(written, needed);
        // Safety not needed: all bytes come from validated ASCII alphabet or b'='.
        String::from_utf8(buf).expect("base64 output is valid ASCII")
    }

    /// Append the Base64 encoding of `input` onto `out`, returning the number of bytes written.
    pub fn encode_into(&self, input: impl AsRef<[u8]>, out: &mut String) -> usize {
        let input = input.as_ref();
        let needed = self.encoded_len(input.len());
        let mut buf = vec![0u8; needed];
        let written = self
            .encode_slice(input, &mut buf)
            .expect("allocated buffer has exact encoded_len");
        let s = std::str::from_utf8(&buf[..written]).expect("base64 output is valid ASCII");
        out.push_str(s);
        written
    }

    /// Encode `input` into `output`, returning the number of bytes written.
    pub fn encode_slice(
        &self,
        input: impl AsRef<[u8]>,
        output: &mut [u8],
    ) -> Result<usize, BufferTooSmall> {
        let input = input.as_ref();
        let needed = self.encoded_len(input.len());
        if output.len() < needed {
            return Err(BufferTooSmall { needed });
        }

        let table = &self.alphabet.encode_table;
        let mut out_idx = 0;
        let mut chunks = input.chunks_exact(3);

        for chunk in &mut chunks {
            let b0 = chunk[0] as usize;
            let b1 = chunk[1] as usize;
            let b2 = chunk[2] as usize;

            output[out_idx] = table[b0 >> 2];
            output[out_idx + 1] = table[((b0 & 0x03) << 4) | (b1 >> 4)];
            output[out_idx + 2] = table[((b1 & 0x0f) << 2) | (b2 >> 6)];
            output[out_idx + 3] = table[b2 & 0x3f];
            out_idx += 4;
        }

        let rem = chunks.remainder();
        match rem.len() {
            1 => {
                let b0 = rem[0] as usize;
                output[out_idx] = table[b0 >> 2];
                output[out_idx + 1] = table[(b0 & 0x03) << 4];
                out_idx += 2;
                if self.pad {
                    output[out_idx] = b'=';
                    output[out_idx + 1] = b'=';
                    out_idx += 2;
                }
            }
            2 => {
                let b0 = rem[0] as usize;
                let b1 = rem[1] as usize;
                output[out_idx] = table[b0 >> 2];
                output[out_idx + 1] = table[((b0 & 0x03) << 4) | (b1 >> 4)];
                output[out_idx + 2] = table[(b1 & 0x0f) << 2];
                out_idx += 3;
                if self.pad {
                    output[out_idx] = b'=';
                    out_idx += 1;
                }
            }
            _ => {}
        }

        Ok(out_idx)
    }

    /// Decode `input` into a newly allocated `Vec<u8>`.
    pub fn decode(&self, input: impl AsRef<[u8]>) -> Result<Vec<u8>, DecodeError> {
        let input = input.as_ref();
        let mut out = Vec::with_capacity((input.len() / 4) * 3 + 3);
        self.decode_into(input, &mut out)?;
        Ok(out)
    }

    /// Decode `input` and append the resulting bytes to `out`.
    ///
    /// On error, `out` is rolled back to its original length so no partial bytes
    /// remain appended.
    pub fn decode_into(
        &self,
        input: impl AsRef<[u8]>,
        out: &mut Vec<u8>,
    ) -> Result<usize, DecodeError> {
        let input = input.as_ref();
        let start_len = out.len();
        let upper_bound = (input.len() / 4) * 3 + 3;
        out.reserve(upper_bound);
        let res = self.decode_with(input, |byte| out.push(byte));
        match res {
            Ok(written) => Ok(written),
            Err(err) => {
                out.truncate(start_len);
                Err(err)
            }
        }
    }

    /// Decode `input` into `output`, returning the number of bytes written.
    pub fn decode_slice(
        &self,
        input: impl AsRef<[u8]>,
        output: &mut [u8],
    ) -> Result<usize, DecodeError> {
        let input = input.as_ref();
        let len = input.len();
        if len == 0 {
            return Ok(0);
        }

        let estimated_needed = match len % 4 {
            0 => {
                if input[len - 2] == b'=' {
                    (len / 4) * 3 - 2
                } else if input[len - 1] == b'=' {
                    (len / 4) * 3 - 1
                } else {
                    (len / 4) * 3
                }
            }
            2 => (len / 4) * 3 + 1,
            3 => (len / 4) * 3 + 2,
            _ => return Err(DecodeError::InvalidLength { len }),
        };

        if output.len() < estimated_needed {
            let needed = self.decode_with(input, |_| {})?;
            return Err(DecodeError::OutputTooSmall { needed });
        }

        let mut idx = 0;
        let needed = self.decode_with(input, |byte| {
            output[idx] = byte;
            idx += 1;
        })?;
        Ok(needed)
    }

    fn decode_sym(&self, byte: u8, index: usize) -> Result<u8, DecodeError> {
        if byte == b'=' {
            return Err(DecodeError::InvalidPadding { index });
        }
        let val = self.alphabet.decode_table[byte as usize];
        if val == INVALID_SYM {
            return Err(DecodeError::InvalidByte { index, byte });
        }
        Ok(val as u8)
    }

    fn decode_with<F: FnMut(u8)>(&self, input: &[u8], mut emit: F) -> Result<usize, DecodeError> {
        let len = input.len();
        if len == 0 {
            return Ok(0);
        }

        if len % 4 == 1 {
            // Check whether there is an invalid byte or padding before reporting InvalidLength
            // only if needed? Wait: len % 4 == 1 is always invalid length in Base64,
            // except let's check if byte 0 itself is invalid or padding?
            // Checking bytes in full quads first handles e.g. "!!!!!" cleanly; for 1 byte,
            // check padding/byte or length. Wait: "A" -> InvalidLength { len: 1 }.
            // What about "=" -> InvalidPadding { index: 0 } or InvalidLength { len: 1 }?
            // Let's validate bytes/padding up to the last chunk, and check length!
            // Actually, if len % 4 == 1, returning InvalidLength { len } immediately
            // is standard (e.g. "a" -> InvalidLength { len: 1 }, "aaaaa" -> InvalidLength { len: 5 }).
            return Err(DecodeError::InvalidLength { len });
        }

        let rem_len = len % 4;
        if rem_len != 0 && self.decode_padding == DecodePadding::Required {
            return Err(DecodeError::InvalidLength { len });
        }

        let full_quad_count = len / 4;
        let has_tail_unpadded = rem_len != 0;
        let main_quads = if has_tail_unpadded {
            full_quad_count
        } else {
            // Last quad may contain '=' padding, so process full_quad_count - 1 strictly unpadded quads first
            full_quad_count - 1
        };

        let mut written = 0;
        for q in 0..main_quads {
            let base = q * 4;
            let v0 = self.decode_sym(input[base], base)?;
            let v1 = self.decode_sym(input[base + 1], base + 1)?;
            let v2 = self.decode_sym(input[base + 2], base + 2)?;
            let v3 = self.decode_sym(input[base + 3], base + 3)?;

            emit((v0 << 2) | (v1 >> 4));
            emit(((v1 & 0x0f) << 4) | (v2 >> 2));
            emit(((v2 & 0x03) << 6) | v3);
            written += 3;
        }

        if has_tail_unpadded {
            let base = main_quads * 4;
            let v0 = self.decode_sym(input[base], base)?;
            let v1 = self.decode_sym(input[base + 1], base + 1)?;

            if rem_len == 2 {
                if !self.allow_trailing_bits && (v1 & 0x0f) != 0 {
                    return Err(DecodeError::InvalidTrailingBits { index: base + 1 });
                }
                emit((v0 << 2) | (v1 >> 4));
                written += 1;
            } else {
                // rem_len == 3
                let v2 = self.decode_sym(input[base + 2], base + 2)?;
                if !self.allow_trailing_bits && (v2 & 0x03) != 0 {
                    return Err(DecodeError::InvalidTrailingBits { index: base + 2 });
                }
                emit((v0 << 2) | (v1 >> 4));
                emit(((v1 & 0x0f) << 4) | (v2 >> 2));
                written += 2;
            }
        } else {
            // Final 4-byte quad: may end with '=' or '=='
            let base = main_quads * 4;
            let b0 = input[base];
            let b1 = input[base + 1];
            let b2 = input[base + 2];
            let b3 = input[base + 3];

            let v0 = self.decode_sym(b0, base)?;
            let v1 = self.decode_sym(b1, base + 1)?;

            if b2 == b'=' {
                if self.decode_padding == DecodePadding::Forbidden {
                    return Err(DecodeError::InvalidPadding { index: base + 2 });
                }
                if b3 != b'=' {
                    // Validate b3 in case it's an invalid byte vs valid symbol after '='
                    if self.alphabet.decode_table[b3 as usize] == INVALID_SYM {
                        return Err(DecodeError::InvalidByte {
                            index: base + 3,
                            byte: b3,
                        });
                    }
                    return Err(DecodeError::InvalidPadding { index: base + 2 });
                }
                if !self.allow_trailing_bits && (v1 & 0x0f) != 0 {
                    return Err(DecodeError::InvalidTrailingBits { index: base + 1 });
                }
                emit((v0 << 2) | (v1 >> 4));
                written += 1;
            } else if b3 == b'=' {
                let v2 = self.decode_sym(b2, base + 2)?;
                if self.decode_padding == DecodePadding::Forbidden {
                    return Err(DecodeError::InvalidPadding { index: base + 3 });
                }
                if !self.allow_trailing_bits && (v2 & 0x03) != 0 {
                    return Err(DecodeError::InvalidTrailingBits { index: base + 2 });
                }
                emit((v0 << 2) | (v1 >> 4));
                emit(((v1 & 0x0f) << 4) | (v2 >> 2));
                written += 2;
            } else {
                let v2 = self.decode_sym(b2, base + 2)?;
                let v3 = self.decode_sym(b3, base + 3)?;
                emit((v0 << 2) | (v1 >> 4));
                emit(((v1 & 0x0f) << 4) | (v2 >> 2));
                emit(((v2 & 0x03) << 6) | v3);
                written += 3;
            }
        }

        Ok(written)
    }
}

/// Encode `input` using the standard padded Base64 engine.
pub fn encode(input: impl AsRef<[u8]>) -> String {
    Engine::STANDARD.encode(input)
}

/// Decode `input` using the standard padded Base64 engine.
pub fn decode(input: impl AsRef<[u8]>) -> Result<Vec<u8>, DecodeError> {
    Engine::STANDARD.decode(input)
}

/// Encode `input` using the URL-safe unpadded Base64 engine.
pub fn encode_url(input: impl AsRef<[u8]>) -> String {
    Engine::URL_SAFE_NO_PAD.encode(input)
}

/// Decode `input` using the URL-safe unpadded Base64 engine.
pub fn decode_url(input: impl AsRef<[u8]>) -> Result<Vec<u8>, DecodeError> {
    Engine::URL_SAFE_NO_PAD.decode(input)
}

/// Encode `input` into `out` using the standard padded Base64 engine.
pub fn encode_into(input: impl AsRef<[u8]>, out: &mut String) -> usize {
    Engine::STANDARD.encode_into(input, out)
}

/// Decode `input` into `out` using the standard padded Base64 engine, rolling
/// back `out` on error.
pub fn decode_into(input: impl AsRef<[u8]>, out: &mut Vec<u8>) -> Result<usize, DecodeError> {
    Engine::STANDARD.decode_into(input, out)
}
