use std::fmt;
use std::time::SystemTime;

use super::error::Result;
use super::fs::Filesystem;
use super::types::{
    Attr, Caller, DirCursor, DirPage, FsInfo, FsStat, Limits, Name, NodeId, ReadResult,
};

/// In-memory [`Filesystem`] backend for tests, fixtures, and scratch trees.
///
/// Backed by an interior-mutable tree whose async operations complete on their
/// first poll. Node ids start at `1` for the root, increase monotonically, and
/// are never reused after removal.
pub struct MemFs {
    #[allow(dead_code)] // G3 stub; remove in G4
    _private: (),
}

impl fmt::Debug for MemFs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemFs {
    /// Default maximum number of nodes (including the root): `1_048_576`.
    pub const DEFAULT_MAX_NODES: usize = 0;

    /// Default maximum total accounted bytes: `268_435_456` (256 MiB).
    pub const DEFAULT_MAX_BYTES: usize = 0;

    /// Creates an empty `MemFs` with [`Limits::new()`],
    /// [`DEFAULT_MAX_NODES`](Self::DEFAULT_MAX_NODES), and
    /// [`DEFAULT_MAX_BYTES`](Self::DEFAULT_MAX_BYTES).
    pub fn new() -> Self {
        todo!()
    }

    /// Creates an empty `MemFs` with custom `limits`, node cap `max_nodes`, and
    /// byte cap `max_bytes`.
    pub fn with_limits(limits: Limits, max_nodes: usize, max_bytes: usize) -> Self {
        let _ = (limits, max_nodes, max_bytes);
        todo!()
    }

    /// Returns the [`Limits`] configured on this instance.
    pub fn limits(&self) -> Limits {
        todo!()
    }

    /// Creates a regular file named `name` inside directory `dir` with initial
    /// contents `data`, returning its new [`NodeId`].
    pub fn create_file(&self, dir: NodeId, name: &[u8], data: &[u8]) -> Result<NodeId> {
        let _ = (dir, name, data);
        todo!()
    }

    /// Creates a subdirectory named `name` inside directory `dir`, returning
    /// its new [`NodeId`].
    pub fn mkdir(&self, dir: NodeId, name: &[u8]) -> Result<NodeId> {
        let _ = (dir, name);
        todo!()
    }

    /// Creates a symbolic link named `name` inside directory `dir` pointing to
    /// `target`, returning its new [`NodeId`].
    pub fn symlink(&self, dir: NodeId, name: &[u8], target: &[u8]) -> Result<NodeId> {
        let _ = (dir, name, target);
        todo!()
    }

    /// Writes `data` into regular file `id` starting at `offset`, zero-filling
    /// any gap past the current end.
    ///
    /// All-or-nothing: on error, file contents and budgets are unchanged.
    pub fn write_all(&self, id: NodeId, offset: u64, data: &[u8]) -> Result<()> {
        let _ = (id, offset, data);
        todo!()
    }

    /// Resizes regular file `id` to `len` bytes, truncating or zero-extending.
    ///
    /// All-or-nothing: on error, file contents and budgets are unchanged.
    pub fn set_len(&self, id: NodeId, len: u64) -> Result<()> {
        let _ = (id, len);
        todo!()
    }

    /// Removes a file, symlink, or empty directory named `name` from `dir`.
    ///
    /// The removed node id becomes permanently [`ErrorKind::Stale`](super::ErrorKind::Stale)
    /// and its node slot and accounted bytes return to the budget.
    pub fn remove(&self, dir: NodeId, name: &[u8]) -> Result<()> {
        let _ = (dir, name);
        todo!()
    }

    /// Sets POSIX permission bits (`0o0000..=0o7777`) on `id`.
    pub fn set_perm(&self, id: NodeId, perm: u32) -> Result<()> {
        let _ = (id, perm);
        todo!()
    }

    /// Sets the owner `uid` and `gid` on `id`.
    pub fn set_owner(&self, id: NodeId, uid: u32, gid: u32) -> Result<()> {
        let _ = (id, uid, gid);
        todo!()
    }

    /// Sets the `atime` and `mtime` timestamps on `id`.
    pub fn set_times(&self, id: NodeId, atime: SystemTime, mtime: SystemTime) -> Result<()> {
        let _ = (id, atime, mtime);
        todo!()
    }

    /// Returns the total number of live nodes, including the root directory.
    pub fn node_count(&self) -> usize {
        todo!()
    }

    /// Returns the total accounted bytes across entry names, file data, and
    /// symlink targets.
    pub fn bytes_used(&self) -> usize {
        todo!()
    }
}

impl Filesystem for MemFs {
    fn info(&self) -> FsInfo {
        todo!()
    }

    fn root(&self) -> NodeId {
        todo!()
    }

    async fn getattr(&self, caller: &Caller, id: NodeId) -> Result<Attr> {
        let _ = (caller, id);
        todo!()
    }

    async fn lookup(&self, caller: &Caller, dir: NodeId, name: Name<'_>) -> Result<Attr> {
        let _ = (caller, dir, name);
        todo!()
    }

    async fn parent(&self, caller: &Caller, dir: NodeId) -> Result<Attr> {
        let _ = (caller, dir);
        todo!()
    }

    async fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<ReadResult> {
        let _ = (caller, id, offset, buf);
        todo!()
    }

    async fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> Result<DirPage> {
        let _ = (caller, dir, cursor, max_entries, with_attrs);
        todo!()
    }

    async fn read_link(&self, caller: &Caller, id: NodeId) -> Result<Vec<u8>> {
        let _ = (caller, id);
        todo!()
    }

    async fn statfs(&self, caller: &Caller, id: NodeId) -> Result<FsStat> {
        let _ = (caller, id);
        todo!()
    }
}
