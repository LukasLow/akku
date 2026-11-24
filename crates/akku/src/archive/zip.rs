//! PKZIP archives: reading and writing Store (method 0) and Deflate
//! (method 8) entries per the APPNOTE.
//!
//! Pure safe Rust on `std` only. The central directory and EOCD record are
//! parsed with bounds checks, every entry's CRC-32 and size are verified,
//! and [`ZipReader::safe_extract_to`] enforces the family's zip-slip rules
//! plus per-entry and total output caps (zip-bomb guard).
//!
//! # Examples
//!
//! ```
//! use akku::archive::zip::{ZipReader, ZipWriter};
//! use akku::archive::CompressionLevel;
//!
//! let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
//! writer.add_file("notes/a.txt", b"stored", CompressionLevel::Stored).unwrap();
//! writer.add_file("notes/b.txt", b"deflated, deflated!", CompressionLevel::Normal).unwrap();
//! let bytes = writer.finish().unwrap().into_inner();
//!
//! let mut reader = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
//! let entry = reader.entry_by_name("notes/b.txt").cloned().unwrap();
//! assert_eq!(reader.read_entry(&entry, 1024).unwrap(), b"deflated, deflated!");
//! ```

use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::archive::checksum::{crc32, Crc32};
use crate::archive::deflate::{self, DeflateError};
use crate::archive::{safe_target, ExtractOptions};

pub use crate::archive::deflate::CompressionLevel;

/// Local file header signature (`PK\x03\x04`).
const SIG_LOCAL: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];
/// Central directory header signature (`PK\x01\x02`).
const SIG_CENTRAL: [u8; 4] = [0x50, 0x4B, 0x01, 0x02];
/// End-of-central-directory signature (`PK\x05\x06`).
const SIG_EOCD: [u8; 4] = [0x50, 0x4B, 0x05, 0x06];

/// General-purpose flag bit 0: encrypted entries.
const FLAG_ENCRYPTED: u16 = 1 << 0;

/// General-purpose flag bit 11: UTF-8 file names.
const FLAG_UTF8: u16 = 1 << 11;

/// Zip64 sentinel.
const U32_SENTINEL: u32 = 0xFFFF_FFFF;

/// Entry count sentinel (Zip64).
const U16_SENTINEL: u16 = 0xFFFF;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The compression method of a zip entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    /// Stored (no compression).
    Store,
    /// Raw DEFLATE (RFC 1951).
    Deflate,
    /// Any method this wave does not support.
    Unsupported(u16),
}

impl Method {
    fn from_raw(raw: u16) -> Self {
        match raw {
            0 => Self::Store,
            8 => Self::Deflate,
            other => Self::Unsupported(other),
        }
    }
}

/// Central-directory metadata of one entry. Cheap to [`Clone`] — clone out of
/// [`ZipReader::entries`] before calling [`ZipReader::read_entry`].
#[derive(Debug, Clone)]
pub struct ZipEntry {
    name: String,
    size: u64,
    compressed_size: u64,
    method: Method,
    crc: u32,
    offset: u64,
    encrypted: bool,
    is_dir: bool,
}

impl ZipEntry {
    /// The entry's archive path (`/`-separated; directories end with `/`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The uncompressed size.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The compressed size as stored in the archive.
    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    /// The compression method.
    pub fn method(&self) -> Method {
        self.method
    }

    /// The expected CRC-32 of the uncompressed data.
    pub fn crc32(&self) -> u32 {
        self.crc
    }

    /// Whether this is a directory entry (name ends with `/`).
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }
}

