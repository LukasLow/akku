//! RFC 6455 wire framing, opcodes, close codes, and fast XOR masking.

use std::fmt;
use std::io::{Read, Write};

use crate::ws::error::{Error, Result};

/// Maximum payload length for RFC 6455 control frames (`Close`, `Ping`, `Pong`).
pub const MAX_CONTROL_PAYLOAD_LEN: usize = 125;

/// Size of the coalesced header + payload write buffer used by [`Frame::write_to`].
const WRITE_CHUNK_SIZE: usize = 64 * 1024;

/// A 4-bit RFC 6455 WebSocket frame opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OpCode {
    /// Continuation frame (`0x0`).
    Continuation = 0x0,
    /// UTF-8 text data frame (`0x1`).
    Text = 0x1,
    /// Binary data frame (`0x2`).
    Binary = 0x2,
    /// Connection close control frame (`0x8`).
    Close = 0x8,
    /// Ping control frame (`0x9`).
    Ping = 0x9,
    /// Pong control frame (`0xA`).
    Pong = 0xA,
}

impl OpCode {
    /// Parses a 4-bit wire opcode value into an [`OpCode`].
    pub fn from_u8(byte: u8) -> Result<Self> {
        match byte {
            0x0 => Ok(Self::Continuation),
            0x1 => Ok(Self::Text),
            0x2 => Ok(Self::Binary),
            0x8 => Ok(Self::Close),
            0x9 => Ok(Self::Ping),
            0xA => Ok(Self::Pong),
            other => Err(Error::InvalidOpcode(other)),
        }
    }

    /// Returns the raw 4-bit wire opcode value.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Returns `true` if this opcode represents a control frame (`Close`, `Ping`, `Pong`).
    pub const fn is_control(self) -> bool {
        (self as u8) >= 0x8
    }

    /// Returns `true` if this opcode represents a data frame (`Continuation`, `Text`, `Binary`).
    pub const fn is_data(self) -> bool {
        !self.is_control()
    }
}

/// An RFC 6455 §7.4 WebSocket close status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CloseCode(u16);

impl CloseCode {
    /// Normal closure (`1000`).
    pub const NORMAL: Self = Self(1000);
    /// Endpoint is going away (`1001`).
    pub const GOING_AWAY: Self = Self(1001);
    /// Protocol error (`1002`).
    pub const PROTOCOL_ERROR: Self = Self(1002);
    /// Unsupported data type (`1003`).
    pub const UNSUPPORTED_DATA: Self = Self(1003);
    /// No status code was present (`1005`, local-only — forbidden on the wire).
    pub const NO_STATUS: Self = Self(1005);
    /// Abnormal closure (`1006`, local-only — forbidden on the wire).
    pub const ABNORMAL: Self = Self(1006);
    /// Invalid frame payload data such as non-UTF-8 text (`1007`).
    pub const INVALID_PAYLOAD: Self = Self(1007);
    /// Policy violation (`1008`).
    pub const POLICY_VIOLATION: Self = Self(1008);
    /// Message too big to process (`1009`).
    pub const MESSAGE_TOO_BIG: Self = Self(1009);
    /// Client expected a mandatory extension (`1010`).
    pub const MANDATORY_EXT: Self = Self(1010);
    /// Unexpected server condition (`1011`).
    pub const INTERNAL_ERROR: Self = Self(1011);
    /// Service restart (`1012`).
    pub const SERVICE_RESTART: Self = Self(1012);
    /// Try again later (`1013`).
    pub const TRY_AGAIN_LATER: Self = Self(1013);
    /// Bad gateway (`1014`).
    pub const BAD_GATEWAY: Self = Self(1014);
    /// TLS handshake failure (`1015`, local-only — forbidden on the wire).
    pub const TLS_HANDSHAKE: Self = Self(1015);

    /// Wraps a raw 16-bit close status code.
    pub const fn new(code: u16) -> Self {
        Self(code)
    }

