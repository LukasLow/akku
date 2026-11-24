# Go Standard Library Gap Checklist

This checklist uses the Go standard library as a **gap finder**: every Go
package is mapped to where the same capability lives for an akku user. The goal
is to spot what akku is missing, not to copy Go's API or package layout. akku
keeps its own rules: flat modules first, families only per
[`Namespace.md`](../Namespace.md), zero dependencies by default.

- **Source:** `go list std` with Go 1.27.3 (`nix shell nixpkgs#go -c go list std`),
  180 packages after removing `internal/`, `vendor/`, and `cmd/` packages.
- **Namespace baseline:** the current decisions (TODO 0.4): a flat `net`
  (superset of `std::net`) plus flat `dns` and `quic` instead of a `net`
  family; `xdr`, `vfs`, and `nfs` planned; an OS layer `akku::os::<platform>`
  plus flat unified domain modules (TODO 0.7).
- **Waves:** `W1`/`W2`/`W3` are the waves in [`ROADMAP.md`](ROADMAP.md);
  `0.7` and `0.9` are TODO.md sections (OS layer RFC, megaghost-driven libs).

## Status legend

| Status | Meaning |
| --- | --- |
| `Rust std` | Covered by `core`/`alloc`/`std` or the language itself. |
| `akku` | Shipped in akku today (path exists in `crates/akku/src/`). |
| `planned` | Listed in `Namespace.md` / `ROADMAP.md` / TODO.md, not shipped yet. |
| `missing → proposal` | Not covered and not planned; a flat-first name is suggested. |
| `n/a` | Not applicable to akku (Go toolchain, runtime, reflection, legacy). |

## Counts

| Status | Packages |
| --- | --- |
| `Rust std` | 29 |
| `akku` | 34 |
| `planned` | 46 |
| `missing → proposal` | 29 |
| `n/a` | 42 |
| **Total** | **180** |

## Known gaps in shipped modules

Rows marked `akku` or `planned` that are only partly covered today:

1. `akku::process` has no signal handling, although Namespace lists it (`os/signal`).
2. `akku::http` has no cookie support yet (`net/http/cookiejar`).
3. `akku::html` has no public `unescape` (`html`).
4. `akku::archive::checksum` has CRC-32 (IEEE) only, no CRC-32C (`hash/crc32`).
5. `akku::time` has fixed UTC offsets only, no IANA zones (`time`, `time/tzdata`).
6. `akku::bytes` has no LEB128/zigzag varints, although its `meta.toml` summary
   and the catalog claim them (`encoding/binary`).

---

## archive

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `archive/tar` | `akku` | `akku::archive::tar` | Native reader/writer. |
| `archive/zip` | `akku` | `akku::archive::zip` | Native reader/writer. |

## bufio

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `bufio` | `Rust std` | `std::io::{BufReader, BufWriter, BufRead}` | `Scanner` maps to `BufRead::lines`/`split`. |

## bytes

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `bytes` | `Rust std` | `[u8]`, `Vec<u8>`, `std::io::Cursor` | Shared/zero-copy buffers in `akku::bytes`. |

## cmp

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `cmp` | `Rust std` | `std::cmp` | `Ord`, `min`/`max`, `Ordering::then`. |

## compress

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `compress/bzip2` | `missing → proposal` | `akku::archive::bzip2` | Decode-only first; still common in source tarballs. |
| `compress/flate` | `akku` | `akku::archive::deflate` | Raw DEFLATE with `CompressionLevel`. |
| `compress/gzip` | `akku` | `akku::archive::gzip` | |
| `compress/lzw` | `missing → proposal` | `akku::archive::lzw` | Needed by GIF and TIFF decoders in `pic`. |
| `compress/zlib` | `missing → proposal` | `akku::archive::zlib` | RFC 1950 wrapper over existing `deflate` + `Adler32`; PNG needs it. |

## container

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `container/heap` | `Rust std` | `std::collections::BinaryHeap` | |
| `container/list` | `Rust std` | `std::collections::LinkedList` | |
| `container/ring` | `Rust std` | `std::collections::VecDeque` | Fixed-size rings via `VecDeque` with capacity. |

## context

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `context` | `planned` | `akku::task` (W2) | Namespace: "Timeout, Cancellation"; request-scoped values stay explicit. |

## crypto

