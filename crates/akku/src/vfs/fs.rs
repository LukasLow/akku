use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::error::Result;
use super::types::{
    Access, Attr, Caller, DirCursor, DirPage, FsInfo, FsStat, Limits, Name, NodeId, ReadResult,
};

/// Heap-allocated `Send` future used by [`DynFilesystem`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Static async read-side virtual filesystem trait.
///
/// Implementors may write `async fn` for every async method; the compiler
/// checks the `Send` bound on each returned future. For dynamic dispatch, use
/// [`DynFilesystem`] (blanket-implemented for every `F: Filesystem`), which
/// pairs with forwarding impls for `dyn DynFilesystem`, `Arc<F>`, `Box<F>`,
/// and `&F`.
///
/// # Contracts
///
/// Every implementation must satisfy:
/// - **Concurrent access:** all methods take `&self` and may run concurrently.
/// - **Non-blocking:** never block the executor thread or hold a lock across `.await`.
/// - **Cancellation safety:** dropping any returned future after any number of
///   polls leaves the filesystem consistent (`read` may leave `buf` partially written).
/// - **Stale ids:** unknown, removed, or foreign-epoch node ids return
///   [`ErrorKind::Stale`](super::ErrorKind::Stale) on every id-taking method;
///   [`ErrorKind::NotFound`](super::ErrorKind::NotFound) is reserved for absent names.
/// - **Bounded outputs and progress:** `read` writes at most `buf.len()` bytes
///   and returns `len >= 1` when `!buf.is_empty() && !eof`; `read_dir` returns
///   at most `max(1, max_entries)` entries and is non-empty when `!eof`.
/// - **No symlink following:** `lookup` returns a symlink's own [`Attr`];
///   traversing through a symlink returns [`ErrorKind::NotADirectory`](super::ErrorKind::NotADirectory).
pub trait Filesystem: Send + Sync {
    /// Returns static filesystem capabilities and parameters.
    ///
    /// Must be cheap and return equal values for the lifetime of the backend.
    fn info(&self) -> FsInfo;

    /// Returns the root directory's [`NodeId`], constant within one [`FsInfo::id_epoch`].
    fn root(&self) -> NodeId;

