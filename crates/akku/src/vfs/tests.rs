//! Public-API adversarial tests for `akku::vfs` (G3 test plan items 1–67 and
//! conformance kit rules K01–K22, KC1–KC2).

use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::num::NonZeroU64;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akku::task::{block_on, spawn, yield_now};
use akku::vfs::{
    self, testing, Access, Attr, Caller, Cookie, DirCursor, DirPage, DynFilesystem, Entry, Error,
    ErrorKind, Filesystem, FsInfo, FsStat, Limits, MemFs, Name, NodeId, NodeKind, ReadResult,
    Result,
};

// --- Shared test fixtures and helpers --------------------------------------

fn b_bin_pattern() -> Vec<u8> {
    (0..1000usize)
        .map(|i| ((i.wrapping_mul(37) ^ (i >> 3)) & 0xff) as u8)
        .collect()
}

/// Standard test tree:
/// - `/a.txt` (`b"hello"`)
/// - `/dir/`
/// - `/dir/b.bin` (1000 bytes of known pattern)
/// - `/link` -> `b"a.txt"`
fn fixture() -> MemFs {
    let fs = MemFs::new();
    let root = fs.root();
    fs.create_file(root, b"a.txt", b"hello").unwrap();
    let dir = fs.mkdir(root, b"dir").unwrap();
    fs.create_file(dir, b"b.bin", &b_bin_pattern()).unwrap();
    fs.symlink(root, b"link", b"a.txt").unwrap();
    fs
}

fn name(bytes: &[u8]) -> Name<'_> {
    Name::new(bytes, &Limits::new()).unwrap()
}

struct CountingFs<F> {
    inner: F,
    getattr_calls: AtomicUsize,
    lookup_calls: AtomicUsize,
    parent_calls: AtomicUsize,
    read_calls: AtomicUsize,
    read_dir_calls: AtomicUsize,
    access_calls: AtomicUsize,
    read_link_calls: AtomicUsize,
    statfs_calls: AtomicUsize,
}

impl<F> CountingFs<F> {
    fn new(inner: F) -> Self {
        Self {
            inner,
            getattr_calls: AtomicUsize::new(0),
            lookup_calls: AtomicUsize::new(0),
            parent_calls: AtomicUsize::new(0),
            read_calls: AtomicUsize::new(0),
            read_dir_calls: AtomicUsize::new(0),
            access_calls: AtomicUsize::new(0),
            read_link_calls: AtomicUsize::new(0),
            statfs_calls: AtomicUsize::new(0),
        }
    }

    fn total_calls(&self) -> usize {
        self.getattr_calls.load(Ordering::Relaxed)
            + self.lookup_calls.load(Ordering::Relaxed)
            + self.parent_calls.load(Ordering::Relaxed)
            + self.read_calls.load(Ordering::Relaxed)
            + self.read_dir_calls.load(Ordering::Relaxed)
            + self.access_calls.load(Ordering::Relaxed)
            + self.read_link_calls.load(Ordering::Relaxed)
            + self.statfs_calls.load(Ordering::Relaxed)
    }

    fn reset(&self) {
        self.getattr_calls.store(0, Ordering::Relaxed);
        self.lookup_calls.store(0, Ordering::Relaxed);
        self.parent_calls.store(0, Ordering::Relaxed);
        self.read_calls.store(0, Ordering::Relaxed);
        self.read_dir_calls.store(0, Ordering::Relaxed);
        self.access_calls.store(0, Ordering::Relaxed);
        self.read_link_calls.store(0, Ordering::Relaxed);
        self.statfs_calls.store(0, Ordering::Relaxed);
    }
}

impl<F: Filesystem> Filesystem for CountingFs<F> {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }

    fn root(&self) -> NodeId {
        self.inner.root()
    }

    async fn getattr(&self, caller: &Caller, id: NodeId) -> Result<Attr> {
        self.getattr_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.getattr(caller, id).await
    }

    async fn lookup(&self, caller: &Caller, dir: NodeId, name: Name<'_>) -> Result<Attr> {
        self.lookup_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.lookup(caller, dir, name).await
    }

    async fn parent(&self, caller: &Caller, dir: NodeId) -> Result<Attr> {
        self.parent_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.parent(caller, dir).await
    }

    async fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<ReadResult> {
        self.read_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.read(caller, id, offset, buf).await
    }

    async fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> Result<DirPage> {
        self.read_dir_calls.fetch_add(1, Ordering::Relaxed);
        self.inner
            .read_dir(caller, dir, cursor, max_entries, with_attrs)
            .await
    }

    async fn access(&self, caller: &Caller, id: NodeId, want: Access) -> Result<Access> {
        self.access_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.access(caller, id, want).await
    }

    async fn read_link(&self, caller: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.read_link_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.read_link(caller, id).await
    }

    async fn statfs(&self, caller: &Caller, id: NodeId) -> Result<FsStat> {
        self.statfs_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.statfs(caller, id).await
    }
}

/// Awaits `akku::task::yield_now()` once on every async operation before
/// delegating, so cancellation drops happen at a genuine `Poll::Pending` point.
struct PendingOnce<F> {
    inner: F,
}

impl<F> PendingOnce<F> {
    fn new(inner: F) -> Self {
        Self { inner }
    }
}

impl<F: Filesystem> Filesystem for PendingOnce<F> {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }

    fn root(&self) -> NodeId {
        self.inner.root()
    }

    async fn getattr(&self, caller: &Caller, id: NodeId) -> Result<Attr> {
        yield_now().await;
        self.inner.getattr(caller, id).await
    }

    async fn lookup(&self, caller: &Caller, dir: NodeId, name: Name<'_>) -> Result<Attr> {
        yield_now().await;
        self.inner.lookup(caller, dir, name).await
    }

    async fn parent(&self, caller: &Caller, dir: NodeId) -> Result<Attr> {
        yield_now().await;
        self.inner.parent(caller, dir).await
    }

    async fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<ReadResult> {
        yield_now().await;
        self.inner.read(caller, id, offset, buf).await
    }

    async fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> Result<DirPage> {
        yield_now().await;
        self.inner
            .read_dir(caller, dir, cursor, max_entries, with_attrs)
            .await
    }

    async fn access(&self, caller: &Caller, id: NodeId, want: Access) -> Result<Access> {
        yield_now().await;
        self.inner.access(caller, id, want).await
    }

    async fn read_link(&self, caller: &Caller, id: NodeId) -> Result<Vec<u8>> {
        yield_now().await;
        self.inner.read_link(caller, id).await
    }

    async fn statfs(&self, caller: &Caller, id: NodeId) -> Result<FsStat> {
        yield_now().await;
        self.inner.statfs(caller, id).await
    }
}

/// Overrides `access` to return `Access::empty()` and `statfs` to fixed values.
struct Overriding<F> {
    inner: F,
}

impl<F> Overriding<F> {
    fn new(inner: F) -> Self {
        Self { inner }
    }

    fn fixed_stat() -> FsStat {
        let mut st = FsStat::unknown();
        st.total_bytes = 123_456;
        st.free_bytes = 65_432;
        st.avail_bytes = 60_000;
        st.total_files = 999;
        st.free_files = 888;
        st.avail_files = 777;
        st
    }
}

impl<F: Filesystem> Filesystem for Overriding<F> {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }

    fn root(&self) -> NodeId {
        self.inner.root()
    }

    async fn getattr(&self, caller: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(caller, id).await
    }

    async fn lookup(&self, caller: &Caller, dir: NodeId, name: Name<'_>) -> Result<Attr> {
        self.inner.lookup(caller, dir, name).await
    }

    async fn parent(&self, caller: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(caller, dir).await
    }

    async fn read(
        &self,
        caller: &Caller,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<ReadResult> {
        self.inner.read(caller, id, offset, buf).await
    }

    async fn read_dir(
        &self,
        caller: &Caller,
        dir: NodeId,
        cursor: DirCursor,
        max_entries: usize,
        with_attrs: bool,
    ) -> Result<DirPage> {
        self.inner
            .read_dir(caller, dir, cursor, max_entries, with_attrs)
            .await
    }

    async fn access(&self, _caller: &Caller, _id: NodeId, _want: Access) -> Result<Access> {
        Ok(Access::empty())
    }

    async fn read_link(&self, caller: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(caller, id).await
    }

    async fn statfs(&self, _caller: &Caller, _id: NodeId) -> Result<FsStat> {
        Ok(Self::fixed_stat())
    }
}

// --- Types and static assertions (1–9) -------------------------------------

#[test]
fn t01_send_sync_and_error_bounds() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn assert_std_error<T: std::error::Error + 'static>() {}

    assert_send_sync::<MemFs>();
    assert_send_sync::<Error>();
    assert_send_sync::<Caller>();
    assert_send_sync::<Limits>();
    assert_send_sync::<Attr>();
    assert_send_sync::<DirPage>();
    assert_send_sync::<Entry>();
    assert_send_sync::<testing::Report>();
    assert_send_sync::<dyn DynFilesystem>();
    assert_std_error::<Error>();
}

async fn spawnable<F: Filesystem + ?Sized + 'static>(fs: Arc<F>) {
    let caller = Caller::new(0, 0);
    let limits = Limits::new();
    let info = fs.info();
    assert!(info.max_read >= 1);
    let root = fs.root();
    let root_attr = fs.getattr(&caller, root).await.unwrap();
    assert_eq!(root_attr.id, root);
    let parent_attr = fs.parent(&caller, root).await.unwrap();
    assert_eq!(parent_attr.id, root);
    let a_attr = fs
        .lookup(&caller, root, Name::new(b"a.txt", &limits).unwrap())
        .await
        .unwrap();
    let mut buf = [0u8; 16];
    let rd = fs.read(&caller, a_attr.id, 0, &mut buf).await.unwrap();
    assert_eq!(&buf[..rd.len], b"hello");
    assert!(rd.eof);
    let page = fs
        .read_dir(&caller, root, DirCursor::START, 16, false)
        .await
        .unwrap();
    assert!(!page.entries.is_empty());
    let acc = fs.access(&caller, root, Access::READ).await.unwrap();
    assert!(acc.contains(Access::READ));
    let link_attr = fs
        .lookup(&caller, root, Name::new(b"link", &limits).unwrap())
        .await
        .unwrap();
    let target = fs.read_link(&caller, link_attr.id).await.unwrap();
    assert_eq!(target, b"a.txt");
    let _stat = fs.statfs(&caller, root).await.unwrap();
}

#[test]
fn t02_spawnable_generic_arc_filesystem() {
    fn assert_send_static<Fut: Future<Output = ()> + Send + 'static>(f: Fut) -> Fut {
        f
    }

    let f1: Arc<MemFs> = Arc::new(fixture());
    let f2: Arc<dyn DynFilesystem> = Arc::new(fixture());
    let f3: Arc<Arc<dyn DynFilesystem>> = Arc::new(Arc::new(fixture()) as Arc<dyn DynFilesystem>);
    let f4: Arc<Box<dyn DynFilesystem>> = Arc::new(Box::new(fixture()) as Box<dyn DynFilesystem>);

    block_on(async {
        spawn(assert_send_static(spawnable(f1))).await.unwrap();
        spawn(assert_send_static(spawnable(f2))).await.unwrap();
        spawn(assert_send_static(spawnable(f3))).await.unwrap();
        spawn(assert_send_static(spawnable(f4))).await.unwrap();
    });
}

#[test]
fn t03_resolve_and_filesystem_futures_are_send() {
    fn assert_send<T: Send>(v: T) -> T {
        v
    }

    let fs = fixture();
    let caller = Caller::new(0, 0);
    let limits = Limits::new();
    let root = fs.root();
    let dyn_fs: &dyn DynFilesystem = &fs;

    block_on(async {
        let resolved = assert_send(vfs::resolve(dyn_fs, &caller, b"/a.txt", &limits))
            .await
            .unwrap();
        assert_eq!(resolved.kind, NodeKind::File);

        let _ = assert_send(fs.getattr(&caller, root)).await.unwrap();
        let _ = assert_send(fs.lookup(&caller, root, name(b"a.txt")))
            .await
            .unwrap();
        let _ = assert_send(fs.parent(&caller, root)).await.unwrap();
        let mut buf = [0u8; 8];
        let _ = assert_send(fs.read(&caller, resolved.id, 0, &mut buf))
            .await
            .unwrap();
        let _ = assert_send(fs.read_dir(&caller, root, DirCursor::START, 8, false))
            .await
            .unwrap();
        let _ = assert_send(fs.access(&caller, root, Access::READ))
            .await
            .unwrap();
        let link = fs.lookup(&caller, root, name(b"link")).await.unwrap();
        let _ = assert_send(fs.read_link(&caller, link.id)).await.unwrap();
        let _ = assert_send(fs.statfs(&caller, root)).await.unwrap();
    });
}

#[test]
fn t04_both_traits_in_scope_no_e0034() {
    use akku::vfs::{DynFilesystem as _, Filesystem as _};

    let fs = fixture();
    let caller = Caller::new(0, 0);
    let info = fs.info();
    let root = fs.root();
    let info_dyn = fs.info_dyn();
    assert_eq!(info, info_dyn);
    let attr = block_on(fs.getattr(&caller, root)).unwrap();
    assert_eq!(attr.id, root);
}

