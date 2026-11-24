//! Zero-dependency RFC 6455 WebSocket framing, handshake, masking, and UTF-8 state machine.
//!
//! `akku::ws` works without any external dependencies (including under
//! `--no-default-features`) and provides:
//!
//! - Pure-Rust RFC 6455 wire framing ([`Frame`], [`OpCode`], [`CloseCode`],
//!   [`CloseFrame`], [`apply_mask`]) with strict validation and `max_frame_size`
//!   DoS protection.
//! - RFC 6455 HTTP/1.1 opening handshake ([`handshake::compute_accept_key`],
//!   [`handshake::generate_key`], [`handshake::is_valid_key`]).
//! - Connection state machine ([`WebSocket`], [`Message`], [`Config`], [`Role`])
//!   supporting fragmentation reassembly, fail-fast streaming UTF-8 validation,
//!   interleaved control frames, and automatic `Pong` / `Close` replies.
//! - Synchronous blocking [`client`] and [`server`] helpers over [`std::net::TcpStream`]
//!   (or any `Read + Write` stream on the server side).
//!
//! # Examples
//!
//! ```
//! use akku::ws::handshake;
//! use akku::ws::{Frame, OpCode};
//!
//! // RFC 6455 §1.3 golden handshake vector
//! let accept = handshake::compute_accept_key("dGhlIHNhbXBsZSBub25jZQ==");
//! assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
//!
//! // Encode and decode a masked text frame
//! let frame = Frame::text("Hello").with_mask([0x37, 0xfa, 0x21, 0x3d]);
//! let wire = frame.encode().unwrap();
//! let mut cursor = std::io::Cursor::new(wire);
//! let decoded = Frame::read_from(&mut cursor, 1024).unwrap();
//! assert_eq!(decoded.opcode, OpCode::Text);
//! assert_eq!(decoded.payload, b"Hello");
//! ```

pub mod client;
mod error;
mod frame;
pub mod handshake;
mod message;
pub mod server;
mod sha1;
mod socket;

pub use error::{Error, Result};
pub use frame::{apply_mask, CloseCode, CloseFrame, Frame, OpCode, MAX_CONTROL_PAYLOAD_LEN};
pub use message::Message;
pub use socket::{Config, Role, WebSocket, DEFAULT_MAX_FRAME_SIZE, DEFAULT_MAX_MESSAGE_SIZE};
