# vfs — design record

Status: **G1 proposal** (2026-11-26). §Purpose, §Non-goals, §Namespace, §Equivalents, §Dependencies, and §Key design questions are the G1 deliverable. §References and §API sketch front-load G2 research, the way `xdr` did. They are **non-binding** until the human answers the questions below and approves the API at G2.

## Purpose

`akku::vfs` is a protocol-neutral, async **virtual filesystem trait layer**. Storage backends implement it, and protocol servers consume it. It is the seam between "where the bytes live" and "how a client reaches them".

**Primary driver (megaghost).** megaghost mounts a MEGA cloud drive on macOS. It serves the drive over NFSv3 from `akku::nfs::server` on localhost, and `akku::os::macos` mounts it. The data path is:

```text
macOS NFS client ──TCP──▶ akku::nfs::server ──▶ akku::vfs::Filesystem ──▶ backend
                     (akku::net + akku::xdr,                         ├─ MEGA remote tree (megaghost crate)
                      ONC-RPC internal)                              ├─ vfs::MemFs (tests, fixtures, scratch)
                                                                     └─ LocalFs passthrough (later)
```

The TODO order is `xdr` ✓ → `net` ✓ → **`vfs`** → `nfs::server` (read-only first) ([`TODO.md`](../../../../../TODO.md) §0.9).

**Who uses it:**
- **Servers:** `akku::nfs::server` first. Possible later servers are 9P, WebDAV, or FUSE-like ones. Each translates its wire operations into `vfs` calls and maps `vfs::Error` back to its wire status codes.
- **Backends:** megaghost's MEGA backend, which lives outside akku. It is network-bound: node tree, encrypted chunk fetch, quota. The in-module test and fixture backend is `vfs::MemFs`. A local-directory passthrough comes later (Q8).

**Why a separate module instead of a trait inside `nfs`:**
1. [`Namespace.md`](../../../../../Namespace.md) already lists `akku::vfs` as its own flat module ("trait layer that backends and servers plug into"), with `akku::nfs` "over `vfs`".
2. **Protocol neutrality.** The closest prior art, `nfsserve`'s `NFSFileSystem`, puts NFS wire types into the trait (`nfsstat3`, `fattr3`, `sattr3`, `filename3`, `fileid3`; §References). That makes the trait unusable for 9P or WebDAV and leaks XDR details into every backend. `vfs` owns its own neutral types, and the NFS mapping lives in `nfs::server`.
3. **Testability.** Backends can be tested without sockets, and servers can be tested against `MemFs` without MEGA.
4. **One audit boundary.** Name validation (no `/`, NUL, `.`, `..`), DoS limits, and node-identity rules sit in one place. Each server does not re-invent them.

## Non-goals

- **Not a FUSE binding, kernel driver, or mount helper.** Mounting is `akku::os::macos` or megaghost's job. `vfs` never talks to a kernel.
- **Not a replacement for `std::fs` or `akku::fs`, and not a `std` superset.** `vfs` re-exports nothing from `std::fs` and does not try to be the filesystem API for application code. A Go-`io/fs`-style path convenience layer is deferred (`_dev/TODO.md`).
- **No protocol wire types.** No `nfsstat3`, `fattr3`, XDR, or file-handle byte layout inside `vfs`. `nfs::server` builds its 64-byte `nfs_fh3` from `vfs` identity (Q2) and maps `vfs::ErrorKind` to `NFS3ERR_*` (Q6).
- **No caching policy in v1.** RFC 1813 §4.11 defines no caching policy (§References). Backends own their caches: the MEGA backend needs its own chunk and metadata cache anyway. Attribute and lookup caching is the NFS client kernel's job. An optional `Cached<F>` wrapper is deferred (`_dev/TODO.md`). The human confirms this in Q13.
- **No MEGA backend in akku.** MEGA's API, crypto (AES, rule 10), and account handling belong in megaghost. akku ships only the trait and generic backends.
- **No byte-range locks (NLM), xattrs, ACLs, device nodes (`MKNOD`/`rdev`), or change notifications in v1.** All are deferred (`_dev/TODO.md`).
- **No synchronous trait and no blocking backend in v1.** Every consumer is an async server on `akku::task` (Q1). A blocking `std::fs` passthrough would stall the single-threaded Phase 1 runtime, which has no `spawn_blocking` yet (Q8).
- **No permission-enforcement policy in the core.** `vfs` passes the caller's identity to the backend (Q10). Only the backend decides access, and a default `access` derived from mode bits is offered.
- **No `unsafe`.** Pure safe `std`. The crate is `#![deny(unsafe_code)]`, and `vfs` needs no allowlist entry in v1.

## Namespace

`akku::vfs` is a flat level-1 module, already listed in [`Namespace.md`](../../../../../Namespace.md) under *Network, transport & storage protocols*:

> `akku::vfs` | Virtual filesystem trait layer that backends and servers plug into (Wave 2, megaghost-driven)

The row is accurate. **No `Namespace.md` edit is needed**: all proposed names are module-internal (rule 2). `vfs` is not a Rust keyword, does not shadow `::core` (rule 6), and is not a `std` module name, so the `std`-superset rules of `Namespace.md` §3 do not apply.

**Proposed public items (v1, pending Q1–Q13):**
- Traits: `Filesystem` (read side, required), `DynFilesystem` (dyn-compatible boxed-future twin, blanket-implemented). Later: `FilesystemWrite: Filesystem` (Q3).
- Identity and attributes: `NodeId`, `NodeKind`, `Attr`, `FsInfo`, `FsStat`.
- Directories: `Entry`, `DirCursor`, `DirPage`.
- Names and callers: `Name<'a>` (validated borrowed name bytes), `Caller`, `Access` (bit mask).
- I/O results: `ReadResult`.
- Errors and limits: `Error`, `ErrorKind`, `Result`, `Limits`.
- Helpers: `BoxFuture<'a, T>` (alias), `resolve` (path → `NodeId` walk with depth limit), `read_dir_all` (bounded convenience).
- Backend: `MemFs` (Q8).

**Relation to `std::fs` names.** `vfs` is not a superset, so it may reuse `std` names. It deliberately does **not**, because users routinely import `akku::fs` (which re-exports `std::fs::{Metadata, FileType, DirEntry, ReadDir}`) in the same file:

| `std::fs` name | `vfs` name | Reason |
|---|---|---|
| `Metadata` | `Attr` | Matches the `getattr` vocabulary of NFS (`fattr3`), FUSE (`FileAttr`), and `nfsserve`. Carries `NodeId`, generation, and change counter, which `Metadata` lacks. |
| `FileType` | `NodeKind` | A closed, protocol-neutral enum, not a platform bitfield. |
| `DirEntry`, `ReadDir` | `Entry`, `DirPage` | One page of a cookie-paged listing, not a lazy OS iterator (Q5). |
| `File` | none | v1 has no open-file object. NFSv3 is stateless (RFC 1813 §1), so reads are `(NodeId, offset)` calls (Q2). |

**Trait name and `Async` prefix.** The `Async` prefix rule (`Namespace.md` §3 (a)) exists so async items never take a `std` name inside a `std`-superset module. `vfs` is neither a superset nor has a sync twin, so the proposal is the plain `vfs::Filesystem` (as `fuser::Filesystem`). Alternatives are `vfs::Fs` (Go's `fs.FS`) or `vfs::AsyncFilesystem` (pre-reserves a sync twin). See Q1 (d).

`Node`, `Context`, `Handler`, and `Error` are reserved generic names (`Namespace.md` §5). They may appear only inside a module. `vfs` uses `Error` and `Result` in-module only. It avoids `vfs::Node` and `vfs::Context`: the latter would also be confused with `std::task::Context` in poll code, so the per-call identity is `Caller`.

## Equivalents

| Ecosystem | Item | Model |
|---|---|---|
| Go stdlib | `io/fs`: `fs.FS` (`Open`), `fs.File`, `fs.ReadDirFS`, `fs.StatFS`, `fs.ReadLinkFS`, `fs.Sub`, `fs.ValidPath`; `testing/fstest.MapFS` | Path-based (slash-separated, UTF-8), read-only, sync, capability-by-interface-upgrade |
| Go x/net | `golang.org/x/net/webdav.FileSystem` (`Mkdir`, `OpenFile`, `RemoveAll`, `Rename`, `Stat`, all with `context.Context`), `webdav.Dir`, in-memory `memFS` | Path-based read-write, per-call context |
| Go stdlib | `os.Root` / `os.OpenInRoot` (traversal-safe directory confinement) | Local passthrough security reference |
| Rust std | `std::fs` (`metadata`, `read_dir`, `File`), `std::os::unix::fs::MetadataExt::{ino, dev}`; no VFS trait | Path-based OS API |
| Rust crates | `nfsserve` `NFSFileSystem` (async via `async_trait`, `fileid3`-keyed: `lookup`/`getattr`/`setattr`/`read`/`write`/`create`/`create_exclusive`/`mkdir`/`remove`/`rename`/`readdir`/`symlink`/`readlink`) | **Id-based**, NFS-typed, closest prior art |
| Rust crates | `fuser` `Filesystem` (inode-keyed `lookup`/`forget`/`getattr`/`readdir(offset)`/…, callback `Reply*` objects) | Id-based, FUSE low-level |
| Rust crates | `vfs` (rust-vfs) `FileSystem` / `AsyncFileSystem`; `virtual-filesystem` `FileSystem` (+ `MountableFS`, `ScopedFS`) | Path-based, `&str`, boxed file objects |
| Rust crates | `opendal` `Service` (RPITIT futures + `MaybeSend`) and `ServiceDyn` (boxed futures, blanket impl) | Path-based object storage. **Async trait pattern reference.** |
| C / Linux | Linux VFS (`inode`, `dentry`, `inode_operations::lookup`) + `export_operations` (`encode_fh` / `fh_to_dentry`, inode + generation) | Id-based kernel VFS with stable export handles |
| Plan 9 | 9P (`walk`, `fid`, `qid.path` unique and never reused, `qid.version`) | Id-based (qid), fid-per-client |
| MEGA | `meganz/sdk` `handle` (`uint64_t`), FUSE `InodeID : FileID` from `NodeHandle` | Backend identity is a stable 64-bit handle |
| Protocol | NFSv3 (RFC 1813): 22 procedures, `fattr3`, `nfs_fh3<64>`, `cookie3` + `cookieverf3`, `nfsstat3` | The first server's contract |

## Dependencies

`native`: zero external dependencies (`std` only), `#![deny(unsafe_code)]`, 0 `unsafe` blocks, `wasm32 = true` (traits and `MemFs` are pure computation).

- **`std`:** `core::future::Future`, `core::pin::Pin`, `std::time::{SystemTime, Duration}` (also re-exported unchanged as `akku::time::{SystemTime, Duration}`), `std::num::NonZeroU64`, `std::sync::{Arc, RwLock}` (for `MemFs`).
- **`akku::task`:** the runtime that `nfs::server` and megaghost run on. `vfs` itself needs no runtime: `MemFs` futures complete without waiting, and tests drive them with `akku::task::block_on`. The `Send` bound on every `vfs` future (Q1) exists because `akku::task::spawn` requires `F: Future + Send + 'static` (`crates/akku/src/task/mod.rs:1226-1229`).
- **`akku::io`:** not needed in v1. `read` fills a caller buffer (Q12). An `AsyncRead`/`AsyncSeek` adapter over `(fs, NodeId)` is deferred (`_dev/TODO.md`). `akku::io::AsyncRead` is dyn-compatible poll-style (`crates/akku/src/io/mod.rs:120-123`), and Q1 explains why `vfs` does not copy that style.
- **`akku::bytes`:** optional zero-copy alternative for `read` (Q12). It is not used if the recommendation is accepted.
- **No `async-trait`, `futures`, or `trait-variant`.** The boxed-future dyn shim is hand-written with `Pin<Box<dyn Future + Send + 'a>>`, the pattern `async-trait` generates. AGENTS.md rule 8 applies.

## Key design questions for the human (G1)

Each question lists options and a **recommendation**. §API sketch shows the recommendations combined.

### Q1. Async vs sync trait, and how to spell async (Rust 1.85 MSRV)

The MEGA backend is network-bound. `nfs::server` runs on `akku::task`, spawns per-connection tasks, and must stay responsive while one READ waits for MEGA. So the trait must be **async**. Sync is rejected: a sync backend would block the single-threaded Phase 1 runtime. The open question is how to spell the async methods:

- **(A) Poll-style** (`fn poll_lookup(self: Pin<&mut Self>, cx, …) -> Poll<…>`, as `akku::io::AsyncRead`): dyn-compatible without allocation, but every backend must hand-write a state machine for multi-step work (HTTP request → decrypt → cache insert). That is unacceptable for the MEGA backend's ergonomics and auditability.
- **(B) `async fn` / RPITIT only** (`fn lookup(&self, …) -> impl Future<Output = Result<Attr>> + Send`): stable since Rust 1.75, within MSRV 1.85. Implementors may write `async fn`. Zero allocation, static dispatch. **Not dyn-compatible**, so `Arc<dyn Filesystem>` is impossible.
- **(C) Boxed futures only** (`fn lookup<'a>(&'a self, …) -> BoxFuture<'a, Result<Attr>>`, the hand-written equivalent of `async-trait`): dyn-compatible, one heap allocation per call, implementors must `Box::pin(async move { … })` by hand.
- **(D) B + C**: a static `Filesystem` trait with RPITIT `+ Send` futures, plus a dyn-compatible `DynFilesystem` twin with boxed futures, blanket-implemented for every `F: Filesystem`, and `Filesystem` implemented for `Arc<dyn DynFilesystem>`. This is exactly `opendal`'s `Service` / `ServiceDyn` / `Servicer` design (§References). Backends write `async fn` once. Generic servers pay nothing. Composition (mount tables, overlays, runtime backend selection in megaghost) uses `Arc<dyn DynFilesystem>` and pays one box per call.

Sub-points:
- (a) **`Send` is fixed in the trait.** Return-type notation (`F::lookup(..): Send`) is still unstable (rust-lang/rust#109417, open), so callers cannot add the bound later. The Rust blog recommends deciding it up front for public traits (§References). `akku::task::spawn` requires `Send`, so the futures are `+ Send` and `Filesystem: Send + Sync + 'static`.
- (b) **Cancellation safety.** A server drops a future when a client disconnects. The contract: dropping any `vfs` future at any await point leaves the filesystem consistent. This is pinned by G3 tests on `MemFs`.
- (c) **wasm32** is Tier 2. Fixed `Send` costs nothing on Tier 1. `opendal`'s `MaybeSend` cfg trick is not needed.
- (d) **Naming:** plain `Filesystem` (no `Async` prefix), because `vfs` is async-only and not a `std` superset.

**Recommendation: (D)**, with fixed `+ Send` futures and the trait named `vfs::Filesystem` / `vfs::DynFilesystem`.

### Q2. Node identity: ids vs paths, persistence, generations

NFSv3 clients hold opaque file handles (≤ 64 bytes) indefinitely and expect them to keep working across server restarts. RFC 1813 §1 (stateless server) and Linux `exporting.rst` ("stable across renames, truncates, and server-reboot") both say so. `nfsserve` embeds the server start time in every handle (`vfs.rs:69-75,221-242`), so **every megaghost restart would invalidate every handle the macOS client holds** (`NFS3ERR_STALE` everywhere, Finder windows break).

- **(A) Path-based** (Go `io/fs`, WebDAV, rust-vfs, opendal): simple, but a path is not stable across rename. An NFS server would need a persistent path ↔ id table, which is what `nfsserve`'s mirrorfs builds in memory (`id_to_path`/`path_to_id`).
- **(B) Opaque `NodeId(NonZeroU64)` plus a `generation: u64` in `Attr`**: `lookup(dir, name) -> Attr` (which carries the id), and every other op takes a `NodeId`. The contract: an id is stable for the node's lifetime, the pair `(id, generation)` never names a different object, and ids survive restarts iff `FsInfo::persistent_ids`. Id `0` is excluded because RFC 1813 says clients treat fileid 0 specially (READDIR implementation note). This matches Linux (inode + generation, `FILEID_INO32_GEN`), 9P (`qid.path`, never reused), FUSE (`ino`), and MEGA (64-bit node `handle`; MEGA's own FUSE layer derives `InodeID` from `NodeHandle`).
- **(C) Backend-defined opaque handle bytes** (≤ 48 bytes, Linux `encode_fh` style): maximally flexible, but every server must carry variable bytes, and `MemFs` gains nothing.

`nfs::server` then builds `nfs_fh3` as `[version | fsid | NodeId | generation | (later: MAC)]`. That is server policy, not `vfs`.

**Recommendation: (B)**, plus a `vfs::resolve(fs, caller, path, limits)` helper that walks `lookup` for path-based users. `Name` rejects `.` and `..`. A dedicated `parent(dir) -> NodeId` op serves NFS `LOOKUP ".."` (RFC 1813 §3.2 item 3), and the root's parent is the root. (C) is deferred.

### Q3. Read-only first: split traits vs one trait with `Unsupported` defaults

- **(A) One trait, write methods defaulting to `Err(ReadOnly)`, plus a capabilities flag** (`nfsserve` `VFSCapabilities`, fuser `ENOSYS` defaults, rust-vfs `NotSupported` defaults): one server type and one dyn type. But a forgotten override fails silently at runtime, and the read-only server still carries the whole write surface.
- **(B) Split: `Filesystem` (read) + `FilesystemWrite: Filesystem` (write)**: type-checked, so a read-only server takes `F: Filesystem` and nothing else. The write trait is purely additive later. Cost: a second dyn twin (`DynFilesystemWrite`) and a second server constructor when writes land.

**Recommendation: (B). v1 ships only `Filesystem`.** `FilesystemWrite` is designed at G2 (so the read types already carry what WRITE/CREATE need: `change` for wcc data, a write verifier slot in `FsInfo`) but lands additively together with `nfs::server` write support. `MemFs` exposes inherent build methods (`create_file`, `mkdir`, `symlink`, `write_all`) in v1 so tests and fixtures can populate trees.

### Q4. Attribute model

- **(A) Mirror `fattr3` exactly** (type, mode, nlink, uid, gid, size, used, rdev, fsid, fileid, atime/mtime/ctime as `nfstime3` = u32 s + u32 ns): an easy NFS mapping, but NFS-shaped. The u32 seconds field cannot represent times before 1970 or after 2106, and per-node `fsid` is redundant.
- **(B) Neutral model with `Option` fields everywhere:** flexible, but every server must invent defaults, and RFC 1813 says servers "should support this set of attributes even if they have to simulate some of the fields" (fattr3 section).
- **(C) Neutral, complete, mostly non-optional:** `Attr { id, generation, kind: NodeKind {File, Dir, Symlink, Other}, perm: u32 (masked to 0o7777), nlink: u32, uid: u32, gid: u32, size: u64, used: u64, atime, mtime, ctime: SystemTime, change: u64 }`. `#[non_exhaustive]`, built with `Attr::new(id, kind)`, which documents the synthesized defaults (`perm` 0o644 for files and 0o755 for dirs, `nlink` 1 or 2, uid/gid 0, times `UNIX_EPOCH`, `change` 0). `fsid` lives once on `FsInfo`. `change` is a monotonic per-node change counter (9P `qid.version`, NFSv4 change attribute) that servers use for cookie verifiers and wcc data. `rdev` and birth time are deferred.

**Recommendation: (C).** `nfs::server` clamps `SystemTime` into `nfstime3`.

### Q5. Directory listing: cookie paging vs iterator

- **(A) Iterator or stream of all entries** (Go `ReadDir(n)`, rust-vfs `Box<dyn Iterator>`): natural for local code, but NFS READDIR is stateless and resumable at any cookie across separate RPCs. A per-client open iterator would be server state the client never closes.
- **(B) `start_after: fileid`** (`nfsserve`): breaks when two names in one directory share a fileid (hard links), and forces an O(n) scan to resume.
- **(C) Opaque per-entry cookie + directory verifier:** `read_dir(caller, dir, DirCursor { cookie, verifier }, max_entries, with_attrs) -> DirPage { entries: Vec<Entry { name, id, kind, cookie, attr: Option<Attr> }>, eof, verifier }`. This maps 1:1 onto READDIR and READDIRPLUS (RFC 1813 §3.3.16–17), 9P directory reads (`offset`), and FUSE `readdir(offset)`. The backend chooses cookies (index, hash, name-derived) and may return verifier `0`, meaning "cookies always valid", as RFC 1813 allows. A stale cursor yields `ErrorKind::BadCookie`. `.` and `..` are never returned: servers synthesize them. The order is stable between calls if the directory is unchanged. Fitting entries into READDIR's byte budget (`dircount`/`maxcount`) is the server's job, not the backend's.

**Recommendation: (C)**, plus a bounded `vfs::read_dir_all` helper for non-NFS users. Note that `nfsserve`'s Mac-client experience argues for lenient verifiers (§References): `MemFs` uses verifier `0` with stable, name-ordered cookies.

### Q6. Error model

- **(A) Raw `nfsstat3`** (`nfsserve`): couples every backend to NFS.
- **(B) Plain `std::io::Error`:** universal, but `io::ErrorKind` at MSRV 1.85 lacks stale-cookie, try-again-later (NFS `JUKEBOX`), and name-too-long kinds (`InvalidFilename` is unstable), and servers would need to downcast.
- **(C) Own `vfs::Error { kind: ErrorKind, message, source: Option<Box<dyn std::error::Error + Send + Sync>> }`** with a `#[non_exhaustive] enum ErrorKind`, plus `From<vfs::Error> for io::Error` and `From<io::Error> for vfs::Error`.

Proposed kinds and the intended mappings (the NFS column is implemented in `nfs::server`, not `vfs`):

| `vfs::ErrorKind` | NFSv3 (RFC 1813 §2.6) | `io::ErrorKind` (Rust 1.85) |
|---|---|---|
| `NotFound` | `NOENT` 2 | `NotFound` |
| `NotPermitted` | `PERM` 1 | `PermissionDenied` |
| `PermissionDenied` | `ACCES` 13 | `PermissionDenied` |
| `AlreadyExists` | `EXIST` 17 | `AlreadyExists` |
| `NotADirectory` | `NOTDIR` 20 | `NotADirectory` (1.83) |
| `IsADirectory` | `ISDIR` 21 | `IsADirectory` (1.83) |
| `DirectoryNotEmpty` | `NOTEMPTY` 66 | `DirectoryNotEmpty` (1.83) |
| `InvalidInput` | `INVAL` 22 | `InvalidInput` |
| `InvalidName` | `ACCES` 13 (RFC 1813 §3.2 item 5) | `InvalidInput` (`InvalidFilename` unstable) |
| `NameTooLong` | `NAMETOOLONG` 63 | `InvalidInput` |
| `ReadOnly` | `ROFS` 30 | `ReadOnlyFilesystem` (1.83) |
| `NoSpace` | `NOSPC` 28 | `StorageFull` (1.83) |
| `QuotaExceeded` | `DQUOT` 69 | `QuotaExceeded` (1.85) |
| `FileTooLarge` | `FBIG` 27 | `FileTooLarge` (1.83) |
| `CrossesDevices` | `XDEV` 18 | `CrossesDevices` (1.85) |
| `TooManyLinks` | `MLINK` 31 | `TooManyLinks` (1.83) |
| `Stale` | `STALE` 70 | `StaleNetworkFileHandle` (1.83) |
| `BadCookie` | `BAD_COOKIE` 10003 | `InvalidInput` |
| `Unsupported` | `NOTSUPP` 10004 | `Unsupported` |
| `TryAgain` | `JUKEBOX` 10008 | `ResourceBusy` (1.83) |
| `TimedOut` | `JUKEBOX` 10008 (GUESS: retry is better than `IO` for a slow MEGA fetch) | `TimedOut` |
| `Io` | `IO` 5 | any other / `Other` |

`TryAgain` matters for megaghost. RFC 1813 defines `JUKEBOX` for "hierarchical storage … file that has been migrated", which is exactly an uncached MEGA chunk. The client waits and retries instead of failing the read.

**Recommendation: (C).** `vfs::Error` is `Send + Sync + 'static`, implements `std::error::Error`, and has `Error::new(kind, msg)`, `Error::kind()`, and `Error::with_source(...)`.

### Q7. DoS limits and defaults

A `vfs::Limits` struct follows the `xdr::Limits` / `task::Limits` pattern (const builder `with_*`, `Default`). Servers clamp client requests before calling a backend. `vfs` helpers and `MemFs` enforce limits themselves, and backends must never return more than asked. Proposed defaults:

| Limit | Default | Rationale |
|---|---|---|
| `max_read_len` | 1 MiB | `nfsserve` advertises `rtmax = 1 MiB` (`vfs.rs:204`). GUESS: macOS and Linux NFS clients cap rsize at 1 MiB. |
| `max_write_len` | 1 MiB | Same, for `wtmax` (used once writes land). |
| `max_dir_entries` | 4 096 per `read_dir` page | Bounds `Vec<Entry>` per call. NFS byte budgets usually stop earlier. |
| `max_name_len` | 255 bytes | GUESS: `NAME_MAX` on Linux and macOS. Reported via `FsInfo` / PATHCONF `name_max`, `no_trunc = true` (RFC 1813 §3.2 item 4). |
| `max_path_len` | 4 096 bytes | GUESS: Linux `PATH_MAX`. Applies to `resolve` and symlink targets. |
| `max_path_depth` | 256 components | Bounds `resolve` walks. |
| `max_symlink_len` | 4 096 bytes | Bounds `read_link`. |
| `MemFs`: `max_nodes` | 1 048 576 | Memory bound for the fixture backend. |
| `MemFs`: `max_bytes` | 256 MiB total file data | Memory bound. Inherent writes beyond it fail with `NoSpace`. |

**Recommendation:** accept these defaults. G2 pins exact semantics (inclusive bounds, clamp vs error per limit).

### Q8. Which backends ship in-module in v1

- **(A) Traits only.**
- **(B) Traits + `MemFs`:** an in-memory tree behind `RwLock`, monotonic `NodeId`s that are never reused (`generation` 0), `persistent_ids = false`. It is used by `nfs::server` tests, megaghost tests, and fixtures.
- **(C) + `LocalFs`:** a passthrough over `std::fs` rooted at a directory. Tier-1 implications (AGENTS.md rule 11: all five Tier-1 platforms, native, in the same PR):
  1. `std::fs` is **blocking**. With no `spawn_blocking` until `task` Phase 4, it would stall the runtime.
  2. Stable `NodeId` needs inode identity. `MetadataExt::{ino, dev}` is stable on Linux, Android, macOS, and iOS. **On Windows, `MetadataExt::file_index` is still unstable** (`windows_by_handle`, rust-lang/rust#63010, open at 1.85), so Windows needs `GetFileInformationByHandle` FFI in an allowlisted `sys` submodule (`unsafe`, rule 9).
  3. Inode numbers are reused after delete, and `std` exposes no generation number (`st_gen`), so NFS handle staleness cannot be detected safely without a side table.
  4. Confinement: `webdav.Dir` documents that it does not stop symlink escapes (`file.go:60-62`). `virtual-filesystem`'s sandbox uses `canonicalize` + `starts_with`, which is a check-then-use race (`path_resolver.rs:13-29`). A safe `LocalFs` needs `openat`/`O_NOFOLLOW`-style resolution (Go `os.Root`, `root.go:35-53`) per platform, which is more `sys` work.
- **(D) + wrappers** (`ReadOnly<F>`, `Sub<F>`, overlays, mount tables).

**Recommendation: (B).** `LocalFs` is deferred to its own NewLib-style extension after `task` Phase 4 (`spawn_blocking`), with a native identity and confinement design for all five Tier-1 OSes. Wrappers and a Go-`fstest.TestFS`-style conformance kit are deferred too (`_dev/TODO.md`). One open option for the human: pull the conformance kit (`vfs::testing`) into v1 so megaghost can test its MEGA backend against the same rules.

### Q9. Wave, MSRV, and feature gating

- **Wave 2**, alongside `xdr`, `net`, `nfs` (`xdr` Decisions §8).
- **MSRV 1.85 / edition 2021** (`Cargo.toml:12-13`) is enough. RPITIT has been stable since 1.75. The `io::ErrorKind` variants used in Q6 are stable by 1.83 or 1.85. Return-type notation is unstable, so `Send` is fixed (Q1 a). `InvalidFilename` and `FilesystemLoop` are unstable and are not used.
- **Feature gate:** none. Zero-dep and pure computation, like `xdr`. `meta.toml`: `status = "native"`, `wasm32 = true`, `wave = 2`.

**Recommendation:** as stated. No new Cargo feature.

### Q10. Caller identity per call

NFS `AUTH_SYS` carries uid, gid, and gids on every call. FUSE passes `&Request`, WebDAV passes `ctx`, and opendal passes `&OperationContext`. `nfsserve` passes nothing.

- **(A) None:** the server enforces mode bits itself.
- **(B) `&Caller` on every op:** `Caller { uid, gid, groups }`, `#[non_exhaustive]` so a deadline or cancellation token can be added later without breaking the API. Backends may ignore it. A default `Filesystem::access(caller, id, Access) -> Result<Access>` evaluates POSIX mode bits from `getattr` (NFS ACCESS, RFC 1813 §3.3.4).
- **(C) Only an `access()` method.**

Adding a parameter later is breaking. **Recommendation: (B).** megaghost is single-user and can ignore it.

### Q11. Name type

- **(A) `&[u8]`:** raw, unchecked.
- **(B) `&str`** (Go, rust-vfs, opendal): NFS `filename3` is opaque bytes (`xdr` Decisions §1 routes filenames through `decode_opaque`), and macOS clients can send non-UTF-8 names.
- **(C) `&OsStr`** (fuser): platform-dependent, and not bytes on Windows.
- **(D) `vfs::Name<'a>`**, a `Copy` wrapper over `&'a [u8]` built only through `Name::new(bytes, &limits) -> Result<Name>`. It rejects empty names, `/`, NUL, `.`, `..`, and anything longer than `max_name_len`, and offers `to_str()` for UTF-8 backends. It is safe: a plain struct, not an unsized newtype (which would need `unsafe` casts).

**Recommendation: (D).** Backends receive only validated names. This is the adversarial-safety boundary for path traversal. A backend that needs UTF-8 (GUESS: MEGA node names are UTF-8) returns `InvalidName` for other bytes.

### Q12. `read` buffer ownership

- **(A) Return `Vec<u8>`** (`nfsserve`): one allocation per READ, and the backend picks the size.
- **(B) Caller buffer:** `read(caller, id, offset, buf: &mut [u8]) -> ReadResult { len, eof }`. No allocation, the length bound is structural (DoS-safe), and it matches `AsyncRead`. `nfs::server` reuses a per-connection buffer.
- **(C) Return `akku::bytes::Bytes`:** zero-copy from a backend cache, but it ties the public API to `akku::bytes`.

**Recommendation: (B).** (C) can be added as an optional method later if profiling shows copies matter.

### Q13. Caching policy

Confirm the §Non-goals decision: **no caching in the `vfs` core**. Backends own data and metadata caches, and an optional `vfs::Cached<F>` wrapper (TTL attr/lookup cache) is deferred. **Recommendation: confirm.**

## References (G2 research, front-loaded)

Pinned to the commits read on 2026-11-26.

- **RFC 1813: NFS Version 3** (<https://www.rfc-editor.org/rfc/rfc1813>, line numbers refer to `rfc1813.txt`)
  - §1 (`:467-475`): "The NFS version 3 protocol assumes a stateless server implementation … a client need only retry a request until the server responds". Handles and cookies must not depend on per-client server state.
  - §2.4 (`:791`, `:794`, `:802`): `NFS3_FHSIZE 64`, `NFS3_COOKIEVERFSIZE 8`, `NFS3_WRITEVERFSIZE 8`.
  - §2.5 `nfsstat3` (`:874-909`): `PERM 1 … REMOTE 71`, `BADHANDLE 10001 … JUKEBOX 10008`. `JUKEBOX` (`:1071-1080`): "not able to complete it in a timely fashion. The client should wait and then try … hierarchical storage … file that has been migrated".
  - `ftype3` (`:1083-1091`): REG, DIR, BLK, CHR, LNK, SOCK, FIFO. `nfs_fh3` (`:1129-1155`): opaque, ≤ 64 bytes, byte-compared by clients, and `NFS3ERR_STALE` once the object is gone or access is revoked.
  - `fattr3` (`:1189-1203`): type, mode, nlink, uid, gid, size, used, rdev, fsid, fileid, atime, mtime, ctime. `:1218-1219`: "All servers should support this set of attributes even if they have to simulate some of the fields." `sattr3` (`:1448-1455`): settable mode, uid, gid, size, atime, mtime.
  - §3.2 filenames (`:1634-1680`): empty names → `ACCES`; `.`/`..` handling; names longer than `name_max` with `no_trunc` → `NAMETOOLONG`; unhandleable characters → `EACCES`.
  - §3.3.0–§3.3.21 (`:1689-5123`): NULL, GETATTR, SETATTR, LOOKUP, ACCESS, READLINK, READ, WRITE, CREATE, MKDIR, SYMLINK, MKNOD, REMOVE, RMDIR, RENAME, LINK, READDIR, READDIRPLUS, FSSTAT, FSINFO, PATHCONF, COMMIT. Planned mapping onto `vfs`: read side `getattr`, `lookup`/`parent`, `access`, `read_link`, `read`, `read_dir`(`with_attrs`), `statfs`, `info`; write side (Q3) `setattr`, `write`, `create`, `mkdir`, `symlink`, `remove`, `rmdir`, `rename`, `link`, `commit`. `MKNOD` → `Unsupported`.
  - READDIR implementation note (`:4388-4419`): cookie + cookie verifier, `NFS3ERR_BAD_COOKIE`, "Servers in which directory cookies are always valid are free to use zero as the verifier always", and "servers should try to avoid sending a zero fileid".
  - WRITE `stable_how` (`:2701-2704`) UNSTABLE / DATA_SYNC / FILE_SYNC and COMMIT. These shape `FilesystemWrite` (Q3).
  - FSINFO `rtmax`/`wtmax`/`dtpref` (`:4859-4898`): requests above max yield short reads and writes.
  - §4.11 (`:5707-5717`): "does not define a policy for caching on the client or server".
- **Rust `nfsserve`** (`huggingface/nfsserve`, formerly `xetdata/nfsserve`) `@327b45530f04664da678297ce2568c47bf3fbf81`
  - `src/vfs.rs:94-107,111-193`: `#[async_trait] pub trait NFSFileSystem: Sync` with `lookup(dirid, &filename3) -> Result<fileid3, nfsstat3>`, `getattr`, `setattr(sattr3)`, `read(id, offset, count) -> (Vec<u8>, bool)`, `write`, `create`, `create_exclusive`, `mkdir`, `remove`, `rename`, `readdir(dirid, start_after, max_entries)`, `symlink`, `readlink`. NFS wire types throughout, and the external `async_trait` dependency.
  - `src/vfs.rs:61-65,97`: `VFSCapabilities { ReadOnly, ReadWrite }`, single trait, with write methods documented to return `NFS3ERR_ROFS` (`:113-151`).
  - `src/vfs.rs:69-75,221-242`: the file handle is generation (server start time in ms) + fileid, so handles go `NFS3ERR_STALE` on every restart. `:49-59` stores the generation in a `static mut` read through `unsafe`.
  - `src/vfs.rs:77-87,160-173`: readdir paginates by `start_after: fileid` with no cookie verifier, and the byte budget is the caller's problem. `:91`, `:106`: getattr and lookup "should be fast". `:93`: "The 0 fileid is reserved".
  - `src/vfs.rs:196-219`: default `fsinfo` advertises `rtmax = wtmax = 1 MiB` and `maxfilesize = 128 GiB`.
  - `src/nfs_handlers.rs:799-867`: the cookie verifier is derived from directory mtime, but the `BAD_COOKIE` check is commented out because the macOS client "pretty much expects" cookies to stay valid during concurrent modification.
  - `src/nfs_handlers.rs:869`: `args.maxcount as usize - 128` underflows for a client-sent `maxcount < 128` (panic in debug, huge budget in release). This is an adversarial-test idea for `nfs::server`, not `vfs`.
  - `examples/mirrorfs.rs:6,31-36`: the local mirror is Unix-only (`std::os::unix::ffi::OsStrExt`) and keeps in-memory `id_to_path`/`path_to_id` maps, so ids are not persistent.
- **Rust `fuser`** `cberner/fuser@8f85855c112721aa8dcad261f6498d3058287491`
  - `src/lib.rs:672`: `pub trait Filesystem: Send + Sync + 'static`. Methods take `&Request` and reply through callback objects (`ReplyEntry`, `ReplyAttr`, …), with `ENOSYS` defaults (`:696-700`).
  - `src/lib.rs:702-709`: `forget(ino, nlookup)`: kernel inode lookup refcounts. NFS has no equivalent, so `vfs` ids must stay valid without refcounts.
  - `src/lib.rs:241-273`: `FileAttr { ino, size, blocks, atime, mtime, ctime, crtime, kind, perm: u16, nlink, uid, gid, rdev, blksize, flags }` with `SystemTime` times.
  - `src/lib.rs:1077-1088`: `readdir(ino, fh, offset: u64, reply)`: offset-cookie paging, with `readdirplus` at `:1094`.
- **Rust `vfs` (rust-vfs)** `manuel-woelker/rust-vfs@9189433daabe961d4fa6905f665e923235a812db`
  - `src/filesystem.rs:9-15`: path-based `&str`, `/`-separated, components "any UTF-8 string, except "/", "." and ".."", `FileSystem: Debug + Sync + Send + 'static`.
  - `src/filesystem.rs:18,24-28`: `read_dir -> Box<dyn Iterator<Item = String> + Send>`, with boxed `SeekAndRead`/`SeekAndWrite` file objects.
  - `src/filesystem.rs:31-60`: optional ops default to `NotSupported`.
  - `src/error.rs:103-130`: `VfsErrorKind { IoError, FileNotFound, InvalidPath, Other(String), DirectoryExists, FileExists, NotSupported }`.
  - `src/async_vfs/filesystem.rs:7-9,20-21`: the async variant depends on `async-std` + `async-trait`.
  - Backends (`src/impls/`): `memory`, `physical`, `altroot`, `overlay`, `embedded`.
- **Rust `virtual-filesystem`** `MrElectrify/virtual-fs@ca1569e79d04d7022aac919e216e1c89def4bb34`
  - `src/lib.rs:25-37`: sync path-based `FileSystem` (`create_dir`, `metadata`, `open_file_options -> Box<dyn File>`, `read_dir -> Box<dyn Iterator<Item = Result<DirEntry>>>`, `remove_*`) over `io::Result` (`src/error.rs:3-4`). Mount, scoped, zip, and tar backends.
  - `src/physical_fs/path_resolver.rs:13-29`: `SandboxedPathResolver` uses `canonicalize()` then `starts_with(root)`. That is check-then-use (TOCTOU) against concurrent symlink swaps, a `LocalFs` lesson.
- **Rust `opendal`** `apache/opendal@e17226f489ecf5c452ff73254b60cb1aa785f7b9`
  - `core/core/src/raw/service.rs:123,155-176,255-261`: `pub trait Service: Send + Sync + Debug + Unpin + 'static` with methods returning `impl Future<Output = Result<…>> + MaybeSend` (RPITIT).
  - `core/core/src/raw/service.rs:297,310,318`: `ServiceDyn`, the dyn twin with `BoxedFuture<'a, Result<…>>`. `:391`: `pub type Servicer = Arc<dyn ServiceDyn>`. `:393-417`: blanket `impl<S: Service + ?Sized> ServiceDyn for S { … Box::pin(self.stat(…)) }`. `:500`: `impl<T: ServiceDyn + ?Sized> Service for Arc<T>`. This is the Q1 (D) pattern.
  - `core/core/src/raw/futures_util.rs:29-64`: `BoxedFuture` / `MaybeSend` switch to non-`Send` on `wasm32`.
- **Rust blog, "Announcing `async fn` and return-position `impl Trait` in traits"** (<https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits.html>; source `rust-lang/blog.rust-lang.org@c57feeee759361fa7d876ed75337ae549d6486d1` `content/async-fn-rpit-in-traits.md`)
  - `:12,34`: stable in Rust 1.75. `:98`: "Since users cannot add bounds later … you as a trait author need to make a choice". `:124`: "Traits that use `-> impl Trait` and `async fn` are not object-safe". `:182`: implementors may freely switch between the `async fn` and `-> impl Future` spellings.
  - Return type notation: tracking issue rust-lang/rust#109417, still open.
- **Rust `std` at 1.85.0** `rust-lang/rust@4d91de4e48198da2e33413efdcd9cd2cc0c46688`
  - `library/std/src/io/error.rs:274-300,334-378,395`: `NotADirectory`, `IsADirectory`, `DirectoryNotEmpty`, `ReadOnlyFilesystem`, `StaleNetworkFileHandle`, `StorageFull`, `FileTooLarge`, `ResourceBusy`, and `TooManyLinks` are stable since 1.83, and `QuotaExceeded` and `CrossesDevices` since 1.85. `FilesystemLoop` (`:293-294`) and `InvalidFilename` (`:377-378`) are still unstable (`io_error_more`, #86442).
  - `library/std/src/os/unix/fs.rs:425,442,459`: `MetadataExt::{dev, ino}` (stable inode identity on Unix-family Tier-1 targets).
  - `library/std/src/os/windows/fs.rs:453-472`: `volume_serial_number`, `number_of_links`, and `file_index` are `#[unstable(feature = "windows_by_handle", issue = "63010")]`.
- **Go `io/fs`** `golang/go@f9d574dc8740640da3f7a5654a47ea0b371b43b0`
  - `src/io/fs/fs.go:40-52`: `type FS interface { Open(name string) (File, error) }`, the minimal interface. Extra capabilities come through optional interfaces (`:34-36`).
  - `src/io/fs/fs.go:54-89`: `ValidPath`: UTF-8, slash-separated, no empty, `.`, or `..` elements (except the root `.`). Backslash is never a separator.
  - `src/io/fs/fs.go:130-151`: `ReadDirFile.ReadDir(n)` is a stateful iterator over an open directory file.
  - `src/io/fs/fs.go:156-166`: sentinel errors `ErrInvalid`, `ErrPermission`, `ErrExist`, `ErrNotExist`, `ErrClosed`. `:268-276`: `PathError { Op, Path, Err }`.
  - `src/io/fs/fs.go:175-182,189-214`: `FileInfo { Name, Size, Mode, ModTime, IsDir, Sys }` and `FileMode` type bits (`ModeDir`, `ModeSymlink`, …).
  - `src/io/fs/readdir.go:15-21`: `ReadDirFS` returns entries sorted by filename. `src/io/fs/stat.go:8-14`: `StatFS`. `src/io/fs/readlink.go:9-20`: `ReadLinkFS { ReadLink, Lstat }`. `src/io/fs/sub.go:13,37`: `SubFS` / `Sub`.
  - `src/testing/fstest/mapfs.go:16,33`: `MapFS`, the in-memory test filesystem (the model for `MemFs` as a fixture backend).
- **Go `os.Root`** `golang/go@f9d574dc8740640da3f7a5654a47ea0b371b43b0` `src/os/root.go:19-53`: confinement to a directory tree that follows only symlinks staying inside the root, with documented platform caveats (Windows reserved names, a Unix `Chmod`/`Chown` race).
- **Go `x/net/webdav`** `golang/net@fd4c9ad541370b6cd20aa7915cb34d3802ecc41c`
  - `webdav/file.go:30-46`: `FileSystem { Mkdir, OpenFile, RemoveAll, Rename, Stat }`, each taking `context.Context`, slash-separated regardless of OS. Rename-overwrite semantics are OS-dependent (`:36-38`).
  - `webdav/file.go:58-73`: `Dir` "does not prevent traversal of symbolic links within its directory tree, including links that reference locations outside of the tree".
  - `webdav/file.go:157-176,420`: `memFS` in-memory backend.
- **Linux VFS** `torvalds/linux@40212b29a6f718d4e7fb3275cede181cb8e137cf`
  - `Documentation/filesystems/vfs.rst:36-60`: dentry cache over inodes. Lookup goes through the parent inode's `lookup()`, and one inode can have several dentries (hard links).
  - `Documentation/filesystems/nfs/exporting.rst:9-27`: NFS needs a reference "stable across renames, truncates, and server-reboot … there is no simple answer like 'filename'", so filesystems provide opaque filehandle fragments.
  - `include/linux/exportfs.h:33-35`: `FILEID_INO32_GEN`, "32bit inode number, 32 bit generation number".
- **Plan 9 / 9P** `9fans/plan9port@1f1396913fb5372408f442ed26933f5ae4b399cd` `man/man9/0intro.9p:417-437`: "two files on the same server hierarchy are the same if and only if their qids are the same". `qid.path` is unique, and a file deleted and recreated under the same name gets a different path. `qid.version` is "incremented every time the file is modified". This is the model for `NodeId` (never reused) and `Attr::change`.
- **MEGA SDK** `meganz/sdk@2c95a487b4b297dba1d792c88189d432c3452694`
  - `include/mega/types.h:335,668`: `typedef uint64_t handle;`, `UNDEF = ~(handle)0`.
  - `include/mega/file_service/file_id.h` (`FileID::from(NodeHandle)`, 64-bit `mID`) and `include/mega/fuse/common/inode_id.h:26` (`class InodeID: public file_service::FileID`): MEGA's own FUSE layer uses a stable 64-bit id derived from the node handle. So `NodeId(NonZeroU64)` fits the MEGA backend, with `UNDEF` / 0 mapped away.
- **akku internal**
  - `crates/akku/src/task/mod.rs:1226-1229`: `spawn` requires `F: Future + Send + 'static`.
  - `crates/akku/src/io/mod.rs:120-123`: `AsyncRead` is poll-style and dyn-compatible.
  - `crates/akku/src/kv/mod.rs:199`: `KvStore: Send + Sync` is akku's existing (sync) backend-trait precedent, with backends `MemoryStore` and `FileStore`. Naming alternative for Q8: `vfs::MemoryFs`.
  - `crates/akku/src/xdr/_dev/DESIGN.md` Decisions §1: NFS filenames are opaque bytes (`decode_opaque`).

## API sketch (G2 input, non-binding)

This sketch combines the recommendations of Q1–Q12. It contains signatures only. Exact lifetimes, blanket impls, and docs are settled at G2.

```rust
pub type Result<T> = std::result::Result<T, Error>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(NonZeroU64);            // new(u64) -> Option<Self>, get() -> u64

#[non_exhaustive] pub enum NodeKind { File, Dir, Symlink, Other }

#[non_exhaustive] pub struct Attr {        // Attr::new(id, kind) with documented defaults
    pub id: NodeId, pub generation: u64, pub kind: NodeKind,
    pub perm: u32, pub nlink: u32, pub uid: u32, pub gid: u32,
    pub size: u64, pub used: u64,
    pub atime: SystemTime, pub mtime: SystemTime, pub ctime: SystemTime,
    pub change: u64,
}

#[non_exhaustive] pub struct FsInfo {      // static; FSINFO + PATHCONF
    pub fsid: u64, pub read_only: bool, pub persistent_ids: bool,
    pub max_read: u32, pub max_write: u32, pub max_name_len: u32,
    pub time_granularity: Duration, pub case_insensitive: bool, pub case_preserving: bool,
    pub symlinks: bool, pub hard_links: bool,
}
#[non_exhaustive] pub struct FsStat { /* total/free/avail bytes and files; FSSTAT */ }

#[derive(Clone, Copy)] pub struct Name<'a>(&'a [u8]);   // Name::new(bytes, &Limits) -> Result<Name<'a>>
#[non_exhaustive] pub struct Caller { pub uid: u32, pub gid: u32, pub groups: Vec<u32> }
pub struct Access(u32);                    // READ | LOOKUP | MODIFY | EXTEND | DELETE | EXECUTE

#[derive(Clone, Copy, Default)] pub struct DirCursor { pub cookie: u64, pub verifier: u64 }
pub struct Entry { pub name: Vec<u8>, pub id: NodeId, pub kind: NodeKind, pub cookie: u64, pub attr: Option<Attr> }
pub struct DirPage { pub entries: Vec<Entry>, pub eof: bool, pub verifier: u64 }
pub struct ReadResult { pub len: usize, pub eof: bool }

pub trait Filesystem: Send + Sync + 'static {
    fn info(&self) -> FsInfo;
    fn root(&self) -> NodeId;
    fn lookup(&self, caller: &Caller, dir: NodeId, name: Name<'_>) -> impl Future<Output = Result<Attr>> + Send;
    fn parent(&self, caller: &Caller, dir: NodeId) -> impl Future<Output = Result<NodeId>> + Send;
    fn getattr(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<Attr>> + Send;
    fn access(&self, caller: &Caller, id: NodeId, want: Access) -> impl Future<Output = Result<Access>> + Send; // default: mode bits
    fn read(&self, caller: &Caller, id: NodeId, offset: u64, buf: &mut [u8]) -> impl Future<Output = Result<ReadResult>> + Send;
    fn read_dir(&self, caller: &Caller, dir: NodeId, cursor: DirCursor, max_entries: usize, with_attrs: bool)
        -> impl Future<Output = Result<DirPage>> + Send;
    fn read_link(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<Vec<u8>>> + Send;
    fn statfs(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<FsStat>> + Send;
}

pub trait DynFilesystem: Send + Sync + 'static { /* same ops, `-> BoxFuture<'a, Result<…>>` */ }
impl<F: Filesystem> DynFilesystem for F { /* Box::pin(self.op(..)) */ }
impl Filesystem for Arc<dyn DynFilesystem> { /* forwards */ }

pub fn resolve<F: Filesystem + ?Sized>(fs: &F, caller: &Caller, path: &[u8], limits: &Limits) -> impl Future<Output = Result<Attr>> + Send + '_;
pub fn read_dir_all<F: Filesystem + ?Sized>(fs: &F, caller: &Caller, dir: NodeId, limits: &Limits) -> impl Future<Output = Result<Vec<Entry>>> + Send + '_;

pub struct MemFs { /* RwLock<tree>, monotonic ids, Limits */ }  // impl Filesystem + inherent build methods
```

**Seams (preliminary, for G3 test planning).** Name validation (`Name::new` adversarial bytes: empty, `/`, NUL, `.`, `..`, over-length, non-UTF-8). `NodeId` zero and unknown ids (`Stale`/`NotFound`). Lookup on a non-directory (`NotADirectory`). `read` at and past EOF and with an empty buffer. `read` on a directory (`IsADirectory`). `read_dir` cookie resume, stale verifier (`BadCookie`), `max_entries` 0 and 1, very large directories. `resolve` depth and length limits. The `parent` of the root. `DynFilesystem` round trip equivalence. Future drop at every await point (cancellation safety). `Send`/`Sync` static assertions. `MemFs` `max_nodes`/`max_bytes` exhaustion. Ids never reused after remove + re-create (9P rule).