    /// Returns the attributes of `id` (`attr.id == id`).
    ///
    /// Errors: unknown id -> [`ErrorKind::Stale`](super::ErrorKind::Stale).
    fn getattr(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<Attr>> + Send;

    /// Looks up `name` in directory `dir` and returns the child's attributes
    /// without following symlinks.
    ///
    /// Errors: unknown `dir` -> `Stale`; `dir` not a directory -> `NotADirectory`;
    /// absent name -> `NotFound`; name longer than backend's `max_name_len` -> `NameTooLong`.
    fn lookup(
        &self,
        caller: &Caller,
        dir: NodeId,
        name: Name<'_>,
    ) -> impl Future<Output = Result<Attr>> + Send;

    /// Returns the attributes of the parent directory of `dir`.
    ///
    /// The parent of [`root`](Self::root) is the root itself.
    /// Errors: unknown `dir` -> `Stale`; `dir` not a directory -> `NotADirectory`.
    fn parent(&self, caller: &Caller, dir: NodeId) -> impl Future<Output = Result<Attr>> + Send;

    /// Reads file bytes at `offset` into `buf`.
    ///
    /// Guarantees `len <= buf.len()`, `offset >= size` returns `(0, true)`,
    /// empty `buf` returns `(0, offset >= size)`, and non-empty `buf` with
    /// `eof == false` returns `len >= 1`. Bytes past `buf[..len]` are unspecified,
    /// and a dropped future may leave `buf` partially written.
    ///
    /// Errors: unknown `id` -> `Stale`; directory -> `IsADirectory`;
    /// symlink -> `InvalidInput`; temporarily unavailable -> `TryAgain`.
    fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<ReadResult>> + Send;

    /// Returns one page of directory `dir` following `cursor`.
    ///
    /// Returns at most `max(1, max_entries)` entries (backends may clamp further).
    /// When `with_attrs` is `false`, every entry's `attr` must be `None`; when
    /// `true`, any `Some(attr)` must match `entry.id` and `entry.kind`.
    ///
    /// Errors: unknown `dir` -> `Stale`; non-directory -> `NotADirectory`;
    /// invalid cursor/verifier -> `BadCookie`.
    fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> impl Future<Output = Result<DirPage>> + Send;

    /// Checks which bits in `want` the `caller` is permitted on `id`.
    ///
    /// The returned mask is always a subset of `want`. The default
    /// implementation evaluates POSIX mode bits via [`Access::from_mode`].
    fn access(
        &self,
        caller: &Caller,
        id: NodeId,
        want: Access,
    ) -> impl Future<Output = Result<Access>> + Send {
        async move {
            let _ = (caller, id, want);
            todo!()
        }
    }

    /// Returns the raw target bytes of symlink `id`.
    ///
    /// Targets are non-empty, contain no NUL bytes, and fit within the backend's
    /// `max_symlink_len`. The default calls [`getattr`](Self::getattr) (enforcing
    /// the stale rule) and returns [`ErrorKind::InvalidInput`](super::ErrorKind::InvalidInput).
    fn read_link(
        &self,
        caller: &Caller,
        id: NodeId,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send {
        async move {
            let _ = (caller, id);
            todo!()
        }
    }

    /// Returns space and file-count statistics for the filesystem containing `id`.
    ///
    /// The default calls [`getattr`](Self::getattr) (enforcing the stale rule)
    /// and returns [`FsStat::unknown`].
    fn statfs(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<FsStat>> + Send {
        async move {
            let _ = (caller, id);
            todo!()
        }
    }
}

/// Dyn-compatible twin of [`Filesystem`] returning boxed futures.
///
/// Automatically implemented for every sized `F: Filesystem`. Methods use a
/// `_dyn` suffix so importing both traits never causes method-call ambiguity.
pub trait DynFilesystem: Send + Sync {
    /// Dynamic dispatch version of [`Filesystem::info`].
    fn info_dyn(&self) -> FsInfo;

    /// Dynamic dispatch version of [`Filesystem::root`].
    fn root_dyn(&self) -> NodeId;

    /// Dynamic dispatch version of [`Filesystem::getattr`].
    fn getattr_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId) -> BoxFuture<'a, Result<Attr>>;

    /// Dynamic dispatch version of [`Filesystem::lookup`].
    fn lookup_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        dir: NodeId,
        name: Name<'a>,
    ) -> BoxFuture<'a, Result<Attr>>;

    /// Dynamic dispatch version of [`Filesystem::parent`].
    fn parent_dyn<'a>(&'a self, caller: &'a Caller, dir: NodeId) -> BoxFuture<'a, Result<Attr>>;

    /// Dynamic dispatch version of [`Filesystem::read`].
    fn read_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        id: NodeId,
        offset: u64,
        buf: &'a mut [u8],
    ) -> BoxFuture<'a, Result<ReadResult>>;

    /// Dynamic dispatch version of [`Filesystem::read_dir`].
    fn read_dir_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> BoxFuture<'a, Result<DirPage>>;

    /// Dynamic dispatch version of [`Filesystem::access`].
    fn access_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        id: NodeId,
        want: Access,
    ) -> BoxFuture<'a, Result<Access>>;

    /// Dynamic dispatch version of [`Filesystem::read_link`].
    fn read_link_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        id: NodeId,
    ) -> BoxFuture<'a, Result<Vec<u8>>>;

    /// Dynamic dispatch version of [`Filesystem::statfs`].
    fn statfs_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId) -> BoxFuture<'a, Result<FsStat>>;
}

impl<F: Filesystem> DynFilesystem for F {
    fn info_dyn(&self) -> FsInfo {
        Filesystem::info(self)
    }

    fn root_dyn(&self) -> NodeId {
        Filesystem::root(self)
    }

