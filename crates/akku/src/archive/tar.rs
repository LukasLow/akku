//! tar archives: POSIX ustar with PAX extended headers (plus GNU longname
//! read compatibility).
//!
//! Pure safe Rust on `std` only. Headers are checksum-verified before any
//! field is trusted, PAX `x`/`g` records and GNU `L`/`K` records are merged
//! transparently, and [`TarReader::safe_extract_to`] enforces the family's
//! zip-slip and symlink-escape rules (see [`archive`](super) for the shared
//! path-safety contract).
//!
//! # Examples
//!
//! ```
//! use akku::archive::tar::{TarReader, TarWriter};
//! use std::io::Read;
//!
//! let mut writer = TarWriter::new(Vec::new());
//! writer
//!     .append_file("notes/hello.txt", 0o644, 1, b"hi tar\n")
//!     .unwrap();
//! let bytes = writer.finish().unwrap();
//!
//! let mut reader = TarReader::new(&bytes[..]);
//! let mut entry = reader.next_entry().unwrap().unwrap();
//! assert_eq!(entry.path(), "notes/hello.txt");
//! let mut data = Vec::new();
//! entry.read_to_end(&mut data).unwrap();
//! assert_eq!(data, b"hi tar\n");
//! ```

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::archive::error::ExtractError;
use crate::archive::{
    resolve_hardlink_source, resolve_symlink_target, safe_components, safe_target, ExtractOptions,
};

/// Size of every tar header block.
pub const BLOCK_SIZE: usize = 512;

/// Largest PAX/GNU metadata payload accepted while reading.
const MAX_META_SIZE: u64 = 1 << 20;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The kind of an archive entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryType {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link (target in `link_name`).
    Symlink,
    /// A hard link (target in `link_name`).
    HardLink,
    /// A FIFO (never created during extraction).
    Fifo,
    /// Any other type flag.
    Other(u8),
}

impl EntryType {
    fn from_flag(flag: u8) -> Self {
        match flag {
            b'0' | 0 | b'7' => Self::File,
            b'5' => Self::Directory,
            b'2' => Self::Symlink,
            b'1' => Self::HardLink,
            b'6' => Self::Fifo,
            other => Self::Other(other),
        }
    }

    fn as_flag(self) -> u8 {
        match self {
            Self::File => b'0',
            Self::Directory => b'5',
            Self::Symlink => b'2',
            Self::HardLink => b'1',
            Self::Fifo => b'6',
            Self::Other(f) => f,
        }
    }
}