#[test]
fn t05_node_id_invariants() {
    assert_eq!(NodeId::new(0), None);
    let one = NodeId::new(1).expect("1 is non-zero");
    assert_eq!(one.get(), 1);
    assert_eq!(u64::from(one), 1);

    let max = NodeId::new(u64::MAX).expect("u64::MAX is non-zero");
    assert_eq!(max.get(), u64::MAX);
    assert_eq!(u64::from(max), u64::MAX);

    let nz = NonZeroU64::new(42).unwrap();
    let from_nz = NodeId::from_nonzero(nz);
    assert_eq!(from_nz, NodeId::from(nz));
    assert_eq!(from_nz, NodeId::new(42).unwrap());
    assert_eq!(u64::from(from_nz), 42);
}

#[test]
fn t06_cookie_and_dir_cursor_constants() {
    assert_eq!(Cookie::START.get(), 0);
    assert_eq!(Cookie::DOT.get(), 1);
    assert_eq!(Cookie::DOT_DOT.get(), 2);
    assert_eq!(Cookie::FIRST.get(), 3);

    assert!(Cookie::START.is_start());
    assert!(Cookie::DOT.is_start());
    assert!(Cookie::DOT_DOT.is_start());
    assert!(Cookie::new(0).is_start());
    assert!(Cookie::new(1).is_start());
    assert!(Cookie::new(2).is_start());
    assert!(!Cookie::FIRST.is_start());
    assert!(!Cookie::new(3).is_start());
    assert!(!Cookie::new(4).is_start());
    assert!(!Cookie::new(u64::MAX).is_start());

    assert_eq!(Cookie::new(9876).get(), 9876);
    assert_eq!(DirCursor::START, DirCursor::default());
    assert_eq!(DirCursor::START.cookie, Cookie::START);
    assert_eq!(DirCursor::START.verifier, 0);

    let c = DirCursor::new(Cookie::FIRST, 0x1234);
    assert_eq!(c.cookie, Cookie::FIRST);
    assert_eq!(c.verifier, 0x1234);
}

#[test]
fn t07_access_constants_and_bitops() {
    assert_eq!(Access::READ.bits(), 0x01);
    assert_eq!(Access::LOOKUP.bits(), 0x02);
    assert_eq!(Access::MODIFY.bits(), 0x04);
    assert_eq!(Access::EXTEND.bits(), 0x08);
    assert_eq!(Access::DELETE.bits(), 0x10);
    assert_eq!(Access::EXECUTE.bits(), 0x20);

    assert!(Access::empty().is_empty());
    assert_eq!(Access::empty().bits(), 0);
    assert!(!Access::READ.is_empty());

    assert_eq!(Access::all().bits(), 0x3f);
    assert_eq!(Access::from_bits_truncate(u32::MAX), Access::all());
    assert_eq!(Access::from_bits_truncate(0x01 | 0x8000_0000), Access::READ);

    let rw = Access::READ | Access::MODIFY;
    assert_eq!(rw.bits(), 0x05);
    assert!(rw.contains(Access::READ));
    assert!(rw.contains(Access::MODIFY));
    assert!(rw.contains(rw));
    assert!(!rw.contains(Access::EXECUTE));
    assert!(!rw.contains(Access::READ | Access::EXECUTE));

    assert_eq!(rw & Access::EXECUTE, Access::empty());
    assert_eq!(rw & (Access::MODIFY | Access::EXTEND), Access::MODIFY);
    assert_eq!(rw.union(Access::EXECUTE), rw | Access::EXECUTE);
    assert_eq!(rw.intersection(Access::READ | Access::DELETE), Access::READ);

    let mut acc = Access::READ;
    acc |= Access::LOOKUP;
    assert_eq!(acc, Access::READ | Access::LOOKUP);
}

#[test]
fn t08_limits_defaults_and_builders() {
    let d = Limits::new();
    assert_eq!(d, Limits::default());
    assert_eq!(d.max_read_len, 1_048_576);
    assert_eq!(d.max_dir_entries, 4_096);
    assert_eq!(d.max_name_len, 255);
    assert_eq!(d.max_path_len, 4_096);
    assert_eq!(d.max_path_depth, 256);
    assert_eq!(d.max_symlink_len, 4_096);
    assert_eq!(d.max_groups, 16);

    let l = d.with_max_read_len(11);
    assert_eq!(l.max_read_len, 11);
    assert_eq!(l.max_dir_entries, d.max_dir_entries);
    assert_eq!(l.max_name_len, d.max_name_len);
    assert_eq!(l.max_path_len, d.max_path_len);
    assert_eq!(l.max_path_depth, d.max_path_depth);
    assert_eq!(l.max_symlink_len, d.max_symlink_len);
    assert_eq!(l.max_groups, d.max_groups);

    let l = d.with_max_dir_entries(12);
    assert_eq!(l.max_dir_entries, 12);
    assert_eq!(l.max_read_len, d.max_read_len);

    let l = d.with_max_name_len(13);
    assert_eq!(l.max_name_len, 13);
    assert_eq!(l.max_path_len, d.max_path_len);

    let l = d.with_max_path_len(14);
    assert_eq!(l.max_path_len, 14);
    assert_eq!(l.max_path_depth, d.max_path_depth);

    let l = d.with_max_path_depth(15);
    assert_eq!(l.max_path_depth, 15);
    assert_eq!(l.max_symlink_len, d.max_symlink_len);

    let l = d.with_max_symlink_len(17);
    assert_eq!(l.max_symlink_len, 17);
    assert_eq!(l.max_groups, d.max_groups);

    let l = d.with_max_groups(18);
    assert_eq!(l.max_groups, 18);
    assert_eq!(l.max_read_len, d.max_read_len);
}

#[test]
fn t09_attr_fsinfo_fsstat_entry_readresult_defaults() {
    let id = NodeId::new(7).unwrap();

    let file = Attr::new(id, NodeKind::File);
    assert_eq!(file.id, id);
    assert_eq!(file.generation, 0);
    assert_eq!(file.kind, NodeKind::File);
    assert_eq!(file.perm, 0o644);
    assert_eq!(file.nlink, 1);
    assert_eq!(file.uid, 0);
    assert_eq!(file.gid, 0);
    assert_eq!(file.size, 0);
    assert_eq!(file.used, 0);
    assert_eq!(file.atime, UNIX_EPOCH);
    assert_eq!(file.mtime, UNIX_EPOCH);
    assert_eq!(file.ctime, UNIX_EPOCH);
    assert_eq!(file.change, 0);

    let dir = Attr::new(id, NodeKind::Dir);
    assert_eq!(dir.perm, 0o755);
    assert_eq!(dir.nlink, 2);

    let sym = Attr::new(id, NodeKind::Symlink);
    assert_eq!(sym.perm, 0o777);
    assert_eq!(sym.nlink, 1);

    let info = FsInfo::new(42, 999);
    assert_eq!(info.fsid, 42);
    assert_eq!(info.id_epoch, 999);
    assert!(info.read_only);
    assert_eq!(info.max_read, 1_048_576);
    assert_eq!(info.max_name_len, 255);
    assert_eq!(info.max_file_size, i64::MAX as u64);
    assert_eq!(info.time_granularity, Duration::from_nanos(1));
    assert!(!info.case_insensitive);
    assert!(info.case_preserving);
    assert!(info.symlinks);
    assert!(!info.hard_links);

    let st = FsStat::unknown();
    assert_eq!(st, FsStat::default());
    assert_eq!(st.total_bytes, 0);
    assert_eq!(st.free_bytes, 0);
    assert_eq!(st.avail_bytes, 0);
    assert_eq!(st.total_files, 0);
    assert_eq!(st.free_files, 0);
    assert_eq!(st.avail_files, 0);

    let entry = Entry::new(name(b"foo"), id, NodeKind::File, Cookie::FIRST);
    assert_eq!(entry.name, b"foo");
    assert_eq!(entry.id, id);
    assert_eq!(entry.kind, NodeKind::File);
    assert_eq!(entry.cookie, Cookie::FIRST);
    assert!(entry.attr.is_none());

    let entry_with_attr = entry.with_attr(file.clone());
    assert_eq!(entry_with_attr.attr, Some(file));

    let page = DirPage::new(vec![entry_with_attr], true, 55);
    assert_eq!(page.entries.len(), 1);
    assert!(page.eof);
    assert_eq!(page.verifier, 55);

    let rr = ReadResult::new(12, false);
    assert_eq!(rr.len, 12);
    assert!(!rr.eof);
}

// --- Name validation (10–13) -----------------------------------------------

#[test]
fn t10_name_rejects_invalid_bytes() {
    let limits = Limits::new();
    let bad: &[&[u8]] = &[b"", b".", b"..", b"/", b"a/b", b"/a", b"a/", b"\0", b"a\0b"];
    for &case in bad {
        let err = Name::new(case, &limits).unwrap_err();
        assert_eq!(
            err.kind(),
            ErrorKind::InvalidName,
            "expected InvalidName for {case:?}"
        );
    }
}

#[test]
fn t11_name_accepts_tricky_valid_bytes() {
    let limits = Limits::new();
    for &valid in &[b"..." as &[u8], b".a", b"a.", b" ", b"a\\b", b"\x01\x7f"] {
        let n = Name::new(valid, &limits).unwrap();
        assert_eq!(n.as_bytes(), valid);
    }

    let non_utf8 = Name::new(&[0xff, 0xfe], &limits).unwrap();
    assert_eq!(non_utf8.to_str(), None);

    let utf8 = Name::new("ü€𝄞".as_bytes(), &limits).unwrap();
    assert_eq!(utf8.to_str(), Some("ü€𝄞"));
}

#[test]
fn t12_name_length_bounds_and_check_order() {
    let limits = Limits::new();
    let ok255 = vec![b'a'; 255];
    assert_eq!(Name::new(&ok255, &limits).unwrap().as_bytes().len(), 255);

    let too_long = vec![b'a'; 256];
    assert_eq!(
        Name::new(&too_long, &limits).unwrap_err().kind(),
        ErrorKind::NameTooLong
    );

    // Length checked BEFORE content (`/` and NUL).
    let mut long_with_slash = vec![b'a'; 256];
    long_with_slash[10] = b'/';
    assert_eq!(
        Name::new(&long_with_slash, &limits).unwrap_err().kind(),
        ErrorKind::NameTooLong
    );
    let mut long_with_nul = vec![b'a'; 256];
    long_with_nul[0] = 0;
    assert_eq!(
        Name::new(&long_with_nul, &limits).unwrap_err().kind(),
        ErrorKind::NameTooLong
    );

    // Empty checked BEFORE length.
    let zero_lim = Limits::new().with_max_name_len(0);
    assert_eq!(
        Name::new(b"", &zero_lim).unwrap_err().kind(),
        ErrorKind::InvalidName
    );
    assert_eq!(
        Name::new(b"a", &zero_lim).unwrap_err().kind(),
        ErrorKind::NameTooLong
    );

    let huge_lim = Limits::new().with_max_name_len(usize::MAX);
    let big = vec![b'x'; 65_536];
    assert_eq!(Name::new(&big, &huge_lim).unwrap().as_bytes().len(), 65_536);
}

#[test]
fn t13_name_as_bytes_as_ref_and_debug() {
    let limits = Limits::new();
    let raw = &[0xff, 0x01, b'a'];
    let n = Name::new(raw, &limits).unwrap();
    assert_eq!(n.as_bytes(), raw);
    assert_eq!(n.as_ref(), raw);

    let dbg = format!("{n:?}");
    assert!(!dbg.is_empty());
    assert!(dbg.contains('a'), "debug output missing ascii char: {dbg}");
    assert!(
        dbg.contains("ff") || dbg.contains("FF"),
        "debug output should escape non-UTF-8 byte 0xff: {dbg}"
    );
}

// --- Caller and access (14–16) ---------------------------------------------

#[test]
fn t14_caller_and_groups_bounds() {
    let limits = Limits::new();
    let c = Caller::new(5, 7);
    assert_eq!(c.uid(), 5);
    assert_eq!(c.gid(), 7);
    assert!(c.groups().is_empty());

    let g16: Vec<u32> = (1..=16).collect();
    let c16 = c.clone().with_groups(&g16, &limits).unwrap();
    assert_eq!(c16.groups(), &g16[..]);

    let g17: Vec<u32> = (1..=17).collect();
    let err = c.clone().with_groups(&g17, &limits).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);

    let zero_lim = Limits::new().with_max_groups(0);
    let ok0 = c.clone().with_groups(&[], &zero_lim).unwrap();
    assert!(ok0.groups().is_empty());
    assert_eq!(
        c.clone().with_groups(&[1], &zero_lim).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );

    // Second with_groups replaces the first.
    let replaced = c16.with_groups(&[99, 100], &limits).unwrap();
    assert_eq!(replaced.groups(), &[99, 100]);
}

