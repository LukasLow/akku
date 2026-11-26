# vfs — backlog

Deferred candidates from the G1 proposal. Items whose origin is a design question (Q*) depend on the human's G1 answers.

## Write side
- `FilesystemWrite: Filesystem` + `DynFilesystemWrite`: `setattr`, `write` (with `stable_how`-style durability and a write verifier), `commit`, `create` (unchecked, guarded, or exclusive), `mkdir`, `symlink`, `link`, `remove`, `rmdir`, `rename`. Lands additively together with `nfs::server` write support, and `MemFs` then implements it. (origin: `DESIGN.md` Q3 / RFC 1813 §3.3.2, §3.3.7–§3.3.15, §3.3.21)
- `SetAttr` type mirroring `sattr3` (optional mode, uid, gid, size, atime, mtime; set to server time or client time). (origin: `DESIGN.md` §References / RFC 1813 `sattr3`)

## Backends and wrappers
- `LocalFs`: passthrough over a local directory. Needs `task` Phase 4 `spawn_blocking`, native stable identity on all five Tier-1 OSes (Unix `MetadataExt::{dev, ino}`; Windows `GetFileInformationByHandle` in an allowlisted `sys` submodule while `file_index` is unstable), an inode-reuse/generation side table, and race-free confinement (`openat`/`O_NOFOLLOW` style, Go `os.Root`). (origin: `DESIGN.md` Q8)
- `ReadOnly<F>`: wrapper that hides write capability and reports `read_only = true`. (origin: `DESIGN.md` Q8 / rust-vfs `altroot`/`overlay`)
- `Sub<F>`: view rooted at a sub-directory. (origin: `DESIGN.md` §References / Go `fs.Sub`, `virtual-filesystem` `ScopedFS`)
- `Overlay` / union and `Mount` table that compose several backends under one root (uses `Arc<dyn DynFilesystem>`). (origin: `DESIGN.md` Q1 / rust-vfs `overlay`, `virtual-filesystem` `MountableFS`)
- `Cached<F>`: optional TTL attribute, lookup, and directory cache wrapper. (origin: `DESIGN.md` Q13 / RFC 1813 §4.11)

## Ergonomics and testing
- `vfs::testing` conformance kit (Go `fstest.TestFS`-style) that any backend, including megaghost's MEGA backend, can run against. Candidate for v1 if the human pulls it in. (origin: `DESIGN.md` Q8 / Go `testing/fstest`)
- Path-based convenience layer (`read_to_end(fs, path)`, `metadata(fs, path)`, `walk`) on top of `resolve`. (origin: `DESIGN.md` §Non-goals / Go `io/fs`)
- `AsyncRead` / `AsyncSeek` adapter (`vfs::Reader`) over `(fs, NodeId)` for streaming consumers (HTTP or WebDAV servers). Needs `akku::io::AsyncSeek` (io `_dev/TODO.md`). (origin: `DESIGN.md` §Dependencies)
- Zero-copy `read_bytes -> akku::bytes::Bytes` optional method, if profiling shows copies matter. (origin: `DESIGN.md` Q12)

## Identity and protocol extensions
- Backend-defined opaque handle bytes (Linux `encode_fh` style) if `NodeId(u64)` + generation proves too narrow for a backend. (origin: `DESIGN.md` Q2 / Linux `exporting.rst`)
- Deadline and cancellation token on `Caller`. (origin: `DESIGN.md` Q10)
- Extended attributes, ACLs, birth time (`crtime`), `rdev` / device nodes (`MKNOD`), byte-range locks (NLM), server-side copy / `copy_file_range`, `fallocate`. (origin: `DESIGN.md` §Non-goals / fuser `Filesystem`, RFC 1813 §3.3.11)
- Change notifications / watch stream for cache invalidation, which servers could turn into change-attribute updates. (origin: `DESIGN.md` §Non-goals; GUESS: the MEGA API pushes tree changes)
- Other servers over `vfs`: 9P, WebDAV, FUSE-like. Each is a separate module or `nfs`-style extension, not part of `vfs`. (origin: `DESIGN.md` §Purpose)
