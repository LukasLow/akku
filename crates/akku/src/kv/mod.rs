//! Zero-dependency key-value store with durable WAL, snapshots, and prefix scans.
//!
//! `akku::kv` provides a unified synchronous [`KvStore`] interface backed by:
//! - [`MemoryStore`]: Thread-safe, ordered in-memory `BTreeMap` store with TTL expiration,
//!   lexicographical prefix and range scans, and configurable size limits.
//! - [`FileStore`]: Embedded durable store using an append-only Write-Ahead Log (WAL)
//!   with IEEE 802.3 CRC32 record framing, crash recovery that automatically truncates
//!   torn trailing WAL records, and atomic snapshot compaction via [`akku::fs::write_atomic`](crate::fs::write_atomic).
//! - [`redis::RedisClient`]: Zero-dependency blocking Redis client over [`std::net::TcpStream`]
//!   implementing the RESP2 wire protocol (tolerating common RESP3 reply frames) with depth
//!   and bulk-size DoS caps.
//!
//! # Examples
//!
//! ```
//! use akku::kv::{BatchOp, KvStore, MemoryStore};
//!
//! let store = MemoryStore::new();
//! store.set("user:1", b"Ada").unwrap();
//! store.set("user:2", b"Grace").unwrap();
//!
//! assert_eq!(store.get("user:1").unwrap(), Some(b"Ada".to_vec()));
//!
//! let users = store.scan_prefix("user:").unwrap();
//! assert_eq!(users.len(), 2);
//! ```

use std::error::Error;
use std::fmt;
use std::io;
use std::time::Duration;

pub mod file;
pub mod memory;
pub mod redis;

pub use file::FileStore;
pub use memory::MemoryStore;

/// Default maximum key size in bytes (4 KiB).
pub const DEFAULT_MAX_KEY_SIZE: usize = 4 * 1024;

/// Default maximum value size in bytes (16 MiB).
pub const DEFAULT_MAX_VALUE_SIZE: usize = 16 * 1024 * 1024;

/// Result alias for key-value store operations.
pub type KvResult<T> = Result<T, KvError>;

/// Errors produced by [`KvStore`] backends and the [`redis`] client.
#[derive(Debug)]
pub enum KvError {
    /// Underlying filesystem or network I/O error.
    Io(io::Error),
    /// On-disk WAL or record corruption (bad magic header, invalid CRC32 checksum, malformed frame).
    CorruptData(String),
    /// Wire-protocol violation or DoS limit exceeded (e.g., RESP2/RESP3 framing, depth, or bulk size).
    Protocol(String),
    /// Server-side error returned by a remote store (e.g., Redis `-ERR ...`).
    Server(String),
    /// Invalid key, oversized key/value, unsafe path, or invalid argument provided by the caller.
    InvalidInput(String),
}

impl fmt::Display for KvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::CorruptData(msg) => write!(f, "corrupt data: {msg}"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::Server(msg) => write!(f, "server error: {msg}"),
            Self::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
        }
    }
}

impl Error for KvError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for KvError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// DoS protection and resource limits for [`KvStore`] keys and values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KvLimits {
    /// Maximum allowed byte length of a key (defaults to [`DEFAULT_MAX_KEY_SIZE`]).
    pub max_key_size: usize,
    /// Maximum allowed byte length of a value (defaults to [`DEFAULT_MAX_VALUE_SIZE`]).
    pub max_value_size: usize,
}

impl Default for KvLimits {
    fn default() -> Self {
        Self {
            max_key_size: DEFAULT_MAX_KEY_SIZE,
            max_value_size: DEFAULT_MAX_VALUE_SIZE,
        }
    }
}

impl KvLimits {
    /// Creates a new [`KvLimits`] with explicit `max_key_size` and `max_value_size`.
    pub const fn new(max_key_size: usize, max_value_size: usize) -> Self {
        Self {
            max_key_size,
            max_value_size,
        }
    }

