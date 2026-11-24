//! Directory walking with depth, hidden-file, sorting, symlink and cycle
//! controls.
//!
//! [`Walk`] is a lazy, depth-first iterator over the entries below a root.
//! It is built with [`WalkBuilder`] or the short [`walk`] helper and yields
//! `Result<WalkEntry, WalkError>` items, so a mid-tree failure (an unreadable
//! directory, a vanished file) does not abort the whole walk.
//!
//! # Examples
//!
//! List a folder and its children:
//!
//! ```
//! use akku::fs::walk;
//!
//! let root = std::env::temp_dir().join("akku-walk-doctest");
//! std::fs::create_dir_all(&root).unwrap();
//! std::fs::write(root.join("a.txt"), b"").unwrap();
//! let names: Vec<String> = walk(&root)
//!     .filter_map(|entry| entry.ok())
//!     .map(|entry| entry.path.file_name().unwrap().to_string_lossy().into_owned())
//!     .collect();
//! assert!(names.contains(&"a.txt".into()));
//! let _ = std::fs::remove_dir_all(&root);
//! ```

use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A root or directory whose content still has to be scheduled.
#[derive(Debug)]
enum Frame {
    /// The root path. Its metadata decides between a single entry and a
    /// recursive scan.
    Root(PathBuf),
    /// A directory to read; its children land on the stack at `depth + 1`.
    Dir { path: PathBuf, depth: usize },
    /// A resolved entry, ready to yield to the caller.
    Entry(WalkEntry),
    /// An entry-level error deferred so sibling entries remain scheduled.
    Err(WalkError),
}

/// One directory entry produced by [`Walk`].
///
/// The `path` is the path within (possibly through) the root of the walk;
/// `depth` counts levels below the root, with the root itself at `0` and its
/// direct children at `1`.
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// The full path of the entry.
    pub path: PathBuf,
    /// Depth relative to the walk root: `0` for the root itself.
    pub depth: usize,
    /// File type of the entry; when symlinks are followed this is the
    /// target's type, otherwise the link's own type.
    pub file_type: fs::FileType,
}

impl WalkEntry {
    /// Whether this is a directory.
    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    /// Whether this is a regular file.
    pub fn is_file(&self) -> bool {
        self.file_type.is_file()
    }

    /// Whether this is a symbolic link.
    ///
    /// When the walk follows symlinks, the type is already resolved, so
    /// linked entries report their target type instead.
    pub fn is_symlink(&self) -> bool {
        self.file_type.is_symlink()
    }

    /// Metadata matching the type reported by this entry.
    ///
    /// Symbolic links that are not followed return the link metadata.
    ///
    /// # Errors
    ///
    /// Propagates any I/O error, for example if the entry vanished since the
    /// walk visited it.
    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        if self.file_type.is_symlink() {
            fs::symlink_metadata(&self.path)
        } else {
            fs::metadata(&self.path)
        }
    }
}

/// An error during a [`Walk`], keeping both the path and the I/O cause.
#[derive(Debug)]
pub struct WalkError {
    /// The path that failed.
    pub path: PathBuf,
    /// The underlying I/O error.
    pub io_error: io::Error,
}

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not walk {}: {}",
            self.path.display(),
            self.io_error
        )
    }
}

impl Error for WalkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.io_error)
    }
}

/// Builder for a [`Walk`] iterator.
///
/// Defaults: unlimited depth, symlinks are not followed, hidden entries
/// (names starting with a dot, except the root itself) are excluded, entries
/// come in directory-listing order, and errors are yielded as
/// [`WalkError`] items.
#[derive(Debug, Clone)]
pub struct WalkBuilder {
    root: PathBuf,
    max_depth: Option<usize>,
    follow_symlinks: bool,
    include_hidden: bool,
    sort_by_name: bool,
    skip_errors: bool,
}

impl WalkBuilder {
    /// Create a walker for `root`.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            max_depth: None,
            follow_symlinks: false,
            include_hidden: false,
            sort_by_name: false,
            skip_errors: false,
        }
    }

    /// Limit how deep the walk goes; the root has depth `0` and children
    /// start at `1`.
    pub fn max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = Some(max_depth);
        self
    }

    /// Resolve symlinked entries to their targets and descend into
    /// symlinked directories.
    ///
    /// A cycle guard remembers every visited directory by canonical path, so
    /// self-referential trees terminate instead of looping forever.
    pub fn follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    /// Include entries whose file names start with a dot.
    pub fn include_hidden(mut self, include: bool) -> Self {
        self.include_hidden = include;
        self
    }

    /// Sort the entries of each directory by file name so the walk order is
    /// deterministic.
    pub fn sort_by_name(mut self, sort: bool) -> Self {
        self.sort_by_name = sort;
        self
    }

    /// Silently skip failing entries instead of yielding [`WalkError`]
    /// items; a missing or unreachable branch simply ends.
    pub fn skip_errors(mut self, skip: bool) -> Self {
        self.skip_errors = skip;
        self
    }

    /// Build the iterator.
    ///
    /// The root is inspected lazily: a missing root yields a [`WalkError`]
    /// item on the first `next` call, or nothing at all with `skip_errors`.
    pub fn build(self) -> Walk {
        Walk {
            stack: vec![Frame::Root(self.root)],
            max_depth: self.max_depth,
            follow_symlinks: self.follow_symlinks,
            include_hidden: self.include_hidden,
            sort_by_name: self.sort_by_name,
            skip_errors: self.skip_errors,
            seen: if self.follow_symlinks {
                Some(HashSet::new())
            } else {
                None
            },
        }
    }
}

