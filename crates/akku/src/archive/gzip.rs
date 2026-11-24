//! gzip container format (RFC 1952).
//!
//! Pure safe Rust on `std` only, built on
//! [`archive::deflate`](super::deflate). Every member's CRC-32 and ISIZE
//! trailer are verified, concatenated multi-member streams are decoded
//! transparently, and every decompression path is bounded by an output cap.
//!
//! # Examples
//!
//! ```
//! use akku::archive::gzip;
//!
//! let gz = gzip::compress(b"hello gzip, hello gzip!");
//! assert_eq!(gzip::decompress(&gz).unwrap(), b"hello gzip, hello gzip!");
//! ```

use std::fmt;
use std::io::{self, Read, Write};

use crate::archive::checksum::Crc32;
use crate::archive::deflate::{
    self, CompressionLevel, DeflateEncoder, DeflateError, InflateStream,
};

/// Magic bytes of every gzip member.
const MAGIC: [u8; 2] = [0x1F, 0x8B];

/// The only compression method defined by RFC 1952.
const METHOD_DEFLATE: u8 = 8;

/// FTEXT flag bit.
const FTEXT: u8 = 0x01;

/// FHCRC flag bit (header CRC16 present).
const FHCRC: u8 = 0x02;

/// FEXTRA flag bit.
const FEXTRA: u8 = 0x04;

/// FNAME flag bit.
const FNAME: u8 = 0x08;

/// FCOMMENT flag bit.
const FCOMMENT: u8 = 0x10;

/// Default output cap for decompression paths without an explicit limit:
/// 1 GiB across all members.
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 1 << 30;

/// Longest FNAME / FCOMMENT header string accepted while reading (64 KiB).
pub const MAX_HEADER_FIELD_BYTES: usize = 64 * 1024;

/// Upper bound on a complete member header: fixed part, FEXTRA, FNAME,
/// FCOMMENT, and FHCRC.
const MAX_HEADER_BYTES: usize = 10 + 2 + 65_535 + 2 * (MAX_HEADER_FIELD_BYTES + 1) + 2;

/// Buffered-but-undecodable DEFLATE input beyond which the stream is
/// considered corrupt (no header or symbol needs this much lookahead).
const MAX_STALLED_INPUT: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by the gzip codec.
#[derive(Debug)]
#[non_exhaustive]
pub enum GzipError {
    /// The stream did not start with the gzip magic bytes `1f 8b`.
    BadMagic {
        /// The first two bytes actually found.
        actual: [u8; 2],
    },
    /// The member uses a compression method other than DEFLATE (8).
    UnsupportedMethod(u8),
    /// The FLG byte sets one of the reserved bits (5-7).
    ReservedFlagSet(u8),
    /// The stored header CRC16 (FHCRC) did not match the computed one.
    HeaderCrcMismatch {
        /// The stored CRC16.
        expected: u16,
        /// The computed CRC16.
        computed: u16,
    },
    /// The input ended inside a header, member, or trailer.
    Truncated,
    /// A header string field (FNAME/FCOMMENT) exceeds
    /// [`MAX_HEADER_FIELD_BYTES`], or the header as a whole is implausibly
    /// large.
    HeaderTooLarge,
    /// The DEFLATE payload was malformed or exceeded its output cap.
    Deflate(DeflateError),
    /// The member trailer's CRC-32 did not match the decompressed data.
    CrcMismatch {
        /// The stored CRC-32.
        expected: u32,
        /// The CRC-32 computed over the decompressed bytes.
        computed: u32,
    },
    /// The member trailer's ISIZE did not match (length mod 2^32).
    LengthMismatch {
        /// The stored ISIZE.
        expected: u32,
        /// The actual decompressed length, mod 2^32.
        computed: u64,
    },
    /// Non-gzip bytes follow the last member.
    TrailingGarbage,
    /// An I/O error on the underlying reader or writer.
    Io(io::Error),
}