All `planned` rows are the `akku::crypto` family (W2, audited fork/adapter only,
never from scratch).

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `crypto` | `planned` | `akku::crypto` (W2) | Family root: `hash`, `aead`, `sign`, `kem`, `kdf`, `rand`. |
| `crypto/aes` | `planned` | `akku::crypto::aead` (W2) | Exposed as AEAD modes (AES-GCM), not raw block cipher. |
| `crypto/cipher` | `planned` | `akku::crypto::aead` (W2) | AEAD interface; raw CBC/CTR modes deliberately not exposed. |
| `crypto/des` | `n/a` | - | Broken legacy cipher; deliberately excluded. |
| `crypto/dsa` | `n/a` | - | Deprecated in Go; deliberately excluded. |
| `crypto/ecdh` | `planned` | `akku::crypto::kem` (W2) | X25519 / P-256 key agreement. |
| `crypto/ecdsa` | `planned` | `akku::crypto::sign` (W2) | |
| `crypto/ed25519` | `planned` | `akku::crypto::sign` (W2) | |
| `crypto/elliptic` | `n/a` | - | Low-level curve API, deprecated in Go; covered by `kem`/`sign`. |
| `crypto/fips140` | `n/a` | - | Go FIPS module switch; certification is a property of the chosen fork/adapter. |
| `crypto/hkdf` | `planned` | `akku::crypto::kdf` (W2) | |
| `crypto/hmac` | `planned` | `akku::crypto::hash` (W2) | MAC over the hash family. |
| `crypto/hpke` | `planned` | `akku::crypto::kem` (W2) | RFC 9180 as a composition of `kem` + `kdf` + `aead`. |
| `crypto/md5` | `planned` | `akku::crypto::hash` (W2) | Legacy, non-security use only (checksums, interop). |
| `crypto/mldsa` | `planned` | `akku::crypto::sign` (W2) | Post-quantum signature (FIPS 204). |
| `crypto/mlkem` | `planned` | `akku::crypto::kem` (W2) | Post-quantum KEM (FIPS 203). |
| `crypto/mlkem/mlkemtest` | `n/a` | - | Go test-vector helper. |
| `crypto/pbkdf2` | `planned` | `akku::crypto::kdf` (W2) | Password hashing itself is `akku::auth::password` (Argon2id). |
| `crypto/rand` | `planned` | `akku::crypto::rand` (W2) | OS CSPRNG. |
| `crypto/rc4` | `n/a` | - | Broken stream cipher; deliberately excluded. |
| `crypto/rsa` | `planned` | `akku::crypto::sign` (W2) | RSA-PSS / PKCS#1 v1.5 verify for interop. |
| `crypto/sha1` | `planned` | `akku::crypto::hash` (W2) | Legacy; a private SHA-1 already exists inside `akku::ws` for the handshake. |
| `crypto/sha256` | `planned` | `akku::crypto::hash` (W2) | |
| `crypto/sha3` | `planned` | `akku::crypto::hash` (W2) | Includes SHAKE. |
| `crypto/sha512` | `planned` | `akku::crypto::hash` (W2) | |
| `crypto/subtle` | `planned` | `akku::crypto` (W2) | Constant-time compare; not named in Namespace, belongs at the family root. |
| `crypto/tls` | `planned` | `akku::tls` (W2) | `#[doc(alias = "ssl")]`; never `akku::ssl`. |
| `crypto/x509` | `planned` | `akku::tls` (W2) | Namespace lists certificates under `tls`; parsing needs `asn1` + `pem` (see below). |
| `crypto/x509/pkix` | `planned` | `akku::tls` (W2) | Distinguished names, extensions. |

## database

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `database/sql` | `planned` | `akku::sql` (W2) | Unified API, pool, `migrate`. |
| `database/sql/driver` | `planned` | `akku::sql` (W2) | Driver trait; drivers are `sql::sqlite`, `sql::postgres`. |

## debug

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `debug/buildinfo` | `n/a` | - | Reads Go build metadata from Go binaries. |
| `debug/dwarf` | `missing → proposal` | `akku::obj::dwarf` | Debug info for symbolizers and crash tooling. |
| `debug/elf` | `missing → proposal` | `akku::obj::elf` | Object-file reading for build/release tooling. |
| `debug/gosym` | `n/a` | - | Go symbol tables only. |
| `debug/macho` | `missing → proposal` | `akku::obj::macho` | Same `obj` module, Mach-O backend. |
| `debug/pe` | `missing → proposal` | `akku::obj::pe` | Same `obj` module, PE/COFF backend. |
| `debug/plan9obj` | `n/a` | - | Plan 9 objects; no akku target. |

