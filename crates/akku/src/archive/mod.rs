//! Zero-dependency DEFLATE, Gzip, Tar, and Zip compression and archive family.
//!
//! Four formats, one style: strict, safe, and fully implemented in pure safe
//! Rust on top of `std` only — no external dependencies, no `unsafe`.
//!
//! - [`deflate`] — raw RFC 1951 DEFLATE streams (INFLATE with a mandatory
//!   output cap, DEFLATE encoder with several levels).
//! - [`gzip`] — RFC 1952 gzip members with header/trailer verification and
//!   transparent multi-member streams.
//! - [`tar`] — POSIX ustar and PAX archives with a zip-slip-safe extraction
//!   API.
//! - [`zip`] — PKZIP archives with Store and Deflate entries and guarded
//!   extraction.
//! - [`checksum`] — CRC-32 and Adler-32 primitives shared by the formats.
//!
//! Every decompression path is bounded by a mandatory `max_output_bytes` cap
//! (zip-bomb guard), and every extraction path validates entry paths before
//! writing (zip-slip and symlink-escape guards).
//!
//! # Examples
//!
//! Compose the family: build a tar archive in memory, wrap it in gzip, unwrap
//! both:
//!
//! ```
//! use akku::archive::{gzip, tar};
//!
//! let mut writer = tar::TarWriter::new(Vec::new());
//! writer
//!     .append_file("hello.txt", 0o644, 1_700_000_000, b"hello archive\n")
//!     .unwrap();
//! let tar_bytes = writer.finish().unwrap();
//!
//! let gz = gzip::compress(&tar_bytes);
//! let raw = gzip::decompress(&gz).unwrap();
//! assert_eq!(raw, tar_bytes);
//!
//! let mut reader = tar::TarReader::new(&raw[..]);
//! let entry = reader.next_entry().unwrap().unwrap();
//! assert_eq!(entry.path(), "hello.txt");
//! ```

pub mod checksum;
pub mod deflate;
pub mod error;
pub mod gzip;
pub mod tar;
pub mod zip;

pub use deflate::CompressionLevel;

use std::io;
use std::path::{Path, PathBuf};

use error::ExtractError;

/// Default per-entry extraction cap of [`ExtractOptions`]: 4 GiB.
pub const DEFAULT_MAX_ENTRY_BYTES: u64 = 4 << 30;

/// Default total extraction cap of [`ExtractOptions`]: 16 GiB.
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 16 << 30;

/// Limits and overwrite behaviour shared by
/// [`tar::TarReader::safe_extract_to`] and [`zip::ZipReader::safe_extract_to`].
///
/// Extraction is always bounded: a single entry may not produce more than
/// `max_entry_bytes` and the archive as a whole may not produce more than
/// `max_total_bytes`. Existing destinations are never touched unless
/// `overwrite_existing` is set.
///
/// ```
/// use akku::archive::ExtractOptions;
///
/// let opts = ExtractOptions::new();
/// assert_eq!(opts.max_entry_bytes, 4 << 30);
/// assert!(!opts.overwrite_existing);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractOptions {
    /// Maximum uncompressed bytes a single entry may produce.
    pub max_entry_bytes: u64,
    /// Maximum uncompressed bytes the whole archive may produce.
    pub max_total_bytes: u64,
    /// Whether existing files/directories may be overwritten.
    pub overwrite_existing: bool,
}