    /// Validates a key against `max_key_size` and non-empty / NUL-free constraints.
    pub fn validate_key(&self, key: &str) -> KvResult<()> {
        if key.is_empty() {
            return Err(KvError::InvalidInput("key must not be empty".to_string()));
        }
        if key.len() > self.max_key_size {
            return Err(KvError::InvalidInput(format!(
                "key length {} exceeds max_key_size {}",
                key.len(),
                self.max_key_size
            )));
        }
        Ok(())
    }

    /// Validates both a key and value payload against configured limits.
    pub fn validate_kv(&self, key: &str, value: &[u8]) -> KvResult<()> {
        self.validate_key(key)?;
        if value.len() > self.max_value_size {
            return Err(KvError::InvalidInput(format!(
                "value length {} exceeds max_value_size {}",
                value.len(),
                self.max_value_size
            )));
        }
        Ok(())
    }
}

/// A single mutation within an atomic or pipelined [`KvStore::batch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchOp {
    /// Sets `key` to `value` with no expiration.
    Set {
        /// Key string.
        key: String,
        /// Raw byte payload.
        value: Vec<u8>,
    },
    /// Sets `key` to `value` with a time-to-live `ttl`.
    SetEx {
        /// Key string.
        key: String,
        /// Raw byte payload.
        value: Vec<u8>,
        /// Expiration duration.
        ttl: Duration,
    },
    /// Deletes `key`.
    Delete {
        /// Key string.
        key: String,
    },
}

impl BatchOp {
    /// Convenience constructor for [`BatchOp::Set`].
    pub fn set(key: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        Self::Set {
            key: key.into(),
            value: value.into(),
        }
    }

    /// Convenience constructor for [`BatchOp::SetEx`].
    pub fn set_ex(key: impl Into<String>, value: impl Into<Vec<u8>>, ttl: Duration) -> Self {
        Self::SetEx {
            key: key.into(),
            value: value.into(),
            ttl,
        }
    }

    /// Convenience constructor for [`BatchOp::Delete`].
    pub fn delete(key: impl Into<String>) -> Self {
        Self::Delete { key: key.into() }
    }
}

/// Unified synchronous key-value store interface implemented by [`MemoryStore`],
/// [`FileStore`], and [`redis::RedisClient`].
pub trait KvStore: Send + Sync {
    /// Retrieves the raw byte value for `key`, or `Ok(None)` if absent or expired.
    fn get(&self, key: &str) -> KvResult<Option<Vec<u8>>>;

    /// Stores `key` -> `value` without expiration (clears any previous TTL on `key`).
    fn set(&self, key: &str, value: &[u8]) -> KvResult<()>;

    /// Stores `key` -> `value` with a time-to-live `ttl`.
    fn set_ex(&self, key: &str, value: &[u8], ttl: Duration) -> KvResult<()>;

    /// Deletes `key`, returning `Ok(true)` if a live key existed and was removed.
    fn delete(&self, key: &str) -> KvResult<bool>;

    /// Returns `Ok(true)` if `key` exists and has not expired.
    fn exists(&self, key: &str) -> KvResult<bool>;

    /// Returns all live `(key, value)` pairs whose key starts with `prefix`,
    /// sorted lexicographically by key.
    fn scan_prefix(&self, prefix: &str) -> KvResult<Vec<(String, Vec<u8>)>>;

    /// Returns all live `(key, value)` pairs with keys in the half-open range `[start, end)`,
    /// sorted lexicographically by key.
    fn scan_range(&self, start: &str, end: &str) -> KvResult<Vec<(String, Vec<u8>)>> {
        if start >= end {
            return Ok(Vec::new());
        }
        let all = self.scan_prefix("")?;
        Ok(all
            .into_iter()
            .filter(|(k, _)| k.as_str() >= start && k.as_str() < end)
            .collect())
    }

    /// Applies a sequence of [`BatchOp`] mutations atomically (for embedded stores)
    /// or pipelined (for remote stores).
    fn batch(&self, ops: &[BatchOp]) -> KvResult<()>;
}