## embed

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `embed` | `Rust std` | `include_bytes!`, `include_str!` | Whole-directory embedding would need an `akku-drive` macro; not proposed yet. |

## encoding

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `encoding` | `Rust std` | `std::fmt::Display`, `std::str::FromStr` | Go's marshaler interfaces; akku formats use their own traits. |
| `encoding/ascii85` | `missing → proposal` | `akku::base85` | Ascii85/Z85; needed by `pdf` and Git binary patches. |
| `encoding/asn1` | `missing → proposal` | `akku::asn1` | DER/BER; prerequisite for `tls` certificates and `crypto::sign` keys. |
| `encoding/base32` | `missing → proposal` | `akku::base32` | RFC 4648 Base32/Base32hex; `auth::totp` secrets need it. |
| `encoding/base64` | `akku` | `akku::base64` | |
| `encoding/binary` | `akku` | `akku::bytes` | Endian `put_*`/`Cursor` reads; varints not yet. Plus `to_be_bytes` in Rust std. |
| `encoding/csv` | `akku` | `akku::csv` | |
| `encoding/gob` | `n/a` | - | Go-specific wire format; akku uses `msgpack`/`cbor`/`protobuf` (W3). |
| `encoding/hex` | `akku` | `akku::hex` | |
| `encoding/json` | `akku` | `akku::json` | |
| `encoding/json/jsontext` | `akku` | `akku::json` | Streaming decoder/encoder. |
| `encoding/json/v2` | `akku` | `akku::json` | Typed mapping. |
| `encoding/pem` | `missing → proposal` | `akku::pem` | RFC 7468 armor for keys/certificates used by `tls`. |
| `encoding/xml` | `akku` | `akku::xml` | |

## errors

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `errors` | `akku` | `akku::error` | Context chains, `bail!`/`ensure!`; base trait is `std::error::Error`. |

## expvar

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `expvar` | `planned` | `akku::trace` (W3) | Namespace lists spans, tracing, and metrics under `trace`. |

## flag

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `flag` | `akku` | `akku::cli` | |

## fmt

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `fmt` | `Rust std` | `std::fmt`, `format!`, `print!` | Go's `Sscanf` scanning has no equivalent; use `str::parse` or `akku::regex`. |

## go

All `go/*` packages are the Go toolchain's own parser and type checker.

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `go/ast` | `n/a` | - | Go toolchain internals. |
| `go/build` | `n/a` | - | Go toolchain internals. |
| `go/build/constraint` | `n/a` | - | Go build tags; Rust uses `cfg`. |
| `go/constant` | `n/a` | - | Go toolchain internals. |
| `go/doc` | `n/a` | - | Go toolchain internals. |
| `go/doc/comment` | `n/a` | - | Go toolchain internals. |
| `go/format` | `n/a` | - | Go toolchain internals (`rustfmt` covers Rust). |
| `go/importer` | `n/a` | - | Go toolchain internals. |
| `go/parser` | `n/a` | - | Go toolchain internals. |
| `go/printer` | `n/a` | - | Go toolchain internals. |
| `go/scanner` | `n/a` | - | Go toolchain internals. |
| `go/token` | `n/a` | - | Go toolchain internals. |
| `go/types` | `n/a` | - | Go toolchain internals. |
| `go/version` | `n/a` | - | Go toolchain internals. |

## hash

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `hash` | `Rust std` | `std::hash::Hasher` | Cryptographic digests are `akku::crypto::hash` (W2). |
| `hash/adler32` | `akku` | `akku::archive::checksum` | `Adler32`, `adler32()`. |
| `hash/crc32` | `akku` | `akku::archive::checksum` | `Crc32` (IEEE); Castagnoli (CRC-32C) not yet. |
| `hash/crc64` | `missing → proposal` | `akku::checksum` | Flat checksum module (CRC-32C, CRC-64, FNV); `archive::checksum` could move there since `xdr`/`nfs`/`kv` need checksums too. |
| `hash/fnv` | `missing → proposal` | `akku::checksum` | FNV-1/1a; fast non-crypto hash for IDs and sharding. |
| `hash/maphash` | `Rust std` | `std::hash::{DefaultHasher, RandomState}` | |

## html

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `html` | `akku` | `akku::html` | `escape` only; `unescape` (entity decoding) is not public yet. |
| `html/template` | `akku` | `akku::template` | Auto-escaping engine. |

