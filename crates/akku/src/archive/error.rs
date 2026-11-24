//! Errors shared by the safe-extraction APIs of [`tar`](super::tar) and
//! [`zip`](super::zip).
//!
//! Both archive formats face the same extraction hazards — path traversal
//! ("zip slip"), symlink escapes, and decompression bombs — so they share one
//! error type for those failures. Format-specific errors stay in each
//! module's own error enum, which embeds [`ExtractError`] when extraction
//! fails.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why a guarded extraction refused to write an entry.
#[derive(Debug)]
#[non_exhaustive]
pub enum ExtractError {
    /// The entry name is absolute, contains `..`, contains a NUL byte, or is
    /// otherwise unusable as a relative destination.
    UnsafePath(String),
    /// Writing the entry would follow (or write through) a symlink that
    /// escapes the destination root, or a link target resolves outside it.
    SymlinkEscape(String),
    /// A hard link's target was not extracted (yet) inside the destination.
    LinkTargetMissing(String),
    /// A single entry exceeds the configured per-entry byte cap.
    EntryTooLarge {
        /// The configured cap.
        limit: u64,
        /// The entry's declared size.
        size: u64,
    },
    /// The accumulated extracted bytes exceed the configured total cap.
    TotalTooLarge {
        /// The configured cap.
        limit: u64,
        /// Bytes written so far when the cap tripped.
        written: u64,
    },
    /// The destination already exists and overwriting is disabled.
    DestinationExists(PathBuf),
    /// The operation is not supported on this platform or for this entry
    /// type (e.g. symlinks outside Unix).
    Unsupported(&'static str),
    /// An I/O error from the underlying reader, writer, or filesystem.
    Io(io::Error),
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafePath(p) => write!(f, "unsafe entry path: {p}"),
            Self::SymlinkEscape(p) => write!(f, "symlink escape: {p}"),
            Self::LinkTargetMissing(p) => write!(f, "hard link target missing: {p}"),
            Self::EntryTooLarge { limit, size } => {
                write!(f, "entry of {size} bytes exceeds the {limit}-byte cap")
            }
            Self::TotalTooLarge { limit, written } => {
                write!(f, "total of {written} bytes exceeds the {limit}-byte cap")
            }
            Self::DestinationExists(p) => {
                write!(f, "destination already exists: {}", p.to_string_lossy())
            }
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for ExtractError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ExtractError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