impl ExtractOptions {
    /// Default limits: 4 GiB per entry, 16 GiB total, no overwriting.
    pub fn new() -> Self {
        Self {
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            overwrite_existing: false,
        }
    }
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Shared safe-path machinery (used identically by tar and zip extraction).
// ---------------------------------------------------------------------------

/// True when `name` looks like an absolute Windows drive path (`C:` prefix).
fn windows_drive_prefix(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && (b[1] == b':' || b[1] == b'|')
}

/// Split an archive entry name into safe path components.
///
/// Rejects empty names, absolute paths (Unix or Windows), any `..`
/// component, and NUL bytes. `.` components are dropped; both `/` and `\`
/// are treated as separators.
pub(crate) fn safe_components(name: &str) -> Result<Vec<String>, ExtractError> {
    if name.is_empty() {
        return Err(ExtractError::UnsafePath("empty entry name".into()));
    }
    if name.starts_with('/') || name.starts_with('\\') || windows_drive_prefix(name) {
        return Err(ExtractError::UnsafePath(format!("absolute path: {name}")));
    }
    let mut out = Vec::new();
    for part in name.split(['/', '\\']) {
        if part.contains('\0') {
            return Err(ExtractError::UnsafePath(format!(
                "NUL byte in path: {name}"
            )));
        }
        match part {
            "" | "." => {}
            ".." => {
                return Err(ExtractError::UnsafePath(format!(
                    "parent traversal (`..`) in path: {name}"
                )));
            }
            p => out.push(p.to_string()),
        }
    }
    if out.is_empty() {
        return Err(ExtractError::UnsafePath(format!(
            "path normalizes to nothing: {name}"
        )));
    }
    Ok(out)
}

/// Join validated components under `dest_root`.
pub(crate) fn join_comps(dest_root: &Path, comps: &[String]) -> PathBuf {
    let mut target = dest_root.to_path_buf();
    for c in comps {
        target.push(c);
    }
    target
}

/// The validated destination path for `name` under `dest_root`, or an error.
///
/// The returned path is guaranteed to stay inside `dest_root` and to not pass
/// through any symlink. Symlinks *at* the final component are rejected as
/// escapes too, so extraction can never write through a link.
pub(crate) fn safe_target(dest_root: &Path, name: &str) -> Result<PathBuf, ExtractError> {
    let comps = safe_components(name)?;
    let target = join_comps(dest_root, &comps);
    ensure_no_symlink_on_path(dest_root, &comps)?;
    Ok(target)
}

/// Reject if any component of the relative path exists and is a symlink.
fn ensure_no_symlink_on_path(dest_root: &Path, comps: &[String]) -> Result<(), ExtractError> {
    let mut cur = dest_root.to_path_buf();
    for c in comps {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) => {
                if md.file_type().is_symlink() {
                    return Err(ExtractError::SymlinkEscape(format!(
                        "path passes through symlink: {}",
                        cur.to_string_lossy()
                    )));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(ExtractError::Io(e)),
        }
    }
    Ok(())
}

/// Validate a symlink target and resolve it lexically inside `dest_root`.
///
/// `link_comps` are the validated components of the symlink entry itself
/// (including its own name); the target is resolved relative to the link's
/// **parent directory**, exactly like the OS does.
///
/// The rules are strict so that the lexical result always matches what the
/// kernel resolves at use time:
///
/// - absolute targets (Unix or Windows), empty targets, and NUL bytes are
///   rejected;
/// - `..` is only allowed as a leading prefix (`../../x`, never `a/../b`),
///   because `a/..` may resolve differently when `a` is itself a symlink;
/// - the leading `..` run may not pop above `dest_root`.
///
/// Because the link's parent directories are guaranteed to be real
/// directories (see [`safe_target`]), the result can never escape the root.
pub(crate) fn resolve_symlink_target(
    dest_root: &Path,
    link_comps: &[String],
    target: &str,
) -> Result<PathBuf, ExtractError> {
    let escape = |why: &str| ExtractError::SymlinkEscape(format!("{why}: {target}"));
    if target.is_empty() {
        return Err(ExtractError::SymlinkEscape("empty link target".into()));
    }
    if target.starts_with('/') || target.starts_with('\\') || windows_drive_prefix(target) {
        return Err(escape("absolute link target"));
    }
    if target.contains('\0') {
        return Err(escape("NUL byte in link target"));
    }
    let parent_len = link_comps.len().saturating_sub(1);
    let mut stack: Vec<&str> = link_comps[..parent_len]
        .iter()
        .map(String::as_str)
        .collect();
    let mut seen_normal = false;
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if seen_normal {
                    return Err(escape("`..` after a normal component in link target"));
                }
                if stack.pop().is_none() {
                    return Err(escape("link target escapes the destination"));
                }
            }
            p => {
                seen_normal = true;
                stack.push(p);
            }
        }
    }
    let owned: Vec<String> = stack.into_iter().map(str::to_string).collect();
    Ok(join_comps(dest_root, &owned))
}

/// Validate a hard-link source (an archive-root-relative path) and return
/// its on-disk location.
///
/// The source must be a safe relative path (no `..`, not absolute), must not
/// pass through or be a symlink, and must already exist as a regular file
/// inside `dest_root`. This prevents hard-linking (and later overwriting)
/// files outside the destination via a previously extracted symlink.
pub(crate) fn resolve_hardlink_source(
    dest_root: &Path,
    target: &str,
) -> Result<PathBuf, ExtractError> {
    let comps = safe_components(target)?;
    ensure_no_symlink_on_path(dest_root, &comps)?;
    let src = join_comps(dest_root, &comps);
    match std::fs::symlink_metadata(&src) {
        Ok(md) if md.file_type().is_file() => Ok(src),
        Ok(_) => Err(ExtractError::LinkTargetMissing(format!(
            "hard link source is not a regular file: {target}"
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Err(ExtractError::LinkTargetMissing(target.to_string()))
        }
        Err(e) => Err(ExtractError::Io(e)),
    }
}