## image

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `image` | `planned` | `akku::pic` (W3) | Image buffer model. |
| `image/color` | `planned` | `akku::color` (W3) | RGB, HSL, OKLCH. |
| `image/color/palette` | `planned` | `akku::color` (W3) | Standard palettes. |
| `image/draw` | `planned` | `akku::pic::canvas` (W3) | Compositing. |
| `image/gif` | `missing → proposal` | `akku::pic::gif` | Namespace lists `png`/`jpeg`/`webp`/`bmp` but not GIF; needs `archive::lzw`. |
| `image/jpeg` | `planned` | `akku::pic::jpeg` (W3) | |
| `image/png` | `planned` | `akku::pic::png` (W3) | Needs `archive::zlib`. |

## index

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `index/suffixarray` | `missing → proposal` | `akku::algo::suffix` | Suffix array next to the existing KMP/Aho-Corasick in `akku::algo::search`. |

## io

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `io` | `Rust std` | `std::io::{Read, Write, Seek, copy}` | |
| `io/fs` | `planned` | `akku::vfs` (0.9) | Read-only filesystem abstraction (`fs.FS`); OS filesystem is `akku::fs`. |
| `io/ioutil` | `n/a` | - | Deprecated in Go; see `std::fs::read`/`write` and `akku::fs`. |

## iter

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `iter` | `Rust std` | `std::iter` | |

## log

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `log` | `akku` | `akku::log` | |
| `log/slog` | `akku` | `akku::log` | Structured fields, human + JSON formatters. |
| `log/syslog` | `missing → proposal` | `akku::log::syslog` | RFC 5424 sink (Unix socket / UDP) for daemons. |

## maps

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `maps` | `Rust std` | `std::collections::{HashMap, BTreeMap}` | |

## math

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `math` | `Rust std` | `f64` methods, `std::f64::consts` | Extra functions in `akku::math::special`. |
| `math/big` | `missing → proposal` | `akku::math::big` | Arbitrary-precision Int/Rat; `math::decimal` is `i128`-bounded. |
| `math/bits` | `Rust std` | `u64::{leading_zeros, count_ones, rotate_left, overflowing_add}` | |
| `math/cmplx` | `missing → proposal` | `akku::math::complex` | No complex type in Rust std; needed by FFT in `audio::dsp`. |
| `math/rand` | `planned` | `akku::crypto::rand` (W2) | Secure randomness; seeded PRNGs see `math/rand/v2`. |
| `math/rand/v2` | `missing → proposal` | `akku::math::rng` | Seedable deterministic PRNG (PCG/ChaCha8), shuffle, distributions; for tests and simulations. |

## mime

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `mime` | `akku` | `akku::mime` | |
| `mime/multipart` | `missing → proposal` | `akku::mime::multipart` | `multipart/form-data` uploads for `http`, and `mail` bodies. |
| `mime/quotedprintable` | `missing → proposal` | `akku::mime::quoted_printable` | Needed by `mail`. |

## net

`net` is flat (superset of `std::net`) with flat `dns` and `quic` beside it, not a
family. `Namespace.md` on `main` still shows the old `net` family; it is being
updated in a parallel PR.

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `net` | `planned` | `akku::net` (W2, 0.9) | TCP/UDP/UDS sockets; name resolution in `akku::dns`. |
| `net/http` | `akku` | `akku::http` | Server is an axum adapter; client is native. |
| `net/http/cgi` | `n/a` | - | Legacy gateway protocol. |
| `net/http/cookiejar` | `planned` | `akku::http` (W1) | Server-boundary item "cookies"; a client cookie jar belongs in `http::client`. |
| `net/http/fcgi` | `n/a` | - | Legacy gateway protocol. |
| `net/http/httptest` | `akku` | `akku::test` | `TestServer`. |
| `net/http/httptrace` | `planned` | `akku::trace` (W3) | Client lifecycle hooks as spans. |
| `net/http/httputil` | `missing → proposal` | `akku::http::proxy` | Reverse proxy and request/response dumping. |
| `net/http/pprof` | `n/a` | - | Go runtime profiler endpoint. |
| `net/mail` | `planned` | `akku::mail` (W3) | RFC 5322 address/header parsing. |
| `net/netip` | `planned` | `akku::net` (W2, 0.9) | IP prefixes/CIDR on top of `std::net::IpAddr`. |
| `net/rpc` | `planned` | `akku::rpc` (W3) | |
| `net/rpc/jsonrpc` | `planned` | `akku::rpc` (W3) | JSON-RPC codec; ONC-RPC stays internal to `nfs`. |
| `net/smtp` | `planned` | `akku::mail` (W3) | Namespace: "SMTP-Client". |
| `net/textproto` | `n/a` | - | Shared internals of `http`/`mail`; not a public akku API. |
| `net/url` | `akku` | `akku::url` | |