/// Depth-first directory iterator; see [`walk`] and [`WalkBuilder`].
#[derive(Debug)]
pub struct Walk {
    stack: Vec<Frame>,
    max_depth: Option<usize>,
    follow_symlinks: bool,
    include_hidden: bool,
    sort_by_name: bool,
    skip_errors: bool,
    seen: Option<HashSet<PathBuf>>,
}

/// Short-cut for `WalkBuilder::new(root).build()`.
pub fn walk(root: impl AsRef<Path>) -> Walk {
    WalkBuilder::new(root).build()
}

fn is_hidden(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| name.starts_with('.'))
}

fn file_name_of(path: &Path) -> std::ffi::OsString {
    path.file_name()
        .map_or_else(|| path.as_os_str().to_owned(), ToOwned::to_owned)
}

impl Iterator for Walk {
    type Item = Result<WalkEntry, WalkError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame = self.stack.pop()?;

            match frame {
                Frame::Entry(entry) => return Some(Ok(entry)),
                Frame::Err(err) => return Some(Err(err)),
                Frame::Root(path) => {
                    match fs::metadata(&path) {
                        Ok(meta) if meta.is_dir() => {
                            // The root is yielded first (depth 0), then its
                            // content is scheduled for the next calls.
                            self.stack.push(Frame::Dir {
                                path: path.clone(),
                                depth: 0,
                            });
                            return Some(Ok(WalkEntry {
                                path,
                                depth: 0,
                                file_type: meta.file_type(),
                            }));
                        }
                        Ok(meta) => {
                            return Some(Ok(WalkEntry {
                                path,
                                depth: 0,
                                file_type: meta.file_type(),
                            }));
                        }
                        Err(err) => {
                            if !self.skip_errors {
                                return Some(Err(WalkError {
                                    path,
                                    io_error: err,
                                }));
                            }
                        }
                    }
                }
                Frame::Dir { path, depth } => {
                    // Children of this directory would live at `depth + 1`;
                    // a directory beyond the limit contributes nothing.
                    if self.max_depth.is_some_and(|max| depth + 1 > max) {
                        continue;
                    }

                    if let Some(seen) = self.seen.as_mut() {
                        match fs::canonicalize(&path) {
                            Ok(canon) if seen.contains(&canon) => {
                                // Cycle: this directory was expanded already.
                                continue;
                            }
                            Ok(canon) => {
                                seen.insert(canon);
                            }
                            Err(err) => {
                                if !self.skip_errors {
                                    return Some(Err(WalkError {
                                        path,
                                        io_error: err,
                                    }));
                                }
                                continue;
                            }
                        }
                    }

                    let read = match fs::read_dir(&path) {
                        Ok(read) => read,
                        Err(err) => {
                            if !self.skip_errors {
                                return Some(Err(WalkError {
                                    path,
                                    io_error: err,
                                }));
                            }
                            continue;
                        }
                    };

                    enum ChildItem {
                        Entry(WalkEntry),
                        Err(WalkError),
                    }
                    impl ChildItem {
                        fn path(&self) -> &Path {
                            match self {
                                Self::Entry(entry) => &entry.path,
                                Self::Err(err) => &err.path,
                            }
                        }
                    }

                    let mut items = Vec::new();
                    for entry in read {
                        let entry = match entry {
                            Ok(entry) => entry,
                            Err(err) => {
                                if !self.skip_errors {
                                    items.push(ChildItem::Err(WalkError {
                                        path: path.clone(),
                                        io_error: err,
                                    }));
                                }
                                continue;
                            }
                        };
                        let name = entry.file_name();
                        if !self.include_hidden && is_hidden(&name) {
                            continue;
                        }
                        let child_depth = depth + 1;
                        if self.max_depth.is_some_and(|max| child_depth > max) {
                            continue;
                        }
                        let child_path = path.join(&name);
                        let file_type = if self.follow_symlinks {
                            match fs::metadata(&child_path) {
                                Ok(meta) => meta.file_type(),
                                Err(err) => {
                                    if !self.skip_errors {
                                        items.push(ChildItem::Err(WalkError {
                                            path: child_path,
                                            io_error: err,
                                        }));
                                    }
                                    continue;
                                }
                            }
                        } else {
                            match entry.file_type() {
                                Ok(kind) => kind,
                                Err(err) => {
                                    if !self.skip_errors {
                                        items.push(ChildItem::Err(WalkError {
                                            path: child_path,
                                            io_error: err,
                                        }));
                                    }
                                    continue;
                                }
                            }
                        };
                        items.push(ChildItem::Entry(WalkEntry {
                            path: child_path,
                            depth: child_depth,
                            file_type,
                        }));
                    }

                    if self.sort_by_name {
                        items.sort_by_key(|a| file_name_of(a.path()));
                    }

                    // Push in reverse so the stack pops the children in scan
                    // order; the directory itself is yielded before its own
                    // children.
                    for item in items.into_iter().rev() {
                        match item {
                            ChildItem::Entry(kid) => {
                                if kid.file_type.is_dir() {
                                    self.stack.push(Frame::Dir {
                                        path: kid.path.clone(),
                                        depth: kid.depth,
                                    });
                                }
                                self.stack.push(Frame::Entry(kid));
                            }
                            ChildItem::Err(err) => {
                                self.stack.push(Frame::Err(err));
                            }
                        }
                    }
                }
            }
        }
    }
}
