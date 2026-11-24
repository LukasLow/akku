//! Self-cleaning temporary files and directories.
//!
//! [`TempFile`] and [`TempDir`] create uniquely named temporary paths with
//! `create_new` (nothing is ever overwritten) and delete themselves when
//! dropped — unless [`persist`](TempFile::persist) keeps them.
//!
//! # Examples
//!
//! ```
//! use akku::fs::TempDir;
//!
//! let gone = {
//!     let dir = TempDir::new("build").unwrap();
//!     std::fs::write(dir.as_path().join("out.txt"), b"data").unwrap();
//!     assert!(dir.as_path().join("out.txt").exists());
//!     dir.as_path().join("out.txt")
//! };
//! // The directory (and its content) is gone after the TempDir was dropped:
//! assert!(!gone.exists());
//! ```

use std::fs;
use std::io;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Longest allowed prefix length; keeps generated names below `NAME_MAX`.
const MAX_PREFIX_CHARS: usize = 64;
/// Upper bound on unique-name attempts before giving up.
const MAX_ATTEMPTS: u32 = 1_000;

/// Check a temp prefix: only `A-Z`, `a-z`, `0-9`, `.`, `_`, `-` are allowed.
fn validate_prefix(prefix: &str) -> io::Result<()> {
    if prefix.chars().count() > MAX_PREFIX_CHARS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("temp prefix is longer than {MAX_PREFIX_CHARS} characters"),
        ));
    }
    if let Some(bad) = prefix
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '_' | '-'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "temp prefix contains the character {bad:?}; allowed are A-Za-z0-9, '.', '_' and '-'"
            ),
        ));
    }
    Ok(())
}

fn exhausted_error(dir: &Path, kind: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "could not create a unique temp {kind} in {} after {MAX_ATTEMPTS} attempts",
            dir.display()
        ),
    )
}

/// A temporary file that deletes itself when dropped.
///
/// The file is created with `create_new(true)`, so the name is always fresh.
/// The content stays empty until [`write`](TempFile::write) fills it.
///
/// # Examples
///
/// ```
/// use akku::fs::TempFile;
///
/// let temp = TempFile::new("demo").unwrap();
/// temp.write(b"payload").unwrap();
/// assert!(temp.as_path().exists());
///
/// // Persisting detaches the file from self-cleaning:
/// let kept = temp.persist().unwrap();
/// assert!(kept.exists());
/// let _ = std::fs::remove_file(kept);
/// ```
#[derive(Debug)]
pub struct TempFile {
    path: Option<PathBuf>,
}

impl TempFile {
    /// Create a temp file inside the system temp directory.
    ///
    /// An empty prefix falls back to `akku`.
    ///
    /// # Errors
    ///
    /// Returns an [`io::ErrorKind::InvalidInput`] error for bad prefixes and
    /// propagates any error from creating the file.
    pub fn new(prefix: &str) -> io::Result<Self> {
        Self::new_in(std::env::temp_dir(), prefix)
    }

    /// Create a temp file inside `dir`, which must already exist.
    ///
    /// # Errors
    ///
    /// Returns an [`io::ErrorKind::InvalidInput`] error for bad prefixes and
    /// propagates any error from creating the file, including
    /// [`io::ErrorKind::NotFound`] when `dir` is missing.
    pub fn new_in(dir: impl AsRef<Path>, prefix: &str) -> io::Result<Self> {
        validate_prefix(prefix)?;
        let dir = dir.as_ref();
        for _ in 0..MAX_ATTEMPTS {
            let path = dir.join(super::temp_stem(prefix));
            match fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
            {
                Ok(_file) => return Ok(Self { path: Some(path) }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
        }
        Err(exhausted_error(dir, "file"))
    }

    /// The path of the temp file.
    pub fn as_path(&self) -> &Path {
        self.path.as_deref().unwrap_or(Path::new(""))
    }

    /// Write `bytes` to the file, replacing any previous content.
    ///
    /// # Errors
    ///
    /// Propagates any I/O error from opening or writing the file.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "temp file is already detached",
            ));
        };
        fs::File::create(path)?.write_all(bytes)
    }

    /// Keep the file and disable self-cleaning; returns its path.
    ///
    /// # Errors
    ///
    /// Returns an [`io::ErrorKind::NotFound`] error when the file was
    /// already detached.
    pub fn persist(mut self) -> io::Result<PathBuf> {
        self.path
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "temp file is already detached"))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(&path);
        }
    }
}

/// A temporary directory that deletes itself (recursively) when dropped.
///
/// # Examples
///
/// ```
/// use akku::fs::TempDir;
///
/// let dir = TempDir::new("work")?;
/// std::fs::write(dir.as_path().join("a.txt"), b"x")?;
/// assert!(dir.as_path().join("a.txt").exists());
/// drop(dir);
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct TempDir {
    path: Option<PathBuf>,
}

impl TempDir {
    /// Create a temp directory inside the system temp directory.
    ///
    /// An empty prefix falls back to `akku`.
    ///
    /// # Errors
    ///
    /// Returns an [`io::ErrorKind::InvalidInput`] error for bad prefixes and
    /// propagates any error from creating the directory.
    pub fn new(prefix: &str) -> io::Result<Self> {
        Self::new_in(std::env::temp_dir(), prefix)
    }

    /// Create a temp directory inside `dir`, which must already exist.
    ///
    /// # Errors
    ///
    /// Returns an [`io::ErrorKind::InvalidInput`] error for bad prefixes and
    /// propagates any error from creating the directory, including
    /// [`io::ErrorKind::NotFound`] when `dir` is missing.
    pub fn new_in(dir: impl AsRef<Path>, prefix: &str) -> io::Result<Self> {
        validate_prefix(prefix)?;
        let dir = dir.as_ref();
        for _ in 0..MAX_ATTEMPTS {
            let path = dir.join(super::temp_stem(prefix));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path: Some(path) }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
        }
        Err(exhausted_error(dir, "directory"))
    }

    /// The path of the temp directory.
    pub fn as_path(&self) -> &Path {
        self.path.as_deref().unwrap_or(Path::new(""))
    }

    /// Keep the directory and disable self-cleaning; returns its path.
    ///
    /// # Errors
    ///
    /// Returns an [`io::ErrorKind::NotFound`] error when the directory was
    /// already detached.
    pub fn persist(mut self) -> io::Result<PathBuf> {
        self.path
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "temp dir is already detached"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_dir_all(&path);
        }
    }
}