    /// Returns the numeric status code.
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// Returns `true` if RFC 6455 §7.4 permits sending or receiving this code on the wire.
    ///
    /// Valid wire codes are `1000..=1003`, `1007..=1014`, and `3000..=4999`.
    /// Codes `1004`, `1005`, `1006`, `1015`, `< 1000`, `1016..=2999`, and `>= 5000`
    /// are rejected.
    pub const fn is_allowed_on_wire(self) -> bool {
        matches!(self.0, 1000..=1003 | 1007..=1014 | 3000..=4999)
    }
}

impl fmt::Display for CloseCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u16> for CloseCode {
    fn from(code: u16) -> Self {
        Self(code)
    }
}

impl From<CloseCode> for u16 {
    fn from(code: CloseCode) -> Self {
        code.0
    }
}

/// Decoded payload of a WebSocket `Close` control frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFrame {
    /// The 16-bit close status code.
    pub code: CloseCode,
    /// Optional human-readable UTF-8 close reason (at most 123 bytes).
    pub reason: String,
}

impl CloseFrame {
    /// Creates a new [`CloseFrame`] with the given status `code` and `reason`.
    pub fn new(code: CloseCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }

    /// Encodes the close code and UTF-8 reason into a wire payload.
    ///
    /// Rejects status codes forbidden on the wire and reasons exceeding 123 bytes.
    pub fn encode_payload(&self) -> Result<Vec<u8>> {
        if !self.code.is_allowed_on_wire() {
            return Err(Error::InvalidCloseCode(self.code.as_u16()));
        }
        let total = 2 + self.reason.len();
        if total > MAX_CONTROL_PAYLOAD_LEN {
            return Err(Error::Protocol(format!(
                "Close frame payload length {total} exceeds 125-byte control frame limit"
            )));
        }
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&self.code.as_u16().to_be_bytes());
        out.extend_from_slice(self.reason.as_bytes());
        Ok(out)
    }

    /// Decodes a Close frame payload per RFC 6455 §5.5.1 and §7.4.
    ///
    /// - Empty payload returns `Ok(None)`.
    /// - 1-byte payload returns `Err(Error::Protocol)`.
    /// - `>= 2` bytes validates [`CloseCode::is_allowed_on_wire`] and UTF-8 reason.
    pub fn decode_payload(payload: &[u8]) -> Result<Option<Self>> {
        if payload.is_empty() {
            return Ok(None);
        }
        if payload.len() == 1 {
            return Err(Error::Protocol(
                "Close frame payload must be empty or at least 2 bytes".to_string(),
            ));
        }
        if payload.len() > MAX_CONTROL_PAYLOAD_LEN {
            return Err(Error::Protocol(format!(
                "Close frame payload length {} exceeds 125 bytes",
                payload.len()
            )));
        }
        let code = CloseCode::new(u16::from_be_bytes([payload[0], payload[1]]));
        if !code.is_allowed_on_wire() {
            return Err(Error::InvalidCloseCode(code.as_u16()));
        }
        let reason = std::str::from_utf8(&payload[2..])?.to_string();
        Ok(Some(Self { code, reason }))
    }
}

/// Applies RFC 6455 §5.3 4-byte XOR masking in-place.
///
/// Uses a pure-safe word-at-a-time (`u64`) loop with a scalar tail for speed.
pub fn apply_mask(buf: &mut [u8], mask: [u8; 4]) {
    let mask64 = u64::from_ne_bytes([
        mask[0], mask[1], mask[2], mask[3], mask[0], mask[1], mask[2], mask[3],
    ]);
    let mut chunks = buf.chunks_exact_mut(8);
    for chunk in chunks.by_ref() {
        let mut word_bytes = [0u8; 8];
        word_bytes.copy_from_slice(chunk);
        let masked = u64::from_ne_bytes(word_bytes) ^ mask64;
        chunk.copy_from_slice(&masked.to_ne_bytes());
    }
    for (i, byte) in chunks.into_remainder().iter_mut().enumerate() {
        *byte ^= mask[i & 3];
    }
}

