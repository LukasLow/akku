//! High-level WebSocket messages.

use crate::ws::frame::CloseFrame;

/// A high-level RFC 6455 WebSocket message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// A valid UTF-8 text message.
    Text(String),
    /// An arbitrary binary message.
    Binary(Vec<u8>),
    /// A `Ping` control message (payload at most 125 bytes).
    Ping(Vec<u8>),
    /// A `Pong` control message (payload at most 125 bytes).
    Pong(Vec<u8>),
    /// A `Close` control message with optional status code and reason.
    Close(Option<CloseFrame>),
}

impl Message {
    /// Creates a [`Message::Text`] variant.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// Creates a [`Message::Binary`] variant.
    pub fn binary(data: impl Into<Vec<u8>>) -> Self {
        Self::Binary(data.into())
    }

    /// Consumes the message and returns the text string if this is [`Message::Text`].
    pub fn into_text(self) -> Option<String> {
        match self {
            Self::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Consumes the message and returns its payload bytes (empty for `Close(None)`).
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Text(s) => s.into_bytes(),
            Self::Binary(b) | Self::Ping(b) | Self::Pong(b) => b,
            Self::Close(Some(cf)) => cf.encode_payload().unwrap_or_default(),
            Self::Close(None) => Vec::new(),
        }
    }

    /// Returns `true` if this message is [`Message::Text`].
    pub const fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// Returns `true` if this message is [`Message::Binary`].
    pub const fn is_binary(&self) -> bool {
        matches!(self, Self::Binary(_))
    }

    /// Returns `true` if this message is [`Message::Close`].
    pub const fn is_close(&self) -> bool {
        matches!(self, Self::Close(_))
    }
}

impl From<String> for Message {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for Message {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl From<Vec<u8>> for Message {
    fn from(b: Vec<u8>) -> Self {
        Self::Binary(b)
    }
}

impl From<&[u8]> for Message {
    fn from(b: &[u8]) -> Self {
        Self::Binary(b.to_vec())
    }
}
