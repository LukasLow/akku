//! RFC 6455 WebSocket connection state machine with fragmentation reassembly,
//! streaming UTF-8 validation, control-frame interleaving, and DoS caps.

use std::io::{self, BufReader, Read, Write};

use crate::ws::error::{Error, Result};
use crate::ws::frame::{CloseCode, CloseFrame, Frame, OpCode};
use crate::ws::message::Message;
use crate::ws::sha1;

/// Default maximum payload length for a single WebSocket frame (16 MiB).
pub const DEFAULT_MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Default maximum payload length for a reassembled WebSocket message (64 MiB).
pub const DEFAULT_MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Endpoint role determining RFC 6455 masking direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Client endpoint: sends masked frames, requires unmasked incoming frames.
    Client,
    /// Server endpoint: sends unmasked frames, requires masked incoming frames.
    Server,
}

/// Configuration knobs for [`WebSocket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Maximum allowed payload size in bytes for a single wire frame (default: 16 MiB).
    pub max_frame_size: usize,
    /// Maximum allowed payload size in bytes for a reassembled message (default: 64 MiB).
    pub max_message_size: usize,
    /// When `true` (default), automatically sends a `Pong` whenever a `Ping` is received.
    pub auto_pong: bool,
    /// When `true` (default), automatically echoes a `Close` frame when the peer initiates close.
    pub auto_close_reply: bool,
}

impl Config {
    /// Creates a [`Config`] with default limits and automatic Pong/Close replies enabled.
    pub const fn new() -> Self {
        Self {
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            max_message_size: DEFAULT_MAX_MESSAGE_SIZE,
            auto_pong: true,
            auto_close_reply: true,
        }
    }

    /// Sets [`Config::max_frame_size`].
    pub const fn with_max_frame_size(mut self, max_frame_size: usize) -> Self {
        self.max_frame_size = max_frame_size;
        self
    }

    /// Sets [`Config::max_message_size`].
    pub const fn with_max_message_size(mut self, max_message_size: usize) -> Self {
        self.max_message_size = max_message_size;
        self
    }

    /// Sets [`Config::auto_pong`].
    pub const fn with_auto_pong(mut self, enabled: bool) -> Self {
        self.auto_pong = enabled;
        self
    }