impl fmt::Display for GzipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic { actual } => {
                write!(f, "bad gzip magic bytes {actual:02x?} (expected 1f 8b)")
            }
            Self::UnsupportedMethod(m) => write!(f, "unsupported gzip method {m} (only 8)"),
            Self::ReservedFlagSet(flg) => write!(f, "reserved FLG bits set: 0x{flg:02x}"),
            Self::HeaderCrcMismatch { expected, computed } => write!(
                f,
                "gzip header CRC16 mismatch: stored {expected:#06x}, computed {computed:#06x}"
            ),
            Self::Truncated => f.write_str("truncated gzip input"),
            Self::HeaderTooLarge => write!(
                f,
                "gzip header field exceeds the {MAX_HEADER_FIELD_BYTES}-byte limit"
            ),
            Self::Deflate(e) => write!(f, "deflate error: {e}"),
            Self::CrcMismatch { expected, computed } => write!(
                f,
                "gzip CRC-32 mismatch: stored {expected:#010x}, computed {computed:#010x}"
            ),
            Self::LengthMismatch { expected, computed } => write!(
                f,
                "gzip ISIZE mismatch: stored {expected}, actual length mod 2^32 is {computed}"
            ),
            Self::TrailingGarbage => f.write_str("trailing garbage after the final gzip member"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for GzipError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Deflate(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for GzipError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<DeflateError> for GzipError {
    fn from(e: DeflateError) -> Self {
        Self::Deflate(e)
    }
}

// ---------------------------------------------------------------------------
// Options and header
// ---------------------------------------------------------------------------

/// Builder of gzip member metadata for [`compress_with`] and
/// [`GzipEncoder::with_options`].
///
/// ```
/// use akku::archive::gzip::{self, GzipOptions};
/// use akku::archive::CompressionLevel;
///
/// let opts = GzipOptions::new()
///     .level(CompressionLevel::Best)
///     .mtime(1_700_000_000)
///     .filename("data.txt");
/// let gz = gzip::compress_with(b"payload", &opts);
/// assert_eq!(gzip::decompress(&gz).unwrap(), b"payload");
/// ```
#[derive(Debug, Clone)]
pub struct GzipOptions {
    level: CompressionLevel,
    mtime: u32,
    filename: Option<String>,
    comment: Option<String>,
    extra: Option<Vec<u8>>,
    os: u8,
}

impl GzipOptions {
    /// Default options: [`CompressionLevel::Normal`], no mtime, Unix OS
    /// byte, no name/comment/extra fields.
    pub fn new() -> Self {
        Self {
            level: CompressionLevel::Normal,
            mtime: 0,
            filename: None,
            comment: None,
            extra: None,
            os: 3,
        }
    }

    /// Set the compression level.
    pub fn level(mut self, level: CompressionLevel) -> Self {
        self.level = level;
        self
    }

    /// Set the modification time (Unix seconds; 0 means "none").
    pub fn mtime(mut self, mtime: u32) -> Self {
        self.mtime = mtime;
        self
    }

    /// Set the original filename (FNAME). Non-Latin-1 characters and NUL
    /// bytes are replaced at encode time.
    pub fn filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    /// Set a comment (FCOMMENT). Encoded like the filename.
    pub fn comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// Set the extra field (FEXTRA), truncated to 65535 bytes at encode time.
    pub fn extra(mut self, extra: Vec<u8>) -> Self {
        self.extra = Some(extra);
        self
    }

    /// Set the operating-system byte (RFC 1952 §2.3.1.2; 3 = Unix).
    pub fn os(mut self, os: u8) -> Self {
        self.os = os;
        self
    }
}

impl Default for GzipOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Encode a string field as Latin-1, replacing non-Latin-1 characters and
/// NUL bytes with `'?'`.
fn latin1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| {
            if (c as u32) < 256 && c != '\0' {
                c as u32 as u8
            } else {
                b'?'
            }
        })
        .collect()
}

/// Serialize the member header for `opts`.
fn encode_header(opts: &GzipOptions) -> Vec<u8> {
    let mut flags = 0u8;
    if opts.filename.is_some() {
        flags |= FNAME;
    }
    if opts.comment.is_some() {
        flags |= FCOMMENT;
    }
    if opts.extra.is_some() {
        flags |= FEXTRA;
    }
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&MAGIC);
    out.push(METHOD_DEFLATE);
    out.push(flags);
    out.extend_from_slice(&opts.mtime.to_le_bytes());
    out.push(0); // XFL
    out.push(opts.os);
    if let Some(extra) = &opts.extra {
        let len = extra.len().min(65_535);
        out.extend_from_slice(&(len as u16).to_le_bytes());
        out.extend_from_slice(&extra[..len]);
    }
    if let Some(name) = &opts.filename {
        out.extend_from_slice(&latin1(name));
        out.push(0);
    }
    if let Some(comment) = &opts.comment {
        out.extend_from_slice(&latin1(comment));
        out.push(0);
    }
    out
}