#[test]
fn t15_access_from_mode_table() {
    let limits = Limits::new();
    let id = NodeId::new(10).unwrap();

    let mut dir_attr = Attr::new(id, NodeKind::Dir);
    dir_attr.uid = 1000;
    dir_attr.gid = 2000;
    dir_attr.perm = 0o750;

    let mut file_attr = Attr::new(id, NodeKind::File);
    file_attr.uid = 1000;
    file_attr.gid = 2000;
    file_attr.perm = 0o750;

    let owner = Caller::new(1000, 9999);
    let primary_grp = Caller::new(1001, 2000);
    let supp_grp = Caller::new(1001, 9999)
        .with_groups(&[10, 2000], &limits)
        .unwrap();
    let other = Caller::new(1001, 9999).with_groups(&[10], &limits).unwrap();
    let root = Caller::new(0, 0);

    // Owner (rwx = 7)
    assert_eq!(
        Access::from_mode(&owner, &dir_attr, false),
        Access::READ | Access::LOOKUP | Access::MODIFY | Access::EXTEND | Access::DELETE
    );
    assert_eq!(
        Access::from_mode(&owner, &dir_attr, true),
        Access::READ | Access::LOOKUP
    );
    assert_eq!(
        Access::from_mode(&owner, &file_attr, false),
        Access::READ | Access::MODIFY | Access::EXTEND | Access::EXECUTE
    );
    assert_eq!(
        Access::from_mode(&owner, &file_attr, true),
        Access::READ | Access::EXECUTE
    );

    // Primary group (r-x = 5)
    assert_eq!(
        Access::from_mode(&primary_grp, &dir_attr, false),
        Access::READ | Access::LOOKUP
    );
    assert_eq!(
        Access::from_mode(&primary_grp, &file_attr, false),
        Access::READ | Access::EXECUTE
    );

    // Supplementary group (r-x = 5)
    assert_eq!(
        Access::from_mode(&supp_grp, &dir_attr, false),
        Access::READ | Access::LOOKUP
    );
    assert_eq!(
        Access::from_mode(&supp_grp, &file_attr, true),
        Access::READ | Access::EXECUTE
    );

    // Other (--- = 0)
    assert_eq!(Access::from_mode(&other, &dir_attr, false), Access::empty());
    assert_eq!(
        Access::from_mode(&other, &file_attr, false),
        Access::empty()
    );

    // Root rules
    let mut file_640 = file_attr.clone();
    file_640.perm = 0o640;
    assert_eq!(
        Access::from_mode(&root, &file_640, false),
        Access::READ | Access::MODIFY | Access::EXTEND
    );
    assert_eq!(Access::from_mode(&root, &file_640, true), Access::READ);

    let mut file_001 = file_attr.clone();
    file_001.perm = 0o001;
    assert_eq!(
        Access::from_mode(&root, &file_001, false),
        Access::READ | Access::MODIFY | Access::EXTEND | Access::EXECUTE
    );

    let mut dir_000 = dir_attr.clone();
    dir_000.perm = 0o000;
    assert_eq!(
        Access::from_mode(&root, &dir_000, false),
        Access::READ | Access::LOOKUP | Access::MODIFY | Access::EXTEND | Access::DELETE
    );
    assert_eq!(
        Access::from_mode(&root, &dir_000, true),
        Access::READ | Access::LOOKUP
    );

    // Owner with 0o077 does NOT fall through to group/other.
    let mut no_owner = file_attr.clone();
    no_owner.perm = 0o077;
    assert_eq!(Access::from_mode(&owner, &no_owner, false), Access::empty());
}

#[test]
fn t16_default_memfs_access() {
    let fs = fixture();
    let root = fs.root();
    let a = block_on(fs.lookup(&Caller::new(0, 0), root, name(b"a.txt"))).unwrap();
    fs.set_owner(a.id, 1000, 2000).unwrap();
    fs.set_perm(a.id, 0o640).unwrap();

    let owner = Caller::new(1000, 9999);
    let other = Caller::new(1001, 9999);

    block_on(async {
        let got_all = fs.access(&owner, a.id, Access::all()).await.unwrap();
        // MemFs is read_only, so MODIFY/EXTEND/DELETE are stripped.
        assert_eq!(got_all, Access::READ);

        let got_empty = fs.access(&owner, a.id, Access::empty()).await.unwrap();
        assert_eq!(got_empty, Access::empty());

        let got_mod = fs.access(&owner, a.id, Access::MODIFY).await.unwrap();
        assert_eq!(got_mod, Access::empty());

        let got_other = fs.access(&other, a.id, Access::all()).await.unwrap();
        assert_eq!(got_other, Access::empty());

        fs.remove(root, b"a.txt").unwrap();
        let stale = fs.access(&owner, a.id, Access::READ).await.unwrap_err();
        assert_eq!(stale.kind(), ErrorKind::Stale);
    });
}

// --- Errors (17–20) --------------------------------------------------------

const ALL_ERROR_KINDS: [ErrorKind; 21] = [
    ErrorKind::NotFound,
    ErrorKind::NotPermitted,
    ErrorKind::PermissionDenied,
    ErrorKind::AlreadyExists,
    ErrorKind::NotADirectory,
    ErrorKind::IsADirectory,
    ErrorKind::DirectoryNotEmpty,
    ErrorKind::InvalidInput,
    ErrorKind::InvalidName,
    ErrorKind::NameTooLong,
    ErrorKind::ReadOnly,
    ErrorKind::NoSpace,
    ErrorKind::QuotaExceeded,
    ErrorKind::FileTooLarge,
    ErrorKind::CrossesDevices,
    ErrorKind::TooManyLinks,
    ErrorKind::Stale,
    ErrorKind::BadCookie,
    ErrorKind::Unsupported,
    ErrorKind::TryAgain,
    ErrorKind::Io,
];

#[test]
fn t17_error_display_and_source() {
    let e_empty = Error::new(ErrorKind::Stale, "");
    assert_eq!(e_empty.kind(), ErrorKind::Stale);
    assert_eq!(e_empty.message(), "");
    assert_eq!(e_empty.to_string(), ErrorKind::Stale.as_str());
    assert!(!ErrorKind::Stale.as_str().is_empty());
    assert_eq!(ErrorKind::Stale.to_string(), ErrorKind::Stale.as_str());

    let e_from = Error::from(ErrorKind::NotFound);
    assert_eq!(e_from.kind(), ErrorKind::NotFound);
    assert_eq!(e_from.message(), "");
    assert_eq!(e_from.to_string(), ErrorKind::NotFound.as_str());

    let e_msg = Error::new(ErrorKind::Stale, "gone");
    assert_eq!(e_msg.message(), "gone");
    assert_eq!(
        e_msg.to_string(),
        format!("{}: gone", ErrorKind::Stale.as_str())
    );

    let src = io::Error::other("inner-cause");
    let e_src = Error::new(ErrorKind::Io, "outer").with_source(src);
    let attached = std::error::Error::source(&e_src).expect("source attached");
    assert!(attached.to_string().contains("inner-cause"));
}

#[test]
fn t18_error_vfs_to_io_exhaustive() {
    for kind in ALL_ERROR_KINDS {
        let expected = match kind {
            ErrorKind::NotFound => io::ErrorKind::NotFound,
            ErrorKind::NotPermitted => io::ErrorKind::PermissionDenied,
            ErrorKind::PermissionDenied => io::ErrorKind::PermissionDenied,
            ErrorKind::AlreadyExists => io::ErrorKind::AlreadyExists,
            ErrorKind::NotADirectory => io::ErrorKind::NotADirectory,
            ErrorKind::IsADirectory => io::ErrorKind::IsADirectory,
            ErrorKind::DirectoryNotEmpty => io::ErrorKind::DirectoryNotEmpty,
            ErrorKind::InvalidInput => io::ErrorKind::InvalidInput,
            ErrorKind::InvalidName => io::ErrorKind::InvalidInput,
            ErrorKind::NameTooLong => io::ErrorKind::InvalidInput,
            ErrorKind::ReadOnly => io::ErrorKind::ReadOnlyFilesystem,
            ErrorKind::NoSpace => io::ErrorKind::StorageFull,
            ErrorKind::QuotaExceeded => io::ErrorKind::QuotaExceeded,
            ErrorKind::FileTooLarge => io::ErrorKind::FileTooLarge,
            ErrorKind::CrossesDevices => io::ErrorKind::CrossesDevices,
            ErrorKind::TooManyLinks => io::ErrorKind::TooManyLinks,
            ErrorKind::Stale => io::ErrorKind::StaleNetworkFileHandle,
            ErrorKind::BadCookie => io::ErrorKind::InvalidInput,
            ErrorKind::Unsupported => io::ErrorKind::Unsupported,
            ErrorKind::TryAgain => io::ErrorKind::ResourceBusy,
            ErrorKind::Io => io::ErrorKind::Other,
            other => panic!("unhandled ErrorKind in test table: {other:?}"),
        };
        let io_err: io::Error = Error::from(kind).into();
        assert_eq!(io_err.kind(), expected, "mapping mismatch for {kind:?}");
    }
}

#[test]
fn t19_error_vfs_io_vfs_round_trip() {
    for kind in ALL_ERROR_KINDS {
        let msg = format!("detail for {kind:?}");
        let orig = Error::new(kind, msg.clone()).with_source(io::Error::other("root-cause"));
        let io_err: io::Error = orig.into();
        let back: Error = io_err.into();
        assert_eq!(back.kind(), kind, "round-trip lost kind for {kind:?}");
        assert_eq!(back.message(), msg, "round-trip lost message for {kind:?}");
        let src = std::error::Error::source(&back).expect("round-trip lost source");
        assert!(src.to_string().contains("root-cause"));
    }
}

#[test]
fn t20_error_plain_io_to_vfs() {
    let cases = [
        (io::ErrorKind::TimedOut, ErrorKind::TryAgain),
        (io::ErrorKind::ResourceBusy, ErrorKind::TryAgain),
        (io::ErrorKind::WouldBlock, ErrorKind::TryAgain),
        (io::ErrorKind::Interrupted, ErrorKind::TryAgain),
        (io::ErrorKind::StaleNetworkFileHandle, ErrorKind::Stale),
        (io::ErrorKind::NotFound, ErrorKind::NotFound),
        (io::ErrorKind::PermissionDenied, ErrorKind::PermissionDenied),
        (io::ErrorKind::AlreadyExists, ErrorKind::AlreadyExists),
        (io::ErrorKind::NotADirectory, ErrorKind::NotADirectory),
        (io::ErrorKind::IsADirectory, ErrorKind::IsADirectory),
        (
            io::ErrorKind::DirectoryNotEmpty,
            ErrorKind::DirectoryNotEmpty,
        ),
        (io::ErrorKind::InvalidInput, ErrorKind::InvalidInput),
        (io::ErrorKind::ReadOnlyFilesystem, ErrorKind::ReadOnly),
        (io::ErrorKind::StorageFull, ErrorKind::NoSpace),
        (io::ErrorKind::QuotaExceeded, ErrorKind::QuotaExceeded),
        (io::ErrorKind::FileTooLarge, ErrorKind::FileTooLarge),
        (io::ErrorKind::CrossesDevices, ErrorKind::CrossesDevices),
        (io::ErrorKind::TooManyLinks, ErrorKind::TooManyLinks),
        (io::ErrorKind::Unsupported, ErrorKind::Unsupported),
        (io::ErrorKind::InvalidData, ErrorKind::Io),
        (io::ErrorKind::Other, ErrorKind::Io),
        (io::ErrorKind::BrokenPipe, ErrorKind::Io),
    ];
    for (io_kind, vfs_kind) in cases {
        let err = Error::from(io::Error::new(io_kind, "plain-io-boom"));
        assert_eq!(err.kind(), vfs_kind, "io::{io_kind:?} -> vfs::{vfs_kind:?}");
        assert!(err.message().is_empty());
        let src = std::error::Error::source(&err).expect("plain io::Error should be source");
        assert!(src.to_string().contains("plain-io-boom"));
    }
}

// --- getattr / lookup / parent (21–23) -------------------------------------

#[test]
fn t21_getattr_root_in_fixture() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let root = fs.root();
    let attr = block_on(fs.getattr(&caller, root)).unwrap();
    assert_eq!(attr.id, root);
    assert_eq!(attr.kind, NodeKind::Dir);
    assert_eq!(attr.nlink, 3); // 2 + /dir
    assert_eq!(attr.size, 3); // a.txt, dir, link
    assert_eq!(attr.perm, 0o755);
    assert_eq!(attr.generation, 0);
}

#[test]
fn t22_lookup_semantics_and_errors() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let root = fs.root();

    block_on(async {
        let a_lookup = fs.lookup(&caller, root, name(b"a.txt")).await.unwrap();
        let a_getattr = fs.getattr(&caller, a_lookup.id).await.unwrap();
        assert_eq!(a_lookup, a_getattr);

        let missing = fs.lookup(&caller, root, name(b"nope")).await.unwrap_err();
        assert_eq!(missing.kind(), ErrorKind::NotFound);

        let on_file = fs
            .lookup(&caller, a_lookup.id, name(b"x"))
            .await
            .unwrap_err();
        assert_eq!(on_file.kind(), ErrorKind::NotADirectory);

        let link = fs.lookup(&caller, root, name(b"link")).await.unwrap();
        assert_eq!(link.kind, NodeKind::Symlink);
        assert_eq!(link.size, 5);

        let on_link = fs.lookup(&caller, link.id, name(b"x")).await.unwrap_err();
        assert_eq!(on_link.kind(), ErrorKind::NotADirectory);

        // Name longer than backend's max_name_len -> NameTooLong.
        let short_fs = MemFs::with_limits(
            Limits::new().with_max_name_len(4),
            MemFs::DEFAULT_MAX_NODES,
            MemFs::DEFAULT_MAX_BYTES,
        );
        let wide_name = Name::new(b"12345", &Limits::new()).unwrap();
        let err = short_fs
            .lookup(&caller, short_fs.root(), wide_name)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NameTooLong);
    });
}