/// Errors returned when reading or writing tar archives.
#[derive(Debug)]
#[non_exhaustive]
pub enum TarError {
    /// A header block's checksum did not match.
    BadChecksum {
        /// The checksum stored in the header.
        stored: u32,
        /// The checksum computed over the block.
        computed: u32,
    },
    /// A numeric field was not strict octal (GNU base-256 is rejected).
    BadOctalField {
        /// The name of the offending field.
        field: &'static str,
    },
    /// The archive ended in the middle of a header, entry, or pad.
    Truncated,
    /// A PAX extended header record was malformed.
    InvalidPaxRecord(String),
    /// The operation is unsupported (e.g. oversized metadata payloads).
    Unsupported(&'static str),
    /// A guarded extraction refused an entry; see
    /// [`archive::error`](crate::archive::error) for the reasons.
    Extract(ExtractError),
    /// An I/O error on the underlying reader, writer, or filesystem.
    Io(io::Error),
}

impl fmt::Display for TarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadChecksum { stored, computed } => write!(
                f,
                "tar header checksum mismatch: stored {stored:#o}, computed {computed:#o}"
            ),
            Self::BadOctalField { field } => write!(f, "tar field {field} is not valid octal"),
            Self::Truncated => f.write_str("truncated tar archive"),
            Self::InvalidPaxRecord(reason) => write!(f, "invalid PAX record: {reason}"),
            Self::Unsupported(what) => write!(f, "unsupported tar feature: {what}"),
            Self::Extract(e) => write!(f, "extraction refused: {e}"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for TarError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Extract(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for TarError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ExtractError> for TarError {
    fn from(e: ExtractError) -> Self {
        Self::Extract(e)
    }
}

/// Metadata of the entry currently positioned in the stream.
#[derive(Debug, Clone)]
struct EntryMeta {
    path: String,
    link: Option<String>,
    size: u64,
    mode: u32,
    mtime: u64,
    etype: EntryType,
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Streaming tar reader over any [`Read`].
///
/// PAX and GNU extension records are consumed transparently and merged into
/// the entries they describe. Unread entry data is skipped automatically
/// when the next entry is requested.
#[derive(Debug)]
pub struct TarReader<R: Read> {
    input: R,
    remaining: u64,
    pad_after: u64,
    pax_local: HashMap<String, String>,
    pax_global: HashMap<String, String>,
    gnu_longname: Option<String>,
    gnu_longlink: Option<String>,
}

impl<R: Read> TarReader<R> {
    /// A reader over `input`.
    pub fn new(input: R) -> Self {
        Self {
            input,
            remaining: 0,
            pad_after: 0,
            pax_local: HashMap::new(),
            pax_global: HashMap::new(),
            gnu_longname: None,
            gnu_longlink: None,
        }
    }

    /// The next entry, or `None` at the end of the archive.
    pub fn next_entry(&mut self) -> Result<Option<TarEntry<'_, R>>, TarError> {
        loop {
            self.skip_remaining()?;
            let mut block = [0u8; BLOCK_SIZE];
            if !read_block(&mut self.input, &mut block)? {
                return Ok(None);
            }
            if block.iter().all(|&b| b == 0) {
                // End-of-archive marker (one zero block accepted).
                return Ok(None);
            }
            let stored = parse_octal(&block[148..156], "checksum")?;
            let computed = checksum_of(&block);
            if stored != computed as u64 {
                return Err(TarError::BadChecksum {
                    stored: stored as u32,
                    computed,
                });
            }
            let size = parse_octal(&block[124..136], "size")?;
            let typeflag = block[156];

            match typeflag {
                b'x' | b'g' => {
                    let data = self.read_meta_data(size)?;
                    let map = parse_pax(&data)?;
                    if typeflag == b'x' {
                        self.pax_local = map;
                    } else {
                        self.pax_global = map;
                    }
                    continue;
                }
                b'L' | b'K' => {
                    let data = self.read_meta_data(size)?;
                    let name = String::from_utf8_lossy(&data).into_owned();
                    let name = name.trim_end_matches('\0').to_string();
                    if typeflag == b'L' {
                        self.gnu_longname = Some(name);
                    } else {
                        self.gnu_longlink = Some(name);
                    }
                    continue;
                }
                _ => {}
            }

            // Regular entry: gather ustar fields, then apply overrides.
            let mut path = str_field(&block[0..100]);
            if &block[257..263] == b"ustar\x00" {
                let prefix = str_field(&block[345..500]);
                if !prefix.is_empty() {
                    path = format!("{prefix}/{path}");
                }
            }
            let mut link = str_field(&block[157..257]);
            let mode = (parse_octal(&block[100..108], "mode")? & 0o7777) as u32;
            let mut size = size;
            let mut mtime = parse_octal(&block[136..148], "mtime")?;

            let local = std::mem::take(&mut self.pax_local);
            let lookup = |key: &str| local.get(key).or_else(|| self.pax_global.get(key));
            if let Some(v) = lookup("path") {
                path = v.clone();
            }
            if let Some(v) = lookup("linkpath") {
                link = v.clone();
            }
            if let Some(v) = lookup("size") {
                size = parse_pax_u64(v, "size")?;
            }
            if let Some(v) = lookup("mtime") {
                mtime = parse_pax_u64(v, "mtime")?;
            }
            if let Some(v) = self.gnu_longname.take() {
                path = v;
            }
            if let Some(v) = self.gnu_longlink.take() {
                link = v;
            }

            self.remaining = size;
            self.pad_after = pad_bytes(size);
            let meta = EntryMeta {
                path,
                link: if link.is_empty() { None } else { Some(link) },
                size,
                mode,
                mtime,
                etype: EntryType::from_flag(typeflag),
            };
            return Ok(Some(TarEntry { reader: self, meta }));
        }
    }

    /// Extract all entries under `dest_root` with the family safety rules:
    /// path validation, symlink-escape checks, per-entry and total caps.
    /// Returns the number of entries written (FIFOs and devices skipped).
    pub fn safe_extract_to(
        &mut self,
        dest_root: &Path,
        opts: &ExtractOptions,
    ) -> Result<usize, TarError> {
        let mut written = 0usize;
        let mut total_bytes = 0u64;
        while let Some(entry) = self.next_entry()? {
            written += Self::extract_entry(entry, dest_root, opts, &mut total_bytes)?;
        }
        Ok(written)
    }

    fn extract_entry(
        mut entry: TarEntry<'_, R>,
        dest_root: &Path,
        opts: &ExtractOptions,
        total_bytes: &mut u64,
    ) -> Result<usize, TarError> {
        let path = entry.path().to_string();
        let link = entry.link_name().map(str::to_string);
        let extract = |e: ExtractError| TarError::Extract(e);
        match entry.entry_type() {
            EntryType::File => {
                let size = entry.size();
                if size > opts.max_entry_bytes {
                    return Err(extract(ExtractError::EntryTooLarge {
                        limit: opts.max_entry_bytes,
                        size,
                    }));
                }
                if total_bytes.saturating_add(size) > opts.max_total_bytes {
                    return Err(extract(ExtractError::TotalTooLarge {
                        limit: opts.max_total_bytes,
                        written: *total_bytes,
                    }));
                }
                let target = safe_target(dest_root, &path).map_err(TarError::Extract)?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(TarError::Io)?;
                }
                let mut file = open_target_file(&target, opts.overwrite_existing)?;
                let n = io::copy(&mut entry, &mut file)?;
                if n != size {
                    return Err(TarError::Truncated);
                }
                file.flush()?;
                *total_bytes += n;
                Ok(1)
            }
            EntryType::Directory => {
                let target = safe_target(dest_root, &path).map_err(TarError::Extract)?;
                match fs::create_dir(&target) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        let is_dir = fs::symlink_metadata(&target)
                            .map(|md| md.is_dir())
                            .unwrap_or(false);
                        if !is_dir {
                            return Err(extract(ExtractError::DestinationExists(target)));
                        }
                    }
                    Err(e) => return Err(TarError::Io(e)),
                }
                Ok(1)
            }
            EntryType::Symlink => {
                let target = safe_target(dest_root, &path).map_err(TarError::Extract)?;
                let comps = safe_components(&path).map_err(TarError::Extract)?;
                let link_t = link.clone().ok_or_else(|| {
                    extract(ExtractError::UnsafePath("symlink without target".into()))
                })?;
                // Validate that the target stays inside dest_root (resolved
                // relative to the link's parent directory); the raw
                // (relative) target string is what gets stored.
                resolve_symlink_target(dest_root, &comps, &link_t).map_err(TarError::Extract)?;
                create_symlink(&link_t, &target)?;
                Ok(1)
            }
            EntryType::HardLink => {
                let target = safe_target(dest_root, &path).map_err(TarError::Extract)?;
                let link_t = link.clone().ok_or_else(|| {
                    extract(ExtractError::UnsafePath("hard link without target".into()))
                })?;
                // Hard-link names are archive-root-relative. The source must
                // be a regular file inside dest_root reached without any
                // symlink, so a planted link can never alias a host file.
                let src = resolve_hardlink_source(dest_root, &link_t).map_err(TarError::Extract)?;
                match fs::hard_link(&src, &target) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        if opts.overwrite_existing {
                            let md = fs::symlink_metadata(&target).map_err(TarError::Io)?;
                            if !md.file_type().is_file() {
                                return Err(extract(ExtractError::DestinationExists(target)));
                            }
                            fs::remove_file(&target).map_err(TarError::Io)?;
                            fs::hard_link(&src, &target).map_err(TarError::Io)?;
                        } else {
                            return Err(extract(ExtractError::DestinationExists(target)));
                        }
                    }
                    Err(e) => return Err(TarError::Io(e)),
                }
                Ok(1)
            }
            EntryType::Fifo | EntryType::Other(_) => Ok(0),
        }
    }

    fn skip_remaining(&mut self) -> Result<(), TarError> {
        let mut left = self.remaining.saturating_add(self.pad_after);
        let mut scratch = [0u8; 4096];
        while left > 0 {
            let want = (left.min(scratch.len() as u64)) as usize;
            match self.input.read(&mut scratch[..want]) {
                Ok(0) => return Err(TarError::Truncated),
                Ok(n) => left -= n as u64,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(TarError::Io(e)),
            }
        }
        self.remaining = 0;
        self.pad_after = 0;
        Ok(())
    }

    /// Read `size` bytes of a metadata payload plus its padding.
    fn read_meta_data(&mut self, size: u64) -> Result<Vec<u8>, TarError> {
        if size > MAX_META_SIZE {
            return Err(TarError::Unsupported("metadata payload exceeds 1 MiB"));
        }
        let mut buf = vec![0u8; size as usize];
        read_exact_block(&mut self.input, &mut buf)?;
        self.skip_exact(pad_bytes(size))?;
        Ok(buf)
    }

    fn skip_exact(&mut self, mut n: u64) -> Result<(), TarError> {
        let mut scratch = [0u8; 4096];
        while n > 0 {
            let want = (n.min(scratch.len() as u64)) as usize;
            match self.input.read(&mut scratch[..want]) {
                Ok(0) => return Err(TarError::Truncated),
                Ok(got) => n -= got as u64,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(TarError::Io(e)),
            }
        }
        Ok(())
    }
}