/// The parsed metadata of one gzip member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GzipHeader {
    /// Whether the FTEXT flag is set (data is probably ASCII).
    pub is_text: bool,
    /// The member's modification time (Unix seconds; 0 = none).
    pub mtime: u32,
    /// The operating-system byte.
    pub os: u8,
    /// The extra field (FEXTRA), when present.
    pub extra: Option<Vec<u8>>,
    /// The original filename (FNAME), when present.
    pub filename: Option<String>,
    /// The comment (FCOMMENT), when present.
    pub comment: Option<String>,
}

impl GzipHeader {
    /// Read and validate exactly one member header from `r`.
    ///
    /// Consumes only the header bytes; the compressed payload is left in the
    /// reader. The header CRC16 is verified when FHCRC is set.
    pub fn read_from<R: Read>(r: &mut R) -> Result<Self, GzipError> {
        let mut crc = Crc32::new();
        let mut base = [0u8; 10];
        read_exact_mapped(r, &mut base)?;
        crc.update(&base);
        if base[0..2] != MAGIC {
            return Err(GzipError::BadMagic {
                actual: [base[0], base[1]],
            });
        }
        if base[2] != METHOD_DEFLATE {
            return Err(GzipError::UnsupportedMethod(base[2]));
        }
        let flg = base[3];
        if flg & 0xE0 != 0 {
            return Err(GzipError::ReservedFlagSet(flg));
        }
        let mtime = u32::from_le_bytes([base[4], base[5], base[6], base[7]]);
        let os = base[9];

        let mut extra = None;
        if flg & FEXTRA != 0 {
            let mut lenb = [0u8; 2];
            read_exact_mapped(r, &mut lenb)?;
            crc.update(&lenb);
            let mut buf = vec![0u8; u16::from_le_bytes(lenb) as usize];
            read_exact_mapped(r, &mut buf)?;
            crc.update(&buf);
            extra = Some(buf);
        }
        let filename = if flg & FNAME != 0 {
            Some(read_cstring(r, &mut crc)?)
        } else {
            None
        };
        let comment = if flg & FCOMMENT != 0 {
            Some(read_cstring(r, &mut crc)?)
        } else {
            None
        };
        if flg & FHCRC != 0 {
            let mut b = [0u8; 2];
            read_exact_mapped(r, &mut b)?;
            let computed = (crc.value() & 0xFFFF) as u16;
            let expected = u16::from_le_bytes(b);
            if computed != expected {
                return Err(GzipError::HeaderCrcMismatch { expected, computed });
            }
        }
        Ok(Self {
            is_text: flg & FTEXT != 0,
            mtime,
            os,
            extra,
            filename,
            comment,
        })
    }
}

/// `read_exact` that maps premature EOF to [`GzipError::Truncated`].
fn read_exact_mapped<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<(), GzipError> {
    r.read_exact(buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            GzipError::Truncated
        } else {
            GzipError::Io(e)
        }
    })
}

/// Read a NUL-terminated Latin-1 header string of at most
/// [`MAX_HEADER_FIELD_BYTES`] bytes (excluding the terminator).
fn read_cstring<R: Read>(r: &mut R, crc: &mut Crc32) -> Result<String, GzipError> {
    let mut buf = Vec::new();
    loop {
        let mut b = [0u8; 1];
        read_exact_mapped(r, &mut b)?;
        crc.update(&b);
        if b[0] == 0 {
            break;
        }
        if buf.len() >= MAX_HEADER_FIELD_BYTES {
            // Implausibly long header field; refuse instead of buffering.
            return Err(GzipError::HeaderTooLarge);
        }
        buf.push(b[0]);
    }
    Ok(buf.iter().map(|&b| b as char).collect())
}