#[test]
fn t23_parent_semantics_and_errors() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let root = fs.root();

    block_on(async {
        let root_parent = fs.parent(&caller, root).await.unwrap();
        assert_eq!(root_parent.id, root);

        let dir = fs.lookup(&caller, root, name(b"dir")).await.unwrap();
        assert_eq!(fs.parent(&caller, dir.id).await.unwrap().id, root);

        let sub = fs.mkdir(dir.id, b"sub").unwrap();
        assert_eq!(fs.parent(&caller, sub).await.unwrap().id, dir.id);

        let file = fs.lookup(&caller, root, name(b"a.txt")).await.unwrap();
        assert_eq!(
            fs.parent(&caller, file.id).await.unwrap_err().kind(),
            ErrorKind::NotADirectory
        );

        let link = fs.lookup(&caller, root, name(b"link")).await.unwrap();
        assert_eq!(
            fs.parent(&caller, link.id).await.unwrap_err().kind(),
            ErrorKind::NotADirectory
        );
    });
}

// --- read (24–29) ----------------------------------------------------------

#[test]
fn t24_read_exact_and_eof() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let a = block_on(fs.lookup(&caller, fs.root(), name(b"a.txt"))).unwrap();

    block_on(async {
        let mut exact = [0u8; 5];
        let r = fs.read(&caller, a.id, 0, &mut exact).await.unwrap();
        assert_eq!(r, ReadResult::new(5, true));
        assert_eq!(&exact, b"hello");

        let mut short = [0u8; 4];
        let r = fs.read(&caller, a.id, 0, &mut short).await.unwrap();
        assert_eq!(r, ReadResult::new(4, false));
        assert_eq!(&short, b"hell");

        let mut large = [0xAAu8; 64];
        let r = fs.read(&caller, a.id, 0, &mut large).await.unwrap();
        assert_eq!(r, ReadResult::new(5, true));
        assert_eq!(&large[..5], b"hello");

        let mut tail = [0u8; 4];
        let r = fs.read(&caller, a.id, 2, &mut tail).await.unwrap();
        assert_eq!(r, ReadResult::new(3, true));
        assert_eq!(&tail[..3], b"llo");
    });
}

#[test]
fn t25_read_at_and_past_eof_and_u64_max() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let a = block_on(fs.lookup(&caller, fs.root(), name(b"a.txt"))).unwrap();

    block_on(async {
        let mut buf = [0u8; 16];
        for offset in [5u64, 1u64 << 40, u64::MAX, u64::MAX - 3] {
            let r = fs.read(&caller, a.id, offset, &mut buf).await.unwrap();
            assert_eq!(r, ReadResult::new(0, true), "offset={offset}");
        }
    });
}

#[test]
fn t26_read_empty_buffer() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let a = block_on(fs.lookup(&caller, fs.root(), name(b"a.txt"))).unwrap();
    let empty_file = fs.create_file(fs.root(), b"empty", b"").unwrap();

    block_on(async {
        let r0 = fs.read(&caller, a.id, 0, &mut []).await.unwrap();
        assert_eq!(r0, ReadResult::new(0, false));

        let r5 = fs.read(&caller, a.id, 5, &mut []).await.unwrap();
        assert_eq!(r5, ReadResult::new(0, true));

        let re = fs.read(&caller, empty_file, 0, &mut []).await.unwrap();
        assert_eq!(re, ReadResult::new(0, true));
    });
}

#[test]
fn t27_read_clamps_to_max_read_len() {
    let caller = Caller::new(0, 0);

    let fs = MemFs::with_limits(
        Limits::new().with_max_read_len(4),
        MemFs::DEFAULT_MAX_NODES,
        MemFs::DEFAULT_MAX_BYTES,
    );
    let f = fs.create_file(fs.root(), b"f", b"0123456789").unwrap();

    block_on(async {
        let mut buf = [0u8; 16];
        let r1 = fs.read(&caller, f, 0, &mut buf).await.unwrap();
        assert_eq!(r1, ReadResult::new(4, false));
        assert_eq!(&buf[..4], b"0123");

        let r2 = fs.read(&caller, f, 4, &mut buf).await.unwrap();
        assert_eq!(r2, ReadResult::new(4, false));
        assert_eq!(&buf[..4], b"4567");

        let r3 = fs.read(&caller, f, 8, &mut buf).await.unwrap();
        assert_eq!(r3, ReadResult::new(2, true));
        assert_eq!(&buf[..2], b"89");
    });

    // max_read_len = 0 behaves as 1 and still makes progress.
    let fs0 = MemFs::with_limits(
        Limits::new().with_max_read_len(0),
        MemFs::DEFAULT_MAX_NODES,
        MemFs::DEFAULT_MAX_BYTES,
    );
    let f0 = fs0.create_file(fs0.root(), b"f", b"abc").unwrap();
    block_on(async {
        let mut out = Vec::new();
        let mut off = 0u64;
        let mut buf = [0u8; 8];
        loop {
            let r = fs0.read(&caller, f0, off, &mut buf).await.unwrap();
            assert_eq!(r.len, 1);
            out.extend_from_slice(&buf[..r.len]);
            off += r.len as u64;
            if r.eof {
                break;
            }
        }
        assert_eq!(out, b"abc");
    });
}

#[test]
fn t28_read_wrong_node_kind() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let root = fs.root();
    let link = block_on(fs.lookup(&caller, root, name(b"link"))).unwrap();

    block_on(async {
        let mut buf = [0u8; 8];
        assert_eq!(
            fs.read(&caller, root, 0, &mut buf)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::IsADirectory
        );
        assert_eq!(
            fs.read(&caller, link.id, 0, &mut buf)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
    });
}

#[test]
fn t29_read_large_file_3mib_default_clamp() {
    let fs = MemFs::new();
    let total = 3 * 1024 * 1024usize;
    let data: Vec<u8> = (0..total)
        .map(|i| ((i.wrapping_mul(31) ^ (i >> 8)) & 0xff) as u8)
        .collect();
    let id = fs.create_file(fs.root(), b"big.bin", &data).unwrap();
    let caller = Caller::new(0, 0);

    block_on(async {
        let mut buf = vec![0u8; total];
        let first = fs.read(&caller, id, 0, &mut buf).await.unwrap();
        assert_eq!(first.len, 1_048_576);
        assert!(!first.eof);

        let mut assembled = Vec::with_capacity(total);
        assembled.extend_from_slice(&buf[..first.len]);
        let mut offset = first.len as u64;
        loop {
            let r = fs.read(&caller, id, offset, &mut buf).await.unwrap();
            assert!(r.len <= 1_048_576);
            assembled.extend_from_slice(&buf[..r.len]);
            offset += r.len as u64;
            if r.eof {
                break;
            }
        }
        assert_eq!(assembled, data);
    });
}

// --- read_dir and cookies (30–39) ------------------------------------------

#[test]
fn t30_read_dir_empty_dir() {
    let fs = MemFs::new();
    let caller = Caller::new(0, 0);
    let page = block_on(fs.read_dir(&caller, fs.root(), DirCursor::START, 16, false)).unwrap();
    assert!(page.entries.is_empty());
    assert!(page.eof);
    assert_eq!(page.verifier, 0);
}

#[test]
fn t31_read_dir_insertion_order_and_cookies() {
    let fs = MemFs::new();
    let root = fs.root();
    fs.create_file(root, b"c", b"").unwrap();
    fs.create_file(root, b"a", b"").unwrap();
    fs.create_file(root, b"b", b"").unwrap();

    let caller = Caller::new(0, 0);
    let page = block_on(fs.read_dir(&caller, root, DirCursor::START, 16, false)).unwrap();
    let names: Vec<&[u8]> = page.entries.iter().map(|e| e.name.as_slice()).collect();
    assert_eq!(names, vec![&b"c"[..], &b"a"[..], &b"b"[..]]);
    assert!(page.eof);
    assert!(page.entries[0].cookie >= Cookie::FIRST);
    assert!(page.entries[1].cookie > page.entries[0].cookie);
    assert!(page.entries[2].cookie > page.entries[1].cookie);
}

#[test]
fn t32_read_dir_paging_one_by_one() {
    let fs = MemFs::new();
    let root = fs.root();
    for i in 0..5 {
        fs.create_file(root, format!("f{i}").as_bytes(), b"")
            .unwrap();
    }

    let caller = Caller::new(0, 0);
    block_on(async {
        let mut cursor = DirCursor::START;
        let mut seen = Vec::new();
        for step in 0..5 {
            let page = fs.read_dir(&caller, root, cursor, 1, false).await.unwrap();
            assert_eq!(page.entries.len(), 1);
            assert_eq!(page.eof, step == 4, "step={step}");
            let e = &page.entries[0];
            seen.push(e.name.clone());
            cursor = DirCursor::new(e.cookie, page.verifier);
        }
        assert_eq!(seen.len(), 5);

        let after = fs.read_dir(&caller, root, cursor, 1, false).await.unwrap();
        assert!(after.entries.is_empty());
        assert!(after.eof);
    });
}

#[test]
fn t33_read_dir_max_entries_zero_and_usize_max() {
    let fs = MemFs::with_limits(
        Limits::new().with_max_dir_entries(5),
        MemFs::DEFAULT_MAX_NODES,
        MemFs::DEFAULT_MAX_BYTES,
    );
    let root = fs.root();
    for i in 0..8 {
        fs.create_file(root, format!("e{i}").as_bytes(), b"")
            .unwrap();
    }
    let caller = Caller::new(0, 0);

    block_on(async {
        let p0 = fs
            .read_dir(&caller, root, DirCursor::START, 0, false)
            .await
            .unwrap();
        assert_eq!(p0.entries.len(), 1);
        assert!(!p0.eof);

        let pmax = fs
            .read_dir(&caller, root, DirCursor::START, usize::MAX, false)
            .await
            .unwrap();
        assert_eq!(pmax.entries.len(), 5);
        assert!(!pmax.eof);
    });
}

#[test]
fn t34_read_dir_ten_thousand_entries() {
    let fs = MemFs::new();
    let root = fs.root();
    for i in 0..10_000usize {
        fs.create_file(root, format!("item_{i:05}").as_bytes(), b"")
            .unwrap();
    }
    let caller = Caller::new(0, 0);

    block_on(async {
        let mut cursor = DirCursor::START;
        let mut page_sizes = Vec::new();
        let mut names = HashSet::new();
        loop {
            let page = fs
                .read_dir(&caller, root, cursor, usize::MAX, false)
                .await
                .unwrap();
            page_sizes.push(page.entries.len());
            for e in &page.entries {
                assert!(names.insert(e.name.clone()), "duplicate entry");
            }
            if page.eof {
                break;
            }
            let last = page.entries.last().unwrap();
            cursor = DirCursor::new(last.cookie, page.verifier);
        }
        assert_eq!(page_sizes, vec![4096, 4096, 1808]);
        assert_eq!(names.len(), 10_000);
    });

    // Custom small limit
    let fs7 = MemFs::with_limits(
        Limits::new().with_max_dir_entries(7),
        MemFs::DEFAULT_MAX_NODES,
        MemFs::DEFAULT_MAX_BYTES,
    );
    for i in 0..25usize {
        fs7.create_file(fs7.root(), format!("k{i}").as_bytes(), b"")
            .unwrap();
    }
    block_on(async {
        let mut cursor = DirCursor::START;
        let mut total = 0usize;
        loop {
            let page = fs7
                .read_dir(&caller, fs7.root(), cursor, 100, false)
                .await
                .unwrap();
            assert!(page.entries.len() <= 7);
            total += page.entries.len();
            if page.eof {
                break;
            }
            cursor = DirCursor::new(page.entries.last().unwrap().cookie, page.verifier);
        }
        assert_eq!(total, 25);
    });
}

#[test]
fn t35_read_dir_start_cursors_and_verifiers() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let root = fs.root();

    block_on(async {
        let base = fs
            .read_dir(&caller, root, DirCursor::START, 16, false)
            .await
            .unwrap();
        for cursor in [
            DirCursor::new(Cookie::START, 0xdead_beef),
            DirCursor::new(Cookie::DOT, 0),
            DirCursor::new(Cookie::DOT, 0x1234_5678),
            DirCursor::new(Cookie::DOT_DOT, 0),
            DirCursor::new(Cookie::DOT_DOT, u64::MAX),
        ] {
            let page = fs.read_dir(&caller, root, cursor, 16, false).await.unwrap();
            assert_eq!(page, base, "cursor={cursor:?}");
        }
    });
}

#[test]
fn t36_read_dir_arbitrary_cookies() {
    let fs = MemFs::new();
    let root = fs.root();
    fs.create_file(root, b"e1", b"").unwrap();
    fs.create_file(root, b"e2", b"").unwrap();
    fs.create_file(root, b"e3", b"").unwrap();
    fs.remove(root, b"e2").unwrap();

    let caller = Caller::new(0, 0);
    block_on(async {
        let all = fs
            .read_dir(&caller, root, DirCursor::START, 16, false)
            .await
            .unwrap();
        assert_eq!(all.entries.len(), 2);
        let c1 = all.entries[0].cookie.get();
        let c3 = all.entries[1].cookie.get();
        assert!(c3 > c1 + 1, "expected gap cookie between e1 and e3");

        // Resume from the never-emitted gap cookie between c1 and c3.
        let mid = fs
            .read_dir(
                &caller,
                root,
                DirCursor::new(Cookie::new(c1 + 1), 0),
                16,
                false,
            )
            .await
            .unwrap();
        assert_eq!(mid.entries.len(), 1);
        assert_eq!(mid.entries[0].name, b"e3");
        assert!(mid.eof);

        // u64::MAX cookie returns empty eof, never BadCookie.
        let end = fs
            .read_dir(
                &caller,
                root,
                DirCursor::new(Cookie::new(u64::MAX), 99),
                16,
                false,
            )
            .await
            .unwrap();
        assert!(end.entries.is_empty());
        assert!(end.eof);
    });
}