/// An entry positioned in a [`TarReader`]: metadata plus a bounded [`Read`]
/// over exactly `size` bytes of entry data.
///
/// The entry borrows the reader mutably; request the next entry only after
/// this one is dropped (or fully read — leftovers are skipped for you).
pub struct TarEntry<'a, R: Read> {
    reader: &'a mut TarReader<R>,
    meta: EntryMeta,
}

impl<R: Read> TarEntry<'_, R> {
    /// The entry's (PAX/GNU-resolved) path.
    pub fn path(&self) -> &str {
        &self.meta.path
    }

    /// The entry's declared data size.
    pub fn size(&self) -> u64 {
        self.meta.size
    }

    /// Permission bits (0..=0o7777).
    pub fn mode(&self) -> u32 {
        self.meta.mode
    }

    /// Modification time (Unix seconds).
    pub fn mtime(&self) -> u64 {
        self.meta.mtime
    }

    /// The entry's kind.
    pub fn entry_type(&self) -> EntryType {
        self.meta.etype
    }

    /// The symlink/hardlink target, when present.
    pub fn link_name(&self) -> Option<&str> {
        self.meta.link.as_deref()
    }

    /// The validated destination path for this entry under `dest_root`
    /// (zip-slip check only; nothing is written).
    pub fn validate_destination(&self, dest_root: &Path) -> Result<PathBuf, TarError> {
        safe_target(dest_root, &self.meta.path).map_err(TarError::Extract)
    }
}