    fn getattr_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId) -> BoxFuture<'a, Result<Attr>> {
        Box::pin(Filesystem::getattr(self, caller, id))
    }

    fn lookup_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        dir: NodeId,
        name: Name<'a>,
    ) -> BoxFuture<'a, Result<Attr>> {
        Box::pin(Filesystem::lookup(self, caller, dir, name))
    }

    fn parent_dyn<'a>(&'a self, caller: &'a Caller, dir: NodeId) -> BoxFuture<'a, Result<Attr>> {
        Box::pin(Filesystem::parent(self, caller, dir))
    }

    fn read_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        id: NodeId,
        offset: u64,
        buf: &'a mut [u8],
    ) -> BoxFuture<'a, Result<ReadResult>> {
        Box::pin(Filesystem::read(self, caller, id, offset, buf))
    }

    fn read_dir_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> BoxFuture<'a, Result<DirPage>> {
        Box::pin(Filesystem::read_dir(
            self,
            caller,
            dir,
            cursor,
            max_entries,
            with_attrs,
        ))
    }

    fn access_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        id: NodeId,
        want: Access,
    ) -> BoxFuture<'a, Result<Access>> {
        Box::pin(Filesystem::access(self, caller, id, want))
    }

    fn read_link_dyn<'a>(
        &'a self,
        caller: &'a Caller,
        id: NodeId,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(Filesystem::read_link(self, caller, id))
    }

    fn statfs_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId) -> BoxFuture<'a, Result<FsStat>> {
        Box::pin(Filesystem::statfs(self, caller, id))
    }
}

impl<'d> Filesystem for dyn DynFilesystem + 'd {
    fn info(&self) -> FsInfo {
        self.info_dyn()
    }

    fn root(&self) -> NodeId {
        self.root_dyn()
    }

    async fn getattr(&self, caller: &Caller, id: NodeId) -> Result<Attr> {
        self.getattr_dyn(caller, id).await
    }

    async fn lookup(&self, caller: &Caller, dir: NodeId, name: Name<'_>) -> Result<Attr> {
        self.lookup_dyn(caller, dir, name).await
    }

    async fn parent(&self, caller: &Caller, dir: NodeId) -> Result<Attr> {
        self.parent_dyn(caller, dir).await
    }

    async fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<ReadResult> {
        self.read_dyn(caller, id, offset, buf).await
    }

    async fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> Result<DirPage> {
        self.read_dir_dyn(caller, dir, cursor, max_entries, with_attrs)
            .await
    }

    async fn access(&self, caller: &Caller, id: NodeId, want: Access) -> Result<Access> {
        self.access_dyn(caller, id, want).await
    }

    async fn read_link(&self, caller: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.read_link_dyn(caller, id).await
    }

    async fn statfs(&self, caller: &Caller, id: NodeId) -> Result<FsStat> {
        self.statfs_dyn(caller, id).await
    }
}

impl<F: Filesystem + ?Sized> Filesystem for Arc<F> {
    fn info(&self) -> FsInfo {
        (**self).info()
    }

    fn root(&self) -> NodeId {
        (**self).root()
    }