## os

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `os` | `akku` | `akku::fs`, `akku::env` | Supersets of `std::fs`/`std::env`; platform specifics go to `akku::os::<platform>` (0.7). |
| `os/exec` | `akku` | `akku::process` | |
| `os/signal` | `planned` | `akku::process` (W1) | Namespace lists signals under `process`; not implemented yet. |
| `os/user` | `missing → proposal` | `akku::env::user` | Current user name, uid/gid, home lookup; Unix lookup needs a `sys` submodule (0.7). |

## path

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `path` | `missing → proposal` | `akku::vfs::path` | OS-independent slash paths with lexical `clean`/`join` for `vfs`/`nfs`. |
| `path/filepath` | `Rust std` | `std::path` | `Walk`/`Glob` are `akku::fs` (`walk`, `glob`). |

## plugin

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `plugin` | `planned` | `akku::wasm::host` (W3) | akku plugins are WASM, not native shared objects. |

## reflect

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `reflect` | `n/a` | - | Runtime reflection; Rust uses traits, `std::any`, and macros. |

## regexp

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `regexp` | `akku` | `akku::regex` | Linear-time like Go's RE2 engine. |
| `regexp/syntax` | `n/a` | - | Parser internals; the akku AST stays private. |

## runtime

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `runtime` | `n/a` | - | Go runtime and GC. |
| `runtime/cgo` | `n/a` | - | Rust FFI uses `extern "C"` in `sys` submodules. |
| `runtime/coverage` | `n/a` | - | Toolchain coverage (`cargo llvm-cov`). |
| `runtime/debug` | `Rust std` | `std::backtrace::Backtrace` | GC tuning parts are n/a. |
| `runtime/metrics` | `n/a` | - | Go runtime metrics; app metrics are `akku::trace` (W3). |
| `runtime/pprof` | `n/a` | - | Go profiler; use OS profilers. |
| `runtime/race` | `n/a` | - | Go race detector; Rust uses ownership + sanitizers. |
| `runtime/trace` | `n/a` | - | Go execution tracer; app spans are `akku::trace` (W3). |

## slices

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `slices` | `Rust std` | slice methods (`sort`, `binary_search`, `contains`) | |

## sort

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `sort` | `Rust std` | `slice::sort`, `slice::sort_unstable` | `lower_bound`/`upper_bound` in `akku::algo::search`. |

## strconv

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `strconv` | `Rust std` | `str::parse`, `ToString`, `{:?}` | Go-syntax `Quote`/`Unquote` are n/a. |

## strings

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `strings` | `Rust std` | `str`, `String` | Case conversion, wrapping, distances in `akku::text`. |

## structs

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `structs` | `n/a` | - | Layout marker types; Rust uses `#[repr(C)]`. |

## sync

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `sync` | `Rust std` | `std::sync::{Mutex, RwLock, Once, OnceLock, Condvar}` | Async channels, semaphore, broadcast, watch are `akku::sync` (W2). |
| `sync/atomic` | `Rust std` | `std::sync::atomic` | |

## syscall

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `syscall` | `planned` | `akku::os::<platform>` (0.7) | Raw OS calls only inside `sys` submodules with `// SAFETY:`. |

## testing

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `testing` | `Rust std` | `#[test]`, `cargo test` | Snapshots, temp dirs, assertions in `akku::test`; benchmarks are `akku::bench` (W3). |
| `testing/cryptotest` | `n/a` | - | Go crypto test harness. |
| `testing/fstest` | `missing → proposal` | `akku::vfs::memory` | In-memory `vfs` plus a conformance check, mirroring `MapFS`/`TestFS`. |
| `testing/iotest` | `missing → proposal` | `akku::test::io` | Short/erroring/one-byte readers and writers for adversarial tests. |
| `testing/quick` | `missing → proposal` | `akku::test::prop` | Property-based testing with shrinking; supports hard rule 13. |
| `testing/slogtest` | `akku` | `akku::log` | In-memory log capture for tests. |
| `testing/synctest` | `akku` | `akku::time::clock` | Injectable clocks; virtual time for async tasks will follow `akku::task` (W2). |

## text

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `text/scanner` | `n/a` | - | Go-syntax tokenizer; akku parsers keep private lexers. |
| `text/tabwriter` | `akku` | `akku::term` | `Table` with alignment; elastic tabstops not supported. |
| `text/template` | `akku` | `akku::template` | |
| `text/template/parse` | `n/a` | - | Template AST internals. |