#[test]
fn t37_read_dir_cookie_stability_under_insert_delete() {
    let fs = MemFs::new();
    let root = fs.root();
    for nm in [b"e0", b"e1", b"e2", b"e3", b"e4"] {
        fs.create_file(root, nm, b"").unwrap();
    }
    let caller = Caller::new(0, 0);

    block_on(async {
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let mut seen_set: HashSet<Vec<u8>> = HashSet::new();

        // Page 1: yields e0.
        let p1 = fs
            .read_dir(&caller, root, DirCursor::START, 1, false)
            .await
            .unwrap();
        assert_eq!(p1.entries.len(), 1);
        assert_eq!(p1.entries[0].name, b"e0");
        assert!(seen_set.insert(p1.entries[0].name.clone()));
        seen.push(p1.entries[0].name.clone());
        let mut cursor = DirCursor::new(p1.entries[0].cookie, p1.verifier);

        // Remove next unlisted entry (e1).
        fs.remove(root, b"e1").unwrap();

        // Page 2: yields e2.
        let p2 = fs.read_dir(&caller, root, cursor, 1, false).await.unwrap();
        assert_eq!(p2.entries.len(), 1);
        assert_eq!(p2.entries[0].name, b"e2");
        assert!(seen_set.insert(p2.entries[0].name.clone()));
        seen.push(p2.entries[0].name.clone());
        cursor = DirCursor::new(p2.entries[0].cookie, p2.verifier);

        // Remove already-listed entry (e0), remove entry at cursor (e2), insert e5.
        fs.remove(root, b"e0").unwrap();
        fs.remove(root, b"e2").unwrap();
        fs.create_file(root, b"e5", b"").unwrap();

        loop {
            let p = fs.read_dir(&caller, root, cursor, 1, false).await.unwrap();
            for e in &p.entries {
                assert!(seen_set.insert(e.name.clone()), "duplicate {:?}", e.name);
                seen.push(e.name.clone());
            }
            if p.eof {
                break;
            }
            let last = p.entries.last().unwrap();
            cursor = DirCursor::new(last.cookie, p.verifier);
        }

        // e3, e4 present for whole listing -> each appears once.
        // e1 removed before listed -> never appears.
        // e5 inserted during listing -> appears once, at the end.
        assert_eq!(
            seen,
            vec![
                b"e0".to_vec(),
                b"e2".to_vec(),
                b"e3".to_vec(),
                b"e4".to_vec(),
                b"e5".to_vec(),
            ]
        );
    });
}

#[test]
fn t38_read_dir_with_attrs() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let root = fs.root();

    block_on(async {
        let without = fs
            .read_dir(&caller, root, DirCursor::START, 16, false)
            .await
            .unwrap();
        assert!(without.entries.iter().all(|e| e.attr.is_none()));

        let with = fs
            .read_dir(&caller, root, DirCursor::START, 16, true)
            .await
            .unwrap();
        assert_eq!(with.entries.len(), without.entries.len());
        for entry in &with.entries {
            let attr = entry.attr.as_ref().expect("with_attrs=true populates attr");
            let expected = fs.getattr(&caller, entry.id).await.unwrap();
            assert_eq!(attr, &expected);
        }
    });
}

#[test]
fn t39_read_dir_on_non_directory() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let root = fs.root();

    block_on(async {
        let file = fs.lookup(&caller, root, name(b"a.txt")).await.unwrap();
        let link = fs.lookup(&caller, root, name(b"link")).await.unwrap();
        assert_eq!(
            fs.read_dir(&caller, file.id, DirCursor::START, 16, false)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::NotADirectory
        );
        assert_eq!(
            fs.read_dir(&caller, link.id, DirCursor::START, 16, false)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::NotADirectory
        );
    });
}

// --- Stale ids and identity (40–44) ----------------------------------------

async fn assert_all_8_ops_stale<F: Filesystem + ?Sized>(fs: &F, id: NodeId) {
    let caller = Caller::new(0, 0);
    let mut buf = [0u8; 8];
    assert_eq!(
        fs.getattr(&caller, id).await.unwrap_err().kind(),
        ErrorKind::Stale,
        "getattr"
    );
    assert_eq!(
        fs.lookup(&caller, id, name(b"x")).await.unwrap_err().kind(),
        ErrorKind::Stale,
        "lookup"
    );
    assert_eq!(
        fs.parent(&caller, id).await.unwrap_err().kind(),
        ErrorKind::Stale,
        "parent"
    );
    assert_eq!(
        fs.access(&caller, id, Access::READ)
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Stale,
        "access"
    );
    assert_eq!(
        fs.read(&caller, id, 0, &mut buf).await.unwrap_err().kind(),
        ErrorKind::Stale,
        "read"
    );
    assert_eq!(
        fs.read_dir(&caller, id, DirCursor::START, 8, false)
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Stale,
        "read_dir"
    );
    assert_eq!(
        fs.read_link(&caller, id).await.unwrap_err().kind(),
        ErrorKind::Stale,
        "read_link"
    );
    assert_eq!(
        fs.statfs(&caller, id).await.unwrap_err().kind(),
        ErrorKind::Stale,
        "statfs"
    );
}

#[test]
fn t40_stale_unknown_ids_all_ops() {
    let fs = MemFs::new();
    block_on(async {
        assert_all_8_ops_stale(&fs, NodeId::new(u64::MAX).unwrap()).await;
        assert_all_8_ops_stale(&fs, NodeId::new(2).unwrap()).await;
    });
}

#[test]
fn t41_stale_after_remove() {
    let fs = MemFs::new();
    let root = fs.root();
    let f = fs.create_file(root, b"gone.txt", b"bye").unwrap();
    let d = fs.mkdir(root, b"gone_dir").unwrap();

    fs.remove(root, b"gone.txt").unwrap();
    fs.remove(root, b"gone_dir").unwrap();

    block_on(async {
        assert_all_8_ops_stale(&fs, f).await;
        assert_all_8_ops_stale(&fs, d).await;
    });
}

#[test]
fn t42_ids_never_reused() {
    let fs = MemFs::new();
    let root = fs.root();
    let id1 = fs.create_file(root, b"x", b"1").unwrap();
    fs.remove(root, b"x").unwrap();
    let id2 = fs.create_file(root, b"x", b"2").unwrap();
    assert!(id2 > id1);
    block_on(assert_all_8_ops_stale(&fs, id1));

    // Also when node budget is tight (max_nodes = 2: root + 1 file).
    let tight = MemFs::with_limits(Limits::new(), 2, MemFs::DEFAULT_MAX_BYTES);
    let r = tight.root();
    let a = tight.create_file(r, b"x", b"1").unwrap();
    tight.remove(r, b"x").unwrap();
    let b = tight.create_file(r, b"x", b"2").unwrap();
    assert!(b > a);
    block_on(assert_all_8_ops_stale(&tight, a));
}

#[test]
fn t43_id_epoch_unique_per_instance_and_info_stable() {
    let a = MemFs::new();
    let b = MemFs::new();
    assert_ne!(a.info().id_epoch, b.info().id_epoch);
    assert_eq!(a.info(), a.info());
    assert_eq!(b.info(), b.info());
}

#[test]
fn t44_attribute_bookkeeping() {
    let fs = MemFs::new();
    let root = fs.root();
    let caller = Caller::new(0, 0);

    block_on(async {
        let r0 = fs.getattr(&caller, root).await.unwrap();
        let f = fs.create_file(root, b"f", b"abc").unwrap();
        let r1 = fs.getattr(&caller, root).await.unwrap();
        assert_eq!(r1.change, r0.change + 1);
        assert_eq!(r1.size, 1);
        assert_eq!(r1.nlink, 2);

        let _d = fs.mkdir(root, b"sub").unwrap();
        let r2 = fs.getattr(&caller, root).await.unwrap();
        assert_eq!(r2.change, r1.change + 1);
        assert_eq!(r2.nlink, 3);

        let f0 = fs.getattr(&caller, f).await.unwrap();
        fs.write_all(f, 0, b"wxyz").unwrap();
        let f1 = fs.getattr(&caller, f).await.unwrap();
        assert_eq!(f1.change, f0.change + 1);
        assert!(f1.mtime >= f0.mtime);
        assert_eq!(f1.size, 4);

        // Read operations never mutate attributes.
        let mut buf = [0u8; 8];
        let _ = fs.read(&caller, f, 0, &mut buf).await.unwrap();
        let _ = fs
            .read_dir(&caller, root, DirCursor::START, 16, true)
            .await
            .unwrap();
        let _ = fs.lookup(&caller, root, name(b"f")).await.unwrap();
        let _ = fs.access(&caller, f, Access::READ).await.unwrap();

        assert_eq!(fs.getattr(&caller, f).await.unwrap(), f1);
        assert_eq!(fs.getattr(&caller, root).await.unwrap(), r2);
    });
}

// --- MemFs build API and limits (45–52) ------------------------------------