/// Errors returned when reading or writing zip archives.
#[derive(Debug)]
#[non_exhaustive]
pub enum ZipError {
    /// No end-of-central-directory record was found.
    NotAZipArchive,
    /// The central directory is inconsistent (bad signature, offsets out of
    /// range, or a count mismatch).
    CorruptCentralDirectory,
    /// The local header at an entry's offset does not match the central
    /// record.
    LocalHeaderMismatch(String),
    /// The entry uses an unsupported compression method.
    UnsupportedMethod(u16),
    /// Zip64 archives are not supported in this wave.
    UnsupportedZip64,
    /// The entry is encrypted; encryption is not supported.
    UnsupportedEncryption,
    /// The entry's CRC-32 did not match after decompression.
    CrcMismatch {
        /// The CRC-32 from the central directory.
        expected: u32,
        /// The CRC-32 computed over the data read.
        computed: u32,
    },
    /// The entry's data length did not match its declared size.
    SizeMismatch {
        /// The declared uncompressed size.
        expected: u64,
        /// The actual length read.
        actual: u64,
    },
    /// The archive ended inside a header or entry.
    Truncated,
    /// The DEFLATE payload was malformed or exceeded its output cap.
    Deflate(DeflateError),
    /// A guarded extraction refused an entry; see
    /// [`archive::error`](crate::archive::error).
    Extract(crate::archive::error::ExtractError),
    /// An I/O error on the underlying reader, writer, or filesystem.
    Io(io::Error),
}

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAZipArchive => f.write_str("no zip end-of-central-directory found"),
            Self::CorruptCentralDirectory => f.write_str("corrupt zip central directory"),
            Self::LocalHeaderMismatch(name) => write!(f, "local header mismatch for {name:?}"),
            Self::UnsupportedMethod(m) => write!(f, "unsupported zip method {m}"),
            Self::UnsupportedZip64 => f.write_str("Zip64 archives are not supported"),
            Self::UnsupportedEncryption => f.write_str("encrypted entries are not supported"),
            Self::CrcMismatch { expected, computed } => write!(
                f,
                "zip CRC-32 mismatch: stored {expected:#010x}, computed {computed:#010x}"
            ),
            Self::SizeMismatch { expected, actual } => {
                write!(f, "zip size mismatch: declared {expected}, actual {actual}")
            }
            Self::Truncated => f.write_str("truncated zip archive"),
            Self::Deflate(e) => write!(f, "deflate error: {e}"),
            Self::Extract(e) => write!(f, "extraction refused: {e}"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for ZipError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Deflate(e) => Some(e),
            Self::Extract(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ZipError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<DeflateError> for ZipError {
    fn from(e: DeflateError) -> Self {
        Self::Deflate(e)
    }
}

impl From<crate::archive::error::ExtractError> for ZipError {
    fn from(e: crate::archive::error::ExtractError) -> Self {
        Self::Extract(e)
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// A reader over a zip archive, parsing the central directory up front.
#[derive(Debug)]
pub struct ZipReader<R: Read + Seek> {
    input: R,
    entries: Vec<ZipEntry>,
}

impl<R: Read + Seek> ZipReader<R> {
    /// Locate the EOCD record by backward scan and parse the central
    /// directory.
    pub fn new(mut input: R) -> Result<Self, ZipError> {
        let len = input.seek(SeekFrom::End(0))?;
        let scan = len.min(22 + 65_535);
        input.seek(SeekFrom::Start(len - scan))?;
        let mut buf = vec![0u8; scan as usize];
        input.read_exact(&mut buf).map_err(truncated_io)?;

        let eocd = find_last(&buf, &SIG_EOCD).ok_or(ZipError::NotAZipArchive)?;
        let (disk, cd_disk, disk_entries, count, cd_size, cd_offset) =
            parse_eocd(&buf[eocd..]).ok_or(ZipError::CorruptCentralDirectory)?;
        if disk != 0 || cd_disk != 0 || disk_entries != count {
            return Err(ZipError::CorruptCentralDirectory);
        }
        if count == U16_SENTINEL || cd_size == U32_SENTINEL || cd_offset == U32_SENTINEL {
            return Err(ZipError::UnsupportedZip64);
        }
        let eocd_pos = (len - scan) + eocd as u64;
        if cd_offset as u64 + cd_size as u64 > eocd_pos {
            return Err(ZipError::CorruptCentralDirectory);
        }

        input.seek(SeekFrom::Start(cd_offset as u64))?;
        let cd = read_bounded(&mut input, cd_size as u64)?;

        let entries = parse_central_directory(&cd, count as usize, cd_offset)?;
        Ok(Self { input, entries })
    }

    /// All entries in central-directory order.
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    /// Look an entry up by its exact archive name.
    pub fn entry_by_name(&self, name: &str) -> Option<&ZipEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Read and verify one entry. `max_output_bytes` bounds the decompressed
    /// size (zip-bomb guard).
    ///
    /// Tip: `reader.entry_by_name("x").cloned()` avoids borrow conflicts with
    /// the `&mut self` receiver.
    pub fn read_entry(
        &mut self,
        entry: &ZipEntry,
        max_output_bytes: u64,
    ) -> Result<Vec<u8>, ZipError> {
        self.read_entry_inner(entry, max_output_bytes)
    }

    /// [`read_entry`](ZipReader::read_entry) by exact name.
    pub fn read_entry_by_name(
        &mut self,
        name: &str,
        max_output_bytes: u64,
    ) -> Result<Vec<u8>, ZipError> {
        let entry = self
            .entry_by_name(name)
            .cloned()
            .ok_or_else(|| ZipError::LocalHeaderMismatch(name.to_string()))?;
        self.read_entry_inner(&entry, max_output_bytes)
    }

    /// Extract all entries under `dest_root` with the family safety rules:
    /// path validation, symlink-escape checks, per-entry and total caps.
    /// Returns the number of entries written.
    pub fn safe_extract_to(
        &mut self,
        dest_root: &Path,
        opts: &ExtractOptions,
    ) -> Result<usize, ZipError> {
        let entries = self.entries.clone();
        let mut written = 0usize;
        let mut total_bytes = 0u64;
        for entry in entries {
            if entry.is_dir() {
                let target = safe_target(dest_root, &entry.name).map_err(ZipError::Extract)?;
                match fs::create_dir(&target) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        let is_dir = fs::symlink_metadata(&target)
                            .map(|md| md.is_dir())
                            .unwrap_or(false);
                        if !is_dir {
                            return Err(ZipError::Extract(
                                crate::archive::error::ExtractError::DestinationExists(target),
                            ));
                        }
                    }
                    Err(e) => return Err(ZipError::Io(e)),
                }
                written += 1;
                continue;
            }
            if entry.size > opts.max_entry_bytes {
                return Err(ZipError::Extract(
                    crate::archive::error::ExtractError::EntryTooLarge {
                        limit: opts.max_entry_bytes,
                        size: entry.size,
                    },
                ));
            }
            if total_bytes.saturating_add(entry.size) > opts.max_total_bytes {
                return Err(ZipError::Extract(
                    crate::archive::error::ExtractError::TotalTooLarge {
                        limit: opts.max_total_bytes,
                        written: total_bytes,
                    },
                ));
            }
            // Validate the entry and its local header before touching the
            // filesystem, then stream the payload to disk in bounded chunks
            // (never buffering the whole entry in memory).
            self.seek_to_data(&entry, opts.max_entry_bytes)?;
            let target = safe_target(dest_root, &entry.name).map_err(ZipError::Extract)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = open_target_file(&target, opts.overwrite_existing)?;
            let result = self
                .stream_entry_data(&entry, &mut file)
                .and_then(|n| file.flush().map(|()| n).map_err(ZipError::Io));
            drop(file);
            match result {
                Ok(n) => total_bytes += n,
                Err(e) => {
                    // Never leave a partial or corrupt file behind. We only
                    // get here after opening (creating or truncating) the
                    // target ourselves, so nothing pre-existing is lost.
                    let _ = fs::remove_file(&target);
                    return Err(e);
                }
            }
            written += 1;
        }
        Ok(written)
    }

    /// Give the underlying reader back.
    pub fn into_inner(self) -> R {
        self.input
    }

    fn read_entry_inner(&mut self, entry: &ZipEntry, cap: u64) -> Result<Vec<u8>, ZipError> {
        self.seek_to_data(entry, cap)?;

        // Grow incrementally: a lying `compressed_size` cannot force a huge
        // up-front allocation, only as much as the input actually holds.
        let data = read_bounded(&mut self.input, entry.compressed_size)?;

        let out = match entry.method {
            Method::Store => data,
            // The declared size is already <= cap; bound inflation by it so a
            // stream longer than declared is cut off early.
            Method::Deflate => deflate::inflate(&data, entry.size)?,
            Method::Unsupported(raw) => return Err(ZipError::UnsupportedMethod(raw)),
        };
        if out.len() as u64 != entry.size {
            return Err(ZipError::SizeMismatch {
                expected: entry.size,
                actual: out.len() as u64,
            });
        }
        let computed = crc32(&out);
        if computed != entry.crc {
            return Err(ZipError::CrcMismatch {
                expected: entry.crc,
                computed,
            });
        }
        Ok(out)
    }

    /// Stream the entry's payload (the reader must already be positioned by
    /// [`seek_to_data`](Self::seek_to_data)) into `out` in bounded chunks,
    /// verifying the declared size and CRC-32 on the fly. Returns the number
    /// of bytes written. On error, `out` may hold a partial payload.
    fn stream_entry_data<W: Write>(
        &mut self,
        entry: &ZipEntry,
        out: &mut W,
    ) -> Result<u64, ZipError> {
        let mut buf = vec![0u8; STREAM_CHUNK];
        let mut crc = Crc32::new();
        let mut total = 0u64;
        let compressed = (&mut self.input).take(entry.compressed_size);
        match entry.method {
            Method::Store => {
                let mut src = compressed;
                loop {
                    let n = match src.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => return Err(ZipError::Io(e)),
                    };
                    crc.update(&buf[..n]);
                    out.write_all(&buf[..n])?;
                    total += n as u64;
                }
                if total != entry.compressed_size {
                    return Err(ZipError::Truncated);
                }
            }
            Method::Deflate => {
                // Inflation is capped at the declared size, so a stream
                // longer than declared trips `OutputLimitExceeded` early.
                let mut dec = deflate::InflateDecoder::new(compressed, entry.size);
                loop {
                    let n = match dec.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => return Err(inflate_io_error(e)),
                    };
                    crc.update(&buf[..n]);
                    out.write_all(&buf[..n])?;
                    total += n as u64;
                }
            }
            Method::Unsupported(raw) => return Err(ZipError::UnsupportedMethod(raw)),
        }
        if total != entry.size {
            return Err(ZipError::SizeMismatch {
                expected: entry.size,
                actual: total,
            });
        }
        let computed = crc.value();
        if computed != entry.crc {
            return Err(ZipError::CrcMismatch {
                expected: entry.crc,
                computed,
            });
        }
        Ok(total)
    }

    /// Validate `entry` against `cap` and its local header, leaving the
    /// reader positioned at the start of the entry's (compressed) data.
    fn seek_to_data(&mut self, entry: &ZipEntry, cap: u64) -> Result<(), ZipError> {
        if entry.encrypted {
            return Err(ZipError::UnsupportedEncryption);
        }
        // Validate declared sizes against the cap *before* allocating or
        // reading anything (zip-bomb / OOM guard).
        if entry.size > cap {
            return Err(ZipError::Deflate(DeflateError::OutputLimitExceeded));
        }
        if let Method::Unsupported(raw) = entry.method {
            return Err(ZipError::UnsupportedMethod(raw));
        }
        if entry.method == Method::Store && entry.compressed_size != entry.size {
            return Err(ZipError::SizeMismatch {
                expected: entry.size,
                actual: entry.compressed_size,
            });
        }
        self.input.seek(SeekFrom::Start(entry.offset))?;
        let mut lfh = [0u8; 30];
        self.input.read_exact(&mut lfh).map_err(truncated_io)?;
        if lfh[0..4] != SIG_LOCAL {
            return Err(ZipError::LocalHeaderMismatch(entry.name.clone()));
        }
        let l_namelen = u16::from_le_bytes([lfh[26], lfh[27]]) as usize;
        let l_extralen = u16::from_le_bytes([lfh[28], lfh[29]]) as usize;
        let mut name_buf = vec![0u8; l_namelen];
        self.input.read_exact(&mut name_buf).map_err(truncated_io)?;
        if name_buf.as_slice() != entry.name.as_bytes() {
            return Err(ZipError::LocalHeaderMismatch(entry.name.clone()));
        }
        // Skip the local extra field to reach the data.
        if l_extralen > 0 {
            self.input.seek(SeekFrom::Current(l_extralen as i64))?;
        }
        Ok(())
    }
}