    /// Sets [`Config::auto_close_reply`].
    pub const fn with_auto_close_reply(mut self, enabled: bool) -> Self {
        self.auto_close_reply = enabled;
        self
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

/// A synchronous RFC 6455 WebSocket connection over any `S: Read + Write`.
#[derive(Debug)]
pub struct WebSocket<S> {
    reader: BufReader<S>,
    role: Role,
    config: Config,
    protocol: Option<String>,
    close_sent: bool,
    close_received: bool,
    frag_opcode: Option<OpCode>,
    frag_buf: Vec<u8>,
    frag_utf8_validated: usize,
}

impl<S: Read + Write> WebSocket<S> {
    /// Wraps an already-upgraded stream as a [`WebSocket`].
    pub fn from_raw(stream: S, role: Role, config: Config) -> Self {
        Self::from_buffered(BufReader::new(stream), role, config, None)
    }

    /// Wraps an already-buffered stream from the opening handshake so any pipelined
    /// bytes already buffered after `\r\n\r\n` are preserved.
    pub(crate) fn from_buffered(
        reader: BufReader<S>,
        role: Role,
        config: Config,
        protocol: Option<String>,
    ) -> Self {
        Self {
            reader,
            role,
            config,
            protocol,
            close_sent: false,
            close_received: false,
            frag_opcode: None,
            frag_buf: Vec::new(),
            frag_utf8_validated: 0,
        }
    }

    /// Returns this endpoint's [`Role`].
    pub const fn role(&self) -> Role {
        self.role
    }

    /// Returns the active [`Config`].
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the negotiated `Sec-WebSocket-Protocol`, if any.
    pub fn protocol(&self) -> Option<&str> {
        self.protocol.as_deref()
    }

    /// Borrows the underlying transport stream.
    pub fn get_ref(&self) -> &S {
        self.reader.get_ref()
    }

    /// Mutably borrows the underlying transport stream.
    pub fn get_mut(&mut self) -> &mut S {
        self.reader.get_mut()
    }

    /// Consumes the [`WebSocket`] and returns the underlying transport stream.
    pub fn into_inner(self) -> S {
        self.reader.into_inner()
    }

    /// Sends a single low-level [`Frame`] and flushes the stream.
    pub fn write_frame(&mut self, frame: &Frame) -> Result<()> {
        let writer = self.reader.get_mut();
        frame.write_to(writer)?;
        writer.flush()?;
        Ok(())
    }

    /// Reads a single low-level [`Frame`], enforcing `max_frame_size` and RFC 6455
    /// role-based masking rules.
    pub fn read_frame(&mut self) -> Result<Frame> {
        let frame = match Frame::read_from(&mut self.reader, self.config.max_frame_size) {
            Ok(f) => f,
            Err(Error::Io(e))
                if e.kind() == io::ErrorKind::UnexpectedEof && self.close_received =>
            {
                return Err(Error::ConnectionClosed);
            }
            Err(Error::FrameTooLarge { len, max }) => {
                self.try_send_error_close(CloseCode::MESSAGE_TOO_BIG);
                return Err(Error::FrameTooLarge { len, max });
            }
            Err(Error::Protocol(msg)) => {
                self.try_send_error_close(CloseCode::PROTOCOL_ERROR);
                return Err(Error::Protocol(msg));
            }
            Err(Error::InvalidOpcode(op)) => {
                self.try_send_error_close(CloseCode::PROTOCOL_ERROR);
                return Err(Error::InvalidOpcode(op));
            }
            Err(err) => return Err(err),
        };

        match (self.role, frame.mask.is_some()) {
            (Role::Server, false) => {
                self.try_send_error_close(CloseCode::PROTOCOL_ERROR);
                return Err(Error::Protocol(
                    "client-to-server frame must be masked (RFC 6455 §5.1)".to_string(),
                ));
            }
            (Role::Client, true) => {
                self.try_send_error_close(CloseCode::PROTOCOL_ERROR);
                return Err(Error::Protocol(
                    "server-to-client frame must not be masked (RFC 6455 §5.1)".to_string(),
                ));
            }
            _ => {}
        }

        Ok(frame)
    }

    /// Sends a high-level [`Message`], applying client masking automatically when [`Role::Client`].
    pub fn send(&mut self, message: Message) -> Result<()> {
        if self.close_sent {
            return Err(Error::ConnectionClosed);
        }

        let is_close = message.is_close();
        let mut frame = match message {
            Message::Text(text) => Frame::text(text),
            Message::Binary(data) => Frame::binary(data),
            Message::Ping(data) => Frame::ping(data),
            Message::Pong(data) => Frame::pong(data),
            Message::Close(cf) => Frame::close(cf.as_ref())?,
        };

        if self.role == Role::Client {
            let mut key = [0u8; 4];
            sha1::fill_random(&mut key);
            frame.mask = Some(key);
        }

        self.write_frame(&frame)?;
        if is_close {
            self.close_sent = true;
        }
        Ok(())
    }

    /// Sends a UTF-8 [`Message::Text`].
    pub fn send_text(&mut self, text: impl Into<String>) -> Result<()> {
        self.send(Message::Text(text.into()))
    }

    /// Sends a [`Message::Binary`].
    pub fn send_binary(&mut self, data: impl Into<Vec<u8>>) -> Result<()> {
        self.send(Message::Binary(data.into()))
    }

    /// Sends a `Ping` control frame (payload must be at most 125 bytes).
    pub fn ping(&mut self, payload: impl Into<Vec<u8>>) -> Result<()> {
        self.send(Message::Ping(payload.into()))
    }

    /// Sends a `Pong` control frame (payload must be at most 125 bytes).
    pub fn pong(&mut self, payload: impl Into<Vec<u8>>) -> Result<()> {
        self.send(Message::Pong(payload.into()))
    }

    /// Initiates or completes a clean WebSocket close handshake.
    ///
    /// Calling `close` after a Close frame has already been sent is a no-op returning `Ok(())`.
    pub fn close(&mut self, frame: Option<CloseFrame>) -> Result<()> {
        if self.close_sent {
            return Ok(());
        }
        self.send(Message::Close(frame))
    }

    /// Receives the next complete [`Message`], handling fragmentation reassembly,
    /// streaming UTF-8 validation, interleaved control frames, and optional auto-Pong / auto-Close.
    pub fn recv(&mut self) -> Result<Message> {
        loop {
            let frame = self.read_frame()?;
            match frame.opcode {
                OpCode::Ping => {
                    if self.config.auto_pong && !self.close_sent {
                        let _ = self.pong(frame.payload.clone());
                    }
                    return Ok(Message::Ping(frame.payload));
                }
                OpCode::Pong => {
                    return Ok(Message::Pong(frame.payload));
                }
                OpCode::Close => {
                    let close_frame = match CloseFrame::decode_payload(&frame.payload) {
                        Ok(cf) => cf,
                        Err(Error::InvalidUtf8(e)) => {
                            self.try_send_error_close(CloseCode::INVALID_PAYLOAD);
                            return Err(Error::InvalidUtf8(e));
                        }
                        Err(err) => {
                            self.try_send_error_close(CloseCode::PROTOCOL_ERROR);
                            return Err(err);
                        }
                    };
                    self.close_received = true;
                    if self.config.auto_close_reply && !self.close_sent {
                        let reply = close_frame
                            .as_ref()
                            .map(|cf| CloseFrame::new(cf.code, String::new()));
                        let _ = self.close(reply);
                    }
                    return Ok(Message::Close(close_frame));
                }
                OpCode::Text | OpCode::Binary => {
                    if self.frag_opcode.is_some() {
                        self.try_send_error_close(CloseCode::PROTOCOL_ERROR);
                        return Err(Error::Protocol(
                            "received new data frame while fragmented message is in progress"
                                .to_string(),
                        ));
                    }

                    if frame.payload.len() > self.config.max_message_size {
                        self.try_send_error_close(CloseCode::MESSAGE_TOO_BIG);
                        return Err(Error::MessageTooLarge {
                            len: frame.payload.len(),
                            max: self.config.max_message_size,
                        });
                    }

                    if frame.fin {
                        return match frame.opcode {
                            OpCode::Text => match String::from_utf8(frame.payload) {
                                Ok(s) => Ok(Message::Text(s)),
                                Err(e) => {
                                    self.try_send_error_close(CloseCode::INVALID_PAYLOAD);
                                    Err(Error::InvalidUtf8(e.utf8_error()))
                                }
                            },
                            OpCode::Binary => Ok(Message::Binary(frame.payload)),
                            _ => unreachable!(),
                        };
                    }

                    self.frag_opcode = Some(frame.opcode);
                    self.frag_buf = frame.payload;
                    self.frag_utf8_validated = 0;

                    if frame.opcode == OpCode::Text {
                        self.validate_streaming_utf8(false)?;
                    }
                }
                OpCode::Continuation => {
                    let Some(active_op) = self.frag_opcode else {
                        self.try_send_error_close(CloseCode::PROTOCOL_ERROR);
                        return Err(Error::Protocol(
                            "unexpected Continuation frame without active fragmented message"
                                .to_string(),
                        ));
                    };

                    let new_len = self.frag_buf.len().saturating_add(frame.payload.len());
                    if new_len > self.config.max_message_size {
                        self.reset_fragment_state();
                        self.try_send_error_close(CloseCode::MESSAGE_TOO_BIG);
                        return Err(Error::MessageTooLarge {
                            len: new_len,
                            max: self.config.max_message_size,
                        });
                    }

                    self.frag_buf.extend_from_slice(&frame.payload);

                    if active_op == OpCode::Text {
                        self.validate_streaming_utf8(frame.fin)?;
                    }

                    if frame.fin {
                        let payload = std::mem::take(&mut self.frag_buf);
                        self.frag_opcode = None;
                        self.frag_utf8_validated = 0;
                        return match active_op {
                            OpCode::Text => match String::from_utf8(payload) {
                                Ok(s) => Ok(Message::Text(s)),
                                Err(e) => {
                                    self.try_send_error_close(CloseCode::INVALID_PAYLOAD);
                                    Err(Error::InvalidUtf8(e.utf8_error()))
                                }
                            },
                            OpCode::Binary => Ok(Message::Binary(payload)),
                            _ => unreachable!(),
                        };
                    }
                }
            }
        }
    }

    /// Incrementally validates UTF-8 across fragments (RFC 6455 §8.1 fail-fast).
    ///
    /// - Rejects definitively invalid UTF-8 bytes immediately on the fragment where they arrive.
    /// - Allows an incomplete multi-byte UTF-8 sequence (1..=3 bytes) at the end of a non-final
    ///   fragment to complete in the next `Continuation` fragment.
    /// - Rejects any incomplete sequence when `is_final == true`.
    fn validate_streaming_utf8(&mut self, is_final: bool) -> Result<()> {
        let tail = &self.frag_buf[self.frag_utf8_validated..];
        match std::str::from_utf8(tail) {
            Ok(_) => {
                self.frag_utf8_validated = self.frag_buf.len();
                Ok(())
            }
            Err(err) => {
                if err.error_len().is_some() || is_final {
                    self.reset_fragment_state();
                    self.try_send_error_close(CloseCode::INVALID_PAYLOAD);
                    Err(Error::InvalidUtf8(err))
                } else {
                    // Valid prefix followed by an incomplete trailing multibyte sequence.
                    self.frag_utf8_validated += err.valid_up_to();
                    Ok(())
                }
            }
        }
    }

    fn reset_fragment_state(&mut self) {
        self.frag_opcode = None;
        self.frag_buf.clear();
        self.frag_utf8_validated = 0;
    }

    fn try_send_error_close(&mut self, code: CloseCode) {
        if !self.close_sent {
            let _ = self.close(Some(CloseFrame::new(code, String::new())));
        }
    }
}