impl<R: Read> Read for TarEntry<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.reader.remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.reader.remaining) as usize;
        let n = self.reader.input.read(&mut buf[..want])?;
        self.reader.remaining -= n as u64;
        Ok(n)
    }
}

/// Open the destination file honoring the overwrite flag. `safe_target`
/// already guarantees the path never passes through a symlink.
fn open_target_file(target: &Path, overwrite: bool) -> Result<fs::File, TarError> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true);
    let file = if overwrite {
        opts.create(true).truncate(true).open(target)
    } else {
        opts.create_new(true).open(target)
    };
    file.map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            TarError::Extract(ExtractError::DestinationExists(target.to_path_buf()))
        } else {
            TarError::Io(e)
        }
    })
}

/// Create a symlink, mapping an existing destination to
/// [`ExtractError::DestinationExists`].
#[cfg(unix)]
fn create_symlink(target: &str, path: &Path) -> Result<(), TarError> {
    match std::os::unix::fs::symlink(target, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(TarError::Extract(
            ExtractError::DestinationExists(path.to_path_buf()),
        )),
        Err(e) => Err(TarError::Io(e)),
    }
}

/// Symlink extraction requires a Unix platform in this wave.
#[cfg(not(unix))]
fn create_symlink(_target: &str, _path: &Path) -> Result<(), TarError> {
    Err(TarError::Extract(ExtractError::Unsupported(
        "symlink extraction requires a Unix platform",
    )))
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Streaming tar writer over any [`Write`]: POSIX ustar with PAX extended
/// headers for long names/links, big sizes, and big mtimes.
///
/// The writer refuses unsafe entry names (absolute paths, `..` components,
/// NUL bytes) — Akku never writes archives an extractor must reject.
#[derive(Debug)]
pub struct TarWriter<W: Write> {
    out: W,
}

impl<W: Write> TarWriter<W> {
    /// A writer to `out`.
    pub fn new(out: W) -> Self {
        Self { out }
    }

    /// Append a regular file entry with its full contents.
    pub fn append_file(
        &mut self,
        path: &str,
        mode: u32,
        mtime: u64,
        data: &[u8],
    ) -> Result<(), TarError> {
        self.write_entry(path, None, mode, data.len() as u64, mtime, EntryType::File)?;
        self.write_all(data)?;
        self.write_pad(data.len() as u64)?;
        Ok(())
    }

    /// Append a directory entry.
    pub fn append_dir(&mut self, path: &str, mode: u32, mtime: u64) -> Result<(), TarError> {
        self.write_entry(path, None, mode, 0, mtime, EntryType::Directory)
    }

    /// Append a symbolic link entry (`target` may be any relative path).
    pub fn append_symlink(&mut self, path: &str, target: &str, mtime: u64) -> Result<(), TarError> {
        self.write_entry(path, Some(target), 0, 0, mtime, EntryType::Symlink)
    }

    /// Append a hard link entry pointing at an earlier entry's path.
    pub fn append_hardlink(
        &mut self,
        path: &str,
        target: &str,
        mtime: u64,
    ) -> Result<(), TarError> {
        self.write_entry(path, Some(target), 0, 0, mtime, EntryType::HardLink)
    }

    /// Write the two end-of-archive blocks and return the inner writer.
    pub fn finish(mut self) -> Result<W, TarError> {
        let zeros = [0u8; 2 * BLOCK_SIZE];
        self.out.write_all(&zeros)?;
        self.out.flush()?;
        Ok(self.out)
    }

    fn write_all(&mut self, data: &[u8]) -> Result<(), TarError> {
        self.out.write_all(data)?;
        Ok(())
    }

    fn write_pad(&mut self, size: u64) -> Result<(), TarError> {
        let pad = pad_bytes(size);
        if pad > 0 {
            let zeros = vec![0u8; pad as usize];
            self.write_all(&zeros)?;
        }
        Ok(())
    }

    /// Emit any needed PAX record entry, then the entry's own header.
    fn write_entry(
        &mut self,
        path: &str,
        link: Option<&str>,
        mode: u32,
        size: u64,
        mtime: u64,
        etype: EntryType,
    ) -> Result<(), TarError> {
        safe_components(path).map_err(TarError::Extract)?;
        if path.contains('\\') {
            return Err(TarError::Extract(ExtractError::UnsafePath(
                "backslash separator (use '/')".into(),
            )));
        }
        if path.len() > 65_535 {
            return Err(TarError::Extract(ExtractError::UnsafePath(
                "path too long".into(),
            )));
        }
        if let Some(l) = link {
            if l.contains('\0') {
                return Err(TarError::Extract(ExtractError::UnsafePath(
                    "NUL byte in link target".into(),
                )));
            }
            if l.contains('\\') {
                return Err(TarError::Extract(ExtractError::UnsafePath(
                    "backslash separator in link target (use '/')".into(),
                )));
            }
            if l.len() > 65_535 {
                return Err(TarError::Extract(ExtractError::UnsafePath(
                    "link target too long".into(),
                )));
            }
        }

        let mut pax: Vec<(&str, String)> = Vec::new();
        let (mut name, mut prefix) = (path.to_string(), None);
        if path.len() > 100 {
            if let Some((p, base)) = split_ustar(path) {
                prefix = Some(p.to_string());
                name = base.to_string();
            } else {
                pax.push(("path", path.to_string()));
                name = trunc_utf8(path, 100).to_string();
            }
        }
        let mut link_field = link.map(str::to_string);
        if let Some(l) = &link_field {
            if l.len() > 100 {
                pax.push(("linkpath", l.clone()));
                link_field = Some(trunc_utf8(l, 100).to_string());
            }
        }
        let mut size_field = size;
        if size > 0o77_777_777_777 {
            pax.push(("size", size.to_string()));
            size_field = 0;
        }
        let mut mtime_field = mtime;
        if mtime > 0o77_777_777_777 {
            pax.push(("mtime", mtime.to_string()));
            mtime_field = 0;
        }

        if !pax.is_empty() {
            let payload = pax_records_bytes(&pax);
            let pax_name = format!(
                "PaxHeaders.0/{}",
                trunc_utf8(path.rsplit('/').next().unwrap_or(path), 60)
            );
            let header = build_header(
                &pax_name,
                None,
                None,
                0o644,
                payload.len() as u64,
                mtime_field,
                b'x',
            );
            self.write_all(&header)?;
            self.write_all(&payload)?;
            self.write_pad(payload.len() as u64)?;
        }

        let header = build_header(
            &name,
            prefix.as_deref(),
            link_field.as_deref(),
            mode,
            size_field,
            mtime_field,
            etype.as_flag(),
        );
        self.write_all(&header)
    }
}

// ---------------------------------------------------------------------------
// Header helpers
// ---------------------------------------------------------------------------

/// Zero-padding after `size` bytes of entry data.
fn pad_bytes(size: u64) -> u64 {
    (BLOCK_SIZE as u64 - size % BLOCK_SIZE as u64) % BLOCK_SIZE as u64
}

/// Checksum of a header block (checksum field read as spaces).
fn checksum_of(block: &[u8; BLOCK_SIZE]) -> u32 {
    let mut sum = 0u32;
    for (i, &b) in block.iter().enumerate() {
        if (148..156).contains(&i) {
            sum += b' ' as u32;
        } else {
            sum += b as u32;
        }
    }
    sum
}

/// Write `value` as zero-padded octal with `digits` digits plus a NUL.
fn put_octal(dst: &mut [u8], value: u64, digits: usize) {
    let s = format!("{value:0digits$o}");
    dst[..digits].copy_from_slice(s.as_bytes());
    dst[digits] = 0;
}

/// Build a complete 512-byte ustar header with checksum.
fn build_header(
    name: &str,
    prefix: Option<&str>,
    link: Option<&str>,
    mode: u32,
    size: u64,
    mtime: u64,
    typeflag: u8,
) -> [u8; BLOCK_SIZE] {
    let mut h = [0u8; BLOCK_SIZE];
    h[0..name.len()].copy_from_slice(name.as_bytes());
    if let Some(p) = prefix {
        h[345..345 + p.len()].copy_from_slice(p.as_bytes());
    }
    if let Some(l) = link {
        h[157..157 + l.len()].copy_from_slice(l.as_bytes());
    }
    put_octal(&mut h[100..108], (mode & 0o7777) as u64, 7);
    put_octal(&mut h[124..136], size, 11);
    put_octal(&mut h[136..148], mtime, 11);
    h[156] = typeflag;
    h[257..263].copy_from_slice(b"ustar\x00");
    h[263..265].copy_from_slice(b"00");
    let sum = checksum_of(&h);
    let mut ck = [0u8; 8];
    put_octal(&mut ck[..7], sum as u64, 6);
    ck[6] = 0;
    ck[7] = b' ';
    h[148..156].copy_from_slice(&ck);
    h
}

/// Parse a strict octal field; rejects GNU base-256 encodings.
fn parse_octal(field: &[u8], name: &'static str) -> Result<u64, TarError> {
    if !field.is_empty() && field[0] & 0x80 != 0 {
        return Err(TarError::BadOctalField { field: name });
    }
    let mut v: u64 = 0;
    let mut seen = false;
    for &b in field {
        match b {
            b' ' | 0 => {
                if seen {
                    break;
                }
            }
            b'0'..=b'7' => {
                v = v.wrapping_mul(8).wrapping_add((b - b'0') as u64);
                seen = true;
            }
            _ => return Err(TarError::BadOctalField { field: name }),
        }
    }
    Ok(v)
}

/// A NUL-terminated header string, UTF-8 lossy.
fn str_field(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// Read one block; `false` on clean EOF at a block boundary.
fn read_block<R: Read>(input: &mut R, block: &mut [u8; BLOCK_SIZE]) -> Result<bool, TarError> {
    let mut filled = 0usize;
    while filled < block.len() {
        match input.read(&mut block[filled..]) {
            Ok(0) => {
                if filled == 0 {
                    return Ok(false);
                }
                return Err(TarError::Truncated);
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(TarError::Io(e)),
        }
    }
    Ok(true)
}

/// `read_exact` with tar's truncation mapping.
fn read_exact_block<R: Read>(input: &mut R, buf: &mut [u8]) -> Result<(), TarError> {
    input.read_exact(buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            TarError::Truncated
        } else {
            TarError::Io(e)
        }
    })
}

/// Split a path into ustar `prefix`/`name` fields, preferring the longest
/// valid split.
fn split_ustar(path: &str) -> Option<(&str, &str)> {
    let mut best: Option<(&str, &str)> = None;
    for i in 0..path.len() {
        if path.as_bytes()[i] == b'/' {
            let (p, b) = (&path[..i], &path[i + 1..]);
            if !p.is_empty() && p.len() <= 155 && !b.is_empty() && b.len() <= 100 {
                best = Some((p, b));
            }
        }
    }
    best
}

/// Truncate a string to at most `max` bytes on a UTF-8 character boundary.
fn trunc_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Serialize PAX records (`<len> key=value\n`, length includes itself).
fn pax_records_bytes(records: &[(&str, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in records {
        let payload = k.len() + v.len() + 3;
        let mut total = payload + 1;
        loop {
            let s = format!("{total} {k}={v}\n");
            if s.len() == total {
                out.extend_from_slice(s.as_bytes());
                break;
            }
            total += 1;
        }
    }
    out
}

/// Parse PAX records into a key/value map.
fn parse_pax(data: &[u8]) -> Result<HashMap<String, String>, TarError> {
    let invalid = |reason: &str| TarError::InvalidPaxRecord(reason.to_string());
    let mut map = HashMap::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let Some(sp) = data[pos..].iter().position(|&b| b == b' ') else {
            return Err(invalid("missing length separator"));
        };
        let len_bytes = &data[pos..pos + sp];
        // Strict decimal: `usize::from_str` would also accept a leading `+`.
        if len_bytes.is_empty() || !len_bytes.iter().all(u8::is_ascii_digit) {
            return Err(invalid("bad record length"));
        }
        let len: usize = std::str::from_utf8(len_bytes)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| invalid("bad record length"))?;
        let end = match pos.checked_add(len) {
            Some(end) if len > sp + 1 && end <= data.len() => end,
            _ => return Err(invalid("record overruns its payload")),
        };
        let rec = &data[pos + sp + 1..end];
        if rec.last() != Some(&b'\n') {
            return Err(invalid("record does not end with a newline"));
        }
        let kv = &rec[..rec.len() - 1];
        let Some(eq) = kv.iter().position(|&b| b == b'=') else {
            return Err(invalid("record without '=' separator"));
        };
        let key = String::from_utf8_lossy(&kv[..eq]).into_owned();
        let value = String::from_utf8_lossy(&kv[eq + 1..]).into_owned();
        map.insert(key, value);
        pos = end;
    }
    Ok(map)
}

/// Parse a PAX numeric override (integer or fractional seconds).
fn parse_pax_u64(v: &str, field: &str) -> Result<u64, TarError> {
    let int_part = v.split('.').next().unwrap_or(v);
    int_part
        .parse::<u64>()
        .map_err(|_| TarError::InvalidPaxRecord(format!("bad PAX {field} value: {v}")))
}