/// Chunk size for streaming extraction (64 KiB).
const STREAM_CHUNK: usize = 64 * 1024;

/// Open `target` for writing honoring the overwrite flag.
fn open_target_file(target: &Path, overwrite: bool) -> Result<fs::File, ZipError> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true);
    let file = if overwrite {
        opts.create(true).truncate(true).open(target)?
    } else {
        opts.create_new(true).open(target)?
    };
    Ok(file)
}

/// Map an [`InflateDecoder`](deflate::InflateDecoder) I/O error back to
/// the typed [`DeflateError`] it carries, if any.
fn inflate_io_error(e: io::Error) -> ZipError {
    if e.get_ref().is_some_and(|inner| inner.is::<DeflateError>()) {
        if let Some(inner) = e.into_inner() {
            if let Ok(de) = inner.downcast::<DeflateError>() {
                return ZipError::Deflate(*de);
            }
        }
        // Unreachable in practice: the check above guarantees the payload.
        return ZipError::Deflate(DeflateError::InvalidHuffmanCode);
    }
    ZipError::Io(e)
}

/// Initial capacity hint cap for [`read_bounded`] (1 MiB).
const MAX_PREALLOC: u64 = 1 << 20;

/// Read exactly `len` bytes, growing the buffer incrementally instead of
/// trusting `len` for an up-front allocation. Premature EOF is
/// [`ZipError::Truncated`].
fn read_bounded<R: Read>(input: &mut R, len: u64) -> Result<Vec<u8>, ZipError> {
    let mut data = Vec::with_capacity(len.min(MAX_PREALLOC) as usize);
    let n = input.by_ref().take(len).read_to_end(&mut data)?;
    if n as u64 != len {
        return Err(ZipError::Truncated);
    }
    Ok(data)
}