// ---------------------------------------------------------------------------
// One-shot API
// ---------------------------------------------------------------------------

/// Compress `data` into one gzip member at the default level.
pub fn compress(data: &[u8]) -> Vec<u8> {
    compress_with(data, &GzipOptions::new())
}

/// Compress `data` into one gzip member with explicit header options.
pub fn compress_with(data: &[u8], opts: &GzipOptions) -> Vec<u8> {
    let mut out = encode_header(opts);
    out.extend_from_slice(&deflate::deflate(data, opts.level));
    let mut crc = Crc32::new();
    crc.update(data);
    out.extend_from_slice(&crc.value().to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// Decompress a gzip stream (possibly multi-member) with the default cap of
/// [`DEFAULT_MAX_OUTPUT_BYTES`].
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, GzipError> {
    decompress_limited(data, DEFAULT_MAX_OUTPUT_BYTES)
}

/// Decompress a gzip stream with an explicit total output cap across all
/// members. CRC-32 and ISIZE of every member are verified; trailing
/// non-gzip bytes are an error.
pub fn decompress_limited(data: &[u8], max_output_bytes: u64) -> Result<Vec<u8>, GzipError> {
    let mut decoder = GzipDecoder::limited(data, max_output_bytes);
    let mut out = Vec::new();
    let mut scratch = vec![0u8; 8192];
    loop {
        match decoder.read_impl(&mut scratch) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&scratch[..n]),
            Err(e) => return Err(e),
        }
    }
    decoder.finish()?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Streaming encoder
// ---------------------------------------------------------------------------

/// Streaming gzip encoder over any [`Write`].
///
/// The header is written on construction. Call
/// [`into_inner`](GzipEncoder::into_inner) after the last write to emit the
/// final block and the CRC-32/ISIZE trailer and get the writer back; `finish`
/// does the same without consuming the encoder. Dropping without finishing
/// performs a best-effort flush, discarding errors.
///
/// ```
/// use akku::archive::gzip::{self, GzipEncoder};
/// use std::io::Write;
///
/// let mut enc = GzipEncoder::new(Vec::new()).unwrap();
/// enc.write_all(b"part one ").unwrap();
/// enc.write_all(b"part two").unwrap();
/// let gz = enc.into_inner().unwrap();
/// assert_eq!(gzip::decompress(&gz).unwrap(), b"part one part two");
/// ```
#[derive(Debug)]
pub struct GzipEncoder<W: Write> {
    inner: Option<DeflateEncoder<W>>,
    crc: Crc32,
    len: u64,
    finished: bool,
}

impl<W: Write> GzipEncoder<W> {
    /// A new encoder with default options; the header write may fail.
    pub fn new(out: W) -> io::Result<Self> {
        Self::with_options(out, &GzipOptions::new())
    }

    /// A new encoder with explicit header options.
    pub fn with_options(mut out: W, opts: &GzipOptions) -> io::Result<Self> {
        out.write_all(&encode_header(opts))?;
        Ok(Self {
            inner: Some(DeflateEncoder::new(out, opts.level)),
            crc: Crc32::new(),
            len: 0,
            finished: false,
        })
    }

    /// Write the trailer, flush, and return the inner writer. Errors on a
    /// double finish.
    pub fn into_inner(mut self) -> io::Result<W> {
        if self.finished {
            return Err(io::Error::other("gzip encoder already finished"));
        }
        self.finish_inner()
    }

    /// Write the trailer and flush without consuming the encoder. Idempotent.
    /// After `finish`, further writes fail.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finish_inner().map(|_| ())
    }

    fn finish_inner(&mut self) -> io::Result<W> {
        self.finished = true;
        let inner = self
            .inner
            .take()
            .expect("gzip encoder holds its writer until finished");
        let mut w = inner.finish()?;
        w.write_all(&self.crc.value().to_le_bytes())?;
        w.write_all(&(self.len as u32).to_le_bytes())?;
        w.flush()?;
        Ok(w)
    }
}

impl<W: Write> Write for GzipEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| io::Error::other("gzip encoder finished"))?;
        let n = inner.write(buf)?;
        self.crc.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| io::Error::other("gzip encoder finished"))?;
        inner.flush()
    }
}

