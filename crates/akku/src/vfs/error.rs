use std::borrow::Cow;
use std::fmt;
use std::io;

/// Convenience result alias for [`akku::vfs`](crate::vfs) operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Error returned by virtual filesystem operations.
///
/// Carries a neutral [`ErrorKind`] that protocol servers map to wire status
/// codes, an optional human-readable message, and an optional underlying source
/// error accessible via [`std::error::Error::source`].
#[derive(Debug)]
pub struct Error {
    #[allow(dead_code)] // G3 stub; remove in G4
    kind: ErrorKind,
    #[allow(dead_code)] // G3 stub; remove in G4
    message: Cow<'static, str>,
    #[allow(dead_code)] // G3 stub; remove in G4
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl Error {
    /// Builds an error with the given `kind` and `message`.
    ///
    /// `message` may be empty. Passing a `&'static str` avoids heap allocation.
    pub fn new(kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self {
        let _ = (kind, message.into());
        todo!()
    }

    /// Attaches an underlying source error, returned by
    /// [`std::error::Error::source`].
    pub fn with_source(
        self,
        source: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    ) -> Self {
        let _ = (self, source.into());
        todo!()
    }

    /// Returns the error category that protocol servers map to wire status codes.
    pub fn kind(&self) -> ErrorKind {
        todo!()
    }

    /// Returns the human-readable message, which may be empty.
    pub fn message(&self) -> &str {
        todo!()
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        let _ = kind;
        todo!()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        todo!()
    }
}

impl From<Error> for io::Error {
    fn from(err: Error) -> Self {
        let _ = err;
        todo!()
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        let _ = err;
        todo!()
    }
}

/// Neutral error categories for virtual filesystem operations.
///
/// This enum is `#[non_exhaustive]`: protocol servers should map any unknown
/// future variant to their generic I/O error.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The requested entry name does not exist in an existing directory.
    NotFound,
    /// The caller is not permitted to perform an owner-only operation.
    NotPermitted,
    /// Access was denied by permission or policy checks.
    PermissionDenied,
    /// An entry with the given name already exists in the directory.
    AlreadyExists,
    /// A directory operation was attempted on a non-directory node.
    NotADirectory,
    /// A file operation was attempted on a directory node.
    IsADirectory,
    /// A directory could not be removed because it still contains entries.
    DirectoryNotEmpty,
    /// An argument or operation is invalid for the target node or request.
    InvalidInput,
    /// An entry name is empty, `.`, `..`, or contains `/` or NUL.
    InvalidName,
    /// An entry name, path, or symlink target exceeds the configured limit.
    NameTooLong,
    /// A mutation was attempted on a read-only filesystem.
    ReadOnly,
    /// The backend's storage or node budget is exhausted.
    NoSpace,
    /// A user or account quota was exceeded.
    QuotaExceeded,
    /// A file size or write offset exceeds the maximum supported file size.
    FileTooLarge,
    /// An operation attempted to link or rename across filesystems.
    CrossesDevices,
    /// The link count limit for a node was reached.
    TooManyLinks,
    /// The node id is unknown, removed, or from a different `id_epoch`.
    Stale,
    /// A directory listing cookie is invalid for the supplied verifier.
    BadCookie,
    /// The operation is not supported by this backend.
    Unsupported,
    /// The operation is temporarily unavailable and the caller should retry.
    TryAgain,
    /// A generic I/O error.
    Io,
}

impl ErrorKind {
    /// Returns a lower-case static description of this error kind.
    pub const fn as_str(self) -> &'static str {
        todo!()
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}