#[test]
fn t45_memfs_build_name_and_parent_errors() {
    let fs = fixture();
    let root = fs.root();
    let caller = Caller::new(0, 0);
    let file_id = block_on(fs.lookup(&caller, root, name(b"a.txt")))
        .unwrap()
        .id;
    let link_id = block_on(fs.lookup(&caller, root, name(b"link")))
        .unwrap()
        .id;
    let dead_dir = fs.mkdir(root, b"dead").unwrap();
    fs.remove(root, b"dead").unwrap();

    let long256 = vec![b'z'; 256];

    // create_file
    assert_eq!(
        fs.create_file(root, b"a/b", b"").unwrap_err().kind(),
        ErrorKind::InvalidName
    );
    assert_eq!(
        fs.create_file(root, &long256, b"").unwrap_err().kind(),
        ErrorKind::NameTooLong
    );
    assert_eq!(
        fs.create_file(root, b"a.txt", b"").unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(
        fs.create_file(file_id, b"c", b"").unwrap_err().kind(),
        ErrorKind::NotADirectory
    );
    assert_eq!(
        fs.create_file(link_id, b"c", b"").unwrap_err().kind(),
        ErrorKind::NotADirectory
    );
    assert_eq!(
        fs.create_file(dead_dir, b"c", b"").unwrap_err().kind(),
        ErrorKind::Stale
    );

    // mkdir
    assert_eq!(
        fs.mkdir(root, b"..").unwrap_err().kind(),
        ErrorKind::InvalidName
    );
    assert_eq!(
        fs.mkdir(root, &long256).unwrap_err().kind(),
        ErrorKind::NameTooLong
    );
    assert_eq!(
        fs.mkdir(root, b"dir").unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(
        fs.mkdir(file_id, b"sub").unwrap_err().kind(),
        ErrorKind::NotADirectory
    );
    assert_eq!(
        fs.mkdir(dead_dir, b"sub").unwrap_err().kind(),
        ErrorKind::Stale
    );

    // symlink
    assert_eq!(
        fs.symlink(root, b"", b"t").unwrap_err().kind(),
        ErrorKind::InvalidName
    );
    assert_eq!(
        fs.symlink(root, &long256, b"t").unwrap_err().kind(),
        ErrorKind::NameTooLong
    );
    assert_eq!(
        fs.symlink(root, b"link", b"t").unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(
        fs.symlink(file_id, b"s", b"t").unwrap_err().kind(),
        ErrorKind::NotADirectory
    );
    assert_eq!(
        fs.symlink(dead_dir, b"s", b"t").unwrap_err().kind(),
        ErrorKind::Stale
    );
}

#[test]
fn t46_memfs_symlink_target_validation() {
    let fs = MemFs::new();
    let root = fs.root();
    let caller = Caller::new(0, 0);

    let max_target = vec![b't'; 4096];
    let ok_id = fs.symlink(root, b"ok4096", &max_target).unwrap();
    let got = block_on(fs.read_link(&caller, ok_id)).unwrap();
    assert_eq!(got, max_target);

    let too_long = vec![b't'; 4097];
    assert_eq!(
        fs.symlink(root, b"bad4097", &too_long).unwrap_err().kind(),
        ErrorKind::NameTooLong
    );
    assert_eq!(
        fs.symlink(root, b"empty", b"").unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        fs.symlink(root, b"nul", b"a\0b").unwrap_err().kind(),
        ErrorKind::InvalidInput
    );

    let fs8 = MemFs::with_limits(
        Limits::new().with_max_symlink_len(8),
        MemFs::DEFAULT_MAX_NODES,
        MemFs::DEFAULT_MAX_BYTES,
    );
    assert!(fs8.symlink(fs8.root(), b"s8", b"12345678").is_ok());
    assert_eq!(
        fs8.symlink(fs8.root(), b"s9", b"123456789")
            .unwrap_err()
            .kind(),
        ErrorKind::NameTooLong
    );
}

#[test]
fn t47_memfs_max_nodes_budget() {
    let fs = MemFs::with_limits(Limits::new(), 3, MemFs::DEFAULT_MAX_BYTES);
    let root = fs.root();
    assert_eq!(fs.node_count(), 1);
    fs.create_file(root, b"a", b"1").unwrap();
    fs.mkdir(root, b"b").unwrap();
    assert_eq!(fs.node_count(), 3);

    assert_eq!(
        fs.create_file(root, b"c", b"").unwrap_err().kind(),
        ErrorKind::NoSpace
    );
    assert_eq!(fs.node_count(), 3);

    fs.remove(root, b"a").unwrap();
    assert_eq!(fs.node_count(), 2);
    fs.create_file(root, b"c", b"2").unwrap();
    assert_eq!(fs.node_count(), 3);
}

#[test]
fn t48_memfs_max_bytes_exact_and_all_or_nothing() {
    let fs = MemFs::with_limits(Limits::new(), 100, 10);
    let root = fs.root();
    let a = fs.create_file(root, b"a", b"123456789").unwrap();
    assert_eq!(fs.bytes_used(), 10);

    assert_eq!(
        fs.create_file(root, b"b", b"").unwrap_err().kind(),
        ErrorKind::NoSpace
    );
    assert_eq!(fs.bytes_used(), 10);

    assert_eq!(
        fs.write_all(a, 9, b"x").unwrap_err().kind(),
        ErrorKind::NoSpace
    );
    assert_eq!(fs.bytes_used(), 10);

    let mut buf = [0u8; 16];
    let r = block_on(fs.read(&Caller::new(0, 0), a, 0, &mut buf)).unwrap();
    assert_eq!(r, ReadResult::new(9, true));
    assert_eq!(&buf[..9], b"123456789");
}

#[test]
fn t49_memfs_sparse_writes_and_overflow() {
    let fs = MemFs::new();
    let root = fs.root();
    let f = fs.create_file(root, b"f", b"").unwrap();
    let initial_bytes = fs.bytes_used();

    assert_eq!(
        fs.write_all(f, 1u64 << 40, b"x").unwrap_err().kind(),
        ErrorKind::NoSpace
    );
    assert_eq!(fs.bytes_used(), initial_bytes);

    assert_eq!(
        fs.write_all(f, u64::MAX - 1, b"xyz").unwrap_err().kind(),
        ErrorKind::FileTooLarge
    );
    assert_eq!(
        fs.set_len(f, u64::MAX).unwrap_err().kind(),
        ErrorKind::FileTooLarge
    );
    assert_eq!(
        fs.set_len(f, (i64::MAX as u64) + 1).unwrap_err().kind(),
        ErrorKind::FileTooLarge
    );

    // Sparse write within budget zero-fills gap.
    fs.write_all(f, 5, b"hi").unwrap();
    let mut buf = [0xFFu8; 16];
    let r = block_on(fs.read(&Caller::new(0, 0), f, 0, &mut buf)).unwrap();
    assert_eq!(r, ReadResult::new(7, true));
    assert_eq!(&buf[..7], b"\0\0\0\0\0hi");

    // set_len truncates and zero-extends.
    fs.set_len(f, 2).unwrap();
    fs.set_len(f, 4).unwrap();
    let r2 = block_on(fs.read(&Caller::new(0, 0), f, 0, &mut buf)).unwrap();
    assert_eq!(r2, ReadResult::new(4, true));
    assert_eq!(&buf[..4], b"\0\0\0\0");
}

#[test]
fn t50_memfs_write_set_len_set_perm_errors() {
    let fs = fixture();
    let root = fs.root();
    let caller = Caller::new(0, 0);
    let file = block_on(fs.lookup(&caller, root, name(b"a.txt")))
        .unwrap()
        .id;
    let link = block_on(fs.lookup(&caller, root, name(b"link")))
        .unwrap()
        .id;

    assert_eq!(
        fs.write_all(root, 0, b"x").unwrap_err().kind(),
        ErrorKind::IsADirectory
    );
    assert_eq!(
        fs.set_len(root, 0).unwrap_err().kind(),
        ErrorKind::IsADirectory
    );
    assert_eq!(
        fs.write_all(link, 0, b"x").unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        fs.set_len(link, 0).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );

    assert_eq!(
        fs.set_perm(file, 0o10000).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    fs.set_perm(file, 0o7777).unwrap();
    fs.set_owner(file, 42, 43).unwrap();
    let t1 = UNIX_EPOCH + Duration::from_secs(100);
    let t2 = UNIX_EPOCH + Duration::from_secs(200);
    fs.set_times(file, t1, t2).unwrap();

    let attr = block_on(fs.getattr(&caller, file)).unwrap();
    assert_eq!(attr.perm, 0o7777);
    assert_eq!(attr.uid, 42);
    assert_eq!(attr.gid, 43);
    assert_eq!(attr.atime, t1);
    assert_eq!(attr.mtime, t2);
}

#[test]
fn t51_memfs_remove_and_budget_recovery() {
    let fs = fixture();
    let root = fs.root();

    assert_eq!(
        fs.remove(root, b"dir").unwrap_err().kind(),
        ErrorKind::DirectoryNotEmpty
    );
    assert_eq!(
        fs.remove(root, b"missing").unwrap_err().kind(),
        ErrorKind::NotFound
    );

    let dir_id = block_on(fs.lookup(&Caller::new(0, 0), root, name(b"dir")))
        .unwrap()
        .id;
    fs.remove(dir_id, b"b.bin").unwrap();
    fs.remove(root, b"dir").unwrap();
    fs.remove(root, b"a.txt").unwrap();
    fs.remove(root, b"link").unwrap();

    assert_eq!(fs.node_count(), 1);
    assert_eq!(fs.bytes_used(), 0);
}

#[test]
fn t52_memfs_statfs_accounting() {
    let fs = MemFs::with_limits(Limits::new(), 50, 1024);
    let root = fs.root();
    fs.create_file(root, b"hi", b"0123456789").unwrap();
    assert_eq!(fs.node_count(), 2);
    assert_eq!(fs.bytes_used(), 12);

    let st = block_on(fs.statfs(&Caller::new(0, 0), root)).unwrap();
    assert_eq!(st.total_bytes, 1024);
    assert_eq!(st.free_bytes, 1012);
    assert_eq!(st.avail_bytes, 1012);
    assert_eq!(st.total_files, 50);
    assert_eq!(st.free_files, 48);
    assert_eq!(st.avail_files, 48);
}

// --- resolve (53–57) -------------------------------------------------------

#[test]
fn t53_resolve_valid_paths_and_missing() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let limits = Limits::new();

    block_on(async {
        let root_attr = fs.getattr(&caller, fs.root()).await.unwrap();
        for p in [b"" as &[u8], b"/", b"//", b"///"] {
            assert_eq!(
                vfs::resolve(&fs, &caller, p, &limits).await.unwrap(),
                root_attr
            );
        }

        let dir = fs.lookup(&caller, fs.root(), name(b"dir")).await.unwrap();
        let b_bin = fs.lookup(&caller, dir.id, name(b"b.bin")).await.unwrap();
        for p in [b"/dir/b.bin" as &[u8], b"dir//b.bin/", b"///dir///b.bin///"] {
            assert_eq!(vfs::resolve(&fs, &caller, p, &limits).await.unwrap(), b_bin);
        }

        let err = vfs::resolve(&fs, &caller, b"/dir/missing", &limits)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    });
}

#[test]
fn t54_resolve_invalid_names_zero_backend_calls() {
    let fs = CountingFs::new(fixture());
    let caller = Caller::new(0, 0);
    let limits = Limits::new();

    block_on(async {
        for bad in [
            b"dir/./b.bin" as &[u8],
            b"dir/../a.txt",
            b"..",
            b".",
            b"dir/\0bad",
        ] {
            fs.reset();
            let err = vfs::resolve(&fs, &caller, bad, &limits).await.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::InvalidName, "path={bad:?}");
            assert_eq!(fs.total_calls(), 0, "expected 0 backend calls for {bad:?}");
        }
    });
}

#[test]
fn t55_resolve_path_length_bounds() {
    let fs = CountingFs::new(fixture());
    let caller = Caller::new(0, 0);
    let limits = Limits::new();

    // 1 leading '/' + 16 components of 255 'a's + 15 separating '/' = 4096 bytes.
    let mut p4096 = Vec::with_capacity(4096);
    for _ in 0..16 {
        p4096.push(b'/');
        p4096.extend(std::iter::repeat_n(b'a', 255));
    }
    assert_eq!(p4096.len(), 4096);

    let mut p4097 = p4096.clone();
    p4097.push(b'/');
    assert_eq!(p4097.len(), 4097);

    block_on(async {
        fs.reset();
        let err = vfs::resolve(&fs, &caller, &p4096, &limits)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(fs.total_calls() > 0);

        fs.reset();
        let err2 = vfs::resolve(&fs, &caller, &p4097, &limits)
            .await
            .unwrap_err();
        assert_eq!(err2.kind(), ErrorKind::NameTooLong);
        assert_eq!(fs.total_calls(), 0);
    });
}

#[test]
fn t56_resolve_path_depth_bounds() {
    let mem = MemFs::new();
    let mut cur = mem.root();
    let mut path256 = Vec::new();
    for _ in 0..256 {
        cur = mem.mkdir(cur, b"d").unwrap();
        path256.extend_from_slice(b"/d");
    }
    let mut path257 = path256.clone();
    path257.extend_from_slice(b"/d");

    let fs = CountingFs::new(mem);
    let caller = Caller::new(0, 0);
    let limits = Limits::new();

    block_on(async {
        let leaf = vfs::resolve(&fs, &caller, &path256, &limits).await.unwrap();
        assert_eq!(leaf.id, cur);

        fs.reset();
        let err = vfs::resolve(&fs, &caller, &path257, &limits)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert_eq!(fs.total_calls(), 0);
    });
}

#[test]
fn t57_resolve_does_not_follow_symlinks() {
    let fs = fixture();
    let caller = Caller::new(0, 0);
    let limits = Limits::new();

    block_on(async {
        let link = vfs::resolve(&fs, &caller, b"link", &limits).await.unwrap();
        assert_eq!(link.kind, NodeKind::Symlink);
        assert_eq!(link.size, 5);

        assert_eq!(
            vfs::resolve(&fs, &caller, b"link/x", &limits)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::NotADirectory
        );
        assert_eq!(
            vfs::resolve(&fs, &caller, b"a.txt/x", &limits)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::NotADirectory
        );
    });
}

// --- Cancellation (58–60) --------------------------------------------------

fn exercise_cancellation<F: Filesystem>(fs: &F, expect_pending_first_poll: bool) {
    let caller = Caller::new(0, 0);
    let root = fs.root();
    let a = block_on(fs.lookup(&caller, root, name(b"a.txt"))).unwrap();
    let link = block_on(fs.lookup(&caller, root, name(b"link"))).unwrap();

    fn check_op<Fut, T, Make>(mut make: Make, expect_pending: bool)
    where
        Fut: Future<Output = Result<T>>,
        T: PartialEq + std::fmt::Debug,
        Make: FnMut() -> Fut,
    {
        let baseline = block_on(make()).unwrap();
        // Drop unpolled.
        drop(make());
        // Poll once with Waker::noop() and drop.
        {
            let mut fut = pin!(make());
            let mut cx = Context::from_waker(Waker::noop());
            let poll = fut.as_mut().poll(&mut cx);
            if expect_pending {
                assert!(matches!(poll, Poll::Pending), "expected Poll::Pending");
            }
        }
        let after = block_on(make()).unwrap();
        assert_eq!(after, baseline);
    }

    check_op(|| fs.getattr(&caller, root), expect_pending_first_poll);
    check_op(
        || fs.lookup(&caller, root, name(b"a.txt")),
        expect_pending_first_poll,
    );
    check_op(|| fs.parent(&caller, root), expect_pending_first_poll);
    check_op(
        || fs.access(&caller, root, Access::all()),
        expect_pending_first_poll,
    );
    check_op(
        || async {
            let mut buf = [0u8; 16];
            let r = fs.read(&caller, a.id, 0, &mut buf).await?;
            Ok((r, buf[..r.len].to_vec()))
        },
        expect_pending_first_poll,
    );
    check_op(
        || fs.read_dir(&caller, root, DirCursor::START, 16, true),
        expect_pending_first_poll,
    );
    check_op(|| fs.read_link(&caller, link.id), expect_pending_first_poll);
    check_op(|| fs.statfs(&caller, root), expect_pending_first_poll);
}

#[test]
fn t58_cancellation_memfs_all_ops() {
    let fs = fixture();
    let nodes_before = fs.node_count();
    let bytes_before = fs.bytes_used();
    exercise_cancellation(&fs, false);
    assert_eq!(fs.node_count(), nodes_before);
    assert_eq!(fs.bytes_used(), bytes_before);
}

#[test]
fn t59_cancellation_pending_once_all_ops() {
    let mem = fixture();
    let nodes_before = mem.node_count();
    let bytes_before = mem.bytes_used();
    let wrapped = PendingOnce::new(mem);
    exercise_cancellation(&wrapped, true);
    assert_eq!(wrapped.inner.node_count(), nodes_before);
    assert_eq!(wrapped.inner.bytes_used(), bytes_before);
}

#[test]
fn t60_testing_check_cancellation() {
    let caller = Caller::new(0, 0);
    let limits = Limits::new();

    let fs = fixture();
    let rep1 = block_on(testing::check_cancellation(&fs, &caller, &limits, 100));
    assert!(rep1.is_ok(), "MemFs check_cancellation failed: {rep1}");
    assert_eq!(rep1.nodes(), 5);

    let pending = PendingOnce::new(fixture());
    let rep2 = block_on(testing::check_cancellation(&pending, &caller, &limits, 100));
    assert!(
        rep2.is_ok(),
        "PendingOnce check_cancellation failed: {rep2}"
    );
    assert_eq!(rep2.nodes(), 5);
}

// --- Dyn vs static parity (61–62) ------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct ScriptOutput {
    info: FsInfo,
    root: NodeId,
    root_attr: Attr,
    a_attr: Attr,
    missing_err: ErrorKind,
    not_dir_err: ErrorKind,
    parent_root: Attr,
    parent_dir: Attr,
    parent_file_err: ErrorKind,
    read_data: (ReadResult, Vec<u8>),
    read_eof: ReadResult,
    read_dir_err: ErrorKind,
    read_link_err: ErrorKind,
    page_no_attr: DirPage,
    page_with_attr: DirPage,
    read_dir_file_err: ErrorKind,
    access_res: Access,
    link_target: Vec<u8>,
    read_link_file_err: ErrorKind,
    stat: FsStat,
    stale_err: ErrorKind,
}

async fn run_script<F: Filesystem + ?Sized>(fs: &F) -> ScriptOutput {
    let caller = Caller::new(0, 0);
    let info = fs.info();
    let root = fs.root();
    let root_attr = fs.getattr(&caller, root).await.unwrap();
    let a_attr = fs.lookup(&caller, root, name(b"a.txt")).await.unwrap();
    let dir_attr = fs.lookup(&caller, root, name(b"dir")).await.unwrap();
    let link_attr = fs.lookup(&caller, root, name(b"link")).await.unwrap();
    let missing_err = fs
        .lookup(&caller, root, name(b"nope"))
        .await
        .unwrap_err()
        .kind();
    let not_dir_err = fs
        .lookup(&caller, a_attr.id, name(b"x"))
        .await
        .unwrap_err()
        .kind();
    let parent_root = fs.parent(&caller, root).await.unwrap();
    let parent_dir = fs.parent(&caller, dir_attr.id).await.unwrap();
    let parent_file_err = fs.parent(&caller, a_attr.id).await.unwrap_err().kind();
    let mut buf = [0u8; 16];
    let rr = fs.read(&caller, a_attr.id, 0, &mut buf).await.unwrap();
    let read_data = (rr, buf[..rr.len].to_vec());
    let read_eof = fs.read(&caller, a_attr.id, 100, &mut buf).await.unwrap();
    let read_dir_err = fs
        .read(&caller, root, 0, &mut buf)
        .await
        .unwrap_err()
        .kind();
    let read_link_err = fs
        .read(&caller, link_attr.id, 0, &mut buf)
        .await
        .unwrap_err()
        .kind();
    let page_no_attr = fs
        .read_dir(&caller, root, DirCursor::START, 16, false)
        .await
        .unwrap();
    let page_with_attr = fs
        .read_dir(&caller, root, DirCursor::START, 16, true)
        .await
        .unwrap();
    let read_dir_file_err = fs
        .read_dir(&caller, a_attr.id, DirCursor::START, 16, false)
        .await
        .unwrap_err()
        .kind();
    let access_res = fs.access(&caller, root, Access::all()).await.unwrap();
    let link_target = fs.read_link(&caller, link_attr.id).await.unwrap();
    let read_link_file_err = fs.read_link(&caller, a_attr.id).await.unwrap_err().kind();
    let stat = fs.statfs(&caller, root).await.unwrap();
    let stale_err = fs
        .getattr(&caller, NodeId::new(u64::MAX).unwrap())
        .await
        .unwrap_err()
        .kind();

    ScriptOutput {
        info,
        root,
        root_attr,
        a_attr,
        missing_err,
        not_dir_err,
        parent_root,
        parent_dir,
        parent_file_err,
        read_data,
        read_eof,
        read_dir_err,
        read_link_err,
        page_no_attr,
        page_with_attr,
        read_dir_file_err,
        access_res,
        link_target,
        read_link_file_err,
        stat,
        stale_err,
    }
}

async fn run_script_direct_dyn(fs: &dyn DynFilesystem) -> ScriptOutput {
    let caller = Caller::new(0, 0);
    let info = fs.info_dyn();
    let root = fs.root_dyn();
    let root_attr = fs.getattr_dyn(&caller, root).await.unwrap();
    let a_attr = fs.lookup_dyn(&caller, root, name(b"a.txt")).await.unwrap();
    let dir_attr = fs.lookup_dyn(&caller, root, name(b"dir")).await.unwrap();
    let link_attr = fs.lookup_dyn(&caller, root, name(b"link")).await.unwrap();
    let missing_err = fs
        .lookup_dyn(&caller, root, name(b"nope"))
        .await
        .unwrap_err()
        .kind();
    let not_dir_err = fs
        .lookup_dyn(&caller, a_attr.id, name(b"x"))
        .await
        .unwrap_err()
        .kind();
    let parent_root = fs.parent_dyn(&caller, root).await.unwrap();
    let parent_dir = fs.parent_dyn(&caller, dir_attr.id).await.unwrap();
    let parent_file_err = fs.parent_dyn(&caller, a_attr.id).await.unwrap_err().kind();
    let mut buf = [0u8; 16];
    let rr = fs.read_dyn(&caller, a_attr.id, 0, &mut buf).await.unwrap();
    let read_data = (rr, buf[..rr.len].to_vec());
    let read_eof = fs
        .read_dyn(&caller, a_attr.id, 100, &mut buf)
        .await
        .unwrap();
    let read_dir_err = fs
        .read_dyn(&caller, root, 0, &mut buf)
        .await
        .unwrap_err()
        .kind();
    let read_link_err = fs
        .read_dyn(&caller, link_attr.id, 0, &mut buf)
        .await
        .unwrap_err()
        .kind();
    let page_no_attr = fs
        .read_dir_dyn(&caller, root, DirCursor::START, 16, false)
        .await
        .unwrap();
    let page_with_attr = fs
        .read_dir_dyn(&caller, root, DirCursor::START, 16, true)
        .await
        .unwrap();
    let read_dir_file_err = fs
        .read_dir_dyn(&caller, a_attr.id, DirCursor::START, 16, false)
        .await
        .unwrap_err()
        .kind();
    let access_res = fs.access_dyn(&caller, root, Access::all()).await.unwrap();
    let link_target = fs.read_link_dyn(&caller, link_attr.id).await.unwrap();
    let read_link_file_err = fs
        .read_link_dyn(&caller, a_attr.id)
        .await
        .unwrap_err()
        .kind();
    let stat = fs.statfs_dyn(&caller, root).await.unwrap();
    let stale_err = fs
        .getattr_dyn(&caller, NodeId::new(u64::MAX).unwrap())
        .await
        .unwrap_err()
        .kind();

    ScriptOutput {
        info,
        root,
        root_attr,
        a_attr,
        missing_err,
        not_dir_err,
        parent_root,
        parent_dir,
        parent_file_err,
        read_data,
        read_eof,
        read_dir_err,
        read_link_err,
        page_no_attr,
        page_with_attr,
        read_dir_file_err,
        access_res,
        link_target,
        read_link_file_err,
        stat,
        stale_err,
    }
}

#[test]
fn t61_dyn_vs_static_parity() {
    let mem = fixture();
    let ref_mem: &MemFs = &mem;
    let dyn_ref: &dyn DynFilesystem = &mem;

    let arc_mem: Arc<MemFs> = Arc::new(fixture());
    let box_mem: Box<MemFs> = Box::new(fixture());
    let arc_dyn: Arc<dyn DynFilesystem> = Arc::new(fixture());
    let box_dyn: Box<dyn DynFilesystem> = Box::new(fixture());

    // Helper: zero out instance-specific id_epoch & timestamps when comparing distinct MemFs instances.
    fn normalize(mut s: ScriptOutput) -> ScriptOutput {
        s.info.id_epoch = 0;
        let zero_attr = |a: &mut Attr| {
            a.atime = UNIX_EPOCH;
            a.mtime = UNIX_EPOCH;
            a.ctime = UNIX_EPOCH;
        };
        zero_attr(&mut s.root_attr);
        zero_attr(&mut s.a_attr);
        zero_attr(&mut s.parent_root);
        zero_attr(&mut s.parent_dir);
        for e in &mut s.page_with_attr.entries {
            if let Some(a) = &mut e.attr {
                zero_attr(a);
            }
        }
        s
    }

    block_on(async {
        let out_direct = run_script(&mem).await;
        let out_ref = run_script(&ref_mem).await;
        let out_dyn_ref = run_script(dyn_ref).await;
        let out_direct_dyn = run_script_direct_dyn(dyn_ref).await;

        assert_eq!(out_direct, out_ref);
        assert_eq!(out_direct, out_dyn_ref);
        assert_eq!(out_direct, out_direct_dyn);

        let base_norm = normalize(out_direct);
        assert_eq!(normalize(run_script(&arc_mem).await), base_norm);
        assert_eq!(normalize(run_script(&box_mem).await), base_norm);
        assert_eq!(normalize(run_script(&arc_dyn).await), base_norm);
        assert_eq!(normalize(run_script(&box_dyn).await), base_norm);
    });
}

#[test]
fn t62_overrides_forwarded_through_all_wrappers() {
    let caller = Caller::new(0, 0);
    let expected_stat = Overriding::<MemFs>::fixed_stat();

    let ov = Overriding::new(fixture());
    let root = ov.root();
    let ref_ov: &Overriding<MemFs> = &ov;
    let dyn_ref: &dyn DynFilesystem = &ov;
    let arc_ov: Arc<Overriding<MemFs>> = Arc::new(Overriding::new(fixture()));
    let box_ov: Box<Overriding<MemFs>> = Box::new(Overriding::new(fixture()));
    let arc_dyn: Arc<dyn DynFilesystem> = Arc::new(Overriding::new(fixture()));
    let box_dyn: Box<dyn DynFilesystem> = Box::new(Overriding::new(fixture()));

    async fn check<F: Filesystem + ?Sized>(fs: &F, caller: &Caller, expected: FsStat) {
        let r = fs.root();
        assert_eq!(
            fs.access(caller, r, Access::all()).await.unwrap(),
            Access::empty()
        );
        assert_eq!(fs.statfs(caller, r).await.unwrap(), expected);
    }

    block_on(async {
        check(&ref_ov, &caller, expected_stat).await;
        check(&arc_ov, &caller, expected_stat).await;
        check(&box_ov, &caller, expected_stat).await;
        check(dyn_ref, &caller, expected_stat).await;
        check(&arc_dyn, &caller, expected_stat).await;
        check(&box_dyn, &caller, expected_stat).await;

        // Direct *_dyn calls
        assert_eq!(
            dyn_ref
                .access_dyn(&caller, root, Access::all())
                .await
                .unwrap(),
            Access::empty()
        );
        assert_eq!(
            dyn_ref.statfs_dyn(&caller, root).await.unwrap(),
            expected_stat
        );
    });
}

// --- Testing kit (63–65) ---------------------------------------------------

#[test]
fn t63_testing_check_on_memfs_and_max_nodes() {
    let caller = Caller::new(0, 0);
    let limits = Limits::new();

    block_on(async {
        // Empty MemFs
        let empty = MemFs::new();
        let r_empty = testing::check(&empty, &caller, &limits, 100).await;
        assert!(r_empty.is_ok(), "empty failed: {r_empty}");
        assert_eq!(r_empty.nodes(), 1);

        // fixture()
        let fix = fixture();
        let r_fix = testing::check(&fix, &caller, &limits, 100).await;
        assert!(r_fix.is_ok(), "fixture failed: {r_fix}");
        assert_eq!(r_fix.nodes(), 5);

        // K22: max_nodes = 2 stops early without violations.
        let r_cap = testing::check(&fix, &caller, &limits, 2).await;
        assert!(r_cap.is_ok(), "capped check failed: {r_cap}");
        assert!(r_cap.nodes() <= 2);

        // 10 000-entry directory
        let big = MemFs::new();
        for i in 0..10_000usize {
            big.create_file(big.root(), format!("f{i:05}").as_bytes(), b"x")
                .unwrap();
        }
        let r_big = testing::check(&big, &caller, &limits, 10_005).await;
        assert!(r_big.is_ok(), "10k dir failed: {r_big}");
        assert_eq!(r_big.nodes(), 10_001);

        // 256-deep chain
        let deep = MemFs::new();
        let mut cur = deep.root();
        for _ in 0..256 {
            cur = deep.mkdir(cur, b"d").unwrap();
        }
        let r_deep = testing::check(&deep, &caller, &limits, 300).await;
        assert!(r_deep.is_ok(), "256-deep chain failed: {r_deep}");
        assert_eq!(r_deep.nodes(), 257);
    });
}

// --- Deliberately broken backends for K01–K21 and KC2 ----------------------

struct BrokenK01 {
    inner: MemFs,
    n: AtomicU64,
}
impl Filesystem for BrokenK01 {
    fn info(&self) -> FsInfo {
        FsInfo::new(0, self.n.fetch_add(1, Ordering::Relaxed))
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK02 {
    inner: MemFs,
}
impl Filesystem for BrokenK02 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        let mut a = self.inner.getattr(c, id).await?;
        if id == self.inner.root() {
            a.kind = NodeKind::File;
        }
        Ok(a)
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK03 {
    inner: MemFs,
}
impl Filesystem for BrokenK03 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        if dir == self.inner.root() {
            return Ok(Attr::new(NodeId::new(999_999).unwrap(), NodeKind::Dir));
        }
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK04 {
    inner: MemFs,
}
impl Filesystem for BrokenK04 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        let mut a = self.inner.getattr(c, id).await?;
        if id != self.inner.root() {
            a.id = NodeId::new(888_888).unwrap();
        }
        Ok(a)
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK05 {
    inner: MemFs,
}
impl Filesystem for BrokenK05 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        let mut a = self.inner.getattr(c, id).await?;
        a.perm = 0o10_755;
        Ok(a)
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK06 {
    inner: MemFs,
}
impl Filesystem for BrokenK06 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        if dir == self.inner.root() {
            return Ok(DirPage::new(Vec::new(), false, 0));
        }
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK07 {
    inner: MemFs,
}
impl Filesystem for BrokenK07 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        let mut page = self.inner.read_dir(c, dir, cur, max, attrs).await?;
        if dir == self.inner.root() && page.entries.len() >= 2 {
            page.entries[1].name = page.entries[0].name.clone();
        }
        Ok(page)
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK08 {
    inner: MemFs,
}
impl Filesystem for BrokenK08 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        let mut page = self.inner.read_dir(c, dir, cur, max, attrs).await?;
        if dir == self.inner.root() && !page.entries.is_empty() {
            page.entries[0].cookie = Cookie::DOT;
        }
        Ok(page)
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK09 {
    inner: MemFs,
}
impl Filesystem for BrokenK09 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        if dir == self.inner.root() && !cur.cookie.is_start() {
            return Ok(DirPage::new(Vec::new(), true, 0));
        }
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK10 {
    inner: MemFs,
}
impl Filesystem for BrokenK10 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        if dir == self.inner.root() && cur.cookie == Cookie::DOT {
            return Ok(DirPage::new(Vec::new(), true, 0));
        }
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK11 {
    inner: MemFs,
}
impl Filesystem for BrokenK11 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        let mut page = self.inner.read_dir(c, dir, cur, max, attrs).await?;
        if !attrs {
            for e in &mut page.entries {
                e.attr = Some(Attr::new(e.id, e.kind));
            }
        }
        Ok(page)
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK12 {
    inner: MemFs,
}
impl Filesystem for BrokenK12 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        let mut a = self.inner.lookup(c, dir, n).await?;
        if n.as_bytes() == b"a.txt" {
            a.id = NodeId::new(777_777).unwrap();
        }
        Ok(a)
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK13 {
    inner: MemFs,
}
impl Filesystem for BrokenK13 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        let mut a = self.inner.parent(c, dir).await?;
        if dir != self.inner.root() {
            a.id = dir;
        }
        Ok(a)
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK14 {
    inner: MemFs,
}
impl Filesystem for BrokenK14 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await.map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                Error::from(ErrorKind::Stale)
            } else {
                e
            }
        })
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK15 {
    inner: MemFs,
}
impl Filesystem for BrokenK15 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await.map_err(|e| {
            if e.kind() == ErrorKind::NotADirectory {
                Error::from(ErrorKind::NotFound)
            } else {
                e
            }
        })
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK16 {
    inner: MemFs,
}
impl Filesystem for BrokenK16 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await.map_err(|e| {
            if e.kind() == ErrorKind::IsADirectory {
                Error::from(ErrorKind::InvalidInput)
            } else {
                e
            }
        })
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK17 {
    inner: MemFs,
}
impl Filesystem for BrokenK17 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        let res = self.inner.read(c, id, off, buf).await?;
        if !buf.is_empty() && res.len > 0 {
            return Ok(ReadResult::new(buf.len().saturating_add(17), false));
        }
        Ok(res)
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK18 {
    inner: MemFs,
    max_symlink_len: usize,
}
impl Filesystem for BrokenK18 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        let _ = self.inner.read_link(c, id).await?;
        Ok(vec![b'a'; self.max_symlink_len + 1])
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK19 {
    inner: MemFs,
}
impl Filesystem for BrokenK19 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn access(&self, c: &Caller, id: NodeId, _want: Access) -> Result<Access> {
        let _ = self.inner.getattr(c, id).await?;
        Ok(Access::all())
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenK20 {
    inner: MemFs,
}
impl Filesystem for BrokenK20 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        let _ = self.inner.getattr(c, id).await?;
        Err(Error::from(ErrorKind::Io))
    }
}