impl<W: Write> Drop for GzipEncoder<W> {
    fn drop(&mut self) {
        if !self.finished && self.inner.is_some() {
            let _ = self.finish_inner();
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming decoder
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum DecState {
    /// Reading the next member header.
    Header,
    /// Inside a member's DEFLATE payload.
    Body,
    /// Reading/verifying the current member's 8-byte trailer.
    Trailer,
    /// All members consumed (check `garbage` for trailing bytes).
    Done,
}

/// Streaming gzip decoder over any [`Read`].
///
/// Concatenated members are decoded transparently in reading order. The
/// header of the member currently being decoded is available via
/// [`header`](GzipDecoder::header). Call [`finish`](GzipDecoder::finish)
/// after EOF to detect truncation or trailing garbage.
///
/// ```
/// use akku::archive::gzip::{self, GzipDecoder};
/// use std::io::Read;
///
/// let stream = [gzip::compress(b"alpha,"), gzip::compress(b"beta")].concat();
/// let mut dec = GzipDecoder::new(&stream[..]);
/// let mut out = String::new();
/// dec.read_to_string(&mut out).unwrap();
/// dec.finish().unwrap();
/// assert_eq!(out, "alpha,beta");
/// ```
#[derive(Debug)]
pub struct GzipDecoder<R: Read> {
    input: R,
    ibuf: Vec<u8>,
    ipos: usize,
    eof: bool,
    state: DecState,
    cap: u64,
    budget: u64,
    stream: InflateStream,
    crc: Crc32,
    member_out: u64,
    header: Option<GzipHeader>,
    garbage: bool,
}

impl<R: Read> GzipDecoder<R> {
    /// A decoder with the default output cap.
    pub fn new(input: R) -> Self {
        Self::limited(input, DEFAULT_MAX_OUTPUT_BYTES)
    }

    /// A decoder with an explicit total output cap across all members.
    pub fn limited(input: R, max_output_bytes: u64) -> Self {
        Self {
            input,
            ibuf: Vec::new(),
            ipos: 0,
            eof: false,
            state: DecState::Header,
            cap: max_output_bytes,
            budget: max_output_bytes,
            stream: InflateStream::new(max_output_bytes),
            crc: Crc32::new(),
            member_out: 0,
            header: None,
            garbage: false,
        }
    }

    /// The header of the member currently being decoded.
    pub fn header(&self) -> Option<&GzipHeader> {
        self.header.as_ref()
    }

    /// The total output cap this decoder was created with.
    pub fn max_output_bytes(&self) -> u64 {
        self.cap
    }

    /// Verify: all input must be consumed cleanly, with no trailing garbage.
    pub fn finish(self) -> Result<(), GzipError> {
        match self.state {
            DecState::Done => {
                if self.garbage {
                    Err(GzipError::TrailingGarbage)
                } else {
                    Ok(())
                }
            }
            _ => Err(GzipError::Truncated),
        }
    }

    /// Compact the staging buffer and refill it from the reader.
    /// Returns `false` at end of input.
    fn refill(&mut self) -> Result<bool, GzipError> {
        if self.ipos > 0 {
            self.ibuf.drain(..self.ipos);
            self.ipos = 0;
        }
        if self.eof {
            return Ok(false);
        }
        let mut chunk = [0u8; 8 * 1024];
        let n = loop {
            match self.input.read(&mut chunk) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(GzipError::Io(e)),
            }
        };
        if n == 0 {
            self.eof = true;
            Ok(false)
        } else {
            self.ibuf.extend_from_slice(&chunk[..n]);
            Ok(true)
        }
    }

    /// Ensure at least `n` bytes are buffered; `false` when input is over.
    fn ensure(&mut self, n: usize) -> Result<bool, GzipError> {
        while self.ibuf.len() - self.ipos < n {
            if !self.refill()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn io_error(e: GzipError) -> io::Error {
        match e {
            GzipError::Io(io_err) => io_err,
            GzipError::Truncated | GzipError::Deflate(DeflateError::TruncatedStream) => {
                io::Error::new(io::ErrorKind::UnexpectedEof, e)
            }
            other => io::Error::new(io::ErrorKind::InvalidData, other),
        }
    }

    /// Core read loop returning [`GzipError`] directly.
    fn read_impl(&mut self, out: &mut [u8]) -> Result<usize, GzipError> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            match self.state {
                DecState::Header => {
                    let attempt = {
                        let mut slice: &[u8] = &self.ibuf[self.ipos..];
                        GzipHeader::read_from(&mut slice)
                            .map(|h| (h, self.ibuf.len() - self.ipos - slice.len()))
                    };
                    match attempt {
                        Ok((h, used)) => {
                            self.ipos += used;
                            self.header = Some(h);
                            self.crc.reset();
                            self.member_out = 0;
                            self.stream = InflateStream::new(self.budget);
                            self.state = DecState::Body;
                        }
                        Err(GzipError::Truncated) => {
                            if self.ibuf.len() - self.ipos > MAX_HEADER_BYTES {
                                return Err(GzipError::HeaderTooLarge);
                            }
                            if !self.refill()? {
                                self.state = DecState::Done;
                                return Err(GzipError::Truncated);
                            }
                        }
                        Err(other) => return Err(other),
                    }
                }
                DecState::Body => {
                    let before = self.stream.total_out();
                    let consumed = self.stream.push(&self.ibuf[self.ipos..])?;
                    self.ipos += consumed;
                    let mut produced = 0usize;
                    loop {
                        let n = self.stream.pull(&mut out[produced..]);
                        if n == 0 {
                            break;
                        }
                        self.crc.update(&out[produced..produced + n]);
                        self.member_out += n as u64;
                        produced += n;
                    }
                    if produced > 0 {
                        return Ok(produced);
                    }
                    if self.stream.is_finished() {
                        self.state = DecState::Trailer;
                        continue;
                    }
                    let progressed =
                        consumed > 0 || self.stream.total_out() > before || self.stream.is_done();
                    if !progressed {
                        // Either no buffered input, or only an incomplete
                        // symbol/header at a chunk boundary: keep the tail
                        // and append more input after it.
                        if self.ibuf.len() - self.ipos >= MAX_STALLED_INPUT {
                            return Err(GzipError::Deflate(DeflateError::InvalidHuffmanCode));
                        }
                        if !self.refill()? {
                            return Err(GzipError::Truncated);
                        }
                    }
                }
                DecState::Trailer => {
                    if !self.ensure(8)? {
                        return Err(GzipError::Truncated);
                    }
                    let stored_crc = u32::from_le_bytes(
                        self.ibuf[self.ipos..self.ipos + 4]
                            .try_into()
                            .expect("ensured 8 trailer bytes"),
                    );
                    let stored_len = u32::from_le_bytes(
                        self.ibuf[self.ipos + 4..self.ipos + 8]
                            .try_into()
                            .expect("ensured 8 trailer bytes"),
                    );
                    self.ipos += 8;
                    if self.crc.value() != stored_crc {
                        return Err(GzipError::CrcMismatch {
                            expected: stored_crc,
                            computed: self.crc.value(),
                        });
                    }
                    if self.member_out & 0xFFFF_FFFF != stored_len as u64 {
                        return Err(GzipError::LengthMismatch {
                            expected: stored_len,
                            computed: self.member_out & 0xFFFF_FFFF,
                        });
                    }
                    self.budget = self.budget.saturating_sub(self.member_out);
                    // Peek: another member, clean EOF, or garbage? Refill
                    // first — the next header may still be in the reader.
                    if !self.ensure(2)? {
                        // Input exhausted with fewer than 2 bytes buffered.
                        if self.ibuf.len() - self.ipos == 1 {
                            self.garbage = true;
                        }
                        self.state = DecState::Done;
                        continue;
                    }
                    if self.ibuf[self.ipos] == MAGIC[0] && self.ibuf[self.ipos + 1] == MAGIC[1] {
                        self.state = DecState::Header;
                    } else {
                        self.garbage = true;
                        self.state = DecState::Done;
                    }
                }
                DecState::Done => return Ok(0),
            }
        }
    }
}

impl<R: Read> Read for GzipDecoder<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.read_impl(out).map_err(Self::io_error)
    }
}
