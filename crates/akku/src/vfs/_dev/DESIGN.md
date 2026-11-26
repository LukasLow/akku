# vfs — design record

Status: **G2 API** (2026-11-26). §Purpose, §Non-goals, §Namespace, §Equivalents, §Dependencies, and §Key design questions are the G1 deliverable (merged in PR #22). §References was front-loaded at G1. §Decisions (G2) records the final answers to Q1–Q13, and §G2: API is the binding public API, contracts, seams, and G3 test plan. Where the G1 text (§Key design questions, §API sketch) disagrees with §Decisions (G2) or §G2: API, the G2 sections win.

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
- **MSRV 1.85 / edition 2021** (workspace root `Cargo.toml:12-13`, `[workspace.package]` `edition` / `rust-version`) is enough. RPITIT has been stable since 1.75. The `io::ErrorKind` variants used in Q6 are stable by 1.83 or 1.85. Return-type notation is unstable, so `Send` is fixed (Q1 a). `InvalidFilename` and `FilesystemLoop` are unstable and are not used.
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

This sketch combines the recommendations of Q1–Q12. It contains signatures only. Exact lifetimes, blanket impls, and docs are settled at G2. **Superseded by §G2: API** (kept for the record of what G1 proposed).

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

## Decisions (G2)

Approved by **Orcha + independent Opus review, 2026-11-26**. The human delegated G1/G2 approval for this module to that pair. All G1 recommendations (Q1–Q13) are accepted, with the changes in the table. The trait skeleton in §G2: API was compiled with `rustc 1.85.1 --edition 2021 -D warnings` and with stable `clippy-driver -D warnings` before this record was written (scratch file outside the repo).

| Q | Final decision |
|---|---|
| Q1 async spelling | **(D).** Static `Filesystem: Send + Sync` (no `'static` supertrait) with RPITIT methods returning `impl Future<Output = …> + Send`. Dyn twin `DynFilesystem: Send + Sync` whose methods are suffixed `_dyn` (so having both traits in scope never causes `E0034`) and return `BoxFuture<'a, T>`. Blanket `impl<F: Filesystem> DynFilesystem for F` (`Box::pin`). `impl<'d> Filesystem for dyn DynFilesystem + 'd` forwards with `async fn … { self.op_dyn(..).await }` (returning the box directly gives `E0700`). Forwarding impls for `Arc<F>`, `Box<F>`, `&F` with `F: Filesystem + ?Sized`. Alias `BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>`. Spawning code adds `'static` itself (`Arc<F>` with `F: 'static`). |
| Q2 identity | **(B)** with changes. `NodeId(NonZeroU64)` + `Attr::generation`. `persistent_ids: bool` is replaced by `FsInfo::id_epoch: u64`: ids and generations are meaningful only while `id_epoch` is unchanged. `MemFs` draws a fresh epoch per instance; persistent backends return a constant. **Stale rule:** an unknown or removed id yields `ErrorKind::Stale` on every id-taking operation, never `NotFound`. NFS handle layout budget (for `nfs::server`, not `vfs`): version 1 + fsid 8 + epoch 8 + id 8 + generation 8 (+ MAC 16) = 49 ≤ 64 bytes. |
| Q3 read/write split | **(B)** agreed. v1 ships only `Filesystem`. The write verifier and `max_write` belong to the future `FilesystemWrite` / `FsInfo` extension (the structs are `#[non_exhaustive]`, so fields can be added). |
| Q4 attributes | **(C)** with changes. `NodeKind` is `#[non_exhaustive]` with `File`, `Dir`, `Symlink`; `Other` is dropped (future kinds arrive with MKNOD support). Contract: `change`, `mtime`, `ctime`, and `size` reflect remote changes as soon as the backend knows them. |
| Q5 directory paging | **(C)** with a `Cookie(u64)` newtype. `Cookie::START = 0`; `1` and `2` are reserved for server-synthesized `.` and `..`; backends emit cookies ≥ `Cookie::FIRST` (3). A cookie resumes at the entry after it and survives unrelated inserts and deletes. `START` (any verifier, including 0) is always accepted. Entry names are valid `Name`s and unique per directory; backends mangle duplicate remote names (MEGA allows duplicates). `MemFs`: per-directory monotonic insertion-sequence cookies, verifier 0. No `.`/`..` from backends. |
| Q6 errors | **(C)** agreed, plus the Stale rule (Q2). `Error { kind, message: Cow<'static, str>, source }`. Documented `From<io::Error>` mapping: `TimedOut`, `ResourceBusy`, `WouldBlock`, `Interrupted` → `TryAgain`; the G1 `TimedOut` kind is dropped (it mapped to `JUKEBOX` like `TryAgain`). `resolve` follows no symlinks, so no loop kind is needed. |
| Q7 limits | Agreed with changes. **Clamp sizes** (a read larger than the limit is a legal short read with `eof = false`; `max_entries` is clamped), **error on names, paths, symlink targets, and group lists**. New `max_groups = 16` (AUTH_SYS). `max_nodes` / `max_bytes` move out of `Limits` into `MemFs::with_limits`. `max_write_len` is not in v1 (`Limits` is `#[non_exhaustive]`, it lands with writes). Effective server read size = `min(FsInfo::max_read, Limits::max_read_len)`. |
| Q8 backends | **(B) + public `vfs::testing` conformance kit** (`check`, `check_cancellation`; the latter polls with `std::task::Waker::noop`, stable since 1.85). `LocalFs` deferred. |
| Q9 wave / MSRV | Wave 2, MSRV 1.85, no feature gate. The pin is the workspace root `Cargo.toml:12-13`. Because smd defaults to stable (1.100), **G3 and G4 must additionally run `smd cargo +1.85 check -p akku --all-targets`.** |
| Q10 caller | **(B)**: `Caller::new(uid, gid)` + `Caller::with_groups(&[u32], &Limits)`; fields are private (the group bound is structural). |
| Q11 names | **(D)** + `Name::as_bytes()`. Unicode normalization and case folding are the backend's job. |
| Q12 read buffer | **(B)** with the contract: `len ≤ buf.len()`; short reads are legal; `offset ≥ size` → `(0, eof = true)`; bytes of `buf` past `len` are unspecified; a dropped read future may leave `buf` partially written. |
| Q13 caching | Agreed: no caching in the core. |

Also adopted from the review: `#[non_exhaustive]` plus constructors on `Entry`, `DirPage`, `ReadResult`, `FsStat`, `FsInfo`, `Attr`; required methods `info`, `root`, `getattr`, `lookup`, `parent`, `read`, `read_dir`; default methods `access` (mode bits), `read_link` (`InvalidInput`), `statfs` (unknown); the global contracts in §G2: API › Contracts; the megaghost notes in §G2: API › Notes for consumers.

G2 Worker choices not named by the review (Checker: please confirm or push back):
- `parent` returns `Attr` (not `NodeId`): NFS `LOOKUP ".."` needs the object attributes, and a remote backend saves a round trip.
- `read_dir_all` is **not** in v1 (unbounded result size needs its own limit design); it moves to the path convenience layer in `_dev/TODO.md`. The testing kit pages internally.
- `Access::from_mode` is public so a backend overriding `access` can reuse the POSIX evaluation.
- Default `read_link` / `statfs` call `getattr` first, so the Stale rule holds even for backends that do not override them.

## G2: API

Everything below lives in `akku::vfs` (`crates/akku/src/vfs/`). Planned files: `mod.rs` (module rustdoc + re-exports), `error.rs`, `types.rs` (ids, attrs, names, cursors), `fs.rs` (traits, forwarding impls, `resolve`), `memfs.rs`, `testing.rs` (public submodule `vfs::testing`), `tests.rs`, `meta.toml` (`status = "native"`, `wasm32 = true`, `wave = 2`). No `unsafe`, no `sys/`, zero dependencies.

### Item list

| Item | Kind | Purpose |
|---|---|---|
| `Filesystem` | trait | Static async read-side backend trait |
| `DynFilesystem` | trait | Dyn-compatible twin (`*_dyn`, boxed futures) |
| `BoxFuture<'a, T>` | type alias | `Pin<Box<dyn Future<Output = T> + Send + 'a>>` |
| `Result<T>` | type alias | `std::result::Result<T, vfs::Error>` |
| `Error`, `ErrorKind` | struct, enum | Neutral error model |
| `Limits` | struct | DoS limits (`with_*` builder) |
| `NodeId`, `NodeKind`, `Attr` | struct, enum, struct | Node identity and attributes |
| `FsInfo`, `FsStat` | structs | Static filesystem info; space/file counters |
| `Name<'a>` | struct | Validated borrowed entry name |
| `Caller`, `Access` | structs | Per-call identity; ACCESS bit mask |
| `Cookie`, `DirCursor`, `Entry`, `DirPage` | structs | Cookie-paged directory listing |
| `ReadResult` | struct | Outcome of `read` |
| `resolve` | async fn | Path → `Attr` walk from the root |
| `MemFs` | struct | In-memory backend for tests and fixtures |
| `testing::{check, check_cancellation, Report, Violation}` | module | Conformance kit for any backend |

### Contracts (apply to every `Filesystem` implementation)

1. **Shared, concurrent access.** Every method takes `&self`. Backends are `Send + Sync` and use interior mutability; any number of calls may run concurrently from any threads. Results must be as if each call happened atomically at some point between its start and completion.
2. **Never block the thread.** A backend never performs blocking I/O or waits on a lock held across an `.await` inside a method; it waits only by returning `Poll::Pending`. Short critical sections (a `RwLock` around an in-memory tree) are fine.
3. **Cancellation safety.** A server drops a future when the client disconnects. Dropping any returned future after any number of polls (including zero) leaves the filesystem consistent: later calls behave as if the dropped call never happened or completed. `read` may leave its `buf` partially written.
4. **Stale rule.** An id the backend does not know (never issued, removed, or from another `id_epoch`) yields `ErrorKind::Stale` from every operation that takes it: `getattr`, `lookup` (as `dir`), `parent`, `access`, `read`, `read_dir` (as `dir`), `read_link`, `statfs`. `NotFound` is reserved for "this name is absent from this existing directory" (`lookup`, `resolve`).
5. **Identity.** For a fixed `id_epoch`, `root()` is constant, a `NodeId` names one node for that node's lifetime, and the pair `(id, generation)` never names a different object. A backend that never reuses ids keeps `generation` at 0.
6. **Bounded outputs.** Backends never return more than asked: `ReadResult::len ≤ buf.len()`, `DirPage::entries.len() ≤ max(1, max_entries)`, `read_link` ≤ the backend's `max_symlink_len`.
7. **Progress.** `read` with a non-empty `buf` returning `eof = false` has `len ≥ 1`. `read_dir` with `eof = false` returns at least one entry. (Otherwise a server loops forever.)
8. **No panics.** No input (any `NodeId`, any `offset` up to `u64::MAX`, any cookie or verifier, any `max_entries` up to `usize::MAX`) makes a backend panic or overflow; offset arithmetic is checked or saturating.
9. **Freshness.** `Attr::{size, mtime, ctime, change}` reflect remote changes as soon as the backend knows them. `change` strictly increases whenever data or attributes of the node change (a directory changes when an entry is added, removed, or renamed).
10. **No symlink following.** No operation follows symlinks: `lookup` and `resolve` return the symlink's own `Attr`; `lookup` *through* a symlink is `NotADirectory`.
11. **Caller.** Backends may ignore `Caller`. A backend that enforces permissions returns `PermissionDenied` (or `NotPermitted` for owner-only operations).

### Errors

```rust
/// A `vfs` error: a kind, an optional static-or-owned message, and an optional source.
#[derive(Debug)]
pub struct Error { /* kind: ErrorKind, message: Cow<'static, str>, source: Option<Box<dyn std::error::Error + Send + Sync>> */ }

impl Error {
    /// Builds an error. `message` may be empty; `&'static str` does not allocate.
    pub fn new(kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self;
    /// Attaches a source error (kept by `std::error::Error::source`).
    pub fn with_source(self, source: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self;
    /// The kind servers map to wire status codes.
    pub fn kind(&self) -> ErrorKind;
    /// The message, possibly empty.
    pub fn message(&self) -> &str;
}
impl From<ErrorKind> for Error;          // empty message
impl fmt::Display for Error;             // "<kind>" if the message is empty, else "<kind>: <message>"
impl std::error::Error for Error;        // source() returns the attached source
impl From<Error> for std::io::Error;     // io::Error::new(mapped kind, the vfs::Error)
impl From<std::io::Error> for Error;     // see mapping; recovers a wrapped vfs::Error exactly

/// Error categories. `#[non_exhaustive]`: servers map unknown future kinds to their generic I/O error.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind { NotFound, NotPermitted, PermissionDenied, AlreadyExists, NotADirectory,
    IsADirectory, DirectoryNotEmpty, InvalidInput, InvalidName, NameTooLong, ReadOnly, NoSpace,
    QuotaExceeded, FileTooLarge, CrossesDevices, TooManyLinks, Stale, BadCookie, Unsupported,
    TryAgain, Io }
impl ErrorKind { pub const fn as_str(self) -> &'static str; }   // lower-case description, e.g. "stale node id"
impl fmt::Display for ErrorKind;
```

`Error: Send + Sync + 'static` and `!Clone` (the source is a boxed trait object).

| `ErrorKind` | Meaning | NFSv3 (`nfs::server`) | `vfs` → `io::ErrorKind` | `io::ErrorKind` → `vfs` |
|---|---|---|---|---|
| `NotFound` | name absent in an existing dir | `NFS3ERR_NOENT` 2 | `NotFound` | `NotFound` |
| `NotPermitted` | owner-only operation | `NFS3ERR_PERM` 1 | `PermissionDenied` | — |
| `PermissionDenied` | access check failed | `NFS3ERR_ACCES` 13 | `PermissionDenied` | `PermissionDenied` |
| `AlreadyExists` | name taken (MemFs build API) | `NFS3ERR_EXIST` 17 | `AlreadyExists` | `AlreadyExists` |
| `NotADirectory` | dir op on a non-dir | `NFS3ERR_NOTDIR` 20 | `NotADirectory` | `NotADirectory` |
| `IsADirectory` | file op on a dir | `NFS3ERR_ISDIR` 21 | `IsADirectory` | `IsADirectory` |
| `DirectoryNotEmpty` | remove of a non-empty dir | `NFS3ERR_NOTEMPTY` 66 | `DirectoryNotEmpty` | `DirectoryNotEmpty` |
| `InvalidInput` | bad argument (read on symlink, read_link on non-symlink, bad perm, too many groups, too deep path) | `NFS3ERR_INVAL` 22 | `InvalidInput` | `InvalidInput` |
| `InvalidName` | empty, `.`, `..`, `/`, or NUL in a name | `NFS3ERR_ACCES` 13 (RFC 1813 §3.2) | `InvalidInput` | — |
| `NameTooLong` | name, path, or symlink target over its limit | `NFS3ERR_NAMETOOLONG` 63 | `InvalidInput` | — |
| `ReadOnly` | write to a read-only fs | `NFS3ERR_ROFS` 30 | `ReadOnlyFilesystem` | `ReadOnlyFilesystem` |
| `NoSpace` | storage or node budget exhausted | `NFS3ERR_NOSPC` 28 | `StorageFull` | `StorageFull` |
| `QuotaExceeded` | user quota | `NFS3ERR_DQUOT` 69 | `QuotaExceeded` | `QuotaExceeded` |
| `FileTooLarge` | size or offset + len over the max file size | `NFS3ERR_FBIG` 27 | `FileTooLarge` | `FileTooLarge` |
| `CrossesDevices` | cross-fs link/rename | `NFS3ERR_XDEV` 18 | `CrossesDevices` | `CrossesDevices` |
| `TooManyLinks` | link count limit | `NFS3ERR_MLINK` 31 | `TooManyLinks` | `TooManyLinks` |
| `Stale` | unknown/removed/foreign-epoch id | `NFS3ERR_STALE` 70 | `StaleNetworkFileHandle` | `StaleNetworkFileHandle` |
| `BadCookie` | cookie invalid for the verifier | `NFS3ERR_BAD_COOKIE` 10003 | `InvalidInput` | — |
| `Unsupported` | operation not provided | `NFS3ERR_NOTSUPP` 10004 | `Unsupported` | `Unsupported` |
| `TryAgain` | temporarily unavailable (uncached MEGA chunk, rate limit, timeout) | `NFS3ERR_JUKEBOX` 10008 | `ResourceBusy` | `ResourceBusy`, `TimedOut`, `WouldBlock`, `Interrupted` |
| `Io` | anything else | `NFS3ERR_IO` 5 | `Other` | every other kind |

`From<io::Error>`: if the `io::Error` wraps a `vfs::Error` (as produced by `From<vfs::Error>`), that error is returned unchanged, so `vfs → io → vfs` preserves kind, message, and source for every kind. Otherwise the kind is mapped as in the last column, the message is empty, and the `io::Error` becomes the source. All `io::ErrorKind` variants named are stable at Rust 1.85 (§References). A backend contract violation detected by a server (e.g. `len > buf.len()`) is the server's `NFS3ERR_SERVERFAULT` 10006, not a `vfs` kind.

### Limits

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_read_len: usize,    pub max_dir_entries: usize, pub max_name_len: usize,
    pub max_path_len: usize,    pub max_path_depth: usize,  pub max_symlink_len: usize,
    pub max_groups: usize,
}
impl Default for Limits { /* Limits::new() */ }
impl Limits {
    pub const fn new() -> Self;
    pub const fn with_max_read_len(self, max: usize) -> Self;
    pub const fn with_max_dir_entries(self, max: usize) -> Self;
    pub const fn with_max_name_len(self, max: usize) -> Self;
    pub const fn with_max_path_len(self, max: usize) -> Self;
    pub const fn with_max_path_depth(self, max: usize) -> Self;
    pub const fn with_max_symlink_len(self, max: usize) -> Self;
    pub const fn with_max_groups(self, max: usize) -> Self;
}
```

All bounds are inclusive (a value equal to the limit is accepted).

| Field | Default | On excess | Enforced by |
|---|---|---|---|
| `max_read_len` | `1_048_576` (1 MiB) | **clamp**: short read, `eof = false`; `0` behaves as `1` | servers (count clamp), `MemFs::read` |
| `max_dir_entries` | `4_096` | **clamp** `max_entries` to `1..=max`; `0` behaves as `1` | servers, `MemFs::read_dir` |
| `max_name_len` | `255` bytes | **error** `NameTooLong` | `Name::new`, `MemFs` build API |
| `max_path_len` | `4_096` bytes | **error** `NameTooLong` | `resolve` |
| `max_path_depth` | `256` components | **error** `InvalidInput` | `resolve` |
| `max_symlink_len` | `4_096` bytes | **error** `NameTooLong` | `MemFs::symlink`; servers treat a longer `read_link` result as a backend fault |
| `max_groups` | `16` (AUTH_SYS) | **error** `InvalidInput` | `Caller::with_groups` |
| `MemFs` `max_nodes` | `1_048_576` (`MemFs::DEFAULT_MAX_NODES`) | **error** `NoSpace` | `MemFs` build API |
| `MemFs` `max_bytes` | `268_435_456` (256 MiB, `MemFs::DEFAULT_MAX_BYTES`) | **error** `NoSpace`, all-or-nothing | `MemFs` build API |

### Identity and attributes

```rust
/// Opaque, non-zero node id. Stable for the node's lifetime within one `FsInfo::id_epoch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(NonZeroU64);
impl NodeId {
    pub const fn new(id: u64) -> Option<Self>;        // None for 0 (RFC 1813: fileid 0 is special)
    pub const fn from_nonzero(id: NonZeroU64) -> Self;
    pub const fn get(self) -> u64;
}
impl From<NonZeroU64> for NodeId;
impl From<NodeId> for u64;

/// Node type. Future kinds (fifo, socket, devices) arrive with MKNOD support.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind { File, Dir, Symlink }

/// Node attributes (getattr / fattr3 superset, protocol neutral).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub id: NodeId,
    pub generation: u64,       // (id, generation) never names a different object
    pub kind: NodeKind,
    pub perm: u32,             // permission bits incl. setuid/setgid/sticky; consumers ignore bits above 0o7777
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,             // bytes for files; target length for symlinks; backend-defined for dirs
    pub used: u64,             // bytes of storage used
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
    pub change: u64,           // strictly increasing per-node change counter (contract 9)
}
impl Attr {
    /// Defaults: generation 0; perm 0o644 (File) / 0o755 (Dir) / 0o777 (Symlink);
    /// nlink 1 (File, Symlink) / 2 (Dir); uid = gid = 0; size = used = 0;
    /// all times `UNIX_EPOCH`; change 0. Backends set the remaining public fields.
    pub fn new(id: NodeId, kind: NodeKind) -> Self;
}

