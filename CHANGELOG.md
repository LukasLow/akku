# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[changie](https://changie.dev/) for fragments.

## 0.0.2 - 2026-11-28
### Added
* Land 37 modules in a one-folder-per-module layout (each with tests.rs and meta.toml), apply the Wave-2 security bugfix pass, and add the akku catalog gen/check generator.
* Add an optional MSRV 1.85.0 CI job to verify workspace compatibility with the declared minimum supported Rust version while keeping normal builds on stable.
* Add `akku::xdr`, a zero-dependency XDR (RFC 4506) encoder and decoder with DoS limits, strict padding and bool checks, and rollback on encode errors.
* `akku::bytes::varint` adds LEB128 and zigzag varints (Go `encoding/binary` `Uvarint`/`Varint` parity, protobuf-compatible) with lenient and strict decoders, plus `BytesMut::put_uvarint`/`put_varint` and `Cursor::get_uvarint`/`get_varint`.
* `html::unescape` decodes HTML character references (decimal/hex numeric, Windows-1252 C1 remapping, the full WHATWG named-entity table with legacy no-semicolon forms) and borrows its input when there is nothing to decode.
* Add `akku::task` (zero-dependency single-thread async runtime with `spawn`/`JoinHandle`, limits and a cooperative budget), `akku::io` (`std::io` superset with `AsyncRead`/`AsyncWrite`/`AsyncBufRead` and `copy_to`), and `akku::net` (`std::net` superset with `AsyncTcpListener`/`AsyncTcpStream` on native epoll, kqueue and WSAPoll backends).
### Changed
* Relicense the repository and future releases under MPL-2.0, matching dree.
* **Breaking:** `akku::time::Duration` is renamed to `akku::time::SignedDuration` (after jiff) and the mockable clock reading `akku::time::Instant` to `akku::time::ClockInstant`; `akku::time::{Duration, Instant}` are now `std::time`'s exact items, re-exported by explicit list together with `SystemTime`, `SystemTimeError`, `TryFromFloatSecsError`, and `UNIX_EPOCH`. New conversions: `SignedDuration` <-> `std::time::Duration` (`TryFrom` both ways, rejecting negatives and overflow), `std::time::Instant` -> `ClockInstant` (`From`) and back (`TryFrom`, rejecting `MockClock` readings with the new `TimeError::MockInstant`), and `OffsetDateTime` <-> `SystemTime` (`TryFrom`). `akku::env::home_dir` is now a re-export of `std::env::home_dir` instead of akku's own function (an empty `HOME` no longer yields `None`: std returns `Some("")` on Rust 1.85 and falls back to the OS user lookup on newer toolchains; it carries std's deprecation warning on Rust < 1.87), and `akku::env`, `akku::fs`, and `akku::process` add the missing stable std re-exports (`env::{consts, Args, ArgsOs, JoinPathsError, SplitPaths, VarError, Vars, VarsOs}`, `fs::{exists, DirBuilder, FileTimes}`, `process::{abort, CommandArgs, CommandEnvs, Termination}`).
* Switch `akku` from `#![forbid(unsafe_code)]` to `#![deny(unsafe_code)]` with `#![deny(unsafe_op_in_unsafe_fn)]`, `#![deny(clippy::undocumented_unsafe_blocks)]`, and a CI-enforced `sys`-only allowlist (still empty), and ship `LICENSE` and `THIRD_PARTY_LICENSES` in published crates.
### Fixed
* `hex::encoded_len` now panics with a clear message when `input_len * 2` overflows `usize`, instead of silently wrapping to a too-small length in release builds.
* Windows: `fs::glob` now accepts drive/UNC prefixes and `\` separators (e.g. `C:\logs\*.txt`), and archive extraction rejects path components Windows would reinterpret (`:` drive prefixes and alternate data streams, reserved device names like `CON`/`NUL`/`COM1`, trailing dots or spaces), closing a zip-slip escape via entries like `a/C:evil`. Unix behaviour is unchanged.

## 0.0.1 - 2026-10-25
### Added
* Initial `akku` HTML and HTTP modules.
* `akku-drive` attribute macros: `get`, `post` and `main`.
* `akku-cli` project scaffolder.
* Namespace constitution, concept paper and HTTP API docs.
