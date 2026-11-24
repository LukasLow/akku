//! File-system helpers.
//!
//! The `fs` domain bundles the `std` file-system API with a few practical
//! additions: directory [`walk`] iteration, glob [`glob`] matching,
//! [`write_atomic`], and self-cleaning temp files and folders ([`TempFile`],
//! [`TempDir`]).
//!
//! Names that `std` already owns well are re-exported unchanged so one
//! `use akku::fs;` statement is enough for everyday code.
//!
//! # Examples
//!
//! Read and write a file atomically:
//!
//! ```
//! let file = std::env::temp_dir().join("akku-fs-doctest.txt");
//! akku::fs::write_atomic(&file, b"version 2");
//! assert_eq!(std::fs::read_to_string(&file).unwrap(), "version 2");
//! let _ = std::fs::remove_file(&file);
//! ```
//!
//! Match a file tree with a glob:
//!
//! ```
//! use akku::fs::GlobMatcher;
//!
//! let matcher = GlobMatcher::new("src/**/*.rs").unwrap();
//! assert!(matcher.is_match("src/fs/walk.rs"));
//! assert!(!matcher.is_match("target/walk.rs"));
//! ```

use std::io;
use std::path::{Path, PathBuf};

pub(crate) mod glob;
pub(crate) mod temp;
pub(crate) mod walk;

pub use glob::{glob, GlobError, GlobMatcher, PatternIssue};
pub use temp::{TempDir, TempFile};
pub use walk::{walk, Walk, WalkBuilder, WalkEntry, WalkError};

// Non-deprecated std::fs and std::io surface, re-exported on purpose.
pub use std::fs::{
    canonicalize, copy, create_dir, create_dir_all, hard_link, metadata, read, read_dir, read_link,
    read_to_string, remove_dir, remove_dir_all, remove_file, rename, set_permissions,
    symlink_metadata, write,
};
pub use std::fs::{DirEntry, File, FileType, Metadata, OpenOptions, Permissions, ReadDir};
pub use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};

/// Upper bound on write-atomic temp-file attempts before giving up.
const ATOMIC_ATTEMPTS: u32 = 10_000;

/// Process-wide counter that makes generated temp names unique in one process.
static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_counter() -> u64 {
    std::sync::atomic::AtomicU64::fetch_add(&COUNTER, 1, std::sync::atomic::Ordering::Relaxed)
}

fn temp_stem(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    let name = if prefix.is_empty() { "akku" } else { prefix };
    format!(
        "{name}.{}.{:x}.{:x}.tmp",
        std::process::id(),
        next_counter(),
        nanos
    )
}

/// Whether a path exists, distinguishing "missing" from "unreachable".
///
/// Unlike `std::fs::exists`, this exposes the same non-following semantics
/// as [`std::path::Path::try_exists`]: a dangling symlink is not an error.
///
/// # Examples
///
/// ```
/// let missing = std::env::temp_dir().join("akku-never-exists");
/// assert!(!akku::fs::try_exists(&missing).unwrap());
/// ```
///
/// # Errors
///
/// Propagates any I/O error.
pub fn try_exists(path: impl AsRef<Path>) -> io::Result<bool> {
    path.as_ref().try_exists()
}

/// Write `bytes` to `path` atomically.
///
/// The payload goes into a fresh temp file inside the same parent directory
/// (created with `create_new(true)`), is flushed to disk, and is then moved
/// onto `path` with a single [`rename`]. Readers therefore only ever see a
/// complete old or a complete new file, never a partial write.
///
/// On unix the parent directory is opened and `fsync`ed after the rename so
/// the new directory entry itself survives a power loss. On platforms where a
/// directory cannot be opened as a file (e.g. Windows) that step is skipped.
///
/// The call fails with [`io::ErrorKind::NotFound`] when the parent directory
/// does not exist.
///
/// # Examples
///
/// ```
/// use akku::fs::write_atomic;
///
/// let path = std::env::temp_dir().join("akku-atomic-doctest.txt");
/// write_atomic(&path, b"first")?;
/// write_atomic(&path, b"second")?;
/// assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
/// let _ = std::fs::remove_file(&path);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// Returns any error from creating, writing, or renaming the temp file, or
/// from `fsync`ing the parent directory after the rename (unix only). When the
/// directory sync fails the rename has already happened, so `path` may hold
/// the new contents even though an error is returned.
pub fn write_atomic(path: impl AsRef<Path>, bytes: &[u8]) -> io::Result<()> {
    let path = path.as_ref();
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };

    for _ in 0..ATOMIC_ATTEMPTS {
        let stem = temp_stem("akku-atomic");
        let tmp = parent.join(stem);
        let mut file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(err) if matches!(err.kind(), io::ErrorKind::AlreadyExists) => continue,
            Err(err) => return Err(err),
        };
        if let Err(err) = file.write_all(bytes) {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
        if let Err(err) = file.sync_all() {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
        drop(file);
        if let Err(err) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
        return sync_parent_dir(&parent);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "write_atomic could not create a unique temp name beside {} after {ATOMIC_ATTEMPTS} attempts",
            path.display()
        ),
    ))
}

/// Flushes the directory entry of a just-renamed file to disk.
#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

/// Directory handles cannot be `fsync`ed portably outside unix; the rename is
/// already as durable as the platform allows.
#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> io::Result<()> {
    Ok(())
}