/// A single RFC 6455 WebSocket wire frame.
///
/// `payload` is always stored **unmasked** in memory; [`Frame::encode`] and
/// [`Frame::write_to`] apply `mask` on the wire when `mask` is `Some`, and
/// [`Frame::read_from`] unmasks incoming payloads in-place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Final fragment flag (`FIN`).
    pub fin: bool,
    /// Frame [`OpCode`].
    pub opcode: OpCode,
    /// Optional 4-byte masking key (required for client-to-server frames, forbidden for server-to-client).
    pub mask: Option<[u8; 4]>,
    /// Unmasked frame payload bytes.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Creates an unmasked frame with the specified `fin`, `opcode`, and `payload`.
    pub fn new(fin: bool, opcode: OpCode, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            fin,
            opcode,
            mask: None,
            payload: payload.into(),
        }
    }

    /// Attaches a 4-byte masking key to this frame.
    pub fn with_mask(mut self, mask: [u8; 4]) -> Self {
        self.mask = Some(mask);
        self
    }

    /// Creates a final (`FIN=1`) UTF-8 text frame.
    pub fn text(data: impl Into<String>) -> Self {
        Self::new(true, OpCode::Text, data.into().into_bytes())
    }

    /// Creates a final (`FIN=1`) binary frame.
    pub fn binary(data: impl Into<Vec<u8>>) -> Self {
        Self::new(true, OpCode::Binary, data)
    }

    /// Creates a `Ping` control frame.
    pub fn ping(data: impl Into<Vec<u8>>) -> Self {
        Self::new(true, OpCode::Ping, data)
    }

    /// Creates a `Pong` control frame.
    pub fn pong(data: impl Into<Vec<u8>>) -> Self {
        Self::new(true, OpCode::Pong, data)
    }

    /// Creates a `Close` control frame from an optional [`CloseFrame`].
    pub fn close(frame: Option<&CloseFrame>) -> Result<Self> {
        let payload = match frame {
            Some(cf) => cf.encode_payload()?,
            None => Vec::new(),
        };
        Ok(Self::new(true, OpCode::Close, payload))
    }

    /// Validates frame invariants and encodes this frame to RFC 6455 wire bytes.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let (header, header_len) = self.encode_header()?;
        let mut out = Vec::with_capacity(header_len + self.payload.len());
        out.extend_from_slice(&header[..header_len]);
        out.extend_from_slice(&self.payload);
        if let Some(mask) = self.mask {
            apply_mask(&mut out[header_len..], mask);
        }
        Ok(out)
    }

    /// Validates frame invariants and writes this frame in RFC 6455 wire format to `writer`.
    ///
    /// The header is coalesced with the (masked) payload into a single buffer so a
    /// frame of up to 64 KiB total (header + payload) is emitted with exactly one
    /// `write_all` call (avoiding Nagle / delayed-ACK stalls on TCP). Larger frames
    /// are written in 64 KiB pieces from one reusable buffer, with the
    /// header coalesced into the first piece.
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        let (header, header_len) = self.encode_header()?;
        let total = header_len.saturating_add(self.payload.len());
        let mut buf = Vec::with_capacity(total.min(WRITE_CHUNK_SIZE));
        buf.extend_from_slice(&header[..header_len]);

        let mut offset = 0usize;
        loop {
            let room = WRITE_CHUNK_SIZE - buf.len();
            let take = (self.payload.len() - offset).min(room);
            let start = buf.len();
            buf.extend_from_slice(&self.payload[offset..offset + take]);
            if let Some(mask) = self.mask {
                let rotated = [
                    mask[offset & 3],
                    mask[(offset + 1) & 3],
                    mask[(offset + 2) & 3],
                    mask[(offset + 3) & 3],
                ];
                apply_mask(&mut buf[start..], rotated);
            }
            offset += take;
            writer.write_all(&buf)?;
            if offset >= self.payload.len() {
                return Ok(());
            }
            buf.clear();
        }
    }

    /// Validates frame invariants and returns the encoded header and its length.
    fn encode_header(&self) -> Result<([u8; 14], usize)> {
        if self.opcode.is_control() {
            if !self.fin {
                return Err(Error::Protocol(
                    "control frames must not be fragmented (FIN must be 1)".to_string(),
                ));
            }
            if self.payload.len() > MAX_CONTROL_PAYLOAD_LEN {
                return Err(Error::Protocol(format!(
                    "control frame payload length {} exceeds 125 bytes",
                    self.payload.len()
                )));
            }
        }

        let b0 = (if self.fin { 0x80 } else { 0x00 }) | (self.opcode.as_u8() & 0x0F);
        let mask_bit = if self.mask.is_some() { 0x80 } else { 0x00 };

        let mut header = [0u8; 14];
        header[0] = b0;
        let len = self.payload.len();
        let mut header_len = if len <= 125 {
            header[1] = mask_bit | (len as u8);
            2
        } else if len <= u16::MAX as usize {
            header[1] = mask_bit | 126;
            header[2..4].copy_from_slice(&(len as u16).to_be_bytes());
            4
        } else {
            let len64 = len as u64;
            if (len64 & (1 << 63)) != 0 {
                return Err(Error::Protocol(
                    "64-bit payload length MSB must be 0".to_string(),
                ));
            }
            header[1] = mask_bit | 127;
            header[2..10].copy_from_slice(&len64.to_be_bytes());
            10
        };

        if let Some(mask) = self.mask {
            header[header_len..header_len + 4].copy_from_slice(&mask);
            header_len += 4;
        }
        Ok((header, header_len))
    }

    /// Reads and validates a single RFC 6455 frame from `reader`.
    ///
    /// Enforces `max_frame_size` before allocating the payload buffer and unmasks
    /// masked payloads in-place.
    pub fn read_from<R: Read>(reader: &mut R, max_frame_size: usize) -> Result<Self> {
        let mut head = [0u8; 2];
        reader.read_exact(&mut head)?;

        let b0 = head[0];
        let b1 = head[1];

        if (b0 & 0x70) != 0 {
            return Err(Error::Protocol(format!(
                "non-zero reserved bits (RSV = 0x{:x}) without negotiated extension",
                (b0 & 0x70) >> 4
            )));
        }

        let fin = (b0 & 0x80) != 0;
        let opcode = OpCode::from_u8(b0 & 0x0F)?;
        let masked = (b1 & 0x80) != 0;
        let len7 = b1 & 0x7F;

        let payload_len: u64 = match len7 {
            0..=125 => u64::from(len7),
            126 => {
                let mut ext = [0u8; 2];
                reader.read_exact(&mut ext)?;
                let len = u16::from_be_bytes(ext);
                if len < 126 {
                    return Err(Error::Protocol(format!(
                        "non-minimal 16-bit payload length encoding ({len})"
                    )));
                }
                u64::from(len)
            }
            127 => {
                let mut ext = [0u8; 8];
                reader.read_exact(&mut ext)?;
                let len = u64::from_be_bytes(ext);
                if (len & (1 << 63)) != 0 {
                    return Err(Error::Protocol(
                        "64-bit payload length has most significant bit set".to_string(),
                    ));
                }
                if len <= u64::from(u16::MAX) {
                    return Err(Error::Protocol(format!(
                        "non-minimal 64-bit payload length encoding ({len})"
                    )));
                }
                len
            }
            _ => unreachable!(),
        };

        if opcode.is_control() {
            if !fin {
                return Err(Error::Protocol(
                    "fragmented control frame (FIN=0)".to_string(),
                ));
            }
            if payload_len > MAX_CONTROL_PAYLOAD_LEN as u64 {
                return Err(Error::Protocol(format!(
                    "control frame payload length {payload_len} exceeds 125 bytes"
                )));
            }
        }

        if payload_len > max_frame_size as u64 {
            return Err(Error::FrameTooLarge {
                len: payload_len,
                max: max_frame_size,
            });
        }

        let mask = if masked {
            let mut key = [0u8; 4];
            reader.read_exact(&mut key)?;
            Some(key)
        } else {
            None
        };

        let len_usize = usize::try_from(payload_len).map_err(|_| Error::FrameTooLarge {
            len: payload_len,
            max: max_frame_size,
        })?;
        let mut payload = vec![0u8; len_usize];
        if len_usize > 0 {
            reader.read_exact(&mut payload)?;
            if let Some(key) = mask {
                apply_mask(&mut payload, key);
            }
        }

        Ok(Self {
            fin,
            opcode,
            mask,
            payload,
        })
    }
}