struct BrokenK21 {
    inner: MemFs,
}
impl Filesystem for BrokenK21 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await.map_err(|e| {
            if e.kind() == ErrorKind::Stale {
                Error::from(ErrorKind::NotFound)
            } else {
                e
            }
        })
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        self.inner.read(c, id, off, buf).await
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

struct BrokenKC2 {
    inner: MemFs,
    corrupted: AtomicBool,
}

struct FlipOnDrop<'a> {
    flag: &'a AtomicBool,
    armed: bool,
}

impl Drop for FlipOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::SeqCst);
        }
    }
}

impl Filesystem for BrokenKC2 {
    fn info(&self) -> FsInfo {
        self.inner.info()
    }
    fn root(&self) -> NodeId {
        self.inner.root()
    }
    async fn getattr(&self, c: &Caller, id: NodeId) -> Result<Attr> {
        self.inner.getattr(c, id).await
    }
    async fn lookup(&self, c: &Caller, dir: NodeId, n: Name<'_>) -> Result<Attr> {
        self.inner.lookup(c, dir, n).await
    }
    async fn parent(&self, c: &Caller, dir: NodeId) -> Result<Attr> {
        self.inner.parent(c, dir).await
    }
    async fn read(&self, c: &Caller, id: NodeId, off: u64, buf: &mut [u8]) -> Result<ReadResult> {
        let mut guard = FlipOnDrop {
            flag: &self.corrupted,
            armed: true,
        };
        yield_now().await;
        guard.armed = false;
        let res = self.inner.read(c, id, off, buf).await?;
        if self.corrupted.load(Ordering::SeqCst) {
            for b in &mut buf[..res.len] {
                *b ^= 0xFF;
            }
        }
        Ok(res)
    }
    async fn read_dir(
        &self,
        c: &Caller,
        dir: NodeId,
        cur: DirCursor,
        max: usize,
        attrs: bool,
    ) -> Result<DirPage> {
        self.inner.read_dir(c, dir, cur, max, attrs).await
    }
    async fn read_link(&self, c: &Caller, id: NodeId) -> Result<Vec<u8>> {
        self.inner.read_link(c, id).await
    }
    async fn statfs(&self, c: &Caller, id: NodeId) -> Result<FsStat> {
        self.inner.statfs(c, id).await
    }
}

