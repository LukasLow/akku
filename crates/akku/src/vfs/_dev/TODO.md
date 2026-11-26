# vfs — backlog

Researched but not in v1. v1 (G2, `DESIGN.md` §G2: API) is `Filesystem` + `DynFilesystem`, the core types, `resolve`, `MemFs`, and the `vfs::testing` conformance kit.

## Write side
- `FilesystemWrite: Filesystem` + `DynFilesystemWrite` (`*_dyn`): `setattr`, `write` (with `stable_how`-style durability and a write verifier), `commit`, `create` (unchecked, guarded, or exclusive), `mkdir`, `symlink`, `link`, `remove`, `rmdir`, `rename`. Lands additively together with `nfs::server` write support; `MemFs` then implements it and its inherent build API stays as the fixture shortcut. (origin: `DESIGN.md` Q3 / RFC 1813 §3.3.2, §3.3.7–§3.3.15, §3.3.21)
- `FsInfo` fields for writes: `max_write`, write verifier (`writeverf3`), `can_set_time`; `Limits::max_write_len` (default 1 MiB, clamp). Both structs are `#[non_exhaustive]`, so this is additive. (origin: `DESIGN.md` Decisions (G2) Q3/Q7)
- `SetAttr` type mirroring `sattr3` (optional mode, uid, gid, size, atime, mtime; set to server time or client time). (origin: `DESIGN.md` §References / RFC 1813 `sattr3`)
- `MemFs` hard links (`FsInfo::hard_links = true`, `nlink > 1`, parent of a multiply-linked directory is unspecified). (origin: `DESIGN.md` §G2: API › MemFs)

## Backends and wrappers
- `LocalFs`: passthrough over a local directory. Needs `task` Phase 4 `spawn_blocking`, native stable identity on all five Tier-1 OSes (Unix `MetadataExt::{dev, ino}`; Windows `GetFileInformationByHandle` in an allowlisted `sys` submodule while `file_index` is unstable), an inode-reuse/generation side table, a persistent `id_epoch`, and race-free confinement (`openat`/`O_NOFOLLOW` style, Go `os.Root`). (origin: `DESIGN.md` Q8)
- `ReadOnly<F>`: wrapper that hides write capability and reports `read_only = true`. (origin: `DESIGN.md` Q8 / rust-vfs `altroot`/`overlay`)
- `Sub<F>`: view rooted at a sub-directory (`root()` = the sub-directory, `parent` clamped at it). (origin: `DESIGN.md` §References / Go `fs.Sub`, `virtual-filesystem` `ScopedFS`)
- `Overlay` / union and `Mount` table that compose several backends under one root (uses `Arc<dyn DynFilesystem>`; needs an id-space partitioning scheme). (origin: `DESIGN.md` Q1 / rust-vfs `overlay`, `virtual-filesystem` `MountableFS`)
- `Cached<F>`: optional TTL attribute, lookup, negative-lookup, and directory cache wrapper. (origin: `DESIGN.md` Q13 / RFC 1813 §4.11)

## Ergonomics and testing
- Path-based convenience layer on top of `resolve`: `resolve_at(fs, caller, dir, path, limits)`, `read_dir_all` with its own total-entries limit (dropped from v1 because its result is unbounded), `read_to_end(fs, path)`, `metadata(fs, path)`, `walk`. (origin: `DESIGN.md` §Non-goals, Decisions (G2) / Go `io/fs`)
- `AsyncRead` / `AsyncSeek` adapter (`vfs::Reader`) over `(fs, NodeId)` for streaming consumers (HTTP or WebDAV servers). Needs `akku::io::AsyncSeek` (io `_dev/TODO.md`). (origin: `DESIGN.md` §Dependencies)
- Zero-copy `read_bytes -> akku::bytes::Bytes` optional method, if profiling shows copies matter. (origin: `DESIGN.md` Q12)
- `testing` kit extensions: mutation-aware checks (cookie stability while the kit itself inserts/removes, for writable backends), a `check_write` once `FilesystemWrite` exists, and a seeded random-operation fuzzer comparing a backend against `MemFs`. (origin: `DESIGN.md` §G2: API › testing kit / Go `testing/fstest`)
- Optional atime updates in `MemFs` (v1 never changes attributes on read). (origin: `DESIGN.md` §G2: API › MemFs)

## Identity and protocol extensions
- Backend-defined opaque handle bytes (Linux `encode_fh` style) if `NodeId(u64)` + generation + `id_epoch` proves too narrow for a backend. (origin: `DESIGN.md` Q2 / Linux `exporting.rst`)
- Deadline, cancellation token, and auth flavor on `Caller` (fields are private, so additive). (origin: `DESIGN.md` Q10)
- More `NodeKind`s (fifo, socket, block/char device) with MKNOD and `rdev`. (origin: `DESIGN.md` Decisions (G2) Q4 / RFC 1813 `ftype3`, §3.3.11)
- Extended attributes, ACLs, birth time (`crtime`), byte-range locks (NLM), server-side copy / `copy_file_range`, `fallocate`. (origin: `DESIGN.md` §Non-goals / fuser `Filesystem`)
- Change notifications / watch stream for cache invalidation, which servers could turn into change-attribute updates. (origin: `DESIGN.md` §Non-goals; GUESS: the MEGA API pushes tree changes)
- Other servers over `vfs`: 9P, WebDAV, FUSE-like. Each is a separate module or `nfs`-style extension, not part of `vfs`. (origin: `DESIGN.md` §Purpose)