/// Map premature EOF to [`ZipError::Truncated`].
fn truncated_io(e: io::Error) -> ZipError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        ZipError::Truncated
    } else {
        ZipError::Io(e)
    }
}

/// Find the last occurrence of `needle` in `haystack`.
fn find_last(haystack: &[u8], needle: &[u8; 4]) -> Option<usize> {
    if haystack.len() < 4 {
        return None;
    }
    (0..=haystack.len() - 4)
        .rev()
        .find(|&i| &haystack[i..i + 4] == needle)
}

/// Parse an EOCD record; `None` if it is truncated.
fn parse_eocd(b: &[u8]) -> Option<(u16, u16, u16, u16, u32, u32)> {
    if b.len() < 22 {
        return None;
    }
    Some((
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        u16::from_le_bytes([b[8], b[9]]),
        u16::from_le_bytes([b[10], b[11]]),
        u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
    ))
}

/// Parse all central-directory entries.
fn parse_central_directory(
    cd: &[u8],
    count: usize,
    cd_offset: u32,
) -> Result<Vec<ZipEntry>, ZipError> {
    let corrupt = ZipError::CorruptCentralDirectory;
    // Each record is at least 46 bytes; never trust `count` beyond that.
    let mut entries = Vec::with_capacity(count.min(cd.len() / 46));
    let mut pos = 0usize;
    for _ in 0..count {
        let fixed_end = match pos.checked_add(46) {
            Some(end) if end <= cd.len() => end,
            _ => return Err(corrupt),
        };
        if cd[pos..pos + 4] != SIG_CENTRAL {
            return Err(corrupt);
        }
        let flags = u16::from_le_bytes([cd[pos + 8], cd[pos + 9]]);
        let method = u16::from_le_bytes([cd[pos + 10], cd[pos + 11]]);
        let crc = u32::from_le_bytes([cd[pos + 16], cd[pos + 17], cd[pos + 18], cd[pos + 19]]);
        let csize = u32::from_le_bytes([cd[pos + 20], cd[pos + 21], cd[pos + 22], cd[pos + 23]]);
        let size = u32::from_le_bytes([cd[pos + 24], cd[pos + 25], cd[pos + 26], cd[pos + 27]]);
        let namelen = u16::from_le_bytes([cd[pos + 28], cd[pos + 29]]) as usize;
        let extralen = u16::from_le_bytes([cd[pos + 30], cd[pos + 31]]) as usize;
        let commentlen = u16::from_le_bytes([cd[pos + 32], cd[pos + 33]]) as usize;
        let disk = u16::from_le_bytes([cd[pos + 34], cd[pos + 35]]);
        let offset = u32::from_le_bytes([cd[pos + 42], cd[pos + 43], cd[pos + 44], cd[pos + 45]]);
        if csize == U32_SENTINEL || size == U32_SENTINEL || offset == U32_SENTINEL || disk != 0 {
            return Err(ZipError::UnsupportedZip64);
        }
        let name_end = fixed_end + namelen;
        let record_end = name_end + extralen + commentlen;
        if record_end > cd.len() {
            return Err(corrupt);
        }
        let name = String::from_utf8_lossy(&cd[fixed_end..name_end]).into_owned();
        entries.push(ZipEntry {
            is_dir: name.ends_with('/'),
            name,
            size: size as u64,
            compressed_size: csize as u64,
            method: Method::from_raw(method),
            crc,
            offset: offset as u64,
            encrypted: flags & FLAG_ENCRYPTED != 0,
        });
        pos = record_end;
    }
    // The records must exactly cover the central directory and stay in range.
    let _ = cd_offset;
    if pos != cd.len() {
        return Err(corrupt);
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// One central-directory record built while writing.
#[derive(Debug)]
struct CentralRecord {
    name: String,
    method: u16,
    crc: u32,
    size: u32,
    csize: u32,
    offset: u32,
    ext_attr: u32,
}

/// A zip writer producing Store and Deflate entries with a central directory.
///
/// Entry data is compressed in memory, so local headers carry the real
/// sizes (no data descriptors). Unsafe entry names (absolute paths, `..`,
/// NUL bytes) are refused.
#[derive(Debug)]
pub struct ZipWriter<W: Write + Seek> {
    out: W,
    records: Vec<CentralRecord>,
    time: u16,
    date: u16,
}

impl<W: Write + Seek> ZipWriter<W> {
    /// A writer to `out`, stamping entries with the current DOS time.
    pub fn new(out: W) -> Self {
        let (time, date) = dos_datetime_now();
        Self {
            out,
            records: Vec::new(),
            time,
            date,
        }
    }

    /// Append a file entry; `level` picks Store vs Deflate (with automatic
    /// Store fallback when compression does not help).
    pub fn add_file(
        &mut self,
        name: &str,
        data: &[u8],
        level: CompressionLevel,
    ) -> Result<(), ZipError> {
        validate_entry_name(name)?;
        if name.ends_with('/') {
            return Err(ZipError::Extract(
                crate::archive::error::ExtractError::UnsafePath(format!(
                    "file name ends with '/' (use add_dir): {name:?}"
                )),
            ));
        }
        if data.len() as u64 > u32::MAX as u64 {
            return Err(ZipError::UnsupportedZip64);
        }
        let crc = crc32(data);
        let (method, packed) = if level.is_stored() {
            (0u16, data.to_vec())
        } else {
            let packed = deflate::deflate(data, level);
            if packed.len() >= data.len() {
                (0u16, data.to_vec())
            } else {
                (8u16, packed)
            }
        };
        self.write_entry(
            name,
            method,
            crc,
            data.len() as u32,
            &packed,
            0o100_644 << 16,
        )?;
        Ok(())
    }

    /// Append a directory entry (trailing `/` enforced).
    pub fn add_dir(&mut self, name: &str) -> Result<(), ZipError> {
        validate_entry_name(name)?;
        let mut name = name.to_string();
        if !name.ends_with('/') {
            name.push('/');
        }
        self.write_entry(&name, 0, 0, 0, &[], 0o040_755 << 16)
    }

    /// Write the central directory and EOCD, then return the inner writer.
    pub fn finish(mut self) -> Result<W, ZipError> {
        if self.records.len() > U16_SENTINEL as usize {
            return Err(ZipError::UnsupportedZip64);
        }
        let cd_offset = self.out.stream_position()?;
        if cd_offset > u32::MAX as u64 {
            return Err(ZipError::UnsupportedZip64);
        }
        let mut cd_size = 0u64;
        for rec in &self.records {
            let mut header = Vec::with_capacity(46 + rec.name.len());
            header.extend_from_slice(&SIG_CENTRAL);
            header.extend_from_slice(&(((3u16) << 8) | 20u16).to_le_bytes()); // version made by
            header.extend_from_slice(&20u16.to_le_bytes()); // version needed
            header.extend_from_slice(&FLAG_UTF8.to_le_bytes());
            header.extend_from_slice(&rec.method.to_le_bytes());
            header.extend_from_slice(&self.time.to_le_bytes());
            header.extend_from_slice(&self.date.to_le_bytes());
            header.extend_from_slice(&rec.crc.to_le_bytes());
            header.extend_from_slice(&rec.csize.to_le_bytes());
            header.extend_from_slice(&rec.size.to_le_bytes());
            header.extend_from_slice(&(rec.name.len() as u16).to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes()); // extra len
            header.extend_from_slice(&0u16.to_le_bytes()); // comment len
            header.extend_from_slice(&0u16.to_le_bytes()); // disk start
            header.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            header.extend_from_slice(&rec.ext_attr.to_le_bytes());
            header.extend_from_slice(&rec.offset.to_le_bytes());
            header.extend_from_slice(rec.name.as_bytes());
            self.out.write_all(&header)?;
            cd_size += header.len() as u64;
        }
        let mut eocd = Vec::with_capacity(22);
        eocd.extend_from_slice(&SIG_EOCD);
        eocd.extend_from_slice(&0u16.to_le_bytes()); // disk
        eocd.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        eocd.extend_from_slice(&(self.records.len() as u16).to_le_bytes());
        eocd.extend_from_slice(&(self.records.len() as u16).to_le_bytes());
        eocd.extend_from_slice(&(cd_size as u32).to_le_bytes());
        eocd.extend_from_slice(&(cd_offset as u32).to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // comment len
        self.out.write_all(&eocd)?;
        self.out.flush()?;
        Ok(self.out)
    }

    fn write_entry(
        &mut self,
        name: &str,
        method: u16,
        crc: u32,
        size: u32,
        packed: &[u8],
        ext_attr: u32,
    ) -> Result<(), ZipError> {
        let offset = self.out.stream_position()?;
        if offset > u32::MAX as u64 {
            return Err(ZipError::UnsupportedZip64);
        }
        let mut header = Vec::with_capacity(30 + name.len());
        header.extend_from_slice(&SIG_LOCAL);
        header.extend_from_slice(&20u16.to_le_bytes()); // version needed
        header.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        header.extend_from_slice(&method.to_le_bytes());
        header.extend_from_slice(&self.time.to_le_bytes());
        header.extend_from_slice(&self.date.to_le_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        header.extend_from_slice(&size.to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra len
        header.extend_from_slice(name.as_bytes());
        self.out.write_all(&header)?;
        self.out.write_all(packed)?;
        self.records.push(CentralRecord {
            name: name.to_string(),
            method,
            crc,
            size,
            csize: packed.len() as u32,
            offset: offset as u32,
            ext_attr,
        });
        Ok(())
    }
}

/// Writer-side name validation (defense in depth: no absolute paths, no
/// `..`, no NUL, no backslash separators, no trailing `/` for files).
fn validate_entry_name(name: &str) -> Result<(), ZipError> {
    if name.is_empty() || name.ends_with('/') && name.len() == 1 {
        // Root directory entry is pointless but harmless; reject empties.
        return Err(ZipError::Extract(
            crate::archive::error::ExtractError::UnsafePath("empty entry name".into()),
        ));
    }
    crate::archive::safe_components(name).map_err(ZipError::Extract)?;
    if name.contains('\\') {
        // safe_components splits on backslash, but the writer demands `/`.
        return Err(ZipError::Extract(
            crate::archive::error::ExtractError::UnsafePath(format!(
                "backslash separator in {name:?} (use '/')"
            )),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// DOS date/time
// ---------------------------------------------------------------------------

/// Current time as (DOS time, DOS date), clamped into the DOS range.
fn dos_datetime_now() -> (u16, u16) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dos_datetime(secs)
}

/// Unix seconds to (DOS time, DOS date), clamped to 1980-01-01..2107-12-31.
fn dos_datetime(unix_secs: u64) -> (u16, u16) {
    let secs = unix_secs.clamp(315_532_800, 4_354_819_199);
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let date = (((y - 1980) as u16) << 9) | ((m as u16) << 5) | d as u16;
    let hour = (secs_of_day / 3_600) as u16;
    let minute = ((secs_of_day % 3_600) / 60) as u16;
    let second = ((secs_of_day % 60) / 2) as u16;
    ((hour << 11) | (minute << 5) | second, date)
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
