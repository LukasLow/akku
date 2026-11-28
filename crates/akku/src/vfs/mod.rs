//! Protocol-neutral, async virtual filesystem trait layer.
//!
//! `akku::vfs` defines the seam between storage backends (such as remote drives
//! or in-memory trees) and protocol servers (such as NFSv3). Backends implement
//! [`Filesystem`] once using `async fn`; generic servers call [`Filesystem`]
//! with zero dispatch overhead, and dynamic composition uses [`DynFilesystem`]
//! (blanket-implemented for every `F: Filesystem`).
//!
//! # Overview
//!
//! | Area | Items |
//! |---|---|
//! | Traits & dispatch | [`Filesystem`], [`DynFilesystem`], [`BoxFuture`] |
//! | Identity & attributes | [`NodeId`], [`NodeKind`], [`Attr`], [`FsInfo`], [`FsStat`] |
//! | Names & callers | [`Name`], [`Caller`], [`Access`] |
//! | Directory paging | [`Cookie`], [`DirCursor`], [`Entry`], [`DirPage`] |
//! | Reads & path walking | [`ReadResult`], [`resolve`] |
//! | Errors & bounds | [`Error`], [`ErrorKind`], [`Result`], [`Limits`] |
//! | In-memory backend | [`MemFs`] |
//! | Conformance kit | [`testing::check`], [`testing::check_cancellation`], [`testing::Report`], [`testing::Violation`] |
//!
//! # Limits and defaults
//!
//! All [`Limits`] bounds are inclusive:
//!
//! - [`Limits::max_read_len`]: `1_048_576` bytes (1 MiB, clamped on excess; `0` behaves as `1`).
//! - [`Limits::max_dir_entries`]: `4_096` entries per page (clamped on excess; `0` behaves as `1`).
//! - [`Limits::max_name_len`]: `255` bytes (error [`ErrorKind::NameTooLong`]).
//! - [`Limits::max_path_len`]: `4_096` bytes (error [`ErrorKind::NameTooLong`]).
//! - [`Limits::max_path_depth`]: `256` components (error [`ErrorKind::InvalidInput`]).
//! - [`Limits::max_symlink_len`]: `4_096` bytes (error [`ErrorKind::NameTooLong`]).
//! - [`Limits::max_groups`]: `16` supplementary groups (error [`ErrorKind::InvalidInput`]).
//!
//! [`MemFs`] enforces additional memory bounds: [`MemFs::DEFAULT_MAX_NODES`]
//! (`1_048_576`) and [`MemFs::DEFAULT_MAX_BYTES`] (`268_435_456`, 256 MiB).
//!
//! # Examples
//!
//! Creating an in-memory tree and reading through [`Filesystem`] and [`resolve`]:
//!
//! ```
//! use akku::task;
//! use akku::vfs::{self, Caller, Filesystem, Limits, MemFs};
//!
//! let fs = MemFs::new();
//! let root = fs.root();
//! let dir = fs.mkdir(root, b"docs")?;
//! fs.create_file(dir, b"hello.txt", b"hello vfs")?;
//!
//! let caller = Caller::new(1000, 1000);
//! let limits = Limits::new();
//! task::block_on(async {
//!     let attr = vfs::resolve(&fs, &caller, b"/docs/hello.txt", &limits).await?;
//!     let mut buf = [0u8; 32];
//!     let res = fs.read(&caller, attr.id, 0, &mut buf).await?;
//!     assert_eq!(&buf[..res.len], b"hello vfs");
//!     assert!(res.eof);
//!     vfs::Result::Ok(())
//! })?;
//! # Ok::<(), akku::vfs::Error>(())
//! ```

mod error;
mod fs;
mod memfs;
pub mod testing;
mod types;

pub use error::{Error, ErrorKind, Result};
pub use fs::{resolve, BoxFuture, DynFilesystem, Filesystem};
pub use memfs::MemFs;
pub use types::{
    Access, Attr, Caller, Cookie, DirCursor, DirPage, Entry, FsInfo, FsStat, Limits, Name, NodeId,
    NodeKind, ReadResult,
};