## time

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `time` | `akku` | `akku::time` | Plus `std::time::{Duration, Instant, SystemTime}`; only fixed UTC offsets today. |
| `time/tzdata` | `missing → proposal` | `akku::time::tzdb` | IANA time zone database (embedded + system `zoneinfo`) with DST rules. |

## unicode

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `unicode` | `akku` | `akku::text` | Graphemes, normalization; character classes via `char` methods in Rust std. |
| `unicode/utf16` | `Rust std` | `String::from_utf16`, `str::encode_utf16` | |
| `unicode/utf8` | `Rust std` | `str`, `char::encode_utf8`, `std::str::from_utf8` | |

## unique

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `unique` | `missing → proposal` | `akku::intern` | Value/string interning with cheap comparable handles for parsers and compilers. |

## unsafe

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `unsafe` | `Rust std` | `unsafe`, `std::ptr`, `std::mem` | akku itself is `#![forbid(unsafe_code)]`; see the unsafe policy. |

## uuid

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `uuid` | `akku` | `akku::id` | `Uuid` v4/v7, plus ULID and NanoID. |

## weak

| Go package | Status | Path | Note |
| --- | --- | --- | --- |
| `weak` | `Rust std` | `std::sync::Weak`, `std::rc::Weak` | |

---

## Proposals

Candidates for the NewLib intake. Nothing here is decided; `ROADMAP.md` and
`Namespace.md` are unchanged. The suggested wave is where each would unblock
planned work.

### Wave 2 — prerequisites for `tls`, `crypto`, `auth`, `http`, `pic`

| Proposal | Go source | Reason |
| --- | --- | --- |
| `akku::asn1` | `encoding/asn1` | DER/BER for certificates and keys (`tls`, `crypto::sign`). |
| `akku::pem` | `encoding/pem` | Key/certificate armor for `tls`. |
| `akku::base32` | `encoding/base32` | `auth::totp` secrets. |
| `akku::archive::zlib` | `compress/zlib` | PNG and HTTP `deflate`; builds on existing `deflate` + `Adler32`. |
| `akku::mime::multipart` | `mime/multipart` | File uploads in `http`. |
| `akku::http::proxy` | `net/http/httputil` | Reverse proxy and request dumping. |
| `akku::math::rng` | `math/rand/v2` | Seedable PRNG for tests and simulations, separate from `crypto::rand`. |
| `akku::math::big` | `math/big` | Arbitrary-precision integers/rationals. |
| `akku::time::tzdb` | `time/tzdata` | Named time zones with DST. |
| `akku::checksum` | `hash/crc64`, `hash/fnv` | Flat checksums (CRC-32C, CRC-64, FNV) for `xdr`/`nfs`/`kv`; could absorb `archive::checksum`. |
| `akku::test::io` | `testing/iotest` | Faulty readers/writers for adversarial tests. |
| `akku::test::prop` | `testing/quick` | Property-based testing. |

### Wave 3 — media, mail, tooling

| Proposal | Go source | Reason |
| --- | --- | --- |
| `akku::archive::lzw` | `compress/lzw` | GIF/TIFF decoding. |
| `akku::archive::bzip2` | `compress/bzip2` | Legacy `.tar.bz2` archives. |
| `akku::pic::gif` | `image/gif` | Missing from the planned `pic` list. |
| `akku::math::complex` | `math/cmplx` | FFT for `audio::dsp`. |
| `akku::mime::quoted_printable` | `mime/quotedprintable` | `mail` bodies. |
| `akku::base85` | `encoding/ascii85` | `pdf` streams. |
| `akku::log::syslog` | `log/syslog` | Daemon logging sink. |
| `akku::algo::suffix` | `index/suffixarray` | Fast substring search over large texts. |
| `akku::intern` | `unique` | Interning for parsers/compilers. |
| `akku::obj` (`elf`, `macho`, `pe`, `dwarf`) | `debug/elf`, `debug/macho`, `debug/pe`, `debug/dwarf` | Object-file and debug-info reading for tooling. |

### With 0.7 (OS layer) / 0.9 (megaghost)

| Proposal | Go source | Reason |
| --- | --- | --- |
| `akku::env::user` | `os/user` | User lookup needs a platform `sys` submodule (0.7). |
| `akku::vfs::path` | `path` | Slash paths for `vfs`/`nfs` (0.9). |
| `akku::vfs::memory` | `testing/fstest` | In-memory `vfs` for tests (0.9). |