#[test]
fn t64_testing_check_broken_backends() {
    let caller = Caller::new(0, 0);
    let limits = Limits::new();

    async fn expect_rule<F: Filesystem>(fs: &F, caller: &Caller, limits: &Limits, rule: &str) {
        let rep = testing::check(fs, caller, limits, 100).await;
        assert!(
            !rep.is_ok(),
            "expected violation {rule}, but report was ok: {rep}"
        );
        assert!(
            rep.violations().iter().any(|v| v.rule == rule),
            "expected rule {rule} in violations, got: {rep}"
        );
    }

    block_on(async {
        // Plan item 64 required broken backends:
        expect_rule(&BrokenK17 { inner: fixture() }, &caller, &limits, "K17").await;
        expect_rule(&BrokenK06 { inner: fixture() }, &caller, &limits, "K06").await;
        expect_rule(&BrokenK07 { inner: fixture() }, &caller, &limits, "K07").await;
        expect_rule(&BrokenK08 { inner: fixture() }, &caller, &limits, "K08").await;
        expect_rule(&BrokenK09 { inner: fixture() }, &caller, &limits, "K09").await;
        expect_rule(&BrokenK11 { inner: fixture() }, &caller, &limits, "K11").await;
        expect_rule(&BrokenK04 { inner: fixture() }, &caller, &limits, "K04").await;
        expect_rule(&BrokenK03 { inner: fixture() }, &caller, &limits, "K03").await;
        expect_rule(&BrokenK21 { inner: fixture() }, &caller, &limits, "K21").await;
        expect_rule(
            &BrokenK18 {
                inner: fixture(),
                max_symlink_len: limits.max_symlink_len,
            },
            &caller,
            &limits,
            "K18",
        )
        .await;

        // Additional kit rules coverage (K01, K02, K05, K10, K12–K16, K19, K20, KC2):
        expect_rule(
            &BrokenK01 {
                inner: fixture(),
                n: AtomicU64::new(1),
            },
            &caller,
            &limits,
            "K01",
        )
        .await;
        expect_rule(&BrokenK02 { inner: fixture() }, &caller, &limits, "K02").await;
        expect_rule(&BrokenK05 { inner: fixture() }, &caller, &limits, "K05").await;
        expect_rule(&BrokenK10 { inner: fixture() }, &caller, &limits, "K10").await;
        expect_rule(&BrokenK12 { inner: fixture() }, &caller, &limits, "K12").await;
        expect_rule(&BrokenK13 { inner: fixture() }, &caller, &limits, "K13").await;
        expect_rule(&BrokenK14 { inner: fixture() }, &caller, &limits, "K14").await;
        expect_rule(&BrokenK15 { inner: fixture() }, &caller, &limits, "K15").await;
        expect_rule(&BrokenK16 { inner: fixture() }, &caller, &limits, "K16").await;
        expect_rule(&BrokenK19 { inner: fixture() }, &caller, &limits, "K19").await;
        expect_rule(&BrokenK20 { inner: fixture() }, &caller, &limits, "K20").await;

        let kc2 = BrokenKC2 {
            inner: fixture(),
            corrupted: AtomicBool::new(false),
        };
        let rep_kc2 = testing::check_cancellation(&kc2, &caller, &limits, 100).await;
        assert!(
            rep_kc2.violations().iter().any(|v| v.rule == "KC2"),
            "expected KC2 violation, got: {rep_kc2}"
        );
    });
}

#[test]
fn t65a_report_assert_ok_succeeds_on_memfs() {
    let fs = fixture();
    let rep = block_on(testing::check(&fs, &Caller::new(0, 0), &Limits::new(), 100));
    rep.assert_ok();
}

#[test]
#[should_panic(expected = "vfs::testing:")]
fn t65b_report_assert_ok_panics_on_broken_backend() {
    let fs = BrokenK03 { inner: fixture() };
    let rep = block_on(testing::check(&fs, &Caller::new(0, 0), &Limits::new(), 100));
    rep.assert_ok();
}

// --- Concurrency (66–67) ---------------------------------------------------

#[test]
fn t66_concurrent_reads_and_mutations() {
    let fs = Arc::new(fixture());
    let done = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();

    for tid in 0..4usize {
        let fs = Arc::clone(&fs);
        let done = Arc::clone(&done);
        handles.push(thread::spawn(move || {
            let caller = Caller::new(0, 0);
            let limits = Limits::new();
            let root = fs.root();
            let mut iters = 0usize;
            block_on(async {
                while !done.load(Ordering::Relaxed) || iters < 20 {
                    iters += 1;
                    let _ = fs.getattr(&caller, root).await.unwrap();
                    let _ = vfs::resolve(&*fs, &caller, b"/dir/b.bin", &limits).await;

                    // Full directory listing must have unique names and unique cookies >= FIRST.
                    let mut cursor = DirCursor::START;
                    let mut seen_names = HashSet::new();
                    let mut seen_cookies = HashSet::new();
                    loop {
                        let page = fs.read_dir(&caller, root, cursor, 16, false).await.unwrap();
                        for e in &page.entries {
                            assert!(e.cookie >= Cookie::FIRST);
                            assert!(seen_names.insert(e.name.clone()));
                            assert!(seen_cookies.insert(e.cookie));
                            if e.kind == NodeKind::File {
                                let mut buf = [0u8; 32];
                                if let Ok(rr) = fs.read(&caller, e.id, 0, &mut buf).await {
                                    assert!(rr.len <= buf.len());
                                }
                            }
                        }
                        if page.eof {
                            break;
                        }
                        cursor = DirCursor::new(page.entries.last().unwrap().cookie, page.verifier);
                    }
                    if iters % 10 == 0 {
                        let _ = tid;
                        thread::yield_now();
                    }
                }
            });
        }));
    }

    let root = fs.root();
    for i in 0..1000usize {
        let nm = format!("tmp_{i}");
        let id = fs.create_file(root, nm.as_bytes(), b"payload").unwrap();
        let _ = fs.set_times(id, SystemTime::now(), SystemTime::now());
        fs.remove(root, nm.as_bytes()).unwrap();
    }
    done.store(true, Ordering::Relaxed);

    for h in handles {
        h.join().expect("reader thread panicked");
    }
}

#[test]
fn t67_concurrent_create_file_race() {
    let fs = Arc::new(MemFs::new());
    let root = fs.root();
    let initial_nodes = fs.node_count();
    let barrier = Arc::new(Barrier::new(4));

    let mut handles = Vec::new();
    for i in 0..4usize {
        let fs = Arc::clone(&fs);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            fs.create_file(root, b"race.txt", format!("t{i}").as_bytes())
        }));
    }

    let mut ok_count = 0usize;
    let mut exists_count = 0usize;
    for h in handles {
        match h.join().expect("thread panicked") {
            Ok(_) => ok_count += 1,
            Err(e) => {
                assert_eq!(e.kind(), ErrorKind::AlreadyExists);
                exists_count += 1;
            }
        }
    }
    assert_eq!(ok_count, 1);
    assert_eq!(exists_count, 3);
    assert_eq!(fs.node_count(), initial_nodes + 1);
}