    fn getattr(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<Attr>> + Send {
        (**self).getattr(caller, id)
    }

    fn lookup(
        &self,
        caller: &Caller,
        dir: NodeId,
        name: Name<'_>,
    ) -> impl Future<Output = Result<Attr>> + Send {
        (**self).lookup(caller, dir, name)
    }

    fn parent(&self, caller: &Caller, dir: NodeId) -> impl Future<Output = Result<Attr>> + Send {
        (**self).parent(caller, dir)
    }

    fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<ReadResult>> + Send {
        (**self).read(caller, id, offset, buf)
    }

    fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> impl Future<Output = Result<DirPage>> + Send {
        (**self).read_dir(caller, dir, cursor, max_entries, with_attrs)
    }

    fn access(
        &self,
        caller: &Caller,
        id: NodeId,
        want: Access,
    ) -> impl Future<Output = Result<Access>> + Send {
        (**self).access(caller, id, want)
    }

    fn read_link(
        &self,
        caller: &Caller,
        id: NodeId,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send {
        (**self).read_link(caller, id)
    }

    fn statfs(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<FsStat>> + Send {
        (**self).statfs(caller, id)
    }
}

impl<F: Filesystem + ?Sized> Filesystem for Box<F> {
    fn info(&self) -> FsInfo {
        (**self).info()
    }

    fn root(&self) -> NodeId {
        (**self).root()
    }

    fn getattr(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<Attr>> + Send {
        (**self).getattr(caller, id)
    }

    fn lookup(
        &self,
        caller: &Caller,
        dir: NodeId,
        name: Name<'_>,
    ) -> impl Future<Output = Result<Attr>> + Send {
        (**self).lookup(caller, dir, name)
    }

    fn parent(&self, caller: &Caller, dir: NodeId) -> impl Future<Output = Result<Attr>> + Send {
        (**self).parent(caller, dir)
    }

    fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<ReadResult>> + Send {
        (**self).read(caller, id, offset, buf)
    }

    fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> impl Future<Output = Result<DirPage>> + Send {
        (**self).read_dir(caller, dir, cursor, max_entries, with_attrs)
    }

    fn access(
        &self,
        caller: &Caller,
        id: NodeId,
        want: Access,
    ) -> impl Future<Output = Result<Access>> + Send {
        (**self).access(caller, id, want)
    }

    fn read_link(
        &self,
        caller: &Caller,
        id: NodeId,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send {
        (**self).read_link(caller, id)
    }

    fn statfs(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<FsStat>> + Send {
        (**self).statfs(caller, id)
    }
}

impl<F: Filesystem + ?Sized> Filesystem for &F {
    fn info(&self) -> FsInfo {
        (**self).info()
    }

    fn root(&self) -> NodeId {
        (**self).root()
    }

    fn getattr(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<Attr>> + Send {
        (**self).getattr(caller, id)
    }

    fn lookup(
        &self,
        caller: &Caller,
        dir: NodeId,
        name: Name<'_>,
    ) -> impl Future<Output = Result<Attr>> + Send {
        (**self).lookup(caller, dir, name)
    }

    fn parent(&self, caller: &Caller, dir: NodeId) -> impl Future<Output = Result<Attr>> + Send {
        (**self).parent(caller, dir)
    }

    fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<ReadResult>> + Send {
        (**self).read(caller, id, offset, buf)
    }

    fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> impl Future<Output = Result<DirPage>> + Send {
        (**self).read_dir(caller, dir, cursor, max_entries, with_attrs)
    }

    fn access(
        &self,
        caller: &Caller,
        id: NodeId,
        want: Access,
    ) -> impl Future<Output = Result<Access>> + Send {
        (**self).access(caller, id, want)
    }

    fn read_link(
        &self,
        caller: &Caller,
        id: NodeId,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send {
        (**self).read_link(caller, id)
    }

    fn statfs(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<FsStat>> + Send {
        (**self).statfs(caller, id)
    }
}

/// Walks a `/`-separated byte path from `fs.root()` and returns the final
/// node's [`Attr`].
///
/// All path validation happens before the first backend call:
/// - `path.len() > limits.max_path_len` -> [`ErrorKind::NameTooLong`](super::ErrorKind::NameTooLong)
/// - More than `limits.max_path_depth` non-empty components -> [`ErrorKind::InvalidInput`](super::ErrorKind::InvalidInput)
/// - Each non-empty component is validated through [`Name::new`]
///
/// Empty components (leading, trailing, or repeated `/`) are skipped, so `b""`
/// and `b"/"` resolve to the root. Symlinks are not followed: a final symlink
/// returns its own attributes, and a symlink in the middle causes the next
/// `lookup` to fail with [`ErrorKind::NotADirectory`](super::ErrorKind::NotADirectory).
pub async fn resolve<F: Filesystem + ?Sized>(
    fs: &F,
    caller: &Caller,
    path: &[u8],
    limits: &Limits,
) -> Result<Attr> {
    let _ = (fs, caller, path, limits);
    todo!()
}