/// Static per-filesystem information (FSINFO + PATHCONF). Constant for the backend's lifetime.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsInfo {
    pub fsid: u64,
    pub id_epoch: u64,               // ids/generations valid only while this is unchanged
    pub read_only: bool,
    pub max_read: u32,               // preferred and maximum read size the backend serves
    pub max_name_len: u32,
    pub max_file_size: u64,
    pub time_granularity: Duration,
    pub case_insensitive: bool,
    pub case_preserving: bool,
    pub symlinks: bool,
    pub hard_links: bool,
}
impl FsInfo {
    /// Defaults: read_only true (v1 has no write trait); max_read 1 MiB; max_name_len 255;
    /// max_file_size i64::MAX; time_granularity 1 ns; case_insensitive false;
    /// case_preserving true; symlinks true; hard_links false.
    pub const fn new(fsid: u64, id_epoch: u64) -> Self;
}

/// Space and file counters (FSSTAT).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FsStat {
    pub total_bytes: u64, pub free_bytes: u64, pub avail_bytes: u64,
    pub total_files: u64, pub free_files: u64, pub avail_files: u64,
}
impl FsStat {
    /// All counters 0: "no information" (what the default `statfs` returns).
    pub const fn unknown() -> Self;
}
```

`id_epoch` semantics: a persistent backend (megaghost: derived from account + root handle) returns the same value across restarts, so NFS handles survive a restart. `MemFs` returns a fresh random value per instance (derived from `std::collections::hash_map::RandomState`, the current time, and a process-wide counter), so handles from an earlier instance are rejected as stale. `nfs::server` compares the handle's epoch with `info().id_epoch` and answers `NFS3ERR_STALE` without calling the backend on mismatch.

### Names, callers, access

```rust
/// A validated directory-entry name: 1..=max_name_len bytes, not "." or "..", no '/' or NUL.
/// Any other bytes are allowed (non-UTF-8, '\\', spaces, control characters).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name<'a>(&'a [u8]);
impl<'a> Name<'a> {
    /// Checks, in this order: empty → InvalidName; len > limits.max_name_len → NameTooLong;
    /// "." or ".." → InvalidName; contains b'/' or 0 → InvalidName.
    pub fn new(bytes: &'a [u8], limits: &Limits) -> Result<Self>;
    pub const fn as_bytes(self) -> &'a [u8];
    pub fn to_str(self) -> Option<&'a str>;      // Some iff valid UTF-8
}
impl fmt::Debug for Name<'_>;                    // escaped bytes, never panics on non-UTF-8
impl AsRef<[u8]> for Name<'_>;

/// Per-call identity (AUTH_SYS shape). Not `Default`: no implicit root caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller { /* private: uid, gid, groups */ }
impl Caller {
    pub const fn new(uid: u32, gid: u32) -> Self;   // no supplementary groups
    /// groups.len() > limits.max_groups → InvalidInput. Replaces earlier groups.
    pub fn with_groups(self, groups: &[u32], limits: &Limits) -> Result<Self>;
    pub const fn uid(&self) -> u32;
    pub const fn gid(&self) -> u32;
    pub fn groups(&self) -> &[u32];
}

/// ACCESS bit mask. Values equal RFC 1813 ACCESS3_* (READ 0x01, LOOKUP 0x02, MODIFY 0x04,
/// EXTEND 0x08, DELETE 0x10, EXECUTE 0x20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Access(u32);
impl Access {
    pub const READ: Self; pub const LOOKUP: Self; pub const MODIFY: Self;
    pub const EXTEND: Self; pub const DELETE: Self; pub const EXECUTE: Self;
    pub const fn empty() -> Self;
    pub const fn all() -> Self;                         // 0x3f
    pub const fn bits(self) -> u32;
    pub const fn from_bits_truncate(bits: u32) -> Self; // drops unknown bits
    pub const fn contains(self, other: Self) -> bool;
    pub const fn is_empty(self) -> bool;
    pub const fn union(self, other: Self) -> Self;
    pub const fn intersection(self, other: Self) -> Self;
    /// POSIX mode-bit evaluation (see below). Pure; no I/O.
    pub fn from_mode(caller: &Caller, attr: &Attr, read_only: bool) -> Self;
}
impl BitOr for Access; impl BitAnd for Access; impl BitOrAssign for Access;
```

`Access::from_mode` algorithm (the default `Filesystem::access` returns `want ∩ from_mode(caller, getattr(id)?, info().read_only)`):
1. Pick the permission triple `rwx`: if `caller.uid() == 0`, `r = w = 1` and `x = 1` iff the node is a `Dir` or any of `perm & 0o111` is set; else if `caller.uid() == attr.uid`, the owner bits `(perm >> 6) & 7`; else if `caller.gid() == attr.gid` or `attr.gid ∈ caller.groups()`, the group bits `(perm >> 3) & 7`; else the other bits `perm & 7`.
2. Map: `r` → `READ`; `w` → `MODIFY | EXTEND`, plus `DELETE` for a `Dir`; `x` → `LOOKUP` for a `Dir`, `EXECUTE` otherwise.
3. If `read_only`, remove `MODIFY | EXTEND | DELETE`.

### Directories

```rust
/// Opaque directory position. 0 = START; 1 and 2 are reserved for server-synthesized "." and "..";
/// backends emit only cookies >= FIRST (3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Cookie(u64);
impl Cookie {
    pub const START: Self;     // 0
    pub const DOT: Self;       // 1
    pub const DOT_DOT: Self;   // 2
    pub const FIRST: Self;     // 3, the smallest backend cookie
    pub const fn new(raw: u64) -> Self;
    pub const fn get(self) -> u64;
    pub const fn is_start(self) -> bool;   // true for 0, 1, 2 (all mean "from the first entry")
}

/// Where to resume a listing (cookie3 + cookieverf3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DirCursor { pub cookie: Cookie, pub verifier: u64 }
impl DirCursor {
    pub const START: Self;                                  // cookie START, verifier 0
    pub const fn new(cookie: Cookie, verifier: u64) -> Self;
}

/// One directory entry. `name` is always a valid `Name` (contract), unique within the directory.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: Vec<u8>, pub id: NodeId, pub kind: NodeKind, pub cookie: Cookie,
    pub attr: Option<Attr>,     // Some only when requested with `with_attrs` (may still be None)
}
impl Entry {
    pub fn new(name: Name<'_>, id: NodeId, kind: NodeKind, cookie: Cookie) -> Self;  // attr None
    pub fn with_attr(self, attr: Attr) -> Self;
}

/// One page of a listing.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DirPage { pub entries: Vec<Entry>, pub eof: bool, pub verifier: u64 }
impl DirPage { pub fn new(entries: Vec<Entry>, eof: bool, verifier: u64) -> Self; }
```

Cookie rules (all backends):
- `read_dir` with cursor cookie `c` returns entries that follow the entry whose cookie is `c` in the directory's listing order. `Cookie::is_start()` cookies (0, 1, 2) mean "from the first entry", with **any** verifier.
- Cookies are unique within one directory listing and never `0`, `1`, or `2`.
- **Stability:** with verifier `0` ("cookies always valid", RFC 1813 READDIR note), a cookie stays usable after unrelated inserts and deletes, including deletion of the entry it came from. During a paged listing, every entry that exists for the whole listing is returned exactly once; entries added or removed meanwhile appear at most once. A backend that cannot promise this returns a non-zero verifier and answers `BadCookie` when a cursor's verifier no longer matches.
- An unchanged directory lists in the same order with the same cookies.
- `eof = true` iff no entry follows the last returned one at the time of the call (so a page that returns the last entry has `eof = true`).
- Backends never return `.` or `..`; servers synthesize them with cookies `DOT` / `DOT_DOT`.

### Reads

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadResult { pub len: usize, pub eof: bool }
impl ReadResult { pub const fn new(len: usize, eof: bool) -> Self; }
```

### `Filesystem`

```rust
pub trait Filesystem: Send + Sync {
    /// Static filesystem information. Synchronous, cheap, constant for the backend's lifetime.
    fn info(&self) -> FsInfo;

    /// The root directory id. Constant for one `id_epoch`.
    fn root(&self) -> NodeId;

    /// Attributes of `id`; `attr.id == id`. Unknown id → Stale.
    fn getattr(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<Attr>> + Send;

    /// Looks `name` up in directory `dir` and returns the child's attributes (symlinks not followed).
    /// Errors: dir unknown → Stale; dir not a Dir → NotADirectory; absent → NotFound;
    /// name longer than the backend's own max_name_len → NameTooLong.
    /// Should answer from cached metadata when possible (negative lookups are frequent).
    fn lookup(&self, caller: &Caller, dir: NodeId, name: Name<'_>) -> impl Future<Output = Result<Attr>> + Send;

    /// Attributes of the parent of directory `dir` (NFS LOOKUP ".."). The root's parent is the root.
    /// Errors: unknown → Stale; not a Dir → NotADirectory.
    fn parent(&self, caller: &Caller, dir: NodeId) -> impl Future<Output = Result<Attr>> + Send;

    /// Reads file bytes at `offset` into `buf` and returns how many were written to `buf[..len]`.
    /// Contract: len <= buf.len(); short reads are legal; with a stable file,
    /// eof == (offset + len >= size) (computed without overflow); offset >= size → (0, true);
    /// empty buf → (0, offset >= size); non-empty buf and !eof → len >= 1; bytes of buf past
    /// len are unspecified; a dropped future may leave buf partially written.
    /// Errors: unknown → Stale; Dir → IsADirectory; Symlink → InvalidInput; TryAgain when the
    /// data is temporarily unavailable.
    fn read(&self, caller: &Caller, id: NodeId, offset: u64, buf: &mut [u8]) -> impl Future<Output = Result<ReadResult>> + Send;

    /// One page of directory `dir` after `cursor` (see Cookie rules). At most max(1, max_entries)
    /// entries (backends may clamp further). `with_attrs == false` → every `attr` is None;
    /// `true` → `attr` should be Some, and when Some, `attr.id == id` and `attr.kind == kind`.
    /// Errors: unknown → Stale; not a Dir → NotADirectory; invalid cursor → BadCookie.
    fn read_dir(&self, caller: &Caller, dir: NodeId, cursor: DirCursor, max_entries: usize, with_attrs: bool)
        -> impl Future<Output = Result<DirPage>> + Send;

    /// Which of `want` the caller may do on `id`; the result is a subset of `want`.
    /// Default: `want ∩ Access::from_mode(caller, &getattr(id)?, info().read_only)`.
    fn access(&self, caller: &Caller, id: NodeId, want: Access) -> impl Future<Output = Result<Access>> + Send { /* default */ }

    /// Target bytes of symlink `id`: non-empty, no NUL, at most the backend's max_symlink_len.
    /// Errors: unknown → Stale; not a Symlink → InvalidInput.
    /// Default: `getattr(id)?` then InvalidInput. Backends with symlinks must override it.
    fn read_link(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<Vec<u8>>> + Send { /* default */ }

    /// Space and file counters for the filesystem containing `id`.
    /// Default: `getattr(id)?` then `FsStat::unknown()`.
    fn statfs(&self, caller: &Caller, id: NodeId) -> impl Future<Output = Result<FsStat>> + Send { /* default */ }
}
```

Implementors may write `async fn` for every method; the compiler checks the `Send` bound on the resulting future.

### `DynFilesystem` and forwarding impls

```rust
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Dyn-compatible twin of `Filesystem` (one heap allocation per async call).
/// Implemented for every `F: Filesystem` (sized); do not implement it by hand.
pub trait DynFilesystem: Send + Sync {
    fn info_dyn(&self) -> FsInfo;
    fn root_dyn(&self) -> NodeId;
    fn getattr_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId) -> BoxFuture<'a, Result<Attr>>;
    fn lookup_dyn<'a>(&'a self, caller: &'a Caller, dir: NodeId, name: Name<'a>) -> BoxFuture<'a, Result<Attr>>;
    fn parent_dyn<'a>(&'a self, caller: &'a Caller, dir: NodeId) -> BoxFuture<'a, Result<Attr>>;
    fn read_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId, offset: u64, buf: &'a mut [u8]) -> BoxFuture<'a, Result<ReadResult>>;
    fn read_dir_dyn<'a>(&'a self, caller: &'a Caller, dir: NodeId, cursor: DirCursor, max_entries: usize, with_attrs: bool)
        -> BoxFuture<'a, Result<DirPage>>;
    fn access_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId, want: Access) -> BoxFuture<'a, Result<Access>>;
    fn read_link_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId) -> BoxFuture<'a, Result<Vec<u8>>>;
    fn statfs_dyn<'a>(&'a self, caller: &'a Caller, id: NodeId) -> BoxFuture<'a, Result<FsStat>>;
}

impl<F: Filesystem> DynFilesystem for F { /* Box::pin(Filesystem::op(self, ..)) */ }
impl<'d> Filesystem for dyn DynFilesystem + 'd { /* async fn op(..) { self.op_dyn(..).await } */ }
impl<F: Filesystem + ?Sized> Filesystem for Arc<F> { /* (**self).op(..) */ }
impl<F: Filesystem + ?Sized> Filesystem for Box<F> { /* (**self).op(..) */ }
impl<F: Filesystem + ?Sized> Filesystem for &F     { /* (**self).op(..) */ }
```

All forwarding impls forward **every** method, including `access`, `read_link`, and `statfs`, so a backend's overrides are honored through every wrapper. Consequences (all compile-checked): `Arc<dyn DynFilesystem>`, `Box<dyn DynFilesystem>`, and `&dyn DynFilesystem` implement `Filesystem`; a generic `F: Filesystem + ?Sized + 'static` behind `Arc<F>` yields `Send + 'static` futures suitable for `akku::task::spawn`; with both traits imported, `fs.getattr(..)` on `MemFs` is unambiguous.

### Helpers

```rust
/// Walks a '/'-separated byte path from `fs.root()` with `lookup` and returns the final Attr.
pub async fn resolve<F: Filesystem + ?Sized>(fs: &F, caller: &Caller, path: &[u8], limits: &Limits) -> Result<Attr>;
```

Order of checks, all before the first backend call: `path.len() > max_path_len` → `NameTooLong`; more than `max_path_depth` non-empty components → `InvalidInput`; each component through `Name::new` (so `.` / `..` / NUL → `InvalidName`, over-long → `NameTooLong`). Empty components (leading, trailing, repeated `/`) are skipped, so `b""` and `b"/"` resolve to the root (one `getattr`). Then `getattr(root)` and one `lookup` per component. Symlinks are not followed: a final symlink is returned as is, and a symlink in the middle makes the next `lookup` fail with `NotADirectory`. The future is `Send` when `F: Filesystem + ?Sized`.

### `MemFs`

```rust
/// In-memory backend: a tree behind a RwLock. All futures complete on their first poll.
pub struct MemFs { /* private */ }
impl fmt::Debug for MemFs;        // node count and bytes used only, never the tree
impl Default for MemFs;           // MemFs::new()
impl Filesystem for MemFs;        // read-only view; default access, own read_link and statfs
impl MemFs {
    pub const DEFAULT_MAX_NODES: usize = 1_048_576;
    pub const DEFAULT_MAX_BYTES: usize = 268_435_456;
    pub fn new() -> Self;                                                  // Limits::new() + defaults
    pub fn with_limits(limits: Limits, max_nodes: usize, max_bytes: usize) -> Self;
    pub fn limits(&self) -> Limits;
    pub fn create_file(&self, dir: NodeId, name: &[u8], data: &[u8]) -> Result<NodeId>;
    pub fn mkdir(&self, dir: NodeId, name: &[u8]) -> Result<NodeId>;
    pub fn symlink(&self, dir: NodeId, name: &[u8], target: &[u8]) -> Result<NodeId>;
    pub fn write_all(&self, id: NodeId, offset: u64, data: &[u8]) -> Result<()>;
    pub fn set_len(&self, id: NodeId, len: u64) -> Result<()>;
    pub fn remove(&self, dir: NodeId, name: &[u8]) -> Result<()>;
    pub fn set_perm(&self, id: NodeId, perm: u32) -> Result<()>;
    pub fn set_owner(&self, id: NodeId, uid: u32, gid: u32) -> Result<()>;
    pub fn set_times(&self, id: NodeId, atime: SystemTime, mtime: SystemTime) -> Result<()>;
    pub fn node_count(&self) -> usize;      // including the root
    pub fn bytes_used(&self) -> usize;      // see accounting
}
```

Semantics:
- **Identity.** Root is `NodeId 1`. New nodes get strictly increasing ids from 2; ids are **never reused** (id overflow → `NoSpace`). `generation` is always 0. `info()`: `fsid` 0, fresh `id_epoch`, `read_only` true, `max_read = min(limits.max_read_len, u32::MAX)` (0 → 1), `max_name_len = min(limits.max_name_len, u32::MAX)`, `max_file_size = i64::MAX`, `time_granularity` 1 ns, case-sensitive and case-preserving, `symlinks` true, `hard_links` false.
- **Attributes.** Kind defaults from `Attr::new` (perm 0o644 / 0o755 / 0o777, uid = gid = 0). Files: `size = used = data length`, `nlink` 1. Symlinks: `size = used = target length`, `nlink` 1. Dirs: `size = used = entry count`, `nlink = 2 + child directories`. A new node has `change = 1` and `atime = mtime = ctime = SystemTime::now()`. Every mutation of a node increments its `change` by exactly 1 and sets `ctime = now` (data changes also set `mtime`); inserting or removing an entry does the same to the directory (and sets its `mtime`). Reads never change attributes (no atime updates).
- **Build API.** Names go through `Name::new(name, &self.limits())`. Errors in this order: unknown `dir`/`id` → `Stale`; wrong kind (`dir` not a Dir → `NotADirectory`; `write_all`/`set_len` on a Dir → `IsADirectory`, on a Symlink → `InvalidInput`); invalid name → `InvalidName` / `NameTooLong`; name exists → `AlreadyExists`; symlink target empty or containing NUL → `InvalidInput`, longer than `max_symlink_len` → `NameTooLong`; `set_perm` with bits above `0o7777` → `InvalidInput`; `offset + data.len()` overflowing `u64`, or a resulting size above `i64::MAX` → `FileTooLarge`; node or byte budget → `NoSpace`. Every operation is **all-or-nothing**: on error nothing changes (no partial write, no allocation for a rejected sparse write). `write_all` past the end zero-fills the gap; `set_len` truncates or zero-extends.
- **`remove(dir, name)`.** Removes a file, a symlink, or an **empty** directory (`DirectoryNotEmpty` otherwise; `NotFound` if absent). The removed id is `Stale` on every operation afterwards, and its bytes and node slot return to the budget.
- **Accounting.** `bytes_used = Σ entry-name lengths + Σ file data lengths + Σ symlink target lengths`. A build call fails with `NoSpace` if it would make `bytes_used > max_bytes` or `node_count > max_nodes`.
- **`Filesystem` impl.** `read` clamps `buf` to `max(1, limits.max_read_len)` (short read, `eof` exact). `read_dir` clamps `max_entries` to `1..=max(1, limits.max_dir_entries)`, lists in **insertion order**, uses the entry's per-directory insertion sequence number (starting at `Cookie::FIRST`, strictly increasing, never reused in that directory) as its cookie, ignores verifiers, and always returns verifier 0. Resuming at cookie `c` returns entries with sequence `> c`, so any `u64` cookie is accepted (never `BadCookie`), and an entry inserted during a listing appears exactly once, at the end. `read_link` returns the stored target. `statfs` returns `total_bytes = max_bytes`, `free_bytes = avail_bytes = max_bytes − bytes_used`, `total_files = max_nodes`, `free_files = avail_files = max_nodes − node_count`. `lookup` with a name longer than the instance's `max_name_len` → `NameTooLong`. `Caller` is used only by the default `access`.
- **Concurrency.** One `RwLock` around the tree; it is never held across an `.await` (there are none). Lock poisoning is ignored (`PoisonError::into_inner`): mutations validate everything before changing state and do not panic inside critical sections.

### `vfs::testing` conformance kit

```rust
pub mod testing {
    /// One broken rule. `rule` is a stable id from the table below.
    #[non_exhaustive]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Violation { pub rule: &'static str, pub detail: String }

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct Report { /* private: violations, nodes */ }
    impl Report {
        pub fn is_ok(&self) -> bool;
        pub fn violations(&self) -> &[Violation];
        pub fn nodes(&self) -> usize;                 // nodes visited
        #[track_caller] pub fn assert_ok(&self);      // panics "vfs::testing: <Display>" if not ok
    }
    impl fmt::Display for Report;                     // "<n> violation(s) in <m> node(s)" + one line per violation

    /// Walks the tree from the root (breadth first, at most `max_nodes` nodes) and checks every rule.
    pub async fn check<F: Filesystem + ?Sized>(fs: &F, caller: &Caller, limits: &Limits, max_nodes: usize) -> Report;

    /// For every operation on every visited node: drop the future unpolled, and poll it exactly
    /// once with `Context::from_waker(Waker::noop())` then drop it; afterwards the same operation
    /// run to completion must equal the baseline; finally runs `check`.
    pub async fn check_cancellation<F: Filesystem + ?Sized>(fs: &F, caller: &Caller, limits: &Limits, max_nodes: usize) -> Report;
}
```

Preconditions (documented in rustdoc): the tree is not modified during the run; `caller` may read every node; `limits` are the server-side limits the backend must respect. The kit itself never panics on a misbehaving backend (e.g. it never slices `buf[..len]` with `len > buf.len()`), every loop is bounded (by `max_nodes`, page counts, and a repeated-cookie guard), and any unexpected `Err` is reported as a violation of the rule being checked. Files with `size ≤ limits.max_read_len` are read fully; larger files are probed at offsets `0`, `size − 1`, `size`, and `u64::MAX` only (bounded network cost for MEGA).

| Rule | `check` verifies |
|---|---|
| K01 | `info()` returns equal values on repeated calls; `max_read ≥ 1`; `max_name_len ≥ 1`. `root()` is stable. |
| K02 | `getattr(root)` succeeds, `kind == Dir`, `id == root`. |
| K03 | `parent(root)` returns the root's id. |
| K04 | For each node: `getattr(id).id == id`; two `getattr` calls agree on `id`, `generation`, `kind`, `size`, `change`. |
| K05 | `perm ≤ 0o7777`. |
| K06 | `read_dir` pages: `entries.len() ≤ max(1, max_entries)`; a non-`eof` page is non-empty; paging terminates. |
| K07 | Entry names pass `Name::new(name, limits)` and are unique per directory; no `.` / `..`. |
| K08 | Cookies are unique per directory and never 0, 1, or 2. |
| K09 | Listing with page size 1 and with `limits.max_dir_entries` gives the same `(name, id, kind, cookie)` sequence; resuming from each emitted cookie (with the returned verifier) yields exactly the suffix; `eof` is set on the page with the last entry. |
| K10 | `START` with a random verifier, `Cookie::DOT`, and `Cookie::DOT_DOT` each list from the first entry. |
| K11 | `with_attrs = false` → every `attr` is `None`; `true` → each `Some(attr)` has matching `id` and `kind` and equals `getattr` on `id`, `kind`, `size`, `generation`. |
| K12 | `lookup(dir, entry.name)` returns `entry.id` and `entry.kind`. |
| K13 | For each child directory: `parent(child).id == dir`. |
| K14 | `lookup(dir, <name absent from the listing>)` → `NotFound`. |
| K15 | On a non-directory: `lookup` / `read_dir` / `parent` → `NotADirectory`. |
| K16 | On a directory: `read` → `IsADirectory`. |
| K17 | Files: `len ≤ buf.len()`; progress (non-empty buf and `!eof` → `len ≥ 1`); `eof` exact; `offset == size` and `offset == u64::MAX` and `offset = u64::MAX − 3` with a 16-byte buffer → `(0, true)`; empty buffer → `(0, offset ≥ size)`; two reads of the same range return equal bytes; a full read of a small file returns exactly `size` bytes; a buffer larger than `info().max_read` never yields `len > buf.len()`. |
| K18 | Symlinks: `read_link` is non-empty, has no NUL, is ≤ `limits.max_symlink_len`; `read` → `InvalidInput`. Non-symlinks: `read_link` → `InvalidInput`. |
| K19 | `access(id, want)` ⊆ `want` for `want ∈ {empty, each single bit, all}`; `access(id, empty)` is empty. |
| K20 | `statfs(root)` succeeds. |
| K21 | **Stale probe**: for the first id in `u64::MAX, u64::MAX − 1, …` not seen during the walk, every id-taking operation (`getattr`, `lookup` as dir, `parent`, `access`, `read`, `read_dir`, `read_link`, `statfs`) returns `Stale`. |
| K22 | The walk visits at most `max_nodes` nodes (`Report::nodes() ≤ max_nodes`). |
| KC1 | (`check_cancellation`) Dropping each operation's future unpolled changes nothing observable. |
| KC2 | (`check_cancellation`) Polling each operation's future once with a no-op waker and dropping it changes nothing observable: the completed operation equals the baseline (`getattr` fields as in K04, `read` bytes, `read_dir` sequence, `read_link` bytes), and `check` afterwards reports no new violations. |

### API rationale

- **Static trait + `_dyn` twin** (Q1): zero-cost generic servers, `async fn` for backend authors, and runtime composition with one box per call (`opendal` `Service`/`ServiceDyn`, §References). The `_dyn` suffix exists because the blanket impl makes every backend implement both traits, and identical method names would be ambiguous (`E0034`) when both are imported (the same hazard `akku::io` documents for sync/async traits). No `'static` supertrait: borrowed backends (`&dyn DynFilesystem`, test fixtures on the stack) work, and spawning code states `'static` where it needs it.
- **`id_epoch` instead of `persistent_ids`** (Q2): a boolean cannot tell a server *which* instance issued a handle; an epoch lets `nfs::server` reject handles from a previous `MemFs` or a re-created MEGA session without a backend call, and keeps NFS handles valid across megaghost restarts (the `nfsserve` start-time handle problem, §References).
- **Stale, not NotFound, for ids** (Q2/Q6): RFC 1813 `nfs_fh3` says a handle to a gone object is `NFS3ERR_STALE`, which makes clients drop cached handles; `NOENT` would make the macOS client think a name lookup failed.
- **`Cookie` newtype with reserved 1/2** (Q5): servers can synthesize `.` / `..` without colliding with backend cookies, and a typed cookie cannot be confused with a `NodeId` (the `nfsserve` `start_after: fileid` bug class). Insertion-sequence cookies in `MemFs` give the strongest stability guarantee cheaply, which the macOS client relies on (`nfsserve` disabled `BAD_COOKIE` for it, §References).
- **Clamp vs error** (Q7): sizes are negotiable per protocol (NFS FSINFO `rtmax`: requests above max give short reads, §References), so clamping is correct and keeps clients working; names, paths, and lists are identities, and silently truncating them would address a different object.
- **Private `Caller` fields** (Q10): `max_groups` is then enforced at construction, and fields (deadline, auth flavor) can be added without breaking anyone.
- **Defaults call `getattr` first**: keeps contract 4 (Stale rule) true for every backend, at the cost of one metadata lookup on an error path.
- **`parent` returns `Attr`**: NFS `LOOKUP ".."` must return post-op attributes; returning only an id would force a second remote call in megaghost.
- **No `read_dir_all` in v1**: its result size is unbounded by any existing limit; designing a total-entries limit is deferred with the path layer.

### Seams

| Seam | Catches | Misses |
|---|---|---|
| `Name::new` | traversal (`/`, `.`, `..`), NUL, empty, length bound and check order | backend-specific charset rules |
| `Caller::with_groups`, `Limits` builders | group bound, default values | — |
| `Error` / `ErrorKind` + `io::Error` conversions | mapping table, exact round trip, Display, source chain | NFS mapping (lives in `nfs::server`) |
| `Access::from_mode` | POSIX class selection, root rule, dir vs file bit mapping, read-only masking | ACL backends |
| `Filesystem` on `MemFs` (`getattr`, `lookup`, `parent`, `read`, `read_dir`, `access`, `read_link`, `statfs`) | every contract in §Contracts on a reference backend | remote latency behaviour |
| `MemFs` build API | budgets, all-or-nothing, id non-reuse, Stale after remove, attribute bookkeeping | — |
| `resolve` | path limits, validation before I/O, no symlink following | path layers above it |
| `DynFilesystem` + forwarding impls | parity of dyn / `Arc` / `Box` / `&` with static calls, override forwarding, `Send` futures, no `E0034` | — |
| `testing::check`, `check_cancellation` | that the kit passes on `MemFs` **and** flags each rule on deliberately broken backends | rules outside the kit (performance) |

### G3 test plan

All tests live in `crates/akku/src/vfs/tests.rs` (public API only), drive futures with `akku::task::block_on`, and use these test-local helpers: `fixture()` → `MemFs` with `/a.txt` (`b"hello"`), `/dir/`, `/dir/b.bin` (1000 bytes of a known pattern), `/link` → `b"a.txt"`; `CountingFs<F>` (counts calls per operation, delegates); `PendingOnce<F>` (each operation awaits `akku::task::yield_now()` once before delegating, so a drop happens at a real await point); `Overriding` (overrides `access` to return `empty()` and `statfs` to fixed numbers); and one `Broken*` wrapper per kit rule. Gate commands for G3/G4: `smd cargo test -p akku --test vfs`, `smd cargo clippy -p akku --all-targets -- -D warnings`, **`smd cargo +1.85 check -p akku --all-targets`**.

**Types and static assertions**
1. `MemFs`, `Error`, `Caller`, `Limits`, `Attr`, `DirPage`, `Entry`, `Report`, `dyn DynFilesystem` are `Send + Sync`; `Error: std::error::Error + 'static`.
2. A generic `fn spawnable<F: Filesystem + ?Sized + 'static>(Arc<F>) -> impl Future + Send + 'static` calling every operation compiles for `MemFs`, `dyn DynFilesystem`, `Arc<dyn DynFilesystem>`, `Box<dyn DynFilesystem>`; the future runs under `akku::task::spawn`.
3. `resolve(&dyn DynFilesystem, ..)` and every `Filesystem` method on `&MemFs` return `Send` futures.
4. With `use akku::vfs::{Filesystem, DynFilesystem};` in scope, `fs.getattr(..)`, `fs.info()`, `fs.root()` on `MemFs` compile (no `E0034`).
5. `NodeId::new(0)` is `None`; `NodeId::new(1).unwrap().get() == 1`; `NodeId::new(u64::MAX)` round-trips; `From<NonZeroU64>` / `Into<u64>` agree.
6. `Cookie::{START, DOT, DOT_DOT, FIRST}` are 0/1/2/3; `is_start` is true exactly for 0, 1, 2; `DirCursor::START == DirCursor::default()`.
7. `Access` constants equal RFC 1813 values `0x01, 0x02, 0x04, 0x08, 0x10, 0x20`; `from_bits_truncate(u32::MAX) == all()`; `all().bits() == 0x3f`; `BitOr` / `BitAnd` / `contains` behave on disjoint and overlapping sets.
8. `Limits::new() == Limits::default()` and equals the defaults table field by field; each `with_*` changes only its field.
9. `Attr::new` defaults per kind (perm 0o644 / 0o755 / 0o777, nlink 1 / 2 / 1, times `UNIX_EPOCH`, change 0); `FsInfo::new` defaults; `FsStat::unknown()` all zero; `Entry::new(..).attr.is_none()`; `with_attr` sets it.

**Name validation**
10. Rejected with `InvalidName`: `b""`, `b"."`, `b".."`, `b"/"`, `b"a/b"`, `b"/a"`, `b"a/"`, `b"\0"`, `b"a\0b"`.
11. Accepted: `b"..."`, `b".a"`, `b"a."`, `b" "`, `b"a\\b"`, `[0xff, 0xfe]` (`to_str() == None`), `"ü€𝄞"` (`to_str() == Some`), `b"\x01\x7f"`.
12. Length: 255 bytes accepted; 256 bytes → `NameTooLong`; 256 bytes containing `/` → `NameTooLong` (length checked before content); `with_max_name_len(0)`: `b"a"` → `NameTooLong`, `b""` → `InvalidName`; `with_max_name_len(usize::MAX)` accepts a 64 KiB name.
13. `as_bytes` returns the input slice; `Debug` of a non-UTF-8 name does not panic and shows escapes.

**Caller and access**
14. `Caller::new(5, 7)` getters; `with_groups` with 16 groups OK, 17 → `InvalidInput`; with `with_max_groups(0)` an empty slice is OK and one group → `InvalidInput`; a second `with_groups` replaces the first.
15. `Access::from_mode` table: owner / group (primary gid) / group (supplementary) / other / uid 0 for perm `0o750` on File and Dir, with expected sets written out by hand (e.g. owner on Dir `0o750` → `READ|LOOKUP|MODIFY|EXTEND|DELETE`; other → empty; root on File `0o640` → `READ|MODIFY|EXTEND` without `EXECUTE`); `read_only = true` strips `MODIFY|EXTEND|DELETE`.
16. Default `MemFs::access` returns `want ∩ from_mode`, never bits outside `want`, and `Stale` for a removed id.

**Errors**
17. `Display`: `Error::new(Stale, "")` prints the kind text; with a message prints `"<kind>: <msg>"`; `source()` returns the attached source.
18. `vfs → io` kind for every `ErrorKind` matches the table (exhaustive list in the test; a new kind fails the test until added).
19. `vfs → io → vfs` preserves kind and message for every kind (including `InvalidName`, `BadCookie`, `NotPermitted`, which share `io` kinds).
20. `io → vfs` for plain `io::Error`s: `TimedOut`, `ResourceBusy`, `WouldBlock`, `Interrupted` → `TryAgain`; `StaleNetworkFileHandle` → `Stale`; `InvalidData`, `Other`, `BrokenPipe` → `Io`; the `io::Error` is the source.

**getattr / lookup / parent**
21. `getattr(root)`: `Dir`, `id == root`, `nlink == 3` in `fixture()` (2 + `/dir`), `size` = entry count.
22. `lookup` returns attributes equal to `getattr` of the child; `lookup` of a missing name → `NotFound`; `lookup` on a file and on a symlink as `dir` → `NotADirectory`; `lookup(root, "link")` returns the symlink itself (`kind == Symlink`, `size == 5`).
23. `parent(root) == root`; `parent(/dir) == root`; `parent(/dir/sub) == /dir`; `parent(file)` and `parent(symlink)` → `NotADirectory`.

**read**
24. Exact content and `eof`: `read(a.txt, 0, [0; 5]) == (5, true)`; `[0; 4] → (4, false)`; `[0; 64] → (5, true)` and `buf[5..]` untouched is **not** asserted (unspecified).
25. `offset == size` → `(0, true)`; `offset = 1 << 40` → `(0, true)`; `offset = u64::MAX` → `(0, true)`; `offset = u64::MAX − 3` with a 16-byte buffer → `(0, true)` (no overflow panic in debug).
26. Empty buffer: `(0, false)` at offset 0 of a non-empty file, `(0, true)` at `size`.
27. Clamp: `MemFs::with_limits(Limits::new().with_max_read_len(4), ..)` on a 10-byte file reads `4/false`, `4/false`, `2/true`; `with_max_read_len(0)` behaves as 1 and still makes progress.
28. `read` on a dir → `IsADirectory`; on a symlink → `InvalidInput`.
29. Large file: a 3 MiB file read with a 3 MiB buffer returns 1 MiB (default clamp), and a loop of reads reassembles the exact pattern.

**read_dir and cookies**
30. Empty dir: `START` → no entries, `eof == true`, verifier 0.
31. Insertion order and cookies: entries created as `c`, `a`, `b` list as `c, a, b` with strictly increasing cookies ≥ 3.
32. `max_entries = 1` over 5 entries gives 5 pages of one entry; only the 5th has `eof == true`; resuming from the last cookie gives an empty `eof` page.
33. `max_entries = 0` behaves as 1; `max_entries = usize::MAX` is clamped to `limits.max_dir_entries`.
34. 10 000 entries with the default limit: pages of 4096, 4096, 1808; the union is every entry exactly once; with `with_max_dir_entries(7)` the pages are ≤ 7.
35. `START` with verifier `0xdead_beef`, `Cookie::DOT`, and `Cookie::DOT_DOT` all list from the first entry.
36. Arbitrary cookies: a never-emitted cookie between two emitted ones resumes after it; `Cookie::new(u64::MAX)` → empty, `eof == true`; never `BadCookie` on `MemFs`.
37. **Cookie stability under insert/delete** (single-threaded interleaving): page with `max_entries = 1`; between pages remove the next unlisted entry, remove an already-listed entry, remove the entry whose cookie is the cursor, and insert a new entry. Every entry present for the whole listing appears exactly once, no entry appears twice, the removed-before-listed entry never appears, and the inserted entry appears exactly once at the end.
38. `with_attrs = false` → all `attr == None`; `true` → all `Some` and equal to `getattr` of the entry id.
39. `read_dir` on a file and on a symlink → `NotADirectory`.

**Stale ids and identity**
40. On a fresh `MemFs`, `NodeId::new(u64::MAX)` and `NodeId::new(2)` give `Stale` from all 8 id-taking operations (never `NotFound`).
41. Remove a file, then all 8 operations with its id → `Stale`; remove an empty dir, then `lookup` / `read_dir` / `parent` with it as dir → `Stale`.
42. Ids are never reused: create `x`, remove it, create `x` again → a different, larger id; the old id stays `Stale`; also after the node budget is restored.
43. Two `MemFs` instances have different `id_epoch`s; one instance reports the same `info()` on every call.
44. Attribute bookkeeping: creating an entry increments the dir's `change` by exactly 1 and updates `nlink` for `mkdir`; `write_all` increments the file's `change` by 1 and `mtime` does not go backwards; `read` / `read_dir` / `lookup` never change any `Attr`.

**MemFs build API and limits**
45. `create_file` / `mkdir` / `symlink` name errors: invalid name → `InvalidName`, 256 bytes → `NameTooLong`, duplicate → `AlreadyExists`, non-dir parent → `NotADirectory`, removed parent → `Stale`.
46. Symlink targets: exactly `max_symlink_len` (4096) accepted and returned byte-exact by `read_link`; 4097 → `NameTooLong`; empty and NUL-containing → `InvalidInput`; with `with_max_symlink_len(8)`, 8 OK and 9 → `NameTooLong`.
47. `max_nodes`: `with_limits(.., max_nodes = 3, ..)`: two creates succeed (root counts), the third → `NoSpace`, and `node_count()` stays 3; after a `remove`, a create succeeds again.
48. `max_bytes` exact boundary: with `max_bytes = 10`, `create_file(root, b"a", 9 bytes)` succeeds (`bytes_used == 10`); `create_file(root, b"b", b"")` → `NoSpace`; `write_all(a, 9, b"x")` → `NoSpace` and the file is unchanged (all-or-nothing).
49. Sparse writes: `write_all(f, 1 << 40, b"x")` on a default `MemFs` → `NoSpace` quickly with `bytes_used` unchanged; `write_all(f, u64::MAX − 1, b"xyz")` → `FileTooLarge`; `set_len(f, u64::MAX)` → `FileTooLarge`; `write_all` past the end zero-fills (bytes read back as `0`).
50. `write_all` / `set_len` on a dir → `IsADirectory`, on a symlink → `InvalidInput`; `set_perm(f, 0o10000)` → `InvalidInput`, `0o7777` OK.
51. `remove`: non-empty dir → `DirectoryNotEmpty`; missing name → `NotFound`; budgets return (`bytes_used` and `node_count` drop by the removed amounts).
52. `statfs` on `MemFs` reports `max_bytes` / `max_nodes` and the free counters from the accounting formula.

**resolve**
53. `b""`, `b"/"`, `b"//"` → root; `b"/dir/b.bin"`, `b"dir//b.bin/"` → `b.bin`; missing component → `NotFound`.
54. `b"dir/./b.bin"`, `b"dir/../a.txt"`, `b".."` → `InvalidName`, with **zero** backend calls (`CountingFs`).
55. Path length: exactly 4096 bytes passes validation (then `NotFound`); 4097 → `NameTooLong` with zero backend calls.
56. Depth: a 256-deep `MemFs` chain resolves; 257 components → `InvalidInput` with zero backend calls.
57. Symlinks: `b"link"` returns the symlink; `b"link/x"` → `NotADirectory`; `b"a.txt/x"` → `NotADirectory`.

**Cancellation**
58. For each of the 8 async operations on `MemFs`: drop unpolled; poll once with `Waker::noop()` and drop; then the completed operation equals the baseline and `node_count` / `bytes_used` are unchanged.
59. Same as 58 on `PendingOnce<MemFs>` (the first poll is `Pending`, so the drop happens at a real await point).
60. `testing::check_cancellation` reports OK on `fixture()` and on `PendingOnce<fixture()>`.

**Dyn vs static parity**
61. The same scripted operation sequence (including error cases) gives equal results through `MemFs`, `&MemFs`, `Arc<MemFs>`, `Box<MemFs>`, `&dyn DynFilesystem`, `Arc<dyn DynFilesystem>`, `Box<dyn DynFilesystem>`, and the `*_dyn` methods called directly.
62. Overrides are forwarded: `Overriding`'s `access` and `statfs` results are seen through all 7 wrappers (not the defaults).

**Testing kit**
63. `testing::check` reports OK on `MemFs::new()`, `fixture()`, a 10 000-entry directory, and a 256-deep chain; `Report::nodes()` counts visited nodes; `max_nodes = 2` stops early (K22) without violations.
64. Each `Broken*` backend triggers its rule id and only stops the run where unsafe to continue: `len > buf.len()` → K17 (and the kit does not panic); empty non-eof page → K06 (and terminates); duplicate names → K07; emitted cookie 1 → K08; resume returns a wrong suffix → K09; `with_attrs = false` returns `Some` → K11; `getattr` returns a different id → K04; `parent(root) != root` → K03; `NotFound` instead of `Stale` → K21; `read_link` longer than `max_symlink_len` → K18.
65. `Report::assert_ok` panics with a message starting `vfs::testing:` on a broken backend (`#[should_panic(expected = "vfs::testing:")]`) and does not panic on `MemFs`.

**Concurrency**
66. `Arc<MemFs>` shared by 4 `std::thread`s, each running `block_on` loops of `getattr` / `read` / `read_dir` / `resolve`, while the main thread creates and removes 1000 files: no panic, no deadlock; every observed `ReadResult` honours `len ≤ buf.len()`; every completed listing has unique names and cookies.
67. Concurrent `create_file` of the same name from 4 threads: exactly one succeeds, the others get `AlreadyExists`; `node_count` grows by exactly 1.

### Notes for consumers

**`nfs::server`:**
- Handle layout `[version u8 | fsid u64 | id_epoch u64 | NodeId u64 | generation u64 | optional MAC 16]` = 49 ≤ 64 bytes (`NFS3_FHSIZE`). Epoch or generation mismatch → `NFS3ERR_STALE` without a backend call when possible.
- Effective `rtmax` = `min(info().max_read, limits.max_read_len)`; clamp client `count` to it before calling `read`.
- Clamp READDIR `count` / `maxcount` byte budgets itself (it owns the XDR sizes); pass `max_entries ≤ limits.max_dir_entries`. Synthesize `.` (cookie 1) and `..` (cookie 2, via `parent`) and pass `Cookie::START` to the backend for cookies 0–2.
- Validate backend output defensively (`len ≤ count`, entry names re-checked, `read_link` length) and answer `NFS3ERR_SERVERFAULT` on violations.
- Map unknown future `ErrorKind`s to `NFS3ERR_IO`, and answer `NFS3ERR_IO` for a future `NodeKind` the server does not map yet; clamp `SystemTime` into `nfstime3`; mask `perm` to `0o7777`.

**megaghost (MEGA backend):**
- `NodeId` from the MEGA node handle (48 significant bits, non-zero); `generation` 0 (handles are never reused); `id_epoch` constant per account + root handle, so macOS keeps working handles across restarts.
- Cookie suggestion: order a directory by child handle and use `handle + 3` as the cookie (no overflow under 2^48); resuming with `range(cookie + 1..)` then survives inserts and deletes with verifier 0.
- MEGA allows duplicate names in one folder: mangle deterministically and stably (e.g. append a handle suffix) so names are unique.
- macOS probes `._*` (AppleDouble) and `.DS_Store` constantly: answer those `lookup`s with a fast `NotFound` from the cached tree, never a network round trip.
- Return `TryAgain` (→ `NFS3ERR_JUKEBOX`) while a chunk is being fetched rather than blocking the request or failing with `Io`; never block the thread (contract 2).
- Removed remote nodes → `Stale` (contract 4); quota → `statfs`; remote changes bump `change` / `mtime` / `size` (contract 9).
- Run `vfs::testing::check` and `check_cancellation` against a recorded fixture tree in megaghost's CI.
